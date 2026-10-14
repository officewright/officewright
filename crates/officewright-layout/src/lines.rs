//! Breaking a paragraph into lines, as Word does: first fit, line by line, at the last
//! place where a line may end; then each line's height and its alignment.

// reason: the layout itself uses this once pagination (plan PR 8) lays out pages; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::ops::Range;
use std::sync::Arc;

use officewright_core::Pt;

use crate::breaks::{
    BreakMap, Opportunity, PLACEHOLDER_BREAK, PLACEHOLDER_MARK, PLACEHOLDER_OBJECT, PLACEHOLDER_TAB,
};
use crate::fonts::Fonts;
use crate::ir::{
    Alignment, BreakKind, CompatFlag, Document, Inline, LineSpacing, Paragraph, RunStyleId,
    TabKind, TabLeader,
};
use crate::itemize::{InlinePath, Item, Itemized, request};
use crate::metrics::{LineMetrics, TypoRule, line_metrics};
use crate::report::{Location, ReportKind};
use crate::shape::{ShapedRun, Shaper, ShapingFace};
use crate::tabs::{Stop, TabStops};

/// Where the baseline of a line of exact height is, as a share of the height from its
/// top: Word's renders of `E-line-exact.docx` (Calibri 11 in lines of 8, 12, 20 and 30
/// pt, and a 28 pt run in a 20 pt line) put it there whatever the font and its size,
/// within the 0.12 pt of the printer's grid.
const EXACT_BASELINE: f64 = 0.8;

/// How far past the end of a line a piece may reach and still fit: rounding in sums of
/// advances.
const EPSILON: f64 = 1e-9;

/// How many clusters after a cut are shaped again at most when HarfRust marks none of
/// them safe to break before, which bounds the work per line in a run kerned
/// throughout. Where the shaped pieces meet there, the shaping of the one before ends as
/// at the end of a text: a kerning pair may be lost, and in a joining script the form of
/// a letter may change. It takes a word of more than this many clusters without a place
/// safe to break.
const RESHAPE_WINDOW: usize = 64;

/// Where first fit ends a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fit {
    /// The unit after the line's last.
    end: usize,
    end_kind: LineEnd,
    /// Whether the line ends at a soft hyphen whose hyphen fits, which it draws.
    hyphen: bool,
    /// Whether the line holds a piece too wide for any line.
    overflow: bool,
    /// The first tab that starts past the end of the line, with no place before it
    /// where the line may end: it and the tabs after it hang like the spaces there.
    hanging_tab: Option<usize>,
}

/// What shaping again at a cut replaced: the units at `first`, now `len` others.
#[derive(Clone, Debug)]
struct Splice {
    first: usize,
    len: usize,
    replaced: Vec<Unit>,
}

/// A line of a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    /// What is on the line, in order, each where it is drawn.
    pub(crate) pieces: Vec<Piece>,
    /// The bytes of the break text the line holds.
    pub(crate) range: Range<usize>,
    /// Where the line may start and end, from the start edge of the column: the
    /// paragraph's indents.
    pub(crate) start: Pt,
    pub(crate) end_edge: Pt,
    /// The width of what is on the line, without the spaces that hang at its end.
    pub(crate) width: Pt,
    /// How far the line's text reaches above and below its baseline, and the leading
    /// its fonts add.
    pub(crate) ascent: Pt,
    pub(crate) descent: Pt,
    /// The height, after the paragraph's line spacing.
    pub(crate) height: Pt,
    /// The baseline, from the top of the line.
    pub(crate) baseline: Pt,
    pub(crate) end: LineEnd,
    /// Whether the line holds a piece too wide for any line, which reaches past its
    /// end and is reported.
    pub(crate) overflow: bool,
}

/// Why a line ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineEnd {
    /// The next word does not fit.
    Wrap,
    /// A line break.
    Break,
    /// A page break.
    PageBreak,
    /// A column break.
    ColumnBreak,
    /// The paragraph ends.
    Paragraph,
}

/// A piece of a line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Piece {
    /// Where it starts, from the start edge of the column.
    pub(crate) x: Pt,
    pub(crate) width: Pt,
    pub(crate) kind: PieceKind,
    /// Whether it comes after the last content of the line and hangs past its edge, as
    /// the spaces at its end and a tab that starts past it do: such pieces count for no
    /// width, justification leaves them alone, and they are not underlined.
    pub(crate) hanging: bool,
}

/// What a piece of a line is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PieceKind {
    /// Glyphs `glyphs` of `run`, the shaped text of item `item`, which stand for bytes
    /// `text` of the drawn text. Justification widens each space by `space_extra` and
    /// adds `cluster_extra` after each cluster, the last one too unless `last_extra`
    /// is false.
    Text {
        item: usize,
        run: Arc<ShapedRun>,
        glyphs: Range<usize>,
        text: Range<usize>,
        scale: Scale,
        space_extra: Pt,
        cluster_extra: Pt,
        last_extra: bool,
    },
    /// The hyphen drawn where a line ends at a soft hyphen of item `item`.
    Hyphen {
        item: usize,
        run: Arc<ShapedRun>,
        scale: Scale,
    },
    /// A tab of item `item`, with what fills it.
    Tab { item: usize, leader: TabLeader },
    /// A break of item `item`.
    Break { item: usize },
    /// An inline object `height` high, or an anchored object or a note mark (no
    /// height), of item `item`.
    Object { item: usize, height: Option<Pt> },
}

/// A piece of a paragraph that a line does not split: a cluster of glyphs or a piece
/// that is not text.
#[derive(Clone, Debug)]
struct Unit {
    /// Its bytes of the break text.
    range: Range<usize>,
    kind: UnitKind,
    /// Its width; a tab's comes from where it is.
    width: Pt,
}

#[derive(Clone, Debug)]
enum UnitKind {
    /// Glyphs `glyphs` of `run`, standing for bytes `text` of the drawn text.
    Glyphs {
        item: usize,
        run: Arc<ShapedRun>,
        glyphs: Range<usize>,
        text: Range<usize>,
    },
    Tab {
        item: usize,
    },
    Break {
        item: usize,
        kind: BreakKind,
    },
    /// An inline object, `width` wide and `height` high.
    Object {
        item: usize,
        height: Pt,
    },
    /// An anchored object or a note mark: no width.
    Zero {
        item: usize,
    },
}

impl UnitKind {
    fn item(&self) -> usize {
        match self {
            Self::Glyphs { item, .. }
            | Self::Tab { item }
            | Self::Break { item, .. }
            | Self::Object { item, .. }
            | Self::Zero { item } => *item,
        }
    }
}

/// How a text item's glyphs are measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Scale {
    /// Points per design unit, the character scale (`w:w`) included.
    pub(crate) per_unit: f64,
    /// The spacing after each character (`w:spacing`).
    pub(crate) letter_spacing: Pt,
}

/// A glyph of a line, where it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlacedGlyph {
    pub(crate) glyph: officewright_font::GlyphId,
    /// From the start edge of the column.
    pub(crate) x: Pt,
    /// Up from the baseline.
    pub(crate) y: Pt,
    /// Its advance, the spacing after it included.
    pub(crate) advance: Pt,
    /// The byte offset in the drawn text of the first character it stands for.
    pub(crate) cluster: usize,
}

impl Piece {
    /// The glyphs of the piece, each where it is drawn: their advances, the spacing
    /// after each character, and what justification adds. `itemized` is the
    /// paragraph's.
    pub(crate) fn glyphs(&self, itemized: &Itemized) -> Vec<PlacedGlyph> {
        let (run, glyphs, text, scale, space_extra, cluster_extra, last_extra) = match &self.kind {
            PieceKind::Text {
                run,
                glyphs,
                text,
                scale,
                space_extra,
                cluster_extra,
                last_extra,
                ..
            } => (
                run,
                glyphs.clone(),
                Some(text.clone()),
                *scale,
                *space_extra,
                *cluster_extra,
                *last_extra,
            ),
            PieceKind::Hyphen { run, scale, .. } => (
                run,
                0..run.glyphs().len(),
                None,
                *scale,
                Pt(0.0),
                Pt(0.0),
                false,
            ),
            _ => return Vec::new(),
        };
        let shaped = run.glyphs().get(glyphs.clone()).unwrap_or(&[]);
        // The run's text starts before the piece's: clusters are offsets in the run.
        let run_start = text.as_ref().map(|text| {
            text.start
                .saturating_sub(shaped.first().map_or(0, |glyph| glyph.cluster))
        });
        let mut placed = Vec::with_capacity(shaped.len());
        let mut x = self.x.0;
        // The design units of the cluster so far, for the spacing after it.
        let mut cluster_units = 0i64;
        for (index, glyph) in shaped.iter().enumerate() {
            let mut advance = f64::from(glyph.x_advance) * scale.per_unit;
            cluster_units += i64::from(glyph.x_advance);
            let next = shaped.get(index + 1);
            if next.is_none_or(|next| next.cluster != glyph.cluster) {
                advance += cluster_spacing(cluster_units, scale);
                cluster_units = 0;
                if let (Some(start), Some(text)) = (run_start, &text) {
                    let end = next.map_or(text.end, |next| start + next.cluster);
                    let cluster = itemized.text.get(start + glyph.cluster..end).unwrap_or("");
                    let spaces = cluster
                        .chars()
                        .filter(|&ch| ch == ' ' || ch == '\u{00A0}')
                        .count();
                    advance += spaces as f64 * space_extra.0;
                }
                if next.is_some() || last_extra {
                    advance += cluster_extra.0;
                }
            }
            placed.push(PlacedGlyph {
                glyph: glyph.glyph,
                x: Pt(x + f64::from(glyph.x_offset) * scale.per_unit),
                y: Pt(f64::from(glyph.y_offset) * scale.per_unit),
                advance: Pt(advance),
                cluster: run_start.map_or(0, |start| start + glyph.cluster),
            });
            x += advance;
        }
        placed
    }
}

/// The paragraph a line breaker breaks, and where.
#[derive(Clone, Copy)]
pub(crate) struct Input<'p> {
    pub(crate) document: &'p Document,
    pub(crate) paragraph: &'p Paragraph,
    /// The width of the column.
    pub(crate) width: Pt,
    /// The paragraph's place, for the report.
    pub(crate) location: &'p Location,
}

/// What the line breaker works with.
pub(crate) struct Breaker<'p, 'f, 'b> {
    document: &'p Document,
    paragraph: &'p Paragraph,
    itemized: &'p Itemized,
    fonts: &'f mut Fonts<'b>,
    shaper: &'f mut Shaper,
    location: &'p Location,
    /// The break text: the drawn text, with a character standing for each item that
    /// is not text.
    text: String,
    /// Where each item starts in the break text.
    starts: Vec<usize>,
    breaks: BreakMap,
    units: Vec<Unit>,
    scales: Vec<Option<Scale>>,
    tabs: TabStops,
    /// The width of the column.
    width: Pt,
    overflow_reported: bool,
    /// What the last call of `settle` replaced, if it shaped again.
    splice: Option<Splice>,
}

impl<'p, 'f, 'b> Breaker<'p, 'f, 'b> {
    /// A breaker for the paragraph of `input`, from its itemized text and the shaped
    /// run of each text item, as `shape_runs` gives them.
    pub(crate) fn new(
        input: Input<'p>,
        itemized: &'p Itemized,
        shaped: &[Option<Arc<ShapedRun>>],
        fonts: &'f mut Fonts<'b>,
        shaper: &'f mut Shaper,
    ) -> Self {
        let Input {
            document,
            paragraph,
            width,
            location,
        } = input;
        let (text, starts) = break_text(itemized);
        let props = paragraph.props();
        let breaks = BreakMap::new(&text, props.word_wrap());
        let tabs = TabStops::new(props, document.settings(), paragraph.list().is_some());
        let mut breaker = Self {
            document,
            paragraph,
            itemized,
            fonts,
            shaper,
            location,
            text,
            starts,
            breaks,
            units: Vec::new(),
            scales: Vec::new(),
            tabs,
            width,
            overflow_reported: false,
            splice: None,
        };
        breaker.scales = breaker.measure_items();
        breaker.units = breaker.make_units(shaped);
        breaker
    }

    /// How each text item's glyphs are measured.
    fn measure_items(&mut self) -> Vec<Option<Scale>> {
        let itemized = self.itemized;
        let mut scales = Vec::with_capacity(itemized.items.len());
        for item in &itemized.items {
            let Item::Text(run) = item else {
                scales.push(None);
                continue;
            };
            let face = run.face.and_then(|id| self.fonts.face(id));
            let style = self.document.run_style(run.style);
            let h_scale = style.map_or(1.0, |style| f64::from(style.scale()) / 100.0);
            let letter_spacing = style.map_or(Pt(0.0), |style| style.letter_spacing());
            scales.push(face.map(|face| Scale {
                per_unit: run.size.0 / f64::from(face.face().units_per_em().max(1)) * h_scale,
                letter_spacing,
            }));
        }
        scales
    }

    /// The units of every item, in order.
    fn make_units(&self, shaped: &[Option<Arc<ShapedRun>>]) -> Vec<Unit> {
        let mut units = Vec::new();
        let mut shaped = shaped.iter();
        for (index, item) in self.itemized.items.iter().enumerate() {
            let start = self.starts.get(index).copied().unwrap_or(0);
            match item {
                Item::Text(run) => match shaped.next() {
                    Some(Some(result)) => {
                        units.extend(self.clusters(index, result, run.range.clone(), start));
                    }
                    // Without a face nothing is drawn, but the text keeps its place.
                    _ => units.push(Unit {
                        range: start..start + run.range.len(),
                        kind: UnitKind::Zero { item: index },
                        width: Pt(0.0),
                    }),
                },
                Item::Tab(_) => units.push(Unit {
                    range: start..start + PLACEHOLDER_TAB.len_utf8(),
                    kind: UnitKind::Tab { item: index },
                    width: Pt(0.0),
                }),
                Item::Break(kind, _) => units.push(Unit {
                    range: start..start + PLACEHOLDER_BREAK.len_utf8(),
                    kind: UnitKind::Break {
                        item: index,
                        kind: *kind,
                    },
                    width: Pt(0.0),
                }),
                Item::Object(path) => {
                    let (width, height) = self.object_size(path);
                    units.push(Unit {
                        range: start..start + PLACEHOLDER_OBJECT.len_utf8(),
                        kind: UnitKind::Object {
                            item: index,
                            height,
                        },
                        width,
                    });
                }
                Item::NoteReference(..) | Item::NoteMark(_) => units.push(Unit {
                    range: start..start + PLACEHOLDER_MARK.len_utf8(),
                    kind: UnitKind::Zero { item: index },
                    width: Pt(0.0),
                }),
                Item::Anchored(_) => units.push(Unit {
                    range: start..start,
                    kind: UnitKind::Zero { item: index },
                    width: Pt(0.0),
                }),
            }
        }
        units
    }

    /// The clusters of `run`, the shaped bytes `text` of the drawn text of item
    /// `item`, which starts at `start` of the break text.
    fn clusters(
        &self,
        item: usize,
        run: &Arc<ShapedRun>,
        text: Range<usize>,
        start: usize,
    ) -> Vec<Unit> {
        let scale = self.scales.get(item).copied().flatten();
        let glyphs = run.glyphs();
        let mut units = Vec::new();
        let mut first = 0;
        while first < glyphs.len() {
            let cluster = glyphs.get(first).map_or(0, |glyph| glyph.cluster);
            let mut end = first + 1;
            while glyphs
                .get(end)
                .is_some_and(|glyph| glyph.cluster == cluster)
            {
                end += 1;
            }
            let next = glyphs.get(end).map_or(text.len(), |glyph| glyph.cluster);
            let advance: i64 = glyphs
                .get(first..end)
                .unwrap_or(&[])
                .iter()
                .map(|glyph| i64::from(glyph.x_advance))
                .sum();
            let width = scale.map_or(Pt(0.0), |scale| {
                Pt(advance as f64 * scale.per_unit + cluster_spacing(advance, scale))
            });
            units.push(Unit {
                range: start + cluster..start + next.max(cluster),
                kind: UnitKind::Glyphs {
                    item,
                    run: Arc::clone(run),
                    glyphs: first..end,
                    text: text.start + cluster..text.start + next.max(cluster),
                },
                width,
            });
            first = end;
        }
        units
    }

    /// The width and height an inline object takes: its size and the room its
    /// effects take.
    fn object_size(&self, path: &InlinePath) -> (Pt, Pt) {
        match inline_at(self.paragraph.inlines(), path) {
            Some(Inline::Object(object)) => {
                let (size, extent) = (object.size(), object.effect_extent());
                (
                    Pt((size.width().0 + extent.left().0 + extent.right().0).max(0.0)),
                    Pt((size.height().0 + extent.top().0 + extent.bottom().0).max(0.0)),
                )
            }
            _ => (Pt(0.0), Pt(0.0)),
        }
    }

    /// Breaks the paragraph into lines.
    pub(crate) fn lines(mut self) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut start = 0;
        loop {
            let first = lines.is_empty();
            let fit = self.fit(start, first);
            let (end, mut line) = self.refit(start, fit, first);
            let line_end = line.end;
            let last =
                end >= self.units.len() && !(line_end == LineEnd::Break && end == self.units.len());
            self.align(&mut line, last);
            self.set_height(&mut line, last);
            lines.push(line);
            if last {
                break;
            }
            start = end;
            if end >= self.units.len() {
                // A line break at the very end: an empty line follows, with the
                // paragraph mark.
                let fit = Fit {
                    end,
                    end_kind: LineEnd::Paragraph,
                    hyphen: false,
                    overflow: false,
                    hanging_tab: None,
                };
                let mut line = self.place(end, fit, false);
                self.set_height(&mut line, true);
                lines.push(line);
                break;
            }
        }
        lines
    }

    /// The line from unit `start` that first fit chose, placed: after shaping again the
    /// run a cut divides where its shaping depends on what follows. Shaping again can
    /// make the line wider (a kerned pair cut apart, which a soft hyphen does not stop);
    /// then first fit runs again over the units as they are now shaped, which ends the
    /// line at the place before, or without its hyphen. Returns the unit after the
    /// line's last, and the line.
    fn refit(&mut self, start: usize, fit: Fit, first: bool) -> (usize, Line) {
        let (_, right) = self.edges(first);
        let mut fit = fit;
        fit.end = self.settle(start, fit.end, fit.end_kind);
        let mut splice = self.splice.take();
        loop {
            // Every line takes at least one unit, so that breaking ends.
            if fit.end <= start && start < self.units.len() {
                fit.end = start + 1;
            }
            let mut line = self.place(start, fit, first);
            // Indents wider than the column leave no room, not less than none.
            let room = (right.0 - line.start.0).max(0.0);
            let fits = line.width.0 <= room + EPSILON;
            if fits || fit.end_kind != LineEnd::Wrap || line.overflow {
                return (fit.end, line);
            }
            let again = self.fit(start, first);
            // Each step goes to a line that ends earlier, or at the same place without
            // its hyphen (`rank`), so the loop ends: first fit over the units as they
            // are now shaped, if it gives such a line; else the line drops its hyphen;
            // else it gives up its last unit that takes room, with what follows it; a
            // single one stays, reported.
            let rank = |fit: &Fit| (fit.end, fit.hyphen);
            let last_wide = (start + 1..fit.end)
                .rev()
                .find(|&i| self.units.get(i).is_some_and(|unit| unit.width.0 > 0.0));
            fit = if rank(&again) < rank(&fit) {
                again
            } else if fit.hyphen {
                Fit {
                    hyphen: false,
                    ..fit
                }
            } else if let Some(end) = last_wide {
                Fit {
                    end,
                    hyphen: false,
                    hanging_tab: fit.hanging_tab.filter(|&tab| tab < end),
                    ..fit
                }
            } else {
                if let Some(kind) = self.units.get(start).map(|unit| unit.kind.clone()) {
                    self.report_overflow(&kind);
                }
                line.overflow = true;
                return (fit.end, line);
            };
            // The cut given up keeps nothing of its shaping: the text around it is as
            // first shaped again, for the cut that follows.
            if let Some(splice) = splice.take() {
                fit.end = self.undo(splice, fit.end);
            }
            fit.end = self.settle(start, fit.end, fit.end_kind);
            splice = self.splice.take();
        }
    }

    /// Puts back the units `splice` replaced, and returns the unit `end` of the units
    /// with the splice as a unit of those without it: the one that starts at the same
    /// place of the text, or the last before it.
    fn undo(&mut self, splice: Splice, end: usize) -> usize {
        let at = self
            .units
            .get(end)
            .map_or(self.text.len(), |unit| unit.range.start);
        let Splice {
            first,
            len,
            replaced,
        } = splice;
        let count = replaced.len();
        let removed = self.units.len().min(first.saturating_add(len));
        self.units.splice(first.min(removed)..removed, replaced);
        if end <= first {
            end
        } else if end >= first + len {
            end - len + count
        } else {
            (first..first + count)
                .rev()
                .find(|&index| {
                    self.units
                        .get(index)
                        .is_some_and(|unit| unit.range.start <= at)
                })
                .unwrap_or(first)
        }
    }

    /// The edges of a line, from the start edge of the column.
    fn edges(&self, first: bool) -> (Pt, Pt) {
        let indent = self.paragraph.props().indent();
        let start = indent.start().0 + if first { indent.first_line().0 } else { 0.0 };
        (Pt(start), Pt(self.width.0 - indent.end().0))
    }

    /// Where the line that starts at unit `start` ends, by first fit.
    fn fit(&mut self, start: usize, first: bool) -> Fit {
        let (left, right) = self.edges(first);
        let mut cursor = Cursor::new(left);
        // The unit after the last place the line may end, where it would fit, and
        // whether that place is a soft hyphen.
        let mut candidate: Option<(usize, bool)> = None;
        let mut overflow = false;
        let mut hanging_tab: Option<usize> = None;
        let wrap = |end: usize, hyphen: bool, overflow: bool, hanging_tab: Option<usize>| Fit {
            end,
            end_kind: LineEnd::Wrap,
            hyphen,
            overflow,
            hanging_tab: hanging_tab.filter(|&tab| tab < end),
        };
        let mut index = start;
        while let Some(unit) = self.units.get(index).cloned() {
            match &unit.kind {
                UnitKind::Break { kind, .. } => {
                    return Fit {
                        end: index + 1,
                        end_kind: line_end(*kind),
                        hyphen: false,
                        overflow,
                        hanging_tab,
                    };
                }
                UnitKind::Tab { .. } => {
                    // A tab after spaces that already hang past the end of the line
                    // starts the next line, if this one may end before it; else it stays,
                    // and hangs with the spaces.
                    if cursor.x.0 > right.0 + EPSILON {
                        if let Some((end, hyphen)) = candidate {
                            return wrap(end, hyphen, overflow, hanging_tab);
                        }
                        hanging_tab.get_or_insert(index);
                    }
                    cursor.tab(&self.tabs, right);
                }
                UnitKind::Glyphs { text, .. } if is_space(self.itemized, text) => {
                    cursor.space(unit.width);
                }
                kind => {
                    let what = self.decimal_char(kind);
                    let x = cursor.peek(unit.width, what);
                    // What takes no room fits wherever the line has got to.
                    let over = unit.width.0 > 0.0 && x.0 > right.0 + EPSILON;
                    if over {
                        // The line ends at its last place to end, if it has one (after
                        // spaces at its start, too), else before the unit that does not
                        // fit, between two clusters; a unit alone on its line that does
                        // not fit stays there, reported.
                        if let Some((end, hyphen)) = candidate {
                            return wrap(end, hyphen, overflow, hanging_tab);
                        }
                        if cursor.has_content {
                            return wrap(index, false, overflow, hanging_tab);
                        }
                        self.report_overflow(kind);
                        overflow = true;
                    }
                    cursor.add(unit.width, what);
                }
            }
            index += 1;
            let at = unit.range.end;
            match self.breaks.at(at) {
                Some(Opportunity::Allowed) if index < self.units.len() => {
                    let hyphen = self.ends_in_soft_hyphen(index - 1);
                    if !hyphen || cursor.x.0 + self.hyphen_width(index - 1).0 <= right.0 + EPSILON {
                        candidate = Some((index, hyphen));
                    }
                }
                Some(Opportunity::Mandatory) if index < self.units.len() => {
                    return Fit {
                        end: index,
                        end_kind: LineEnd::Break,
                        hyphen: false,
                        overflow,
                        hanging_tab,
                    };
                }
                _ => {}
            }
        }
        Fit {
            end: self.units.len(),
            end_kind: LineEnd::Paragraph,
            hyphen: false,
            overflow,
            hanging_tab,
        }
    }

    /// What a unit holds, for a decimal tab before it.
    fn decimal_char(&self, kind: &UnitKind) -> DecimalChar {
        let UnitKind::Glyphs { text, .. } = kind else {
            return DecimalChar::Other;
        };
        let separator = self.document.settings().decimal_separator();
        let text = self.itemized.text.get(text.clone()).unwrap_or("");
        if text.contains(separator) {
            DecimalChar::Separator
        } else if !text.is_empty() && text.chars().all(|ch| ch.is_ascii_digit()) {
            DecimalChar::Digit
        } else {
            DecimalChar::Other
        }
    }

    /// Reports, once per paragraph, a unit wider than its whole line.
    fn report_overflow(&mut self, kind: &UnitKind) {
        if !self.overflow_reported {
            self.overflow_reported = true;
            let what = match kind {
                UnitKind::Object { .. } => "an inline object",
                _ => "a character",
            };
            self.fonts.report_once(
                ReportKind::Overflow,
                self.location,
                format!("{what} is wider than its line and reaches past its end"),
            );
        }
    }

    /// Whether unit `index` is the soft hyphen of a run.
    fn ends_in_soft_hyphen(&self, index: usize) -> bool {
        match self.units.get(index).map(|unit| &unit.kind) {
            Some(UnitKind::Glyphs { text, .. }) => {
                self.itemized.text.get(text.clone()) == Some("\u{00AD}")
            }
            _ => false,
        }
    }

    /// The width of the hyphen a line that ends at soft hyphen `index` draws.
    fn hyphen_width(&mut self, index: usize) -> Pt {
        let Some((item, run)) = self.hyphen(index) else {
            return Pt(0.0);
        };
        self.run_width(item, &run)
    }

    /// The hyphen drawn for soft hyphen `index`: item, and the hyphen shaped in its
    /// face and style.
    fn hyphen(&mut self, index: usize) -> Option<(usize, Arc<ShapedRun>)> {
        let item = self.units.get(index)?.kind.item();
        let Some(Item::Text(run)) = self.itemized.items.get(item) else {
            return None;
        };
        let face = self.fonts.face(run.face?)?;
        let style = self
            .document
            .run_style(run.style)
            .cloned()
            .unwrap_or_default();
        // U+2010 when the face has it, as Word draws it, else the hyphen-minus.
        let hyphen = if face.face().glyph_id('\u{2010}').is_some() {
            "\u{2010}"
        } else {
            "-"
        };
        let shaped = self.shaper.shape(&face, hyphen, &request(run, &style));
        Some((item, shaped))
    }

    /// The width of all of `run`, shaped text of item `item` that is one cluster (the
    /// hyphen of a soft hyphen).
    fn run_width(&self, item: usize, run: &ShapedRun) -> Pt {
        let Some(scale) = self.scales.get(item).copied().flatten() else {
            return Pt(0.0);
        };
        let advance: i64 = run
            .glyphs()
            .iter()
            .map(|glyph| i64::from(glyph.x_advance))
            .sum();
        // One cluster, measured as `Piece::glyphs` draws it.
        Pt(advance as f64 * scale.per_unit + cluster_spacing(advance, scale))
    }

    /// The end of the line from unit `start` to before unit `end`, after shaping again
    /// the run a break cuts where its shaping depends on what follows. Returns the
    /// unit after the line's last, which shaping again can move.
    fn settle(&mut self, start: usize, end: usize, line_end: LineEnd) -> usize {
        self.splice = None;
        if line_end != LineEnd::Wrap || end <= start || end >= self.units.len() {
            return end;
        }
        let (Some(before), Some(after)) = (self.units.get(end - 1), self.units.get(end)) else {
            return end;
        };
        let (
            UnitKind::Glyphs { item, run, .. },
            UnitKind::Glyphs {
                item: next_item,
                run: next_run,
                glyphs,
                ..
            },
        ) = (&before.kind, &after.kind)
        else {
            return end;
        };
        if item != next_item
            || !Arc::ptr_eq(run, next_run)
            || !run
                .glyphs()
                .get(glyphs.start)
                .is_some_and(|glyph| glyph.unsafe_to_break)
        {
            return end;
        }
        let item = *item;
        // Shape again only the piece around the cut that cannot be shaped on its own:
        // from the last unit on the line where a piece may start, to the first after the
        // cut where one may, at most a window of clusters on.
        let first = (start..end)
            .rev()
            .find(|&i| i == start || self.starts_piece(i))
            .unwrap_or(start);
        let mut last = end + 1;
        while last < self.units.len() && last < end + RESHAPE_WINDOW && !self.starts_piece(last) {
            last += 1;
        }
        let text_of = |unit: Option<&Unit>| match unit.map(|unit| &unit.kind) {
            Some(UnitKind::Glyphs { text, .. }) => Some(text.clone()),
            _ => None,
        };
        let (Some(head), Some(cut), Some(tail)) = (
            text_of(self.units.get(first)),
            text_of(self.units.get(end)),
            text_of(self.units.get(last - 1)),
        ) else {
            return end;
        };
        let (Some(left), Some(right)) = (
            self.reshape(item, head.start..cut.start),
            self.reshape(item, cut.start..tail.end),
        ) else {
            return end;
        };
        let item_start = self.starts.get(item).copied().unwrap_or(0);
        let text_start = match self.itemized.items.get(item) {
            Some(Item::Text(run)) => run.range.start,
            _ => 0,
        };
        let offset = |text: usize| item_start + text - text_start;
        let left_units = self.clusters(item, &left, head.start..cut.start, offset(head.start));
        let right_units = self.clusters(item, &right, cut.start..tail.end, offset(cut.start));
        let new_end = first + left_units.len();
        let replacement: Vec<Unit> = left_units.into_iter().chain(right_units).collect();
        let len = replacement.len();
        let replaced: Vec<Unit> = self.units.splice(first..last, replacement).collect();
        self.splice = Some(Splice {
            first,
            len,
            replaced,
        });
        new_end
    }

    /// Whether unit `index` starts a piece that may be shaped on its own: it is not
    /// glyphs of the same run as the unit before it, or HarfRust marks its first glyph
    /// safe to break before.
    fn starts_piece(&self, index: usize) -> bool {
        let (Some(before), Some(unit)) = (
            index.checked_sub(1).and_then(|i| self.units.get(i)),
            self.units.get(index),
        ) else {
            return true;
        };
        match (&before.kind, &unit.kind) {
            (
                UnitKind::Glyphs {
                    item: a, run: ra, ..
                },
                UnitKind::Glyphs {
                    item: b,
                    run: rb,
                    glyphs,
                    ..
                },
            ) => {
                a != b
                    || !Arc::ptr_eq(ra, rb)
                    || !rb
                        .glyphs()
                        .get(glyphs.start)
                        .is_some_and(|glyph| glyph.unsafe_to_break)
            }
            _ => true,
        }
    }

    /// Bytes `text` of the drawn text of item `item`, shaped on their own.
    fn reshape(&mut self, item: usize, text: Range<usize>) -> Option<Arc<ShapedRun>> {
        let Some(Item::Text(run)) = self.itemized.items.get(item) else {
            return None;
        };
        let face = self.fonts.face(run.face?)?;
        let style = self
            .document
            .run_style(run.style)
            .cloned()
            .unwrap_or_default();
        let string = self.itemized.text.get(text)?;
        Some(self.shaper.shape(&face, string, &request(run, &style)))
    }

    /// The line from unit `start` to before unit `fit.end`, each piece where it goes, with
    /// a hyphen where the line ends at a soft hyphen that has room for one.
    fn place(&mut self, start: usize, fit: Fit, first: bool) -> Line {
        let end = fit.end;
        let (left, right) = self.edges(first);
        let mut cursor = Cursor::new(left);
        let mut pieces: Vec<Piece> = Vec::new();
        // Where the pieces after a pending tab start in `pieces`.
        let mut segment = 0;
        let mut content_end = left;
        // Where the content after a pending tab ends, from the tab's end, until the tab
        // is settled: spaces after it change the tab's width but are not content.
        let mut segment_content: Option<f64> = None;
        let units: Vec<Unit> = self.units.get(start..end).unwrap_or(&[]).to_vec();
        // The spaces after the last piece of content hang past the end of the line, in
        // pieces of their own: those before the break that ends the line, and those
        // before what takes no room, such as soft hyphens, too.
        // A tab that starts past the end hangs too.
        let hanging_tab = fit.hanging_tab.and_then(|tab| tab.checked_sub(start));
        let last_content = units
            .iter()
            .enumerate()
            .rposition(|(index, unit)| match unit.kind {
                UnitKind::Tab { .. } => hanging_tab.is_none_or(|tab| index < tab),
                UnitKind::Object { .. } => unit.width.0 > 0.0,
                UnitKind::Glyphs { .. } => unit.width.0 > 0.0 && !self.is_space_unit(unit),
                UnitKind::Break { .. } | UnitKind::Zero { .. } => false,
            })
            .map_or(0, |index| index + 1);
        for (index, unit) in units.iter().enumerate() {
            if let UnitKind::Tab { item } = unit.kind {
                // The tab after a pending one settles it; the line's content then ends
                // at this tab's end, unless the tab hangs.
                if let (Some(origin), Some(offset)) = (
                    cursor.settle_into(&mut pieces, segment),
                    segment_content.take(),
                ) {
                    content_end = Pt(origin + offset);
                }
                let hanging = index >= last_content;
                let (x, width, leader) = cursor.tab(&self.tabs, right);
                pieces.push(Piece {
                    x,
                    width,
                    kind: PieceKind::Tab { item, leader },
                    hanging,
                });
                segment = pieces.len();
                if !hanging {
                    content_end = cursor.x;
                }
                continue;
            }
            let content = index < last_content;
            // Before the pending tab is settled, a piece's place is its offset from
            // the tab's end.
            let x = cursor.pending.map_or(cursor.x, |pending| Pt(pending.text));
            if self.is_space_unit(unit) {
                cursor.space(unit.width);
            } else {
                let what = self.decimal_char(&unit.kind);
                cursor.add(unit.width, what);
            }
            if content {
                match cursor.pending {
                    Some(pending) => segment_content = Some(pending.text),
                    None => content_end = cursor.x,
                }
            }
            let scale = self
                .scales
                .get(unit.kind.item())
                .copied()
                .flatten()
                .unwrap_or_default();
            push_piece(&mut pieces, x, unit, !content, index != last_content, scale);
        }
        if let (Some(origin), Some(offset)) = (
            cursor.settle_into(&mut pieces, segment),
            segment_content.take(),
        ) {
            content_end = Pt(origin + offset);
        }
        // The hyphen of a line that ends at a soft hyphen.
        if fit.hyphen && end > start && self.ends_in_soft_hyphen(end - 1) {
            if let Some((item, run)) = self.hyphen(end - 1) {
                let width = self.run_width(item, &run);
                let scale = self.scales.get(item).copied().flatten().unwrap_or_default();
                pieces.push(Piece {
                    x: content_end,
                    width,
                    kind: PieceKind::Hyphen { item, run, scale },
                    hanging: false,
                });
                content_end = Pt(content_end.0 + width.0);
            }
        }
        let range_start = self
            .units
            .get(start)
            .map_or(self.text.len(), |unit| unit.range.start);
        let range_end = units.last().map_or(range_start, |unit| unit.range.end);
        Line {
            pieces,
            range: range_start..range_end.max(range_start),
            start: left,
            end_edge: right,
            width: Pt(content_end.0 - left.0),
            ascent: Pt(0.0),
            descent: Pt(0.0),
            height: Pt(0.0),
            baseline: Pt(0.0),
            end: fit.end_kind,
            overflow: fit.overflow,
        }
    }

    /// Whether `unit` is a space that may hang at the end of a line.
    fn is_space_unit(&self, unit: &Unit) -> bool {
        matches!(&unit.kind, UnitKind::Glyphs { text, .. } if is_space(self.itemized, text))
    }

    /// Aligns `line`: centred, at the end, or justified by widening its spaces (every
    /// line but the `last`, and a line that ends in a line break unless the document
    /// says otherwise) or by spacing out every cluster (every line).
    fn align(&mut self, line: &mut Line, last: bool) {
        let room = line.end_edge.0 - line.start.0 - line.width.0;
        if room <= 0.0 {
            return;
        }
        let expand_break = !self
            .document
            .settings()
            .has_compat_flag(&CompatFlag::DoNotExpandShiftReturn);
        match self.paragraph.props().alignment() {
            Alignment::Start => {}
            Alignment::Center => shift(line, Pt(room / 2.0)),
            Alignment::End => shift(line, Pt(room)),
            Alignment::Both => {
                let justified = match line.end {
                    LineEnd::Wrap => !last,
                    LineEnd::Break => expand_break,
                    LineEnd::PageBreak | LineEnd::ColumnBreak | LineEnd::Paragraph => false,
                };
                if justified {
                    self.stretch(line, room, Stretch::Spaces);
                }
            }
            Alignment::Distribute => self.stretch(line, room, Stretch::Clusters),
        }
    }

    /// Spreads `room` over the spaces or the clusters of `line` after its last tab that
    /// does not hang, leaving out the spaces that hang at its end; distributed text gets
    /// nothing after its last cluster.
    fn stretch(&self, line: &mut Line, room: f64, how: Stretch) {
        let from = line
            .pieces
            .iter()
            .rposition(|piece| matches!(piece.kind, PieceKind::Tab { .. }) && !piece.hanging)
            .map_or(0, |tab| tab + 1);
        // The stretched pieces, and how many places each widens.
        let mut counts: Vec<(usize, usize)> = Vec::new();
        for (index, piece) in line.pieces.iter().enumerate().skip(from) {
            if piece.hanging {
                break;
            }
            if let PieceKind::Text {
                text, run, glyphs, ..
            } = &piece.kind
            {
                let count = match how {
                    Stretch::Spaces => stretchable_spaces(self.itemized, text),
                    Stretch::Clusters => clusters(run, glyphs),
                };
                counts.push((index, count));
            }
        }
        let mut total: usize = counts.iter().map(|&(_, count)| count).sum();
        if how == Stretch::Clusters {
            total = total.saturating_sub(1);
        }
        if total == 0 {
            return;
        }
        let extra = room / total as f64;
        let last_stretched = counts.last().map(|&(index, _)| index);
        // How many places each piece widens, by its index.
        let mut by_piece: Vec<Option<usize>> = vec![None; line.pieces.len()];
        for &(index, count) in &counts {
            if let Some(slot) = by_piece.get_mut(index) {
                *slot = Some(count);
            }
        }
        let mut moved = 0.0;
        for (index, piece) in line.pieces.iter_mut().enumerate().skip(from) {
            piece.x = Pt(piece.x.0 + moved);
            let Some(count) = by_piece.get(index).copied().flatten() else {
                continue;
            };
            let PieceKind::Text {
                space_extra,
                cluster_extra,
                last_extra,
                ..
            } = &mut piece.kind
            else {
                continue;
            };
            let places = match how {
                Stretch::Spaces => {
                    *space_extra = Pt(extra);
                    count
                }
                Stretch::Clusters => {
                    *cluster_extra = Pt(extra);
                    if Some(index) == last_stretched {
                        *last_extra = false;
                        count.saturating_sub(1)
                    } else {
                        count
                    }
                }
            };
            let added = places as f64 * extra;
            piece.width = Pt(piece.width.0 + added);
            moved += added;
        }
        line.width = Pt(line.width.0 + moved);
    }

    /// Sets the height and baseline of `line`, the `last` of its paragraph or not.
    fn set_height(&mut self, line: &mut Line, last: bool) {
        let no_leading = self
            .document
            .settings()
            .has_compat_flag(&CompatFlag::NoLeading);
        let mut extent = Extent::default();
        for piece in &line.pieces {
            match &piece.kind {
                PieceKind::Text { item, .. } | PieceKind::Hyphen { item, .. } => {
                    let metrics = self.text_metrics(*item);
                    extent.add(metrics, self.position(*item));
                }
                PieceKind::Tab { item, .. } | PieceKind::Break { item } => {
                    if let Some(style) = item_style(self.itemized, *item) {
                        let metrics = self.style_metrics(style);
                        extent.add(metrics, Pt(0.0));
                    }
                }
                PieceKind::Object { item, height } => {
                    if let Some(height) = height {
                        extent.ascent = extent.ascent.max(height.0);
                        extent.empty = false;
                    } else if let Some(style) = item_style(self.itemized, *item) {
                        let metrics = self.style_metrics(style);
                        extent.add(metrics, Pt(0.0));
                    }
                }
            }
        }
        // The line with the paragraph mark includes it; an empty line has only the
        // mark, or the break that ends it.
        if last || extent.empty {
            let metrics = self.style_metrics(self.paragraph.mark());
            extent.add(metrics, Pt(0.0));
        }
        let leading = if no_leading { 0.0 } else { extent.leading };
        let natural = extent.ascent + extent.descent + leading;
        let top = leading + extent.ascent;
        let (height, baseline) = match self.line_spacing() {
            LineSpacing::Multiple(factor) if factor >= 1.0 => (natural * factor, top),
            // Lines closer than single are scaled whole: the text keeps its place in
            // the line (E-line-multiple, 0.5 and 0.83 lines).
            LineSpacing::Multiple(factor) => (natural * factor, top * factor),
            LineSpacing::Exact(height) => (height.0, height.0 * EXACT_BASELINE),
            // The room the minimum adds goes above the text (E-line-at-least).
            LineSpacing::AtLeast(minimum) => {
                let height = natural.max(minimum.0);
                (height, height - natural + top)
            }
        };
        line.ascent = Pt(extent.ascent);
        line.descent = Pt(extent.descent);
        line.height = Pt(height);
        line.baseline = Pt(baseline);
    }

    /// The paragraph's line spacing, with the values Word has no meaning for taken as
    /// single lines and reported.
    fn line_spacing(&mut self) -> LineSpacing {
        let spacing = self.paragraph.props().spacing().line();
        let degenerate = match spacing {
            LineSpacing::Multiple(factor) => factor <= 0.0,
            LineSpacing::Exact(height) => height.0 <= 0.0,
            LineSpacing::AtLeast(_) => false,
        };
        if degenerate {
            self.fonts.report_once(
                ReportKind::ValueClamped,
                self.location,
                format!(
                    "line spacing {} has no height; the lines are single",
                    describe(spacing)
                ),
            );
            return LineSpacing::Multiple(1.0);
        }
        spacing
    }

    /// The vertical metrics of text item `item`: those of its face at the size its
    /// formatting gives, which small capitals, superscripts and subscripts do not
    /// change.
    fn text_metrics(&mut self, item: usize) -> LineMetrics {
        let Some(Item::Text(run)) = self.itemized.items.get(item) else {
            return no_font(Pt(10.0));
        };
        let size = run.formatted_size;
        match run.face.and_then(|id| self.fonts.face(id)) {
            Some(face) => line_metrics(face.face(), size, TypoRule::default()),
            None => no_font(size),
        }
    }

    /// How far text item `item` is raised (positive) or lowered by its position
    /// (`w:position`), which moves the line's top or bottom with it. The rise of a
    /// superscript and the drop of a subscript do not: the line keeps the height of
    /// their full size (a hypothesis, like their size).
    fn position(&self, item: usize) -> Pt {
        let Some(Item::Text(run)) = self.itemized.items.get(item) else {
            return Pt(0.0);
        };
        self.document
            .run_style(run.style)
            .map_or(Pt(0.0), |style| style.position())
    }

    /// The vertical metrics of text in run style `id`, in the face of its first font.
    fn style_metrics(&mut self, id: RunStyleId) -> LineMetrics {
        let style = self.document.run_style(id).cloned().unwrap_or_default();
        let complex = style.complex_script() || style.rtl();
        let (family, size, bold, italic) = if complex {
            (
                style.fonts().complex_script(),
                style.size_complex_script(),
                style.bold_complex_script(),
                style.italic_complex_script(),
            )
        } else {
            (
                style.fonts().ascii(),
                style.size(),
                style.bold(),
                style.italic(),
            )
        };
        let choice = self.fonts.choose(family, bold, italic);
        let face: Option<Arc<ShapingFace>> = choice.face.and_then(|id| self.fonts.face(id));
        match face {
            Some(face) => line_metrics(face.face(), size, TypoRule::default()),
            None => no_font(size),
        }
    }
}

/// Line spacing in words, for the report.
fn describe(spacing: LineSpacing) -> String {
    match spacing {
        LineSpacing::Multiple(factor) => format!("of {factor} lines"),
        LineSpacing::Exact(height) => format!("of exactly {} pt", height.0),
        LineSpacing::AtLeast(height) => format!("of at least {} pt", height.0),
    }
}

/// What justification spreads the room over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stretch {
    Spaces,
    Clusters,
}

/// What a unit after a decimal tab holds, which decides where the text aligns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecimalChar {
    Separator,
    Digit,
    Other,
}

/// The height a line's text reaches above and below its baseline, and the leading.
#[derive(Clone, Copy, Debug)]
struct Extent {
    ascent: f64,
    descent: f64,
    leading: f64,
    empty: bool,
}

impl Default for Extent {
    fn default() -> Self {
        Self {
            ascent: 0.0,
            descent: 0.0,
            leading: 0.0,
            empty: true,
        }
    }
}

impl Extent {
    /// Adds text with `metrics`, raised by `shift`.
    fn add(&mut self, metrics: LineMetrics, shift: Pt) {
        self.ascent = self.ascent.max(metrics.ascent.0 + shift.0);
        self.descent = self.descent.max(metrics.descent.0 - shift.0);
        self.leading = self.leading.max(metrics.leading.0);
        self.empty = false;
    }
}

/// The metrics of text at `size` when there is no font at all.
fn no_font(size: Pt) -> LineMetrics {
    LineMetrics {
        ascent: Pt(size.0 * 0.8),
        descent: Pt(size.0 * 0.2),
        leading: Pt(0.0),
    }
}

/// Where the text of a line has got to, with the tab whose alignment waits for the
/// text after it.
struct Cursor {
    x: Pt,
    /// Whether the line has anything but spaces.
    has_content: bool,
    pending: Option<Pending>,
}

/// A centred, end or decimal tab that waits for the text after it.
#[derive(Clone, Copy, Debug)]
struct Pending {
    /// Where the tab starts.
    start: Pt,
    stop: Stop,
    /// The width of the text after the tab so far.
    text: f64,
    /// The width of that text before its decimal separator, once there is one.
    before_separator: Option<f64>,
    /// The width at the end of its first number, once that has ended.
    number_end: Option<f64>,
    /// Whether the first number has started.
    in_number: bool,
}

impl Pending {
    /// The part of the text after the tab that goes before the stop.
    fn before_stop(&self) -> f64 {
        match self.stop.kind {
            TabKind::Center => self.text / 2.0,
            TabKind::Decimal => self
                .before_separator
                .or(self.number_end)
                .or(if self.in_number {
                    Some(self.text)
                } else {
                    None
                })
                .unwrap_or(self.text),
            _ => self.text,
        }
    }

    /// The width of the tab.
    fn width(&self) -> f64 {
        (self.stop.position.0 - self.start.0 - self.before_stop()).max(0.0)
    }

    /// Adds text `width` wide, holding `what`.
    fn add(&mut self, width: f64, what: DecimalChar) {
        match what {
            DecimalChar::Separator if self.before_separator.is_none() => {
                self.before_separator = Some(self.text);
            }
            DecimalChar::Digit => self.in_number = true,
            _ => {
                if self.in_number && self.number_end.is_none() {
                    self.number_end = Some(self.text);
                }
            }
        }
        self.text += width;
    }
}

impl Cursor {
    fn new(start: Pt) -> Self {
        Self {
            x: start,
            has_content: false,
            pending: None,
        }
    }

    /// Where a piece `width` wide holding `what` would end.
    fn peek(&self, width: Pt, what: DecimalChar) -> Pt {
        match self.pending {
            Some(pending) => {
                let mut pending = pending;
                pending.add(width.0, what);
                Pt(pending.start.0 + pending.width() + pending.text)
            }
            None => Pt(self.x.0 + width.0),
        }
    }

    /// Adds a piece `width` wide holding `what`.
    fn add(&mut self, width: Pt, what: DecimalChar) {
        self.x = self.peek(width, what);
        if let Some(pending) = &mut self.pending {
            pending.add(width.0, what);
        }
        self.has_content = true;
    }

    /// Adds a space, which may hang past the end of the line.
    fn space(&mut self, width: Pt) {
        self.x = self.peek(width, DecimalChar::Other);
        if let Some(pending) = &mut self.pending {
            pending.add(width.0, DecimalChar::Other);
        }
    }

    /// Settles the pending tab: its width, and the places of the pieces from
    /// `segment` on, which until then are offsets from where the tab starts. Returns
    /// where the text after it starts, `None` without a pending tab.
    fn settle_into(&mut self, pieces: &mut [Piece], segment: usize) -> Option<f64> {
        let pending = self.pending.take()?;
        let width = pending.width();
        if let Some(tab) = segment
            .checked_sub(1)
            .and_then(|index| pieces.get_mut(index))
        {
            tab.width = Pt(width);
        }
        let origin = pending.start.0 + width;
        for piece in pieces.get_mut(segment..).unwrap_or(&mut []) {
            piece.x = Pt(origin + piece.x.0);
        }
        Some(origin)
    }

    /// Adds a tab: the stop it goes to after the current place, given the end of the
    /// line. Returns where the tab starts, its width so far and its leader.
    ///
    /// A tab whose stop is past the end of the line, or that has no stop, reaches the
    /// end of the line, so that the text after it goes on the next line. That is a
    /// hypothesis until a Word render (`E-tab-overflow`).
    fn tab(&mut self, tabs: &TabStops, end: Pt) -> (Pt, Pt, TabLeader) {
        if let Some(pending) = self.pending.take() {
            self.x = Pt(pending.start.0 + pending.width() + pending.text);
        }
        let start = self.x;
        self.has_content = true;
        let stop = tabs.next(start).filter(|stop| stop.position.0 <= end.0);
        let Some(stop) = stop else {
            let width = Pt((end.0 - start.0).max(0.0));
            self.x = Pt(start.0 + width.0);
            return (start, width, TabLeader::None);
        };
        match stop.kind {
            TabKind::Center | TabKind::End | TabKind::Decimal => {
                self.pending = Some(Pending {
                    start,
                    stop,
                    text: 0.0,
                    before_separator: None,
                    number_end: None,
                    in_number: false,
                });
                self.x = Pt(stop.position.0.max(start.0));
                (start, Pt(self.x.0 - start.0), stop.leader)
            }
            _ => {
                self.x = stop.position;
                (start, Pt(stop.position.0 - start.0), stop.leader)
            }
        }
    }
}

/// Adds unit `unit` at `x` to `pieces`, joining it to the text piece before it when it
/// continues it and `join` allows.
fn push_piece(
    pieces: &mut Vec<Piece>,
    x: Pt,
    unit: &Unit,
    hanging: bool,
    join: bool,
    scale: Scale,
) {
    if let UnitKind::Glyphs {
        item,
        run,
        glyphs,
        text,
    } = &unit.kind
    {
        if let Some(Piece {
            width,
            kind:
                PieceKind::Text {
                    item: last_item,
                    run: last_run,
                    glyphs: last_glyphs,
                    text: last_text,
                    ..
                },
            ..
        }) = pieces.last_mut()
        {
            if join
                && last_item == item
                && Arc::ptr_eq(last_run, run)
                && last_glyphs.end == glyphs.start
            {
                last_glyphs.end = glyphs.end;
                last_text.end = text.end;
                *width = Pt(width.0 + unit.width.0);
                return;
            }
        }
        pieces.push(Piece {
            x,
            width: unit.width,
            kind: PieceKind::Text {
                item: *item,
                run: Arc::clone(run),
                glyphs: glyphs.clone(),
                text: text.clone(),
                scale,
                space_extra: Pt(0.0),
                cluster_extra: Pt(0.0),
                last_extra: true,
            },
            hanging,
        });
        return;
    }
    let kind = match unit.kind {
        UnitKind::Break { item, .. } => PieceKind::Break { item },
        UnitKind::Tab { item } => PieceKind::Tab {
            item,
            leader: TabLeader::None,
        },
        UnitKind::Object { item, height } => PieceKind::Object {
            item,
            height: Some(height),
        },
        UnitKind::Zero { item } => PieceKind::Object { item, height: None },
        UnitKind::Glyphs { .. } => return,
    };
    pieces.push(Piece {
        x,
        width: unit.width,
        kind,
        hanging,
    });
}

/// Moves every piece of `line` by `by`.
fn shift(line: &mut Line, by: Pt) {
    for piece in &mut line.pieces {
        piece.x = Pt(piece.x.0 + by.0);
    }
    line.start = Pt(line.start.0 + by.0);
}

/// The break text of `itemized`: its drawn text with a character standing for each
/// piece that is not text, and where each item starts in it.
pub(crate) fn break_text(itemized: &Itemized) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut starts = Vec::with_capacity(itemized.items.len());
    for item in &itemized.items {
        starts.push(text.len());
        match item {
            Item::Text(run) => text.push_str(itemized.text.get(run.range.clone()).unwrap_or("")),
            Item::Tab(_) => text.push(PLACEHOLDER_TAB),
            Item::Break(..) => text.push(PLACEHOLDER_BREAK),
            Item::Object(_) => text.push(PLACEHOLDER_OBJECT),
            Item::NoteReference(..) | Item::NoteMark(_) => text.push(PLACEHOLDER_MARK),
            // An anchored object takes no room and does not change where lines end.
            Item::Anchored(_) => {}
        }
    }
    (text, starts)
}

/// The spacing after a cluster of `units` design units in a run measured by `scale`:
/// the spacing after each character (`w:spacing`), but none after a cluster that takes no
/// room, such as a soft hyphen's or a joiner's, and no more condensing than the cluster
/// is wide, so that text never moves back. Both are hypotheses until a Word render
/// (`E-spacing-condensed.docx`).
fn cluster_spacing(units: i64, scale: Scale) -> f64 {
    if units == 0 {
        return 0.0;
    }
    let advance = units as f64 * scale.per_unit;
    (advance + scale.letter_spacing.0).max(0.0) - advance
}

/// Whether bytes `text` of the drawn text are a space that may hang at the end of a
/// line.
fn is_space(itemized: &Itemized, text: &Range<usize>) -> bool {
    itemized
        .text
        .get(text.clone())
        .is_some_and(|text| !text.is_empty() && text.chars().all(|ch| ch == ' '))
}

/// How many spaces that justification widens bytes `text` of the drawn text hold: the
/// space and the no-break space.
fn stretchable_spaces(itemized: &Itemized, text: &Range<usize>) -> usize {
    itemized.text.get(text.clone()).map_or(0, |text| {
        text.chars()
            .filter(|&ch| ch == ' ' || ch == '\u{00A0}')
            .count()
    })
}

/// How many clusters glyphs `glyphs` of `run` form.
fn clusters(run: &ShapedRun, glyphs: &Range<usize>) -> usize {
    let glyphs = run.glyphs().get(glyphs.clone()).unwrap_or(&[]);
    let mut count = 0;
    let mut previous = None;
    for glyph in glyphs {
        if previous != Some(glyph.cluster) {
            count += 1;
            previous = Some(glyph.cluster);
        }
    }
    count
}

/// Why a line that a break of `kind` ends, ends.
fn line_end(kind: BreakKind) -> LineEnd {
    match kind {
        BreakKind::Page => LineEnd::PageBreak,
        BreakKind::Column => LineEnd::ColumnBreak,
        _ => LineEnd::Break,
    }
}

/// The run style of item `item`.
fn item_style(itemized: &Itemized, item: usize) -> Option<RunStyleId> {
    match itemized.items.get(item)? {
        Item::Text(run) => Some(run.style),
        Item::Tab(style)
        | Item::Break(_, style)
        | Item::NoteReference(_, style)
        | Item::NoteMark(style) => Some(*style),
        Item::Object(_) | Item::Anchored(_) => None,
    }
}

/// The inline at `path` among `inlines`: an index, then indexes into the results of
/// fields.
pub(crate) fn inline_at<'a>(inlines: &'a [Inline], path: &[usize]) -> Option<&'a Inline> {
    let (first, rest) = path.split_first()?;
    let inline = inlines.get(*first)?;
    if rest.is_empty() {
        return Some(inline);
    }
    match inline {
        Inline::Field(field) => inline_at(field.result(), rest),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Alignment, ParagraphProps, RunFonts, RunStyle, Spacing};
    use crate::paragraph::{ParagraphLayout, lay_out};
    use crate::testing::{book, minimal};

    /// Lays out the inlines `make` gives for a run style in the test font at 10 pt
    /// (space 2.5 pt, `A` 6 pt; lines of 9 pt above the baseline and 2.5 below) in a
    /// column `width` wide. Returns the layout and the report's messages.
    fn lay(
        make: impl Fn(RunStyleId) -> Vec<Inline>,
        props: ParagraphProps,
        width: f64,
    ) -> (ParagraphLayout, Vec<String>) {
        let book = book(vec![minimal()]);
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let paragraph = Paragraph::new(style)
            .with_props(props)
            .with_inlines(make(style));
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let location = Location::default();
        let layout = lay_out(
            &document,
            &paragraph,
            Pt(width),
            &mut fonts,
            &mut shaper,
            &location,
        );
        let report = fonts.into_report();
        let messages = report
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect();
        (layout, messages)
    }

    fn text(text: &'static str) -> impl Fn(RunStyleId) -> Vec<Inline> {
        move |style| vec![Inline::text(text, style)]
    }

    fn widths(layout: &ParagraphLayout) -> Vec<f64> {
        layout.lines.iter().map(|line| line.width.0).collect()
    }

    #[test]
    fn fills_lines_first_fit_with_spaces_hanging() {
        // "AA " is 14.5 pt; three words are 41 pt.
        let (layout, report) = lay(text("AA AA AA"), ParagraphProps::default(), 30.0);
        assert_eq!(widths(&layout), [26.5, 12.0]);
        let ends: Vec<LineEnd> = layout.lines.iter().map(|line| line.end).collect();
        assert_eq!(ends, [LineEnd::Wrap, LineEnd::Paragraph]);
        assert_eq!(layout.height(), Pt(23.0));
        assert!(report.is_empty());
        // A word longer than the line breaks after the last character that fits.
        let (layout, _) = lay(text("AAAAAAAAAA"), ParagraphProps::default(), 20.0);
        assert_eq!(widths(&layout), [18.0, 18.0, 18.0, 6.0]);
    }

    #[test]
    fn undoing_a_splice_maps_an_end_by_its_place_in_the_text() {
        // "AAAA" in the test font: a unit per letter, at bytes 0, 1, 2 and 3.
        let book = book(vec![minimal()]);
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let paragraph = Paragraph::new(style).with_inlines([Inline::text("AAAA", style)]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let location = Location::default();
        let itemized =
            crate::itemize::itemize(&document, paragraph.inlines(), &mut fonts, &location);
        let shaped =
            crate::itemize::shape_runs(&document, &itemized, &mut fonts, &mut shaper, &location);
        let input = Input {
            document: &document,
            paragraph: &paragraph,
            width: Pt(100.0),
            location: &location,
        };
        let starts = |breaker: &Breaker<'_, '_, '_>| -> Vec<usize> {
            breaker.units.iter().map(|unit| unit.range.start).collect()
        };
        let mut breaker = Breaker::new(input, &itemized, &shaped, &mut fonts, &mut shaper);
        assert_eq!(starts(&breaker), [0, 1, 2, 3]);
        // Shaped again, letters 1 to 3 became two units, as a ligature over 1 and 2
        // would make them: units at 0, 1 and 3.
        let splice_like = |breaker: &mut Breaker<'_, '_, '_>| {
            let mut joined = breaker.units.get(1).cloned().expect("a unit");
            joined.range = 1..3;
            let last = breaker.units.get(3).cloned().expect("a unit");
            let replaced: Vec<Unit> = breaker.units.splice(1..4, [joined, last]).collect();
            Splice {
                first: 1,
                len: 2,
                replaced,
            }
        };
        // An end before the splice stays; one inside it goes to the unit that starts
        // where it did (byte 3, now unit 3); one after it moves with the units.
        for (end, mapped) in [(1, 1), (2, 3), (3, 4)] {
            let splice = splice_like(&mut breaker);
            assert_eq!(starts(&breaker), [0, 1, 3]);
            assert_eq!(breaker.undo(splice, end), mapped, "end {end}");
            assert_eq!(starts(&breaker), [0, 1, 2, 3]);
        }
    }

    #[test]
    fn heights_follow_the_line_spacing() {
        let measure = |line| {
            let props = ParagraphProps::default().with_spacing(Spacing::default().with_line(line));
            let (layout, report) = lay(text("A"), props, 100.0);
            let line = layout.lines.first().cloned().expect("a line");
            let round = |pt: Pt| (pt.0 * 1e6).round() / 1e6;
            (round(line.height), round(line.baseline), report.len())
        };
        // Single: 11.5 pt, the baseline 9 pt down.
        assert_eq!(measure(LineSpacing::Multiple(1.0)), (11.5, 9.0, 0));
        // Double: the room goes below the text.
        assert_eq!(measure(LineSpacing::Multiple(2.0)), (23.0, 9.0, 0));
        // Half: the line shrinks whole.
        assert_eq!(measure(LineSpacing::Multiple(0.5)), (5.75, 4.5, 0));
        // Exactly 20 pt: the baseline at 80 %.
        assert_eq!(measure(LineSpacing::Exact(Pt(20.0))), (20.0, 16.0, 0));
        // At least 20 pt: the room goes above the text; less than single does nothing.
        assert_eq!(measure(LineSpacing::AtLeast(Pt(20.0))), (20.0, 17.5, 0));
        assert_eq!(measure(LineSpacing::AtLeast(Pt(5.0))), (11.5, 9.0, 0));
        // No height at all: single lines, reported.
        assert_eq!(measure(LineSpacing::Exact(Pt(0.0))), (11.5, 9.0, 1));
        assert_eq!(measure(LineSpacing::Multiple(-1.0)), (11.5, 9.0, 1));
    }

    #[test]
    fn tabs_go_to_the_automatic_stops_and_justification_widens_spaces() {
        let tabbed = |style| {
            vec![
                Inline::text("A", style),
                Inline::tab(style),
                Inline::text("A", style),
            ]
        };
        let (layout, _) = lay(tabbed, ParagraphProps::default(), 100.0);
        let line = layout.lines.first().expect("a line");
        let starts: Vec<f64> = line.pieces.iter().map(|piece| piece.x.0).collect();
        // The tab runs from 6 pt to the stop at 36 pt.
        assert_eq!(starts, [0.0, 6.0, 36.0]);
        assert_eq!(line.width.0, 42.0);
        // Justified: the first of two lines widened to 30 pt by its one space, which
        // takes all of the 3.5 pt; the space at its end still hangs.
        let both = ParagraphProps::default().with_alignment(Alignment::Both);
        let (layout, _) = lay(text("AA AA AA"), both, 30.0);
        assert_eq!(widths(&layout), [30.0, 12.0]);
        let line = layout.lines.first().expect("a line");
        let glyphs: Vec<(f64, f64)> = line
            .pieces
            .iter()
            .flat_map(|piece| piece.glyphs(&layout.itemized))
            .map(|glyph| (glyph.x.0, glyph.advance.0))
            .collect();
        assert_eq!(
            glyphs,
            [
                (0.0, 6.0),
                (6.0, 6.0),
                (12.0, 6.0),
                (18.0, 6.0),
                (24.0, 6.0),
                (30.0, 2.5)
            ]
        );
    }
}
