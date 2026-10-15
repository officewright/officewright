//! Tables in a column: the geometry of their grid, their cells laid out as stacks of
//! paragraphs and nested tables, their borders and their shading (design §8).
//!
//! Rows tied together by a vertically merged cell form a band, which a page takes whole:
//! a row is never split (plan PR 13), and a merged cell is never cut. Column widths are
//! the grid's, scaled to an explicit table width; fitting columns to their content
//! (autofit) comes with plan PR 13, so the grid Word saved with its own results stands in.

use std::ops::Range;

use officewright_core::{Color, Pt};

use crate::display::{self, DisplayItem, LeaderBudget, LineItem, LineStyle, Painting, Point};
use crate::fonts::Fonts;
use crate::ir::{
    Block, Border, BorderSide, BorderStyle, Cell, CellMargins, CellVerticalAlign, ColorValue,
    CompatMode, Document, Paragraph, PreferredWidth, RowHeight, Table, TableAlignment,
    TableBorders, TableLayout, TextDirection, VerticalMerge,
};
use crate::itemize::FieldValues;
use crate::lists::Label;
use crate::paragraph;
use crate::report::{Location, ReportKind, Step};
use crate::shape::Shaper;

/// The widest a grid column is laid out: Word's widest page, 22 inches. A wider one is
/// laid out that wide, and reported.
const MAX_COLUMN: f64 = 1584.0;

/// The widest a table is laid out, as a percentage of its column or in points: as far as
/// a PDF places anything from a page's corner.
const MAX_TABLE: f64 = 32767.0;

/// What lays out the content of cells: the document, the fonts and the shaper, the
/// leader glyphs that may still be drawn, and the values of the fields the layout
/// computes, for the page the table is laid out for.
pub(crate) struct Context<'d, 'f, 'b> {
    pub(crate) document: &'d Document,
    pub(crate) fonts: &'f mut Fonts<'b>,
    pub(crate) shaper: &'f mut Shaper,
    pub(crate) leaders: &'f mut LeaderBudget,
    pub(crate) values: Option<&'f FieldValues>,
}

/// What a stack of blocks, or a band of a table, draws: its height and its display
/// items, from its top left corner.
#[derive(Debug, Default)]
pub(crate) struct Drawn {
    pub(crate) height: Pt,
    pub(crate) items: Vec<DisplayItem>,
}

/// How a cell takes part in vertical merging.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Merge {
    /// A cell of its own row.
    Single,
    /// The first cell of a merge, over this many rows from its own.
    Spans(usize),
    /// Below the first cell of a merge: drawn by that cell.
    Continues,
}

/// A cell in its row: its place among the row's cells, its grid columns and how it
/// merges. The columns of the cells of a row follow one another; a cell past the grid has
/// none, at its end.
#[derive(Clone, Debug)]
struct Placed {
    cell: usize,
    columns: Range<usize>,
    merge: Merge,
}

/// A border along the grid columns `columns` of a grid line.
#[derive(Clone, Debug, PartialEq)]
struct Segment {
    columns: Range<usize>,
    border: Border,
}

/// A table prepared for a column: its grid, its cells' places, its bands and its
/// borders, laid out band by band.
pub(crate) struct Prepared<'d, 'l> {
    table: &'d Table,
    location: Location,
    /// The x of each grid line, from the column's start edge: one more than the columns.
    lines: Vec<f64>,
    /// Whether borders take room inside the cells, as in Word 2013's mode, or are
    /// centred on the grid lines outside the cells' text, as in earlier modes.
    borders_inside: bool,
    rows: Vec<Vec<Placed>>,
    bands: Vec<Range<usize>>,
    /// The borders drawn along the grid line above each row, and last below the last
    /// row: in order, each over the columns it runs along.
    horizontal: Vec<Vec<Segment>>,
    /// The borders drawn beside each row: in order, each with the grid line it is on.
    vertical: Vec<Vec<(usize, Border)>>,
    /// The labels of the table's paragraphs in reading order, and where each row's start.
    labels: &'l [Option<Label>],
    row_labels: Vec<usize>,
}

impl<'d, 'l> Prepared<'d, 'l> {
    /// Prepares `table` of `document` for a column `width` wide; `labels` are the list
    /// labels of its paragraphs in reading order, `location` its place, for the report.
    pub(crate) fn new(
        document: &'d Document,
        table: &'d Table,
        labels: &'l [Option<Label>],
        width: Pt,
        location: &Location,
        fonts: &mut Fonts<'_>,
    ) -> Self {
        report_unsupported(table, location, fonts);
        let rows = placements(table, location, fonts);
        let columns = table.grid().len();
        let bands = bands(&rows);
        let mut prepared = Self {
            table,
            location: location.clone(),
            lines: Vec::new(),
            borders_inside: document.settings().compat_mode() >= CompatMode::Word2013,
            horizontal: Vec::new(),
            vertical: Vec::new(),
            row_labels: row_labels(table),
            rows,
            bands,
            labels,
        };
        prepared.resolve_borders();
        prepared.lines = prepared.grid_lines(width.0, columns, fonts);
        prepared
    }

    /// How many bands the table has.
    pub(crate) fn band_count(&self) -> usize {
        self.bands.len()
    }

    /// How many paragraphs the table has, nested tables' included: the labels it takes.
    pub(crate) fn paragraphs(&self) -> usize {
        self.row_labels.last().copied().unwrap_or(0)
    }

    /// Lays out band `band`: its height, with the borders above its first row and, for
    /// the table's last band, below its last; and what it draws, from the band's top at
    /// the column's start edge.
    pub(crate) fn band(&self, band: usize, context: &mut Context<'d, '_, '_>) -> Drawn {
        let Some(rows) = self.bands.get(band).cloned() else {
            return Drawn::default();
        };
        let last = rows.end == self.rows.len();
        // The room each row's top border takes, and below the table its bottom border's.
        let thickness = |line: usize| -> f64 {
            self.horizontal
                .get(line)
                .into_iter()
                .flatten()
                .map(|segment| segment.border.width().0)
                .fold(0.0, f64::max)
        };
        // Each cell's content, laid out at its width, by row and place in the row.
        let mut labels = Labels {
            list: self.labels,
            next: self.row_labels.get(rows.start).copied().unwrap_or(0),
        };
        let mut contents: Vec<Vec<Option<(Drawn, CellMargins)>>> = Vec::new();
        for row in rows.clone() {
            let mut cells = Vec::new();
            for placed in self.rows.get(row).into_iter().flatten() {
                let cell = self.cell(row, placed.cell);
                let laid = match (cell, placed.merge) {
                    (Some(cell), Merge::Single | Merge::Spans(_)) => {
                        let margins = self.margins(cell);
                        let width = self.content_width(row, placed, &margins);
                        let mut location = self.location.clone();
                        for step in [
                            Step::Field("rows"),
                            Step::Index(row),
                            Step::Field("cells"),
                            Step::Index(placed.cell),
                            Step::Field("blocks"),
                        ] {
                            location.push(step);
                        }
                        let background = cell
                            .shading()
                            .and_then(|shading| {
                                display::fill_color(&shading, context.fonts, &location)
                            })
                            .or_else(|| self.table_background(context.fonts));
                        let drawn = if hidden_mark(cell) {
                            skip_labels(cell.blocks(), &mut labels);
                            Drawn::default()
                        } else {
                            stack(
                                cell.blocks(),
                                (width, background),
                                &mut labels,
                                &mut location,
                                context,
                            )
                        };
                        Some((drawn, margins))
                    }
                    (Some(cell), Merge::Continues) => {
                        // A cell merged into the one above draws nothing of its own, but
                        // its paragraphs keep their labels.
                        skip_labels(cell.blocks(), &mut labels);
                        None
                    }
                    (None, _) => None,
                };
                cells.push(laid);
            }
            contents.push(cells);
        }
        // Each row's height below its top border: its tallest cell of its own, as its
        // height rule says.
        let mut heights: Vec<f64> = Vec::new();
        for (offset, row) in rows.clone().enumerate() {
            let content = self
                .rows
                .get(row)
                .into_iter()
                .flatten()
                .zip(contents.get(offset).into_iter().flatten())
                .filter(|(placed, _)| placed.merge == Merge::Single)
                .filter_map(|(_, laid)| laid.as_ref())
                .map(|(drawn, margins)| margins.top().0 + drawn.height.0 + margins.bottom().0)
                .fold(0.0, f64::max);
            let height = match self.table.rows().get(row).map(|row| row.height()) {
                Some(RowHeight::AtLeast(least)) => content.max(least.0),
                // Word adds the row's largest bottom margin to an exact height
                // (MS-OI29500 2.1.180 d).
                Some(RowHeight::Exact(exact)) => exact.0 + self.largest_bottom_margin(row),
                _ => content,
            };
            heights.push(height);
        }
        // A merged cell taller than its rows makes the last of them taller (design 8.3).
        for (offset, row) in rows.clone().enumerate() {
            let row_cells = self.rows.get(row).into_iter().flatten();
            for (placed, laid) in row_cells.zip(contents.get(offset).into_iter().flatten()) {
                let (Merge::Spans(count), Some((drawn, margins))) = (placed.merge, laid) else {
                    continue;
                };
                let needed = margins.top().0 + drawn.height.0 + margins.bottom().0;
                let spanned = offset..(offset + count).min(heights.len());
                let available: f64 = heights
                    .get(spanned.clone())
                    .unwrap_or(&[])
                    .iter()
                    .sum::<f64>()
                    + spanned
                        .clone()
                        .skip(1)
                        .map(|inner| thickness(rows.start + inner))
                        .sum::<f64>();
                if needed > available {
                    if let Some(height) = spanned.last().and_then(|last| heights.get_mut(last)) {
                        *height += needed - available;
                    }
                }
            }
        }
        // The top of each row, from the band's top, and the band's bottom.
        let mut tops = Vec::new();
        let mut y = 0.0;
        for (offset, row) in rows.clone().enumerate() {
            tops.push(y);
            y += thickness(row) + heights.get(offset).copied().unwrap_or(0.0);
        }
        let bottom = y + if last {
            thickness(self.rows.len())
        } else {
            0.0
        };
        tops.push(y);
        let mut items = Vec::new();
        self.paint_table_shading((y, bottom), last, &mut items, context.fonts);
        // Shading, then content, cell by cell.
        for (offset, row) in rows.clone().enumerate() {
            let row_cells = self.rows.get(row).into_iter().flatten();
            for (placed, laid) in row_cells.zip(contents.get_mut(offset).into_iter().flatten()) {
                let Some((drawn, margins)) = laid.take() else {
                    continue;
                };
                let span = match placed.merge {
                    Merge::Spans(count) => count,
                    _ => 1,
                };
                let end = (offset + span).min(heights.len());
                let top = tops.get(offset).copied().unwrap_or(0.0);
                let below = tops.get(end).copied().unwrap_or(y);
                let (left, right) = self.cell_edges(placed);
                if let Some(color) = self
                    .cell(row, placed.cell)
                    .and_then(Cell::shading)
                    .and_then(|shading| {
                        display::fill_color(&shading, context.fonts, &self.location)
                    })
                {
                    if right > left && below > top {
                        items.push(DisplayItem::Rect {
                            at: Point {
                                x: Pt(left),
                                y: Pt(top),
                            },
                            width: Pt(right - left),
                            height: Pt(below - top),
                            color,
                        });
                    }
                }
                // The content below the row's top border and the cell's top margin,
                // aligned in the room the row leaves it.
                let content_top = top + thickness(row) + margins.top().0;
                let room = below - content_top - margins.bottom().0;
                let align = self
                    .cell(row, placed.cell)
                    .map_or(CellVerticalAlign::Top, Cell::vertical_align);
                let free = (room - drawn.height.0).max(0.0);
                let down = match align {
                    CellVerticalAlign::Center => free / 2.0,
                    CellVerticalAlign::Bottom => free,
                    _ => 0.0,
                };
                let x = left + margins.start().0 + self.inset(row, placed.columns.start);
                let mut content = drawn.items;
                shift(&mut content, x, content_top + down);
                items.extend(content);
            }
        }
        self.paint_borders((rows.clone(), &tops, bottom), &mut items);
        Drawn {
            height: Pt(bottom),
            items,
        }
    }

    /// Lays out every band, one under the other.
    pub(crate) fn whole(&self, context: &mut Context<'d, '_, '_>) -> Drawn {
        let mut drawn = Drawn::default();
        for band in 0..self.bands.len() {
            let mut next = self.band(band, context);
            shift(&mut next.items, 0.0, drawn.height.0);
            drawn.height = Pt(drawn.height.0 + next.height.0);
            drawn.items.extend(next.items);
        }
        drawn
    }

    /// Cell `index` of row `row`.
    fn cell(&self, row: usize, index: usize) -> Option<&'d Cell> {
        self.table.rows().get(row)?.cells().get(index)
    }

    /// The margins of `cell`: its own, else the table's.
    fn margins(&self, cell: &Cell) -> CellMargins {
        cell.margins()
            .unwrap_or_else(|| self.table.props().cell_margins())
    }

    /// The largest bottom margin of the cells of row `row`.
    fn largest_bottom_margin(&self, row: usize) -> f64 {
        self.table
            .rows()
            .get(row)
            .into_iter()
            .flat_map(|row| row.cells())
            .map(|cell| self.margins(cell).bottom().0)
            .fold(0.0, f64::max)
    }

    /// The left and right edge of a cell: the grid lines of its columns.
    fn cell_edges(&self, placed: &Placed) -> (f64, f64) {
        let at = |line: usize| self.lines.get(line).copied().unwrap_or(0.0);
        (at(placed.columns.start), at(placed.columns.end))
    }

    /// How far a cell's text is inset from its start edge at grid line `line` of row
    /// `row` besides its margin: in Word 2013's mode, by half the border there (Word's
    /// renders of `E-table-indent-14` and `-15`: 0.25 pt for a border of 0.5 pt).
    fn inset(&self, row: usize, line: usize) -> f64 {
        if !self.borders_inside {
            return 0.0;
        }
        self.vertical_border(row, line)
            .map_or(0.0, |border| border.width().0 / 2.0)
    }

    /// The border drawn along grid line `line` beside row `row`.
    fn vertical_border(&self, row: usize, line: usize) -> Option<&Border> {
        let borders = self.vertical.get(row)?;
        let index = borders.binary_search_by_key(&line, |(at, _)| *at).ok()?;
        borders.get(index).map(|(_, border)| border)
    }

    /// The width of a cell's content: its columns less its margins and, in Word 2013's
    /// mode, half the borders on either side.
    fn content_width(&self, row: usize, placed: &Placed, margins: &CellMargins) -> Pt {
        let (left, right) = self.cell_edges(placed);
        let borders = self.inset(row, placed.columns.start) + self.inset(row, placed.columns.end);
        Pt((right - left - margins.start().0 - margins.end().0 - borders).max(0.0))
    }

    /// The x of each grid line in a column `width` wide, the table placed as its
    /// alignment and indent say.
    fn grid_lines(&self, width: f64, columns: usize, fonts: &mut Fonts<'_>) -> Vec<f64> {
        let props = self.table.props();
        let mut widths: Vec<f64> = (0..columns)
            .map(|column| self.table.grid().get(column).map_or(0.0, |width| width.0))
            .collect();
        if widths.iter().any(|&width| width > MAX_COLUMN) {
            fonts.report_once(
                ReportKind::ValueClamped,
                &self.location,
                format!("table columns wider than {MAX_COLUMN} pt are laid out that wide"),
            );
            for width in &mut widths {
                *width = width.min(MAX_COLUMN);
            }
        }
        let total: f64 = widths.iter().sum();
        // An explicit width scales the grid to it: in points for a table of fixed
        // layout, in points or as a share of the column for one that fits its content,
        // whose grid holds Word's own last fitting.
        let target = match (props.width(), props.layout()) {
            (PreferredWidth::Points(points), TableLayout::Fixed) => Some(points.0),
            (PreferredWidth::Percent(percent), _) => {
                Some(percent / 100.0 * (width - props.indent().0).max(0.0))
            }
            _ => None,
        };
        if let Some(target) = target.map(|target| target.clamp(0.0, MAX_TABLE)) {
            if total > 0.0 && (target - total).abs() > 1e-6 {
                let scale = target / total;
                for width in &mut widths {
                    *width *= scale;
                }
            }
        }
        let total: f64 = widths.iter().sum();
        // The borders at the table's outer edges, in its first row.
        let edge = |line: usize| {
            self.vertical_border(0, line)
                .map_or(0.0, |border| border.width().0)
        };
        let outer = if self.borders_inside {
            (edge(0) + edge(columns)) / 2.0
        } else {
            0.0
        };
        // Word's renders of `E-table-indent-14` and `-15` (tables F and G): a table
        // aligned at the start sits at its indent, in Word 2010's mode and earlier
        // pulled out by its first cell's start margin, so that its text lines up with the
        // paragraphs'; a centred table is centred, its borders included in Word 2013's
        // mode, whatever its indent. A table at the end is placed likewise, by
        // hypothesis.
        let start = match props.alignment() {
            TableAlignment::Center => (width - total - outer) / 2.0,
            TableAlignment::End => width - total - outer,
            _ => {
                let pulled = if self.borders_inside {
                    0.0
                } else {
                    self.first_start_margin()
                };
                props.indent().0 - pulled
            }
        };
        let mut lines = Vec::with_capacity(columns + 1);
        let mut x = start;
        lines.push(x);
        for width in widths {
            x += width;
            lines.push(x);
        }
        lines
    }

    /// The start margin of the first cell of the first row.
    fn first_start_margin(&self) -> f64 {
        self.rows
            .first()
            .and_then(|row| row.first())
            .and_then(|placed| self.cell(0, placed.cell))
            .map_or(self.table.props().cell_margins().start().0, |cell| {
                self.margins(cell).start().0
            })
    }

    /// The colour the table's shading fills with.
    fn table_background(&self, fonts: &mut Fonts<'_>) -> Option<Color> {
        self.table
            .props()
            .shading()
            .and_then(|shading| display::fill_color(&shading, fonts, &self.location))
    }

    /// The table's shading under the cells of a band `height` high, down to `bottom`
    /// for the last band.
    fn paint_table_shading(
        &self,
        (height, bottom): (f64, f64),
        last: bool,
        items: &mut Vec<DisplayItem>,
        fonts: &mut Fonts<'_>,
    ) {
        let Some(color) = self.table_background(fonts) else {
            return;
        };
        let (left, right) = (
            self.lines.first().copied().unwrap_or(0.0),
            self.lines.last().copied().unwrap_or(0.0),
        );
        let height = if last { bottom } else { height };
        if right > left && height > 0.0 {
            items.push(DisplayItem::Rect {
                at: Point {
                    x: Pt(left),
                    y: Pt(0.0),
                },
                width: Pt(right - left),
                height: Pt(height),
                color,
            });
        }
    }

    /// The borders of `rows`, whose tops are `tops` from the band's top (one more for
    /// the bottom of the last), and the table's bottom border when the band ends it at
    /// `bottom`.
    fn paint_borders(
        &self,
        (rows, tops, bottom): (Range<usize>, &[f64], f64),
        items: &mut Vec<DisplayItem>,
    ) {
        let x = |line: usize| self.lines.get(line).copied().unwrap_or(0.0);
        let horizontal = |line: usize, top: f64, items: &mut Vec<DisplayItem>| {
            let segments = self.horizontal.get(line).map_or(&[][..], Vec::as_slice);
            let thickest = segments
                .iter()
                .map(|segment| segment.border.width().0)
                .fold(0.0, f64::max);
            let y = top + thickest / 2.0;
            for segment in segments {
                let (from, to) = (x(segment.columns.start), x(segment.columns.end));
                border_line(&segment.border, (from, y), (to, y), items);
            }
        };
        for (offset, row) in rows.clone().enumerate() {
            let top = tops.get(offset).copied().unwrap_or(0.0);
            horizontal(row, top, items);
        }
        if rows.end == self.rows.len() {
            let top = tops.last().copied().unwrap_or(0.0);
            horizontal(rows.end, top, items);
        }
        for (offset, row) in rows.clone().enumerate() {
            let top = tops.get(offset).copied().unwrap_or(0.0);
            let below = if row + 1 == self.rows.len() {
                bottom
            } else {
                tops.get(offset + 1).copied().unwrap_or(bottom)
            };
            for (line, border) in self.vertical.get(row).into_iter().flatten() {
                border_line(border, (x(*line), top), (x(*line), below), items);
            }
        }
    }

    /// Resolves the borders between and around the cells. Each row's cells are taken
    /// in order, so the work and the room are in proportion to the cells, whatever the
    /// grid's width.
    fn resolve_borders(&mut self) {
        let table = self.table.props().borders();
        let count = self.rows.len();
        // The cells of row `row` that have columns, in order, with their place.
        let covering = |row: usize| -> Vec<(&Placed, &'d Cell)> {
            self.rows
                .get(row)
                .into_iter()
                .flatten()
                .filter(|placed| !placed.columns.is_empty())
                .filter_map(|placed| Some((placed, self.cell(row, placed.cell)?)))
                .collect()
        };
        let mut horizontal = Vec::with_capacity(count + 1);
        let mut previous = Vec::new();
        for line in 0..=count {
            let current = if line < count {
                covering(line)
            } else {
                Vec::new()
            };
            horizontal.push(horizontal_line(&previous, &current, &table));
            previous = current;
        }
        let vertical = (0..count)
            .map(|row| vertical_lines(&covering(row), &table))
            .collect();
        self.horizontal = horizontal;
        self.vertical = vertical;
    }
}

/// The borders along the grid line between the cells `above` and the cells `below`,
/// each in order: one segment wherever the cells on either side change, sides of the
/// same border joined.
fn horizontal_line(
    above: &[(&Placed, &Cell)],
    below: &[(&Placed, &Cell)],
    table: &TableBorders,
) -> Vec<Segment> {
    let mut cuts: Vec<usize> = above
        .iter()
        .chain(below)
        .flat_map(|(placed, _)| [placed.columns.start, placed.columns.end])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    // The cell of `cells` over column `column`, the cells before it passed by.
    let over = |cells: &[(&Placed, &Cell)], next: &mut usize, column: usize| -> Option<usize> {
        while cells
            .get(*next)
            .is_some_and(|(placed, _)| placed.columns.end <= column)
        {
            *next += 1;
        }
        cells
            .get(*next)
            .filter(|(placed, _)| placed.columns.contains(&column))
            .map(|_| *next)
    };
    let (mut next_above, mut next_below) = (0, 0);
    let mut segments: Vec<Segment> = Vec::new();
    for pair in cuts.windows(2) {
        let [from, to] = [pair.first(), pair.last()].map(|cut| cut.copied().unwrap_or(0));
        let upper = over(above, &mut next_above, from).and_then(|index| above.get(index));
        let lower = over(below, &mut next_below, from).and_then(|index| below.get(index));
        // No line inside a merged cell.
        if lower.is_some_and(|(placed, _)| placed.merge == Merge::Continues) {
            continue;
        }
        let upper_side = upper.map(|(_, cell)| {
            inherit(
                cell.borders().bottom(),
                lower.is_none(),
                table.bottom(),
                table.inside_horizontal(),
            )
        });
        let lower_side = lower.map(|(_, cell)| {
            inherit(
                cell.borders().top(),
                upper.is_none(),
                table.top(),
                table.inside_horizontal(),
            )
        });
        let Some(border) = conflict(upper_side, lower_side) else {
            continue;
        };
        // Columns side by side with the same border are one line.
        match segments.last_mut() {
            Some(last) if last.columns.end == from && last.border == border => {
                last.columns.end = to;
            }
            _ => segments.push(Segment {
                columns: from..to,
                border,
            }),
        }
    }
    segments
}

/// The borders beside the cells of a row, in order: on the grid line where each cell
/// starts and where each ends, one where two cells meet.
fn vertical_lines(cells: &[(&Placed, &Cell)], table: &TableBorders) -> Vec<(usize, Border)> {
    // Each grid line with the cell that ends there and the cell that starts there.
    let mut lines: Vec<(usize, Option<&Cell>, Option<&Cell>)> = Vec::new();
    for (placed, cell) in cells {
        match lines.last_mut() {
            Some(last) if last.0 == placed.columns.start => last.2 = Some(*cell),
            _ => lines.push((placed.columns.start, None, Some(*cell))),
        }
        lines.push((placed.columns.end, Some(*cell), None));
    }
    lines
        .into_iter()
        .filter_map(|(line, before, after)| {
            let end = before.map(|cell| {
                inherit(
                    cell.borders().end(),
                    after.is_none(),
                    table.end(),
                    table.inside_vertical(),
                )
            });
            let start = after.map(|cell| {
                inherit(
                    cell.borders().start(),
                    before.is_none(),
                    table.start(),
                    table.inside_vertical(),
                )
            });
            conflict(end, start).map(|border| (line, border))
        })
        .collect()
}

/// The labels of a table's paragraphs, taken in reading order.
struct Labels<'l> {
    list: &'l [Option<Label>],
    next: usize,
}

impl Labels<'_> {
    /// The next paragraph's label.
    fn take(&mut self) -> Option<&Label> {
        let label = self.list.get(self.next).and_then(Option::as_ref);
        self.next += 1;
        label
    }
}

/// Lays out `blocks` one under the other, `width` wide, on `background`, as a header
/// stacks its paragraphs: the space above the first, between each two, and below the
/// last, all in its height.
fn stack<'d>(
    blocks: &'d [Block],
    (width, background): (Pt, Option<Color>),
    labels: &mut Labels<'_>,
    location: &mut Location,
    context: &mut Context<'d, '_, '_>,
) -> Drawn {
    let document = context.document;
    let mut height = 0.0;
    let mut items = Vec::new();
    let mut above: Option<&Paragraph> = None;
    for (index, block) in blocks.iter().enumerate() {
        location.push(Step::Index(index));
        match block {
            Block::Paragraph(paragraph) => {
                let props = paragraph.props();
                height += match above {
                    Some(above) => paragraph::space_between(document, above.props(), props).0,
                    None => paragraph::space_before(document, props).0,
                };
                let label = labels.take();
                let layout = paragraph::lay_out_with(
                    document,
                    (paragraph, label),
                    width,
                    (&mut *context.fonts, &mut *context.shaper),
                    location,
                    context.values,
                );
                report_borders(paragraph, location, context.fonts);
                let origin = Point {
                    x: Pt(0.0),
                    y: Pt(height),
                };
                let mut painting = Painting {
                    document,
                    paragraph,
                    layout: &layout,
                    fonts: context.fonts,
                    location,
                    background,
                    leaders: context.leaders,
                };
                let lines = 0..layout.lines.len();
                painting.paint_shading(lines.clone(), origin, &mut items);
                painting.paint(lines, origin, &mut items);
                height += layout.height().0;
                above = Some(paragraph);
            }
            Block::Table(table) => {
                if let Some(above) = above.take() {
                    height += paragraph::space_after(document, above.props()).0;
                }
                let rest = labels.list.get(labels.next..).unwrap_or(&[]);
                let nested = Prepared::new(document, table, rest, width, location, context.fonts);
                let mut drawn = nested.whole(context);
                labels.next += nested.paragraphs();
                shift(&mut drawn.items, 0.0, height);
                height += drawn.height.0;
                items.extend(drawn.items);
            }
        }
        location.pop();
    }
    if let Some(last) = above {
        height += paragraph::space_after(document, last.props()).0;
    }
    Drawn {
        height: Pt(height),
        items,
    }
}

/// Passes over the labels of the paragraphs of `blocks`, nested tables' included.
fn skip_labels(blocks: &[Block], labels: &mut Labels<'_>) {
    labels.next += paragraphs_in(blocks);
}

/// How many paragraphs `blocks` hold, nested tables' included.
fn paragraphs_in(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(_) => 1,
            Block::Table(table) => table
                .rows()
                .iter()
                .flat_map(|row| row.cells())
                .map(|cell| paragraphs_in(cell.blocks()))
                .sum(),
        })
        .sum()
}

/// Where the labels of each row of `table` start among those of its paragraphs, and,
/// last, how many it has.
fn row_labels(table: &Table) -> Vec<usize> {
    let mut starts = Vec::with_capacity(table.rows().len() + 1);
    let mut count = 0;
    starts.push(count);
    for row in table.rows() {
        count += row
            .cells()
            .iter()
            .map(|cell| paragraphs_in(cell.blocks()))
            .sum::<usize>();
        starts.push(count);
    }
    starts
}

/// Whether `cell` hides its end mark and has no content: it takes no height
/// (`w:hideMark`, design 8.3).
fn hidden_mark(cell: &Cell) -> bool {
    cell.hide_mark()
        && cell.blocks().iter().all(|block| match block {
            Block::Paragraph(paragraph) => paragraph.inlines().is_empty(),
            Block::Table(_) => false,
        })
}

/// The cells of each row of `table` in their grid columns, with how they merge
/// vertically: a cell that continues a merge joins the cell above it whose columns are
/// the same, and one that has none above starts a merge of its own. Columns past the
/// grid are laid out with no width, so a cell's columns end at the grid's end: however
/// many columns cells span, a table takes room for its grid and its cells only.
fn placements(table: &Table, location: &Location, fonts: &mut Fonts<'_>) -> Vec<Vec<Placed>> {
    let grid = table.grid().len();
    let mut past = false;
    let mut rows: Vec<Vec<Placed>> = Vec::with_capacity(table.rows().len());
    for row in table.rows() {
        let mut column = usize::from(row.grid_before());
        let mut placed = Vec::with_capacity(row.cells().len());
        for (index, cell) in row.cells().iter().enumerate() {
            let span = usize::from(cell.grid_span().max(1));
            let merge = match cell.vertical_merge() {
                VerticalMerge::Continue => Merge::Continues,
                _ => Merge::Single,
            };
            past |= column.saturating_add(span) > grid;
            placed.push(Placed {
                cell: index,
                columns: column.min(grid)..column.saturating_add(span).min(grid),
                merge,
            });
            column = column.saturating_add(span);
        }
        rows.push(placed);
    }
    // The cell of `cells` whose columns are `columns`, if any: the cells of a row are in
    // the order of their columns.
    let same = |cells: &[Placed], columns: &Range<usize>| -> Option<usize> {
        let index = cells.partition_point(|placed| placed.columns.start < columns.start);
        cells
            .get(index..)
            .unwrap_or(&[])
            .iter()
            .take_while(|placed| placed.columns.start == columns.start)
            .position(|placed| placed.columns == *columns)
            .map(|offset| index + offset)
    };
    // A continuing cell without a cell above of the same columns starts a merge.
    for row in 1..rows.len() {
        let Some((before, after)) = rows.split_at_mut_checked(row) else {
            continue;
        };
        let (Some(above), Some(cells)) = (before.last(), after.first_mut()) else {
            continue;
        };
        for placed in cells.iter_mut() {
            if placed.merge == Merge::Continues && same(above, &placed.columns).is_none() {
                placed.merge = Merge::Single;
            }
        }
    }
    if let Some(cells) = rows.first_mut() {
        for placed in cells.iter_mut() {
            if placed.merge == Merge::Continues {
                placed.merge = Merge::Single;
            }
        }
    }
    // Count each merge's rows from its first cell.
    for row in (0..rows.len()).rev() {
        let Some(cells) = rows.get(row) else {
            continue;
        };
        let firsts: Vec<(usize, Range<usize>)> = cells
            .iter()
            .enumerate()
            .filter(|(_, placed)| placed.merge != Merge::Continues)
            .map(|(index, placed)| (index, placed.columns.clone()))
            .collect();
        for (index, columns) in firsts {
            let mut span = 1;
            while rows.get(row + span).is_some_and(|below| {
                same(below, &columns)
                    .and_then(|found| below.get(found))
                    .is_some_and(|placed| placed.merge == Merge::Continues)
            }) {
                span += 1;
            }
            if span > 1 {
                if let Some(placed) = rows.get_mut(row).and_then(|cells| cells.get_mut(index)) {
                    placed.merge = Merge::Spans(span);
                }
            }
        }
    }
    if past {
        fonts.report_once(
            ReportKind::ValueClamped,
            location,
            "cells past the columns of their table's grid are laid out with no width".to_owned(),
        );
    }
    rows
}

/// The bands of `rows`: rows that a merged cell ties together.
fn bands(rows: &[Vec<Placed>]) -> Vec<Range<usize>> {
    let mut bands = Vec::new();
    let mut start = 0;
    let mut end = 0;
    for (row, cells) in rows.iter().enumerate() {
        for placed in cells {
            if let Merge::Spans(count) = placed.merge {
                end = end.max(row + count);
            }
        }
        end = end.max(row + 1);
        if end == row + 1 {
            bands.push(start..end);
            start = end;
        }
    }
    bands
}

/// Reports what of `table` is laid out otherwise than Word does.
fn report_unsupported(table: &Table, location: &Location, fonts: &mut Fonts<'_>) {
    let props = table.props();
    let mut notes = Vec::new();
    if props.layout() == TableLayout::Autofit {
        notes.push(
            "tables that fit their columns to their content are laid out with the column \
             widths the document stores",
        );
    }
    if props.cell_spacing().0 > 0.0 {
        notes.push("spacing between table cells is laid out as none");
    }
    if props.floating() {
        notes.push("floating tables are laid out in the text where they are anchored");
    }
    if props.bidi_visual() {
        notes.push("right-to-left tables are laid out left to right");
    }
    let vertical = table
        .rows()
        .iter()
        .flat_map(|row| row.cells())
        .any(|cell| cell.text_direction() != TextDirection::LeftToRightTopToBottom);
    if vertical {
        notes.push("vertical text in table cells is laid out horizontally");
    }
    let exact = table
        .rows()
        .iter()
        .any(|row| matches!(row.height(), RowHeight::Exact(_)));
    if exact {
        notes.push("content taller than an exact row height is not cut off");
    }
    for note in notes {
        fonts.report_once(ReportKind::FeatureUnsupported, location, note.to_owned());
    }
}

/// Reports, once, that `paragraph`'s borders are not drawn.
pub(crate) fn report_borders(paragraph: &Paragraph, location: &Location, fonts: &mut Fonts<'_>) {
    let borders = paragraph.props().borders();
    let bordered = [
        borders.top(),
        borders.bottom(),
        borders.start(),
        borders.end(),
        borders.between(),
        borders.bar(),
    ]
    .iter()
    .any(Option::is_some);
    if bordered {
        fonts.report_once(
            ReportKind::FeatureUnsupported,
            location,
            "paragraph borders are not drawn yet".to_owned(),
        );
    }
}

/// A cell's side, its table's border where it inherits: `outer` at an edge of the
/// table, else `inside` (MS-OI29500 2.1.169 a: the producer merged the table style and
/// the exceptions into the cell's sides).
fn inherit(side: BorderSide, outer: bool, edge: BorderSide, inside: BorderSide) -> BorderSide {
    match side {
        BorderSide::Inherit if outer => edge,
        BorderSide::Inherit => inside,
        other => other,
    }
}

/// The border drawn where two cells' sides meet, or one side at an edge: `nil` on
/// either draws none, `none` gives way to the other, else the heavier, the earlier in
/// Word's precedence list, then the darker colour (MS-OI29500 2.1.169 a, Part 1
/// §17.4.66).
fn conflict(a: Option<BorderSide>, b: Option<BorderSide>) -> Option<Border> {
    let line = |side: Option<BorderSide>| match side {
        Some(BorderSide::Line(border)) if border.width().0 > 0.0 => Some(border),
        _ => None,
    };
    if matches!(a, Some(BorderSide::Nil)) || matches!(b, Some(BorderSide::Nil)) {
        return None;
    }
    match (line(a), line(b)) {
        (Some(a), Some(b)) => Some(if wins(&b, &a) { b } else { a }),
        (one, other) => one.or(other),
    }
}

/// Whether border `a` is drawn over border `b` where they meet.
fn wins(a: &Border, b: &Border) -> bool {
    let (weight_a, weight_b) = (weight(a), weight(b));
    if weight_a != weight_b {
        return weight_a > weight_b;
    }
    let (rank_a, rank_b) = (a.style() as usize, b.style() as usize);
    if rank_a != rank_b {
        return rank_a < rank_b;
    }
    let shade = |border: &Border| -> [u32; 3] {
        let color = match border.color() {
            ColorValue::Rgb(color) => color,
            ColorValue::Auto => Color::BLACK,
        };
        let (r, g, b) = (
            u32::from(color.red()),
            u32::from(color.green()),
            u32::from(color.blue()),
        );
        [r + b + 2 * g, b + 2 * g, g]
    };
    // The darker; the first in reading order, `b`, when they are the same.
    shade(a) < shade(b)
}

/// A border's weight: its width in eighths of a point times its style's number; dotted
/// and dashed lines weigh 1 (MS-OI29500 2.1.169 a).
fn weight(border: &Border) -> f64 {
    let number = match border.style() {
        BorderStyle::Dotted | BorderStyle::Dashed => return 1.0,
        BorderStyle::Single => 1.0,
        BorderStyle::Thick => 2.0,
        BorderStyle::Double => 3.0,
        BorderStyle::DotDash => 8.0,
        BorderStyle::DotDotDash => 9.0,
        BorderStyle::Triple => 10.0,
        BorderStyle::ThinThickSmallGap => 11.0,
        BorderStyle::ThickThinSmallGap => 12.0,
        BorderStyle::ThinThickThinSmallGap => 13.0,
        BorderStyle::ThinThickMediumGap => 14.0,
        BorderStyle::ThickThinMediumGap => 15.0,
        BorderStyle::ThinThickThinMediumGap => 16.0,
        BorderStyle::ThinThickLargeGap => 17.0,
        BorderStyle::ThickThinLargeGap => 18.0,
        BorderStyle::ThinThickThinLargeGap => 19.0,
        BorderStyle::Wave => 20.0,
        BorderStyle::DoubleWave => 21.0,
        BorderStyle::DashSmallGap => 22.0,
        BorderStyle::DashDotStroked => 23.0,
        BorderStyle::ThreeDEmboss => 24.0,
        BorderStyle::ThreeDEngrave => 25.0,
        BorderStyle::Outset => 26.0,
        BorderStyle::Inset => 27.0,
    };
    border.width().0 * 8.0 * number
}

/// Adds the line of `border` from `from` to `to`, centred on them. Lines of several
/// strokes are drawn as one, as thick as the border. Hypothesis until a Word reference
/// (`E-table-borders-geometry.docx`): where the strokes of a double border go.
fn border_line(border: &Border, from: (f64, f64), to: (f64, f64), items: &mut Vec<DisplayItem>) {
    let style = match border.style() {
        BorderStyle::Dotted => LineStyle::Dotted,
        BorderStyle::Dashed | BorderStyle::DashSmallGap => LineStyle::Dashed,
        BorderStyle::DotDash | BorderStyle::DashDotStroked => LineStyle::DotDash,
        BorderStyle::DotDotDash => LineStyle::DotDotDash,
        BorderStyle::Wave | BorderStyle::DoubleWave => LineStyle::Wave,
        _ => LineStyle::Solid,
    };
    let color = match border.color() {
        ColorValue::Rgb(color) => color,
        ColorValue::Auto => Color::BLACK,
    };
    if from == to {
        return;
    }
    items.push(DisplayItem::Line(LineItem {
        from: Point {
            x: Pt(from.0),
            y: Pt(from.1),
        },
        to: Point {
            x: Pt(to.0),
            y: Pt(to.1),
        },
        width: border.width(),
        style,
        color,
    }));
}

/// Moves `items` by `dx` across and `dy` down.
pub(crate) fn shift(items: &mut [DisplayItem], dx: f64, dy: f64) {
    let moved = |point: &mut Point| {
        point.x = Pt(point.x.0 + dx);
        point.y = Pt(point.y.0 + dy);
    };
    for item in items {
        match item {
            DisplayItem::Glyphs(run) => moved(&mut run.origin),
            DisplayItem::Rect { at, .. } => moved(at),
            DisplayItem::Line(line) => {
                moved(&mut line.from);
                moved(&mut line.to);
            }
            DisplayItem::Image(image) => moved(&mut image.at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        BreakClear, CellBorders, DocumentSettings, Inline, LevelAlignment, LineSpacing,
        ParagraphProps, Row, RunFonts, RunStyle, RunStyleId, Shading, Spacing, Suffix,
        TableBorders, TableProps,
    };
    use crate::testing::{book, minimal};

    /// A document in Word 2013's mode, or Word 2010's, with a run style of the test font
    /// at 10 pt.
    fn document(mode: CompatMode) -> (Document, RunStyleId) {
        let mut document = Document::new(DocumentSettings::default().with_compat_mode(mode));
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        (document, style)
    }

    /// A paragraph of `count` exact lines of 20 pt of `text`.
    fn lines(style: RunStyleId, count: usize, text: &str) -> Block {
        let mut inlines = Vec::new();
        for line in 0..count {
            if line > 0 {
                inlines.push(Inline::line_break(BreakClear::None, style));
            }
            inlines.push(Inline::text(text, style));
        }
        Paragraph::new(style)
            .with_props(
                ParagraphProps::default()
                    .with_spacing(Spacing::default().with_line(LineSpacing::Exact(Pt(20.0)))),
            )
            .with_inlines(inlines)
            .into()
    }

    fn cell(blocks: Vec<Block>) -> Cell {
        Cell::default().with_blocks(blocks)
    }

    fn row(cells: Vec<Cell>) -> Row {
        Row::default().with_cells(cells)
    }

    /// Single borders `width` thick on every side and between every cell.
    fn borders(width: f64) -> TableBorders {
        let line = BorderSide::Line(Border::default().with_width(Pt(width)));
        TableBorders::default()
            .with_top(line)
            .with_start(line)
            .with_bottom(line)
            .with_end(line)
            .with_inside_horizontal(line)
            .with_inside_vertical(line)
    }

    /// `table` of `document` prepared for a column `width` wide, with `labels`, and each of
    /// its bands laid out.
    fn laid(
        document: &Document,
        table: &Table,
        (width, labels): (f64, &[Option<Label>]),
    ) -> (Vec<f64>, Vec<Drawn>) {
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let mut leaders = LeaderBudget::default();
        let prepared = Prepared::new(
            document,
            table,
            labels,
            Pt(width),
            &Location::default(),
            &mut fonts,
        );
        let mut context = Context {
            document,
            fonts: &mut fonts,
            shaper: &mut shaper,
            leaders: &mut leaders,
            values: None,
        };
        let bands = (0..prepared.band_count())
            .map(|band| prepared.band(band, &mut context))
            .collect();
        (prepared.lines.clone(), bands)
    }

    /// The origins of the glyph runs of `drawn`, with their text, in drawing order.
    fn glyphs(drawn: &Drawn) -> Vec<(f64, f64, String)> {
        drawn
            .items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Glyphs(run) => Some((
                    run.origin.x.0,
                    run.origin.y.0,
                    run.clusters.iter().map(|c| c.text.as_str()).collect(),
                )),
                _ => None,
            })
            .collect()
    }

    /// How many grid lines, and how many border entries, `table` takes once prepared.
    fn prepared_size(document: &Document, table: &Table) -> (usize, usize) {
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let prepared = Prepared::new(
            document,
            table,
            &[],
            Pt(500.0),
            &Location::default(),
            &mut fonts,
        );
        let borders = prepared.horizontal.iter().map(Vec::len).sum::<usize>()
            + prepared.vertical.iter().map(Vec::len).sum::<usize>();
        (prepared.lines.len(), borders)
    }

    #[test]
    fn a_table_takes_room_by_its_cells_not_by_their_spans_or_its_grid() {
        // Cells past the grid, however many columns they span, take none of their own:
        // 1,000 cells of 65,535 columns each are not 65 million columns.
        let (document, style) = document(CompatMode::Word2013);
        let wide = Cell::default()
            .with_grid_span(u16::MAX)
            .with_blocks([Paragraph::new(style).into()]);
        let table = Table::default()
            .with_grid([Pt(100.0)])
            .with_props(TableProps::default().with_borders(borders(0.5)))
            .with_rows([row(vec![wide; 1000])]);
        let (lines, entries) = prepared_size(&document, &table);
        assert_eq!(lines, 2);
        assert!(entries <= 4, "{entries}");
        // A grid of 2,000 columns under 2,000 rows of one cell spanning them all: the
        // borders are drawn along each row's cells, not each grid column.
        let all = Cell::default()
            .with_grid_span(2000)
            .with_blocks([Paragraph::new(style).into()]);
        let table = Table::default()
            .with_grid(vec![Pt(0.25); 2000])
            .with_props(TableProps::default().with_borders(borders(0.5)))
            .with_rows(vec![row(vec![all]); 2000]);
        let (lines, entries) = prepared_size(&document, &table);
        assert_eq!(lines, 2001);
        assert!(entries <= 3 * 2001, "{entries}");
    }

    #[test]
    fn the_grid_sits_where_the_mode_and_the_alignment_put_it() {
        // Word's renders of E-table-indent-14 and -15: at the start, a table sits at its
        // indent, pulled out by its first cell's start margin in Word 2010's mode; centred,
        // its borders count in Word 2013's mode; an explicit width scales the grid.
        let table = |props: TableProps, style| {
            Table::default()
                .with_grid([Pt(50.0), Pt(100.0)])
                .with_props(props.with_borders(borders(0.5)))
                .with_rows(vec![row(vec![
                    cell(vec![lines(style, 1, "A")]),
                    cell(vec![lines(style, 1, "A")]),
                ])])
        };
        let fixed = |width: PreferredWidth| {
            TableProps::default()
                .with_layout(TableLayout::Fixed)
                .with_width(width)
                .with_indent(Pt(36.0))
        };
        let cases = [
            (
                CompatMode::Word2013,
                fixed(PreferredWidth::Auto),
                vec![36.0, 86.0, 186.0],
            ),
            (
                CompatMode::Word2010,
                fixed(PreferredWidth::Auto),
                vec![30.6, 80.6, 180.6],
            ),
            (
                CompatMode::Word2013,
                fixed(PreferredWidth::Points(Pt(300.0))),
                vec![36.0, 136.0, 336.0],
            ),
            // Half of the column less the indent: 182 pt.
            (
                CompatMode::Word2013,
                fixed(PreferredWidth::Percent(50.0)),
                vec![36.0, 36.0 + 50.0 * 182.0 / 150.0, 218.0],
            ),
            (
                CompatMode::Word2013,
                fixed(PreferredWidth::Auto).with_alignment(TableAlignment::Center),
                vec![124.75, 174.75, 274.75],
            ),
            (
                CompatMode::Word2010,
                fixed(PreferredWidth::Auto).with_alignment(TableAlignment::Center),
                vec![125.0, 175.0, 275.0],
            ),
            (
                CompatMode::Word2010,
                fixed(PreferredWidth::Auto).with_alignment(TableAlignment::End),
                vec![250.0, 300.0, 400.0],
            ),
        ];
        for (mode, props, expected) in cases {
            let (document, style) = document(mode);
            let (lines, bands) = laid(&document, &table(props, style), (400.0, &[]));
            let close = lines.len() == expected.len()
                && lines
                    .iter()
                    .zip(&expected)
                    .all(|(a, b)| (a - b).abs() < 1e-9);
            assert!(close, "{mode:?}: {lines:?} for {expected:?}");
            // The text of the first cell: at its margin, and in Word 2013's mode also
            // past half the border.
            let inset = if mode == CompatMode::Word2013 {
                0.25
            } else {
                0.0
            };
            let first = glyphs(&bands[0])[0].0;
            assert!(
                (first - (expected[0] + 5.4 + inset)).abs() < 1e-9,
                "{mode:?}: {first}"
            );
        }
    }

    #[test]
    fn borders_meet_as_word_resolves_them() {
        let line = |style: BorderStyle, width: f64, color: Color| {
            BorderSide::Line(
                Border::default()
                    .with_style(style)
                    .with_width(Pt(width))
                    .with_color(ColorValue::Rgb(color)),
            )
        };
        let black = Color::BLACK;
        let single = line(BorderStyle::Single, 1.0, black);
        let winner = |a: BorderSide, b: BorderSide| conflict(Some(a), Some(b));
        // Nil draws nothing; none gives way.
        assert_eq!(winner(single, BorderSide::Nil), None);
        assert_eq!(winner(BorderSide::Nil, single), None);
        let BorderSide::Line(one) = single else {
            panic!("a line")
        };
        assert_eq!(winner(BorderSide::None, single), Some(one));
        assert_eq!(conflict(None, Some(single)), Some(one));
        // The heavier: double 0.5 pt weighs 4 × 3, single 1 pt 8 × 1.
        let double = line(BorderStyle::Double, 0.5, black);
        assert_eq!(
            winner(single, double).map(|b| b.style()),
            Some(BorderStyle::Double)
        );
        // Dotted weighs 1 whatever its width.
        let dotted = line(BorderStyle::Dotted, 6.0, black);
        assert_eq!(
            winner(dotted, single).map(|b| b.style()),
            Some(BorderStyle::Single)
        );
        // Equal weights: the earlier in the precedence list (single 0.5 pt and thick
        // 0.25 pt both weigh 4).
        let thin = line(BorderStyle::Single, 0.5, black);
        let thick = line(BorderStyle::Thick, 0.25, black);
        assert_eq!(
            winner(thick, thin).map(|b| b.style()),
            Some(BorderStyle::Single)
        );
        // The same style: the darker, by R + B + 2G.
        let red = line(BorderStyle::Single, 1.0, Color::rgb(255, 0, 0));
        let green = line(BorderStyle::Single, 1.0, Color::rgb(0, 200, 0));
        let color = |side: Option<Border>| side.map(|b| b.color());
        assert_eq!(
            color(winner(green, red)),
            Some(ColorValue::Rgb(Color::rgb(255, 0, 0)))
        );
        // As bright by R + B + 2G: the darker by B + 2G.
        let blue = line(BorderStyle::Single, 1.0, Color::rgb(0, 0, 10));
        let red = line(BorderStyle::Single, 1.0, Color::rgb(10, 0, 0));
        assert_eq!(
            color(winner(blue, red)),
            Some(ColorValue::Rgb(Color::rgb(10, 0, 0)))
        );
        // The same colour: the first in reading order.
        let shadowed = BorderSide::Line(
            Border::default()
                .with_width(Pt(1.0))
                .with_color(ColorValue::Rgb(black))
                .with_shadow(true),
        );
        let plain = line(BorderStyle::Single, 1.0, black);
        assert_eq!(winner(shadowed, plain).map(|b| b.shadow()), Some(true));
        assert_eq!(winner(plain, shadowed).map(|b| b.shadow()), Some(false));
    }

    #[test]
    fn borders_take_room_above_rows_and_below_the_table_and_cells_inherit_them() {
        // Word's renders of E-space-before-top and E-table-indent: the top border's
        // width is above the cell's text, the bottom border's below the last row.
        let (document, style) = document(CompatMode::Word2013);
        let table = Table::default()
            .with_grid([Pt(100.0), Pt(100.0)])
            .with_props(TableProps::default().with_borders(borders(2.0)))
            .with_rows(vec![
                row(vec![
                    cell(vec![lines(style, 1, "A")]),
                    // A cell's own side wins over the table's where it is heavier.
                    cell(vec![lines(style, 1, "A")]).with_borders(
                        CellBorders::default()
                            .with_start(BorderSide::Line(Border::default().with_width(Pt(3.0)))),
                    ),
                ]),
                row(vec![
                    cell(vec![lines(style, 1, "A")]),
                    cell(vec![lines(style, 1, "A")]),
                ]),
            ]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        let heights: Vec<f64> = bands.iter().map(|band| band.height.0).collect();
        assert_eq!(heights, [22.0, 24.0]);
        let drawn_lines: Vec<(f64, f64, f64, f64, f64)> = bands[0]
            .items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Line(line) => Some((
                    line.from.x.0,
                    line.from.y.0,
                    line.to.x.0,
                    line.to.y.0,
                    line.width.0,
                )),
                _ => None,
            })
            .collect();
        // Above the row across both columns; beside it at the three grid lines, the
        // middle one 3 pt.
        assert_eq!(
            drawn_lines,
            [
                (0.0, 1.0, 200.0, 1.0, 2.0),
                (0.0, 0.0, 0.0, 22.0, 2.0),
                (100.0, 0.0, 100.0, 22.0, 3.0),
                (200.0, 0.0, 200.0, 22.0, 2.0),
            ]
        );
        // The text below the top border: 2 pt lower than without borders.
        let plain = Table::default()
            .with_grid([Pt(100.0)])
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "A")])])]);
        let (_, without) = laid(&document, &plain, (400.0, &[]));
        let (_, with_border, _) = glyphs(&bands[0])[0];
        let (_, no_border, _) = glyphs(&without[0])[0];
        assert!((with_border - no_border - 2.0).abs() < 1e-9);
    }

    /// The lines `drawn` draws: from, to and width.
    fn drawn_lines(drawn: &Drawn) -> Vec<(f64, f64, f64, f64, f64)> {
        drawn
            .items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Line(line) => Some((
                    line.from.x.0,
                    line.from.y.0,
                    line.to.x.0,
                    line.to.y.0,
                    line.width.0,
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn cells_take_the_table_s_edges_outside_and_its_inside_lines_between_them() {
        // Each of the table's six borders has a width of its own.
        let (document, style) = document(CompatMode::Word2010);
        let line = |width| BorderSide::Line(Border::default().with_width(Pt(width)));
        let borders = TableBorders::default()
            .with_top(line(1.0))
            .with_bottom(line(2.0))
            .with_start(line(3.0))
            .with_end(line(4.0))
            .with_inside_horizontal(line(0.5))
            .with_inside_vertical(line(0.75));
        let two = || {
            row(vec![
                cell(vec![lines(style, 1, "A")]),
                cell(vec![lines(style, 1, "A")]),
            ])
        };
        let table = Table::default()
            .with_grid([Pt(100.0), Pt(100.0)])
            .with_props(TableProps::default().with_borders(borders))
            .with_rows(vec![two(), two()]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        // Row 1: the top edge above it; the start edge, the inside line and the end edge
        // beside it.
        let widths = |band: usize| -> Vec<(bool, f64)> {
            drawn_lines(&bands[band])
                .into_iter()
                .map(|(x0, _, x1, _, width)| ((x0 - x1).abs() > 1e-9, width))
                .collect()
        };
        assert_eq!(
            widths(0),
            [(true, 1.0), (false, 3.0), (false, 0.75), (false, 4.0)]
        );
        // Row 2: the inside line above it and the bottom edge below it.
        assert_eq!(
            widths(1),
            [
                (true, 0.5),
                (true, 2.0),
                (false, 3.0),
                (false, 0.75),
                (false, 4.0)
            ]
        );
    }

    #[test]
    fn no_line_is_drawn_inside_a_merged_cell() {
        let (document, style) = document(CompatMode::Word2010);
        let merged = |merge| cell(vec![lines(style, 1, "A")]).with_vertical_merge(merge);
        let table = Table::default()
            .with_grid([Pt(100.0), Pt(100.0)])
            .with_props(TableProps::default().with_borders(borders(0.5)))
            .with_rows(vec![
                row(vec![
                    merged(VerticalMerge::Restart),
                    cell(vec![lines(style, 1, "A")]),
                ]),
                row(vec![
                    merged(VerticalMerge::Continue),
                    cell(vec![lines(style, 1, "A")]),
                ]),
            ]);
        let (grid, bands) = laid(&document, &table, (400.0, &[]));
        assert_eq!(bands.len(), 1);
        // The line between the rows runs along the second column only.
        let between: Vec<(f64, f64)> = drawn_lines(&bands[0])
            .into_iter()
            .filter(|(_, y0, _, y1, _)| (y0 - y1).abs() < 1e-9 && *y0 > 1.0 && *y0 < 40.0)
            .map(|(x0, _, x1, _, _)| (x0, x1))
            .collect();
        assert_eq!(between, [(grid[1], grid[2])]);
    }

    #[test]
    fn a_merged_cell_ties_its_rows_into_a_band_and_makes_the_last_taller() {
        let (document, style) = document(CompatMode::Word2013);
        let merged = |merge, count| cell(vec![lines(style, count, "A")]).with_vertical_merge(merge);
        let table = Table::default()
            .with_grid([Pt(100.0), Pt(100.0)])
            .with_rows(vec![
                row(vec![
                    merged(VerticalMerge::Restart, 3),
                    cell(vec![lines(style, 1, "A")]),
                ]),
                row(vec![
                    merged(VerticalMerge::Continue, 1),
                    cell(vec![lines(style, 1, "A")]),
                ]),
                row(vec![
                    cell(vec![lines(style, 1, "A")]),
                    cell(vec![lines(style, 1, "A")]),
                ]),
            ]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        // Rows of 20 pt hold a merged cell of 60 pt: the second row is 40 pt.
        let heights: Vec<f64> = bands.iter().map(|band| band.height.0).collect();
        assert_eq!(heights, [60.0, 20.0]);
        // The continuing cell draws nothing: three lines of the merged cell and one of
        // each other cell of the band.
        assert_eq!(glyphs(&bands[0]).len(), 5);
        // A continuing cell with no cell above it of its columns is a cell of its own.
        let lone = Table::default()
            .with_grid([Pt(100.0)])
            .with_rows(vec![row(vec![merged(VerticalMerge::Continue, 1)])]);
        let (_, bands) = laid(&document, &lone, (400.0, &[]));
        assert_eq!(glyphs(&bands[0]).len(), 1);
    }

    #[test]
    fn rows_are_as_high_as_their_rule_says_and_cells_align_in_them() {
        let (document, style) = document(CompatMode::Word2013);
        let margins = CellMargins::default().with_bottom(Pt(3.0));
        let shading = Shading::default().with_fill(ColorValue::Rgb(Color::rgb(0, 0, 200)));
        let table = Table::default()
            .with_grid([Pt(100.0), Pt(100.0)])
            .with_rows(vec![
                row(vec![
                    cell(vec![lines(style, 1, "A")])
                        .with_vertical_align(CellVerticalAlign::Center)
                        .with_shading(Some(shading)),
                    cell(vec![lines(style, 1, "A")]).with_vertical_align(CellVerticalAlign::Bottom),
                ])
                .with_height(RowHeight::AtLeast(Pt(60.0))),
                // Exact: the height and the row's largest bottom margin (MS-OI29500
                // 2.1.180 d), whatever the content.
                row(vec![
                    cell(vec![lines(style, 3, "A")]).with_margins(Some(margins)),
                ])
                .with_height(RowHeight::Exact(Pt(10.0))),
                row(vec![cell(vec![lines(style, 2, "A")])]),
            ]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        let heights: Vec<f64> = bands.iter().map(|band| band.height.0).collect();
        assert_eq!(heights, [60.0, 13.0, 40.0]);
        let tops: Vec<f64> = glyphs(&bands[0]).iter().map(|(_, y, _)| *y).collect();
        // Centred 20 pt down, at the bottom 40 pt down: the baselines differ by as much.
        assert!((tops[1] - tops[0] - 20.0).abs() < 1e-9, "{tops:?}");
        let fill = bands[0].items.iter().find_map(|item| match item {
            DisplayItem::Rect {
                at, width, height, ..
            } => Some((at.x.0, at.y.0, width.0, height.0)),
            _ => None,
        });
        assert_eq!(fill, Some((0.0, 0.0, 100.0, 60.0)));
    }

    #[test]
    fn labels_are_taken_in_reading_order_through_merged_and_nested_cells() {
        let (document, style) = document(CompatMode::Word2013);
        let label = |text: &str| {
            Some(Label {
                text: text.into(),
                style,
                suffix: Suffix::Nothing,
                alignment: LevelAlignment::Start,
            })
        };
        let nested = Table::default()
            .with_grid([Pt(80.0)])
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "B")])])]);
        let table = Table::default()
            .with_grid([Pt(100.0), Pt(100.0)])
            .with_rows(vec![
                row(vec![
                    cell(vec![lines(style, 1, "B")]).with_vertical_merge(VerticalMerge::Restart),
                    cell(vec![nested.into(), lines(style, 1, "B")]),
                ]),
                row(vec![
                    cell(vec![lines(style, 1, "B")]).with_vertical_merge(VerticalMerge::Continue),
                    cell(vec![lines(style, 1, "B")]),
                ]),
            ]);
        let labels = [label("AA"), label("AAA"), None, label("AAAA"), label("A")];
        let (_, bands) = laid(&document, &table, (400.0, &labels));
        let text: String = glyphs(&bands[0])
            .into_iter()
            .map(|(_, _, text)| text)
            .collect();
        // The merged cell, the nested table and the paragraph after it, then the last
        // cell; the continuing cell's label is passed over.
        assert_eq!(text, "AABAAABBAB");
        assert_eq!(row_labels(&table), [0, 3, 5]);
    }

    #[test]
    fn an_empty_cell_hiding_its_mark_takes_no_height() {
        let (document, style) = document(CompatMode::Word2013);
        let empty = Cell::default()
            .with_hide_mark(true)
            .with_blocks([Paragraph::new(style).into()]);
        let table = Table::default().with_grid([Pt(100.0)]).with_rows(vec![
            row(vec![empty.clone()]),
            row(vec![empty.with_hide_mark(false)]),
        ]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        let heights: Vec<f64> = bands.iter().map(|band| band.height.0).collect();
        assert_eq!(heights[0], 0.0);
        assert!(heights[1] > 0.0);
    }
}
