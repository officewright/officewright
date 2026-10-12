use std::io::Write;
use std::path::Path;
use std::{fs, io};

use officewright_core::{PageSize, Pt};

use crate::font::check_font_size;
use crate::{Error, Font, Result};

/// Pages smaller or larger than this are outside ISO 32000-1, Annex C.
const PAGE_SIDE: std::ops::RangeInclusive<f64> = 3.0..=14400.0;

/// Text positions stay within this many points of the origin: 32767 is the largest
/// real number every PDF 1.4 reader has to handle (PDF Reference 1.4, Appendix C), far
/// beyond the largest page.
pub(crate) const MAX_POSITION: f64 = 32767.0;

/// The largest byte offset the 10-digit fields of a cross-reference table can hold.
const MAX_XREF_OFFSET: u64 = 9_999_999_999;

/// A PDF document being built.
///
/// A `Document` can be moved between threads; it has no interior mutability.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pages: Vec<Page>,
}

/// A page of a [`Document`].
#[derive(Clone, Debug)]
pub struct Page {
    size: PageSize,
    content: Vec<u8>,
}

impl Document {
    /// Creates a document with no pages.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a page and returns it.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidPageSize`] if a side is not between 3 and 14400 pt.
    pub fn add_page(&mut self, size: PageSize) -> Result<&mut Page> {
        if !PAGE_SIDE.contains(&size.width().0) || !PAGE_SIDE.contains(&size.height().0) {
            return Err(Error::InvalidPageSize {
                width: size.width(),
                height: size.height(),
            });
        }
        let index = self.pages.len();
        self.pages.push(Page {
            size,
            content: Vec::new(),
        });
        #[allow(clippy::indexing_slicing)] // reason: the page was just pushed at `index`
        let page = &mut self.pages[index];
        Ok(page)
    }

    /// The number of pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The page at `index`, counting from 0 in the order the pages were added.
    pub fn page_mut(&mut self, index: usize) -> Option<&mut Page> {
        self.pages.get_mut(index)
    }

    /// Writes the document as a PDF file to `out`.
    ///
    /// # Errors
    ///
    /// [`Error::NoPages`] if the document has no pages, [`Error::TooLarge`] if the file
    /// would exceed 10<sup>10</sup> bytes, and [`Error::Io`] if writing fails.
    pub fn write_to(&self, out: impl Write) -> Result<()> {
        if self.pages.is_empty() {
            return Err(Error::NoPages);
        }
        // Objects: 1 catalog, 2 page tree, 3 font, then page and content stream per page.
        let page_object = |i: usize| 4 + 2 * i;
        let mut out = CountingWriter::new(out);
        let mut offsets = Vec::with_capacity(3 + 2 * self.pages.len());

        // The comment's bytes above 127 tell transfer tools the file is binary (§7.5.2).
        out.put(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n")?;

        offsets.push(out.written);
        out.put(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n")?;

        offsets.push(out.written);
        let kids: Vec<String> = (0..self.pages.len())
            .map(|i| format!("{} 0 R", page_object(i)))
            .collect();
        out.put(
            format!(
                "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
                kids.join(" "),
                self.pages.len()
            )
            .as_bytes(),
        )?;

        offsets.push(out.written);
        out.put(
            b"3 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
              /Encoding /WinAnsiEncoding >>\nendobj\n",
        )?;

        for (i, page) in self.pages.iter().enumerate() {
            let page_id = page_object(i);
            offsets.push(out.written);
            out.put(
                format!(
                    "{page_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] \
                     /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>\nendobj\n",
                    number(page.size.width().0),
                    number(page.size.height().0),
                    page_id + 1
                )
                .as_bytes(),
            )?;

            offsets.push(out.written);
            out.put(
                format!(
                    "{} 0 obj\n<< /Length {} >>\nstream\n",
                    page_id + 1,
                    page.content.len()
                )
                .as_bytes(),
            )?;
            out.put(&page.content)?;
            // The end-of-line before `endstream` is not part of /Length (§7.3.8.1).
            out.put(b"\nendstream\nendobj\n")?;
        }

        let xref = out.written;
        let size = offsets.len() + 1;
        out.put(format!("xref\n0 {size}\n0000000000 65535 f\r\n").as_bytes())?;
        for offset in offsets {
            if offset > MAX_XREF_OFFSET {
                return Err(Error::TooLarge);
            }
            // Each entry is exactly 20 bytes, including its two-byte end-of-line (§7.5.4).
            out.put(format!("{offset:010} 00000 n\r\n").as_bytes())?;
        }
        out.put(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        )?;
        out.inner.flush()?;
        Ok(())
    }

    /// Returns the document as a PDF file in memory.
    ///
    /// # Errors
    ///
    /// As [`write_to`](Self::write_to).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.write_to(&mut bytes)?;
        Ok(bytes)
    }

    /// Writes the document as a PDF file at `path`, replacing any existing file.
    ///
    /// The file is generated in memory first, so a document error never touches `path`;
    /// an I/O error while writing can leave a partial file.
    ///
    /// # Errors
    ///
    /// As [`write_to`](Self::write_to), and [`Error::Write`] if the file cannot be written.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let bytes = self.to_bytes()?;
        fs::write(path, bytes).map_err(|source| Error::Write {
            path: path.to_path_buf(),
            source,
        })
    }
}

impl Page {
    /// The page size.
    pub fn size(&self) -> PageSize {
        self.size
    }

    /// Draws `text` as a single line in `font` at `font_size`.
    ///
    /// Coordinates are native PDF coordinates in points: the origin is the bottom-left
    /// corner of the page, `y` grows upwards, and (`x`, `y`) is where the baseline of the
    /// first character starts. The text is not wrapped or clipped;
    /// [`Font::text_width`] measures it.
    ///
    /// # Errors
    ///
    /// [`Error::Unencodable`] if `text` has a character the font's encoding lacks (for
    /// Helvetica anything outside WinAnsiEncoding, including control characters such as
    /// `\n` and `\t`), [`Error::InvalidFontSize`] for a size outside
    /// [`Font::MIN_SIZE`] to [`Font::MAX_SIZE`], and [`Error::InvalidPosition`] for a
    /// position outside -32767 to 32767 pt. On error the page is unchanged.
    pub fn add_text(&mut self, x: Pt, y: Pt, font: &Font, font_size: Pt, text: &str) -> Result<()> {
        check_font_size(font_size)?;
        let positions = -MAX_POSITION..=MAX_POSITION;
        if !positions.contains(&x.0) || !positions.contains(&y.0) {
            return Err(Error::InvalidPosition { x, y });
        }
        let encoded = font.encode(text)?;
        let content = &mut self.content;
        // Helvetica, the only font so far, is the page resource /F1 (object 3).
        content.extend_from_slice(format!("BT\n/F1 {} Tf\n", number(font_size.0)).as_bytes());
        content.extend_from_slice(format!("{} {} Td\n", number(x.0), number(y.0)).as_bytes());
        push_string(content, &encoded);
        content.extend_from_slice(b" Tj\nET\n");
        Ok(())
    }
}

/// Formats a finite number as a PDF real: at most four decimals, no exponent.
fn number(value: f64) -> String {
    let fixed = format!("{value:.4}");
    match fixed.trim_end_matches('0').trim_end_matches('.') {
        "-0" => "0".to_owned(),
        trimmed => trimmed.to_owned(),
    }
}

/// Appends `bytes` as a PDF literal string (§7.3.4.2). Bytes outside printable ASCII are
/// written as octal escapes, so content streams stay ASCII.
fn push_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'(');
    for &byte in bytes {
        match byte {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', byte]),
            b' '..=b'~' => out.push(byte),
            _ => out.extend_from_slice(&[
                b'\\',
                b'0' + (byte >> 6),
                b'0' + ((byte >> 3) & 7),
                b'0' + (byte & 7),
            ]),
        }
    }
    out.push(b')');
}

/// Counts the bytes written, so object offsets can go into the cross-reference table.
struct CountingWriter<W> {
    inner: W,
    written: u64,
}

impl<W: Write> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StandardFont;

    fn helvetica() -> Font {
        Font::standard(StandardFont::Helvetica)
    }

    fn hello() -> Document {
        let mut doc = Document::new();
        doc.add_page(PageSize::A4)
            .unwrap()
            .add_text(Pt(72.0), Pt(769.89), &helvetica(), Pt(12.0), "Hello, world")
            .unwrap();
        doc
    }

    /// Shows the bytes line by line, escaping everything outside printable ASCII.
    /// Update with `INSTA_UPDATE=always cargo test` or `cargo insta review`.
    fn readable(bytes: &[u8]) -> String {
        bytes
            .split(|&b| b == b'\n')
            .map(|line| line.escape_ascii().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn writes_a_minimal_pdf() {
        insta::assert_snapshot!(readable(&hello().to_bytes().unwrap()));
    }

    #[test]
    fn xref_offsets_point_at_their_objects() {
        let mut doc = hello();
        for _ in 0..2 {
            doc.add_page(PageSize::LETTER)
                .unwrap()
                .add_text(Pt(10.0), Pt(10.0), &helvetica(), Pt(9.5), "(a) \\ b")
                .unwrap();
        }
        let pdf = doc.to_bytes().unwrap();
        let text = String::from_utf8_lossy(&pdf);

        let startxref: usize = text
            .rsplit_once("startxref\n")
            .and_then(|(_, tail)| tail.lines().next())
            .and_then(|line| line.parse().ok())
            .unwrap();
        assert!(pdf[startxref..].starts_with(b"xref\n0 10\n"));

        let entries = &pdf[startxref + b"xref\n0 10\n".len()..];
        assert!(entries.starts_with(b"0000000000 65535 f\r\n"));
        for object in 1..10 {
            let entry = &entries[object * 20..(object + 1) * 20];
            assert!(entry.ends_with(b" 00000 n\r\n"), "{}", entry.escape_ascii());
            let offset: usize = std::str::from_utf8(&entry[..10]).unwrap().parse().unwrap();
            assert!(
                pdf[offset..].starts_with(format!("{object} 0 obj\n").as_bytes()),
                "object {object} is not at {offset}"
            );
        }
        assert!(entries[10 * 20..].starts_with(b"trailer\n<< /Size 10 /Root 1 0 R >>"));
    }

    #[test]
    fn stream_length_counts_the_content_exactly() {
        let pdf = hello().to_bytes().unwrap();
        let text = String::from_utf8_lossy(&pdf);
        let (_, after) = text.split_once("/Length ").unwrap();
        let (length, after) = after.split_once(" >>\nstream\n").unwrap();
        let length: usize = length.parse().unwrap();
        assert!(after[length..].starts_with("\nendstream\n"));
    }

    #[test]
    fn escapes_strings() {
        let mut out = Vec::new();
        push_string(&mut out, b"a(b)c\\d\x80\xFF\x07");
        assert_eq!(out, b"(a\\(b\\)c\\\\d\\200\\377\\007)");
    }

    #[test]
    fn formats_numbers_without_exponents() {
        assert_eq!(number(0.0), "0");
        assert_eq!(number(-0.00001), "0");
        assert_eq!(number(12.0), "12");
        assert_eq!(number(100.0), "100");
        assert_eq!(number(595.275_590_551), "595.2756");
        assert_eq!(number(-3.5), "-3.5");
        assert_eq!(number(1e-7), "0");
        assert_eq!(number(1e20), "100000000000000000000");
    }

    #[test]
    fn rejects_invalid_page_sizes() {
        let mut doc = Document::new();
        for (w, h) in [
            (2.9, 100.0),
            (100.0, 14400.1),
            (f64::NAN, 100.0),
            (100.0, f64::INFINITY),
        ] {
            let err = doc.add_page(PageSize::new(Pt(w), Pt(h))).unwrap_err();
            assert!(matches!(err, Error::InvalidPageSize { .. }), "{err:?}");
        }
        assert!(doc.add_page(PageSize::new(Pt(3.0), Pt(14400.0))).is_ok());
    }

    #[test]
    fn rejects_invalid_text_arguments_and_leaves_the_page_unchanged() {
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        for size in [0.0, 0.009, -1.0, 32767.1, 1e39, f64::NAN, f64::INFINITY] {
            let err = page
                .add_text(Pt(0.0), Pt(0.0), &helvetica(), Pt(size), "x")
                .unwrap_err();
            assert!(matches!(err, Error::InvalidFontSize(_)), "{err:?}");
        }
        for (x, y) in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (1e19, 0.0),
            (0.0, -32767.1),
        ] {
            let err = page
                .add_text(Pt(x), Pt(y), &helvetica(), Pt(10.0), "x")
                .unwrap_err();
            assert!(matches!(err, Error::InvalidPosition { .. }), "{err:?}");
        }
        let err = page
            .add_text(Pt(0.0), Pt(0.0), &helvetica(), Pt(10.0), "x\ny")
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::Unencodable {
                    ch: '\n',
                    offset: 1
                }
            ),
            "{err:?}"
        );
        assert!(page.content.is_empty());
    }

    #[test]
    fn accepts_the_extremes_of_the_ranges() {
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        page.add_text(Pt(-32767.0), Pt(32767.0), &helvetica(), Font::MAX_SIZE, "x")
            .unwrap();
        page.add_text(Pt(0.0), Pt(0.0), &helvetica(), Font::MIN_SIZE, "x")
            .unwrap();
        let pdf = String::from_utf8_lossy(&doc.to_bytes().unwrap()).into_owned();
        assert!(pdf.contains("/F1 32767 Tf\n-32767 32767 Td"), "{pdf}");
        assert!(pdf.contains("/F1 0.01 Tf\n0 0 Td"), "{pdf}");
    }

    #[test]
    fn refuses_to_write_a_document_without_pages() {
        assert!(matches!(Document::new().to_bytes(), Err(Error::NoPages)));
    }

    #[test]
    fn saves_the_same_bytes_as_to_bytes() {
        let dir = std::env::temp_dir().join(format!("officewright-pdf-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello.pdf");
        hello().save(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), hello().to_bytes().unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_the_path_it_cannot_write() {
        let path = std::env::temp_dir().join("officewright-missing-dir/x/out.pdf");
        let err = hello().save(&path).unwrap_err();
        assert_eq!(err.to_string(), format!("cannot write {}", path.display()));
        assert!(std::error::Error::source(&err).is_some());
    }
}
