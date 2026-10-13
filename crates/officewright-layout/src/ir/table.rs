//! Tables: their grid, rows and cells, with widths, margins, borders and shading.

use officewright_core::Pt;

use super::macros::{ir_struct, no_rules};
use super::paragraph::{Block, Border, Shading};
use super::rules::{self, WORD_MAX_LENGTH, WORD_MAX_SHORT_LENGTH};
use super::walk::{Check, Checker, Clamp, Clamper};
use crate::Result;

ir_struct! {
    /// A table: a grid of columns, and rows of cells placed on it.
    ///
    /// ```
    /// use officewright_core::Pt;
    /// use officewright_layout::ir::{
    ///     Border, BorderSide, Cell, CellBorders, Document, PageSetup, Paragraph, PreferredWidth,
    ///     Row, RunStyle, Section, Table, TableBorders, TableProps, VerticalMerge,
    /// };
    ///
    /// let mut document = Document::default();
    /// let style = document.add_run_style(RunStyle::default());
    /// let cell = || Cell::default().with_blocks([Paragraph::new(style).into()]);
    /// let line = BorderSide::Line(Border::default().with_width(Pt(0.5)));
    /// // Three columns; the first row's first cell spans two, and the third column's cells
    /// // are merged down both rows.
    /// let table = Table::default()
    ///     .with_props(
    ///         TableProps::default()
    ///             .with_width(PreferredWidth::Percent(100.0))
    ///             .with_borders(TableBorders::default().with_top(line).with_inside_horizontal(line)),
    ///     )
    ///     .with_grid([Pt(150.0), Pt(150.0), Pt(168.0)])
    ///     .with_rows([
    ///         Row::default().with_header(true).with_cells([
    ///             cell().with_grid_span(2),
    ///             cell().with_vertical_merge(VerticalMerge::Restart),
    ///         ]),
    ///         Row::default().with_cells([
    ///             cell(),
    ///             // No border on its right, whatever the table says.
    ///             cell().with_borders(CellBorders::default().with_end(BorderSide::Nil)),
    ///             cell().with_vertical_merge(VerticalMerge::Continue),
    ///         ]),
    ///     ]);
    /// document.add_section(Section::new(PageSetup::default()).with_blocks([table.into()]));
    /// document.validate()?;
    /// # Ok::<(), officewright_layout::Error>(())
    /// ```
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Table {
        /// The table's properties.
        ref props: TableProps => with_props;
        /// The widths of the grid's columns, from the start edge (`w:tblGrid`). Each cell
        /// covers one or more grid columns.
        vec grid: Pt => with_grid, check grid_widths;
        /// The rows, from the top.
        vec rows: Row => with_rows, check at_least_one_row;
    }
}

fn grid_widths(grid: &[Pt], checker: &mut Checker<'_>) -> Result<()> {
    for (column, width) in grid.iter().enumerate() {
        if width.0 < 0.0 {
            return Err(checker.error(format!(
                "grid column {column} is {} pt wide, a negative width",
                width.0
            )));
        }
    }
    Ok(())
}

// The IR's own rule: a table without rows has no geometry, so a producer leaves it out.
// Word does the like for a row without a cell, which it reports as corrupt (MS-OI29500
// 2.1.178 a).
fn at_least_one_row(rows: &[Row], checker: &mut Checker<'_>) -> Result<()> {
    if rows.is_empty() {
        return Err(checker.error("a table without rows"));
    }
    Ok(())
}

ir_struct! {
    /// The properties of a table, with the table style already applied.
    #[derive(Clone, Debug, PartialEq)]
    pub struct TableProps {
        /// The preferred width of the table.
        copy width: PreferredWidth = PreferredWidth::Auto => with_width,
            check preferred_width_non_negative, clamp preferred_width;
        /// Whether the columns keep the grid's widths or fit the content.
        copy layout: TableLayout = TableLayout::Autofit => with_layout;
        /// The distance from the start edge of the column to the table, honoured whatever
        /// the alignment (MS-OI29500 2.1.155 a). Negative moves the table into the margin.
        copy indent: Pt = Pt(0.0) => with_indent, clamp table_indent;
        /// Where the table sits between the edges of its column.
        copy alignment: TableAlignment = TableAlignment::Start => with_alignment;
        /// The margins inside every cell, unless a cell has its own.
        copy cell_margins: CellMargins = CellMargins::default() => with_cell_margins;
        /// The space between cells, and between the cells and the table's edge. With none,
        /// adjacent cells share their borders.
        copy cell_spacing: Pt = Pt(0.0) => with_cell_spacing, check rules::non_negative,
            clamp table_length;
        /// The borders of the table: the outer edges and the lines between cells.
        copy borders: TableBorders = TableBorders::default() => with_borders;
        /// The shading behind the whole table.
        copy shading: Option<Shading> = None => with_shading;
        /// Whether the table floats at a position of its own (`w:tblpPr`). Floating tables
        /// are laid out in the text flow for now, and reported.
        copy floating: bool = false => with_floating;
        /// Whether the columns run from right to left. Not laid out yet: reported.
        copy bidi_visual: bool = false => with_bidi_visual;
    }
}

/// A preferred width of a table or a cell.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum PreferredWidth {
    /// None: the layout chooses. Also what Word makes of a width of 0 or of type `nil`
    /// (MS-OI29500 2.1.166 a and b, 2.1.171 a and b).
    #[default]
    Auto,
    /// This many points.
    Points(Pt),
    /// This percentage of the available width: 100.0 is all of it. Not clamped: Word
    /// reads the value as fiftieths of a percent without a percent sign and as whole
    /// percents with one (MS-OI29500 2.1.185 a and b), so its limit of a `short` (2.1.185
    /// c) depends on how the file wrote it.
    Percent(f64),
}

impl Check for PreferredWidth {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Auto => Ok(()),
            Self::Points(width) => width.check(checker),
            Self::Percent(percent) if percent.is_finite() => Ok(()),
            Self::Percent(percent) => {
                Err(checker.error(format!("{percent} % is not a finite percentage")))
            }
        }
    }
}

impl Clamp for PreferredWidth {
    fn clamp(&mut self, _: &mut Clamper) {}
}

/// Word stores a table measurement as a `short` of twips (MS-OI29500 2.1.185 c), at
/// least 0 except for the table indent (2.1.185 d); negative ones are rejected by
/// `validate`.
fn table_length(value: &mut Pt, clamper: &mut Clamper) {
    rules::limit(
        value,
        Pt(0.0),
        WORD_MAX_SHORT_LENGTH,
        "table measurement",
        clamper,
    );
}

/// The table indent is a `short` of twips that may be negative (MS-OI29500 2.1.185 c and
/// d).
fn table_indent(value: &mut Pt, clamper: &mut Clamper) {
    let max = WORD_MAX_SHORT_LENGTH;
    rules::limit(value, Pt(-max.0), max, "table indent", clamper);
}

/// A preferred width in points is a table measurement; see [`PreferredWidth::Percent`]
/// for percentages.
fn preferred_width(width: &mut PreferredWidth, clamper: &mut Clamper) {
    if let PreferredWidth::Points(points) = width {
        table_length(points, clamper);
    }
}

/// Word takes only widths of at least zero, except for the table indent (MS-OI29500
/// 2.1.185 d).
fn preferred_width_non_negative(width: &PreferredWidth, checker: &mut Checker<'_>) -> Result<()> {
    match width {
        PreferredWidth::Points(points) if points.0 < 0.0 => {
            Err(checker.error(format!("{} pt is negative", points.0)))
        }
        PreferredWidth::Percent(percent) if *percent < 0.0 => {
            Err(checker.error(format!("{percent} % is negative")))
        }
        _ => Ok(()),
    }
}

/// How the columns of a table get their widths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TableLayout {
    /// The widths of the grid, scaled only to an explicit table width.
    Fixed,
    /// Widths that fit the content and the preferred widths.
    #[default]
    Autofit,
}

/// Where a table sits between the edges of its column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TableAlignment {
    /// At the start edge, after the indent.
    #[default]
    Start,
    /// Centred.
    Center,
    /// At the end edge.
    End,
}

ir_struct! {
    /// The margins inside a cell, between its edges and its content. Margins of type `pct`
    /// or `auto` at the start or end are 0 for Word (MS-OI29500 2.1.146 a, 2.1.125 a),
    /// which the producer passes as 0. The defaults are those of Word's built-in table
    /// style "Normal Table", which Word writes into every document it saves: 0 at the top
    /// and bottom, 108 twips (5.4 pt) at the start and end.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct CellMargins {
        /// At the top.
        copy top: Pt = Pt(0.0) => with_top, check rules::non_negative, clamp table_length;
        /// At the start edge.
        copy start: Pt = Pt(5.4) => with_start, check rules::non_negative, clamp table_length;
        /// At the bottom.
        copy bottom: Pt = Pt(0.0) => with_bottom, check rules::non_negative,
            clamp table_length;
        /// At the end edge.
        copy end: Pt = Pt(5.4) => with_end, check rules::non_negative, clamp table_length;
    }
}

/// One side of a table's or a cell's borders. The difference between
/// [`None`](Self::None) and [`Nil`](Self::Nil) shows where two cells meet without
/// spacing: a side without a border gives way to the other cell's border, a nil side
/// suppresses both (MS-OI29500 2.1.169 a). Word looks a cell's border up in the table
/// style, the row's exceptions and the table when the cell's own is omitted or `none`
/// (2.1.169 a): the producer passes a cell-level `none` as [`Inherit`](Self::Inherit),
/// and [`None`](Self::None) is a border that is still none after that lookup.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum BorderSide {
    /// Not set: a cell takes the table's border on that side; for a table it is
    /// [`None`](Self::None).
    #[default]
    Inherit,
    /// No border (`none`).
    None,
    /// No border, and none of the adjacent cell either (`nil`).
    Nil,
    /// This border.
    Line(Border),
}

impl Check for BorderSide {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Line(border) => border.check(checker),
            Self::Inherit | Self::None | Self::Nil => Ok(()),
        }
    }
}

impl Clamp for BorderSide {
    fn clamp(&mut self, clamper: &mut Clamper) {
        if let Self::Line(border) = self {
            border.clamp(clamper);
        }
    }
}

ir_struct! {
    /// The borders of a table.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct TableBorders {
        /// At the top of the table.
        copy top: BorderSide => with_top;
        /// At its start edge.
        copy start: BorderSide => with_start;
        /// At its bottom.
        copy bottom: BorderSide => with_bottom;
        /// At its end edge.
        copy end: BorderSide => with_end;
        /// Between rows.
        copy inside_horizontal: BorderSide => with_inside_horizontal;
        /// Between columns.
        copy inside_vertical: BorderSide => with_inside_vertical;
    }
}

ir_struct! {
    /// A row of a table. Word's per-row alignment and cell spacing (`w:trPr/w:jc`,
    /// `w:trPr/w:tblCellSpacing`) and hidden rows are not in the IR yet.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Row {
        /// The height of the row.
        copy height: RowHeight = RowHeight::Auto => with_height, clamp row_height;
        /// Whether the row stays on one page.
        copy cant_split: bool = false => with_cant_split;
        /// Whether the row is a header row, repeated on every page the table continues on.
        /// Only rows at the top of the table are.
        copy header: bool = false => with_header;
        /// How many grid columns are left empty before the first cell.
        copy grid_before: u16 = 0 => with_grid_before;
        /// The preferred width of the space before the first cell.
        copy width_before: PreferredWidth = PreferredWidth::Auto => with_width_before,
            check preferred_width_non_negative, clamp preferred_width;
        /// How many grid columns are left empty after the last cell.
        copy grid_after: u16 = 0 => with_grid_after;
        /// The preferred width of the space after the last cell.
        copy width_after: PreferredWidth = PreferredWidth::Auto => with_width_after,
            check preferred_width_non_negative, clamp preferred_width;
        /// The cells, from the start edge.
        vec cells: Cell = Vec::new() => with_cells, check at_least_one_cell;
    }
}

// Word reports a row without a cell as corrupt (MS-OI29500 2.1.178 a).
fn at_least_one_cell(cells: &[Cell], checker: &mut Checker<'_>) -> Result<()> {
    if cells.is_empty() {
        return Err(checker.error("a row without cells"));
    }
    Ok(())
}

/// The height of a table row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum RowHeight {
    /// As high as the content.
    #[default]
    Auto,
    /// As high as the content, but at least this. Word reads a height without a rule
    /// this way (MS-OI29500 2.1.180 a).
    AtLeast(Pt),
    /// This height plus the largest bottom margin of the row's cells (MS-OI29500 2.1.180
    /// d); content that does not fit is cut off.
    Exact(Pt),
}

impl Check for RowHeight {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Auto => Ok(()),
            Self::AtLeast(height) | Self::Exact(height) => {
                height.check(checker)?;
                rules::non_negative(height, checker)
            }
        }
    }
}

impl Clamp for RowHeight {
    fn clamp(&mut self, _: &mut Clamper) {}
}

/// Word takes row heights from 0 to 22 inches (MS-OI29500 2.1.180 b), and an exact
/// height must not be 0 (2.1.180 c); such a row is laid out as high as its content.
fn row_height(height: &mut RowHeight, clamper: &mut Clamper) {
    if let RowHeight::AtLeast(value) | RowHeight::Exact(value) = height {
        rules::limit(value, Pt(0.0), WORD_MAX_LENGTH, "row height", clamper);
    }
    if *height == RowHeight::Exact(Pt(0.0)) {
        clamper.clamped(
            "an exact row height of 0 pt, which Word does not accept, becomes the height of \
             the content"
                .into(),
        );
        *height = RowHeight::Auto;
    }
}

ir_struct! {
    /// A cell of a table row. Fitting text to the cell's width (`w:tcFitText`) and
    /// diagonal borders (`w:tl2br`, `w:tr2bl`) are not in the IR yet; a producer folds
    /// horizontally merged cells (`w:hMerge`) into [`grid_span`](Self::grid_span).
    #[derive(Clone, Debug, PartialEq)]
    pub struct Cell {
        /// The preferred width of the cell.
        copy width: PreferredWidth = PreferredWidth::Auto => with_width,
            check preferred_width_non_negative, clamp preferred_width;
        /// How many grid columns the cell covers.
        copy grid_span: u16 = 1 => with_grid_span, clamp grid_span;
        /// Whether the cell is merged with the cells above and below it.
        copy vertical_merge: VerticalMerge = VerticalMerge::None => with_vertical_merge;
        /// The cell's own margins, which replace the table's. `w:tcMar` sets them side by
        /// side; the producer fills the sides it leaves out from the table's.
        copy margins: Option<CellMargins> = None => with_margins;
        /// The cell's borders, with those of the table style and the row's exceptions
        /// applied.
        copy borders: CellBorders = CellBorders::default() => with_borders;
        /// The shading behind the cell.
        copy shading: Option<Shading> = None => with_shading;
        /// Where the content sits between the top and bottom of the cell.
        copy vertical_align: CellVerticalAlign = CellVerticalAlign::Top => with_vertical_align;
        /// Whether the content is kept on one line where the table can widen the cell.
        copy no_wrap: bool = false => with_no_wrap;
        /// Whether the end-of-cell mark is left out of the height of an empty cell.
        copy hide_mark: bool = false => with_hide_mark;
        /// The direction of the text. Only left to right, top to bottom is laid out yet.
        copy text_direction: TextDirection = TextDirection::LeftToRightTopToBottom
            => with_text_direction;
        /// The content. Its last block is a paragraph, as Word requires (MS-OI29500
        /// 2.1.168 b); that paragraph's mark is the cell's end mark.
        vec blocks: Block = Vec::new() => with_blocks, check ends_with_a_paragraph;
    }
}

fn ends_with_a_paragraph(blocks: &[Block], checker: &mut Checker<'_>) -> Result<()> {
    match blocks.last() {
        Some(Block::Paragraph(_)) => Ok(()),
        Some(_) => Err(checker.error(
            "a cell whose last block is not a paragraph; Word needs one after a nested table",
        )),
        None => Err(checker.error("a cell without content; it needs at least an empty paragraph")),
    }
}

/// Word treats a cell that spans no grid column as one that spans one (MS-OI29500
/// 2.1.130 a).
fn grid_span(span: &mut u16, clamper: &mut Clamper) {
    if *span == 0 {
        clamper.clamped("a cell spanning 0 grid columns spans 1".into());
        *span = 1;
    }
}

/// Whether a cell is merged with the cells above and below it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VerticalMerge {
    /// Not merged.
    #[default]
    None,
    /// The first of merged cells, which holds the content.
    Restart,
    /// A cell merged with the one above.
    Continue,
}

ir_struct! {
    /// The borders of a cell.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct CellBorders {
        /// At the top.
        copy top: BorderSide => with_top;
        /// At the start edge.
        copy start: BorderSide => with_start;
        /// At the bottom.
        copy bottom: BorderSide => with_bottom;
        /// At the end edge.
        copy end: BorderSide => with_end;
    }
}

/// Where the content of a cell sits between its top and bottom. Word knows only these
/// three (MS-OI29500 2.1.181 a).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CellVerticalAlign {
    /// At the top.
    #[default]
    Top,
    /// Centred.
    Center,
    /// At the bottom.
    Bottom,
}

/// The direction of the text in a cell (ST_TextDirection). Left to right, top to bottom
/// when nothing sets it (MS-OI29500 2.1.172 a).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextDirection {
    /// `lrTb`: lines left to right, stacked top to bottom.
    #[default]
    LeftToRightTopToBottom,
    /// `tbRl`: lines top to bottom, stacked right to left.
    TopToBottomRightToLeft,
    /// `btLr`: lines bottom to top, stacked left to right.
    BottomToTopLeftToRight,
    /// `lrTbV`: as `lrTb`, with East Asian characters turned.
    LeftToRightTopToBottomRotated,
    /// `tbRlV`: as `tbRl`, with East Asian characters turned.
    TopToBottomRightToLeftRotated,
    /// `tbLrV`: lines top to bottom, stacked left to right, East Asian characters turned.
    TopToBottomLeftToRightRotated,
}

no_rules!(
    TableLayout,
    TableAlignment,
    VerticalMerge,
    CellVerticalAlign,
    TextDirection
);
