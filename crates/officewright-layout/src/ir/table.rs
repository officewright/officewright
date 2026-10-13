//! Tables: their grid, rows and cells, with widths, margins, borders and shading.

use officewright_core::Pt;

use super::macros::{ir_struct, no_rules};
use super::paragraph::{Block, Border, Shading};
use super::rules::{self, WORD_MAX_LENGTH};
use super::walk::{Check, Checker, Clamp, Clamper};
use crate::Result;

ir_struct! {
    /// A table: a grid of columns, and rows of cells placed on it.
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

// Word reports a file whose table has no row as corrupt, as it does a row without a cell
// (MS-OI29500 2.1.178 a), so a producer leaves such a table out.
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
            check preferred_width_non_negative;
        /// Whether the columns keep the grid's widths or fit the content.
        copy layout: TableLayout = TableLayout::Autofit => with_layout;
        /// The distance from the start edge of the column to the table, honoured whatever
        /// the alignment (MS-OI29500 2.1.155 a). Negative moves the table into the margin.
        copy indent: Pt = Pt(0.0) => with_indent;
        /// Where the table sits between the edges of its column.
        copy alignment: TableAlignment = TableAlignment::Start => with_alignment;
        /// The margins inside every cell, unless a cell has its own.
        copy cell_margins: CellMargins = CellMargins::default() => with_cell_margins;
        /// The space between cells, and between the cells and the table's edge. With none,
        /// adjacent cells share their borders.
        copy cell_spacing: Pt = Pt(0.0) => with_cell_spacing, check rules::non_negative;
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
    /// This percentage of the available width: 100.0 is all of it.
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
    /// or `auto` are 0 for Word (MS-OI29500 2.1.146 a), which the producer passes as 0.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct CellMargins {
        /// At the top.
        copy top: Pt = Pt(0.0) => with_top, check rules::non_negative;
        /// At the start edge. 5.4 pt (108 twips) by default, as Word's built-in table style
        /// sets.
        copy start: Pt = Pt(5.4) => with_start, check rules::non_negative;
        /// At the bottom.
        copy bottom: Pt = Pt(0.0) => with_bottom, check rules::non_negative;
        /// At the end edge, 5.4 pt by default.
        copy end: Pt = Pt(5.4) => with_end, check rules::non_negative;
    }
}

/// One side of a table's or a cell's borders. The difference between
/// [`None`](Self::None) and [`Nil`](Self::Nil) shows where two cells meet without
/// spacing: a side without a border gives way to the other cell's border, a nil side
/// suppresses both (MS-OI29500 2.1.169 a).
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
    /// A row of a table.
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
            check preferred_width_non_negative;
        /// How many grid columns are left empty after the last cell.
        copy grid_after: u16 = 0 => with_grid_after;
        /// The preferred width of the space after the last cell.
        copy width_after: PreferredWidth = PreferredWidth::Auto => with_width_after,
            check preferred_width_non_negative;
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
    match height {
        RowHeight::Auto => {}
        RowHeight::Exact(value) if value.0 == 0.0 => {
            clamper.clamped(
                "an exact row height of 0 pt, which Word does not accept, becomes the height \
                 of the content"
                    .into(),
            );
            *height = RowHeight::Auto;
        }
        RowHeight::AtLeast(value) | RowHeight::Exact(value) => {
            rules::limit(value, Pt(0.0), WORD_MAX_LENGTH, "row height", clamper);
        }
    }
}

ir_struct! {
    /// A cell of a table row.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Cell {
        /// The preferred width of the cell.
        copy width: PreferredWidth = PreferredWidth::Auto => with_width,
            check preferred_width_non_negative;
        /// How many grid columns the cell covers.
        copy grid_span: u16 = 1 => with_grid_span, clamp grid_span;
        /// Whether the cell is merged with the cells above and below it.
        copy vertical_merge: VerticalMerge = VerticalMerge::None => with_vertical_merge;
        /// The cell's own margins, which replace the table's.
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
        /// The content: at least one block, as Word requires (MS-OI29500 2.1.168 b).
        vec blocks: Block = Vec::new() => with_blocks, check at_least_one_block;
    }
}

fn at_least_one_block(blocks: &[Block], checker: &mut Checker<'_>) -> Result<()> {
    if blocks.is_empty() {
        return Err(checker.error("a cell without content; it needs at least an empty paragraph"));
    }
    Ok(())
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

/// The direction of the text in a cell (ST_TextDirection).
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
