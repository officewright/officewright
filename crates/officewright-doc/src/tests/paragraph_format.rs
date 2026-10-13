//! The paragraph formatting in effect: [`Formats::paragraph`] and [`Paragraph::format`].

use std::sync::Arc;

use super::format::paragraphs;
use super::*;

/// A document whose body is `body`, with a styles part holding `styles` (the children
/// of `w:styles`) and a numbering part holding `numbering` (the children of
/// `w:numbering`) when given.
fn build(body: &str, styles: &str, numbering: Option<&str>) -> Document {
    let rel = |id: &str, kind: &str, target: &str| {
        format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
        )
    };
    let mut rels = rel("rId1", "styles", "styles.xml");
    let mut parts = vec![(
        "word/styles.xml".to_owned(),
        format!(r#"<w:styles xmlns:w="{W}">{styles}</w:styles>"#),
    )];
    if let Some(numbering) = numbering {
        rels.push_str(&rel("rId2", "numbering", "numbering.xml"));
        parts.push((
            "word/numbering.xml".to_owned(),
            format!(r#"<w:numbering xmlns:w="{W}">{numbering}</w:numbering>"#),
        ));
    }
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#
    );
    let mut extra = vec![("word/_rels/document.xml.rels", rels.as_str())];
    extra.extend(
        parts
            .iter()
            .map(|(name, xml)| (name.as_str(), xml.as_str())),
    );
    let document = Document::from_bytes(package(DOCUMENT, &document_xml(body), &extra)).unwrap();
    assert!(messages(&document).is_empty(), "{:?}", messages(&document));
    document
}

/// As [`build`], for a document whose damage the reader reports.
fn build_damaged(body: &str, styles: &str, numbering: &str) -> Document {
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/></Relationships>"#;
    let styles = format!(r#"<w:styles xmlns:w="{W}">{styles}</w:styles>"#);
    let numbering = format!(r#"<w:numbering xmlns:w="{W}">{numbering}</w:numbering>"#);
    let extra = [
        ("word/_rels/document.xml.rels", rels),
        ("word/styles.xml", styles.as_str()),
        ("word/numbering.xml", numbering.as_str()),
    ];
    Document::from_bytes(package(DOCUMENT, &document_xml(body), &extra)).unwrap()
}

/// `w:docDefaults` with these paragraph properties.
fn defaults(p_pr: &str) -> String {
    format!("<w:docDefaults><w:pPrDefault><w:pPr>{p_pr}</w:pPr></w:pPrDefault></w:docDefaults>")
}

/// A style of `kind` with these paragraph properties.
fn style(kind: &str, id: &str, based_on: Option<&str>, p_pr: &str) -> String {
    let based_on = based_on.map_or_else(String::new, |id| format!(r#"<w:basedOn w:val="{id}"/>"#));
    format!(
        r#"<w:style w:type="{kind}" w:styleId="{id}"><w:name w:val="{id}"/>{based_on}<w:pPr>{p_pr}</w:pPr></w:style>"#
    )
}

/// A paragraph with the paragraph style `style` (none when empty) and these paragraph
/// properties after `w:pStyle`.
fn paragraph(style: &str, p_pr: &str) -> String {
    let p_style = if style.is_empty() {
        String::new()
    } else {
        format!(r#"<w:pStyle w:val="{style}"/>"#)
    };
    format!("<w:p><w:pPr>{p_style}{p_pr}</w:pPr><w:r><w:t>x</w:t></w:r></w:p>")
}

/// The formatting of every paragraph of the body, in order.
fn formats(document: &Document) -> Vec<ParagraphFormat> {
    let mut formats = document.formats();
    paragraphs(document)
        .into_iter()
        .map(|paragraph| formats.paragraph(paragraph))
        .collect()
}

fn tab(kind: TabKind, position: f64) -> (Option<TabKind>, f64) {
    (Some(kind), position)
}

fn tabs(format: &ParagraphFormat) -> Vec<(Option<TabKind>, f64)> {
    format
        .tabs()
        .iter()
        .map(|tab| (tab.kind(), tab.position().0))
        .collect()
}

#[test]
fn a_paragraph_without_styles_takes_the_defaults() {
    let format = open("<w:p/>")
        .sections()
        .next()
        .unwrap()
        .blocks()
        .next()
        .map(|block| {
            let Block::Paragraph(paragraph) = block else {
                panic!("a paragraph");
            };
            paragraph.format()
        });
    let format = format.unwrap();
    assert_eq!(format.style(), None);
    // Word aligns to the start when nothing gives an alignment (MS-OI29500 §17.3.1.13(a)).
    assert_eq!(format.justification(), Justification::Start);
    assert_eq!(
        (
            format.start_indent(),
            format.end_indent(),
            format.first_line_indent()
        ),
        (Pt(0.0), Pt(0.0), Pt(0.0))
    );
    assert_eq!(
        (format.before_spacing(), format.after_spacing()),
        (Pt(0.0), Pt(0.0))
    );
    assert_eq!(format.line_spacing(), LineSpacing::Multiple(1.0));
    // On when never given: ECMA-376 §17.3.1.44, §17.3.1.45, §17.3.1.32.
    assert!(format.widow_control() && format.word_wrap() && format.snap_to_grid());
    assert!(!format.keep_next() && !format.keep_lines() && !format.page_break_before());
    assert!(!format.contextual_spacing() && !format.right_to_left());
    assert_eq!(format.borders(), &ParagraphBorders::default());
    assert!(format.tabs().is_empty());
    assert_eq!(format.outline_level(), None);
    assert!(format.list().is_none());
}

#[test]
fn takes_each_property_from_the_last_level_that_gives_it() {
    let styles = [
        defaults(
            r#"<w:widowControl w:val="0"/><w:spacing w:after="160" w:line="259" w:lineRule="auto"/>"#,
        ),
        r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>"#.to_owned(),
        style("paragraph", "Base", None, r#"<w:ind w:left="720" w:right="200"/><w:jc w:val="center"/>"#),
        style("paragraph", "Heading", Some("Base"), r#"<w:keepNext/><w:spacing w:before="240"/>"#),
    ]
    .concat();
    let body = [
        paragraph("", ""),
        paragraph(
            "Heading",
            r#"<w:spacing w:after="0"/><w:ind w:right="360"/>"#,
        ),
    ]
    .concat();
    let formats = formats(&build(&body, &styles, None));
    let [plain, heading] = &formats[..] else {
        panic!("two paragraphs");
    };
    assert_eq!(plain.style(), Some("Normal"));
    assert!(!plain.widow_control());
    assert_eq!(plain.after_spacing(), Pt(8.0));
    assert_eq!(plain.line_spacing(), LineSpacing::Multiple(259.0 / 240.0));
    // Every attribute of w:ind and w:spacing is a property of its own (ECMA-376
    // §17.3.1.12).
    assert_eq!(heading.style(), Some("Heading"));
    assert_eq!(heading.justification(), Justification::Center);
    assert_eq!(
        (heading.start_indent(), heading.end_indent()),
        (Pt(36.0), Pt(18.0))
    );
    assert_eq!(
        (heading.before_spacing(), heading.after_spacing()),
        (Pt(12.0), Pt(0.0))
    );
    assert_eq!(heading.line_spacing(), LineSpacing::Multiple(259.0 / 240.0));
    assert!(heading.keep_next() && !heading.widow_control());
}

#[test]
fn resolves_first_line_and_hanging_indents() {
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "FirstLine",
            None,
            r#"<w:ind w:firstLine="720"/>"#,
        ),
        style("paragraph", "Hanging", None, r#"<w:ind w:hanging="360"/>"#),
    ]
    .concat();
    let body = [
        // In one w:ind the hanging indent wins (ECMA-376 §17.3.1.12).
        paragraph("", r#"<w:ind w:firstLine="720" w:hanging="360"/>"#),
        paragraph("FirstLine", r#"<w:ind w:hanging="180"/>"#),
        paragraph("Hanging", r#"<w:ind w:firstLine="240"/>"#),
        paragraph("Hanging", ""),
    ]
    .concat();
    let first_lines: Vec<_> = formats(&build(&body, &styles, None))
        .iter()
        .map(|format| format.first_line_indent().0)
        .collect();
    assert_eq!(first_lines, [-18.0, -9.0, 12.0, -18.0]);
}

#[test]
fn character_and_line_units_win_over_points() {
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Chars",
            None,
            r#"<w:spacing w:beforeLines="100"/><w:ind w:leftChars="200" w:hangingChars="150"/>"#,
        ),
    ]
    .concat();
    let body = [
        // A character or line unit from an earlier level wins over points of a later one
        // (MS-OI29500 §17.3.1.12(b), §17.3.1.33(a)).
        paragraph(
            "Chars",
            r#"<w:spacing w:before="240"/><w:ind w:left="1440"/>"#,
        ),
        // A character unit of 0 cancels those of the levels below (§17.3.1.12(a)).
        paragraph(
            "Chars",
            r#"<w:spacing w:beforeLines="0"/><w:ind w:leftChars="0"/>"#,
        ),
    ]
    .concat();
    let formats = formats(&build(&body, &styles, None));
    let [chars, cancelled] = &formats[..] else {
        panic!("two paragraphs");
    };
    assert_eq!(chars.start_indent_chars(), Some(200));
    assert_eq!(chars.start_indent(), Pt(72.0));
    assert_eq!(chars.first_line_indent_chars(), Some(-150));
    assert_eq!(chars.before_spacing_lines(), Some(100));
    assert_eq!(cancelled.start_indent_chars(), None);
    assert_eq!(cancelled.before_spacing_lines(), None);
    assert_eq!(cancelled.first_line_indent_chars(), Some(-150));
}

#[test]
fn adds_up_tab_stops_and_clears_those_of_lower_levels() {
    let styles = [
        defaults(r#"<w:tabs><w:tab w:val="left" w:pos="5000"/></w:tabs>"#),
        style(
            "paragraph",
            "Tabbed",
            None,
            r#"<w:tabs><w:tab w:val="left" w:pos="720"/><w:tab w:val="decimal" w:pos="2880"/></w:tabs>"#,
        ),
    ]
    .concat();
    let body = [
        paragraph("Tabbed", ""),
        paragraph(
            "Tabbed",
            r#"<w:tabs><w:tab w:val="clear" w:pos="720"/><w:tab w:val="center" w:pos="4320"/><w:tab w:val="right" w:pos="2880"/><w:tab w:val="clear" w:pos="5000"/></w:tabs>"#,
        ),
    ]
    .concat();
    let formats = formats(&build(&body, &styles, None));
    assert_eq!(
        tabs(&formats[0]),
        [
            tab(TabKind::Left, 36.0),
            tab(TabKind::Decimal, 144.0),
            tab(TabKind::Left, 250.0),
        ]
    );
    assert_eq!(
        tabs(&formats[1]),
        [tab(TabKind::Right, 144.0), tab(TabKind::Center, 216.0)]
    );
}

#[test]
fn takes_each_border_from_the_level_that_gives_it() {
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Boxed",
            None,
            r#"<w:pBdr><w:top w:val="single" w:sz="4" w:space="1" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="1" w:color="auto"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="EEEEEE"/>"#,
        ),
    ]
    .concat();
    let body = paragraph(
        "Boxed",
        r#"<w:pBdr><w:bottom w:val="double" w:sz="12" w:space="1" w:color="FF0000"/></w:pBdr>"#,
    );
    let format = &formats(&build(&body, &styles, None))[0];
    let borders = format.borders();
    assert_eq!(borders.top().unwrap().style(), Some(BorderStyle::Single));
    assert_eq!(borders.bottom().unwrap().style(), Some(BorderStyle::Double));
    assert!(borders.start().is_none());
    assert!(format.shading().is_some());
}

#[test]
fn takes_the_outline_level_of_built_in_headings_from_their_ids() {
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Heading2",
            None,
            r#"<w:outlineLvl w:val="5"/>"#,
        ),
        style("paragraph", "Chapter", None, r#"<w:outlineLvl w:val="3"/>"#),
        style("paragraph", "Body", None, r#"<w:outlineLvl w:val="9"/>"#),
        style(
            "paragraph",
            "Heading01",
            None,
            r#"<w:outlineLvl w:val="4"/>"#,
        ),
        style("paragraph", "Heading+1", None, ""),
        style("paragraph", "Heading9", None, ""),
    ]
    .concat();
    let body = [
        paragraph("Heading2", ""),
        paragraph("Chapter", ""),
        paragraph("Body", ""),
        paragraph("Chapter", r#"<w:outlineLvl w:val="0"/>"#),
        paragraph("Heading01", ""),
        paragraph("Heading+1", ""),
        paragraph("Heading9", ""),
    ]
    .concat();
    let levels: Vec<_> = formats(&build(&body, &styles, None))
        .iter()
        .map(ParagraphFormat::outline_level)
        .collect();
    // MS-OI29500 §17.3.1.20(a): Heading1 to Heading9 by their ids; 9 is body text.
    assert_eq!(
        levels,
        [Some(1), Some(3), None, Some(0), Some(4), None, Some(8)]
    );
}

/// An abstract numbering definition with levels 0 to 2, each indented by its number.
fn definition(id: i64, extra_level_2: &str) -> String {
    let levels: String = (0..3)
        .map(|level| {
            let indent = 720 * (level + 1);
            let extra = if level == 2 { extra_level_2 } else { "" };
            format!(
                r#"<w:lvl w:ilvl="{level}"><w:start w:val="{start}"/><w:numFmt w:val="decimal"/>{extra}<w:lvlText w:val="%{number}."/><w:lvlJc w:val="left"/><w:pPr><w:spacing w:before="999"/><w:ind w:left="{indent}" w:hanging="360"/><w:jc w:val="right"/></w:pPr></w:lvl>"#,
                start = level + 3,
                number = level + 1,
            )
        })
        .collect();
    format!(r#"<w:abstractNum w:abstractNumId="{id}">{levels}</w:abstractNum>"#)
}

#[test]
fn resolves_the_list_and_lays_its_level_between_the_style_and_direct_formatting() {
    let numbering = [
        definition(10, r#"<w:pStyle w:val="ListStyle"/>"#),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
        // ECMA-376 §17.9.18: 0 never names an instance, even one written with that id.
        r#"<w:num w:numId="0"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [
        defaults(""),
        style("paragraph", "Indented", None, r#"<w:ind w:left="2000"/>"#),
        style(
            "paragraph",
            "ListStyle",
            None,
            r#"<w:numPr><w:numId w:val="1"/></w:numPr>"#,
        ),
        style(
            "paragraph",
            "StyleLevel",
            None,
            r#"<w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr>"#,
        ),
    ]
    .concat();
    let numbered = |level: i64, id: i64| {
        format!(r#"<w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{id}"/></w:numPr>"#)
    };
    let body = [
        paragraph("", &numbered(1, 1)),
        // The level's indent wins over the paragraph style's (MS-OI29500 §17.7.2(a)),
        // direct formatting over the level's.
        paragraph("Indented", &numbered(0, 1)),
        paragraph("", &format!(r#"{}<w:ind w:left="0"/>"#, numbered(0, 1))),
        // A style's list: the level whose w:pStyle names the style (ECMA-376 §17.9.23).
        paragraph("ListStyle", ""),
        // Else the level the style writes (MS-OI29500 §17.3.1.19(a)).
        paragraph("StyleLevel", ""),
        // Instance 0 takes the paragraph out of its style's list.
        paragraph("ListStyle", &numbered(0, 0)),
        // A missing instance and a level beyond 8 are no list.
        paragraph("", &numbered(0, 7)),
        paragraph("", &numbered(9, 1)),
        // The instance in direct formatting, the level from the style.
        paragraph("StyleLevel", r#"<w:numPr><w:numId w:val="1"/></w:numPr>"#),
    ]
    .concat();
    let document = build(&body, &styles, Some(&numbering));
    let formats = formats(&document);
    let lists: Vec<_> = formats
        .iter()
        .map(|format| {
            format.list().map(|list| {
                (
                    list.instance(),
                    list.definition(),
                    list.level_number(),
                    list.start(),
                )
            })
        })
        .collect();
    assert_eq!(
        lists,
        [
            Some((1, Some(10), 1, 4)),
            Some((1, Some(10), 0, 3)),
            Some((1, Some(10), 0, 3)),
            Some((1, Some(10), 2, 5)),
            Some((1, Some(10), 1, 4)),
            None,
            None,
            None,
            Some((1, Some(10), 1, 4)),
        ]
    );
    let indents: Vec<_> = formats
        .iter()
        .map(|format| (format.start_indent().0, format.first_line_indent().0))
        .collect();
    assert_eq!(
        indents,
        [
            (72.0, -18.0),
            (36.0, -18.0),
            (0.0, -18.0),
            (108.0, -18.0),
            (72.0, -18.0),
            (0.0, 0.0),
            (0.0, 0.0),
            (0.0, 0.0),
            (72.0, -18.0),
        ]
    );
    // Word takes only the alignment, indentation and tabs of a level's w:pPr
    // (MS-OI29500 §17.9.22(b)): its spacing is not applied.
    assert_eq!(formats[0].justification(), Justification::Right);
    assert_eq!(formats[0].before_spacing(), Pt(0.0));
    assert_eq!(
        formats[0].list().unwrap().level(&document).unwrap().text(),
        Some("%2.")
    );
}

#[test]
fn applies_the_overrides_of_an_instance() {
    let numbering = [
        definition(10, ""),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="7"/></w:lvlOverride><w:lvlOverride w:ilvl="1"><w:lvl w:ilvl="5"><w:start w:val="40"/><w:numFmt w:val="upperRoman"/><w:lvlText w:val="(%2)"/><w:pPr><w:ind w:left="3000" w:hanging="500"/></w:pPr></w:lvl></w:lvlOverride></w:num>"#.to_owned(),
    ]
    .concat();
    let numbered =
        |level: i64| format!(r#"<w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="1"/></w:numPr>"#);
    let body = [paragraph("", &numbered(0)), paragraph("", &numbered(1))].concat();
    let document = build(&body, &defaults(""), Some(&numbering));
    let formats = formats(&document);
    let start_override = formats[0].list().unwrap();
    assert_eq!(start_override.start(), 7);
    assert_eq!(start_override.level(&document).unwrap().text(), Some("%1."));
    // An override's level replaces the definition's, but Word ignores its start
    // (MS-OI29500 §17.9.25(b)).
    let replaced = formats[1].list().unwrap();
    assert_eq!(replaced.level(&document).unwrap().text(), Some("(%2)"));
    // The replacement is level 1, whatever its own w:ilvl says.
    assert_eq!(replaced.level(&document).unwrap().level(), 1);
    assert_eq!(
        replaced.level(&document).unwrap().number_format(),
        Some(NumberFormat::UpperRoman)
    );
    assert_eq!(replaced.start(), 4);
    assert_eq!(formats[1].start_indent(), Pt(150.0));
    assert_eq!(formats[1].first_line_indent(), Pt(-25.0));
}

#[test]
fn follows_the_link_of_a_numbering_style_to_its_definition() {
    let numbering = [
        definition(30, ""),
        // ECMA-376 §17.9.21: definition 20 stands for the numbering style, whose
        // instance names definition 30.
        r#"<w:abstractNum w:abstractNumId="20"><w:numStyleLink w:val="OutlineList"/></w:abstractNum>"#.to_owned(),
        r#"<w:num w:numId="3"><w:abstractNumId w:val="30"/></w:num>"#.to_owned(),
        r#"<w:num w:numId="4"><w:abstractNumId w:val="20"/></w:num>"#.to_owned(),
        // A link that leads back to itself ends without levels.
        r#"<w:abstractNum w:abstractNumId="40"><w:numStyleLink w:val="Loop"/></w:abstractNum>"#.to_owned(),
        r#"<w:num w:numId="5"><w:abstractNumId w:val="40"/></w:num>"#.to_owned(),
        // So does a link to NoList, whose children Word ignores (MS-OI29500
        // §17.7.4.17(a)), and a definition that does not exist.
        r#"<w:abstractNum w:abstractNumId="50"><w:numStyleLink w:val="NoList"/></w:abstractNum>"#.to_owned(),
        r#"<w:num w:numId="6"><w:abstractNumId w:val="50"/></w:num>"#.to_owned(),
        r#"<w:num w:numId="7"><w:abstractNumId w:val="99"/></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [
        defaults(""),
        style(
            "numbering",
            "OutlineList",
            None,
            r#"<w:numPr><w:numId w:val="3"/></w:numPr>"#,
        ),
        style(
            "numbering",
            "Loop",
            None,
            r#"<w:numPr><w:numId w:val="5"/></w:numPr>"#,
        ),
        style(
            "numbering",
            "NoList",
            None,
            r#"<w:numPr><w:numId w:val="3"/></w:numPr>"#,
        ),
    ]
    .concat();
    let numbered =
        |id: i64| format!(r#"<w:numPr><w:ilvl w:val="1"/><w:numId w:val="{id}"/></w:numPr>"#);
    let body = [
        paragraph("", &numbered(4)),
        paragraph("", &numbered(5)),
        paragraph("", &numbered(6)),
        paragraph("", &numbered(7)),
    ]
    .concat();
    let document = build(&body, &styles, Some(&numbering));
    let formats = formats(&document);
    let linked = formats[0].list().unwrap();
    assert_eq!((linked.instance(), linked.definition()), (4, Some(30)));
    assert_eq!(linked.level(&document).unwrap().text(), Some("%2."));
    assert_eq!(formats[0].start_indent(), Pt(72.0));
    for format in &formats[1..] {
        let list = format.list().unwrap();
        assert_eq!((list.definition(), list.level(&document)), (None, None));
    }
}

#[test]
fn a_paragraph_of_another_document_takes_the_styles_of_its_own() {
    let first = build(
        &paragraph("P", ""),
        &[
            defaults(""),
            style("paragraph", "P", None, r#"<w:jc w:val="center"/>"#),
        ]
        .concat(),
        None,
    );
    let second = build(
        &paragraph("P", ""),
        &[
            defaults(""),
            style("paragraph", "P", None, r#"<w:jc w:val="right"/>"#),
        ]
        .concat(),
        None,
    );
    let theirs = paragraphs(&second)[0];
    let format = first.formats().paragraph(theirs);
    assert_eq!(format.justification(), Justification::Right);
    assert_eq!(format, theirs.format());
}

#[test]
fn instances_of_one_definition_share_its_levels() {
    // A document may name a definition through many instances: their levels are parsed
    // once, and only overridden ones are parsed again.
    let mut numbering = definition(10, "");
    for id in 1..=2000 {
        numbering.push_str(&format!(
            r#"<w:num w:numId="{id}"><w:abstractNumId w:val="10"/></w:num>"#
        ));
    }
    numbering.push_str(
        r#"<w:num w:numId="3000"><w:abstractNumId w:val="10"/><w:lvlOverride w:ilvl="0"><w:lvl w:ilvl="0"><w:lvlText w:val="*"/></w:lvl></w:lvlOverride></w:num>"#,
    );
    let body: String = (1..=2000)
        .chain([3000])
        .map(|id| {
            paragraph(
                "",
                &format!(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{id}"/></w:numPr>"#),
            )
        })
        .collect();
    let document = build(&body, &defaults(""), Some(&numbering));
    let mut formats = document.formats();
    let results: Vec<_> = paragraphs(&document)
        .into_iter()
        .map(|paragraph| formats.paragraph(paragraph))
        .collect();
    assert_eq!(formats.work().definitions, 1);
    assert_eq!(formats.work().replacements, 1);
    // The instances without an override share the definition's level, laid over the
    // defaults once; the override's level gives nothing to lay.
    assert_eq!(formats.work().level_layers, 1);
    assert_eq!(formats.work().combinations, 1);
    let text = |format: &ParagraphFormat| format.list().unwrap().level(&document).unwrap().text;
    assert!(
        results[..2000]
            .iter()
            .all(|format| text(format).as_deref() == Some("%1."))
    );
    assert_eq!(text(&results[2000]).as_deref(), Some("*"));
}

#[test]
fn orders_tab_stops_and_keeps_the_last_of_one_position() {
    let body = paragraph(
        "",
        r#"<w:tabs><w:tab w:val="right" w:pos="2880"/><w:tab w:val="left" w:pos="720"/><w:tab w:val="center" w:pos="2880"/></w:tabs>"#,
    );
    let format = &formats(&build(&body, &defaults(""), None))[0];
    assert_eq!(
        tabs(format),
        [tab(TabKind::Left, 36.0), tab(TabKind::Center, 144.0)]
    );
}

#[test]
fn takes_the_list_level_of_the_document_defaults() {
    let numbering = [
        definition(10, ""),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = defaults(r#"<w:numPr><w:ilvl w:val="2"/><w:numId w:val="1"/></w:numPr>"#);
    let format = &formats(&build(&paragraph("", ""), &styles, Some(&numbering)))[0];
    assert_eq!(format.list().unwrap().level_number(), 2);
}

#[test]
fn keeps_at_most_64_tab_stops_at_the_lowest_positions() {
    // [MS-DOC] 2.9.179: Word's own format adds at most 64 stops in one change. Without a
    // bound, stops every paragraph of a style inherits would cost each paragraph their
    // number.
    let stops = |from: i64, count: i64| -> String {
        let tabs: String = (0..count)
            .map(|n| format!(r#"<w:tab w:val="left" w:pos="{}"/>"#, from + 100 * n))
            .collect();
        format!("<w:tabs>{tabs}</w:tabs>")
    };
    let styles = [
        defaults(&stops(10_000, 100)),
        style("paragraph", "Tabbed", None, &stops(100, 100)),
    ]
    .concat();
    let body = [paragraph("Tabbed", ""), paragraph("Tabbed", &stops(50, 3))].concat();
    let mut results = formats(&build(&body, &styles, None));
    // Stops at one level only, with nothing to merge them with.
    results.extend(formats(&build(
        &paragraph("", &stops(100, 100)),
        &defaults(""),
        None,
    )));
    for format in &results {
        assert_eq!(format.tabs().len(), 64);
        assert!(
            format
                .tabs()
                .windows(2)
                .all(|pair| pair[0].position().0 < pair[1].position().0)
        );
    }
    assert_eq!(results[0].tabs()[0].position(), Pt(5.0));
    assert_eq!(results[0].tabs()[63].position(), Pt(320.0));
    // The paragraph's own stops come first by position.
    assert_eq!(results[1].tabs()[0].position(), Pt(2.5));
    assert_eq!(results[1].tabs()[1].position(), Pt(5.0));
    assert_eq!(results[2].tabs()[63].position(), Pt(320.0));
}

#[test]
fn follows_each_link_once_and_shares_the_levels_it_leads_to() {
    // 300 definitions that only link to one numbering style, two instances of each and
    // two paragraphs per instance: the links are followed once per definition and the
    // definition they lead to is parsed once.
    let mut numbering = definition(9000, "");
    numbering.push_str(r#"<w:num w:numId="1000"><w:abstractNumId w:val="9000"/></w:num>"#);
    for id in 1..=300 {
        numbering.push_str(&format!(
            r#"<w:abstractNum w:abstractNumId="{id}"><w:numStyleLink w:val="Linked"/></w:abstractNum><w:num w:numId="{id}"><w:abstractNumId w:val="{id}"/></w:num><w:num w:numId="{}"><w:abstractNumId w:val="{id}"/></w:num>"#,
            id + 2000
        ));
    }
    let styles = [
        defaults(""),
        style(
            "numbering",
            "Linked",
            None,
            r#"<w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs><w:numPr><w:numId w:val="1000"/></w:numPr>"#,
        ),
    ]
    .concat();
    let body: String = (1..=300)
        .flat_map(|id| [id, id, id + 2000, id + 2000])
        .map(|id| {
            paragraph(
                "",
                &format!(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{id}"/></w:numPr>"#),
            )
        })
        .collect();
    let document = build(&body, &styles, Some(&numbering));
    let mut formats = document.formats();
    let lists: Vec<_> = paragraphs(&document)
        .into_iter()
        .map(|paragraph| formats.paragraph(paragraph).list().cloned().unwrap())
        .collect();
    assert!(lists.iter().all(|list| list.definition() == Some(9000)));
    assert!(
        lists
            .iter()
            .all(|list| list.level(&document).unwrap().text() == Some("%1."))
    );
    // Each linking definition's link is read once, and so is the definition they reach.
    assert_eq!(formats.work().links, 301);
    assert_eq!(formats.work().numbering_styles, 1);
    assert_eq!(formats.work().definitions, 1);
}

#[test]
fn reads_only_the_overrides_of_the_levels_paragraphs_use() {
    // Overrides of every level, and many of levels that do not exist: only the level the
    // paragraphs use is replaced, once.
    let mut overrides: String = (0..9)
        .map(|level| {
            format!(
                r#"<w:lvlOverride w:ilvl="{level}"><w:lvl w:ilvl="{level}"><w:lvlText w:val="R{level}"/></w:lvl></w:lvlOverride>"#
            )
        })
        .collect();
    for level in 100..1100 {
        overrides.push_str(&format!(
            r#"<w:lvlOverride w:ilvl="{level}"><w:lvl w:ilvl="0"/></w:lvlOverride>"#
        ));
    }
    let numbering = [
        definition(10, ""),
        format!(r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/>{overrides}</w:num>"#),
    ]
    .concat();
    let item = paragraph(
        "",
        r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>"#,
    );
    let body = [item.as_str(); 50].concat();
    let document = build(&body, &defaults(""), Some(&numbering));
    let mut formats = document.formats();
    for paragraph in paragraphs(&document) {
        let format = formats.paragraph(paragraph);
        assert_eq!(
            format.list().unwrap().level(&document).unwrap().text(),
            Some("R0")
        );
    }
    assert_eq!(formats.work().replacements, 1);
}

#[test]
fn works_out_each_list_definition_and_style_match_once() {
    // 400 list paragraphs of one style, whose level the definition's w:pStyle names: the
    // definition is parsed once and searched for the style once.
    let numbering = [
        definition(10, r#"<w:pStyle w:val="Body"/>"#),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Body",
            None,
            r#"<w:numPr><w:numId w:val="1"/></w:numPr><w:jc w:val="both"/>"#,
        ),
    ]
    .concat();
    let body = paragraph("Body", "").repeat(400);
    let document = build(&body, &styles, Some(&numbering));
    let mut formats = document.formats();
    let results: Vec<_> = paragraphs(&document)
        .into_iter()
        .map(|paragraph| formats.paragraph(paragraph))
        .collect();
    // The list level's alignment wins over the style's (MS-OI29500 §17.7.2(a)).
    assert!(results.iter().all(|format| {
        format.justification() == Justification::Right && format.list().unwrap().level_number() == 2
    }));
    // The paragraphs share the style's id rather than copying it: an id may be long.
    let id = results[0].style.clone().unwrap();
    assert!(
        results
            .iter()
            .all(|format| Arc::ptr_eq(format.style.as_ref().unwrap(), &id))
    );
    assert_eq!(formats.work().definitions, 1);
    assert_eq!(formats.work().combinations, 1);
}

#[test]
fn reads_each_on_off_property_from_its_own_element() {
    // One paragraph per property, the property set at the style level; every other
    // property keeps its default. Widow control, word wrap and snapping to the grid are
    // on when not given, so the style turns those off.
    type Getter = fn(&ParagraphFormat) -> bool;
    let flags: [(&str, Getter, bool); 11] = [
        ("keepNext", ParagraphFormat::keep_next, false),
        ("keepLines", ParagraphFormat::keep_lines, false),
        ("pageBreakBefore", ParagraphFormat::page_break_before, false),
        ("widowControl", ParagraphFormat::widow_control, true),
        (
            "suppressLineNumbers",
            ParagraphFormat::suppress_line_numbers,
            false,
        ),
        (
            "suppressAutoHyphens",
            ParagraphFormat::suppress_auto_hyphens,
            false,
        ),
        (
            "contextualSpacing",
            ParagraphFormat::contextual_spacing,
            false,
        ),
        ("mirrorIndents", ParagraphFormat::mirror_indents, false),
        ("wordWrap", ParagraphFormat::word_wrap, true),
        ("bidi", ParagraphFormat::right_to_left, false),
        ("snapToGrid", ParagraphFormat::snap_to_grid, true),
    ];
    for (element, _, default) in &flags {
        let written = if *default {
            format!(r#"<w:{element} w:val="0"/>"#)
        } else {
            format!("<w:{element}/>")
        };
        let styles = [defaults(""), style("paragraph", "Flag", None, &written)].concat();
        let format = &formats(&build(&paragraph("Flag", ""), &styles, None))[0];
        for (other, getter, other_default) in &flags {
            let expected = if other == element {
                !other_default
            } else {
                *other_default
            };
            assert_eq!(getter(format), expected, "w:{other} with w:{element} set");
        }
    }
}

#[test]
fn takes_each_border_and_unit_from_the_level_that_gives_it() {
    let border = |side: &str, kind: &str| {
        format!(r#"<w:{side} w:val="{kind}" w:sz="4" w:space="1" w:color="auto"/>"#)
    };
    let styles = [
        defaults(&format!(
            r#"<w:pBdr>{}{}</w:pBdr><w:spacing w:beforeAutospacing="1" w:afterLines="50"/><w:ind w:rightChars="100"/>"#,
            border("between", "dotted"),
            border("bar", "dashed"),
        )),
        style(
            "paragraph",
            "Sides",
            None,
            &format!(
                r#"<w:pBdr>{}{}</w:pBdr><w:spacing w:afterAutospacing="1"/><w:ind w:right="200"/>"#,
                border("left", "double"),
                border("right", "thick"),
            ),
        ),
    ]
    .concat();
    // Direct formatting gives other attributes of the same elements: what the levels
    // below give stays.
    let body = paragraph(
        "Sides",
        &format!(
            r#"<w:pBdr>{}{}</w:pBdr><w:spacing w:line="300" w:lineRule="auto"/><w:ind w:left="100" w:firstLineChars="100" w:hangingChars="200"/>"#,
            border("top", "single"),
            border("bottom", "wave"),
        ),
    );
    let format = &formats(&build(&body, &styles, None))[0];
    let borders = format.borders();
    let side = |border: Option<&Border>| border.and_then(Border::style);
    assert_eq!(
        [
            side(borders.top()),
            side(borders.bottom()),
            side(borders.start()),
            side(borders.end()),
            side(borders.between()),
            side(borders.bar()),
        ],
        [
            Some(BorderStyle::Single),
            Some(BorderStyle::Wave),
            Some(BorderStyle::Double),
            Some(BorderStyle::Thick),
            Some(BorderStyle::Dotted),
            Some(BorderStyle::Dashed),
        ]
    );
    assert_eq!(format.end_indent_chars(), Some(100));
    assert_eq!(format.end_indent(), Pt(10.0));
    assert_eq!(format.after_spacing_lines(), Some(50));
    assert!(format.before_auto_spacing() && format.after_auto_spacing());
    // In one w:ind the hanging indent wins in character units too (ECMA-376 §17.3.1.12).
    assert_eq!(format.first_line_indent_chars(), Some(-200));
}

#[test]
fn adds_the_tab_stops_of_the_list_level() {
    let level = r#"<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:tabs><w:tab w:val="num" w:pos="1080"/></w:tabs><w:ind w:left="1440" w:hanging="360"/></w:pPr></w:lvl>"#;
    let numbering = format!(
        r#"<w:abstractNum w:abstractNumId="10">{level}</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#
    );
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Tabbed",
            None,
            r#"<w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs>"#,
        ),
    ]
    .concat();
    let body = paragraph(
        "Tabbed",
        r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr><w:tabs><w:tab w:val="right" w:pos="2880"/></w:tabs>"#,
    );
    let format = &formats(&build(&body, &styles, Some(&numbering)))[0];
    assert_eq!(
        tabs(format),
        [
            tab(TabKind::Left, 36.0),
            tab(TabKind::Number, 54.0),
            tab(TabKind::Right, 144.0),
        ]
    );
}

#[test]
fn takes_the_level_a_definition_links_to_the_style_before_the_styles_own() {
    // The style writes level 0, level 2 of the definition names the style: the level of
    // w:pStyle comes first (ECMA-376 §17.9.23). Word calls the combination unpredictable
    // (MS-OI29500 §17.3.1.19(a)); a rule document for Word to render is pending.
    let numbering = [
        definition(10, r#"<w:pStyle w:val="Linked"/>"#),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Linked",
            None,
            r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>"#,
        ),
    ]
    .concat();
    let format = &formats(&build(&paragraph("Linked", ""), &styles, Some(&numbering)))[0];
    assert_eq!(format.list().unwrap().level_number(), 2);
}

#[test]
fn bounds_set_and_cleared_tab_stops_apart() {
    // [MS-DOC] 2.9.179 and 2.9.180: one change adds at most 64 stops and removes at most
    // 64, so cleared stops do not take the place of set ones.
    let set: String = (1..=64)
        .map(|n| format!(r#"<w:tab w:val="left" w:pos="{}"/>"#, 100 * n))
        .collect();
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Tabbed",
            None,
            &format!("<w:tabs>{set}</w:tabs>"),
        ),
    ]
    .concat();
    let clear = |count: i64| -> String {
        let cleared: String = (1..=count)
            .map(|n| format!(r#"<w:tab w:val="clear" w:pos="{}"/>"#, 100 * n))
            .collect();
        format!(r#"<w:tabs>{cleared}<w:tab w:val="right" w:pos="7000"/></w:tabs>"#)
    };
    let body = [
        paragraph("Tabbed", &clear(64)),
        paragraph("Tabbed", &clear(10)),
    ]
    .concat();
    let results = formats(&build(&body, &styles, None));
    assert_eq!(tabs(&results[0]), [tab(TabKind::Right, 350.0)]);
    assert_eq!(results[1].tabs().len(), 55);
    assert_eq!(results[1].tabs()[0].position(), Pt(55.0));
    assert_eq!(results[1].tabs()[54].position(), Pt(350.0));
}

#[test]
fn inherits_the_list_instance_and_level_through_based_on_styles() {
    let numbering = [
        definition(10, ""),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Instance",
            None,
            r#"<w:numPr><w:numId w:val="1"/></w:numPr>"#,
        ),
        style(
            "paragraph",
            "InstanceThenLevel",
            Some("Instance"),
            r#"<w:numPr><w:ilvl w:val="2"/></w:numPr>"#,
        ),
        style(
            "paragraph",
            "Level",
            None,
            r#"<w:numPr><w:ilvl w:val="1"/></w:numPr>"#,
        ),
        style(
            "paragraph",
            "LevelThenInstance",
            Some("Level"),
            r#"<w:numPr><w:numId w:val="1"/></w:numPr>"#,
        ),
    ]
    .concat();
    let body = [
        paragraph("InstanceThenLevel", ""),
        paragraph("LevelThenInstance", ""),
    ]
    .concat();
    let levels: Vec<_> = formats(&build(&body, &styles, Some(&numbering)))
        .iter()
        .map(|format| {
            format
                .list()
                .map(|list| (list.instance(), list.level_number()))
        })
        .collect();
    assert_eq!(levels, [Some((1, 2)), Some((1, 1))]);
}

#[test]
fn follows_each_link_once_through_definitions_on_the_way() {
    // 300 definitions link through NS1 to definition 500, which links through NS2 to
    // definition 600: each definition's link is read once.
    let mut numbering = definition(600, "");
    numbering.push_str(
        r#"<w:abstractNum w:abstractNumId="500"><w:numStyleLink w:val="NS2"/></w:abstractNum><w:num w:numId="5000"><w:abstractNumId w:val="500"/></w:num><w:num w:numId="6000"><w:abstractNumId w:val="600"/></w:num>"#,
    );
    for id in 1..=300 {
        numbering.push_str(&format!(
            r#"<w:abstractNum w:abstractNumId="{id}"><w:numStyleLink w:val="NS1"/></w:abstractNum><w:num w:numId="{id}"><w:abstractNumId w:val="{id}"/></w:num>"#
        ));
    }
    let styles = [
        defaults(""),
        style(
            "numbering",
            "NS1",
            None,
            r#"<w:numPr><w:numId w:val="5000"/></w:numPr>"#,
        ),
        style(
            "numbering",
            "NS2",
            None,
            r#"<w:numPr><w:numId w:val="6000"/></w:numPr>"#,
        ),
    ]
    .concat();
    let body: String = (1..=300)
        .map(|id| {
            paragraph(
                "",
                &format!(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{id}"/></w:numPr>"#),
            )
        })
        .collect();
    let document = build(&body, &styles, Some(&numbering));
    let mut formats = document.formats();
    for paragraph in paragraphs(&document) {
        assert_eq!(
            formats.paragraph(paragraph).list().unwrap().definition(),
            Some(600)
        );
    }
    assert_eq!(formats.work().links, 302);
    assert_eq!(formats.work().numbering_styles, 2);
    assert_eq!(formats.work().definitions, 1);
}

#[test]
fn takes_the_first_override_of_a_level_and_the_last_num_pr_of_a_numbering_style() {
    let numbering = [
        definition(10, ""),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride><w:lvlOverride w:ilvl="0"><w:startOverride w:val="9"/></w:lvlOverride></w:num>"#.to_owned(),
        definition(11, ""),
        r#"<w:num w:numId="2"><w:abstractNumId w:val="11"/></w:num>"#.to_owned(),
        r#"<w:abstractNum w:abstractNumId="30"><w:numStyleLink w:val="Linked"/></w:abstractNum><w:num w:numId="3"><w:abstractNumId w:val="30"/></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [
        defaults(""),
        // A w:pPr with two w:numPr: the last counts, as for any paragraph properties.
        style(
            "numbering",
            "Linked",
            None,
            r#"<w:numPr><w:numId w:val="1"/></w:numPr><w:numPr><w:numId w:val="2"/></w:numPr>"#,
        ),
    ]
    .concat();
    let numbered = |id: i64| {
        paragraph(
            "",
            &format!(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{id}"/></w:numPr>"#),
        )
    };
    let body = [numbered(1), numbered(3)].concat();
    // The reader reports the second override of level 0.
    let results = formats(&build_damaged(&body, &styles, &numbering));
    assert_eq!(results[0].list().unwrap().start(), 5);
    // Instance 3's definition links through Linked to instance 2's definition, 11.
    assert_eq!(results[1].list().unwrap().definition(), Some(11));
}

#[test]
fn an_instance_without_its_definition_has_no_levels_even_with_overrides() {
    let numbering = r#"<w:num w:numId="1"><w:abstractNumId w:val="99"/><w:lvlOverride w:ilvl="0"><w:lvl w:ilvl="0"><w:lvlText w:val="*"/><w:pPr><w:ind w:left="2880"/></w:pPr></w:lvl></w:lvlOverride></w:num>"#;
    let body = paragraph(
        "",
        r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>"#,
    );
    let document = build(&body, &defaults(""), Some(numbering));
    let format = &formats(&document)[0];
    let list = format.list().unwrap();
    assert_eq!((list.definition(), list.level(&document)), (None, None));
    assert_eq!(format.start_indent(), Pt(0.0));
}

#[test]
fn matches_a_level_to_a_style_within_its_own_definition() {
    // Definition 10's level 2 names style S, definition 20 names no style. A paragraph
    // of S in instance 2 (definition 20) without a w:ilvl of its own takes S's level,
    // 0, not the level definition 10 names S at.
    let numbering = [
        definition(10, r#"<w:pStyle w:val="S"/>"#),
        definition(20, ""),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
        r#"<w:num w:numId="2"><w:abstractNumId w:val="20"/></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "S",
            None,
            r#"<w:numPr><w:numId w:val="1"/></w:numPr>"#,
        ),
    ]
    .concat();
    let body = [
        paragraph("S", ""),
        paragraph("S", r#"<w:numPr><w:numId w:val="2"/></w:numPr>"#),
    ]
    .concat();
    let levels: Vec<_> = formats(&build(&body, &styles, Some(&numbering)))
        .iter()
        .map(|format| {
            let list = format.list().unwrap();
            (list.instance(), list.level_number())
        })
        .collect();
    assert_eq!(levels, [(1, 2), (2, 0)]);
}

#[test]
fn takes_a_list_levels_indentation_in_character_units_alone() {
    // A level whose w:ind gives only character units gives them to its paragraphs.
    let numbering = r#"<w:abstractNum w:abstractNumId="10"><w:lvl w:ilvl="0"><w:pPr><w:ind w:rightChars="150"/></w:pPr></w:lvl><w:lvl w:ilvl="1"><w:pPr><w:ind w:hangingChars="250"/></w:pPr></w:lvl><w:lvl w:ilvl="2"><w:pPr><w:ind w:leftChars="350"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#;
    let body: String = (0..3)
        .map(|level| {
            paragraph(
                "",
                &format!(r#"<w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="1"/></w:numPr>"#),
            )
        })
        .collect();
    let units: Vec<_> = formats(&build(&body, &defaults(""), Some(numbering)))
        .iter()
        .map(|format| {
            (
                format.start_indent_chars(),
                format.end_indent_chars(),
                format.first_line_indent_chars(),
            )
        })
        .collect();
    assert_eq!(
        units,
        [
            (None, Some(150), None),
            (None, None, Some(-250)),
            (Some(350), None, None),
        ]
    );
}

#[test]
fn a_clear_up_a_style_chain_removes_a_stop_of_the_defaults() {
    // Base clears 64 stops where nothing is set, which fills its bound of cleared stops;
    // Derived then clears the defaults' stop at 7000 twips. Each style is laid over the
    // defaults, so the clear finds that stop.
    let clears: String = (1..=64)
        .map(|n| format!(r#"<w:tab w:val="clear" w:pos="{}"/>"#, n * 10))
        .collect();
    let styles = [
        defaults(r#"<w:tabs><w:tab w:val="left" w:pos="7000"/></w:tabs>"#),
        style(
            "paragraph",
            "Base",
            None,
            &format!("<w:tabs>{clears}</w:tabs>"),
        ),
        style(
            "paragraph",
            "Derived",
            Some("Base"),
            r#"<w:tabs><w:tab w:val="clear" w:pos="7000"/></w:tabs>"#,
        ),
    ]
    .concat();
    let body = [paragraph("Base", ""), paragraph("Derived", "")].concat();
    let formats = formats(&build(&body, &styles, None));
    assert_eq!(tabs(&formats[0]), [tab(TabKind::Left, 350.0)]);
    assert_eq!(tabs(&formats[1]), []);
}

#[test]
fn uses_only_as_many_styles_as_words_format_holds() {
    // Word's binary format holds fewer than 0x0FFE styles ([MS-DOC] 2.9.274): the
    // resolver takes the first 4,093 and leaves the rest out, as the styles of
    // paragraphs, as bases and as the default.
    let mut styles = defaults("");
    for n in 0..4092 {
        styles.push_str(&style("paragraph", &format!("S{n}"), None, ""));
    }
    styles.push_str(&style(
        "paragraph",
        "Last",
        Some("Beyond"),
        r#"<w:jc w:val="right"/>"#,
    ));
    styles.push_str(&style(
        "paragraph",
        "Beyond",
        None,
        r#"<w:jc w:val="center"/><w:ind w:left="720"/>"#,
    ));
    styles.push_str(
        r#"<w:style w:type="paragraph" w:default="1" w:styleId="LateDefault"><w:name w:val="LateDefault"/><w:pPr><w:jc w:val="both"/></w:pPr></w:style>"#,
    );
    let body = [
        paragraph("Last", ""),
        paragraph("Beyond", ""),
        paragraph("", ""),
    ]
    .concat();
    let formats = formats(&build(&body, &styles, None));
    let found: Vec<_> = formats
        .iter()
        .map(|format| {
            (
                format.style(),
                format.justification(),
                format.start_indent(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (Some("Last"), Justification::Right, Pt(0.0)),
            (None, Justification::Start, Pt(0.0)),
            (None, Justification::Start, Pt(0.0)),
        ]
    );
}

#[test]
fn keeps_a_bounded_number_of_style_and_list_level_combinations() {
    // 1,100 styles each with a paragraph in a list whose level gives an indentation:
    // more combinations than the cache keeps. Every paragraph still gets its style's
    // alignment and the level's indentation, and a second pass over them finds the
    // same.
    let numbering = [
        definition(10, ""),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
    ]
    .concat();
    let mut styles = defaults(r#"<w:numPr><w:numId w:val="1"/></w:numPr>"#);
    for n in 0..1100 {
        let alignment = if n % 2 == 0 { "center" } else { "both" };
        styles.push_str(&style(
            "paragraph",
            &format!("S{n}"),
            None,
            &format!(
                r#"<w:jc w:val="{alignment}"/><w:tabs><w:tab w:val="left" w:pos="{}"/></w:tabs>"#,
                2000 + n
            ),
        ));
    }
    let body: String = (0..1100).map(|n| paragraph(&format!("S{n}"), "")).collect();
    let document = build(&body, &styles, Some(&numbering));
    let mut formats = document.formats();
    for pass in 0..2 {
        for (n, paragraph) in paragraphs(&document).into_iter().enumerate() {
            let format = formats.paragraph(paragraph);
            // The list level's alignment wins over the style's.
            assert_eq!(format.justification(), Justification::Right, "pass {pass}");
            assert_eq!(format.start_indent(), Pt(36.0), "pass {pass}");
            assert_eq!(
                tabs(&format),
                [tab(TabKind::Left, (2000.0 + n as f64) / 20.0)],
                "pass {pass}, paragraph {n}"
            );
            assert!(formats.kept_combinations() <= 1024);
        }
    }
    assert_eq!(formats.work().combinations, 2200);
}

#[test]
fn an_override_of_a_level_does_not_change_the_level_for_other_instances() {
    // Instances 1 and 2 use one definition; 2 replaces level 0 with one indented 2 in.
    // Paragraphs of one style in each keep their own level's indentation, in either
    // order.
    let numbering = [
        definition(10, ""),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
        r#"<w:num w:numId="2"><w:abstractNumId w:val="10"/><w:lvlOverride w:ilvl="0"><w:lvl w:ilvl="0"><w:pPr><w:ind w:left="2880"/></w:pPr></w:lvl></w:lvlOverride></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [defaults(""), style("paragraph", "Body", None, "")].concat();
    let item = |id: i64| {
        paragraph(
            "Body",
            &format!(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{id}"/></w:numPr>"#),
        )
    };
    let body = [item(1), item(2), item(1)].concat();
    let indents: Vec<_> = formats(&build(&body, &styles, Some(&numbering)))
        .iter()
        .map(|format| format.start_indent().0)
        .collect();
    assert_eq!(indents, [36.0, 144.0, 36.0]);
}

#[test]
fn a_style_on_a_based_on_cycle_still_lies_over_the_defaults() {
    // A and B are based on each other, which Word does not allow: each is taken with
    // its own properties, over the document defaults.
    let styles = [
        defaults(
            r#"<w:spacing w:after="200"/><w:tabs><w:tab w:val="left" w:pos="7000"/></w:tabs>"#,
        ),
        style("paragraph", "A", Some("B"), r#"<w:jc w:val="center"/>"#),
        style("paragraph", "B", Some("A"), r#"<w:ind w:left="720"/>"#),
    ]
    .concat();
    let body = [paragraph("A", ""), paragraph("B", "")].concat();
    for format in formats(&build(&body, &styles, None)) {
        assert_eq!(format.after_spacing(), Pt(10.0));
        assert_eq!(tabs(&format), [tab(TabKind::Left, 350.0)]);
    }
}

#[test]
fn a_replaced_level_keeps_the_start_and_restart_of_the_definitions() {
    // MS-OI29500 §17.9.25(b), §17.9.10(b): Word ignores the start and restart of an
    // override's own level.
    let numbering = r#"<w:abstractNum w:abstractNumId="10"><w:lvl w:ilvl="0"><w:start w:val="3"/><w:lvlRestart w:val="0"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="10"/><w:lvlOverride w:ilvl="0"><w:lvl w:ilvl="0"><w:start w:val="40"/><w:lvlRestart w:val="1"/><w:lvlText w:val="(%1)"/></w:lvl></w:lvlOverride></w:num>"#;
    let body = paragraph(
        "",
        r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>"#,
    );
    let document = build(&body, &defaults(""), Some(numbering));
    let format = &formats(&document)[0];
    let list = format.list().unwrap();
    let level = list.level(&document).unwrap();
    assert_eq!(level.text(), Some("(%1)"));
    assert_eq!((level.start(), level.restart()), (Some(3), Some(0)));
    assert_eq!(list.start(), 3);
}

#[test]
fn a_numbering_style_naming_instance_0_leads_nowhere() {
    // Instance 0 is no list (ECMA-376 §17.9.18), even when a w:num has that id: a
    // definition linked to a numbering style that names it has no levels.
    let numbering = [
        definition(30, ""),
        r#"<w:num w:numId="0"><w:abstractNumId w:val="30"/></w:num>"#.to_owned(),
        r#"<w:abstractNum w:abstractNumId="20"><w:numStyleLink w:val="Zero"/></w:abstractNum>"#
            .to_owned(),
        r#"<w:num w:numId="4"><w:abstractNumId w:val="20"/></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [
        defaults(""),
        style(
            "numbering",
            "Zero",
            None,
            r#"<w:numPr><w:numId w:val="0"/></w:numPr>"#,
        ),
    ]
    .concat();
    let body = paragraph(
        "",
        r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="4"/></w:numPr>"#,
    );
    let document = build(&body, &styles, Some(&numbering));
    let format = &formats(&document)[0];
    let list = format.list().unwrap();
    assert_eq!((list.definition(), list.level(&document)), (None, None));
    assert_eq!(format.start_indent(), Pt(0.0));
}

#[test]
fn takes_the_first_level_that_names_the_style() {
    // Two levels of one definition name the style in w:pStyle: the lower one is taken,
    // a choice no source settles (pending a rule document).
    let numbering = r#"<w:abstractNum w:abstractNumId="10"><w:lvl w:ilvl="1"><w:pStyle w:val="Item"/></w:lvl><w:lvl w:ilvl="4"><w:pStyle w:val="Item"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#;
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Item",
            None,
            r#"<w:numPr><w:numId w:val="1"/></w:numPr>"#,
        ),
    ]
    .concat();
    let format = &formats(&build(&paragraph("Item", ""), &styles, Some(numbering)))[0];
    assert_eq!(format.list().unwrap().level_number(), 1);
}

#[test]
fn a_hanging_indent_of_the_lowest_number_saturates() {
    // w:hangingChars of i64::MIN has no negative: the indent saturates, without a panic.
    let body = paragraph("", r#"<w:ind w:hangingChars="-9223372036854775808"/>"#);
    let format = &formats(&build(&body, &defaults(""), None))[0];
    assert_eq!(format.first_line_indent_chars(), Some(i64::MAX));
}

#[test]
fn reads_the_paragraph_style_as_the_paragraph_properties_do() {
    // A w:pStyle in mc:AlternateContent counts, through the branch taken (#116), and of
    // two the last wins, as Paragraph::properties reads them.
    let styles = [
        defaults(""),
        style("paragraph", "Centred", None, r#"<w:jc w:val="center"/>"#),
        style("paragraph", "Right", None, r#"<w:jc w:val="right"/>"#),
    ]
    .concat();
    let body = [
        r#"<w:p><w:pPr><mc:AlternateContent><mc:Choice Requires="w99"><w:pStyle w:val="Right"/></mc:Choice><mc:Fallback><w:pStyle w:val="Centred"/></mc:Fallback></mc:AlternateContent></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
        // A choice that requires only w is taken.
        r#"<w:p><w:pPr><mc:AlternateContent><mc:Choice Requires="w"><w:pStyle w:val="Right"/></mc:Choice><mc:Fallback><w:pStyle w:val="Centred"/></mc:Fallback></mc:AlternateContent></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="Centred"/><w:pStyle w:val="Right"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
        // The last w:pStyle names no style: the paragraph has the default style.
        r#"<w:p><w:pPr><w:pStyle w:val="Centred"/><w:pStyle/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
        // Of two w:pPr, the first counts.
        r#"<w:p><w:pPr><w:pStyle w:val="Right"/></w:pPr><w:pPr><w:pStyle w:val="Centred"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
    ]
    .concat();
    let document = build(&body, &styles, None);
    let found: Vec<_> = paragraphs(&document)
        .into_iter()
        .map(|paragraph| {
            let format = paragraph.format();
            assert_eq!(
                format.style(),
                paragraph.properties().style.as_deref(),
                "the same style as the properties"
            );
            (format.style().map(str::to_owned), format.justification())
        })
        .collect();
    assert_eq!(
        found,
        [
            (Some("Centred".to_owned()), Justification::Center),
            (Some("Right".to_owned()), Justification::Right),
            (Some("Right".to_owned()), Justification::Right),
            (None, Justification::Start),
            (Some("Right".to_owned()), Justification::Right),
        ]
    );
}

#[test]
fn reads_the_style_of_each_paragraph_once_for_the_document() {
    // Issue #143: Run::format, and a Formats asked about another paragraph between two
    // runs of one paragraph, walked the paragraph's whole w:pPr for every run. Each
    // paragraph's style is now worked out once, when the document is read.
    let styles = [
        defaults(""),
        style("paragraph", "Centred", None, r#"<w:jc w:val="center"/>"#),
    ]
    .concat();
    let filler = "<w:x/>".repeat(10_000);
    let runs = r#"<w:r><w:t>x</w:t></w:r>"#.repeat(300);
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Centred"/>{filler}</w:pPr>{runs}</w:p><w:p><w:pPr>{filler}<w:pStyle w:val="Centred"/><w:pStyle w:val="Missing"/></w:pPr>{runs}</w:p>"#
    );
    let document = build(&body, &styles, None);
    let [first, second] = paragraphs(&document)[..] else {
        panic!("two paragraphs");
    };
    crate::model::xml::STEPS.set(0);
    let mut formats = document.formats();
    let mut calls = 0;
    for (a, b) in first.runs().zip(second.runs()) {
        // Alternating between the paragraphs, and each run on its own.
        assert_eq!(formats.run(a), a.format());
        assert_eq!(formats.run(b), b.format());
        calls += 4;
    }
    let steps = crate::model::xml::STEPS.get();
    // A walk of either w:pPr per run would take 20,000 steps or more each time; a run's
    // formatting now walks only the few elements of the defaults and the style.
    assert!(steps < calls * 100, "{steps} steps for {calls} calls");
    // The last w:pStyle counts, and one naming no style leaves the default style.
    assert_eq!(formats.paragraph(first).style(), Some("Centred"));
    assert_eq!(formats.paragraph(second).style(), None);
}

#[test]
fn formats_a_list_label_as_the_mark_with_the_level_over_it() {
    // The label takes the paragraph mark's formatting with the level's run properties
    // over it; Word ignores the level's w:rStyle, w:highlight and w:oMath there
    // (MS-OI29500 §17.9.24(a)).
    let level = |ilvl: i64, r_pr: &str| {
        format!(
            r#"<w:lvl w:ilvl="{ilvl}"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="o"/><w:rPr>{r_pr}</w:rPr></w:lvl>"#
        )
    };
    let numbering = [
        format!(
            r#"<w:abstractNum w:abstractNumId="10">{}{}</w:abstractNum>"#,
            level(
                0,
                r#"<w:rStyle w:val="Emphasis"/><w:rFonts w:ascii="Symbol" w:hAnsi="Symbol" w:hint="default"/><w:b/><w:i w:val="0"/><w:highlight w:val="yellow"/><w:oMath/>"#
            ),
            level(1, ""),
        ),
        r#"<w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>"#.to_owned(),
        // An override's replacement level gives the label its own run properties.
        r#"<w:num w:numId="2"><w:abstractNumId w:val="10"/><w:lvlOverride w:ilvl="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:rPr><w:color w:val="00FF00"/></w:rPr></w:lvl></w:lvlOverride></w:num>"#.to_owned(),
    ]
    .concat();
    let styles = [
        defaults(""),
        r#"<w:style w:type="character" w:styleId="Emphasis"><w:name w:val="Emphasis"/><w:rPr><w:u w:val="single"/><w:caps/></w:rPr></w:style>"#.to_owned(),
    ]
    .concat();
    let item = |instance: i64, level: i64| {
        format!(
            r#"<w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{instance}"/></w:numPr><w:rPr><w:i/><w:color w:val="FF0000"/><w:sz w:val="28"/></w:rPr>"#
        )
    };
    let body = [
        paragraph("", &item(1, 0)),
        paragraph("", &item(1, 1)),
        paragraph("", &item(2, 0)),
        paragraph("", r#"<w:rPr><w:sz w:val="28"/></w:rPr>"#),
    ]
    .concat()
    .repeat(50);
    let document = build(&body, &styles, Some(&numbering));
    let mut formats = document.formats();
    let labels: Vec<_> = paragraphs(&document)
        .into_iter()
        .map(|paragraph| formats.list_label(paragraph))
        .collect();
    let red = ColorValue::Rgb(255, 0, 0);

    // Level 0: the level's font and bold, its "not italic" over the mark's italic; the
    // mark's size and colour; no highlight, no maths, and nothing of the character style
    // the level names.
    let first = labels[0].as_ref().unwrap();
    assert_eq!(first.fonts().ascii(), "Symbol");
    assert_eq!((first.bold(), first.italic()), (true, false));
    assert_eq!((first.size(), first.color()), (Pt(14.0), red));
    assert_eq!(first.highlight(), None);
    assert!(!first.math());
    assert_eq!(
        (first.underline(), first.caps(), first.style()),
        (None, false, None)
    );
    // The paragraph's own mark is italic.
    assert!(formats.paragraph_mark(paragraphs(&document)[0]).italic());

    // Level 1 gives nothing: the label is formatted as the mark.
    let second = labels[1].as_ref().unwrap();
    assert_eq!(second, &formats.paragraph_mark(paragraphs(&document)[1]));

    // The replacement level of instance 2: its colour, nothing of the definition's level.
    let third = labels[2].as_ref().unwrap();
    assert_eq!(third.color(), ColorValue::Rgb(0, 255, 0));
    assert_eq!((third.bold(), third.italic()), (false, true));
    // The mark's font: Word's defaults, as these styles give no run properties.
    assert_eq!(third.fonts().ascii(), "Calibri");

    // Not a list item.
    assert_eq!(labels[3], None);
    // The same for every repetition.
    for (n, label) in labels.iter().enumerate() {
        assert_eq!(label, &labels[n % 4]);
    }
    // Each level was read once for 150 list items: two of the definition, and the
    // replacement in instance 2's override.
    assert_eq!(formats.work().level_layers, 2);
    assert_eq!(formats.work().replacements, 1);
}
