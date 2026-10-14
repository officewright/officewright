//! Tables as IR tables: the grid, rows and cells with their properties in effect, table
//! styles and their conditional formats applied. What a table style gives paragraphs and
//! runs, the table properties of its conditional formats, a row's table property
//! exceptions, and the properties the IR has no place for yet are not applied; each is
//! listed.

use officewright_core::Pt;
use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::paragraph::border;
use super::{Step, Translator};
use crate::model::{
    Block, Border, BorderStyle, CellMerge, HeightRule, PositionAnchor, Table, TableBorders,
    TableCell, TableCellProperties, TableJustification, TableLayout, TablePosition, TableRow,
    TableWidth, TextDirection, VerticalJustification,
};

impl<'a> Translator<'a> {
    /// A table, or none when it has no rows or is nested deeper than the IR allows; both
    /// are listed.
    pub(super) fn table(&mut self, table: Table<'a>) -> Option<ir::Table> {
        if self.table_depth >= ir::MAX_TABLE_DEPTH {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                format!(
                    "a table nested more than {} deep is left out",
                    ir::MAX_TABLE_DEPTH
                )
            });
            return None;
        }
        // What is built before and after the rows is built in functions of their own, so
        // that its temporaries take no room in this frame, which every nested table
        // repeats (#187).
        let props = self.table_props(table);
        // The reader reads w:gridCol as ST_TwipsMeasure, which has no negative widths.
        let grid: Vec<Pt> = table
            .grid_columns()
            .map(Option::unwrap_or_default)
            .collect();
        self.table_depth += 1;
        let mut rows = Vec::new();
        for row in table.rows() {
            self.path
                .extend([Step::Field("rows"), Step::Index(rows.len())]);
            if let Some(row) = self.row(row, &props) {
                rows.push(row);
            }
            self.path.truncate(self.path.len().saturating_sub(2));
        }
        self.table_depth -= 1;
        // A vector keeps the room it grew to; a long table would keep it.
        rows.shrink_to_fit();
        if rows.is_empty() {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "a table without rows is left out".into()
            });
            return None;
        }
        Some(new_table(props, grid, rows))
    }

    /// The properties of a table, its style listed when it has one.
    #[inline(never)]
    fn table_props(&mut self, table: Table<'a>) -> ir::TableProps {
        let properties = self.formats.table_properties(table);
        let (text, conditional_table) = self.formats.table_style_gives_unapplied(table);
        if text {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "what the table's style gives paragraphs and runs is not applied yet".into()
            });
        }
        if conditional_table {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "the table properties of the table style's conditional formats are not applied"
                    .into()
            });
        }
        let margins = cell_margins(properties.cell_margins(), &ir::CellMargins::default());
        let borders = properties.borders();
        ir::TableProps::default()
            .with_width(preferred(properties.width()))
            .with_layout(match properties.layout() {
                Some(TableLayout::Fixed) => ir::TableLayout::Fixed,
                _ => ir::TableLayout::Autofit,
            })
            .with_indent(length(properties.indentation()).unwrap_or_default())
            .with_alignment(alignment(properties.justification()))
            .with_cell_margins(margins)
            .with_cell_spacing(spacing(properties.cell_spacing()))
            .with_borders(
                ir::TableBorders::default()
                    .with_top(self.side(borders.and_then(TableBorders::top), false))
                    .with_start(self.side(borders.and_then(TableBorders::start), false))
                    .with_bottom(self.side(borders.and_then(TableBorders::bottom), false))
                    .with_end(self.side(borders.and_then(TableBorders::end), false))
                    .with_inside_horizontal(
                        self.side(borders.and_then(TableBorders::inside_horizontal), false),
                    )
                    .with_inside_vertical(
                        self.side(borders.and_then(TableBorders::inside_vertical), false),
                    ),
            )
            .with_shading(
                properties
                    .shading()
                    .and_then(|shading| self.shading(shading)),
            )
            // Word ignores a w:tblpPr in a note (MS-OI29500 §17.4.57(b)).
            .with_floating(!self.in_note && floats(properties.position()))
            .with_bidi_visual(properties.right_to_left().unwrap_or(false))
    }

    /// A row, or none when it has no cells; that is listed.
    fn row(&mut self, row: TableRow<'a>, table: &ir::TableProps) -> Option<ir::Row> {
        let head = self.row_head(row, table);
        let mut cells: Vec<ir::Cell> = Vec::new();
        // The last cell, kept out of `cells` while the cells after it may widen it.
        let mut open: Option<ir::Cell> = None;
        // In tests, where the last cell's blocks were built.
        #[cfg(test)]
        let mut built_at = 0;
        for cell in row.cells() {
            let (span, continues, properties) = self.merge(cell);
            // A cell that continues a horizontal merge (`w:hMerge`, ISO/IEC 29500-4
            // §14.4.13) widens the cell before it, as a span does. The widened cell is
            // moved, never copied: a copy per continuing cell costs the content times
            // the cells.
            if continues {
                if let Some(last) = open.take() {
                    let widened = last.grid_span().saturating_add(span);
                    open = Some(last.with_grid_span(widened));
                    #[cfg(test)]
                    if open
                        .as_ref()
                        .is_some_and(|cell| cell.blocks().as_ptr().addr() != built_at)
                    {
                        super::work(|work| work.cell_copies += 1);
                    }
                    if has_content(cell) {
                        self.path
                            .extend([Step::Field("cells"), Step::Index(cells.len())]);
                        self.leave_out(ReportKind::FeatureUnsupported, || {
                            "the content of a cell that continues a horizontal merge is left out"
                                .into()
                        });
                        self.path.truncate(self.path.len().saturating_sub(2));
                    }
                    continue;
                }
            }
            cells.extend(open.take());
            self.path
                .extend([Step::Field("cells"), Step::Index(cells.len())]);
            let built = self.cell(cell, &properties, span, &table.cell_margins());
            #[cfg(test)]
            {
                built_at = built.blocks().as_ptr().addr();
            }
            open = Some(built);
            self.path.truncate(self.path.len().saturating_sub(2));
        }
        cells.extend(open);
        cells.shrink_to_fit();
        if cells.is_empty() {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "a row without cells is left out".into()
            });
            return None;
        }
        Some(head.with_cells(cells))
    }

    /// A row's properties as an IR row without its cells; what the IR has no place for
    /// is listed.
    #[inline(never)]
    fn row_head(&mut self, row: TableRow<'a>, table: &ir::TableProps) -> ir::Row {
        let properties = self.formats.row_properties(row);
        if row.table_property_exceptions() != Default::default() {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "the row's table property exceptions are not applied".into()
            });
        }
        // The IR's row has no alignment or cell spacing of its own: Word writes the
        // table's into every row it saves, so only a row whose own differ is listed.
        if properties
            .justification()
            .is_some_and(|justification| alignment(Some(justification)) != table.alignment())
        {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "the row's own alignment is not applied; the table's is used".into()
            });
        }
        if properties
            .cell_spacing()
            .is_some_and(|written| spacing(Some(written)) != table.cell_spacing())
        {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "the row's own cell spacing is not applied; the table's is used".into()
            });
        }
        if properties.hidden() == Some(true) {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "the row's hidden end mark (w:hidden) is not applied".into()
            });
        }
        let height = match properties.height() {
            Some(height) => match (height.height(), height.rule()) {
                (None, _) | (_, Some(HeightRule::Auto)) => ir::RowHeight::Auto,
                (Some(value), Some(HeightRule::Exact)) => ir::RowHeight::Exact(value),
                // Without a rule, at least (MS-OI29500 §17.4.80(a)).
                (Some(value), _) => ir::RowHeight::AtLeast(value),
            },
            None => ir::RowHeight::Auto,
        };
        // A negative count, or one too large, is 0.
        let count = |value: Option<i64>| {
            value
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(0)
        };
        ir::Row::default()
            .with_height(height)
            .with_cant_split(properties.cant_split().unwrap_or(false))
            .with_header(properties.header().unwrap_or(false))
            .with_grid_before(count(properties.grid_before()))
            .with_width_before(preferred(properties.width_before()))
            .with_grid_after(count(properties.grid_after()))
            .with_width_after(preferred(properties.width_after()))
    }

    /// A cell, with its properties in effect and the span it has so far.
    fn cell(
        &mut self,
        cell: TableCell<'a>,
        properties: &TableCellProperties,
        span: u16,
        margins: &ir::CellMargins,
    ) -> ir::Cell {
        let head = self.cell_head(properties, span, margins);
        let blocks = self.cell_blocks(cell.blocks());
        head.with_blocks(blocks)
    }

    /// A cell's properties as an IR cell without its blocks; what the IR has no place
    /// for is listed.
    #[inline(never)]
    fn cell_head(
        &mut self,
        properties: &TableCellProperties,
        span: u16,
        margins: &ir::CellMargins,
    ) -> ir::Cell {
        if properties.fit_text() == Some(true) {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "the cell's text fitting (w:tcFitText) is not applied".into()
            });
        }
        let drawn = |border: Option<&Border>| {
            border.is_some_and(|border| {
                !matches!(
                    border.style(),
                    None | Some(BorderStyle::None | BorderStyle::Nil)
                )
            })
        };
        if properties.borders().is_some_and(|borders| {
            drawn(borders.top_left_to_bottom_right()) || drawn(borders.top_right_to_bottom_left())
        }) {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "the cell's diagonal borders are not drawn".into()
            });
        }
        // A cell has no inner edges for its own w:insideH and w:insideV (ISO/IEC 29500-1,
        // insideH of tcBorders); a table style's reach the cell's sides through the groups
        // of cells its conditional formats apply to (`Formats::cell_properties`).
        let written = properties.borders().map(|borders| borders.sides());
        let borders = ir::CellBorders::default()
            .with_top(self.side(written.and_then(TableBorders::top), true))
            .with_start(self.side(written.and_then(TableBorders::start), true))
            .with_bottom(self.side(written.and_then(TableBorders::bottom), true))
            .with_end(self.side(written.and_then(TableBorders::end), true));
        ir::Cell::default()
            .with_width(preferred(properties.width()))
            .with_grid_span(span)
            .with_vertical_merge(match properties.vertical_merge() {
                Some(CellMerge::Restart) => ir::VerticalMerge::Restart,
                Some(CellMerge::Continue) => ir::VerticalMerge::Continue,
                None => ir::VerticalMerge::None,
            })
            // The sides a cell leaves out come from the table.
            .with_margins(
                properties
                    .margins()
                    .map(|written| cell_margins(Some(written), margins)),
            )
            .with_borders(borders)
            .with_shading(
                properties
                    .shading()
                    .and_then(|shading| self.shading(shading)),
            )
            .with_vertical_align(match properties.vertical_justification() {
                Some(VerticalJustification::Center) => ir::CellVerticalAlign::Center,
                Some(VerticalJustification::Bottom) => ir::CellVerticalAlign::Bottom,
                // Word does not take both in a cell (MS-OI29500 §17.4.83(a)).
                _ => ir::CellVerticalAlign::Top,
            })
            .with_no_wrap(properties.no_wrap().unwrap_or(false))
            .with_hide_mark(properties.hide_mark().unwrap_or(false))
            .with_text_direction(text_direction(properties.text_direction()))
    }

    /// The blocks of a cell, which end with a paragraph, as the IR asks. Word refuses a
    /// document with a cell that does not (MS-OI29500 2.1.168 b); the translation adds
    /// an empty paragraph, with the mark of the default paragraph style, and lists it.
    fn cell_blocks(&mut self, blocks: impl Iterator<Item = Block<'a>> + 'a) -> Vec<ir::Block> {
        let mut blocks = self.blocks(blocks);
        if !matches!(blocks.last(), Some(ir::Block::Paragraph(_))) {
            blocks.push(self.added_paragraph());
        }
        // Most cells hold one paragraph; a vector keeps room for four after its first
        // push.
        blocks.shrink_to_fit();
        blocks
    }

    /// A cell's span in effect, 1 for none or one out of range, whether it continues a
    /// horizontal merge, and its properties in effect, read once for the cell. They are
    /// boxed, so that they take no room in the frames of the cells' translation, which
    /// recurses.
    #[inline(never)]
    fn merge(&mut self, cell: TableCell<'a>) -> (u16, bool, Box<TableCellProperties>) {
        let properties = Box::new(self.formats.cell_properties(cell));
        let span = properties
            .grid_span()
            .and_then(|span| u16::try_from(span.max(1)).ok())
            .unwrap_or(1);
        let continues = properties.horizontal_merge() == Some(CellMerge::Continue);
        (span, continues, properties)
    }

    /// The empty paragraph added to a cell that does not end with one, listed; built
    /// here, so that the format of its mark takes no room in the frames of the cells'
    /// translation, which recurses.
    #[inline(never)]
    fn added_paragraph(&mut self) -> ir::Block {
        self.leave_out(ReportKind::ValueClamped, || {
            "a cell that does not end with a paragraph gets an empty one".into()
        });
        let mark = self.formats.default_mark();
        let mark = self.run_style(&mark);
        ir::Paragraph::new(mark).into()
    }

    /// A border of a table or a cell: none written is `Inherit`. Word looks a cell's
    /// border up in the table when the cell's own is omitted or of style none
    /// (MS-OI29500 2.1.169 a), so a cell's `none` is `Inherit` too; `nil` is no border,
    /// not even the adjacent cell's.
    fn side(&mut self, written: Option<&Border>, in_cell: bool) -> ir::BorderSide {
        let Some(written) = written else {
            return ir::BorderSide::Inherit;
        };
        match written.style() {
            None => ir::BorderSide::Inherit,
            Some(BorderStyle::None) if in_cell => ir::BorderSide::Inherit,
            Some(BorderStyle::None) => ir::BorderSide::None,
            Some(BorderStyle::Nil) => ir::BorderSide::Nil,
            Some(_) => border(self, written).map_or(ir::BorderSide::None, ir::BorderSide::Line),
        }
    }
}

/// A table of these parts; built here, so that its temporaries take no room in the
/// frame of [`Translator::table`], which recurses.
#[inline(never)]
fn new_table(props: ir::TableProps, grid: Vec<Pt>, rows: Vec<ir::Row>) -> ir::Table {
    ir::Table::default()
        .with_props(props)
        .with_grid(grid)
        .with_rows(rows)
}

/// A preferred width of a table, a cell or the space before or after a row: automatic
/// when it is written so, as `nil` or as 0 (MS-OI29500 §17.4.63(a), (b) for a table's,
/// §17.4.71(a), (b) for a cell's), or not written; a negative one, which Word does not
/// expect (MS-OI29500 §17.4.87(d)), is automatic too.
fn preferred(width: Option<TableWidth>) -> ir::PreferredWidth {
    match width {
        Some(TableWidth::Length(length)) if length.0 > 0.0 => ir::PreferredWidth::Points(length),
        Some(TableWidth::Percent(percent)) if percent > 0.0 => ir::PreferredWidth::Percent(percent),
        _ => ir::PreferredWidth::Auto,
    }
}

/// A width in points; none for automatic, `nil` and percentages.
fn length(width: Option<TableWidth>) -> Option<Pt> {
    match width {
        Some(TableWidth::Length(length)) => Some(length),
        _ => None,
    }
}

/// A table's alignment between the edges of its column.
fn alignment(justification: Option<TableJustification>) -> ir::TableAlignment {
    match justification {
        Some(TableJustification::Center) => ir::TableAlignment::Center,
        Some(TableJustification::End | TableJustification::Right) => ir::TableAlignment::End,
        _ => ir::TableAlignment::Start,
    }
}

/// A cell spacing: Word gives one in percent or auto no space (MS-OI29500 §17.4.45(a)),
/// and expects every table measurement but the indent to be at least 0 (MS-OI29500
/// §17.4.87(d)), so a negative one is 0.
fn spacing(width: Option<TableWidth>) -> Pt {
    length(width).map_or(Pt(0.0), |length| Pt(length.0.max(0.0)))
}

/// Whether a table floats. Word ignores a `w:tblpPr` with `tblpX` and `tblpY` 0 whose
/// horizontal anchor is the text and vertical anchor is not; without anchors it takes
/// the text horizontally and the margin vertically, so a `w:tblpPr` without attributes,
/// which Word ignores too, is such a one (MS-OI29500 §17.4.57(b), (c)). Word also
/// ignores one in a note, which the caller sees to, and in a text box or a comment,
/// whose tables this translation does not reach yet.
fn floats(position: Option<&TablePosition>) -> bool {
    let Some(position) = position else {
        return false;
    };
    // A distance as written, 0 when it is not: Word writes 1 twip for 0 to avoid this
    // case (§17.4.57(e)). A position given as an alignment (`tblpXSpec`, `tblpYSpec`),
    // which wins over the distance, is no distance of 0: Word 2007 writes tables placed
    // at the bottom of the margin so (n779642.docx of LibreOffice's tests), the margin
    // being what Word measures from without a vertical anchor (§17.4.57(c)).
    let at_zero = |distance: Option<Pt>, aligned: bool| {
        !aligned && distance.is_none_or(|distance| distance.0 == 0.0)
    };
    let ignored = at_zero(position.x(), position.horizontal_alignment().is_some())
        && at_zero(position.y(), position.vertical_alignment().is_some())
        && position.horizontal_anchor().unwrap_or(PositionAnchor::Text) == PositionAnchor::Text
        && position.vertical_anchor().unwrap_or(PositionAnchor::Margin) != PositionAnchor::Text;
    !ignored
}

/// Whether a cell holds anything but a single empty paragraph.
fn has_content(cell: TableCell<'_>) -> bool {
    let mut blocks = cell.blocks();
    match (blocks.next(), blocks.next()) {
        (None, _) => false,
        (Some(Block::Paragraph(paragraph)), None) => paragraph.inlines().next().is_some(),
        _ => true,
    }
}

/// Cell margins: those written, the others from `below`: the table's for a cell, the
/// IR's defaults, which are Word's, for a table.
///
/// A margin of the table written as a percentage or automatic is 0: Word reads it so at
/// the start and end (MS-OI29500 2.1.146 a, 2.1.125 a), as the IR's `CellMargins`
/// describes, and it is taken so at the top and bottom too. A cell's own margin in
/// percent or auto is taken as 0 as well, which no note confirms yet. Word ignores a
/// side of a cell's `w:tcMar` of type `nil` (MS-OI29500 §17.4.2(a), §17.4.10(a),
/// §17.4.77(a), and the same note for the start), so that side comes from the table.
/// The notes on the table's top and bottom margins, and the one on every table
/// measurement, point to the same rule (§17.4.75, §17.4.5, §17.4.87), so a table's
/// `nil` is taken as not written on every side; at the start and end, whose notes speak
/// only of percent and auto, a Word render is to confirm it (#172).
fn cell_margins(
    written: Option<&crate::model::CellMargins>,
    below: &ir::CellMargins,
) -> ir::CellMargins {
    let Some(written) = written else {
        return *below;
    };
    let side = |width: Option<TableWidth>, default: Pt| match width {
        None | Some(TableWidth::Nil) => default,
        Some(TableWidth::Length(length)) => Pt(length.0.max(0.0)),
        Some(_) => Pt(0.0),
    };
    ir::CellMargins::default()
        .with_top(side(written.top(), below.top()))
        .with_start(side(written.start(), below.start()))
        .with_bottom(side(written.bottom(), below.bottom()))
        .with_end(side(written.end(), below.end()))
}

fn text_direction(direction: Option<TextDirection>) -> ir::TextDirection {
    match direction {
        None | Some(TextDirection::LeftToRightTopToBottom) => {
            ir::TextDirection::LeftToRightTopToBottom
        }
        Some(TextDirection::TopToBottomRightToLeft) => ir::TextDirection::TopToBottomRightToLeft,
        Some(TextDirection::BottomToTopLeftToRight) => ir::TextDirection::BottomToTopLeftToRight,
        Some(TextDirection::LeftToRightTopToBottomRotated) => {
            ir::TextDirection::LeftToRightTopToBottomRotated
        }
        Some(TextDirection::TopToBottomRightToLeftRotated) => {
            ir::TextDirection::TopToBottomRightToLeftRotated
        }
        Some(TextDirection::TopToBottomLeftToRightRotated) => {
            ir::TextDirection::TopToBottomLeftToRightRotated
        }
    }
}
