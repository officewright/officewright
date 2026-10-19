//! Rendering into PDF: a document of the layout IR laid out on pages, and every page
//! painted with `officewright-pdf`.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use officewright_core::{Color, PageSize, Pt};
use officewright_font::{FaceId, FontBook, FontOrigin};
use officewright_pdf as pdf;

use crate::display::{DisplayItem, DrawnGlyph, GlyphItem, ImageItem, LineItem, LineStyle, Point};
use crate::error::{Error, Result};
use crate::fonts::Fonts;
use crate::ir::{Document, ImageFormat, ImageId};
use crate::pages::{self, Page, PageSink};
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

/// The farthest a position may be from a page's corner, and the largest a picture or a
/// length of a dash may be, in a PDF: 32767 pt, the largest real number a PDF 1.4 reader
/// must read (PDF Reference 1.4, Appendix C), which `officewright-pdf` checks. The
/// painter keeps within it.
const PDF_LIMIT: f64 = 32767.0;

/// The thickest line drawn: the longest dash of a pattern is eight times the width, so
/// that every dash stays within [`PDF_LIMIT`]. Only a font with broken metrics asks for
/// more, and that is reported.
const MAX_LINE_WIDTH: f64 = PDF_LIMIT / 8.0;

/// The smallest side of a page a PDF holds (PDF Reference 1.4, Appendix C), which
/// `officewright-pdf` checks; a smaller page is made this large, and that is reported.
const MIN_PAGE_SIDE: f64 = 3.0;

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
    render(document, fonts, options).map(|(rendered, _)| rendered)
}

/// [`render_pdf`], with the most pages that were laid out and not yet painted at once.
fn render(
    document: &Document,
    fonts: &FontBook,
    options: &RenderOptions,
) -> Result<(Rendered, usize)> {
    document.validate()?;
    let mut document = document.clone();
    let mut report = document.clamp();
    let mut layout_fonts = Fonts::new(fonts);
    let mut shaper = Shaper::new();
    // Each page is painted as soon as it is laid out, and dropped (#332).
    let painted = pages::paginate_into(
        &document,
        &mut layout_fonts,
        &mut shaper,
        options.max_pages,
        || PdfPages::new(&document, options.compression),
    )?;
    // Only the last pass's painting is reported, after what the layout reported, as when
    // the pages were painted once they were all laid out.
    for (kind, location, message) in painted.notes {
        layout_fonts.report_once(kind, &location, message);
    }
    report.extend(layout_fonts.into_report());
    let waiting = painted.most_waiting;
    Ok((
        Rendered {
            pdf: painted.pdf,
            report,
        },
        waiting,
    ))
}

/// A PDF that pages are painted into as they are laid out: what a [`Painter`] keeps from
/// page to page.
struct PdfPages<'a> {
    document: &'a Document,
    pdf_fonts: HashMap<FaceId, Option<pdf::Font>>,
    images: HashMap<ImageId, Option<pdf::Image>>,
    pdf: pdf::Document,
    /// What painting reported, for the report of the layout pass whose PDF this is.
    notes: Vec<Note>,
    /// The pages painted, and the most pages laid out and not yet painted at once.
    painted: usize,
    most_waiting: usize,
}

/// An entry of the report: its kind, where, and what.
type Note = (ReportKind, Location, String);

impl<'a> PdfPages<'a> {
    fn new(document: &'a Document, compression: pdf::Compression) -> Self {
        let mut pdf = pdf::Document::new();
        pdf.set_compression(compression);
        Self {
            document,
            pdf_fonts: HashMap::new(),
            images: HashMap::new(),
            pdf,
            notes: Vec::new(),
            painted: 0,
            most_waiting: 0,
        }
    }
}

impl PageSink for PdfPages<'_> {
    fn take(&mut self, page: Page, made: usize, fonts: &mut Fonts<'_>) -> Result<()> {
        self.most_waiting = self.most_waiting.max(made.saturating_sub(self.painted));
        self.painted += 1;
        let mut painter = Painter {
            document: self.document,
            fonts,
            pdf_fonts: std::mem::take(&mut self.pdf_fonts),
            images: std::mem::take(&mut self.images),
            pdf: std::mem::take(&mut self.pdf),
            notes: std::mem::take(&mut self.notes),
        };
        let painted = painter.page(&page);
        self.pdf_fonts = painter.pdf_fonts;
        self.images = painter.images;
        self.pdf = painter.pdf;
        self.notes = painter.notes;
        painted
    }
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
    /// What painting reported.
    notes: Vec<Note>,
}

impl Painter<'_, '_, '_> {
    /// Notes `message` for the report, kept with the PDF of its layout pass, so that a
    /// pass whose PDF is dropped leaves nothing in the report.
    fn note(&mut self, kind: ReportKind, location: &Location, message: String) {
        self.notes.push((kind, location.clone(), message));
    }

    /// Adds a page of the size of `page`, at least [`MIN_PAGE_SIDE`] each way, and paints
    /// what it draws.
    fn page(&mut self, page: &Page) -> Result<()> {
        let width = Pt(page.width.0.max(MIN_PAGE_SIDE));
        let height = Pt(page.height.0.max(MIN_PAGE_SIDE));
        if (width, height) != (page.width, page.height) {
            self.note(
                ReportKind::ValueClamped,
                &Location::default(),
                format!(
                    "pages narrower or shorter than {MIN_PAGE_SIDE} pt, the least a PDF \
                     holds, are made that large"
                ),
            );
        }
        self.paint(width, height, &page.items)
    }

    /// Adds a page `width` by `height` and paints `items` on it.
    fn paint(&mut self, width: Pt, height: Pt, items: &[DisplayItem]) -> Result<()> {
        let items = self.drawn(width, height, items);
        let pdf_page = self.pdf.add_page(PageSize::new(width, height))?;
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
                    drawn.extend(self.glyphs(run, flip, page).into_iter().map(Drawn::Glyphs));
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
                    let line = self.thin_enough(line);
                    if let Some(line) = on_page(&line, page) {
                        let (path, stroke) = line_path(&line, flip);
                        drawn.push(Drawn::Stroke(path, stroke));
                    }
                }
                DisplayItem::Image(image) => drawn.extend(self.image(image, flip, page)),
            }
        }
        drawn
    }

    /// `line`, at most [`MAX_LINE_WIDTH`] thick; a thicker one is reported.
    fn thin_enough(&mut self, line: &LineItem) -> LineItem {
        if line.width.0 <= MAX_LINE_WIDTH {
            return *line;
        }
        self.note(
            ReportKind::ValueClamped,
            &Location::default(),
            format!(
                "lines thicker than {MAX_LINE_WIDTH} pt are drawn that thick, which keeps \
                 their dashes and the margin they are cut to within what a PDF holds"
            ),
        );
        LineItem {
            width: Pt(MAX_LINE_WIDTH),
            ..*line
        }
    }

    /// The PDF glyph runs of `run`, with the glyphs of it that reach the page and a margin
    /// around it, each cluster with its text, but a cluster cut down to one of its several
    /// glyphs with none; none when no glyph reaches the page, or when its face cannot be
    /// embedded, which is reported. A run starts again wherever a glyph would be farther
    /// from the start of the one before than a PDF holds.
    fn glyphs(
        &mut self,
        run: &GlyphItem,
        flip: impl Fn(Pt) -> Pt,
        page: Area,
    ) -> Vec<pdf::GlyphRun> {
        // No glyph is drawn farther than four times its size from its place, nor, scaled
        // wider (`w:w`), farther across than that times its scale; but a glyph kept is
        // never placed farther from the page's corner than a PDF holds.
        let reach = 4.0 * run.size.0;
        let across = (reach * run.h_scale.max(1.0)).min((PDF_LIMIT - page.width).max(0.0));
        let (x, y) = (run.origin.x.0, flip(run.origin.y).0);
        let skew = if run.synthetic_italic {
            SYNTHETIC_ITALIC_SKEW
        } else {
            0.0
        };
        // Where a glyph is drawn: synthetic italic slants it about the run's baseline.
        let place = |glyph: &DrawnGlyph| (x + glyph.x.0 + skew * glyph.y.0, y + glyph.y.0);
        let on_page = |glyph: &&DrawnGlyph| {
            let at = place(glyph);
            page.reaches_by(at, at, across, reach)
        };
        let mut runs = Vec::new();
        let mut font = None;
        let mut current: Option<OpenRun> = None;
        // Glyph by glyph: a cluster may be many, such as the dots of a tab leader, of
        // which only some are on the page.
        for cluster in &run.clusters {
            let on: Vec<&DrawnGlyph> = cluster.glyphs.iter().filter(on_page).collect();
            let Some(&lead) = on.first() else {
                continue;
            };
            // The run made so far takes the cluster when it holds every glyph of it that
            // reaches the page; else a run starts at the cluster's first.
            let held = current.as_ref().is_some_and(|open| {
                on.iter()
                    .all(|glyph| holds(open.start, offset(glyph, open.first)))
            });
            if !held {
                let start = place(lead);
                if !holds(start, (0.0, 0.0)) {
                    continue;
                }
                let font = match &font {
                    Some(font) => font,
                    None => match self.font(run.face) {
                        Some(made) => font.insert(made),
                        None => return Vec::new(),
                    },
                };
                let mut glyphs = pdf::GlyphRun::new(font, run.size, Pt(start.0), Pt(start.1))
                    .with_color(run.color)
                    .with_horizontal_scale(run.h_scale);
                if run.synthetic_bold {
                    glyphs = glyphs.with_stroke(Pt(run.size.0 * SYNTHETIC_BOLD_STROKE));
                }
                if run.synthetic_italic {
                    glyphs = glyphs.with_skew(SYNTHETIC_ITALIC_SKEW);
                }
                runs.extend(current.take().map(|open| open.glyphs));
                current = Some(OpenRun {
                    glyphs,
                    first: (lead.x.0, lead.y.0),
                    start,
                });
            }
            let Some(open) = current.as_mut() else {
                continue;
            };
            let kept: Vec<&DrawnGlyph> = on
                .into_iter()
                .filter(|glyph| holds(open.start, offset(glyph, open.first)))
                .collect();
            // One glyph left of several would stand for all the cluster's text, which the
            // PDF would then give it wherever else it is drawn: it stands for none, since
            // the rest of the cluster is off the page.
            let text = if kept.len() == 1 && cluster.glyphs.len() > 1 {
                ""
            } else {
                cluster.text.as_str()
            };
            let first = open.first;
            let placed = kept.into_iter().map(|glyph| {
                let (dx, dy) = offset(glyph, first);
                pdf::PlacedGlyph::new(glyph.glyph, Pt(dx), Pt(dy))
            });
            open.glyphs.push_cluster(text, placed);
        }
        runs.extend(current.map(|open| open.glyphs));
        runs
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
                        self.note(
                            ReportKind::FontEmbedding,
                            &Location::default(),
                            format!("{name}: {note}"),
                        );
                    }
                    Some(font)
                }
                Err(err) => {
                    // With the causes, which say why the font cannot be used.
                    let mut why = err.to_string();
                    let mut cause = std::error::Error::source(&err);
                    while let Some(inner) = cause {
                        why = format!("{why}: {inner}");
                        cause = inner.source();
                    }
                    self.note(
                        ReportKind::FontEmbedding,
                        &Location::default(),
                        format!("the text in {name} is not drawn: {why}"),
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
            self.note(
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
        let fits = width <= PDF_LIMIT
            && height <= PDF_LIMIT
            && [left, bottom, corner.0, corner.1]
                .iter()
                .all(|value| value.abs() <= PDF_LIMIT);
        match image {
            Some(image) if fits => Some(Drawn::Image(
                image,
                (Pt(left), Pt(bottom), Pt(width), Pt(height)),
            )),
            image => {
                if image.is_some() {
                    self.note(
                        ReportKind::FeatureUnsupported,
                        &Location::default(),
                        format!(
                            "pictures larger than {PDF_LIMIT} pt or reaching farther from the \
                             page's corner, more than a PDF holds, are drawn as grey boxes"
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
                        self.note(
                            ReportKind::FeatureUnsupported,
                            &Location::default(),
                            format!("a picture is drawn as a grey box: {err}"),
                        );
                        None
                    }
                }
            }
            ImageFormat::Other(name) => {
                self.note(
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
        self.reaches_by(a, b, margin, margin)
    }

    /// Whether the box with corners `a` and `b` reaches the page widened by `across` on
    /// its left and right and by `up` above and below it.
    fn reaches_by(self, a: (f64, f64), b: (f64, f64), across: f64, up: f64) -> bool {
        a.0.max(b.0) >= -across
            && a.0.min(b.0) <= self.width + across
            && a.1.max(b.1) >= -up
            && a.1.min(b.1) <= self.height + up
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

/// A PDF glyph run being made. It starts where its first glyph is drawn, so that every
/// position of it is near the page. Synthetic italic slants about the run's start, so
/// the offsets are taken from that glyph's place before the slant: every glyph is drawn
/// where it would be from the baseline.
struct OpenRun {
    glyphs: pdf::GlyphRun,
    /// The place of its first glyph, before the slant.
    first: (f64, f64),
    /// Where it starts.
    start: (f64, f64),
}

/// The offset of `glyph` from the place `first`, before the slant: where a PDF glyph run
/// that starts at the glyph at `first` places it.
fn offset(glyph: &DrawnGlyph, first: (f64, f64)) -> (f64, f64) {
    (glyph.x.0 - first.0, glyph.y.0 - first.1)
}

/// Whether a PDF glyph run that starts at `start` holds a glyph at `offset` from it: the
/// offset and the place it makes, before the slant, within what a PDF holds, as
/// officewright-pdf checks them.
fn holds(start: (f64, f64), offset: (f64, f64)) -> bool {
    let within = |value: f64| value.abs() <= PDF_LIMIT;
    within(offset.0) && within(offset.1) && within(start.0 + offset.0) && within(start.1 + offset.1)
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
    use crate::display::Cluster;

    #[test]
    fn a_layout_pass_that_numpages_makes_necessary_paints_a_pdf_of_its_own() {
        use crate::ir::{
            Block, Field, FieldKind, Inline, PageSetup, Paragraph, ParagraphProps, RunFonts,
            RunStyle, Section,
        };

        // Issue #332: pages are painted as they are laid out, so the PDF of a pass whose
        // page counts change is dropped. A `NUMPAGES` field on the first of three pages
        // lays them out twice (the first time for one page), and the PDF has three.
        let book = crate::testing::book(vec![crate::testing::minimal()]);
        let mut document = Document::default();
        let style = document
            .add_run_style(RunStyle::default().with_fonts(RunFonts::new("Officewright Test")));
        let page = |break_before: bool, inline: Inline| -> Block {
            Paragraph::new(style)
                .with_props(ParagraphProps::default().with_page_break_before(break_before))
                .with_inlines([inline])
                .into()
        };
        document.add_section(Section::new(PageSetup::new(PageSize::A4)).with_blocks([
            page(false, Inline::Field(Field::new(FieldKind::NumPages, style))),
            page(true, Inline::text("A", style)),
            page(true, Inline::text("A", style)),
        ]));
        let rendered = render_pdf(&document, &book, &RenderOptions::new()).expect("a PDF");
        assert_eq!(rendered.pdf().page_count(), 3);
    }

    #[test]
    fn pages_are_painted_as_they_are_laid_out() {
        use crate::ir::{
            Inline, PageSetup, Paragraph, ParagraphProps, RunFonts, RunStyle, Section,
        };

        // Issue #332: render_pdf paints each page as soon as it is laid out. 50 paragraphs,
        // each on a page of its own, never wait more than two at a time: the page handed on
        // and the one just made.
        let book = crate::testing::book(vec![crate::testing::minimal()]);
        let mut document = Document::default();
        let style = document
            .add_run_style(RunStyle::default().with_fonts(RunFonts::new("Officewright Test")));
        document.add_section(
            Section::new(PageSetup::new(PageSize::A4)).with_blocks((0..50).map(|n| {
                Paragraph::new(style)
                    .with_props(ParagraphProps::default().with_page_break_before(n > 0))
                    .with_inlines([Inline::text("A", style)])
                    .into()
            })),
        );
        let (rendered, waiting) = render(&document, &book, &RenderOptions::new()).expect("a PDF");
        assert_eq!(rendered.pdf().page_count(), 50);
        assert_eq!(waiting, 2);
    }

    #[test]
    fn what_painting_reports_comes_after_what_the_layout_reports() {
        use crate::ir::{
            Block, Border, ImageData, ImageFormat, Inline, InlineObject, ObjectContent, PageSetup,
            Paragraph, ParagraphBorders, ParagraphProps, Picture, RunFonts, RunStyle, Section,
            Size,
        };

        // Issue #332: pages are painted as they are laid out, but what painting reports is
        // kept with the PDF of its layout pass and added once the last pass is done, after
        // what the layout reports, as when every page was painted at the end. A picture
        // that cannot be decoded on page 1 is painted (and reported) while page 2 is laid
        // out, and the layout reports the borders of page 2's paragraph after that.
        let book = crate::testing::book(vec![crate::testing::minimal()]);
        let mut document = Document::default();
        let style = document
            .add_run_style(RunStyle::default().with_fonts(RunFonts::new("Officewright Test")));
        let image = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
        let picture = Inline::Object(InlineObject::new(
            ObjectContent::Picture(Picture::new(image)),
            Size::new(Pt(10.0), Pt(10.0)),
        ));
        let bordered = ParagraphProps::default()
            .with_page_break_before(true)
            .with_borders(ParagraphBorders::default().with_top(Some(Border::default())));
        document.add_section(
            Section::new(PageSetup::new(PageSize::A4)).with_blocks([
                Block::from(Paragraph::new(style).with_inlines([picture])),
                Paragraph::new(style)
                    .with_props(bordered)
                    .with_inlines([Inline::text("A", style)])
                    .into(),
            ]),
        );
        let rendered = render_pdf(&document, &book, &RenderOptions::new()).expect("a PDF");
        let messages: Vec<&str> = rendered
            .report()
            .entries()
            .iter()
            .map(|entry| entry.message())
            .collect();
        let at = |what: &str| {
            messages
                .iter()
                .position(|message| message.starts_with(what))
        };
        let (Some(borders), Some(picture)) = (
            at("paragraph borders are not drawn yet"),
            at("a picture is drawn as a grey box"),
        ) else {
            panic!("{messages:?}");
        };
        assert!(borders < picture, "{messages:?}");
    }

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
            notes: Vec::new(),
        };
        let drawn = painter.drawn(Pt(100.0), Pt(200.0), items);
        // What the painter notes goes into the report, as render_pdf puts it.
        for (kind, location, message) in painter.notes {
            fonts.report_once(kind, &location, message);
        }
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
            // Wider than a PDF holds, though its corners are within it.
            DisplayItem::Image(picture(png, -16_500.0, 20.0, 33_000.0, 40.0)),
            // Narrower than the least a PDF draws: nothing.
            DisplayItem::Image(picture(png, 10.0, 20.0, 0.005, 10.0)),
        ];
        let (drawn, report) = painted(&document, crate::testing::minimal(), &items);
        assert_eq!(
            image_boxes(&drawn),
            [(Pt(10.0), Pt(140.0), Pt(30.0), Pt(40.0))]
        );
        assert_eq!(
            fills(&drawn),
            [
                (
                    pdf::GraphicsPath::rect(Pt(10.0), Pt(0.0), Pt(90.0), Pt(180.0)),
                    PLACEHOLDER
                ),
                (
                    pdf::GraphicsPath::rect(Pt(0.0), Pt(140.0), Pt(100.0), Pt(40.0)),
                    PLACEHOLDER
                ),
            ]
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
        // A tab leader: one cluster of dots every 3 pt from 300 pt left of the page to
        // 90 pt into it.
        let mut leader = run(0.0, 70.0, &[]);
        leader.clusters = vec![Cluster {
            text: "\t".to_owned(),
            glyphs: (0..=130)
                .map(|n| DrawnGlyph {
                    glyph: officewright_font::GlyphId::new(1),
                    x: Pt(-300.0 + 3.0 * f64::from(n)),
                    y: Pt(0.0),
                })
                .collect(),
        }];
        let items = [
            // The second cluster is far past the right edge.
            DisplayItem::Glyphs(run(10.0, 50.0, &[(0.0, 0.0), (40_000.0, 0.0)])),
            // The run starts far left, its second cluster on the page: it starts there.
            DisplayItem::Glyphs(run(-40_000.0, 60.0, &[(0.0, 0.0), (40_010.0, 0.0)])),
            // Far below the page: nothing.
            DisplayItem::Glyphs(run(10.0, 1_000.0, &[(0.0, 0.0)])),
            // Just past the right edge, within the reach of four times the size: kept.
            DisplayItem::Glyphs(run(95.0, 80.0, &[(0.0, 0.0), (8.0, 0.0)])),
            // The dots within the reach of the page: from 39 pt left of it.
            DisplayItem::Glyphs(leader),
        ];
        let (drawn, _) = painted(&Document::default(), crate::testing::minimal(), &items);
        let runs: Vec<(Pt, Pt, Vec<Pt>)> = drawn
            .iter()
            .filter_map(|item| match item {
                Drawn::Glyphs(run) => Some((
                    run.x(),
                    run.y(),
                    run.glyphs().iter().map(|glyph| glyph.x()).collect(),
                )),
                _ => None,
            })
            .collect();
        let dots: Vec<Pt> = (0..44).map(|n| Pt(3.0 * f64::from(n))).collect();
        assert_eq!(
            runs,
            [
                (Pt(10.0), Pt(150.0), vec![Pt(0.0)]),
                (Pt(10.0), Pt(140.0), vec![Pt(0.0)]),
                (Pt(95.0), Pt(120.0), vec![Pt(0.0), Pt(8.0)]),
                (Pt(-39.0), Pt(130.0), dots),
            ]
        );
    }

    #[test]
    fn glyphs_scaled_wider_reach_farther_across() {
        // Issue #215: at 600 %, a glyph of 10 pt reaches 240 pt across, not 40 pt, but
        // still 40 pt up and down; at most as far as a PDF holds positions.
        let scaled = |x: f64, y: f64, size: f64| GlyphItem {
            h_scale: 6.0,
            size: Pt(size),
            ..run(x, y, &[(0.0, 0.0)])
        };
        let items = [
            DisplayItem::Glyphs(scaled(-100.0, 50.0, 10.0)),
            DisplayItem::Glyphs(scaled(-239.0, 60.0, 10.0)),
            DisplayItem::Glyphs(scaled(-241.0, 70.0, 10.0)),
            DisplayItem::Glyphs(scaled(10.0, -39.0, 10.0)),
            DisplayItem::Glyphs(scaled(10.0, -41.0, 10.0)),
            // Right of the page the same, 240 pt; below it, 40 pt.
            DisplayItem::Glyphs(scaled(339.0, 110.0, 10.0)),
            DisplayItem::Glyphs(scaled(341.0, 120.0, 10.0)),
            DisplayItem::Glyphs(scaled(10.0, 239.0, 10.0)),
            DisplayItem::Glyphs(scaled(10.0, 241.0, 10.0)),
            // 6 × 4 × 1638 pt across, cut to 32767 pt from the page's corner.
            DisplayItem::Glyphs(scaled(-32_660.0, 80.0, 1638.0)),
            DisplayItem::Glyphs(scaled(-32_680.0, 90.0, 1638.0)),
            // Condensed: as far as at 100 %, not half as far.
            DisplayItem::Glyphs(GlyphItem {
                h_scale: 0.5,
                ..run(-30.0, 100.0, &[(0.0, 0.0)])
            }),
        ];
        let (drawn, _) = painted(&Document::default(), crate::testing::minimal(), &items);
        let starts: Vec<(Pt, Pt)> = drawn
            .iter()
            .filter_map(|item| match item {
                Drawn::Glyphs(run) => Some((run.x(), run.y())),
                _ => None,
            })
            .collect();
        assert_eq!(
            starts,
            [
                (Pt(-100.0), Pt(150.0)),
                (Pt(-239.0), Pt(140.0)),
                (Pt(10.0), Pt(239.0)),
                (Pt(339.0), Pt(90.0)),
                (Pt(10.0), Pt(-39.0)),
                (Pt(-32_660.0), Pt(120.0)),
                (Pt(-30.0), Pt(100.0)),
            ]
        );
    }

    /// What `items` draw on a page 100 by 200 pt, and whether officewright-pdf takes it.
    fn painted_into_pdf(items: &[DisplayItem]) -> (Vec<Drawn>, Result<()>) {
        let document = Document::default();
        let book = crate::testing::book(vec![crate::testing::minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut painter = Painter {
            document: &document,
            fonts: &mut fonts,
            pdf_fonts: HashMap::new(),
            images: HashMap::new(),
            pdf: pdf::Document::new(),
            notes: Vec::new(),
        };
        let drawn = painter.drawn(Pt(100.0), Pt(200.0), items);
        (drawn, painter.paint(Pt(100.0), Pt(200.0), items))
    }

    /// The start of a glyph run and the offsets of its glyphs.
    type RunPlaces = ((Pt, Pt), Vec<(Pt, Pt)>);

    /// The start and the glyphs' offsets of every glyph run of `drawn`.
    fn runs_of(drawn: &[Drawn]) -> Vec<RunPlaces> {
        drawn
            .iter()
            .filter_map(|item| match item {
                Drawn::Glyphs(run) => Some((
                    (run.x(), run.y()),
                    run.glyphs()
                        .iter()
                        .map(|glyph| (glyph.x(), glyph.y()))
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_run_wider_than_a_pdf_holds_is_drawn_as_several() {
        // Review of #270: glyphs of 1,638 pt at 600 % reach 32,667 pt across, so the
        // glyphs of one run that reach the page can be farther apart than a PDF holds
        // from a run's start: 20 of them 2,222 pt apart from 1,510 pt left of the page.
        // The 16 that reach the page are drawn, in a second run from the one that would
        // be more than 32,767 pt from the first.
        let offsets: Vec<(f64, f64)> = (0..20).map(|n| (2222.0 * f64::from(n), 0.0)).collect();
        let item = GlyphItem {
            size: Pt(1638.0),
            h_scale: 6.0,
            ..run(-1510.0, 100.0, &offsets)
        };
        let (drawn, painted) = painted_into_pdf(&[DisplayItem::Glyphs(item)]);
        assert!(painted.is_ok(), "{painted:?}");
        let runs = runs_of(&drawn);
        let starts: Vec<(Pt, Pt)> = runs.iter().map(|(start, _)| *start).collect();
        assert_eq!(
            starts,
            [
                (Pt(-1510.0), Pt(100.0)),
                (Pt(-1510.0 + 2222.0 * 15.0), Pt(100.0))
            ]
        );
        let counts: Vec<usize> = runs.iter().map(|(_, glyphs)| glyphs.len()).collect();
        assert_eq!(counts, [15, 1]);
    }

    #[test]
    fn a_slanted_run_starts_again_where_its_offsets_would_leave_a_pdf() {
        // Review of #270: synthetic italic slants about the run's start, so a glyph
        // raised 1,500 pt starts the run 500 pt farther right than the glyph after it
        // would be placed from it; that glyph, 700 pt on, would then be 33,200 pt from the
        // page's corner. It starts a run of its own.
        let item = GlyphItem {
            size: Pt(1638.0),
            h_scale: 6.0,
            synthetic_italic: true,
            ..run(32_000.0, 50.0, &[(0.0, 1500.0), (700.0, 0.0)])
        };
        let (drawn, painted) = painted_into_pdf(&[DisplayItem::Glyphs(item)]);
        assert!(painted.is_ok(), "{painted:?}");
        assert_eq!(
            runs_of(&drawn),
            [
                ((Pt(32_500.0), Pt(1650.0)), vec![(Pt(0.0), Pt(0.0))]),
                ((Pt(32_700.0), Pt(150.0)), vec![(Pt(0.0), Pt(0.0))]),
            ]
        );
        // Every run is slanted, and scaled, as the item is.
        for item in &drawn {
            if let Drawn::Glyphs(run) = item {
                assert_eq!(
                    (run.skew(), run.horizontal_scale()),
                    (SYNTHETIC_ITALIC_SKEW, 6.0)
                );
            }
        }
    }

    /// A glyph of the test font at (`x`, `y`) from its run's origin.
    fn glyph(x: f64, y: f64) -> DrawnGlyph {
        DrawnGlyph {
            glyph: officewright_font::GlyphId::new(1),
            x: Pt(x),
            y: Pt(y),
        }
    }

    #[test]
    fn a_cluster_goes_whole_into_a_run_that_holds_all_of_it() {
        // Review of #270: a cluster whose first glyph the run made so far holds, but not
        // its second, starts a run of its own rather than losing the second and its text.
        let item = GlyphItem {
            size: Pt(1638.0),
            h_scale: 6.0,
            clusters: vec![
                Cluster {
                    text: "A".to_owned(),
                    glyphs: vec![glyph(-32_667.0, 0.0)],
                },
                Cluster {
                    text: "e\u{301}".to_owned(),
                    glyphs: vec![glyph(50.0, 0.0), glyph(150.0, 0.0)],
                },
            ],
            ..run(0.0, 100.0, &[])
        };
        let (drawn, painted) = painted_into_pdf(&[DisplayItem::Glyphs(item)]);
        assert!(painted.is_ok(), "{painted:?}");
        let clusters: Vec<Vec<(String, usize)>> = drawn
            .iter()
            .filter_map(|item| match item {
                Drawn::Glyphs(run) => Some(
                    run.clusters()
                        .map(|(text, glyphs)| (text.to_owned(), glyphs.len()))
                        .collect(),
                ),
                _ => None,
            })
            .collect();
        assert_eq!(
            clusters,
            [vec![("A".to_owned(), 1)], vec![("e\u{301}".to_owned(), 2)]]
        );
    }

    #[test]
    fn a_cluster_wider_than_a_pdf_holds_keeps_what_its_run_holds() {
        // One cluster whose two glyphs both reach the page but are 65,300 pt apart: no
        // run holds both, so the first is drawn, without the cluster's text, and the PDF
        // is made.
        let item = GlyphItem {
            size: Pt(1638.0),
            h_scale: 6.0,
            clusters: vec![Cluster {
                text: "AB".to_owned(),
                glyphs: vec![glyph(-32_600.0, 0.0), glyph(32_700.0, 0.0)],
            }],
            ..run(0.0, 100.0, &[])
        };
        let (drawn, painted) = painted_into_pdf(&[DisplayItem::Glyphs(item)]);
        assert!(painted.is_ok(), "{painted:?}");
        assert_eq!(
            runs_of(&drawn),
            [((Pt(-32_600.0), Pt(100.0)), vec![(Pt(0.0), Pt(0.0))])]
        );
        let Some(Drawn::Glyphs(run)) = drawn.first() else {
            panic!("a glyph run");
        };
        let texts: Vec<&str> = run.clusters().map(|(text, _)| text).collect();
        assert_eq!(texts, [""]);
    }

    #[test]
    fn a_glyph_raised_onto_the_page_starts_its_run() {
        // On a line 40,000 pt down, raised 39,850 pt by its offset: 50 pt up the page,
        // where its run starts, with no offset left.
        let (drawn, _) = painted(
            &Document::default(),
            crate::testing::minimal(),
            &[DisplayItem::Glyphs(run(10.0, 40_000.0, &[(0.0, 39_850.0)]))],
        );
        let [Drawn::Glyphs(raised)] = drawn.as_slice() else {
            panic!("one run");
        };
        assert_eq!((raised.x(), raised.y()), (Pt(10.0), Pt(50.0)));
        let offsets: Vec<(Pt, Pt)> = raised
            .glyphs()
            .iter()
            .map(|glyph| (glyph.x(), glyph.y()))
            .collect();
        assert_eq!(offsets, [(Pt(0.0), Pt(0.0))]);
    }

    #[test]
    fn synthetic_italic_slants_about_the_baseline_wherever_the_run_starts() {
        // A glyph raised 6 pt, then one on the baseline 10 pt on: slanted by a third
        // about the baseline, they are drawn from 12 and 20 pt. The run starts where the
        // first is drawn, and the offsets are taken from it before the slant.
        let mut item = run(10.0, 50.0, &[(0.0, 6.0), (10.0, 0.0)]);
        item.synthetic_italic = true;
        let (drawn, _) = painted(
            &Document::default(),
            crate::testing::minimal(),
            &[DisplayItem::Glyphs(item)],
        );
        let [Drawn::Glyphs(slanted)] = drawn.as_slice() else {
            panic!("one run");
        };
        assert_eq!((slanted.x(), slanted.y()), (Pt(12.0), Pt(156.0)));
        let drawn_at: Vec<(f64, f64)> = slanted
            .glyphs()
            .iter()
            .map(|glyph| {
                let (dx, dy) = (glyph.x().0, glyph.y().0);
                (slanted.x().0 + dx + slanted.skew() * dy, slanted.y().0 + dy)
            })
            .collect();
        assert_eq!(drawn_at, [(12.0, 156.0), (20.0, 150.0)]);
        // From a line 300 pt below the page, raised 400 pt: on the page upright, but
        // slanted 133 pt right, past the page's reach of 40 pt, so nothing is drawn.
        let mut far = run(10.0, 500.0, &[(0.0, 400.0)]);
        far.synthetic_italic = true;
        let (drawn, _) = painted(
            &Document::default(),
            crate::testing::minimal(),
            &[DisplayItem::Glyphs(far)],
        );
        assert!(drawn.is_empty());
    }

    #[test]
    fn a_cluster_cut_to_several_glyphs_keeps_its_text() {
        // "EFG" of a glyph left of the page's reach and two on it.
        let mut item = run(10.0, 50.0, &[]);
        let glyph = |x: f64| DrawnGlyph {
            glyph: officewright_font::GlyphId::new(1),
            x: Pt(x),
            y: Pt(0.0),
        };
        item.clusters = vec![Cluster {
            text: "EFG".to_owned(),
            glyphs: vec![glyph(-200.0), glyph(30.0), glyph(40.0)],
        }];
        let (drawn, _) = painted(
            &Document::default(),
            crate::testing::minimal(),
            &[DisplayItem::Glyphs(item)],
        );
        let [Drawn::Glyphs(cut)] = drawn.as_slice() else {
            panic!("one run");
        };
        let texts: Vec<(String, usize)> = cut
            .clusters()
            .map(|(text, glyphs)| (text.to_owned(), glyphs.len()))
            .collect();
        assert_eq!(texts, [("EFG".to_owned(), 2)]);
    }

    #[test]
    fn a_cluster_cut_down_to_one_glyph_stands_for_no_text() {
        // "AB" of a glyph left of the page's reach and one on it; then "CD" whole.
        let mut item = run(10.0, 50.0, &[]);
        let glyph = |x: f64| DrawnGlyph {
            glyph: officewright_font::GlyphId::new(1),
            x: Pt(x),
            y: Pt(0.0),
        };
        item.clusters = vec![
            Cluster {
                text: "AB".to_owned(),
                glyphs: vec![glyph(-200.0), glyph(0.0)],
            },
            Cluster {
                text: "CD".to_owned(),
                glyphs: vec![glyph(10.0), glyph(20.0)],
            },
        ];
        let (drawn, _) = painted(
            &Document::default(),
            crate::testing::minimal(),
            &[DisplayItem::Glyphs(item)],
        );
        let [Drawn::Glyphs(cut)] = drawn.as_slice() else {
            panic!("one run");
        };
        let texts: Vec<(String, usize)> = cut
            .clusters()
            .map(|(text, glyphs)| (text.to_owned(), glyphs.len()))
            .collect();
        assert_eq!(texts, [(String::new(), 1), ("CD".to_owned(), 2)]);
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
        // Level, along the page but far above it.
        let above = LineItem {
            from: Point {
                x: Pt(0.0),
                y: Pt(-500.0),
            },
            to: Point {
                x: Pt(50.0),
                y: Pt(-500.0),
            },
            ..level
        };
        // Thicker than any dash pattern of it could be drawn: as thick as allowed.
        let thick = LineItem {
            width: Pt(10_000.0),
            style: LineStyle::LongDashed,
            ..level
        };
        let (drawn, report) = painted(
            &Document::default(),
            crate::testing::minimal(),
            &[
                DisplayItem::Line(level),
                DisplayItem::Line(off),
                DisplayItem::Line(above),
                DisplayItem::Line(thick),
            ],
        );
        // A margin of four widths and a point: 3 pt either side, at 150 pt up.
        let mut expected = pdf::GraphicsPath::new(Pt(-3.0), Pt(150.0));
        expected.line_to(Pt(103.0), Pt(150.0));
        let strokes: Vec<(&pdf::GraphicsPath, &pdf::Stroke)> = drawn
            .iter()
            .filter_map(|item| match item {
                Drawn::Stroke(path, stroke) => Some((path, stroke)),
                _ => None,
            })
            .collect();
        let [(path, _), (thick_path, thick_stroke)] = strokes.as_slice() else {
            panic!("two lines: {}", strokes.len());
        };
        assert_eq!(*path, &expected);
        // Thinned before it is cut: its margin is four of its thinned widths and a point,
        // 16,384.5 pt, which keeps it within what a PDF holds; cut first, it would keep
        // its start 40,000 pt left of the page.
        let thinned = LineItem {
            width: Pt(MAX_LINE_WIDTH),
            ..thick
        };
        let page = Area {
            width: 100.0,
            height: 200.0,
        };
        let cut = on_page(&thinned, page).expect("on the page");
        assert!((cut.from.x.0 + 16_384.5).abs() < 1e-6, "{:?}", cut.from);
        let (cut_path, _) = line_path(&cut, |y| Pt(200.0 - y.0));
        assert_eq!(*thick_path, &cut_path);
        assert_eq!(thick_stroke.width(), Pt(MAX_LINE_WIDTH));
        assert!(
            thick_stroke
                .dash()
                .iter()
                .all(|length| length.0 <= PDF_LIMIT)
        );
        assert!(
            messages(&report).iter().any(|m| m.contains("thicker")),
            "{report:?}"
        );
    }

    #[test]
    fn an_enlarged_page_keeps_its_content_at_its_top_left_corner() {
        // A page 2 pt square, made 3 pt: a rectangle 1 pt square at its top left corner
        // is drawn from 2 pt up, not 1.
        let book = crate::testing::book(vec![crate::testing::minimal()]);
        let mut fonts = Fonts::new(&book);
        let document = Document::default();
        let mut painter = Painter {
            document: &document,
            fonts: &mut fonts,
            pdf_fonts: HashMap::new(),
            images: HashMap::new(),
            pdf: pdf::Document::new(),
            notes: Vec::new(),
        };
        painter.pdf.set_compression(pdf::Compression::None);
        let page = Page {
            section: 0,
            width: Pt(2.0),
            height: Pt(2.0),
            number: 1,
            body: pages::Body {
                left: Pt(0.0),
                top: Pt(0.0),
                width: Pt(2.0),
                bottom: Pt(2.0),
            },
            fragments: Vec::new(),
            items: vec![DisplayItem::Rect {
                at: Point {
                    x: Pt(0.0),
                    y: Pt(0.0),
                },
                width: Pt(1.0),
                height: Pt(1.0),
                color: Color::BLACK,
            }],
            blank: false,
        };
        painter.page(&page).expect("the page");
        let pdf = String::from_utf8_lossy(&painter.pdf.to_bytes().expect("the PDF")).into_owned();
        assert!(pdf.contains("0 2 1 1 re"), "{pdf}");
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
    fn the_report_of_a_font_it_cannot_embed_says_why() {
        // Issue #258: the reason is in the error's causes.
        let font = crate::testing::with_name_copies(crate::testing::minimal(), 32);
        let (drawn, report) = painted(
            &Document::default(),
            font,
            &[DisplayItem::Glyphs(run(10.0, 50.0, &[(0.0, 0.0)]))],
        );
        assert!(drawn.is_empty());
        let messages: Vec<&str> = report
            .entries()
            .iter()
            .filter(|entry| entry.kind() == ReportKind::FontEmbedding)
            .map(|entry| entry.message())
            .collect();
        assert_eq!(
            messages,
            [
                "the text in Test is not drawn: cannot use the font data: font face 0: table \
              'name' is past a limit of the library: the name records to keep point at \
              1048584 bytes of strings, more than the 1048576 read before subsetting"
            ]
        );
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
