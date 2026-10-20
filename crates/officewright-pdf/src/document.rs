use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs, io};

use officewright_core::{PageSize, Pt};
use officewright_font::SubsetBudget;

use officewright_core::Color;

use crate::embed::{Codes, EXTRA_OBJECTS, Prepared, show};
use crate::font::check_font_size;
use crate::glyphs::MAX_STRING_BYTES;
use crate::image::draw_operators;
use crate::path::check_range;
use crate::stream::stream_object;
use crate::{Compression, Error, Font, GlyphRun, GraphicsPath, Image, Result, Stroke};

/// Pages smaller or larger than this are outside ISO 32000-1, Annex C.
const PAGE_SIDE: std::ops::RangeInclusive<f64> = 3.0..=14400.0;

/// Positions stay within this many points of the origin: 32767 is the largest real number
/// every PDF 1.4 reader has to handle (PDF Reference 1.4, Appendix C), far beyond the
/// largest page.
pub(crate) const MAX_POSITION: f64 = 32767.0;

/// The smallest side of an image on a page: smaller ones would round to 0 in the four
/// decimals the file stores, and an image of no size cannot be drawn.
const MIN_IMAGE_SIDE: f64 = 0.01;

/// The largest byte offset the 10-digit fields of a cross-reference table can hold.
const MAX_XREF_OFFSET: u64 = 9_999_999_999;

/// What reducing the embedded fonts of a document may read of their CFF charstrings in
/// all, unless set otherwise. Every subset is kept until the file is written, so without a
/// budget for all of them, a document with nine crafted fonts, each within the limit of one
/// subset (2^24 units, `MAX_STEPS` in officewright-font's `cff.rs`), would keep eight
/// subsets of about 82 MB while the subsetter takes about 490 MB for the ninth (#277). The
/// subsetter spends about 30 bytes for each unit while it reduces a font, and a subset
/// keeps about 5 bytes for each; with this budget the subsets of a document take at most
/// about 600 MB while they are made, and the subsetter's copy of the local subroutines of
/// every Font DICT, which the units do not count, up to about 267 MB more in one run
/// (#284). It leaves room for two CJK fonts the size of Noto Sans CJK SC kept whole
/// (16,082,053 units each); larger ones are past the limit of one subset anyway, and a
/// document reduced to the glyphs it uses reads far less.
const SUBSET_BUDGET: u64 = 1 << 25;

/// A PDF document being built.
///
/// A `Document` can be moved between threads; it has no interior mutability.
#[derive(Debug)]
pub struct Document {
    /// Tells this document's pages from those another document encoded.
    id: u64,
    pages: Vec<Page>,
    compression: Compression,
    subset_budget: u64,
    /// The CIDs of the embedded fonts' glyphs that the encoded pages show.
    codes: FontCodes,
    /// The page handed out last, by `add_page` or `page_mut`: the only page whose text
    /// and glyph runs may not be encoded yet.
    open: Option<usize>,
}

/// The identity of the next document made or cloned.
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

impl Default for Document {
    fn default() -> Self {
        Self {
            id: next_id(),
            pages: Vec::new(),
            compression: Compression::default(),
            subset_budget: SUBSET_BUDGET,
            codes: FontCodes::default(),
            open: None,
        }
    }
}

/// A page of a [`Document`].
///
/// A page belongs to its document. Once the document has handed out another page, the
/// text drawn on this one in an embedded font is encoded with the document's own glyph
/// codes, and a copy of the page put into another document, or the page swapped into one,
/// makes that document's [`write_to`](Document::write_to) fail with
/// [`Error::ForeignPage`]. A page that has only text in a standard font, paths and images,
/// or whose text is not encoded yet, can go into another document.
#[derive(Clone, Debug)]
pub struct Page {
    size: PageSize,
    /// The fonts the page uses, in the order of first use; the first is the page's font
    /// resource `/F1`.
    fonts: Vec<Font>,
    /// The images the page draws, in the order of first use; the first is the page's
    /// image resource `/Im1`.
    images: Vec<Image>,
    /// The content stream: operators, followed by the text and glyph runs drawn since the
    /// page was last encoded and the operators drawn after them. The document encodes
    /// a page when it hands out another one, so that only the page being drawn keeps text
    /// and runs, which take far more memory than the operators that show them; the
    /// glyphs are then given CIDs in the order they are drawn.
    content: Vec<Content>,
    /// How many clipping paths are in effect: the `q` operators of push_clip that no
    /// pop_clip has closed yet.
    clips: usize,
    /// Whether some text encoded so far needed replacement text.
    actual_text: bool,
    /// The document whose glyph codes the page's encoded text uses, once it has some.
    encoded_by: Option<u64>,
}

/// A piece of a page's content stream.
#[derive(Clone, Debug)]
enum Content {
    /// Operators, ready to write.
    Operators(Vec<u8>),
    /// Text in an embedded font that has been checked against it, to show with one string
    /// operator.
    Text {
        /// The font's index in the page's fonts.
        font: usize,
        text: String,
        /// The glyph of each character.
        glyphs: Vec<u16>,
    },
    /// A run of glyphs that has been checked.
    Glyphs {
        /// The index of the run's font in the page's fonts.
        font: usize,
        run: GlyphRun,
    },
}

impl Clone for Document {
    /// A copy that goes on by itself: its pages are its own, encoded with its own copy of
    /// the glyph codes.
    fn clone(&self) -> Self {
        let id = next_id();
        let mut pages = self.pages.clone();
        for page in &mut pages {
            if page.encoded_by == Some(self.id) {
                page.encoded_by = Some(id);
            }
        }
        Self {
            id,
            pages,
            compression: self.compression,
            subset_budget: self.subset_budget,
            codes: self.codes.clone(),
            open: self.open,
        }
    }
}

impl Document {
    /// Creates a document with no pages.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a page and returns it. The page belongs to the document ([`Page`]).
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
        self.encode_open_page();
        let index = self.pages.len();
        self.pages.push(Page {
            size,
            fonts: Vec::new(),
            images: Vec::new(),
            content: Vec::new(),
            clips: 0,
            actual_text: false,
            encoded_by: None,
        });
        self.open = Some(index);
        #[allow(clippy::indexing_slicing)] // reason: the page was just pushed at `index`
        let page = &mut self.pages[index];
        Ok(page)
    }

    /// The number of pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The page at `index`, counting from 0 in the order the pages were added. Like the
    /// page [`add_page`](Self::add_page) returns, it belongs to the document ([`Page`]).
    pub fn page_mut(&mut self, index: usize) -> Option<&mut Page> {
        // The page being drawn stays as it is until another is: a caller that asks for it
        // for each thing it draws, as the C ABI does, encodes it once.
        if self.open != Some(index) {
            self.encode_open_page();
            if index < self.pages.len() {
                self.open = Some(index);
            }
        }
        self.pages.get_mut(index)
    }

    /// Encodes the text and glyph runs of the page handed out last.
    fn encode_open_page(&mut self) {
        if let Some(page) = self.open.take().and_then(|index| self.pages.get_mut(index)) {
            page.encode(&mut self.codes, self.id);
        }
    }

    /// Sets how the file stores page content, embedded fonts, and the samples and soft
    /// masks of images decoded from PNG files: [`Compression::Flate`] unless set
    /// otherwise. JPEG files, and PNG files that go into the PDF as they are, keep their
    /// own compression either way.
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

    /// How the file stores page content, embedded fonts, and the samples and soft masks of
    /// images decoded from PNG files.
    pub fn compression(&self) -> Compression {
        self.compression
    }

    /// Sets how much reducing the embedded fonts to the glyphs the document shows may read
    /// of their CFF charstrings in all, in operands, operators and hint mask bytes
    /// ([`SubsetBudget`](officewright_font::SubsetBudget)): 33,554,432 unless set
    /// otherwise, room for two CJK fonts the size of Noto Sans CJK SC kept whole. The fonts
    /// are reduced when the document is written, one after another in the order the pages
    /// first use them, and each subset is kept until the file is: a unit costs about 30
    /// bytes of memory while its font is reduced and about 5 bytes after. Fonts with
    /// TrueType outlines take nothing from the budget.
    ///
    /// ```
    /// use officewright_pdf::Document;
    ///
    /// let mut doc = Document::new();
    /// assert_eq!(doc.subset_budget(), 33_554_432);
    /// doc.set_subset_budget(1 << 20);
    /// assert_eq!(doc.subset_budget(), 1 << 20);
    /// ```
    pub fn set_subset_budget(&mut self, units: u64) {
        self.subset_budget = units;
    }

    /// How much reducing the embedded fonts may read of their CFF charstrings in all; see
    /// [`set_subset_budget`](Self::set_subset_budget).
    pub fn subset_budget(&self) -> u64 {
        self.subset_budget
    }

    /// Writes the document as a PDF file to `out`.
    ///
    /// Embedded fonts are reduced to the glyphs the document shows now, unless a font's
    /// license forbids subsetting
    /// ([`EmbeddingNote::WholeFont`](crate::EmbeddingNote::WholeFont)). Nothing is written
    /// to `out` unless the whole document can be. The file is PDF 1.4, or PDF 1.5
    /// if some text needs replacement text for text extraction (where two characters share
    /// a glyph of an embedded font, or a cluster of a glyph run cannot be given by the
    /// ToUnicode map), or if an image keeps 16-bit samples: a PNG file of 16 bits per
    /// sample without interlacing or transparency, which goes into the PDF as it is.
    ///
    /// # Errors
    ///
    /// [`Error::NoPages`] if the document has no pages, [`Error::Embed`] if an embedded
    /// font turns out to be damaged when it is reduced, or its glyphs the document uses
    /// are past a limit of the library or past what is left of the
    /// [subset budget](Self::set_subset_budget) after the fonts reduced before it,
    /// [`Error::TooLarge`] if the file
    /// would exceed 10<sup>10</sup> bytes, [`Error::ForeignPage`] for a page that another
    /// document encoded ([`Page`]), and [`Error::Io`] if writing fails.
    pub fn write_to(&self, out: impl Write) -> Result<()> {
        if self.pages.is_empty() {
            return Err(Error::NoPages);
        }
        if let Some(page) = self
            .pages
            .iter()
            .position(|page| page.encoded_by.is_some_and(|id| id != self.id))
        {
            return Err(Error::ForeignPage { page });
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
        // The same for images.
        let mut images: Vec<&Image> = Vec::new();
        let page_images: Vec<Vec<usize>> = self
            .pages
            .iter()
            .map(|page| {
                page.images
                    .iter()
                    .map(
                        |image| match images.iter().position(|known| *known == image) {
                            Some(index) => index,
                            None => {
                                images.push(image);
                                images.len() - 1
                            }
                        },
                    )
                    .collect()
            })
            .collect();

        // Everything is prepared before anything is written, so an error leaves `out`
        // untouched. First the text and runs of the page drawn last, which give the last
        // glyphs their CIDs.
        let mut codes = self.codes.clone();
        // Only the page handed out last can have them, but every page that has is encoded.
        let mut encoded = Vec::new();
        let mut actual_text = false;
        for (i, page) in self.pages.iter().enumerate() {
            actual_text |= page.actual_text;
            if page.content.iter().any(Content::is_pending) {
                let mut stream = Vec::new();
                actual_text |= encode(&page.content, &page.fonts, &mut codes, &mut stream);
                encoded.push((i, stream));
            }
        }
        // The fonts share one budget, since all their subsets are kept until the file is
        // written.
        let entries: Vec<Option<usize>> = fonts.iter().map(|font| codes.entry(font)).collect();
        let mut budget = SubsetBudget::new(self.subset_budget);
        let prepared = fonts
            .iter()
            .zip(&entries)
            .map(|(font, entry)| match (font.embedded(), entry) {
                (Some(embedded), Some(entry)) => {
                    Prepared::new(embedded, codes.get(*entry), &mut budget).map(Some)
                }
                _ => Ok(None),
            })
            .collect::<Result<Vec<_>>>()?;

        // Objects: 1 catalog, 2 page tree, then a font dictionary per font, then a page
        // and its content stream per page, then the other objects of each embedded font,
        // then the objects of each image.
        let font_object = |i: usize| 3 + i;
        let first_page = 3 + fonts.len();
        let page_object = |i: usize| first_page + 2 * i;
        let mut next_object = first_page + 2 * self.pages.len();
        let mut font_dictionaries = Vec::with_capacity(fonts.len());
        let mut font_objects = Vec::new();
        for (font, prepared) in fonts.iter().zip(&prepared) {
            match prepared {
                None => {
                    font_dictionaries.push(font.standard_dictionary().unwrap_or_default().to_vec());
                }
                Some(prepared) => {
                    font_dictionaries.push(prepared.dictionary(next_object));
                    font_objects.extend(prepared.objects(next_object, self.compression));
                    next_object += EXTRA_OBJECTS;
                }
            }
        }
        // The images' objects are only numbered here: making them cannot fail, so each is
        // made as it is written, and the images are never all copied at once.
        let mut image_objects = Vec::with_capacity(images.len());
        for image in &images {
            image_objects.push(next_object);
            next_object += image.object_count();
        }

        let mut out = CountingWriter::new(out);
        let mut offsets = Vec::with_capacity(next_object);

        // Replacement text in marked content (§14.9.4) and 16-bit image samples (§8.9.5.1)
        // need PDF 1.5; everything else PDF 1.4. The comment's bytes above 127 tell
        // transfer tools the file is binary (§7.5.2).
        let pdf_15 = actual_text || images.iter().any(|image| image.needs_pdf_15());
        let version = if pdf_15 { "1.5" } else { "1.4" };
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

        for (i, ((page, fonts), images)) in self
            .pages
            .iter()
            .zip(&page_fonts)
            .zip(&page_images)
            .enumerate()
        {
            let page_id = page_object(i);
            let fonts: Vec<String> = fonts
                .iter()
                .enumerate()
                .map(|(resource, &font)| format!("/F{} {} 0 R", resource + 1, font_object(font)))
                .collect();
            let images: Vec<String> = images
                .iter()
                .enumerate()
                .filter_map(|(resource, &image)| {
                    let object = image_objects.get(image)?;
                    Some(format!("/Im{} {object} 0 R", resource + 1))
                })
                .collect();
            let mut resources = Vec::new();
            if !fonts.is_empty() {
                resources.push(format!("/Font << {} >>", fonts.join(" ")));
            }
            if !images.is_empty() {
                resources.push(format!("/XObject << {} >>", images.join(" ")));
            }
            let resources = if resources.is_empty() {
                "<< >>".to_owned()
            } else {
                format!("<< {} >>", resources.join(" "))
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

            // One page's content stream at a time.
            let content = match encoded.iter().find(|(page, _)| *page == i) {
                Some((_, stream)) => page.content_stream(stream),
                None => page.content_stream(&[]),
            };
            offsets.push(out.written);
            out.put_object(page_id + 1, &stream_object("", &content, self.compression))?;
        }

        let mut id = first_page + 2 * self.pages.len();
        for object in &font_objects {
            offsets.push(out.written);
            out.put_object(id, object)?;
            id += 1;
        }
        for (image, &first) in images.iter().zip(&image_objects) {
            for object in image.objects(first, self.compression) {
                offsets.push(out.written);
                out.put_object(id, &object)?;
                id += 1;
            }
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
        let mut glyphs = Vec::new();
        for (offset, ch) in text.char_indices() {
            glyphs.push(lookup.glyph(ch, offset)?.id);
        }
        // Text in a standard font needs no CIDs, and is encoded at once.
        let standard = match font.embedded() {
            None => Some(font.encode(text)?),
            Some(_) => None,
        };
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
        match standard {
            Some(encoded) => {
                let mut operators = Vec::new();
                show_standard(&mut operators, &encoded);
                self.push_operators(&operators);
            }
            None => self.content.push(Content::Text {
                font,
                text: text.to_owned(),
                glyphs,
            }),
        }
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
    /// - [`Error::InvalidGlyph`] if a glyph ID is not in the font,
    ///   [`Error::EmptyCluster`] if a cluster has no glyphs, and [`Error::ClusterTooLong`]
    ///   if a cluster's text has more than 16382 UTF-16 code units.
    /// - [`Error::InvalidFontSize`] for a size outside [`Font::MIN_SIZE`] to
    ///   [`Font::MAX_SIZE`]; [`Error::InvalidPosition`] if the origin, a glyph's offset
    ///   or a glyph's position is outside -32767 to 32767 pt; and [`Error::OutOfRange`]
    ///   for a horizontal scale outside 0.01 to 10, a skew outside -1 to 1, or a stroke
    ///   width outside 0 to 32767 pt.
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
    /// Draws `image` stretched over the rectangle with its lower-left corner at (`x`, `y`)
    /// and the given width and height, in points. The image's own resolution is ignored;
    /// a document embeds each image once however often its pages draw it.
    ///
    /// ```
    /// use officewright_pdf::{Document, Image, PageSize, Pt};
    ///
    /// # let png = &b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0\x90wS\xde\
    /// #     \0\0\0\x0cIDATx\xdac\xf8\xcf\xc0\0\0\x03\x01\x01\0\xf7\x03AC\
    /// #     \0\0\0\0IEND\xaeB`\x82"[..];
    /// let image = Image::from_bytes(png)?;
    /// let mut doc = Document::new();
    /// let page = doc.add_page(PageSize::A4)?;
    /// // 2 inches wide, keeping the image's proportions.
    /// let width = 144.0;
    /// let height = width * f64::from(image.height()) / f64::from(image.width());
    /// page.draw_image(&image, Pt(72.0), Pt(600.0), Pt(width), Pt(height))?;
    /// # Ok::<(), officewright_pdf::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::OutOfRange`] if the width or height is outside 0.01 to 32767 pt, and
    /// [`Error::InvalidPosition`] if a corner of the rectangle is outside -32767 to 32767
    /// pt. On error the page is unchanged.
    pub fn draw_image(&mut self, image: &Image, x: Pt, y: Pt, width: Pt, height: Pt) -> Result<()> {
        check_range("image width", width.0, MIN_IMAGE_SIDE, MAX_POSITION)?;
        check_range("image height", height.0, MIN_IMAGE_SIDE, MAX_POSITION)?;
        check_position(x, y)?;
        check_position(Pt(x.0 + width.0), Pt(y.0 + height.0))?;
        let resource = match self.images.iter().position(|known| known == image) {
            Some(index) => index,
            None => {
                self.images.push(image.clone());
                self.images.len() - 1
            }
        };
        let operators = draw_operators(resource + 1, x.0, y.0, width.0, height.0);
        self.push_operators(operators.as_bytes());
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

    /// Encodes the text and glyph runs drawn since the page was last encoded, giving their
    /// glyphs CIDs in `codes`, those of document `document`.
    fn encode(&mut self, codes: &mut FontCodes, document: u64) {
        // A page another document encoded keeps its text and runs as they were drawn: the
        // document whose codes its encoded text uses can still encode them, and any other
        // refuses the page when it is written.
        if self.encoded_by.is_some_and(|id| id != document) {
            return;
        }
        let Some(first) = self.content.iter().position(Content::is_pending) else {
            return;
        };
        self.encoded_by = Some(document);
        let pending: Vec<Content> = self.content.drain(first..).collect();
        // Operators that come one after another are kept as one piece.
        let mut stream = match self.content.pop() {
            Some(Content::Operators(operators)) => operators,
            Some(other) => {
                self.content.push(other);
                Vec::new()
            }
            None => Vec::new(),
        };
        self.actual_text |= encode(&pending, &self.fonts, codes, &mut stream);
        // The page is left: what it keeps until the file is written is no larger than it
        // needs to be.
        stream.shrink_to_fit();
        self.content.push(Content::Operators(stream));
    }

    /// The content stream: the page's operators, or `encoded` for a page whose text and
    /// runs `write_to` encoded itself, and the end of the clipping paths still in effect.
    fn content_stream(&self, encoded: &[u8]) -> Vec<u8> {
        let mut stream = Vec::new();
        if self.content.iter().any(Content::is_pending) {
            stream.extend_from_slice(encoded);
        } else {
            for piece in &self.content {
                if let Content::Operators(operators) = piece {
                    stream.extend_from_slice(operators);
                }
            }
        }
        for _ in 0..self.clips {
            stream.extend_from_slice(b"Q\n");
        }
        stream
    }
}

impl Content {
    /// Whether the piece is text or a run that has not been encoded yet.
    fn is_pending(&self) -> bool {
        !matches!(self, Self::Operators(_))
    }
}

/// Writes `pieces` of the content of a page whose fonts are `fonts` to `out`, encoding
/// their text and glyph runs with the CIDs of `codes`, which gives CIDs to the glyphs
/// shown for the first time, and says whether some text needed replacement text.
fn encode(pieces: &[Content], fonts: &[Font], codes: &mut FontCodes, out: &mut Vec<u8>) -> bool {
    let mut actual_text = false;
    // Where the codes of each of the page's fonts are, found once: finding them compares
    // the font with the document's, which compares the font data of two fonts loaded
    // from the same bytes.
    let mut entries: Vec<Option<Option<usize>>> = vec![None; fonts.len()];
    for piece in pieces {
        let (font, text) = match piece {
            Content::Operators(operators) => {
                out.extend_from_slice(operators);
                continue;
            }
            Content::Text { font, .. } | Content::Glyphs { font, .. } => (*font, piece),
        };
        // Text and runs are kept only in embedded fonts, which have codes.
        let Some((embedded, entry)) = fonts.get(font).and_then(|of_page| {
            let entry = *entries
                .get_mut(font)?
                .get_or_insert_with(|| codes.entry(of_page));
            Some((of_page.embedded()?, entry?))
        }) else {
            continue;
        };
        let codes = codes.get_mut(entry);
        match text {
            Content::Text { text, glyphs, .. } => actual_text |= show(out, codes, text, glyphs),
            Content::Glyphs { run, .. } => {
                run.record(codes);
                actual_text |= run.write(out, font + 1, embedded, codes);
            }
            Content::Operators(_) => {}
        }
    }
    actual_text
}

/// The CIDs of each embedded font of a document.
#[derive(Clone, Debug, Default)]
struct FontCodes(Vec<(Font, Codes)>);

impl FontCodes {
    /// Where the codes of `font` are, added on its first use; `None` for a standard font.
    fn entry(&mut self, font: &Font) -> Option<usize> {
        let embedded = font.embedded()?;
        match self.0.iter().position(|(known, _)| known == font) {
            Some(index) => Some(index),
            None => {
                self.0.push((font.clone(), Codes::new(embedded.whole)));
                Some(self.0.len() - 1)
            }
        }
    }

    fn get(&self, entry: usize) -> &Codes {
        // reason: entries come from `entry`, and none is ever removed
        #[allow(clippy::indexing_slicing)]
        &self.0[entry].1
    }

    fn get_mut(&mut self, entry: usize) -> &mut Codes {
        // reason: as for `get`
        #[allow(clippy::indexing_slicing)]
        &mut self.0[entry].1
    }
}

/// Writes text in a standard font, `encoded` with one byte a character, with the operators
/// that show it.
fn show_standard(out: &mut Vec<u8>, encoded: &[u8]) {
    // Long text is shown with several strings, each within the 32767 bytes readers have
    // to handle (ISO 32000-1, Annex C).
    let mut pieces = encoded.chunks(MAX_STRING_BYTES).peekable();
    if pieces.peek().is_none() {
        out.extend_from_slice(b"() Tj");
    }
    for (i, piece) in pieces.enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        push_string(out, piece);
        out.extend_from_slice(b" Tj");
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

    /// The literal strings of `content`, decoded: escaped characters and octal codes are
    /// one byte each.
    fn literal_strings(content: &[u8]) -> Vec<Vec<u8>> {
        let mut strings = Vec::new();
        let mut bytes = content.iter().copied();
        while let Some(byte) = bytes.next() {
            if byte != b'(' {
                continue;
            }
            let mut string = Vec::new();
            while let Some(byte) = bytes.next() {
                match byte {
                    b')' => break,
                    b'\\' => match bytes.next().unwrap() {
                        digit @ b'0'..=b'7' => {
                            let rest = [bytes.next().unwrap(), bytes.next().unwrap()];
                            let octal = [digit, rest[0], rest[1]].map(|d| d - b'0');
                            string.push(octal[0] << 6 | octal[1] << 3 | octal[2]);
                        }
                        escaped => string.push(escaped),
                    },
                    _ => string.push(byte),
                }
            }
            strings.push(string);
        }
        strings
    }

    #[test]
    fn shows_long_text_with_strings_readers_handle() {
        // 70,000 characters, some of them escaped in the string: more than the 65,535
        // bytes of a string in PDF 1.4 and the 32,767 of PDF 1.5.
        let text = "ab(c\u{e9}".repeat(14_000);
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        page.add_text(Pt(72.0), Pt(700.0), &helvetica(), Pt(12.0), &text)
            .unwrap();
        page.add_text(Pt(72.0), Pt(680.0), &helvetica(), Pt(12.0), "")
            .unwrap();
        doc.set_compression(Compression::None);
        let pdf = doc.to_bytes().unwrap();
        let content = String::from_utf8_lossy(&pdf);
        let strings = literal_strings(&pdf);
        let lengths: Vec<usize> = strings.iter().map(Vec::len).collect();
        // Three strings for the long text, at the limit, and one for the empty text.
        assert_eq!(lengths, [32767, 32767, 4466, 0]);
        assert_eq!(strings.concat(), helvetica().encode(&text).unwrap());
        assert_eq!(content.matches(" Tj").count(), 4, "{content}");
        assert!(content.contains("680 Td\n() Tj\nET"), "{content}");
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
            Error::ClusterTooLong {
                cluster: 1,
                units: 20000,
            },
            Error::OutOfRange {
                what: "skew",
                value: 2.0,
                min: -1.0,
                max: 1.0,
            },
            Error::InvalidDash,
            Error::NoClip,
            Error::TooManyClips,
            Error::ForeignPage { page: 2 },
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
    fn draws_each_image_once_for_the_pages_that_use_it() {
        use crate::image::tests::Png;
        let png = Image::from_bytes(Png::new(3, 2, 2, 8).encode()).unwrap();
        // With an alpha channel: an image and its soft mask.
        let alpha = Image::from_bytes(Png::new(2, 2, 6, 8).encode()).unwrap();
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        page.draw_image(&alpha, Pt(10.0), Pt(20.0), Pt(30.0), Pt(40.0))
            .unwrap();
        page.draw_image(&png, Pt(0.5), Pt(0.0), Pt(1.25), Pt(2.0))
            .unwrap();
        page.add_text(Pt(72.0), Pt(700.0), &helvetica(), Pt(12.0), "x")
            .unwrap();
        // The same image, read again, on a second page.
        let again = Image::from_bytes(Png::new(3, 2, 2, 8).encode()).unwrap();
        doc.add_page(PageSize::A4)
            .unwrap()
            .draw_image(&again, Pt(0.0), Pt(0.0), Pt(100.0), Pt(100.0))
            .unwrap();
        doc.set_compression(Compression::None);
        let pdf = String::from_utf8_lossy(&doc.to_bytes().unwrap()).into_owned();
        // 1 catalog, 2 page tree, 3 Helvetica, 4 and 6 the pages, 5 and 7 their content,
        // 8 the image with alpha and 9 its mask, 10 the other image.
        assert!(pdf.starts_with("%PDF-1.4\n"));
        assert!(
            object(&pdf, 4).contains(
                "/Resources << /Font << /F1 3 0 R >> /XObject << /Im1 8 0 R /Im2 10 0 R >> >>"
            ),
            "{pdf}"
        );
        assert!(
            object(&pdf, 6).contains("/Resources << /XObject << /Im1 10 0 R >> >>"),
            "{pdf}"
        );
        assert!(
            object(&pdf, 5).contains(
                "q\n30 0 0 40 10 20 cm\n/Im1 Do\nQ\nq\n1.25 0 0 2 0.5 0 cm\n/Im2 Do\nQ\nBT\n"
            ),
            "{pdf}"
        );
        assert!(object(&pdf, 7).contains("q\n100 0 0 100 0 0 cm\n/Im1 Do\nQ\n"));
        assert!(object(&pdf, 8).contains("/SMask 9 0 R"), "{pdf}");
        assert!(object(&pdf, 9).contains("/ColorSpace /DeviceGray"), "{pdf}");
        assert!(object(&pdf, 10).contains("/Predictor 15"), "{pdf}");
        assert_eq!(pdf.matches("/Subtype /Image").count(), 3);
        assert!(pdf.contains("xref\n0 11\n"));
    }

    #[test]
    fn an_image_drawn_twice_on_a_page_is_one_resource() {
        use crate::image::tests::Png;
        let image = Image::from_bytes(Png::new(2, 2, 0, 8).encode()).unwrap();
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        for x in [0.0, 50.0] {
            page.draw_image(&image, Pt(x), Pt(0.0), Pt(10.0), Pt(10.0))
                .unwrap();
        }
        doc.set_compression(Compression::None);
        let pdf = String::from_utf8_lossy(&doc.to_bytes().unwrap()).into_owned();
        assert!(
            object(&pdf, 3).contains("/Resources << /XObject << /Im1 5 0 R >> >>"),
            "{pdf}"
        );
        assert_eq!(pdf.matches("/Im1 Do").count(), 2);
        assert_eq!(pdf.matches("/Subtype /Image").count(), 1);
    }

    #[test]
    fn sixteen_bit_images_make_the_file_pdf_1_5() {
        use crate::image::tests::Png;
        let mut doc = Document::new();
        let image = Image::from_bytes(Png::new(2, 2, 0, 16).encode()).unwrap();
        doc.add_page(PageSize::A4)
            .unwrap()
            .draw_image(&image, Pt(0.0), Pt(0.0), Pt(10.0), Pt(10.0))
            .unwrap();
        assert!(doc.to_bytes().unwrap().starts_with(b"%PDF-1.5\n"));
    }

    #[test]
    fn rejects_image_rectangles_out_of_range_and_leaves_the_page_unchanged() {
        use crate::image::tests::Png;
        let image = Image::from_bytes(Png::new(1, 1, 0, 8).encode()).unwrap();
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        let cases = [
            (
                (0.0, 0.0, 0.0, 1.0),
                "image width 0 is outside the 0.01 to 32767 range",
            ),
            (
                (0.0, 0.0, 1.0, -1.0),
                "image height -1 is outside the 0.01 to 32767 range",
            ),
            (
                (0.0, 0.0, f64::NAN, 1.0),
                "image width NaN is outside the 0.01 to 32767 range",
            ),
            (
                (0.0, 0.0, 32767.5, 1.0),
                "image width 32767.5 is outside the 0.01 to 32767 range",
            ),
            (
                (f64::INFINITY, 0.0, 1.0, 1.0),
                "position (inf, 0) pt is outside the -32767 to 32767 pt range",
            ),
            (
                (32767.0, 0.0, 1.0, 1.0),
                "position (32768, 1) pt is outside the -32767 to 32767 pt range",
            ),
            // Wider than high: the far corner is x + width, y + height.
            (
                (32000.0, 0.0, 1000.0, 1.0),
                "position (33000, 1) pt is outside the -32767 to 32767 pt range",
            ),
        ];
        for ((x, y, width, height), message) in cases {
            let err = page
                .draw_image(&image, Pt(x), Pt(y), Pt(width), Pt(height))
                .unwrap_err();
            assert_eq!(err.to_string(), message);
        }
        assert!(page.content.is_empty() && page.images.is_empty());
        page.draw_image(&image, Pt(-32767.0), Pt(0.0), Pt(0.01), Pt(32767.0))
            .unwrap();
    }

    #[test]
    fn image_errors_have_their_kinds() {
        let malformed = Error::MalformedImage {
            path: None,
            reason: "x".to_owned(),
        };
        assert!(matches!(
            officewright_core::Error::from(malformed),
            officewright_core::Error::Malformed(m) if m == "image data is damaged: x"
        ));
        let unsupported = Error::UnsupportedImage {
            path: Some("a.gif".into()),
            reason: "y".to_owned(),
        };
        assert!(matches!(
            officewright_core::Error::from(unsupported),
            officewright_core::Error::Unsupported(m) if m == "image a.gif cannot be used: y"
        ));
        assert!(matches!(
            officewright_core::Error::from(Error::ImageTooLarge { path: None }),
            officewright_core::Error::LimitExceeded(_)
        ));
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
