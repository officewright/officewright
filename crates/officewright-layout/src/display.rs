//! The display list: what laid-out paragraphs draw, as glyph runs, filled rectangles,
//! lines and images, in page coordinates (points from the top left corner of the page,
//! y down), for a painter to draw.

// reason: the layout itself uses this once pagination (plan PR 8) lays out pages; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::ops::Range;

use officewright_core::{Color, Pt};
use officewright_font::{FaceId, FontFace, GlyphId};

use crate::fonts::Fonts;
use crate::ir::{
    ColorValue, Crop, Document, Inline, ObjectContent, Paragraph, RunStyle, Shading,
    ShadingPattern, TabKind, TabLeader, UnderlineKind, VerticalPosition,
};
use crate::itemize::{Item, Itemized, TextRun};
use crate::lines::{Piece, PieceKind, PlacedGlyph, inline_at};
use crate::metrics::{TypoRule, line_metrics};
use crate::paragraph::ParagraphLayout;
use crate::report::{Location, ReportKind};

/// How far the baseline of a superscript rises, as a fraction of the size its
/// formatting gives. Hypothesis, like the size superscripts are drawn at, until a Word
/// reference shows it (`E-superscript.docx`).
const SUPERSCRIPT_RISE: f64 = 1.0 / 3.0;

/// How far the baseline of a subscript drops, as a fraction of the size its formatting
/// gives. Hypothesis, like [`SUPERSCRIPT_RISE`].
const SUBSCRIPT_DROP: f64 = 1.0 / 3.0;

/// The width of the line a bar tab draws. Hypothesis until a Word reference shows it
/// (`E-bar-tab.docx`).
const BAR_TAB_WIDTH: Pt = Pt(0.75);

/// The luminance (0 to 1) below which a background is dark, so that text in the
/// automatic colour is white on it rather than black. Hypothesis until a Word reference
/// shows where Word turns it (`E-auto-color.docx`).
const DARK_BACKGROUND: f64 = 0.5;

/// A point on a page: from its left edge and from its top edge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Point {
    pub(crate) x: Pt,
    pub(crate) y: Pt,
}

/// Something to draw.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DisplayItem {
    Glyphs(GlyphItem),
    /// A rectangle filled with a colour, from its top left corner.
    Rect {
        at: Point,
        width: Pt,
        height: Pt,
        color: Color,
    },
    Line(LineItem),
    Image(ImageItem),
}

/// Glyphs of one face, size and look, on one baseline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GlyphItem {
    pub(crate) face: FaceId,
    pub(crate) size: Pt,
    pub(crate) color: Color,
    /// The character scale (`w:w`): 1.0 is none.
    pub(crate) h_scale: f64,
    /// Bold the face lacks, drawn by stroking the outlines.
    pub(crate) synthetic_bold: bool,
    /// Italic the face lacks, drawn by slanting the glyphs.
    pub(crate) synthetic_italic: bool,
    /// Where the run starts, on its baseline.
    pub(crate) origin: Point,
    pub(crate) clusters: Vec<Cluster>,
}

/// Glyphs that stand for some text together.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cluster {
    /// The text as the document has it, for copying and searching; empty when the
    /// cluster before it stands for this one's text too.
    pub(crate) text: String,
    pub(crate) glyphs: Vec<DrawnGlyph>,
}

/// A glyph, from the origin of its run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DrawnGlyph {
    pub(crate) glyph: GlyphId,
    pub(crate) x: Pt,
    /// Up from the baseline.
    pub(crate) y: Pt,
}

/// A straight line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LineItem {
    pub(crate) from: Point,
    pub(crate) to: Point,
    pub(crate) width: Pt,
    pub(crate) style: LineStyle,
    pub(crate) color: Color,
}

/// How a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineStyle {
    Solid,
    Dotted,
    Dashed,
    LongDashed,
    DotDash,
    DotDotDash,
    Wave,
}

/// A picture, in a box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ImageItem {
    pub(crate) image: crate::ir::ImageId,
    /// The top left corner of the box.
    pub(crate) at: Point,
    pub(crate) width: Pt,
    pub(crate) height: Pt,
    pub(crate) crop: Option<Crop>,
}

/// What the display list of a paragraph is made from.
pub(crate) struct Painting<'a, 'f, 'b> {
    pub(crate) document: &'a Document,
    pub(crate) paragraph: &'a Paragraph,
    pub(crate) layout: &'a ParagraphLayout,
    pub(crate) fonts: &'f mut Fonts<'b>,
    pub(crate) location: &'a Location,
    /// What is behind the paragraph, such as a table cell's shading, which decides the
    /// automatic text colour; `None` for the page.
    pub(crate) background: Option<Color>,
}

impl Painting<'_, '_, '_> {
    /// Adds to `out` what `lines` of the paragraph draw, the first of them with its top
    /// at `origin.y`, in a column whose start edge is at `origin.x`: shading and
    /// highlighting first, then the glyphs, the lines and the pictures.
    pub(crate) fn paint(&mut self, lines: Range<usize>, origin: Point, out: &mut Vec<DisplayItem>) {
        let mut behind = Vec::new();
        let mut front = Vec::new();
        let mut top = origin.y.0;
        let itemized = &self.layout.itemized;
        let bars: Vec<Pt> = self
            .paragraph
            .props()
            .tabs()
            .iter()
            .filter(|stop| stop.kind() == TabKind::Bar)
            .map(|stop| stop.position())
            .collect();
        let bar_color = text_color(ColorValue::Auto, self.background);
        for line in self.layout.lines.get(lines).unwrap_or(&[]) {
            let baseline = top + line.baseline.0;
            // A bar tab draws a vertical line through every line of the paragraph.
            for bar in &bars {
                let x = Pt(origin.x.0 + bar.0);
                front.push(DisplayItem::Line(LineItem {
                    from: Point { x, y: Pt(top) },
                    to: Point {
                        x,
                        y: Pt(top + line.height.0),
                    },
                    width: BAR_TAB_WIDTH,
                    style: LineStyle::Solid,
                    color: bar_color,
                }));
            }
            for piece in &line.pieces {
                let x = origin.x.0 + piece.x.0;
                match &piece.kind {
                    PieceKind::Text { item, .. } | PieceKind::Hyphen { item, .. } => {
                        let Some(Item::Text(run)) = itemized.items.get(*item) else {
                            continue;
                        };
                        let style = self
                            .document
                            .run_style(run.style)
                            .cloned()
                            .unwrap_or_default();
                        let background =
                            self.background_of(&style, &mut behind, run, piece, x, baseline);
                        let color = text_color(style.color(), background);
                        if let Some(glyphs) = self.glyphs(run, &style, piece, x, baseline, color) {
                            front.push(DisplayItem::Glyphs(glyphs));
                        }
                        // Spaces that hang past the end of the line are not underlined
                        // (Word's default, without `ulTrailSpace`).
                        if !piece.hanging {
                            let words_only = style
                                .underline()
                                .is_some_and(|underline| underline.kind() == UnderlineKind::Words);
                            self.decorate(
                                run, &style, piece, x, baseline, color, words_only, &mut front,
                            );
                        }
                    }
                    PieceKind::Tab { item, leader } => {
                        self.leader(*item, *leader, x, piece.width.0, baseline, &mut front);
                    }
                    PieceKind::Object {
                        item,
                        height: Some(height),
                    } => {
                        if let Some(image) = self.picture(*item, x, baseline - height.0) {
                            front.push(DisplayItem::Image(image));
                        }
                    }
                    PieceKind::Object { .. } | PieceKind::Break { .. } => {}
                }
            }
            top += line.height.0;
        }
        out.extend(behind);
        out.extend(front);
    }

    /// The background of a piece of text in `style`: the run's shading or highlight,
    /// whose rectangle goes to `behind`, else the paragraph's shading, else what is
    /// behind the paragraph.
    fn background_of(
        &mut self,
        style: &RunStyle,
        behind: &mut Vec<DisplayItem>,
        run: &TextRun,
        piece: &Piece,
        x: f64,
        baseline: f64,
    ) -> Option<Color> {
        let shading = style
            .shading()
            .and_then(|shading| self.shading_color(&shading));
        let own = style.highlight().or(shading);
        if let Some(color) = own {
            // The run's own box: as high as its font reaches. Hypothesis until a Word
            // reference (`E-highlight.docx`).
            let metrics = self.metrics(run);
            behind.push(DisplayItem::Rect {
                at: Point {
                    x: Pt(x),
                    y: Pt(baseline - metrics.0),
                },
                width: piece.width,
                height: Pt(metrics.0 + metrics.1),
                color,
            });
            return Some(color);
        }
        let paragraph = self
            .paragraph
            .props()
            .shading()
            .and_then(|shading| self.shading_color(&shading));
        paragraph.or(self.background)
    }

    /// The colour a shading fills with: the fill, the pattern colour, or a mix of the
    /// two for a percentage. Other patterns are drawn as an even mix with the share of
    /// the pattern colour they cover, and reported.
    fn shading_color(&mut self, shading: &Shading) -> Option<Color> {
        let color = match shading.color() {
            ColorValue::Rgb(color) => color,
            ColorValue::Auto => Color::BLACK,
        };
        let fill = match shading.fill() {
            ColorValue::Rgb(fill) => Some(fill),
            ColorValue::Auto => None,
        };
        let share = match pattern_share(shading.pattern()) {
            Share::Exact(share) => share,
            Share::Approximate(share) => {
                self.fonts.report_once(
                    ReportKind::FeatureUnsupported,
                    self.location,
                    "a shading pattern of stripes or crosses is drawn as an even tint".to_owned(),
                );
                share
            }
        };
        if share <= 0.0 {
            return fill;
        }
        let under = fill.unwrap_or(Color::WHITE);
        Some(mix(color, under, share))
    }

    /// How far the font of `run` reaches above and below its baseline, at the size its
    /// formatting gives.
    fn metrics(&mut self, run: &TextRun) -> (f64, f64) {
        let face = run.face.and_then(|id| self.fonts.face(id));
        match face {
            Some(face) => {
                let metrics = line_metrics(face.face(), run.formatted_size, TypoRule::default());
                (metrics.ascent.0, metrics.descent.0)
            }
            None => (run.formatted_size.0 * 0.8, run.formatted_size.0 * 0.2),
        }
    }

    /// The glyph run of a piece of text.
    fn glyphs(
        &mut self,
        run: &TextRun,
        style: &RunStyle,
        piece: &Piece,
        x: f64,
        baseline: f64,
        color: Color,
    ) -> Option<GlyphItem> {
        let face = run.face?;
        let placed = piece.glyphs(&self.layout.itemized);
        if placed.is_empty() {
            return None;
        }
        let rise = rise(run, style);
        let origin = Point {
            x: Pt(x),
            y: Pt(baseline - rise),
        };
        let clusters = clusters(&self.layout.itemized, run, &placed, piece);
        Some(GlyphItem {
            face,
            size: run.size,
            color,
            h_scale: f64::from(style.scale()) / 100.0,
            synthetic_bold: run.synthetic_bold,
            synthetic_italic: run.synthetic_italic,
            origin,
            clusters: clusters
                .into_iter()
                .map(|(text, glyphs)| Cluster {
                    text,
                    glyphs: glyphs
                        .into_iter()
                        .map(|glyph| DrawnGlyph {
                            glyph: glyph.glyph,
                            x: Pt(glyph.x.0 - piece.x.0),
                            y: glyph.y,
                        })
                        .collect(),
                })
                .collect(),
        })
    }

    /// The underline and strikethrough of a piece of text: at the places and in the
    /// thickness its face gives, at the size it is drawn, in the underline's colour or
    /// the text's. An underline of words only leaves out the spaces.
    #[allow(clippy::too_many_arguments)] // reason: the piece's place and look, computed once by the caller
    fn decorate(
        &mut self,
        run: &TextRun,
        style: &RunStyle,
        piece: &Piece,
        x: f64,
        baseline: f64,
        color: Color,
        words_only: bool,
        out: &mut Vec<DisplayItem>,
    ) {
        let Some(face) = run.face.and_then(|id| self.fonts.face(id)) else {
            return;
        };
        let face = face.face();
        let scale = run.size.0 / f64::from(face.units_per_em().max(1));
        let baseline = baseline - rise(run, style);
        // The piece's glyphs are placed from the column's start edge, `x` from the page's.
        let to_page = x - piece.x.0;
        let spans: Vec<(f64, f64)> = if words_only {
            word_spans(&self.layout.itemized, &piece.glyphs(&self.layout.itemized))
                .into_iter()
                .map(|(start, end)| (start + to_page, end + to_page))
                .collect()
        } else {
            vec![(x, x + piece.width.0)]
        };
        if let Some(underline) = style.underline() {
            let line_color = match underline.color() {
                ColorValue::Rgb(color) => color,
                ColorValue::Auto => color,
            };
            let (position, thickness) = decoration(face, Decoration::Underline, scale);
            for (start, end) in &spans {
                push_lines(
                    out,
                    underline_lines(underline.kind()),
                    *start,
                    *end,
                    baseline - position,
                    thickness,
                    line_color,
                );
            }
        }
        let strikes = match (style.strike(), style.double_strike()) {
            (_, true) => Some(2),
            (true, false) => Some(1),
            (false, false) => None,
        };
        if let Some(count) = strikes {
            let (position, thickness) = decoration(face, Decoration::Strikeout, scale);
            let kind = if count == 2 {
                [(LineStyle::Solid, 1.0, -1.0), (LineStyle::Solid, 1.0, 1.0)].to_vec()
            } else {
                [(LineStyle::Solid, 1.0, 0.0)].to_vec()
            };
            push_lines(
                out,
                kind,
                x,
                x + piece.width.0,
                baseline - position,
                thickness,
                color,
            );
        }
    }

    /// The leader of a tab of item `item`: repeated glyphs of the tab's font, the last
    /// ending at the tab's end, or a line for the line leaders.
    fn leader(
        &mut self,
        item: usize,
        leader: TabLeader,
        x: f64,
        width: f64,
        baseline: f64,
        out: &mut Vec<DisplayItem>,
    ) {
        let character = match leader {
            TabLeader::None => return,
            TabLeader::Dot => '.',
            TabLeader::Hyphen => '-',
            TabLeader::MiddleDot => '\u{00B7}',
            TabLeader::Underscore | TabLeader::Heavy => '_',
        };
        let Some(Item::Tab(style_id)) = self.layout.itemized.items.get(item) else {
            return;
        };
        let style = self
            .document
            .run_style(*style_id)
            .cloned()
            .unwrap_or_default();
        let choice = self
            .fonts
            .choose(style.fonts().ascii(), style.bold(), style.italic());
        let Some(face_id) = choice.face else {
            return;
        };
        let Some(face) = self.fonts.face(face_id) else {
            return;
        };
        let face = face.face();
        let scale = style.size().0 / f64::from(face.units_per_em().max(1));
        let color = text_color(style.color(), self.background);
        if matches!(leader, TabLeader::Underscore | TabLeader::Heavy) {
            let (position, thickness) = decoration(face, Decoration::Underline, scale);
            let thickness = if leader == TabLeader::Heavy {
                thickness * 2.0
            } else {
                thickness
            };
            push_lines(
                out,
                vec![(LineStyle::Solid, 1.0, 0.0)],
                x,
                x + width,
                baseline - position,
                thickness,
                color,
            );
            return;
        }
        let Some(glyph) = face.glyph_id(character) else {
            return;
        };
        let advance = f64::from(face.advance_width(glyph).unwrap_or(0)) * scale;
        if advance <= 0.0 || width < advance {
            return;
        }
        // As many as fit, the last ending at the stop.
        let count = (width / advance).floor() as usize;
        let start = x + width - count as f64 * advance;
        let glyphs = (0..count)
            .map(|index| DrawnGlyph {
                glyph,
                x: Pt(index as f64 * advance),
                y: Pt(0.0),
            })
            .collect();
        out.push(DisplayItem::Glyphs(GlyphItem {
            face: face_id,
            size: style.size(),
            color,
            h_scale: 1.0,
            synthetic_bold: choice.synthetic_bold,
            synthetic_italic: choice.synthetic_italic,
            origin: Point {
                x: Pt(start),
                y: Pt(baseline),
            },
            clusters: vec![Cluster {
                text: "\t".to_owned(),
                glyphs,
            }],
        }));
    }

    /// The picture of an inline object of item `item`, its box's top left corner at
    /// `x`, `top`: inside the room its effects take.
    fn picture(&self, item: usize, x: f64, top: f64) -> Option<ImageItem> {
        let Some(Item::Object(path)) = self.layout.itemized.items.get(item) else {
            return None;
        };
        let Some(Inline::Object(object)) = inline_at(self.paragraph.inlines(), path) else {
            return None;
        };
        let (size, extent) = (object.size(), object.effect_extent());
        match object.content() {
            ObjectContent::Picture(picture) => Some(ImageItem {
                image: picture.image(),
                at: Point {
                    x: Pt(x + extent.left().0),
                    y: Pt(top + extent.top().0),
                },
                width: size.width(),
                height: size.height(),
                crop: picture.crop(),
            }),
        }
    }
}

/// How far the baseline of `run` in `style` is raised: superscripts up, subscripts down.
fn rise(run: &TextRun, style: &RunStyle) -> f64 {
    let size = run.formatted_size.0;
    let vertical = match style.vertical() {
        VerticalPosition::Superscript => size * SUPERSCRIPT_RISE,
        VerticalPosition::Subscript => -size * SUBSCRIPT_DROP,
        _ => 0.0,
    };
    style.position().0 + vertical
}

/// The glyphs `placed` of `piece` in clusters, each with the text it stands for: the
/// source text where the run's drawn text has as many characters as its source (each
/// cluster gets its characters' source characters), else the run's whole source with
/// its first cluster.
fn clusters(
    itemized: &Itemized,
    run: &TextRun,
    placed: &[PlacedGlyph],
    piece: &Piece,
) -> Vec<(String, Vec<PlacedGlyph>)> {
    let drawn = itemized.text.get(run.range.clone()).unwrap_or("");
    let source = itemized.source.get(run.source.clone()).unwrap_or("");
    let one_to_one = drawn.chars().count() == source.chars().count();
    let hyphen = matches!(piece.kind, PieceKind::Hyphen { .. });
    let mut out: Vec<(String, Vec<PlacedGlyph>)> = Vec::new();
    let mut index = 0;
    while let Some(first) = placed.get(index) {
        let mut end = index + 1;
        while placed
            .get(end)
            .is_some_and(|glyph| glyph.cluster == first.cluster)
        {
            end += 1;
        }
        let glyphs = placed.get(index..end).unwrap_or(&[]).to_vec();
        let text = if hyphen {
            // The hyphen drawn at a soft hyphen stands for no text of its own.
            String::new()
        } else {
            let next = placed.get(end).map_or_else(
                || match &piece.kind {
                    PieceKind::Text { text, .. } => text.end,
                    _ => first.cluster,
                },
                |glyph| glyph.cluster,
            );
            let start_char = itemized
                .text
                .get(run.range.start..first.cluster)
                .map_or(0, |before| before.chars().count());
            let chars = itemized
                .text
                .get(first.cluster..next)
                .map_or(0, |cluster| cluster.chars().count());
            if one_to_one {
                source.chars().skip(start_char).take(chars).collect()
            } else if first.cluster == run.range.start {
                source.to_owned()
            } else {
                String::new()
            }
        };
        out.push((text, glyphs));
        index = end;
    }
    out
}

/// The spans of `placed` that are words, leaving out spaces, where the glyphs are.
fn word_spans(itemized: &Itemized, placed: &[PlacedGlyph]) -> Vec<(f64, f64)> {
    let mut spans: Vec<(f64, f64)> = Vec::new();
    let mut open: Option<(f64, f64)> = None;
    for (index, glyph) in placed.iter().enumerate() {
        let next = placed
            .iter()
            .skip(index + 1)
            .find(|next| next.cluster != glyph.cluster)
            .map_or(glyph.cluster + 1, |next| next.cluster);
        let space = itemized
            .text
            .get(glyph.cluster..next.max(glyph.cluster))
            .is_some_and(|text| !text.is_empty() && text.chars().all(char::is_whitespace));
        let (start, end) = (glyph.x.0, glyph.x.0 + glyph.advance.0);
        match (space, open) {
            (true, Some(span)) => {
                spans.push(span);
                open = None;
            }
            (true, None) => {}
            (false, Some((from, _))) => open = Some((from, end)),
            (false, None) => open = Some((start, end)),
        }
    }
    spans.extend(open);
    spans
}

/// Which decoration of a face.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Decoration {
    Underline,
    Strikeout,
}

/// Where a decoration's stroke is centred, up from the baseline, and its thickness, in
/// points at `scale` points per unit: from the face, else from the usual proportions
/// (an underline a tenth of an em below the baseline, a strikeout a quarter above, a
/// twentieth of an em thick).
fn decoration(face: &FontFace, which: Decoration, scale: f64) -> (f64, f64) {
    let units_per_em = f64::from(face.units_per_em().max(1));
    let metrics = match which {
        Decoration::Underline => face.underline_metrics(),
        Decoration::Strikeout => face.strikeout_metrics(),
    };
    let (top, thickness) = match metrics {
        Some(metrics) if metrics.thickness() > 0 => (
            f64::from(metrics.position()),
            f64::from(metrics.thickness()),
        ),
        _ => {
            let top = match which {
                Decoration::Underline => -0.1,
                Decoration::Strikeout => 0.3,
            };
            (top * units_per_em, 0.05 * units_per_em)
        }
    };
    ((top - thickness / 2.0) * scale, thickness * scale)
}

/// The strokes of an underline of `kind`: each with its style, its thickness as a
/// multiple of the face's, and its offset from the face's position in thicknesses.
fn underline_lines(kind: UnderlineKind) -> Vec<(LineStyle, f64, f64)> {
    let one = |style, thickness| vec![(style, thickness, 0.0)];
    match kind {
        UnderlineKind::Double => vec![(LineStyle::Solid, 1.0, -1.0), (LineStyle::Solid, 1.0, 1.0)],
        UnderlineKind::WavyDouble => {
            vec![(LineStyle::Wave, 1.0, -1.5), (LineStyle::Wave, 1.0, 1.5)]
        }
        UnderlineKind::Thick => one(LineStyle::Solid, 2.0),
        UnderlineKind::Dotted => one(LineStyle::Dotted, 1.0),
        UnderlineKind::DottedHeavy => one(LineStyle::Dotted, 2.0),
        UnderlineKind::Dash => one(LineStyle::Dashed, 1.0),
        UnderlineKind::DashedHeavy => one(LineStyle::Dashed, 2.0),
        UnderlineKind::DashLong => one(LineStyle::LongDashed, 1.0),
        UnderlineKind::DashLongHeavy => one(LineStyle::LongDashed, 2.0),
        UnderlineKind::DotDash => one(LineStyle::DotDash, 1.0),
        UnderlineKind::DashDotHeavy => one(LineStyle::DotDash, 2.0),
        UnderlineKind::DotDotDash => one(LineStyle::DotDotDash, 1.0),
        UnderlineKind::DashDotDotHeavy => one(LineStyle::DotDotDash, 2.0),
        UnderlineKind::Wave => one(LineStyle::Wave, 1.0),
        UnderlineKind::WavyHeavy => one(LineStyle::Wave, 2.0),
        _ => one(LineStyle::Solid, 1.0),
    }
}

/// Adds horizontal `strokes` from `start` to `end`, centred `centre` (down the page)
/// plus each stroke's offset.
fn push_lines(
    out: &mut Vec<DisplayItem>,
    strokes: Vec<(LineStyle, f64, f64)>,
    start: f64,
    end: f64,
    centre: f64,
    thickness: f64,
    color: Color,
) {
    if end <= start {
        return;
    }
    for (style, factor, offset) in strokes {
        let y = Pt(centre + offset * thickness);
        out.push(DisplayItem::Line(LineItem {
            from: Point { x: Pt(start), y },
            to: Point { x: Pt(end), y },
            width: Pt(thickness * factor),
            style,
            color,
        }));
    }
}

/// The colour of text whose formatting gives `value`, on `background`: automatic text
/// is black, or white on a dark background.
fn text_color(value: ColorValue, background: Option<Color>) -> Color {
    match value {
        ColorValue::Rgb(color) => color,
        ColorValue::Auto => match background {
            Some(background) if luminance(background) < DARK_BACKGROUND => Color::WHITE,
            _ => Color::BLACK,
        },
    }
}

/// The luminance of `color`, 0 for black to 1 for white (ITU-R BT.601 weights).
fn luminance(color: Color) -> f64 {
    (0.299 * f64::from(color.red())
        + 0.587 * f64::from(color.green())
        + 0.114 * f64::from(color.blue()))
        / 255.0
}

/// `share` of `top` over `under`.
fn mix(top: Color, under: Color, share: f64) -> Color {
    let channel = |a: u8, b: u8| {
        let value = f64::from(a) * share + f64::from(b) * (1.0 - share);
        value.round().clamp(0.0, 255.0) as u8
    };
    Color::rgb(
        channel(top.red(), under.red()),
        channel(top.green(), under.green()),
        channel(top.blue(), under.blue()),
    )
}

/// The share of a shading's pattern colour.
enum Share {
    Exact(f64),
    /// Of stripes and crosses, which are not drawn as such.
    Approximate(f64),
}

/// The share of the pattern colour in shading `pattern`.
fn pattern_share(pattern: ShadingPattern) -> Share {
    use ShadingPattern as P;
    let exact = |percent: f64| Share::Exact(percent / 100.0);
    match pattern {
        P::Clear => exact(0.0),
        P::Solid => exact(100.0),
        P::Percent5 => exact(5.0),
        P::Percent10 => exact(10.0),
        P::Percent12 => exact(12.5),
        P::Percent15 => exact(15.0),
        P::Percent20 => exact(20.0),
        P::Percent25 => exact(25.0),
        P::Percent30 => exact(30.0),
        P::Percent35 => exact(35.0),
        P::Percent37 => exact(37.5),
        P::Percent40 => exact(40.0),
        P::Percent45 => exact(45.0),
        P::Percent50 => exact(50.0),
        P::Percent55 => exact(55.0),
        P::Percent60 => exact(60.0),
        P::Percent62 => exact(62.5),
        P::Percent65 => exact(65.0),
        P::Percent70 => exact(70.0),
        P::Percent75 => exact(75.0),
        P::Percent80 => exact(80.0),
        P::Percent85 => exact(85.0),
        P::Percent87 => exact(87.5),
        P::Percent90 => exact(90.0),
        P::Percent95 => exact(95.0),
        P::HorizontalCross | P::DiagonalCross => Share::Approximate(0.75),
        P::ThinHorizontalCross | P::ThinDiagonalCross => Share::Approximate(0.44),
        P::ThinHorizontalStripe
        | P::ThinVerticalStripe
        | P::ThinReverseDiagonalStripe
        | P::ThinDiagonalStripe => Share::Approximate(0.25),
        _ => Share::Approximate(0.5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        ImageData, ImageFormat, InlineObject, ParagraphProps, RunFonts, Size, TabKind, TabStop,
        Underline,
    };
    use crate::paragraph::lay_out;
    use crate::shape::Shaper;
    use crate::testing::{book, minimal};

    /// The display list of a paragraph of the inlines `make` gives for run styles made
    /// from the test font at 10 pt by `styles`, laid out 100 pt wide at the page's
    /// point 50, 20.
    fn painted(
        styles: &[RunStyle],
        make: impl Fn(&[crate::ir::RunStyleId], &mut Document) -> (ParagraphProps, Vec<Inline>),
    ) -> (Vec<DisplayItem>, usize) {
        let book = book(vec![minimal()]);
        let mut document = Document::default();
        let ids: Vec<_> = styles
            .iter()
            .map(|style| {
                document.add_run_style(
                    style
                        .clone()
                        .with_fonts(RunFonts::new("Officewright Test"))
                        .with_size(Pt(10.0)),
                )
            })
            .collect();
        let (props, inlines) = make(&ids, &mut document);
        let mark = ids.first().copied().expect("a style");
        let paragraph = Paragraph::new(mark).with_props(props).with_inlines(inlines);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let location = Location::default();
        let layout = lay_out(
            &document,
            &paragraph,
            Pt(100.0),
            &mut fonts,
            &mut shaper,
            &location,
        );
        let mut painting = Painting {
            document: &document,
            paragraph: &paragraph,
            layout: &layout,
            fonts: &mut fonts,
            location: &location,
            background: None,
        };
        let mut items = Vec::new();
        let origin = Point {
            x: Pt(50.0),
            y: Pt(20.0),
        };
        painting.paint(0..layout.lines.len(), origin, &mut items);
        let reported = fonts.into_report().len();
        (items, reported)
    }

    #[test]
    fn glyphs_underlines_and_highlights_are_where_the_text_is() {
        let underlined = RunStyle::default()
            .with_underline(Some(Underline::default()))
            .with_highlight(Some(Color::rgb(255, 255, 0)));
        let (items, reported) = painted(&[underlined], |ids, _| {
            let style = ids.first().copied().expect("a style");
            (ParagraphProps::default(), vec![Inline::text("AA A", style)])
        });
        assert_eq!(reported, 0);
        // The highlight first, behind the text: from the ascent (9 pt) to the descent
        // (2.5 pt) of the test font, around the baseline at 20 + 9 pt.
        let [
            DisplayItem::Rect {
                at,
                width,
                height,
                color,
            },
            DisplayItem::Glyphs(glyphs),
            DisplayItem::Line(line),
        ] = items.as_slice()
        else {
            panic!("{items:?}");
        };
        assert_eq!(
            (*at, *width, *height),
            (
                Point {
                    x: Pt(50.0),
                    y: Pt(20.0)
                },
                Pt(20.5),
                Pt(11.5)
            )
        );
        assert_eq!(*color, Color::rgb(255, 255, 0));
        assert_eq!(
            glyphs.origin,
            Point {
                x: Pt(50.0),
                y: Pt(29.0)
            }
        );
        assert_eq!(glyphs.color, Color::BLACK);
        let places: Vec<(u16, f64)> = glyphs
            .clusters
            .iter()
            .flat_map(|cluster| &cluster.glyphs)
            .map(|glyph| (glyph.glyph.to_u16(), glyph.x.0))
            .collect();
        assert_eq!(places, [(2, 0.0), (2, 6.0), (1, 12.0), (2, 14.5)]);
        let texts: Vec<&str> = glyphs
            .clusters
            .iter()
            .map(|cluster| cluster.text.as_str())
            .collect();
        assert_eq!(texts, ["A", "A", " ", "A"]);
        // The underline: the test font's top at -100 units, 50 thick: its middle 1.25 pt
        // below the baseline, half a point thick.
        assert_eq!((line.from.x, line.to.x), (Pt(50.0), Pt(70.5)));
        assert!(
            (line.from.y.0 - 30.25).abs() < 1e-9 && (line.width.0 - 0.5).abs() < 1e-9,
            "{line:?}"
        );
    }

    #[test]
    fn superscripts_rise_leaders_fill_tabs_and_pictures_sit_on_the_baseline() {
        let plain = RunStyle::default();
        let superscript = RunStyle::default().with_vertical(VerticalPosition::Superscript);
        let (items, _) = painted(&[plain, superscript], |ids, document| {
            let (style, raised) = (
                ids.first().copied().expect("a style"),
                ids.get(1).copied().expect("a style"),
            );
            let image = document.add_image(ImageData::new(ImageFormat::Png, Vec::new()));
            let object = InlineObject::new(
                ObjectContent::Picture(crate::ir::Picture::new(image)),
                Size::new(Pt(8.0), Pt(5.0)),
            );
            let props = ParagraphProps::default().with_tabs([TabStop::default()
                .with_position(Pt(40.0))
                .with_kind(TabKind::Start)
                .with_leader(TabLeader::Dot)]);
            let inlines = vec![
                Inline::text("A", style),
                Inline::text("A", raised),
                Inline::tab(style),
                Inline::Object(object),
            ];
            (props, inlines)
        });
        let glyph_runs: Vec<&GlyphItem> = items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Glyphs(glyphs) => Some(glyphs),
                _ => None,
            })
            .collect();
        // The superscript is a third of 10 pt higher, at 6.5 pt.
        let (Some(first), Some(raised)) = (glyph_runs.first(), glyph_runs.get(1)) else {
            panic!("{items:?}");
        };
        assert!((first.origin.y.0 - raised.origin.y.0 - 10.0 / 3.0).abs() < 1e-9);
        assert_eq!(raised.size, Pt(6.5));
        // The test font has no period: no leader is drawn.
        assert_eq!(glyph_runs.len(), 2);
        // The picture: its bottom on the baseline, at the tab's stop.
        let picture = items.iter().find_map(|item| match item {
            DisplayItem::Image(image) => Some(*image),
            _ => None,
        });
        let picture = picture.expect("a picture");
        assert_eq!(
            picture.at,
            Point {
                x: Pt(90.0),
                y: Pt(first.origin.y.0 - 5.0)
            }
        );
        assert_eq!((picture.width, picture.height), (Pt(8.0), Pt(5.0)));
    }

    #[test]
    fn colours_mix_and_automatic_text_turns_white_on_dark() {
        assert_eq!(
            mix(Color::BLACK, Color::WHITE, 0.2),
            Color::rgb(204, 204, 204)
        );
        assert_eq!(text_color(ColorValue::Auto, None), Color::BLACK);
        assert_eq!(
            text_color(ColorValue::Auto, Some(Color::rgb(0, 0, 80))),
            Color::WHITE
        );
        assert_eq!(
            text_color(ColorValue::Auto, Some(Color::rgb(200, 200, 200))),
            Color::BLACK
        );
        let red = Color::rgb(255, 0, 0);
        assert_eq!(text_color(ColorValue::Rgb(red), Some(Color::BLACK)), red);
        assert!(
            matches!(pattern_share(ShadingPattern::Percent12), Share::Exact(share) if (share - 0.125).abs() < 1e-12)
        );
        assert!(matches!(
            pattern_share(ShadingPattern::DiagonalStripe),
            Share::Approximate(_)
        ));
        assert!(luminance(Color::WHITE) > 0.99 && luminance(Color::BLACK) < 0.01);
    }
}
