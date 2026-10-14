//! Rendering into PDF: a document of the layout IR laid out on pages, and every page
//! painted with `officewright-pdf`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use officewright_core::{Color, PageSize, Pt};
use officewright_font::{FaceId, FontBook, FontOrigin};
use officewright_pdf as pdf;

use crate::display::{DisplayItem, GlyphItem, ImageItem, LineItem, LineStyle};
use crate::error::{Error, Result};
use crate::fonts::Fonts;
use crate::ir::{Document, ImageFormat, ImageId};
use crate::pages::{self, Page};
use crate::report::{LayoutReport, Location, ReportKind};
use crate::shape::Shaper;

/// The most pages a document is laid out on unless [`RenderOptions`] say otherwise.
pub const DEFAULT_MAX_PAGES: usize = 100_000;

/// The width of the stroke that draws synthetic bold, as a share of the size. Hypothesis
/// until Word's PDF of a font with one weight in bold (`E-synthetic-styles.docx`).
const SYNTHETIC_BOLD_STROKE: f64 = 1.0 / 30.0;

/// The slant of synthetic italic, as a tangent. Hypothesis, like the stroke of bold.
const SYNTHETIC_ITALIC_SKEW: f64 = 1.0 / 3.0;

/// The most half-waves a wavy line is drawn with: a longer one is drawn straight, so that
/// no line makes a path without bound.
const MAX_HALF_WAVES: f64 = 4096.0;

/// The grey of the box drawn for a picture whose format cannot be drawn.
const PLACEHOLDER: Color = Color::rgb(0xC0, 0xC0, 0xC0);

/// How a document is rendered into PDF: the most pages it may take, the fonts to lay it
/// out with besides those of the document, and how the PDF's streams are compressed.
///
/// ```
/// use officewright_layout::RenderOptions;
///
/// let options = RenderOptions::new()
///     .with_max_pages(500)
///     .with_system_fonts(false)
///     .with_font_dir("fonts");
/// assert_eq!(options.max_pages(), 500);
/// assert!(!options.system_fonts());
/// ```
#[derive(Clone, Debug)]
pub struct RenderOptions {
    max_pages: usize,
    system_fonts: bool,
    font_dirs: Vec<PathBuf>,
    fonts: Vec<Arc<[u8]>>,
    compression: pdf::Compression,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOptions {
    /// At most [`DEFAULT_MAX_PAGES`] pages, with the system's fonts, compressed with Flate.
    pub fn new() -> Self {
        Self {
            max_pages: DEFAULT_MAX_PAGES,
            system_fonts: true,
            font_dirs: Vec::new(),
            fonts: Vec::new(),
            compression: pdf::Compression::Flate,
        }
    }

    /// The most pages the document may take; past it, rendering fails with
    /// [`Error::TooManyPages`].
    pub fn with_max_pages(mut self, max_pages: usize) -> Self {
        self.max_pages = max_pages;
        self
    }

    /// Whether the fonts installed on the system are used, after those given here.
    pub fn with_system_fonts(mut self, system_fonts: bool) -> Self {
        self.system_fonts = system_fonts;
        self
    }

    /// Adds the fonts of a directory, searched before the system's.
    pub fn with_font_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.font_dirs.push(dir.into());
        self
    }

    /// Adds a font file held in memory (TrueType, OpenType or a collection), searched
    /// before the system's.
    pub fn with_font_data(mut self, data: impl Into<Arc<[u8]>>) -> Self {
        self.fonts.push(data.into());
        self
    }

    /// How the PDF's streams are compressed.
    pub fn with_compression(mut self, compression: pdf::Compression) -> Self {
        self.compression = compression;
        self
    }

    /// The most pages the document may take.
    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    /// Whether the system's fonts are used.
    pub fn system_fonts(&self) -> bool {
        self.system_fonts
    }

    /// How the PDF's streams are compressed.
    pub fn compression(&self) -> pdf::Compression {
        self.compression
    }

    /// A book of the fonts these options give: those in memory, then those of the
    /// directories in order, then the system's if they are used. A producer adds the
    /// document's own fonts to it before rendering.
    ///
    /// # Errors
    ///
    /// When a font in memory cannot be read, or a directory cannot be listed. Fonts in a
    /// directory that cannot be read are skipped, and listed by
    /// [`FontBook::skipped`].
    pub fn font_book(&self) -> officewright_font::Result<FontBook> {
        let mut book = FontBook::new();
        for data in &self.fonts {
            book.add_bytes(data.clone(), FontOrigin::User)?;
        }
        for dir in &self.font_dirs {
            book.add_dir(dir)?;
        }
        if self.system_fonts {
            book.add_system_fonts();
        }
        Ok(book)
    }
}

/// A document rendered into PDF, and the report of what the layout did differently
/// from Word.
#[derive(Debug)]
pub struct Rendered {
    pdf: pdf::Document,
    report: LayoutReport,
}

impl Rendered {
    /// The PDF, to save or write.
    pub fn pdf(&self) -> &pdf::Document {
        &self.pdf
    }

    /// What the layout did differently from Word: substitutions, omissions, clamps.
    pub fn report(&self) -> &LayoutReport {
        &self.report
    }

    /// The PDF and the report.
    pub fn into_parts(self) -> (pdf::Document, LayoutReport) {
        (self.pdf, self.report)
    }
}

/// Renders `document` into PDF with the fonts of `fonts`: checks it
/// ([`Document::validate`]), brings its values within Word's limits
/// ([`Document::clamp`]), lays it out on pages and paints every page.
///
/// # Errors
///
/// [`Error::InvalidDocument`] or [`Error::NestingTooDeep`] for a document its producer
/// built wrongly, [`Error::TooManyPages`] past the options' limit, and [`Error::Pdf`]
/// when the PDF cannot be made.
///
/// ```
/// use officewright_core::PageSize;
/// use officewright_font::FontBook;
/// use officewright_layout::ir::{Document, PageSetup, Paragraph, RunStyle, Section};
/// use officewright_layout::{RenderOptions, render_pdf};
///
/// let mut document = Document::default();
/// let style = document.add_run_style(RunStyle::default());
/// document.add_section(
///     Section::new(PageSetup::new(PageSize::A4)).with_blocks([Paragraph::new(style).into()]),
/// );
/// let rendered = render_pdf(&document, &FontBook::new(), &RenderOptions::new())?;
/// assert_eq!(rendered.pdf().page_count(), 1);
/// # Ok::<(), officewright_layout::Error>(())
/// ```
pub fn render_pdf(
    document: &Document,
    fonts: &FontBook,
    options: &RenderOptions,
) -> Result<Rendered> {
    document.validate()?;
    let mut document = document.clone();
    let mut report = document.clamp();
    let mut layout_fonts = Fonts::new(fonts);
    let mut shaper = Shaper::new();
    let laid = pages::paginate(&document, &mut layout_fonts, &mut shaper, options.max_pages)?;
    let mut painter = Painter {
        document: &document,
        fonts: &mut layout_fonts,
        pdf_fonts: HashMap::new(),
        images: HashMap::new(),
        pdf: pdf::Document::new(),
    };
    painter.pdf.set_compression(options.compression);
    for page in &laid.pages {
        painter.page(page)?;
    }
    let pdf = painter.pdf;
    report.extend(layout_fonts.into_report());
    Ok(Rendered { pdf, report })
}

/// Paints pages of display items into a PDF.
struct Painter<'a, 'f, 'b> {
    document: &'a Document,
    fonts: &'f mut Fonts<'b>,
    /// The PDF font of every face drawn so far; `None` for one that cannot be embedded.
    pdf_fonts: HashMap<FaceId, Option<pdf::Font>>,
    /// The PDF image of every picture drawn so far; `None` for one that cannot be drawn.
    images: HashMap<ImageId, Option<pdf::Image>>,
    pdf: pdf::Document,
}

impl Painter<'_, '_, '_> {
    /// Adds a page of the size of `page` and paints what it draws.
    fn page(&mut self, page: &Page) -> Result<()> {
        let height = page.height.0;
        let size = PageSize::new(page.width, page.height);
        // The display list's places are from the page's top left corner, y down; the
        // PDF's from its bottom left corner, y up.
        let flip = |y: Pt| Pt(height - y.0);
        let mut items = Vec::new();
        for item in &page.items {
            match item {
                DisplayItem::Glyphs(run) => {
                    if let Some(glyphs) = self.glyphs(run, flip) {
                        items.push(Drawn::Glyphs(glyphs));
                    }
                }
                DisplayItem::Rect {
                    at,
                    width,
                    height,
                    color,
                } => {
                    let bottom = flip(Pt(at.y.0 + height.0));
                    let path = pdf::GraphicsPath::rect(at.x, bottom, *width, *height);
                    items.push(Drawn::Fill(path, *color));
                }
                DisplayItem::Line(line) => {
                    let (path, stroke) = line_path(line, flip);
                    items.push(Drawn::Stroke(path, stroke));
                }
                DisplayItem::Image(image) => items.push(self.image(image, flip)),
            }
        }
        let pdf_page = self.pdf.add_page(size)?;
        for item in &items {
            match item {
                Drawn::Glyphs(run) => pdf_page.draw_glyphs(run)?,
                Drawn::Fill(path, color) => pdf_page.fill_path(path, *color)?,
                Drawn::Stroke(path, stroke) => pdf_page.stroke_path(path, stroke)?,
                Drawn::Image(image, at) => {
                    pdf_page.draw_image(image, at.0, at.1, at.2, at.3)?;
                }
            }
        }
        Ok(())
    }

    /// The PDF glyph run of `run`, unless its face cannot be embedded, which is
    /// reported.
    fn glyphs(&mut self, run: &GlyphItem, flip: impl Fn(Pt) -> Pt) -> Option<pdf::GlyphRun> {
        let font = self.font(run.face)?;
        let mut glyphs = pdf::GlyphRun::new(&font, run.size, run.origin.x, flip(run.origin.y))
            .with_color(run.color)
            .with_horizontal_scale(run.h_scale);
        if run.synthetic_bold {
            glyphs = glyphs.with_stroke(Pt(run.size.0 * SYNTHETIC_BOLD_STROKE));
        }
        if run.synthetic_italic {
            glyphs = glyphs.with_skew(SYNTHETIC_ITALIC_SKEW);
        }
        for cluster in &run.clusters {
            let placed = cluster
                .glyphs
                .iter()
                .map(|glyph| pdf::PlacedGlyph::new(glyph.glyph, glyph.x, glyph.y));
            glyphs.push_cluster(&cluster.text, placed);
        }
        Some(glyphs)
    }

    /// The PDF font of face `id`, made once; `None`, reported once, when it cannot be
    /// embedded.
    fn font(&mut self, id: FaceId) -> Option<pdf::Font> {
        if let Some(font) = self.pdf_fonts.get(&id) {
            return font.clone();
        }
        let face = self.fonts.face(id);
        let font = face.and_then(|face| {
            let name = face.face().names().family().unwrap_or("a font").to_owned();
            match pdf::Font::from_face(face.face().clone()) {
                Ok(font) => {
                    if font
                        .embedding_notes()
                        .contains(&pdf::EmbeddingNote::DefaultInstance)
                    {
                        self.fonts.report_once(
                            ReportKind::FeatureUnsupported,
                            &Location::default(),
                            format!(
                                "{name} is a variable font; only its default instance is drawn"
                            ),
                        );
                    }
                    Some(font)
                }
                Err(err) => {
                    self.fonts.report_once(
                        ReportKind::FontSubstituted,
                        &Location::default(),
                        format!("the text in {name} is not drawn: {err}"),
                    );
                    None
                }
            }
        });
        self.pdf_fonts.insert(id, font.clone());
        font
    }

    /// What picture `item` draws: the image, or a grey box of its size when its format
    /// cannot be drawn, which is reported. A crop, rotation or mirroring is not drawn yet,
    /// and is reported.
    fn image(&mut self, item: &ImageItem, flip: impl Fn(Pt) -> Pt) -> Drawn {
        let bottom = flip(Pt(item.at.y.0 + item.height.0));
        if item.crop.is_some() || item.rotation != 0.0 || item.flip_horizontal || item.flip_vertical
        {
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                &Location::default(),
                "cropped, turned or mirrored pictures are drawn whole and upright".to_owned(),
            );
        }
        let image = match self.images.get(&item.image) {
            Some(image) => image.clone(),
            None => {
                let image = self.load_image(item.image);
                self.images.insert(item.image, image.clone());
                image
            }
        };
        match image {
            Some(image) => Drawn::Image(image, (item.at.x, bottom, item.width, item.height)),
            None => Drawn::Fill(
                pdf::GraphicsPath::rect(item.at.x, bottom, item.width, item.height),
                PLACEHOLDER,
            ),
        }
    }

    /// The PDF image of picture `id`; `None`, reported, when its format or its data
    /// cannot be drawn.
    fn load_image(&mut self, id: ImageId) -> Option<pdf::Image> {
        let data = self.document.image(id)?;
        match data.format() {
            ImageFormat::Png | ImageFormat::Jpeg => {
                match pdf::Image::from_bytes(data.bytes().clone()) {
                    Ok(image) => Some(image),
                    Err(err) => {
                        self.fonts.report_once(
                            ReportKind::FeatureUnsupported,
                            &Location::default(),
                            format!("a picture is drawn as a grey box: {err}"),
                        );
                        None
                    }
                }
            }
            ImageFormat::Other(name) => {
                self.fonts.report_once(
                    ReportKind::FeatureUnsupported,
                    &Location::default(),
                    format!("pictures in {name} format are drawn as grey boxes"),
                );
                None
            }
        }
    }
}

/// What the painter draws on a page, made before the page is borrowed.
enum Drawn {
    Glyphs(pdf::GlyphRun),
    Fill(pdf::GraphicsPath, Color),
    Stroke(pdf::GraphicsPath, pdf::Stroke),
    /// An image, with its box: left, bottom, width and height.
    Image(pdf::Image, (Pt, Pt, Pt, Pt)),
}

/// The path and stroke of `line`: dashes as a pattern of the stroke, a wave as one path
/// of curves, each a half-wave of twice the line's width up or down, so that a dotted
/// or wavy line is one stroke whatever its length. Hypothesis until a Word reference
/// (`E-underline.docx`): the lengths of dashes, dots and waves.
fn line_path(line: &LineItem, flip: impl Fn(Pt) -> Pt) -> (pdf::GraphicsPath, pdf::Stroke) {
    let width = line.width.0.max(0.0);
    let (x0, y0) = (line.from.x.0, flip(line.from.y).0);
    let (x1, y1) = (line.to.x.0, flip(line.to.y).0);
    let stroke = pdf::Stroke::new(line.width, line.color);
    let dashes: &[f64] = match line.style {
        LineStyle::Dotted => &[1.0, 1.0],
        LineStyle::Dashed => &[4.0, 2.0],
        LineStyle::LongDashed => &[8.0, 3.0],
        LineStyle::DotDash => &[4.0, 2.0, 1.0, 2.0],
        LineStyle::DotDotDash => &[4.0, 2.0, 1.0, 2.0, 1.0, 2.0],
        LineStyle::Solid | LineStyle::Wave => &[],
    };
    let stroke = if dashes.is_empty() || width <= 0.0 {
        stroke
    } else {
        let lengths: Vec<Pt> = dashes.iter().map(|share| Pt(share * width)).collect();
        stroke.with_dash(&lengths, Pt(0.0))
    };
    let mut path = pdf::GraphicsPath::new(Pt(x0), Pt(y0));
    let half = (2.0 * width).max(0.5);
    let length = x1 - x0;
    let halves = (length.abs() / half).floor();
    let level = (y0 - y1).abs() < 1e-9;
    if line.style == LineStyle::Wave && level && (1.0..=MAX_HALF_WAVES).contains(&halves) {
        let step = length / halves;
        let mut x = x0;
        for index in 0..halves as usize {
            let up = if index % 2 == 0 { width } else { -width };
            path.curve_to(
                Pt(x + step / 3.0),
                Pt(y0 + up),
                Pt(x + 2.0 * step / 3.0),
                Pt(y0 + up),
                Pt(x + step),
                Pt(y0),
            );
            x += step;
        }
    } else {
        path.line_to(Pt(x1), Pt(y1));
    }
    (path, stroke)
}

impl From<pdf::Error> for Error {
    fn from(source: pdf::Error) -> Self {
        Error::Pdf { source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::Point;

    fn line(style: LineStyle, from: (f64, f64), to: (f64, f64)) -> LineItem {
        LineItem {
            from: Point {
                x: Pt(from.0),
                y: Pt(from.1),
            },
            to: Point {
                x: Pt(to.0),
                y: Pt(to.1),
            },
            width: Pt(0.5),
            style,
            color: Color::BLACK,
        }
    }

    /// The flip of a page 100 pt high.
    fn flip(y: Pt) -> Pt {
        Pt(100.0 - y.0)
    }

    #[test]
    fn dashes_are_patterns_of_the_stroke_and_waves_one_path() {
        let dashes = |style| {
            let (_, stroke) = line_path(&line(style, (10.0, 20.0), (60.0, 20.0)), flip);
            stroke
                .dash()
                .iter()
                .map(|length| length.0)
                .collect::<Vec<_>>()
        };
        assert_eq!(dashes(LineStyle::Solid), Vec::<f64>::new());
        assert_eq!(dashes(LineStyle::Dotted), [0.5, 0.5]);
        assert_eq!(dashes(LineStyle::Dashed), [2.0, 1.0]);
        assert_eq!(dashes(LineStyle::LongDashed), [4.0, 1.5]);
        assert_eq!(dashes(LineStyle::DotDash), [2.0, 1.0, 0.5, 1.0]);
        assert_eq!(
            dashes(LineStyle::DotDotDash),
            [2.0, 1.0, 0.5, 1.0, 0.5, 1.0]
        );
        // A wave of 50 pt in half-waves of 1 pt: one path, no dashes.
        let (wave, stroke) = line_path(&line(LineStyle::Wave, (10.0, 20.0), (60.0, 20.0)), flip);
        assert!(stroke.dash().is_empty());
        assert_ne!(
            wave,
            line_path(&line(LineStyle::Solid, (10.0, 20.0), (60.0, 20.0)), flip).0
        );
    }

    #[test]
    fn lines_are_flipped_and_waves_past_the_bound_drawn_straight() {
        // From the top 20 pt down to 80 pt up from the bottom of a page 100 pt high.
        let (path, _) = line_path(&line(LineStyle::Solid, (10.0, 20.0), (60.0, 30.0)), flip);
        let mut expected = pdf::GraphicsPath::new(Pt(10.0), Pt(80.0));
        expected.line_to(Pt(60.0), Pt(70.0));
        assert_eq!(path, expected);
        // A wave far longer than the bound of half-waves: a straight line.
        let far = line(LineStyle::Wave, (0.0, 20.0), (1e9, 20.0));
        let (path, _) = line_path(&far, flip);
        let mut straight = pdf::GraphicsPath::new(Pt(0.0), Pt(80.0));
        straight.line_to(Pt(1e9), Pt(80.0));
        assert_eq!(path, straight);
    }
}
