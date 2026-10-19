//! Indents in character units and spacing in line units, as Word's render of
//! `word-rules/E-paragraph-units.docx` places its eight cases. It reads `tests/corpus` at
//! the root of the repository, so it is left out of the published crate.
#![cfg(feature = "pdf")]

use std::path::Path;

use officewright_core::Pt;
use officewright_doc::Document;
use officewright_layout::ir;

#[test]
fn converts_character_and_line_units_as_words_render_shows() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/word-rules/E-paragraph-units.docx");
    let document = Document::open(path).unwrap();
    let translation = document.to_layout_ir();
    let props: Vec<&ir::ParagraphProps> = translation.document().sections()[0]
        .blocks()
        .iter()
        .filter_map(|block| match block {
            ir::Block::Paragraph(paragraph) => Some(paragraph.props()),
            _ => None,
        })
        .collect();
    // From the reference's geometry.json: each case's line starts at the 72 pt margin
    // plus its indent (93.98, 93.98, 144.02 and 105.02 pt), and the space before a case
    // is the distance from the line before less the line pitch of 13.46 pt (11.86, 11.98,
    // 23.98 and 5.98 pt). The text is 11 pt.
    let indents: Vec<Pt> = props[..4].iter().map(|p| p.indent().start()).collect();
    // 1: a style's 2 characters; 2: the same over a later 72 pt; 3: characters 0 cancel
    // them, so the 72 pt counts; 4: 3 characters over 50 pt at one level.
    assert_eq!(indents, [Pt(22.0), Pt(22.0), Pt(72.0), Pt(33.0)]);
    let before: Vec<Pt> = props[4..8].iter().map(|p| p.spacing().before()).collect();
    // 5: a style's line; 6: the same over a later 24 pt; 7: lines 0 cancel it; 8: half a
    // line over 24 pt at one level.
    assert_eq!(before, [Pt(12.0), Pt(12.0), Pt(24.0), Pt(6.0)]);
    // Nothing about the units is listed.
    assert!(
        translation
            .left_out()
            .iter()
            .all(|entry| !entry.message().contains("units")),
        "{:?}",
        translation.left_out()
    );
}
