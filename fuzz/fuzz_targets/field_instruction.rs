//! Reads the input as a field instruction (UTF-8, lossily) and checks what the parser
//! gives: written back, the instruction reads again as the same instruction; no part of it
//! is longer than the input; and a `HYPERLINK` field's switches read without panicking,
//! each of its values one of the instruction's arguments.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_doc::{FieldInstruction, FieldToken};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let instruction = FieldInstruction::parse(&text);
    let written = instruction.to_string();
    assert_eq!(
        FieldInstruction::parse(&written),
        instruction,
        "{written:?}"
    );
    // Every argument and the name come from the input, and writing back at most doubles
    // the escaped characters and adds quotes and spaces.
    assert!(instruction.name().len() <= text.len());
    for token in instruction.tokens() {
        if let FieldToken::Argument(argument) = token {
            assert!(argument.len() <= text.len());
        }
    }
    assert!(written.len() <= 2 * text.len() + 3 * instruction.tokens().len() + 1);
    if let Some(link) = instruction.hyperlink() {
        let arguments: Vec<&str> = instruction
            .tokens()
            .iter()
            .filter_map(|token| match token {
                FieldToken::Argument(argument) => Some(argument.as_str()),
                _ => None,
            })
            .collect();
        for value in [
            link.target(),
            link.bookmark(),
            link.doc_location(),
            link.target_frame(),
            link.tooltip(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(arguments.contains(&value), "{value:?}");
        }
        let _ = (link.image_map(), link.new_window(), link.no_history());
    }
});
