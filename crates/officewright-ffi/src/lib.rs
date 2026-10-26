//! C ABI for officewright.
//!
//! Built with [cargo-c](https://github.com/lu-zero/cargo-c) into the native library
//! `officewright` and the header `officewright.h`.
//!
//! Conventions for every function:
//!
//! - Fallible functions return [`OwStatus`]; results go through out-parameters, which
//!   must not be NULL, except a report out-parameter that a function says may be. On
//!   failure an out-parameter is set to NULL or 0, and [`ow_last_error_message`]
//!   describes the error.
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

use std::sync::Arc;

use officewright_core::Error;
use officewright_doc::{LayoutReport, RenderOptions, ReportKind};
use officewright_font::{FontBook, FontOrigin, FontRequest, FontResolver, ResolutionKind};
use officewright_pdf::{Compression, Document, EmbeddingNote, Font, PageSize, Pt, StandardFont};

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
    /// The input is damaged, or is not what it was given as: data that is not a font, a
    /// font whose tables cannot be read.
    Malformed = 7,
    /// The input exceeds one of the limits that keep the time and memory a file can take
    /// bounded.
    LimitExceeded = 8,
}

/// A PDF document being built.
///
/// Created by `ow_pdf_document_new` or `ow_pdf_document_from_text` and released by
/// `ow_pdf_document_free`. Used by one thread at a time; may be moved between threads.
pub struct OwPdfDocument(Document);

/// A font to draw and measure text with.
///
/// Created by `ow_pdf_font_new_standard`, `ow_pdf_font_open_path`,
/// `ow_pdf_font_open_bytes` or `ow_pdf_font_resolve` and released by `ow_pdf_font_free`. A document keeps what it
/// needs from the fonts drawn with it, so a font may be freed before the document. Used
/// by one thread at a time; may be moved between threads.
pub struct OwPdfFont {
    font: Font,
    /// The messages of the font's embedding notes, for `ow_pdf_font_note`.
    notes: Vec<(EmbeddingNote, CString)>,
}

impl OwPdfFont {
    fn new(font: Font) -> Self {
        let notes = font
            .embedding_notes()
            .iter()
            .map(|&note| {
                let message = CString::new(note.to_string()).unwrap_or_default();
                (note, message)
            })
            .collect();
        Self { font, notes }
    }
}

/// The standard font Helvetica, with WinAnsiEncoding (Latin text), for
/// `ow_pdf_font_new_standard`. Later versions add more standard fonts; existing values
/// never change.
pub const OW_STANDARD_FONT_HELVETICA: i32 = 0;

/// An embedding note this version of the header does not know, from `ow_pdf_font_note`;
/// its message says what it is. Also the kind a failed `ow_pdf_font_note` returns.
pub const OW_FONT_NOTE_OTHER: i32 = 0;

/// An embedding note, from `ow_pdf_font_note`: the font's license lets it be used only
/// to view and print the document ("Preview & Print" embedding).
pub const OW_FONT_NOTE_PREVIEW_AND_PRINT: i32 = 1;

/// An embedding note: the font's license lets it be used to view, print and edit the
/// document, but not installed ("Editable" embedding).
pub const OW_FONT_NOTE_EDITABLE: i32 = 2;

/// An embedding note: the font's license forbids subsetting, so the whole font is
/// embedded and the file is larger.
pub const OW_FONT_NOTE_WHOLE_FONT: i32 = 3;

/// An embedding note: the font is a variable font; only its default instance is
/// embedded. Later versions add notes with new values; existing values never change.
pub const OW_FONT_NOTE_DEFAULT_INSTANCE: i32 = 4;

/// The fonts that can be chosen by family name: fonts the caller adds and the fonts
/// installed on the system.
///
/// Created by `ow_font_book_new` and released by `ow_font_book_free`. Adding fonts reads
/// only their names and style; a font file is read again when a font is chosen from it.
/// Used by one thread at a time; may be moved between threads.
pub struct OwFontBook {
    book: FontBook,
    /// The messages of the skipped fonts, for `ow_font_book_skipped`.
    skipped: Vec<CString>,
}

impl OwFontBook {
    /// Renews the messages if the book's list of skipped entries changed. An unchanged
    /// list keeps its strings, which `ow_font_book_skipped` promises callers; a call that
    /// fails usually changes nothing.
    fn refresh(&mut self) {
        let skipped: Vec<CString> = self
            .book
            .skipped()
            .iter()
            .map(|skipped| c_string(skipped.to_string()))
            .collect();
        if skipped != self.skipped {
            self.skipped = skipped;
        }
    }
}

/// Chooses fonts by family name from a font book, the way Word names fonts, and keeps
/// the report of substitutions.
///
/// Created by `ow_font_resolver_new` and released by `ow_font_resolver_free`. Used by
/// one thread at a time; may be moved between threads.
pub struct OwFontResolver {
    resolver: FontResolver<'static>,
    /// The kinds and messages of the substitutions, for `ow_font_resolver_substitution`.
    substitutions: Vec<(i32, CString)>,
}

/// The kind of a substitution, from `ow_font_resolver_substitution`, that this version
/// of the header does not know; its message says what it is. Also the kind a failed
/// `ow_font_resolver_substitution` returns. Later versions add kinds with new values;
/// existing values never change.
pub const OW_RESOLUTION_OTHER: i32 = 0;

/// A face has the requested family name; the substitution is a synthetic style, a bold
/// or italic the face lacks, or a bold face for regular text when the family has no
/// regular face installed.
pub const OW_RESOLUTION_EXACT: i32 = 1;

/// Windows maps the requested name to another family, such as "Times New Roman CYR" to
/// Times New Roman; reported only with a synthetic style or a bold face for regular text.
pub const OW_RESOLUTION_ALIAS: i32 = 2;

/// No face has the requested family name, but one has it as its typographic family,
/// full name or PostScript name.
pub const OW_RESOLUTION_OTHER_NAME: i32 = 3;

/// The requested name is a named instance of a variable font; its default instance is
/// used.
pub const OW_RESOLUTION_VARIABLE_INSTANCE: i32 = 4;

/// A font with the same advance widths replaces the requested family, such as Carlito
/// for Calibri, so text lays out the same.
pub const OW_RESOLUTION_METRIC_COMPATIBLE: i32 = 5;

/// A font of similar design, such as Caladea for Cambria, or a common font of the same
/// kind replaces the requested family, and text lays out differently.
pub const OW_RESOLUTION_FALLBACK: i32 = 6;

/// No font could be found at all.
pub const OW_RESOLUTION_MISSING: i32 = 7;

/// For `ow_pdf_document_set_compression` and `ow_render_options_set_compression`: page
/// content and embedded fonts are stored as they are, so the page content can be read in
/// a text editor.
pub const OW_COMPRESSION_NONE: i32 = 0;

/// For `ow_pdf_document_set_compression` and `ow_render_options_set_compression`: page
/// content and embedded fonts are compressed with the Flate filter wherever that makes
/// them smaller. Every PDF reader decompresses Flate, so this is the default.
pub const OW_COMPRESSION_FLATE: i32 = 1;

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
/// byte order mark is dropped, and long lines wrap at spaces, measured with the font's
/// advance widths. Fails with `OW_STATUS_UNSUPPORTED` for characters the font cannot
/// show (as `ow_pdf_document_add_text` describes) and with `OW_STATUS_INVALID_ARGUMENT`
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
/// Each character is drawn with its own glyph, without kerning or shaping. Fails with
/// `OW_STATUS_UNSUPPORTED` if `text` has a character the font cannot show: for Helvetica
/// anything outside WinAnsiEncoding, such as non-Latin text; for an embedded font a
/// character it has no glyph for, or one of a script that needs shaping or right-to-left
/// layout (see `ow_pdf_font_open_path`); for both, control characters such as newline
/// and tab. Fails with `OW_STATUS_INVALID_ARGUMENT` for a page index out of range, a font
/// size outside 0.01 to 32767 pt, or a position outside -32767 to 32767 pt. On failure
/// the page is unchanged.
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

/// Sets how the document stores page content and embedded fonts when it is saved:
/// `OW_COMPRESSION_FLATE`, the default, or `OW_COMPRESSION_NONE`. Fails with
/// `OW_STATUS_INVALID_ARGUMENT` for any other value, leaving the document unchanged.
///
/// # Safety
///
/// `doc` must be NULL or a live handle not used by another thread during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_document_set_compression(
    doc: *mut OwPdfDocument,
    compression: i32,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let doc = unsafe { doc_mut(doc) }?;
        doc.set_compression(compression_arg(compression)?);
        Ok(())
    })
}

/// Sets how much reducing the document's embedded fonts to the glyphs it shows may read
/// of their CFF charstrings in all, in operands, operators and hint mask bytes:
/// 33,554,432 unless set otherwise, room for two CJK fonts the size of Noto Sans CJK SC
/// kept whole. The fonts are reduced when the document is saved, one after another in the
/// order the pages first use them, and every subset is kept until the file is: a unit
/// costs about 30 bytes of memory while its font is reduced and about 5 bytes after.
/// Fonts with TrueType outlines take nothing from the budget. Saving fails with
/// `OW_STATUS_LIMIT_EXCEEDED` at the first font past what is left.
///
/// # Safety
///
/// `doc` must be NULL or a live handle not used by another thread during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_document_set_subset_budget(
    doc: *mut OwPdfDocument,
    units: u64,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let doc = unsafe { doc_mut(doc) }?;
        doc.set_subset_budget(units);
        Ok(())
    })
}

/// Writes the document as a PDF file at `path` (UTF-8), replacing any existing file.
///
/// The file is generated in memory first, so a document error never touches `path`;
/// an I/O error while writing can leave a partial file. Fails with
/// `OW_STATUS_INVALID_ARGUMENT` if the document has no pages, `OW_STATUS_MALFORMED` if an
/// embedded font turns out to be damaged when it is reduced to the glyphs the document
/// uses, `OW_STATUS_LIMIT_EXCEEDED` if decompiling the glyphs the document uses of a CFF
/// font reads more than 16,777,216 operands, operators and hint mask bytes, a limit of
/// the library, or more than is left of the document's subset budget
/// (`ow_pdf_document_set_subset_budget`), `OW_STATUS_UNSUPPORTED` if it uses a feature
/// that cannot be subset yet, and `OW_STATUS_IO` if the file cannot be written.
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
/// Fails with `OW_STATUS_INVALID_ARGUMENT` if the document has no pages,
/// `OW_STATUS_MALFORMED` if an embedded font turns out to be damaged when it is reduced
/// to the glyphs the document uses, `OW_STATUS_LIMIT_EXCEEDED` if decompiling the glyphs
/// the document uses of a CFF font reads more than 16,777,216 operands, operators and
/// hint mask bytes, or more than is left of the document's subset budget
/// (`ow_pdf_document_set_subset_budget`), and `OW_STATUS_UNSUPPORTED` if it uses a
/// feature that cannot be subset yet.
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
        let font = Box::into_raw(Box::new(OwPdfFont::new(Font::standard(standard))));
        // SAFETY: as above.
        unsafe { put(out_font, "out_font", font) }
    })
}

/// Opens face `face_index` of a TrueType or OpenType font file or collection at `path`
/// (UTF-8) for embedding; use 0 for a file with one font. The file is read once; it may
/// change or go away afterwards.
///
/// The documents drawn with it embed the font reduced to the glyphs they use, unless
/// its license forbids subsetting (`OW_FONT_NOTE_WHOLE_FONT`). It shows the Latin,
/// Greek, Cyrillic, Armenian, Georgian, Han, Hiragana and Katakana scripts, precomposed
/// Hangul syllables, and the punctuation, digits and symbols every script shares, one
/// glyph per character; text in scripts that need shaping or right-to-left layout is
/// refused. Restrictions in the font's license that do not stop embedding are listed by
/// `ow_pdf_font_note_count` and `ow_pdf_font_note`.
///
/// Fails with `OW_STATUS_IO` if the file cannot be read, `OW_STATUS_MALFORMED` if it is
/// not a font or is damaged, `OW_STATUS_LIMIT_EXCEEDED` if it is past a limit of the
/// library (`name` records that point at more than 1 MiB of strings, CFF Font DICTs that
/// point at more than 1 MiB of Private DICTs or have more than 262,144 local subroutines
/// in all, an FDSelect of more than 2,048 ranges, or a CFF `.notdef` glyph that takes more
/// than 65,536 operands, operators and hint mask bytes to decompile),
/// `OW_STATUS_INVALID_ARGUMENT` if it has no such face, and `OW_STATUS_UNSUPPORTED` if its
/// license forbids embedding or its outlines cannot be embedded (CFF2, or CID-keyed CFF
/// with more than 255 Font DICTs). On success
/// `*out_font` is a new handle for `ow_pdf_font_free`.
///
/// # Safety
///
/// `path` must be NULL or a NUL-terminated string; `out_font` must be NULL or valid for
/// writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_font_open_path(
    path: *const c_char,
    face_index: u32,
    out_font: *mut *mut OwPdfFont,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_font, "out_font", ptr::null_mut()) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let path = unsafe { str_arg(path, "path") }?;
        let font = Font::from_file(path, face_index)?;
        let font = Box::into_raw(Box::new(OwPdfFont::new(font)));
        // SAFETY: as above.
        unsafe { put(out_font, "out_font", font) }
    })
}

/// Opens face `face_index` of a TrueType or OpenType font or font collection in the
/// `len` bytes at `data`, as `ow_pdf_font_open_path` opens a file. The bytes are copied;
/// the caller keeps its buffer.
///
/// Fails as `ow_pdf_font_open_path`, apart from reading the file.
///
/// # Safety
///
/// `data` must be NULL or valid for reading `len` bytes; `out_font` must be NULL or valid
/// for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_font_open_bytes(
    data: *const u8,
    len: usize,
    face_index: u32,
    out_font: *mut *mut OwPdfFont,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_font, "out_font", ptr::null_mut()) }?;
        if data.is_null() {
            return Err(Failure::null("data"));
        }
        // SAFETY: not NULL, and the caller guarantees `len` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        let font = Font::from_bytes(bytes, face_index)?;
        let font = Box::into_raw(Box::new(OwPdfFont::new(font)));
        // SAFETY: as above.
        unsafe { put(out_font, "out_font", font) }
    })
}

/// Returns in `*out_count` the number of embedding notes of `font`: what its user should
/// know about the font's license and format, such as a license that forbids subsetting.
/// None of them stops the font from being used. A standard font has none.
///
/// # Safety
///
/// `font` must be NULL or a live handle; `out_count` must be NULL or valid for writing a
/// `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_font_note_count(
    font: *const OwPdfFont,
    out_count: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_count, "out_count", 0) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let font = unsafe { font_handle(font) }?;
        // SAFETY: as above.
        unsafe { put(out_count, "out_count", font.notes.len()) }
    })
}

/// Returns embedding note `index` (counting from 0) of `font`: its kind, an
/// `OW_FONT_NOTE_*` value, in `*out_kind`, and a sentence that describes it in
/// `*out_message`, UTF-8 and owned by the font: it stays valid until the font is freed.
///
/// Fails with `OW_STATUS_INVALID_ARGUMENT` if `index` is not below the count
/// `ow_pdf_font_note_count` gives.
///
/// # Safety
///
/// `font` must be NULL or a live handle; `out_kind` and `out_message` must each be NULL
/// or valid for writing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_font_note(
    font: *const OwPdfFont,
    index: usize,
    out_kind: *mut i32,
    out_message: *mut *const c_char,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or pointers valid for writes.
        unsafe { put(out_kind, "out_kind", OW_FONT_NOTE_OTHER) }?;
        // SAFETY: as above.
        unsafe { put(out_message, "out_message", ptr::null()) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let font = unsafe { font_handle(font) }?;
        let (note, message) = font.notes.get(index).ok_or_else(|| Failure {
            status: OwStatus::InvalidArgument,
            message: format!(
                "note index {index} is out of range for a font with {} notes",
                font.notes.len()
            ),
        })?;
        let kind = match note {
            EmbeddingNote::PreviewAndPrint => OW_FONT_NOTE_PREVIEW_AND_PRINT,
            EmbeddingNote::Editable => OW_FONT_NOTE_EDITABLE,
            EmbeddingNote::WholeFont => OW_FONT_NOTE_WHOLE_FONT,
            EmbeddingNote::DefaultInstance => OW_FONT_NOTE_DEFAULT_INSTANCE,
            _ => OW_FONT_NOTE_OTHER,
        };
        // SAFETY: both pointers were checked above.
        unsafe { put(out_kind, "out_kind", kind) }?;
        // SAFETY: as above. The message lives as long as the handle.
        unsafe { put(out_message, "out_message", message.as_ptr()) }
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

/// Creates an empty font book. On success `*out_book` is a new handle for
/// `ow_font_book_free`.
///
/// # Safety
///
/// `out_book` must be NULL or valid for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_new(out_book: *mut *mut OwFontBook) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_book, "out_book", ptr::null_mut()) }?;
        let book = OwFontBook {
            book: FontBook::new(),
            skipped: Vec::new(),
        };
        let book = Box::into_raw(Box::new(book));
        // SAFETY: as above.
        unsafe { put(out_book, "out_book", book) }
    })
}

/// Adds the fonts installed on the system and returns in `*out_added` the number of
/// font faces added. Called again, it adds the fonts installed since. Folders that do
/// not exist are left out; files that are not fonts are skipped and listed by
/// `ow_font_book_skipped`.
///
/// # Safety
///
/// `book` must be NULL or a live handle not used by another thread during the call;
/// `out_added` must be NULL or valid for writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_add_system_fonts(
    book: *mut OwFontBook,
    out_added: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_added, "out_added", 0) }?;
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let book = unsafe { book_mut(book) }?;
        let added = book.book.add_system_fonts();
        book.refresh();
        // SAFETY: as above.
        unsafe { put(out_added, "out_added", added) }
    })
}

/// Adds the fonts in the folder at `path` (UTF-8) and its subfolders, and returns in
/// `*out_added` the number of font faces added. Files that are not fonts are skipped and
/// listed by `ow_font_book_skipped`.
///
/// Fails with `OW_STATUS_IO` if the folder cannot be read.
///
/// # Safety
///
/// `book` must be NULL or a live handle not used by another thread during the call;
/// `path` must be NULL or a NUL-terminated string; `out_added` must be NULL or valid for
/// writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_add_dir(
    book: *mut OwFontBook,
    path: *const c_char,
    out_added: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_added, "out_added", 0) }?;
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let book = unsafe { book_mut(book) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let path = unsafe { str_arg(path, "path") }?;
        let added = book.book.add_dir(path);
        book.refresh();
        // SAFETY: as above.
        unsafe { put(out_added, "out_added", added?) }
    })
}

/// Adds the font faces of the font file or collection at `path` (UTF-8), and returns in
/// `*out_added` the number of faces added.
///
/// Fails with `OW_STATUS_IO` if the file cannot be read and `OW_STATUS_MALFORMED` if it
/// is not a font or every face in it is damaged.
///
/// # Safety
///
/// `book` must be NULL or a live handle not used by another thread during the call;
/// `path` must be NULL or a NUL-terminated string; `out_added` must be NULL or valid for
/// writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_add_file(
    book: *mut OwFontBook,
    path: *const c_char,
    out_added: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_added, "out_added", 0) }?;
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let book = unsafe { book_mut(book) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let path = unsafe { str_arg(path, "path") }?;
        let added = book.book.add_file(path);
        book.refresh();
        // SAFETY: as above.
        unsafe { put(out_added, "out_added", added?) }
    })
}

/// Adds the font faces of the font or font collection in the `len` bytes at `data`,
/// and returns in `*out_added` the number of faces added. The bytes are copied; the
/// caller keeps its buffer.
///
/// Fails with `OW_STATUS_MALFORMED` if the data is not a font or every face in it is
/// damaged.
///
/// # Safety
///
/// `book` must be NULL or a live handle not used by another thread during the call;
/// `data` must be NULL or valid for reading `len` bytes; `out_added` must be NULL or
/// valid for writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_add_bytes(
    book: *mut OwFontBook,
    data: *const u8,
    len: usize,
    out_added: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_added, "out_added", 0) }?;
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let book = unsafe { book_mut(book) }?;
        if data.is_null() {
            return Err(Failure::null("data"));
        }
        // SAFETY: not NULL, and the caller guarantees `len` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        let added = book.book.add_bytes(bytes, FontOrigin::User);
        book.refresh();
        // SAFETY: as above.
        unsafe { put(out_added, "out_added", added?) }
    })
}

/// Adds the font faces in the `len` bytes at `data` as `ow_font_book_add_bytes` does, and
/// the faces added also answer to the family name `family` (UTF-8, NUL-terminated), as one
/// of their own: `ow_pdf_font_resolve` finds them for a request of that family. Their own
/// names stay, and their own tables give their style. For a font a document embeds under a
/// name it does not carry itself, such as a subset Word embedded and renamed
/// `___WRD_EMBED_SUB_` and a number. No face gets the name where one of the faces added has
/// it as a family name or names a named instance by it; else, of the faces added (a
/// collection has several), those that answer to it by a PostScript, typographic or full
/// name get it, if any, and all others otherwise. A `family` that is empty or only white
/// space, or longer than 512 bytes, adds no name.
///
/// Fails with `OW_STATUS_MALFORMED` if the data is not a font or every face in it is
/// damaged, and with `OW_STATUS_INVALID_UTF8` if `family` is not UTF-8.
///
/// # Safety
///
/// `book` must be NULL or a live handle not used by another thread during the call;
/// `data` must be NULL or valid for reading `len` bytes; `family` must be NULL or a
/// NUL-terminated string; `out_added` must be NULL or valid for writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_add_bytes_as(
    book: *mut OwFontBook,
    data: *const u8,
    len: usize,
    family: *const c_char,
    out_added: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_added, "out_added", 0) }?;
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let book = unsafe { book_mut(book) }?;
        if data.is_null() {
            return Err(Failure::null("data"));
        }
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let family = unsafe { str_arg(family, "family") }?;
        // SAFETY: not NULL, and the caller guarantees `len` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        let added = book.book.add_bytes_as(bytes, FontOrigin::User, family);
        book.refresh();
        // SAFETY: as above.
        unsafe { put(out_added, "out_added", added?) }
    })
}

/// Adds the font faces of the file at `path` (UTF-8, NUL-terminated) as
/// `ow_font_book_add_file` does, and the faces added also answer to the family name
/// `family`, as `ow_font_book_add_bytes_as` has them. A file the book has read already
/// adds no faces, and so no name.
///
/// Fails with `OW_STATUS_IO` if the file cannot be read, `OW_STATUS_MALFORMED` if it is
/// not a font or every face in it is damaged, and `OW_STATUS_INVALID_UTF8` if `path` or
/// `family` is not UTF-8.
///
/// # Safety
///
/// `book` must be NULL or a live handle not used by another thread during the call;
/// `path` and `family` must be NULL or NUL-terminated strings; `out_added` must be NULL or
/// valid for writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_add_file_as(
    book: *mut OwFontBook,
    path: *const c_char,
    family: *const c_char,
    out_added: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_added, "out_added", 0) }?;
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let book = unsafe { book_mut(book) }?;
        // SAFETY: the caller passes NULL or NUL-terminated strings.
        let path = unsafe { str_arg(path, "path") }?;
        // SAFETY: as above.
        let family = unsafe { str_arg(family, "family") }?;
        let added = book.book.add_file_as(path, family);
        book.refresh();
        // SAFETY: as above.
        unsafe { put(out_added, "out_added", added?) }
    })
}

/// Returns in `*out_count` the number of font faces in the book.
///
/// # Safety
///
/// `book` must be NULL or a live handle not modified by another thread during the call;
/// `out_count` must be NULL or valid for writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_face_count(
    book: *const OwFontBook,
    out_count: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_count, "out_count", 0) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let book = unsafe { book_ref(book) }?;
        // SAFETY: as above.
        unsafe { put(out_count, "out_count", book.book.len()) }
    })
}

/// Returns in `*out_count` the number of folders, files and font faces the book could
/// not read: why a font that should be there was not found.
///
/// # Safety
///
/// `book` must be NULL or a live handle not modified by another thread during the call;
/// `out_count` must be NULL or valid for writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_skipped_count(
    book: *const OwFontBook,
    out_count: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_count, "out_count", 0) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let book = unsafe { book_ref(book) }?;
        // SAFETY: as above.
        unsafe { put(out_count, "out_count", book.skipped.len()) }
    })
}

/// Returns in `*out_message` what skipped entry `index` (counting from 0) is and why it
/// was skipped, such as `fonts/broken.ttf: data is not a TrueType or OpenType font or
/// font collection: the file is too short`. The string is UTF-8 and owned by the book:
/// it stays valid until an `ow_font_book_add_*` call changes the list of skipped entries
/// (one that skips another file, or reads a skipped one after all) or the book is freed.
/// A call that changes nothing, such as a failed one, leaves the strings valid.
///
/// Fails with `OW_STATUS_INVALID_ARGUMENT` if `index` is not below the count
/// `ow_font_book_skipped_count` gives.
///
/// # Safety
///
/// `book` must be NULL or a live handle not modified by another thread during the call;
/// `out_message` must be NULL or valid for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_skipped(
    book: *const OwFontBook,
    index: usize,
    out_message: *mut *const c_char,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_message, "out_message", ptr::null()) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let book = unsafe { book_ref(book) }?;
        let message = book.skipped.get(index).ok_or_else(|| Failure {
            status: OwStatus::InvalidArgument,
            message: format!(
                "skipped index {index} is out of range for a font book with {} skipped",
                book.skipped.len()
            ),
        })?;
        // SAFETY: as above. The message lives until the book changes.
        unsafe { put(out_message, "out_message", message.as_ptr()) }
    })
}

/// Releases a font book. Does nothing if `book` is NULL. Resolvers made from it keep
/// their own copy, so they may outlive it.
///
/// # Safety
///
/// `book` must be NULL or a live handle, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_book_free(book: *mut OwFontBook) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if !book.is_null() {
            // SAFETY: the handle came from `Box::into_raw` and is released only once.
            drop(unsafe { Box::from_raw(book) });
        }
    }));
}

/// Creates a resolver that chooses fonts from the fonts `book` has now, with an empty
/// substitution report. The resolver keeps its own copy of the book's index: fonts added
/// to the book later are not seen by it, and the book may be freed first. On success
/// `*out_resolver` is a new handle for `ow_font_resolver_free`.
///
/// # Safety
///
/// `book` must be NULL or a live handle not modified by another thread during the call;
/// `out_resolver` must be NULL or valid for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_resolver_new(
    book: *const OwFontBook,
    out_resolver: *mut *mut OwFontResolver,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_resolver, "out_resolver", ptr::null_mut()) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let book = unsafe { book_ref(book) }?;
        let resolver = OwFontResolver {
            resolver: FontResolver::from_shared(Arc::new(book.book.clone())),
            substitutions: Vec::new(),
        };
        let resolver = Box::into_raw(Box::new(resolver));
        // SAFETY: as above.
        unsafe { put(out_resolver, "out_resolver", resolver) }
    })
}

/// Chooses a font by family name, the way Word names fonts (without regard to case),
/// and returns it in `*out_font`, a new handle for `ow_pdf_font_free`, to embed like a
/// font from `ow_pdf_font_open_path`.
///
/// `bold` and `italic` are 0 or 1. When the family is not in the book, the resolver
/// takes its metric-compatible substitute (Carlito for Calibri, Liberation for Arial,
/// Times New Roman and Courier New), else a font of similar design (Caladea for
/// Cambria, which lays out differently) or a common font of the same kind; a style the
/// face lacks is synthetic, italic before taking a face of another weight, as Word does,
/// and a family with no regular face installed gives regular text its bold one. Every
/// such substitution is added to the resolver's report (`ow_font_resolver_substitution`).
/// `ow_pdf_document_add_text` draws the synthetic styles on top of the face: italic
/// slanted by a third about the baseline, bold filled and stroked with a stroke a
/// thirtieth of the size wide.
///
/// Fails with `OW_STATUS_UNSUPPORTED` if no font could be found, or the chosen font's
/// license forbids embedding or its outlines cannot be embedded (CFF2, or CID-keyed CFF
/// with more than 255 Font DICTs), `OW_STATUS_INVALID_ARGUMENT` if `bold` or `italic`
/// is not 0 or 1, `OW_STATUS_MALFORMED` if the chosen font is damaged,
/// `OW_STATUS_LIMIT_EXCEEDED` if it is past a limit of the library, and `OW_STATUS_IO`
/// if its file cannot be read any more.
///
/// # Safety
///
/// `resolver` must be NULL or a live handle not used by another thread during the call;
/// `family` must be NULL or a NUL-terminated string; `out_font` must be NULL or valid for
/// writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_pdf_font_resolve(
    resolver: *mut OwFontResolver,
    family: *const c_char,
    bold: i32,
    italic: i32,
    out_font: *mut *mut OwPdfFont,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_font, "out_font", ptr::null_mut()) }?;
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let resolver = unsafe { resolver.as_mut() }.ok_or_else(|| Failure::null("resolver"))?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let family = unsafe { str_arg(family, "family") }?;
        let request = FontRequest::new(family)
            .bold(flag(bold, "bold")?)
            .italic(flag(italic, "italic")?);
        let font = Font::resolve(&mut resolver.resolver, &request);
        // The report grows even when the font cannot be used.
        let known = resolver.substitutions.len();
        for substitution in resolver.resolver.substitutions().iter().skip(known) {
            let kind = match substitution.kind() {
                ResolutionKind::Exact => OW_RESOLUTION_EXACT,
                ResolutionKind::Alias => OW_RESOLUTION_ALIAS,
                ResolutionKind::OtherName => OW_RESOLUTION_OTHER_NAME,
                ResolutionKind::VariableInstance => OW_RESOLUTION_VARIABLE_INSTANCE,
                ResolutionKind::MetricCompatible => OW_RESOLUTION_METRIC_COMPATIBLE,
                ResolutionKind::Fallback => OW_RESOLUTION_FALLBACK,
                ResolutionKind::Missing => OW_RESOLUTION_MISSING,
                _ => OW_RESOLUTION_OTHER,
            };
            resolver
                .substitutions
                .push((kind, c_string(substitution.to_string())));
        }
        let font = Box::into_raw(Box::new(OwPdfFont::new(font?)));
        // SAFETY: as above.
        unsafe { put(out_font, "out_font", font) }
    })
}

/// Returns in `*out_count` the number of substitutions the resolver has reported: each
/// family it could not give as requested, with what it took instead, each outcome once.
///
/// # Safety
///
/// `resolver` must be NULL or a live handle not modified by another thread during the
/// call; `out_count` must be NULL or valid for writing a `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_resolver_substitution_count(
    resolver: *const OwFontResolver,
    out_count: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_count, "out_count", 0) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let resolver = unsafe { resolver.as_ref() }.ok_or_else(|| Failure::null("resolver"))?;
        // SAFETY: as above.
        unsafe { put(out_count, "out_count", resolver.substitutions.len()) }
    })
}

/// Returns substitution `index` (counting from 0, in the order the families were first
/// resolved): its kind, an `OW_RESOLUTION_*` value, in `*out_kind`, and a line for a
/// report in `*out_message`, such as `"Calibri" → "Carlito" (metric-compatible
/// substitute)`. The string is UTF-8 and owned by the resolver: it stays valid until the
/// resolver is freed.
///
/// Fails with `OW_STATUS_INVALID_ARGUMENT` if `index` is not below the count
/// `ow_font_resolver_substitution_count` gives.
///
/// # Safety
///
/// `resolver` must be NULL or a live handle not modified by another thread during the
/// call; `out_kind` and `out_message` must each be NULL or valid for writing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_resolver_substitution(
    resolver: *const OwFontResolver,
    index: usize,
    out_kind: *mut i32,
    out_message: *mut *const c_char,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or pointers valid for writes.
        unsafe { put(out_kind, "out_kind", OW_RESOLUTION_OTHER) }?;
        // SAFETY: as above.
        unsafe { put(out_message, "out_message", ptr::null()) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let resolver = unsafe { resolver.as_ref() }.ok_or_else(|| Failure::null("resolver"))?;
        let (kind, message) = resolver.substitutions.get(index).ok_or_else(|| Failure {
            status: OwStatus::InvalidArgument,
            message: format!(
                "substitution index {index} is out of range for a resolver with {} \
                 substitutions",
                resolver.substitutions.len()
            ),
        })?;
        // SAFETY: both pointers were checked above.
        unsafe { put(out_kind, "out_kind", *kind) }?;
        // SAFETY: as above. The message lives as long as the resolver.
        unsafe { put(out_message, "out_message", message.as_ptr()) }
    })
}

/// Releases a resolver. Does nothing if `resolver` is NULL. Fonts it chose stay valid.
///
/// # Safety
///
/// `resolver` must be NULL or a live handle, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_font_resolver_free(resolver: *mut OwFontResolver) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if !resolver.is_null() {
            // SAFETY: the handle came from `Box::into_raw` and is released only once.
            drop(unsafe { Box::from_raw(resolver) });
        }
    }));
}

thread_local! {
    /// The message behind `ow_last_error_message`.
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

/// A Word document (DOCX) opened for conversion.
///
/// Created by `ow_doc_document_open_path` or `ow_doc_document_open_bytes` and released by
/// `ow_doc_document_free`. Converting it does not change it, so it may be converted
/// several times, with different options. Used by one thread at a time; may be moved
/// between threads.
pub struct OwDocDocument(officewright_doc::Document);

/// How a document is rendered into PDF: the most pages it may take, the fonts to use
/// besides those the document embeds, and how the PDF's streams are compressed.
///
/// Created by `ow_render_options_new` with the defaults (at most 100,000 pages, the
/// system's fonts, Flate compression) and released by `ow_render_options_free`. It is
/// read, never changed, by a conversion, so one may serve many conversions, one after
/// another. Used by one thread at a time; may be moved between threads.
pub struct OwRenderOptions(RenderOptions);

/// What a conversion did otherwise than Word would: fonts it substituted, glyphs it
/// could not draw, features it does not lay out yet, values it brought within limits.
///
/// Returned by `ow_doc_document_save_pdf_path` and `ow_doc_document_save_pdf_bytes`,
/// read with `ow_layout_report_len` and `ow_layout_report_entry`, and released by
/// `ow_layout_report_free`. Used by one thread at a time; may be moved between threads.
pub struct OwLayoutReport {
    /// Each entry's kind, an `OW_REPORT_KIND_*` value, its place and its message.
    entries: Vec<(i32, CString, CString)>,
}

impl OwLayoutReport {
    fn new(report: LayoutReport) -> Self {
        let entries = report
            .into_iter()
            .map(|entry| {
                let kind = match entry.kind() {
                    ReportKind::FontSubstituted => OW_REPORT_KIND_FONT_SUBSTITUTED,
                    ReportKind::GlyphMissing => OW_REPORT_KIND_GLYPH_MISSING,
                    ReportKind::FeatureUnsupported => OW_REPORT_KIND_FEATURE_UNSUPPORTED,
                    ReportKind::FieldCached => OW_REPORT_KIND_FIELD_CACHED,
                    ReportKind::CompatIgnored => OW_REPORT_KIND_COMPAT_IGNORED,
                    ReportKind::ValueClamped => OW_REPORT_KIND_VALUE_CLAMPED,
                    ReportKind::Overflow => OW_REPORT_KIND_OVERFLOW,
                    ReportKind::FontEmbedding => OW_REPORT_KIND_FONT_EMBEDDING,
                    _ => OW_REPORT_KIND_OTHER,
                };
                (
                    kind,
                    c_string(entry.location().to_string()),
                    c_string(entry.message().to_owned()),
                )
            })
            .collect();
        Self { entries }
    }
}

/// A report entry of a kind this version of the header does not know, from
/// `ow_layout_report_entry`; its message says what it is. Also the kind a failed
/// `ow_layout_report_entry` returns.
pub const OW_REPORT_KIND_OTHER: i32 = 0;
/// A report entry: a font is not available and another one is used in its place.
pub const OW_REPORT_KIND_FONT_SUBSTITUTED: i32 = 1;
/// A report entry: no available font has a glyph for a character; the font's
/// missing-glyph shape is drawn.
pub const OW_REPORT_KIND_GLYPH_MISSING: i32 = 2;
/// A report entry: something the document uses is not laid out yet, or not the way Word
/// does.
pub const OW_REPORT_KIND_FEATURE_UNSUPPORTED: i32 = 3;
/// A report entry: a field whose value is not computed shows the result stored in the
/// document.
pub const OW_REPORT_KIND_FIELD_CACHED: i32 = 4;
/// A report entry: a legacy compatibility option is set that the layout does not follow.
pub const OW_REPORT_KIND_COMPAT_IGNORED: i32 = 5;
/// A report entry: a value beyond what Word accepts, or what a PDF holds, was brought
/// within its limits.
pub const OW_REPORT_KIND_VALUE_CLAMPED: i32 = 6;
/// A report entry: content does not fit its area, such as a word wider than its column.
pub const OW_REPORT_KIND_OVERFLOW: i32 = 7;
/// A report entry: a font is embedded with what its license or format restricts, or it
/// cannot be embedded and its text is not drawn. Later versions add kinds with new
/// values; existing values never change.
pub const OW_REPORT_KIND_FONT_EMBEDDING: i32 = 8;

/// Opens the Word document (DOCX) in the file at `path` (UTF-8). On success `*out_doc`
/// is a new handle for `ow_doc_document_free`.
///
/// Fails with `OW_STATUS_IO` if the file cannot be read, `OW_STATUS_MALFORMED` if it is
/// not a WordprocessingML package or its structure is broken, `OW_STATUS_UNSUPPORTED`
/// for an encrypted or binary (DOC) Word document, and `OW_STATUS_LIMIT_EXCEEDED` if it
/// exceeds the limits that keep the time and memory a file can take bounded.
///
/// # Safety
///
/// `path` must be NULL or a NUL-terminated string; `out_doc` must be NULL or valid for
/// writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_doc_document_open_path(
    path: *const c_char,
    out_doc: *mut *mut OwDocDocument,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_doc, "out_doc", ptr::null_mut()) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let path = unsafe { str_arg(path, "path") }?;
        let doc = officewright_doc::Document::open(path)?;
        let doc = Box::into_raw(Box::new(OwDocDocument(doc)));
        // SAFETY: as above.
        unsafe { put(out_doc, "out_doc", doc) }
    })
}

/// Opens the Word document (DOCX) in the `len` bytes at `data`, which the library
/// copies. On success `*out_doc` is a new handle for `ow_doc_document_free`.
///
/// Fails as `ow_doc_document_open_path` does, except with `OW_STATUS_IO`.
///
/// # Safety
///
/// `data` must be NULL or valid for reading `len` bytes; `out_doc` must be NULL or valid
/// for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_doc_document_open_bytes(
    data: *const u8,
    len: usize,
    out_doc: *mut *mut OwDocDocument,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_doc, "out_doc", ptr::null_mut()) }?;
        // SAFETY: the caller passes NULL or `len` readable bytes.
        let bytes = unsafe { bytes_arg(data, len, "data") }?;
        let doc = officewright_doc::Document::from_bytes(bytes.to_vec())?;
        let doc = Box::into_raw(Box::new(OwDocDocument(doc)));
        // SAFETY: as above.
        unsafe { put(out_doc, "out_doc", doc) }
    })
}

/// Renders `doc` into PDF with `options` and writes it to the file at `path` (UTF-8),
/// which is replaced if it exists. When `out_report` is not NULL, `*out_report` is a new
/// report of what the PDF shows otherwise than Word would, for `ow_layout_report_free`;
/// NULL if the call fails.
///
/// Fails with `OW_STATUS_IO` if the file cannot be written or a font folder of the
/// options cannot be read, `OW_STATUS_MALFORMED` if a font the PDF embeds is damaged
/// where only embedding looks (a glyph's outline, say), and `OW_STATUS_LIMIT_EXCEEDED`
/// if the document takes more pages than the options allow, nests tables deeper than
/// the layout follows, or uses glyphs of a CFF font that take more than 16,777,216
/// operands, operators and hint mask bytes to decompile, or of all its CFF fonts together
/// more than 33,554,432 (the default of `ow_pdf_document_set_subset_budget`), limits of
/// the library.
/// `OW_STATUS_INVALID_ARGUMENT` here would mean that officewright
/// translated the document wrongly, a defect worth reporting with the document. A font
/// that `ow_pdf_font_open_bytes` would refuse is no error here: its text is not drawn,
/// and the report says so (`OW_REPORT_KIND_FONT_EMBEDDING`).
///
/// # Safety
///
/// `doc` and `options` must each be NULL or a live handle not modified by another thread
/// during the call; `path` must be NULL or a NUL-terminated string; `out_report` must be
/// NULL or valid for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_doc_document_save_pdf_path(
    doc: *const OwDocDocument,
    path: *const c_char,
    options: *const OwRenderOptions,
    out_report: *mut *mut OwLayoutReport,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put_optional(out_report, ptr::null_mut()) };
        // SAFETY: the caller passes NULL or a live handle.
        let doc = unsafe { doc_document_ref(doc) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let path = unsafe { str_arg(path, "path") }?;
        // SAFETY: the caller passes NULL or a live handle.
        let options = unsafe { options_ref(options) }?;
        let report = doc.save_pdf_path(path, options)?;
        let report = Box::into_raw(Box::new(OwLayoutReport::new(report)));
        // SAFETY: as above; the report is freed here if nobody takes it.
        unsafe { give_optional(out_report, report) };
        Ok(())
    })
}

/// Renders `doc` into PDF with `options` in memory: `*out_data` points to `*out_len`
/// bytes, which the caller releases with `ow_bytes_free(*out_data, *out_len)`. When
/// `out_report` is not NULL, `*out_report` is a new report as for
/// `ow_doc_document_save_pdf_path`; NULL if the call fails.
///
/// Fails as `ow_doc_document_save_pdf_path` does, except for writing the file.
///
/// # Safety
///
/// `doc` and `options` must each be NULL or a live handle not modified by another thread
/// during the call; `out_data`, `out_len` and `out_report` must each be NULL or valid for
/// writing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_doc_document_save_pdf_bytes(
    doc: *const OwDocDocument,
    options: *const OwRenderOptions,
    out_data: *mut *mut u8,
    out_len: *mut usize,
    out_report: *mut *mut OwLayoutReport,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or pointers valid for writes.
        unsafe { put_optional(out_report, ptr::null_mut()) };
        // SAFETY: as above.
        unsafe { put(out_data, "out_data", ptr::null_mut()) }?;
        // SAFETY: as above.
        unsafe { put(out_len, "out_len", 0) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let doc = unsafe { doc_document_ref(doc) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let options = unsafe { options_ref(options) }?;
        let (bytes, report) = doc.save_pdf_bytes(options)?;
        let bytes = bytes.into_boxed_slice();
        let len = bytes.len();
        let data = Box::into_raw(bytes).cast::<u8>();
        // SAFETY: both pointers were checked above.
        unsafe { put(out_len, "out_len", len) }?;
        // SAFETY: as above.
        unsafe { put(out_data, "out_data", data) }?;
        let report = Box::into_raw(Box::new(OwLayoutReport::new(report)));
        // SAFETY: as above; the report is freed here if nobody takes it.
        unsafe { give_optional(out_report, report) };
        Ok(())
    })
}

/// Releases a document. Does nothing if `doc` is NULL.
///
/// # Safety
///
/// `doc` must be NULL or a live handle, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_doc_document_free(doc: *mut OwDocDocument) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if !doc.is_null() {
            // SAFETY: the handle came from `Box::into_raw` and is released only once.
            drop(unsafe { Box::from_raw(doc) });
        }
    }));
}

/// Creates render options with the defaults: at most 100,000 pages, the fonts installed
/// on the system, Flate compression. On success `*out_options` is a new handle for
/// `ow_render_options_free`.
///
/// # Safety
///
/// `out_options` must be NULL or valid for writing a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_render_options_new(out_options: *mut *mut OwRenderOptions) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_options, "out_options", ptr::null_mut()) }?;
        let options = Box::into_raw(Box::new(OwRenderOptions(RenderOptions::new())));
        // SAFETY: as above.
        unsafe { put(out_options, "out_options", options) }
    })
}

/// Sets the most pages a document may take; past it, a conversion fails with
/// `OW_STATUS_LIMIT_EXCEEDED`. Fails with `OW_STATUS_INVALID_ARGUMENT` for 0.
///
/// # Safety
///
/// `options` must be NULL or a live handle not used by another thread during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_render_options_set_max_pages(
    options: *mut OwRenderOptions,
    max_pages: usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let options = unsafe { options_mut(options) }?;
        if max_pages == 0 {
            return Err(Failure {
                status: OwStatus::InvalidArgument,
                message: "max_pages is 0; a document takes at least one page".to_owned(),
            });
        }
        options.0 = std::mem::take(&mut options.0).with_max_pages(max_pages);
        Ok(())
    })
}

/// Sets whether the fonts installed on the system are used, after the folders and fonts
/// added to the options: 1 (the default) or 0. Any other value fails with
/// `OW_STATUS_INVALID_ARGUMENT`.
///
/// # Safety
///
/// `options` must be NULL or a live handle not used by another thread during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_render_options_set_system_fonts(
    options: *mut OwRenderOptions,
    system_fonts: i32,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let options = unsafe { options_mut(options) }?;
        let system_fonts = flag(system_fonts, "system_fonts")?;
        options.0 = std::mem::take(&mut options.0).with_system_fonts(system_fonts);
        Ok(())
    })
}

/// Adds the folder at `path` (UTF-8): its fonts, and those of its subfolders, are used
/// before the system's. The folder is read when a document is converted, which fails
/// with `OW_STATUS_IO` if it cannot be read.
///
/// # Safety
///
/// `options` must be NULL or a live handle not used by another thread during the call;
/// `path` must be NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_render_options_add_font_dir(
    options: *mut OwRenderOptions,
    path: *const c_char,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let options = unsafe { options_mut(options) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let path = unsafe { str_arg(path, "path") }?;
        options.0 = std::mem::take(&mut options.0).with_font_dir(path);
        Ok(())
    })
}

/// Adds the font file (TrueType, OpenType or a collection) in the `len` bytes at
/// `data`, which the library copies; its fonts are used before the folders' and the
/// system's. Fails with `OW_STATUS_MALFORMED` if the data is not a font or every face in
/// it is damaged, leaving the options unchanged. The check reads what identifies the
/// font, not every glyph: a font damaged deeper is accepted here. The conversion that
/// embeds it then fails with `OW_STATUS_MALFORMED`, or, when the font cannot be embedded
/// at all, reports it (`OW_REPORT_KIND_FONT_EMBEDDING`) and does not draw its text.
///
/// # Safety
///
/// `options` must be NULL or a live handle not used by another thread during the call;
/// `data` must be NULL or valid for reading `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_render_options_add_font_bytes(
    options: *mut OwRenderOptions,
    data: *const u8,
    len: usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let options = unsafe { options_mut(options) }?;
        // SAFETY: the caller passes NULL or `len` readable bytes.
        let bytes = unsafe { bytes_arg(data, len, "data") }?;
        add_font(options, Arc::from(bytes))
    })
}

/// Adds the font file (TrueType, OpenType or a collection) at `path` (UTF-8), which the
/// library reads now; its fonts are used before the folders' and the system's, as those
/// of `ow_render_options_add_font_bytes` are. Fails with `OW_STATUS_IO` if the file cannot
/// be read and `OW_STATUS_MALFORMED` if it is not a font or every face in it is damaged,
/// with the path in the message, leaving the options unchanged. Like that of
/// `ow_render_options_add_font_bytes`, the check does not read every glyph.
///
/// # Safety
///
/// `options` must be NULL or a live handle not used by another thread during the call;
/// `path` must be NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_render_options_add_font_file(
    options: *mut OwRenderOptions,
    path: *const c_char,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let options = unsafe { options_mut(options) }?;
        // SAFETY: the caller passes NULL or a NUL-terminated string.
        let path = unsafe { str_arg(path, "path") }?;
        let data = std::fs::read(path).map_err(|source| Failure {
            status: OwStatus::Io,
            message: format!("cannot read {path}: {source}"),
        })?;
        add_font(options, Arc::from(data)).map_err(|failure| Failure {
            status: failure.status,
            message: format!("{path}: {}", failure.message),
        })
    })
}

/// Adds `data` to the options once it has been read as a font, so that data that is not
/// one fails here, where the caller can tell which it is, rather than at the conversion.
fn add_font(options: &mut OwRenderOptions, data: Arc<[u8]>) -> Result<(), Failure> {
    FontBook::new().add_bytes(Arc::clone(&data), FontOrigin::User)?;
    options.0 = std::mem::take(&mut options.0).with_font_data(data);
    Ok(())
}

/// Sets how the PDF's streams are compressed: `OW_COMPRESSION_FLATE`, the default, or
/// `OW_COMPRESSION_NONE`. Fails with `OW_STATUS_INVALID_ARGUMENT` for any other value,
/// leaving the options unchanged.
///
/// # Safety
///
/// `options` must be NULL or a live handle not used by another thread during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_render_options_set_compression(
    options: *mut OwRenderOptions,
    compression: i32,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a live handle it does not share.
        let options = unsafe { options_mut(options) }?;
        let compression = compression_arg(compression)?;
        options.0 = std::mem::take(&mut options.0).with_compression(compression);
        Ok(())
    })
}

/// Releases render options. Does nothing if `options` is NULL.
///
/// # Safety
///
/// `options` must be NULL or a live handle, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_render_options_free(options: *mut OwRenderOptions) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if !options.is_null() {
            // SAFETY: the handle came from `Box::into_raw` and is released only once.
            drop(unsafe { Box::from_raw(options) });
        }
    }));
}

/// Returns in `*out_len` the number of entries of `report`.
///
/// # Safety
///
/// `report` must be NULL or a live handle; `out_len` must be NULL or valid for writing a
/// `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_layout_report_len(
    report: *const OwLayoutReport,
    out_len: *mut usize,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or a pointer valid for writes.
        unsafe { put(out_len, "out_len", 0) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let report = unsafe { report_ref(report) }?;
        // SAFETY: as above.
        unsafe { put(out_len, "out_len", report.entries.len()) }
    })
}

/// Returns entry `index` (counting from 0) of `report`: its kind, an `OW_REPORT_KIND_*`
/// value, in `*out_kind`; where it is in the document, as a path such as
/// `sections[0].blocks[3]`, or `document` for the document as a whole, in
/// `*out_location`; and what happened, a sentence, in `*out_message`. Both strings are
/// UTF-8 and owned by the report: they stay valid until it is freed.
///
/// Fails with `OW_STATUS_INVALID_ARGUMENT` if `index` is not below the count
/// `ow_layout_report_len` gives.
///
/// # Safety
///
/// `report` must be NULL or a live handle; `out_kind`, `out_location` and `out_message`
/// must each be NULL or valid for writing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_layout_report_entry(
    report: *const OwLayoutReport,
    index: usize,
    out_kind: *mut i32,
    out_location: *mut *const c_char,
    out_message: *mut *const c_char,
) -> OwStatus {
    guard(|| {
        // SAFETY: the caller passes NULL or pointers valid for writes.
        unsafe { put(out_kind, "out_kind", OW_REPORT_KIND_OTHER) }?;
        // SAFETY: as above.
        unsafe { put(out_location, "out_location", ptr::null()) }?;
        // SAFETY: as above.
        unsafe { put(out_message, "out_message", ptr::null()) }?;
        // SAFETY: the caller passes NULL or a live handle.
        let report = unsafe { report_ref(report) }?;
        let (kind, location, message) = report.entries.get(index).ok_or_else(|| Failure {
            status: OwStatus::InvalidArgument,
            message: format!(
                "entry index {index} is out of range for a report with {} entries",
                report.entries.len()
            ),
        })?;
        // SAFETY: the pointers were checked above. The strings live as long as the
        // handle.
        unsafe { put(out_kind, "out_kind", *kind) }?;
        // SAFETY: as above.
        unsafe { put(out_location, "out_location", location.as_ptr()) }?;
        // SAFETY: as above.
        unsafe { put(out_message, "out_message", message.as_ptr()) }
    })
}

/// Releases a report. Does nothing if `report` is NULL.
///
/// # Safety
///
/// `report` must be NULL or a live handle, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ow_layout_report_free(report: *mut OwLayoutReport) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if !report.is_null() {
            // SAFETY: the handle came from `Box::into_raw` and is released only once.
            drop(unsafe { Box::from_raw(report) });
        }
    }));
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
            Error::Malformed(_) => OwStatus::Malformed,
            Error::LimitExceeded(_) => OwStatus::LimitExceeded,
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

impl From<officewright_font::Error> for Failure {
    fn from(err: officewright_font::Error) -> Self {
        Error::from(err).into()
    }
}

impl From<officewright_doc::Error> for Failure {
    fn from(err: officewright_doc::Error) -> Self {
        Error::from(err).into()
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

/// A C string of `text`, with any NUL byte replaced by U+FFFD.
fn c_string(text: String) -> CString {
    CString::new(text.replace('\0', "\u{FFFD}")).unwrap_or_default()
}

/// Reads an `int32_t` flag argument, which must be 0 or 1.
fn flag(value: i32, name: &str) -> Result<bool, Failure> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(Failure {
            status: OwStatus::InvalidArgument,
            message: format!("{name} is {other}, not 0 or 1"),
        }),
    }
}

/// Reads an `OW_COMPRESSION_*` argument.
fn compression_arg(value: i32) -> Result<Compression, Failure> {
    match value {
        OW_COMPRESSION_NONE => Ok(Compression::None),
        OW_COMPRESSION_FLATE => Ok(Compression::Flate),
        other => Err(Failure {
            status: OwStatus::InvalidArgument,
            message: format!(
                "compression is {other}, not OW_COMPRESSION_NONE (0) or \
                 OW_COMPRESSION_FLATE (1)"
            ),
        }),
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

/// Writes `value` through the out-parameter `out` when it is not NULL.
///
/// # Safety
///
/// `out` must be NULL or valid for writing a `T`.
unsafe fn put_optional<T>(out: *mut T, value: T) {
    if !out.is_null() {
        // SAFETY: not NULL, and the caller guarantees it is valid for writes.
        unsafe { out.write(value) };
    }
}

/// Gives the new `report` to the caller through `out` when it is not NULL, and frees it
/// otherwise.
///
/// # Safety
///
/// `out` must be NULL or valid for writing a pointer; `report` must come from
/// `Box::into_raw` and be given to nobody else.
unsafe fn give_optional(out: *mut *mut OwLayoutReport, report: *mut OwLayoutReport) {
    if out.is_null() {
        // SAFETY: the caller gives up `report`, which nobody else has.
        drop(unsafe { Box::from_raw(report) });
    } else {
        // SAFETY: not NULL, and the caller guarantees it is valid for writes.
        unsafe { out.write(report) };
    }
}

/// Borrows `len` bytes at `data`.
///
/// # Safety
///
/// `data` must be NULL or valid for reading `len` bytes that outlive the call.
unsafe fn bytes_arg<'a>(data: *const u8, len: usize, name: &str) -> Result<&'a [u8], Failure> {
    if data.is_null() {
        return Err(Failure::null(name));
    }
    // SAFETY: not NULL, and the caller guarantees `len` readable bytes.
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}

/// Borrows a Word document handle.
///
/// # Safety
///
/// `doc` must be NULL or a live handle that is not modified during the borrow.
unsafe fn doc_document_ref<'a>(
    doc: *const OwDocDocument,
) -> Result<&'a officewright_doc::Document, Failure> {
    // SAFETY: the caller guarantees a live handle if not NULL.
    unsafe { doc.as_ref() }
        .map(|doc| &doc.0)
        .ok_or_else(|| Failure::null("doc"))
}

/// Borrows a render options handle.
///
/// # Safety
///
/// `options` must be NULL or a live handle that is not modified during the borrow.
unsafe fn options_ref<'a>(options: *const OwRenderOptions) -> Result<&'a RenderOptions, Failure> {
    // SAFETY: the caller guarantees a live handle if not NULL.
    unsafe { options.as_ref() }
        .map(|options| &options.0)
        .ok_or_else(|| Failure::null("options"))
}

/// Borrows a render options handle mutably.
///
/// # Safety
///
/// `options` must be NULL or a live handle that nothing else uses during the borrow.
unsafe fn options_mut<'a>(
    options: *mut OwRenderOptions,
) -> Result<&'a mut OwRenderOptions, Failure> {
    // SAFETY: the caller guarantees a live, unshared handle if not NULL.
    unsafe { options.as_mut() }.ok_or_else(|| Failure::null("options"))
}

/// Borrows a report handle.
///
/// # Safety
///
/// `report` must be NULL or a live handle.
unsafe fn report_ref<'a>(report: *const OwLayoutReport) -> Result<&'a OwLayoutReport, Failure> {
    // SAFETY: the caller guarantees a live handle if not NULL.
    unsafe { report.as_ref() }.ok_or_else(|| Failure::null("report"))
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
unsafe fn font_handle<'a>(font: *const OwPdfFont) -> Result<&'a OwPdfFont, Failure> {
    // SAFETY: the caller guarantees a live handle if not NULL.
    unsafe { font.as_ref() }.ok_or_else(|| Failure::null("font"))
}

/// Borrows the font of a font handle.
///
/// # Safety
///
/// `font` must be NULL or a live handle.
unsafe fn font_ref<'a>(font: *const OwPdfFont) -> Result<&'a Font, Failure> {
    // SAFETY: the caller guarantees a live handle if not NULL.
    unsafe { font_handle(font) }.map(|font| &font.font)
}

/// Borrows a font book handle.
///
/// # Safety
///
/// `book` must be NULL or a live handle that is not modified during the borrow.
unsafe fn book_ref<'a>(book: *const OwFontBook) -> Result<&'a OwFontBook, Failure> {
    // SAFETY: the caller guarantees a live handle if not NULL.
    unsafe { book.as_ref() }.ok_or_else(|| Failure::null("book"))
}

/// Borrows a font book handle mutably.
///
/// # Safety
///
/// `book` must be NULL or a live handle that nothing else uses during the borrow.
unsafe fn book_mut<'a>(book: *mut OwFontBook) -> Result<&'a mut OwFontBook, Failure> {
    // SAFETY: the caller guarantees a live, unshared handle if not NULL.
    unsafe { book.as_mut() }.ok_or_else(|| Failure::null("book"))
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
        // Uncompressed, to read the page content.
        // SAFETY: a live handle.
        let status = unsafe { ow_pdf_document_set_compression(doc, OW_COMPRESSION_NONE) };
        assert_eq!(status, OwStatus::Ok);
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

    #[test]
    fn sets_how_streams_are_stored() {
        let contains = |pdf: &[u8], part: &[u8]| pdf.windows(part.len()).any(|w| w == part);
        let font = helvetica();
        let mut doc = ptr::null_mut();
        let text = c(&"Hello, world\n".repeat(40));
        // SAFETY: valid strings, out-pointers and handles, each freed once.
        unsafe {
            let status =
                ow_pdf_document_from_text(text.as_ptr(), 595.0, 842.0, font, 11.0, &mut doc);
            assert_eq!(status, OwStatus::Ok);
            ow_pdf_font_free(font);
            // Compressed unless set otherwise.
            let compressed = save_bytes(doc);
            assert!(contains(&compressed, b"/Filter /FlateDecode"));
            assert!(!contains(&compressed, b"(Hello, world) Tj"));

            let status = ow_pdf_document_set_compression(doc, OW_COMPRESSION_NONE);
            assert_eq!(status, OwStatus::Ok);
            let plain = save_bytes(doc);
            assert!(!contains(&plain, b"/Filter"));
            assert!(contains(&plain, b"(Hello, world) Tj"));
            assert!(plain.len() > compressed.len());

            assert_eq!(
                ow_pdf_document_set_compression(doc, 2),
                OwStatus::InvalidArgument
            );
            assert_eq!(
                last_error(),
                "compression is 2, not OW_COMPRESSION_NONE (0) or OW_COMPRESSION_FLATE (1)"
            );
            assert_eq!(save_bytes(doc), plain);
            let status = ow_pdf_document_set_compression(doc, OW_COMPRESSION_FLATE);
            assert_eq!(status, OwStatus::Ok);
            assert_eq!(save_bytes(doc), compressed);

            assert_eq!(
                ow_pdf_document_set_compression(ptr::null_mut(), OW_COMPRESSION_NONE),
                OwStatus::NullArgument
            );
            ow_pdf_document_free(doc);
        }
    }

    #[test]
    fn maps_each_kind_of_error_to_its_status() {
        let io = Error::Io {
            message: "cannot read a.ttf".into(),
            source: std::io::Error::other("gone"),
        };
        let cases = [
            (io, OwStatus::Io, "cannot read a.ttf: gone"),
            (
                Error::InvalidArgument("bad".into()),
                OwStatus::InvalidArgument,
                "bad",
            ),
            (
                Error::Unsupported("later".into()),
                OwStatus::Unsupported,
                "later",
            ),
            (
                Error::Malformed("broken".into()),
                OwStatus::Malformed,
                "broken",
            ),
            (
                Error::LimitExceeded("too big".into()),
                OwStatus::LimitExceeded,
                "too big",
            ),
        ];
        for (err, status, message) in cases {
            let failure = Failure::from(err);
            assert_eq!(failure.status, status);
            assert_eq!(failure.message, message);
        }
        // Statuses are only ever appended, so existing values keep their numbers.
        assert_eq!(OwStatus::Panic as i32, 6);
        assert_eq!(OwStatus::Malformed as i32, 7);
        assert_eq!(OwStatus::LimitExceeded as i32, 8);
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

    /// officewright-doc's smallest test document, "Hello, world", small enough for Miri.
    fn hello_docx_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../officewright-doc/tests/data/hello.docx")
    }

    /// Render options with only the minimal test font: no system fonts, no compression.
    fn minimal_options() -> *mut OwRenderOptions {
        let font = fs::read(minimal_font_path()).unwrap();
        let mut options = ptr::null_mut();
        // SAFETY: valid pointers and handles throughout.
        unsafe {
            assert_eq!(ow_render_options_new(&mut options), OwStatus::Ok);
            assert_eq!(ow_render_options_set_system_fonts(options, 0), OwStatus::Ok);
            assert_eq!(
                ow_render_options_add_font_bytes(options, font.as_ptr(), font.len()),
                OwStatus::Ok
            );
            assert_eq!(
                ow_render_options_set_compression(options, OW_COMPRESSION_NONE),
                OwStatus::Ok
            );
        }
        options
    }

    /// The entries of `report`: kind, location and message.
    fn report_entries(report: *const OwLayoutReport) -> Vec<(i32, String, String)> {
        let mut len = 0;
        // SAFETY: a live report and valid out-parameters.
        unsafe {
            assert_eq!(ow_layout_report_len(report, &mut len), OwStatus::Ok);
            (0..len)
                .map(|index| {
                    let (mut kind, mut location, mut message) = (0, ptr::null(), ptr::null());
                    assert_eq!(
                        ow_layout_report_entry(
                            report,
                            index,
                            &mut kind,
                            &mut location,
                            &mut message
                        ),
                        OwStatus::Ok
                    );
                    let text = |ptr| CStr::from_ptr(ptr).to_str().unwrap().to_owned();
                    (kind, text(location), text(message))
                })
                .collect()
        }
    }

    #[test]
    fn converts_a_word_document_to_pdf_in_memory_and_to_a_path() {
        let path = CString::new(hello_docx_path().to_str().unwrap()).unwrap();
        let bytes = fs::read(hello_docx_path()).unwrap();
        let options = minimal_options();
        // SAFETY: valid pointers and handles throughout.
        unsafe {
            let (mut from_path, mut from_bytes) = (ptr::null_mut(), ptr::null_mut());
            assert_eq!(
                ow_doc_document_open_path(path.as_ptr(), &mut from_path),
                OwStatus::Ok
            );
            assert_eq!(
                ow_doc_document_open_bytes(bytes.as_ptr(), bytes.len(), &mut from_bytes),
                OwStatus::Ok
            );
            let (mut data, mut len, mut report) = (ptr::null_mut(), 0, ptr::null_mut());
            assert_eq!(
                ow_doc_document_save_pdf_bytes(
                    from_bytes,
                    options,
                    &mut data,
                    &mut len,
                    &mut report
                ),
                OwStatus::Ok
            );
            let pdf = std::slice::from_raw_parts(data, len).to_vec();
            ow_bytes_free(data, len);
            assert!(pdf.starts_with(b"%PDF-"));
            // The options lack the document's font (Calibri), and the report says so.
            let entries = report_entries(report);
            assert!(
                entries.iter().any(
                    |(kind, _, message)| *kind == OW_REPORT_KIND_FONT_SUBSTITUTED
                        && !message.is_empty()
                ),
                "{entries:?}"
            );
            ow_layout_report_free(report);
            // The minimal font, given from memory, is in the book the options make.
            let book = (*options).0.font_book().unwrap();
            let families: Vec<&str> = book
                .faces()
                .filter_map(|(_, face)| face.names().family())
                .collect();
            assert_eq!(families, ["Officewright Test"]);
            // Through a path, without a report: the same file.
            let dir =
                std::env::temp_dir().join(format!("officewright-ffi-docx-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            let out = dir.join("hello.pdf");
            let out_c = CString::new(out.to_str().unwrap()).unwrap();
            assert_eq!(
                ow_doc_document_save_pdf_path(from_path, out_c.as_ptr(), options, ptr::null_mut()),
                OwStatus::Ok
            );
            assert_eq!(fs::read(&out).unwrap(), pdf);
            fs::remove_dir_all(&dir).unwrap();
            ow_doc_document_free(from_path);
            ow_doc_document_free(from_bytes);
            ow_render_options_free(options);
        }
    }

    #[test]
    fn conversion_errors_have_a_status_and_a_message() {
        let bytes = fs::read(hello_docx_path()).unwrap();
        let options = minimal_options();
        // SAFETY: valid pointers and handles throughout, NULL where the test says.
        unsafe {
            let mut doc = ptr::null_mut();
            // Not a Word document.
            assert_eq!(
                ow_doc_document_open_bytes(b"not a zip".as_ptr(), 9, &mut doc),
                OwStatus::Malformed
            );
            assert!(doc.is_null() && !last_error().is_empty());
            assert_eq!(
                ow_doc_document_open_bytes(ptr::null(), 0, &mut doc),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "data is null");
            let missing = CString::new("/nonexistent/officewright/hello.docx").unwrap();
            assert_eq!(
                ow_doc_document_open_path(missing.as_ptr(), &mut doc),
                OwStatus::Io
            );
            assert_eq!(
                ow_doc_document_open_bytes(bytes.as_ptr(), bytes.len(), &mut doc),
                OwStatus::Ok
            );
            // Options: no pages, a flag that is not 0 or 1, an unknown compression.
            assert_eq!(
                ow_render_options_set_max_pages(options, 0),
                OwStatus::InvalidArgument
            );
            assert_eq!(
                ow_render_options_set_system_fonts(options, 2),
                OwStatus::InvalidArgument
            );
            assert_eq!(last_error(), "system_fonts is 2, not 0 or 1");
            assert_eq!(
                ow_render_options_set_compression(options, 9),
                OwStatus::InvalidArgument
            );
            // A report entry out of range, and the out-parameters reset.
            let (mut data, mut len, mut report) = (ptr::null_mut(), 0, ptr::null_mut());
            assert_eq!(
                ow_doc_document_save_pdf_bytes(doc, options, &mut data, &mut len, &mut report),
                OwStatus::Ok
            );
            ow_bytes_free(data, len);
            let mut count = 0;
            assert_eq!(ow_layout_report_len(report, &mut count), OwStatus::Ok);
            let (mut kind, mut location, mut message) = (7, ptr::null(), ptr::null());
            assert_eq!(
                ow_layout_report_entry(report, count, &mut kind, &mut location, &mut message),
                OwStatus::InvalidArgument
            );
            assert!(kind == OW_REPORT_KIND_OTHER && location.is_null() && message.is_null());
            ow_layout_report_free(report);
            // Font data that is not a font is refused at once, and leaves the options as
            // they were.
            let mut damaged = ptr::null_mut();
            assert_eq!(ow_render_options_new(&mut damaged), OwStatus::Ok);
            assert_eq!(ow_render_options_set_system_fonts(damaged, 0), OwStatus::Ok);
            assert_eq!(
                ow_render_options_add_font_bytes(damaged, b"no font".as_ptr(), 7),
                OwStatus::Malformed
            );
            assert!(last_error().starts_with("data is not a TrueType or OpenType font"));
            assert_eq!(
                ow_doc_document_save_pdf_bytes(doc, damaged, &mut data, &mut len, ptr::null_mut()),
                OwStatus::Ok
            );
            ow_bytes_free(data, len);
            // A conversion that fails (a font folder that cannot be read) gives no PDF and
            // no report.
            let missing = CString::new("/nonexistent/officewright/fonts").unwrap();
            assert_eq!(
                ow_render_options_add_font_dir(damaged, missing.as_ptr()),
                OwStatus::Ok
            );
            let mut report = ptr::dangling_mut::<OwLayoutReport>();
            let (mut data, mut len) = (ptr::dangling_mut::<u8>(), 7);
            assert_eq!(
                ow_doc_document_save_pdf_bytes(doc, damaged, &mut data, &mut len, &mut report),
                OwStatus::Io
            );
            assert!(report.is_null() && data.is_null() && len == 0);
            // NULL handles.
            assert_eq!(
                ow_doc_document_save_pdf_bytes(
                    ptr::null(),
                    options,
                    &mut data,
                    &mut len,
                    ptr::null_mut()
                ),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "doc is null");
            assert_eq!(
                ow_doc_document_save_pdf_bytes(
                    doc,
                    ptr::null(),
                    &mut data,
                    &mut len,
                    ptr::null_mut()
                ),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "options is null");
            assert_eq!(
                ow_layout_report_len(ptr::null(), &mut count),
                OwStatus::NullArgument
            );
            // Freeing NULL does nothing.
            ow_doc_document_free(ptr::null_mut());
            ow_render_options_free(ptr::null_mut());
            ow_layout_report_free(ptr::null_mut());
            ow_render_options_free(damaged);
            ow_doc_document_free(doc);
            ow_render_options_free(options);
        }
    }

    #[test]
    fn a_report_from_a_path_is_given_or_reset() {
        let bytes = fs::read(hello_docx_path()).unwrap();
        let options = minimal_options();
        let dir =
            std::env::temp_dir().join(format!("officewright-ffi-report-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let out = CString::new(dir.join("hello.pdf").to_str().unwrap()).unwrap();
        let unwritable =
            CString::new(dir.join("missing-dir").join("hello.pdf").to_str().unwrap()).unwrap();
        // SAFETY: valid pointers and handles throughout.
        unsafe {
            let mut doc = ptr::null_mut();
            assert_eq!(
                ow_doc_document_open_bytes(bytes.as_ptr(), bytes.len(), &mut doc),
                OwStatus::Ok
            );
            let mut report = ptr::null_mut();
            assert_eq!(
                ow_doc_document_save_pdf_path(doc, out.as_ptr(), options, &mut report),
                OwStatus::Ok
            );
            assert!(
                report_entries(report)
                    .iter()
                    .any(|(kind, _, _)| *kind == OW_REPORT_KIND_FONT_SUBSTITUTED)
            );
            ow_layout_report_free(report);
            // A file that cannot be written: IO, and the report reset.
            let mut report = ptr::dangling_mut::<OwLayoutReport>();
            assert_eq!(
                ow_doc_document_save_pdf_path(doc, unwritable.as_ptr(), options, &mut report),
                OwStatus::Io
            );
            assert!(report.is_null());
            ow_doc_document_free(doc);
            ow_render_options_free(options);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_report_kind_has_its_constant() {
        use officewright_doc::{Location, ReportEntry};
        let kinds = [
            (
                ReportKind::FontSubstituted,
                OW_REPORT_KIND_FONT_SUBSTITUTED,
                1,
            ),
            (ReportKind::GlyphMissing, OW_REPORT_KIND_GLYPH_MISSING, 2),
            (
                ReportKind::FeatureUnsupported,
                OW_REPORT_KIND_FEATURE_UNSUPPORTED,
                3,
            ),
            (ReportKind::FieldCached, OW_REPORT_KIND_FIELD_CACHED, 4),
            (ReportKind::CompatIgnored, OW_REPORT_KIND_COMPAT_IGNORED, 5),
            (ReportKind::ValueClamped, OW_REPORT_KIND_VALUE_CLAMPED, 6),
            (ReportKind::Overflow, OW_REPORT_KIND_OVERFLOW, 7),
            (ReportKind::FontEmbedding, OW_REPORT_KIND_FONT_EMBEDDING, 8),
        ];
        let report: LayoutReport = kinds
            .iter()
            .map(|(kind, _, _)| ReportEntry::new(*kind, Location::document(), "a"))
            .collect();
        let handle = OwLayoutReport::new(report);
        let found: Vec<i32> = handle.entries.iter().map(|(kind, _, _)| *kind).collect();
        // The values are fixed: bindings hard-code them.
        let expected: Vec<i32> = kinds
            .iter()
            .map(|(_, constant, value)| {
                assert_eq!(constant, value);
                *value
            })
            .collect();
        assert_eq!(found, expected);
        assert_eq!(OW_REPORT_KIND_OTHER, 0);
    }

    #[test]
    fn conversion_statuses_name_the_failure() {
        // SAFETY: valid pointers and handles throughout.
        unsafe {
            // A binary Word document (an OLE compound file) is not supported.
            let mut ole = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
            ole.resize(512, 0);
            let mut doc = ptr::null_mut();
            assert_eq!(
                ow_doc_document_open_bytes(ole.as_ptr(), ole.len(), &mut doc),
                OwStatus::Unsupported
            );
            assert!(doc.is_null());
            // A font folder that cannot be read fails the conversion with IO.
            let bytes = fs::read(hello_docx_path()).unwrap();
            assert_eq!(
                ow_doc_document_open_bytes(bytes.as_ptr(), bytes.len(), &mut doc),
                OwStatus::Ok
            );
            let mut options = ptr::null_mut();
            assert_eq!(ow_render_options_new(&mut options), OwStatus::Ok);
            assert_eq!(ow_render_options_set_system_fonts(options, 0), OwStatus::Ok);
            let missing = CString::new("/nonexistent/officewright/fonts").unwrap();
            assert_eq!(
                ow_render_options_add_font_dir(options, missing.as_ptr()),
                OwStatus::Ok
            );
            let (mut data, mut len) = (ptr::null_mut(), 0);
            assert_eq!(
                ow_doc_document_save_pdf_bytes(doc, options, &mut data, &mut len, ptr::null_mut()),
                OwStatus::Io
            );
            // A font file read from a path: IO when it is missing, used when it is there.
            let missing_file = CString::new("/nonexistent/officewright/font.ttf").unwrap();
            assert_eq!(
                ow_render_options_add_font_file(options, missing_file.as_ptr()),
                OwStatus::Io
            );
            assert!(last_error().starts_with("cannot read /nonexistent/officewright/font.ttf"));
            assert_eq!(
                ow_render_options_add_font_file(options, ptr::null()),
                OwStatus::NullArgument
            );
            // A file that is not a font: refused at once, with its path.
            let docx = CString::new(hello_docx_path().to_str().unwrap()).unwrap();
            assert_eq!(
                ow_render_options_add_font_file(options, docx.as_ptr()),
                OwStatus::Malformed
            );
            assert!(last_error().starts_with(&format!("{}: data is not", docx.to_str().unwrap())));
            ow_render_options_free(options);
            let mut options = ptr::null_mut();
            assert_eq!(ow_render_options_new(&mut options), OwStatus::Ok);
            assert_eq!(ow_render_options_set_system_fonts(options, 0), OwStatus::Ok);
            let font = CString::new(minimal_font_path().to_str().unwrap()).unwrap();
            assert_eq!(
                ow_render_options_add_font_file(options, font.as_ptr()),
                OwStatus::Ok
            );
            // The file's font is in the book the options make, as from memory.
            let book = (*options).0.font_book().unwrap();
            let families: Vec<&str> = book
                .faces()
                .filter_map(|(_, face)| face.names().family())
                .collect();
            assert_eq!(families, ["Officewright Test"]);
            ow_render_options_free(options);
            ow_doc_document_free(doc);
        }
    }

    #[test]
    fn render_options_keep_what_they_are_set_to() {
        let mut options = ptr::null_mut();
        // SAFETY: valid pointers and a live handle throughout.
        unsafe {
            assert_eq!(ow_render_options_new(&mut options), OwStatus::Ok);
            let inner = |options: *mut OwRenderOptions| &(*options).0;
            assert_eq!(inner(options).max_pages(), 100_000);
            assert!(inner(options).system_fonts());
            assert_eq!(inner(options).compression(), Compression::Flate);
            assert_eq!(ow_render_options_set_max_pages(options, 7), OwStatus::Ok);
            assert_eq!(ow_render_options_set_system_fonts(options, 0), OwStatus::Ok);
            assert_eq!(
                ow_render_options_set_compression(options, OW_COMPRESSION_NONE),
                OwStatus::Ok
            );
            assert_eq!(inner(options).max_pages(), 7);
            assert!(!inner(options).system_fonts());
            assert_eq!(inner(options).compression(), Compression::None);
            ow_render_options_free(options);
        }
    }

    #[test]
    fn a_page_limit_of_the_document_s_pages_lets_it_through() {
        // "Hello, world" takes one page, so a limit of one passes. A document of more
        // pages is refused in tests/c and tests/ffi-python, which are not run under Miri.
        let options = minimal_options();
        let bytes = fs::read(hello_docx_path()).unwrap();
        // SAFETY: valid pointers and handles throughout.
        unsafe {
            let mut doc = ptr::null_mut();
            assert_eq!(
                ow_doc_document_open_bytes(bytes.as_ptr(), bytes.len(), &mut doc),
                OwStatus::Ok
            );
            assert_eq!(ow_render_options_set_max_pages(options, 1), OwStatus::Ok);
            let (mut data, mut len) = (ptr::null_mut(), 0);
            assert_eq!(
                ow_doc_document_save_pdf_bytes(doc, options, &mut data, &mut len, ptr::null_mut()),
                OwStatus::Ok
            );
            ow_bytes_free(data, len);
            ow_doc_document_free(doc);
            ow_render_options_free(options);
        }
    }

    /// officewright-font's minimal test font, small enough for Miri.
    fn minimal_font_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../officewright-font/tests/data/minimal.ttf")
    }

    /// The minimal font with `fsType` in its `OS/2` table (version 4) set to `fs_type`.
    fn minimal_font_with_fs_type(fs_type: u16) -> Vec<u8> {
        let mut font = fs::read(minimal_font_path()).unwrap();
        let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
        let record = (0..tables)
            .map(|i| 12 + 16 * i)
            .find(|&record| &font[record..record + 4] == b"OS/2")
            .unwrap();
        let os2 = u32::from_be_bytes(font[record + 8..record + 12].try_into().unwrap()) as usize;
        font[os2 + 8..os2 + 10].copy_from_slice(&fs_type.to_be_bytes());
        font
    }

    /// The minimal font with a new `name` table: the family, and `copies` records that
    /// the subsetter keeps (copyright, Windows Unicode) pointing at one string of 32,768
    /// bytes. With the family's 8 bytes, 31 copies point at less than 1 MiB of strings
    /// and 32 at more.
    fn minimal_font_with_name_copies(copies: u16) -> Vec<u8> {
        let mut font = fs::read(minimal_font_path()).unwrap();
        font.resize(font.len().next_multiple_of(4), 0);
        let count = copies + 1;
        let mut name = [0u16, count, 6 + 12 * count]
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect::<Vec<u8>>();
        let mut record = |id: u16, length: u16, offset: u16| {
            for value in [3, 1, 0x0409, id, length, offset] {
                name.extend(u16::to_be_bytes(value));
            }
        };
        record(1, 8, 0);
        for _ in 0..copies {
            record(0, 32_768, 8);
        }
        name.extend("Test".encode_utf16().flat_map(u16::to_be_bytes));
        name.extend([0, b'A'].repeat(16_384));
        let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
        let entry = (0..tables)
            .map(|i| 12 + 16 * i)
            .find(|&entry| &font[entry..entry + 4] == b"name")
            .unwrap();
        let offset = u32::try_from(font.len()).unwrap();
        let length = u32::try_from(name.len()).unwrap();
        font[entry + 8..entry + 12].copy_from_slice(&offset.to_be_bytes());
        font[entry + 12..entry + 16].copy_from_slice(&length.to_be_bytes());
        font.extend(name);
        font
    }

    fn open_bytes(data: &[u8]) -> (OwStatus, *mut OwPdfFont) {
        let mut font: *mut OwPdfFont = placeholder();
        // SAFETY: `data` is valid for its length; a valid out-pointer.
        let status = unsafe { ow_pdf_font_open_bytes(data.as_ptr(), data.len(), 0, &mut font) };
        (status, font)
    }

    #[test]
    fn embeds_fonts_opened_from_a_path_or_from_bytes() {
        let path = c(minimal_font_path().to_str().unwrap());
        let mut from_path = ptr::null_mut();
        // SAFETY: a NUL-terminated path and a valid out-pointer.
        let status = unsafe { ow_pdf_font_open_path(path.as_ptr(), 0, &mut from_path) };
        assert_eq!(status, OwStatus::Ok, "{}", last_error());
        let (status, from_bytes) = open_bytes(&fs::read(minimal_font_path()).unwrap());
        assert_eq!(status, OwStatus::Ok, "{}", last_error());

        let mut doc = ptr::null_mut();
        let mut page = 0;
        let mut width = 0.0;
        let mut notes = 7;
        let text = c("AÁ A");
        // SAFETY: live handles, NUL-terminated strings and valid out-pointers; each
        // handle is freed once, the fonts before the document that uses them.
        unsafe {
            assert_eq!(ow_pdf_document_new(&mut doc), OwStatus::Ok);
            assert_eq!(
                ow_pdf_document_add_page(doc, 595.0, 842.0, &mut page),
                OwStatus::Ok
            );
            for font in [from_path, from_bytes] {
                assert_eq!(
                    ow_pdf_document_add_text(doc, page, 72.0, 700.0, font, 12.0, text.as_ptr()),
                    OwStatus::Ok
                );
            }
            // A is 600 and the space 250 font units of 1000.
            assert_eq!(
                ow_pdf_font_text_width(from_bytes, c("A A").as_ptr(), 10.0, &mut width),
                OwStatus::Ok
            );
            assert_eq!(ow_pdf_font_note_count(from_path, &mut notes), OwStatus::Ok);
            ow_pdf_font_free(from_path);
            ow_pdf_font_free(from_bytes);
            let pdf = String::from_utf8_lossy(&save_bytes(doc)).into_owned();
            ow_pdf_document_free(doc);
            // Both handles are the same font, embedded once.
            assert_eq!(pdf.matches("/Subtype /Type0").count(), 1, "{pdf}");
            assert!(pdf.contains("/FontFile2"));
        }
        assert_eq!(width, 14.5);
        assert_eq!(notes, 0);
    }

    #[test]
    fn reports_fonts_it_cannot_open() {
        let (status, font) = open_bytes(b"not a font");
        assert_eq!(status, OwStatus::Malformed);
        assert!(font.is_null());
        assert!(
            last_error().starts_with("cannot use the font data: data is not a TrueType"),
            "{}",
            last_error()
        );
        // A face the file does not have is the caller's mistake, not damage.
        let data = fs::read(minimal_font_path()).unwrap();
        let mut font: *mut OwPdfFont = placeholder();
        // SAFETY: `data` is valid for its length; a valid out-pointer.
        let status = unsafe { ow_pdf_font_open_bytes(data.as_ptr(), data.len(), 1, &mut font) };
        assert_eq!(status, OwStatus::InvalidArgument);
        assert!(font.is_null());
        assert_eq!(
            last_error(),
            "cannot use the font data: font face 1 does not exist, the file has 1 face"
        );
        let (status, _) = open_bytes(&minimal_font_with_fs_type(0x0002));
        assert_eq!(status, OwStatus::Unsupported);
        assert_eq!(
            last_error(),
            "the license of font OfficewrightTest-Regular does not allow embedding it"
        );

        let path = std::env::temp_dir().join("officewright-missing-dir/font.ttf");
        let c_path = c(path.to_str().unwrap());
        let mut font: *mut OwPdfFont = placeholder();
        // SAFETY: NUL-terminated strings, valid out-pointers and NULL where tested.
        unsafe {
            assert_eq!(
                ow_pdf_font_open_path(c_path.as_ptr(), 0, &mut font),
                OwStatus::Io
            );
            assert!(font.is_null());
            assert!(
                last_error().starts_with(&format!("cannot read {}: ", path.display())),
                "{}",
                last_error()
            );
            assert_eq!(
                ow_pdf_font_open_path(ptr::null(), 0, &mut font),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "path is null");
            assert_eq!(
                ow_pdf_font_open_bytes(ptr::null(), 0, 0, &mut font),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "data is null");
            assert_eq!(
                ow_pdf_font_open_path(c_path.as_ptr(), 0, ptr::null_mut()),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "out_font is null");
        }
    }

    #[test]
    fn a_font_past_a_limit_of_the_library_is_refused_as_a_limit() {
        let (status, font) = open_bytes(&minimal_font_with_name_copies(31));
        assert_eq!(status, OwStatus::Ok, "{}", last_error());
        // SAFETY: a live handle, freed once.
        unsafe { ow_pdf_font_free(font) };
        let (status, font) = open_bytes(&minimal_font_with_name_copies(32));
        assert_eq!(status, OwStatus::LimitExceeded, "{}", last_error());
        assert!(font.is_null());
        assert_eq!(
            last_error(),
            "cannot use the font data: font face 0: table 'name' is past a limit of the \
             library: the name records to keep point at 1048584 bytes of strings, more \
             than the 1048576 read before subsetting"
        );
    }

    #[test]
    fn lists_the_embedding_notes_of_a_font() {
        let (status, font) = open_bytes(&minimal_font_with_fs_type(0x0104));
        assert_eq!(status, OwStatus::Ok, "{}", last_error());
        let mut count = 0;
        let mut kind = -1;
        let mut message: *const c_char = ptr::null();
        // SAFETY: a live handle and valid out-pointers; messages are read while the
        // handle lives; NULL where tested.
        unsafe {
            assert_eq!(ow_pdf_font_note_count(font, &mut count), OwStatus::Ok);
            assert_eq!(count, 2);
            let mut notes = Vec::new();
            for index in 0..count {
                assert_eq!(
                    ow_pdf_font_note(font, index, &mut kind, &mut message),
                    OwStatus::Ok
                );
                notes.push((kind, CStr::from_ptr(message).to_str().unwrap().to_owned()));
            }
            assert_eq!(
                notes,
                [
                    (
                        OW_FONT_NOTE_PREVIEW_AND_PRINT,
                        "its license allows the embedded font only for viewing and printing \
                         the document"
                            .to_owned()
                    ),
                    (
                        OW_FONT_NOTE_WHOLE_FONT,
                        "its license forbids subsetting, so the whole font is embedded".to_owned()
                    ),
                ]
            );
            assert_eq!(
                ow_pdf_font_note(font, 2, &mut kind, &mut message),
                OwStatus::InvalidArgument
            );
            assert_eq!(
                last_error(),
                "note index 2 is out of range for a font with 2 notes"
            );
            assert_eq!(kind, OW_FONT_NOTE_OTHER);
            assert!(message.is_null());
            assert_eq!(
                ow_pdf_font_note(font, 0, ptr::null_mut(), &mut message),
                OwStatus::NullArgument
            );
            assert_eq!(
                ow_pdf_font_note_count(ptr::null(), &mut count),
                OwStatus::NullArgument
            );
            assert_eq!(count, 0);
            ow_pdf_font_free(font);

            // A standard font has no notes.
            let helvetica = helvetica();
            assert_eq!(ow_pdf_font_note_count(helvetica, &mut count), OwStatus::Ok);
            assert_eq!(count, 0);
            ow_pdf_font_free(helvetica);
        }
    }

    #[test]
    fn reports_text_an_embedded_font_cannot_show() {
        let (status, font) = open_bytes(&fs::read(minimal_font_path()).unwrap());
        assert_eq!(status, OwStatus::Ok);
        let doc = hello();
        // SAFETY: live handles and NUL-terminated strings, each handle freed once.
        unsafe {
            for (text, message) in [
                (
                    "Aש",
                    "character U+05E9 'ש' at byte 1 is in the Hebrew script, which needs \
                     shaping or right-to-left layout; that is not supported yet",
                ),
                (
                    "AЖ",
                    "font OfficewrightTest-Regular has no glyph for character U+0416 'Ж' at byte 1",
                ),
            ] {
                let text = c(text);
                let status = ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 10.0, text.as_ptr());
                assert_eq!(status, OwStatus::Unsupported);
                assert_eq!(last_error(), message);
            }
            ow_pdf_font_free(font);
            ow_pdf_document_free(doc);
        }
    }

    /// A new font book handle; the caller frees it.
    fn new_book() -> *mut OwFontBook {
        let mut book = ptr::null_mut();
        // SAFETY: a valid out-pointer.
        assert_eq!(unsafe { ow_font_book_new(&mut book) }, OwStatus::Ok);
        book
    }

    /// A resolver for `book`; the caller frees it.
    fn new_resolver(book: *const OwFontBook) -> *mut OwFontResolver {
        let mut resolver = ptr::null_mut();
        // SAFETY: a live handle and a valid out-pointer.
        let status = unsafe { ow_font_resolver_new(book, &mut resolver) };
        assert_eq!(status, OwStatus::Ok);
        resolver
    }

    /// Resolves `family`; returns the status and the font, which the caller frees.
    fn resolve(
        resolver: *mut OwFontResolver,
        family: &str,
        bold: i32,
    ) -> (OwStatus, *mut OwPdfFont) {
        let family = c(family);
        let mut font: *mut OwPdfFont = placeholder();
        // SAFETY: a live handle, a NUL-terminated string and a valid out-pointer.
        let status = unsafe { ow_pdf_font_resolve(resolver, family.as_ptr(), bold, 0, &mut font) };
        (status, font)
    }

    #[test]
    fn reports_a_chosen_font_that_turned_out_damaged() {
        let dir =
            std::env::temp_dir().join(format!("officewright-ffi-damaged-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("minimal.ttf");
        // Read and write rather than `fs::copy`: on Linux it calls `copy_file_range`, which Miri
        // does not support, and the nightly run tests this crate under Miri.
        fs::write(&path, fs::read(minimal_font_path()).unwrap()).unwrap();
        let book = new_book();
        let mut added = 0;
        // SAFETY: a live handle, a NUL-terminated path and a valid out-pointer.
        let status =
            unsafe { ow_font_book_add_dir(book, c(dir.to_str().unwrap()).as_ptr(), &mut added) };
        assert_eq!(status, OwStatus::Ok);
        assert_eq!(added, 1);
        let resolver = new_resolver(book);
        // The resolver reads the file when it chooses the font, by which time it is junk.
        fs::write(&path, b"no longer a font").unwrap();
        let (status, font) = resolve(resolver, "Officewright Test", 0);
        assert_eq!(status, OwStatus::Malformed);
        assert!(font.is_null());
        assert!(
            last_error().starts_with(&format!("cannot use the font in {}: ", path.display())),
            "{}",
            last_error()
        );
        // SAFETY: live handles, each freed once.
        unsafe {
            ow_font_resolver_free(resolver);
            ow_font_book_free(book);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The substitution report of `resolver`.
    fn substitutions(resolver: *const OwFontResolver) -> Vec<(i32, String)> {
        let mut count = 99;
        // SAFETY: a live handle and valid out-pointers; messages are read while it lives.
        unsafe {
            assert_eq!(
                ow_font_resolver_substitution_count(resolver, &mut count),
                OwStatus::Ok
            );
            (0..count)
                .map(|index| {
                    let mut kind = -1;
                    let mut message = ptr::null();
                    assert_eq!(
                        ow_font_resolver_substitution(resolver, index, &mut kind, &mut message),
                        OwStatus::Ok
                    );
                    (kind, CStr::from_ptr(message).to_str().unwrap().to_owned())
                })
                .collect()
        }
    }

    /// A font collection of the single font `font`, its table offsets moved to where it
    /// lies in the collection, and of a face whose offset is past the end of the data.
    fn collection_with_a_damaged_face(font: &[u8]) -> Vec<u8> {
        let at = 20u32;
        let mut moved = font.to_vec();
        let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
        for i in 0..tables {
            let field = 12 + 16 * i + 8;
            let offset = u32::from_be_bytes(font[field..field + 4].try_into().unwrap());
            moved[field..field + 4].copy_from_slice(&(offset + at).to_be_bytes());
        }
        let past = at + u32::try_from(moved.len()).unwrap() + 100;
        let mut ttc = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x02".to_vec();
        ttc.extend_from_slice(&at.to_be_bytes());
        ttc.extend_from_slice(&past.to_be_bytes());
        ttc.extend_from_slice(&moved);
        ttc
    }

    #[test]
    fn lists_what_a_font_added_under_a_family_name_skipped() {
        let book = new_book();
        let ttc = collection_with_a_damaged_face(&fs::read(minimal_font_path()).unwrap());
        let (mut added, mut count) = (7, 7);
        let mut message = ptr::null();
        // SAFETY: a live handle, a buffer of its length, a NUL-terminated string and valid
        // out-pointers; the handle is freed once.
        unsafe {
            assert_eq!(
                ow_font_book_add_bytes_as(
                    book,
                    ttc.as_ptr(),
                    ttc.len(),
                    c("Any").as_ptr(),
                    &mut added
                ),
                OwStatus::Ok,
                "{}",
                last_error()
            );
            assert_eq!(added, 1);
            assert_eq!(ow_font_book_skipped_count(book, &mut count), OwStatus::Ok);
            assert_eq!(count, 1);
            assert_eq!(ow_font_book_skipped(book, 0, &mut message), OwStatus::Ok);
            assert!(!message.is_null());
            ow_font_book_free(book);
        }
        // The same from a file.
        let path = std::env::temp_dir().join(format!("ow-add-file-as-{}.ttc", std::process::id()));
        fs::write(&path, &ttc).unwrap();
        let book = new_book();
        let c_path = c(path.to_str().unwrap());
        // SAFETY: as above.
        unsafe {
            assert_eq!(
                ow_font_book_add_file_as(book, c_path.as_ptr(), c("Any").as_ptr(), &mut added),
                OwStatus::Ok,
                "{}",
                last_error()
            );
            assert_eq!(ow_font_book_skipped_count(book, &mut count), OwStatus::Ok);
            assert_eq!(count, 1);
            assert_eq!(ow_font_book_skipped(book, 0, &mut message), OwStatus::Ok);
            assert!(!message.is_null());
            ow_font_book_free(book);
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn adds_a_font_under_another_family_name() {
        let book = new_book();
        let data = fs::read(minimal_font_path()).unwrap();
        let mut added = 7;
        // SAFETY: a live handle, a buffer of its length, NUL-terminated strings and valid
        // out-pointers.
        unsafe {
            assert_eq!(
                ow_font_book_add_bytes_as(
                    book,
                    data.as_ptr(),
                    data.len(),
                    c("IBM Plex Serif Light").as_ptr(),
                    &mut added
                ),
                OwStatus::Ok
            );
            assert_eq!(added, 1);
            assert_eq!(
                ow_font_book_add_bytes_as(book, data.as_ptr(), data.len(), ptr::null(), &mut added),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "family is null");
            assert_eq!(
                ow_font_book_add_bytes_as(book, ptr::null(), 0, c("Any").as_ptr(), &mut added),
                OwStatus::NullArgument
            );
            let invalid = [0xFFu8, 0];
            assert_eq!(
                ow_font_book_add_bytes_as(
                    book,
                    data.as_ptr(),
                    data.len(),
                    invalid.as_ptr().cast(),
                    &mut added
                ),
                OwStatus::InvalidUtf8
            );
            assert_eq!(
                ow_font_book_add_bytes_as(book, b"junk".as_ptr(), 4, c("Any").as_ptr(), &mut added),
                OwStatus::Malformed
            );
            assert_eq!(added, 0);
        }
        let path = c(minimal_font_path().to_str().unwrap());
        // SAFETY: as above.
        unsafe {
            assert_eq!(
                ow_font_book_add_file_as(book, path.as_ptr(), c("Plex File").as_ptr(), &mut added),
                OwStatus::Ok
            );
            assert_eq!(added, 1);
            assert_eq!(
                ow_font_book_add_file_as(book, path.as_ptr(), ptr::null(), &mut added),
                OwStatus::NullArgument
            );
            added = 7;
            assert_eq!(
                ow_font_book_add_file_as(
                    book,
                    c("no-such-font.ttf").as_ptr(),
                    c("Any").as_ptr(),
                    &mut added
                ),
                OwStatus::Io
            );
            assert_eq!(added, 0);
            let invalid = [0xFFu8, 0];
            assert_eq!(
                ow_font_book_add_file_as(book, path.as_ptr(), invalid.as_ptr().cast(), &mut added),
                OwStatus::InvalidUtf8
            );
        }
        let resolver = new_resolver(book);
        // SAFETY: a live handle, freed once.
        unsafe { ow_font_book_free(book) };
        for family in ["IBM Plex Serif Light", "Officewright Test", "Plex File"] {
            let (status, font) = resolve(resolver, family, 0);
            assert_eq!(status, OwStatus::Ok, "{family}: {}", last_error());
            // SAFETY: a live handle, freed once.
            unsafe { ow_pdf_font_free(font) };
        }
        assert!(substitutions(resolver).is_empty());
        // SAFETY: a live handle, freed once.
        unsafe { ow_font_resolver_free(resolver) };
    }

    #[test]
    fn chooses_fonts_by_family_and_reports_substitutions() {
        let book = new_book();
        let path = c(minimal_font_path().to_str().unwrap());
        let data = fs::read(minimal_font_path()).unwrap();
        let mut added = 7;
        let mut faces = 0;
        // SAFETY: live handles, a NUL-terminated path, a buffer of its length and valid
        // out-pointers; each handle is freed once.
        unsafe {
            assert_eq!(
                ow_font_book_add_file(book, path.as_ptr(), &mut added),
                OwStatus::Ok
            );
            assert_eq!(added, 1);
            assert_eq!(
                ow_font_book_add_bytes(book, data.as_ptr(), data.len(), &mut added),
                OwStatus::Ok
            );
            assert_eq!(ow_font_book_face_count(book, &mut faces), OwStatus::Ok);
            assert_eq!(faces, 2);
        }
        let resolver = new_resolver(book);
        // The resolver has its own copy of the book.
        // SAFETY: a live handle, freed once.
        unsafe { ow_font_book_free(book) };

        let (status, font) = resolve(resolver, "Officewright Test", 0);
        assert_eq!(status, OwStatus::Ok, "{}", last_error());
        assert!(substitutions(resolver).is_empty());
        let (status, bold) = resolve(resolver, "officewright test", 1);
        assert_eq!(status, OwStatus::Ok, "{}", last_error());
        let (status, missing) = resolve(resolver, "Calibri", 0);
        assert_eq!(status, OwStatus::Unsupported);
        assert_eq!(last_error(), "no font found for family \"Calibri\"");
        assert!(missing.is_null());
        assert_eq!(
            substitutions(resolver),
            [
                (
                    OW_RESOLUTION_EXACT,
                    "\"officewright test\" bold → \"Officewright Test\" with synthetic bold"
                        .to_owned()
                ),
                (
                    OW_RESOLUTION_MISSING,
                    "\"Calibri\" → no font available".to_owned()
                ),
            ]
        );
        let mut width = 0.0;
        // SAFETY: live handles and valid pointers, each freed once.
        unsafe {
            assert_eq!(
                ow_pdf_font_text_width(font, c("A").as_ptr(), 10.0, &mut width),
                OwStatus::Ok
            );
            ow_pdf_font_free(font);
            ow_pdf_font_free(bold);
            ow_font_resolver_free(resolver);
        }
        assert_eq!(width, 6.0);
    }

    #[test]
    fn a_resolver_sees_the_fonts_its_book_had_when_it_was_made() {
        let book = new_book();
        let resolver = new_resolver(book);
        let path = c(minimal_font_path().to_str().unwrap());
        let mut added = 0;
        // SAFETY: a live handle, a NUL-terminated path and a valid out-pointer.
        let status = unsafe { ow_font_book_add_file(book, path.as_ptr(), &mut added) };
        assert_eq!((status, added), (OwStatus::Ok, 1));
        let (status, font) = resolve(resolver, "Officewright Test", 0);
        assert_eq!(status, OwStatus::Unsupported);
        assert!(font.is_null());
        let later = new_resolver(book);
        let (status, font) = resolve(later, "Officewright Test", 0);
        assert_eq!(status, OwStatus::Ok);
        // SAFETY: live handles, each freed once.
        unsafe {
            ow_pdf_font_free(font);
            ow_font_resolver_free(later);
            ow_font_resolver_free(resolver);
            ow_font_book_free(book);
        }
    }

    #[test]
    fn lists_what_a_font_book_skipped() {
        let dir =
            std::env::temp_dir().join(format!("officewright-ffi-book-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let broken = dir.join("broken.ttf");
        fs::write(&broken, b"not a font").unwrap();
        let c_dir = c(dir.to_str().unwrap());
        let missing = c(dir.join("missing").to_str().unwrap());
        let book = new_book();
        let mut added = 7;
        let mut count = 7;
        let mut message: *const c_char = placeholder();
        // SAFETY: a live handle, NUL-terminated paths and valid out-pointers; messages are
        // read before the book changes; NULL where tested.
        unsafe {
            assert_eq!(
                ow_font_book_add_dir(book, c_dir.as_ptr(), &mut added),
                OwStatus::Ok
            );
            assert_eq!(added, 0);
            assert_eq!(ow_font_book_skipped_count(book, &mut count), OwStatus::Ok);
            assert_eq!(count, 1);
            assert_eq!(ow_font_book_skipped(book, 0, &mut message), OwStatus::Ok);
            let kept = message;
            let text = CStr::from_ptr(kept).to_str().unwrap().to_owned();
            assert!(
                text.starts_with(&format!("{}: data is not a TrueType", broken.display())),
                "{text}"
            );
            assert_eq!(
                ow_font_book_skipped(book, 1, &mut message),
                OwStatus::InvalidArgument
            );
            assert_eq!(
                last_error(),
                "skipped index 1 is out of range for a font book with 1 skipped"
            );
            assert!(message.is_null());
            added = 7;
            assert_eq!(
                ow_font_book_add_dir(book, missing.as_ptr(), &mut added),
                OwStatus::Io
            );
            assert_eq!(added, 0);
            assert_eq!(
                ow_font_book_add_file(book, c(broken.to_str().unwrap()).as_ptr(), &mut added),
                OwStatus::Malformed
            );
            assert_eq!(
                ow_font_book_add_bytes(book, b"junk".as_ptr(), 4, &mut added),
                OwStatus::Malformed
            );
            // A scan that finds nothing new changes nothing either. Calls that change
            // nothing keep the message valid; Miri would report a read of freed memory.
            assert_eq!(
                ow_font_book_add_dir(book, c_dir.as_ptr(), &mut added),
                OwStatus::Ok
            );
            assert_eq!(CStr::from_ptr(kept).to_str().unwrap(), text);
            ow_font_book_free(book);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The only test that reads the machine's fonts, so the others stay hermetic.
    #[test]
    fn adds_the_system_fonts_whatever_the_machine_has() {
        let book = new_book();
        let mut added = 0;
        let mut faces = 7;
        // SAFETY: a live handle and valid out-pointers; the handle is freed once.
        unsafe {
            assert_eq!(
                ow_font_book_add_system_fonts(book, &mut added),
                OwStatus::Ok
            );
            assert_eq!(ow_font_book_face_count(book, &mut faces), OwStatus::Ok);
            ow_font_book_free(book);
        }
        assert_eq!(faces, added);
    }

    #[test]
    fn checks_the_arguments_of_the_font_book_and_resolver() {
        let book = new_book();
        let resolver = new_resolver(book);
        let family = c("x");
        let mut font: *mut OwPdfFont = placeholder();
        let mut count = 7;
        let mut handle: *mut OwFontResolver = placeholder();
        // SAFETY: live handles and valid pointers; NULL is what is being tested.
        unsafe {
            assert_eq!(
                ow_pdf_font_resolve(resolver, family.as_ptr(), 2, 0, &mut font),
                OwStatus::InvalidArgument
            );
            assert_eq!(last_error(), "bold is 2, not 0 or 1");
            assert!(font.is_null());
            assert_eq!(
                ow_pdf_font_resolve(resolver, family.as_ptr(), 0, -1, &mut font),
                OwStatus::InvalidArgument
            );
            assert_eq!(last_error(), "italic is -1, not 0 or 1");
            assert_eq!(
                ow_pdf_font_resolve(ptr::null_mut(), family.as_ptr(), 0, 0, &mut font),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "resolver is null");
            assert_eq!(
                ow_pdf_font_resolve(resolver, ptr::null(), 0, 0, &mut font),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "family is null");
            assert_eq!(
                ow_font_resolver_new(ptr::null(), &mut handle),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "book is null");
            assert!(handle.is_null());
            assert_eq!(
                ow_font_book_face_count(ptr::null(), &mut count),
                OwStatus::NullArgument
            );
            assert_eq!(count, 0);
            assert_eq!(
                ow_font_book_add_bytes(book, ptr::null(), 0, &mut count),
                OwStatus::NullArgument
            );
            assert_eq!(last_error(), "data is null");
            assert_eq!(ow_font_book_new(ptr::null_mut()), OwStatus::NullArgument);
            assert_eq!(last_error(), "out_book is null");
            assert_eq!(
                ow_font_book_add_system_fonts(ptr::null_mut(), &mut count),
                OwStatus::NullArgument
            );
            ow_font_book_free(ptr::null_mut());
            ow_font_resolver_free(ptr::null_mut());
            ow_font_resolver_free(resolver);
            ow_font_book_free(book);
        }
    }
}
