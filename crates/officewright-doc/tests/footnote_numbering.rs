//! Footnote numbering as Word's render of `word-rules/E-footnote-numbering-settings.docx`
//! shows it: from the sections only, never from the settings' `w:footnotePr`. It reads
//! `tests/corpus` at the root of the repository, so it is left out of the published crate.
#![cfg(feature = "pdf")]

use std::path::Path;

use officewright_doc::Document;
use officewright_layout::ir;

#[test]
fn numbers_footnotes_by_the_sections_only_as_words_render_shows() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/word-rules/E-footnote-numbering-settings.docx");
    let document = Document::open(path).unwrap();
    // The settings give lower-case letters from 3, starting again on each page.
    let settings = document.settings().footnotes().unwrap();
    assert!(settings.number_format().is_some() && settings.start() == Some(3));
    let translation = document.to_layout_ir();
    let sections = translation.document().sections();
    // From the reference's geometry.json: section 1 numbers its notes 1 and 2 on page 1
    // and 3 on page 2, Word's defaults; section 2 numbers them D and E, in the upper-case
    // letters it gives, and with none of the settings' start and restart.
    assert_eq!(sections[0].footnotes(), &ir::NoteProps::footnotes());
    assert_eq!(
        sections[1].footnotes(),
        &ir::NoteProps::footnotes().with_format(ir::NumberFormat::UpperLetter)
    );
}
