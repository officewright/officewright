use std::io::Write;
use std::path::Path;
use std::{fs, io};

use officewright_core::{PageSize, Pt};

use officewright_core::Color;

use crate::embed::{EXTRA_OBJECTS, Prepared, Usage};
use crate::font::check_font_size;
use crate::stream::stream_object;
use crate::{Compression, Error, Font, GlyphRun, GraphicsPath, Result, Stroke};

/// Pages smaller or larger than this are outside ISO 32000-1, Annex C.
const PAGE_SIDE: std::ops::RangeInclusive<f64> = 3.0..=14400.0;

/// Positions stay within this many points of the origin: 32767 is the largest real number
/// every PDF 1.4 reader has to handle (PDF Reference 1.4, Appendix C), far beyond the
/// largest page.
pub(crate) const MAX_POSITION: f64 = 32767.0;

/// The largest byte offset the 10-digit fields of a cross-reference table can hold.
const MAX_XREF_OFFSET: u64 = 9_999_999_999;

/// A PDF document being built.
///
/// A `Document` can be moved between threads; it has no interior mutability.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pages: Vec<Page>,
    compression: Compression,
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
    /// How many clipping paths are in effect: the `q` operators of push_clip that no
    /// pop_clip has closed yet.
    clips: usize,
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
    /// A run of glyphs that has been checked.
    Glyphs {
        /// The index of the run's font in the page's fonts.
        font: usize,
        run: GlyphRun,
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
            clips: 0,
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

    /// Sets how the file stores page content and embedded fonts:
    /// [`Compression::Flate`] unless set otherwise.
    ///
    /// ```
    /// use officewright_pdf::{Compression, Font, PageSize, Pt, StandardFont, text_to_pdf};
    ///
    /// let helvetica = Font::standard(StandardFont::Helvetica);
    /// let mut doc = text_to_pdf(&"Hello, world\n".repeat(40), PageSize::A4, &helvetica, Pt(11.0))?;
    /// let compressed = doc.to_bytes()?;
    /// doc.set_compression(Compression::None);
    /// assert!(doc.to_bytes()?.len() > compressed.len());
    /// # Ok::<(), officewright_pdf::Error>(())
    /// ```
    pub fn set_compression(&mut self, compression: Compression) {
        self.compression = compression;
    }

    /// How the file stores page content and embedded fonts.
    pub fn compression(&self) -> Compression {
        self.compression
    }

    /// Writes the document as a PDF file to `out`.
    ///
    /// Embedded fonts are reduced to the glyphs the document shows now. Nothing is
    /// written to `out` unless the whole document can be. The file is PDF 1.4, or PDF 1.5
    /// if some text needs replacement text for text extraction: where two characters share
    /// a glyph of an embedded font, or a cluster of a glyph run cannot be given by the
    /// ToUnicode map.
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
            for piece in &page.content {
                let (Content::Text { font, .. } | Content::Glyphs { font, .. }) = piece else {
                    continue;
                };
                // reason: `map` has an entry for each of the page's fonts, which are the
                // only fonts its texts and runs use
                #[allow(clippy::indexing_slicing)]
                let font = map[*font];
                let (Some(font), Some(usage)) = (fonts.get(font), usages.get_mut(font)) else {
                    continue;
                };
                match piece {
                    Content::Text { text, .. } if font.embedded().is_some() => {
                        let lookup = font.lookup();
                        for (offset, ch) in text.char_indices() {
                            usage.add(lookup.glyph(ch, offset)?.id, ch.encode_utf8(&mut [0; 4]));
                        }
                    }
                    Content::Glyphs { run, .. } => run.record(usage),
                    _ => {}
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
        let mut actual_text = false;
        let mut contents = Vec::with_capacity(self.pages.len());
        for (page, map) in self.pages.iter().zip(&page_fonts) {
            let (content, spans) = page.content_stream(map, &encoders)?;
            contents.push(content);
            actual_text |= spans;
        }

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
                    font_objects.extend(prepared.objects(next_object, self.compression));
                    next_object += EXTRA_OBJECTS;
                }
            }
        }

        let mut out = CountingWriter::new(out);
        let mut offsets = Vec::with_capacity(next_object);

        // Replacement text in marked content needs PDF 1.5 (§14.9.4); everything else
        // PDF 1.4. The comment's bytes above 127 tell transfer tools the file is binary
        // (§7.5.2).
        let version = if actual_text { "1.5" } else { "1.4" };
        out.put(format!("%PDF-{version}\n").as_bytes())?;
        out.put(b"%\xE2\xE3\xCF\xD3\n")?;

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
            out.put_object(page_id + 1, &stream_object("", content, self.compression))?;
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
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// The most clipping paths that can be in effect at once. Each is saved with the
    /// graphics state, which every PDF reader can nest 28 deep (PDF Reference 1.4,
    /// Appendix C), and drawing takes one more level.
    pub const MAX_CLIPS: usize = 27;

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
        check_position(x, y)?;
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

    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// Draws a run of glyphs at the positions it gives them.
    ///
    /// This is how text that a layout engine has shaped is drawn: the run says which
    /// glyphs of an embedded font go where and which text each cluster of them stands
    /// for, and nothing is measured, kerned or refused by script here. A run without
    /// clusters draws nothing. See [`GlyphRun`] for an example.
    ///
    /// # Errors
    ///
    /// - [`Error::NotEmbedded`] if the run's font is a standard font.
    /// - [`Error::InvalidGlyph`] if a glyph ID is not in the font, and
    ///   [`Error::EmptyCluster`] if a cluster has no glyphs.
    /// - [`Error::InvalidFontSize`] for a size outside [`Font::MIN_SIZE`] to
    ///   [`Font::MAX_SIZE`]; [`Error::InvalidPosition`] if the origin, a glyph's offset
    ///   or a glyph's position is outside -32767 to 32767 pt; and [`Error::OutOfRange`]
    ///   for a horizontal scale outside 0.01 to 10, a skew outside -1 to 1, a stroke width
    ///   outside 0 to 32767 pt, or a cluster's text longer than 16382 UTF-16 code units.
    ///
    /// On error the page is unchanged.
    pub fn draw_glyphs(&mut self, run: &GlyphRun) -> Result<()> {
        run.check()?;
        if run.glyphs().is_empty() {
            return Ok(());
        }
        let font = self.font_index(run.font());
        self.content.push(Content::Glyphs {
            font,
            run: run.clone(),
        });
        Ok(())
    }

    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// Fills the inside of `path` with `color`, by the nonzero winding number rule.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidPosition`] if a point of the path is outside -32767 to 32767 pt.
    /// On error the page is unchanged.
    pub fn fill_path(&mut self, path: &GraphicsPath, color: Color) -> Result<()> {
        path.check()?;
        let mut operators = format!("q\n{} rg\n", crate::path::color(color));
        path.write(&mut operators);
        operators.push_str("f\nQ\n");
        self.push_operators(operators.as_bytes());
        Ok(())
    }

    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// Strokes the outline of `path` as `stroke` says. Open subpaths get the stroke's caps
    /// at their ends, closed ones joins at every corner.
    ///
    /// ```
    /// use officewright_pdf::{Color, Document, GraphicsPath, PageSize, Pt, Stroke};
    ///
    /// // A frame around a box of text, 0.75 pt wide in grey.
    /// let mut doc = Document::new();
    /// let frame = GraphicsPath::rect(Pt(70.0), Pt(690.0), Pt(200.0), Pt(30.0));
    /// doc.add_page(PageSize::A4)?
    ///     .stroke_path(&frame, &Stroke::new(Pt(0.75), Color::rgb(128, 128, 128)))?;
    /// # Ok::<(), officewright_pdf::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::InvalidPosition`] if a point of the path is outside -32767 to 32767 pt;
    /// [`Error::OutOfRange`] if the line width, a dash length or the dash phase is outside
    /// 0 to 32767 pt or the dash pattern has more than 8191 lengths; [`Error::InvalidDash`]
    /// if all its lengths are 0. On error the page is unchanged.
    pub fn stroke_path(&mut self, path: &GraphicsPath, stroke: &Stroke) -> Result<()> {
        path.check()?;
        stroke.check()?;
        let mut operators = String::from("q\n");
        stroke.write(&mut operators);
        path.write(&mut operators);
        operators.push_str("S\nQ\n");
        self.push_operators(operators.as_bytes());
        Ok(())
    }

    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// Clips everything drawn on the page from now on to the inside of `path`, by the
    /// nonzero winding number rule, until [`pop_clip`](Self::pop_clip) removes it. Clipping
    /// paths nest: what is drawn shows only inside all of those in effect. The end of the
    /// page removes those still in effect.
    ///
    /// ```
    /// use officewright_pdf::{Color, Document, GraphicsPath, PageSize, Pt};
    ///
    /// let mut doc = Document::new();
    /// let page = doc.add_page(PageSize::A4)?;
    /// // Only the left half of the square is drawn.
    /// page.push_clip(&GraphicsPath::rect(Pt(100.0), Pt(100.0), Pt(50.0), Pt(100.0)))?;
    /// page.fill_path(&GraphicsPath::rect(Pt(100.0), Pt(100.0), Pt(100.0), Pt(100.0)), Color::BLACK)?;
    /// page.pop_clip()?;
    /// # Ok::<(), officewright_pdf::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::InvalidPosition`] if a point of the path is outside -32767 to 32767 pt, and
    /// [`Error::TooManyClips`] if [`MAX_CLIPS`](Self::MAX_CLIPS) clipping paths are in
    /// effect already. On error the page is unchanged.
    pub fn push_clip(&mut self, path: &GraphicsPath) -> Result<()> {
        path.check()?;
        if self.clips >= Self::MAX_CLIPS {
            return Err(Error::TooManyClips);
        }
        let mut operators = String::from("q\n");
        path.write(&mut operators);
        operators.push_str("W n\n");
        self.push_operators(operators.as_bytes());
        self.clips += 1;
        Ok(())
    }

    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// Removes the clipping path added last by [`push_clip`](Self::push_clip).
    ///
    /// # Errors
    ///
    /// [`Error::NoClip`] if no clipping path is in effect.
    pub fn pop_clip(&mut self) -> Result<()> {
        if self.clips == 0 {
            return Err(Error::NoClip);
        }
        self.push_operators(b"Q\n");
        self.clips -= 1;
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

    /// Writes the content stream, encoding the text, and says whether it needed
    /// replacement text. `map` gives the document's index of each of the page's fonts,
    /// and `encoders` the document's fonts.
    fn content_stream(&self, map: &[usize], encoders: &[Encoder<'_>]) -> Result<(Vec<u8>, bool)> {
        let mut actual_text = false;
        let mut stream = Vec::new();
        for piece in &self.content {
            match piece {
                Content::Operators(operators) => stream.extend_from_slice(operators),
                Content::Text { font, text } => {
                    // reason: `font` came from font_index, a page's fonts are never removed,
                    // and `map` and `encoders` cover every font of the document
                    #[allow(clippy::indexing_slicing)]
                    let encoder = &encoders[map[*font]];
                    actual_text |= encoder.show(&mut stream, text)?;
                }
                Content::Glyphs { font, run } => {
                    // reason: as for text
                    #[allow(clippy::indexing_slicing)]
                    let encoder = &encoders[map[*font]];
                    match encoder {
                        Encoder::Embedded(_, prepared) => {
                            actual_text |= run.write(&mut stream, font + 1, prepared);
                        }
                        // draw_glyphs refuses runs in a standard font.
                        Encoder::Standard(font) => {
                            return Err(Error::NotEmbedded {
                                font: font.name().to_owned(),
                            });
                        }
                    }
                }
            }
        }
        // The clipping paths still in effect end with the page.
        for _ in 0..self.clips {
            stream.extend_from_slice(b"Q\n");
        }
        Ok((stream, actual_text))
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
    /// Writes `text` with the operators that show it, and says whether it needed
    /// replacement text.
    fn show(&self, out: &mut Vec<u8>, text: &str) -> Result<bool> {
        match self {
            Self::Standard(font) => {
                push_string(out, &font.encode(text)?);
                out.extend_from_slice(b" Tj");
                Ok(false)
            }
            Self::Embedded(font, prepared) => prepared.show(out, &font.lookup(), text),
        }
    }
}

/// Fails unless (`x`, `y`) is within -32767 to 32767 pt of the origin.
pub(crate) fn check_position(x: Pt, y: Pt) -> Result<()> {
    let positions = -MAX_POSITION..=MAX_POSITION;
    if positions.contains(&x.0) && positions.contains(&y.0) {
        Ok(())
    } else {
        Err(Error::InvalidPosition { x, y })
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
    fn writes_a_compressed_pdf() {
        // The same bytes on every platform: CI runs this on Linux, macOS and Windows.
        let text = "Hello, world\n".repeat(10);
        let doc = crate::text_to_pdf(&text, PageSize::A4, &helvetica(), Pt(12.0)).unwrap();
        let pdf = doc.to_bytes().unwrap();
        assert!(String::from_utf8_lossy(&pdf).contains("/Filter /FlateDecode"));
        insta::assert_snapshot!(readable(&pdf));
    }

    #[test]
    fn xref_offsets_point_at_their_objects() {
        let mut doc = hello();
        // Enough text on the first page for its content stream to be compressed.
        for line in 0..30 {
            let y = Pt(700.0 - 14.0 * f64::from(line));
            doc.page_mut(0)
                .unwrap()
                .add_text(Pt(72.0), y, &helvetica(), Pt(12.0), "Hello, world")
                .unwrap();
        }
        for _ in 0..2 {
            doc.add_page(PageSize::LETTER)
                .unwrap()
                .add_text(Pt(10.0), Pt(10.0), &helvetica(), Pt(9.5), "(a) \\ b")
                .unwrap();
        }
        let pdf = doc.to_bytes().unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert_eq!(text.matches("/Filter /FlateDecode").count(), 1);

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
    fn compressed_content_decompresses_to_the_uncompressed_content() {
        /// The dictionary and data of object 5, the first page's content stream.
        fn content(pdf: &[u8]) -> (String, Vec<u8>) {
            let find = |from: usize, what: &[u8]| {
                from + pdf[from..]
                    .windows(what.len())
                    .position(|w| w == what)
                    .unwrap()
            };
            let dictionary = find(0, b"\n5 0 obj\n") + b"\n5 0 obj\n".len();
            let data = find(dictionary, b"\nstream\n") + b"\nstream\n".len();
            let dictionary = String::from_utf8(pdf[dictionary..data - 8].to_vec()).unwrap();
            let (_, length) = dictionary.split_once("/Length ").unwrap();
            let length: usize = length.split(' ').next().unwrap().parse().unwrap();
            assert!(pdf[data + length..].starts_with(b"\nendstream\nendobj\n"));
            (dictionary, pdf[data..data + length].to_vec())
        }

        let text = "Hello, world\n".repeat(40);
        let mut doc = crate::text_to_pdf(&text, PageSize::A4, &helvetica(), Pt(11.0)).unwrap();
        assert_eq!(doc.compression(), Compression::Flate);
        let compressed = doc.to_bytes().unwrap();
        doc.set_compression(Compression::None);
        let uncompressed = doc.to_bytes().unwrap();

        let (dictionary, data) = content(&compressed);
        assert_eq!(
            dictionary,
            format!("<< /Length {} /Filter /FlateDecode >>", data.len())
        );
        let (dictionary, plain) = content(&uncompressed);
        assert_eq!(dictionary, format!("<< /Length {} >>", plain.len()));
        assert_eq!(
            miniz_oxide::inflate::decompress_to_vec_zlib(&data).unwrap(),
            plain
        );
        assert!(
            data.len() * 5 < plain.len(),
            "{} of {}",
            data.len(),
            plain.len()
        );
        assert!(!String::from_utf8_lossy(&uncompressed).contains("/Filter"));
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
        // Uncompressed, to read the content streams.
        doc.set_compression(Compression::None);
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
        doc.set_compression(Compression::None);
        let pdf = String::from_utf8_lossy(&doc.to_bytes().unwrap()).into_owned();
        assert!(pdf.contains("/F1 32767 Tf\n-32767 32767 Td"), "{pdf}");
        assert!(pdf.contains("/F1 0.01 Tf\n0 0 Td"), "{pdf}");
    }

    /// The content stream of the only page of `doc`, uncompressed.
    fn content(doc: &mut Document) -> String {
        doc.set_compression(Compression::None);
        let pdf = String::from_utf8_lossy(&doc.to_bytes().unwrap()).into_owned();
        let (_, stream) = pdf.split_once("stream\n").unwrap();
        let (content, _) = stream.split_once("\nendstream").unwrap();
        content.to_owned()
    }

    #[test]
    fn fills_and_strokes_paths_in_a_state_of_their_own() {
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        let mut triangle = GraphicsPath::new(Pt(10.0), Pt(20.0));
        triangle.line_to(Pt(30.0), Pt(20.0)).close();
        page.fill_path(&triangle, Color::rgb(255, 0, 0)).unwrap();
        page.stroke_path(
            &GraphicsPath::rect(Pt(1.0), Pt(2.0), Pt(3.0), Pt(4.0)),
            &Stroke::new(Pt(0.5), Color::rgb(0, 0, 255)).with_dash(&[Pt(2.0)], Pt(1.0)),
        )
        .unwrap();
        // Text after the shapes is black again: their colours ended with their Q.
        page.add_text(Pt(72.0), Pt(700.0), &helvetica(), Pt(12.0), "x")
            .unwrap();
        assert_eq!(
            content(&mut doc),
            "q\n1 0 0 rg\n10 20 m\n30 20 l\nh\nf\nQ\n\
             q\n0.5 w\n[2] 1 d\n0 0 1 RG\n1 2 3 4 re\nS\nQ\n\
             BT\n/F1 12 Tf\n72 700 Td\n(x) Tj\nET\n"
        );
    }

    #[test]
    fn clips_until_popped_and_closes_the_rest_at_the_end_of_the_page() {
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        let clip = GraphicsPath::rect(Pt(0.0), Pt(0.0), Pt(100.0), Pt(50.0));
        page.push_clip(&clip).unwrap();
        page.fill_path(&clip, Color::BLACK).unwrap();
        page.pop_clip().unwrap();
        assert!(matches!(page.pop_clip(), Err(Error::NoClip)));
        page.push_clip(&clip).unwrap();
        page.push_clip(&clip).unwrap();
        page.pop_clip().unwrap();
        let content = content(&mut doc);
        assert_eq!(
            content,
            "q\n0 0 100 50 re\nW n\nq\n0 0 0 rg\n0 0 100 50 re\nf\nQ\nQ\n\
             q\n0 0 100 50 re\nW n\nq\n0 0 100 50 re\nW n\nQ\n\
             Q\n"
        );
        // Every q has its Q.
        let operators: Vec<&str> = content.split_whitespace().collect();
        let saves = operators.iter().filter(|&&op| op == "q").count();
        assert_eq!(saves, operators.iter().filter(|&&op| op == "Q").count());
    }

    #[test]
    fn nests_at_most_max_clips_clipping_paths() {
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        let clip = GraphicsPath::rect(Pt(0.0), Pt(0.0), Pt(100.0), Pt(50.0));
        for _ in 0..Page::MAX_CLIPS {
            page.push_clip(&clip).unwrap();
        }
        let before = page.content.len();
        assert!(matches!(page.push_clip(&clip), Err(Error::TooManyClips)));
        assert_eq!(page.content.len(), before);
        // The deepest drawing still fits in the 28 levels every reader nests.
        page.fill_path(&clip, Color::BLACK).unwrap();
        let mut depth: usize = 0;
        let mut deepest = 0;
        for op in content(&mut doc).split_whitespace() {
            match op {
                "q" => depth += 1,
                "Q" => depth -= 1,
                _ => {}
            }
            deepest = deepest.max(depth);
        }
        assert_eq!((deepest, depth), (28, 0));
        assert_eq!(
            Error::TooManyClips.to_string(),
            "more than 27 clipping paths would be in effect at once"
        );
    }

    #[test]
    fn rejects_invalid_shapes_and_leaves_the_page_unchanged() {
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        let far = GraphicsPath::new(Pt(0.0), Pt(40000.0));
        let fine = GraphicsPath::rect(Pt(0.0), Pt(0.0), Pt(1.0), Pt(1.0));
        let solid = Stroke::new(Pt(1.0), Color::BLACK);
        let position = |result: Result<()>| matches!(result, Err(Error::InvalidPosition { .. }));
        assert!(position(page.fill_path(&far, Color::BLACK)));
        assert!(position(page.stroke_path(&far, &solid)));
        assert!(position(page.push_clip(&far)));
        assert!(matches!(
            page.stroke_path(&fine, &Stroke::new(Pt(-1.0), Color::BLACK)),
            Err(Error::OutOfRange {
                what: "line width",
                ..
            })
        ));
        assert!(matches!(
            page.stroke_path(&fine, &solid.clone().with_dash(&[Pt(0.0)], Pt(0.0))),
            Err(Error::InvalidDash)
        ));
        assert!(page.content.is_empty());
        assert_eq!(page.clips, 0);
        assert_eq!(
            Error::InvalidPosition {
                x: Pt(0.0),
                y: Pt(40000.0)
            }
            .to_string(),
            "position (0, 40000) pt is outside the -32767 to 32767 pt range"
        );
    }

    #[test]
    fn glyph_runs_need_an_embedded_font() {
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        let mut run = GlyphRun::new(&helvetica(), Pt(12.0), Pt(0.0), Pt(0.0));
        run.push_cluster(
            "A",
            [crate::PlacedGlyph::new(
                crate::GlyphId::new(65),
                Pt(0.0),
                Pt(0.0),
            )],
        );
        let err = page.draw_glyphs(&run).unwrap_err();
        assert_eq!(
            err.to_string(),
            "glyphs can be drawn only in an embedded font, and Helvetica is a standard font"
        );
        assert!(page.content.is_empty() && page.fonts.is_empty());
    }

    #[test]
    fn new_errors_are_invalid_arguments() {
        for err in [
            Error::NotEmbedded {
                font: "Helvetica".to_owned(),
            },
            Error::InvalidGlyph {
                glyph: 7,
                font: "F".to_owned(),
            },
            Error::EmptyCluster { cluster: 0 },
            Error::OutOfRange {
                what: "skew",
                value: 2.0,
                min: -1.0,
                max: 1.0,
            },
            Error::InvalidDash,
            Error::NoClip,
            Error::TooManyClips,
        ] {
            let message = err.to_string();
            assert!(
                matches!(
                    officewright_core::Error::from(err),
                    officewright_core::Error::InvalidArgument(m) if m == message
                ),
                "{message}"
            );
        }
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
