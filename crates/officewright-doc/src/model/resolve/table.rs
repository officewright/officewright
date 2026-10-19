//! What table styles give tables, rows and cells (ECMA-376 Part 1, §17.7.6): the style
//! with the styles it is based on, its conditional formats for the parts of the table
//! that the table's `w:tblLook` turns on, and the table's own properties over them.
//!
//! The parts of each table (which rows and cells are its first row, its bands, its
//! corners) are worked out once per table, for all its rows and cells at once, so asking
//! about every cell of a table costs one pass over the table.

use std::collections::HashMap;
use std::sync::Arc;

use super::character::CharacterLevel;
use super::paragraph::ParagraphLevel;
use super::{Formats, Layer, style_index};
use crate::model::table_properties::GroupEdges;
use crate::model::{
    Document, Kind, Node, Style, StyleKind, Table, TableCell, TableCellProperties, TableLook,
    TableProperties, TableRow, TableRowProperties, TableStyleCondition,
};

/// The conditional formats Word applies, in the order it applies them, later ones over
/// earlier ones (MS-OI29500 §17.7.6.6(a)); the whole table's it ignores (§17.18.89(a)).
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

/// The table, row and cell properties one part of a table style gives, and what it
/// gives the paragraphs and runs of its cells.
#[derive(Clone, Debug, Default)]
struct TablePart {
    table: TableProperties,
    row: TableRowProperties,
    cell: TableCellProperties,
    paragraph: ParagraphLevel,
    character: CharacterLevel,
}

impl TablePart {
    fn overlay(&mut self, above: &Self) {
        self.table.overlay(&above.table);
        self.row.overlay(&above.row);
        self.cell.overlay(&above.cell);
        self.paragraph.overlay(&above.paragraph);
        self.character.overlay(&above.character);
    }
}

/// What a table style gives the paragraphs and runs of the cells that meet a set of its
/// conditions: the style's own, then its conditional formats', in Word's order
/// ([`ORDER`]). In the hierarchy of a paragraph in a cell it lies between the document
/// defaults and the paragraph's list level and style (ECMA-376 Part 1, §17.7.2).
#[derive(Debug)]
pub(super) struct TableText {
    /// Tells the layers apart, for the caches of what lies over them.
    pub(super) id: u64,
    pub(super) paragraph: ParagraphLevel,
    pub(super) character: CharacterLevel,
}

/// What a table style gives with the styles it is based on: its own properties, and each
/// conditional format it has, by its place in [`ORDER`], boxed so that a style with few
/// formats takes little room.
#[derive(Clone, Debug, Default)]
pub(super) struct TableStyleLevel {
    whole: TablePart,
    conditions: [Option<Box<TablePart>>; 12],
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
                row: style.row_properties().of_table_style(),
                cell: style.cell_properties().of_table_style(false),
                paragraph: ParagraphLevel::of_table_style(&style.paragraph_properties()),
                character: CharacterLevel::of(&style.run_properties()),
            },
            ..Self::default()
        };
        // Of two conditional formats of one type, the first counts, as for elements
        // written twice elsewhere; our choice, not yet confirmed against Word (#172).
        let mut seen = 0u16;
        for format in style.conditional_formats() {
            // Word neither applies nor keeps a whole-table conditional format (MS-OI29500
            // §17.18.89(a)).
            if format.condition() == TableStyleCondition::WholeTable {
                continue;
            }
            let bit = bit(format.condition());
            if seen & bit != 0 {
                continue;
            }
            seen |= bit;
            let part = TablePart {
                table: format.table_properties().of_table_style(true),
                row: format.row_properties().of_table_style(),
                cell: format.cell_properties().of_table_style(true),
                paragraph: ParagraphLevel::of_table_style(&format.paragraph_properties()),
                character: CharacterLevel::of(&format.run_properties()),
            };
            level.gives_conditional_table |= part.table != TableProperties::default();
            if let Some(slot) = ORDER
                .iter()
                .position(|&known| known == format.condition())
                .and_then(|place| level.conditions.get_mut(place))
            {
                *slot = Some(Box::new(part));
            }
        }
        level
    }

    fn overlay(&mut self, above: &Self) {
        self.whole.overlay(&above.whole);
        for (below, above) in self.conditions.iter_mut().zip(&above.conditions) {
            if let Some(above) = above {
                below.get_or_insert_with(Box::default).overlay(above);
            }
        }
        self.gives_conditional_table |= above.gives_conditional_table;
    }
}

/// What is worked out once per table: its style's level, its properties in effect, and
/// the conditions each of its rows and cells meets.
#[derive(Debug)]
pub(super) struct TableContext {
    style: Option<Arc<TableStyleLevel>>,
    /// The place of the table's style among the styles.
    style_index: Option<usize>,
    properties: TableProperties,
    look: TableLook,
    rows: HashMap<u32, u16>,
    cells: HashMap<u32, CellPlace>,
}

/// Where a cell is among the parts of its table.
#[derive(Clone, Copy, Debug, Default)]
struct CellPlace {
    /// The conditions the cell meets.
    conditions: u16,
    /// The conditions any cell of its row meets.
    row: u16,
    /// For each condition the cell meets, by its place in [`ORDER`], 4 bits: whether the
    /// cell's top, bottom, start and end lie on the edges of the condition's group of
    /// cells.
    edges: u64,
}

const TOP: u64 = 1;
const BOTTOM: u64 = 2;
const START: u64 = 4;
const END: u64 = 8;

impl CellPlace {
    fn meets(&mut self, condition: TableStyleCondition, edges: u64) {
        let bit = bit(condition);
        self.conditions |= bit;
        self.edges |= edges << (4 * bit.trailing_zeros());
    }

    fn edges(self, place: usize) -> GroupEdges {
        let edges = self.edges >> (4 * place);
        GroupEdges {
            top: edges & TOP != 0,
            bottom: edges & BOTTOM != 0,
            start: edges & START != 0,
            end: edges & END != 0,
        }
    }
}

/// How many [`TableText`]s the resolver keeps at a time, with the style chains laid over
/// them: one per table style and set of conditions that cells use, so a few per table
/// style in practice.
const MAX_TABLE_TEXTS: usize = 256;

/// The cell a paragraph is in, the innermost; none for one in no cell.
fn cell_of(document: &Document, node: &Node) -> Option<u32> {
    let mut current = node;
    for _ in 0..document.nodes.len() {
        let parent = document.node(current.parent)?;
        match parent.kind {
            Kind::Cell => return Some(current.parent),
            Kind::Table { .. } | Kind::Row => return None,
            _ => current = parent,
        }
    }
    None
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

/// The band of `size` items, counted from 0, that holds the item at `at` among `count`,
/// and whether the item is the band's first and its last; with the first `skip_first`
/// items and, with `skip_last`, the last one out of the bands. None for an item outside
/// the bands or when the size is 0, which Word takes for no bands (MS-OI29500
/// §17.7.6.5(c), §17.7.6.7(b)). Word leaves the first row and the first column out of
/// the bands when they are formatted on their own (word-rules/E-table-style-look.docx,
/// cases 01 and 04), and every header row (E-table-style-header-rows-on.docx, #300).
/// Leaving the last row and column out too is our choice: where they are formatted on
/// their own, their format hides whether they count, and no render shows it yet.
fn band(
    at: usize,
    count: usize,
    size: i64,
    skip_first: usize,
    skip_last: bool,
) -> Option<(usize, bool, bool)> {
    let size = usize::try_from(size).ok().filter(|&size| size > 0)?;
    let first = skip_first;
    let end = count.checked_sub(usize::from(skip_last))?;
    if at < first || at >= end {
        return None;
    }
    let offset = at - first;
    let last = offset % size == size - 1 || at + 1 == end;
    Some((offset / size, offset % size == 0, last))
}

/// The odd band (the first, counted from 1) or the even one.
fn band_condition(
    band: usize,
    odd: TableStyleCondition,
    even: TableStyleCondition,
) -> TableStyleCondition {
    if band % 2 == 0 { odd } else { even }
}

/// The bits of the edges a cell lies on: top, bottom, start and end.
fn edge_bits(top: bool, bottom: bool, start: bool, end: bool) -> u64 {
    [(top, TOP), (bottom, BOTTOM), (start, START), (end, END)]
        .into_iter()
        .filter(|&(on_edge, _)| on_edge)
        .fold(0, |bits, (_, edge)| bits | edge)
}

impl<'a> Formats<'a> {
    /// The table properties in effect: those of the table's style (`w:tblStyle`, else
    /// the default table style) and the styles it is based on, then the table's own.
    /// Word ignores a style's whole-table conditional format (MS-OI29500 §17.18.89(a)).
    /// Word does not allow some properties in a style (the width, layout, look,
    /// position, overlap and direction, §17.7.6.4(a)); they come from the table only.
    /// The band sizes come from the style only (§17.4.59(a)).
    ///
    /// ```
    /// # fn main() -> officewright_doc::Result<()> {
    /// use officewright_doc::{Block, Document};
    ///
    /// // A table in the style "Grid", which gives its borders.
    /// let document = Document::open("tests/data/table-style.docx")?;
    /// let mut formats = document.formats();
    /// let Some(Block::Table(table)) = document.sections().flat_map(|s| s.blocks()).next() else {
    ///     panic!("the document starts with a table");
    /// };
    /// assert!(formats.table_properties(table).borders().is_some());
    /// // The table's w:tblLook turns on the style's format for the first row.
    /// assert_eq!(formats.table_look(table).first_row(), Some(true));
    /// # Ok(())
    /// # }
    /// ```
    pub fn table_properties(&mut self, table: Table<'a>) -> TableProperties {
        let (document, index) = table.place();
        if !std::ptr::eq(document, self.document) {
            return Formats::new(document).table_properties(table);
        }
        self.table_context(index)
            .map_or_else(|| table.properties(), |context| context.properties.clone())
    }

    /// The options of the table's `w:tblLook` in effect, each given: Word's 0x04A0 when
    /// the table has none (MS-OI29500 §17.4.55(a)), and on for an option a `w:tblLook`
    /// that writes another one leaves out, as Word takes it.
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
    /// last row), then the row's own. A style gives no grid or widths before or after
    /// the cells, no height and no hidden mark (MS-OI29500 §17.7.6.10, §17.7.6.11).
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
            if let Some(part) = part {
                if conditions & (1 << place) != 0 {
                    properties.overlay(&part.row);
                }
            }
        }
        properties.overlay(&own);
        properties
    }

    /// The cell properties in effect: the table style's, those of the conditional
    /// formats for the parts of the table the cell is in, in the order Word applies them
    /// (MS-OI29500 §17.7.6.6(a)), then the cell's own.
    ///
    /// - A conditional format's borders belong to its group of cells, such as the first
    ///   column: the cell takes the group's outer borders on the sides that lie on the
    ///   group's edges, and `w:insideH` or `w:insideV` on the others.
    /// - A conditional format's no wrapping, margins and vertical alignment apply to every
    ///   cell of a row one of whose cells it applies to (§17.7.6.8(a)).
    /// - A border the cell itself sets to `none` takes the style's (§17.4.66(a)).
    ///
    /// ```
    /// # fn main() -> officewright_doc::Result<()> {
    /// use officewright_doc::{Block, ColorValue, Document, TableStyleCondition};
    ///
    /// // The style "Grid" shades the first row of the table.
    /// let document = Document::open("tests/data/table-style.docx")?;
    /// let mut formats = document.formats();
    /// let Some(Block::Table(table)) = document.sections().flat_map(|s| s.blocks()).next() else {
    ///     panic!("the document starts with a table");
    /// };
    /// let mut fills = Vec::new();
    /// for row in table.rows() {
    ///     let cell = row.cells().next().expect("a cell");
    ///     let properties = formats.cell_properties(cell);
    ///     fills.push(properties.shading().and_then(|shading| shading.fill().value()));
    /// }
    /// assert_eq!(fills, [Some(ColorValue::Rgb(0x44, 0x72, 0xC4)), None]);
    /// let first = table.rows().next().and_then(|row| row.cells().next()).expect("a cell");
    /// assert_eq!(formats.cell_conditions(first), [TableStyleCondition::FirstRow]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn cell_properties(&mut self, cell: TableCell<'a>) -> TableCellProperties {
        #[cfg(test)]
        {
            self.work.cells += 1;
        }
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
        let cell_place = context.cells.get(&index).copied().unwrap_or_default();
        let mut properties = style.whole.cell.clone();
        for (place, part) in style.conditions.iter().enumerate() {
            let Some(part) = part else {
                continue;
            };
            if cell_place.conditions & (1 << place) != 0 {
                properties.overlay(&part.cell.in_group(cell_place.edges(place)));
            } else if cell_place.row & (1 << place) != 0 {
                properties.overlay(&part.cell.of_whole_rows());
            }
        }
        properties.overlay(&own.over_style(&properties));
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
            .and_then(|context| context.cells.get(&index).map(|place| place.conditions))
            .unwrap_or(0);
        ORDER
            .iter()
            .enumerate()
            .filter(|(place, _)| conditions & (1 << place) != 0)
            .map(|(_, &condition)| condition)
            .collect()
    }

    /// Whether the table's style gives conditional table properties, which the
    /// formatting in effect does not take yet.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn table_style_gives_conditional_table(&mut self, table: Table<'a>) -> bool {
        let (_, index) = table.place();
        self.table_context(index)
            .and_then(|context| context.style.clone())
            .is_some_and(|style| style.gives_conditional_table)
    }

    /// What the style of the table around the paragraph `node` gives it and its runs: none
    /// for a paragraph in no cell, or in a table without a style. The innermost cell
    /// counts. Worked out once per style and set of conditions; looking it up costs a walk
    /// up to the cell and a few lookups.
    pub(super) fn table_text(&mut self, node: &Node) -> Option<Arc<TableText>> {
        let cell = cell_of(self.document, node)?;
        let context = self.context_of(cell)?;
        let style = context.style.as_ref()?;
        let index = context.style_index?;
        let conditions = context.cells.get(&cell).map_or(0, |place| place.conditions);
        if let Some(text) = self.table_texts.get(&(index, conditions)) {
            return Some(Arc::clone(text));
        }
        #[cfg(test)]
        {
            self.work.table_texts += 1;
        }
        let mut paragraph = style.whole.paragraph.clone();
        let mut character = style.whole.character.clone();
        for (place, part) in style.conditions.iter().enumerate() {
            if let Some(part) = part.as_deref().filter(|_| conditions & (1 << place) != 0) {
                paragraph.overlay(&part.paragraph);
                character.overlay(&part.character);
            }
        }
        if self.table_texts.len() >= MAX_TABLE_TEXTS {
            self.forget_table_texts();
        }
        self.next_table_text += 1;
        let text = Arc::new(TableText {
            id: self.next_table_text,
            paragraph,
            character,
        });
        self.table_texts
            .insert((index, conditions), Arc::clone(&text));
        Some(text)
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
        let style_index = own
            .style()
            .and_then(|id| style_index(document, id, StyleKind::Table))
            .or_else(|| self.default_table_style());
        let style = style_index.map(|style| {
            self.table_styles
                .level(document, style, &TableStyleLevel::default())
        });
        let mut properties = style
            .as_ref()
            .map(|style| style.whole.table.clone())
            .unwrap_or_default();
        properties.overlay(&own.without_band_sizes());
        // Once one of the six options is written, Word ignores the mask of w:val
        // (MS-OI29500 §17.4.55(c)) and takes each option not written as on:
        // word-rules/E-table-style-look.docx, cases 05 and 06, and the copy Word saved
        // of it (#300). A w:tblLook with neither is taken as all off, which no render
        // shows yet.
        let look = properties.look.map_or_else(default_look, |look| {
            let written = [
                look.first_row,
                look.last_row,
                look.first_column,
                look.last_column,
                look.no_horizontal_bands,
                look.no_vertical_bands,
            ]
            .iter()
            .any(Option::is_some);
            let option = |option: Option<bool>| Some(option.unwrap_or(written));
            TableLook {
                first_row: option(look.first_row),
                last_row: option(look.last_row),
                first_column: option(look.first_column),
                last_column: option(look.last_column),
                no_horizontal_bands: option(look.no_horizontal_bands),
                no_vertical_bands: option(look.no_vertical_bands),
            }
        });
        let on = |option: Option<bool>| option.unwrap_or(false);
        let (first_row, last_row) = (on(look.first_row), on(look.last_row));
        let (first_column, last_column) = (on(look.first_column), on(look.last_column));
        // MS-OI29500 §17.7.6.5(a) and §17.7.6.7(c) say Word keeps a band size within 0
        // to 3, but Word bands by a larger size as written, and keeps it when it saves:
        // word-rules/E-table-style-bands-5.docx bands rows by 5 (#300).
        let band_size = |no_bands: Option<bool>, size: Option<i64>| {
            if on(no_bands) { 0 } else { size.unwrap_or(0) }
        };
        let row_band = band_size(look.no_horizontal_bands, properties.row_band_size);
        let column_band = band_size(look.no_vertical_bands, properties.column_band_size);
        let rows: Vec<TableRow<'a>> = table.rows().collect();
        // With the first row's format on, every header row (w:tblHeader) at the top of
        // the table takes it, and the row bands start after the last of them, whatever
        // differentiateMultirowTableHeaders says: word-rules/E-table-style-header-rows-on
        // and -off.docx (#300). The header rows make one group for the format's borders,
        // and each takes the corner formats; no render shows those yet (#298).
        let header_rows = if first_row {
            rows.iter()
                .take_while(|row| row.properties().header() == Some(true))
                .count()
                .max(1)
        } else {
            0
        };
        let mut row_conditions = HashMap::new();
        let mut cell_places = HashMap::new();
        for (at, row) in rows.iter().enumerate() {
            let top = at == 0;
            let bottom = at + 1 == rows.len();
            let header = at < header_rows;
            let row_band = band(at, rows.len(), row_band, header_rows, last_row);
            let mut conditions = 0;
            if header {
                conditions |= bit(TableStyleCondition::FirstRow);
            }
            if last_row && bottom {
                conditions |= bit(TableStyleCondition::LastRow);
            }
            if let Some((band, _, _)) = row_band {
                conditions |= bit(band_condition(
                    band,
                    TableStyleCondition::OddRowBand,
                    TableStyleCondition::EvenRowBand,
                ));
            }
            let (_, row_index) = row.place();
            row_conditions.insert(row_index, conditions);
            self.table_of.insert(row_index, index);
            let cells: Vec<TableCell<'a>> = row.cells().collect();
            let mut in_row = Vec::with_capacity(cells.len());
            for (column, cell) in cells.iter().enumerate() {
                let left = column == 0;
                let right = column + 1 == cells.len();
                let mut place = CellPlace::default();
                // A band of rows spans its rows' cells, a band of columns its columns'
                // cells in every row; the first or last row or column is one row or column.
                if let Some((band, first, last)) = row_band {
                    let condition = band_condition(
                        band,
                        TableStyleCondition::OddRowBand,
                        TableStyleCondition::EvenRowBand,
                    );
                    place.meets(condition, edge_bits(first, last, left, right));
                }
                if let Some((band, first, last)) = band(
                    column,
                    cells.len(),
                    column_band,
                    usize::from(first_column),
                    last_column,
                ) {
                    let condition = band_condition(
                        band,
                        TableStyleCondition::OddColumnBand,
                        TableStyleCondition::EvenColumnBand,
                    );
                    place.meets(condition, edge_bits(top, bottom, first, last));
                }
                if first_column && left {
                    place.meets(
                        TableStyleCondition::FirstColumn,
                        edge_bits(top, bottom, true, true),
                    );
                }
                if last_column && right {
                    place.meets(
                        TableStyleCondition::LastColumn,
                        edge_bits(top, bottom, true, true),
                    );
                }
                if header {
                    place.meets(
                        TableStyleCondition::FirstRow,
                        edge_bits(top, at + 1 == header_rows, left, right),
                    );
                }
                if last_row && bottom {
                    place.meets(
                        TableStyleCondition::LastRow,
                        edge_bits(true, true, left, right),
                    );
                }
                // A corner cell is styled on its own when both its row and its column are
                // (MS-OI29500 §17.4.55(b)).
                for (corner, row_on, column_on) in [
                    (
                        TableStyleCondition::TopLeftCell,
                        header,
                        first_column && left,
                    ),
                    (
                        TableStyleCondition::TopRightCell,
                        header,
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
                        place.meets(corner, edge_bits(true, true, true, true));
                    }
                }
                let (_, cell_index) = cell.place();
                in_row.push((cell_index, place));
                self.table_of.insert(cell_index, index);
            }
            let union = in_row
                .iter()
                .fold(0, |union, (_, place)| union | place.conditions);
            for (cell_index, mut place) in in_row {
                place.row = union;
                cell_places.insert(cell_index, place);
            }
        }
        let context = Arc::new(TableContext {
            style,
            style_index,
            properties,
            look,
            rows: row_conditions,
            cells: cell_places,
        });
        self.tables.insert(index, Arc::clone(&context));
        Some(context)
    }

    /// The index of the default table style, once looked up: the last, as
    /// [`Styles::default_style`](crate::Styles::default_style) gives it.
    fn default_table_style(&mut self) -> Option<usize> {
        let document = self.document;
        *self.default_table_style.get_or_insert_with(|| {
            document
                .styles
                .styles
                .iter()
                .take(super::MAX_STYLES)
                .rposition(|style| style.kind == StyleKind::Table && style.is_default)
        })
    }
}
