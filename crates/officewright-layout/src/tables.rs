//! Tables in a column: the geometry of their grid, their cells laid out as stacks of
//! paragraphs and nested tables, their borders and their shading (design §8).
//!
//! Rows tied together by a vertically merged cell form a band, which a page takes whole.
//! A band of one row is cut between the lines of its cells where a page ends inside it,
//! and nested tables in them between their bands, as Word cuts them; header rows start
//! every page the table goes on to. A table of fixed layout takes the grid's column
//! widths, scaled to an explicit table width; one that fits its columns to their content
//! measures its cells and fits them (`autofit`).

use std::collections::HashMap;
use std::ops::Range;

use officewright_core::{Color, Pt};

use crate::autofit::{self, Column, Target};
use crate::display::{self, DisplayItem, LeaderBudget, LineItem, LineStyle, Painting, Point};
use crate::fonts::Fonts;
use crate::ir::{
    Block, Border, BorderSide, BorderStyle, Cell, CellMargins, CellVerticalAlign, ColorValue,
    CompatFlag, CompatMode, Document, Paragraph, PreferredWidth, Row, RowHeight, Table,
    TableAlignment, TableBorders, TableLayout, TextDirection, VerticalMerge,
};
use crate::itemize::{FieldValues, Item};
use crate::lists::Label;
use crate::paragraph::{self, ParagraphLayout};
use crate::report::{Location, ReportKind, Step};
use crate::shape::Shaper;

/// How far a piece of a table may reach past the room it is given and still fit: what
/// sums of positions in points lose.
const EPSILON: f64 = 1e-6;

/// The widest a grid column is laid out: Word's widest page, 22 inches. A wider one is
/// laid out that wide, and reported.
const MAX_COLUMN: f64 = 1584.0;

/// The widest a table is laid out, as a percentage of its column or in points: as far as
/// a PDF places anything from a page's corner.
const MAX_TABLE: f64 = 32767.0;

/// What lays out the content of cells: the document, the fonts and the shaper, the
/// leader glyphs that may still be drawn, the values of the fields the layout computes,
/// for the page the table is laid out for, and the widths of the nested tables measured
/// with those values.
pub(crate) struct Context<'d, 'f, 'b> {
    pub(crate) document: &'d Document,
    pub(crate) fonts: &'f mut Fonts<'b>,
    pub(crate) shaper: &'f mut Shaper,
    pub(crate) leaders: &'f mut LeaderBudget,
    pub(crate) values: Option<&'f FieldValues>,
    pub(crate) measured: &'f mut Measured,
}

/// The narrowest and the widest each nested table measured so far is laid out
/// (`Prepared::widths`), by the table's address in the document: a table that fits its
/// columns to their content measures the tables nested in its cells, and each of those
/// is measured again when it is laid out, so that without it a table nested `n` deep would
/// be measured `n` times, and its content with it.
pub(crate) type Measured = HashMap<usize, (f64, f64)>;

/// What a stack of blocks, or a band of a table, draws: its height and its display
/// items, from its top left corner.
#[derive(Clone, Debug, Default)]
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
    /// Borders are centred on them, and a cell's text runs from its start line plus its
    /// start margin to its end line less its end margin (Word's renders of
    /// `E-table-autofit-*`, in both modes).
    lines: Vec<f64>,
    /// Whether the table's outer borders lie inside its indent and its column, as in
    /// Word 2013's mode, or its first cell's text lines up with the paragraphs, as in
    /// earlier modes.
    borders_inside: bool,
    /// What each grid column needs, for a table that fits its columns to its content.
    needs: Vec<Column>,
    /// Whether columns whose minima do not fit keep them (`growAutofit` in Word 2010's
    /// mode and earlier).
    grow: bool,
    rows: Vec<Vec<Placed>>,
    bands: Vec<Range<usize>>,
    /// The borders drawn along the grid line above each row, and last below the last
    /// row: in order, each over the columns it runs along.
    horizontal: Vec<Vec<Segment>>,
    /// The room the borders along each of those grid lines take: the thickest of them.
    thickness: Vec<f64>,
    /// The borders drawn beside each row: in order, each with the grid line it is on.
    vertical: Vec<Vec<(usize, Border)>>,
    /// The labels of the table's paragraphs in reading order, and where each row's start.
    labels: &'l [Option<Label>],
    row_labels: Vec<usize>,
}

impl<'d, 'l> Prepared<'d, 'l> {
    /// Prepares `table` of the context's document for a column `width` wide; `labels` are
    /// the list labels of its paragraphs in reading order, `location` its place, for the
    /// report. A table that fits its columns to their content measures its cells' content
    /// with the context's fonts and field values.
    pub(crate) fn new(
        table: &'d Table,
        labels: &'l [Option<Label>],
        width: Pt,
        location: &Location,
        context: &mut Context<'d, '_, '_>,
    ) -> Self {
        let settings = context.document.settings();
        report_unsupported(table, location, context.fonts);
        let rows = placements(table, location, context.fonts);
        let columns = table.grid().len();
        let bands = bands(&rows);
        let mut prepared = Self {
            table,
            location: location.clone(),
            lines: Vec::new(),
            borders_inside: settings.compat_mode() >= CompatMode::Word2013,
            needs: Vec::new(),
            // `growAutofit` is one of Word 2010's options, which Word 2013's mode leaves
            // out.
            grow: settings.compat_mode() < CompatMode::Word2013
                && settings.has_compat_flag(&CompatFlag::GrowAutofit),
            horizontal: Vec::new(),
            thickness: Vec::new(),
            vertical: Vec::new(),
            row_labels: row_labels(table),
            rows,
            bands,
            labels,
        };
        prepared.resolve_borders();
        if table.props().layout() == TableLayout::Autofit {
            prepared.needs = prepared.needs(context);
        }
        prepared.lines = prepared.grid_lines(width.0, columns, context.fonts);
        prepared
    }

    /// What each grid column needs (design 8.2): the minimum and the maximum width of the
    /// content of the cells that lie in it alone, with their start and end margins, and
    /// their preferred widths, the largest of each (a hypothesis for preferred widths that
    /// differ from row to row); then what the cells that span several columns need beyond
    /// those ([`autofit::Needs::spread`]), and so the room a row leaves before or after
    /// its cells (`w:wBefore`, `w:wAfter`) when its width is in points. Hypotheses until a
    /// Word render: a cell merged into the one above, or one that hides its mark and has
    /// no content, needs nothing; a column no cell and no such room lies in keeps the
    /// grid's width.
    fn needs(&self, context: &mut Context<'d, '_, '_>) -> Vec<Column> {
        let grid = self.table.grid();
        let mut columns = vec![Column::default(); grid.len()];
        let mut spans = Vec::new();
        // Where the columns that rows cover start and end, for which columns none does.
        let mut covered = vec![0_i64; grid.len() + 1];
        let mut cover = |range: &Range<usize>| {
            if range.start < range.end && range.end <= grid.len() {
                if let Some(at) = covered.get_mut(range.start) {
                    *at += 1;
                }
                if let Some(at) = covered.get_mut(range.end) {
                    *at -= 1;
                }
            }
        };
        let mut labels = Labels {
            list: self.labels,
            next: 0,
        };
        for (row, placed_row) in self.rows.iter().enumerate() {
            if let Some(ir_row) = self.table.rows().get(row) {
                let before = usize::from(ir_row.grid_before()).min(grid.len());
                let end = placed_row
                    .last()
                    .map_or(before, |placed| placed.columns.end);
                let after = end
                    .saturating_add(usize::from(ir_row.grid_after()))
                    .min(grid.len());
                for (range, width) in [
                    (0..before, ir_row.width_before()),
                    (end..after, ir_row.width_after()),
                ] {
                    if let PreferredWidth::Points(points) = width {
                        cover(&range);
                        spans.push((
                            range,
                            Column {
                                preferred: Some(points.0.clamp(0.0, MAX_TABLE)),
                                ..Column::default()
                            },
                        ));
                    }
                }
            }
            for placed in placed_row {
                cover(&placed.columns);
                let Some(cell) = self.cell(row, placed.cell) else {
                    continue;
                };
                if placed.merge == Merge::Continues || hidden_mark(cell) {
                    skip_labels(cell.blocks(), &mut labels);
                    continue;
                }
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
                let (min, max) = block_widths(cell.blocks(), &mut labels, &mut location, context);
                let margins = self.margins(cell);
                let sides = margins.start().0 + margins.end().0;
                // A cell that keeps its content on one line (`w:noWrap`) needs its widest
                // line where the table fits its columns, when its preferred width is auto or
                // in percent: one in points ignores it, as [MS-DOC] CellRangeNoWrap says
                // (ECMA-376 Part 1, §17.4.30, has such a cell at least as wide as its width
                // instead). A hypothesis for Word until a render.
                let keeps_one_line =
                    cell.no_wrap() && !matches!(cell.width(), PreferredWidth::Points(_));
                let min = if keeps_one_line { max } else { min };
                let need = Column {
                    min: min + sides,
                    max: max + sides,
                    preferred: match cell.width() {
                        PreferredWidth::Points(points) => Some(points.0.clamp(0.0, MAX_TABLE)),
                        _ => None,
                    },
                    percent: match cell.width() {
                        PreferredWidth::Percent(percent) => Some(percent.clamp(0.0, 100.0)),
                        _ => None,
                    },
                };
                if placed.columns.len() == 1 {
                    if let Some(column) = columns.get_mut(placed.columns.start) {
                        *column = Column {
                            min: column.min.max(need.min),
                            max: column.max.max(need.max),
                            preferred: larger(column.preferred, need.preferred),
                            percent: larger(column.percent, need.percent),
                        };
                    }
                } else if !placed.columns.is_empty() {
                    spans.push((placed.columns.clone(), need));
                }
            }
        }
        let mut depth = 0;
        for ((column, width), change) in columns.iter_mut().zip(grid).zip(&covered) {
            depth += change;
            if depth == 0 {
                column.preferred = Some(width.0.clamp(0.0, MAX_COLUMN));
            }
        }
        let mut needs = autofit::Needs::new(columns);
        for (range, need) in spans {
            needs.spread(range, need);
        }
        needs.into_columns()
    }

    /// The narrowest and the widest the table is laid out, from the outer edge of its
    /// start border to that of its end border in Word 2013's mode: what a cell that holds
    /// it needs for it. Word's render of case 7 of `E-table-autofit-15`: the outer column
    /// is the inner grid, the inner borders and the outer cell's margins. In earlier
    /// modes the grid alone, a hypothesis. A table that fits its columns to their content
    /// is measured as it is laid out: one with a width in points takes that width,
    /// whatever the room, or its minima where `growAutofit` keeps them wider, and one with
    /// shares of a width in percent as wide as its shares need ([`autofit::natural`]);
    /// hypotheses, as no render nests one.
    fn widths(&self) -> (f64, f64) {
        let outer = self.outer();
        let (min, max) = if self.table.props().layout() == TableLayout::Autofit {
            match self.table.props().width() {
                PreferredWidth::Points(points) => {
                    // As `grid_lines` fits it, `growAutofit` included.
                    let target = Target::Exact(points.0.clamp(0.0, MAX_TABLE));
                    let width = autofit::widths(&self.needs, target, self.grow).iter().sum();
                    (width, width)
                }
                _ => autofit::natural(&self.needs, MAX_TABLE),
            }
        } else {
            let total = self.lines.last().copied().unwrap_or(0.0)
                - self.lines.first().copied().unwrap_or(0.0);
            (total, total)
        };
        (min + outer, max + outer)
    }

    /// The room the table's outer borders take beyond its grid in Word 2013's mode: half
    /// of each, at its start and at its end, in its first row.
    fn outer(&self) -> f64 {
        if !self.borders_inside {
            return 0.0;
        }
        let columns = self.table.grid().len();
        let edge = |line: usize| {
            self.vertical_border(0, line)
                .map_or(0.0, |border| border.width().0)
        };
        (edge(0) + edge(columns)) / 2.0
    }

    /// How many bands the table has.
    pub(crate) fn band_count(&self) -> usize {
        self.bands.len()
    }

    /// How many paragraphs the table has, nested tables' included: the labels it takes.
    pub(crate) fn paragraphs(&self) -> usize {
        self.row_labels.last().copied().unwrap_or(0)
    }

    /// How many bands at the top of the table hold header rows only: those Word repeats
    /// at the top of every page the table goes on to (`w:tblHeader`).
    pub(crate) fn header_bands(&self) -> usize {
        self.bands
            .iter()
            .take_while(|rows| {
                (rows.start..rows.end)
                    .all(|row| self.table.rows().get(row).is_some_and(Row::header))
            })
            .count()
    }

    /// Whether a row of band `band` stays on one page (`w:cantSplit`).
    pub(crate) fn cant_split(&self, band: usize) -> bool {
        self.bands.get(band).is_some_and(|rows| {
            rows.clone()
                .any(|row| self.table.rows().get(row).is_some_and(Row::cant_split))
        })
    }

    /// Whether band `band` is rows that a vertically merged cell ties together.
    pub(crate) fn ties_rows(&self, band: usize) -> bool {
        self.bands.get(band).is_some_and(|rows| rows.len() > 1)
    }

    /// The header rows: those of the header bands.
    pub(crate) fn header_rows(&self) -> &'d [Row] {
        let end = self
            .bands
            .get(self.header_bands().wrapping_sub(1))
            .map_or(0, |rows| rows.end);
        self.table.rows().get(..end).unwrap_or(&[])
    }

    /// Lays out the header bands whole, one under the other: each band, and the height
    /// they take with the border below the last.
    pub(crate) fn header(&self, context: &mut Context<'d, '_, '_>) -> (Vec<Drawn>, f64) {
        let count = self.header_bands();
        let bands: Vec<Drawn> = (0..count).map(|band| self.band(band, context)).collect();
        let below = self
            .bands
            .get(count.wrapping_sub(1))
            .map_or(0.0, |rows| self.thickness(rows.end));
        let height = bands.iter().map(|band| band.height.0).sum::<f64>() + below;
        (bands, height)
    }

    /// The most pieces the table may be cut into, besides its header rows repeated: one
    /// for each band, and one for each unit of the content of its cells, since every piece
    /// but a band's last takes at least one.
    #[cfg(feature = "fuzzing")]
    pub(crate) fn pieces_bound(&self, context: &mut Context<'d, '_, '_>) -> usize {
        (0..self.bands.len())
            .map(|band| {
                let laid = self.lay_out(band, context);
                let units: usize = laid
                    .cells
                    .iter()
                    .flatten()
                    .flatten()
                    .map(|cell| cell.content.units.len())
                    .sum();
                units + 1
            })
            .sum()
    }

    /// Lays out band `band` whole: its height, with the borders above its first row and,
    /// for the table's last band, below its last; and what it draws, from the band's top
    /// at the column's start edge.
    pub(crate) fn band(&self, band: usize, context: &mut Context<'d, '_, '_>) -> Drawn {
        let mut laid = self.lay_out(band, context);
        self.piece(&mut laid, None, (true, false), context)
            .map_or_else(Drawn::default, |piece| piece.drawn)
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

    /// Lays out the content of the cells of band `band`, each at its width, for
    /// [`piece`](Self::piece) to put on pages.
    pub(crate) fn lay_out(&self, band: usize, context: &mut Context<'d, '_, '_>) -> LaidBand<'d> {
        let rows = self.bands.get(band).cloned().unwrap_or(0..0);
        let mut labels = Labels {
            list: self.labels,
            next: self.row_labels.get(rows.start).copied().unwrap_or(0),
        };
        let mut cells = Vec::new();
        for row in rows.clone() {
            let mut laid = Vec::new();
            for placed in self.rows.get(row).into_iter().flatten() {
                let cell = self.cell(row, placed.cell);
                laid.push(match (cell, placed.merge) {
                    (Some(cell), Merge::Single | Merge::Spans(_)) => {
                        let margins = self.margins(cell);
                        let width = self.content_width(placed, &margins);
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
                        let content = if hidden_mark(cell) {
                            skip_labels(cell.blocks(), &mut labels);
                            Stacked::default()
                        } else {
                            stack(
                                cell.blocks(),
                                (width, background),
                                &mut labels,
                                &mut location,
                                context,
                            )
                        };
                        Some(LaidCell {
                            content,
                            margins,
                            next: 0,
                        })
                    }
                    (Some(cell), Merge::Continues) => {
                        // A cell merged into the one above draws nothing of its own, but
                        // its paragraphs keep their labels. Hypothesis until a Word render
                        // (`E-table-merged-content.docx`): what Word does with content in
                        // such a cell, which Word itself never writes; reported.
                        if !hidden_content(cell) {
                            context.fonts.report_once(
                                ReportKind::FeatureUnsupported,
                                &self.location,
                                "the content of a cell merged into the cell above it is not \
                                 drawn"
                                    .to_owned(),
                            );
                        }
                        skip_labels(cell.blocks(), &mut labels);
                        None
                    }
                    (None, _) => None,
                });
            }
            cells.push(laid);
        }
        LaidBand {
            rows,
            cells,
            put: None,
        }
    }

    /// Puts on a page what of `laid` fits in `room` below its top, the border below it
    /// included, which Word leaves room for (Word's renders of `E-table-header-repeat.docx`
    /// and `E-table-row-split.docx`): the rest of the band whole, or, where `split`
    /// allows it and the band is a row of its own of no exact height, the lines of each
    /// cell that fit, each cell cut on its own. `None` when nothing fits, unless `fresh`:
    /// on a page that holds nothing else, a band that does not fit is put all the same,
    /// whole or, where it may split, cut after at least one line of each cell. `room`
    /// `None` puts the band whole.
    ///
    /// A piece that ends its band has the border below it only where the band ends the
    /// table; one that does not is closed by the line its row's bottom border draws, and
    /// the rest goes on below that row's top border (`E-table-row-split.docx`, where every
    /// border is alike: hypotheses for which borders Word draws there). Hypotheses until
    /// a Word render: a cell's margins are kept above and below each piece of it, the
    /// pieces of a split row are aligned at their top whatever the cell's vertical
    /// alignment, its height rule's least height is filled by its last piece as far as
    /// its page leaves room, and a row of an exact height is never split.
    pub(crate) fn piece(
        &self,
        laid: &mut LaidBand<'d>,
        room: Option<f64>,
        (fresh, split): (bool, bool),
        context: &mut Context<'d, '_, '_>,
    ) -> Option<Piece> {
        let rows = laid.rows.clone();
        let below = self.thickness(rows.end);
        let row_height = self.table.rows().get(rows.start).map(Row::height);
        if laid.put.is_none() {
            let heights = self.heights(laid, context);
            let whole: f64 = rows
                .clone()
                .zip(&heights)
                .map(|(row, height)| self.thickness(row) + height)
                .sum();
            let fits = room.is_none_or(|room| whole + below <= room + EPSILON);
            let splits =
                split && rows.len() == 1 && !matches!(row_height, Some(RowHeight::Exact(_)));
            if fits || !splits {
                if !fits && !fresh {
                    return None;
                }
                return Some(Piece {
                    drawn: self.paint_band(laid, &heights, context),
                    done: true,
                });
            }
        }
        // A row of its own, cut where the lines of its cells allow.
        let room = room.unwrap_or(f64::INFINITY);
        let above = self.thickness(rows.start);
        let cells = laid.cells.first_mut()?;
        let mut takes: Vec<usize> = Vec::with_capacity(cells.len());
        let mut fallback: Vec<usize> = Vec::with_capacity(cells.len());
        for cell in cells.iter().flatten() {
            let margins = cell.margins.top().0 + cell.margins.bottom().0;
            let (take, any) = cell.content.fit(cell.next, room - above - below - margins);
            takes.push(take);
            fallback.push(any);
        }
        let left = |cell: &LaidCell<'_>| {
            cell.content.units.len() - cell.next.min(cell.content.units.len())
        };
        let progress = takes.iter().any(|&take| take > 0);
        if !progress {
            if !fresh {
                return None;
            }
            // Nothing moves it further: at least one line of each cell.
            for ((take, any), cell) in takes.iter_mut().zip(&fallback).zip(cells.iter().flatten()) {
                *take = (*any).max(1).min(left(cell));
            }
        }
        let done = cells
            .iter()
            .flatten()
            .zip(&takes)
            .all(|(cell, take)| *take == left(cell));
        if done && laid.put.is_none() {
            // Only its least height keeps it from the room left.
            if !fresh {
                return None;
            }
            let heights = self.heights(laid, context);
            return Some(Piece {
                drawn: self.paint_band(laid, &heights, context),
                done: true,
            });
        }
        let mut height: f64 = 0.0;
        let mut contents = Vec::with_capacity(takes.len());
        for (cell, take) in cells.iter_mut().flatten().zip(&takes) {
            let units = cell.next..cell.next + take;
            cell.next = units.end;
            let drawn = cell.content.paint(units, context);
            height = height.max(cell.margins.top().0 + drawn.height.0 + cell.margins.bottom().0);
            contents.push(drawn);
        }
        let put = laid.put.unwrap_or(0.0);
        if done {
            if let Some(RowHeight::AtLeast(least)) = row_height {
                height = height.max((least.0 - put).min(room - above - below));
            }
        }
        laid.put = Some(put + height);
        let closed = !done || rows.end == self.rows.len();
        let bottom = above + height + if closed { below } else { 0.0 };
        let mut items = Vec::new();
        self.paint_table_shading((above + height, bottom), closed, &mut items, context.fonts);
        let row = rows.start;
        let placed_cells = self.rows.get(row).into_iter().flatten();
        let laid_cells = laid.cells.first().into_iter().flatten();
        let mut contents = contents.into_iter();
        for (placed, cell) in placed_cells.zip(laid_cells) {
            let Some(cell) = cell else {
                continue;
            };
            let Some(mut drawn) = contents.next() else {
                break;
            };
            let (left, right) = self.cell_edges(placed);
            self.paint_cell_shading(
                row,
                placed,
                (left, right),
                (0.0, above + height),
                &mut items,
                context.fonts,
            );
            shift(
                &mut drawn.items,
                left + cell.margins.start().0,
                above + cell.margins.top().0,
            );
            items.extend(drawn.items);
        }
        self.paint_borders((rows, &[0.0, above + height], bottom, closed), &mut items);
        Some(Piece {
            drawn: Drawn {
                height: Pt(bottom),
                items,
            },
            done,
        })
    }

    /// The line below band `band`, which closes it where a page ends under it: its
    /// height, the thickest border along it, and what it draws from its top, the vertical
    /// borders of the band's last row run through it.
    pub(crate) fn closing(&self, band: usize, fonts: &mut Fonts<'_>) -> Drawn {
        let Some(rows) = self.bands.get(band).cloned() else {
            return Drawn::default();
        };
        let height = self.thickness(rows.end);
        let mut items = Vec::new();
        self.paint_table_shading((0.0, height), true, &mut items, fonts);
        self.paint_horizontal(rows.end, 0.0, &mut items);
        self.paint_vertical(rows.end.saturating_sub(1), (0.0, height), &mut items);
        Drawn {
            height: Pt(height),
            items,
        }
    }

    /// The height of each row of `laid` laid out whole, below its top border: its tallest
    /// cell of its own, as its height rule says, and a merged cell taller than its rows
    /// making the last of them taller.
    fn heights(&self, laid: &LaidBand<'d>, context: &mut Context<'d, '_, '_>) -> Vec<f64> {
        let rows = laid.rows.clone();
        let mut heights: Vec<f64> = Vec::new();
        for (offset, row) in rows.clone().enumerate() {
            let content = self
                .rows
                .get(row)
                .into_iter()
                .flatten()
                .zip(laid.cells.get(offset).into_iter().flatten())
                .filter(|(placed, _)| placed.merge == Merge::Single)
                .filter_map(|(_, laid)| laid.as_ref())
                .map(LaidCell::height)
                .fold(0.0, f64::max);
            let height = match self.table.rows().get(row).map(Row::height) {
                Some(RowHeight::AtLeast(least)) => content.max(least.0),
                // The height runs from the top of the row's top border to the top of the
                // next row's (Word's render of `E-table-row-exact-height.docx`), and Word
                // adds the row's largest bottom margin (MS-OI29500 2.1.180 d).
                Some(RowHeight::Exact(exact)) => {
                    let height =
                        (exact.0 - self.thickness(row)).max(0.0) + self.largest_bottom_margin(row);
                    if content > height + 1e-6 {
                        context.fonts.report_once(
                            ReportKind::FeatureUnsupported,
                            &self.location,
                            "content taller than an exact row height is not cut off".to_owned(),
                        );
                    }
                    height
                }
                _ => content,
            };
            heights.push(height);
        }
        // A merged cell taller than its rows makes the last of them taller (design 8.3),
        // of an exact height too: a hypothesis until a Word render
        // (`E-table-merged-exact.docx`).
        for (offset, row) in rows.clone().enumerate() {
            let row_cells = self.rows.get(row).into_iter().flatten();
            for (placed, laid) in row_cells.zip(laid.cells.get(offset).into_iter().flatten()) {
                let (Merge::Spans(count), Some(laid)) = (placed.merge, laid) else {
                    continue;
                };
                let needed = laid.height();
                let spanned = offset..(offset + count).min(heights.len());
                let available: f64 = heights
                    .get(spanned.clone())
                    .unwrap_or(&[])
                    .iter()
                    .sum::<f64>()
                    + spanned
                        .clone()
                        .skip(1)
                        .map(|inner| self.thickness(rows.start + inner))
                        .sum::<f64>();
                if needed > available {
                    if let Some(height) = spanned.last().and_then(|last| heights.get_mut(last)) {
                        *height += needed - available;
                    }
                }
            }
        }
        heights
    }

    /// Paints `laid` whole, its rows `heights` high: the borders above its rows and, for
    /// the table's last band, below its last; each cell's content aligned in the room its
    /// rows leave it.
    fn paint_band(
        &self,
        laid: &mut LaidBand<'d>,
        heights: &[f64],
        context: &mut Context<'d, '_, '_>,
    ) -> Drawn {
        let rows = laid.rows.clone();
        let last = rows.end == self.rows.len();
        // The top of each row, from the band's top, and the band's bottom.
        let mut tops = Vec::new();
        let mut y = 0.0;
        for (offset, row) in rows.clone().enumerate() {
            tops.push(y);
            y += self.thickness(row) + heights.get(offset).copied().unwrap_or(0.0);
        }
        let bottom = y + if last { self.thickness(rows.end) } else { 0.0 };
        tops.push(y);
        let mut items = Vec::new();
        self.paint_table_shading((y, bottom), last, &mut items, context.fonts);
        // Shading, then content, cell by cell.
        for (offset, row) in rows.clone().enumerate() {
            let row_cells = self.rows.get(row).into_iter().flatten();
            for (placed, cell) in row_cells.zip(laid.cells.get_mut(offset).into_iter().flatten()) {
                let Some(cell) = cell else {
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
                self.paint_cell_shading(
                    row,
                    placed,
                    (left, right),
                    (top, below),
                    &mut items,
                    context.fonts,
                );
                let units = cell.next..cell.content.units.len();
                cell.next = units.end;
                let drawn = cell.content.paint(units, context);
                // The content below the row's top border and the cell's top margin,
                // aligned in the room the row leaves it.
                let margins = &cell.margins;
                let content_top = top + self.thickness(row) + margins.top().0;
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
                let x = left + margins.start().0;
                let mut content = drawn.items;
                shift(&mut content, x, content_top + down);
                items.extend(content);
            }
        }
        self.paint_borders((rows, &tops, bottom, last), &mut items);
        Drawn {
            height: Pt(bottom),
            items,
        }
    }

    /// The shading of the cell `placed` of row `row`, between `x` and from `y.0` to
    /// `y.1`.
    fn paint_cell_shading(
        &self,
        row: usize,
        placed: &Placed,
        (left, right): (f64, f64),
        (top, below): (f64, f64),
        items: &mut Vec<DisplayItem>,
        fonts: &mut Fonts<'_>,
    ) {
        let color = self
            .cell(row, placed.cell)
            .and_then(Cell::shading)
            .and_then(|shading| display::fill_color(&shading, fonts, &self.location));
        if let Some(color) = color {
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
    }

    /// The room the borders along grid line `line` take: above row `line`, or below the
    /// table after its last row.
    fn thickness(&self, line: usize) -> f64 {
        self.thickness.get(line).copied().unwrap_or(0.0)
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

    /// The border drawn along grid line `line` beside row `row`.
    fn vertical_border(&self, row: usize, line: usize) -> Option<&Border> {
        let borders = self.vertical.get(row)?;
        let index = borders.binary_search_by_key(&line, |(at, _)| *at).ok()?;
        borders.get(index).map(|(_, border)| border)
    }

    /// The width of a cell's content: its columns less its margins.
    fn content_width(&self, placed: &Placed, margins: &CellMargins) -> Pt {
        let (left, right) = self.cell_edges(placed);
        Pt((right - left - margins.start().0 - margins.end().0).max(0.0))
    }

    /// The x of each grid line in a column `width` wide, the table placed as its
    /// alignment and indent say. A table of fixed layout takes the grid's widths, scaled to
    /// an explicit width; one that fits its columns to their content fits them to the room
    /// it may take or to its explicit width ([`autofit::widths`]), keeping its columns at
    /// their minima past it with `growAutofit`.
    fn grid_lines(&self, width: f64, columns: usize, fonts: &mut Fonts<'_>) -> Vec<f64> {
        let props = self.table.props();
        let outer = self.outer();
        let room = self.room(width, outer);
        let mut widths: Vec<f64> = if props.layout() == TableLayout::Autofit {
            // An explicit width is fitted as case 2 fits 100 % of the room: what the columns
            // want, preferred widths included, scaled to it, or below their minima cut to
            // it. Hypotheses until a Word render, as the renders have only 100 % with auto
            // cells: an explicit width is kept past the room, and minima are cut to it, not
            // kept.
            let target = match props.width() {
                PreferredWidth::Points(points) => Target::Exact(points.0.clamp(0.0, MAX_TABLE)),
                PreferredWidth::Percent(percent) => {
                    Target::Exact((percent / 100.0 * room).clamp(0.0, MAX_TABLE))
                }
                PreferredWidth::Auto => Target::Auto(room.clamp(0.0, MAX_TABLE)),
            };
            autofit::widths(&self.needs, target, self.grow)
        } else {
            let mut widths: Vec<f64> = (0..columns)
                .map(|column| self.table.grid().get(column).map_or(0.0, |width| width.0))
                .collect();
            // An explicit width scales the grid to it. Hypotheses until a Word render
            // (`E-table-width-scale.docx`): no reference has a table of fixed layout whose
            // width differs from its grid, nor one with a width in percent, which takes the
            // room of a table that fits its content (Word's renders of case 2 of
            // `E-table-autofit-*`).
            let target = match props.width() {
                PreferredWidth::Points(points) => Some(points.0),
                PreferredWidth::Percent(percent) => Some(percent / 100.0 * room),
                PreferredWidth::Auto => None,
            };
            let total: f64 = widths.iter().map(|width| width.min(MAX_COLUMN)).sum();
            if let Some(target) = target.map(|target| target.clamp(0.0, MAX_TABLE)) {
                if total > 0.0 && (target - total).abs() > 1e-6 {
                    let scale = target / total;
                    for width in &mut widths {
                        *width = width.min(MAX_COLUMN) * scale;
                    }
                }
            }
            widths
        };
        if widths.iter().any(|&width| width > MAX_COLUMN) {
            fonts.report_once(
                ReportKind::ValueClamped,
                &self.location,
                format!("table columns wider than {MAX_COLUMN} pt are laid out that wide"),
            );
        }
        for width in &mut widths {
            *width = width.clamp(0.0, MAX_COLUMN);
        }
        let total: f64 = widths.iter().sum();
        // Word's renders of `E-table-indent-14` and `-15` (tables F and G) and of
        // `E-table-autofit-*`: a table aligned at the start sits at its indent, in Word
        // 2013's mode with the outer edge of its start border there, in earlier modes
        // pulled out by its first cell's start margin, so that its text lines up with the
        // paragraphs'; a centred table is centred, its borders included in Word 2013's
        // mode, whatever its indent. A table at the end is placed likewise, by hypothesis.
        let edge = if self.borders_inside {
            self.vertical_border(0, 0)
                .map_or(0.0, |border| border.width().0 / 2.0)
        } else {
            0.0
        };
        let start = edge
            + match props.alignment() {
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

    /// The width the grid may take in a column `width` wide, the table's outer borders
    /// taking `outer` beyond it: Word's renders of `E-table-autofit-*` (100 %), in Word
    /// 2013's mode the column less the indent and the outer borders, in earlier modes the
    /// column less the indent and with the first cell's start margin and the last cell's
    /// end margin, by which the table reaches past it on either side. A centred table, or
    /// one at the end, ignores the indent, and in earlier modes is not pulled out:
    /// hypotheses.
    fn room(&self, width: f64, outer: f64) -> f64 {
        let props = self.table.props();
        let start =
            props.alignment() != TableAlignment::Center && props.alignment() != TableAlignment::End;
        let indent = if start { props.indent().0 } else { 0.0 };
        let margins = if self.borders_inside || !start {
            0.0
        } else {
            self.first_start_margin() + self.last_end_margin()
        };
        width - indent - outer + margins
    }

    /// The end margin of the last cell of the first row.
    fn last_end_margin(&self) -> f64 {
        self.rows
            .first()
            .and_then(|row| row.last())
            .and_then(|placed| self.cell(0, placed.cell))
            .map_or(self.table.props().cell_margins().end().0, |cell| {
                self.margins(cell).end().0
            })
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
    /// the bottom of the last), and, when `closed`, the line below the last row, through
    /// which the vertical borders of that row run to `bottom`: the table's bottom border
    /// where the band ends it, or the line that closes it at the end of a page.
    fn paint_borders(
        &self,
        (rows, tops, bottom, closed): (Range<usize>, &[f64], f64, bool),
        items: &mut Vec<DisplayItem>,
    ) {
        for (offset, row) in rows.clone().enumerate() {
            let top = tops.get(offset).copied().unwrap_or(0.0);
            self.paint_horizontal(row, top, items);
        }
        if closed {
            let top = tops.last().copied().unwrap_or(0.0);
            self.paint_horizontal(rows.end, top, items);
        }
        for (offset, row) in rows.clone().enumerate() {
            let top = tops.get(offset).copied().unwrap_or(0.0);
            let below = if closed && row + 1 == rows.end {
                bottom
            } else {
                tops.get(offset + 1).copied().unwrap_or(bottom)
            };
            self.paint_vertical(row, (top, below), items);
        }
    }

    /// The borders along grid line `line`, the line's top at `top`.
    fn paint_horizontal(&self, line: usize, top: f64, items: &mut Vec<DisplayItem>) {
        let x = |line: usize| self.lines.get(line).copied().unwrap_or(0.0);
        let segments = self.horizontal.get(line).map_or(&[][..], Vec::as_slice);
        let y = top + self.thickness(line) / 2.0;
        for segment in segments {
            let (from, to) = (x(segment.columns.start), x(segment.columns.end));
            border_line(&segment.border, (from, y), (to, y), items);
        }
    }

    /// The borders beside row `row`, from `y.0` down to `y.1`.
    fn paint_vertical(&self, row: usize, (top, below): (f64, f64), items: &mut Vec<DisplayItem>) {
        for (line, border) in self.vertical.get(row).into_iter().flatten() {
            let x = self.lines.get(*line).copied().unwrap_or(0.0);
            border_line(border, (x, top), (x, below), items);
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
        self.thickness = horizontal
            .iter()
            .map(|segments: &Vec<Segment>| {
                segments
                    .iter()
                    .map(|segment| segment.border.width().0)
                    .fold(0.0, f64::max)
            })
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

/// A band laid out: its cells' content, which a page takes whole or, a row that may
/// split, piece by piece ([`Prepared::piece`]).
pub(crate) struct LaidBand<'d> {
    rows: Range<usize>,
    /// Each cell's content, by row and place in the row: `None` for a cell merged into
    /// the one above, and for a place without a cell.
    cells: Vec<Vec<Option<LaidCell<'d>>>>,
    /// The height of the pieces of its row already put on pages, below their top
    /// borders; `None` while none is.
    put: Option<f64>,
}

/// A cell's content laid out, with its margins, and the first of its units no page has
/// taken yet.
struct LaidCell<'d> {
    content: Stacked<'d>,
    margins: CellMargins,
    next: usize,
}

impl LaidBand<'_> {
    /// Whether a piece of it is on a page.
    pub(crate) fn started(&self) -> bool {
        self.put.is_some()
    }
}

impl LaidCell<'_> {
    /// The height the cell takes whole, with its margins.
    fn height(&self) -> f64 {
        self.margins.top().0 + self.content.height + self.margins.bottom().0
    }
}

/// A piece of a band, put on a page.
pub(crate) struct Piece {
    /// What it draws from its top at the column's start edge, and its height: with the
    /// border above it, and the border below it when it ends the table or its band goes
    /// on on the next page.
    pub(crate) drawn: Drawn,
    /// Whether it ends its band.
    pub(crate) done: bool,
}

/// The content of a cell laid out: its paragraphs and nested tables one under the
/// other, painted whole or in pieces, a page taking whole units of it: a line of a
/// paragraph or a band of a nested table.
#[derive(Default)]
struct Stacked<'d> {
    /// Its height whole: the space above its first paragraph, between each two blocks
    /// and below its last paragraph included.
    height: f64,
    /// What is behind it, the cell's shading or the table's, which decides the automatic
    /// text colour.
    background: Option<Color>,
    parts: Vec<Part<'d>>,
    units: Vec<Unit>,
}

/// A block of a cell's content laid out.
enum Part<'d> {
    /// A paragraph, painted when a page takes its lines.
    Paragraph {
        paragraph: &'d Paragraph,
        layout: ParagraphLayout,
        location: Location,
    },
    /// A nested table: what each of its bands draws, and the line below each, which
    /// closes it where a page ends under it.
    Table {
        bands: Vec<Drawn>,
        closings: Vec<Drawn>,
        /// Whether it has header rows, which a page it goes on to does not repeat.
        header: bool,
        location: Location,
    },
}

/// A line of a paragraph or a band of a nested table, which a page takes whole.
#[derive(Clone, Copy, Debug)]
struct Unit {
    /// The part it is of, and its line or band there.
    part: usize,
    index: usize,
    /// Its top from the top of the content, and its height.
    top: f64,
    height: f64,
    /// Whether a page may end above it: not inside a nested table's band, and not where
    /// the widow control of its paragraph keeps lines together.
    breaks: bool,
}

impl<'d> Stacked<'d> {
    /// Where the piece that starts with unit `from` starts: the content's top for its
    /// first unit, so that the space above its first paragraph comes with it, and
    /// otherwise the unit's top, leaving out the space above it.
    fn start(&self, from: usize) -> f64 {
        if from == 0 {
            0.0
        } else {
            self.units.get(from).map_or(self.height, |unit| unit.top)
        }
    }

    /// Where the piece that ends before unit `to` ends: the content's bottom after its
    /// last unit, so that the space below its last paragraph comes with it, and
    /// otherwise the bottom of unit `to - 1` and of the line that closes a nested table
    /// cut after it.
    fn end(&self, to: usize) -> f64 {
        if to >= self.units.len() {
            return self.height;
        }
        let Some(unit) = to.checked_sub(1).and_then(|last| self.units.get(last)) else {
            return 0.0;
        };
        unit.top + unit.height + self.closing(unit).map_or(0.0, |closing| closing.height.0)
    }

    /// The line that closes the nested table of `unit` when a page ends below it: none
    /// after the table's last band, whose height has the table's bottom border.
    fn closing(&self, unit: &Unit) -> Option<&Drawn> {
        match self.parts.get(unit.part)? {
            Part::Table {
                bands, closings, ..
            } if unit.index + 1 < bands.len() => closings.get(unit.index),
            _ => None,
        }
    }

    /// The height of the piece of units `from..to`.
    fn extent(&self, from: usize, to: usize) -> f64 {
        if to <= from {
            return 0.0;
        }
        self.end(to) - self.start(from)
    }

    /// How many units from `from` on fit in `room`: as many as a page may end after, and
    /// as many as fit whatever the breaks allow.
    fn fit(&self, from: usize, room: f64) -> (usize, usize) {
        let start = self.start(from);
        let (mut allowed, mut any) = (from, from);
        for to in from + 1..=self.units.len() {
            let Some(unit) = self.units.get(to - 1) else {
                break;
            };
            if unit.top + unit.height - start > room + EPSILON {
                break;
            }
            if self.extent(from, to) <= room + EPSILON {
                any = to;
                if self.units.get(to).is_none_or(|next| next.breaks) {
                    allowed = to;
                }
            }
        }
        (allowed - from, any - from)
    }

    /// Paints units `units`: the paragraphs' lines and the nested tables' bands among
    /// them, from the piece's top, and the line that closes a nested table cut after its
    /// last.
    fn paint(&mut self, units: Range<usize>, context: &mut Context<'d, '_, '_>) -> Drawn {
        let start = self.start(units.start);
        let height = self.extent(units.start, units.end);
        let mut items = Vec::new();
        let mut at = units.start;
        while at < units.end {
            let Some(first) = self.units.get(at).copied() else {
                break;
            };
            let mut end = at + 1;
            while end < units.end
                && self
                    .units
                    .get(end)
                    .is_some_and(|unit| unit.part == first.part)
            {
                end += 1;
            }
            let last = self.units.get(end - 1).copied().unwrap_or(first);
            let closing = (end == units.end)
                .then(|| self.closing(&last))
                .flatten()
                .map(|closing| closing.items.clone());
            match self.parts.get_mut(first.part) {
                Some(Part::Paragraph {
                    paragraph,
                    layout,
                    location,
                }) => {
                    let lines = first.index..last.index + 1;
                    let origin = Point {
                        x: Pt(0.0),
                        y: Pt(first.top - start),
                    };
                    let mut painting = Painting {
                        document: context.document,
                        paragraph,
                        layout: &*layout,
                        fonts: context.fonts,
                        location: &*location,
                        background: self.background,
                        leaders: context.leaders,
                        // A cell does not place floating objects yet: reported by `stack`.
                        page: None,
                        floats: Vec::new(),
                        // Hypothesis: in a cell, as no reference shows it, a bar tab reaches
                        // only through the lines.
                        bar_reach: (0.0, 0.0),
                    };
                    painting.paint_shading(lines.clone(), origin, &mut items);
                    painting.paint(lines, origin, &mut items);
                }
                Some(Part::Table {
                    bands,
                    header,
                    location,
                    ..
                }) => {
                    for unit in self.units.get(at..end).unwrap_or(&[]) {
                        if let Some(band) = bands.get_mut(unit.index) {
                            let mut drawn = std::mem::take(&mut band.items);
                            shift(&mut drawn, 0.0, unit.top - start);
                            items.extend(drawn);
                        }
                    }
                    if let Some(mut closing) = closing {
                        shift(&mut closing, 0.0, last.top + last.height - start);
                        items.extend(closing);
                        if *header {
                            context.fonts.report_once(
                                ReportKind::FeatureUnsupported,
                                location,
                                "header rows of a table in a table cell are not repeated on \
                                 the page it goes on to"
                                    .to_owned(),
                            );
                        }
                    }
                }
                None => {}
            }
            at = end;
        }
        Drawn {
            height: Pt(height),
            items,
        }
    }
}

/// Lays out `blocks`, a cell's content, one under the other, `width` wide, on
/// `background`, as a header stacks its paragraphs: the space above the first, between
/// each two, and below the last, all in its height; each nested table band by band.
/// The empty paragraph that ends a cell after a nested table takes no room (Word's
/// renders of `E-table-autofit-15.docx` and `E-table-row-split-nested.docx`, in Word
/// 2013's mode; a hypothesis for earlier modes).
fn stack<'d>(
    blocks: &'d [Block],
    (width, background): (Pt, Option<Color>),
    labels: &mut Labels<'_>,
    location: &mut Location,
    context: &mut Context<'d, '_, '_>,
) -> Stacked<'d> {
    let document = context.document;
    let mut stacked = Stacked {
        background,
        ..Stacked::default()
    };
    let mut height = 0.0;
    let mut above: Option<&Paragraph> = None;
    for (index, block) in blocks.iter().enumerate() {
        location.push(Step::Index(index));
        match block {
            Block::Paragraph(paragraph) => {
                let label = labels.take();
                let after_table = index
                    .checked_sub(1)
                    .is_some_and(|before| matches!(blocks.get(before), Some(Block::Table(_))));
                if after_table
                    && index + 1 == blocks.len()
                    && label.is_none()
                    && paragraph.inlines().is_empty()
                {
                    location.pop();
                    continue;
                }
                let props = paragraph.props();
                height += match above {
                    Some(above) => paragraph::space_between(document, above.props(), props).0,
                    None => paragraph::space_before(document, props).0,
                };
                let layout = paragraph::lay_out_with(
                    document,
                    (paragraph, label),
                    width,
                    (&mut *context.fonts, &mut *context.shaper),
                    location,
                    context.values,
                );
                report_borders(paragraph, location, context.fonts);
                let floating = layout
                    .itemized
                    .items
                    .iter()
                    .any(|item| matches!(item, Item::Anchored(_)));
                if floating {
                    context.fonts.report_once(
                        ReportKind::FeatureUnsupported,
                        location,
                        "floating pictures in table cells are not drawn yet".to_owned(),
                    );
                }
                // Widow control keeps two lines on each side of a page's end, as in the
                // body: Word moves a row of two lines whole (its render of
                // `E-table-autofit-15.docx`, case 11).
                let widow = props.widow_control();
                let count = layout.lines.len();
                for (line, laid) in layout.lines.iter().enumerate() {
                    stacked.units.push(Unit {
                        part: stacked.parts.len(),
                        index: line,
                        top: height,
                        height: laid.height.0,
                        breaks: line == 0 || !widow || (line >= 2 && count - line >= 2),
                    });
                    height += laid.height.0;
                }
                stacked.parts.push(Part::Paragraph {
                    paragraph,
                    layout,
                    location: location.clone(),
                });
                above = Some(paragraph);
            }
            Block::Table(table) => {
                if let Some(above) = above.take() {
                    height += paragraph::space_after(document, above.props()).0;
                }
                let rest = labels.list.get(labels.next..).unwrap_or(&[]);
                let nested = Prepared::new(table, rest, width, location, context);
                let mut bands = Vec::new();
                let mut closings = Vec::new();
                for band in 0..nested.band_count() {
                    let drawn = nested.band(band, context);
                    closings.push(nested.closing(band, context.fonts));
                    stacked.units.push(Unit {
                        part: stacked.parts.len(),
                        index: band,
                        top: height,
                        height: drawn.height.0,
                        breaks: true,
                    });
                    height += drawn.height.0;
                    bands.push(drawn);
                }
                labels.next += nested.paragraphs();
                stacked.parts.push(Part::Table {
                    bands,
                    closings,
                    header: has_header_rows(table),
                    location: location.clone(),
                });
            }
        }
        location.pop();
    }
    if let Some(last) = above {
        height += paragraph::space_after(document, last.props()).0;
    }
    stacked.height = height;
    stacked
}

/// The narrowest and the widest `blocks`, the content of a cell, are laid out: those of
/// their widest paragraph or nested table (Word's renders of `E-table-autofit-*`). Each
/// paragraph takes its label from `labels`; `location` is the cell's, for the report.
fn block_widths<'d>(
    blocks: &'d [Block],
    labels: &mut Labels<'_>,
    location: &mut Location,
    context: &mut Context<'d, '_, '_>,
) -> (f64, f64) {
    let mut widths: (f64, f64) = (0.0, 0.0);
    for (index, block) in blocks.iter().enumerate() {
        location.push(Step::Index(index));
        let (min, max) = match block {
            Block::Paragraph(paragraph) => {
                let label = labels.take();
                let (min, max) = paragraph::content_widths(
                    context.document,
                    (paragraph, label),
                    (&mut *context.fonts, &mut *context.shaper),
                    location,
                    context.values,
                );
                (min.0, max.0)
            }
            Block::Table(table) => {
                let key = std::ptr::from_ref(table) as usize;
                let widths = match context.measured.get(&key) {
                    Some(&widths) => widths,
                    None => {
                        let rest = labels.list.get(labels.next..).unwrap_or(&[]);
                        let widths =
                            Prepared::new(table, rest, Pt(0.0), location, context).widths();
                        context.measured.insert(key, widths);
                        widths
                    }
                };
                labels.next += row_labels(table).last().copied().unwrap_or(0);
                widths
            }
        };
        widths = (widths.0.max(min), widths.1.max(max));
        location.pop();
    }
    widths
}

/// The larger of `a` and `b`, or whichever there is.
fn larger(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
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
    cell.hide_mark() && hidden_content(cell)
}

/// Whether `cell` has no content: only paragraphs without inlines.
fn hidden_content(cell: &Cell) -> bool {
    cell.blocks().iter().all(|block| match block {
        Block::Paragraph(paragraph) => paragraph.inlines().is_empty(),
        Block::Table(_) => false,
    })
}

/// The cells of each row of `table` in their grid columns, with how they merge
/// vertically: a cell that continues a merge joins the cell above it whose columns are
/// the same, and one that has none above starts a merge of its own. Columns past the
/// grid are laid out with no width, so a cell's columns end at the grid's end: however
/// many columns cells span, a table takes room for its grid and its cells only. A cell
/// with no columns (past the grid, or in a table without one) merges with nothing: the
/// columns of the other cells of a row do not overlap, so each continuing cell joins one
/// merge, and finding them takes work in proportion to the cells.
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
            if placed.merge == Merge::Continues
                && (placed.columns.is_empty() || same(above, &placed.columns).is_none())
            {
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

/// Whether `table` starts with header rows, which Word repeats at the top of every page
/// the table continues on (`w:tblHeader`).
pub(crate) fn has_header_rows(table: &Table) -> bool {
    table.rows().first().is_some_and(|row| row.header())
}

/// The work laying out `blocks` again takes: one for each paragraph, and for each table
/// its rows, cells and grid columns too, nested tables' included. A header or footer
/// with a field is laid out again on every page, so this is what bounds it.
pub(crate) fn work(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(_) => 1,
            Block::Table(table) => table_work(table),
        })
        .fold(0, usize::saturating_add)
}

/// [`work`] of `table`.
pub(crate) fn table_work(table: &Table) -> usize {
    let cells = table
        .rows()
        .iter()
        .flat_map(|row| row.cells())
        .map(|cell| work(cell.blocks()).saturating_add(1))
        .fold(0, usize::saturating_add);
    cells
        .saturating_add(table.rows().len())
        .saturating_add(table.grid().len())
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
    let (rank_a, rank_b) = (rank(a.style()), rank(b.style()));
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

/// A border style's place in Word's precedence list, the earlier drawn over the later
/// where two borders of the same weight meet (MS-OI29500 2.1.169 a).
fn rank(style: BorderStyle) -> u8 {
    match style {
        BorderStyle::Single => 0,
        BorderStyle::Thick => 1,
        BorderStyle::Double => 2,
        BorderStyle::Dotted => 3,
        BorderStyle::Dashed => 4,
        BorderStyle::DotDash => 5,
        BorderStyle::DotDotDash => 6,
        BorderStyle::Triple => 7,
        BorderStyle::ThinThickSmallGap => 8,
        BorderStyle::ThickThinSmallGap => 9,
        BorderStyle::ThinThickThinSmallGap => 10,
        BorderStyle::ThinThickMediumGap => 11,
        BorderStyle::ThickThinMediumGap => 12,
        BorderStyle::ThinThickThinMediumGap => 13,
        BorderStyle::ThinThickLargeGap => 14,
        BorderStyle::ThickThinLargeGap => 15,
        BorderStyle::ThinThickThinLargeGap => 16,
        BorderStyle::Wave => 17,
        BorderStyle::DoubleWave => 18,
        BorderStyle::DashSmallGap => 19,
        BorderStyle::DashDotStroked => 20,
        BorderStyle::ThreeDEmboss => 21,
        BorderStyle::ThreeDEngrave => 22,
        BorderStyle::Outset => 23,
        BorderStyle::Inset => 24,
    }
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

    /// The properties of a table of fixed layout, which takes its grid's widths.
    fn fixed_props() -> TableProps {
        TableProps::default().with_layout(TableLayout::Fixed)
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
        let mut measured = Measured::new();
        let mut context = Context {
            document,
            fonts: &mut fonts,
            shaper: &mut shaper,
            leaders: &mut leaders,
            values: None,
            measured: &mut measured,
        };
        let prepared = Prepared::new(table, labels, Pt(width), &Location::default(), &mut context);
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
        let mut shaper = Shaper::new();
        let mut leaders = LeaderBudget::default();
        let mut measured = Measured::new();
        let mut context = Context {
            document,
            fonts: &mut fonts,
            shaper: &mut shaper,
            leaders: &mut leaders,
            values: None,
            measured: &mut measured,
        };
        let prepared = Prepared::new(table, &[], Pt(500.0), &Location::default(), &mut context);
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
        // Word's renders of E-table-indent-14 and -15, and of E-table-autofit-*: at the
        // start, a table sits at its indent, in Word 2013's mode with the outer edge of its
        // 0.5 pt border there, so its grid lines are 0.25 pt in, and pulled out by its first
        // cell's start margin in Word 2010's mode; centred, its borders count in Word 2013's
        // mode; an explicit width scales the grid, a width in percent that of the column
        // less the indent and, in Word 2013's mode, the outer borders.
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
                vec![36.25, 86.25, 186.25],
            ),
            (
                CompatMode::Word2010,
                fixed(PreferredWidth::Auto),
                vec![30.6, 80.6, 180.6],
            ),
            (
                CompatMode::Word2013,
                fixed(PreferredWidth::Points(Pt(300.0))),
                vec![36.25, 136.25, 336.25],
            ),
            // Half of the column less the indent and the outer borders: 181.75 pt.
            (
                CompatMode::Word2013,
                fixed(PreferredWidth::Percent(50.0)),
                vec![36.25, 36.25 + 50.0 * 181.75 / 150.0, 218.0],
            ),
            (
                CompatMode::Word2013,
                fixed(PreferredWidth::Auto).with_alignment(TableAlignment::Center),
                vec![125.0, 175.0, 275.0],
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
            // The text of the first cell: at its margin from the grid line, in both modes.
            let first = glyphs(&bands[0])[0].0;
            assert!(
                (first - (expected[0] + 5.4)).abs() < 1e-9,
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
            .with_props(fixed_props().with_borders(borders(2.0)))
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
        // middle one 3 pt. In Word 2013's mode the start border's outer edge is at the
        // column's start, so the grid lines are 1 pt in (Word's renders of
        // E-table-autofit-*).
        assert_eq!(
            drawn_lines,
            [
                (1.0, 1.0, 201.0, 1.0, 2.0),
                (1.0, 0.0, 1.0, 22.0, 2.0),
                (101.0, 0.0, 101.0, 22.0, 3.0),
                (201.0, 0.0, 201.0, 22.0, 2.0),
            ]
        );
        // The text below the top border: 2 pt lower than without borders.
        let plain = Table::default()
            .with_grid([Pt(100.0)])
            .with_props(fixed_props())
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
            .with_props(fixed_props())
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
        // Word's render of `E-table-row-exact-height.docx`: an exact height runs from the
        // top of the row's top border to the top of the next row's.
        let bordered = Table::default()
            .with_grid([Pt(100.0)])
            .with_props(fixed_props().with_borders(borders(1.5)))
            .with_rows(vec![
                row(vec![cell(vec![lines(style, 1, "A")])]),
                row(vec![cell(vec![lines(style, 3, "A")])]).with_height(RowHeight::Exact(Pt(30.0))),
                row(vec![cell(vec![lines(style, 1, "A")])]),
            ]);
        let (_, bands) = laid(&document, &bordered, (400.0, &[]));
        let heights: Vec<f64> = bands.iter().map(|band| band.height.0).collect();
        assert_eq!(heights, [21.5, 30.0, 23.0]);
    }

    #[test]
    fn the_empty_paragraph_that_ends_a_cell_after_a_nested_table_takes_no_room() {
        // Word's renders of `E-table-autofit-15.docx` (case 7) and
        // `E-table-row-split-nested.docx`: the cell is as high as its nested table. A
        // paragraph with text there, or one before the table, takes its line.
        let (document, style) = document(CompatMode::Word2013);
        let nested = || -> Block {
            Table::default()
                .with_grid([Pt(50.0)])
                .with_props(fixed_props())
                .with_rows(vec![row(vec![cell(vec![lines(style, 2, "A")])])])
                .into()
        };
        let height = |blocks: Vec<Block>| -> f64 {
            let table = Table::default()
                .with_grid([Pt(100.0)])
                .with_props(fixed_props())
                .with_rows(vec![row(vec![cell(blocks)])]);
            let (_, bands) = laid(&document, &table, (400.0, &[]));
            bands.iter().map(|band| band.height.0).sum()
        };
        let empty = || -> Block { Paragraph::new(style).into() };
        let line = height(vec![empty()]);
        assert!(line > 0.0);
        assert_eq!(height(vec![nested(), empty()]), 40.0);
        assert_eq!(height(vec![nested(), lines(style, 1, "A")]), 60.0);
        assert_eq!(height(vec![empty(), nested()]), 40.0 + line);
        assert_eq!(height(vec![nested(), empty(), empty()]), 40.0 + 2.0 * line);
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

    /// What laying out every band of `table` of `document` in a column `width` wide
    /// reports, by message.
    fn reported(document: &Document, table: &Table, width: f64) -> Vec<String> {
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let mut leaders = LeaderBudget::default();
        let mut measured = Measured::new();
        let mut context = Context {
            document,
            fonts: &mut fonts,
            shaper: &mut shaper,
            leaders: &mut leaders,
            values: None,
            measured: &mut measured,
        };
        let prepared = Prepared::new(table, &[], Pt(width), &Location::default(), &mut context);
        for band in 0..prepared.band_count() {
            prepared.band(band, &mut context);
        }
        fonts
            .into_report()
            .entries()
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect()
    }

    /// How many bands `table` of `document` has.
    fn band_count(document: &Document, table: &Table) -> usize {
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let mut leaders = LeaderBudget::default();
        let mut measured = Measured::new();
        let mut context = Context {
            document,
            fonts: &mut fonts,
            shaper: &mut shaper,
            leaders: &mut leaders,
            values: None,
            measured: &mut measured,
        };
        Prepared::new(table, &[], Pt(500.0), &Location::default(), &mut context).band_count()
    }

    #[test]
    fn cells_without_columns_merge_with_nothing() {
        // Each continuing cell joins at most one merge, so the work is in proportion to the
        // cells. A row of 2,001 cells over a grid of one column, then 2,000 rows whose
        // second cell, past the grid, continues a merge: every row is a band of its own.
        let (document, _) = document(CompatMode::Word2013);
        let continuing = || {
            row(vec![
                Cell::default(),
                Cell::default().with_vertical_merge(VerticalMerge::Continue),
            ])
        };
        let mut rows = vec![row(vec![Cell::default(); 2001])];
        rows.extend((0..2000).map(|_| continuing()));
        let table = Table::default().with_grid([Pt(100.0)]).with_rows(rows);
        assert_eq!(band_count(&document, &table), 2001);
        // Without a grid no cell has columns: 2,000 rows of one continuing cell under a
        // row of 2,000 cells.
        let mut rows = vec![row(vec![Cell::default(); 2000])];
        rows.extend((0..2000).map(|_| {
            row(vec![
                Cell::default().with_vertical_merge(VerticalMerge::Continue),
            ])
        }));
        assert_eq!(
            band_count(&document, &Table::default().with_rows(rows)),
            2001
        );
        // Cells with columns still merge.
        let table = Table::default().with_grid([Pt(100.0)]).with_rows(vec![
            row(vec![
                Cell::default().with_vertical_merge(VerticalMerge::Restart),
            ]),
            row(vec![
                Cell::default().with_vertical_merge(VerticalMerge::Continue),
            ]),
        ]);
        assert_eq!(band_count(&document, &table), 1);
    }

    #[test]
    fn a_row_s_top_border_takes_the_room_of_its_thickest_segment() {
        // One row of 20,000 cells without content, whose top borders are 0.5 and 1 pt in
        // turn, over a grid of 20,000 columns, is 1 pt high. The room is found once for the
        // grid line, not once for each cell: this case took seconds when it was found for
        // each cell, which no assertion here can tell from milliseconds.
        let (document, _) = document(CompatMode::Word2013);
        let top = |width: f64| {
            CellBorders::default()
                .with_top(BorderSide::Line(Border::default().with_width(Pt(width))))
        };
        let cells: Vec<Cell> = (0..20_000)
            .map(|n| Cell::default().with_borders(top(if n % 2 == 0 { 0.5 } else { 1.0 })))
            .collect();
        let table = Table::default()
            .with_grid(vec![Pt(0.01); 20_000])
            .with_rows(vec![row(cells)]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        assert_eq!(bands.len(), 1);
        assert!(
            (bands[0].height.0 - 1.0).abs() < 1e-9,
            "{}",
            bands[0].height.0
        );
    }

    #[test]
    fn a_merged_cell_s_room_takes_in_the_borders_between_its_rows() {
        // Borders of 6 pt everywhere: a merged cell of three lines (60 pt) over two rows
        // of one line has 20 + 6 + 20 pt; the last row grows by 14 pt, not by 20.
        let (document, style) = document(CompatMode::Word2013);
        let table = Table::default()
            .with_grid([Pt(100.0), Pt(100.0)])
            .with_props(TableProps::default().with_borders(borders(6.0)))
            .with_rows(vec![
                row(vec![
                    cell(vec![lines(style, 3, "A")]).with_vertical_merge(VerticalMerge::Restart),
                    cell(vec![lines(style, 1, "A")]),
                ]),
                row(vec![
                    cell(vec![]).with_vertical_merge(VerticalMerge::Continue),
                    cell(vec![lines(style, 1, "A")]),
                ]),
            ]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        let heights: Vec<f64> = bands.iter().map(|band| band.height.0).collect();
        assert_eq!(heights, [6.0 + 20.0 + 6.0 + 34.0 + 6.0]);
    }

    #[test]
    fn what_tables_lay_out_otherwise_than_word_is_reported() {
        let (document, style) = document(CompatMode::Word2013);
        let has = |messages: &[String], start: &str| {
            messages.iter().any(|message| message.starts_with(start))
        };
        // A column wider than 22 inches is laid out that wide.
        let wide = Table::default()
            .with_grid([Pt(2000.0)])
            .with_props(fixed_props())
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "A")])])]);
        let (lines_x, _) = laid(&document, &wide, (400.0, &[]));
        assert_eq!(lines_x, [0.0, MAX_COLUMN]);
        assert!(has(
            &reported(&document, &wide, 400.0),
            "table columns wider than"
        ));
        // A table that fits its columns to its content is laid out as Word lays it out,
        // with nothing to report.
        let fitting = Table::default()
            .with_grid([Pt(100.0)])
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "A")])])]);
        assert!(reported(&document, &fitting, 400.0).is_empty());
        // Cells past the grid.
        let past = Table::default()
            .with_grid([Pt(100.0)])
            .with_rows(vec![row(vec![
                cell(vec![lines(style, 1, "A")]),
                cell(vec![lines(style, 1, "B")]),
            ])]);
        assert!(has(
            &reported(&document, &past, 400.0),
            "cells past the columns of their table's grid"
        ));
        // Spacing between cells.
        let spaced = Table::default()
            .with_grid([Pt(100.0)])
            .with_props(TableProps::default().with_cell_spacing(Pt(2.0)))
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "A")])])]);
        assert!(has(
            &reported(&document, &spaced, 400.0),
            "spacing between table cells"
        ));
        // An exact row height: reported only where the content is taller.
        let exact = |height: f64| {
            Table::default().with_grid([Pt(100.0)]).with_rows(vec![
                row(vec![cell(vec![lines(style, 2, "A")])])
                    .with_height(RowHeight::Exact(Pt(height))),
            ])
        };
        let note = "content taller than an exact row height";
        assert!(has(&reported(&document, &exact(30.0), 400.0), note));
        assert!(!has(&reported(&document, &exact(40.0), 400.0), note));
        // A continuing cell with content draws none of it.
        let merged = |text: bool| {
            let blocks = if text {
                vec![lines(style, 1, "B")]
            } else {
                vec![Paragraph::new(style).into()]
            };
            Table::default().with_grid([Pt(100.0)]).with_rows(vec![
                row(vec![
                    cell(vec![lines(style, 1, "A")]).with_vertical_merge(VerticalMerge::Restart),
                ]),
                row(vec![
                    cell(blocks).with_vertical_merge(VerticalMerge::Continue),
                ]),
            ])
        };
        let note = "the content of a cell merged into the cell above";
        assert!(has(&reported(&document, &merged(true), 400.0), note));
        assert!(!has(&reported(&document, &merged(false), 400.0), note));
    }

    #[test]
    fn floating_pictures_in_cells_are_reported() {
        use crate::ir::{
            AnchoredObject, HorizontalAnchor, ImageData, ImageFormat, ObjectContent, Picture, Size,
            VerticalAnchor,
        };

        // A cell does not place floating objects yet: a picture anchored in one is not
        // drawn, and that is reported.
        let (mut document, style) = document(CompatMode::Word2013);
        let image = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
        let picture = Inline::anchored(AnchoredObject::new(
            ObjectContent::Picture(Picture::new(image)),
            Size::new(Pt(30.0), Pt(10.0)),
            HorizontalAnchor::default(),
            VerticalAnchor::default(),
        ));
        let paragraph = Paragraph::new(style).with_inlines([picture, Inline::text("A", style)]);
        let table = Table::default()
            .with_grid([Pt(100.0)])
            .with_rows(vec![row(vec![cell(vec![paragraph.into()])])]);
        let messages = reported(&document, &table, 400.0);
        assert!(
            messages
                .iter()
                .any(|message| message.starts_with("floating pictures in table cells")),
            "{messages:?}"
        );
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        assert!(
            !bands[0]
                .items
                .iter()
                .any(|item| matches!(item, DisplayItem::Image(_)))
        );
    }

    #[test]
    fn a_hidden_mark_hides_only_a_cell_without_content() {
        // A cell hiding its mark whose content is a nested table still draws the table.
        let (document, style) = document(CompatMode::Word2013);
        let nested = Table::default()
            .with_grid([Pt(80.0)])
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "N")])])]);
        let table = Table::default()
            .with_grid([Pt(100.0)])
            .with_rows(vec![row(vec![
                cell(vec![nested.into()]).with_hide_mark(true),
            ])]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        let texts: Vec<String> = glyphs(&bands[0]).into_iter().map(|(_, _, t)| t).collect();
        assert_eq!(texts, ["N"]);
        assert!((bands[0].height.0 - 20.0).abs() < 1e-9);
    }

    #[test]
    fn the_last_row_s_lines_reach_through_the_bottom_border() {
        // Borders of 2 pt: one row of one line is 24 pt high with its top and bottom
        // borders, and its vertical lines run all of it.
        let (document, style) = document(CompatMode::Word2013);
        let table = Table::default()
            .with_grid([Pt(100.0)])
            .with_props(TableProps::default().with_borders(borders(2.0)))
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "A")])])]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        let verticals: Vec<(f64, f64)> = drawn_lines(&bands[0])
            .into_iter()
            .filter(|(x0, _, x1, _, _)| (x0 - x1).abs() < 1e-9)
            .map(|(_, y0, _, y1, _)| (y0, y1))
            .collect();
        assert_eq!(verticals, [(0.0, 24.0), (0.0, 24.0)]);
    }

    #[test]
    fn a_cell_s_text_ends_at_its_end_margin() {
        // Text at the end of a cell 100 pt wide ends 50 pt before its right edge with an
        // end margin of 50 pt, where it ends at the edge with none.
        use crate::ir::Alignment;
        let (document, style) = document(CompatMode::Word2010);
        let end = |margin: f64| {
            let paragraph = Paragraph::new(style)
                .with_props(ParagraphProps::default().with_alignment(Alignment::End))
                .with_inlines([Inline::text("A", style)]);
            let table = Table::default()
                .with_grid([Pt(100.0)])
                .with_props(fixed_props())
                .with_rows(vec![row(vec![
                    Cell::default()
                        .with_margins(Some(CellMargins::default().with_end(Pt(margin))))
                        .with_blocks([paragraph.into()]),
                ])]);
            let (_, bands) = laid(&document, &table, (400.0, &[]));
            glyphs(&bands[0])[0].0
        };
        assert!((end(0.0) - end(50.0) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn table_shading_fills_the_table_and_is_the_background_of_its_text() {
        // Borders of 2 pt, black shading: the fill runs from the top border through the
        // bottom one, 24 pt, and automatic text on it is white.
        let (document, style) = document(CompatMode::Word2013);
        let black = Shading::default().with_fill(ColorValue::Rgb(Color::BLACK));
        let table = Table::default()
            .with_grid([Pt(100.0)])
            .with_props(
                TableProps::default()
                    .with_borders(borders(2.0))
                    .with_shading(Some(black)),
            )
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "A")])])]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        let fills: Vec<(f64, f64, Color)> = bands[0]
            .items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Rect {
                    at, height, color, ..
                } => Some((at.y.0, height.0, *color)),
                _ => None,
            })
            .collect();
        assert_eq!(fills, [(0.0, 24.0, Color::BLACK)]);
        let colors: Vec<Color> = bands[0]
            .items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Glyphs(run) => Some(run.color),
                _ => None,
            })
            .collect();
        assert_eq!(colors, [Color::WHITE]);
    }

    #[test]
    fn word_2010_pulls_a_table_out_by_its_first_cell_s_own_margin() {
        // Word's render of E-table-indent-14 (table H): the first cell's own start margin,
        // not the table's, pulls the table out.
        let (document, style) = document(CompatMode::Word2010);
        let table = Table::default()
            .with_grid([Pt(100.0)])
            .with_props(fixed_props().with_indent(Pt(36.0)))
            .with_rows(vec![row(vec![
                cell(vec![lines(style, 1, "A")])
                    .with_margins(Some(CellMargins::default().with_start(Pt(10.0)))),
            ])]);
        let (lines_x, _) = laid(&document, &table, (400.0, &[]));
        assert_eq!(lines_x, [26.0, 126.0]);
    }

    #[test]
    fn a_nested_table_is_measured_once_however_deep_it_lies() {
        // A table that fits its columns measures the tables in its cells, and each of
        // those is measured again when it is laid out: without the widths kept in the
        // context, a paragraph 50 tables deep would be measured 50 times. Its time, laid
        // out, is a few times that of the paragraph in a single table, not 50.
        let (document, style) = document(CompatMode::Word2013);
        let text = "A ".repeat(10_000);
        let chain = |depth: usize| {
            let mut table = Table::default()
                .with_grid([Pt(100.0)])
                .with_rows(vec![row(vec![cell(vec![
                    Paragraph::new(style)
                        .with_inlines([Inline::text(text.as_str(), style)])
                        .into(),
                ])])]);
            for _ in 1..depth {
                table = Table::default()
                    .with_grid([Pt(100.0)])
                    .with_rows(vec![row(vec![cell(vec![
                        table.into(),
                        Paragraph::new(style).into(),
                    ])])]);
            }
            table
        };
        let time = |table: &Table| {
            (0..3)
                .map(|_| {
                    let start = std::time::Instant::now();
                    laid(&document, table, (400.0, &[]));
                    start.elapsed().as_secs_f64()
                })
                .fold(f64::INFINITY, f64::min)
        };
        let (one, deep) = (time(&chain(1)), time(&chain(50)));
        assert!(deep < 8.0 * one + 0.05, "50 deep: {deep} s, alone: {one} s");
    }

    /// Whether `lines` are `expected`, to a thousandth of a point.
    fn at(lines: &[f64], expected: &[f64]) -> bool {
        lines.len() == expected.len()
            && lines
                .iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 1e-3)
    }

    #[test]
    fn a_table_that_fits_its_content_takes_the_widths_of_its_cells() {
        // The test font at 10 pt (`A` 6 pt, a space 2.5 pt), cell margins of 5.4 pt; the
        // grid of 100 pt columns is the document's, which such a table does not use.
        let (old, style_old) = document(CompatMode::Word2010);
        let (document, style) = document(CompatMode::Word2013);
        let one_row = |props: TableProps, cells: Vec<Cell>| {
            Table::default()
                .with_grid(vec![Pt(100.0); cells.len()])
                .with_props(props)
                .with_rows(vec![row(cells)])
        };
        let two = || {
            vec![
                cell(vec![lines(style, 1, "A")]),
                cell(vec![lines(style, 1, "A A A")]),
            ]
        };
        // Its widest lines with the margins, when they fit: 16.8 and 33.8 pt.
        let (lines_x, _) = laid(
            &document,
            &one_row(TableProps::default(), two()),
            (400.0, &[]),
        );
        assert!(at(&lines_x, &[0.0, 16.8, 50.6]), "{lines_x:?}");
        // Between its minima and maxima: the second column takes the room over the
        // minima, as the only one whose maximum is above its minimum.
        let (lines_x, _) = laid(
            &document,
            &one_row(TableProps::default(), two()),
            (40.0, &[]),
        );
        assert!(at(&lines_x, &[0.0, 16.8, 40.0]), "{lines_x:?}");
        // 100 % of the column less its outer borders, in Word 2013's mode, where the outer
        // edge of the start border is at the column's start.
        let full = TableProps::default()
            .with_width(PreferredWidth::Percent(100.0))
            .with_borders(borders(2.0));
        let (lines_x, _) = laid(&document, &one_row(full.clone(), two()), (400.0, &[]));
        assert_eq!((lines_x[0], lines_x[2]), (1.0, 399.0));
        // In Word 2010's mode the table reaches the cell margins past the column on
        // either side, so that its text lines up with the paragraphs'.
        let cells = vec![
            cell(vec![lines(style_old, 1, "A")]),
            cell(vec![lines(style_old, 1, "A A A")]),
        ];
        let (lines_x, _) = laid(&old, &one_row(full, cells), (400.0, &[]));
        assert!(at(&[lines_x[0], lines_x[2]], &[-5.4, 405.4]), "{lines_x:?}");
        // A cell's preferred width in points.
        let preferred =
            vec![cell(vec![lines(style, 1, "A")]).with_width(PreferredWidth::Points(Pt(72.0)))];
        let (lines_x, _) = laid(
            &document,
            &one_row(TableProps::default(), preferred),
            (400.0, &[]),
        );
        assert!(at(&lines_x, &[0.0, 72.0]), "{lines_x:?}");
        // Shares in percent, the table as wide as the widest share needs: twice 33.8 pt.
        let halves = vec![
            cell(vec![lines(style, 1, "A")]).with_width(PreferredWidth::Percent(50.0)),
            cell(vec![lines(style, 1, "A A A")]).with_width(PreferredWidth::Percent(50.0)),
        ];
        let (lines_x, _) = laid(
            &document,
            &one_row(TableProps::default(), halves),
            (400.0, &[]),
        );
        assert!(at(&lines_x, &[0.0, 33.8, 67.6]), "{lines_x:?}");
    }

    #[test]
    fn spans_merged_cells_nested_tables_and_labels_count_as_word_counts_them() {
        let (document, style) = document(CompatMode::Word2013);
        let grid = |columns: usize| vec![Pt(100.0); columns];
        // A span's long word goes to its last column.
        let spanned = Table::default().with_grid(grid(2)).with_rows(vec![
            row(vec![
                cell(vec![lines(style, 1, "AAAAAAAAAA")]).with_grid_span(2),
            ]),
            row(vec![
                cell(vec![lines(style, 1, "A")]),
                cell(vec![lines(style, 1, "A")]),
            ]),
        ]);
        let (lines_x, _) = laid(&document, &spanned, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 16.8, 70.8]), "{lines_x:?}");
        // A cell merged into the one above needs nothing.
        let merged = Table::default().with_grid(grid(1)).with_rows(vec![
            row(vec![
                cell(vec![lines(style, 1, "A")]).with_vertical_merge(VerticalMerge::Restart),
            ]),
            row(vec![
                cell(vec![lines(style, 1, "AAAAAAAAAA")])
                    .with_vertical_merge(VerticalMerge::Continue),
            ]),
        ]);
        let (lines_x, _) = laid(&document, &merged, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 16.8]), "{lines_x:?}");
        // Word's render of case 7 of E-table-autofit-15: a nested table counts with its
        // outer borders, here of 0.5 pt, and the outer cell's margins: 33.8 + 0.5 + 10.8.
        let inner = Table::default()
            .with_grid(grid(1))
            .with_props(TableProps::default().with_borders(borders(0.5)))
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "A A A")])])]);
        let outer = Table::default()
            .with_grid(grid(1))
            .with_rows(vec![row(vec![cell(vec![
                inner.into(),
                Paragraph::new(style).into(),
            ])])]);
        let (lines_x, _) = laid(&document, &outer, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 45.1]), "{lines_x:?}");
        // A list label is part of its paragraph's line: `AA` and `A`.
        let label = Label {
            text: "AA".into(),
            style,
            suffix: Suffix::Nothing,
            alignment: LevelAlignment::Start,
        };
        let listed = Table::default()
            .with_grid(grid(1))
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "A")])])]);
        let (lines_x, _) = laid(&document, &listed, (400.0, &[Some(label)]));
        assert!(at(&lines_x, &[0.0, 28.8]), "{lines_x:?}");
    }

    #[test]
    fn minima_that_do_not_fit_are_cut_unless_word_2010_grows_the_table() {
        let table = |style| {
            Table::default()
                .with_grid(vec![Pt(100.0); 2])
                .with_rows(vec![row(vec![
                    cell(vec![lines(style, 1, "AAAAA")]),
                    cell(vec![lines(style, 1, "AAAAA")]),
                ])])
        };
        let grown = |mode: CompatMode| {
            Document::new(
                DocumentSettings::default()
                    .with_compat_mode(mode)
                    .with_compat_flags([CompatFlag::GrowAutofit]),
            )
        };
        let run_style = || {
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0))
        };
        // Two words of 40.8 pt with their margins in 30 pt, 40.8 pt in Word 2010's mode.
        let (old, style) = document(CompatMode::Word2010);
        let (lines_x, _) = laid(&old, &table(style), (30.0, &[]));
        assert!(at(&lines_x, &[-5.4, 15.0, 35.4]), "{lines_x:?}");
        let mut flagged = grown(CompatMode::Word2010);
        let style = flagged.add_run_style(run_style());
        let (lines_x, _) = laid(&flagged, &table(style), (30.0, &[]));
        assert!(at(&lines_x, &[-5.4, 35.4, 76.2]), "{lines_x:?}");
        // Word 2013's mode leaves Word 2010's option out.
        let mut flagged = grown(CompatMode::Word2013);
        let style = flagged.add_run_style(run_style());
        let (lines_x, _) = laid(&flagged, &table(style), (30.0, &[]));
        assert!(at(&lines_x, &[0.0, 15.0, 30.0]), "{lines_x:?}");
    }

    #[test]
    fn room_before_a_row_s_cells_and_a_nested_table_s_width_count() {
        let (document, style) = document(CompatMode::Word2013);
        // Review of #415: a row that leaves 100 pt before its cell (`w:wBefore`), whose grid
        // column no cell lies in; and a column no cell and no such room lies in keeps the
        // grid's width.
        let before = |width: PreferredWidth| {
            Table::default()
                .with_grid([Pt(80.0), Pt(100.0)])
                .with_rows(vec![
                    row(vec![cell(vec![lines(style, 1, "A")])])
                        .with_grid_before(1)
                        .with_width_before(width),
                ])
        };
        let (lines_x, _) = laid(
            &document,
            &before(PreferredWidth::Points(Pt(100.0))),
            (400.0, &[]),
        );
        assert!(at(&lines_x, &[0.0, 100.0, 116.8]), "{lines_x:?}");
        let (lines_x, _) = laid(&document, &before(PreferredWidth::Auto), (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 80.0, 96.8]), "{lines_x:?}");
        // A nested table 300 pt wide that fits its columns to their content wants its
        // width, which its outer column then holds: 300 + 10.8.
        let inner = Table::default()
            .with_grid([Pt(100.0)])
            .with_props(TableProps::default().with_width(PreferredWidth::Points(Pt(300.0))))
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "A")])])]);
        let outer = Table::default()
            .with_grid([Pt(100.0)])
            .with_rows(vec![row(vec![cell(vec![
                inner.into(),
                Paragraph::new(style).into(),
            ])])]);
        let (lines_x, _) = laid(&document, &outer, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 310.8]), "{lines_x:?}");
    }

    #[test]
    fn nested_tables_are_measured_as_they_are_laid_out() {
        let (document, style) = document(CompatMode::Word2013);
        let nested = |props: TableProps, cells: Vec<Cell>| -> Block {
            Table::default()
                .with_grid(vec![Pt(100.0); cells.len()])
                .with_props(props)
                .with_rows(vec![row(cells)])
                .into()
        };
        let holder = |inner: Block, other: &str| {
            Table::default()
                .with_grid(vec![Pt(100.0); 2])
                .with_rows(vec![row(vec![
                    cell(vec![inner, Paragraph::new(style).into()]),
                    cell(vec![lines(style, 1, other)]),
                ])])
        };
        // Review of #415: a nested table 300 pt wide is laid out that wide whatever the
        // room, so its column never gives up any of its 310.8 pt to the next one, which
        // wants more than the room leaves.
        let points = nested(
            TableProps::default().with_width(PreferredWidth::Points(Pt(300.0))),
            vec![cell(vec![lines(style, 1, "A")])],
        );
        let (lines_x, _) = laid(&document, &holder(points, &"A ".repeat(40)), (400.0, &[]));
        assert!(at(&lines_x[..2], &[0.0, 310.8]), "{lines_x:?}");
        // A nested table of two 50 % cells is as wide as its shares need, 67.6 pt, and its
        // column holds it with the outer cell's margins.
        let shares = nested(
            TableProps::default(),
            vec![
                cell(vec![lines(style, 1, "A")]).with_width(PreferredWidth::Percent(50.0)),
                cell(vec![lines(style, 1, "A A A")]).with_width(PreferredWidth::Percent(50.0)),
            ],
        );
        let (lines_x, _) = laid(&document, &holder(shares, "A"), (400.0, &[]));
        assert!(at(&lines_x[..2], &[0.0, 78.4]), "{lines_x:?}");
        // Two different nested tables in one row are each measured as themselves.
        let one = nested(
            TableProps::default(),
            vec![cell(vec![lines(style, 1, "A")])],
        );
        let three = nested(
            TableProps::default(),
            vec![cell(vec![lines(style, 1, "A A A")])],
        );
        let both = Table::default()
            .with_grid(vec![Pt(100.0); 2])
            .with_rows(vec![row(vec![
                cell(vec![one, Paragraph::new(style).into()]),
                cell(vec![three, Paragraph::new(style).into()]),
            ])]);
        let (lines_x, _) = laid(&document, &both, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 27.6, 72.2]), "{lines_x:?}");
    }

    #[test]
    fn a_width_in_points_no_wrap_and_the_largest_preferred_width_count() {
        let (document, style) = document(CompatMode::Word2013);
        // A table of 300 pt that fits its content is 300 pt wide.
        let fixed_width = Table::default()
            .with_grid(vec![Pt(100.0); 2])
            .with_props(TableProps::default().with_width(PreferredWidth::Points(Pt(300.0))))
            .with_rows(vec![row(vec![
                cell(vec![lines(style, 1, "A")]),
                cell(vec![lines(style, 1, "A")]),
            ])]);
        let (lines_x, _) = laid(&document, &fixed_width, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 150.0, 300.0]), "{lines_x:?}");
        // A cell that keeps its text on one line needs its widest line, 33.8 pt; the
        // other column takes the rest of 55 pt.
        let no_wrap = Table::default()
            .with_grid(vec![Pt(100.0); 2])
            .with_rows(vec![row(vec![
                cell(vec![lines(style, 1, "A A A")]).with_no_wrap(true),
                cell(vec![lines(style, 1, "A A A")]),
            ])]);
        let (lines_x, _) = laid(&document, &no_wrap, (55.0, &[]));
        assert!(at(&lines_x, &[0.0, 33.8, 55.0]), "{lines_x:?}");
        // Of the preferred widths of a column's cells, the largest counts, neither the
        // first nor the last.
        let preferred_row = |width: f64| {
            row(vec![
                cell(vec![lines(style, 1, "A")]).with_width(PreferredWidth::Points(Pt(width))),
            ])
        };
        let preferred = Table::default().with_grid([Pt(100.0)]).with_rows(vec![
            preferred_row(50.0),
            preferred_row(72.0),
            preferred_row(60.0),
        ]);
        let (lines_x, _) = laid(&document, &preferred, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 72.0]), "{lines_x:?}");
        // Room before a row's cells narrower than the grid's column sets that column.
        let before = Table::default()
            .with_grid([Pt(80.0), Pt(100.0)])
            .with_rows(vec![
                row(vec![cell(vec![lines(style, 1, "A")])])
                    .with_grid_before(1)
                    .with_width_before(PreferredWidth::Points(Pt(50.0))),
            ]);
        let (lines_x, _) = laid(&document, &before, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 50.0, 66.8]), "{lines_x:?}");
    }

    #[test]
    fn a_cell_s_text_runs_between_its_margins_whatever_its_borders() {
        // Word's renders of E-table-autofit-*: in Word 2013's mode a cell's text takes its
        // column less its margins, however thick its borders. Five `A` of 30 pt fit a
        // column of 40.8 pt with 6 pt borders on one line.
        let (document, style) = document(CompatMode::Word2013);
        let table = Table::default()
            .with_grid([Pt(40.8)])
            .with_props(fixed_props().with_borders(borders(6.0)))
            .with_rows(vec![row(vec![cell(vec![lines(style, 1, "AAAAA")])])]);
        let (_, bands) = laid(&document, &table, (400.0, &[]));
        let rows: Vec<f64> = glyphs(&bands[0]).iter().map(|(_, y, _)| *y).collect();
        assert_eq!(rows.len(), 1, "{rows:?}");
    }

    #[test]
    fn grown_nested_tables_hidden_cells_and_no_wrap_in_points_count_as_laid_out() {
        // Re-check of #415: in Word 2010's mode with `growAutofit`, a nested table of 40 pt
        // whose two words need 81.6 pt keeps its minima, and is measured so: its outer
        // column holds it with the outer cell's margins.
        let mut grown = Document::new(
            DocumentSettings::default()
                .with_compat_mode(CompatMode::Word2010)
                .with_compat_flags([CompatFlag::GrowAutofit]),
        );
        let style = grown.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let nested: Block = Table::default()
            .with_grid(vec![Pt(20.0); 2])
            .with_props(TableProps::default().with_width(PreferredWidth::Points(Pt(40.0))))
            .with_rows(vec![row(vec![
                cell(vec![lines(style, 1, "AAAAA")]),
                cell(vec![lines(style, 1, "AAAAA")]),
            ])])
            .into();
        let outer = Table::default()
            .with_grid(vec![Pt(100.0); 2])
            .with_rows(vec![row(vec![
                cell(vec![nested, Paragraph::new(style).into()]),
                cell(vec![lines(style, 1, &"A ".repeat(40))]),
            ])]);
        let (lines_x, _) = laid(&grown, &outer, (400.0, &[]));
        assert!(lines_x[1] - lines_x[0] >= 92.4 - 1e-9, "{lines_x:?}");
        let (document, style) = document(CompatMode::Word2013);
        // A cell that hides its mark and has no content needs nothing, its preferred width
        // neither.
        let hidden = Table::default().with_grid([Pt(100.0)]).with_rows(vec![
            row(vec![cell(vec![lines(style, 1, "A")])]),
            row(vec![
                Cell::default()
                    .with_hide_mark(true)
                    .with_width(PreferredWidth::Points(Pt(200.0)))
                    .with_blocks([Paragraph::new(style).into()]),
            ]),
        ]);
        let (lines_x, _) = laid(&document, &hidden, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 16.8]), "{lines_x:?}");
        // `w:noWrap` with a preferred width in percent keeps the text on one line, the
        // cell's share widened to hold it: 33.8 pt of the 55 pt table.
        let shared_no_wrap = Table::default()
            .with_grid(vec![Pt(100.0); 2])
            .with_props(TableProps::default().with_width(PreferredWidth::Points(Pt(55.0))))
            .with_rows(vec![row(vec![
                cell(vec![lines(style, 1, "A A A")])
                    .with_no_wrap(true)
                    .with_width(PreferredWidth::Percent(10.0)),
                cell(vec![lines(style, 1, "A")]),
            ])]);
        let (lines_x, _) = laid(&document, &shared_no_wrap, (400.0, &[]));
        assert!(lines_x[1] - lines_x[0] >= 33.8 - 1e-9, "{lines_x:?}");
        // `w:noWrap` with a preferred width in points is ignored: the cell takes its 20 pt.
        let fixed_no_wrap = Table::default()
            .with_grid(vec![Pt(100.0); 2])
            .with_rows(vec![row(vec![
                cell(vec![lines(style, 1, "A A A")])
                    .with_no_wrap(true)
                    .with_width(PreferredWidth::Points(Pt(20.0))),
                cell(vec![lines(style, 1, "A")]),
            ])]);
        let (lines_x, _) = laid(&document, &fixed_no_wrap, (400.0, &[]));
        assert!(at(&lines_x, &[0.0, 20.0, 36.8]), "{lines_x:?}");
    }
}
