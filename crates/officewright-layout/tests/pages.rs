//! Pages against Word's: which paragraph starts each page and where its first line is.
//! With the test fonts of tests/fonts/ at the repository root, which keep their own
//! licenses (THIRD-PARTY-NOTICES.md) and are not part of this crate, and Word's
//! reference renders of tests/corpus/word-rules/; so this test is not in the published
//! crate. The rule documents are in Calibri, whose metrics Carlito shares.
#![cfg(feature = "fuzzing")]

mod common;

use common::{TestResult, book, random_paragraph, word_lines};
use officewright_core::{PageSize, Pt};
use officewright_layout::fuzzing;
use officewright_layout::ir::{
    Block, Border, BorderSide, BreakClear, Cell, Document, Inline, LineSpacing, Margins, PageSetup,
    Paragraph, ParagraphProps, PreferredWidth, Row, RunFonts, RunStyle, RunStyleId, Section,
    Spacing, Table, TableBorders, TableLayout, TableProps,
};

/// How far Word's lines may be from ours: the printer's grid of 600 dots per inch and
/// the rounding of the coordinates `pdftotext` writes, as in `tests/lines.rs`.
const GRID: f64 = 0.15;

/// The page of the rule documents: A4 with margins of 1 inch.
fn a4() -> PageSetup {
    PageSetup::new(PageSize::new(Pt(11906.0 / 20.0), Pt(16838.0 / 20.0))).with_margins(
        Margins::default()
            .with_top(Pt(72.0))
            .with_bottom(Pt(72.0))
            .with_left(Pt(72.0))
            .with_right(Pt(72.0)),
    )
}

/// Exact lines of 20 pt with `before` above, as the cases of `E-space-before-top` have.
fn exact(before: f64) -> ParagraphProps {
    ParagraphProps::default().with_spacing(
        Spacing::default()
            .with_line(LineSpacing::Exact(Pt(20.0)))
            .with_before(Pt(before)),
    )
}

fn paragraph(style: RunStyleId, props: ParagraphProps, inlines: Vec<Inline>) -> Block {
    Paragraph::new(style)
        .with_props(props)
        .with_inlines(inlines)
        .into()
}

/// A table of one cell holding `block`, as `E-space-before-top` has them: as wide as the
/// text, with single borders of 0.5 pt.
fn in_table(block: Block) -> Block {
    let line = BorderSide::Line(Border::default().with_width(Pt(0.5)));
    let borders = TableBorders::default()
        .with_top(line)
        .with_start(line)
        .with_bottom(line)
        .with_end(line)
        .with_inside_horizontal(line)
        .with_inside_vertical(line);
    Table::default()
        .with_grid([Pt(451.3)])
        .with_props(
            TableProps::default()
                .with_width(PreferredWidth::Points(Pt(451.3)))
                .with_layout(TableLayout::Fixed)
                .with_borders(borders),
        )
        .with_rows(vec![
            Row::default().with_cells(vec![Cell::default().with_blocks([block])]),
        ])
        .into()
}

/// `E-space-before-top.docx` (scripts/make-rule-docs.py) in the IR, and the first words
/// of its paragraphs in reading order; `None` for the paragraphs that hold only a page
/// break, which never start a page.
fn space_before_top() -> (Document, Vec<Option<&'static str>>) {
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Calibri"))
            .with_size(Pt(11.0)),
    );
    let text = |words: &str| vec![Inline::text(words, style)];
    let case = |words: &str| paragraph(style, exact(24.0), text(words));
    let filler: Vec<Inline> = (1..34)
        .flat_map(|n| {
            let mut line = vec![Inline::text(format!("Filler line {n}"), style)];
            if n < 33 {
                line.push(Inline::line_break(BreakClear::None, style));
            }
            line
        })
        .collect();
    let first = vec![
        case("Case 1: first paragraph of the document, space before 24 pt"),
        paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::page_break(style)],
        ),
        case("Case 2: after a paragraph that holds only a page break"),
        paragraph(
            style,
            exact(24.0).with_page_break_before(true),
            text("Case 3: page break before, space before 24 pt"),
        ),
        paragraph(
            style,
            exact(0.0).with_page_break_before(true),
            text("Control: page break before, space before 0"),
        ),
        paragraph(style, exact(0.0).with_widow_control(false), filler),
        case("Case 4: flowed to a new page; the last paragraph of section 1"),
    ];
    let mut ends_in_break = text("A paragraph with text that ends in a page break.");
    ends_in_break.push(Inline::page_break(style));
    let second = vec![
        case("Case 5: first paragraph of section 2, which starts on a new page"),
        paragraph(style, ParagraphProps::default(), ends_in_break),
        case("Case 6: after a page break that ends a paragraph with text"),
        paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::page_break(style)],
        ),
        in_table(case(
            "Case 7: first paragraph of a table cell at the top of a page",
        )),
        paragraph(
            style,
            ParagraphProps::default(),
            text("Reference paragraph between the two tables, single line spacing."),
        ),
        in_table(case(
            "Case 8: first paragraph of a table cell in the middle of a page",
        )),
        paragraph(
            style,
            ParagraphProps::default(),
            text("End of the document."),
        ),
    ];
    document.add_section(Section::new(a4()).with_blocks(first));
    document.add_section(Section::new(a4()).with_blocks(second));
    let words = vec![
        Some("Case 1:"),
        None,
        Some("Case 2:"),
        Some("Case 3:"),
        Some("Control:"),
        Some("Filler"),
        Some("Case 4:"),
        Some("Case 5:"),
        Some("A"),
        Some("Case 6:"),
        None,
        Some("Case 7:"),
        Some("Reference"),
        Some("Case 8:"),
        Some("End"),
    ];
    (document, words)
}

#[test]
fn space_above_at_the_top_of_a_page_is_where_word_keeps_it() -> TestResult {
    let book = book()?;
    let (document, words) = space_before_top();
    let laid = fuzzing::paginate(&book, &document, 100)?;
    let word = word_lines("E-space-before-top")?;
    let pages = word.iter().map(|line| line.page).max().unwrap_or(0);
    assert_eq!(laid.pages().len(), pages);
    // Word's text tops are below the line's top by what Calibri reaches above its
    // baseline in an exact 20 pt line: the control paragraph, space before 0 at the top
    // of page 4, has its line's top at the margin.
    let control = word
        .iter()
        .find(|line| line.text.starts_with("Control"))
        .ok_or("no control line")?;
    let offset = control.top - 72.0;
    for (number, page) in laid.pages().iter().enumerate() {
        let first = word
            .iter()
            .find(|line| line.page == number + 1)
            .ok_or("an empty page")?;
        let fragment = page.fragments().first().ok_or("an empty page of ours")?;
        let label = words
            .get(fragment.paragraph())
            .ok_or("no such paragraph")?
            .ok_or_else(|| format!("page {} starts with a page break's paragraph", number + 1))?;
        assert!(
            first.text.starts_with(label),
            "page {}: Word starts it with {:?}, we with {label:?}",
            number + 1,
            first.text
        );
        // Page 8 starts with a table: the cell's first line is below the table's top
        // border, 0.5 pt, and the space before it, which a cell keeps at the top of a
        // page.
        let ours = if label.starts_with("Case 7") {
            fragment.top() + 0.5 + 24.0
        } else {
            fragment.top()
        };
        let theirs = first.top - offset;
        assert!(
            (ours - theirs).abs() <= GRID,
            "page {}: {label} at {ours} where Word has {theirs}",
            number + 1,
        );
    }
    Ok(())
}

#[test]
fn the_pages_of_space_before_top_keep_what_the_layout_relies_on() -> TestResult {
    let book = book()?;
    let (document, _) = space_before_top();
    fuzzing::check_pages(&book, &document, 100)?;
    Ok(())
}

/// A document built from `seed`: up to three sections of random starts, page heights and
/// numbering, each of up to five random paragraphs (`random_paragraph`) kept with the
/// next, kept together, with or without widow control, with page breaks before, a `PAGE`
/// field, some of them in tables; headers and footers of random paragraphs and page
/// fields, a title page and even-page headers some of the time. The `PAGE` fields send
/// paragraphs that land on another page through their layout again, which `check_pages`
/// checks as any other layout; what number they draw, the unit tests of `pages.rs` check.
fn random_document(seed: u64) -> Document {
    use officewright_layout::ir::{
        DocumentSettings, Field, FieldKind, HeaderFooterSet, PageNumbering, SectionStart,
    };

    let mut state = seed
        .wrapping_mul(2862933555777941757)
        .wrapping_add(3037000493);
    let mut next = move |below: u64| {
        state = state
            .wrapping_mul(2862933555777941757)
            .wrapping_add(3037000493);
        (state >> 33) % below.max(1)
    };
    let starts = [
        SectionStart::NextPage,
        SectionStart::Continuous,
        SectionStart::EvenPage,
        SectionStart::OddPage,
        SectionStart::NextColumn,
    ];
    let mut document =
        Document::new(DocumentSettings::default().with_even_and_odd_headers(next(2) == 0));
    for section in 0..1 + next(3) {
        let mut blocks = Vec::new();
        let mut width = 100.0;
        for index in 0..1 + next(5) {
            let (paragraph, column) =
                random_paragraph(seed * 31 + section * 7 + index, &mut document);
            if index == 0 {
                width = column;
            }
            let props = paragraph
                .props()
                .clone()
                .with_keep_next(next(3) == 0)
                .with_keep_lines(next(4) == 0)
                .with_widow_control(next(4) != 0)
                .with_page_break_before(next(6) == 0);
            // A page number in some, laid out again when the paragraph lands elsewhere.
            let mut inlines = paragraph.inlines().to_vec();
            if next(4) == 0 {
                let at = next(inlines.len() as u64 + 1) as usize;
                let field = Inline::Field(Field::new(FieldKind::Page, paragraph.mark()));
                inlines.insert(at.min(inlines.len()), field);
            }
            let block: Block = paragraph.with_props(props).with_inlines(inlines).into();
            blocks.push(if next(5) == 0 { in_table(block) } else { block });
        }
        let height = 10.0 + next(300) as f64;
        let setup = PageSetup::new(PageSize::new(Pt(width + 20.0), Pt(height + 20.0)))
            .with_margins(
                Margins::default()
                    .with_top(Pt(10.0))
                    .with_bottom(Pt(10.0))
                    .with_left(Pt(10.0))
                    .with_right(Pt(10.0)),
            );
        // A header and a footer: a random paragraph, or a page field, or nothing.
        let mut frame = |document: &mut Document, salt: u64| -> Vec<Block> {
            match next(3) {
                0 => Vec::new(),
                1 => {
                    let (paragraph, _) = random_paragraph(seed * 131 + salt, document);
                    vec![paragraph.into()]
                }
                _ => {
                    let style = document.add_run_style(
                        RunStyle::default()
                            .with_fonts(RunFonts::new("Calibri"))
                            .with_size(Pt(1.0 + next(30) as f64)),
                    );
                    let kinds = [
                        FieldKind::Page,
                        FieldKind::NumPages,
                        FieldKind::SectionPages,
                    ];
                    let fields = kinds
                        .into_iter()
                        .map(|kind| Inline::Field(Field::new(kind, style)));
                    vec![paragraph(
                        style,
                        ParagraphProps::default(),
                        fields.collect(),
                    )]
                }
            }
        };
        let header = frame(&mut document, section * 2 + 1);
        let footer = frame(&mut document, section * 2 + 2);
        let first = frame(&mut document, section * 2 + 3);
        let numbering = if next(4) == 0 {
            PageNumbering::default().with_start(Some(next(5) as u32))
        } else {
            PageNumbering::default()
        };
        document.add_section(
            Section::new(setup)
                .with_headers(
                    HeaderFooterSet::default()
                        .with_primary(header.clone())
                        .with_even(footer.clone())
                        .with_first(first),
                )
                .with_footers(
                    HeaderFooterSet::default()
                        .with_primary(footer)
                        .with_even(header),
                )
                .with_title_page(next(2) == 0)
                .with_start(
                    starts
                        .get(next(starts.len() as u64) as usize)
                        .copied()
                        .unwrap_or(SectionStart::NextPage),
                )
                .with_page_numbering(numbering)
                .with_blocks(blocks),
        );
    }
    document
}

#[test]
fn every_document_on_pages_keeps_what_the_painter_relies_on() -> TestResult {
    let book = book()?;
    for seed in 0..500 {
        let document = random_document(seed);
        fuzzing::check_pages(&book, &document, 512)
            .map_err(|problem| format!("seed {seed}: {problem}"))?;
    }
    Ok(())
}

#[test]
fn a_chain_that_moves_draws_its_leaders_once() -> TestResult {
    use officewright_layout::ir::{TabLeader, TabStop};

    // A line of 50 dot-leader tabs at 1 pt and 1 % (about 198,000 glyphs, under the
    // document's bound of 262,144), kept with a paragraph that does not fit: both move to
    // the next page, and the leaders drawn on the first try are not counted against the
    // bound again.
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Liberation Sans"))
            .with_size(Pt(1.0))
            .with_scale(1),
    );
    let stops = (1..=50).map(|stop| {
        TabStop::default()
            .with_position(Pt(f64::from(stop) * 11.0))
            .with_leader(TabLeader::Dot)
    });
    let mut leaders = vec![Inline::text("A", style)];
    leaders.extend((0..50).map(|_| Inline::tab(style)));
    let mut blocks: Vec<Block> = (0..6)
        .map(|_| paragraph(style, exact(0.0), vec![Inline::text("A", style)]))
        .collect();
    blocks.push(paragraph(
        style,
        exact(0.0).with_keep_next(true).with_tabs(stops),
        leaders,
    ));
    let three = vec![
        Inline::text("A", style),
        Inline::line_break(BreakClear::None, style),
        Inline::text("A", style),
        Inline::line_break(BreakClear::None, style),
        Inline::text("A", style),
    ];
    blocks.push(paragraph(style, exact(0.0).with_keep_lines(true), three));
    // A body 160 pt high and 600 pt wide: eight lines of 20 pt.
    let setup = PageSetup::new(PageSize::new(Pt(744.0), Pt(304.0)));
    document.add_section(Section::new(setup).with_blocks(blocks));
    let laid = fuzzing::paginate(&book, &document, 100)?;
    let second = laid.pages().get(1).ok_or("no second page")?;
    assert_eq!(second.fragments().first().map(|f| f.paragraph()), Some(6));
    let glyphs: usize = second
        .text()
        .lines()
        .filter(|line| line.starts_with("glyphs") && line.contains("\"\\t\""))
        .map(|line| {
            line.split(' ')
                .filter(|word| word.contains('@') && !word.starts_with('@'))
                .count()
        })
        .sum();
    assert!(glyphs > 150_000, "{glyphs}");
    assert!(
        !laid
            .report()
            .entries()
            .iter()
            .any(|entry| entry.message().contains("tab leader")),
        "{:?}",
        laid.report()
    );
    Ok(())
}
