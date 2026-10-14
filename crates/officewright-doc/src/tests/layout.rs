//! The translation into the layout IR: [`Document::to_layout_ir`].

use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::*;

/// A document whose body is `body`, with parts related to the main part, each given by
/// the last segment of its relationship type, its target under `word/` and its XML.
fn with_parts(body: &str, parts: &[(&str, &str, String)]) -> Document {
    let relationships: String = parts
        .iter()
        .enumerate()
        .map(|(n, (kind, target, _))| {
            format!(
                r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
            )
        })
        .collect();
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{relationships}</Relationships>"#
    );
    let mut extra = vec![("word/_rels/document.xml.rels".to_owned(), rels)];
    for (_, target, xml) in parts {
        extra.push((format!("word/{target}"), xml.clone()));
    }
    let extra: Vec<(&str, &str)> = extra.iter().map(|(a, b)| (&**a, &**b)).collect();
    Document::from_bytes(package(DOCUMENT, &document_xml(body), &extra)).unwrap()
}

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
        .map(LeftOut::message)
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
            "sections[0]: the section's headers and footers are left out: they are not translated yet",
            "sections[0].blocks[0].inlines: a drawing is left out (w:drawing)",
            "sections[0].blocks[0].inlines: a footnote reference is left out: notes are not translated yet",
            "sections[0].blocks[0].inlines: an endnote reference is left out: notes are not translated yet",
            "sections[0].blocks[0].inlines: an absolute position tab is left out (w:ptab)",
            "sections[0].blocks[0].inlines: an equation is left out",
            "sections[0].blocks[1]: a table is left out: tables are not translated yet",
            "sections[0].blocks[1]: a block-level w:altChunk is left out",
            "sections[0].blocks[1]: indents or spacing in character or line units keep their value in points",
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
fn lists_at_most_a_thousand_entries() {
    let references = r#"<w:r><w:footnoteReference w:id="1"/></w:r>"#.repeat(1200);
    let document = open(&format!("<w:p>{references}</w:p>"));
    let translation = translate(&document);
    let left_out = translation.left_out();
    // 999 listed, and one counting the others.
    assert_eq!(left_out.len(), 1000);
    assert_eq!(left_out[999].message(), "201 more entries are not listed");
    assert_eq!(left_out[999].location(), "document");
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
        r#"<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="10"><w:lvl w:ilvl="0"><w:start w:val="3"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="right"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="lowerLetter"/><w:lvlRestart w:val="0"/><w:lvlText w:val="%1.%2)"/><w:suff w:val="space"/><w:isLgl/></w:lvl><w:lvl w:ilvl="3"><w:numFmt w:val="bullet"/><w:lvlText w:val="%1"/><w:lvlRestart w:val="2"/><w:suff w:val="nothing"/><w:rPr><w:b/></w:rPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="10"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride><w:lvlOverride w:ilvl="1"><w:lvl w:ilvl="1"><w:numFmt w:val="lowerRoman"/><w:lvlText w:val="(%2)"/></w:lvl></w:lvlOverride></w:num><w:num w:numId="3"><w:abstractNumId w:val="99"/></w:num></w:numbering>"#
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
    // The label of the bullet level is bold.
    assert!(ir.run_style(bullet.label_style()).unwrap().bold());
    assert!(!ir.run_style(first.label_style()).unwrap().bold());

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
        [
            "sections[0].blocks[1]: the alignment lowKashida is laid out as justified",
            "sections[0].blocks[2]: indents or spacing in character or line units keep their value in points",
        ]
    );
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
        .map(LeftOut::message)
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
        .map(LeftOut::message)
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
            .map(LeftOut::message)
            .collect();
        assert_eq!(left_out, [message], "{properties}");
    }
    // A symbol's own font.
    let body = format!(r#"<w:p><w:r><w:sym w:font="{long_name}" w:char="F020"/></w:r></w:p>"#);
    let translation = translate(&open(&body));
    let left_out: Vec<&str> = translation
        .left_out()
        .iter()
        .map(LeftOut::message)
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
        // Values against values, not against none.
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
        let translation = translate(&open(&body));
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
        .map(LeftOut::message)
        .collect();
    assert_eq!(left_out, ["a font name of more than 256 bytes is cut"]);
    assert!(report(&tag(64)).is_empty());
    assert_eq!(
        report(&tag(65)),
        ["a language tag of more than 64 bytes is left out"]
    );
}
