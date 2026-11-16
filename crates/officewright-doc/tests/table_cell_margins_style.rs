//! A cell's own margins of type `nil`, `pct` and `auto` under a table style's cell
//! margins, as Word's render of `word-rules/E-table-cell-margins-style.docx` shows them. It
//! reads `tests/corpus` at the root of the repository, so it is left out of the published
//! crate.
#![cfg(feature = "pdf")]

use std::path::Path;

use officewright_core::Pt;
use officewright_doc::Document;
use officewright_layout::ir;

#[test]
fn gives_a_cell_s_own_top_margin_that_is_no_length_the_style_s_as_words_render_shows() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/word-rules/E-table-cell-margins-style.docx");
    let document = Document::open(path).unwrap();
    let translation = document.to_layout_ir();
    let tables: Vec<&ir::Table> = translation.document().sections()[0]
        .blocks()
        .iter()
        .filter_map(|block| match block {
            ir::Block::Table(table) => Some(table),
            _ => None,
        })
        .collect();
    let margins = |top: f64, sides: f64| {
        Some(
            ir::CellMargins::default()
                .with_top(Pt(top))
                .with_start(Pt(sides))
                .with_bottom(Pt(top))
                .with_end(Pt(sides)),
        )
    };
    let of = |table: &ir::Table| {
        table.rows()[0]
            .cells()
            .iter()
            .map(ir::Cell::margins)
            .collect::<Vec<_>>()
    };
    // From the reference's geometry: the first lines of all four cells of case 01 at one
    // height, Word's 15 pt above each; the text of the cells with margins of their own
    // starts and ends at the table's 5 pt, that of the last cell at the style's 15 pt.
    assert_eq!(
        of(tables[0]),
        [
            margins(15.0, 5.0),
            margins(15.0, 5.0),
            margins(15.0, 5.0),
            margins(15.0, 15.0)
        ]
    );
    // Case 05: the first column's 15 pt, which reaches the whole row, above both cells.
    assert_eq!(of(tables[1]), [margins(15.0, 5.0), margins(15.0, 15.0)]);
}
