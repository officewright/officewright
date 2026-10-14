//! Rendering into PDF: a document of the layout IR laid out on pages, and every page
//! painted with `officewright-pdf`.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use officewright_core::{Color, PageSize, Pt};
use officewright_font::{FaceId, FontBook, FontOrigin};
use officewright_pdf as pdf;

use crate::display::{
    Cluster, DisplayItem, DrawnGlyph, GlyphItem, ImageItem, LineItem, LineStyle, Point,
};
use crate::error::{Error, Result};
use crate::fonts::Fonts;
use crate::ir::{Document, ImageFormat, ImageId};
use crate::pages::{self, Page};
use crate::report::{LayoutReport, Location, ReportKind};
use crate::shape::Shaper;

/// The most pages a document is laid out on unless [`RenderOptions`] say otherwise.
pub const DEFAULT_MAX_PAGES: usize = 100_000;

/// The width of the stroke that draws synthetic bold, as a share of the size. Hypothesis
/// until Word's PDF of a font with no bold face (a rule document still to come, #186).
const SYNTHETIC_BOLD_STROKE: f64 = 1.0 / 30.0;

/// The slant of synthetic italic, as a tangent. Hypothesis until the rasters of Word's
/// render of `E-italic-missing.docx` are measured.
const SYNTHETIC_ITALIC_SKEW: f64 = 1.0 / 3.0;

/// The most half-waves a wavy line is drawn with: a longer one is drawn straight, so that
/// no line makes a path without bound.
const MAX_HALF_WAVES: f64 = 4096.0;

/// The grey of the box drawn for a picture whose format cannot be drawn.
const PLACEHOLDER: Color = Color::rgb(0xC0, 0xC0, 0xC0);

/// The farthest a position may be from a page's corner, and the largest a picture may
/// be, in a PDF: 32767 pt, the largest real number every reader must read (ISO 32000-1,
/// Annex C), which `officewright-pdf` checks. The painter keeps within it, so that no
/// content of a document can make the PDF fail.
const PDF_LIMIT: f64 = 32767.0;

/// The smallest width or height of a picture that `officewright-pdf` draws; a smaller
/// picture shows nothing, and is not drawn.
const MIN_PICTURE: f64 = 0.01;

/// How a document is rendered into PDF: the most pages it may take, the fonts besides
/// those of the document that the book of [`font_book`](Self::font_book) holds, and how
/// the PDF's streams are compressed.
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
#[derive(Clone)]
pub struct RenderOptions {
    max_pages: usize,
    system_fonts: bool,
    font_dirs: Vec<PathBuf>,
    fonts: Vec<Arc<[u8]>>,
    compression: pdf::Compression,
}

impl fmt::Debug for RenderOptions {
    /// The options, with the length of each font in memory rather than its bytes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lengths: Vec<usize> = self.fonts.iter().map(|data| data.len()).collect();
        f.debug_struct("RenderOptions")
            .field("max_pages", &self.max_pages)
            .field("system_fonts", &self.system_fonts)
            .field("font_dirs", &self.font_dirs)
            .field("font_data_lengths", &lengths)
            .field("compression", &self.compression)
            .finish()
    }
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
/// ([`Document::clamp`]), lays it out on pages and paints every page. The text is laid
/// out in the fonts of `fonts` alone: the font settings of `options` make such a book
/// with [`RenderOptions::font_book`], to which a producer adds the document's own fonts.
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
        let items = self.drawn(page.width, page.height, &page.items);
        let pdf_page = self.pdf.add_page(PageSize::new(page.width, page.height))?;
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

    /// What `items` draw on a page `width` by `height`, in the PDF's space, and within
    /// what a PDF holds: what lies wholly off the page is left out, since none of it
    /// would show; rectangles and grey boxes are cut to the page, and lines and runs of
    /// glyphs to the page and a margin around it.
    fn drawn(&mut self, width: Pt, height: Pt, items: &[DisplayItem]) -> Vec<Drawn> {
        let page = Area {
            width: width.0,
            height: height.0,
        };
        // The display list's places are from the page's top left corner, y down; the
        // PDF's from its bottom left corner, y up.
        let flip = move |y: Pt| Pt(height.0 - y.0);
        let mut drawn = Vec::new();
        for item in items {
            match item {
                DisplayItem::Glyphs(run) => {
                    drawn.extend(self.glyphs(run, flip, page).map(Drawn::Glyphs));
                }
                DisplayItem::Rect {
                    at,
                    width,
                    height,
                    color,
                } => {
                    let bottom = flip(Pt(at.y.0 + height.0));
                    if let Some((x, y, w, h)) = page.clip(at.x.0, bottom.0, width.0, height.0) {
                        drawn.push(Drawn::Fill(pdf::GraphicsPath::rect(x, y, w, h), *color));
                    }
                }
                DisplayItem::Line(line) => {
                    if let Some(line) = on_page(line, page) {
                        let (path, stroke) = line_path(&line, flip);
                        drawn.push(Drawn::Stroke(path, stroke));
                    }
                }
                DisplayItem::Image(image) => drawn.extend(self.image(image, flip, page)),
            }
        }
        drawn
    }

    /// The PDF glyph run of `run`, with its clusters that reach the page; `None` when
    /// none does, or when its face cannot be embedded, which is reported.
    fn glyphs(
        &mut self,
        run: &GlyphItem,
        flip: impl Fn(Pt) -> Pt,
        page: Area,
    ) -> Option<pdf::GlyphRun> {
        // No glyph is drawn farther than four times its size from its place.
        let reach = 4.0 * run.size.0;
        let (x, y) = (run.origin.x.0, flip(run.origin.y).0);
        let on_page = |glyph: &DrawnGlyph| {
            let at = (x + glyph.x.0, y + glyph.y.0);
            page.reaches(at, at, reach)
        };
        let clusters: Vec<&Cluster> = run
            .clusters
            .iter()
            .filter(|cluster| !cluster.glyphs.is_empty() && cluster.glyphs.iter().all(on_page))
            .collect();
        // The run starts at its first glyph on the page, so that every position of it
        // is near the page, within what a PDF holds.
        let first = clusters.first()?.glyphs.first()?.x.0;
        let font = self.font(run.face)?;
        let mut glyphs = pdf::GlyphRun::new(&font, run.size, Pt(x + first), Pt(y))
            .with_color(run.color)
            .with_horizontal_scale(run.h_scale);
        if run.synthetic_bold {
            glyphs = glyphs.with_stroke(Pt(run.size.0 * SYNTHETIC_BOLD_STROKE));
        }
        if run.synthetic_italic {
            glyphs = glyphs.with_skew(SYNTHETIC_ITALIC_SKEW);
        }
        for cluster in clusters {
            let placed = cluster
                .glyphs
                .iter()
                .map(|glyph| pdf::PlacedGlyph::new(glyph.glyph, Pt(glyph.x.0 - first), glyph.y));
            glyphs.push_cluster(&cluster.text, placed);
        }
        Some(glyphs)
    }

    /// The PDF font of face `id`, made once, with what its license or format restricts
    /// reported; `None`, reported once, when it cannot be embedded.
    fn font(&mut self, id: FaceId) -> Option<pdf::Font> {
        if let Some(font) = self.pdf_fonts.get(&id) {
            return font.clone();
        }
        let face = self.fonts.face(id);
        let font = face.and_then(|face| {
            let name = face.face().names().family().unwrap_or("a font").to_owned();
            match pdf::Font::from_face(face.face().clone()) {
                Ok(font) => {
                    for note in font.embedding_notes() {
                        self.fonts.report_once(
                            ReportKind::FontEmbedding,
                            &Location::default(),
                            format!("{name}: {note}"),
                        );
                    }
                    Some(font)
                }
                Err(err) => {
                    self.fonts.report_once(
                        ReportKind::FontEmbedding,
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

    /// What picture `item` draws: the image, or a grey box of the part of it on the page
    /// when its format cannot be drawn or it is larger than a PDF holds, which is
    /// reported; nothing when it has no width or height or is wholly off the page. A
    /// crop, rotation or mirroring is not drawn yet, and is reported.
    fn image(&mut self, item: &ImageItem, flip: impl Fn(Pt) -> Pt, page: Area) -> Option<Drawn> {
        let (left, width, height) = (item.at.x.0, item.width.0, item.height.0);
        let bottom = flip(Pt(item.at.y.0 + height)).0;
        let corner = (left + width, bottom + height);
        if width < MIN_PICTURE || height < MIN_PICTURE || !page.reaches((left, bottom), corner, 0.0)
        {
            return None;
        }
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
        let fits = [left, bottom, corner.0, corner.1]
            .iter()
            .all(|value| value.abs() <= PDF_LIMIT);
        match image {
            Some(image) if fits => Some(Drawn::Image(
                image,
                (Pt(left), Pt(bottom), Pt(width), Pt(height)),
            )),
            image => {
                if image.is_some() {
                    self.fonts.report_once(
                        ReportKind::FeatureUnsupported,
                        &Location::default(),
                        format!(
                            "pictures reaching farther than {PDF_LIMIT} pt from the page's \
                             corner, more than a PDF holds, are drawn as grey boxes"
                        ),
                    );
                }
                let (x, y, w, h) = page.clip(left, bottom, width, height)?;
                Some(Drawn::Fill(
                    pdf::GraphicsPath::rect(x, y, w, h),
                    PLACEHOLDER,
                ))
            }
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

/// A page, `width` by `height` points: what is drawn on it is kept to it, since nothing
/// past it shows. Its box is the same in the display list's space and the PDF's.
#[derive(Clone, Copy)]
struct Area {
    width: f64,
    height: f64,
}

impl Area {
    /// Whether the box with corners `a` and `b` reaches the page widened by `margin` on
    /// each side.
    fn reaches(self, a: (f64, f64), b: (f64, f64), margin: f64) -> bool {
        a.0.max(b.0) >= -margin
            && a.0.min(b.0) <= self.width + margin
            && a.1.max(b.1) >= -margin
            && a.1.min(b.1) <= self.height + margin
    }

    /// The part on the page of the box `width` by `height` from `left`, `bottom`, as the
    /// same four; `None` when none of it is.
    fn clip(self, left: f64, bottom: f64, width: f64, height: f64) -> Option<(Pt, Pt, Pt, Pt)> {
        let (x0, y0) = (left.max(0.0), bottom.max(0.0));
        let (x1, y1) = (
            (left + width).min(self.width),
            (bottom + height).min(self.height),
        );
        (x1 > x0 && y1 > y0).then_some((Pt(x0), Pt(y0), Pt(x1 - x0), Pt(y1 - y0)))
    }
}

/// The part of `line` on the page and a margin around it, as wide as a wave reaches,
/// so that every point of its path is within what a PDF holds; `None` when no part of
/// it is there. A wave cut at the page's edge may start its curves elsewhere along it.
fn on_page(line: &LineItem, page: Area) -> Option<LineItem> {
    let margin = 4.0 * line.width.0.max(0.0) + 1.0;
    let (x0, y0) = (line.from.x.0, line.from.y.0);
    let (dx, dy) = (line.to.x.0 - x0, line.to.y.0 - y0);
    // Liang and Barsky's clipping: the stretch of the segment, from 0 to 1, inside each
    // edge of the box.
    let (mut start, mut end) = (0.0_f64, 1.0_f64);
    let edges = [
        (-dx, x0 + margin),
        (dx, page.width + margin - x0),
        (-dy, y0 + margin),
        (dy, page.height + margin - y0),
    ];
    for (towards, room) in edges {
        if towards == 0.0 {
            if room < 0.0 {
                return None;
            }
            continue;
        }
        let at = room / towards;
        if towards < 0.0 {
            start = start.max(at);
        } else {
            end = end.min(at);
        }
    }
    if start > end {
        return None;
    }
    let point = |t: f64| Point {
        x: Pt(x0 + t * dx),
        y: Pt(y0 + t * dy),
    };
    Some(LineItem {
        from: point(start),
        to: point(end),
        ..*line
    })
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

    /// What `items` of `document` draw on a page 100 pt wide and 200 pt high, with the
    /// test font `font`, and what the painter reported.
    fn painted(
        document: &Document,
        font: Vec<u8>,
        items: &[DisplayItem],
    ) -> (Vec<Drawn>, LayoutReport) {
        let book = crate::testing::book(vec![font]);
        let mut fonts = Fonts::new(&book);
        let mut painter = Painter {
            document,
            fonts: &mut fonts,
            pdf_fonts: HashMap::new(),
            images: HashMap::new(),
            pdf: pdf::Document::new(),
        };
        let drawn = painter.drawn(Pt(100.0), Pt(200.0), items);
        (drawn, fonts.into_report())
    }

    /// The face of the test font in a book of it alone.
    fn face() -> FaceId {
        crate::testing::book(vec![crate::testing::minimal()])
            .faces()
            .next()
            .map(|(id, _)| id)
            .expect("the test font")
    }

    /// A run of the test font at 10 pt from (`x`, `y`), of clusters of glyph 1 at the
    /// given offsets, each standing for "A".
    fn run(x: f64, y: f64, offsets: &[(f64, f64)]) -> GlyphItem {
        GlyphItem {
            face: face(),
            size: Pt(10.0),
            color: Color::BLACK,
            h_scale: 1.0,
            synthetic_bold: false,
            synthetic_italic: false,
            origin: Point { x: Pt(x), y: Pt(y) },
            clusters: offsets
                .iter()
                .map(|&(x, y)| Cluster {
                    text: "A".to_owned(),
                    glyphs: vec![DrawnGlyph {
                        glyph: officewright_font::GlyphId::new(1),
                        x: Pt(x),
                        y: Pt(y),
                    }],
                })
                .collect(),
        }
    }

    /// A picture of `image` at (`x`, `y`) of `width` by `height`.
    fn picture(image: ImageId, x: f64, y: f64, width: f64, height: f64) -> ImageItem {
        ImageItem {
            image,
            at: Point { x: Pt(x), y: Pt(y) },
            width: Pt(width),
            height: Pt(height),
            crop: None,
            rotation: 0.0,
            flip_horizontal: false,
            flip_vertical: false,
        }
    }

    /// The paths of `drawn` that fill, with their colours.
    fn fills(drawn: &[Drawn]) -> Vec<(pdf::GraphicsPath, Color)> {
        drawn
            .iter()
            .filter_map(|item| match item {
                Drawn::Fill(path, color) => Some((path.clone(), *color)),
                _ => None,
            })
            .collect()
    }

    /// A PNG of one green pixel.
    const PIXEL: [u8; 69] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2,
        0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 0, 0,
        3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    /// The boxes of the images of `drawn`: left, bottom, width and height.
    fn image_boxes(drawn: &[Drawn]) -> Vec<(Pt, Pt, Pt, Pt)> {
        drawn
            .iter()
            .filter_map(|item| match item {
                Drawn::Image(_, at) => Some(*at),
                _ => None,
            })
            .collect()
    }

    /// The messages of `report`.
    fn messages(report: &LayoutReport) -> Vec<String> {
        report
            .entries()
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect()
    }

    #[test]
    fn rectangles_are_flipped_and_cut_to_the_page() {
        let rect = |x: f64, y: f64, width: f64, height: f64| DisplayItem::Rect {
            at: Point { x: Pt(x), y: Pt(y) },
            width: Pt(width),
            height: Pt(height),
            color: Color::rgb(1, 2, 3),
        };
        let items = [
            // 20 pt down to 60 pt down: 140 to 180 pt up.
            rect(10.0, 20.0, 30.0, 40.0),
            // Past the left and top edges: the part on the page.
            rect(-50.0, -10.0, 100.0, 30.0),
            // Wholly off the page: nothing.
            rect(500.0, 500.0, 10.0, 10.0),
        ];
        let (drawn, _) = painted(&Document::default(), crate::testing::minimal(), &items);
        let color = Color::rgb(1, 2, 3);
        assert_eq!(
            fills(&drawn),
            [
                (
                    pdf::GraphicsPath::rect(Pt(10.0), Pt(140.0), Pt(30.0), Pt(40.0)),
                    color
                ),
                (
                    pdf::GraphicsPath::rect(Pt(0.0), Pt(180.0), Pt(50.0), Pt(20.0)),
                    color
                ),
            ]
        );
    }

    #[test]
    fn pictures_are_flipped_and_kept_to_what_a_pdf_holds() {
        let mut document = Document::default();
        let emf = document.add_image(crate::ir::ImageData::new(
            ImageFormat::Other("emf".into()),
            vec![1, 2],
        ));
        let cropped = ImageItem {
            crop: Some(crate::ir::Crop::default().with_left(0.1)),
            ..picture(emf, 60.0, 20.0, 10.0, 10.0)
        };
        let items = [
            // A grey box of the picture's size, 140 to 180 pt up.
            DisplayItem::Image(picture(emf, 10.0, 20.0, 30.0, 40.0)),
            // No width or no height, or wholly off the page: nothing.
            DisplayItem::Image(picture(emf, 10.0, 20.0, 0.0, 0.0)),
            DisplayItem::Image(picture(emf, 10.0, 20.0, 0.0, 20.0)),
            DisplayItem::Image(picture(emf, 500.0, 500.0, 10.0, 10.0)),
            // Larger than a PDF holds: the part on the page.
            DisplayItem::Image(picture(emf, 10.0, 20.0, 40_000.0, 40_000.0)),
            DisplayItem::Image(cropped),
        ];
        let (drawn, report) = painted(&document, crate::testing::minimal(), &items);
        let boxes: Vec<pdf::GraphicsPath> = fills(&drawn)
            .into_iter()
            .map(|(path, color)| {
                assert_eq!(color, PLACEHOLDER);
                path
            })
            .collect();
        assert_eq!(
            boxes,
            [
                pdf::GraphicsPath::rect(Pt(10.0), Pt(140.0), Pt(30.0), Pt(40.0)),
                pdf::GraphicsPath::rect(Pt(10.0), Pt(0.0), Pt(90.0), Pt(180.0)),
                pdf::GraphicsPath::rect(Pt(60.0), Pt(170.0), Pt(10.0), Pt(10.0)),
            ]
        );
        let all = messages(&report);
        assert!(all.iter().any(|m| m.contains("emf")), "{all:?}");
        assert!(all.iter().any(|m| m.contains("cropped")), "{all:?}");
    }

    #[test]
    fn images_are_flipped_and_left_out_off_the_page_or_past_what_a_pdf_holds() {
        let mut document = Document::default();
        let png = document.add_image(crate::ir::ImageData::new(ImageFormat::Png, PIXEL.to_vec()));
        let items = [
            // The image, 140 to 180 pt up.
            DisplayItem::Image(picture(png, 10.0, 20.0, 30.0, 40.0)),
            // Wholly off the page: nothing.
            DisplayItem::Image(picture(png, 500.0, 500.0, 10.0, 10.0)),
            // Larger than a PDF holds: a grey box of its part on the page.
            DisplayItem::Image(picture(png, 10.0, 20.0, 40_000.0, 40_000.0)),
        ];
        let (drawn, report) = painted(&document, crate::testing::minimal(), &items);
        assert_eq!(
            image_boxes(&drawn),
            [(Pt(10.0), Pt(140.0), Pt(30.0), Pt(40.0))]
        );
        assert_eq!(
            fills(&drawn),
            [(
                pdf::GraphicsPath::rect(Pt(10.0), Pt(0.0), Pt(90.0), Pt(180.0)),
                PLACEHOLDER
            )]
        );
        let all = messages(&report);
        assert!(
            all.iter().any(|m| m.contains("more than a PDF holds")),
            "{all:?}"
        );
    }

    #[test]
    fn glyph_runs_keep_their_look_text_and_places() {
        let mut item = run(10.0, 50.0, &[(0.0, 0.0), (5.0, 3.0)]);
        item.color = Color::rgb(200, 0, 0);
        item.h_scale = 0.5;
        item.synthetic_bold = true;
        item.synthetic_italic = true;
        if let Some(mark) = item.clusters.get_mut(1) {
            mark.text = "\u{301}".to_owned();
        }
        let (drawn, _) = painted(
            &Document::default(),
            crate::testing::minimal(),
            &[DisplayItem::Glyphs(item)],
        );
        let [Drawn::Glyphs(glyphs)] = drawn.as_slice() else {
            panic!("one run");
        };
        // 50 pt down is 150 pt up.
        assert_eq!((glyphs.x(), glyphs.y()), (Pt(10.0), Pt(150.0)));
        assert_eq!(glyphs.color(), Color::rgb(200, 0, 0));
        assert_eq!(glyphs.horizontal_scale(), 0.5);
        assert_eq!(glyphs.stroke(), Some(Pt(10.0 * SYNTHETIC_BOLD_STROKE)));
        assert_eq!(glyphs.skew(), SYNTHETIC_ITALIC_SKEW);
        let clusters: Vec<(String, Vec<pdf::PlacedGlyph>)> = glyphs
            .clusters()
            .map(|(text, placed)| (text.to_owned(), placed.to_vec()))
            .collect();
        let glyph = officewright_font::GlyphId::new(1);
        assert_eq!(
            clusters,
            [
                (
                    "A".to_owned(),
                    vec![pdf::PlacedGlyph::new(glyph, Pt(0.0), Pt(0.0))]
                ),
                (
                    "\u{301}".to_owned(),
                    vec![pdf::PlacedGlyph::new(glyph, Pt(5.0), Pt(3.0))]
                ),
            ]
        );
    }

    #[test]
    fn glyphs_off_the_page_are_left_out() {
        let items = [
            // The second cluster is far past the right edge.
            DisplayItem::Glyphs(run(10.0, 50.0, &[(0.0, 0.0), (40_000.0, 0.0)])),
            // The run starts far left, its second cluster on the page: it starts there.
            DisplayItem::Glyphs(run(-40_000.0, 60.0, &[(0.0, 0.0), (40_010.0, 0.0)])),
            // Far below the page: nothing.
            DisplayItem::Glyphs(run(10.0, 1_000.0, &[(0.0, 0.0)])),
        ];
        let (drawn, _) = painted(&Document::default(), crate::testing::minimal(), &items);
        let runs: Vec<(Pt, Pt, usize)> = drawn
            .iter()
            .filter_map(|item| match item {
                Drawn::Glyphs(run) => Some((run.x(), run.y(), run.glyphs().len())),
                _ => None,
            })
            .collect();
        assert_eq!(runs, [(Pt(10.0), Pt(150.0), 1), (Pt(10.0), Pt(140.0), 1)]);
    }

    #[test]
    fn lines_are_cut_to_the_page_and_a_margin() {
        let level = LineItem {
            from: Point {
                x: Pt(-40_000.0),
                y: Pt(50.0),
            },
            to: Point {
                x: Pt(500.0),
                y: Pt(50.0),
            },
            width: Pt(0.5),
            style: LineStyle::Solid,
            color: Color::BLACK,
        };
        let off = LineItem {
            from: Point {
                x: Pt(10.0),
                y: Pt(-500.0),
            },
            to: Point {
                x: Pt(20.0),
                y: Pt(-400.0),
            },
            ..level
        };
        let (drawn, _) = painted(
            &Document::default(),
            crate::testing::minimal(),
            &[DisplayItem::Line(level), DisplayItem::Line(off)],
        );
        // A margin of four widths and a point: 3 pt either side, at 150 pt up.
        let mut expected = pdf::GraphicsPath::new(Pt(-3.0), Pt(150.0));
        expected.line_to(Pt(103.0), Pt(150.0));
        let paths: Vec<&pdf::GraphicsPath> = drawn
            .iter()
            .filter_map(|item| match item {
                Drawn::Stroke(path, _) => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(paths, [&expected]);
    }

    #[test]
    fn what_a_font_s_license_restricts_is_reported() {
        // `fsType` of the `OS/2` table: preview and print, editable, no subsetting, and
        // restricted, which forbids embedding.
        for (bits, expected) in [
            (0x0004_u16, "viewing and printing"),
            (0x0008, "editing"),
            (0x0100, "whole font"),
            (0x0002, "not drawn"),
        ] {
            let font =
                crate::testing::patched(crate::testing::minimal(), b"OS/2", 8, &bits.to_be_bytes());
            let (drawn, report) = painted(
                &Document::default(),
                font,
                &[DisplayItem::Glyphs(run(10.0, 50.0, &[(0.0, 0.0)]))],
            );
            assert!(
                report
                    .entries()
                    .iter()
                    .any(|entry| entry.kind() == ReportKind::FontEmbedding
                        && entry.message().contains(expected)),
                "{bits:#06x}: {report:?}"
            );
            assert_eq!(drawn.len(), usize::from(bits != 0x0002), "{bits:#06x}");
        }
    }

    #[test]
    fn options_show_the_lengths_of_fonts_in_memory() {
        let options = RenderOptions::new().with_font_data(vec![7_u8, 8, 9]);
        let shown = format!("{options:?}");
        assert!(shown.contains("font_data_lengths: [3]"), "{shown}");
        assert!(!shown.contains("7, 8, 9"), "{shown}");
    }

    #[test]
    fn a_pdf_error_says_what_failed_through_its_source() {
        let refused = || match pdf::Document::new().add_page(PageSize::new(Pt(1.0), Pt(1.0))) {
            Err(err) => err,
            Ok(_) => panic!("a page of 1 pt is refused"),
        };
        let message = refused().to_string();
        let error = Error::from(refused());
        assert_eq!(error.to_string(), "the PDF could not be made");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some(message.clone())
        );
        // As `officewright-pdf`'s own error converts: the same kind and message.
        let converted = officewright_core::Error::from(error);
        let direct = officewright_core::Error::from(refused());
        assert_eq!(
            std::mem::discriminant(&converted),
            std::mem::discriminant(&direct)
        );
        assert_eq!(converted.to_string(), direct.to_string());
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
        // A wave of one half-wave past the bound (of 1 pt each, at a width of 0.5 pt): a
        // straight line; at the bound, a wave.
        let straight_at = |length: f64| {
            let (path, _) = line_path(&line(LineStyle::Wave, (0.0, 20.0), (length, 20.0)), flip);
            let mut straight = pdf::GraphicsPath::new(Pt(0.0), Pt(80.0));
            straight.line_to(Pt(length), Pt(80.0));
            path == straight
        };
        assert!(straight_at(MAX_HALF_WAVES + 1.0));
        assert!(!straight_at(MAX_HALF_WAVES));
    }
}
