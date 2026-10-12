//! C ABI for officewright.
//!
//! Built with [cargo-c](https://github.com/lu-zero/cargo-c) into the native library
//! `officewright` and the header `officewright.h`.
//!
//! Conventions for every function:
//!
//! - Fallible functions return [`OwStatus`]; results go through out-parameters, which
//!   must not be NULL. On failure an out-parameter is set to NULL or 0, and
//!   [`ow_last_error_message`] describes the error.
//! - Strings are NUL-terminated UTF-8.
//! - Handles are created by `ow_<type>_new` (or another constructor) and released by
//!   `ow_<type>_free`, which accepts NULL. A handle may be used by one thread at a time
//!   and may be moved between threads; different handles are independent.
//! - Buffers returned by the library are released with [`ow_bytes_free`].
//! - A Rust panic never crosses the boundary: it becomes [`OwStatus::Panic`].

use std::any::Any;
use std::cell::RefCell;
use std::error::Error as _;
use std::ffi::{CStr, CString, c_char};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;

use officewright_core::Error;
use officewright_pdf::{Document, Font, PageSize, Pt, StandardFont};

/// Version of the C ABI described by `officewright.h`, bumped on every breaking
/// change. Bindings compare it with `ow_abi_version()` when they load the library.
pub const OW_ABI_VERSION: u32 = 1;

/// The result of a fallible `ow_*` function.
///
/// New values may be added in later versions; treat any value other than
/// `OW_STATUS_OK` as a failure.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OwStatus {
    /// The call succeeded.
    Ok = 0,
    /// A pointer argument that must not be NULL was NULL.
    NullArgument = 1,
    /// A string argument is not valid UTF-8.
    InvalidUtf8 = 2,
    /// An argument is out of range or inconsistent with the document.
    InvalidArgument = 3,
    /// Reading or writing a file failed.
    Io = 4,
    /// The input is valid but uses something officewright cannot handle yet, such as a
    /// character the standard fonts cannot show.
    Unsupported = 5,
    /// officewright panicked, which is a bug. Handles passed to the call may only be
    /// freed afterwards.
    Panic = 6,
}

/// A PDF document being built.
///
/// Created by `ow_pdf_document_new` or `ow_pdf_document_from_text` and released by
/// `ow_pdf_document_free`. Used by one thread at a time; may be moved between threads.
pub struct OwPdfDocument(Document);

/// A font to draw and measure text with.
///
/// Created by `ow_pdf_font_new_standard` and released by `ow_pdf_font_free`. A document
/// keeps what it needs from the fonts drawn with it, so a font may be freed before the
/// document. Used by one thread at a time; may be moved between threads.
pub struct OwPdfFont(Font);

/// The standard font Helvetica, with WinAnsiEncoding (Latin text), for
/// `ow_pdf_font_new_standard`. Later versions add more standard fonts; existing values
/// never change.
pub const OW_STANDARD_FONT_HELVETICA: i32 = 0;

/// Returns the C ABI version of the loaded library.
///
/// Callers compiled against `officewright.h` should check that it equals
/// `OW_ABI_VERSION` before calling anything else. Safe to call from any thread.
#[unsafe(no_mangle)]
pub extern "C" fn ow_abi_version() -> u32 {
    // A constant cannot panic, so there is nothing to guard with catch_unwind.
    OW_ABI_VERSION
}

/// Returns a description of the error of the last failed `ow_*` call on this thread.
///
/// The string is UTF-8, never NULL (empty if there is no error) and owned by the
/// library. It stays valid until the next `ow_*` call on the same thread.
#[unsafe(no_mangle)]
pub extern "C" fn ow_last_error_message() -> *const c_char {
    panic::catch_unwind(|| {
        LAST_ERROR
            .try_with(|message| message.try_borrow().map(|message| message.as_ptr()))
            .ok()
            .and_then(Result::ok)
    })
    .ok()
    .flatten()
    .unwrap_or(c"".as_ptr())
}

/// Releases a buffer returned by the library, such as the output of
/// `ow_pdf_document_save_bytes`. Does nothing if `data` is NULL.
///
/// # Safety
///
/// `data` and `len` must be exactly as returned by one call, and the buffer must not be
/// released twice or used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_bytes_free(data: *mut u8, len: usize) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if !data.is_null() {
            // SAFETY: the caller passes a `Box<[u8]>` of `len` bytes that the library
            // leaked with `Box::into_raw`, and passes it only once.
            drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(data, len)) });
        }
    }));
}

/// Creates a PDF document with no pages.
///
/// On success `*out_doc` is a new handle for `ow_pdf_document_free`.
///
/// # Safety
///
/// `out_doc` must be NULL or valid for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_document_new(out_doc: *mut *mut OwPdfDocument) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_doc, "out_doc", ptr::null_mut()) }?;
        let doc = Box::into_raw(Box::new(OwPdfDocument(Document::new())));
        // SAFETY: as above.
        unsafe { put(out_doc, "out_doc", doc) }
    })
}

/// Lays out plain text on pages of `page_width_pt` by `page_height_pt` points, in
/// `font` at `font_size_pt`, and returns the document in `*out_doc`.
///
/// Margins are 72 pt and baselines 1.2 times the font size apart. `\n`, `\r\n` and `\r`
/// end a line, a form feed starts a new page, tabs stop every 8 characters, a leading
/// byte order mark is dropped, and long lines wrap at spaces. Fails with
/// `OW_STATUS_UNSUPPORTED` for characters the font cannot show (Helvetica: anything
/// outside WinAnsiEncoding, such as non-Latin text) and with `OW_STATUS_INVALID_ARGUMENT`
/// for sizes PDF does not allow or a page not larger than its margins.
///
/// # Safety
///
/// `text` must be NULL or a NUL-terminated string; `font` must be NULL or a live handle;
/// `out_doc` must be NULL or valid for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_document_from_text(
    text: *const c_char,
    page_width_pt: f64,
    page_height_pt: f64,
    font: *const OwPdfFont,
    font_size_pt: f64,
    out_doc: *mut *mut OwPdfDocument,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_doc, "out_doc", ptr::null_mut()) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let text = unsafe { str_arg(text, "text") }?;
        // SAFETY: the caller passes NULL or a live handle.
        let font = unsafe { font_ref(font) }?;
        let page_size = PageSize::new(Pt(page_width_pt), Pt(page_height_pt));
        let doc = officewright_pdf::text_to_pdf(text, page_size, font, Pt(font_size_pt))?;
        let doc = Box::into_raw(Box::new(OwPdfDocument(doc)));
        // SAFETY: as above.
        unsafe { put(out_doc, "out_doc", doc) }
    })
}

/// Appends a page of `width_pt` by `height_pt` points and returns its index (counting
/// from 0) in `*out_page_index`. Each side must be between 3 and 14400 pt.
///
/// # Safety
///
/// `doc` must be NULL or a live handle not used by another thread during the call;
/// `out_page_index` must be NULL or valid for writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_document_add_page(
    doc: *mut OwPdfDocument,
    width_pt: f64,
    height_pt: f64,
    out_page_index: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_page_index, "out_page_index", 0) }?;
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let doc = unsafe { doc_mut(doc) }?;
        doc.add_page(PageSize::new(Pt(width_pt), Pt(height_pt)))?;
        let index = doc.page_count() - 1;
        // SAFETY: as above.
        unsafe { put(out_page_index, "out_page_index", index) }
    })
}

/// Draws `text` as a single line in `font` at `font_size_pt` on the page at
/// `page_index`.
///
/// Coordinates are native PDF coordinates in points: the origin is the bottom-left
/// corner of the page, y grows upwards, and (`x_pt`, `y_pt`) is where the baseline of
/// the first character starts. The text is not wrapped or clipped;
/// `ow_pdf_font_text_width` measures it.
///
/// Fails with `OW_STATUS_UNSUPPORTED` if `text` has a character the font cannot show
/// (Helvetica: anything outside WinAnsiEncoding, such as non-Latin text, and control
/// characters such as newline and tab), and with `OW_STATUS_INVALID_ARGUMENT` for a page
/// index out of range, a font size outside 0.01 to 32767 pt, or a position outside
/// -32767 to 32767 pt. On failure the page is unchanged.
///
/// # Safety
///
/// `doc` must be NULL or a live handle not used by another thread during the call;
/// `font` must be NULL or a live handle; `text` must be NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_document_add_text(
    doc: *mut OwPdfDocument,
    page_index: usize,
    x_pt: f64,
    y_pt: f64,
    font: *const OwPdfFont,
    font_size_pt: f64,
    text: *const c_char,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let doc = unsafe { doc_mut(doc) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let font = unsafe { font_ref(font) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let text = unsafe { str_arg(text, "text") }?;
        let page_count = doc.page_count();
        let page = doc.page_mut(page_index).ok_or_else(|| Failure {
            status: OwStatus::InvalidArgument,
            message: format!(
                "page index {page_index} is out of range for a document with {page_count} pages"
            ),
        })?;
        page.add_text(Pt(x_pt), Pt(y_pt), font, Pt(font_size_pt), text)?;
        Ok(())
    })
}

/// Writes the document as a PDF file at `path` (UTF-8), replacing any existing file.
///
/// The file is generated in memory first, so a document error never touches `path`;
/// an I/O error while writing can leave a partial file. Fails with
/// `OW_STATUS_INVALID_ARGUMENT` if the document has no pages and `OW_STATUS_IO` if the
/// file cannot be written.
///
/// # Safety
///
/// `doc` must be NULL or a live handle not modified by another thread during the call;
/// `path` must be NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_document_save_path(
    doc: *const OwPdfDocument,
    path: *const c_char,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle.
        let doc = unsafe { doc_ref(doc) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let path = unsafe { str_arg(path, "path") }?;
        doc.save(path)?;
        Ok(())
    })
}

/// Returns the document as a PDF file in memory: `*out_data` points to `*out_len`
/// bytes, which the caller releases with `ow_bytes_free(*out_data, *out_len)`.
///
/// Fails with `OW_STATUS_INVALID_ARGUMENT` if the document has no pages.
///
/// # Safety
///
/// `doc` must be NULL or a live handle not modified by another thread during the call;
/// `out_data` and `out_len` must each be NULL or valid for writing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_document_save_bytes(
    doc: *const OwPdfDocument,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or pointers valid for writes.
        unsafe { put(out_data, "out_data", ptr::null_mut()) }?;
        // SAFETY: as above.
        unsafe { put(out_len, "out_len", 0) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let doc = unsafe { doc_ref(doc) }?;
        let bytes = doc.to_bytes()?.into_boxed_slice();
        let len = bytes.len();
        let data = Box::into_raw(bytes).cast::<u8>();
        // SAFETY: both pointers were checked above.
        unsafe { put(out_len, "out_len", len) }?;
        // SAFETY: as above.
        unsafe { put(out_data, "out_data", data) }
    })
}

/// Releases a document. Does nothing if `doc` is NULL.
///
/// # Safety
///
/// `doc` must be NULL or a live handle, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_document_free(doc: *mut OwPdfDocument) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if !doc.is_null() {
            // SAFETY: the handle came from `Box::into_raw` and is released only once.
            drop(unsafe { Box::from_raw(doc) });
        }
    }));
}

/// Creates one of the standard fonts that PDF viewers provide without embedding.
///
/// `standard_font` is an `OW_STANDARD_FONT_*` constant; any other value fails with
/// `OW_STATUS_INVALID_ARGUMENT`. On success `*out_font` is a new handle for
/// `ow_pdf_font_free`.
///
/// # Safety
///
/// `out_font` must be NULL or valid for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_font_new_standard(
    standard_font: i32,
    out_font: *mut *mut OwPdfFont,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_font, "out_font", ptr::null_mut()) }?;
        // An integer rather than a C enum: an out-of-range enum value would be undefined
        // behaviour in Rust, an integer is simply checked.
        let standard = match standard_font {
            OW_STANDARD_FONT_HELVETICA => StandardFont::Helvetica,
            other => {
                return Err(Failure {
                    status: OwStatus::InvalidArgument,
                    message: format!(
                        "standard font {other} is not one of the OW_STANDARD_FONT_* values"
                    ),
                });
            }
        };
        let font = Box::into_raw(Box::new(OwPdfFont(Font::standard(standard))));
        // SAFETY: as above.
        unsafe { put(out_font, "out_font", font) }
    })
}

/// Returns in `*out_width_pt` the width of `text` drawn in `font` at `font_size_pt`, as
/// `ow_pdf_document_add_text` draws it (no kerning).
///
/// Fails with `OW_STATUS_UNSUPPORTED` if `text` has a character the font cannot show,
/// and with `OW_STATUS_INVALID_ARGUMENT` for a font size outside 0.01 to 32767 pt.
///
/// # Safety
///
/// `font` must be NULL or a live handle; `text` must be NULL or a NUL-terminated string;
/// `out_width_pt` must be NULL or valid for writing a `double`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_font_text_width(
    font: *const OwPdfFont,
    text: *const c_char,
    font_size_pt: f64,
    out_width_pt: *mut f64,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_width_pt, "out_width_pt", 0.0) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let font = unsafe { font_ref(font) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let text = unsafe { str_arg(text, "text") }?;
        let width = font.text_width(text, Pt(font_size_pt))?;
        // SAFETY: as above.
        unsafe { put(out_width_pt, "out_width_pt", width.0) }
    })
}

/// Releases a font. Does nothing if `font` is NULL.
///
/// # Safety
///
/// `font` must be NULL or a live handle, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_font_free(font: *mut OwPdfFont) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if !font.is_null() {
            // SAFETY: the handle came from `Box::into_raw` and is released only once.
            drop(unsafe { Box::from_raw(font) });
        }
    }));
}

thread_local! {
    /// The message behind `ow_last_error_message`.
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

/// Why a call failed.
struct Failure {
    status: OwStatus,
    message: String,
}

impl Failure {
    fn null(name: &str) -> Self {
        Self {
            status: OwStatus::NullArgument,
            message: format!("{name} is null"),
        }
    }
}

impl From<Error> for Failure {
    fn from(err: Error) -> Self {
        let status = match err {
            Error::Io { .. } => OwStatus::Io,
            Error::InvalidArgument(_) => OwStatus::InvalidArgument,
            Error::Unsupported(_) => OwStatus::Unsupported,
            // A kind of error this version of the ABI has no status for yet.
            _ => OwStatus::Unsupported,
        };
        // The message includes the causes, such as the operating system's I/O error.
        let mut message = err.to_string();
        let mut source = err.source();
        while let Some(cause) = source {
            message = format!("{message}: {cause}");
            source = cause.source();
        }
        Self { status, message }
    }
}

impl From<officewright_pdf::Error> for Failure {
    fn from(err: officewright_pdf::Error) -> Self {
        Error::from(err).into()
    }
}

/// Runs the body of an `ow_*` function: clears the last error, turns a panic into
/// `OwStatus::Panic` and records the message of any failure.
fn guard(body: impl FnOnce() -> Result<(), Failure>) -> OwStatus {
    set_last_error("");
    let failure = match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => return OwStatus::Ok,
        Ok(Err(failure)) => failure,
        Err(payload) => Failure {
            status: OwStatus::Panic,
            message: format!("officewright panicked: {}", panic_message(&*payload)),
        },
    };
    set_last_error(&failure.message);
    failure.status
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else {
        "no message"
    }
}

/// Stores `message` for `ow_last_error_message`. Never panics.
fn set_last_error(message: &str) {
    let message = CString::new(message.replace('\0', "\u{FFFD}")).unwrap_or_default();
    let _ = LAST_ERROR.try_with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            *slot = message;
        }
    });
}

/// Writes `value` through the out-parameter `out`.
///
/// # Safety
///
/// `out` must be NULL or valid for writing a `T`.
unsafe fn put<T>(out: *mut T, name: &str, value: T) -> Result<(), Failure> {
    if out.is_null() {
        return Err(Failure::null(name));
    }
    // SAFETY: not NULL, and the caller guarantees it is valid for writes. `write` does
    // not read or drop the previous, possibly uninitialized, value.
    unsafe { out.write(value) };
    Ok(())
}

/// Borrows a NUL-terminated UTF-8 string argument.
///
/// # Safety
///
/// `ptr` must be NULL or a NUL-terminated string that outlives the call.
unsafe fn str_arg<'a>(ptr: *const c_char, name: &str) -> Result<&'a str, Failure> {
    if ptr.is_null() {
        return Err(Failure::null(name));
    }
    // SAFETY: not NULL, and the caller guarantees a NUL-terminated string.
    let bytes = unsafe { CStr::from_ptr(ptr) };
    bytes.to_str().map_err(|err| Failure {
        status: OwStatus::InvalidUtf8,
        message: format!("{name} is not valid UTF-8 at byte {}", err.valid_up_to()),
    })
}

/// Borrows a document handle.
///
/// # Safety
///
/// `doc` must be NULL or a live handle that is not modified during the borrow.
unsafe fn doc_ref<'a>(doc: *const OwPdfDocument) -> Result<&'a Document, Failure> {
    // SAFETY: the caller guarantees a live handle if not NULL.
    unsafe { doc.as_ref() }
        .map(|doc| &doc.0)
        .ok_or_else(|| Failure::null("doc"))
}

/// Borrows a font handle.
///
/// # Safety
///
/// `font` must be NULL or a live handle.
unsafe fn font_ref<'a>(font: *const OwPdfFont) -> Result<&'a Font, Failure> {
    // SAFETY: the caller guarantees a live handle if not NULL.
    unsafe { font.as_ref() }
        .map(|font| &font.0)
        .ok_or_else(|| Failure::null("font"))
}

/// Borrows a document handle mutably.
///
/// # Safety
///
/// `doc` must be NULL or a live handle that nothing else uses during the borrow.
unsafe fn doc_mut<'a>(doc: *mut OwPdfDocument) -> Result<&'a mut Document, Failure> {
    // SAFETY: the caller guarantees a live, unshared handle if not NULL.
    unsafe { doc.as_mut() }
        .map(|doc| &mut doc.0)
        .ok_or_else(|| Failure::null("doc"))
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::fs;

    use super::*;

    fn last_error() -> String {
        // SAFETY: the library returns a NUL-terminated string valid until the next call.
        unsafe { CStr::from_ptr(ow_last_error_message()) }
            .to_str()
            .unwrap()
            .to_owned()
    }

    fn c(text: &str) -> CString {
        CString::new(text).unwrap()
    }

    /// A Helvetica handle; the caller frees it.
    fn helvetica() -> *mut OwPdfFont {
        let mut font = ptr::null_mut();
        // SAFETY: a valid out-pointer.
        let status = unsafe { ow_pdf_font_new_standard(OW_STANDARD_FONT_HELVETICA, &mut font) };
        assert_eq!(status, OwStatus::Ok);
        font
    }

    /// A one-page A4 document with a line of text. The font is freed before the
    /// document is returned, which the document must survive.
    fn hello() -> *mut OwPdfDocument {
        let mut doc = ptr::null_mut();
        let mut page = usize::MAX;
        let font = helvetica();
        // SAFETY: valid out-pointers, strings and live handles.
        unsafe {
            assert_eq!(ow_pdf_document_new(&mut doc), OwStatus::Ok);
            assert_eq!(
                ow_pdf_document_add_page(doc, 595.0, 842.0, &mut page),
                OwStatus::Ok
            );
            assert_eq!(page, 0);
            let text = c("Grüße (€5)");
            assert_eq!(
                ow_pdf_document_add_text(doc, page, 72.0, 770.0, font, 12.0, text.as_ptr()),
                OwStatus::Ok
            );
            ow_pdf_font_free(font);
        }
        doc
    }

    fn save_bytes(doc: *const OwPdfDocument) -> Vec<u8> {
        let mut data = ptr::null_mut();
        let mut len = 0;
        // SAFETY: a live handle and valid out-pointers; the buffer is freed once.
        unsafe {
            assert_eq!(
                ow_pdf_document_save_bytes(doc, &mut data, &mut len),
                OwStatus::Ok
            );
            let bytes = std::slice::from_raw_parts(data, len).to_vec();
            ow_bytes_free(data, len);
            bytes
        }
    }

    #[test]
    fn abi_version_is_the_header_constant() {
        assert_eq!(ow_abi_version(), OW_ABI_VERSION);
    }

    #[test]
    fn builds_and_saves_a_document() {
        let doc = hello();
        let pdf = save_bytes(doc);
        assert!(pdf.starts_with(b"%PDF-1.4\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let line = b"(Gr\\374\\337e \\(\\2005\\)) Tj";
        assert!(pdf.windows(line.len()).any(|w| w == line));
        assert_eq!(last_error(), "");
        // SAFETY: a live handle, freed once.
        unsafe { ow_pdf_document_free(doc) };
    }

    #[test]
    fn saves_the_same_bytes_to_a_path() {
        let doc = hello();
        let dir = std::env::temp_dir().join(format!("officewright-ffi-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello.pdf");
        let c_path = c(path.to_str().unwrap());
        // SAFETY: a live handle and a NUL-terminated path.
        let status = unsafe { ow_pdf_document_save_path(doc, c_path.as_ptr()) };
        assert_eq!(status, OwStatus::Ok);
        assert_eq!(fs::read(&path).unwrap(), save_bytes(doc));
        fs::remove_dir_all(&dir).unwrap();
        // SAFETY: a live handle, freed once.
        unsafe { ow_pdf_document_free(doc) };
    }

    #[test]
    fn lays_out_text_and_measures_it() {
        let font = helvetica();
        let mut doc = ptr::null_mut();
        let mut width = 0.0;
        let text = c("Hello\nworld");
        // SAFETY: valid strings, out-pointers and handles.
        unsafe {
            let status =
                ow_pdf_document_from_text(text.as_ptr(), 612.0, 792.0, font, 11.0, &mut doc);
            assert_eq!(status, OwStatus::Ok);
            assert!(save_bytes(doc).starts_with(b"%PDF-1.4"));
            ow_pdf_document_free(doc);
            assert_eq!(
                ow_pdf_font_text_width(font, c("H").as_ptr(), 10.0, &mut width),
                OwStatus::Ok
            );
            ow_pdf_font_free(font);
        }
        assert_eq!(width, 7.22);
    }

    /// A non-NULL placeholder for an out-parameter, to see that a failing call resets it.
    fn placeholder<T>() -> *mut T {
        ptr::NonNull::dangling().as_ptr()
    }

    #[test]
    fn rejects_unknown_standard_fonts() {
        let mut font: *mut OwPdfFont = placeholder();
        // SAFETY: a valid out-pointer.
        let status = unsafe { ow_pdf_font_new_standard(7, &mut font) };
        assert_eq!(status, OwStatus::InvalidArgument);
        assert!(font.is_null());
        assert_eq!(
            last_error(),
            "standard font 7 is not one of the OW_STANDARD_FONT_* values"
        );
    }

    #[test]
    fn reports_null_arguments() {
        let font = helvetica();
        let mut doc: *mut OwPdfDocument = placeholder();
        let mut index = 7;
        let mut data: *mut u8 = placeholder();
        let mut len = 7;
        let mut width = -1.0;
        let text = c("x");
        // SAFETY: every non-null pointer is valid; NULL is what is being tested.
        unsafe {
            assert_eq!(ow_pdf_document_new(ptr::null_mut()), OwStatus::NullArgument);
            assert_eq!(last_error(), "out_doc is null");
            assert_eq!(
                ow_pdf_font_new_standard(OW_STANDARD_FONT_HELVETICA, ptr::null_mut()),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "out_font is null");
            assert_eq!(
                ow_pdf_document_from_text(ptr::null(), 612.0, 792.0, font, 11.0, &mut doc),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "text is null");
            assert!(doc.is_null());
            doc = placeholder();
            assert_eq!(
                ow_pdf_document_from_text(text.as_ptr(), 612.0, 792.0, ptr::null(), 11.0, &mut doc),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "font is null");
            assert!(doc.is_null());
            assert_eq!(
                ow_pdf_document_add_page(ptr::null_mut(), 612.0, 792.0, &mut index),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "doc is null");
            assert_eq!(index, 0);
            assert_eq!(
                ow_pdf_document_add_text(ptr::null_mut(), 0, 0.0, 0.0, font, 1.0, text.as_ptr()),
                OwStatus::NullArgument
            );
            assert_eq!(
                ow_pdf_document_save_path(ptr::null(), text.as_ptr()),
                OwStatus::NullArgument
            );
            assert_eq!(
                ow_pdf_document_save_bytes(ptr::null(), &mut data, &mut len),
                OwStatus::NullArgument
            );
            assert!(data.is_null());
            assert_eq!(len, 0);
            assert_eq!(
                ow_pdf_font_text_width(font, text.as_ptr(), 10.0, ptr::null_mut()),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "out_width_pt is null");
            assert_eq!(
                ow_pdf_font_text_width(ptr::null(), text.as_ptr(), 10.0, &mut width),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "font is null");
            assert_eq!(width, 0.0);
            assert_eq!(
                ow_pdf_font_text_width(font, ptr::null(), 10.0, &mut width),
                OwStatus::NullArgument
            );
            ow_pdf_document_free(ptr::null_mut());
            ow_pdf_font_free(ptr::null_mut());
            ow_bytes_free(ptr::null_mut(), 0);
            ow_pdf_font_free(font);
        }
    }

    #[test]
    fn reports_invalid_utf8() {
        let doc = hello();
        let font = helvetica();
        let bad = b"ab\xFFc\0";
        // SAFETY: live handles and a NUL-terminated (invalid UTF-8) string.
        let status =
            unsafe { ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 10.0, bad.as_ptr().cast()) };
        assert_eq!(status, OwStatus::InvalidUtf8);
        assert_eq!(last_error(), "text is not valid UTF-8 at byte 2");
        // SAFETY: live handles, freed once.
        unsafe {
            ow_pdf_font_free(font);
            ow_pdf_document_free(doc);
        }
    }

    #[test]
    fn reports_unsupported_characters() {
        let doc = hello();
        let font = helvetica();
        let text = c("Жар");
        // SAFETY: live handles and a NUL-terminated string.
        let status =
            unsafe { ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 10.0, text.as_ptr()) };
        assert_eq!(status, OwStatus::Unsupported);
        assert_eq!(
            last_error(),
            "character U+0416 'Ж' at byte 0 is not in WinAnsiEncoding"
        );
        // SAFETY: live handles, freed once.
        unsafe {
            ow_pdf_font_free(font);
            ow_pdf_document_free(doc);
        }
    }

    #[test]
    fn reports_invalid_arguments() {
        let doc = hello();
        let font = helvetica();
        let mut index = 7;
        let text = c("x");
        // SAFETY: live handles, valid strings and out-pointers.
        unsafe {
            assert_eq!(
                ow_pdf_document_add_text(doc, 1, 0.0, 0.0, font, 10.0, text.as_ptr()),
                OwStatus::InvalidArgument
            );
            assert_eq!(
                last_error(),
                "page index 1 is out of range for a document with 1 pages"
            );
            assert_eq!(
                ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, f64::NAN, text.as_ptr()),
                OwStatus::InvalidArgument
            );
            assert_eq!(
                ow_pdf_document_add_page(doc, 1.0, 1.0, &mut index),
                OwStatus::InvalidArgument
            );
            assert_eq!(index, 0);
            assert_eq!(
                last_error(),
                "page size 1 x 1 pt is outside the 3 to 14400 pt range PDF allows"
            );
            ow_pdf_document_free(doc);
            ow_pdf_font_free(font);

            let mut empty = ptr::null_mut();
            let mut data: *mut u8 = placeholder();
            let mut len = 7;
            assert_eq!(ow_pdf_document_new(&mut empty), OwStatus::Ok);
            assert_eq!(
                ow_pdf_document_save_bytes(empty, &mut data, &mut len),
                OwStatus::InvalidArgument
            );
            assert_eq!(last_error(), "document has no pages");
            assert!(data.is_null());
            assert_eq!(len, 0);
            ow_pdf_document_free(empty);
        }
    }

    #[test]
    fn reports_io_errors_with_the_path_and_cause() {
        let doc = hello();
        let path = std::env::temp_dir().join("officewright-missing-dir/out.pdf");
        let c_path = c(path.to_str().unwrap());
        // SAFETY: a live handle and a NUL-terminated path.
        let status = unsafe { ow_pdf_document_save_path(doc, c_path.as_ptr()) };
        assert_eq!(status, OwStatus::Io);
        let message = last_error();
        assert!(
            message.starts_with(&format!("cannot write {}: ", path.display())),
            "{message}"
        );
        // SAFETY: a live handle, freed once.
        unsafe { ow_pdf_document_free(doc) };
    }

    #[test]
    fn a_successful_call_clears_the_last_error() {
        // SAFETY: NULL is what is being tested, then a valid out-pointer.
        unsafe {
            assert_eq!(ow_pdf_document_new(ptr::null_mut()), OwStatus::NullArgument);
            assert_ne!(last_error(), "");
            let mut doc = ptr::null_mut();
            assert_eq!(ow_pdf_document_new(&mut doc), OwStatus::Ok);
            assert_eq!(last_error(), "");
            ow_pdf_document_free(doc);
        }
    }

    #[test]
    fn turns_a_panic_into_a_status() {
        let status = guard(|| panic!("boom"));
        assert_eq!(status, OwStatus::Panic);
        assert_eq!(last_error(), "officewright panicked: boom");
    }

    #[test]
    fn keeps_messages_with_nul_bytes() {
        set_last_error("a\0b");
        assert_eq!(last_error(), "a\u{FFFD}b");
    }
}
