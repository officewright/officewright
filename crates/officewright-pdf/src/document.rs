use std::io::Write;
use std::path::Path;
use std::{fs, io};

use officewright_core::{PageSize, Pt};

use crate::embed::{EXTRA_OBJECTS, Prepared, Usage};
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
    /// The fonts the page uses, in the order of first use; the first is the page's font
    /// resource `/F1`.
    fonts: Vec<Font>,
    /// The content stream. Text stays text until the document is written, when the text
    /// of every page is known.
    content: Vec<Content>,
}

/// A piece of a page's content stream.
#[derive(Clone, Debug)]
enum Content {
    /// Operators, ready to write.
    Operators(Vec<u8>),
    /// Text that has been checked against its font, to show with one string operator.
    Text {
        /// The font's index in the page's fonts.
        font: usize,
        text: String,
    },
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
            fonts: Vec::new(),
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
    /// Embedded fonts are reduced to the glyphs the document shows now. Nothing is
    /// written to `out` unless the whole document can be.
    ///
    /// # Errors
    ///
    /// [`Error::NoPages`] if the document has no pages, [`Error::Embed`] if an embedded
    /// font turns out to be damaged when it is reduced, [`Error::TooLarge`] if the file
    /// would exceed 10<sup>10</sup> bytes, and [`Error::Io`] if writing fails.
    pub fn write_to(&self, out: impl Write) -> Result<()> {
        if self.pages.is_empty() {
            return Err(Error::NoPages);
        }
        // Each font is written once, in the order the pages first use it. For each page,
        // `page_fonts` has the document's index of each of its font resources.
        let mut fonts: Vec<&Font> = Vec::new();
        let page_fonts: Vec<Vec<usize>> = self
            .pages
            .iter()
            .map(|page| {
                page.fonts
                    .iter()
                    .map(|font| match fonts.iter().position(|known| *known == font) {
                        Some(index) => index,
                        None => {
                            fonts.push(font);
                            fonts.len() - 1
                        }
                    })
                    .collect()
            })
            .collect();

        // Everything is prepared before anything is written, so an error leaves `out`
        // untouched. First the glyphs each embedded font shows, over all pages.
        let mut usages = vec![Usage::default(); fonts.len()];
        for (page, map) in self.pages.iter().zip(&page_fonts) {
            for (font, text) in page.texts() {
                // reason: `map` has an entry for each of the page's fonts, which are the
                // only fonts its texts use
                #[allow(clippy::indexing_slicing)]
                let font = map[font];
                if let (Some(font), Some(usage)) = (fonts.get(font), usages.get_mut(font)) {
                    if font.embedded().is_some() {
                        let lookup = font.lookup();
                        for (offset, ch) in text.char_indices() {
                            usage.add(lookup.glyph(ch, offset)?.id, ch);
                        }
                    }
                }
            }
        }
        let encoders = fonts
            .iter()
            .zip(usages)
            .map(|(&font, usage)| match font.embedded() {
                Some(embedded) => Ok(Encoder::Embedded(font, Prepared::new(embedded, usage)?)),
                None => Ok(Encoder::Standard(font)),
            })
            .collect::<Result<Vec<_>>>()?;
        let contents = self
            .pages
            .iter()
            .zip(&page_fonts)
            .map(|(page, map)| page.content_stream(map, &encoders))
            .collect::<Result<Vec<_>>>()?;

        // Objects: 1 catalog, 2 page tree, then a font dictionary per font, then a page
        // and its content stream per page, then the other objects of each embedded font.
        let font_object = |i: usize| 3 + i;
        let first_page = 3 + fonts.len();
        let page_object = |i: usize| first_page + 2 * i;
        let mut next_object = first_page + 2 * self.pages.len();
        let mut font_dictionaries = Vec::with_capacity(encoders.len());
        let mut font_objects = Vec::new();
        for encoder in &encoders {
            match encoder {
                Encoder::Standard(font) => {
                    font_dictionaries.push(font.standard_dictionary().unwrap_or_default().to_vec());
                }
                Encoder::Embedded(_, prepared) => {
                    font_dictionaries.push(prepared.dictionary(next_object));
                    font_objects.extend(prepared.objects(next_object));
                    next_object += EXTRA_OBJECTS;
                }
            }
        }

        let mut out = CountingWriter::new(out);
        let mut offsets = Vec::with_capacity(next_object);

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

        for (i, dictionary) in font_dictionaries.iter().enumerate() {
            offsets.push(out.written);
            out.put_object(font_object(i), dictionary)?;
        }

        for (i, ((page, resources), content)) in self
            .pages
            .iter()
            .zip(&page_fonts)
            .zip(&contents)
            .enumerate()
        {
            let page_id = page_object(i);
            let fonts: Vec<String> = resources
                .iter()
                .enumerate()
                .map(|(resource, &font)| format!("/F{} {} 0 R", resource + 1, font_object(font)))
                .collect();
            let resources = if fonts.is_empty() {
                "<< >>".to_owned()
            } else {
                format!("<< /Font << {} >> >>", fonts.join(" "))
            };
            offsets.push(out.written);
            out.put(
                format!(
                    "{page_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] \
                     /Resources {resources} /Contents {} 0 R >>\nendobj\n",
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
                    content.len()
                )
                .as_bytes(),
            )?;
            out.put(content)?;
            // The end-of-line before `endstream` is not part of /Length (§7.3.8.1).
            out.put(b"\nendstream\nendobj\n")?;
        }

        for (i, object) in font_objects.iter().enumerate() {
            offsets.push(out.written);
            out.put_object(first_page + 2 * self.pages.len() + i, object)?;
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
    /// Each character is drawn with its own glyph, one after the other, without
    /// kerning, ligatures or other shaping. With an embedded font that is right for the
    /// Latin, Greek, Cyrillic, Armenian, Georgian, Han, Hiragana and Katakana scripts,
    /// precomposed Hangul syllables, and the punctuation, digits and symbols every script
    /// shares; combining marks are drawn where the font places them. Text in other
    /// scripts needs shaping or right-to-left layout and is refused rather than drawn
    /// wrong.
    ///
    /// # Errors
    ///
    /// - [`Error::Unencodable`] for a standard font if `text` has a character
    ///   WinAnsiEncoding lacks: anything but Latin text, including control characters
    ///   such as `\n` and `\t`.
    /// - [`Error::UnsupportedCharacter`] for an embedded font if `text` has a character
    ///   of another script, a bidirectional formatting character, a conjoining Hangul
    ///   jamo, a control character, or an unassigned or private-use code point.
    /// - [`Error::MissingGlyph`] if an embedded font has no glyph for a character.
    /// - [`Error::InvalidFontSize`] for a size outside [`Font::MIN_SIZE`] to
    ///   [`Font::MAX_SIZE`], and [`Error::InvalidPosition`] for a position outside
    ///   -32767 to 32767 pt.
    ///
    /// On error the page is unchanged.
    pub fn add_text(&mut self, x: Pt, y: Pt, font: &Font, font_size: Pt, text: &str) -> Result<()> {
        check_font_size(font_size)?;
        let positions = -MAX_POSITION..=MAX_POSITION;
        if !positions.contains(&x.0) || !positions.contains(&y.0) {
            return Err(Error::InvalidPosition { x, y });
        }
        let lookup = font.lookup();
        for (offset, ch) in text.char_indices() {
            lookup.glyph(ch, offset)?;
        }
        let font = self.font_index(font);
        self.push_operators(
            format!(
                "BT\n/F{} {} Tf\n{} {} Td\n",
                font + 1,
                number(font_size.0),
                number(x.0),
                number(y.0)
            )
            .as_bytes(),
        );
        self.content.push(Content::Text {
            font,
            text: text.to_owned(),
        });
        self.push_operators(b"\nET\n");
        Ok(())
    }

    /// The index of `font` in the page's fonts, adding it on its first use. The font at
    /// index `i` is the page's font resource `/F(i + 1)`.
    fn font_index(&mut self, font: &Font) -> usize {
        match self.fonts.iter().position(|known| known == font) {
            Some(index) => index,
            None => {
                self.fonts.push(font.clone());
                self.fonts.len() - 1
            }
        }
    }

    fn push_operators(&mut self, operators: &[u8]) {
        match self.content.last_mut() {
            Some(Content::Operators(last)) => last.extend_from_slice(operators),
            _ => self.content.push(Content::Operators(operators.to_vec())),
        }
    }

    /// The page's texts, each with the index of its font in the page's fonts.
    fn texts(&self) -> impl Iterator<Item = (usize, &str)> {
        self.content.iter().filter_map(|piece| match piece {
            Content::Text { font, text } => Some((*font, text.as_str())),
            Content::Operators(_) => None,
        })
    }

    /// Writes the content stream, encoding the text. `map` gives the document's index
    /// of each of the page's fonts, and `encoders` the document's fonts.
    fn content_stream(&self, map: &[usize], encoders: &[Encoder<'_>]) -> Result<Vec<u8>> {
        let mut stream = Vec::new();
        for piece in &self.content {
            match piece {
                Content::Operators(operators) => stream.extend_from_slice(operators),
                Content::Text { font, text } => {
                    // reason: `font` came from font_index, a page's fonts are never removed,
                    // and `map` and `encoders` cover every font of the document
                    #[allow(clippy::indexing_slicing)]
                    let encoder = &encoders[map[*font]];
                    encoder.show(&mut stream, text)?;
                }
            }
        }
        Ok(stream)
    }
}

/// How a document's font turns text into a content stream's strings.
enum Encoder<'a> {
    /// A standard font: a literal string in its encoding.
    Standard(&'a Font),
    /// An embedded font: two-byte CIDs of its subset.
    Embedded(&'a Font, Prepared<'a>),
}

impl Encoder<'_> {
    /// Writes `text` with the operators that show it.
    fn show(&self, out: &mut Vec<u8>, text: &str) -> Result<()> {
        match self {
            Self::Standard(font) => {
                push_string(out, &font.encode(text)?);
                out.extend_from_slice(b" Tj");
                Ok(())
            }
            Self::Embedded(font, prepared) => prepared.show(out, &font.lookup(), text),
        }
    }
}

/// Formats a finite number as a PDF real: at most four decimals, no exponent.
pub(crate) fn number(value: f64) -> String {
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

    /// Writes indirect object `id` with `body`, a dictionary or a stream.
    fn put_object(&mut self, id: usize, body: &[u8]) -> io::Result<()> {
        self.put(format!("{id} 0 obj\n").as_bytes())?;
        self.put(body)?;
        self.put(b"\nendobj\n")
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
        assert!(page.fonts.is_empty());
    }

    /// The object `id` of `pdf`, from `obj` to `endobj`.
    fn object(pdf: &str, id: usize) -> &str {
        let start = pdf.find(&format!("\n{id} 0 obj\n")).unwrap() + 1;
        let end = start + pdf[start..].find("endobj\n").unwrap();
        &pdf[start..end]
    }

    #[test]
    fn writes_each_font_once_for_the_pages_that_use_it() {
        let mut doc = Document::new();
        doc.add_page(PageSize::A4)
            .unwrap()
            .add_text(Pt(72.0), Pt(700.0), &helvetica(), Pt(12.0), "one")
            .unwrap();
        doc.add_page(PageSize::A4).unwrap();
        let page = doc.add_page(PageSize::A4).unwrap();
        for (y, text) in [(700.0, "three"), (680.0, "again")] {
            page.add_text(Pt(72.0), Pt(y), &helvetica(), Pt(12.0), text)
                .unwrap();
        }
        let pdf = String::from_utf8_lossy(&doc.to_bytes().unwrap()).into_owned();

        // 1 catalog, 2 page tree, 3 Helvetica, then each page and its content stream.
        assert_eq!(pdf.matches("/Type /Font").count(), 1, "{pdf}");
        assert!(object(&pdf, 3).contains("/BaseFont /Helvetica"), "{pdf}");
        assert!(
            object(&pdf, 2).contains("/Kids [4 0 R 6 0 R 8 0 R]"),
            "{pdf}"
        );
        for page in [4, 8] {
            assert!(
                object(&pdf, page).contains("/Resources << /Font << /F1 3 0 R >> >>"),
                "{pdf}"
            );
        }
        assert!(object(&pdf, 6).contains("/Resources << >>"), "{pdf}");
        assert!(
            object(&pdf, 9).contains(
                "BT\n/F1 12 Tf\n72 700 Td\n(three) Tj\nET\nBT\n/F1 12 Tf\n72 680 Td\n(again) Tj\nET\n"
            ),
            "{pdf}"
        );
    }

    #[test]
    fn writes_nothing_when_text_cannot_be_encoded() {
        // add_text rejects such text, so it is planted directly.
        let mut doc = hello();
        doc.page_mut(0).unwrap().content.push(Content::Text {
            font: 0,
            text: "Ж".to_owned(),
        });
        let mut out = Vec::new();
        let err = doc.write_to(&mut out).unwrap_err();
        assert!(
            matches!(
                err,
                Error::Unencodable {
                    ch: 'Ж', offset: 0
                }
            ),
            "{err:?}"
        );
        assert!(out.is_empty(), "{} bytes were written", out.len());
    }

    #[test]
    fn a_document_without_text_has_no_font() {
        let mut doc = Document::new();
        doc.add_page(PageSize::A4).unwrap();
        let pdf = String::from_utf8_lossy(&doc.to_bytes().unwrap()).into_owned();
        assert!(!pdf.contains("/Font"), "{pdf}");
        assert!(
            object(&pdf, 3).contains("/Resources << >> /Contents 4 0 R"),
            "{pdf}"
        );
        assert!(pdf.contains("xref\n0 5\n"), "{pdf}");
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
