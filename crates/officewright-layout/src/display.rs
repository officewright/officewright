//! The display list: what laid-out paragraphs draw, as glyph runs, filled rectangles,
//! lines and images, in page coordinates (points from the top left corner of the page,
//! y down), for a painter to draw.

// reason: the layout itself uses this once pagination (plan PR 8) lays out pages; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::ops::Range;

use officewright_core::{Color, Pt};
use officewright_font::{FaceId, FontFace, GlyphId};

use crate::fonts::{Choice, Fonts};
use crate::ir::{
    AnchorHorizontalAlign, AnchorVerticalAlign, AnchoredObject, Block, ColorValue, CompatFlag,
    Crop, Document, HorizontalPlacement, HorizontalReference, Inline, ObjectContent, Paragraph,
    RunStyle, Shading, ShadingPattern, Size, TabKind, TabLeader, TextBox, TextBoxAnchor,
    UnderlineKind, VerticalPlacement, VerticalPosition, VerticalReference, Wrap,
};
use crate::itemize::{Item, Itemized, TextRun, drawn_size};
use crate::lines::{EPSILON, Piece, PieceKind, PlacedGlyph, inline_at, no_font};
use crate::metrics::{TypoRule, line_metrics};
use crate::paragraph::ParagraphLayout;
use crate::report::{Location, ReportKind};
use crate::shape::ShapingFace;
use crate::tables::border_line;

/// How far the baseline of a superscript rises, as a fraction of the size its
/// formatting gives. Hypothesis, like the size superscripts are drawn at, until a Word
/// reference shows it (`E-superscript.docx`).
const SUPERSCRIPT_RISE: f64 = 1.0 / 3.0;

/// How far the baseline of a subscript drops, as a fraction of the size its formatting
/// gives. Hypothesis, like [`SUPERSCRIPT_RISE`].
const SUBSCRIPT_DROP: f64 = 1.0 / 3.0;

/// The width of the line a bar tab draws, as Word's reference of `E-bar-tab.docx` shows
/// it (#309).
const BAR_TAB_WIDTH: Pt = Pt(0.14);

/// The most glyphs a tab leader is drawn with. A longer one, which only a tiny font in a
/// wide tab makes, is drawn as a dotted, dashed or solid line instead, and reported.
const MAX_LEADER_GLYPHS: usize = 4096;

/// The grid Word puts the glyphs of a tab leader on, in points: 1/600 in, the unit of
/// its PDFs. The pitch of a leader is its glyph's advance rounded to it.
const LEADER_GRID: f64 = 72.0 / 600.0;

/// The most glyphs the leaders of one layout run are drawn with together, so that no
/// number of tabs, lines or pages makes them unbounded: far more than the leaders of a
/// long table of contents take. Past it, leaders are drawn as lines, and reported.
const MAX_RUN_LEADER_GLYPHS: usize = 1 << 18;

/// The most bar tabs a paragraph draws; more are reported.
const MAX_BAR_TABS: usize = 256;

/// The luma below which a background is dark, so that text in the automatic colour is
/// white on it rather than black, in the units of [`luma`]: 0.25 of white's. Word's
/// reference of `E-auto-color.docx` (#309) turns it white on 80 % grey (333333, 0.200)
/// and darker and on blue (0000FF, 002060), and keeps it black on 70 % grey (4D4D4D) and
/// lighter, on 75 % grey (404040, 0.251, in `E-bar-tab.docx`), red (FF0000, 0.299) and
/// green (00B050): the threshold lies in (0.200, 0.251], and 0.25 is darker than 75 %
/// grey. Whether a luma of exactly 0.25 (0B6700, for one) is dark is open: no reference
/// has such a colour.
const DARK_BACKGROUND: u32 = 63_750;

/// How far a paragraph's shading reaches past its outermost indent and its end indent,
/// each side: Word's reference of `E-para-shading.docx` (#309) shades 1.42 to 1.56 pt
/// further, within the rounding of its coordinates.
const SHADING_OUTSET: f64 = 1.5;

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
    /// The rotation clockwise about the box's centre, in degrees.
    pub(crate) rotation: f64,
    /// Whether the picture is mirrored left to right.
    pub(crate) flip_horizontal: bool,
    /// Whether the picture is mirrored top to bottom.
    pub(crate) flip_vertical: bool,
}

/// The page a paragraph is painted on, which the objects it anchors are positioned on:
/// its size and margins, in points from its top left corner, and whether its number is
/// odd, which decides its inside and outside margins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PageBox {
    pub(crate) width: f64,
    pub(crate) height: f64,
    /// The left, top, right and bottom edges of the area inside the margins its section
    /// sets.
    pub(crate) margins: [f64; 4],
    pub(crate) odd: bool,
}

/// A floating object drawn on a page: in front of the text or behind it, its place in the
/// stack of floating objects, and what it draws.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Float {
    pub(crate) behind: bool,
    pub(crate) z_order: u32,
    pub(crate) items: Vec<DisplayItem>,
}

/// Orders the floating objects of a page around `items`, what it draws otherwise: those
/// behind the text first, then `items`, then those in front, each group by its place in
/// the stack, in the order they come in when two share it.
pub(crate) fn stack(items: Vec<DisplayItem>, mut floats: Vec<Float>) -> Vec<DisplayItem> {
    if floats.is_empty() {
        return items;
    }
    floats.sort_by_key(|float| (!float.behind, float.z_order));
    let split = floats.partition_point(|float| float.behind);
    let front = floats.split_off(split);
    let mut out = Vec::with_capacity(items.len() + floats.len() + front.len());
    out.extend(floats.into_iter().flat_map(|float| float.items));
    out.extend(items);
    out.extend(front.into_iter().flat_map(|float| float.items));
    out
}

/// Where `object` goes on `page`: the top left corner of its frame. `anchor` is where the
/// object is anchored: its x in its line, the top and bottom of its line, and the top of
/// its paragraph on this page.
///
/// Across, the references follow ECMA-376 Part 1, ST_RelFromH and ST_AlignH: the inside
/// margin is the left one on odd pages and the right one on even pages; a column is the
/// area inside the margins, as every section is laid out in one column. Word's renders
/// confirm the page down and the column across, with a negative offset
/// (`letter-cfpb-validation-notice`), and that the offset positions the frame, not the box
/// its effects reach (the header logo of `report-hhs-usability-test-plan`).
///
/// Hypotheses until Word renders the rule documents of #314: down, the inside margin is
/// the top one on odd pages and the bottom one on even pages, as across (ST_RelFromV and
/// ST_AlignV do not say); a line is aligned to from its top to its bottom, a paragraph
/// and a character are points; a paragraph's top is that of its first line on this page,
/// below its space before, and in the later part of a paragraph split over pages that
/// part's top; the gutter counts into the left margin, and mirrored margins
/// (`w:mirrorMargins`, not laid out yet) do not swap the left and right ones.
pub(crate) fn float_position(
    object: &AnchoredObject,
    page: &PageBox,
    (x, (line_top, line_bottom), paragraph_top): (f64, (f64, f64), f64),
) -> (f64, f64) {
    let [left, top, right, bottom] = page.margins;
    let (width, height) = (object.size().width().0, object.size().height().0);
    let horizontal = object.horizontal();
    let (start, end) = match horizontal.relative_to() {
        HorizontalReference::Character => (x, x),
        HorizontalReference::Page => (0.0, page.width),
        HorizontalReference::LeftMargin => (0.0, left),
        HorizontalReference::RightMargin => (right, page.width),
        HorizontalReference::InsideMargin if page.odd => (0.0, left),
        HorizontalReference::InsideMargin => (right, page.width),
        HorizontalReference::OutsideMargin if page.odd => (right, page.width),
        HorizontalReference::OutsideMargin => (0.0, left),
        HorizontalReference::Margin | HorizontalReference::Column => (left, right),
    };
    let across = match horizontal.placement() {
        HorizontalPlacement::Offset(offset) => start + offset.0,
        HorizontalPlacement::Align(align) => {
            let align = match align {
                AnchorHorizontalAlign::Inside if page.odd => AnchorHorizontalAlign::Left,
                AnchorHorizontalAlign::Inside => AnchorHorizontalAlign::Right,
                AnchorHorizontalAlign::Outside if page.odd => AnchorHorizontalAlign::Right,
                AnchorHorizontalAlign::Outside => AnchorHorizontalAlign::Left,
                other => other,
            };
            match align {
                AnchorHorizontalAlign::Center => (start + end - width) / 2.0,
                AnchorHorizontalAlign::Right => end - width,
                AnchorHorizontalAlign::Left
                | AnchorHorizontalAlign::Inside
                | AnchorHorizontalAlign::Outside => start,
            }
        }
    };
    let vertical = object.vertical();
    let (start, end) = match vertical.relative_to() {
        VerticalReference::Page => (0.0, page.height),
        VerticalReference::TopMargin => (0.0, top),
        VerticalReference::BottomMargin => (bottom, page.height),
        VerticalReference::InsideMargin if page.odd => (0.0, top),
        VerticalReference::InsideMargin => (bottom, page.height),
        VerticalReference::OutsideMargin if page.odd => (bottom, page.height),
        VerticalReference::OutsideMargin => (0.0, top),
        VerticalReference::Paragraph => (paragraph_top, paragraph_top),
        VerticalReference::Line => (line_top, line_bottom),
        VerticalReference::Margin => (top, bottom),
    };
    let down = match vertical.placement() {
        VerticalPlacement::Offset(offset) => start + offset.0,
        VerticalPlacement::Align(align) => {
            let align = match align {
                AnchorVerticalAlign::Inside if page.odd => AnchorVerticalAlign::Top,
                AnchorVerticalAlign::Inside => AnchorVerticalAlign::Bottom,
                AnchorVerticalAlign::Outside if page.odd => AnchorVerticalAlign::Bottom,
                AnchorVerticalAlign::Outside => AnchorVerticalAlign::Top,
                other => other,
            };
            match align {
                AnchorVerticalAlign::Center => (start + end - height) / 2.0,
                AnchorVerticalAlign::Bottom => end - height,
                AnchorVerticalAlign::Top
                | AnchorVerticalAlign::Inside
                | AnchorVerticalAlign::Outside => start,
            }
        }
    };
    (across, down)
}

/// How a tab's leader is drawn: the family it asks for, its style, the size it is
/// drawn at, its character scale and its colour.
#[derive(Clone, Copy)]
struct Look<'a> {
    family: &'a str,
    bold: bool,
    italic: bool,
    size: Pt,
    scale: f64,
    color: Color,
}

/// The leader glyphs a layout run may still draw, [`MAX_RUN_LEADER_GLYPHS`] at first.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LeaderBudget {
    left: usize,
}

impl Default for LeaderBudget {
    fn default() -> Self {
        Self {
            left: MAX_RUN_LEADER_GLYPHS,
        }
    }
}

/// The tables that find the source text of the drawn bytes of a run whose drawn and
/// source characters correspond one to one: the number of characters before each byte of
/// its drawn text, and where each character of its source starts. Made once for a run
/// and kept for the pieces of it that follow, on the lines after.
#[derive(Debug, Default)]
struct SourceTables {
    /// The item of the run they are for.
    item: Option<usize>,
    chars_before: Vec<usize>,
    source_offsets: Vec<usize>,
}

impl SourceTables {
    /// The tables of `run`, item `item` of `itemized`, made unless they are at hand.
    fn of(&mut self, item: usize, itemized: &Itemized, run: &TextRun) -> &Self {
        if self.item != Some(item) {
            let drawn = itemized.text.get(run.range.clone()).unwrap_or("");
            let source = itemized.source.get(run.source.clone()).unwrap_or("");
            self.chars_before.clear();
            self.chars_before.reserve(drawn.len() + 1);
            let mut count = 0;
            for (offset, _) in drawn.char_indices() {
                self.chars_before.resize(offset + 1, count);
                count += 1;
            }
            self.chars_before.resize(drawn.len() + 1, count);
            self.source_offsets.clear();
            self.source_offsets
                .extend(source.char_indices().map(|(offset, _)| offset));
            self.source_offsets.push(source.len());
            self.item = Some(item);
        }
        self
    }

    /// The source text of the drawn bytes `drawn` of `run`.
    fn source_of(&self, itemized: &Itemized, run: &TextRun, drawn: Range<usize>) -> String {
        let char_at = |byte: usize| {
            byte.checked_sub(run.range.start)
                .and_then(|offset| self.chars_before.get(offset))
                .copied()
        };
        let (Some(from), Some(to)) = (char_at(drawn.start), char_at(drawn.end)) else {
            return String::new();
        };
        let (Some(&start), Some(&end)) =
            (self.source_offsets.get(from), self.source_offsets.get(to))
        else {
            return String::new();
        };
        let source = itemized.source.get(run.source.clone()).unwrap_or("");
        source.get(start..end).unwrap_or("").to_owned()
    }
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
    /// The leader glyphs the layout run may still draw.
    pub(crate) leaders: &'a mut LeaderBudget,
    /// The page the paragraph is painted on; `None` where floating objects are not
    /// positioned, which are then not drawn.
    pub(crate) page: Option<PageBox>,
    /// The floating objects the painted lines anchor, for the page to stack.
    pub(crate) floats: Vec<Float>,
    /// How far bar tabs reach above the first line painted and below the last: through
    /// the paragraph's space before and after, as far as its page lays them out.
    pub(crate) bar_reach: (f64, f64),
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
        let lines = self.layout.lines.get(lines).unwrap_or(&[]);
        self.bars(lines, origin, &mut front);
        let mut tables = SourceTables::default();
        // Word underlines the spaces at the end of a line only with this legacy option.
        let trailing = self
            .document
            .settings()
            .has_compat_flag(&CompatFlag::UlTrailSpace);
        for line in lines {
            let baseline = top + line.baseline.0;
            let bounds = (top, top + line.height.0);
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
                        // Spaces that hang past the end of the line have no box: like
                        // their underline, it would reach into the margin.
                        let width = if piece.hanging { Pt(0.0) } else { piece.width };
                        let face = run.face.and_then(|id| self.fonts.face(id));
                        let reach = self.reach(face.as_deref(), run.formatted_size, &style);
                        let background = self.background_of(
                            &style,
                            &mut behind,
                            (x, width),
                            (baseline, reach),
                            bounds,
                        );
                        let color = text_color(style.color(), background);
                        let placed = (piece, x, baseline);
                        if let Some(glyphs) =
                            self.glyphs((*item, run), &style, placed, color, &mut tables)
                        {
                            front.push(DisplayItem::Glyphs(glyphs));
                        }
                        // Spaces that hang past the end of the line are not underlined
                        // (Word's default, without `ulTrailSpace`).
                        if !piece.hanging || trailing {
                            let words_only = style
                                .underline()
                                .is_some_and(|underline| underline.kind() == UnderlineKind::Words);
                            self.decorate(
                                run, &style, piece, x, baseline, color, words_only, &mut front,
                            );
                        }
                    }
                    PieceKind::Tab { item, leader } => {
                        let width = if piece.hanging { Pt(0.0) } else { piece.width };
                        let layers = (&mut behind, &mut front);
                        self.tab(*item, *leader, (x, width), baseline, bounds, layers);
                    }
                    PieceKind::Object {
                        item,
                        height: Some(height),
                    } => self.object(*item, x, baseline - height.0, &mut front),
                    PieceKind::Object { item, height: None } => {
                        self.float(*item, (x, (top, top + line.height.0), origin.y.0));
                    }
                    PieceKind::Break { .. } => {}
                }
            }
            top += line.height.0;
        }
        out.extend(behind);
        out.extend(front);
    }

    /// The paragraph's shading behind `lines` of it, the first with its top at
    /// `origin.y`, in a column whose start edge is at `origin.x`: from
    /// [`SHADING_OUTSET`] before its start indent, or its first line's where that hangs
    /// further out, to as far past its end indent, over the lines only, as Word's
    /// reference of `E-para-shading.docx` shows it. Open until Word renders
    /// `E-para-shading-borders.docx`: whether two shaded paragraphs with space between them
    /// are joined across it, and how shading meets the paragraph's borders.
    pub(crate) fn paint_shading(
        &mut self,
        lines: Range<usize>,
        origin: Point,
        out: &mut Vec<DisplayItem>,
    ) {
        let props = self.paragraph.props();
        let Some(shading) = props.shading() else {
            return;
        };
        let Some(color) = self.shading_color(&shading) else {
            return;
        };
        let indent = props.indent();
        let start = indent
            .start()
            .0
            .min(indent.start().0 + indent.first_line().0)
            - SHADING_OUTSET;
        let end = self.layout.width.0 - indent.end().0 + SHADING_OUTSET;
        let height: f64 = self
            .layout
            .lines
            .get(lines)
            .unwrap_or(&[])
            .iter()
            .map(|line| line.height.0)
            .sum();
        if end > start && height > 0.0 {
            out.push(DisplayItem::Rect {
                at: Point {
                    x: Pt(origin.x.0 + start),
                    y: origin.y,
                },
                width: Pt(end - start),
                height: Pt(height),
                color,
            });
        }
    }

    /// The background of a piece of text or a tab in `style`, `span` its start and width,
    /// on the line's baseline `at.0`, reaching `at.1` above and below it (`reach`), on a
    /// line from `bounds.0` to `bounds.1`: the run's highlight or shading, whose rectangle
    /// goes to `behind`, else the paragraph's shading, else what is behind the paragraph.
    fn background_of(
        &mut self,
        style: &RunStyle,
        behind: &mut Vec<DisplayItem>,
        span: (f64, Pt),
        at: (f64, (f64, f64)),
        bounds: (f64, f64),
    ) -> Option<Color> {
        let shading = style
            .shading()
            .and_then(|shading| self.shading_color(&shading));
        // A highlight hides the run's shading, which is then not painted at all; either
        // is one rectangle over the room the run takes in its line (`reach`), cut to the
        // line (#309).
        let own = style.highlight().or(shading);
        if let Some(color) = own {
            let (x, width) = span;
            let (baseline, (above, below)) = at;
            let top = (baseline - above).max(bounds.0);
            let bottom = (baseline + below).min(bounds.1);
            if width.0 > 0.0 && bottom > top {
                behind.push(DisplayItem::Rect {
                    at: Point {
                        x: Pt(x),
                        y: Pt(top),
                    },
                    width,
                    height: Pt(bottom - top),
                    color,
                });
            }
            return Some(color);
        }
        self.paragraph_background()
    }

    /// What is behind the paragraph's text where no run has a box of its own: its
    /// shading, else what is behind the paragraph.
    fn paragraph_background(&mut self) -> Option<Color> {
        let paragraph = self
            .paragraph
            .props()
            .shading()
            .and_then(|shading| self.shading_color(&shading));
        paragraph.or(self.background)
    }

    /// The bar tabs of the paragraph, each a vertical line through `lines`, the first of
    /// them with its top at `origin.y`, [`BAR_TAB_WIDTH`] wide: from the top of the first
    /// line to the bottom of the last, and through the paragraph's space before and after
    /// as far as [`Painting::bar_reach`] says, as Word's reference of `E-bar-tab.docx`
    /// shows (case 04, 12 pt before and after, unbroken). In the automatic colour: on the
    /// paragraph's shading over its lines, and on what is behind the paragraph over its
    /// spacing, which the shading does not cover; a hypothesis, since the reference shades
    /// it 404040, on which that colour is black anyway.
    fn bars(&mut self, lines: &[crate::lines::Line], origin: Point, out: &mut Vec<DisplayItem>) {
        let mut bars: Vec<f64> = self
            .paragraph
            .props()
            .tabs()
            .iter()
            .filter(|stop| stop.kind() == TabKind::Bar)
            .map(|stop| stop.position().0)
            .collect();
        if bars.is_empty() || lines.is_empty() {
            return;
        }
        bars.sort_by(f64::total_cmp);
        bars.dedup();
        if bars.len() > MAX_BAR_TABS {
            bars.truncate(MAX_BAR_TABS);
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                self.location,
                format!("only the first {MAX_BAR_TABS} bar tabs of a paragraph are drawn"),
            );
        }
        let height: f64 = lines.iter().map(|line| line.height.0).sum();
        // The paragraph's shading covers its lines only (`E-para-shading.docx`), so over
        // its spacing a bar takes the colour on what is behind the paragraph.
        let on_lines = text_color(ColorValue::Auto, self.paragraph_background());
        let on_space = text_color(ColorValue::Auto, self.background);
        let (above, below) = self.bar_reach;
        let (top, bottom) = (origin.y.0, origin.y.0 + height);
        let segments = if on_lines == on_space {
            vec![(top - above, bottom + below, on_lines)]
        } else {
            vec![
                (top - above, top, on_space),
                (top, bottom, on_lines),
                (bottom, bottom + below, on_space),
            ]
        };
        for bar in bars {
            let x = Pt(origin.x.0 + bar);
            for &(from, to, color) in &segments {
                if to > from {
                    out.push(DisplayItem::Line(LineItem {
                        from: Point { x, y: Pt(from) },
                        to: Point { x, y: Pt(to) },
                        width: BAR_TAB_WIDTH,
                        style: LineStyle::Solid,
                        color,
                    }));
                }
            }
        }
    }

    /// The colour a shading fills with: see [`fill_color`].
    fn shading_color(&mut self, shading: &Shading) -> Option<Color> {
        fill_color(shading, self.fonts, self.location)
    }

    /// How far the box of a highlight or run shading in `style` reaches above and below
    /// the line's baseline: as far as the run's text does when the line's height is set
    /// (`lines::Breaker::set_height`), in `face` at `formatted`, the size its formatting
    /// gives: its ascent above and its descent below, further up by a raise of its
    /// position (`w:position`), from the baseline as well, so that a raised run's box
    /// keeps the room below it. A superscript or subscript keeps its full size and the
    /// baseline, as its line does. In a line of one size that is the whole line, as
    /// Word's reference of `E-highlight.docx` shows for superscript, subscript and raised
    /// text, and the box stays the run's own where the layout makes a line taller than
    /// Word does: the title of `contract-usds-statement-of-objectives`, whose 20 pt
    /// paragraph mark the layout counts (#359), is shaded 13.7 pt high, as by Word.
    /// Hypotheses until a Word reference (#346): the leading above the ascent, as a line
    /// puts it; text lowered by its position, whose box takes the room below; runs of
    /// another size than the rest of their line; at-least and multiple line spacing.
    fn reach(&self, face: Option<&ShapingFace>, formatted: Pt, style: &RunStyle) -> (f64, f64) {
        let metrics = face.map_or_else(
            || no_font(formatted),
            |face| line_metrics(face.face(), formatted, TypoRule::default()),
        );
        let leading = if self
            .document
            .settings()
            .has_compat_flag(&CompatFlag::NoLeading)
        {
            0.0
        } else {
            metrics.leading.0
        };
        let position = style.position().0;
        (
            leading + metrics.ascent.0 + position.max(0.0),
            metrics.descent.0 + (-position).max(0.0),
        )
    }

    /// The glyph run of a piece of text.
    fn glyphs(
        &mut self,
        (item, run): (usize, &TextRun),
        style: &RunStyle,
        (piece, x, baseline): (&Piece, f64, f64),
        color: Color,
        tables: &mut SourceTables,
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
        let itemized = &self.layout.itemized;
        let clusters = if run.sources.is_empty() {
            let tables = tables.of(item, itemized, run);
            clusters(itemized, run, &placed, piece, Some(tables))
        } else {
            clusters(itemized, run, &placed, piece, None)
        };
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

    /// The underline and strikethrough of a piece of text. An underline of words only
    /// leaves out the spaces; the hyphen drawn at a soft hyphen is part of its word.
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
        let baseline = baseline - rise(run, style);
        // The piece's glyphs are placed from the column's start edge, `x` from the page's.
        let to_page = x - piece.x.0;
        let hyphen = matches!(piece.kind, PieceKind::Hyphen { .. });
        let spans: Vec<(f64, f64)> = if words_only && !hyphen {
            word_spans(&self.layout.itemized, &piece.glyphs(&self.layout.itemized))
                .into_iter()
                .map(|(start, end)| (start + to_page, end + to_page))
                .collect()
        } else {
            vec![(x, x + piece.width.0)]
        };
        let full = (x, x + piece.width.0);
        decorations(
            face.face(),
            run.size,
            style,
            &spans,
            full,
            baseline,
            color,
            out,
        );
    }
}

/// The underline over `spans` and the strikethrough over `full` (start and end) of text
/// in `style` drawn in `face` at `size` on `baseline`: at the places and in the
/// thickness the face gives, at the size the text is drawn and raised with it, in the
/// underline's colour or the text's `color`. Hypothesis until a Word reference
/// (`E-underline.docx`): that placing and sizing each piece's underline by its own
/// drawn size makes a step under small capitals and superscripts, where Word may draw
/// one line.
#[allow(clippy::too_many_arguments)] // reason: the text's face, look, place and colour
fn decorations(
    face: &FontFace,
    size: Pt,
    style: &RunStyle,
    spans: &[(f64, f64)],
    full: (f64, f64),
    baseline: f64,
    color: Color,
    out: &mut Vec<DisplayItem>,
) {
    let scale = size.0 / f64::from(face.units_per_em().max(1));
    if let Some(underline) = style.underline() {
        let line_color = match underline.color() {
            ColorValue::Rgb(color) => color,
            ColorValue::Auto => color,
        };
        let (position, thickness) = decoration(face, Decoration::Underline, scale);
        for (start, end) in spans {
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
        // A double strike: two lines a thickness above and below the face's place.
        // Hypothesis until a Word reference (`E-strike.docx`).
        let kind = if count == 2 {
            [(LineStyle::Solid, 1.0, -1.0), (LineStyle::Solid, 1.0, 1.0)].to_vec()
        } else {
            [(LineStyle::Solid, 1.0, 0.0)].to_vec()
        };
        push_lines(
            out,
            kind,
            full.0,
            full.1,
            baseline - position,
            thickness,
            color,
        );
    }
}

impl Painting<'_, '_, '_> {
    /// A tab of item `item`, `span` its start and width, on `baseline`, on a line from
    /// `bounds.0` to `bounds.1`: the highlight or shading of its run style behind it,
    /// and its underline, strikethrough and leader in front, as the text of that style
    /// would have them. Its font is the one its line measured it in: the complex-script
    /// font for complex-script or right-to-left text.
    fn tab(
        &mut self,
        item: usize,
        leader: TabLeader,
        span: (f64, Pt),
        baseline: f64,
        bounds: (f64, f64),
        (behind, front): (&mut Vec<DisplayItem>, &mut Vec<DisplayItem>),
    ) {
        let Some(Item::Tab(style_id)) = self.layout.itemized.items.get(item) else {
            return;
        };
        let style = self
            .document
            .run_style(*style_id)
            .cloned()
            .unwrap_or_default();
        let (family, formatted, bold, italic) = if style.complex_script() || style.rtl() {
            (
                style.fonts().complex_script().to_owned(),
                style.size_complex_script(),
                style.bold_complex_script(),
                style.italic_complex_script(),
            )
        } else {
            (
                style.fonts().ascii().to_owned(),
                style.size(),
                style.bold(),
                style.italic(),
            )
        };
        let size = drawn_size(&style, formatted, false);
        let choice = self.fonts.choose(&family, bold, italic);
        let face = choice.face.and_then(|id| self.fonts.face(id));
        let raised = baseline - rise_at(&style, formatted.0);
        let reach = self.reach(face.as_deref(), formatted, &style);
        let background = self.background_of(&style, behind, span, (baseline, reach), bounds);
        let color = text_color(style.color(), background);
        let (x, width) = span;
        if width.0 <= 0.0 {
            return;
        }
        if let Some(face) = &face {
            // A words-only underline leaves tabs out, as it does spaces.
            let words_only = style
                .underline()
                .is_some_and(|underline| underline.kind() == UnderlineKind::Words);
            let spans = if words_only {
                Vec::new()
            } else {
                vec![(x, x + width.0)]
            };
            let full = (x, x + width.0);
            decorations(
                face.face(),
                size,
                &style,
                &spans,
                full,
                raised,
                color,
                front,
            );
        }
        let look = Look {
            family: &family,
            bold,
            italic,
            size,
            scale: f64::from(style.scale()) / 100.0,
            color,
        };
        self.leader(look, choice, leader, x, width.0, raised, front);
    }

    /// The leader of a tab drawn in `look`, whose font is `choice`, from `x` over
    /// `width`, as Word's references of `E-tab-leader.docx` show it (#309): whole glyphs
    /// of that font or, when it lacks the character, of a font that has it, also for the
    /// underscore and the heavy leader, which are both '_' (the heavy one is no thicker).
    /// The glyphs stand on a grid that starts at the left edge of the page, its pitch the
    /// glyph's advance at the tab's size and character scale rounded to 1/600 in, one in
    /// every cell wholly between the start of the tab and the text after it: so the
    /// leader ends up to a pitch before the stop. A leader that would take more than
    /// [`MAX_LEADER_GLYPHS`] glyphs, or whose pitch rounds to nothing, is a dotted,
    /// dashed or solid line instead, and reported; what Word draws there is a
    /// hypothesis.
    #[allow(clippy::too_many_arguments)] // reason: the tab's look, font, place and layer
    fn leader(
        &mut self,
        look: Look<'_>,
        choice: Choice,
        leader: TabLeader,
        x: f64,
        width: f64,
        baseline: f64,
        out: &mut Vec<DisplayItem>,
    ) {
        let (character, fallback) = match leader {
            TabLeader::None => return,
            TabLeader::Dot => ('.', LineStyle::Dotted),
            TabLeader::Hyphen => ('-', LineStyle::Dashed),
            TabLeader::MiddleDot => ('\u{00B7}', LineStyle::Dotted),
            TabLeader::Underscore | TabLeader::Heavy => ('_', LineStyle::Solid),
        };
        let choice = match choice.face {
            Some(id) if self.fonts.has_glyph(id, character) => choice,
            _ => match self
                .fonts
                .covering(character, look.family, look.bold, look.italic)
            {
                Some(covering) => {
                    if let Some(id) = covering.face {
                        self.fonts
                            .report_fallback(look.family, id, character, self.location);
                    }
                    covering
                }
                None => {
                    self.fonts.missing(character, self.location);
                    return;
                }
            },
        };
        let Some(face_id) = choice.face else {
            return;
        };
        let Some(face) = self.fonts.face(face_id) else {
            return;
        };
        let face = face.face();
        let scale = look.size.0 / f64::from(face.units_per_em().max(1));
        let Some(glyph) = face.glyph_id(character) else {
            return;
        };
        // The character scale (`w:w`) of the tab's style widens or narrows each glyph.
        let advance = f64::from(face.advance_width(glyph).unwrap_or(0)) * scale * look.scale;
        if advance <= 0.0 || width <= 0.0 {
            return;
        }
        let pitch = (advance / LEADER_GRID).round() * LEADER_GRID;
        // The cells from `first` to before `end`, each wholly within the tab.
        let (first, fit) = if pitch > 0.0 {
            let first = ((x - EPSILON) / pitch).ceil();
            let end = ((x + width + EPSILON) / pitch).floor();
            (first, end - first)
        } else {
            (0.0, f64::INFINITY)
        };
        if fit < 1.0 {
            return;
        }
        if fit > MAX_LEADER_GLYPHS as f64 || fit > self.leaders.left as f64 {
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                self.location,
                format!(
                    "a tab leader that would take more than {MAX_LEADER_GLYPHS} glyphs, or more \
                     than the {MAX_RUN_LEADER_GLYPHS} the leaders of a document take together, \
                     is drawn as a line"
                ),
            );
            let (position, thickness) = decoration(face, Decoration::Underline, scale);
            push_lines(
                out,
                vec![(fallback, 1.0, 0.0)],
                x,
                x + width,
                baseline - position,
                thickness,
                look.color,
            );
            return;
        }
        let count = fit as usize;
        self.leaders.left -= count;
        let glyphs = (0..count)
            .map(|index| DrawnGlyph {
                glyph,
                x: Pt(index as f64 * pitch),
                y: Pt(0.0),
            })
            .collect();
        out.push(DisplayItem::Glyphs(GlyphItem {
            face: face_id,
            size: look.size,
            color: look.color,
            h_scale: look.scale,
            synthetic_bold: choice.synthetic_bold,
            synthetic_italic: choice.synthetic_italic,
            origin: Point {
                x: Pt(first * pitch),
                y: Pt(baseline),
            },
            clusters: vec![Cluster {
                text: "\t".to_owned(),
                glyphs,
            }],
        }));
    }

    /// Adds the floating object of item `item`, if it is one, to the page's floats,
    /// positioned from its anchor (`float_position`). Text does not wrap around it yet,
    /// and that is reported.
    fn float(&mut self, item: usize, anchor: (f64, (f64, f64), f64)) {
        let Some(page) = self.page else {
            return;
        };
        let Some(Item::Anchored(path)) = self.layout.itemized.items.get(item) else {
            return;
        };
        let Some(Inline::Anchored(object)) = inline_at(self.paragraph.inlines(), path) else {
            return;
        };
        if object.wrap() != Wrap::None {
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                self.location,
                "text does not wrap around floating objects yet: it runs over or under them"
                    .to_owned(),
            );
        }
        let (x, y) = float_position(object, &page, anchor);
        let mut items = Vec::new();
        let at = Point { x: Pt(x), y: Pt(y) };
        self.content(item, object.content(), (at, object.size()), &mut items);
        self.floats.push(Float {
            behind: object.behind_text(),
            z_order: object.z_order(),
            items,
        });
    }

    /// Adds to `out` what the inline object of item `item` shows, its box's top left
    /// corner at `x`, `top`: inside the room its effects take.
    fn object(&mut self, item: usize, x: f64, top: f64, out: &mut Vec<DisplayItem>) {
        let Some(Item::Object(path)) = self.layout.itemized.items.get(item) else {
            return;
        };
        let Some(Inline::Object(object)) = inline_at(self.paragraph.inlines(), path) else {
            return;
        };
        let extent = object.effect_extent();
        let at = Point {
            x: Pt(x + extent.left().0),
            y: Pt(top + extent.top().0),
        };
        self.content(item, object.content(), (at, object.size()), out);
    }

    /// Adds to `out` what `content`, shown by the object of item `item`, draws in its
    /// frame, whose top left corner is at `at`.
    fn content(
        &mut self,
        item: usize,
        content: &ObjectContent,
        (at, size): (Point, Size),
        out: &mut Vec<DisplayItem>,
    ) {
        match content {
            ObjectContent::Picture(picture) => out.push(DisplayItem::Image(ImageItem {
                image: picture.image(),
                at,
                width: size.width(),
                height: size.height(),
                crop: picture.crop(),
                rotation: picture.rotation(),
                flip_horizontal: picture.flip_horizontal(),
                flip_vertical: picture.flip_vertical(),
            })),
            ObjectContent::TextBox(text_box) => self.text_box(item, text_box, (at, size), out),
        }
    }

    /// Adds to `out` what `text_box`, shown by the object of item `item`, draws in its
    /// frame, whose top left corner is at `at`: its fill, its paragraphs inside its insets
    /// and where its anchor puts them down the box, and its outline along the frame. Text
    /// taller than the room inside the insets starts at its top and runs on past the
    /// frame, as `vertOverflow="overflow"`, the default, has it (ECMA-376 Part 1,
    /// §21.1.2.1.1; a hypothesis until Word renders `E-textbox-anchor.docx`).
    fn text_box(
        &mut self,
        item: usize,
        text_box: &TextBox,
        (at, size): (Point, Size),
        out: &mut Vec<DisplayItem>,
    ) {
        let (left, top) = (at.x.0, at.y.0);
        let (width, height) = (size.width().0, size.height().0);
        if let Some(color) = text_box.fill() {
            out.push(DisplayItem::Rect {
                at,
                width: size.width(),
                height: size.height(),
                color,
            });
        }
        let layout = self.layout;
        if let Some(laid) = layout.text_box(item) {
            let insets = text_box.insets();
            let free = (height - insets.top().0 - insets.bottom().0 - laid.height).max(0.0);
            let start = top
                + insets.top().0
                + match text_box.anchor() {
                    TextBoxAnchor::Center => free / 2.0,
                    TextBoxAnchor::Bottom => free,
                    _ => 0.0,
                };
            for (block, inner, offset) in &laid.paragraphs {
                let Some(Block::Paragraph(paragraph)) = text_box.blocks().get(*block) else {
                    continue;
                };
                let mut painting = Painting {
                    document: self.document,
                    paragraph,
                    layout: inner,
                    fonts: &mut *self.fonts,
                    location: self.location,
                    background: text_box.fill().or(self.background),
                    leaders: &mut *self.leaders,
                    // Floating objects in a box are left out, and reported, as the box is
                    // laid out.
                    page: None,
                    floats: Vec::new(),
                    // Hypothesis, as no reference shows them: in a text box, bar tabs
                    // reach only through the lines.
                    bar_reach: (0.0, 0.0),
                };
                let origin = Point {
                    x: Pt(left + insets.left().0),
                    y: Pt(start + offset),
                };
                let lines = 0..inner.lines.len();
                painting.paint_shading(lines.clone(), origin, out);
                painting.paint(lines, origin, out);
            }
        }
        if let Some(outline) = text_box.outline() {
            // The lines across reach over the ends of those down, so that the corners
            // are closed.
            let reach = outline.width().0 / 2.0;
            let (right, bottom) = (left + width, top + height);
            border_line(&outline, (left - reach, top), (right + reach, top), out);
            border_line(
                &outline,
                (left - reach, bottom),
                (right + reach, bottom),
                out,
            );
            border_line(&outline, (left, top), (left, bottom), out);
            border_line(&outline, (right, top), (right, bottom), out);
        }
    }
}

/// How far the baseline of `run` in `style` is raised: superscripts up, subscripts down.
fn rise(run: &TextRun, style: &RunStyle) -> f64 {
    rise_at(style, run.formatted_size.0)
}

/// How far the baseline of text in `style` is raised, at `size`, the size its formatting
/// gives.
fn rise_at(style: &RunStyle, size: f64) -> f64 {
    let vertical = match style.vertical() {
        VerticalPosition::Superscript => size * SUPERSCRIPT_RISE,
        VerticalPosition::Subscript => -size * SUBSCRIPT_DROP,
        _ => 0.0,
    };
    style.position().0 + vertical
}

/// The glyphs `placed` of `piece` in clusters, each with the text it stands for: the
/// source characters of its drawn characters, each source character with the cluster
/// that draws the first character drawn for it (`ß` in capitals, `SS`, goes with the
/// first `S`). A cluster's text runs to the next cluster's start in the text, whichever
/// order the glyphs are in (right-to-left text has them backwards). `tables` are the run's
/// when its characters correspond one to one.
fn clusters(
    itemized: &Itemized,
    run: &TextRun,
    placed: &[PlacedGlyph],
    piece: &Piece,
    tables: Option<&SourceTables>,
) -> Vec<(String, Vec<PlacedGlyph>)> {
    let hyphen = matches!(piece.kind, PieceKind::Hyphen { .. });
    let piece_end = match &piece.kind {
        PieceKind::Text { text, .. } => text.end,
        _ => run.range.end,
    };
    let mut starts: Vec<usize> = placed.iter().map(|glyph| glyph.cluster).collect();
    starts.sort_unstable();
    starts.dedup();
    let text_end = |cluster: usize| {
        starts
            .get(starts.partition_point(|&start| start <= cluster))
            .copied()
            .unwrap_or(piece_end)
            .max(cluster)
    };
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
        } else if let Some(tables) = tables.filter(|_| run.sources.is_empty()) {
            tables.source_of(itemized, run, first.cluster..text_end(first.cluster))
        } else {
            expanded_source(itemized, run, first.cluster..text_end(first.cluster))
        };
        out.push((text, glyphs));
        index = end;
    }
    out
}

/// The source text of the drawn bytes `drawn` of `run`, whose drawn characters do not
/// correspond one to one to its source characters: the source of every drawn character
/// in `drawn` that is the first drawn for its source character.
fn expanded_source(itemized: &Itemized, run: &TextRun, drawn: Range<usize>) -> String {
    let first = run.sources.partition_point(|(at, _)| *at < drawn.start);
    let mut previous = first
        .checked_sub(1)
        .and_then(|index| run.sources.get(index))
        .map(|(_, source)| source);
    let mut text = String::new();
    for (_, source) in run
        .sources
        .iter()
        .skip(first)
        .take_while(|(at, _)| *at < drawn.end)
    {
        if previous != Some(source) {
            text.push_str(itemized.source.get(source.clone()).unwrap_or(""));
        }
        previous = Some(source);
    }
    text
}

/// The spans of `placed` that are words, leaving out spaces, where the glyphs are. The
/// glyphs of a cluster follow one another, and each run of them is looked at once, so the
/// work is in proportion to the glyphs, however many one cluster has (#399).
fn word_spans(itemized: &Itemized, placed: &[PlacedGlyph]) -> Vec<(f64, f64)> {
    let mut spans: Vec<(f64, f64)> = Vec::new();
    let mut open: Option<(f64, f64)> = None;
    let mut clusters = placed.chunk_by(|a, b| a.cluster == b.cluster).peekable();
    while let Some(glyphs) = clusters.next() {
        let Some(cluster) = glyphs.first().map(|glyph| glyph.cluster) else {
            continue;
        };
        // The cluster's text runs to where the next one starts.
        let next = clusters
            .peek()
            .and_then(|next| next.first())
            .map_or(cluster.saturating_add(1), |next| next.cluster);
        let space = itemized
            .text
            .get(cluster..next.max(cluster))
            .is_some_and(|text| !text.is_empty() && text.chars().all(char::is_whitespace));
        for glyph in glyphs {
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
/// (an underline a tenth of an em below the baseline, a strikeout three tenths above,
/// a twentieth of an em thick).
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
            // A face that gives none: a tenth of an em below the baseline, or three
            // tenths above, a twentieth thick. Ours, not Word's.
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
/// Hypothesis until a Word reference (`E-underline.docx`): doubles a thickness above and
/// below, heavy kinds twice as thick, wavy doubles further apart.
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

/// The colour `shading` fills with: the fill, the pattern colour, or a mix of the two
/// for a percentage. Other patterns are drawn as an even mix with the share of
/// the pattern colour they cover, and reported.
pub(crate) fn fill_color(
    shading: &Shading,
    fonts: &mut Fonts<'_>,
    location: &Location,
) -> Option<Color> {
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
            fonts.report_once(
                ReportKind::FeatureUnsupported,
                location,
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

/// The colour of text whose formatting gives `value`, on `background`: automatic text
/// is black, or white on a dark background.
fn text_color(value: ColorValue, background: Option<Color>) -> Color {
    match value {
        ColorValue::Rgb(color) => color,
        ColorValue::Auto => match background {
            Some(background) if luma(background) < DARK_BACKGROUND => Color::WHITE,
            _ => Color::BLACK,
        },
    }
}

/// The luma of `color` with the weights of ITU-R BT.601 in thousandths, from 0 for black
/// to 255,000 for white: in whole numbers, so that a colour on the threshold compares
/// exactly (333333 is 0.2 of white's, which floating point makes a little less).
fn luma(color: Color) -> u32 {
    299 * u32::from(color.red()) + 587 * u32::from(color.green()) + 114 * u32::from(color.blue())
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
    use crate::report::LayoutReport;
    use crate::shape::Shaper;
    use crate::testing::{book, minimal, patched, renamed, with_period};

    #[test]
    fn words_are_found_by_cluster_however_many_glyphs_a_cluster_has() {
        // Issue #399: the words a words-only underline covers are found cluster by cluster.
        // "ab cd", where "a" is drawn with three glyphs and "c" with two, makes two words.
        let glyph = |x: f64, cluster: usize| PlacedGlyph {
            glyph: officewright_font::GlyphId::new(1),
            x: Pt(x),
            y: Pt(0.0),
            advance: Pt(1.0),
            cluster,
        };
        let itemized = Itemized {
            text: "ab cd".to_owned(),
            ..Itemized::default()
        };
        let placed = [
            glyph(0.0, 0),
            glyph(1.0, 0),
            glyph(2.0, 0),
            glyph(3.0, 1),
            glyph(4.0, 2),
            glyph(5.0, 3),
            glyph(6.0, 3),
            glyph(7.0, 4),
        ];
        assert_eq!(word_spans(&itemized, &placed), [(0.0, 4.0), (5.0, 8.0)]);
        // A space that is the last cluster of the piece is no word; nor is a space of more
        // than one byte, which runs to where the next cluster starts.
        let text = |text: &str| Itemized {
            text: text.to_owned(),
            ..Itemized::default()
        };
        let trailing = [glyph(0.0, 0), glyph(1.0, 1), glyph(2.0, 2)];
        assert_eq!(word_spans(&text("ab "), &trailing), [(0.0, 2.0)]);
        let ideographic = [glyph(0.0, 0), glyph(1.0, 1), glyph(2.0, 4)];
        assert_eq!(
            word_spans(&text("a\u{3000}b"), &ideographic),
            [(0.0, 1.0), (2.0, 3.0)]
        );
        // One cluster of up to 262,144 glyphs is one word, found in time in proportion to
        // its glyphs: well under a second in all in a debug build, where the search for each
        // glyph's next cluster passed the bound at 131,072 glyphs.
        let started = std::time::Instant::now();
        for size in (14..=18).map(|power| 1_u32 << power) {
            let many: Vec<PlacedGlyph> = (0..size).map(|n| glyph(f64::from(n), 0)).collect();
            assert_eq!(word_spans(&text("H"), &many), [(0.0, f64::from(size))]);
            assert!(
                started.elapsed() < LINEAR_TIME,
                "clusters up to {size} glyphs took {:?}: finding words is no longer linear",
                started.elapsed()
            );
        }
    }

    /// The longest the clusters of the test above may take in all.
    const LINEAR_TIME: std::time::Duration = std::time::Duration::from_secs(10);

    #[test]
    fn floating_objects_go_where_their_references_and_placements_say() {
        use crate::ir::{HorizontalAnchor, ImageId, Picture, VerticalAnchor};

        // A page 600 x 800 with margins 72 left and top, 72 right, 72 bottom; the object is
        // 100 x 50, anchored at x 150 in a line from 300 to 320, in a paragraph whose top is
        // 280.
        let mut document = Document::default();
        let image: ImageId = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
        let page = |odd| PageBox {
            width: 600.0,
            height: 800.0,
            margins: [72.0, 72.0, 528.0, 728.0],
            odd,
        };
        let at = |across: (HorizontalReference, HorizontalPlacement),
                  down: (VerticalReference, VerticalPlacement),
                  odd: bool| {
            let object = AnchoredObject::new(
                ObjectContent::Picture(Picture::new(image)),
                Size::new(Pt(100.0), Pt(50.0)),
                HorizontalAnchor::default()
                    .with_relative_to(across.0)
                    .with_placement(across.1),
                VerticalAnchor::default()
                    .with_relative_to(down.0)
                    .with_placement(down.1),
            );
            float_position(&object, &page(odd), (150.0, (300.0, 320.0), 280.0))
        };
        let offset = |d: f64| HorizontalPlacement::Offset(Pt(d));
        let down = |d: f64| VerticalPlacement::Offset(Pt(d));
        let align = HorizontalPlacement::Align;
        let valign = VerticalPlacement::Align;
        use AnchorHorizontalAlign as H;
        use AnchorVerticalAlign as V;
        use HorizontalReference as X;
        use VerticalReference as Y;
        // Offsets from each reference's start.
        let cases = [
            ((X::Page, offset(10.0)), (Y::Page, down(20.0)), (10.0, 20.0)),
            (
                (X::Margin, offset(10.0)),
                (Y::Margin, down(20.0)),
                (82.0, 92.0),
            ),
            (
                (X::Column, offset(0.0)),
                (Y::TopMargin, down(0.0)),
                (72.0, 0.0),
            ),
            (
                (X::Character, offset(5.0)),
                (Y::Line, down(5.0)),
                (155.0, 305.0),
            ),
            (
                (X::LeftMargin, offset(0.0)),
                (Y::Paragraph, down(-10.0)),
                (0.0, 270.0),
            ),
            (
                (X::RightMargin, offset(0.0)),
                (Y::BottomMargin, down(0.0)),
                (528.0, 728.0),
            ),
        ];
        for (across, vertical, expected) in cases {
            assert_eq!(
                at(across, vertical, true),
                expected,
                "{across:?} {vertical:?}"
            );
        }
        // Alignments within each reference.
        let cases = [
            (
                (X::Page, align(H::Center)),
                (Y::Page, valign(V::Center)),
                (250.0, 375.0),
            ),
            (
                (X::Margin, align(H::Right)),
                (Y::Margin, valign(V::Bottom)),
                (428.0, 678.0),
            ),
            (
                (X::Margin, align(H::Left)),
                (Y::Margin, valign(V::Top)),
                (72.0, 72.0),
            ),
            (
                (X::Character, align(H::Center)),
                (Y::Line, valign(V::Top)),
                (100.0, 300.0),
            ),
            // Within the margin areas: right in the right margin (528 to 600), centred in
            // the left one (0 to 72); at the bottom of the top margin (0 to 72), centred in
            // the bottom one (728 to 800).
            (
                (X::RightMargin, align(H::Right)),
                (Y::TopMargin, valign(V::Bottom)),
                (500.0, 22.0),
            ),
            (
                (X::LeftMargin, align(H::Center)),
                (Y::BottomMargin, valign(V::Center)),
                (-14.0, 739.0),
            ),
            // A line is aligned to from its top to its bottom (300 to 320): its bottom and
            // its centre.
            (
                (X::Column, align(H::Right)),
                (Y::Line, valign(V::Bottom)),
                (428.0, 270.0),
            ),
            (
                (X::Column, align(H::Center)),
                (Y::Line, valign(V::Center)),
                (250.0, 285.0),
            ),
        ];
        for (across, vertical, expected) in cases {
            assert_eq!(
                at(across, vertical, true),
                expected,
                "{across:?} {vertical:?}"
            );
        }
        // Inside and outside: the left (top) margin on odd pages, the right (bottom) one on
        // even pages, and the other way round.
        let inside = (X::InsideMargin, offset(0.0));
        let outside = (X::OutsideMargin, offset(0.0));
        let top_inside = (Y::InsideMargin, down(0.0));
        let top_outside = (Y::OutsideMargin, down(0.0));
        assert_eq!(at(inside, top_inside, true), (0.0, 0.0));
        assert_eq!(at(inside, top_inside, false), (528.0, 728.0));
        assert_eq!(at(outside, top_outside, true), (528.0, 728.0));
        assert_eq!(at(outside, top_outside, false), (0.0, 0.0));
        let margin_inside = (X::Margin, align(H::Inside));
        let margin_outside = (Y::Margin, valign(V::Outside));
        assert_eq!(at(margin_inside, margin_outside, true), (72.0, 678.0));
        assert_eq!(at(margin_inside, margin_outside, false), (428.0, 72.0));
        let margin_outside_across = (X::Margin, align(H::Outside));
        let margin_inside_down = (Y::Margin, valign(V::Inside));
        assert_eq!(
            at(margin_outside_across, margin_inside_down, true),
            (428.0, 72.0)
        );
        assert_eq!(
            at(margin_outside_across, margin_inside_down, false),
            (72.0, 678.0)
        );
    }

    #[test]
    fn floats_stack_behind_and_in_front_of_the_rest_by_their_place() {
        let rect = |x: f64| DisplayItem::Rect {
            at: Point {
                x: Pt(x),
                y: Pt(0.0),
            },
            width: Pt(1.0),
            height: Pt(1.0),
            color: Color::BLACK,
        };
        let float = |behind, z_order, x| Float {
            behind,
            z_order,
            items: vec![rect(x)],
        };
        let floats = vec![
            float(false, 5, 1.0),
            float(true, 9, 2.0),
            float(false, 2, 3.0),
            float(true, 1, 4.0),
            float(false, 5, 5.0),
        ];
        let stacked = stack(vec![rect(0.0)], floats);
        let order: Vec<f64> = stacked
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Rect { at, .. } => Some(at.x.0),
                _ => None,
            })
            .collect();
        // Behind: z 1, then 9; the rest; in front: z 2, then the two of z 5 as they came.
        assert_eq!(order, [4.0, 2.0, 0.0, 3.0, 1.0, 5.0]);
        assert_eq!(stack(vec![rect(0.0)], Vec::new()), vec![rect(0.0)]);
    }

    /// The display list of a paragraph of the inlines `make` gives for run styles made
    /// from the test font at 10 pt by `styles`, laid out 100 pt wide at the page's
    /// point 50, 20.
    fn painted(
        styles: &[RunStyle],
        make: impl Fn(&[crate::ir::RunStyleId], &mut Document) -> (ParagraphProps, Vec<Inline>),
    ) -> (Vec<DisplayItem>, usize) {
        let (items, report) = painted_with(vec![minimal()], styles, make);
        (items, report.len())
    }

    /// [`painted`] with the fonts `fonts`, and the whole report.
    fn painted_with(
        fonts: Vec<Vec<u8>>,
        styles: &[RunStyle],
        make: impl Fn(&[crate::ir::RunStyleId], &mut Document) -> (ParagraphProps, Vec<Inline>),
    ) -> (Vec<DisplayItem>, LayoutReport) {
        painted_in(crate::ir::DocumentSettings::default(), fonts, styles, make)
    }

    /// [`painted_with`] in a document with `settings`.
    fn painted_in(
        settings: crate::ir::DocumentSettings,
        fonts: Vec<Vec<u8>>,
        styles: &[RunStyle],
        make: impl Fn(&[crate::ir::RunStyleId], &mut Document) -> (ParagraphProps, Vec<Inline>),
    ) -> (Vec<DisplayItem>, LayoutReport) {
        let book = book(fonts);
        let mut document = Document::new(settings);
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
        let mut leaders = LeaderBudget::default();
        let mut painting = Painting {
            document: &document,
            paragraph: &paragraph,
            layout: &layout,
            fonts: &mut fonts,
            location: &location,
            background: None,
            leaders: &mut leaders,
            page: None,
            floats: Vec::new(),
            bar_reach: (0.0, 0.0),
        };
        let mut items = Vec::new();
        let origin = Point {
            x: Pt(50.0),
            y: Pt(20.0),
        };
        painting.paint(0..layout.lines.len(), origin, &mut items);
        (items, fonts.into_report())
    }

    /// The glyph runs of `items`.
    fn glyph_runs(items: &[DisplayItem]) -> Vec<&GlyphItem> {
        items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Glyphs(glyphs) => Some(glyphs),
                _ => None,
            })
            .collect()
    }

    /// The filled rectangles of `items`: top left corner, width, height and colour.
    fn rects(items: &[DisplayItem]) -> Vec<(Point, Pt, Pt, Color)> {
        items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Rect {
                    at,
                    width,
                    height,
                    color,
                } => Some((*at, *width, *height, *color)),
                _ => None,
            })
            .collect()
    }

    /// A tab to 40 pt with `leader`, after an `A`.
    fn leader_paragraph(
        ids: &[crate::ir::RunStyleId],
        leader: TabLeader,
        tab_style: usize,
    ) -> (ParagraphProps, Vec<Inline>) {
        let style = ids.first().copied().expect("a style");
        let tab = ids.get(tab_style).copied().expect("a style");
        let props = ParagraphProps::default().with_tabs([TabStop::default()
            .with_position(Pt(40.0))
            .with_kind(TabKind::Start)
            .with_leader(leader)]);
        (props, vec![Inline::text("A", style), Inline::tab(tab)])
    }

    #[test]
    fn a_leader_falls_back_to_a_font_with_its_character_and_takes_the_tab_s_scale() {
        // The test font has no period; its copy "Officewright Dots" has one instead of
        // `A`, with `A`'s glyph 2.
        let dots = renamed(with_period(minimal()), "Officewright Dots");
        let leader_of = |tab_style: usize| {
            let plain = RunStyle::default();
            let narrow = RunStyle::default().with_scale(50);
            let (items, report) =
                painted_with(vec![minimal(), dots.clone()], &[plain, narrow], |ids, _| {
                    leader_paragraph(ids, TabLeader::Dot, tab_style)
                });
            let leader = glyph_runs(&items).get(1).map(|run| (*run).clone());
            (
                leader.unwrap_or_else(|| panic!("no leader: {items:?}")),
                report,
            )
        };
        let (full, report) = leader_of(0);
        let (half, _) = leader_of(1);
        for leader in [&full, &half] {
            assert_eq!(leader.clusters.first().map(|c| c.text.as_str()), Some("\t"));
            let glyphs: Vec<&DrawnGlyph> = leader.clusters.iter().flat_map(|c| &c.glyphs).collect();
            assert!(glyphs.len() > 2, "{leader:?}");
            assert!(glyphs.iter().all(|glyph| glyph.glyph == GlyphId::new(2)));
        }
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.kind() == ReportKind::FontSubstituted
                    && entry.message().contains("Officewright Dots")),
            "{report:?}"
        );
        // At half width the glyphs are drawn narrower and twice as close together.
        let step = |leader: &GlyphItem| {
            let glyphs: Vec<f64> = leader
                .clusters
                .iter()
                .flat_map(|c| &c.glyphs)
                .map(|glyph| glyph.x.0)
                .collect();
            glyphs.get(1).copied().unwrap_or_default() - glyphs.first().copied().unwrap_or_default()
        };
        assert_eq!((full.h_scale, half.h_scale), (1.0, 0.5));
        assert!(step(&full) > 0.0);
        assert!((step(&half) - step(&full) / 2.0).abs() < 1e-9);
        // The glyphs stand on the page's grid of their pitch, 3 pt here, of which the stop,
        // 40 pt from the column's start at 50, is a multiple: so the last ends at it.
        let last = half
            .clusters
            .iter()
            .flat_map(|c| &c.glyphs)
            .last()
            .expect("a glyph");
        assert!((half.origin.x.0 + last.x.0 + step(&half) - 90.0).abs() < 1e-9);
    }

    #[test]
    fn a_superscript_tab_draws_its_leader_at_the_superscript_s_size_and_height() {
        let dots = renamed(with_period(minimal()), "Officewright Dots");
        let plain = RunStyle::default();
        let raised = RunStyle::default().with_vertical(VerticalPosition::Superscript);
        let (items, _) = painted_with(vec![minimal(), dots], &[plain, raised], |ids, _| {
            leader_paragraph(ids, TabLeader::Dot, 1)
        });
        let runs = glyph_runs(&items);
        let (Some(text), Some(leader)) = (runs.first(), runs.get(1)) else {
            panic!("{items:?}");
        };
        // At 65 % of 10 pt, a third of 10 pt above the baseline.
        assert_eq!(leader.size, Pt(6.5));
        assert!((text.origin.y.0 - leader.origin.y.0 - 10.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_tab_has_its_style_s_highlight_and_its_leader_the_colour_on_it() {
        // On black paragraph shading, a leader in the automatic colour is white, as
        // text is; a highlighted tab has its highlight behind it.
        let dots = renamed(with_period(minimal()), "Officewright Dots");
        let plain = RunStyle::default();
        let highlighted = RunStyle::default().with_highlight(Some(Color::rgb(255, 255, 0)));
        let shaded = |ids: &[crate::ir::RunStyleId], tab: usize| {
            let (props, inlines) = leader_paragraph(ids, TabLeader::Dot, tab);
            let props = props.with_shading(Some(
                Shading::default().with_fill(ColorValue::Rgb(Color::BLACK)),
            ));
            (props, inlines)
        };
        let fonts = || vec![minimal(), dots.clone()];
        let (items, _) = painted_with(fonts(), &[plain.clone(), highlighted.clone()], |ids, _| {
            shaded(ids, 0)
        });
        let runs = glyph_runs(&items);
        assert!(runs.len() == 2, "{items:?}");
        assert!(runs.iter().all(|run| run.color == Color::WHITE), "{runs:?}");
        // Highlighted: a yellow box from the end of `A` to the stop, and black dots on it.
        let (items, _) = painted_with(fonts(), &[plain, highlighted], |ids, _| shaded(ids, 1));
        let yellow: Vec<_> = rects(&items)
            .into_iter()
            .filter(|rect| rect.3 == Color::rgb(255, 255, 0))
            .collect();
        let [(at, width, _, _)] = yellow.as_slice() else {
            panic!("{items:?}");
        };
        assert!((at.x.0 + width.0 - 90.0).abs() < 1e-9, "{at:?} {width:?}");
        let leader = glyph_runs(&items).get(1).copied().expect("a leader");
        assert_eq!(leader.color, Color::BLACK);
    }

    #[test]
    fn a_highlight_covers_the_room_its_run_takes_in_its_line() {
        // E-highlight: a superscript keeps its full size and the baseline, so its box is
        // the plain run's; text raised by its position takes the room above it as well,
        // and keeps the room below.
        let marked = RunStyle::default().with_highlight(Some(Color::rgb(255, 255, 0)));
        let raised = marked
            .clone()
            .with_vertical(VerticalPosition::Superscript)
            .with_position(Pt(2.0));
        let (items, _) = painted(&[marked, raised], |ids, _| {
            let (plain, up) = (
                ids.first().copied().expect("a style"),
                ids.get(1).copied().expect("a style"),
            );
            (
                ParagraphProps::default(),
                vec![Inline::text("A", plain), Inline::text("A", up)],
            )
        });
        let runs = glyph_runs(&items);
        let boxes = rects(&items);
        assert_eq!((runs.len(), boxes.len()), (2, 2), "{items:?}");
        // The test font reaches 9 pt above the baseline and 2.5 below at 10 pt. The
        // raised run puts the line's top 2 pt higher, at 20, and the baseline at 31: its
        // box is the whole line, from 20 to 33.5, and the plain run's starts 2 pt lower.
        let ((plain_at, _, plain_height, _), (raised_at, _, raised_height, _)) =
            (boxes[0], boxes[1]);
        assert!((raised_at.y.0 - 20.0).abs() < 1e-9, "{boxes:?}");
        assert!((raised_height.0 - 13.5).abs() < 1e-9, "{boxes:?}");
        assert!((plain_at.y.0 - 22.0).abs() < 1e-9, "{boxes:?}");
        assert!((plain_height.0 - 11.5).abs() < 1e-9, "{boxes:?}");
        let (Some(low), Some(high)) = (runs.first(), runs.get(1)) else {
            panic!("runs");
        };
        assert!((low.origin.y.0 - 31.0).abs() < 1e-9, "{runs:?}");
        assert!((low.origin.y.0 - high.origin.y.0 - (2.0 + 10.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn a_superscript_s_box_stays_on_the_baseline_in_a_taller_line() {
        // A superscript keeps its full size and the baseline in its line, and so does its
        // box: beside a 14 pt run the line's top is 12.6 pt above the baseline (at 32.6),
        // and the 10 pt superscript's box reaches 9 above it and 2.5 below, not raised
        // with its text.
        let superscript = RunStyle::default()
            .with_highlight(Some(Color::rgb(255, 255, 0)))
            .with_vertical(VerticalPosition::Superscript);
        let (items, _) = painted(&[RunStyle::default(), superscript], |ids, document| {
            let large = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(14.0)),
            );
            let up = ids.get(1).copied().expect("a style");
            (
                ParagraphProps::default(),
                vec![Inline::text("A", large), Inline::text("A", up)],
            )
        });
        let boxes = rects(&items);
        assert_eq!(boxes.len(), 1, "{items:?}");
        let (at, _, height, _) = boxes[0];
        assert!((at.y.0 - 23.6).abs() < 1e-9, "{boxes:?}");
        assert!((height.0 - 11.5).abs() < 1e-9, "{boxes:?}");
    }

    #[test]
    fn a_highlight_is_as_high_as_its_run_in_a_taller_line() {
        // A 6 pt highlighted run in a line its 10 pt paragraph mark makes taller, as the
        // layout counts marks (#359): the box is the run's own. Word shades the 12 pt
        // title of contract-usds-statement-of-objectives 13.7 pt high, its own height,
        // which this keeps where the layout's line is taller than Word's; a hypothesis for
        // runs of different sizes until #346.
        let (items, _) = painted(&[RunStyle::default()], |_, document| {
            let small = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(6.0))
                    .with_highlight(Some(Color::rgb(255, 255, 0))),
            );
            (ParagraphProps::default(), vec![Inline::text("A", small)])
        });
        let boxes = rects(&items);
        assert_eq!(boxes.len(), 1, "{items:?}");
        // The 10 pt mark sets the line, from 20 to 31.5, with its baseline at 29; the 6 pt
        // run reaches 5.4 pt above it and 1.5 below.
        let (at, _, height, _) = boxes[0];
        assert!((at.y.0 - 23.6).abs() < 1e-9, "{boxes:?}");
        assert!((height.0 - 6.9).abs() < 1e-9, "{boxes:?}");
    }

    #[test]
    fn a_highlight_takes_the_leading_its_font_puts_above_its_text() {
        // The test font with a `hhea` line gap of 300 has a line taller than its Windows
        // one: external leading of 0.15 em, which a line puts above its text, and so does
        // a run's box. A 6 pt highlighted run in the line of a 10 pt paragraph mark: with
        // leading the line's baseline is at 20 + 1.5 + 9 = 30.5 and the box reaches
        // 0.9 + 5.4 above it and 1.5 below; without (`w:noLeading`) the baseline is at
        // 29 and the box reaches 5.4 above it.
        let tall = patched(minimal(), b"hhea", 8, &300i16.to_be_bytes());
        for (flags, top, height) in [
            (vec![], 24.2, 7.8),
            (vec![CompatFlag::NoLeading], 23.6, 6.9),
        ] {
            let settings = crate::ir::DocumentSettings::default().with_compat_flags(flags);
            let (items, _) = painted_in(
                settings,
                vec![tall.clone()],
                &[RunStyle::default()],
                |_, document| {
                    let small = document.add_run_style(
                        RunStyle::default()
                            .with_fonts(RunFonts::new("Officewright Test"))
                            .with_size(Pt(6.0))
                            .with_highlight(Some(Color::rgb(255, 255, 0))),
                    );
                    (ParagraphProps::default(), vec![Inline::text("A", small)])
                },
            );
            let boxes = rects(&items);
            assert_eq!(boxes.len(), 1, "{items:?}");
            let (at, _, box_height, _) = boxes[0];
            assert!((at.y.0 - top).abs() < 1e-9, "{top}: {boxes:?}");
            assert!((box_height.0 - height).abs() < 1e-9, "{height}: {boxes:?}");
        }
    }

    #[test]
    fn highlight_and_run_shading_are_cut_to_an_exact_line() {
        // E-highlight case 05: under exact 9 pt line spacing the highlight is cut to the
        // 9 pt line; run shading without a highlight is cut the same way.
        let marked = RunStyle::default().with_highlight(Some(Color::rgb(255, 255, 0)));
        let shaded = RunStyle::default().with_shading(Some(
            Shading::default().with_fill(ColorValue::Rgb(Color::rgb(0, 0, 255))),
        ));
        for style in [marked, shaded] {
            let (items, _) = painted(&[style], |ids, _| {
                let id = ids.first().copied().expect("a style");
                (
                    ParagraphProps::default().with_spacing(
                        crate::ir::Spacing::default()
                            .with_line(crate::ir::LineSpacing::Exact(Pt(9.0))),
                    ),
                    vec![Inline::text("A", id)],
                )
            });
            let boxes = rects(&items);
            assert_eq!(boxes.len(), 1, "{items:?}");
            let (at, _, height, _) = boxes[0];
            assert!((at.y.0 - 20.0).abs() < 1e-9, "{boxes:?}");
            assert!((height.0 - 9.0).abs() < 1e-9, "{boxes:?}");
        }
    }

    /// The lines of `items`.
    fn line_items(items: &[DisplayItem]) -> Vec<&LineItem> {
        items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Line(line) => Some(line),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn pictures_take_their_effects_subscripts_drop_and_text_keeps_its_scale() {
        let plain = RunStyle::default();
        let lowered = RunStyle::default().with_vertical(VerticalPosition::Subscript);
        let narrow = RunStyle::default().with_scale(50);
        let (items, _) = painted(&[plain, lowered, narrow], |ids, document| {
            let image = document.add_image(ImageData::new(ImageFormat::Png, Vec::new()));
            let object = InlineObject::new(
                ObjectContent::Picture(crate::ir::Picture::new(image)),
                Size::new(Pt(8.0), Pt(5.0)),
            )
            .with_effect_extent(
                crate::ir::Insets::default()
                    .with_left(Pt(3.0))
                    .with_top(Pt(2.0))
                    .with_bottom(Pt(1.0)),
            );
            let inlines = vec![
                Inline::text("A", ids[0]),
                Inline::text("A", ids[1]),
                Inline::text("A", ids[2]),
                Inline::Object(object),
            ];
            (ParagraphProps::default(), inlines)
        });
        let runs = glyph_runs(&items);
        let [plain, lowered, narrow] = runs.as_slice() else {
            panic!("{items:?}");
        };
        // A subscript drops a third of its 10 pt.
        assert!((lowered.origin.y.0 - plain.origin.y.0 - 10.0 / 3.0).abs() < 1e-9);
        assert_eq!((plain.h_scale, narrow.h_scale), (1.0, 0.5));
        // The picture's frame, 8 by 5, inside the room its effects take: 3 to its left,
        // 2 above and 1 below, the room's bottom on the baseline.
        let picture = items
            .iter()
            .find_map(|item| match item {
                DisplayItem::Image(image) => Some(*image),
                _ => None,
            })
            .expect("a picture");
        // The room starts after the narrow `A`: 600 units of 1000 at 10 pt, at half width.
        let room_left = narrow.origin.x.0 + 3.0;
        assert!(
            (picture.at.x.0 - (room_left + 3.0)).abs() < 1e-9,
            "{picture:?} {narrow:?}"
        );
        assert!(
            (picture.at.y.0 - (plain.origin.y.0 - 8.0 + 2.0)).abs() < 1e-9,
            "{picture:?}"
        );
    }

    #[test]
    fn strikes_and_underlines_take_their_kinds_and_a_highlight_hides_shading() {
        let double = RunStyle::default().with_double_strike(true);
        let wavy = RunStyle::default()
            .with_underline(Some(Underline::default().with_kind(UnderlineKind::Wave)));
        let both = RunStyle::default()
            .with_highlight(Some(Color::rgb(255, 255, 0)))
            .with_shading(Some(
                Shading::default().with_fill(ColorValue::Rgb(Color::BLACK)),
            ));
        let (items, _) = painted(&[double, wavy, both], |ids, _| {
            let inlines = vec![
                Inline::text("A", ids[0]),
                Inline::text("A", ids[1]),
                Inline::text("A", ids[2]),
            ];
            (ParagraphProps::default(), inlines)
        });
        let lines = line_items(&items);
        let styles: Vec<LineStyle> = lines.iter().map(|line| line.style).collect();
        assert_eq!(
            styles,
            [LineStyle::Solid, LineStyle::Solid, LineStyle::Wave],
            "{items:?}"
        );
        // The double strike's two lines, one above the other.
        assert!(lines[0].from.y.0 < lines[1].from.y.0);
        // The highlight, not the black shading, is behind the third, and its text black.
        let boxes = rects(&items);
        assert_eq!(
            boxes.iter().map(|rect| rect.3).collect::<Vec<_>>(),
            [Color::rgb(255, 255, 0)]
        );
        assert_eq!(glyph_runs(&items)[2].color, Color::BLACK);
    }

    #[test]
    fn a_subscript_s_box_is_its_line_like_the_text_beside_it() {
        // E-highlight case 03: a subscript keeps its full size and the baseline in its line,
        // and so does its box, from the line's top at 20 to its bottom at 31.5 (ascent 9 and
        // descent 2.5 of the test font at 10 pt), not lowered with its text.
        let lowered = RunStyle::default()
            .with_vertical(VerticalPosition::Subscript)
            .with_highlight(Some(Color::rgb(255, 255, 0)));
        let (items, _) = painted(&[RunStyle::default(), lowered], |ids, _| {
            let inlines = vec![Inline::text("A", ids[0]), Inline::text("A", ids[1])];
            (ParagraphProps::default(), inlines)
        });
        let boxes = rects(&items);
        let [(at, _, height, _)] = boxes.as_slice() else {
            panic!("{items:?}");
        };
        assert!((at.y.0 - 20.0).abs() < 1e-9, "{boxes:?}");
        assert!((height.0 - 11.5).abs() < 1e-9, "{boxes:?}");
    }

    #[test]
    fn text_lowered_by_its_position_takes_the_room_below_in_its_box() {
        // Lowered 3 pt beside plain text: the line reaches 2.5 + 3 below the baseline at 29,
        // and the lowered run's box from 9 above it to 5.5 below, the whole line. A
        // hypothesis until a Word reference (#346).
        let lowered = RunStyle::default()
            .with_position(Pt(-3.0))
            .with_highlight(Some(Color::rgb(255, 255, 0)));
        let (items, _) = painted(&[RunStyle::default(), lowered], |ids, _| {
            let inlines = vec![Inline::text("A", ids[0]), Inline::text("A", ids[1])];
            (ParagraphProps::default(), inlines)
        });
        let boxes = rects(&items);
        let [(at, _, height, _)] = boxes.as_slice() else {
            panic!("{items:?}");
        };
        assert!((at.y.0 - 20.0).abs() < 1e-9, "{boxes:?}");
        assert!((height.0 - 14.5).abs() < 1e-9, "{boxes:?}");
    }

    #[test]
    fn a_highlighted_tab_s_box_is_placed_like_its_text_s() {
        // A highlighted tab in a superscript keeps the baseline, as text does: from the
        // line's top at 20 to 31.5. One raised 2 pt by its position, in a line that text
        // raised 4 pt makes taller (top at 20, baseline at 33), reaches 9 + 2 above the
        // baseline and 2.5 below: from 22 to 35.5.
        let superscript = RunStyle::default()
            .with_vertical(VerticalPosition::Superscript)
            .with_highlight(Some(Color::rgb(255, 255, 0)));
        let raised_tab = RunStyle::default()
            .with_position(Pt(2.0))
            .with_highlight(Some(Color::rgb(255, 255, 0)));
        let raised_text = RunStyle::default().with_position(Pt(4.0));
        let styles = [RunStyle::default(), superscript, raised_tab, raised_text];
        let boxes_of = |tab: usize, before: usize| {
            let (items, _) = painted(&styles, |ids, _| {
                let inlines = vec![
                    Inline::text("A", ids[before]),
                    Inline::tab(ids[tab]),
                    Inline::text("A", ids[0]),
                ];
                (ParagraphProps::default(), inlines)
            });
            rects(&items)
                .iter()
                .map(|(at, _, height, _)| (at.y.0, height.0))
                .collect::<Vec<_>>()
        };
        let close = |boxes: &[(f64, f64)], (top, height): (f64, f64)| matches!(boxes, [(t, h)] if (t - top).abs() < 1e-9 && (h - height).abs() < 1e-9);
        let superscript_tab = boxes_of(1, 0);
        assert!(close(&superscript_tab, (20.0, 11.5)), "{superscript_tab:?}");
        let positioned_tab = boxes_of(2, 3);
        assert!(close(&positioned_tab, (22.0, 13.5)), "{positioned_tab:?}");
    }

    #[test]
    fn a_smaller_highlighted_tab_without_a_font_reaches_as_far_as_its_own_text() {
        // With no font, a 6 pt tab under a 10 pt paragraph mark: the line takes the mark's
        // 8 pt above its baseline (at 28) and 2 below, and the tab's box its own 0.8 and
        // 0.2 of 6 pt, from 23.2 to 29.2.
        let (items, _) = painted_with(Vec::new(), &[RunStyle::default()], |_, document| {
            let small = document.add_run_style(
                RunStyle::default()
                    .with_size(Pt(6.0))
                    .with_highlight(Some(Color::rgb(255, 255, 0))),
            );
            (ParagraphProps::default(), vec![Inline::tab(small)])
        });
        let boxes = rects(&items);
        let [(at, _, height, _)] = boxes.as_slice() else {
            panic!("{items:?}");
        };
        assert!((at.y.0 - 23.2).abs() < 1e-9, "{boxes:?}");
        assert!((height.0 - 6.0).abs() < 1e-9, "{boxes:?}");
    }

    #[test]
    fn a_highlighted_tab_without_a_font_reaches_as_far_as_its_line() {
        // With no font at all there is no text, but a tab still takes its room: its line
        // takes 0.8 of the size above the baseline and 0.2 below, and so does its box,
        // from 20 to 30 at 10 pt.
        let marked = RunStyle::default().with_highlight(Some(Color::rgb(255, 255, 0)));
        let (items, _) = painted_with(Vec::new(), &[marked], |ids, _| {
            (ParagraphProps::default(), vec![Inline::tab(ids[0])])
        });
        let boxes = rects(&items);
        let [(at, _, height, _)] = boxes.as_slice() else {
            panic!("{items:?}");
        };
        assert!((at.y.0 - 20.0).abs() < 1e-9, "{boxes:?}");
        assert!((height.0 - 10.0).abs() < 1e-9, "{boxes:?}");
    }

    #[test]
    fn at_most_256_bar_tabs_are_drawn_and_more_are_reported() {
        let (items, report) = painted_with(vec![minimal()], &[RunStyle::default()], |ids, _| {
            let bars = (0..300).map(|stop| {
                TabStop::default()
                    .with_position(Pt(f64::from(stop) / 4.0))
                    .with_kind(TabKind::Bar)
            });
            (
                ParagraphProps::default().with_tabs(bars),
                vec![Inline::text("A", ids[0])],
            )
        });
        assert_eq!(line_items(&items).len(), 256);
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.message().contains("bar tabs")),
            "{report:?}"
        );
    }

    #[test]
    fn bars_are_drawn_once_where_the_column_puts_them_in_the_colour_on_the_paragraph() {
        let plain = RunStyle::default();
        let (items, _) = painted(&[plain], |ids, _| {
            let bar = TabStop::default()
                .with_position(Pt(30.0))
                .with_kind(TabKind::Bar);
            let props = ParagraphProps::default()
                .with_tabs([bar, bar, bar])
                .with_shading(Some(
                    Shading::default().with_fill(ColorValue::Rgb(Color::BLACK)),
                ));
            (props, vec![Inline::text("A", ids[0])])
        });
        let bars = line_items(&items);
        let [bar] = bars.as_slice() else {
            panic!("{items:?}");
        };
        // 30 pt from the column's start edge at 50, white on the black shading.
        assert_eq!((bar.from.x, bar.to.x), (Pt(80.0), Pt(80.0)));
        assert_eq!(bar.color, Color::WHITE);
        assert_eq!(bar.from.y, Pt(20.0));
        assert_eq!(bar.width, Pt(0.14));
    }

    #[test]
    fn a_symbol_that_is_no_character_shifts_no_text() {
        // U+D800 is no character: it is drawn as the font's code 0x00 and stands for no
        // text, and the `A` after it keeps its own.
        let plain = RunStyle::default();
        let (items, _) = painted(&[plain], |ids, _| {
            let inlines = vec![
                Inline::symbol("Officewright Test", 0xD800, ids[0]),
                Inline::text("AA", ids[0]),
            ];
            (ParagraphProps::default(), inlines)
        });
        let clusters: Vec<(Vec<GlyphId>, String)> = glyph_runs(&items)
            .iter()
            .flat_map(|run| &run.clusters)
            .map(|cluster| {
                let glyphs = cluster.glyphs.iter().map(|glyph| glyph.glyph).collect();
                (glyphs, cluster.text.clone())
            })
            .collect();
        let texts: String = clusters.iter().map(|(_, text)| text.as_str()).collect();
        assert_eq!(texts, "AA", "{clusters:?}");
        for (glyphs, text) in &clusters {
            if glyphs.contains(&GlyphId::new(2)) {
                assert_eq!(text, "A", "{clusters:?}");
            }
        }
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
            let picture = crate::ir::Picture::new(image)
                .with_rotation(90.0)
                .with_flip_horizontal(true);
            let object =
                InlineObject::new(ObjectContent::Picture(picture), Size::new(Pt(8.0), Pt(5.0)));
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
        // Turned and mirrored as the picture is, for the painter.
        assert_eq!(picture.rotation, 90.0);
        assert!(picture.flip_horizontal && !picture.flip_vertical);
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
        assert_eq!((luma(Color::WHITE), luma(Color::BLACK)), (255_000, 0));
    }

    #[test]
    fn automatic_text_turns_white_where_word_s_reference_turns_it() {
        // E-auto-color.docx, and 404040 from E-bar-tab.docx: the fills on which Word
        // draws automatic text white.
        let white = [0x333333, 0x191919, 0x0000FF, 0x002060];
        let black = [
            0xE6E6E6, 0xCCCCCC, 0xB2B2B2, 0x999999, 0x808080, 0x666666, 0x4D4D4D, 0x404040,
            0xFF0000, 0x00B050, 0xFFFF00,
        ];
        let rgb = |hex: u32| Color::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        for (fills, expected) in [(&white[..], Color::WHITE), (&black[..], Color::BLACK)] {
            for &fill in fills {
                assert_eq!(
                    text_color(ColorValue::Auto, Some(rgb(fill))),
                    expected,
                    "{fill:06X}"
                );
            }
        }
    }
}
