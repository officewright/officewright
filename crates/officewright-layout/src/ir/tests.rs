//! Tests of the layout IR: validation, clamps and the accessors the macro generates.

use officewright_core::{Color, PageSize, Pt};

use super::*;
use crate::{Error, ReportKind};

/// A document with one run style and one section of A4 pages holding `blocks`.
fn document_with(blocks: impl FnOnce(RunStyleId) -> Vec<Block>) -> Document {
    let mut document = Document::default();
    let style = document.add_run_style(RunStyle::default());
    let blocks = blocks(style);
    document.add_section(Section::new(PageSetup::new(PageSize::A4)).with_blocks(blocks));
    document
}

fn paragraph_with(style: RunStyleId, props: ParagraphProps) -> Vec<Block> {
    vec![
        Paragraph::new(style)
            .with_props(props)
            .with_inlines([Inline::text("text", style)])
            .into(),
    ]
}

fn invalid(document: &Document) -> String {
    match document.validate() {
        Err(err @ Error::InvalidDocument { .. }) => err.to_string(),
        other => panic!("not an invalid document: {other:?}"),
    }
}

/// The location and message of every entry, as `validate` and `clamp` tests compare them.
fn entries(report: &crate::LayoutReport) -> Vec<String> {
    report
        .iter()
        .map(|entry| {
            assert_eq!(entry.kind(), ReportKind::ValueClamped);
            entry.to_string()
        })
        .collect()
}

/// A document that sets every kind of property this IR has, each within Word's limits.
fn rich_document() -> Document {
    let settings = DocumentSettings::default()
        .with_compat_mode(CompatMode::Word2013)
        .with_compat_flags([
            CompatFlag::NoLeading,
            CompatFlag::from_name("usePrinterMetrics"),
        ])
        .with_default_tab_stop(Pt(35.4))
        .with_decimal_separator(',')
        .with_even_and_odd_headers(true)
        .with_hyphenation(
            Hyphenation::default()
                .with_auto(true)
                .with_consecutive_limit(2),
        );
    let mut document = Document::new(settings);
    let body = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Carlito").with_hint(FontHint::EastAsia))
            .with_size(Pt(11.0))
            .with_bold(true)
            .with_color(ColorValue::Rgb(Color::rgb(0x1F, 0x38, 0x64)))
            .with_underline(Some(Underline::default().with_kind(UnderlineKind::Words)))
            .with_letter_spacing(Pt(-0.5))
            .with_scale(90)
            .with_kern_min(Some(Pt(14.0)))
            .with_ligatures(Ligatures::default().with_standard(true))
            .with_highlight(Some(Color::rgb(255, 255, 0)))
            .with_shading(Some(
                Shading::default().with_fill(ColorValue::Rgb(Color::WHITE)),
            ))
            .with_language(Language::default().with_latin(Some("en-US".into()))),
    );
    let props = ParagraphProps::default()
        .with_alignment(Alignment::Both)
        .with_indent(
            Indent::default()
                .with_start(Pt(36.0))
                .with_first_line(Pt(-18.0)),
        )
        .with_spacing(
            Spacing::default()
                .with_after(Pt(8.0))
                .with_line(LineSpacing::Multiple(1.08)),
        )
        .with_widow_control(true)
        .with_tabs([TabStop::default()
            .with_position(Pt(450.0))
            .with_kind(TabKind::End)
            .with_leader(TabLeader::Dot)])
        .with_borders(
            ParagraphBorders::default().with_bottom(Some(
                Border::default()
                    .with_style(BorderStyle::Double)
                    .with_width(Pt(1.5)),
            )),
        )
        .with_style_key(StyleKey::new(3));
    let page_number = Field::new(FieldKind::Page, body)
        .with_format(Some(NumberFormat::UpperRoman))
        .with_result([Inline::text("IV", body)]);
    let paragraph = Paragraph::new(body).with_props(props).with_inlines([
        Inline::text("Total", body),
        Inline::tab(body),
        Inline::text("12,50", body),
        Inline::line_break(BreakClear::None, body),
        Inline::column_break(body),
        Inline::symbol("Wingdings", 0xF0FC, body),
        Inline::soft_hyphen(body),
        Inline::no_break_hyphen(body),
        page_number.into(),
    ]);
    let footer = Paragraph::new(body).with_inlines([Field::new(FieldKind::NumPages, body).into()]);
    let section =
        Section::new(PageSetup::new(PageSize::LETTER).with_orientation(Orientation::Portrait))
            .with_columns(Columns::default().with_count(2).with_separator(true))
            .with_start(SectionStart::OddPage)
            .with_footers(HeaderFooterSet::default().with_primary([footer.into()]))
            .with_title_page(true)
            .with_page_numbering(PageNumbering::default().with_start(Some(1)))
            .with_blocks([paragraph.into()]);
    document.add_section(section);
    document
}

#[test]
fn a_document_within_every_rule_validates_and_needs_no_clamp() {
    let mut document = rich_document();
    assert!(document.validate().is_ok());
    let report = document.clamp();
    assert!(report.is_empty(), "{:?}", entries(&report));
}

#[test]
fn a_document_needs_a_section() {
    assert_eq!(invalid(&Document::default()), "document: has no section");
}

#[test]
fn a_length_that_is_not_finite_is_rejected_where_it_is() {
    let tab = TabStop::default().with_position(Pt(f64::NAN));
    let mut document = document_with(|_| Vec::new());
    let style = document.add_run_style(RunStyle::default());
    let header = Paragraph::new(style).with_props(ParagraphProps::default().with_tabs([tab]));
    let section = Section::new(PageSetup::new(PageSize::A4))
        .with_headers(HeaderFooterSet::default().with_first([header.into()]));
    document.add_section(section);
    assert_eq!(
        invalid(&document),
        "sections[1].headers.first[0].props.tabs[0].position: NaN is not a finite length"
    );
}

#[test]
fn every_kind_of_length_is_checked_for_being_finite() {
    let infinite = Pt(f64::INFINITY);
    let page = PageSetup::new(PageSize::new(infinite, Pt(800.0)));
    let mut document = Document::default();
    document.add_section(Section::new(page));
    assert_eq!(
        invalid(&document),
        "sections[0].page.size.width: inf is not a finite length"
    );

    let document = document_with(|style| {
        paragraph_with(
            style,
            ParagraphProps::default()
                .with_spacing(Spacing::default().with_line(LineSpacing::Exact(Pt(f64::NAN)))),
        )
    });
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].props.spacing.line: NaN is not a finite length"
    );

    let document = document_with(|style| {
        paragraph_with(
            style,
            ParagraphProps::default().with_spacing(
                Spacing::default().with_line(LineSpacing::Multiple(f64::NEG_INFINITY)),
            ),
        )
    });
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].props.spacing.line: line spacing factor -inf is not a finite \
         number"
    );

    let mut document = document_with(|_| Vec::new());
    document.add_run_style(RunStyle::default().with_kern_min(Some(Pt(f64::NAN))));
    assert_eq!(
        invalid(&document),
        "run_styles[1].kern_min: NaN is not a finite length"
    );
}

#[test]
fn lengths_that_wordprocessingml_keeps_unsigned_are_not_negative() {
    let document = document_with(|style| {
        paragraph_with(
            style,
            ParagraphProps::default().with_spacing(Spacing::default().with_before(Pt(-6.0))),
        )
    });
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].props.spacing.before: -6 pt is negative"
    );

    let mut document = document_with(|_| Vec::new());
    document.add_run_style(RunStyle::default().with_kern_min(Some(Pt(-1.0))));
    assert_eq!(
        invalid(&document),
        "run_styles[1].kern_min: -1 pt is negative"
    );

    let mut document = Document::new(DocumentSettings::default().with_default_tab_stop(Pt(-1.0)));
    document.add_section(Section::new(PageSetup::default()));
    assert_eq!(
        invalid(&document),
        "settings.default_tab_stop: -1 pt is negative"
    );

    let mut document = Document::default();
    let margins = Margins::default().with_gutter(Pt(-3.0));
    document.add_section(Section::new(PageSetup::default().with_margins(margins)));
    assert_eq!(
        invalid(&document),
        "sections[0].page.margins.gutter: -3 pt is negative"
    );

    let mut document = Document::default();
    document.add_section(Section::new(PageSetup::new(PageSize::new(
        Pt(-1.0),
        Pt(5.0),
    ))));
    assert_eq!(
        invalid(&document),
        "sections[0].page.size: page size -1 by 5 pt is negative"
    );
}

#[test]
fn negative_top_and_bottom_margins_and_indents_are_valid() {
    let margins = Margins::default()
        .with_top(Pt(-72.0))
        .with_bottom(Pt(-36.0));
    let mut document = document_with(|style| {
        paragraph_with(
            style,
            ParagraphProps::default()
                .with_indent(Indent::default().with_start(Pt(-10.0)).with_end(Pt(-10.0))),
        )
    });
    document.add_section(Section::new(PageSetup::default().with_margins(margins)));
    assert!(document.validate().is_ok());
}

#[test]
fn a_run_style_of_another_document_is_rejected() {
    let mut other = Document::default();
    let foreign = other.add_run_style(RunStyle::default());
    let document = document_with(|style| {
        vec![
            Paragraph::new(style)
                .with_inlines([Inline::text("x", foreign)])
                .into(),
        ]
    });
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].inlines[0].style: the run style id is of another document"
    );
    assert!(document.run_style(foreign).is_none());

    let document = document_with(|_| vec![Paragraph::new(foreign).into()]);
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].mark: the run style id is of another document"
    );
}

#[test]
fn a_run_style_added_to_a_clone_is_not_in_the_original() {
    let mut original = document_with(|_| Vec::new());
    let mut clone = original.clone();
    let only_in_clone = clone.add_run_style(RunStyle::default().with_bold(true));
    assert!(clone.run_style(only_in_clone).is_some_and(RunStyle::bold));
    assert!(original.run_style(only_in_clone).is_none());
    original.add_section(
        Section::new(PageSetup::default()).with_blocks([Paragraph::new(only_in_clone).into()]),
    );
    assert_eq!(
        invalid(&original),
        "sections[1].blocks[0].mark: run style 1 does not exist; the document has 1"
    );
}

#[test]
fn every_inline_checks_its_style() {
    let mut other = Document::default();
    let foreign = other.add_run_style(RunStyle::default());
    let inlines = [
        Inline::tab(foreign),
        Inline::page_break(foreign),
        Inline::symbol("Symbol", 0xF0B7, foreign),
        Inline::soft_hyphen(foreign),
        Inline::no_break_hyphen(foreign),
        Field::new(FieldKind::SectionPages, foreign).into(),
    ];
    for inline in inlines {
        let document = document_with(|style| {
            vec![Paragraph::new(style).with_inlines([inline.clone()]).into()]
        });
        assert_eq!(
            invalid(&document),
            "sections[0].blocks[0].inlines[0].style: the run style id is of another document",
            "{inline:?}"
        );
    }
}

#[test]
fn text_has_no_control_characters() {
    for (text, code, offset) in [
        ("a\tb", "0009", 1),
        ("line\n", "000A", 4),
        ("é\u{85}", "0085", 2),
    ] {
        let document = document_with(|style| {
            vec![
                Paragraph::new(style)
                    .with_inlines([Inline::text(text, style)])
                    .into(),
            ]
        });
        assert_eq!(
            invalid(&document),
            format!(
                "sections[0].blocks[0].inlines[0]: text has the control character U+{code} at \
                 byte {offset}; tabs and breaks are inlines of their own"
            )
        );
    }
    let document = document_with(|style| {
        let text = "soft\u{AD}hyphen and no\u{A0}break";
        vec![
            Paragraph::new(style)
                .with_inlines([Inline::text(text, style)])
                .into(),
        ]
    });
    assert!(document.validate().is_ok());
}

#[test]
fn a_field_result_holds_no_field() {
    let document = document_with(|style| {
        let inner = Field::new(FieldKind::Page, style);
        let outer = Field::new(FieldKind::Other("IF".into()), style).with_result([inner.into()]);
        vec![Paragraph::new(style).with_inlines([outer.into()]).into()]
    });
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].inlines[0].result[0]: a field inside the result of a field; the \
         result of the outer field holds the inner one's result as plain inlines"
    );

    let document = document_with(|style| {
        let first = Field::new(FieldKind::Page, style).with_result([Inline::text("1", style)]);
        let second = Field::new(FieldKind::NumPages, style);
        vec![
            Paragraph::new(style)
                .with_inlines([first.into(), second.into()])
                .into(),
        ]
    });
    assert!(document.validate().is_ok());
}

#[test]
fn validate_does_not_change_the_document() {
    let mut document = document_with(|style| {
        paragraph_with(
            style,
            ParagraphProps::default().with_tabs([TabStop::default().with_position(Pt(2000.0))]),
        )
    });
    assert!(document.validate().is_ok());
    let position = |document: &Document| match &document.sections()[0].blocks()[0] {
        Block::Paragraph(paragraph) => paragraph.props().tabs()[0].position(),
        other => panic!("{other:?}"),
    };
    assert_eq!(position(&document), Pt(2000.0));
    assert_eq!(document.clamp().len(), 1);
    assert_eq!(position(&document), Pt(1584.0));
}

#[test]
fn tab_stops_are_clamped_to_22_inches_either_way() {
    let tabs = [
        TabStop::default().with_position(Pt(1700.0)),
        TabStop::default().with_position(Pt(-1600.0)),
        TabStop::default().with_position(Pt(1584.0)),
        TabStop::default().with_position(Pt(-1584.0)),
    ];
    let mut document =
        document_with(|style| paragraph_with(style, ParagraphProps::default().with_tabs(tabs)));
    assert_eq!(
        entries(&document.clamp()),
        [
            "sections[0].blocks[0].props.tabs[0].position: tab stop position 1700 pt is outside \
             Word's range of -1584 to 1584 pt and becomes 1584 pt",
            "sections[0].blocks[0].props.tabs[1].position: tab stop position -1600 pt is outside \
             Word's range of -1584 to 1584 pt and becomes -1584 pt",
        ]
    );
    assert!(document.clamp().is_empty());
}

#[test]
fn run_styles_are_clamped_to_words_limits() {
    let mut document = document_with(|_| Vec::new());
    for style in [
        RunStyle::default().with_letter_spacing(Pt(1600.0)),
        RunStyle::default().with_letter_spacing(Pt(-1600.0)),
        RunStyle::default().with_kern_min(Some(Pt(2000.0))),
        RunStyle::default().with_scale(0),
        RunStyle::default().with_scale(601),
        RunStyle::default()
            .with_letter_spacing(Pt(1584.0))
            .with_kern_min(Some(Pt(1638.5)))
            .with_scale(600),
        RunStyle::default()
            .with_letter_spacing(Pt(-1584.0))
            .with_scale(1),
    ] {
        document.add_run_style(style);
    }
    assert_eq!(
        entries(&document.clamp()),
        [
            "run_styles[1].letter_spacing: letter spacing 1600 pt is outside Word's range of \
             -1584 to 1584 pt and becomes 1584 pt",
            "run_styles[2].letter_spacing: letter spacing -1600 pt is outside Word's range of \
             -1584 to 1584 pt and becomes -1584 pt",
            "run_styles[3].kern_min: kerning threshold 2000 pt is outside Word's range of 0 to \
             1638.5 pt and becomes 1638.5 pt",
            "run_styles[4].scale: character scale 0 % is outside Word's range of 1 to 600 % and \
             becomes 100 %",
            "run_styles[5].scale: character scale 601 % is outside Word's range of 1 to 600 % \
             and becomes 600 %",
        ]
    );
    assert!(document.clamp().is_empty());
}

#[test]
fn the_default_tab_stop_is_clamped_to_a_short_of_twips() {
    let settings = DocumentSettings::default().with_default_tab_stop(Pt(2000.0));
    let mut document = Document::new(settings);
    document.add_section(Section::new(PageSetup::default()));
    assert_eq!(
        entries(&document.clamp()),
        [
            "settings.default_tab_stop: default tab stop 2000 pt is outside Word's range of 0 to \
          1638.35 pt and becomes 1638.35 pt"
        ]
    );
    assert_eq!(document.settings().default_tab_stop(), Pt(1638.35));

    // Zero is within Word's range: what it means for tabs is the layout's business.
    let mut document = Document::new(DocumentSettings::default().with_default_tab_stop(Pt(0.0)));
    document.add_section(Section::new(PageSetup::default()));
    assert!(document.clamp().is_empty());
}

#[test]
fn margins_are_clamped_to_22_inches() {
    let margins = Margins::default()
        .with_top(Pt(-1600.0))
        .with_bottom(Pt(1600.0))
        .with_right(Pt(1600.0))
        .with_header(Pt(1600.0))
        .with_footer(Pt(1600.0))
        .with_gutter(Pt(1600.0));
    let size = PageSize::new(Pt(1584.0), Pt(1584.0));
    let mut document = Document::default();
    document.add_section(Section::new(PageSetup::new(size).with_margins(margins)));
    let expected: Vec<String> = [
        ("top", "-1600", "-1584"),
        ("bottom", "1600", "1584"),
        ("right", "1600", "1584"),
        ("header", "1600", "1584"),
        ("footer", "1600", "1584"),
        ("gutter", "1600", "1584"),
    ]
    .iter()
    .map(|(field, value, clamped)| {
        let min = if matches!(*field, "top" | "bottom") {
            "-1584"
        } else {
            "0"
        };
        format!(
            "sections[0].page.margins.{field}: margin {value} pt is outside Word's range of \
             {min} to 1584 pt and becomes {clamped} pt"
        )
    })
    .collect();
    assert_eq!(entries(&document.clamp()), expected);
    let margins = document.sections()[0].page().margins();
    assert_eq!(margins.top(), Pt(-1584.0));
    assert_eq!(margins.left(), Pt(72.0));
    assert!(document.clamp().is_empty());
}

#[test]
fn a_page_is_at_least_as_large_as_its_margins_and_at_most_22_inches() {
    let margins = Margins::default()
        .with_left(Pt(300.0))
        .with_right(Pt(300.0))
        .with_gutter(Pt(50.0))
        .with_top(Pt(-200.0))
        .with_bottom(Pt(250.0));
    let mut document = Document::default();
    document.add_section(Section::new(
        PageSetup::new(PageSize::new(Pt(600.0), Pt(400.0))).with_margins(margins),
    ));
    document.add_section(Section::new(PageSetup::new(PageSize::new(
        Pt(2000.0),
        Pt(1700.0),
    ))));
    assert_eq!(
        entries(&document.clamp()),
        [
            "sections[0].page: page width 600 pt is outside Word's range of 650 to 1584 pt and \
             becomes 650 pt",
            "sections[0].page: page height 400 pt is outside Word's range of 450 to 1584 pt and \
             becomes 450 pt",
            "sections[1].page: page width 2000 pt is outside Word's range of 144 to 1584 pt and \
             becomes 1584 pt",
            "sections[1].page: page height 1700 pt is outside Word's range of 144 to 1584 pt \
             and becomes 1584 pt",
        ]
    );
    assert_eq!(
        document.sections()[0].page().size(),
        PageSize::new(Pt(650.0), Pt(450.0))
    );
    assert!(document.clamp().is_empty());
}

#[test]
fn margins_wider_than_word_allows_make_the_page_as_wide_as_word_allows() {
    let margins = Margins::default()
        .with_left(Pt(1000.0))
        .with_right(Pt(1000.0));
    let mut document = Document::default();
    document.add_section(Section::new(
        PageSetup::new(PageSize::A4).with_margins(margins),
    ));
    let report = document.clamp();
    assert_eq!(report.len(), 1);
    assert_eq!(document.sections()[0].page().size().width(), Pt(1584.0));
    assert!(document.clamp().is_empty());
}

#[test]
fn columns_are_clamped_to_words_range() {
    let mut document = Document::default();
    for columns in [
        Columns::default().with_count(0),
        Columns::default().with_count(46),
        Columns::default().with_widths(vec![Column::default().with_width(Pt(10.0)); 50]),
        Columns::default()
            .with_count(45)
            .with_widths(vec![Column::default(); 45]),
    ] {
        document.add_section(Section::new(PageSetup::default()).with_columns(columns));
    }
    assert_eq!(
        entries(&document.clamp()),
        [
            "sections[0].columns: 0 columns are outside Word's range of 1 to 45 and become 1",
            "sections[1].columns: 46 columns are outside Word's range of 1 to 45 and become 45",
            "sections[2].columns: 50 columns of their own widths are more than Word's 45; the \
             columns after the first 45 are left out",
        ]
    );
    assert_eq!(document.sections()[2].columns().widths().len(), 45);
    assert!(document.clamp().is_empty());
}

#[test]
fn clamp_reaches_headers_and_footers() {
    let mut document = document_with(|_| Vec::new());
    let style = document.add_run_style(RunStyle::default());
    let footer = Paragraph::new(style).with_props(
        ParagraphProps::default().with_tabs([TabStop::default().with_position(Pt(-2000.0))]),
    );
    document.add_section(
        Section::new(PageSetup::default())
            .with_footers(HeaderFooterSet::default().with_even([footer.into()])),
    );
    let report = document.clamp();
    assert_eq!(report.len(), 1);
    assert_eq!(
        report.entries()[0].location().to_string(),
        "sections[1].footers.even[0].props.tabs[0].position"
    );
}

#[test]
fn clamp_leaves_values_that_are_not_numbers_to_validate() {
    let mut document = document_with(|style| {
        paragraph_with(
            style,
            ParagraphProps::default().with_tabs([TabStop::default().with_position(Pt(f64::NAN))]),
        )
    });
    document.add_run_style(RunStyle::default().with_letter_spacing(Pt(f64::NAN)));
    // A margin that is not a number gives the page no bound: its valid width stays.
    let nan_margin = Margins::default().with_left(Pt(f64::NAN));
    document.add_section(Section::new(
        PageSetup::new(PageSize::A4).with_margins(nan_margin),
    ));
    let size = PageSize::new(Pt(f64::NAN), Pt(f64::INFINITY));
    document.add_section(Section::new(PageSetup::new(size)));
    let report = document.clamp();
    // Only the infinite page height is out of range; NaN compares with nothing.
    assert_eq!(
        entries(&report),
        [
            "sections[2].page: page height inf pt is outside Word's range of 144 to 1584 pt and \
          becomes 1584 pt"
        ]
    );
    assert_eq!(document.sections()[1].page().size(), PageSize::A4);
    assert!(document.validate().is_err());
}

#[test]
fn every_length_that_wordprocessingml_keeps_unsigned_is_checked() {
    let minus = Pt(-1.0);
    let in_settings = |settings: DocumentSettings| {
        let mut document = Document::new(settings);
        document.add_section(Section::new(PageSetup::default()));
        document
    };
    let in_section = |section: Section| {
        let mut document = Document::default();
        document.add_section(section);
        document
    };
    let in_margins =
        |margins: Margins| in_section(Section::new(PageSetup::default().with_margins(margins)));
    let in_columns =
        |columns: Columns| in_section(Section::new(PageSetup::default()).with_columns(columns));
    let in_props = |props: ParagraphProps| document_with(|style| paragraph_with(style, props));
    let in_border = |border: Border| {
        in_props(
            ParagraphProps::default()
                .with_borders(ParagraphBorders::default().with_top(Some(border))),
        )
    };
    let in_style = |style: RunStyle| {
        let mut document = document_with(|_| Vec::new());
        document.add_run_style(style);
        document
    };
    let cases = [
        (
            in_settings(DocumentSettings::default().with_default_tab_stop(minus)),
            "settings.default_tab_stop",
        ),
        (
            in_settings(
                DocumentSettings::default()
                    .with_hyphenation(Hyphenation::default().with_zone(minus)),
            ),
            "settings.hyphenation.zone",
        ),
        (
            in_margins(Margins::default().with_left(minus)),
            "sections[0].page.margins.left",
        ),
        (
            in_margins(Margins::default().with_right(minus)),
            "sections[0].page.margins.right",
        ),
        (
            in_margins(Margins::default().with_header(minus)),
            "sections[0].page.margins.header",
        ),
        (
            in_margins(Margins::default().with_footer(minus)),
            "sections[0].page.margins.footer",
        ),
        (
            in_margins(Margins::default().with_gutter(minus)),
            "sections[0].page.margins.gutter",
        ),
        (
            in_columns(Columns::default().with_spacing(minus)),
            "sections[0].columns.spacing",
        ),
        (
            in_columns(Columns::default().with_widths([Column::default().with_width(minus)])),
            "sections[0].columns.widths[0].width",
        ),
        (
            in_columns(Columns::default().with_widths([Column::default().with_spacing(minus)])),
            "sections[0].columns.widths[0].spacing",
        ),
        (
            in_props(ParagraphProps::default().with_spacing(Spacing::default().with_before(minus))),
            "sections[0].blocks[0].props.spacing.before",
        ),
        (
            in_props(ParagraphProps::default().with_spacing(Spacing::default().with_after(minus))),
            "sections[0].blocks[0].props.spacing.after",
        ),
        (
            in_border(Border::default().with_width(minus)),
            "sections[0].blocks[0].props.borders.top.width",
        ),
        (
            in_border(Border::default().with_space(minus)),
            "sections[0].blocks[0].props.borders.top.space",
        ),
        (
            in_style(RunStyle::default().with_size(minus)),
            "run_styles[1].size",
        ),
        (
            in_style(RunStyle::default().with_size_complex_script(minus)),
            "run_styles[1].size_complex_script",
        ),
        (
            in_style(RunStyle::default().with_kern_min(Some(minus))),
            "run_styles[1].kern_min",
        ),
    ];
    for (document, location) in cases {
        assert_eq!(invalid(&document), format!("{location}: -1 pt is negative"));
    }

    for (size, problem) in [
        (
            PageSize::new(Pt(1.0), Pt(-1.0)),
            "size: page size 1 by -1 pt is negative",
        ),
        (
            PageSize::new(Pt(1.0), Pt(f64::NAN)),
            "size.height: NaN is not a finite length",
        ),
    ] {
        let document = in_section(Section::new(PageSetup::new(size)));
        assert_eq!(invalid(&document), format!("sections[0].page.{problem}"));
    }
}

#[test]
fn font_sizes_and_border_distances_are_clamped_to_words_limits() {
    let mut document = document_with(|style| {
        let border = Border::default().with_space(Pt(400.0));
        paragraph_with(
            style,
            ParagraphProps::default()
                .with_borders(ParagraphBorders::default().with_bar(Some(border))),
        )
    });
    document.add_run_style(RunStyle::default().with_size(Pt(0.0)));
    document.add_run_style(RunStyle::default().with_size_complex_script(Pt(5000.0)));
    document.add_run_style(
        RunStyle::default()
            .with_size(Pt(1.0))
            .with_size_complex_script(Pt(1638.5)),
    );
    assert_eq!(
        entries(&document.clamp()),
        [
            "run_styles[1].size: font size 0 pt is outside Word's range of 1 to 1638.5 pt and \
             becomes 1 pt",
            "run_styles[2].size_complex_script: font size 5000 pt is outside Word's range of 1 \
             to 1638.5 pt and becomes 1638.5 pt",
            "sections[0].blocks[0].props.borders.bar.space: border distance 400 pt is outside \
             Word's range of 0 to 31 pt and becomes 31 pt",
        ]
    );
    assert!(document.clamp().is_empty());
}

#[test]
fn defaults_are_what_a_document_means_when_it_says_nothing() {
    // ECMA-376 Part 1, §17.3.1.44: widow control is on unless something turns it off.
    assert!(ParagraphProps::default().widow_control());
    // MS-OI29500 2.1.38 b and c: a border without a width or a distance has 0 for both.
    let border = Border::default();
    assert_eq!((border.width(), border.space()), (Pt(0.0), Pt(0.0)));
    assert_eq!(
        DocumentSettings::default().compat_mode(),
        CompatMode::Word2007
    );
    assert_eq!(PageNumbering::default().format(), &NumberFormat::Decimal);
}

#[test]
fn accessors_read_back_what_the_setters_wrote() {
    let fonts = RunFonts::new("Calibri").with_east_asia("MS Mincho");
    assert_eq!(fonts.ascii(), "Calibri");
    assert_eq!(fonts.high_ansi(), "Calibri");
    assert_eq!(fonts.complex_script(), "Calibri");
    assert_eq!(fonts.east_asia(), "MS Mincho");
    assert_eq!(RunFonts::default().ascii(), "Times New Roman");

    let language = Language::default().with_latin(Some("de-DE".into()));
    assert_eq!(language.latin(), Some("de-DE"));
    assert_eq!(language.bidi(), None);

    let style = RunStyle::default()
        .with_fonts(fonts.clone())
        .with_language(language.clone());
    assert_eq!(style.fonts(), &fonts);
    assert_eq!(style.language(), &language);
    assert_eq!(style.size(), Pt(10.0));
    assert_eq!(style.scale(), 100);
    assert_eq!(style.color(), ColorValue::Auto);

    let tabs = [
        TabStop::default(),
        TabStop::default().with_kind(TabKind::Bar),
    ];
    let props = ParagraphProps::default().with_tabs(tabs);
    assert_eq!(props.tabs(), tabs);
    assert!(props.word_wrap());
    assert_eq!(props.spacing().line(), LineSpacing::Multiple(1.0));

    let style = Document::default().add_run_style(RunStyle::default());
    let kind = |inline: Inline| match inline {
        Inline::Break { kind, .. } => kind,
        other => panic!("not a break: {other:?}"),
    };
    assert_eq!(
        kind(Inline::line_break(BreakClear::All, style)),
        BreakKind::Line(BreakClear::All)
    );
    assert_eq!(kind(Inline::page_break(style)), BreakKind::Page);
    assert_eq!(kind(Inline::column_break(style)), BreakKind::Column);
}

#[test]
fn compat_flags_map_names_both_ways() {
    let named = [
        ("doNotExpandShiftReturn", CompatFlag::DoNotExpandShiftReturn),
        (
            "doNotUseIndentAsNumberingTabStop",
            CompatFlag::DoNotUseIndentAsNumberingTabStop,
        ),
        (
            "doNotUseHTMLParagraphAutoSpacing",
            CompatFlag::DoNotUseHTMLParagraphAutoSpacing,
        ),
        ("noLeading", CompatFlag::NoLeading),
        (
            "splitPgBreakAndParaMark",
            CompatFlag::SplitPgBreakAndParaMark,
        ),
        ("growAutofit", CompatFlag::GrowAutofit),
        (
            "doNotAutofitConstrainedTables",
            CompatFlag::DoNotAutofitConstrainedTables,
        ),
        ("layoutRawTableWidth", CompatFlag::LayoutRawTableWidth),
        ("layoutTableRowsApart", CompatFlag::LayoutTableRowsApart),
        (
            "allowSpaceOfSameStyleInTable",
            CompatFlag::AllowSpaceOfSameStyleInTable,
        ),
        (
            "doNotVertAlignCellWithSp",
            CompatFlag::DoNotVertAlignCellWithSp,
        ),
        (
            "doNotBreakWrappedTables",
            CompatFlag::DoNotBreakWrappedTables,
        ),
    ];
    for (name, flag) in &named {
        assert_eq!(&CompatFlag::from_name(name), flag);
        assert_eq!(flag.name(), *name);
    }
    assert_eq!(CompatFlag::NAMED.len(), named.len());
    let other = CompatFlag::from_name("useFELayout");
    assert_eq!(other, CompatFlag::Other("useFELayout".into()));
    assert_eq!(other.name(), "useFELayout");

    let settings = DocumentSettings::default().with_compat_flags([CompatFlag::GrowAutofit]);
    assert!(settings.has_compat_flag(&CompatFlag::GrowAutofit));
    assert!(!settings.has_compat_flag(&CompatFlag::NoLeading));
}

#[test]
fn an_invalid_document_is_an_invalid_argument() {
    let message = invalid(&Document::default());
    let err = Document::default().validate().unwrap_err();
    match officewright_core::Error::from(err) {
        officewright_core::Error::InvalidArgument(text) => assert_eq!(text, message),
        other => panic!("{other:?}"),
    }
}

/// A document with a two-level list: one definition, two instances of it, the second
/// starting level 0 again at 5, and `items` paragraphs of the first instance.
fn list_document(item: impl FnOnce(ListInstanceId, RunStyleId) -> ListItem) -> Document {
    let mut document = Document::default();
    let style = document.add_run_style(RunStyle::default());
    let decimal = Level::default().with_start(1).with_text([
        LevelTextPart::Placeholder(0),
        LevelTextPart::Literal(".".into()),
    ]);
    let letter = Level::default()
        .with_start(1)
        .with_format(NumberFormat::LowerLetter)
        .with_text([
            LevelTextPart::Placeholder(0),
            LevelTextPart::Literal(".".into()),
            LevelTextPart::Placeholder(1),
        ])
        .with_restart(Restart::AfterLevel(0))
        .with_suffix(Suffix::Space)
        .with_alignment(LevelAlignment::End);
    let list = document.add_list(ListDefinition::default().with_levels([decimal, letter]));
    let first = document.add_list_instance(ListInstance::new(list));
    let restarted =
        ListInstance::new(list).with_overrides([LevelOverride::new(0).with_start(Some(5))]);
    document.add_list_instance(restarted);
    let item = item(first, style);
    let paragraph = Paragraph::new(style).with_list(Some(item));
    document.add_section(Section::new(PageSetup::default()).with_blocks([paragraph.into()]));
    document
}

#[test]
fn a_list_validates_and_its_entries_read_back() {
    let document = list_document(|instance, style| ListItem::new(instance, 1, style));
    assert!(document.validate().is_ok());
    let Block::Paragraph(paragraph) = &document.sections()[0].blocks()[0] else {
        panic!("not a paragraph");
    };
    let item = paragraph.list().expect("a list item");
    let instance = document
        .list_instance(item.instance())
        .expect("the instance");
    let list = document
        .list(instance.definition())
        .expect("the definition");
    assert_eq!(list.levels()[1].format(), &NumberFormat::LowerLetter);
    assert_eq!(list.levels()[1].restart(), Restart::AfterLevel(0));
    assert!(Document::default().list(instance.definition()).is_none());
    assert!(Document::default().list_instance(item.instance()).is_none());
}

#[test]
fn list_definitions_have_levels_0_to_8() {
    // A paragraph may be at any level Word loads (MS-OI29500 2.1.277 a, up to 255); one
    // its definition lacks gets no label.
    for level in [9, 255] {
        let document = list_document(|instance, style| ListItem::new(instance, level, style));
        assert!(document.validate().is_ok(), "{level}");
    }

    let mut document = list_document(|instance, style| ListItem::new(instance, 0, style));
    document.add_list(ListDefinition::default().with_levels(vec![Level::default(); 10]));
    assert_eq!(
        invalid(&document),
        "lists[1].levels: 10 levels, but a list has at most 9"
    );

    let mut document = list_document(|instance, style| ListItem::new(instance, 0, style));
    let text = [
        LevelTextPart::Literal("(".into()),
        LevelTextPart::Placeholder(9),
    ];
    document.add_list(ListDefinition::default().with_levels([Level::default().with_text(text)]));
    assert_eq!(
        invalid(&document),
        "lists[1].levels[0].text[1]: level 9 does not exist; levels are 0 to 8"
    );

    // Word takes w:lvlRestart from 0 to 7 (MS-OI29500 2.1.282 a): after levels 0 to 6.
    for (level, valid) in [(6, true), (7, false), (12, false)] {
        let mut document = list_document(|instance, style| ListItem::new(instance, 0, style));
        let restart = Level::default().with_restart(Restart::AfterLevel(level));
        document.add_list(ListDefinition::default().with_levels([restart]));
        if valid {
            assert!(document.validate().is_ok());
        } else {
            assert_eq!(
                invalid(&document),
                format!(
                    "lists[1].levels[0].restart: restarting after level {level}, but Word \
                     restarts after levels 0 to 6 at most"
                )
            );
        }
    }
}

#[test]
fn an_instance_overrides_a_level_at_most_once() {
    let mut document = list_document(|instance, style| ListItem::new(instance, 0, style));
    let list = document.add_list(ListDefinition::default());
    let overrides = [
        LevelOverride::new(2).with_start(Some(1)),
        LevelOverride::new(0),
        LevelOverride::new(2).with_replacement(Some(Level::default())),
    ];
    document.add_list_instance(ListInstance::new(list).with_overrides(overrides));
    assert_eq!(
        invalid(&document),
        "list_instances[2].overrides[2]: a second override of level 2"
    );

    let mut document = list_document(|instance, style| ListItem::new(instance, 0, style));
    let list = document.add_list(ListDefinition::default());
    document.add_list_instance(ListInstance::new(list).with_overrides([LevelOverride::new(9)]));
    assert_eq!(
        invalid(&document),
        "list_instances[2].overrides[0].level: level 9 does not exist; levels are 0 to 8"
    );
}

#[test]
fn list_ids_belong_to_their_document() {
    let mut other = Document::default();
    let foreign_list = other.add_list(ListDefinition::default());
    let foreign_instance = other.add_list_instance(ListInstance::new(foreign_list));

    let document = list_document(|_, style| ListItem::new(foreign_instance, 0, style));
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].list.instance: the list instance id is of another document"
    );

    let mut document = list_document(|instance, style| ListItem::new(instance, 0, style));
    document.add_list_instance(ListInstance::new(foreign_list));
    assert_eq!(
        invalid(&document),
        "list_instances[2].definition: the list definition id is of another document"
    );

    let mut other = Document::default();
    let foreign_style = other.add_run_style(RunStyle::default());
    let document = list_document(|instance, _| ListItem::new(instance, 0, foreign_style));
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].list.label_style: the run style id is of another document"
    );

    // A clone's instance is not in the original.
    let mut original = list_document(|instance, style| ListItem::new(instance, 0, style));
    let mut clone = original.clone();
    let list = clone.add_list(ListDefinition::default());
    original.add_list_instance(ListInstance::new(list));
    assert_eq!(
        invalid(&original),
        "list_instances[2].definition: list definition 1 does not exist; the document has 1"
    );
}

/// A table of `rows`, each holding cells with one empty paragraph.
fn table(style: RunStyleId, rows: usize, cells: usize) -> Table {
    let cell = Cell::default().with_blocks([Paragraph::new(style).into()]);
    let row = Row::default().with_cells(vec![cell; cells]);
    Table::default()
        .with_grid(vec![Pt(100.0); cells])
        .with_rows(vec![row; rows])
}

#[test]
fn a_table_with_every_property_validates_and_needs_no_clamp() {
    let border = Border::default()
        .with_style(BorderStyle::Single)
        .with_width(Pt(0.5));
    let mut document = document_with(|style| {
        let inner = table(style, 1, 1);
        let cell = Cell::default()
            .with_width(PreferredWidth::Percent(50.0))
            .with_grid_span(2)
            .with_vertical_merge(VerticalMerge::Restart)
            .with_margins(Some(CellMargins::default().with_top(Pt(2.0))))
            .with_borders(
                CellBorders::default()
                    .with_top(BorderSide::Nil)
                    .with_start(BorderSide::None)
                    .with_bottom(BorderSide::Line(border)),
            )
            .with_shading(Some(
                Shading::default().with_pattern(ShadingPattern::Percent10),
            ))
            .with_vertical_align(CellVerticalAlign::Center)
            .with_no_wrap(true)
            .with_text_direction(TextDirection::BottomToTopLeftToRight)
            .with_blocks([
                Paragraph::new(style).into(),
                inner.into(),
                Paragraph::new(style).into(),
            ]);
        let row = Row::default()
            .with_height(RowHeight::AtLeast(Pt(20.0)))
            .with_cant_split(true)
            .with_header(true)
            .with_grid_before(1)
            .with_width_before(PreferredWidth::Points(Pt(30.0)))
            .with_cells([cell]);
        let props = TableProps::default()
            .with_width(PreferredWidth::Points(Pt(400.0)))
            .with_layout(TableLayout::Fixed)
            .with_indent(Pt(-5.4))
            .with_alignment(TableAlignment::Center)
            .with_cell_spacing(Pt(1.0))
            .with_borders(
                TableBorders::default()
                    .with_top(BorderSide::Line(border))
                    .with_inside_horizontal(BorderSide::Line(border)),
            )
            .with_floating(true);
        vec![
            Table::default()
                .with_props(props)
                .with_grid([Pt(30.0), Pt(200.0), Pt(170.0)])
                .with_rows([row])
                .into(),
        ]
    });
    assert!(document.validate().is_ok());
    let report = document.clamp();
    assert!(report.is_empty(), "{:?}", entries(&report));
}

#[test]
fn tables_have_rows_rows_have_cells_and_cells_have_content() {
    let document = document_with(|style| vec![table(style, 0, 2).into()]);
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].rows: a table without rows"
    );

    let document = document_with(|style| vec![table(style, 2, 0).into()]);
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].rows[0].cells: a row without cells"
    );

    let document = document_with(|_| {
        let row = Row::default().with_cells([Cell::default()]);
        vec![
            Table::default()
                .with_grid([Pt(10.0)])
                .with_rows([row])
                .into(),
        ]
    });
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].rows[0].cells[0].blocks: a cell without content; it needs at \
         least an empty paragraph"
    );
}

#[test]
fn table_widths_and_margins_are_not_negative() {
    type Change = fn(Table) -> Table;
    let cases: [(Change, &str); 8] = [
        (
            |t| t.with_grid([Pt(10.0), Pt(-3.0)]),
            "grid: grid column 1 is -3 pt wide, a negative width",
        ),
        (
            |t| t.with_props(TableProps::default().with_width(PreferredWidth::Points(Pt(-1.0)))),
            "props.width: -1 pt is negative",
        ),
        (
            |t| t.with_props(TableProps::default().with_width(PreferredWidth::Percent(-5.0))),
            "props.width: -5 % is negative",
        ),
        (
            |t| t.with_props(TableProps::default().with_width(PreferredWidth::Percent(f64::NAN))),
            "props.width: NaN % is not a finite percentage",
        ),
        (
            |t| t.with_props(TableProps::default().with_cell_spacing(Pt(-1.0))),
            "props.cell_spacing: -1 pt is negative",
        ),
        (
            |t| {
                let margins = CellMargins::default().with_end(Pt(-2.0));
                t.with_props(TableProps::default().with_cell_margins(margins))
            },
            "props.cell_margins.end: -2 pt is negative",
        ),
        (
            |t| {
                let rows = t.rows().iter().map(|row| {
                    row.clone()
                        .with_width_after(PreferredWidth::Points(Pt(-4.0)))
                });
                t.clone().with_rows(rows.collect::<Vec<_>>())
            },
            "rows[0].width_after: -4 pt is negative",
        ),
        (
            |t| {
                let rows = t
                    .rows()
                    .iter()
                    .map(|row| row.clone().with_height(RowHeight::Exact(Pt(-1.0))));
                t.clone().with_rows(rows.collect::<Vec<_>>())
            },
            "rows[0].height: -1 pt is negative",
        ),
    ];
    for (change, problem) in cases {
        let document = document_with(|style| vec![change(table(style, 1, 2)).into()]);
        assert_eq!(
            invalid(&document),
            format!("sections[0].blocks[0].{problem}")
        );
    }

    let border = Border::default().with_width(Pt(f64::NAN));
    let document = document_with(|style| {
        let borders = CellBorders::default().with_end(BorderSide::Line(border));
        let cell = Cell::default()
            .with_borders(borders)
            .with_blocks([Paragraph::new(style).into()]);
        let row = Row::default().with_cells([cell]);
        vec![
            Table::default()
                .with_grid([Pt(10.0)])
                .with_rows([row])
                .into(),
        ]
    });
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].rows[0].cells[0].borders.end.width: NaN is not a finite length"
    );
}

#[test]
fn every_length_of_a_table_that_word_keeps_unsigned_is_checked() {
    let minus = Pt(-1.0);
    let in_props = |props: TableProps| {
        document_with(|style| vec![table(style, 1, 1).with_props(props).into()])
    };
    let in_row = |row: fn(Row) -> Row| {
        document_with(|style| {
            let table = table(style, 1, 1);
            let rows: Vec<Row> = table.rows().iter().cloned().map(row).collect();
            vec![table.with_rows(rows).into()]
        })
    };
    let in_cell = |cell: Cell| {
        document_with(|style| {
            let cell = cell.with_blocks([Paragraph::new(style).into()]);
            let row = Row::default().with_cells([cell]);
            vec![
                Table::default()
                    .with_grid([Pt(10.0)])
                    .with_rows([row])
                    .into(),
            ]
        })
    };
    let margins = |change: fn(CellMargins) -> CellMargins| change(CellMargins::default());
    let cases = [
        (
            in_props(TableProps::default().with_cell_margins(margins(|m| m.with_top(Pt(-1.0))))),
            "props.cell_margins.top",
        ),
        (
            in_props(TableProps::default().with_cell_margins(margins(|m| m.with_start(Pt(-1.0))))),
            "props.cell_margins.start",
        ),
        (
            in_props(TableProps::default().with_cell_margins(margins(|m| m.with_bottom(Pt(-1.0))))),
            "props.cell_margins.bottom",
        ),
        (
            in_props(TableProps::default().with_cell_margins(margins(|m| m.with_end(Pt(-1.0))))),
            "props.cell_margins.end",
        ),
        (
            in_props(TableProps::default().with_cell_spacing(minus)),
            "props.cell_spacing",
        ),
        (
            in_props(TableProps::default().with_width(PreferredWidth::Points(minus))),
            "props.width",
        ),
        (
            in_row(|row| row.with_width_before(PreferredWidth::Points(Pt(-1.0)))),
            "rows[0].width_before",
        ),
        (
            in_row(|row| row.with_width_after(PreferredWidth::Points(Pt(-1.0)))),
            "rows[0].width_after",
        ),
        (
            in_row(|row| row.with_height(RowHeight::AtLeast(Pt(-1.0)))),
            "rows[0].height",
        ),
        (
            in_row(|row| row.with_height(RowHeight::Exact(Pt(-1.0)))),
            "rows[0].height",
        ),
        (
            in_cell(Cell::default().with_width(PreferredWidth::Points(minus))),
            "rows[0].cells[0].width",
        ),
        (
            in_cell(Cell::default().with_margins(Some(margins(|m| m.with_top(Pt(-1.0)))))),
            "rows[0].cells[0].margins.top",
        ),
        (
            in_cell(Cell::default().with_margins(Some(margins(|m| m.with_end(Pt(-1.0)))))),
            "rows[0].cells[0].margins.end",
        ),
    ];
    for (document, location) in cases {
        assert_eq!(
            invalid(&document),
            format!("sections[0].blocks[0].{location}: -1 pt is negative")
        );
    }
}

#[test]
fn rows_and_spans_are_clamped_to_words_limits() {
    let mut document = document_with(|style| {
        let cell = |span| {
            Cell::default()
                .with_grid_span(span)
                .with_blocks([Paragraph::new(style).into()])
        };
        let rows = [
            Row::default()
                .with_height(RowHeight::Exact(Pt(0.0)))
                .with_cells([cell(0)]),
            Row::default()
                .with_height(RowHeight::AtLeast(Pt(2000.0)))
                .with_cells([cell(1)]),
            Row::default()
                .with_height(RowHeight::Exact(Pt(1584.0)))
                .with_cells([cell(2)]),
        ];
        vec![
            Table::default()
                .with_grid([Pt(50.0), Pt(50.0)])
                .with_rows(rows)
                .into(),
        ]
    });
    assert_eq!(
        entries(&document.clamp()),
        [
            "sections[0].blocks[0].rows[0].height: an exact row height of 0 pt, which Word does \
             not accept, becomes the height of the content",
            "sections[0].blocks[0].rows[0].cells[0].grid_span: a cell spanning 0 grid columns \
             spans 1",
            "sections[0].blocks[0].rows[1].height: row height 2000 pt is outside Word's range of \
             0 to 1584 pt and becomes 1584 pt",
        ]
    );
    let Block::Table(table) = &document.sections()[0].blocks()[0] else {
        panic!("not a table");
    };
    assert_eq!(table.rows()[0].height(), RowHeight::Auto);
    assert_eq!(table.rows()[0].cells()[0].grid_span(), 1);
    assert!(document.clamp().is_empty());
}

#[test]
fn clamp_reaches_into_nested_tables() {
    let mut document = document_with(|style| {
        let tab = TabStop::default().with_position(Pt(1600.0));
        let paragraph =
            Paragraph::new(style).with_props(ParagraphProps::default().with_tabs([tab]));
        let inner_cell = Cell::default().with_blocks([paragraph.into()]);
        let inner = Table::default()
            .with_grid([Pt(10.0)])
            .with_rows([Row::default().with_cells([inner_cell])]);
        let outer_cell = Cell::default().with_blocks([inner.into(), Paragraph::new(style).into()]);
        vec![
            Table::default()
                .with_grid([Pt(10.0)])
                .with_rows([Row::default().with_cells([outer_cell])])
                .into(),
        ]
    });
    let report = document.clamp();
    assert_eq!(
        report.entries()[0].location().to_string(),
        "sections[0].blocks[0].rows[0].cells[0].blocks[0].rows[0].cells[0].blocks[0].props.\
         tabs[0].position"
    );
}

/// `depth` tables nested in each other; the innermost cell's span is 0.
fn nested_tables(style: RunStyleId, depth: usize) -> Block {
    let mut block: Block = Paragraph::new(style).into();
    for level in 0..depth {
        let span = if level == 0 { 0 } else { 1 };
        let blocks = if level == 0 {
            vec![block]
        } else {
            vec![block, Paragraph::new(style).into()]
        };
        let cell = Cell::default().with_grid_span(span).with_blocks(blocks);
        block = Table::default()
            .with_grid([Pt(10.0)])
            .with_rows([Row::default().with_cells([cell])])
            .into();
    }
    block
}

#[test]
fn tables_nest_at_most_max_table_depth_deep() {
    let mut document = document_with(|style| vec![nested_tables(style, MAX_TABLE_DEPTH)]);
    assert!(document.validate().is_ok());
    assert_eq!(document.clamp().len(), 1, "the innermost span is clamped");

    let document = document_with(|style| vec![nested_tables(style, MAX_TABLE_DEPTH + 1)]);
    let err = document.validate().unwrap_err();
    assert!(
        matches!(
            err,
            Error::NestingTooDeep {
                limit: MAX_TABLE_DEPTH,
                ..
            }
        ),
        "{err:?}"
    );
    let path = "rows[0].cells[0].blocks[0].".repeat(MAX_TABLE_DEPTH);
    assert_eq!(
        err.to_string(),
        format!("sections[0].blocks[0].{path}: tables are nested more than 100 deep")
            .replace(".: ", ": ")
    );
    match officewright_core::Error::from(err) {
        officewright_core::Error::LimitExceeded(_) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_deep_nest_of_tables_stops_both_walks_at_the_limit() {
    // Far deeper than the limit: neither walk may recurse that deep.
    // Kept to 500 so that dropping the nest in a debug build fits the 2 MiB of a test
    // thread on every platform.
    let mut document = document_with(|style| vec![nested_tables(style, 500)]);
    assert!(matches!(
        document.validate(),
        Err(Error::NestingTooDeep { .. })
    ));
    // The span of 0 is in the innermost table, beyond the limit: left alone.
    assert!(document.clamp().is_empty());
}

#[test]
fn a_cell_ends_with_a_paragraph() {
    let in_cell = |blocks: fn(RunStyleId) -> Vec<Block>| {
        document_with(|style| {
            let cell = Cell::default().with_blocks(blocks(style));
            let row = Row::default().with_cells([cell]);
            vec![
                Table::default()
                    .with_grid([Pt(10.0)])
                    .with_rows([row])
                    .into(),
            ]
        })
    };
    // MS-OI29500 2.1.168 b: Word fails to open a cell whose last block is not a paragraph.
    let document = in_cell(|style| vec![table(style, 1, 1).into()]);
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].rows[0].cells[0].blocks: a cell whose last block is not a \
         paragraph; Word needs one after a nested table"
    );
    let document = in_cell(|style| vec![table(style, 1, 1).into(), Paragraph::new(style).into()]);
    assert!(document.validate().is_ok());
}

#[test]
fn table_and_list_defaults_are_what_a_document_means_when_it_says_nothing() {
    let level = Level::default();
    // ECMA-376 Part 1 §17.9: start 0, decimal, a tab after the label, aligned at the start;
    // without w:lvlRestart Word restarts after any outer level (MS-OI29500 2.1.282 d).
    assert_eq!(level.start(), 0);
    assert_eq!(level.format(), &NumberFormat::Decimal);
    assert!(!level.legal());
    assert_eq!(level.restart(), Restart::Default);
    assert_eq!(level.suffix(), Suffix::Tab);
    assert_eq!(level.alignment(), LevelAlignment::Start);
    assert!(level.text().is_empty());

    let props = TableProps::default();
    assert_eq!(props.width(), PreferredWidth::Auto);
    assert_eq!(props.layout(), TableLayout::Autofit);
    assert_eq!(props.indent(), Pt(0.0));
    assert_eq!(props.alignment(), TableAlignment::Start);
    assert_eq!(props.cell_spacing(), Pt(0.0));
    assert!(!props.floating());
    // Word's "Normal Table" style: 0 at the top and bottom, 108 twips at the sides.
    let margins = props.cell_margins();
    assert_eq!(
        (
            margins.top(),
            margins.start(),
            margins.bottom(),
            margins.end()
        ),
        (Pt(0.0), Pt(5.4), Pt(0.0), Pt(5.4))
    );
    assert_eq!(props.borders().top(), BorderSide::Inherit);

    let row = Row::default();
    assert_eq!(row.height(), RowHeight::Auto);
    assert!(!row.header() && !row.cant_split());
    assert_eq!((row.grid_before(), row.grid_after()), (0, 0));

    let cell = Cell::default();
    assert_eq!(cell.grid_span(), 1);
    assert_eq!(cell.vertical_merge(), VerticalMerge::None);
    assert_eq!(cell.vertical_align(), CellVerticalAlign::Top);
    // MS-OI29500 2.1.172 a: left to right, top to bottom.
    assert_eq!(cell.text_direction(), TextDirection::LeftToRightTopToBottom);
    assert_eq!(cell.borders().start(), BorderSide::Inherit);
    assert_eq!(cell.margins(), None);
}

#[test]
fn many_tables_side_by_side_are_not_nested() {
    let mut document = document_with(|style| {
        (0..=MAX_TABLE_DEPTH)
            .map(|_| {
                let cell = Cell::default()
                    .with_grid_span(0)
                    .with_blocks([Paragraph::new(style).into()]);
                Table::default()
                    .with_grid([Pt(10.0)])
                    .with_rows([Row::default().with_cells([cell])])
                    .into()
            })
            .collect()
    });
    assert!(document.validate().is_ok());
    assert_eq!(document.clamp().len(), MAX_TABLE_DEPTH + 1);
}

#[test]
fn table_lengths_are_finite() {
    let in_cell = |cell: Cell| {
        document_with(|style| {
            let row = Row::default().with_cells([cell.with_blocks([Paragraph::new(style).into()])]);
            vec![
                Table::default()
                    .with_grid([Pt(10.0)])
                    .with_rows([row])
                    .into(),
            ]
        })
    };
    let document = in_cell(Cell::default().with_width(PreferredWidth::Points(Pt(f64::NAN))));
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].rows[0].cells[0].width: NaN is not a finite length"
    );
    let document = document_with(|style| {
        let cell = Cell::default().with_blocks([Paragraph::new(style).into()]);
        let row = Row::default()
            .with_height(RowHeight::AtLeast(Pt(f64::INFINITY)))
            .with_cells([cell]);
        vec![
            Table::default()
                .with_grid([Pt(10.0)])
                .with_rows([row])
                .into(),
        ]
    });
    assert_eq!(
        invalid(&document),
        "sections[0].blocks[0].rows[0].height: inf is not a finite length"
    );
}

#[test]
fn table_measurements_are_clamped_to_a_short_of_twips() {
    let mut document = document_with(|style| {
        let border = BorderSide::Line(Border::default().with_space(Pt(40.0)));
        let cell = Cell::default()
            .with_width(PreferredWidth::Points(Pt(2000.0)))
            .with_borders(CellBorders::default().with_top(border))
            .with_blocks([Paragraph::new(style).into()]);
        let row = Row::default()
            .with_width_before(PreferredWidth::Points(Pt(5000.0)))
            .with_width_after(PreferredWidth::Percent(1000.0))
            .with_cells([cell]);
        let props = TableProps::default()
            .with_width(PreferredWidth::Points(Pt(5000.0)))
            .with_indent(Pt(-5000.0))
            .with_cell_spacing(Pt(5000.0))
            .with_cell_margins(CellMargins::default().with_start(Pt(5000.0)))
            .with_borders(TableBorders::default().with_bottom(border));
        vec![
            Table::default()
                .with_props(props)
                .with_grid([Pt(10.0)])
                .with_rows([row])
                .into(),
        ]
    });
    let range = "is outside Word's range of";
    assert_eq!(
        entries(&document.clamp()),
        [
            format!(
                "sections[0].blocks[0].props.width: table measurement 5000 pt {range} 0 to 1638.35 pt and becomes 1638.35 pt"
            ),
            format!(
                "sections[0].blocks[0].props.indent: table indent -5000 pt {range} -1638.35 to 1638.35 pt and becomes -1638.35 pt"
            ),
            format!(
                "sections[0].blocks[0].props.cell_margins.start: table measurement 5000 pt {range} 0 to 1638.35 pt and becomes 1638.35 pt"
            ),
            format!(
                "sections[0].blocks[0].props.cell_spacing: table measurement 5000 pt {range} 0 to 1638.35 pt and becomes 1638.35 pt"
            ),
            format!(
                "sections[0].blocks[0].props.borders.bottom.space: border distance 40 pt {range} 0 to 31 pt and becomes 31 pt"
            ),
            format!(
                "sections[0].blocks[0].rows[0].width_before: table measurement 5000 pt {range} 0 to 1638.35 pt and becomes 1638.35 pt"
            ),
            format!(
                "sections[0].blocks[0].rows[0].cells[0].width: table measurement 2000 pt {range} 0 to 1638.35 pt and becomes 1638.35 pt"
            ),
            format!(
                "sections[0].blocks[0].rows[0].cells[0].borders.top.space: border distance 40 pt {range} 0 to 31 pt and becomes 31 pt"
            ),
        ]
    );
    // Percentages are left as they are: Word's limit depends on how the file wrote them.
    let Block::Table(table) = &document.sections()[0].blocks()[0] else {
        panic!("not a table");
    };
    assert_eq!(
        table.rows()[0].width_after(),
        PreferredWidth::Percent(1000.0)
    );
    assert!(document.clamp().is_empty());
}

#[test]
fn a_negative_exact_row_height_is_clamped_in_one_go() {
    let mut document = document_with(|style| {
        let cell = Cell::default().with_blocks([Paragraph::new(style).into()]);
        let row = Row::default()
            .with_height(RowHeight::Exact(Pt(-1.0)))
            .with_cells([cell]);
        vec![
            Table::default()
                .with_grid([Pt(10.0)])
                .with_rows([row])
                .into(),
        ]
    });
    assert_eq!(document.clamp().len(), 2);
    assert!(document.clamp().is_empty());
}
