//! Field instructions and the HYPERLINK field.

use super::*;

fn argument(text: &str) -> FieldToken {
    FieldToken::Argument(text.into())
}

#[test]
fn divides_an_instruction_into_its_name_arguments_and_switches() {
    let cases: [(&str, &str, Vec<FieldToken>); 9] = [
        ("PAGE", "PAGE", vec![]),
        (
            "  PAGE   \\*  roman \\* MERGEFORMAT ",
            "PAGE",
            vec![
                FieldToken::Switch('*'),
                argument("roman"),
                FieldToken::Switch('*'),
                argument("MERGEFORMAT"),
            ],
        ),
        // Quotes: their text, with \" and \\ read and any other backslash kept.
        (
            r#"HYPERLINK "C:\\Docs\\a b.docx" \o "say \"hi\" \n""#,
            "HYPERLINK",
            vec![
                argument(r"C:\Docs\a b.docx"),
                FieldToken::Switch('o'),
                argument(r#"say "hi" \n"#),
            ],
        ),
        // A word may hold quotes and backslashes after its first character.
        (
            r#"INCLUDETEXT C:\a"b c"#,
            "INCLUDETEXT",
            vec![argument(r#"C:\a"b"#), argument("c")],
        ),
        // A quote right after a word starts text of its own.
        (
            r#"X "a""b"\l"#,
            "X",
            vec![argument("a"), argument("b"), FieldToken::Switch('l')],
        ),
        // Text in quotes that does not end runs to the end, as does a backslash at the end.
        (r#"X "open \"#, "X", vec![argument(r"open \")]),
        ("X \\", "X", vec![argument("\\")]),
        // No name: an instruction that starts with quotes or a switch.
        (
            r#""PAGE" \h"#,
            "",
            vec![argument("PAGE"), FieldToken::Switch('h')],
        ),
        ("", "", vec![]),
    ];
    for (text, name, tokens) in cases {
        let instruction = FieldInstruction::parse(text);
        assert_eq!(
            (instruction.name(), instruction.tokens()),
            (name, &tokens[..]),
            "{text:?}"
        );
        // Written back, it reads as the same instruction.
        let written = instruction.to_string();
        assert_eq!(
            FieldInstruction::parse(&written),
            instruction,
            "{written:?}"
        );
    }
}

#[test]
fn writes_an_instruction_back_quoting_only_what_needs_quotes() {
    let written = |text: &str| FieldInstruction::parse(text).to_string();
    assert_eq!(written("  PAGE  \\* roman "), r"PAGE \* roman");
    assert_eq!(
        written(r#"HYPERLINK "a b" \o "q\"\\" "x" C:\d"#),
        r#"HYPERLINK "a b" \o "q\"\\" x "C:\\d""#
    );
    assert_eq!(written(r#"X "" \l"#), r#"X "" \l"#);
    // Without a name, the first argument keeps its quotes, else it would read as one.
    assert_eq!(written(r#""PAGE" next"#), r#""PAGE" next"#);
}

#[test]
fn gives_a_switch_s_argument_where_one_follows_it() {
    let instruction = FieldInstruction::parse(r#"TOC \o "1-3" \h \z \o "4-5""#);
    assert_eq!(instruction.switch('o'), Some(Some("1-3")));
    assert_eq!(instruction.switch('h'), Some(None));
    assert_eq!(instruction.switch('z'), Some(None));
    assert_eq!(instruction.switch('u'), None);
}

#[test]
fn reads_a_hyperlink_field_by_its_switches() {
    let link = |text: &str| FieldInstruction::parse(text).hyperlink();
    // Word's \s and \h (MS-OI29500 §17.16.5.25(a), (b)), with the standard's switches.
    let full = link(
        r#" hyperlink "https://example.com/a b" \l "Part" \s "1,2" \t "_top" \o "Tip" \m \n \h \* MERGEFORMAT "#,
    )
    .unwrap();
    assert_eq!(full.target(), Some("https://example.com/a b"));
    assert_eq!(full.bookmark(), Some("Part"));
    assert_eq!(full.doc_location(), Some("1,2"));
    assert_eq!(full.target_frame(), Some("_top"));
    assert_eq!(full.tooltip(), Some("Tip"));
    assert!(full.image_map() && full.new_window() && full.no_history());
    // A bookmark alone; switches before the target; the first of a switch written twice.
    let bookmark = link(r#"HYPERLINK \l "_Toc12""#).unwrap();
    assert_eq!(
        (bookmark.target(), bookmark.bookmark()),
        (None, Some("_Toc12"))
    );
    assert!(!bookmark.image_map() && !bookmark.new_window() && !bookmark.no_history());
    let late = link(r#"HYPERLINK \n \o "Tip" "https://example.com" \o "Again""#).unwrap();
    assert_eq!(late.target(), Some("https://example.com"));
    assert_eq!(late.tooltip(), Some("Tip"));
    // A switch that takes an argument has none before the next switch or at the end.
    let bare = link(r"HYPERLINK \l \o").unwrap();
    assert_eq!(
        (bare.target(), bare.bookmark(), bare.tooltip()),
        (None, None, None)
    );
    // A formatting switch takes its argument, which is no target.
    let formatted = link(r#"HYPERLINK \* MERGEFORMAT "x""#).unwrap();
    assert_eq!(formatted.target(), Some("x"));
    // Another field is no hyperlink.
    assert!(link("PAGE").is_none());
    assert!(link("HYPERLINKS x").is_none());
}
