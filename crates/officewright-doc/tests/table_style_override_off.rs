//! The size and justification of a table's text with the setting
//! `overrideTableStyleFontSizeAndJustification` written 0, in
//! `word-rules/E-table-style-override-off-*.docx`: off, in every mode. It reads
//! `tests/corpus` at the root of the repository, so it is left out of the published
//! crate.

use std::path::Path;

use officewright_doc::{Block, Document, Justification};

#[test]
fn takes_a_setting_written_off_as_off_in_every_mode() {
    // The table style Big gives 14 pt and centring; Normal gives its own size and left,
    // over document defaults a point larger and right-aligned. With the setting off,
    // Normal's 11 pt stays, its 12 pt gives way, and its left justification gives way. In
    // mode 14, -11pt-14, Word's render shows it (#595). In mode 15, -11pt and -12pt, no
    // render shows it: Word left the 0 out of the copies it saved, which its renders are
    // exported from, and those copies read as on.
    for (name, size, justification) in [
        (
            "E-table-style-override-off-11pt",
            11.0,
            Justification::Center,
        ),
        (
            "E-table-style-override-off-12pt",
            14.0,
            Justification::Center,
        ),
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
