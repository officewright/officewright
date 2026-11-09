//! The room a text box's line takes from its text, as Word's renders of
//! `word-rules/E-textbox-fill-outline.docx` and `E-textbox-vml.docx` place the text. It reads
//! `tests/corpus` at the root of the repository, so it is left out of the published crate.
#![cfg(feature = "pdf")]

use std::path::Path;

use officewright_doc::Document;
use officewright_layout::ir;

/// A rule document of the committed corpus.
fn open(name: &str) -> officewright_doc::Result<Document> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/word-rules")
        .join(name);
    Document::open(path)
}

/// The text boxes of a translation's paragraphs, in order, and what it left out.
fn boxes(translation: &officewright_doc::Translation) -> (Vec<ir::TextBox>, Vec<String>) {
    let boxes = translation
        .document()
        .sections()
        .iter()
        .flat_map(|section| section.blocks())
        .filter_map(|block| match block {
            ir::Block::Paragraph(paragraph) => Some(paragraph.inlines()),
            _ => None,
        })
        .flatten()
        .filter_map(|inline| {
            let content = match inline {
                ir::Inline::Object(object) => object.content(),
                ir::Inline::Anchored(object) => object.content(),
                _ => return None,
            };
            match content {
                ir::ObjectContent::TextBox(text_box) => Some((**text_box).clone()),
                _ => None,
            }
        })
        .collect();
    let left_out = translation
        .left_out()
        .iter()
        .map(|entry| entry.message().to_owned())
        .collect();
    (boxes, left_out)
}

/// Asserts that each inset is Word's within 0.02 pt.
fn assert_insets(boxes: &[ir::TextBox], expected: &[(f64, f64)]) {
    let insets: Vec<(f64, f64)> = boxes
        .iter()
        .map(|text_box| (text_box.insets().left().0, text_box.insets().top().0))
        .collect();
    assert_eq!(insets.len(), expected.len(), "{insets:?}");
    for (got, want) in insets.iter().zip(expected) {
        assert!(
            (got.0 - want.0).abs() <= 0.02 && (got.1 - want.1).abs() <= 0.02,
            "{insets:?} against Word's {expected:?}"
        );
    }
}

#[test]
fn moves_a_text_box_s_text_inside_its_line_as_word_does() {
    // From the reference's geometry.json: each box is in the line at the 72 pt margin, its
    // first word at 79.224 pt where the box has no line, 82.224 pt for a 6 pt line centred on
    // the frame, 85.224 pt for one inside it, and 79.464 pt for a 0.5 pt line; the body's
    // text starts at 72.024 pt. Down, the first line's top is as far below the case's
    // line, 3 and 6.12 pt more for the 6 pt lines, 0.24 pt more for the thin one. So the
    // text starts inside the line: half a centred line's width, a whole line inside the
    // frame, past DrawingML's insets of 7.2 and 3.6 pt.
    let (boxes, left_out) = boxes(&open("E-textbox-fill-outline.docx").unwrap().to_layout_ir());
    assert_insets(
        &boxes,
        &[
            (7.2, 3.6),
            (7.2, 3.6),
            (7.2, 3.6),
            (10.2, 6.6),
            (13.2, 9.6),
            (7.45, 3.85),
        ],
    );
    // The IR draws a line centred on the frame.
    assert!(
        left_out.contains(&"a text box's line inside its frame (a:ln algn=\"in\") is drawn centred on the frame, here and wherever else the document has one".to_owned()),
        "{left_out:?}"
    );
}

#[test]
fn moves_a_vml_text_box_s_text_inside_its_line_as_word_does() {
    // From the reference's geometry.json: the text of the box in the line, case 05, starts
    // at 79.464 pt, 0.24 pt past VML's inset of 7.2 pt, as a centred line of 0.5 pt leaves
    // it; that of case 03, of no inset, 0.25 pt past where case 01's text, of the default
    // inset, would put the frame (223.49 less 7.45 pt).
    let (boxes, _) = boxes(&open("E-textbox-vml.docx").unwrap().to_layout_ir());
    assert_insets(
        &boxes,
        &[
            (7.45, 3.85),
            (18.25, 18.25),
            (0.25, 0.25),
            (0.25, 0.25),
            (7.45, 3.85),
        ],
    );
}
