//! The formatting in effect: [`Formats`] and [`Run::format`].

use super::*;

/// A document whose body is `body`, with a styles part holding `styles` (the children
/// of `w:styles`), and a settings part and a theme when given.
fn build(body: &str, styles: &str, settings: Option<&str>, theme: Option<&str>) -> Document {
    let rel = |id: &str, kind: &str, target: &str| {
        format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
        )
    };
    let mut rels = rel("rId1", "styles", "styles.xml");
    let styles = format!(r#"<w:styles xmlns:w="{W}">{styles}</w:styles>"#);
    let mut parts = vec![("word/styles.xml".to_owned(), styles)];
    if let Some(settings) = settings {
        rels.push_str(&rel("rId2", "settings", "settings.xml"));
        parts.push((
            "word/settings.xml".to_owned(),
            format!(r#"<w:settings xmlns:w="{W}">{settings}</w:settings>"#),
        ));
    }
    if let Some(theme) = theme {
        rels.push_str(&rel("rId3", "theme", "theme/theme1.xml"));
        parts.push(("word/theme/theme1.xml".to_owned(), theme.to_owned()));
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

/// `w:docDefaults` with these run properties.
fn defaults(r_pr: &str) -> String {
    format!("<w:docDefaults><w:rPrDefault><w:rPr>{r_pr}</w:rPr></w:rPrDefault></w:docDefaults>")
}

/// A style of `kind` with these run properties and, after them, `rest`.
fn style(kind: &str, id: &str, based_on: Option<&str>, r_pr: &str) -> String {
    let based_on = based_on.map_or_else(String::new, |id| format!(r#"<w:basedOn w:val="{id}"/>"#));
    format!(
        r#"<w:style w:type="{kind}" w:styleId="{id}"><w:name w:val="{id}"/>{based_on}<w:rPr>{r_pr}</w:rPr></w:style>"#
    )
}

/// The paragraphs of the body, in order.
fn paragraphs(document: &Document) -> Vec<Paragraph<'_>> {
    document
        .sections()
        .flat_map(|section| section.blocks())
        .filter_map(|block| match block {
            Block::Paragraph(paragraph) => Some(paragraph),
            _ => None,
        })
        .collect()
}

/// The formatting of every run of the body's paragraphs, in order.
fn runs(document: &Document) -> Vec<CharacterFormat> {
    let mut formats = document.formats();
    paragraphs(document)
        .into_iter()
        .flat_map(|paragraph| paragraph.runs())
        .map(|run| formats.run(run))
        .collect()
}

/// A paragraph with the paragraph style `style` (none when empty) and one run per
/// entry of `runs`, each with these run properties.
fn paragraph(style: &str, runs: &[&str]) -> String {
    let p_pr = if style.is_empty() {
        String::new()
    } else {
        format!(r#"<w:pPr><w:pStyle w:val="{style}"/></w:pPr>"#)
    };
    let runs: String = runs
        .iter()
        .map(|r_pr| format!("<w:r><w:rPr>{r_pr}</w:rPr><w:t>x</w:t></w:r>"))
        .collect();
    format!("<w:p>{p_pr}{runs}</w:p>")
}

fn rgb(hex: &str) -> ColorValue {
    let [_, red, green, blue] = u32::from_str_radix(hex, 16).unwrap().to_be_bytes();
    ColorValue::Rgb(red, green, blue)
}

const THEME_FONT_LANG: &str =
    r#"<w:themeFontLang w:val="en-US" w:eastAsia="ja-JP" w:bidi="ar-SA"/>"#;

/// A theme with distinct colours and fonts: the East Asian font of the major fonts and
/// the complex script font of the minor fonts are left empty, as in Office's theme.
fn theme() -> String {
    format!(
        r#"<a:theme xmlns:a="{A}" name="T"><a:themeElements><a:clrScheme name="C"><a:dk1><a:sysClr val="windowText" lastClr="101010"/></a:dk1><a:lt1><a:srgbClr val="FEFEFE"/></a:lt1><a:dk2><a:srgbClr val="202020"/></a:dk2><a:lt2><a:srgbClr val="E0E0E0"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2><a:accent3><a:srgbClr val="030303"/></a:accent3><a:accent4><a:srgbClr val="040404"/></a:accent4><a:accent5><a:srgbClr val="050505"/></a:accent5><a:accent6><a:srgbClr val="060606"/></a:accent6><a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink></a:clrScheme><a:fontScheme name="F"><a:majorFont><a:latin typeface="Major Latin"/><a:ea typeface=""/><a:cs typeface="Major CS"/><a:font script="Jpan" typeface="Major Jpan"/><a:font script="Arab" typeface="Major Arab"/></a:majorFont><a:minorFont><a:latin typeface="Minor Latin"/><a:ea typeface="Minor EA"/><a:cs typeface=""/><a:font script="Jpan" typeface="Minor Jpan"/><a:font script="Arab" typeface="Minor Arab"/></a:minorFont></a:fontScheme><a:fmtScheme name="S"/></a:themeElements></a:theme>"#
    )
}

#[test]
fn a_run_without_styles_takes_words_defaults() {
    let document = open("<w:p><w:r><w:t>x</w:t></w:r></w:p>");
    let format = first_run(&document).format();
    let fonts = format.fonts();
    for font in [
        fonts.ascii(),
        fonts.high_ansi(),
        fonts.east_asia(),
        fonts.complex_script(),
    ] {
        assert_eq!(font, "Times New Roman");
    }
    assert_eq!(fonts.hint(), None);
    assert_eq!(format.size(), Pt(10.0));
    assert_eq!(format.size_complex_script(), Pt(10.0));
    assert!(!format.bold() && !format.italic() && !format.hidden() && !format.math());
    assert_eq!(format.color(), ColorValue::Auto);
    assert_eq!(format.underline(), None);
    assert_eq!(format.underline_color(), ColorValue::Auto);
    assert_eq!(format.vertical_position(), VerticalPosition::Baseline);
    assert_eq!(format.position(), Pt(0.0));
    assert_eq!(format.spacing(), Pt(0.0));
    assert_eq!(format.scale(), 100);
    assert_eq!(format.kerning(), None);
    assert_eq!(format.highlight(), None);
    assert_eq!(format.shading(), None);
    assert_eq!(format.language(), None);
    assert_eq!(format.style(), None);
}

#[test]
fn combines_defaults_paragraph_style_character_style_and_direct_formatting() {
    let styles = [
        defaults(
            r#"<w:rFonts w:ascii="Arial"/><w:sz w:val="20"/><w:color w:val="111111"/><w:lang w:val="en-US" w:eastAsia="ja-JP" w:bidi="ar-SA"/><w:kern w:val="2"/><w:w w:val="90"/>"#,
        ),
        r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:rPr><w:sz w:val="22"/></w:rPr></w:style>"#.to_owned(),
        style("paragraph", "Base", None, r#"<w:sz w:val="24"/><w:color w:val="222222"/>"#),
        style("paragraph", "Heading", Some("Base"), r#"<w:sz w:val="28"/>"#),
        style("character", "Emph", None, r#"<w:sz w:val="30"/><w:color w:val="333333"/>"#),
        style("character", "EmphStrong", Some("Emph"), r#"<w:sz w:val="32"/>"#),
    ]
    .concat();
    let body = [
        paragraph("", &[""]),
        paragraph(
            "Heading",
            &[
                "",
                r#"<w:rStyle w:val="EmphStrong"/>"#,
                r#"<w:rStyle w:val="EmphStrong"/><w:sz w:val="40"/><w:lang w:val="de-DE"/>"#,
            ],
        ),
        // A character style named as a paragraph style, a paragraph style named as a
        // character style and a missing style are not applied: the paragraph takes the
        // default paragraph style.
        paragraph("EmphStrong", &[r#"<w:rStyle w:val="Heading"/>"#]),
        paragraph("Missing", &[r#"<w:rStyle w:val="Missing"/>"#]),
    ]
    .concat();
    let document = build(&body, &styles, None, None);
    let formats = runs(&document);
    let summary: Vec<_> = formats
        .iter()
        .map(|format| (format.size().0, format.color(), format.style()))
        .collect();
    assert_eq!(
        summary,
        [
            (11.0, rgb("111111"), None),
            (14.0, rgb("222222"), None),
            (16.0, rgb("333333"), Some("EmphStrong")),
            (20.0, rgb("333333"), Some("EmphStrong")),
            (11.0, rgb("111111"), None),
            (11.0, rgb("111111"), None),
        ]
    );
    // What only the document defaults set reaches every run.
    for format in &formats {
        assert_eq!(format.fonts().ascii(), "Arial");
        assert_eq!(format.fonts().high_ansi(), "Times New Roman");
        assert_eq!(format.scale(), 90);
        assert_eq!(format.kerning(), Some(Pt(1.0)));
        assert_eq!(format.language_east_asia(), Some("ja-JP"));
        assert_eq!(format.language_bidi(), Some("ar-SA"));
    }
    let languages: Vec<_> = formats.iter().map(CharacterFormat::language).collect();
    assert_eq!(languages[2], Some("en-US"));
    assert_eq!(languages[3], Some("de-DE"));
}

/// Bold of each run of `body`, with the toggle styles of `word-rules/E-style-toggles`.
fn bold(defaults_r_pr: &str, body: &str) -> Vec<bool> {
    let styles = [
        defaults(defaults_r_pr),
        style("paragraph", "ParaBold", None, "<w:b/>"),
        style("paragraph", "ParaBoldAgain", Some("ParaBold"), "<w:b/>"),
        style(
            "paragraph",
            "ParaBoldOff",
            Some("ParaBold"),
            r#"<w:b w:val="0"/>"#,
        ),
        style("character", "CharBold", None, "<w:b/>"),
        style("character", "CharBoldAgain", Some("CharBold"), "<w:b/>"),
        style("character", "CharBoldOff", None, r#"<w:b w:val="0"/>"#),
        style("paragraph", "ParaBoldInherit", Some("ParaBold"), ""),
        style("character", "CharBoldInherit", Some("CharBold"), ""),
    ]
    .concat();
    runs(&build(body, &styles, None, None))
        .iter()
        .map(CharacterFormat::bold)
        .collect()
}

const CHAR_BOLD: &str = r#"<w:rStyle w:val="CharBold"/>"#;

#[test]
fn a_paragraph_style_sets_a_toggle_property_and_a_character_style_toggles_it() {
    let body = [
        paragraph("ParaBold", &[""]),
        // Word sets the value of a paragraph style rather than toggling it
        // (MS-OI29500 §17.7.8(a)).
        paragraph("ParaBoldAgain", &[""]),
        paragraph("ParaBoldOff", &[""]),
        paragraph("", &[CHAR_BOLD]),
        paragraph("", &[r#"<w:rStyle w:val="CharBoldAgain"/>"#]),
        paragraph("ParaBold", &[CHAR_BOLD]),
        paragraph("ParaBold", &[r#"<w:rStyle w:val="CharBoldOff"/>"#]),
        paragraph("ParaBold", &[&format!("{CHAR_BOLD}<w:b/>")]),
        paragraph("ParaBold", &[r#"<w:b w:val="0"/>"#]),
        paragraph("", &["<w:b/>"]),
        // Styles based on bold ones without a w:b of their own (ECMA-376 §17.7.3).
        paragraph("ParaBoldInherit", &[""]),
        paragraph("", &[r#"<w:rStyle w:val="CharBoldInherit"/>"#]),
        paragraph("ParaBold", &[r#"<w:rStyle w:val="CharBoldInherit"/>"#]),
    ]
    .concat();
    let expected = [
        true, true, false, true, false, false, true, true, false, true, true, true, false,
    ];
    assert_eq!(bold("", &body), expected);
}

#[test]
fn starts_toggle_properties_from_the_document_defaults() {
    // Provisional until the reference render of word-rules/E-style-toggles-defaults.docx:
    // the document default is the value a character style toggles.
    let body = [
        paragraph("", &[""]),
        paragraph("", &[CHAR_BOLD]),
        paragraph("ParaBoldOff", &[""]),
        paragraph("ParaBoldOff", &[CHAR_BOLD]),
    ]
    .concat();
    assert_eq!(bold("<w:b/>", &body), [true, false, false, true]);
}

#[test]
fn applies_words_rules_for_other_on_off_properties() {
    let styles = [
        defaults("<w:dstrike/><w:noProof/>"),
        style(
            "paragraph",
            "NoStrike",
            None,
            r#"<w:dstrike w:val="0"/><w:webHidden/>"#,
        ),
        style(
            "character",
            "Strike",
            None,
            "<w:dstrike/><w:webHidden/><w:specVanish/>",
        ),
    ]
    .concat();
    let body = [
        paragraph("", &[""]),
        paragraph("NoStrike", &[""]),
        // Not a toggle property: a character style sets it again rather than toggling.
        paragraph("", &[r#"<w:rStyle w:val="Strike"/>"#]),
        paragraph("NoStrike", &[r#"<w:rStyle w:val="Strike"/><w:vanish/>"#]),
    ]
    .concat();
    let formats = runs(&build(&body, &styles, None, None));
    let flags: Vec<_> = formats
        .iter()
        .map(|format| {
            (
                format.double_strike(),
                format.web_hidden(),
                format.spec_vanish(),
                format.no_proof(),
            )
        })
        .collect();
    assert_eq!(
        flags,
        [
            (true, false, false, true),
            (false, true, false, true),
            // Word ignores webHidden in a character style (MS-OI29500 §17.3.2.44(a)),
            // and specVanish unless the text is hidden (§17.3.2.36(a)).
            (true, false, false, true),
            (true, true, true, true),
        ]
    );
}

#[test]
fn resolves_theme_fonts_through_the_theme_and_the_theme_font_languages() {
    let styles = [
        defaults(
            r#"<w:rFonts w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi" w:eastAsiaTheme="minorEastAsia" w:cstheme="minorBidi"/>"#,
        ),
        style(
            "paragraph",
            "Heading",
            None,
            r#"<w:rFonts w:asciiTheme="majorHAnsi" w:hAnsiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:cstheme="majorBidi"/>"#,
        ),
    ]
    .concat();
    let body = [
        paragraph("", &[""]),
        paragraph(
            "Heading",
            &[
                "",
                // A font written at a later level replaces a theme font of an earlier
                // one, and the other way round (MS-OI29500 §17.3.2.26(e)).
                r#"<w:rFonts w:ascii="Direct"/>"#,
                // In one w:rFonts the theme font wins.
                r#"<w:rFonts w:ascii="Named" w:asciiTheme="minorAscii" w:hint="eastAsia"/>"#,
            ],
        ),
    ]
    .concat();
    let document = build(&body, &styles, Some(THEME_FONT_LANG), Some(&theme()));
    let fonts: Vec<_> = runs(&document)
        .iter()
        .map(|format| {
            let fonts = format.fonts();
            [
                fonts.ascii(),
                fonts.high_ansi(),
                fonts.east_asia(),
                fonts.complex_script(),
            ]
            .map(str::to_owned)
        })
        .collect();
    assert_eq!(
        fonts,
        [
            // The minor complex script font is empty: the theme's font for Arabic, the
            // script of the theme font language ar-SA, stands in.
            ["Minor Latin", "Minor Latin", "Minor EA", "Minor Arab"],
            ["Major Latin", "Major Latin", "Major Jpan", "Major CS"],
            ["Direct", "Major Latin", "Major Jpan", "Major CS"],
            ["Minor Latin", "Major Latin", "Major Jpan", "Major CS"],
        ]
    );
    let hints: Vec<_> = runs(&document)
        .iter()
        .map(|format| format.fonts().hint())
        .collect();
    assert_eq!(hints, [None, None, None, Some(FontHint::EastAsia)]);
}

#[test]
fn falls_back_to_the_written_font_when_a_theme_font_cannot_be_resolved() {
    let styles = defaults(
        r#"<w:rFonts w:asciiTheme="minorHAnsi" w:ascii="Calibri" w:hAnsiTheme="minorHAnsi" w:eastAsiaTheme="minorEastAsia" w:eastAsia="MS Mincho"/>"#,
    );
    let body = paragraph("", &[""]);
    // No theme: the font written next to the theme font, else Word's default.
    let document = build(&body, &styles, Some(THEME_FONT_LANG), None);
    let format = first_run(&document).format();
    assert_eq!(format.fonts().ascii(), "Calibri");
    assert_eq!(format.fonts().high_ansi(), "Times New Roman");
    // A theme, but no theme font language for the empty East Asian slot.
    let theme = theme().replace(r#"<a:ea typeface="Minor EA"/>"#, r#"<a:ea typeface=""/>"#);
    let document = build(&body, &styles, None, Some(&theme));
    let format = first_run(&document).format();
    assert_eq!(format.fonts().ascii(), "Minor Latin");
    assert_eq!(format.fonts().east_asia(), "MS Mincho");
}

#[test]
fn resolves_theme_colours_with_tints_and_shades() {
    let colors = [
        r#"<w:color w:val="123456"/>"#,
        r#"<w:color w:val="auto"/>"#,
        // The examples of MS-OI29500 §17.3.2.6: Word writes 95B3D7 and 943634.
        r#"<w:color w:val="000000" w:themeColor="accent1" w:themeTint="99"/>"#,
        r#"<w:color w:val="000000" w:themeColor="accent2" w:themeShade="BF"/>"#,
        // The tint wins over the shade (MS-OI29500 §17.3.2.6(b)).
        r#"<w:color w:themeColor="accent1" w:themeTint="99" w:themeShade="BF"/>"#,
        // Text 1 is dark 1 when the settings map nothing.
        r#"<w:color w:val="000000" w:themeColor="text1"/>"#,
        r#"<w:color w:val="654321" w:themeColor="none"/>"#,
        r#"<w:color w:themeColor="followedHyperlink" w:themeTint="FF"/>"#,
        r#"<w:color w:themeColor="background2" w:themeShade="00"/>"#,
    ];
    let body = paragraph("", &colors);
    let document = build(&body, "", None, Some(&theme()));
    let values: Vec<_> = runs(&document).iter().map(CharacterFormat::color).collect();
    assert_eq!(
        values,
        [
            rgb("123456"),
            ColorValue::Auto,
            rgb("95B3D7"),
            rgb("943734"),
            rgb("95B3D7"),
            rgb("101010"),
            rgb("654321"),
            rgb("800080"),
            rgb("000000"),
        ]
    );

    // The settings map text 1 to light 1, as a dark theme does.
    let mapping = r#"<w:clrSchemeMapping w:bg1="dark1" w:t1="light1" w:bg2="dark2" w:t2="light2" w:accent1="accent2" w:accent2="accent1" w:accent3="accent3" w:accent4="accent4" w:accent5="accent5" w:accent6="accent6" w:hyperlink="hyperlink" w:followedHyperlink="followedHyperlink"/>"#;
    let body = paragraph(
        "",
        &[
            r#"<w:color w:themeColor="text1"/>"#,
            r#"<w:color w:themeColor="background1"/>"#,
            r#"<w:color w:themeColor="text2"/>"#,
            r#"<w:color w:themeColor="accent1"/>"#,
            // Dark 1 is the scheme's own colour, not mapped.
            r#"<w:color w:themeColor="dark1"/>"#,
        ],
    );
    let document = build(&body, "", Some(mapping), Some(&theme()));
    let values: Vec<_> = runs(&document).iter().map(CharacterFormat::color).collect();
    assert_eq!(
        values,
        [
            rgb("FEFEFE"),
            rgb("101010"),
            rgb("E0E0E0"),
            rgb("C0504D"),
            rgb("101010"),
        ]
    );

    // Without a theme the value written stands.
    let body = paragraph("", &[r#"<w:color w:val="1F3864" w:themeColor="accent1"/>"#]);
    let document = build(&body, "", None, None);
    assert_eq!(first_run(&document).format().color(), rgb("1F3864"));
}

#[test]
fn takes_the_style_and_colour_of_an_underline_from_the_hierarchy_one_by_one() {
    let styles = [
        defaults(""),
        style(
            "paragraph",
            "Underlined",
            None,
            r#"<w:u w:val="double" w:color="FF0000"/>"#,
        ),
    ]
    .concat();
    let body = [
        paragraph(
            "Underlined",
            &[
                "",
                r#"<w:u w:color="00FF00"/>"#,
                r#"<w:u w:val="dotted"/>"#,
                r#"<w:u w:val="none"/>"#,
                r#"<w:u w:themeColor="accent1"/>"#,
            ],
        ),
        paragraph("", &[r#"<w:u w:val="single"/>"#]),
    ]
    .concat();
    let document = build(&body, &styles, None, Some(&theme()));
    let underlines: Vec<_> = runs(&document)
        .iter()
        .map(|format| (format.underline(), format.underline_color()))
        .collect();
    assert_eq!(
        underlines,
        [
            (Some(UnderlineStyle::Double), rgb("FF0000")),
            (Some(UnderlineStyle::Double), rgb("00FF00")),
            (Some(UnderlineStyle::Dotted), rgb("FF0000")),
            (None, rgb("FF0000")),
            (Some(UnderlineStyle::Double), rgb("4F81BD")),
            // Without a colour anywhere the underline's colour is automatic
            // (MS-OI29500 §17.3.2.40(a)).
            (Some(UnderlineStyle::Single), ColorValue::Auto),
        ]
    );
}

#[test]
fn resolves_the_other_run_properties() {
    let styles = [
        defaults(
            r#"<w:kern w:val="28"/><w:highlight w:val="yellow"/><w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/><w:vertAlign w:val="superscript"/><w:position w:val="4"/><w:spacing w:val="10"/><w:szCs w:val="30"/>"#,
        ),
        style(
            "character",
            "Plain",
            None,
            r#"<w:kern w:val="0"/><w:highlight w:val="none"/><w:vertAlign w:val="baseline"/><w:position w:val="-6"/><w:spacing w:val="-20"/>"#,
        ),
    ]
    .concat();
    let body = paragraph("", &["", r#"<w:rStyle w:val="Plain"/>"#]);
    let formats = runs(&build(&body, &styles, None, None));
    let [first, second] = &formats[..] else {
        panic!("two runs");
    };
    assert_eq!(first.kerning(), Some(Pt(14.0)));
    assert_eq!(first.highlight(), Some(Highlight::Yellow));
    assert_eq!(first.shading().unwrap().fill().value(), Some(rgb("D9E2F3")));
    assert_eq!(first.vertical_position(), VerticalPosition::Superscript);
    assert_eq!((first.position(), first.spacing()), (Pt(2.0), Pt(0.5)));
    assert_eq!(first.size_complex_script(), Pt(15.0));
    assert_eq!(first.size(), Pt(10.0));
    // A kerning size of 0 and the highlight none switch them off.
    assert_eq!(second.kerning(), None);
    assert_eq!(second.highlight(), None);
    assert!(second.shading().is_some());
    assert_eq!(second.vertical_position(), VerticalPosition::Baseline);
    assert_eq!((second.position(), second.spacing()), (Pt(-3.0), Pt(-1.0)));
}

#[test]
fn resolves_the_paragraph_mark_on_its_own() {
    let styles = [
        defaults(""),
        style("paragraph", "Big", None, r#"<w:sz w:val="32"/>"#),
        style("character", "Red", None, r#"<w:color w:val="FF0000"/>"#),
    ]
    .concat();
    let body = r#"<w:p><w:pPr><w:pStyle w:val="Big"/><w:rPr><w:rStyle w:val="Red"/><w:b/><w:sz w:val="40"/></w:rPr></w:pPr><w:r><w:t>x</w:t></w:r></w:p><w:p/>"#;
    let document = build(body, &styles, None, None);
    let mut formats = document.formats();
    let [first, second] = paragraphs(&document)[..] else {
        panic!("two paragraphs");
    };
    let mark = formats.paragraph_mark(first);
    assert!(mark.bold());
    assert_eq!(mark.size(), Pt(20.0));
    assert_eq!(mark.color(), rgb("FF0000"));
    assert_eq!(mark.style(), Some("Red"));
    // The run is formatted by the paragraph's style, not by its mark.
    let run = formats.run(first.runs().next().unwrap());
    assert!(!run.bold());
    assert_eq!(run.size(), Pt(16.0));
    assert_eq!(formats.paragraph_mark(second).size(), Pt(10.0));
}

#[test]
fn stops_at_cycles_and_ignores_the_styles_word_ignores() {
    let styles = [
        defaults(""),
        style("paragraph", "A", Some("B"), r#"<w:sz w:val="30"/>"#),
        style("paragraph", "B", Some("A"), r#"<w:sz w:val="40"/><w:i/>"#),
        style("paragraph", "Self", Some("Self"), r#"<w:sz w:val="50"/>"#),
        // Word ignores what DefaultParagraphFont sets (MS-OI29500 §17.7.4.17(a)).
        style("character", "DefaultParagraphFont", None, "<w:b/>"),
        style("character", "Based", Some("DefaultParagraphFont"), "<w:i/>"),
    ]
    .concat();
    let body = [
        paragraph("A", &[""]),
        paragraph("Self", &[""]),
        paragraph(
            "",
            &[
                r#"<w:rStyle w:val="DefaultParagraphFont"/>"#,
                r#"<w:rStyle w:val="Based"/>"#,
            ],
        ),
    ]
    .concat();
    let formats = runs(&build(&body, &styles, None, None));
    let summary: Vec<_> = formats
        .iter()
        .map(|format| (format.size().0, format.bold(), format.italic()))
        .collect();
    assert_eq!(
        summary,
        [
            (15.0, false, true),
            (25.0, false, false),
            (10.0, false, false),
            (10.0, false, true),
        ]
    );
}

#[test]
fn follows_a_long_chain_of_styles() {
    let mut styles = defaults("");
    styles.push_str(&style("paragraph", "S0", None, "<w:b/>"));
    for i in 1..2000 {
        let previous = format!("S{}", i - 1);
        styles.push_str(&style("paragraph", &format!("S{i}"), Some(&previous), ""));
    }
    styles.push_str(&style(
        "paragraph",
        "Leaf",
        Some("S9"),
        r#"<w:sz w:val="30"/>"#,
    ));
    let body = [paragraph("Leaf", &[""]), paragraph("S1999", &[""])].concat();
    let formats = runs(&build(&body, &styles, None, None));
    assert!(formats[0].bold());
    assert_eq!(formats[0].size(), Pt(15.0));
    // A chain longer than the limit is cut at its root.
    assert!(!formats[1].bold());
}

#[test]
fn finds_the_paragraph_style_of_runs_inside_inline_containers() {
    let styles = [
        defaults(""),
        style("paragraph", "Big", None, r#"<w:sz w:val="32"/>"#),
    ]
    .concat();
    let body = r#"<w:p><w:pPr><w:pStyle w:val="Big"/></w:pPr><w:hyperlink w:anchor="a"><w:r><w:t>a</w:t></w:r></w:hyperlink><w:smartTag w:element="x"><w:r><w:t>b</w:t></w:r></w:smartTag><w:sdt><w:sdtContent><w:ins w:author="A"><w:r><w:t>c</w:t></w:r></w:ins></w:sdtContent></w:sdt></w:p>"#;
    let sizes: Vec<_> = runs(&build(body, &styles, None, None))
        .iter()
        .map(|format| format.size().0)
        .collect();
    assert_eq!(sizes, [16.0, 16.0, 16.0]);
}

#[test]
fn a_run_of_another_document_takes_the_styles_of_its_own() {
    let big = [
        defaults(r#"<w:sz w:val="40"/>"#),
        style("paragraph", "P", None, "<w:b/>"),
    ]
    .concat();
    let small = [
        defaults(r#"<w:sz w:val="16"/>"#),
        style("paragraph", "P", None, "<w:i/>"),
    ]
    .concat();
    let body = paragraph("P", &[""]);
    let first = build(&body, &big, None, None);
    let second = build(&body, &small, None, None);
    let mut formats = first.formats();
    let format = formats.run(first_run(&second));
    assert_eq!(format.size(), Pt(8.0));
    assert!(format.italic() && !format.bold());
    assert_eq!(format, first_run(&second).format());
    let mark = formats.paragraph_mark(paragraphs(&second)[0]);
    assert_eq!(mark.size(), Pt(8.0));
}

#[test]
fn formats_can_be_sent_and_shared_between_threads() {
    fn send_and_share<T: Send + Sync>() {}
    send_and_share::<Formats<'static>>();
    send_and_share::<CharacterFormat>();
}

#[test]
fn a_style_is_not_based_on_a_style_of_another_kind() {
    // ECMA-376 Part 1, §17.7.4.3: such a w:basedOn is ignored.
    let styles = [
        defaults(""),
        style("character", "CharBold", None, "<w:b/>"),
        style("paragraph", "OnCharacter", Some("CharBold"), "<w:i/>"),
        style("paragraph", "ParaBold", None, "<w:b/>"),
        style("character", "OnParagraph", Some("ParaBold"), "<w:i/>"),
    ]
    .concat();
    let body = [
        paragraph("OnCharacter", &[""]),
        paragraph("", &[r#"<w:rStyle w:val="OnParagraph"/>"#]),
    ]
    .concat();
    let summary: Vec<_> = runs(&build(&body, &styles, None, None))
        .iter()
        .map(|format| (format.bold(), format.italic()))
        .collect();
    assert_eq!(summary, [(false, true), (false, true)]);
}
