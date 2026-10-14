//! What table styles give tables, rows and cells (ECMA-376 Part 1, §17.7.6): the style
//! with the styles it is based on, its conditional formats for the parts of the table
//! that the table's `w:tblLook` turns on, and the table's own properties over them.
//!
//! The parts of each table (which rows and cells are its first row, its bands, its
//! corners) are worked out once per table, for all its rows and cells at once, so asking
//! about every cell of a table costs one pass over the table.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Formats, Layer, style_index};
use crate::model::{
    Document, Kind, Style, StyleKind, Table, TableCell, TableCellProperties, TableLook,
    TableProperties, TableRow, TableRowProperties, TableStyleCondition,
};

/// The conditional formats other than the whole table's, in the order Word applies them,
/// later ones over earlier ones (MS-OI29500 §17.7.6.6(a)).
const ORDER: [TableStyleCondition; 12] = [
    TableStyleCondition::OddRowBand,
    TableStyleCondition::EvenRowBand,
    TableStyleCondition::OddColumnBand,
    TableStyleCondition::EvenColumnBand,
    TableStyleCondition::FirstColumn,
    TableStyleCondition::LastColumn,
    TableStyleCondition::FirstRow,
    TableStyleCondition::LastRow,
    TableStyleCondition::TopLeftCell,
    TableStyleCondition::TopRightCell,
    TableStyleCondition::BottomLeftCell,
    TableStyleCondition::BottomRightCell,
];

/// The bit of a condition in a set of conditions: its place in [`ORDER`].
fn bit(condition: TableStyleCondition) -> u16 {
    ORDER
        .iter()
        .position(|&known| known == condition)
        .map_or(0, |place| 1 << place)
}

/// The table, row and cell properties one part of a table style gives.
#[derive(Clone, Debug, Default)]
struct TablePart {
    table: TableProperties,
    row: TableRowProperties,
    cell: TableCellProperties,
}

impl TablePart {
    fn overlay(&mut self, above: &Self) {
        self.table.overlay(&above.table);
        self.row.overlay(&above.row);
        self.cell.overlay(&above.cell);
    }
}

/// What a table style gives with the styles it is based on: its own properties with its
/// whole-table conditional format over them, and each other conditional format, by its
/// place in [`ORDER`].
#[derive(Clone, Debug, Default)]
pub(super) struct TableStyleLevel {
    whole: TablePart,
    conditions: [TablePart; 12],
    /// Whether the style gives paragraph or run properties, which are not applied yet.
    gives_text: bool,
    /// Whether a conditional format gives table properties, which are not applied.
    gives_conditional_table: bool,
}

impl Layer for TableStyleLevel {
    fn of_style(document: &Document, index: usize) -> Self {
        let Some(data) = document.styles.styles.get(index) else {
            return Self::default();
        };
        let style = Style::from_data(document, data);
        if style.kind() != StyleKind::Table {
            return Self::default();
        }
        let mut level = Self {
            whole: TablePart {
                table: style.table_properties().of_table_style(false),
                row: style.row_properties(),
                cell: style.cell_properties().of_table_style(false),
            },
            gives_text: style.paragraph_properties() != Default::default()
                || style.run_properties() != Default::default(),
            ..Self::default()
        };
        // Of two conditional formats of one type, the first counts.
        let mut seen = 0u16;
        let mut whole_seen = false;
        for format in style.conditional_formats() {
            let part = TablePart {
                table: format.table_properties().of_table_style(true),
                row: format.row_properties(),
                cell: format.cell_properties().of_table_style(true),
            };
            level.gives_text |= format.paragraph_properties() != Default::default()
                || format.run_properties() != Default::default();
            if format.condition() == TableStyleCondition::WholeTable {
                if !whole_seen {
                    level.whole.overlay(&part);
                    whole_seen = true;
                }
                continue;
            }
            level.gives_conditional_table |= part.table != TableProperties::default();
            let bit = bit(format.condition());
            if seen & bit != 0 {
                continue;
            }
            seen |= bit;
            if let Some(slot) = ORDER
                .iter()
                .position(|&known| known == format.condition())
                .and_then(|place| level.conditions.get_mut(place))
            {
                slot.overlay(&part);
            }
        }
        level
    }

    fn overlay(&mut self, above: &Self) {
        self.whole.overlay(&above.whole);
        for (below, above) in self.conditions.iter_mut().zip(&above.conditions) {
            below.overlay(above);
        }
        self.gives_text |= above.gives_text;
        self.gives_conditional_table |= above.gives_conditional_table;
    }
}

/// What is worked out once per table: its style's level, its properties in effect, and
/// the conditions each of its rows and cells meets.
#[derive(Debug)]
pub(super) struct TableContext {
    style: Option<Arc<TableStyleLevel>>,
    properties: TableProperties,
    look: TableLook,
    rows: HashMap<u32, u16>,
    /// The conditions each cell meets, and those any cell of its row meets.
    cells: HashMap<u32, (u16, u16)>,
}

/// Word's options when a table has no `w:tblLook`: 0x04A0, the first row and the first
/// column, no vertical bands (MS-OI29500 §17.4.55(a)).
fn default_look() -> TableLook {
    TableLook {
        first_row: Some(true),
        last_row: Some(false),
        first_column: Some(true),
        last_column: Some(false),
        no_horizontal_bands: Some(false),
        no_vertical_bands: Some(true),
    }
}

/// Which band of `size` items, counted from 0, holds the item at `at` among `count`,
/// with `skip_first` and `skip_last` items at the ends out of the bands: none for an item
/// outside the bands or when the size is 0, which Word takes for no bands (MS-OI29500
/// §17.7.6.5, §17.7.6.7).
fn band(at: usize, count: usize, size: i64, skip_first: bool, skip_last: bool) -> Option<usize> {
    let size = usize::try_from(size).ok().filter(|&size| size > 0)?;
    let first = usize::from(skip_first);
    let end = count.checked_sub(usize::from(skip_last))?;
    (at >= first && at < end).then(|| (at - first) / size)
}

/// The odd band (the first, counted from 1) or the even one.
fn band_condition(band: usize, odd: TableStyleCondition, even: TableStyleCondition) -> u16 {
    bit(if band % 2 == 0 { odd } else { even })
}

impl<'a> Formats<'a> {
    /// The table properties in effect: those of the table's style (`w:tblStyle`, else
    /// the default table style) and the styles it is based on, its whole-table
    /// conditional format, then the table's own. Word does not allow some properties in a
    /// style (the width, layout, look, position, overlap and direction, MS-OI29500
    /// §17.7.6.4(a)); they come from the table only.
    pub fn table_properties(&mut self, table: Table<'a>) -> TableProperties {
        let (document, index) = table.place();
        if !std::ptr::eq(document, self.document) {
            return Formats::new(document).table_properties(table);
        }
        self.table_context(index)
            .map_or_else(|| table.properties(), |context| context.properties.clone())
    }

    /// The options of the table's `w:tblLook` in effect, each given: Word's 0x04A0 when
    /// the table has none (MS-OI29500 §17.4.55(a)), and off for an option a `w:tblLook`
    /// leaves out.
    pub fn table_look(&mut self, table: Table<'a>) -> TableLook {
        let (document, index) = table.place();
        if !std::ptr::eq(document, self.document) {
            return Formats::new(document).table_look(table);
        }
        self.table_context(index)
            .map_or_else(default_look, |context| context.look)
    }

    /// The row properties in effect: the table style's, those of the conditional
    /// formats for the parts of the table the row is in (its bands, the first or the
    /// last row), then the row's own.
    pub fn row_properties(&mut self, row: TableRow<'a>) -> TableRowProperties {
        let (document, index) = row.place();
        if !std::ptr::eq(document, self.document) {
            return Formats::new(document).row_properties(row);
        }
        let own = row.properties();
        let Some(context) = self.context_of(index) else {
            return own;
        };
        let Some(style) = &context.style else {
            return own;
        };
        let conditions = context.rows.get(&index).copied().unwrap_or(0);
        let mut properties = style.whole.row.clone();
        for (place, part) in style.conditions.iter().enumerate() {
            if conditions & (1 << place) != 0 {
                properties.overlay(&part.row);
            }
        }
        properties.overlay(&own);
        properties
    }

    /// The cell properties in effect: the table style's, those of the conditional
    /// formats for the parts of the table the cell is in, in the order Word applies them
    /// (MS-OI29500 §17.7.6.6(a)), then the cell's own. A conditional format's no
    /// wrapping, margins and vertical alignment apply to every cell of a row one of whose
    /// cells it applies to (MS-OI29500 §17.7.6.8(a)).
    pub fn cell_properties(&mut self, cell: TableCell<'a>) -> TableCellProperties {
        let (document, index) = cell.place();
        if !std::ptr::eq(document, self.document) {
            return Formats::new(document).cell_properties(cell);
        }
        let own = cell.properties();
        let Some(context) = self.context_of(index) else {
            return own;
        };
        let Some(style) = &context.style else {
            return own;
        };
        let (conditions, row) = context.cells.get(&index).copied().unwrap_or((0, 0));
        let mut properties = style.whole.cell.clone();
        for (place, part) in style.conditions.iter().enumerate() {
            if conditions & (1 << place) != 0 {
                properties.overlay(&part.cell);
            } else if row & (1 << place) != 0 {
                properties.overlay(&part.cell.of_whole_rows());
            }
        }
        properties.overlay(&own);
        properties
    }

    /// The parts of its table a cell is in, which select the table style's conditional
    /// formats, in the order Word applies them; the whole table is not listed.
    pub fn cell_conditions(&mut self, cell: TableCell<'a>) -> Vec<TableStyleCondition> {
        let (document, index) = cell.place();
        if !std::ptr::eq(document, self.document) {
            return Formats::new(document).cell_conditions(cell);
        }
        let conditions = self
            .context_of(index)
            .and_then(|context| context.cells.get(&index).map(|&(cell, _)| cell))
            .unwrap_or(0);
        ORDER
            .iter()
            .enumerate()
            .filter(|(place, _)| conditions & (1 << place) != 0)
            .map(|(_, &condition)| condition)
            .collect()
    }

    /// Whether the table's style gives paragraph or run properties, or conditional table
    /// properties, which the formatting in effect does not take yet.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn table_style_gives_unapplied(&mut self, table: Table<'a>) -> (bool, bool) {
        let (_, index) = table.place();
        self.table_context(index)
            .and_then(|context| context.style.clone())
            .map_or((false, false), |style| {
                (style.gives_text, style.gives_conditional_table)
            })
    }

    /// The context of the table a row or cell belongs to.
    fn context_of(&mut self, node: u32) -> Option<Arc<TableContext>> {
        if let Some(&table) = self.table_of.get(&node) {
            return self.table_context(table);
        }
        // Up to the table, through the containers between; once per table, since the
        // table's context then knows all its rows and cells.
        let document = self.document;
        let mut current = document.node(node)?;
        for _ in 0..document.nodes.len() {
            let parent = current.parent;
            let next = document.node(parent)?;
            if matches!(next.kind, Kind::Table { .. }) {
                let context = self.table_context(parent);
                return context.filter(|context| {
                    context.rows.contains_key(&node) || context.cells.contains_key(&node)
                });
            }
            current = next;
        }
        None
    }

    /// The context of the table at `index`, worked out the first time.
    fn table_context(&mut self, index: u32) -> Option<Arc<TableContext>> {
        if let Some(context) = self.tables.get(&index) {
            return Some(Arc::clone(context));
        }
        let document = self.document;
        let table = Table::at(document, index)?;
        #[cfg(test)]
        {
            self.work.tables += 1;
        }
        let own = table.properties();
        let style = own
            .style()
            .and_then(|id| style_index(document, id, StyleKind::Table))
            .or_else(|| self.default_table_style())
            .map(|style| {
                self.table_styles
                    .level(document, style, &TableStyleLevel::default())
            });
        let mut properties = style
            .as_ref()
            .map(|style| style.whole.table.clone())
            .unwrap_or_default();
        properties.overlay(&own);
        let look = properties.look.map_or_else(default_look, |look| TableLook {
            first_row: Some(look.first_row.unwrap_or(false)),
            last_row: Some(look.last_row.unwrap_or(false)),
            first_column: Some(look.first_column.unwrap_or(false)),
            last_column: Some(look.last_column.unwrap_or(false)),
            no_horizontal_bands: Some(look.no_horizontal_bands.unwrap_or(false)),
            no_vertical_bands: Some(look.no_vertical_bands.unwrap_or(false)),
        });
        let on = |option: Option<bool>| option.unwrap_or(false);
        let (first_row, last_row) = (on(look.first_row), on(look.last_row));
        let (first_column, last_column) = (on(look.first_column), on(look.last_column));
        let row_band = if on(look.no_horizontal_bands) {
            0
        } else {
            properties.row_band_size.unwrap_or(0)
        };
        let column_band = if on(look.no_vertical_bands) {
            0
        } else {
            properties.column_band_size.unwrap_or(0)
        };
        let rows: Vec<TableRow<'a>> = table.rows().collect();
        let mut row_conditions = HashMap::new();
        let mut cell_conditions = HashMap::new();
        for (at, row) in rows.iter().enumerate() {
            let top = at == 0;
            let bottom = at + 1 == rows.len();
            let mut conditions = 0;
            if first_row && top {
                conditions |= bit(TableStyleCondition::FirstRow);
            }
            if last_row && bottom {
                conditions |= bit(TableStyleCondition::LastRow);
            }
            if let Some(band) = band(at, rows.len(), row_band, first_row, last_row) {
                conditions |= band_condition(
                    band,
                    TableStyleCondition::OddRowBand,
                    TableStyleCondition::EvenRowBand,
                );
            }
            let (_, row_index) = row.place();
            row_conditions.insert(row_index, conditions);
            self.table_of.insert(row_index, index);
            let cells: Vec<TableCell<'a>> = row.cells().collect();
            let mut in_row = Vec::with_capacity(cells.len());
            for (column, cell) in cells.iter().enumerate() {
                let left = column == 0;
                let right = column + 1 == cells.len();
                let mut conditions = conditions;
                if first_column && left {
                    conditions |= bit(TableStyleCondition::FirstColumn);
                }
                if last_column && right {
                    conditions |= bit(TableStyleCondition::LastColumn);
                }
                if let Some(band) =
                    band(column, cells.len(), column_band, first_column, last_column)
                {
                    conditions |= band_condition(
                        band,
                        TableStyleCondition::OddColumnBand,
                        TableStyleCondition::EvenColumnBand,
                    );
                }
                // A corner cell is styled on its own when both its row and its column are
                // (MS-OI29500 §17.4.55(b)).
                for (corner, row_on, column_on) in [
                    (
                        TableStyleCondition::TopLeftCell,
                        first_row && top,
                        first_column && left,
                    ),
                    (
                        TableStyleCondition::TopRightCell,
                        first_row && top,
                        last_column && right,
                    ),
                    (
                        TableStyleCondition::BottomLeftCell,
                        last_row && bottom,
                        first_column && left,
                    ),
                    (
                        TableStyleCondition::BottomRightCell,
                        last_row && bottom,
                        last_column && right,
                    ),
                ] {
                    if row_on && column_on {
                        conditions |= bit(corner);
                    }
                }
                let (_, cell_index) = cell.place();
                in_row.push((cell_index, conditions));
                self.table_of.insert(cell_index, index);
            }
            let union = in_row
                .iter()
                .fold(0, |union, (_, conditions)| union | conditions);
            for (cell_index, conditions) in in_row {
                cell_conditions.insert(cell_index, (conditions, union));
            }
        }
        let context = Arc::new(TableContext {
            style,
            properties,
            look,
            rows: row_conditions,
            cells: cell_conditions,
        });
        self.tables.insert(index, Arc::clone(&context));
        Some(context)
    }

    /// The index of the default table style, once looked up.
    fn default_table_style(&mut self) -> Option<usize> {
        let document = self.document;
        *self.default_table_style.get_or_insert_with(|| {
            document
                .styles
                .styles
                .iter()
                .take(super::MAX_STYLES)
                .position(|style| style.kind == StyleKind::Table && style.is_default)
        })
    }
}
