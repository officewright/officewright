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
        Ok(()) => panic!("the document is valid"),
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
}

#[test]
fn compat_flags_map_names_both_ways() {
    for flag in CompatFlag::NAMED {
        assert_eq!(CompatFlag::from_name(flag.name()), flag);
        assert!(!matches!(flag, CompatFlag::Other(_)));
    }
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
