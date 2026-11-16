//! The size and justification of a table's text with the setting
//! `overrideTableStyleFontSizeAndJustification` written 0, as Word's renders of
//! `word-rules/E-table-style-override-off-*.docx` show them: as if the setting were not
//! written. It reads `tests/corpus` at the root of the repository, so it is left out of
//! the published crate.

use std::path::Path;

use officewright_doc::{Block, Document, Justification};

#[test]
fn takes_a_setting_written_off_as_not_written_as_words_renders_show() {
    // The table style Big gives 14 pt and centring; Normal gives its own size and left,
    // over document defaults a point larger and right-aligned. From the references'
    // geometry: the cells' text in mode 15 in Normal's size and at the cells' left edge;
    // in mode 14 in Normal's 11 pt, centred.
    for (name, size, justification) in [
        ("E-table-style-override-off-11pt", 11.0, Justification::Left),
        ("E-table-style-override-off-12pt", 12.0, Justification::Left),
        (
            "E-table-style-override-off-11pt-14",
            11.0,
            Justification::Center,
        ),
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/corpus/word-rules/{name}.docx"));
        let document = Document::open(path).unwrap();
        let written = document
            .settings()
            .compatibility_settings()
            .iter()
            .find(|setting| setting.name() == "overrideTableStyleFontSizeAndJustification")
            .and_then(|setting| setting.value());
        assert_eq!(written, Some("0"), "{name}");
        let mut formats = document.formats();
        let mut cells = 0;
        for block in document.sections().flat_map(|section| section.blocks()) {
            match block {
                Block::Table(table) => {
                    for paragraph in table
                        .rows()
                        .flat_map(|row| row.cells())
                        .flat_map(|cell| cell.blocks())
                        .filter_map(|block| match block {
                            Block::Paragraph(paragraph) => Some(paragraph),
                            _ => None,
                        })
                    {
                        cells += 1;
                        let mark = formats.paragraph_mark(paragraph);
                        assert_eq!(
                            (mark.size().0, formats.paragraph(paragraph).justification()),
                            (size, justification),
                            "{name}"
                        );
                    }
                }
                // Outside the table, Normal's own.
                Block::Paragraph(paragraph) => {
                    assert_eq!(
                        formats.paragraph(paragraph).justification(),
                        Justification::Left,
                        "{name}"
                    );
                }
                _ => {}
            }
        }
        assert_eq!(cells, 4, "{name}");
    }
}
