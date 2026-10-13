//! The properties of tables, rows and cells as written (`w:tblPr`, `w:tblPrEx`, `w:trPr`,
//! `w:tcPr`; ECMA-376 Part 1, §17.4). A property the element does not set is `None`.

use officewright_core::Pt;

use super::properties::{Border, Shading, xml_enum};

/// A width or length in a table (CT_TblWidth): of the table, a cell, the indentation,
/// the spacing between cells, or a cell margin.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum TableWidth {
    /// Chosen by the layout (`auto`); the written value is ignored.
    Auto,
    /// No width (`nil`). Word reads a cell width of `nil` as `auto`
    /// (MS-OI29500 §17.4.71(b)).
    Nil,
    /// A length (`dxa`, the type assumed when none is written). Word reads a cell width
    /// of 0 as `auto` (MS-OI29500 §17.4.71(a)).
    Length(Pt),
    /// A percentage of the width available: 100.0 is all of it (`pct`).
    Percent(f64),
}

xml_enum! {
    /// How a table is aligned between the margins (ST_JcTable).
    TableJustification {
        Center = "center",
        End = "end",
        Left = "left",
        Right = "right",
        Start = "start",
    }
}

xml_enum! {
    /// How the widths of a table's columns are found (ST_TblLayoutType).
    TableLayout {
        /// From the preferred widths alone.
        Fixed = "fixed",
        /// Fitted to the contents.
        Autofit = "autofit",
    }
}

xml_enum! {
    /// Whether a floating table may overlap another (ST_TblOverlap).
    TableOverlap {
        Never = "never",
        Overlap = "overlap",
    }
}

xml_enum! {
    /// What a position is measured from (ST_HAnchor and ST_VAnchor), for floating tables
    /// and frames.
    PositionAnchor {
        /// The paragraph or the column of text.
        Text = "text",
        Margin = "margin",
        Page = "page",
    }
}

xml_enum! {
    /// A horizontal position relative to an anchor (ST_XAlign).
    HorizontalAlignment {
        Left = "left",
        Center = "center",
        Right = "right",
        /// Left on odd pages, right on even ones.
        Inside = "inside",
        /// Right on odd pages, left on even ones.
        Outside = "outside",
    }
}

xml_enum! {
    /// A vertical position relative to an anchor (ST_YAlign).
    VerticalAlignment {
        /// In line with the text.
        Inline = "inline",
        Top = "top",
        Center = "center",
        Bottom = "bottom",
        Inside = "inside",
        Outside = "outside",
    }
}

xml_enum! {
    /// How the contents of a cell, or of the pages of a section, are placed between top
    /// and bottom (ST_VerticalJc).
    VerticalJustification {
        Top = "top",
        Center = "center",
        /// Spread over the height.
        Both = "both",
        Bottom = "bottom",
    }
}

xml_enum! {
    /// How a cell takes part in a merge with its neighbours (ST_Merge).
    CellMerge {
        /// Starts a merged group.
        Restart = "restart",
        /// Joins the group before it.
        Continue = "continue",
    }
}

xml_enum! {
    /// The direction text flows in a cell or a section (ST_TextDirection), named by the
    /// Transitional value: first the direction in which a line fills, then the
    /// direction in which lines follow each other. The Strict values name only the
    /// second and are read as the Transitional value the standard makes them
    /// equivalent to (MS-OI29500 §17.18.93 quotes the pairs).
    TextDirection {
        /// Lines fill left to right and follow top to bottom: horizontal text (`tb` in
        /// Strict).
        LeftToRightTopToBottom = "lrTb" | "tb",
        /// Lines fill top to bottom and follow right to left: vertical text (`rl` in
        /// Strict).
        TopToBottomRightToLeft = "tbRl" | "rl",
        /// Lines fill bottom to top and follow left to right: text turned to run up
        /// (`lr` in Strict).
        BottomToTopLeftToRight = "btLr" | "lr",
        /// As [`LeftToRightTopToBottom`](Self::LeftToRightTopToBottom), with East
        /// Asian characters rotated 270° (`tbV` in Strict).
        LeftToRightTopToBottomRotated = "lrTbV" | "tbV",
        /// As [`TopToBottomRightToLeft`](Self::TopToBottomRightToLeft), with all text
        /// rotated 90° (`rlV` in Strict).
        TopToBottomRightToLeftRotated = "tbRlV" | "rlV",
        /// Lines fill top to bottom and follow left to right, with all text rotated 90°
        /// (`lrV` in Strict).
        TopToBottomLeftToRightRotated = "tbLrV" | "lrV",
    }
}

xml_enum! {
    /// How the height of a row is set (ST_HeightRule).
    HeightRule {
        /// Fitted to the contents.
        Auto = "auto",
        /// Exactly the height given.
        Exact = "exact",
        /// At least the height given.
        AtLeast = "atLeast",
    }
}

/// The borders of a table (`w:tblBorders`).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TableBorders {
    pub(crate) top: Option<Border>,
    pub(crate) bottom: Option<Border>,
    pub(crate) start: Option<Border>,
    pub(crate) end: Option<Border>,
    pub(crate) inside_horizontal: Option<Border>,
    pub(crate) inside_vertical: Option<Border>,
}

impl TableBorders {
    /// Above the first row.
    pub fn top(&self) -> Option<&Border> {
        self.top.as_ref()
    }

    /// Below the last row.
    pub fn bottom(&self) -> Option<&Border> {
        self.bottom.as_ref()
    }

    /// On the start side (`w:start`, or `w:left` in Transitional documents).
    pub fn start(&self) -> Option<&Border> {
        self.start.as_ref()
    }

    /// On the end side (`w:end`, or `w:right` in Transitional documents).
    pub fn end(&self) -> Option<&Border> {
        self.end.as_ref()
    }

    /// Between rows (`w:insideH`).
    pub fn inside_horizontal(&self) -> Option<&Border> {
        self.inside_horizontal.as_ref()
    }

    /// Between columns (`w:insideV`).
    pub fn inside_vertical(&self) -> Option<&Border> {
        self.inside_vertical.as_ref()
    }
}

/// The borders of a cell (`w:tcBorders`): those of a table, which a cell may set for
/// itself, and two diagonals.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct CellBorders {
    pub(crate) sides: TableBorders,
    pub(crate) top_left_to_bottom_right: Option<Border>,
    pub(crate) top_right_to_bottom_left: Option<Border>,
}

impl CellBorders {
    /// The borders of the sides, and `w:insideH` and `w:insideV`, which apply when the
    /// cell is part of a group a table style formats.
    pub fn sides(&self) -> &TableBorders {
        &self.sides
    }

    /// The diagonal from the top left corner (`w:tl2br`).
    pub fn top_left_to_bottom_right(&self) -> Option<&Border> {
        self.top_left_to_bottom_right.as_ref()
    }

    /// The diagonal from the top right corner (`w:tr2bl`).
    pub fn top_right_to_bottom_left(&self) -> Option<&Border> {
        self.top_right_to_bottom_left.as_ref()
    }
}

/// The space between the edges of cells and their contents (`w:tblCellMar`,
/// `w:tcMar`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct CellMargins {
    pub(crate) top: Option<TableWidth>,
    pub(crate) bottom: Option<TableWidth>,
    pub(crate) start: Option<TableWidth>,
    pub(crate) end: Option<TableWidth>,
}

impl CellMargins {
    /// Above the contents.
    pub fn top(&self) -> Option<TableWidth> {
        self.top
    }

    /// Below the contents.
    pub fn bottom(&self) -> Option<TableWidth> {
        self.bottom
    }

    /// On the start side (`w:start`, or `w:left` in Transitional documents).
    pub fn start(&self) -> Option<TableWidth> {
        self.start
    }

    /// On the end side (`w:end`, or `w:right` in Transitional documents).
    pub fn end(&self) -> Option<TableWidth> {
        self.end
    }
}

/// Which parts of a table its style formats differently (`w:tblLook`).
///
/// Word reads the older bit mask in `w:val` only when none of the six attributes is
/// written, and assumes `04A0` (first row, first column, no vertical bands) when the
/// element is missing (MS-OI29500 §17.4.55(a), (c)); this type gives what is written,
/// the bit mask decoded when it is what Word reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TableLook {
    pub(crate) first_row: Option<bool>,
    pub(crate) last_row: Option<bool>,
    pub(crate) first_column: Option<bool>,
    pub(crate) last_column: Option<bool>,
    pub(crate) no_horizontal_bands: Option<bool>,
    pub(crate) no_vertical_bands: Option<bool>,
}

impl TableLook {
    /// The first row is formatted as a header row.
    pub fn first_row(&self) -> Option<bool> {
        self.first_row
    }

    /// The last row is formatted as a total row.
    pub fn last_row(&self) -> Option<bool> {
        self.last_row
    }

    /// The first column is formatted apart.
    pub fn first_column(&self) -> Option<bool> {
        self.first_column
    }

    /// The last column is formatted apart.
    pub fn last_column(&self) -> Option<bool> {
        self.last_column
    }

    /// Rows are not banded.
    pub fn no_horizontal_bands(&self) -> Option<bool> {
        self.no_horizontal_bands
    }

    /// Columns are not banded.
    pub fn no_vertical_bands(&self) -> Option<bool> {
        self.no_vertical_bands
    }
}

/// Where a floating table sits (`w:tblpPr`), as written.
///
/// Word does not float every table that has the element. It ignores the element when
/// none of its attributes is written, when the table is in a text box, a note or a
/// comment, and when `tblpX` and `tblpY` are 0 with a horizontal anchor of `text` and a
/// vertical anchor that is not. Without anchors it takes `margin` vertically and `text`
/// horizontally, where the standard says `page` (MS-OI29500 §17.4.57(b), (c)).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TablePosition {
    pub(crate) left_from_text: Option<Pt>,
    pub(crate) right_from_text: Option<Pt>,
    pub(crate) top_from_text: Option<Pt>,
    pub(crate) bottom_from_text: Option<Pt>,
    pub(crate) horizontal_anchor: Option<PositionAnchor>,
    pub(crate) vertical_anchor: Option<PositionAnchor>,
    pub(crate) horizontal_alignment: Option<HorizontalAlignment>,
    pub(crate) x: Option<Pt>,
    pub(crate) vertical_alignment: Option<VerticalAlignment>,
    pub(crate) y: Option<Pt>,
}

impl TablePosition {
    /// The space between the table and the text on its left (`w:leftFromText`).
    pub fn left_from_text(&self) -> Option<Pt> {
        self.left_from_text
    }

    /// The space between the table and the text on its right (`w:rightFromText`).
    pub fn right_from_text(&self) -> Option<Pt> {
        self.right_from_text
    }

    /// The space between the table and the text above it (`w:topFromText`).
    pub fn top_from_text(&self) -> Option<Pt> {
        self.top_from_text
    }

    /// The space between the table and the text below it (`w:bottomFromText`).
    pub fn bottom_from_text(&self) -> Option<Pt> {
        self.bottom_from_text
    }

    /// What the horizontal position is measured from (`w:horzAnchor`).
    pub fn horizontal_anchor(&self) -> Option<PositionAnchor> {
        self.horizontal_anchor
    }

    /// What the vertical position is measured from (`w:vertAnchor`).
    pub fn vertical_anchor(&self) -> Option<PositionAnchor> {
        self.vertical_anchor
    }

    /// The horizontal position as an alignment (`w:tblpXSpec`), which wins over
    /// [`x`](Self::x).
    pub fn horizontal_alignment(&self) -> Option<HorizontalAlignment> {
        self.horizontal_alignment
    }

    /// The horizontal position as a distance from the anchor (`w:tblpX`). Word writes
    /// 1 twip for a distance of 0, to avoid the case it ignores, and reads 1 twip back
    /// as 0 (MS-OI29500 §17.4.57(e)); the value here is as written.
    pub fn x(&self) -> Option<Pt> {
        self.x
    }

    /// The vertical position as an alignment (`w:tblpYSpec`), which wins over
    /// [`y`](Self::y).
    pub fn vertical_alignment(&self) -> Option<VerticalAlignment> {
        self.vertical_alignment
    }

    /// The vertical position as a distance from the anchor (`w:tblpY`), as written: 1
    /// twip is a distance of 0 in Word, as for [`x`](Self::x).
    pub fn y(&self) -> Option<Pt> {
        self.y
    }
}

/// The properties of a table as written (`w:tblPr`), or those a row sets for the table
/// around it (`w:tblPrEx`, which writes only the width, alignment, cell spacing,
/// indentation, borders, shading, layout, cell margins and look).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TableProperties {
    pub(crate) style: Option<Box<str>>,
    pub(crate) position: Option<TablePosition>,
    pub(crate) overlap: Option<TableOverlap>,
    pub(crate) right_to_left: Option<bool>,
    pub(crate) row_band_size: Option<i64>,
    pub(crate) column_band_size: Option<i64>,
    pub(crate) width: Option<TableWidth>,
    pub(crate) justification: Option<TableJustification>,
    pub(crate) cell_spacing: Option<TableWidth>,
    pub(crate) indentation: Option<TableWidth>,
    pub(crate) borders: Option<TableBorders>,
    pub(crate) shading: Option<Shading>,
    pub(crate) layout: Option<TableLayout>,
    pub(crate) cell_margins: Option<CellMargins>,
    pub(crate) look: Option<TableLook>,
    pub(crate) caption: Option<Box<str>>,
    pub(crate) description: Option<Box<str>>,
}

impl TableProperties {
    /// The id of the table style (`w:tblStyle`).
    pub fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }

    /// Where the table floats, if it does (`w:tblpPr`).
    pub fn position(&self) -> Option<&TablePosition> {
        self.position.as_ref()
    }

    /// Whether the table, floating, may overlap other floating tables (`w:tblOverlap`).
    pub fn overlap(&self) -> Option<TableOverlap> {
        self.overlap
    }

    /// The columns run from right to left (`w:bidiVisual`).
    pub fn right_to_left(&self) -> Option<bool> {
        self.right_to_left
    }

    /// How many rows make one band of the style's banded rows (`w:tblStyleRowBandSize`).
    pub fn row_band_size(&self) -> Option<i64> {
        self.row_band_size
    }

    /// How many columns make one band of the style's banded columns
    /// (`w:tblStyleColBandSize`).
    pub fn column_band_size(&self) -> Option<i64> {
        self.column_band_size
    }

    /// The preferred width (`w:tblW`).
    pub fn width(&self) -> Option<TableWidth> {
        self.width
    }

    /// The alignment between the margins (`w:jc`).
    pub fn justification(&self) -> Option<TableJustification> {
        self.justification
    }

    /// The space between cells, and between cells and the table's edge
    /// (`w:tblCellSpacing`).
    pub fn cell_spacing(&self) -> Option<TableWidth> {
        self.cell_spacing
    }

    /// The indentation from the start margin (`w:tblInd`). It is the only width Word
    /// expects to be negative (MS-OI29500 §17.4.87(d)); others are kept as written.
    pub fn indentation(&self) -> Option<TableWidth> {
        self.indentation
    }

    /// The borders (`w:tblBorders`).
    pub fn borders(&self) -> Option<&TableBorders> {
        self.borders.as_ref()
    }

    /// The shading of the table's background (`w:shd`).
    pub fn shading(&self) -> Option<&Shading> {
        self.shading.as_ref()
    }

    /// How the column widths are found (`w:tblLayout`).
    pub fn layout(&self) -> Option<TableLayout> {
        self.layout
    }

    /// The default cell margins (`w:tblCellMar`).
    pub fn cell_margins(&self) -> Option<&CellMargins> {
        self.cell_margins.as_ref()
    }

    /// Which parts the table style formats apart (`w:tblLook`).
    pub fn look(&self) -> Option<&TableLook> {
        self.look.as_ref()
    }

    /// The title given for accessibility (`w:tblCaption`).
    pub fn caption(&self) -> Option<&str> {
        self.caption.as_deref()
    }

    /// The description given for accessibility (`w:tblDescription`).
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

/// The height of a row (`w:trHeight`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct RowHeight {
    pub(crate) height: Option<Pt>,
    pub(crate) rule: Option<HeightRule>,
}

impl RowHeight {
    /// The height (`w:val`).
    pub fn height(&self) -> Option<Pt> {
        self.height
    }

    /// How the height applies (`w:hRule`). When none is written the standard assumes
    /// [`HeightRule::Auto`] but Word [`HeightRule::AtLeast`] (MS-OI29500
    /// §17.4.80(a)).
    pub fn rule(&self) -> Option<HeightRule> {
        self.rule
    }
}

/// The properties of a table row as written (`w:trPr`).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TableRowProperties {
    pub(crate) grid_before: Option<i64>,
    pub(crate) grid_after: Option<i64>,
    pub(crate) width_before: Option<TableWidth>,
    pub(crate) width_after: Option<TableWidth>,
    pub(crate) cant_split: Option<bool>,
    pub(crate) height: Option<RowHeight>,
    pub(crate) header: Option<bool>,
    pub(crate) cell_spacing: Option<TableWidth>,
    pub(crate) justification: Option<TableJustification>,
    pub(crate) hidden: Option<bool>,
}

impl TableRowProperties {
    /// How many grid columns are left empty before the first cell (`w:gridBefore`).
    pub fn grid_before(&self) -> Option<i64> {
        self.grid_before
    }

    /// How many grid columns are left empty after the last cell (`w:gridAfter`).
    pub fn grid_after(&self) -> Option<i64> {
        self.grid_after
    }

    /// The preferred width of the space before the first cell (`w:wBefore`).
    pub fn width_before(&self) -> Option<TableWidth> {
        self.width_before
    }

    /// The preferred width of the space after the last cell (`w:wAfter`).
    pub fn width_after(&self) -> Option<TableWidth> {
        self.width_after
    }

    /// The row is kept on one page (`w:cantSplit`).
    pub fn cant_split(&self) -> Option<bool> {
        self.cant_split
    }

    /// The height (`w:trHeight`).
    pub fn height(&self) -> Option<&RowHeight> {
        self.height.as_ref()
    }

    /// The row is a header row, repeated at the top of each page the table continues on
    /// (`w:tblHeader`).
    pub fn header(&self) -> Option<bool> {
        self.header
    }

    /// The space between the row's cells (`w:tblCellSpacing`).
    pub fn cell_spacing(&self) -> Option<TableWidth> {
        self.cell_spacing
    }

    /// The alignment of the row between the margins (`w:jc`).
    pub fn justification(&self) -> Option<TableJustification> {
        self.justification
    }

    /// The row's end mark is hidden (`w:hidden`).
    pub fn hidden(&self) -> Option<bool> {
        self.hidden
    }
}

/// The properties of a table cell as written (`w:tcPr`).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TableCellProperties {
    pub(crate) width: Option<TableWidth>,
    pub(crate) grid_span: Option<i64>,
    pub(crate) horizontal_merge: Option<CellMerge>,
    pub(crate) vertical_merge: Option<CellMerge>,
    pub(crate) borders: Option<CellBorders>,
    pub(crate) shading: Option<Shading>,
    pub(crate) no_wrap: Option<bool>,
    pub(crate) margins: Option<CellMargins>,
    pub(crate) text_direction: Option<TextDirection>,
    pub(crate) fit_text: Option<bool>,
    pub(crate) vertical_justification: Option<VerticalJustification>,
    pub(crate) hide_mark: Option<bool>,
}

impl TableCellProperties {
    /// The preferred width (`w:tcW`).
    pub fn width(&self) -> Option<TableWidth> {
        self.width
    }

    /// How many grid columns the cell spans (`w:gridSpan`).
    pub fn grid_span(&self) -> Option<i64> {
        self.grid_span
    }

    /// The cell's part in a horizontal merge (`w:hMerge`), an older way of writing what
    /// [`grid_span`](Self::grid_span) says.
    pub fn horizontal_merge(&self) -> Option<CellMerge> {
        self.horizontal_merge
    }

    /// The cell's part in a vertical merge (`w:vMerge`); [`CellMerge::Continue`] when
    /// the element has no `w:val`, as the standard says (§17.4.84).
    pub fn vertical_merge(&self) -> Option<CellMerge> {
        self.vertical_merge
    }

    /// The borders (`w:tcBorders`).
    pub fn borders(&self) -> Option<&CellBorders> {
        self.borders.as_ref()
    }

    /// The shading (`w:shd`).
    pub fn shading(&self) -> Option<&Shading> {
        self.shading.as_ref()
    }

    /// The text is not wrapped (`w:noWrap`).
    pub fn no_wrap(&self) -> Option<bool> {
        self.no_wrap
    }

    /// The margins (`w:tcMar`), which win over those of the table.
    pub fn margins(&self) -> Option<&CellMargins> {
        self.margins.as_ref()
    }

    /// The direction of the text (`w:textDirection`).
    pub fn text_direction(&self) -> Option<TextDirection> {
        self.text_direction
    }

    /// The text is squeezed or stretched to fit the width (`w:tcFitText`).
    pub fn fit_text(&self) -> Option<bool> {
        self.fit_text
    }

    /// How the contents are placed between top and bottom (`w:vAlign`).
    pub fn vertical_justification(&self) -> Option<VerticalJustification> {
        self.vertical_justification
    }

    /// The end mark of the cell takes no height when the cell is empty (`w:hideMark`).
    pub fn hide_mark(&self) -> Option<bool> {
        self.hide_mark
    }
}
