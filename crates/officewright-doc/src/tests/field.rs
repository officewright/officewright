//! Field instructions and the HYPERLINK field.

use super::*;

fn argument(text: &str) -> FieldToken {
    FieldToken::Argument(text.into())
}

fn switch(text: &str) -> FieldToken {
    FieldToken::Switch(text.into())
}

#[test]
fn divides_an_instruction_into_its_name_arguments_and_switches() {
    let cases: [(&str, &str, Vec<FieldToken>); 16] = [
        ("PAGE", "PAGE", vec![]),
        (
            "  PAGE   \\*  roman \\* MERGEFORMAT ",
            "PAGE",
            vec![
                switch("*"),
                argument("roman"),
                switch("*"),
                argument("MERGEFORMAT"),
            ],
        ),
        // Quotes: their text, with \" and \\ read and any other backslash kept.
        (
            r#"HYPERLINK "C:\\Docs\\a b.docx" \o "say \"hi\" \n""#,
            "HYPERLINK",
            vec![
                argument(r"C:\Docs\a b.docx"),
                switch("o"),
                argument(r#"say "hi" \n"#),
            ],
        ),
        // The same escapes in a word without quotes (§17.16.1: the quotes are optional
        // where the text holds no white space); quotes and other backslashes after its
        // first character are its own.
        (
            r#"INCLUDETEXT C:\\Docs\\a.docx \"x\" C:\a"b"#,
            "INCLUDETEXT",
            vec![
                argument(r"C:\Docs\a.docx"),
                argument(r#""x""#),
                argument(r#"C:\a"b"#),
            ],
        ),
        // A quote right after a word starts text of its own.
        (
            r#"X "a""b"\l"#,
            "X",
            vec![argument("a"), argument("b"), switch("l")],
        ),
        // A switch is ! or one or two Latin letters, as written, or a formatting switch.
        (
            r"EQ \co2 \S\Up 3 \! \#",
            "EQ",
            vec![
                switch("co"),
                argument("2"),
                switch("S"),
                switch("Up"),
                argument("3"),
                switch("!"),
                switch("#"),
            ],
        ),
        (
            r#"DATE \@"d MMMM" \*Upper"#,
            "DATE",
            vec![
                switch("@"),
                argument("d MMMM"),
                switch("*"),
                argument("Upper"),
            ],
        ),
        // A backslash before anything else starts a word.
        (
            r"EQ \o(\s\up 2(a),b) \1",
            "EQ",
            vec![
                switch("o"),
                argument(r"(\s\up"),
                argument("2(a),b)"),
                argument(r"\1"),
            ],
        ),
        // Text in quotes that does not end runs to the end, as does a backslash at the end;
        // a backslash before white space is an argument of its own.
        (r#"X "open \"#, "X", vec![argument(r"open \")]),
        ("X \\", "X", vec![argument("\\")]),
        ("X \\ a", "X", vec![argument("\\"), argument("a")]),
        ("X \\\ta", "X", vec![argument("\\"), argument("a")]),
        // No name: an instruction that starts with quotes or a switch.
        (r#""PAGE" \h"#, "", vec![argument("PAGE"), switch("h")]),
        (r"\l x", "", vec![switch("l"), argument("x")]),
        // White space of any kind divides.
        ("REF\u{a0}a\tb", "REF", vec![argument("a"), argument("b")]),
        ("", "", vec![]),
    ];
    for (text, name, tokens) in cases {
        let instruction = FieldInstruction::parse(text);
        assert_eq!(
            (instruction.name(), instruction.tokens()),
            (name, &tokens[..]),
            "{text:?}"
        );
    }
}

#[test]
fn writes_an_instruction_back_as_it_was_read() {
    // Word reads what is written back as it read the instruction, also where the syntax
    // of the field is its own, as EQ's is; instructions from LibreOffice's tests
    // (tdf138779.docx, tdf164620.docx) and Word's.
    for text in [
        r#"EQ \* jc0 \* "Font:Kozuka Mincho Pro R" \* hps16 \o(\s\up 19(にがつ),二月)"#,
        r"RD  c:\\temp\\rd",
        r#" HYPERLINK  "https://example.com/a b" \o "q\"\\"  x "#,
        r#"X "" \l"#,
        r#""PAGE" next"#,
        r"EQ \co2 \S\Up 3",
        "",
    ] {
        assert_eq!(FieldInstruction::parse(text).to_string(), text);
    }
}

#[test]
fn gives_a_switch_s_argument_where_one_follows_it() {
    let instruction = FieldInstruction::parse(r#"TOC \o "1-3" \H \z \o "4-5" \co 2"#);
    assert_eq!(instruction.switch("o"), Some(Some("1-3")));
    // In any case (§17.16.1).
    assert_eq!(instruction.switch("O"), Some(Some("1-3")));
    assert_eq!(instruction.switch("h"), Some(None));
    assert_eq!(instruction.switch("z"), Some(None));
    assert_eq!(instruction.switch("CO"), Some(Some("2")));
    assert_eq!(instruction.switch("c"), None);
    assert_eq!(instruction.switch("u"), None);
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
    // A formatting switch takes its argument, which is no target, and so does a switch
    // the field does not have.
    let formatted = link(r#"HYPERLINK \* MERGEFORMAT "x""#).unwrap();
    assert_eq!(formatted.target(), Some("x"));
    let unknown = link(r#"HYPERLINK \x "y" "https://example.com""#).unwrap();
    assert_eq!(unknown.target(), Some("https://example.com"));
    // Switches in any case (§17.16.1).
    let upper =
        link(r#"HYPERLINK "https://example.com" \L "Intro" \O "Tip" \S "1" \T "_top" \M \N \H"#)
            .unwrap();
    assert_eq!(
        (
            upper.bookmark(),
            upper.tooltip(),
            upper.doc_location(),
            upper.target_frame()
        ),
        (Some("Intro"), Some("Tip"), Some("1"), Some("_top"))
    );
    assert!(upper.image_map() && upper.new_window() && upper.no_history());
    // A target without quotes, its backslashes escaped.
    let path = link(r"HYPERLINK C:\\Docs\\a.docx").unwrap();
    assert_eq!(path.target(), Some(r"C:\Docs\a.docx"));
    // Another field is no hyperlink.
    assert!(link("PAGE").is_none());
    assert!(link("HYPERLINKS x").is_none());
}
