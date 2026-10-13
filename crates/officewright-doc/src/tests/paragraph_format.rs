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
    ]
    .concat();
    let body = [
        paragraph("Heading2", ""),
        paragraph("Chapter", ""),
        paragraph("Body", ""),
        paragraph("Chapter", r#"<w:outlineLvl w:val="0"/>"#),
        paragraph("Heading01", ""),
        paragraph("Heading+1", ""),
    ]
    .concat();
    let levels: Vec<_> = formats(&build(&body, &styles, None))
        .iter()
        .map(ParagraphFormat::outline_level)
        .collect();
    // MS-OI29500 §17.3.1.20(a): Heading1 to Heading9 by their ids; 9 is body text.
    assert_eq!(levels, [Some(1), Some(3), None, Some(0), Some(4), None]);
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
    let formats = formats(&build(&body, &styles, Some(&numbering)));
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
        formats[0].list().unwrap().level().unwrap().text(),
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
    let formats = formats(&build(&body, &defaults(""), Some(&numbering)));
    let start_override = formats[0].list().unwrap();
    assert_eq!(start_override.start(), 7);
    assert_eq!(start_override.level().unwrap().text(), Some("%1."));
    // An override's level replaces the definition's, but Word ignores its start
    // (MS-OI29500 §17.9.25(b)).
    let replaced = formats[1].list().unwrap();
    assert_eq!(replaced.level().unwrap().text(), Some("(%2)"));
    // The replacement is level 1, whatever its own w:ilvl says.
    assert_eq!(replaced.level().unwrap().level(), 1);
    assert_eq!(
        replaced.level().unwrap().number_format(),
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
    let formats = formats(&build(&body, &styles, Some(&numbering)));
    let linked = formats[0].list().unwrap();
    assert_eq!((linked.instance(), linked.definition()), (4, Some(30)));
    assert_eq!(linked.level().unwrap().text(), Some("%2."));
    assert_eq!(formats[0].start_indent(), Pt(72.0));
    for format in &formats[1..] {
        let list = format.list().unwrap();
        assert_eq!((list.definition(), list.level()), (None, None));
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
    // once, and only overridden ones are copied.
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
    let formats = formats(&build(&body, &defaults(""), Some(&numbering)));
    let level = |format: &ParagraphFormat| format.list().unwrap().level.clone().unwrap();
    let first = level(&formats[0]);
    assert!(
        formats[..2000]
            .iter()
            .all(|format| Arc::ptr_eq(&level(format), &first))
    );
    let overridden = level(&formats[2000]);
    assert!(!Arc::ptr_eq(&overridden, &first));
    assert_eq!(overridden.text(), Some("*"));
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
