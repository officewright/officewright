//! Tables as IR tables: the grid, rows and cells with the properties written on them.
//! Table styles are not applied yet, and a row's table property exceptions are not
//! translated; both are listed.

use officewright_core::Pt;
use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::paragraph::border;
use super::{Step, Translator};
use crate::model::{
    Border, BorderStyle, CellMerge, HeightRule, Table, TableBorders, TableJustification,
    TableLayout, TableRow, TableWidth, TextDirection, VerticalJustification,
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
        let properties = table.properties();
        if properties.style().is_some() {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "the table's style is not applied: table styles are not translated yet".into()
            });
        }
        let margins = cell_margins(properties.cell_margins(), &ir::CellMargins::default());
        let borders = properties.borders();
        let props = ir::TableProps::default()
            .with_width(preferred(properties.width()))
            .with_layout(match properties.layout() {
                Some(TableLayout::Fixed) => ir::TableLayout::Fixed,
                _ => ir::TableLayout::Autofit,
            })
            .with_indent(length(properties.indentation()).unwrap_or_default())
            .with_alignment(match properties.justification() {
                Some(TableJustification::Center) => ir::TableAlignment::Center,
                Some(TableJustification::End | TableJustification::Right) => {
                    ir::TableAlignment::End
                }
                _ => ir::TableAlignment::Start,
            })
            .with_cell_margins(margins)
            // Word gives a cell spacing in percent or auto no space (MS-OI29500
            // §17.4.45(a)).
            .with_cell_spacing(length(properties.cell_spacing()).unwrap_or_default())
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
            .with_floating(properties.position().is_some())
            .with_bidi_visual(properties.right_to_left().unwrap_or(false));
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
            if let Some(row) = self.row(row, &margins) {
                rows.push(row);
            }
            self.path.truncate(self.path.len().saturating_sub(2));
        }
        self.table_depth -= 1;
        if rows.is_empty() {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "a table without rows is left out".into()
            });
            return None;
        }
        Some(
            ir::Table::default()
                .with_props(props)
                .with_grid(grid)
                .with_rows(rows),
        )
    }

    /// A row, or none when it has no cells; that is listed.
    fn row(&mut self, row: TableRow<'a>, margins: &ir::CellMargins) -> Option<ir::Row> {
        let properties = row.properties();
        if row.table_property_exceptions() != Default::default() {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "the row's table property exceptions are not applied".into()
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
        let mut cells: Vec<ir::Cell> = Vec::new();
        for cell in row.cells() {
            self.path
                .extend([Step::Field("cells"), Step::Index(cells.len())]);
            let properties = cell.properties();
            let span = properties
                .grid_span()
                .and_then(|span| u16::try_from(span.max(1)).ok())
                .unwrap_or(1);
            // A cell that continues a horizontal merge widens the cell it continues, as
            // a span does (ECMA-376 Part 1, §17.4.22).
            if properties.horizontal_merge() == Some(CellMerge::Continue) {
                if let Some(last) = cells.last_mut() {
                    let widened = last.grid_span().saturating_add(span);
                    *last = last.clone().with_grid_span(widened);
                    self.path.truncate(self.path.len().saturating_sub(2));
                    continue;
                }
            }
            let written = properties.borders().map(|borders| borders.sides());
            let borders = ir::CellBorders::default()
                .with_top(self.side(written.and_then(TableBorders::top), true))
                .with_start(self.side(written.and_then(TableBorders::start), true))
                .with_bottom(self.side(written.and_then(TableBorders::bottom), true))
                .with_end(self.side(written.and_then(TableBorders::end), true));
            let blocks = self.cell_blocks(cell.blocks());
            cells.push(
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
                    .with_blocks(blocks),
            );
            self.path.truncate(self.path.len().saturating_sub(2));
        }
        if cells.is_empty() {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "a row without cells is left out".into()
            });
            return None;
        }
        Some(
            ir::Row::default()
                .with_height(height)
                .with_cant_split(properties.cant_split().unwrap_or(false))
                .with_header(properties.header().unwrap_or(false))
                .with_grid_before(count(properties.grid_before()))
                .with_width_before(preferred(properties.width_before()))
                .with_grid_after(count(properties.grid_after()))
                .with_width_after(preferred(properties.width_after()))
                .with_cells(cells),
        )
    }

    /// The blocks of a cell, which end with a paragraph: Word adds an empty one after a
    /// table or in an empty cell (MS-OI29500 2.1.168 b).
    fn cell_blocks(
        &mut self,
        blocks: impl Iterator<Item = crate::model::Block<'a>> + 'a,
    ) -> Vec<ir::Block> {
        let mut blocks = self.blocks(blocks);
        if !matches!(blocks.last(), Some(ir::Block::Paragraph(_))) {
            let mark = self.formats.default_mark();
            let mark = self.run_style(&mark);
            blocks.push(ir::Paragraph::new(mark).into());
        }
        blocks
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

/// A preferred width; a width of 0 or `nil` is automatic (MS-OI29500 §17.4.71(a)).
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

/// Cell margins: those written, the others from `below`. A margin written as a
/// percentage or automatic is 0: Word reads it so at the start and end (MS-OI29500
/// 2.1.146 a, 2.1.125 a), as the IR's `CellMargins` describes, and it is taken so at the
/// top and bottom too; `nil` is a width of 0.
fn cell_margins(
    written: Option<&crate::model::CellMargins>,
    below: &ir::CellMargins,
) -> ir::CellMargins {
    let Some(written) = written else {
        return *below;
    };
    let side = |width: Option<TableWidth>, default: Pt| match width {
        None => default,
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
