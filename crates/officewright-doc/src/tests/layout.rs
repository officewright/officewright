//! The translation into the layout IR: [`Document::to_layout_ir`].

use officewright_layout::ir;
use officewright_layout::{ReportEntry, ReportKind};

use super::*;

/// The translation of a document, which must be valid IR.
fn translate(document: &Document) -> Translation {
    let translation = document.to_layout_ir();
    if let Err(err) = translation.document().validate() {
        panic!("not valid: {err}");
    }
    translation
}

/// The paragraphs of the first section.
fn paragraphs(translation: &Translation) -> Vec<&ir::Paragraph> {
    translation.document().sections()[0]
        .blocks()
        .iter()
        .map(|block| match block {
            ir::Block::Paragraph(paragraph) => paragraph,
            other => panic!("not a paragraph: {other:?}"),
        })
        .collect()
}

#[test]
fn keeps_line_units_in_points_on_a_document_grid_and_bounds_units() {
    // What a line is on a document grid no render shows yet: there the points count,
    // and that is listed. Characters are converted on a grid as off it.
    let body = r#"<w:p><w:pPr><w:spacing w:before="240" w:beforeLines="100"/><w:ind w:leftChars="100"/></w:pPr></w:p><w:sectPr><w:docGrid w:type="lines" w:linePitch="360"/></w:sectPr>"#;
    let translation = translate(&with_parts(body, &[]));
    let props = paragraphs(&translation)[0].props();
    assert_eq!(props.spacing().before(), Pt(12.0));
    assert_eq!(props.indent().start(), Pt(11.0));
    let left_out: Vec<String> = translation
        .left_out()
        .iter()
        .map(|entry| entry.message().to_owned())
        .collect();
    assert!(
        left_out.contains(
            &"spacing in line units on a document grid keeps its value in points".to_owned()
        ),
        "{left_out:?}"
    );
    // Values in units from a file stay within what lengths in points may be, and space
    // is never negative: the IR validates.
    let body = r#"<w:p><w:pPr><w:spacing w:beforeLines="-500" w:afterLines="9223372036854775807"/><w:ind w:leftChars="9223372036854775807" w:hangingChars="9223372036854775807"/></w:pPr></w:p>"#;
    let translation = translate(&with_parts(body, &[]));
    translation.document().validate().unwrap();
    let props = paragraphs(&translation)[0].props();
    assert_eq!(props.spacing().before(), Pt(0.0));
    assert!(props.spacing().after().0 <= f64::from(i32::MAX) / 20.0);
}

/// An inline, briefly; fields with their results.
fn describe(inline: &ir::Inline) -> String {
    match inline {
        ir::Inline::Text { text, .. } => format!("text {text}"),
        ir::Inline::Tab { .. } => "tab".to_owned(),
        ir::Inline::Break { kind, .. } => format!("break {kind:?}"),
        ir::Inline::Symbol { font, code, .. } => format!("symbol {font} {code:04X}"),
        ir::Inline::SoftHyphen { .. } => "soft hyphen".to_owned(),
        ir::Inline::NoBreakHyphen { .. } => "no-break hyphen".to_owned(),
        ir::Inline::Field(field) => {
            let result: Vec<String> = field.result().iter().map(describe).collect();
            format!(
                "field {:?} {:?} [{}]",
                field.kind(),
                field.format(),
                result.join(", ")
            )
        }
        other => format!("{other:?}"),
    }
}

/// The inlines of each paragraph of the first section, briefly.
fn contents(translation: &Translation) -> Vec<Vec<String>> {
    paragraphs(translation)
        .into_iter()
        .map(|paragraph| paragraph.inlines().iter().map(describe).collect())
        .collect()
}

fn style_of(inline: &ir::Inline) -> ir::RunStyleId {
    match inline {
        ir::Inline::Text { style, .. } => *style,
        other => panic!("not text: {other:?}"),
    }
}

#[test]
fn translates_text_tabs_breaks_symbols_and_hyphens() {
    let document = open(
        r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="Arial"/></w:rPr><w:t>a	b</w:t><w:tab/><w:br/><w:br w:type="page"/><w:br w:type="column"/><w:br w:clear="all"/><w:cr/><w:sym w:font="Wingdings" w:char="F0FC"/><w:sym w:char="F020"/><w:softHyphen/><w:noBreakHyphen/><w:t xml:space="preserve">c&#x85;d&#10;e</w:t></w:r></w:p>"#,
    );
    let translation = translate(&document);
    assert_eq!(
        contents(&translation),
        [[
            "text a",
            "tab",
            "text b",
            "tab",
            "break Line(None)",
            "break Page",
            "break Column",
            "break Line(All)",
            "break Line(None)",
            "symbol Wingdings F0FC",
            // A symbol without a font is in the run's font.
            "symbol Arial F020",
            "soft hyphen",
            "no-break hyphen",
            // Control characters are not IR text: a line feed is a space, others go.
            "text cd e",
        ]]
    );
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
}

#[test]
fn gives_each_distinct_run_format_one_run_style() {
    let styles = [
        format::defaults(r#"<w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:eastAsia="SimSun" w:cs="Arial"/><w:sz w:val="22"/><w:lang w:val="en-GB" w:eastAsia="zh-CN"/>"#),
        format::style("character", "Strong", None, "<w:b/>"),
    ]
    .concat();
    let runs = [
        "",
        r#"<w:rStyle w:val="Strong"/>"#,
        "<w:b/>",
        r#"<w:color w:val="FF0000"/><w:u w:val="double" w:color="00FF00"/><w:highlight w:val="darkCyan"/><w:vertAlign w:val="superscript"/><w:w w:val="80"/><w:spacing w:val="20"/><w:kern w:val="24"/><w:caps/><w:vanish/><w:shd w:val="pct20" w:color="0000FF" w:fill="FFFF00"/>"#,
        // A shading of pattern nil is no shading at all, not even its fill.
        r#"<w:shd w:val="nil" w:fill="FFFF00"/>"#,
    ];
    let body = format::paragraph("", &runs).repeat(2);
    let fonts = format!(
        r#"<w:fonts xmlns:w="{W}"><w:font w:name="SimSun"><w:charset w:val="86"/></w:font></w:fonts>"#
    );
    let document = with_parts(
        &body,
        &[
            (
                "styles",
                "styles.xml",
                format!(r#"<w:styles xmlns:w="{W}">{styles}</w:styles>"#),
            ),
            ("fontTable", "fontTable.xml", fonts),
        ],
    );
    let translation = translate(&document);
    let ir = translation.document();
    let ids: Vec<ir::RunStyleId> = paragraphs(&translation)
        .iter()
        .flat_map(|paragraph| paragraph.inlines().iter().map(style_of))
        .collect();
    // Three run styles for five formats: the run in the bold character style and the bold
    // run differ only in the style they name, which the IR does not keep, and the nil
    // shading is no shading, as in the plain run. The second paragraph repeats the first's
    // styles.
    assert_eq!((ids[1], ids[4]), (ids[2], ids[0]));
    assert!(ids[0] != ids[1] && ids[1] != ids[3] && ids[0] != ids[3]);
    assert_eq!(&ids[..5], &ids[5..]);
    assert_eq!(ir.run_style(ids[4]).unwrap().shading(), None);
    assert!(ir.run_style(ids[2]).unwrap().bold());

    let plain = ir.run_style(ids[0]).unwrap();
    assert_eq!(plain.fonts().ascii(), "Arial");
    assert_eq!(plain.fonts().east_asia(), "SimSun");
    assert_eq!(
        plain.fonts().east_asia_charset(),
        ir::EastAsiaCharset::Gb2312
    );
    assert_eq!(plain.size(), Pt(11.0));
    assert_eq!(plain.language().latin(), Some("en-GB"));
    assert_eq!(plain.language().east_asia(), Some("zh-CN"));
    assert!(!plain.bold());
    assert!(ir.run_style(ids[1]).unwrap().bold());

    let rich = ir.run_style(ids[3]).unwrap();
    let rgb =
        |red, green, blue| ir::ColorValue::Rgb(officewright_core::Color::rgb(red, green, blue));
    assert_eq!(rich.color(), rgb(255, 0, 0));
    assert_eq!(
        rich.underline(),
        Some(
            ir::Underline::default()
                .with_kind(ir::UnderlineKind::Double)
                .with_color(rgb(0, 255, 0))
        )
    );
    assert_eq!(
        rich.highlight(),
        Some(officewright_core::Color::rgb(0, 0x80, 0x80))
    );
    assert_eq!(rich.vertical(), ir::VerticalPosition::Superscript);
    assert_eq!(rich.scale(), 80);
    assert_eq!(rich.letter_spacing(), Pt(1.0));
    assert_eq!(rich.kern_min(), Some(Pt(12.0)));
    assert!(rich.caps() && rich.hidden());
    // Without ligatures in the defaults, none; contextual alternates off.
    assert_eq!(rich.ligatures(), ir::Ligatures::default());
    assert!(!rich.contextual_alternates());
    assert_eq!(
        rich.shading(),
        Some(
            ir::Shading::default()
                .with_pattern(ir::ShadingPattern::Percent20)
                .with_color(rgb(0, 0, 255))
                .with_fill(rgb(255, 255, 0))
        )
    );
}

#[test]
fn cuts_font_names_no_font_has() {
    let name = "N".repeat(300);
    let document = open(&format!(
        r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="{name}"/></w:rPr><w:t>x</w:t></w:r></w:p>"#
    ));
    let translation = translate(&document);
    let style = style_of(&paragraphs(&translation)[0].inlines()[0]);
    let fonts = translation
        .document()
        .run_style(style)
        .unwrap()
        .fonts()
        .clone();
    assert_eq!(fonts.ascii(), "N".repeat(256));
    let messages: Vec<&str> = translation
        .left_out()
        .iter()
        .map(ReportEntry::message)
        .collect();
    assert_eq!(messages, ["a font name of more than 256 bytes is cut"]);
    // The cut counts bytes and falls at the end of a character: 85 characters of three
    // bytes each.
    let document = open(&format!(
        r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="{}"/></w:rPr><w:t>x</w:t></w:r></w:p>"#,
        "€".repeat(200)
    ));
    let translation = translate(&document);
    let style = style_of(&paragraphs(&translation)[0].inlines()[0]);
    let ascii = translation
        .document()
        .run_style(style)
        .unwrap()
        .fonts()
        .ascii()
        .to_owned();
    assert_eq!(ascii, "€".repeat(85));
}

#[test]
fn shares_run_styles_among_many_runs_with_bounded_work() {
    // 2,000 runs of 1,000 distinct sizes: one style per size, and each repetition finds
    // its style again.
    let runs: String = (0..2000)
        .map(|n| {
            format!(
                r#"<w:r><w:rPr><w:sz w:val="{}"/></w:rPr><w:t>x</w:t></w:r>"#,
                2 + n % 1000
            )
        })
        .collect();
    let document = open(&format!("<w:p>{runs}</w:p>"));
    let translation = translate(&document);
    let ids: Vec<ir::RunStyleId> = paragraphs(&translation)[0]
        .inlines()
        .iter()
        .map(style_of)
        .collect();
    for (n, id) in ids.iter().enumerate() {
        assert_eq!(*id, ids[n % 1000], "run {n}");
    }
    assert_ne!(ids[0], ids[1]);
}

/// A complex field: its beginning, instruction, separator, result runs and end.
fn field(instruction: &str, result: &str) -> String {
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{instruction}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{result}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

fn text_run(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

#[test]
fn assembles_fields_with_their_results() {
    let nested = [
        text_run("1 "),
        field("PAGE", &text_run("2")),
        text_run(" 3"),
    ]
    .concat();
    let body = [
        format!(
            "<w:p>{}</w:p>",
            field(" PAGE \\* roman ", r#"<w:r><w:rPr><w:b/></w:rPr><w:t>iv</w:t></w:r>"#)
        ),
        // A field inside the result of another is part of its result.
        format!("<w:p>{}</w:p>", field("DATE \\@ \"d MMMM\"", &nested)),
        // A field without a separator has no result.
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/><w:instrText>NUMPAGES</w:instrText><w:fldChar w:fldCharType="end"/></w:r></w:p>"#.to_owned(),
        // A field that does not end in its paragraph keeps the result it has there.
        format!(
            "<w:p>{}{}</w:p>",
            r#"<w:r><w:fldChar w:fldCharType="begin"/><w:instrText>SECTIONPAGES</w:instrText><w:fldChar w:fldCharType="separate"/></w:r>"#,
            text_run("5")
        ),
        // A separator and an end without a beginning are stray.
        format!(
            "<w:p>{}{}</w:p>",
            r#"<w:r><w:fldChar w:fldCharType="separate"/><w:fldChar w:fldCharType="end"/></w:r>"#,
            text_run("x")
        ),
        r#"<w:p><w:fldSimple w:instr=" PAGE \* ALPHABETIC "><w:r><w:t>B</w:t></w:r></w:fldSimple></w:p>"#.to_owned(),
        // A complex field begun inside a simple field ends with it; what follows the
        // simple field is not part of it (from Copilot's review of #158).
        r#"<w:p><w:fldSimple w:instr="PAGE"><w:r><w:fldChar w:fldCharType="begin"/><w:instrText>NUMPAGES</w:instrText><w:fldChar w:fldCharType="separate"/><w:t>9</w:t></w:r></w:fldSimple><w:r><w:t>after</w:t></w:r></w:p>"#.to_owned(),
        // The legacy page number is a PAGE field.
        r#"<w:p><w:r><w:pgNum/></w:r></w:p>"#.to_owned(),
        // Content before the separator is the instruction, which shows nothing.
        format!(
            "<w:p>{}</w:p>",
            field("PAGE", &text_run("7")).replace(
                r#"<w:r><w:fldChar w:fldCharType="separate"/>"#,
                &format!("{}{}", text_run("hidden"), r#"<w:r><w:fldChar w:fldCharType="separate"/>"#)
            )
        ),
    ]
    .concat();
    let document = open(&body);
    let translation = translate(&document);
    assert_eq!(
        contents(&translation),
        [
            vec!["field Page Some(LowerRoman) [text iv]"],
            vec![r#"field Other("DATE") None [text 1 , text 2, text  3]"#],
            vec!["field NumPages None []"],
            vec!["field SectionPages None [text 5]"],
            vec!["text x"],
            vec!["field Page Some(UpperLetter) [text B]"],
            vec!["field Page None [text 9]", "text after"],
            vec!["field Page None []"],
            vec!["field Page None [text 7]"],
        ]
    );
    // A computed value takes the style of the result's first piece.
    let ir = translation.document();
    let ir::Inline::Field(page) = &paragraphs(&translation)[0].inlines()[0] else {
        panic!("a field");
    };
    assert!(ir.run_style(page.style()).unwrap().bold());
}

#[test]
fn takes_revisions_as_accepted_and_sees_through_containers() {
    let document = open(
        r#"<w:p><w:ins w:id="1" w:author="A"><w:r><w:t>in </w:t></w:r></w:ins><w:del w:id="2" w:author="A"><w:r><w:delText>gone</w:delText></w:r></w:del><w:moveFrom w:id="3" w:author="A"><w:r><w:t>away</w:t></w:r></w:moveFrom><w:moveTo w:id="4" w:author="A"><w:r><w:t>here </w:t></w:r></w:moveTo><w:hyperlink w:anchor="top"><w:r><w:t>link </w:t></w:r></w:hyperlink><w:sdt><w:sdtContent><w:r><w:t>control</w:t></w:r></w:sdtContent></w:sdt><w:bookmarkStart w:id="0" w:name="b"/><w:bookmarkEnd w:id="0"/><w:proofErr w:type="spellStart"/></w:p><w:bookmarkStart w:id="1" w:name="between"/><w:sdt><w:sdtContent><w:p><w:r><w:t>in a block control</w:t></w:r></w:p></w:sdtContent></w:sdt><w:bookmarkEnd w:id="1"/>"#,
    );
    let translation = translate(&document);
    assert_eq!(
        contents(&translation),
        [
            vec!["text in ", "text here ", "text link ", "text control"],
            vec!["text in a block control"],
        ]
    );
    // Bookmarks and proofing marks need no layout and are not listed.
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
}

#[test]
fn lists_what_it_leaves_out() {
    let document = open(
        r#"<w:p><w:r><w:drawing/></w:r><w:r><w:footnoteReference w:id="1"/><w:endnoteReference w:id="2"/><w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/></w:r><m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"/></w:p><w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:altChunk r:id="rId9"/><w:p><w:pPr><w:ind w:leftChars="200"/></w:pPr></w:p><w:sectPr><w:headerReference w:type="default" r:id="rId8"/></w:sectPr>"#,
    );
    let translation = translate(&document);
    let left_out: Vec<String> = translation
        .left_out()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        left_out,
        [
            "sections[0].blocks[0].inlines: a drawing that shows no picture (a shape, chart or group) is left out",
            "sections[0].blocks[0].inlines: a reference to footnote 1, which the document does not have, is left out",
            "sections[0].blocks[0].inlines: a reference to endnote 2, which the document does not have, is left out",
            "sections[0].blocks[0].inlines: an absolute position tab is left out (w:ptab)",
            "sections[0].blocks[0].inlines: an equation is left out",
            "sections[0].blocks[2]: a block-level w:altChunk is left out",
        ]
    );
    assert!(
        translation
            .left_out()
            .iter()
            .all(|entry| entry.kind() == ReportKind::FeatureUnsupported)
    );
}

#[test]
fn lists_what_sections_paragraphs_and_runs_have_that_the_ir_leaves_out() {
    // Issue #303: the PDF shows these otherwise than Word would, so the report says so,
    // each once per document, at its first place: a document may use one on every line,
    // in every row, or in every section (a mail merge writes a section per letter).
    let runs: String = [
        "<w:emboss/>",
        "<w:imprint/>",
        "<w:outline/>",
        "<w:shadow/>",
        r#"<w:em w:val="dot"/>"#,
        r#"<w:bdr w:val="single" w:sz="4"/>"#,
    ]
    .map(|r_pr| format!("<w:r><w:rPr>{r_pr}</w:rPr><w:t>x</w:t></w:r>"))
    .concat();
    let paragraphs = [
        r#"<w:p><w:pPr><w:bidi/></w:pPr></w:p>"#,
        r#"<w:p><w:pPr><w:framePr w:w="2000" w:hAnchor="page"/></w:pPr></w:p>"#,
        r#"<w:p><w:pPr><w:framePr w:dropCap="drop" w:lines="3"/></w:pPr></w:p>"#,
        r#"<w:p><w:pPr><w:rPr><w:del w:id="1" w:author="A" w:date="2026-01-01T00:00:00Z"/></w:rPr></w:pPr><w:r><w:t>first</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tr><w:trPr><w:del w:id="2" w:author="A" w:date="2026-01-01T00:00:00Z"/></w:trPr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#,
    ]
    .concat();
    let section = r#"<w:pgBorders><w:top w:val="single" w:sz="4"/></w:pgBorders><w:lnNumType w:countBy="1"/><w:textDirection w:val="tbRl"/><w:docGrid w:type="lines" w:linePitch="360"/>"#;
    let body = format!(
        "<w:p>{runs}{runs}</w:p>{paragraphs}<w:p><w:pPr><w:sectPr>{section}</w:sectPr></w:pPr></w:p>{paragraphs}<w:p/><w:sectPr>{section}</w:sectPr>"
    );
    let translation = translate(&open(&body));
    let left_out: Vec<String> = translation
        .left_out()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        left_out,
        [
            "sections[0]: page borders (w:pgBorders) are not drawn, here and in every other section that has them",
            "sections[0]: line numbers (w:lnNumType) are not drawn, here and in every other section that has them",
            "sections[0]: the document grid (w:docGrid) is not followed, here and in every other section that has one",
            "sections[0]: vertical text (w:textDirection) is laid out horizontally, here and in every other section that has it",
            "sections[0].blocks[0].inlines: embossed text (w:emboss) is drawn plain, here and wherever else the document has it",
            "sections[0].blocks[0].inlines: engraved text (w:imprint) is drawn plain, here and wherever else the document has it",
            "sections[0].blocks[0].inlines: outlined text (w:outline) is drawn plain, here and wherever else the document has it",
            "sections[0].blocks[0].inlines: shadowed text (w:shadow) is drawn without its shadow, here and wherever else the document has it",
            "sections[0].blocks[0].inlines: emphasis marks (w:em) are not drawn, here and wherever else the document has them",
            "sections[0].blocks[0].inlines: borders around text (w:bdr) are not drawn, here and wherever else the document has them",
            "sections[0].blocks[1]: a right-to-left paragraph (w:bidi) is laid out left to right, here and wherever else the document has one",
            "sections[0].blocks[2]: a text frame (w:framePr) is laid out as an ordinary paragraph, here and wherever else the document has one",
            "sections[0].blocks[3]: a drop cap (w:framePr) is laid out as an ordinary paragraph, here and wherever else the document has one",
            "sections[0].blocks[4]: a paragraph mark deleted or moved away as a tracked change (w:del or w:moveFrom) is kept, so the paragraph is not joined to the next as in Word's PDF of the document without markup, here and wherever else the document has one",
            "sections[0].blocks[5].rows[0]: a table row deleted or moved away as a tracked change (w:del, or w:moveFrom on the marks of its cells) is kept, where Word's PDF of the document without markup leaves it out, here and wherever else the document has one",
        ]
    );
}

#[test]
fn lists_page_borders_of_any_side_or_of_pictures_and_drop_caps_in_the_margin() {
    // Issue #303 and the review of #326: a page border on any one side, or one of
    // pictures (w:art, whose w:val then names the art); a drop cap in the margin.
    let entries = |body: &str| -> Vec<String> {
        translate(&open(body))
            .left_out()
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect()
    };
    let borders =
        "page borders (w:pgBorders) are not drawn, here and in every other section that has them";
    for side in [
        r#"<w:left w:val="single" w:sz="4"/>"#,
        r#"<w:bottom w:val="double" w:sz="4"/>"#,
        r#"<w:right w:val="dotted" w:sz="4"/>"#,
        r#"<w:top w:val="apples" w:sz="20"/>"#,
    ] {
        let body = format!("<w:p/><w:sectPr><w:pgBorders>{side}</w:pgBorders></w:sectPr>");
        assert_eq!(entries(&body), [borders], "{side}");
    }
    assert_eq!(
        entries(r#"<w:p><w:pPr><w:framePr w:dropCap="margin" w:lines="2"/></w:pPr></w:p>"#),
        [
            "a drop cap (w:framePr) is laid out as an ordinary paragraph, here and wherever else the document has one"
        ]
    );
}

#[test]
fn lists_no_feature_of_a_run_that_shows_no_text() {
    // From the review of #326: a hidden run, or one that holds only a field's
    // instruction, shows nothing its features could change.
    let body = r#"<w:p><w:r><w:rPr><w:emboss/><w:vanish/></w:rPr><w:t>hidden</w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:rPr><w:shadow/></w:rPr><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    let translation = translate(&open(body));
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
}

#[test]
fn lists_a_border_around_a_run_of_a_tab_and_the_glyph_features_of_symbols_and_references() {
    // From the re-check of #326: a border is drawn around a tab too (a picture in the
    // line: `tests::drawing`), but the features of glyphs only around what shows text:
    // a symbol, a non-breaking hyphen, a note's reference mark, a legacy page number.
    let messages = |document: &Document| -> Vec<String> {
        translate(document)
            .left_out()
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect()
    };
    let run = |r_pr: &str, content: &str| {
        open(&format!(
            "<w:p><w:r><w:rPr>{r_pr}</w:rPr>{content}</w:r></w:p>"
        ))
    };
    let border = r#"<w:bdr w:val="single" w:sz="4"/>"#;
    let bordered =
        "borders around text (w:bdr) are not drawn, here and wherever else the document has them";
    let embossed =
        "embossed text (w:emboss) is drawn plain, here and wherever else the document has it";
    assert_eq!(messages(&run(border, "<w:tab/>")), [bordered]);
    assert_eq!(
        messages(&run(&format!("{border}<w:emboss/>"), "<w:tab/>")),
        [bordered]
    );
    assert!(messages(&run(&format!("{border}<w:vanish/>"), "<w:tab/>")).is_empty());
    for content in [
        r#"<w:sym w:font="Symbol" w:char="F041"/>"#,
        "<w:noBreakHyphen/>",
        "<w:pgNum/>",
    ] {
        assert_eq!(
            messages(&run("<w:emboss/>", content)),
            [embossed],
            "{content}"
        );
    }
    // A note's reference in the text and the note's own reference mark; the note is
    // translated where it is referred to.
    let footnotes = |r_pr: &str| {
        let footnotes = format!(
            r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1"><w:p><w:r><w:rPr>{r_pr}</w:rPr><w:footnoteRef/></w:r></w:p></w:footnote></w:footnotes>"#
        );
        [("footnotes", "footnotes.xml", footnotes)]
    };
    let entries = |reference: &str, mark: &str| -> Vec<String> {
        let body = format!(
            r#"<w:p><w:r><w:rPr>{reference}</w:rPr><w:footnoteReference w:id="1"/></w:r></w:p>"#
        );
        translate(&with_parts(&body, &footnotes(mark)))
            .left_out()
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    assert_eq!(
        entries("<w:emboss/>", ""),
        [format!("sections[0].blocks[0].inlines: {embossed}")]
    );
    assert_eq!(
        entries("", "<w:emboss/>"),
        [format!("notes[0].blocks[0].inlines: {embossed}")]
    );
    let endnotes =
        format!(r#"<w:endnotes xmlns:w="{W}"><w:endnote w:id="1"><w:p/></w:endnote></w:endnotes>"#);
    let body = r#"<w:p><w:r><w:rPr><w:emboss/></w:rPr><w:endnoteReference w:id="1"/></w:r></w:p>"#;
    let left_out: Vec<String> =
        translate(&with_parts(body, &[("endnotes", "endnotes.xml", endnotes)]))
            .left_out()
            .iter()
            .map(ToString::to_string)
            .collect();
    assert_eq!(
        left_out,
        [format!("sections[0].blocks[0].inlines: {embossed}")]
    );
}

#[test]
fn lists_word_s_text_effects_once_but_no_value_that_changes_nothing() {
    // Issue #339: Word 2010's effects of text ([MS-DOCX] 2.6.1) have no place in the
    // IR's run style, so the PDF draws the text plain. Word writes some with values that
    // change nothing, as in the list levels of LibreOffice's
    // tdf166205_first_page_header_footer_visible.docx: those are no entry. Attributes not
    // written take [MS-DOCX]'s defaults (2.6.3), mostly 0.
    const W14: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
    let messages = |r_pr: &str, content: &str| -> Vec<String> {
        let body =
            format!(r#"<w:p><w:r><w:rPr xmlns:w14="{W14}">{r_pr}</w:rPr>{content}</w:r></w:p>"#);
        translate(&open(&body))
            .left_out()
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect()
    };
    let glow = "glowing text (w14:glow) is drawn without its glow, here and wherever else the document has it";
    let shadow = "text with a shadow effect (w14:shadow) is drawn without its shadow, here and wherever else the document has it";
    let reflection = "a reflection of text (w14:reflection) is not drawn, here and wherever else the document has one";
    let outline = "an outline of text (w14:textOutline) is not drawn, here and wherever else the document has one";
    let fill = "a fill of text (w14:textFill) is not applied, so the text is drawn in its colour, here and wherever else the document has one";
    let depth = "3-D text (w14:scene3d, w14:props3d) is drawn flat, here and wherever else the document has it";
    let red = r#"<w14:srgbClr w14:val="FF0000"/>"#;
    let full = r#"w14:sx="100000" w14:sy="100000""#;
    let front = r#"<w14:camera w14:prst="orthographicFront"/>"#;
    let listed = [
        (format!(r#"<w14:glow w14:rad="63500">{red}</w14:glow>"#), glow),
        (
            format!(
                r#"<w14:shadow w14:blurRad="50800" w14:dist="38100" w14:dir="2700000" w14:sx="100000" w14:sy="100000" w14:kx="0" w14:ky="0" w14:algn="tl">{red}</w14:shadow>"#
            ),
            shadow,
        ),
        // A blurred shadow right under the text shows as a halo; each other difference
        // from one right under it shows too.
        (format!(r#"<w14:shadow w14:blurRad="25400" {full}>{red}</w14:shadow>"#), shadow),
        (format!(r#"<w14:shadow w14:dist="38100" {full}>{red}</w14:shadow>"#), shadow),
        (format!(r#"<w14:shadow w14:sx="90000" w14:sy="100000">{red}</w14:shadow>"#), shadow),
        (format!(r#"<w14:shadow w14:sx="100000" w14:sy="90000">{red}</w14:shadow>"#), shadow),
        (format!(r#"<w14:shadow w14:kx="-800400" {full}>{red}</w14:shadow>"#), shadow),
        (format!(r#"<w14:shadow w14:ky="800400" {full}>{red}</w14:shadow>"#), shadow),
        (
            r#"<w14:reflection w14:blurRad="6350" w14:stA="50000" w14:stPos="0" w14:endA="300" w14:endPos="50000" w14:dist="29997" w14:dir="5400000" w14:fadeDir="5400000" w14:sx="100000" w14:sy="-100000" w14:kx="0" w14:ky="0" w14:algn="bl"/>"#.to_owned(),
            reflection,
        ),
        // Only an end opacity, or only a start one, the other at its default of 0.
        (format!(r#"<w14:reflection w14:endA="300" {full}/>"#), reflection),
        (format!(r#"<w14:reflection w14:stA="47000" {full}/>"#), reflection),
        (
            format!(r#"<w14:textOutline w14:w="9525"><w14:solidFill>{red}</w14:solidFill></w14:textOutline>"#),
            outline,
        ),
        (
            format!(r#"<w14:textOutline w14:w="0"><w14:gradFill><w14:gsLst><w14:gs w14:pos="0">{red}</w14:gs></w14:gsLst></w14:gradFill></w14:textOutline>"#),
            outline,
        ),
        ("<w14:textFill><w14:noFill/></w14:textFill>".to_owned(), fill),
        (
            format!(r#"<w14:textFill><w14:gradFill><w14:gsLst><w14:gs w14:pos="0">{red}</w14:gs></w14:gsLst></w14:gradFill></w14:textFill>"#),
            fill,
        ),
        // Another colour than the text's (automatic), one changed by transparency, black
        // over red text (a fill without a child, a solid fill without a colour), and a
        // theme colour without a theme part to resolve it.
        (format!("<w14:textFill><w14:solidFill>{red}</w14:solidFill></w14:textFill>"), fill),
        (
            r#"<w:color w:val="FF0000"/><w14:textFill><w14:solidFill><w14:srgbClr w14:val="FF0000"><w14:alpha w14:val="74000"/></w14:srgbClr></w14:solidFill></w14:textFill>"#.to_owned(),
            fill,
        ),
        (r#"<w:color w:val="FF0000"/><w14:textFill/>"#.to_owned(), fill),
        (
            r#"<w:color w:val="FF0000"/><w14:textFill><w14:solidFill/></w14:textFill>"#.to_owned(),
            fill,
        ),
        (
            r#"<w14:textFill><w14:solidFill><w14:schemeClr w14:val="tx1"/></w14:solidFill></w14:textFill>"#.to_owned(),
            fill,
        ),
        (
            r#"<w14:scene3d><w14:camera w14:prst="isometricRightUp"/><w14:lightRig w14:rig="threePt" w14:dir="t"/></w14:scene3d>"#.to_owned(),
            depth,
        ),
        (
            r#"<w14:props3d w14:extrusionH="0" w14:contourW="0" w14:prstMaterial="warmMatte"><w14:bevelT w14:w="27940" w14:h="12700" w14:prst="circle"/></w14:props3d>"#.to_owned(),
            depth,
        ),
        (r#"<w14:props3d w14:contourW="8890"/>"#.to_owned(), depth),
    ];
    for (r_pr, message) in &listed {
        assert_eq!(messages(r_pr, "<w:t>x</w:t>"), [*message], "{r_pr}");
    }
    // Values that change nothing, all in one run; the text's own colour as a fill.
    let nothing = format!(
        r#"<w:color w:val="FF0000"/><w14:glow w14:rad="0">{red}</w14:glow><w14:shadow w14:blurRad="0" w14:dist="0" w14:dir="0" w14:sx="0" w14:sy="0" w14:kx="0" w14:ky="0" w14:algn="none">{red}</w14:shadow><w14:reflection w14:blurRad="0" w14:stA="0" w14:stPos="0" w14:endA="0" w14:endPos="0" w14:dist="0" w14:dir="0" w14:fadeDir="0" w14:sx="0" w14:sy="0" w14:kx="0" w14:ky="0" w14:algn="none"/><w14:textOutline w14:w="0" w14:cap="rnd" w14:cmpd="sng" w14:algn="ctr"><w14:noFill/><w14:prstDash w14:val="solid"/><w14:bevel/></w14:textOutline><w14:textFill><w14:solidFill>{red}</w14:solidFill></w14:textFill><w14:scene3d>{front}<w14:lightRig w14:rig="threePt" w14:dir="t"><w14:rot w14:lat="0" w14:lon="0" w14:rev="0"/></w14:lightRig></w14:scene3d><w14:props3d w14:extrusionH="0" w14:contourW="0" w14:prstMaterial="none"/>"#
    );
    assert!(messages(&nothing, "<w:t>x</w:t>").is_empty());
    for r_pr in [
        "<w14:glow/>".to_owned(),
        // Scaled to nothing: sx and sy are 0 unless written.
        format!("<w14:shadow>{red}</w14:shadow>"),
        format!(r#"<w14:shadow w14:blurRad="25400">{red}</w14:shadow>"#),
        format!(r#"<w14:shadow w14:blurRad="25400" w14:sx="0" w14:sy="100000">{red}</w14:shadow>"#),
        format!(r#"<w14:shadow w14:blurRad="25400" w14:sx="100000" w14:sy="0">{red}</w14:shadow>"#),
        format!(r#"<w14:shadow {full}>{red}</w14:shadow>"#),
        "<w14:reflection/>".to_owned(),
        format!("<w14:reflection {full}/>"),
        r#"<w14:reflection w14:stA="50000" w14:sx="0" w14:sy="-100000"/>"#.to_owned(),
        r#"<w14:reflection w14:stA="50000" w14:sx="100000" w14:sy="0"/>"#.to_owned(),
        r#"<w14:textOutline w14:w="9525"/>"#.to_owned(),
        // Black over black text.
        r#"<w:color w:val="000000"/><w14:textFill/>"#.to_owned(),
        r#"<w:color w:val="000000"/><w14:textFill><w14:solidFill/></w14:textFill>"#.to_owned(),
        format!("<w14:scene3d>{front}</w14:scene3d>"),
        // A bevel without a width or a height (0 unless written), and depth and a bottom
        // bevel facing away from a camera in front.
        "<w14:props3d><w14:bevelT/></w14:props3d>".to_owned(),
        r#"<w14:props3d><w14:bevelT w14:w="38100"/></w14:props3d>"#.to_owned(),
        r#"<w14:props3d><w14:bevelT w14:h="38100"/></w14:props3d>"#.to_owned(),
        r#"<w14:props3d w14:extrusionH="57150"><w14:bevelB w14:w="38100" w14:h="38100"/></w14:props3d>"#.to_owned(),
    ] {
        assert!(messages(&r_pr, "<w:t>x</w:t>").is_empty(), "{r_pr}");
    }
    // A hidden run, or one with no text, shows nothing they could change.
    let glowing = format!(r#"<w14:glow w14:rad="63500">{red}</w14:glow>"#);
    assert!(messages(&format!("<w:vanish/>{glowing}"), "<w:t>x</w:t>").is_empty());
    assert!(messages(&glowing, "<w:tab/>").is_empty());
    // What a style gives counts, effect by effect, and a run's own value that changes
    // nothing wins over it.
    let off = [
        r#"<w14:glow w14:rad="0"/>"#,
        "<w14:shadow/>",
        "<w14:reflection/>",
        "<w14:textOutline><w14:noFill/></w14:textOutline>",
        r#"<w14:textFill><w14:solidFill><w14:srgbClr w14:val="000000"/></w14:solidFill></w14:textFill>"#,
        format!("<w14:scene3d>{front}</w14:scene3d>").leak(),
        "<w14:props3d/>",
    ];
    let given = [0, 1, 8, 11, 13, 20, 21];
    for (&at, own) in given.iter().zip(off) {
        let (r_pr, message) = &listed[at];
        let styles = (
            "styles",
            "styles.xml",
            format!(
                r#"<w:styles xmlns:w="{W}" xmlns:w14="{W14}"><w:style w:type="character" w:styleId="Effect"><w:name w:val="Effect"/><w:rPr>{r_pr}</w:rPr></w:style></w:styles>"#
            ),
        );
        let entries = |own: &str| -> Vec<String> {
            let body = format!(
                r#"<w:p><w:r><w:rPr xmlns:w14="{W14}"><w:rStyle w:val="Effect"/><w:color w:val="000000"/>{own}</w:rPr><w:t>x</w:t></w:r></w:p>"#
            );
            translate(&with_parts(&body, std::slice::from_ref(&styles)))
                .left_out()
                .iter()
                .map(|entry| entry.message().to_owned())
                .collect()
        };
        assert_eq!(entries(""), [*message], "{r_pr}");
        assert!(entries(own).is_empty(), "{own} over {r_pr}");
    }
    // A fill of a theme colour is the theme's colour: the text's own changes nothing.
    let theme = (
        "theme",
        "theme/theme1.xml",
        format!(
            r#"<a:theme xmlns:a="{A}" name="T"><a:themeElements><a:clrScheme name="C"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="111111"/></a:dk2><a:lt2><a:srgbClr val="EEEEEE"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2><a:accent3><a:srgbClr val="030303"/></a:accent3><a:accent4><a:srgbClr val="040404"/></a:accent4><a:accent5><a:srgbClr val="050505"/></a:accent5><a:accent6><a:srgbClr val="060606"/></a:accent6><a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink></a:clrScheme><a:fontScheme name="F"><a:majorFont><a:latin typeface="Major"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Minor"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="S"/></a:themeElements></a:theme>"#
        ),
    );
    let themed = |scheme: &str| -> Vec<String> {
        let body = format!(
            r#"<w:p><w:r><w:rPr xmlns:w14="{W14}"><w:color w:val="4F81BD" w:themeColor="accent1"/><w14:textFill><w14:solidFill><w14:schemeClr w14:val="{scheme}"/></w14:solidFill></w14:textFill></w:rPr><w:t>x</w:t></w:r></w:p>"#
        );
        translate(&with_parts(&body, std::slice::from_ref(&theme)))
            .left_out()
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect()
    };
    assert!(themed("accent1").is_empty());
    assert_eq!(themed("accent2"), [fill]);
    // Each colour of the scheme is its theme colour, text and background through the
    // document's mapping, which here swaps them: a fill of each over text of its own theme
    // colour changes nothing, and one over another does.
    let mapping = (
        "settings",
        "settings.xml",
        format!(
            r#"<w:settings xmlns:w="{W}"><w:clrSchemeMapping w:bg1="dark1" w:t1="light1" w:bg2="dark2" w:t2="light2" w:accent1="accent1" w:accent2="accent2" w:accent3="accent3" w:accent4="accent4" w:accent5="accent5" w:accent6="accent6" w:hyperlink="hyperlink" w:followedHyperlink="followedHyperlink"/></w:settings>"#
        ),
    );
    let mapped = |text: &str, scheme: &str| -> Vec<String> {
        let body = format!(
            r#"<w:p><w:r><w:rPr xmlns:w14="{W14}"><w:color w:val="000000" w:themeColor="{text}"/><w14:textFill><w14:solidFill><w14:schemeClr w14:val="{scheme}"/></w14:solidFill></w14:textFill></w:rPr><w:t>x</w:t></w:r></w:p>"#
        );
        translate(&with_parts(&body, &[theme.clone(), mapping.clone()]))
            .left_out()
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect()
    };
    let pairs = [
        ("background1", "bg1"),
        ("text1", "tx1"),
        ("background2", "bg2"),
        ("text2", "tx2"),
        ("dark1", "dk1"),
        ("light1", "lt1"),
        ("dark2", "dk2"),
        ("light2", "lt2"),
        ("accent3", "accent3"),
        ("accent4", "accent4"),
        ("accent5", "accent5"),
        ("accent6", "accent6"),
        ("hyperlink", "hlink"),
        ("followedHyperlink", "folHlink"),
    ];
    for (text, scheme) in pairs {
        assert!(mapped(text, scheme).is_empty(), "{scheme} over {text}");
    }
    // tx1 is light 1 through the mapping, dk1 dark 1 whatever the mapping says.
    assert_eq!(mapped("text1", "dk1"), [fill]);
    assert_eq!(mapped("dark1", "tx1"), [fill]);
    // A gradient is listed over text of any colour, black too.
    let gradient = r#"<w:color w:val="000000"/><w14:textFill><w14:gradFill><w14:gsLst><w14:gs w14:pos="0"><w14:srgbClr w14:val="000000"/></w14:gs></w14:gsLst></w14:gradFill></w14:textFill>"#;
    assert_eq!(messages(gradient, "<w:t>x</w:t>"), [fill]);
}

#[test]
fn lists_a_deleted_mark_only_where_a_block_follows_it() {
    // From the re-checks of #326: Word joins a paragraph whose mark is deleted to what
    // follows it, but the last paragraph of a cell or a story has nothing after it. Word
    // marks those of every cell of a row it deletes or moves away, and such a row is
    // listed as a row.
    let mark = |kind: &str, section: &str| {
        format!(
            r#"<w:p><w:pPr><w:rPr><w:{kind} w:id="1" w:author="A" w:date="2026-01-01T00:00:00Z"/></w:rPr>{section}</w:pPr><w:r><w:t>text</w:t></w:r></w:p>"#
        )
    };
    let deleted = mark("del", "");
    let entries = |document: &Document| -> Vec<String> {
        translate(document)
            .left_out()
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    let message = "a paragraph mark deleted or moved away as a tracked change (w:del or w:moveFrom) is kept, so the paragraph is not joined to the next as in Word's PDF of the document without markup, here and wherever else the document has one";
    let row = |cell: &str| format!("<w:tbl><w:tr><w:tc>{cell}</w:tc></w:tr></w:tbl>");
    // A row of two cells, of which only the first's last mark is deleted, and one of
    // which only the last's is (LibreOffice's tdf157011_ins_del_empty_cols.docx).
    let one_of_two =
        |cell: &str| format!("<w:tbl><w:tr><w:tc>{cell}</w:tc><w:tc><w:p/></w:tc></w:tr></w:tbl>");
    let last_of_two =
        |cell: &str| format!("<w:tbl><w:tr><w:tc><w:p/></w:tc><w:tc>{cell}</w:tc></w:tr></w:tbl>");
    for body in [
        format!("{}<w:p/>", one_of_two(&deleted)),
        format!("{}<w:p/>", last_of_two(&deleted)),
        format!(
            r#"{}<w:p/>"#,
            one_of_two(&format!(r#"<w:p/>{deleted}<w:bookmarkEnd w:id="0"/>"#))
        ),
        format!("<w:p/>{deleted}"),
    ] {
        assert!(entries(&open(&body)).is_empty(), "{body}");
    }
    for (body, at) in [
        (
            format!("{}<w:p/>", row(&format!("{deleted}<w:p/>"))),
            "sections[0].blocks[0].rows[0].cells[0].blocks[0]",
        ),
        (
            format!("<w:p/>{}<w:p/>", mark("moveFrom", "")),
            "sections[0].blocks[1]",
        ),
        (
            format!("{deleted}{}<w:p/>", row("<w:p/>")),
            "sections[0].blocks[0]",
        ),
        (
            format!("{deleted}<w:sdt><w:sdtContent><w:p/></w:sdtContent></w:sdt>"),
            "sections[0].blocks[0]",
        ),
        // Word removes the section break with the mark that holds it.
        (
            format!("{}<w:p/>", mark("del", "<w:sectPr/>")),
            "sections[0].blocks[0]",
        ),
    ] {
        assert_eq!(
            entries(&open(&body)),
            [format!("{at}: {message}")],
            "{body}"
        );
    }
    // A header's last paragraph, translated between two bodies, joins neither, and does
    // not stop the end of the first from joining the second.
    let header = (
        "header",
        "header1.xml",
        format!(r#"<w:hdr xmlns:w="{W}"><w:p/>{deleted}</w:hdr>"#),
    );
    let reference = r#"<w:headerReference w:type="default" r:id="rId0"/>"#;
    let body = format!("<w:p/><w:p/><w:sectPr>{reference}</w:sectPr>");
    assert!(entries(&with_parts(&body, std::slice::from_ref(&header))).is_empty());
    let body = format!(
        "{}<w:p/><w:sectPr>{reference}</w:sectPr>",
        mark("del", "<w:sectPr/>")
    );
    assert_eq!(
        entries(&with_parts(&body, &[header])),
        [format!("sections[0].blocks[0]: {message}")]
    );
    // So does a note's.
    let footnotes = format!(
        r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1">{deleted}</w:footnote></w:footnotes>"#
    );
    let body = r#"<w:p><w:r><w:footnoteReference w:id="1"/></w:r></w:p><w:p/>"#;
    assert!(
        entries(&with_parts(
            body,
            &[("footnotes", "footnotes.xml", footnotes)]
        ))
        .is_empty()
    );
    // The note separators, translated after the last body, do not join its last
    // paragraph either.
    let separators = format!(
        r#"<w:footnotes xmlns:w="{W}"><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p/></w:footnote></w:footnotes>"#
    );
    let body = format!(r#"<w:p><w:r><w:footnoteReference w:id="1"/></w:r></w:p>{deleted}"#);
    assert!(
        entries(&with_parts(
            &body,
            &[("footnotes", "footnotes.xml", separators)]
        ))
        .is_empty()
    );
    // A row whose cells all end with a mark moved away (TC-table-rowDND.docx of
    // LibreOffice's tests, written by Word) or deleted, with nothing in its w:trPr.
    let row_message = "a table row deleted or moved away as a tracked change (w:del, or w:moveFrom on the marks of its cells) is kept, where Word's PDF of the document without markup leaves it out, here and wherever else the document has one";
    for kind in ["moveFrom", "del"] {
        let gone = mark(kind, "");
        let body = format!(
            "<w:tbl><w:tr><w:tc>{gone}</w:tc><w:tc><w:p/>{gone}</w:tc></w:tr></w:tbl><w:p/>"
        );
        assert_eq!(
            entries(&open(&body)),
            [format!("sections[0].blocks[0].rows[0]: {row_message}")],
            "{kind}"
        );
    }
    // Not when a cell ends with a table after the deleted mark, which is listed as a mark
    // (a cell that ends with no paragraph gets one, which is listed too).
    let nested = row("<w:p/>");
    let body = format!("<w:tbl><w:tr><w:tc>{deleted}{nested}</w:tc></w:tr></w:tbl><w:p/>");
    let found = entries(&open(&body));
    assert_eq!(
        found.first(),
        Some(&format!(
            "sections[0].blocks[0].rows[0].cells[0].blocks[0]: {message}"
        ))
    );
    assert!(
        !found.iter().any(|entry| entry.contains("table row")),
        "{found:?}"
    );
}

#[test]
fn lists_rows_and_cells_deleted_as_tracked_changes_but_not_from_a_table_style() {
    // From the re-check of #326: a deleted row of a table with a style, whose row
    // properties the style's are laid under, and a cell deleted with w:cellDel; a table
    // style records no tracked change, so one written in it deletes nothing.
    let revision = r#"w:id="1" w:author="A" w:date="2026-01-01T00:00:00Z""#;
    let styles = (
        "styles",
        "styles.xml",
        format!(
            r#"<w:styles xmlns:w="{W}"><w:style w:type="table" w:styleId="Grid"><w:name w:val="Grid"/><w:trPr><w:del {revision}/></w:trPr><w:tcPr><w:cellDel {revision}/></w:tcPr><w:tblStylePr w:type="firstRow"><w:trPr><w:del {revision}/></w:trPr><w:tcPr><w:cellDel {revision}/></w:tcPr></w:tblStylePr></w:style></w:styles>"#
        ),
    );
    let table = |tr_pr: &str, tc_pr: &str| {
        format!(
            r#"<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/></w:tblPr><w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="1000"/></w:tblGrid><w:tr><w:trPr>{tr_pr}</w:trPr><w:tc><w:p/></w:tc><w:tc><w:tcPr>{tc_pr}</w:tcPr><w:p/></w:tc></w:tr></w:tbl><w:p/>"#
        )
    };
    let entries = |body: &str| -> Vec<String> {
        translate(&with_parts(body, std::slice::from_ref(&styles)))
            .left_out()
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    assert!(entries(&table("", "")).is_empty());
    assert_eq!(
        entries(&table(&format!("<w:del {revision}/>"), "")),
        [
            "sections[0].blocks[0].rows[0]: a table row deleted or moved away as a tracked change (w:del, or w:moveFrom on the marks of its cells) is kept, where Word's PDF of the document without markup leaves it out, here and wherever else the document has one"
        ]
    );
    assert_eq!(
        entries(&table("", &format!("<w:cellDel {revision}/>"))),
        [
            "sections[0].blocks[0].rows[0].cells[1]: a table cell deleted as a tracked change (w:cellDel) is kept, where Word's PDF of the document without markup leaves it out, here and wherever else the document has one"
        ]
    );
}

#[test]
fn lists_nothing_for_what_is_off_and_follows_styles() {
    // Issue #303: a section or run feature that is off, empty or none is no entry; one a
    // style gives is, and direct formatting that turns it off wins.
    let styles = (
        "styles",
        "styles.xml",
        format!(
            r#"<w:styles xmlns:w="{W}"><w:style w:type="character" w:styleId="Embossed"><w:name w:val="Embossed"/><w:rPr><w:emboss/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Framed"><w:name w:val="Framed"/><w:pPr><w:framePr w:w="2000"/></w:pPr></w:style></w:styles>"#
        ),
    );
    let off = r#"<w:p><w:r><w:rPr><w:rStyle w:val="Embossed"/><w:emboss w:val="0"/><w:imprint w:val="false"/><w:em w:val="none"/><w:bdr w:val="none"/></w:rPr><w:t>x</w:t></w:r><w:r><w:rPr><w:bdr w:val="nil"/></w:rPr><w:t>x</w:t></w:r></w:p><w:p><w:pPr><w:bidi w:val="0"/></w:pPr></w:p><w:sectPr><w:pgBorders><w:top w:val="none"/><w:left w:val="nil"/></w:pgBorders><w:lnNumType w:countBy="0"/><w:textDirection w:val="lrTb"/><w:docGrid w:type="default" w:linePitch="360"/></w:sectPr>"#;
    let translation = translate(&with_parts(off, std::slice::from_ref(&styles)));
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
    // What the styles give.
    let on = r#"<w:p><w:pPr><w:pStyle w:val="Framed"/></w:pPr><w:r><w:rPr><w:rStyle w:val="Embossed"/></w:rPr><w:t>x</w:t></w:r></w:p>"#;
    let translation = translate(&with_parts(on, &[styles]));
    let left_out: Vec<&str> = translation
        .left_out()
        .iter()
        .map(|entry| entry.message())
        .collect();
    assert_eq!(
        left_out,
        [
            "a text frame (w:framePr) is laid out as an ordinary paragraph, here and wherever else the document has one",
            "embossed text (w:emboss) is drawn plain, here and wherever else the document has it",
        ]
    );
}

#[test]
fn lists_at_most_a_thousand_entries() {
    let drawings = r#"<w:r><w:drawing/></w:r>"#.repeat(1200);
    let document = open(&format!("<w:p>{drawings}</w:p>"));
    let translation = translate(&document);
    let left_out = translation.left_out();
    // 999 listed, and one counting the others.
    assert_eq!(left_out.len(), 1000);
    assert_eq!(left_out[999].message(), "201 more entries are not listed");
    assert_eq!(left_out[999].location().to_string(), "document");
    // What is listed once is counted too, once the list is full.
    let document = open(&format!(
        "<w:p>{drawings}<w:r><w:rPr><w:emboss/></w:rPr><w:t>x</w:t></w:r><w:r><w:rPr><w:emboss/></w:rPr><w:t>y</w:t></w:r></w:p>"
    ));
    let translation = translate(&document);
    let left_out = translation.left_out();
    assert_eq!(left_out.len(), 1000);
    assert_eq!(left_out[999].message(), "202 more entries are not listed");
}

#[test]
fn translates_paragraph_formatting() {
    let theme = format!(
        r#"<a:theme xmlns:a="{A}" name="T"><a:themeElements><a:clrScheme name="C"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="111111"/></a:dk2><a:lt2><a:srgbClr val="EEEEEE"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2><a:accent3><a:srgbClr val="030303"/></a:accent3><a:accent4><a:srgbClr val="040404"/></a:accent4><a:accent5><a:srgbClr val="050505"/></a:accent5><a:accent6><a:srgbClr val="060606"/></a:accent6><a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink></a:clrScheme><a:fontScheme name="F"><a:majorFont><a:latin typeface="Major"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Minor"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="S"/></a:themeElements></a:theme>"#
    );
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading"><w:name w:val="Heading"/><w:pPr><w:keepNext/><w:pBdr><w:top w:val="double" w:sz="8" w:space="2" w:color="FF0000" w:themeColor="accent1"/><w:bottom w:val="none" w:sz="4"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="DDDDDD" w:themeFill="accent2"/><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9000"/><w:tab w:val="decimal" w:pos="4000"/></w:tabs><w:spacing w:before="240" w:after="120" w:line="360" w:lineRule="auto"/><w:ind w:left="720" w:right="360" w:hanging="360"/><w:contextualSpacing/><w:jc w:val="center"/><w:outlineLvl w:val="1"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Other"><w:name w:val="Other"/></w:style></w:styles>"#
    );
    let body = [
        r#"<w:p><w:pPr><w:pStyle w:val="Heading"/><w:tabs><w:tab w:val="clear" w:pos="4000"/></w:tabs></w:pPr></w:p>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="Heading"/><w:spacing w:line="300" w:lineRule="exact"/><w:jc w:val="both"/></w:pPr></w:p>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="Other"/><w:spacing w:line="280" w:lineRule="atLeast"/><w:jc w:val="right"/></w:pPr></w:p>"#,
        "<w:p/>",
        r#"<w:p><w:pPr><w:pStyle w:val="Normal"/></w:pPr></w:p>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="Missing"/></w:pPr></w:p>"#,
    ]
    .concat();
    let document = with_parts(
        &body,
        &[
            ("styles", "styles.xml", styles),
            ("theme", "theme/theme1.xml", theme),
        ],
    );
    let translation = translate(&document);
    let found = paragraphs(&translation);
    let heading = found[0].props();
    assert_eq!(heading.alignment(), ir::Alignment::Center);
    assert_eq!(
        heading.indent(),
        ir::Indent::default()
            .with_start(Pt(36.0))
            .with_end(Pt(18.0))
            .with_first_line(Pt(-18.0))
    );
    assert_eq!(
        heading.spacing(),
        ir::Spacing::default()
            .with_before(Pt(12.0))
            .with_after(Pt(6.0))
            .with_line(ir::LineSpacing::Multiple(1.5))
    );
    assert!(heading.keep_next() && heading.contextual_spacing());
    assert_eq!(heading.outline_level(), Some(1));
    // The cleared stop of the style is gone.
    assert_eq!(
        heading.tabs(),
        [ir::TabStop::default()
            .with_position(Pt(450.0))
            .with_kind(ir::TabKind::End)
            .with_leader(ir::TabLeader::Dot)]
    );
    // Theme colours are resolved; a border of style none is no border.
    let rgb =
        |red, green, blue| ir::ColorValue::Rgb(officewright_core::Color::rgb(red, green, blue));
    let borders = heading.borders();
    assert_eq!(
        borders.top(),
        Some(
            ir::Border::default()
                .with_style(ir::BorderStyle::Double)
                .with_width(Pt(1.0))
                .with_space(Pt(2.0))
                .with_color(rgb(0x4F, 0x81, 0xBD))
        )
    );
    assert_eq!(borders.bottom(), None);
    assert_eq!(
        heading.shading(),
        Some(ir::Shading::default().with_fill(rgb(0xC0, 0x50, 0x4D)))
    );
    let exact = found[1].props();
    assert_eq!(exact.alignment(), ir::Alignment::Both);
    assert_eq!(exact.spacing().line(), ir::LineSpacing::Exact(Pt(15.0)));
    let other = found[2].props();
    assert_eq!(other.alignment(), ir::Alignment::End);
    assert_eq!(other.spacing().line(), ir::LineSpacing::AtLeast(Pt(14.0)));
    // One key per paragraph style, for contextual spacing; a paragraph without w:pStyle
    // has the default style's.
    let keys: Vec<u32> = found
        .iter()
        .map(|paragraph| paragraph.props().style_key().value())
        .collect();
    assert_eq!(keys[0], keys[1]);
    assert!(keys[0] != keys[2] && keys[2] != keys[3] && keys[0] != keys[3]);
    // Naming the default style, or a style that does not exist, is the same style as
    // naming none; and no style's key is 0.
    assert_eq!((keys[4], keys[5]), (keys[3], keys[3]));
    assert!(keys.iter().all(|&key| key != 0));
    // Without any paragraph style, the key is 0.
    let plain = translate(&open("<w:p/>"));
    assert_eq!(paragraphs(&plain)[0].props().style_key().value(), 0);
}

#[test]
fn translates_lists_as_paragraphs_use_them() {
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="10"><w:lvl w:ilvl="0"><w:start w:val="3"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="right"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="lowerLetter"/><w:lvlRestart w:val="0"/><w:lvlText w:val="%1.%2)"/><w:suff w:val="space"/><w:isLgl/></w:lvl><w:lvl w:ilvl="3"><w:numFmt w:val="bullet"/><w:lvlText w:val="%1"/><w:lvlRestart w:val="2"/><w:suff w:val="nothing"/><w:rPr><w:b/><w:highlight w:val="yellow"/></w:rPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="10"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride><w:lvlOverride w:ilvl="1"><w:lvl w:ilvl="1"><w:numFmt w:val="lowerRoman"/><w:lvlText w:val="(%2)"/></w:lvl></w:lvlOverride></w:num><w:num w:numId="3"><w:abstractNumId w:val="99"/></w:num></w:numbering>"#
    );
    let item = |instance: i64, level: i64| {
        format!(
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{instance}"/></w:numPr></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#
        )
    };
    let body = [
        item(1, 0),
        item(1, 3),
        item(2, 0),
        item(2, 1),
        item(3, 0),
        item(1, 0),
    ]
    .concat();
    let document = with_parts(&body, &[("numbering", "numbering.xml", numbering)]);
    let translation = translate(&document);
    let ir = translation.document();
    let items: Vec<Option<ir::ListItem>> =
        paragraphs(&translation).iter().map(|p| p.list()).collect();
    let [
        Some(first),
        Some(bullet),
        Some(second),
        Some(roman),
        None,
        Some(again),
    ] = items[..]
    else {
        panic!("{items:?}");
    };
    // One IR instance per instance of the document.
    assert_eq!(first.instance(), again.instance());
    assert_eq!(first.instance(), bullet.instance());
    assert_ne!(first.instance(), second.instance());
    assert_eq!((first.level(), bullet.level(), roman.level()), (0, 3, 1));
    // The label of the bullet level is bold and highlighted: Word draws a level's
    // highlight (word-rules/E-list-label-rstyle.docx, case 02, #308).
    let (bullet_style, first_style) = (
        ir.run_style(bullet.label_style()).unwrap(),
        ir.run_style(first.label_style()).unwrap(),
    );
    assert!(bullet_style.bold() && bullet_style.highlight().is_some());
    assert!(!first_style.bold() && first_style.highlight().is_none());

    // Both instances use one definition.
    let one = ir.list_instance(first.instance()).unwrap();
    let two = ir.list_instance(second.instance()).unwrap();
    assert_eq!(one.definition(), two.definition());
    assert!(one.overrides().is_empty());
    let definition = ir.list(one.definition()).unwrap();
    let levels = definition.levels();
    // Levels up to the last one written; level 2, left out, is empty.
    assert_eq!(levels.len(), 4);
    use ir::LevelTextPart::{Literal, Placeholder};
    assert_eq!(
        levels[0],
        ir::Level::default()
            .with_start(3)
            .with_text([Placeholder(0), Literal(".".into())])
            .with_alignment(ir::LevelAlignment::End)
    );
    assert_eq!(
        levels[1],
        ir::Level::default()
            .with_format(ir::NumberFormat::LowerLetter)
            .with_restart(ir::Restart::Never)
            .with_text([
                Placeholder(0),
                Literal(".".into()),
                Placeholder(1),
                Literal(")".into())
            ])
            .with_suffix(ir::Suffix::Space)
            .with_legal(true)
    );
    assert_eq!(levels[2], ir::Level::default());
    // A bullet's text is shown as it is.
    assert_eq!(
        levels[3],
        ir::Level::default()
            .with_format(ir::NumberFormat::Bullet)
            .with_text([Literal("%1".into())])
            .with_restart(ir::Restart::AfterLevel(1))
            .with_suffix(ir::Suffix::Nothing)
    );
    // Instance 2's overrides: a start, and a replacement level.
    let overrides = two.overrides();
    assert_eq!(overrides.len(), 2);
    assert_eq!((overrides[0].level(), overrides[0].start()), (0, Some(5)));
    assert_eq!(overrides[0].replacement(), &None);
    assert_eq!(
        overrides[1].replacement(),
        &Some(
            ir::Level::default()
                .with_format(ir::NumberFormat::LowerRoman)
                .with_text([Literal("(".into()), Placeholder(1), Literal(")".into())])
        )
    );
}

#[test]
fn translates_settings_and_sections() {
    let settings = format!(
        r#"<w:settings xmlns:w="{W}"><w:evenAndOddHeaders/><w:defaultTabStop w:val="360"/><w:autoHyphenation/><w:consecutiveHyphenLimit w:val="2"/><w:hyphenationZone w:val="200"/><w:doNotHyphenateCaps/><w:footnotePr><w:pos w:val="sectEnd"/><w:numFmt w:val="upperLetter"/></w:footnotePr><w:endnotePr><w:pos w:val="sectEnd"/></w:endnotePr><w:compat><w:doNotExpandShiftReturn/><w:usePrinterMetrics/><w:noLeading w:val="0"/><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="14"/><w:compatSetting w:name="enableOpenTypeFeatures" w:uri="http://schemas.microsoft.com/office/word" w:val="1"/><w:compatSetting w:name="differentiateMultirowTableHeaders" w:uri="http://schemas.microsoft.com/office/word" w:val="0"/></w:compat><w:decimalSymbol w:val=","/></w:settings>"#
    );
    let body = r#"<w:p><w:pPr><w:sectPr><w:footnotePr><w:pos w:val="beneathText"/><w:numStart w:val="2"/><w:numRestart w:val="eachPage"/></w:footnotePr><w:endnotePr><w:pos w:val="docEnd"/><w:numFmt w:val="decimal"/></w:endnotePr><w:type w:val="continuous"/><w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/><w:pgMar w:top="1000" w:right="1100" w:bottom="1200" w:left="1300" w:header="400" w:footer="500" w:gutter="100"/><w:pgNumType w:fmt="lowerRoman" w:start="3"/><w:cols w:num="2" w:space="720" w:sep="1"/><w:titlePg/><w:vAlign w:val="center"/></w:sectPr></w:pPr></w:p><w:p/><w:sectPr><w:cols w:num="2" w:equalWidth="0"><w:col w:w="3000" w:space="400"/><w:col w:w="5000"/></w:cols></w:sectPr>"#;
    let document = with_parts(body, &[("settings", "settings.xml", settings)]);
    let translation = translate(&document);
    let ir = translation.document();
    let settings = ir.settings();
    assert_eq!(settings.compat_mode(), ir::CompatMode::Word2010);
    let flags: Vec<&str> = settings
        .compat_flags()
        .iter()
        .map(ir::CompatFlag::name)
        .collect();
    assert_eq!(
        flags,
        [
            "doNotExpandShiftReturn",
            "usePrinterMetrics",
            "enableOpenTypeFeatures"
        ]
    );
    assert_eq!(settings.default_tab_stop(), Pt(18.0));
    assert_eq!(settings.decimal_separator(), ',');
    assert!(settings.even_and_odd_headers());
    assert_eq!(
        settings.hyphenation(),
        ir::Hyphenation::default()
            .with_auto(true)
            .with_zone(Pt(10.0))
            .with_consecutive_limit(2)
            .with_hyphenate_caps(false)
    );

    let sections = ir.sections();
    assert_eq!(sections.len(), 2);
    let first = &sections[0];
    let page = first.page();
    assert_eq!(
        page.size(),
        officewright_core::PageSize::new(Pt(841.9), Pt(595.3))
    );
    assert_eq!(page.orientation(), ir::Orientation::Landscape);
    assert_eq!(
        page.margins(),
        ir::Margins::default()
            .with_top(Pt(50.0))
            .with_right(Pt(55.0))
            .with_bottom(Pt(60.0))
            .with_left(Pt(65.0))
            .with_header(Pt(20.0))
            .with_footer(Pt(25.0))
            .with_gutter(Pt(5.0))
    );
    assert_eq!(
        first.columns(),
        &ir::Columns::default()
            .with_count(2)
            .with_spacing(Pt(36.0))
            .with_separator(true)
    );
    assert_eq!(first.start(), ir::SectionStart::Continuous);
    assert!(first.title_page());
    assert_eq!(
        first.page_numbering(),
        &ir::PageNumbering::default()
            .with_format(ir::NumberFormat::LowerRoman)
            .with_start(Some(3))
    );
    assert_eq!(first.vertical_align(), ir::PageVerticalAlign::Center);
    // Footnotes: the settings' number format, and the section's position, start and
    // restart; Word ignores the settings' position (MS-OI29500 2.1.309 a).
    assert_eq!(
        first.footnotes(),
        &ir::NoteProps::footnotes()
            .with_format(ir::NumberFormat::UpperLetter)
            .with_position(ir::NotePosition::BeneathText)
            .with_start(2)
            .with_restart(ir::NoteRestart::EachPage)
    );
    // Endnotes: the settings' position, not the section's (§17.11.22); the section's
    // number format.
    assert_eq!(
        first.endnotes(),
        &ir::NoteProps::endnotes()
            .with_position(ir::NotePosition::SectionEnd)
            .with_format(ir::NumberFormat::Decimal)
    );
    assert_eq!(first.blocks().len(), 1);

    // No page size: Letter; columns of their own widths.
    let last = &sections[1];
    assert_eq!(last.page().size(), officewright_core::PageSize::LETTER);
    assert_eq!(last.page().margins(), ir::Margins::default());
    assert_eq!(
        last.columns().widths(),
        [
            ir::Column::default()
                .with_width(Pt(150.0))
                .with_spacing(Pt(20.0)),
            ir::Column::default().with_width(Pt(250.0)),
        ]
    );
    assert_eq!(last.start(), ir::SectionStart::NextPage);
    assert_eq!(
        last.footnotes(),
        &ir::NoteProps::footnotes().with_format(ir::NumberFormat::UpperLetter)
    );
    assert_eq!(
        last.endnotes(),
        &ir::NoteProps::endnotes().with_position(ir::NotePosition::SectionEnd)
    );
}

#[test]
fn translates_the_rest_of_paragraph_formatting() {
    let body = [
        r#"<w:p><w:pPr><w:keepLines/><w:pageBreakBefore/><w:widowControl w:val="0"/><w:wordWrap w:val="0"/><w:suppressAutoHyphens/><w:spacing w:before="100" w:beforeAutospacing="1"/><w:tabs><w:tab w:val="center" w:leader="hyphen" w:pos="2000"/></w:tabs><w:pBdr><w:between w:val="single" w:sz="4" w:space="1" w:color="000000" w:shadow="1"/></w:pBdr><w:jc w:val="distribute"/></w:pPr></w:p>"#,
        r#"<w:p><w:pPr><w:jc w:val="lowKashida"/></w:pPr></w:p>"#,
        // A hanging indent in character units is a negative count of them.
        r#"<w:p><w:pPr><w:ind w:hangingChars="100"/></w:pPr></w:p>"#,
    ]
    .concat();
    let translation = translate(&open(&body));
    let found = paragraphs(&translation);
    let first = found[0].props();
    assert!(first.keep_lines() && first.page_break_before() && !first.widow_control());
    assert!(!first.word_wrap() && first.suppress_auto_hyphens());
    assert!(found[1].props().word_wrap() && !found[1].props().suppress_auto_hyphens());
    assert!(first.spacing().before_auto() && !first.spacing().after_auto());
    assert_eq!(
        first.tabs(),
        [ir::TabStop::default()
            .with_position(Pt(100.0))
            .with_kind(ir::TabKind::Center)
            .with_leader(ir::TabLeader::Hyphen)]
    );
    assert_eq!(
        first.borders().between(),
        Some(
            ir::Border::default()
                .with_width(Pt(0.5))
                .with_space(Pt(1.0))
                .with_color(ir::ColorValue::Rgb(officewright_core::Color::rgb(0, 0, 0)))
                .with_shadow(true)
        )
    );
    assert_eq!(first.alignment(), ir::Alignment::Distribute);
    assert_eq!(found[1].props().alignment(), ir::Alignment::Both);
    let left_out: Vec<String> = translation
        .left_out()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        left_out,
        ["sections[0].blocks[1]: the alignment lowKashida is laid out as justified",]
    );
    // A hanging indent of one character: the mark's size, 11 pt here.
    assert_eq!(found[2].props().indent().first_line(), Pt(-11.0));
    // Without the settings part, Word's default tab stop.
    assert_eq!(
        translation.document().settings().default_tab_stop(),
        Pt(36.0)
    );
}

#[test]
fn translates_every_run_style_property() {
    let body = r#"<w:p><w:r><w:rPr><w:rFonts w:hint="eastAsia"/><w:bCs/><w:iCs/><w:szCs w:val="30"/><w:dstrike/><w:rtl/><w:cs/><w:highlight w:val="darkCyan"/></w:rPr><w:t>a</w:t></w:r></w:p>"#;
    let translation = translate(&open(body));
    let id = style_of(&paragraphs(&translation)[0].inlines()[0]);
    let style = translation.document().run_style(id).unwrap();
    assert_eq!(style.fonts().hint(), ir::FontHint::EastAsia);
    assert!(style.bold_complex_script() && style.italic_complex_script());
    assert!(!style.bold() && !style.italic());
    assert_eq!(style.size_complex_script(), Pt(15.0));
    assert!(style.double_strike() && !style.strike());
    assert!(style.rtl() && style.complex_script());
    assert_eq!(
        style.highlight(),
        Some(officewright_core::Color::rgb(0x00, 0x80, 0x80))
    );
}

#[test]
fn assembles_fields_at_the_edges() {
    let begin = |style: &str| {
        format!(r#"<w:r><w:rPr>{style}</w:rPr><w:fldChar w:fldCharType="begin"/></w:r>"#)
    };
    let instruction = |text: &str| {
        format!(r#"<w:r><w:instrText xml:space="preserve">{text}</w:instrText></w:r>"#)
    };
    let separate = r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#;
    let end = r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
    let body = [
        // The value of a field takes the style of its result's first piece, else that of
        // its beginning.
        format!(
            r#"<w:p>{}{}{separate}<w:r><w:rPr><w:b/></w:rPr><w:t>7</w:t></w:r><w:r><w:rPr><w:u w:val="single"/></w:rPr><w:t>8</w:t></w:r>{end}{}{}{end}</w:p>"#,
            begin("<w:i/>"),
            instruction("PAGE \\* roman \\* arabic"),
            begin("<w:i/>"),
            instruction("NUMPAGES \\* Ordinal"),
        ),
        // Two fields open at the paragraph's end: closed there, the inner result part of
        // the outer one's.
        format!(
            r#"<w:p>{}{}{separate}{}{}{}{separate}{}</w:p>"#,
            begin(""),
            instruction("PAGE"),
            text_run("1"),
            begin(""),
            instruction("NUMPAGES"),
            text_run("2"),
        ),
        // A page number inside a field is left out; a carriage return in text is a
        // space; breaks clear to the left and to the right.
        format!(
            r#"<w:p><w:fldSimple w:instr="TIME"><w:r><w:pgNum/></w:r><w:r><w:t>12:00</w:t></w:r></w:fldSimple>{}{}{separate}<w:r><w:pgNum/></w:r>{end}<w:r><w:t>a&#13;b</w:t><w:br w:clear="left"/><w:br w:clear="right"/><w:sym w:font="Segoe" w:char="1F600"/></w:r></w:p>"#,
            begin(""),
            instruction("SECTIONPAGES"),
        ),
    ]
    .concat();
    let translation = translate(&open(&body));
    let ir = translation.document();
    let found = paragraphs(&translation);
    let ir::Inline::Field(page) = &found[0].inlines()[0] else {
        panic!("a field");
    };
    // The first piece's style, not the second's.
    let bold = ir.run_style(page.style()).unwrap();
    assert!(bold.bold() && !bold.italic() && bold.underline().is_none());
    // Of two number formats, the first counts.
    assert_eq!(page.format(), &Some(ir::NumberFormat::LowerRoman));
    let ir::Inline::Field(pages) = &found[0].inlines()[1] else {
        panic!("a field");
    };
    assert!(ir.run_style(pages.style()).unwrap().italic());
    assert_eq!(pages.format(), &Some(ir::NumberFormat::Ordinal));
    assert_eq!(
        contents(&translation)[1],
        ["field Page None [text 1, text 2]"]
    );
    assert_eq!(
        contents(&translation)[2],
        [
            "field Other(\"TIME\") None [text 12:00]",
            "field SectionPages None []",
            "text a b",
            "break Line(Left)",
            "break Line(Right)",
        ]
    );
    let left_out: Vec<&str> = translation
        .left_out()
        .iter()
        .map(ReportEntry::message)
        .collect();
    assert_eq!(
        left_out,
        [
            "a page number (w:pgNum) inside a field is left out",
            "a page number (w:pgNum) inside a field is left out",
            "a symbol beyond U+FFFF (1F600) is left out",
        ]
    );
}

#[test]
fn bounds_what_long_names_and_tags_copy() {
    // A font name and a language tag far longer than any are copied cut, or not at all,
    // into each symbol and run style: memory stays in proportion to the document.
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{}"/><w:lang w:val="{}" w:eastAsia="{}" w:bidi="{}"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#,
        "A".repeat(1000),
        "l".repeat(100),
        "e".repeat(65),
        "b".repeat(65)
    );
    let body = r#"<w:p><w:r><w:sym w:char="F020"/><w:sym w:char="F021"/></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>b</w:t></w:r></w:p>"#;
    let document = with_parts(body, &[("styles", "styles.xml", styles)]);
    let translation = translate(&document);
    let ir = translation.document();
    for inline in paragraphs(&translation)[0].inlines() {
        let style = match inline {
            ir::Inline::Symbol { font, style, .. } => {
                assert_eq!(font.len(), 256);
                *style
            }
            inline => style_of(inline),
        };
        let style = ir.run_style(style).unwrap();
        assert_eq!(style.fonts().ascii().len(), 256);
        let language = style.language();
        assert_eq!(
            (language.latin(), language.east_asia(), language.bidi()),
            (None, None, None)
        );
    }
    let left_out: Vec<&str> = translation
        .left_out()
        .iter()
        .map(ReportEntry::message)
        .collect();
    assert!(left_out.contains(&"a language tag of more than 64 bytes is left out"));
    assert!(left_out.contains(&"a font name of more than 256 bytes is cut"));
    // A tag of 64 bytes is passed on.
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val="{}"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#,
        "l".repeat(64)
    );
    let document = with_parts(
        "<w:p><w:r><w:t>c</w:t></w:r></w:p>",
        &[("styles", "styles.xml", styles)],
    );
    let translation = translate(&document);
    let style = translation
        .document()
        .run_style(style_of(&paragraphs(&translation)[0].inlines()[0]))
        .unwrap();
    assert_eq!(style.language().latin().map(str::len), Some(64));
}

#[test]
fn translates_the_rest_of_settings_and_sections() {
    let settings = format!(
        r#"<w:settings xmlns:w="{W}"><w:compat><w:usePrinterMetrics/><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="13"/><w:compatSetting w:name="usePrinterMetrics" w:uri="http://schemas.microsoft.com/office/word" w:val="true"/><w:compatSetting w:name="overrideTableStyleFontSizeAndJustification" w:uri="http://schemas.microsoft.com/office/word" w:val="on"/><w:compatSetting w:name="doNotFlipMirrorIndents" w:uri="http://schemas.microsoft.com/office/word" w:val="false"/><w:compatSetting w:name="enableOpenTypeFeatures" w:uri="http://schemas.microsoft.com/office/word" w:val="off"/><w:noLeading/><w:noLeading w:val="0"/><w:doNotExpandShiftReturn/><w:doNotExpandShiftReturn/></w:compat></w:settings>"#
    );
    let body = r#"<w:p/><w:sectPr><w:type w:val="evenPage"/><w:pgNumType w:chapStyle="2" w:chapSep="colon"/><w:cols w:space="1080"/></w:sectPr>"#;
    let document = with_parts(body, &[("settings", "settings.xml", settings)]);
    let translation = translate(&document);
    let ir = translation.document();
    assert_eq!(ir.settings().compat_mode(), ir::CompatMode::Word2007);
    // A flag given twice is passed once; "true" and "on" are on, "false" and "off" off;
    // of a legacy option written twice, the last value counts (noLeading is switched off
    // again).
    let flags: Vec<&str> = ir
        .settings()
        .compat_flags()
        .iter()
        .map(ir::CompatFlag::name)
        .collect();
    assert_eq!(
        flags,
        [
            "usePrinterMetrics",
            "doNotExpandShiftReturn",
            "overrideTableStyleFontSizeAndJustification"
        ]
    );
    let section = &ir.sections()[0];
    assert_eq!(section.start(), ir::SectionStart::EvenPage);
    assert_eq!(section.page_numbering().chapter_style(), Some(2));
    assert_eq!(section.columns().spacing(), Pt(54.0));
}

#[test]
fn translates_list_overrides_links_and_restarts() {
    let level = |ilvl: u8, extra: &str| {
        format!(
            r#"<w:lvl w:ilvl="{ilvl}"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%{}."/>{extra}</w:lvl>"#,
            ilvl + 1
        )
    };
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="1"><w:numStyleLink w:val="Outline"/></w:abstractNum><w:abstractNum w:abstractNumId="2"><w:styleLink w:val="Outline"/>{}{}</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="1"/><w:lvlOverride w:ilvl="8"><w:startOverride w:val="4"/></w:lvlOverride><w:lvlOverride w:ilvl="9"><w:startOverride w:val="4"/></w:lvlOverride><w:lvlOverride w:ilvl="1"><w:lvl w:ilvl="1"><w:start w:val="6"/><w:lvlRestart w:val="0"/><w:numFmt w:val="upperRoman"/><w:lvlText w:val="%2"/></w:lvl></w:lvlOverride></w:num><w:num w:numId="2"><w:abstractNumId w:val="2"/></w:num></w:numbering>"#,
        level(0, r#"<w:lvlJc w:val="center"/><w:lvlRestart w:val="8"/>"#),
        level(1, ""),
    );
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:style w:type="numbering" w:styleId="Outline"><w:name w:val="Outline"/><w:pPr><w:numPr><w:numId w:val="2"/></w:numPr></w:pPr></w:style></w:styles>"#
    );
    let item =
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr></w:p>"#;
    let document = with_parts(
        item,
        &[
            ("numbering", "numbering.xml", numbering),
            ("styles", "styles.xml", styles),
        ],
    );
    let translation = translate(&document);
    let ir = translation.document();
    let list = paragraphs(&translation)[0].list().unwrap();
    let instance = ir.list_instance(list.instance()).unwrap();
    // The instance's definition links to a numbering style: the levels are those of the
    // definition the style's instance uses.
    let definition = ir.list(instance.definition()).unwrap();
    let first = &definition.levels()[0];
    assert_eq!(first.alignment(), ir::LevelAlignment::Center);
    // Word takes 0 to 7 for w:lvlRestart: 8 is the default.
    assert_eq!(first.restart(), ir::Restart::Default);
    assert_eq!(definition.levels().len(), 2);
    // Overrides of levels 0 to 8 only; a replacing level keeps the IR's start and
    // restart, which Word ignores.
    let overrides: Vec<u8> = instance
        .overrides()
        .iter()
        .map(ir::LevelOverride::level)
        .collect();
    assert_eq!(overrides, [8, 1]);
    let replacement = instance.overrides()[1].replacement().as_ref().unwrap();
    assert_eq!(replacement.format(), &ir::NumberFormat::UpperRoman);
    assert_eq!(
        (replacement.start(), replacement.restart()),
        (0, ir::Restart::Default)
    );
}

/// The work the translation does on this thread while `f` runs, of the kind `kind`
/// picks.
fn work_of<T>(kind: fn(crate::layout::Work) -> usize, f: impl FnOnce() -> T) -> (T, usize) {
    let before = kind(crate::layout::WORK.get());
    let result = f();
    (result, kind(crate::layout::WORK.get()) - before)
}

#[test]
fn adds_each_inline_of_nested_fields_once() {
    // 20,000 fields, each in the result of the one before: every inline is added once,
    // not moved into each field around it (which took quadratic time).
    const DEPTH: usize = 20_000;
    let begin = format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}"#,
        text_run("x")
    );
    let body = format!(
        "<w:p>{}{}</w:p>",
        begin.repeat(DEPTH),
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#.repeat(DEPTH)
    );
    let document = open(&body);
    let (translation, work) = work_of(|work| work.inlines, || document.to_layout_ir());
    assert!(work <= 2 * DEPTH, "{work} inlines added for {DEPTH} fields");
    let inlines = paragraphs(&translation)[0].inlines();
    let [ir::Inline::Field(field)] = inlines else {
        panic!("one field");
    };
    assert_eq!(field.result().len(), DEPTH);
}

#[test]
fn shares_run_styles_of_long_names_comparing_bounded_parts() {
    // 400 runs, each with its own copy of four font names of 2,000 characters: they share
    // one run style, and comparing them looks at the cut names only (comparing the whole
    // names would take 16,000 bytes a run).
    const RUNS: usize = 400;
    let name = "N".repeat(2_000);
    let run = format!(
        r#"<w:r><w:rPr><w:rFonts w:ascii="{name}" w:hAnsi="{name}" w:eastAsia="{name}" w:cs="{name}"/></w:rPr><w:t>x</w:t></w:r>"#
    );
    let document = open(&format!("<w:p>{}</w:p>", run.repeat(RUNS)));
    let (translation, work) = work_of(|work| work.compared, || document.to_layout_ir());
    let styles: std::collections::HashSet<ir::RunStyleId> = paragraphs(&translation)[0]
        .inlines()
        .iter()
        .map(style_of)
        .collect();
    assert_eq!(styles.len(), 1);
    // At most seven names and tags of 256 bytes per key, of each run and the mark.
    assert!(work <= (RUNS + 1) * 7 * 256, "{work} bytes looked up");
}

#[test]
fn passes_compatibility_flags_once_with_one_lookup_each() {
    // 30,000 settings of distinct names and one written 30,000 times: each is looked up
    // once (a scan of the flags so far took quadratic time).
    const SETTINGS: usize = 30_000;
    let mut compat = String::new();
    for n in 0..SETTINGS {
        compat.push_str(&format!(
            r#"<w:compatSetting w:name="s{n}" w:uri="http://schemas.microsoft.com/office/word" w:val="1"/><w:compatSetting w:name="again" w:uri="http://schemas.microsoft.com/office/word" w:val="1"/>"#
        ));
    }
    let settings =
        format!(r#"<w:settings xmlns:w="{W}"><w:compat>{compat}</w:compat></w:settings>"#);
    let document = with_parts("<w:p/>", &[("settings", "settings.xml", settings)]);
    let (translation, work) = work_of(|work| work.flags, || document.to_layout_ir());
    let flags = translation.document().settings().compat_flags();
    assert_eq!(flags.len(), SETTINGS + 1);
    assert!(work <= 2 * SETTINGS + 64, "{work} lookups");
}

#[test]
fn looks_fonts_up_with_bounded_folding() {
    // A font table of 200 names of 500 characters and 2,000 run styles whose East Asian
    // font has a name of 100,000 characters: the table is folded to lower case once, and
    // each lookup folds the cut name only.
    let fonts: String = (0..200)
        .map(|n| format!(r#"<w:font w:name="F{n:03}{}"/>"#, "x".repeat(496)))
        .collect();
    let fonts = format!(r#"<w:fonts xmlns:w="{W}">{fonts}</w:fonts>"#);
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:eastAsia="{}"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#,
        "E".repeat(100_000)
    );
    let runs: String = (0..2000)
        .map(|n| {
            format!(
                r#"<w:r><w:rPr><w:sz w:val="{}"/><w:spacing w:val="{}"/></w:rPr><w:t>x</w:t></w:r>"#,
                2 + n % 1000,
                n / 1000
            )
        })
        .collect();
    let before = crate::model::FOLDED.get();
    let document = with_parts(
        &format!("<w:p>{runs}</w:p>"),
        &[
            ("styles", "styles.xml", styles),
            ("fontTable", "fontTable.xml", fonts),
        ],
    );
    let translation = translate(&document);
    let folded = crate::model::FOLDED.get() - before;
    assert_eq!(translation.document().sections()[0].blocks().len(), 1);
    assert!(folded <= 200 * 500 + 2_001 * 256, "{folded} bytes folded");
}

#[test]
fn takes_a_fields_style_only_from_what_its_result_shows() {
    // PAGE's result holds an IF field whose instruction holds a field with a bold result:
    // that is not shown, so PAGE takes its style from the IF field's result, "y", plain.
    let field_char = |kind: &str| format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#);
    let instruction = |text: &str| {
        format!(r#"<w:r><w:instrText xml:space="preserve">{text}</w:instrText></w:r>"#)
    };
    let body = [
        "<w:p>".to_owned(),
        field_char("begin"),
        instruction("PAGE"),
        field_char("separate"),
        field_char("begin"),
        instruction("IF "),
        field_char("begin"),
        instruction("NUMPAGES"),
        field_char("separate"),
        r#"<w:r><w:rPr><w:b/></w:rPr><w:t>x</w:t></w:r>"#.to_owned(),
        field_char("end"),
        field_char("separate"),
        text_run("y"),
        field_char("end"),
        field_char("end"),
        "</w:p>".to_owned(),
    ]
    .concat();
    let translation = translate(&open(&body));
    let [ir::Inline::Field(page)] = paragraphs(&translation)[0].inlines() else {
        panic!("one field");
    };
    assert_eq!(page.kind(), &ir::FieldKind::Page);
    let style = translation.document().run_style(page.style()).unwrap();
    assert!(!style.bold());
    assert_eq!(
        page.result().iter().map(describe).collect::<Vec<_>>(),
        ["text y"]
    );
}

#[test]
fn builds_each_run_style_once() {
    // 2,000 empty paragraphs and a run of one format, under defaults whose names are long
    // and not ASCII: the run style is built once, and every other mark and run finds it
    // by its key, without building it again (which took 18 µs a paragraph).
    let euro = "€".repeat(1000);
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{euro}" w:hAnsi="{euro}" w:eastAsia="{euro}" w:cs="{euro}"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#
    );
    let body = format!(
        "{}<w:p><w:r><w:t>x</w:t></w:r></w:p>",
        "<w:p/>".repeat(2_000)
    );
    let document = with_parts(&body, &[("styles", "styles.xml", styles)]);
    let (translation, built) = work_of(|work| work.built, || document.to_layout_ir());
    assert_eq!(built, 1);
    let marks: std::collections::HashSet<ir::RunStyleId> = paragraphs(&translation)
        .iter()
        .map(|paragraph| paragraph.mark())
        .collect();
    assert_eq!(marks.len(), 1);
}

#[test]
fn takes_a_fields_style_from_an_inner_fields_result() {
    let field_char = |kind: &str| format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#);
    let instruction = |text: &str| {
        format!(r#"<w:r><w:instrText xml:space="preserve">{text}</w:instrText></w:r>"#)
    };
    let body = [
        "<w:p>".to_owned(),
        // PAGE's result starts with NUMPAGES's, bold.
        field_char("begin"),
        instruction("PAGE"),
        field_char("separate"),
        field_char("begin"),
        instruction("NUMPAGES"),
        field_char("separate"),
        r#"<w:r><w:rPr><w:b/></w:rPr><w:t>x</w:t></w:r>"#.to_owned(),
        field_char("end"),
        field_char("end"),
        "</w:p><w:p>".to_owned(),
        // SECTIONPAGES's result holds a field that ends without a separator, then an
        // italic piece, which gives SECTIONPAGES its style.
        field_char("begin"),
        instruction("SECTIONPAGES"),
        field_char("separate"),
        field_char("begin"),
        instruction("TIME"),
        field_char("end"),
        r#"<w:r><w:rPr><w:i/></w:rPr><w:t>y</w:t></w:r>"#.to_owned(),
        field_char("end"),
        "</w:p>".to_owned(),
    ]
    .concat();
    let translation = translate(&open(&body));
    let ir = translation.document();
    let found = paragraphs(&translation);
    let [ir::Inline::Field(page)] = found[0].inlines() else {
        panic!("one field");
    };
    assert!(ir.run_style(page.style()).unwrap().bold());
    let [ir::Inline::Field(pages)] = found[1].inlines() else {
        panic!("one field");
    };
    assert!(ir.run_style(pages.style()).unwrap().italic());
}

#[test]
fn lists_each_cut_name_and_tag() {
    // One document per attribute whose value is longer than the IR keeps.
    let long_name = "N".repeat(300);
    let long_tag = "t".repeat(80);
    let cases = [
        (
            format!(r#"<w:rFonts w:ascii="{long_name}"/>"#),
            "a font name of more than 256 bytes is cut",
        ),
        (
            format!(r#"<w:rFonts w:hAnsi="{long_name}"/>"#),
            "a font name of more than 256 bytes is cut",
        ),
        (
            format!(r#"<w:rFonts w:eastAsia="{long_name}"/>"#),
            "a font name of more than 256 bytes is cut",
        ),
        (
            format!(r#"<w:rFonts w:cs="{long_name}"/>"#),
            "a font name of more than 256 bytes is cut",
        ),
        (
            format!(r#"<w:lang w:val="{long_tag}"/>"#),
            "a language tag of more than 64 bytes is left out",
        ),
        (
            format!(r#"<w:lang w:eastAsia="{long_tag}"/>"#),
            "a language tag of more than 64 bytes is left out",
        ),
        (
            format!(r#"<w:lang w:bidi="{long_tag}"/>"#),
            "a language tag of more than 64 bytes is left out",
        ),
    ];
    for (properties, message) in cases {
        let body = format!(r#"<w:p><w:r><w:rPr>{properties}</w:rPr><w:t>x</w:t></w:r></w:p>"#);
        let translation = translate(&open(&body));
        let left_out: Vec<&str> = translation
            .left_out()
            .iter()
            .map(ReportEntry::message)
            .collect();
        assert_eq!(left_out, [message], "{properties}");
    }
    // A symbol's own font.
    let body = format!(r#"<w:p><w:r><w:sym w:font="{long_name}" w:char="F020"/></w:r></w:p>"#);
    let translation = translate(&open(&body));
    let left_out: Vec<&str> = translation
        .left_out()
        .iter()
        .map(ReportEntry::message)
        .collect();
    assert_eq!(left_out, ["a font name of more than 256 bytes is cut"]);
    let ir::Inline::Symbol { font, .. } = &paragraphs(&translation)[0].inlines()[0] else {
        panic!("a symbol");
    };
    assert_eq!(font.len(), 256);
}

#[test]
fn gives_runs_that_differ_in_one_property_different_run_styles() {
    // For each property of a run style, two runs that differ in it alone get different
    // run styles: the key that shares styles leaves none of them out.
    let pairs = [
        ("", r#"<w:color w:val="FF0000"/>"#),
        (
            r#"<w:u w:val="single"/>"#,
            r#"<w:u w:val="single" w:color="00FF00"/>"#,
        ),
        ("", r#"<w:u w:val="double"/>"#),
        ("", r#"<w:rFonts w:hint="eastAsia"/>"#),
        ("", r#"<w:rFonts w:hint="cs"/>"#),
        ("", r#"<w:rFonts w:ascii="A"/>"#),
        ("", r#"<w:rFonts w:hAnsi="A"/>"#),
        ("", r#"<w:rFonts w:eastAsia="A"/>"#),
        ("", r#"<w:rFonts w:cs="A"/>"#),
        ("", r#"<w:lang w:val="de-DE"/>"#),
        ("", r#"<w:lang w:eastAsia="ja-JP"/>"#),
        ("", r#"<w:lang w:bidi="ar-SA"/>"#),
        ("", r#"<w:kern w:val="24"/>"#),
        ("", r#"<w:position w:val="4"/>"#),
        ("", r#"<w:spacing w:val="20"/>"#),
        ("", r#"<w:w w:val="80"/>"#),
        ("", r#"<w:vertAlign w:val="subscript"/>"#),
        ("", r#"<w:highlight w:val="yellow"/>"#),
        ("", r#"<w:sz w:val="30"/>"#),
        ("", r#"<w:szCs w:val="30"/>"#),
        ("", r#"<w:shd w:val="clear" w:fill="00FF00"/>"#),
        ("", "<w:b/>"),
        ("", "<w:i/>"),
        ("", "<w:bCs/>"),
        ("", "<w:iCs/>"),
        ("", "<w:cs/>"),
        ("", "<w:rtl/>"),
        ("", "<w:strike/>"),
        ("", "<w:dstrike/>"),
        ("", "<w:caps/>"),
        ("", "<w:smallCaps/>"),
        ("", "<w:vanish/>"),
        ("", r#"<w14:ligatures w14:val="standard"/>"#),
        ("", r#"<w14:ligatures w14:val="historical"/>"#),
        ("", "<w14:cntxtAlts/>"),
        // Values against values, not against none.
        (
            r#"<w14:ligatures w14:val="standard"/>"#,
            r#"<w14:ligatures w14:val="historical"/>"#,
        ),
        (
            r#"<w14:ligatures w14:val="historical"/>"#,
            r#"<w14:ligatures w14:val="discretional"/>"#,
        ),
        (
            r#"<w:color w:val="FF0000"/>"#,
            r#"<w:color w:val="00FF00"/>"#,
        ),
        (
            r#"<w:u w:val="single" w:color="FF0000"/>"#,
            r#"<w:u w:val="single" w:color="00FF00"/>"#,
        ),
        (r#"<w:u w:val="single"/>"#, r#"<w:u w:val="dotted"/>"#),
        (
            r#"<w:rFonts w:hint="eastAsia"/>"#,
            r#"<w:rFonts w:hint="cs"/>"#,
        ),
        (r#"<w:rFonts w:ascii="A"/>"#, r#"<w:rFonts w:ascii="B"/>"#),
        (
            r#"<w:rFonts w:eastAsia="A"/>"#,
            r#"<w:rFonts w:eastAsia="B"/>"#,
        ),
        (r#"<w:lang w:val="de-DE"/>"#, r#"<w:lang w:val="fr-FR"/>"#),
        (
            r#"<w:lang w:eastAsia="ja-JP"/>"#,
            r#"<w:lang w:eastAsia="ko-KR"/>"#,
        ),
        (r#"<w:lang w:bidi="ar-SA"/>"#, r#"<w:lang w:bidi="he-IL"/>"#),
        (r#"<w:kern w:val="24"/>"#, r#"<w:kern w:val="28"/>"#),
        (r#"<w:position w:val="4"/>"#, r#"<w:position w:val="6"/>"#),
        (r#"<w:position w:val="4"/>"#, r#"<w:spacing w:val="4"/>"#),
        (r#"<w:spacing w:val="20"/>"#, r#"<w:spacing w:val="40"/>"#),
        (r#"<w:w w:val="80"/>"#, r#"<w:w w:val="90"/>"#),
        (
            r#"<w:vertAlign w:val="subscript"/>"#,
            r#"<w:vertAlign w:val="superscript"/>"#,
        ),
        (
            r#"<w:highlight w:val="yellow"/>"#,
            r#"<w:highlight w:val="green"/>"#,
        ),
        (r#"<w:sz w:val="30"/>"#, r#"<w:sz w:val="32"/>"#),
        (
            r#"<w:sz w:val="30"/>"#,
            r#"<w:sz w:val="30"/><w:szCs w:val="32"/>"#,
        ),
        (
            r#"<w:shd w:val="clear" w:fill="00FF00"/>"#,
            r#"<w:shd w:val="clear" w:fill="0000FF"/>"#,
        ),
        (
            r#"<w:color w:val="FF0000"/>"#,
            r#"<w:u w:val="single" w:color="FF0000"/>"#,
        ),
    ];
    for (a, b) in pairs {
        let body = format!(
            r#"<w:p><w:r><w:rPr>{a}</w:rPr><w:t>a</w:t></w:r><w:r><w:rPr>{b}</w:rPr><w:t>b</w:t></w:r></w:p>"#
        );
        // OpenType features on, so that ligatures and alternates count.
        let translation = translate(&open_type(&body));
        let inlines = paragraphs(&translation)[0].inlines();
        let (first, second) = (style_of(&inlines[0]), style_of(&inlines[1]));
        assert_ne!(first, second, "{b}");
        let ir = translation.document();
        assert_ne!(ir.run_style(first), ir.run_style(second), "{b}");
    }
}

#[test]
fn shares_run_styles_across_properties_the_ir_does_not_keep() {
    // Properties the IR does not keep do not split run styles, which would also split
    // the layout's shaping runs.
    for other in [
        "<w:noProof/>",
        "<w:webHidden/>",
        "<w:outline/>",
        "<w:shadow/>",
        "<w:emboss/>",
        "<w:imprint/>",
        "<w:specVanish/>",
        r#"<w:rStyle w:val="Missing"/>"#,
        // Values the IR takes as none.
        r#"<w:rFonts w:hint="default"/>"#,
        r#"<w:u w:val="none" w:color="FF0000"/>"#,
        r#"<w:highlight w:val="none"/>"#,
        // Without enableOpenTypeFeatures, which these documents leave off.
        r#"<w14:ligatures w14:val="all"/>"#,
        "<w14:cntxtAlts/>",
    ] {
        let body = format!(
            r#"<w:p><w:r><w:t>a</w:t></w:r><w:r><w:rPr>{other}</w:rPr><w:t>b</w:t></w:r></w:p>"#
        );
        let translation = translate(&open(&body));
        let inlines = paragraphs(&translation)[0].inlines();
        assert_eq!(style_of(&inlines[0]), style_of(&inlines[1]), "{other}");
    }
}

#[test]
fn cuts_names_in_a_bounded_number_of_steps() {
    // 2,000 runs under defaults whose names have 100,000 characters: each cut tries a few
    // places from the 256th byte back, not every place of the name.
    let name = "N".repeat(100_000);
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{name}" w:hAnsi="{name}" w:eastAsia="{name}" w:cs="{name}"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#
    );
    let body = format!("<w:p>{}</w:p>", "<w:r><w:t>x</w:t></w:r>".repeat(2_000));
    let document = with_parts(&body, &[("styles", "styles.xml", styles)]);
    let (_, steps) = work_of(|work| work.cut_steps, || document.to_layout_ir());
    // At most four tries per name (a character has at most four bytes), four names per
    // key, a key for each run and the mark, and a cut for each name of the style built.
    assert!(steps <= (2_001 + 1) * 4 * 4, "{steps} steps");
}

#[test]
fn cuts_names_and_tags_only_past_their_bounds() {
    let report = |properties: &str| {
        let body = format!(r#"<w:p><w:r><w:rPr>{properties}</w:rPr><w:t>x</w:t></w:r></w:p>"#);
        let translation = translate(&open(&body));
        translation
            .left_out()
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect::<Vec<_>>()
    };
    let name = |length: usize| format!(r#"<w:rFonts w:ascii="{}"/>"#, "N".repeat(length));
    let tag = |length: usize| format!(r#"<w:lang w:val="{}"/>"#, "t".repeat(length));
    assert!(report(&name(256)).is_empty());
    assert_eq!(
        report(&name(257)),
        ["a font name of more than 256 bytes is cut"]
    );
    // A name of 257 bytes after one of 256 that it starts with: their cut names are
    // the same, but only the second was cut, so it gets a style of its own, and the cut
    // is listed.
    let body = format!(
        r#"<w:p><w:r><w:rPr>{}</w:rPr><w:t>a</w:t></w:r><w:r><w:rPr>{}</w:rPr><w:t>b</w:t></w:r></w:p>"#,
        name(256),
        name(257)
    );
    let translation = translate(&open(&body));
    let left_out: Vec<&str> = translation
        .left_out()
        .iter()
        .map(ReportEntry::message)
        .collect();
    assert_eq!(left_out, ["a font name of more than 256 bytes is cut"]);
    assert!(report(&tag(64)).is_empty());
    assert_eq!(
        report(&tag(65)),
        ["a language tag of more than 64 bytes is left out"]
    );
}

#[test]
fn keeps_names_and_tags_apart_in_any_combination() {
    // Names and tags come back in new combinations: each run style has its own, however
    // the table of strings numbers them.
    let runs = [
        ("A", "de-DE"),
        ("B", "fr-FR"),
        ("A", "fr-FR"),
        ("B", "de-DE"),
        ("A", "de-DE"),
        ("C", "it-IT"),
        ("B", "fr-FR"),
    ];
    let body: String = runs
        .iter()
        .map(|(name, tag)| {
            format!(
                r#"<w:r><w:rPr><w:rFonts w:ascii="{name}"/><w:lang w:val="{tag}"/></w:rPr><w:t>x</w:t></w:r>"#
            )
        })
        .collect();
    let translation = translate(&open(&format!("<w:p>{body}</w:p>")));
    let ir = translation.document();
    let found: Vec<(String, Option<String>)> = paragraphs(&translation)[0]
        .inlines()
        .iter()
        .map(|inline| {
            let style = ir.run_style(style_of(inline)).unwrap();
            (
                style.fonts().ascii().to_owned(),
                style.language().latin().map(str::to_owned),
            )
        })
        .collect();
    let expected: Vec<(String, Option<String>)> = runs
        .iter()
        .map(|(name, tag)| ((*name).to_owned(), Some((*tag).to_owned())))
        .collect();
    assert_eq!(found, expected);
    // Two names first met in one run, then swapped in the next.
    let body = r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="P" w:hAnsi="Q"/></w:rPr><w:t>x</w:t></w:r><w:r><w:rPr><w:rFonts w:ascii="Q" w:hAnsi="P"/></w:rPr><w:t>y</w:t></w:r></w:p>"#;
    let translation = translate(&open(body));
    let ir = translation.document();
    let fonts: Vec<(String, String)> = paragraphs(&translation)[0]
        .inlines()
        .iter()
        .map(|inline| {
            let fonts = ir.run_style(style_of(inline)).unwrap().fonts().clone();
            (fonts.ascii().to_owned(), fonts.high_ansi().to_owned())
        })
        .collect();
    assert_eq!(
        fonts,
        [
            ("P".to_owned(), "Q".to_owned()),
            ("Q".to_owned(), "P".to_owned())
        ]
    );
}

#[test]
fn tells_apart_long_names_and_on_off_properties_that_look_alike() {
    // Names that differ only past their first 32 bytes, and two on/off properties.
    let shared = "X".repeat(40);
    let pairs = [
        (
            format!(r#"<w:rFonts w:ascii="{shared}a"/>"#),
            format!(r#"<w:rFonts w:ascii="{shared}b"/>"#),
        ),
        ("<w:b/>".to_owned(), "<w:caps/>".to_owned()),
        ("<w:i/>".to_owned(), "<w:smallCaps/>".to_owned()),
        ("<w:strike/>".to_owned(), "<w:vanish/>".to_owned()),
    ];
    for (a, b) in pairs {
        let body = format!(
            r#"<w:p><w:r><w:rPr>{a}</w:rPr><w:t>a</w:t></w:r><w:r><w:rPr>{b}</w:rPr><w:t>b</w:t></w:r></w:p>"#
        );
        let translation = translate(&open(&body));
        let inlines = paragraphs(&translation)[0].inlines();
        let ir = translation.document();
        assert_ne!(
            ir.run_style(style_of(&inlines[0])),
            ir.run_style(style_of(&inlines[1])),
            "{a} {b}"
        );
    }
}

#[test]
fn lists_a_cut_once_for_runs_that_share_a_style() {
    let name = "N".repeat(300);
    let run = format!(r#"<w:r><w:rPr><w:rFonts w:ascii="{name}"/></w:rPr><w:t>x</w:t></w:r>"#);
    let translation = translate(&open(&format!("<w:p>{}</w:p>", run.repeat(3))));
    let left_out: Vec<&str> = translation
        .left_out()
        .iter()
        .map(ReportEntry::message)
        .collect();
    assert_eq!(left_out, ["a font name of more than 256 bytes is cut"]);
}

/// The tables of the first section.
fn tables(translation: &Translation) -> Vec<&ir::Table> {
    translation.document().sections()[0]
        .blocks()
        .iter()
        .filter_map(|block| match block {
            ir::Block::Table(table) => Some(table),
            _ => None,
        })
        .collect()
}

#[test]
fn translates_tables_with_their_rows_and_cells() {
    let table = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/><w:tblpPr w:tblpX="100"/><w:bidiVisual/><w:tblW w:w="5000" w:type="pct"/><w:jc w:val="center"/><w:tblCellSpacing w:w="40" w:type="dxa"/><w:tblInd w:w="360" w:type="dxa"/><w:tblBorders><w:top w:val="single" w:sz="8" w:color="FF0000"/><w:bottom w:val="none" w:sz="0"/><w:insideH w:val="nil"/></w:tblBorders><w:shd w:val="clear" w:fill="EEEEEE"/><w:tblLayout w:type="fixed"/><w:tblCellMar><w:left w:w="100" w:type="dxa"/><w:right w:w="10" w:type="pct"/></w:tblCellMar></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="3000"/><w:gridCol w:w="1000"/></w:tblGrid>
        <w:tr><w:trPr><w:cantSplit/><w:trHeight w:val="400"/><w:tblHeader/></w:trPr>
            <w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/><w:gridSpan w:val="2"/><w:vMerge w:val="restart"/><w:tcBorders><w:top w:val="none" w:sz="0"/><w:start w:val="single" w:sz="4"/><w:bottom w:val="nil"/></w:tcBorders><w:shd w:val="clear" w:fill="00FF00"/><w:noWrap/><w:tcMar><w:top w:w="50" w:type="dxa"/><w:bottom w:w="0" w:type="auto"/></w:tcMar><w:textDirection w:val="btLr"/><w:vAlign w:val="center"/><w:hideMark/></w:tcPr><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc>
            <w:tc><w:tcPr><w:tcW w:w="0" w:type="dxa"/></w:tcPr><w:p/></w:tc></w:tr>
        <w:tr><w:trPr><w:gridBefore w:val="1"/><w:gridAfter w:val="2"/><w:wBefore w:w="1000" w:type="dxa"/><w:wAfter w:w="20" w:type="pct"/><w:trHeight w:val="300" w:hRule="exact"/></w:trPr><w:tblPrEx><w:shd w:val="clear" w:fill="FF00FF"/></w:tblPrEx>
            <w:tc><w:tcPr><w:hMerge w:val="restart"/><w:vMerge/></w:tcPr><w:p/></w:tc>
            <w:tc><w:tcPr><w:hMerge/></w:tcPr><w:p/></w:tc></w:tr>
        <w:tr><w:tc><w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:tc><w:tc/></w:tr>
        <w:tr/></w:tbl>"#;
    let document = open(&format!("{table}<w:p/>"));
    let translation = translate(&document);
    let tables = tables(&translation);
    assert_eq!(tables.len(), 1);
    let table = tables[0];
    let rgb =
        |red, green, blue| ir::ColorValue::Rgb(officewright_core::Color::rgb(red, green, blue));
    let props = table.props();
    assert_eq!(props.width(), ir::PreferredWidth::Percent(100.0));
    assert_eq!(props.layout(), ir::TableLayout::Fixed);
    assert_eq!(props.indent(), Pt(18.0));
    assert_eq!(props.alignment(), ir::TableAlignment::Center);
    assert_eq!(props.cell_spacing(), Pt(2.0));
    assert!(props.floating() && props.bidi_visual());
    // A margin in percent is 0 (MS-OI29500 2.1.146 a; from Copilot's review of #159).
    assert_eq!(
        props.cell_margins(),
        ir::CellMargins::default()
            .with_start(Pt(5.0))
            .with_end(Pt(0.0))
    );
    let borders = props.borders();
    assert_eq!(
        borders.top(),
        ir::BorderSide::Line(
            ir::Border::default()
                .with_width(Pt(1.0))
                .with_color(rgb(255, 0, 0))
        )
    );
    // At the table, a border of style none is none; nil is nil; none written inherits.
    assert_eq!(borders.bottom(), ir::BorderSide::None);
    assert_eq!(borders.inside_horizontal(), ir::BorderSide::Nil);
    assert_eq!(borders.start(), ir::BorderSide::Inherit);
    assert_eq!(
        props.shading(),
        Some(ir::Shading::default().with_fill(rgb(0xEE, 0xEE, 0xEE)))
    );
    assert_eq!(table.grid(), [Pt(100.0), Pt(150.0), Pt(50.0)]);
    // The row without cells is left out.
    let rows = table.rows();
    assert_eq!(rows.len(), 3);

    let first = &rows[0];
    assert_eq!(first.height(), ir::RowHeight::AtLeast(Pt(20.0)));
    assert!(first.cant_split() && first.header());
    let cell = &first.cells()[0];
    assert_eq!(cell.width(), ir::PreferredWidth::Points(Pt(100.0)));
    assert_eq!(cell.grid_span(), 2);
    assert_eq!(cell.vertical_merge(), ir::VerticalMerge::Restart);
    // The cell's top margin, the others from the table.
    assert_eq!(
        cell.margins(),
        Some(
            ir::CellMargins::default()
                .with_top(Pt(2.5))
                .with_start(Pt(5.0))
                .with_end(Pt(0.0))
                .with_bottom(Pt(0.0))
        )
    );
    // In a cell, none is looked up in the table (MS-OI29500 2.1.169 a).
    let cell_borders = cell.borders();
    assert_eq!(cell_borders.top(), ir::BorderSide::Inherit);
    assert_eq!(cell_borders.bottom(), ir::BorderSide::Nil);
    assert!(matches!(cell_borders.start(), ir::BorderSide::Line(_)));
    assert_eq!(cell_borders.end(), ir::BorderSide::Inherit);
    assert_eq!(
        cell.shading(),
        Some(ir::Shading::default().with_fill(rgb(0, 255, 0)))
    );
    assert_eq!(cell.vertical_align(), ir::CellVerticalAlign::Center);
    assert!(cell.no_wrap() && cell.hide_mark());
    assert_eq!(
        cell.text_direction(),
        ir::TextDirection::BottomToTopLeftToRight
    );
    // A width of 0 is automatic.
    assert_eq!(first.cells()[1].width(), ir::PreferredWidth::Auto);

    let second = &rows[1];
    assert_eq!(second.height(), ir::RowHeight::Exact(Pt(15.0)));
    assert_eq!(
        (second.grid_before(), second.width_before()),
        (1, ir::PreferredWidth::Points(Pt(50.0)))
    );
    assert_eq!(
        (second.grid_after(), second.width_after()),
        (2, ir::PreferredWidth::Percent(0.4))
    );
    // The cell that continues a horizontal merge widens the one it continues.
    assert_eq!(second.cells().len(), 1);
    assert_eq!(second.cells()[0].grid_span(), 2);
    assert_eq!(
        second.cells()[0].vertical_merge(),
        ir::VerticalMerge::Continue
    );

    // A cell that ends with a table, and an empty cell, end with an empty paragraph.
    let third = &rows[2];
    let blocks = third.cells()[0].blocks();
    assert!(matches!(blocks[0], ir::Block::Table(_)));
    assert!(matches!(blocks[1], ir::Block::Paragraph(_)));
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        third.cells()[1].blocks(),
        [ir::Block::Paragraph(_)]
    ));

    let left_out: Vec<String> = translation
        .left_out()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        left_out,
        [
            "sections[0].blocks[0].rows[1]: the row's table property exceptions are not applied",
            "sections[0].blocks[0].rows[2].cells[0]: a cell that does not end with a paragraph gets an empty one",
            "sections[0].blocks[0].rows[2].cells[1]: a cell that does not end with a paragraph gets an empty one",
            "sections[0].blocks[0].rows[3]: a row without cells is left out",
        ]
    );
}

#[test]
fn leaves_out_tables_without_rows_and_tables_nested_too_deep() {
    let depth = ir::MAX_TABLE_DEPTH + 5;
    let nested = format!(
        "{}<w:p/>{}",
        "<w:tbl><w:tr><w:tc>".repeat(depth),
        "</w:tc></w:tr></w:tbl>".repeat(depth)
    );
    let body = format!("<w:tbl><w:tblGrid/></w:tbl>{nested}<w:p/>");
    let options = LoadOptions::new().with_limits(Limits::new().with_max_xml_depth(1 << 12));
    let document = options.open_bytes(docx(&body)).unwrap();
    let translation = translate(&document);
    let messages: Vec<&str> = translation
        .left_out()
        .iter()
        .map(ReportEntry::message)
        .collect();
    // Each kept table's cell ends with the table in it, so gets an empty paragraph.
    let added = "a cell that does not end with a paragraph gets an empty one";
    let mut expected = vec![
        "a table without rows is left out",
        "a table nested more than 100 deep is left out",
    ];
    expected.extend([added; ir::MAX_TABLE_DEPTH]);
    assert_eq!(messages, expected);
    // The tables down to the limit are kept, each cell ending with a paragraph.
    let mut kept = 0;
    let mut blocks = translation.document().sections()[0].blocks();
    while let Some(ir::Block::Table(table)) = blocks.first() {
        kept += 1;
        blocks = table.rows()[0].cells()[0].blocks();
    }
    assert_eq!(kept, ir::MAX_TABLE_DEPTH);
}

#[test]
fn translates_tables_nested_to_the_limit_on_a_small_stack() {
    // The translation recurses once per nested table. With full debug info, as CI builds
    // tests, this takes about 0.85 MiB: some 66 KiB, and 8 KiB a level for the IR's limit
    // of 100 levels; less in release. Before the frames on that path were made small
    // (#187), it took about 2 MiB. The thread's stack of 1.5 MiB leaves room for other
    // platforms and compilers; this test fails if a level nearly doubles, to about
    // 15 KiB.
    let depth = ir::MAX_TABLE_DEPTH;
    let body = format!(
        "{}<w:p/>{}",
        "<w:tbl><w:tr><w:tc>".repeat(depth),
        "</w:tc></w:tr></w:tbl><w:p/>".repeat(depth)
    );
    let options = LoadOptions::new().with_limits(Limits::new().with_max_xml_depth(1 << 12));
    let document = options.open_bytes(docx(&body)).unwrap();
    let kept = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("100 nested tables on 1.5 MiB of stack".into())
            .stack_size(1_536 * 1024)
            .spawn_scoped(scope, || {
                let translation = document.to_layout_ir();
                let mut kept = 0;
                let mut blocks = translation.document().sections()[0].blocks();
                while let Some(ir::Block::Table(table)) = blocks.first() {
                    kept += 1;
                    blocks = table.rows()[0].cells()[0].blocks();
                }
                kept
            })
            .unwrap()
            .join()
            .unwrap()
    });
    assert_eq!(kept, depth);
}

#[test]
fn translates_the_rest_of_table_properties() {
    let table = r#"<w:tbl><w:tblPr><w:tblW w:w="0" w:type="pct"/><w:jc w:val="right"/><w:tblBorders><w:end w:val="single" w:sz="4"/><w:insideV w:val="dotted" w:sz="4"/></w:tblBorders><w:tblCellMar><w:top w:w="-100" w:type="dxa"/></w:tblCellMar></w:tblPr><w:tblGrid><w:gridCol w:w="-100"/><w:gridCol w:w="1000"/></w:tblGrid>
        <w:tr><w:trPr><w:gridBefore w:val="-1"/><w:trHeight w:val="400" w:hRule="auto"/></w:trPr>
            <w:tc><w:tcPr><w:gridSpan w:val="0"/><w:vAlign w:val="bottom"/><w:textDirection w:val="tbRl"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>"#;
    let translation = translate(&open(&format!("{table}<w:p/>")));
    let table = tables(&translation)[0];
    let props = table.props();
    // A width of 0 percent is automatic (MS-OI29500 §17.4.63(a)).
    assert_eq!(props.width(), ir::PreferredWidth::Auto);
    assert_eq!(props.alignment(), ir::TableAlignment::End);
    let line =
        |style| ir::BorderSide::Line(ir::Border::default().with_style(style).with_width(Pt(0.5)));
    assert_eq!(props.borders().end(), line(ir::BorderStyle::Single));
    assert_eq!(
        props.borders().inside_vertical(),
        line(ir::BorderStyle::Dotted)
    );
    // Negative lengths and counts are 0; a span of 0 is 1.
    assert_eq!(props.cell_margins().top(), Pt(0.0));
    assert_eq!(table.grid(), [Pt(0.0), Pt(50.0)]);
    let row = &table.rows()[0];
    assert_eq!(row.grid_before(), 0);
    // The rule auto ignores the height.
    assert_eq!(row.height(), ir::RowHeight::Auto);
    let cell = &row.cells()[0];
    assert_eq!(cell.grid_span(), 1);
    assert_eq!(cell.vertical_align(), ir::CellVerticalAlign::Bottom);
    assert_eq!(
        cell.text_direction(),
        ir::TextDirection::TopToBottomRightToLeft
    );
}

#[test]
fn translates_widths_beyond_range_and_negative_spacing_into_valid_ir() {
    // Values the IR rejects: the reader reads a width beyond 32 bits of its unit as not
    // written (#115), and a negative cell spacing is 0. `translate` validates the IR.
    let long = "1".repeat(400);
    let table_properties = [
        format!(r#"<w:tblW w:w="{long}" w:type="dxa"/>"#),
        format!(r#"<w:tblW w:w="{long}" w:type="pct"/>"#),
        format!(r#"<w:tblW w:w="{long}%"/>"#),
        format!(r#"<w:tblInd w:w="{long}" w:type="dxa"/>"#),
        format!(r#"<w:tblCellSpacing w:w="{long}" w:type="dxa"/>"#),
        format!(r#"<w:tblCellMar><w:top w:w="{long}" w:type="dxa"/></w:tblCellMar>"#),
        r#"<w:tblCellSpacing w:w="-40" w:type="dxa"/>"#.to_owned(),
    ];
    for written in &table_properties {
        let body = format!(
            "<w:tbl><w:tblPr>{written}</w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"
        );
        let translation = translate(&open(&body));
        assert_eq!(tables(&translation)[0].props().cell_spacing(), Pt(0.0));
    }
    let rows = [
        format!(r#"<w:trPr><w:wBefore w:w="{long}" w:type="dxa"/></w:trPr><w:tc><w:p/></w:tc>"#),
        format!(r#"<w:trPr><w:wAfter w:w="{long}" w:type="dxa"/></w:trPr><w:tc><w:p/></w:tc>"#),
        format!(r#"<w:tc><w:tcPr><w:tcW w:w="{long}" w:type="dxa"/></w:tcPr><w:p/></w:tc>"#),
        format!(r#"<w:tc><w:tcPr><w:tcW w:w="{long}" w:type="pct"/></w:tcPr><w:p/></w:tc>"#),
        format!(
            r#"<w:tc><w:tcPr><w:tcMar><w:start w:w="{long}" w:type="dxa"/></w:tcMar></w:tcPr><w:p/></w:tc>"#
        ),
    ];
    for row in &rows {
        translate(&open(&format!("<w:tbl><w:tr>{row}</w:tr></w:tbl><w:p/>")));
    }
}

#[test]
fn folds_long_horizontal_merges_moving_the_widened_cell() {
    // A cell of many paragraphs, widened by as many continuing cells. The widened cell
    // is moved, not copied once per continuing cell, which took half a minute at 30,000:
    // its blocks stay where they were built.
    let n = 10_000;
    let body = format!(
        r#"<w:tbl><w:tr><w:tc><w:tcPr><w:hMerge w:val="restart"/></w:tcPr>{}</w:tc>{}</w:tr></w:tbl><w:p/>"#,
        "<w:p/>".repeat(n),
        "<w:tc><w:tcPr><w:hMerge/></w:tcPr><w:p/></w:tc>".repeat(n)
    );
    let document = open(&body);
    let (translation, copies) = work_of(|work| work.cell_copies, || translate(&document));
    assert_eq!(copies, 0);
    let cells = tables(&translation)[0].rows()[0].cells();
    assert_eq!(cells.len(), 1);
    assert_eq!(usize::from(cells[0].grid_span()), n + 1);
    assert_eq!(cells[0].blocks().len(), n);
    assert!(translation.left_out().is_empty());
}

#[test]
fn translates_cell_values_word_reads_its_own_way() {
    let body = r#"<w:tbl><w:tblPr><w:tblBorders><w:start w:val="double" w:sz="4"/></w:tblBorders><w:tblCellMar><w:top w:w="100" w:type="dxa"/><w:start w:w="200" w:type="dxa"/><w:end w:type="nil"/></w:tblCellMar></w:tblPr>
        <w:tr>
            <w:tc><w:tcPr><w:tcMar><w:top w:w="0" w:type="nil"/><w:start w:type="nil"/><w:bottom w:w="40" w:type="dxa"/></w:tcMar><w:vAlign w:val="both"/><w:textDirection w:val="lrTbV"/></w:tcPr><w:p/></w:tc>
            <w:tc><w:tcPr><w:textDirection w:val="tbRlV"/></w:tcPr><w:p/></w:tc>
            <w:tc><w:tcPr><w:textDirection w:val="tbLrV"/></w:tcPr><w:p/></w:tc>
            <w:tc><w:tcPr><w:hMerge w:val="restart"/></w:tcPr><w:p/></w:tc>
            <w:tc><w:tcPr><w:gridSpan w:val="2"/><w:hMerge/></w:tcPr><w:p/></w:tc>
        </w:tr></w:tbl><w:p/>"#;
    let translation = translate(&open(body));
    let table = tables(&translation)[0];
    let props = table.props();
    assert_eq!(
        props.borders().start(),
        ir::BorderSide::Line(
            ir::Border::default()
                .with_style(ir::BorderStyle::Double)
                .with_width(Pt(0.5))
        )
    );
    // A table's nil margin is taken as not written, as a cell's is.
    let margins = props.cell_margins();
    assert_eq!(
        (
            margins.top(),
            margins.start(),
            margins.bottom(),
            margins.end()
        ),
        (Pt(5.0), Pt(10.0), Pt(0.0), Pt(5.4))
    );
    let cells = table.rows()[0].cells();
    // Word ignores a cell's nil margin, which then comes from the table (MS-OI29500
    // §17.4.77(a) and the notes on the other sides).
    let margins = cells[0].margins().unwrap();
    assert_eq!(
        (
            margins.top(),
            margins.start(),
            margins.bottom(),
            margins.end()
        ),
        (Pt(5.0), Pt(10.0), Pt(2.0), Pt(5.4))
    );
    // Both is top in a cell (MS-OI29500 §17.4.83(a)); so is none.
    assert_eq!(cells[0].vertical_align(), ir::CellVerticalAlign::Top);
    assert_eq!(cells[1].vertical_align(), ir::CellVerticalAlign::Top);
    let directions: Vec<_> = cells[..3].iter().map(ir::Cell::text_direction).collect();
    assert_eq!(
        directions,
        [
            ir::TextDirection::LeftToRightTopToBottomRotated,
            ir::TextDirection::TopToBottomRightToLeftRotated,
            ir::TextDirection::TopToBottomLeftToRightRotated,
        ]
    );
    // A continuing cell adds its own span to the cell it continues.
    assert_eq!(cells.len(), 4);
    assert_eq!(cells[3].grid_span(), 3);
}

#[test]
fn floats_tables_as_word_does() {
    let floats = |position: &str| {
        let body = format!(
            "<w:tbl><w:tblPr>{position}</w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"
        );
        tables(&translate(&open(&body)))[0].props().floating()
    };
    // Word ignores a w:tblpPr without attributes, and one at 0, 0 from the text
    // horizontally and from anything but the text vertically, with the anchors Word
    // takes when none is written (MS-OI29500 §17.4.57(b), (c)).
    for ignored in [
        "",
        "<w:tblpPr/>",
        r#"<w:tblpPr w:tblpX="0" w:tblpY="0"/>"#,
        r#"<w:tblpPr w:horzAnchor="text" w:vertAnchor="margin" w:tblpX="0" w:tblpY="0"/>"#,
        r#"<w:tblpPr w:horzAnchor="text" w:vertAnchor="page"/>"#,
        r#"<w:tblpPr w:leftFromText="180"/>"#,
    ] {
        assert!(!floats(ignored), "{ignored}");
    }
    for kept in [
        r#"<w:tblpPr w:vertAnchor="text" w:tblpX="0" w:tblpY="0"/>"#,
        r#"<w:tblpPr w:horzAnchor="margin"/>"#,
        // Word writes 1 twip for a distance of 0, so that its table is not ignored.
        r#"<w:tblpPr w:tblpX="1" w:tblpY="0"/>"#,
        r#"<w:tblpPr w:tblpX="0" w:tblpY="1"/>"#,
        r#"<w:tblpPr w:tblpY="-720"/>"#,
        // A position given as an alignment is no distance of 0 (n779642.docx).
        r#"<w:tblpPr w:tblpYSpec="bottom"/>"#,
        r#"<w:tblpPr w:horzAnchor="text" w:vertAnchor="page" w:tblpXSpec="center"/>"#,
    ] {
        assert!(floats(kept), "{kept}");
    }
}

#[test]
fn lists_row_and_cell_properties_the_ir_has_no_place_for() {
    let body = r#"<w:tbl><w:tblPr><w:jc w:val="center"/><w:tblCellSpacing w:w="20" w:type="dxa"/></w:tblPr>
        <w:tr><w:trPr><w:jc w:val="center"/><w:tblCellSpacing w:w="20" w:type="dxa"/><w:hidden w:val="0"/></w:trPr><w:tc><w:p/></w:tc></w:tr>
        <w:tr><w:trPr><w:jc w:val="right"/><w:tblCellSpacing w:w="40" w:type="dxa"/><w:hidden/></w:trPr>
            <w:tc><w:tcPr><w:tcBorders><w:tl2br w:val="single" w:sz="4"/></w:tcBorders><w:tcFitText/></w:tcPr><w:p/></w:tc>
            <w:tc><w:tcPr><w:tcBorders><w:tl2br w:val="none"/><w:tr2bl w:val="nil"/></w:tcBorders><w:tcFitText w:val="0"/></w:tcPr><w:p/></w:tc>
            <w:tc><w:tcPr><w:hMerge w:val="restart"/></w:tcPr><w:p/></w:tc>
            <w:tc><w:tcPr><w:hMerge/></w:tcPr><w:p/></w:tc>
            <w:tc><w:tcPr><w:hMerge/></w:tcPr><w:p><w:r><w:t>second</w:t></w:r></w:p></w:tc>
            <w:tc><w:tcPr><w:tcBorders><w:tr2bl w:val="single" w:sz="4"/></w:tcBorders></w:tcPr><w:p/></w:tc>
        </w:tr></w:tbl><w:p/>"#;
    let translation = translate(&open(body));
    let left_out: Vec<String> = translation
        .left_out()
        .iter()
        .map(ToString::to_string)
        .collect();
    // A row that repeats the table's alignment and spacing, as Word writes every row, is
    // not listed; neither is a continuing cell that is empty.
    assert_eq!(
        left_out,
        [
            "sections[0].blocks[0].rows[1]: the row's own alignment is not applied; the table's is used",
            "sections[0].blocks[0].rows[1]: the row's own cell spacing is not applied; the table's is used",
            "sections[0].blocks[0].rows[1]: the row's hidden end mark (w:hidden) is not applied",
            "sections[0].blocks[0].rows[1].cells[0]: the cell's text fitting (w:tcFitText) is not applied",
            "sections[0].blocks[0].rows[1].cells[0]: the cell's diagonal borders are not drawn",
            "sections[0].blocks[0].rows[1].cells[2]: the content of a cell that continues a horizontal merge is left out",
            "sections[0].blocks[0].rows[1].cells[3]: the cell's diagonal borders are not drawn",
        ]
    );
    // The merged cell keeps its own content only.
    let cells = tables(&translation)[0].rows()[1].cells();
    assert_eq!(cells.len(), 4);
    assert_eq!(cells[2].grid_span(), 3);
    assert!(matches!(cells[2].blocks(), [ir::Block::Paragraph(p)] if p.inlines().is_empty()));
}

#[test]
fn gives_the_paragraph_added_to_a_cell_the_default_paragraph_style_s_mark() {
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:rPr><w:sz w:val="40"/></w:rPr></w:style></w:styles>"#
    );
    let document = with_parts(
        "<w:tbl><w:tr><w:tc><w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl></w:tc></w:tr></w:tbl><w:p/>",
        &[("styles", "styles.xml", styles)],
    );
    let translation = translate(&document);
    let blocks = tables(&translation)[0].rows()[0].cells()[0].blocks();
    let [ir::Block::Table(_), ir::Block::Paragraph(added)] = blocks else {
        panic!("no paragraph added after the nested table");
    };
    let ir = translation.document();
    assert_eq!(ir.run_style(added.mark()).unwrap().size(), Pt(20.0));
}

#[test]
fn translates_ligatures_and_contextual_alternates() {
    let body = r#"<w:p><w:r><w:rPr><w14:ligatures w14:val="standardHistorical"/><w14:cntxtAlts/></w:rPr><w:t>x</w:t></w:r><w:r><w:t>y</w:t></w:r></w:p>"#;
    let translation = translate(&open_type(body));
    let ir = translation.document();
    let inlines = paragraphs(&translation)[0].inlines();
    let set = ir.run_style(style_of(&inlines[0])).unwrap();
    assert_eq!(
        set.ligatures(),
        ir::Ligatures::default()
            .with_standard(true)
            .with_historical(true)
    );
    assert!(set.contextual_alternates());
    // Without a styles part, Word's defaults: standard and contextual ligatures.
    let defaults = ir.run_style(style_of(&inlines[1])).unwrap();
    assert_eq!(
        defaults.ligatures(),
        ir::Ligatures::default()
            .with_standard(true)
            .with_contextual(true)
    );
    assert!(!defaults.contextual_alternates());
    // Without enableOpenTypeFeatures, Word uses neither ([MS-DOCX] 2.3.3), and the two
    // runs share a run style. The mark asks for both too, so that the style built first,
    // the mark's, is built from a format that asks for them.
    let marked = body.replace(
        "<w:p>",
        r#"<w:p><w:pPr><w:rPr><w14:ligatures w14:val="all"/><w14:cntxtAlts/></w:rPr></w:pPr>"#,
    );
    let translation = translate(&open(&marked));
    let paragraph = paragraphs(&translation)[0];
    let inlines = paragraph.inlines();
    let ir = translation.document();
    for id in [paragraph.mark(), style_of(&inlines[0])] {
        let style = ir.run_style(id).unwrap();
        assert_eq!(style.ligatures(), ir::Ligatures::default());
        assert!(!style.contextual_alternates());
    }
    assert_eq!(style_of(&inlines[0]), style_of(&inlines[1]));
    // The setting on as any on/off value writes on.
    for on in ["true", "on", " 1 "] {
        let settings = format!(
            r#"<w:compatSetting w:name="enableOpenTypeFeatures" w:uri="http://schemas.microsoft.com/office/word" w:val="{on}"/>"#
        );
        let translation = translate(&with_compat(body, &settings));
        let inlines = paragraphs(&translation)[0].inlines();
        let style = translation
            .document()
            .run_style(style_of(&inlines[0]))
            .unwrap();
        assert!(style.contextual_alternates(), "{on}");
    }
    // The setting off, or turned off by a later one, is the same.
    for settings in [
        r#"<w:compatSetting w:name="enableOpenTypeFeatures" w:uri="http://schemas.microsoft.com/office/word" w:val="0"/>"#,
        r#"<w:compatSetting w:name="enableOpenTypeFeatures" w:uri="http://schemas.microsoft.com/office/word" w:val="1"/><w:compatSetting w:name="enableOpenTypeFeatures" w:uri="http://schemas.microsoft.com/office/word" w:val="0"/>"#,
    ] {
        let translation = translate(&with_compat(body, settings));
        let inlines = paragraphs(&translation)[0].inlines();
        let style = translation
            .document()
            .run_style(style_of(&inlines[0]))
            .unwrap();
        assert_eq!(style.ligatures(), ir::Ligatures::default(), "{settings}");
    }
}

/// A document whose settings hold `compat_settings` among its compatibility settings.
fn with_compat(body: &str, compat_settings: &str) -> Document {
    let settings =
        format!(r#"<w:settings xmlns:w="{W}"><w:compat>{compat_settings}</w:compat></w:settings>"#);
    with_parts(body, &[("settings", "settings.xml", settings)])
}

/// A document that turns OpenType features on, as Word 2010 and later do.
fn open_type(body: &str) -> Document {
    with_compat(
        body,
        r#"<w:compatSetting w:name="enableOpenTypeFeatures" w:uri="http://schemas.microsoft.com/office/word" w:val="1"/>"#,
    )
}

/// The text of the paragraphs of some IR blocks, one line each.
fn texts(blocks: &[ir::Block]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ir::Block::Paragraph(paragraph) => Some(
                paragraph
                    .inlines()
                    .iter()
                    .filter_map(|inline| match inline {
                        ir::Inline::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn translates_headers_and_footers_in_effect() {
    let story = |root: &str, text: &str| {
        format!(r#"<w:{root} xmlns:w="{W}"><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:{root}>"#)
    };
    let body = r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="default" r:id="rId0"/><w:headerReference w:type="first" r:id="rId1"/><w:footerReference w:type="default" r:id="rId2"/><w:titlePg/></w:sectPr></w:pPr></w:p><w:p/><w:sectPr><w:footerReference w:type="even" r:id="rId3"/></w:sectPr>"#;
    let document = with_parts(
        body,
        &[
            (
                "header",
                "header1.xml",
                story(
                    "hdr",
                    r#"default header</w:t></w:r><w:r><w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/></w:r><w:r><w:t>"#,
                ),
            ),
            ("header", "header2.xml", story("hdr", "first header")),
            (
                "footer",
                "footer1.xml",
                story(
                    "ftr",
                    r#"default footer</w:t></w:r><w:r><w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/></w:r><w:r><w:t>"#,
                ),
            ),
            ("footer", "footer2.xml", story("ftr", "even footer")),
        ],
    );
    let translation = translate(&document);
    // Translated once for both sections, and placed as the IR's report places it.
    let left_out: Vec<String> = translation
        .left_out()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        left_out,
        [
            "sections[0].headers.primary[0].inlines: an absolute position tab is left out (w:ptab)",
            "sections[0].footers.primary[0].inlines: an absolute position tab is left out (w:ptab)",
        ]
    );
    let sections = translation.document().sections();
    let (first, second) = (&sections[0], &sections[1]);
    assert_eq!(texts(first.headers().primary()), ["default header"]);
    assert_eq!(texts(first.headers().first()), ["first header"]);
    assert!(first.headers().even().is_empty());
    assert_eq!(texts(first.footers().primary()), ["default footer"]);
    // The second section inherits what it does not name.
    assert_eq!(texts(second.headers().primary()), ["default header"]);
    assert_eq!(texts(second.headers().first()), ["first header"]);
    assert_eq!(texts(second.footers().primary()), ["default footer"]);
    assert_eq!(texts(second.footers().even()), ["even footer"]);
}

#[test]
fn bounds_the_copies_of_headers_in_sections_by_their_memory() {
    // In tests the copies may take 1 MiB, or as much as the document's XML if that is
    // more. A header of one 200,000-byte text in 31 sections is two blocks and inlines,
    // but each copy takes over 200,000 bytes.
    let copies = |body_text: usize| {
        let header = format!(
            r#"<w:hdr xmlns:w="{W}"><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:hdr>"#,
            "h".repeat(200_000)
        );
        let section = r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="default" r:id="rId0"/></w:sectPr></w:pPr></w:p>"#;
        let body = format!(
            "{}<w:p><w:r><w:t>{}</w:t></w:r></w:p>",
            section.repeat(30),
            "b".repeat(body_text)
        );
        let document = with_parts(&body, &[("header", "header1.xml", header)]);
        let translation = translate(&document);
        let with_header = translation
            .document()
            .sections()
            .iter()
            .filter(|section| !section.headers().primary().is_empty())
            .count();
        let left_out: Vec<String> = translation
            .left_out()
            .iter()
            .map(ToString::to_string)
            .collect();
        (with_header, left_out)
    };
    let (with_header, left_out) = copies(0);
    assert_eq!(with_header, 5);
    assert_eq!(left_out.len(), 26, "{left_out:?}");
    assert_eq!(
        left_out[0],
        "sections[5].headers.primary: a header is left out: the copies of headers and \
         footers in sections have reached their limit"
    );
    // A document of 3,000,000 bytes more XML may copy that much more.
    let (with_header, _) = copies(3_000_000);
    assert!((14..=16).contains(&with_header), "{with_header}");
}

#[test]
fn translates_notes_where_the_text_refers_to_them() {
    let notes = |root: &str, note: &str, extra: &str| {
        format!(
            r#"<w:{root} xmlns:w="{W}"><w:{note} w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r><w:r><w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/></w:r></w:p></w:{note}><w:{note} w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:{note}>{extra}<w:{note} w:id="1"><w:p><w:r><w:{note}Ref/></w:r><w:r><w:t xml:space="preserve"> note one</w:t></w:r><w:r><w:footnoteReference w:id="2"/></w:r></w:p></w:{note}><w:{note} w:id="2"><w:p><w:r><w:t>note two</w:t></w:r></w:p></w:{note}></w:{root}>"#
        )
    };
    let notice = r#"<w:footnote w:type="continuationNotice" w:id="3"><w:p><w:r><w:t>continued</w:t></w:r></w:p></w:footnote>"#;
    // The reference to the separator, which is no note, comes first: the notes are
    // still numbered from 0 in the places of what is left out.
    let body = r#"<w:p><w:r><w:footnoteReference w:id="-1"/></w:r><w:r><w:t>a</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r><w:r><w:footnoteReference w:id="1"/></w:r><w:r><w:endnoteReference w:id="2"/></w:r></w:p>"#;
    let document = with_parts(
        body,
        &[
            (
                "footnotes",
                "footnotes.xml",
                notes("footnotes", "footnote", notice),
            ),
            ("endnotes", "endnotes.xml", notes("endnotes", "endnote", "")),
        ],
    );
    let translation = translate(&document);
    let ir = translation.document();
    let references: Vec<ir::NoteId> = paragraphs(&translation)[0]
        .inlines()
        .iter()
        .filter_map(|inline| match inline {
            ir::Inline::NoteReference { note, .. } => Some(*note),
            _ => None,
        })
        .collect();
    // Two references to footnote 1 name one IR note; the separator is no note to refer
    // to.
    assert_eq!(references.len(), 3);
    assert_eq!(references[0], references[1]);
    let footnote = ir.note(references[0]).unwrap();
    assert_eq!(footnote.kind(), ir::NoteKind::Footnote);
    assert_eq!(texts(footnote.blocks()), [" note one"]);
    let ir::Block::Paragraph(paragraph) = &footnote.blocks()[0] else {
        panic!("a paragraph");
    };
    assert!(matches!(
        paragraph.inlines()[0],
        ir::Inline::NoteNumber { .. }
    ));
    let endnote = ir.note(references[2]).unwrap();
    assert_eq!(endnote.kind(), ir::NoteKind::Endnote);
    assert_eq!(texts(endnote.blocks()), ["note two"]);
    // The separators, from the notes of those types.
    let separators = ir.note_separators(ir::NoteKind::Footnote);
    let ir::Block::Paragraph(line) = &separators.separator()[0] else {
        panic!("a paragraph");
    };
    assert!(matches!(line.inlines()[0], ir::Inline::Separator { .. }));
    let ir::Block::Paragraph(line) = &separators.continuation_separator()[0] else {
        panic!("a paragraph");
    };
    assert!(matches!(
        line.inlines()[0],
        ir::Inline::ContinuationSeparator { .. }
    ));
    assert_eq!(texts(separators.continuation_notice()), ["continued"]);
    // Endnotes have no notice: the IR's default.
    assert!(
        ir.note_separators(ir::NoteKind::Endnote)
            .continuation_notice()
            .is_empty()
    );
    let left_out: Vec<String> = translation
        .left_out()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        left_out,
        [
            "sections[0].blocks[0].inlines: a reference to footnote -1, which is the separator, is left out",
            "notes[0].blocks[0].inlines: a reference to a footnote inside a note is left out",
            "footnote_separators.separator[0].inlines: an absolute position tab is left out (w:ptab)",
            "endnote_separators.separator[0].inlines: an absolute position tab is left out (w:ptab)",
        ]
    );
}

#[test]
fn keeps_tables_in_notes_from_floating() {
    // Word ignores a w:tblpPr in a note (MS-OI29500 §17.4.57(b)).
    let table = r#"<w:tbl><w:tblPr><w:tblpPr w:horzAnchor="margin" w:tblpY="100"/></w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#;
    let footnotes = format!(
        r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1">{table}<w:p/></w:footnote></w:footnotes>"#
    );
    let body = format!(r#"{table}<w:p><w:r><w:footnoteReference w:id="1"/></w:r></w:p>"#);
    let document = with_parts(&body, &[("footnotes", "footnotes.xml", footnotes)]);
    let translation = translate(&document);
    assert!(tables(&translation)[0].props().floating());
    let ir = translation.document();
    let ir::Block::Paragraph(paragraph) = &ir.sections()[0].blocks()[1] else {
        panic!("a paragraph");
    };
    let ir::Inline::NoteReference { note, .. } = &paragraph.inlines()[0] else {
        panic!("a note reference");
    };
    let ir::Block::Table(in_note) = &ir.note(*note).unwrap().blocks()[0] else {
        panic!("a table");
    };
    assert!(!in_note.props().floating());
}

/// How many of 31 sections, the first naming the header `header` and the others
/// inheriting it, get a copy of it; `extra` is more XML, as `(relationship, part)`.
fn header_copies(header: &str, styles: Option<String>, extra: &[(&str, &str, String)]) -> usize {
    let header = format!(r#"<w:hdr xmlns:w="{W}">{header}</w:hdr>"#);
    let section = r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="default" r:id="rId0"/></w:sectPr></w:pPr></w:p>"#;
    let body = format!("{}<w:p/>", section.repeat(30));
    let mut parts = vec![("header", "header1.xml", header)];
    if let Some(styles) = styles {
        parts.push(("styles", "styles.xml", styles));
    }
    parts.extend(extra.iter().cloned());
    let translation = translate(&with_parts(&body, &parts));
    let with_header = translation
        .document()
        .sections()
        .iter()
        .filter(|section| !section.headers().primary().is_empty())
        .count();
    let listed = translation
        .left_out()
        .iter()
        .filter(|entry| entry.message().starts_with("a header is left out"))
        .count();
    assert_eq!(with_header + listed, 31);
    with_header
}

#[test]
fn bounds_header_copies_by_every_allocation_they_make() {
    // In tests the copies may take 1 MiB, or as much as the document's XML. Each header
    // below is small as XML, or small as blocks and inlines, but each copy takes several
    // hundred kilobytes, in a different part of the IR each time.
    let paragraph = |inner: &str| format!("<w:p>{inner}</w:p>");
    // A table of 50,000 grid columns: 400,000 bytes of grid.
    let grid = format!(
        "<w:tbl><w:tblGrid>{}</w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>",
        r#"<w:gridCol w:w="10"/>"#.repeat(50_000)
    );
    assert!(header_copies(&grid, None, &[]) <= 3);
    // 1,000 empty paragraphs whose defaults give 64 tab stops each.
    let tabs: String = (1..=64)
        .map(|n| format!(r#"<w:tab w:val="left" w:pos="{}"/>"#, n * 100))
        .collect();
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:pPrDefault><w:pPr><w:tabs>{tabs}</w:tabs></w:pPr></w:pPrDefault></w:docDefaults></w:styles>"#
    );
    assert!(header_copies(&"<w:p/>".repeat(1_000), Some(styles), &[]) <= 1);
    // 500 fields named by 1,000 bytes, and 500 PAGE fields whose number format is named
    // by 1,000 bytes.
    let name = "N".repeat(1_000);
    let names = paragraph(
        &format!(r#"<w:fldSimple w:instr="{name}"><w:r><w:t>r</w:t></w:r></w:fldSimple>"#)
            .repeat(500),
    );
    assert!(header_copies(&names, None, &[]) <= 2);
    let formats = paragraph(
        &format!(r#"<w:fldSimple w:instr="PAGE \* {name}"><w:r><w:t>1</w:t></w:r></w:fldSimple>"#)
            .repeat(500),
    );
    assert!(header_copies(&formats, None, &[]) <= 2);
    // 2,000 symbols whose font is named by 250 bytes.
    let symbols = paragraph(&format!(
        r#"<w:r>{}</w:r>"#,
        format!(r#"<w:sym w:font="{}" w:char="F020"/>"#, "S".repeat(250)).repeat(2_000)
    ));
    assert!(header_copies(&symbols, None, &[]) <= 2);
    // The text of a table's cell, and of a field's result: 200,000 bytes.
    let text = "t".repeat(200_000);
    let cell = format!(
        "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"
    );
    assert!(header_copies(&cell, None, &[]) <= 6);
    let result = paragraph(&format!(
        r#"<w:fldSimple w:instr="PAGE"><w:r><w:t>{text}</w:t></w:r></w:fldSimple>"#
    ));
    assert!(header_copies(&result, None, &[]) <= 6);
    // The XML of every part counts toward what the copies may take: 3,000,000 bytes of
    // footnotes let the 200,000-byte header be copied about three times as often.
    let footnotes = format!(
        r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1"><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:footnote></w:footnotes>"#,
        "f".repeat(3_000_000)
    );
    let copies = header_copies(
        &paragraph(&format!("<w:r><w:t>{text}</w:t></w:r>")),
        None,
        &[("footnotes", "footnotes.xml", footnotes)],
    );
    assert!((14..=16).contains(&copies), "{copies}");
}

#[test]
fn lists_every_reference_left_out_and_places_notes_in_their_order() {
    let footnotes = format!(
        r#"<w:footnotes xmlns:w="{W}"><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p></w:footnote><w:footnote w:type="separator" w:id="-2"><w:p><w:r><w:t>second separator</w:t></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote><w:footnote w:type="continuationNotice" w:id="3"><w:p><w:r><w:t>continued</w:t></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p><w:r><w:t>one</w:t></w:r></w:p></w:footnote><w:footnote w:id="2"><w:p><w:r><w:footnoteReference w:id="1"/></w:r></w:p></w:footnote></w:footnotes>"#
    );
    let references = r#"<w:p><w:r><w:footnoteReference w:id="1"/></w:r><w:r><w:footnoteReference w:id="2"/></w:r><w:r><w:footnoteReference w:id="5"/></w:r><w:r><w:footnoteReference w:id="-1"/></w:r></w:p>"#;
    let body = format!(
        r#"{}<w:p><w:r><w:footnoteReference w:id="0"/></w:r><w:r><w:footnoteReference w:id="3"/></w:r></w:p>"#,
        references.repeat(2)
    );
    let translation = translate(&with_parts(
        &body,
        &[("footnotes", "footnotes.xml", footnotes)],
    ));
    let left_out: Vec<String> = translation
        .left_out()
        .iter()
        .map(ToString::to_string)
        .collect();
    // Each reference left out is listed, the second paragraph's too. Places in notes
    // count the notes added: the reference inside the second is in notes[1]. The
    // separator holds a reference too, which is left out as one inside a note.
    assert_eq!(
        left_out,
        [
            "notes[1].blocks[0].inlines: a reference to a footnote inside a note is left out",
            "sections[0].blocks[0].inlines: a reference to footnote 5, which the document does not have, is left out",
            "sections[0].blocks[0].inlines: a reference to footnote -1, which is the separator, is left out",
            "sections[0].blocks[1].inlines: a reference to footnote 5, which the document does not have, is left out",
            "sections[0].blocks[1].inlines: a reference to footnote -1, which is the separator, is left out",
            "sections[0].blocks[2].inlines: a reference to footnote 0, which is the continuation separator, is left out",
            "sections[0].blocks[2].inlines: a reference to footnote 3, which is the continuation notice, is left out",
            "footnote_separators.separator[0].inlines: a reference to a footnote inside a note is left out",
        ]
    );
    // The first separator counts.
    let separators = translation
        .document()
        .note_separators(ir::NoteKind::Footnote);
    let ir::Block::Paragraph(line) = &separators.separator()[0] else {
        panic!("a paragraph");
    };
    assert!(matches!(line.inlines()[0], ir::Inline::Separator { .. }));
    assert_eq!(line.inlines().len(), 1);
}

#[test]
fn bounds_a_note_s_tables_by_the_tables_around_its_reference() {
    let nested = |depth: usize, inner: &str| {
        format!(
            "{}{inner}{}",
            "<w:tbl><w:tr><w:tc>".repeat(depth),
            "</w:tc></w:tr></w:tbl><w:p/>".repeat(depth)
        )
    };
    let note_depth = |around: usize, inside: usize| {
        let footnotes = format!(
            r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1">{}</w:footnote></w:footnotes>"#,
            nested(inside, "<w:p/>")
        );
        let body = nested(
            around,
            r#"<w:p><w:r><w:footnoteReference w:id="1"/></w:r></w:p>"#,
        );
        let translation = translate(&with_parts(
            &body,
            &[("footnotes", "footnotes.xml", footnotes)],
        ));
        // The note, through the reference at the bottom of the body's tables.
        let mut blocks = translation.document().sections()[0].blocks();
        while let Some(ir::Block::Table(table)) = blocks.first() {
            blocks = table.rows()[0].cells()[0].blocks();
        }
        let ir::Block::Paragraph(paragraph) = &blocks[0] else {
            panic!("a paragraph");
        };
        let ir::Inline::NoteReference { note, .. } = &paragraph.inlines()[0] else {
            panic!("a note reference");
        };
        let note = translation.document().note(*note).unwrap();
        let mut depth = 0;
        let mut blocks = note.blocks();
        while let Some(ir::Block::Table(table)) = blocks.first() {
            depth += 1;
            blocks = table.rows()[0].cells()[0].blocks();
        }
        let listed: Vec<String> = translation
            .left_out()
            .iter()
            .map(ToString::to_string)
            .collect();
        (depth, listed)
    };
    // A note's tables count their depth on from the tables around its reference, which
    // bounds the translation's recursion by the IR's limit: 75 around and 30 inside keep
    // 25, and the rest is listed.
    let (depth, listed) = note_depth(75, 30);
    assert_eq!(depth, 25);
    // The 26th table, inside 25 others.
    let place = format!(
        "notes[0].blocks[0]{}",
        ".rows[0].cells[0].blocks[0]".repeat(25)
    );
    assert_eq!(
        listed,
        [format!(
            "{place}: a table nested more than 100 deep is left out"
        )]
    );
    // 83 around a note of 83 reaches the cap of 100, as every deeper case the default
    // limits allow does, and translates within a test thread's stack.
    let (depth, _) = note_depth(83, 83);
    assert_eq!(depth, 17);
}

#[test]
fn translates_pictures_with_their_images() {
    use super::drawing::{
        bare_floating_picture, floating_picture, group, inline_picture, picture_of_negative_width,
        with_images,
    };
    let body = format!(
        "<w:p>{}{}{}{}{}{}{}{}{}{}</w:p>",
        inline_picture("rIdPng"),
        floating_picture("rIdPng"),
        inline_picture("rIdJpeg"),
        inline_picture("rIdEmf"),
        inline_picture("rIdGif"),
        inline_picture("rIdGone"),
        inline_picture("rIdNone"),
        group("rIdJpeg"),
        bare_floating_picture("rIdPng"),
        picture_of_negative_width("rIdPng"),
    );
    let translation = translate(&with_images(&body));
    let ir = translation.document();
    let picture = |inline: &ir::Inline| -> ir::Picture {
        let content = match inline {
            ir::Inline::Object(object) => object.content(),
            ir::Inline::Anchored(object) => object.content(),
            other => panic!("not an object: {other:?}"),
        };
        match content {
            ir::ObjectContent::Picture(picture) => *picture,
            other => panic!("not a picture: {other:?}"),
        }
    };
    let image = |inline: &ir::Inline| ir.image(picture(inline).image()).unwrap();
    let inlines = paragraphs(&translation)[0].inlines();
    // The group and the picture without a width are left out.
    assert_eq!(inlines.len(), 8);

    let ir::Inline::Object(object) = &inlines[0] else {
        panic!("a picture in the line");
    };
    assert_eq!(object.size(), ir::Size::new(Pt(144.0), Pt(72.0)));
    assert_eq!(
        object.effect_extent(),
        ir::Insets::default().with_top(Pt(2.0)).with_right(Pt(2.0))
    );
    let shown = picture(&inlines[0]);
    assert_eq!(
        shown.crop(),
        Some(ir::Crop::default().with_left(0.1).with_bottom(-0.05))
    );
    assert_eq!(shown.rotation(), -90.0);
    assert!(shown.flip_horizontal() && !shown.flip_vertical());
    assert_eq!(image(&inlines[0]).format(), &ir::ImageFormat::Png);
    assert_eq!(&**image(&inlines[0]).bytes(), PNG_BYTES);

    let ir::Inline::Anchored(floating) = &inlines[1] else {
        panic!("a floating picture");
    };
    assert_eq!(floating.size(), ir::Size::new(Pt(100.0), Pt(40.0)));
    assert_eq!(
        floating.horizontal(),
        ir::HorizontalAnchor::default()
            .with_relative_to(ir::HorizontalReference::Page)
            .with_placement(ir::HorizontalPlacement::Align(
                ir::AnchorHorizontalAlign::Right
            ))
    );
    assert_eq!(
        floating.vertical(),
        ir::VerticalAnchor::default()
            .with_relative_to(ir::VerticalReference::TopMargin)
            .with_placement(ir::VerticalPlacement::Offset(Pt(-10.0)))
    );
    assert_eq!(floating.wrap(), ir::Wrap::Square);
    assert!(floating.behind_text());
    assert!(!floating.layout_in_cell() && !floating.allow_overlap());
    assert_eq!(floating.z_order(), 251_659_264);
    assert_eq!(
        floating.distances(),
        ir::Insets::default().with_left(Pt(9.0))
    );
    assert_eq!(
        floating.effect_extent(),
        ir::Insets::default().with_left(Pt(72.0))
    );
    assert_eq!(
        picture(&inlines[1]),
        ir::Picture::new(picture(&inlines[0]).image())
    );

    // Formats by their signatures, whatever the package calls them, else by the content
    // type.
    assert_eq!(image(&inlines[2]).format(), &ir::ImageFormat::Jpeg);
    assert_eq!(
        image(&inlines[3]).format(),
        &ir::ImageFormat::Other("emf".into())
    );
    assert_eq!(
        image(&inlines[4]).format(),
        &ir::ImageFormat::Other("gif".into())
    );
    // Pictures whose image the package does not have keep their frames, with one empty
    // image.
    assert_eq!(
        image(&inlines[5]).format(),
        &ir::ImageFormat::Other("missing".into())
    );
    assert!(image(&inlines[5]).bytes().is_empty());
    assert_eq!(picture(&inlines[5]).image(), picture(&inlines[6]).image());
    let ids: std::collections::HashSet<ir::ImageId> = inlines
        .iter()
        .map(|inline| picture(inline).image())
        .collect();
    assert_eq!(ids.len(), 5);

    // Where a floating picture leaves them out: from the column and the paragraph, no
    // wrapping, in the cell and overlapping.
    let ir::Inline::Anchored(bare) = &inlines[7] else {
        panic!("a floating picture");
    };
    assert_eq!(
        (bare.horizontal(), bare.vertical()),
        (
            ir::HorizontalAnchor::default()
                .with_relative_to(ir::HorizontalReference::Column)
                .with_placement(ir::HorizontalPlacement::Offset(Pt(1.0))),
            ir::VerticalAnchor::default()
                .with_relative_to(ir::VerticalReference::Paragraph)
                .with_placement(ir::VerticalPlacement::Offset(Pt(2.0)))
        )
    );
    assert_eq!(bare.wrap(), ir::Wrap::None);
    assert!(bare.layout_in_cell() && bare.allow_overlap() && !bare.behind_text());

    // The header's picture shows the image the body's do.
    let header = ir.sections()[0].headers().primary();
    let ir::Block::Paragraph(paragraph) = &header[0] else {
        panic!("a paragraph");
    };
    assert_eq!(
        picture(&paragraph.inlines()[0]).image(),
        picture(&inlines[0]).image()
    );

    let left_out: Vec<&str> = translation
        .left_out()
        .iter()
        .map(ReportEntry::message)
        .collect();
    assert_eq!(
        left_out,
        [
            "a picture's image is not in the package: its frame is left empty",
            "a picture's image is not in the package: its frame is left empty",
            "a drawing that shows no picture (a shape, chart or group) is left out",
            "a picture without a size is left out",
        ]
    );
}

#[test]
fn translates_pictures_whose_lengths_are_out_of_range_into_valid_ir() {
    // Lengths of 308 digits and a unit, which are infinite as numbers of points, in each
    // place a drawing has one: none reaches the IR, which `translate` validates.
    use super::drawing::{anchored, endless, with_images};
    let e = endless();
    let children = format!(
        r#"<wp:simplePos x="{e}" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>{e}</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:posOffset>{e}</wp:posOffset></wp:positionV><wp:extent cx="12700" cy="12700"/><wp:effectExtent l="{e}" t="{e}" r="{e}" b="{e}"/><wp:wrapNone/>"#
    );
    for simple in ["0", "1"] {
        let attributes =
            format!(r#"simplePos="{simple}" distT="{e}" distB="{e}" distL="{e}" distR="{e}""#);
        let document = with_images(&format!(
            "<w:p>{}</w:p>",
            anchored("rIdPng", &attributes, &children)
        ));
        translate(&document);
    }
    let inline = format!(
        r#"<w:p><w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:extent cx="{e}" cy="12700"/><wp:effectExtent l="{e}" t="0" r="0" b="0"/><wp:docPr id="1" name="p"/>{}</wp:inline></w:drawing></w:r></w:p>"#,
        super::drawing::graphic("rIdPng", "", "")
    );
    translate(&with_images(&inline));
}

#[test]
fn lists_a_cell_s_own_entries_before_those_of_its_content() {
    // A cell's properties are listed before its content, and the paragraph added to a
    // cell that ends with a table last, as a value the translation changed.
    let body = r#"<w:tbl><w:tr><w:tc><w:tcPr><w:tcFitText/></w:tcPr><w:p><w:r><w:ptab w:relativeTo="margin" w:alignment="left" w:leader="none"/></w:r></w:p><w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl></w:tc></w:tr></w:tbl><w:p/>"#;
    let translation = translate(&open(body));
    let entries: Vec<(String, ReportKind)> = translation
        .left_out()
        .iter()
        .map(|entry| (entry.to_string(), entry.kind()))
        .collect();
    let cell = "sections[0].blocks[0].rows[0].cells[0]";
    assert_eq!(
        entries,
        [
            (
                format!("{cell}: the cell's text fitting (w:tcFitText) is not applied"),
                ReportKind::FeatureUnsupported
            ),
            (
                format!("{cell}.blocks[0].inlines: an absolute position tab is left out (w:ptab)"),
                ReportKind::FeatureUnsupported
            ),
            (
                format!("{cell}: a cell that does not end with a paragraph gets an empty one"),
                ReportKind::ValueClamped
            ),
        ]
    );
}

#[test]
fn translates_tables_with_their_style() {
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:style w:type="table" w:styleId="Grid"><w:name w:val="Grid"/><w:rPr><w:b/></w:rPr><w:tblPr><w:jc w:val="center"/><w:tblBorders><w:insideH w:val="single" w:sz="8"/></w:tblBorders></w:tblPr><w:tblStylePr w:type="firstRow"><w:tblPr><w:tblCellSpacing w:w="20" w:type="dxa"/></w:tblPr><w:trPr><w:tblHeader/></w:trPr><w:tcPr><w:shd w:val="clear" w:fill="FF0000"/></w:tcPr></w:tblStylePr></w:style></w:styles>"#
    );
    let table = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/></w:tblPr><w:tblGrid><w:gridCol w:w="1000"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let document = with_parts(table, &[("styles", "styles.xml", styles)]);
    let translation = translate(&document);
    let table = tables(&translation)[0];
    assert_eq!(table.props().alignment(), ir::TableAlignment::Center);
    assert!(matches!(
        table.props().borders().inside_horizontal(),
        ir::BorderSide::Line(_)
    ));
    // The first row's format: the header row and the cell's shading.
    let rows = table.rows();
    assert!(rows[0].header() && !rows[1].header());
    let red = ir::ColorValue::Rgb(officewright_core::Color::rgb(255, 0, 0));
    assert_eq!(
        rows[0].cells()[0].shading(),
        Some(ir::Shading::default().with_fill(red))
    );
    assert_eq!(rows[1].cells()[0].shading(), None);
    let left_out: Vec<&str> = translation
        .left_out()
        .iter()
        .map(ReportEntry::message)
        .collect();
    assert_eq!(
        left_out,
        ["the table properties of the table style's conditional formats are not applied"]
    );
    // What the style gives the text of its cells: bold.
    let ir = translation.document();
    for row in rows {
        let ir::Block::Paragraph(paragraph) = &row.cells()[0].blocks()[0] else {
            panic!("no paragraph");
        };
        assert!(ir.run_style(paragraph.mark()).unwrap().bold());
    }
}

#[test]
fn reads_each_cell_s_properties_once() {
    // Four cells, one of which continues a horizontal merge, in a table with a style: the
    // properties in effect of each are worked out once, for its span and merge and for
    // the cell itself.
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:style w:type="table" w:styleId="Grid"><w:tcPr><w:shd w:val="clear" w:fill="FF0000"/></w:tcPr></w:style></w:styles>"#
    );
    let table = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/></w:tblPr><w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="1000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:hMerge w:val="restart"/></w:tcPr><w:p/></w:tc><w:tc><w:tcPr><w:hMerge/></w:tcPr><w:p/></w:tc></w:tr><w:tr><w:tc><w:p/></w:tc><w:tc><w:tcPr><w:gridSpan w:val="1"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let document = with_parts(table, &[("styles", "styles.xml", styles)]);
    let (translation, reads) = work_of(|work| work.cell_reads, || translate(&document));
    assert_eq!(reads, 4);
    let rows = tables(&translation)[0].rows();
    assert_eq!(rows[0].cells().len(), 1);
    assert_eq!(rows[0].cells()[0].grid_span(), 2);
    assert!(rows[1].cells()[1].shading().is_some());
}
