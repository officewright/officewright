//! What laid-out paragraphs draw: glyph runs, underlines, strikes, highlights, tab
//! leaders, superscripts and pictures. With the test fonts of tests/fonts/ at the
//! repository root, which keep their own licenses (THIRD-PARTY-NOTICES.md) and are not
//! part of this crate, so this test is not in the published crate.
#![cfg(feature = "fuzzing")]

use std::path::Path;

use officewright_core::{Color, Pt};
use officewright_font::FontBook;
use officewright_layout::fuzzing::{self, Painted};
use officewright_layout::ir::{
    Alignment, ColorValue, CompatFlag, Document, DocumentSettings, ImageData, ImageFormat, Inline,
    InlineObject, ObjectContent, Paragraph, ParagraphProps, Picture, RunFonts, RunStyle,
    RunStyleId, Shading, ShadingPattern, Size, TabKind, TabLeader, TabStop, Underline,
    UnderlineKind, VerticalPosition,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn book() -> TestResult<FontBook> {
    let mut book = FontBook::new();
    book.add_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fonts"))?;
    Ok(book)
}

/// Liberation Sans at 11 pt.
fn base() -> RunStyle {
    RunStyle::default()
        .with_fonts(RunFonts::new("Liberation Sans"))
        .with_size(Pt(11.0))
}

fn paint(book: &FontBook, document: &Document, paragraphs: &[Paragraph]) -> Painted {
    fuzzing::paint(book, document, paragraphs, 300.0)
}

/// The lines of the dump that start with `kind`.
fn items<'a>(painted: &'a Painted, kind: &str) -> Vec<&'a str> {
    painted
        .text()
        .lines()
        .filter(|line| line.starts_with(kind))
        .collect()
}

/// The place `@X,Y` of an item of the dump.
fn at(item: &str) -> (i64, i64) {
    let place = item
        .split(' ')
        .find_map(|word| word.strip_prefix('@'))
        .unwrap_or_default();
    pair(place, ',')
}

/// The size `WxH` of a rectangle or image of the dump.
fn dims(item: &str) -> (i64, i64) {
    let size = item
        .split(' ')
        .find(|word| word.contains('x') && word.starts_with(|ch: char| ch.is_ascii_digit()))
        .unwrap_or_default();
    pair(size, 'x')
}

/// The ends `X,Y X,Y` of a line of the dump.
fn ends(item: &str) -> ((i64, i64), (i64, i64)) {
    let mut words = item.split(' ').skip(1);
    let from = pair(words.next().unwrap_or_default(), ',');
    let to = pair(words.next().unwrap_or_default(), ',');
    (from, to)
}

fn pair(text: &str, separator: char) -> (i64, i64) {
    let (a, b) = text.split_once(separator).unwrap_or_default();
    (a.parse().unwrap_or(i64::MIN), b.parse().unwrap_or(i64::MIN))
}

/// The glyph places `ID@X[,Y]` of a glyph run of the dump: x from the run's origin.
fn glyph_xs(item: &str) -> Vec<i64> {
    item.split(' ')
        .filter_map(|word| word.split_once('@'))
        .filter(|(id, _)| !id.is_empty())
        .map(|(_, place)| {
            place
                .split(',')
                .next()
                .and_then(|x| x.parse().ok())
                .unwrap_or(i64::MIN)
        })
        .collect()
}

/// The baseline of Liberation Sans 11 pt text at the top of a column: its external
/// leading (67 units of 2048) and its ascent (`usWinAscent` 1854), 10.32 pt, in twips.
const BASELINE: i64 = 206;

fn one_paragraph(style: RunStyleId, inlines: Vec<Inline>) -> Paragraph {
    Paragraph::new(style).with_inlines(inlines)
}

#[test]
fn text_is_drawn_on_its_baseline_with_its_source_text() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(base());
    let caps = document.add_run_style(base().with_caps(true));
    let painted = paint(
        &book,
        &document,
        &[one_paragraph(
            style,
            vec![Inline::text("Hello ", style), Inline::text("world", caps)],
        )],
    );
    let runs = items(&painted, "glyphs");
    assert_eq!(runs.len(), 2, "{}", painted.text());
    let (Some(first), Some(second)) = (runs.first(), runs.get(1)) else {
        return Err("runs".into());
    };
    assert!(
        first.starts_with("glyphs Liberation Sans 220 #000000 @0,"),
        "{first}"
    );
    // The capitals are drawn, the text as written is what they stand for.
    assert!(second.contains("\"world\""), "{second}");
    // Both on the baseline.
    assert_eq!(at(first).1, BASELINE);
    assert_eq!(at(second).1, BASELINE);
    Ok(())
}

#[test]
fn capitals_that_draw_more_characters_keep_their_text_on_the_line_that_draws_it() -> TestResult {
    // In capitals, `ß` is drawn `SS`. Broken into lines, each line's glyph runs stand for
    // its own words as written, so that copying a page gives the text on it.
    let book = book()?;
    let mut document = Document::default();
    let caps = document.add_run_style(base().with_caps(true));
    let words = "straße maße fuß groß";
    let paragraph = one_paragraph(caps, vec![Inline::text(words, caps)]);
    let painted = fuzzing::paint(&book, &document, &[paragraph], 60.0);
    let runs = items(&painted, "glyphs");
    assert!(runs.len() > 1, "{}", painted.text());
    let texts: Vec<&str> = runs
        .iter()
        .map(|run| run.split('"').nth(1).unwrap_or_default())
        .collect();
    // One word a line, the space after it hanging past the end in a run of its own.
    assert_eq!(
        texts,
        ["straße", " ", "maße", " ", "fuß", " ", "groß"],
        "{}",
        painted.text()
    );
    Ok(())
}

#[test]
fn bold_and_italic_take_the_family_s_faces_or_are_drawn_synthetic() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let bold = document.add_run_style(base().with_bold(true));
    let both = document.add_run_style(base().with_bold(true).with_italic(true));
    // Noto Sans of tests/fonts has no bold or italic face.
    let noto = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Noto Sans"))
            .with_size(Pt(11.0))
            .with_bold(true)
            .with_italic(true),
    );
    let painted = paint(
        &book,
        &document,
        &[one_paragraph(
            bold,
            vec![
                Inline::text("Bold ", bold),
                Inline::text("both ", both),
                Inline::text("drawn", noto),
            ],
        )],
    );
    let runs = items(&painted, "glyphs");
    let [first, second, third] = runs.as_slice() else {
        return Err(format!("runs: {}", painted.text()).into());
    };
    assert!(
        first.starts_with("glyphs Liberation Sans Bold 220 #000000 @0,"),
        "{first}"
    );
    assert!(
        second.starts_with("glyphs Liberation Sans Bold Italic 220 "),
        "{second}"
    );
    assert!(
        !second.contains(" bold ") && !second.contains(" italic "),
        "{second}"
    );
    assert!(third.starts_with("glyphs Noto Sans 220 "), "{third}");
    assert!(third.contains(" bold italic "), "{third}");
    Ok(())
}

#[test]
fn underlines_strikes_and_highlights_follow_the_text() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let plain = document.add_run_style(base());
    let underlined = document.add_run_style(base().with_underline(Some(
        Underline::default().with_color(ColorValue::Rgb(Color::rgb(255, 0, 0))),
    )));
    let words = document.add_run_style(
        base().with_underline(Some(Underline::default().with_kind(UnderlineKind::Words))),
    );
    let double = document.add_run_style(
        base().with_underline(Some(Underline::default().with_kind(UnderlineKind::Double))),
    );
    let struck = document.add_run_style(base().with_strike(true));
    let highlighted = document.add_run_style(base().with_highlight(Some(Color::rgb(255, 255, 0))));
    let paragraph = |style| one_paragraph(plain, vec![Inline::text("two words", style)]);
    // An underline in its own colour, under the whole run, the space too.
    let painted = paint(&book, &document, &[paragraph(underlined)]);
    let lines = items(&painted, "line");
    assert_eq!(lines.len(), 1, "{}", painted.text());
    let line = lines.first().copied().unwrap_or_default();
    assert!(line.ends_with("solid #FF0000"), "{line}");
    let width = fuzzing::lay_out(&book, &document, &paragraph(underlined), 300.0)
        .lines()
        .first()
        .map_or(0.0, |line| line.width());
    let ((x1, y1), (x2, y2)) = ends(line);
    assert_eq!((x1, x2), (0, (width * 20.0).round() as i64));
    // Below the baseline, where Liberation Sans puts it: the top of the stroke 67 units
    // of 2048 down (`underlinePosition`), 150 units thick, so its middle 0.76 pt down.
    let middle = (((67.0 + 1854.0) + (67.0 + 75.0)) / 2048.0 * 11.0 * 20.0_f64).round() as i64;
    assert_eq!((y1, y2), (middle, middle));
    // Words only: two lines, the space left out.
    let painted = paint(&book, &document, &[paragraph(words)]);
    assert_eq!(items(&painted, "line").len(), 2, "{}", painted.text());
    // Double: two lines, one above the other.
    let painted = paint(&book, &document, &[paragraph(double)]);
    let lines = items(&painted, "line");
    assert_eq!(lines.len(), 2);
    let ys: Vec<i64> = lines.iter().map(|line| ends(line).0.1).collect();
    assert!(ys.first() < ys.get(1), "{ys:?}");
    // A strike where the face puts it: the top of the stroke 530 units above the
    // baseline (`yStrikeoutPosition`), 102 units thick (`yStrikeoutSize`).
    let painted = paint(&book, &document, &[paragraph(struck)]);
    let strike = items(&painted, "line").first().map(|line| ends(line).0.1);
    let middle = BASELINE - ((530.0 - 51.0) / 2048.0 * 11.0 * 20.0_f64).round() as i64;
    assert_eq!(strike, Some(middle));
    // A highlight is a rectangle behind the text, the height of its line, from its top
    // to its bottom, as Word draws it (E-highlight).
    let painted = paint(&book, &document, &[paragraph(highlighted)]);
    let text = painted.text();
    assert!(text.starts_with("rect @0,0 "), "{text}");
    let rect = items(&painted, "rect").first().copied().unwrap_or_default();
    let height = fuzzing::lay_out(&book, &document, &paragraph(highlighted), 300.0).height();
    assert_eq!(dims(rect).1, (height * 20.0).round() as i64, "{rect}");
    assert!(text.contains("#FFFF00"));
    Ok(())
}

#[test]
fn automatic_text_is_white_on_dark_shading() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let plain = document.add_run_style(base());
    let dark = document.add_run_style(base().with_shading(Some(
        Shading::default().with_fill(ColorValue::Rgb(Color::rgb(0, 0, 80))),
    )));
    let painted = paint(
        &book,
        &document,
        &[one_paragraph(
            plain,
            vec![Inline::text("light ", plain), Inline::text("dark", dark)],
        )],
    );
    let runs = items(&painted, "glyphs");
    assert!(runs.first().is_some_and(|run| run.contains("#000000")));
    assert!(
        runs.get(1).is_some_and(|run| run.contains("#FFFFFF")),
        "{}",
        painted.text()
    );
    // A shading of 20 % black on white is a light grey, and the text stays black.
    let grey = document.add_run_style(base().with_shading(Some(
        Shading::default().with_pattern(ShadingPattern::Percent20),
    )));
    let painted = paint(
        &book,
        &document,
        &[one_paragraph(plain, vec![Inline::text("grey", grey)])],
    );
    assert!(
        painted.text().contains("rect") && painted.text().contains("#CCCCCC"),
        "{}",
        painted.text()
    );
    assert!(
        items(&painted, "glyphs")
            .first()
            .is_some_and(|run| run.contains("#000000"))
    );
    // Stripes are drawn as an even tint, and reported.
    let striped = document.add_run_style(base().with_shading(Some(
        Shading::default().with_pattern(ShadingPattern::HorizontalStripe),
    )));
    let painted = paint(
        &book,
        &document,
        &[one_paragraph(plain, vec![Inline::text("x", striped)])],
    );
    assert_eq!(painted.report().len(), 1);
    Ok(())
}

/// The first leader glyph's x and the number of glyphs of each case of
/// `E-tab-leader.docx` in Word's reference, by case number: the topmost text below the
/// case's title, and above the next case's, that starts with a leader character, and the
/// run of that character it starts with. pdftotext lists the dots of case 21 last, out of
/// reading order, and joins the dots of case 22 and the "x" after them into one word.
fn word_leaders() -> TestResult<Vec<(u32, f64, usize)>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/reference/word-rules/E-tab-leader/geometry.json");
    let json = std::fs::read_to_string(path)?;
    // Every text of the file, a line's or a word's, with the left and top edges of its
    // box (the reference has one page).
    let mut texts: Vec<(String, f64, f64)> = Vec::new();
    let mut lines = json.lines();
    while let Some(line) = lines.next() {
        let Some(text) = line.trim().strip_prefix("\"text\": \"") else {
            continue;
        };
        // "box": [ then the left and top edges.
        let _ = lines.next();
        let mut edge = || -> TestResult<f64> {
            Ok(lines
                .next()
                .ok_or("no box")?
                .trim()
                .trim_end_matches(',')
                .parse()?)
        };
        let (left, top) = (edge()?, edge()?);
        texts.push((text.trim_end_matches(['"', ',']).to_owned(), left, top));
    }
    let mut titles: Vec<(u32, f64)> = texts
        .iter()
        .filter_map(|(text, _, top)| {
            text.strip_prefix("Case ")
                .and_then(|rest| rest.split_once(':'))
                .and_then(|(number, _)| number.parse::<u32>().ok())
                .map(|number| (number, *top))
        })
        .collect();
    titles.sort_by(|a, b| a.1.total_cmp(&b.1));
    titles.dedup_by_key(|title| title.0);
    let mut cases = Vec::new();
    for (index, &(number, top)) in titles.iter().enumerate() {
        let below = titles.get(index + 1).map_or(f64::INFINITY, |next| next.1);
        let leader = texts
            .iter()
            .filter(|(_, _, at)| *at > top && *at < below)
            .filter_map(|(text, left, at)| {
                let first = text.chars().next()?;
                let count = text.chars().take_while(|&ch| ch == first).count();
                (['.', '-', '\u{00B7}', '_'].contains(&first) && count > 1)
                    .then_some((*at, *left, count))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, left, count)) = leader {
            cases.push((number, left, count));
        }
    }
    Ok(cases)
}

#[test]
fn leaders_stand_on_word_s_grid_from_the_page_edge() -> TestResult {
    // E-tab-leader.docx in Carlito, Calibri's metric twin, at 11 pt: a tab from the
    // margin at 72 pt to a stop 216 pt further, then "x" ("12.34" for a decimal stop).
    // Word puts whole glyphs on a grid from the page's left edge whose pitch is the
    // glyph's advance rounded to 1/600 in (2.76 pt for the period), one in every cell
    // wholly within the tab: the first at 74.52, not at 72, and none ending past where
    // the text after it starts. At a character scale of 50 % (case 22) the advance is
    // scaled before it is rounded: a pitch of 1.44, which puts the first at 72 and draws
    // 150 glyphs, where 1.38 would draw 155. At 1 pt (case 21) the pitch is 0.24: 900
    // glyphs from 72.
    let word = word_leaders()?;
    let book = book()?;
    let mut document = Document::default();
    let carlito = RunStyle::default()
        .with_fonts(RunFonts::new("Carlito"))
        .with_size(Pt(11.0));
    let plain = document.add_run_style(carlito.clone());
    let narrow = document.add_run_style(carlito.clone().with_scale(50));
    let tiny = document.add_run_style(carlito.clone().with_size(Pt(1.0)));
    let underlined = document.add_run_style(carlito.with_underline(Some(Underline::default())));
    let leaders = [
        TabLeader::Dot,
        TabLeader::Hyphen,
        TabLeader::MiddleDot,
        TabLeader::Underscore,
        TabLeader::Heavy,
    ];
    let kinds = [
        TabKind::Start,
        TabKind::End,
        TabKind::Center,
        TabKind::Decimal,
    ];
    // Cases 1 to 20: each leader to each kind of stop, in the rule document's order; then
    // the tab at 1 pt, at a character scale of 50 % and underlined.
    let mut cases: Vec<(u32, TabLeader, TabKind, RunStyleId)> = Vec::new();
    for (n, (leader, kind)) in leaders
        .iter()
        .flat_map(|leader| kinds.iter().map(move |kind| (*leader, *kind)))
        .enumerate()
    {
        cases.push((u32::try_from(n)? + 1, leader, kind, plain));
    }
    cases.push((21, TabLeader::Dot, TabKind::Start, tiny));
    cases.push((22, TabLeader::Dot, TabKind::Start, narrow));
    cases.push((23, TabLeader::Dot, TabKind::Start, underlined));
    for (case, leader, kind, tab_style) in cases {
        let (_, left, count) = word
            .iter()
            .find(|(number, _, _)| *number == case)
            .copied()
            .ok_or(format!("no case {case} in the reference"))?;
        let after = if kind == TabKind::Decimal {
            "12.34"
        } else {
            "x"
        };
        // The scaled tab's text is scaled too in the rule document; the others' is plain.
        let text_style = if case == 22 { narrow } else { plain };
        // The column starts at the page's edge here, so the paragraph is indented by the
        // margin and the stop is 72 pt further out.
        let props = ParagraphProps::default()
            .with_indent(officewright_layout::ir::Indent::default().with_start(Pt(72.0)))
            .with_tabs([TabStop::default()
                .with_position(Pt(288.0))
                .with_kind(kind)
                .with_leader(leader)]);
        let paragraph = Paragraph::new(text_style)
            .with_props(props)
            .with_inlines([Inline::tab(tab_style), Inline::text(after, text_style)]);
        // As wide as the rule document's lines reach from the page's edge: A4 less its
        // right margin, so that "12.34" on the decimal stop (to 301.9 pt) fits.
        let painted = fuzzing::paint(&book, &document, &[paragraph], 523.3);
        let runs = items(&painted, "glyphs");
        let ours = runs
            .iter()
            .find(|run| run.contains("\"\\t\""))
            .ok_or(format!("case {case}: no leader in {}", painted.text()))?;
        let xs = glyph_xs(ours);
        // In twips: Word's PDF places text 0.024 pt right of where it lays it out (its
        // margin text starts at 72.024), and both are within 0.1 pt.
        let first = at(ours).0 + xs.first().copied().unwrap_or_default();
        assert!(
            (first as f64 - (left - 0.024) * 20.0).abs() <= 2.0,
            "case {case}: first glyph at {first} twips, Word's at {left} pt"
        );
        assert_eq!(xs.len(), count, "case {case}: {ours}");
        // The heavy leader is the underscore's glyphs, no line; only the underlined tab
        // has one, its underline.
        let lines = items(&painted, "line").len();
        assert_eq!(
            lines,
            usize::from(case == 23),
            "case {case}: {}",
            painted.text()
        );
    }
    Ok(())
}

#[test]
fn superscripts_are_raised_and_pictures_sit_on_the_baseline() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(base());
    let superscript = document.add_run_style(base().with_vertical(VerticalPosition::Superscript));
    let image = document.add_image(ImageData::new(ImageFormat::Png, Vec::new()));
    let object = InlineObject::new(
        ObjectContent::Picture(Picture::new(image)),
        Size::new(Pt(40.0), Pt(30.0)),
    );
    let painted = paint(
        &book,
        &document,
        &[one_paragraph(
            style,
            vec![
                Inline::text("x", style),
                Inline::text("2", superscript),
                Inline::Object(object),
            ],
        )],
    );
    let runs = items(&painted, "glyphs");
    let (Some(x), Some(two)) = (runs.first(), runs.get(1)) else {
        return Err("runs".into());
    };
    // A third of 11 pt up, 73 twips, and drawn at 65 %: 7.15 pt, 143 twips.
    assert_eq!(at(x).1 - at(two).1, 73);
    assert!(two.contains(" 143 "), "{two}");
    let picture = items(&painted, "image")
        .first()
        .copied()
        .unwrap_or_default();
    // 40 by 30 pt, its bottom on the baseline of the line.
    assert_eq!(dims(picture), (800, 600), "{}", painted.text());
    assert_eq!(at(picture).1 + 600, at(x).1);
    Ok(())
}

#[test]
fn a_justified_paragraph_is_drawn_as_laid_out() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(base());
    let underlined = document.add_run_style(base().with_underline(Some(Underline::default())));
    let text = "Lines of a justified paragraph fill the column but the last one, \
                and the underline of a run follows its words.";
    let paragraph = Paragraph::new(style)
        .with_props(ParagraphProps::default().with_alignment(Alignment::Both))
        .with_inlines([
            Inline::text(text, style),
            Inline::text(" Underlined.", underlined),
        ]);
    let painted = fuzzing::paint(&book, &document, &[paragraph.clone(), paragraph], 200.0);
    insta::assert_snapshot!(painted.text());
    Ok(())
}

#[test]
fn a_bar_tab_draws_a_line_through_every_line() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(base());
    let props = ParagraphProps::default().with_tabs([TabStop::default()
        .with_position(Pt(20.0))
        .with_kind(TabKind::Bar)]);
    let text = "A paragraph long enough to take two lines in a column of three hundred points, \
                with a bar tab at twenty points.";
    let paragraph = Paragraph::new(style)
        .with_props(props)
        .with_inlines([Inline::text(text, style)]);
    let painted = paint(&book, &document, &[paragraph]);
    let bars: Vec<((i64, i64), (i64, i64))> = items(&painted, "line")
        .iter()
        .map(|line| ends(line))
        .collect();
    // One line through both lines: Liberation Sans 11 pt lines are 12.65 pt, 253 twips,
    // high.
    assert_eq!(bars, [((400, 0), (400, 506))], "{}", painted.text());
    Ok(())
}

/// The run style `base` with `style` changes, added to `document`.
fn styled(document: &mut Document, style: impl Fn(RunStyle) -> RunStyle) -> RunStyleId {
    document.add_run_style(style(base()))
}

#[test]
fn a_leader_of_a_tiny_font_in_a_wide_tab_is_a_line() -> TestResult {
    // 1 pt at 1 % width: a period of Liberation Sans is 0.003 pt wide, and the tab would
    // take hundreds of thousands of them. Also in a column far wider than any page.
    let book = book()?;
    let mut document = Document::default();
    let tiny = styled(&mut document, |style| {
        style.with_size(Pt(1.0)).with_scale(1)
    });
    for (column, stop) in [(1584.0, 1500.0), (1e20, 1e19)] {
        let props = ParagraphProps::default().with_tabs([TabStop::default()
            .with_position(Pt(stop))
            .with_leader(TabLeader::Dot)]);
        let paragraph = Paragraph::new(tiny)
            .with_props(props)
            .with_inlines([Inline::text("A", tiny), Inline::tab(tiny)]);
        let painted = fuzzing::paint(&book, &document, &[paragraph], column);
        let leaders = items(&painted, "line");
        assert!(
            leaders.len() == 1 && leaders[0].contains(" dotted "),
            "{}",
            painted.text()
        );
        assert_eq!(items(&painted, "glyphs").len(), 1, "{}", painted.text());
        assert!(
            painted
                .report()
                .entries()
                .iter()
                .any(|entry| entry.message().contains("tab leader")),
            "{:?}",
            painted.report()
        );
    }
    Ok(())
}

#[test]
fn an_underlined_tab_is_underlined_but_by_a_words_only_underline() -> TestResult {
    // A fill-in line: "Name:" and an underlined tab to 200 pt.
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(base());
    let underlined = styled(&mut document, |style| {
        style.with_underline(Some(Underline::default()))
    });
    let words = styled(&mut document, |style| {
        style.with_underline(Some(Underline::default().with_kind(UnderlineKind::Words)))
    });
    let props = ParagraphProps::default().with_tabs([TabStop::default().with_position(Pt(200.0))]);
    let line = |tab: RunStyleId| {
        Paragraph::new(style)
            .with_props(props.clone())
            .with_inlines([Inline::text("Name:", style), Inline::tab(tab)])
    };
    let painted = paint(&book, &document, &[line(underlined)]);
    let lines = items(&painted, "line");
    let [line_item] = lines.as_slice() else {
        return Err(painted.text().into());
    };
    let ((_, _), (end, _)) = ends(line_item);
    assert_eq!(end, 4000, "{line_item}");
    let painted = paint(&book, &document, &[line(words)]);
    assert!(items(&painted, "line").is_empty(), "{}", painted.text());
    Ok(())
}

#[test]
fn hanging_spaces_have_no_highlight_and_the_underline_only_with_ul_trail_space() -> TestResult {
    let book = book()?;
    let paint_with = |flags: Vec<CompatFlag>| {
        let mut document = Document::new(DocumentSettings::default().with_compat_flags(flags));
        let marked = styled(&mut document, |style| {
            style
                .with_highlight(Some(Color::rgb(255, 255, 0)))
                .with_underline(Some(Underline::default()))
        });
        let text = format!("{}{}", "word ".repeat(30), " ".repeat(35));
        let justified = ParagraphProps::default().with_alignment(Alignment::Both);
        let paragraph = Paragraph::new(marked)
            .with_props(justified)
            .with_inlines([Inline::text(text, marked)]);
        fuzzing::paint(&book, &document, &[paragraph], 100.0)
    };
    let painted = paint_with(Vec::new());
    assert!(!items(&painted, "rect").is_empty(), "{}", painted.text());
    for rect in items(&painted, "rect") {
        let ((x, _), (width, _)) = (at(rect), dims(rect));
        assert!(x + width <= 2000, "{rect}\n{}", painted.text());
    }
    for line in items(&painted, "line") {
        let ((_, _), (end, _)) = ends(line);
        assert!(end <= 2000, "{line}\n{}", painted.text());
    }
    // With `ulTrailSpace`, the hanging spaces are underlined too.
    let painted = paint_with(vec![CompatFlag::from_name("ulTrailSpace")]);
    assert!(
        items(&painted, "line")
            .iter()
            .any(|line| ends(line).1.0 > 2000),
        "{}",
        painted.text()
    );
    Ok(())
}

#[test]
fn a_words_only_underline_covers_the_hyphen_at_a_soft_hyphen() -> TestResult {
    // A paragraph that starts with a space: the hyphen is still part of its word.
    let book = book()?;
    let mut document = Document::default();
    let words = styled(&mut document, |style| {
        style.with_underline(Some(Underline::default().with_kind(UnderlineKind::Words)))
    });
    let paragraph = Paragraph::new(words).with_inlines([
        Inline::text(" Extraordinarily", words),
        Inline::soft_hyphen(words),
        Inline::text("longwordsplit here", words),
    ]);
    let painted = fuzzing::paint(&book, &document, &[paragraph], 80.0);
    let first_line: Vec<&str> = items(&painted, "line")
        .into_iter()
        .filter(|line| ends(line).0.1 < 253)
        .collect();
    // The hyphen drawn at the soft hyphen: a run that stands for no text.
    let hyphen_run = items(&painted, "glyphs")
        .into_iter()
        .find(|run| run.contains(" \"\" "))
        .ok_or_else(|| format!("no hyphen:\n{}", painted.text()))?;
    let hyphen_x = at(hyphen_run).0;
    assert!(
        first_line.iter().any(|line| {
            let ((from, _), (to, _)) = ends(line);
            from <= hyphen_x && to > hyphen_x
        }),
        "{}",
        painted.text()
    );
    Ok(())
}

#[test]
fn the_leaders_of_a_document_take_a_bounded_number_of_glyphs() -> TestResult {
    // 1 pt at 43 %, whose period stands on Word's smallest grid pitch of 0.12 pt, stops
    // every 11 pt with dot leaders: each of the 143 tabs a line holds stays under the
    // bound of one leader, but together the leaders of 25 such lines would take 325,000
    // glyphs.
    let book = book()?;
    let mut document = Document::default();
    let tiny = styled(&mut document, |style| {
        style.with_size(Pt(1.0)).with_scale(43)
    });
    let stops = (1..=143).map(|stop| {
        TabStop::default()
            .with_position(Pt(f64::from(stop) * 11.0))
            .with_leader(TabLeader::Dot)
    });
    let props = ParagraphProps::default().with_tabs(stops);
    let mut inlines = Vec::new();
    for line in 0..25 {
        if line > 0 {
            inlines.push(Inline::line_break(
                officewright_layout::ir::BreakClear::None,
                tiny,
            ));
        }
        inlines.push(Inline::text("A", tiny));
        inlines.extend((0..143).map(|_| Inline::tab(tiny)));
    }
    let paragraph = Paragraph::new(tiny).with_props(props).with_inlines(inlines);
    let painted = fuzzing::paint(&book, &document, &[paragraph], 1584.0);
    let glyphs: usize = items(&painted, "glyphs")
        .iter()
        .filter(|run| run.contains("\"\\t\""))
        .map(|run| glyph_xs(run).len())
        .sum();
    assert!(glyphs > 0 && glyphs <= 1 << 18, "{glyphs}");
    assert!(
        items(&painted, "line")
            .iter()
            .any(|line| line.contains(" dotted ")),
        "no leader drawn as a line"
    );
    assert!(
        painted
            .report()
            .entries()
            .iter()
            .any(|entry| entry.message().contains("tab leader")),
        "{:?}",
        painted.report()
    );
    Ok(())
}

#[test]
fn leaders_past_the_bound_are_lines_of_their_kind() -> TestResult {
    // In a tab of 1,500 pt at 1 pt and 43 %, every leader would take 12,000 glyphs: a
    // dotted line for the dots, dashed for the hyphens, solid and no thicker for the
    // underscore and the heavy leader.
    let book = book()?;
    let mut document = Document::default();
    let tiny = styled(&mut document, |style| {
        style.with_size(Pt(1.0)).with_scale(43)
    });
    let leader_line = |leader: TabLeader| {
        let props = ParagraphProps::default().with_tabs([TabStop::default()
            .with_position(Pt(1500.0))
            .with_leader(leader)]);
        let paragraph = Paragraph::new(tiny)
            .with_props(props)
            .with_inlines([Inline::text("A", tiny), Inline::tab(tiny)]);
        let painted = fuzzing::paint(&book, &document, &[paragraph], 1584.0);
        items(&painted, "line")
            .first()
            .map(|line| (*line).to_owned())
            .unwrap_or_default()
    };
    let width = |line: &str| -> i64 {
        line.split(' ')
            .nth(3)
            .and_then(|width| width.parse().ok())
            .unwrap_or(-1)
    };
    assert!(leader_line(TabLeader::Dot).contains(" dotted "));
    assert!(leader_line(TabLeader::MiddleDot).contains(" dotted "));
    assert!(leader_line(TabLeader::Hyphen).contains(" dashed "));
    let (thin, heavy) = (
        leader_line(TabLeader::Underscore),
        leader_line(TabLeader::Heavy),
    );
    assert!(
        thin.contains(" solid ") && heavy.contains(" solid "),
        "{thin} {heavy}"
    );
    assert!(
        width(&thin) >= 0 && width(&thin) == width(&heavy),
        "{thin} {heavy}"
    );
    Ok(())
}

#[test]
fn a_tab_in_complex_script_text_has_the_complex_script_font() -> TestResult {
    // A right-to-left style: its tab's leader is drawn in its complex-script font and
    // size, as its line measured it.
    let book = book()?;
    let mut document = Document::default();
    let style = styled(&mut document, |style| {
        style
            .with_rtl(true)
            .with_fonts(RunFonts::new("Liberation Sans").with_complex_script("Liberation Serif"))
            .with_size_complex_script(Pt(20.0))
    });
    let props = ParagraphProps::default().with_tabs([TabStop::default()
        .with_position(Pt(200.0))
        .with_leader(TabLeader::Dot)]);
    let paragraph = Paragraph::new(style)
        .with_props(props)
        .with_inlines([Inline::text("A", style), Inline::tab(style)]);
    let painted = paint(&book, &document, &[paragraph]);
    let leader = items(&painted, "glyphs")
        .into_iter()
        .find(|run| run.contains("\"\\t\""))
        .ok_or_else(|| painted.text().to_owned())?;
    assert!(
        leader.starts_with("glyphs Liberation Serif 400 "),
        "{leader}"
    );
    Ok(())
}

#[test]
fn text_whose_drawn_and_source_bytes_differ_keeps_its_text() -> TestResult {
    // In capitals, `ı` (two bytes) is drawn `I` (one) and `ſ` (two) `S` (one): as many
    // characters, fewer bytes.
    let book = book()?;
    let mut document = Document::default();
    let caps = document.add_run_style(base().with_caps(true));
    let plain = document.add_run_style(base());
    // A run after it, of plain text, has its own text too.
    let inlines = vec![
        Inline::text("ıſxyz ıſ", caps),
        Inline::text(" and plain words after", plain),
    ];
    let paragraph = one_paragraph(caps, inlines);
    let painted = paint(&book, &document, &[paragraph]);
    let texts: Vec<&str> = items(&painted, "glyphs")
        .iter()
        .map(|run| run.split('"').nth(1).unwrap_or_default())
        .collect();
    assert_eq!(
        texts.concat(),
        "ıſxyz ıſ and plain words after",
        "{}",
        painted.text()
    );
    Ok(())
}

#[test]
fn the_leader_bound_is_shared_by_the_paragraphs_of_a_document() -> TestResult {
    // Three paragraphs of eight lines of 143 dot-leader tabs at 1 pt and 43 %, on the
    // smallest grid pitch: each alone stays under the bound, together they would pass it.
    let book = book()?;
    let mut document = Document::default();
    let tiny = styled(&mut document, |style| {
        style.with_size(Pt(1.0)).with_scale(43)
    });
    let stops: Vec<TabStop> = (1..=143)
        .map(|stop| {
            TabStop::default()
                .with_position(Pt(f64::from(stop) * 11.0))
                .with_leader(TabLeader::Dot)
        })
        .collect();
    let paragraph = || {
        let mut inlines = Vec::new();
        for line in 0..8 {
            if line > 0 {
                inlines.push(Inline::line_break(
                    officewright_layout::ir::BreakClear::None,
                    tiny,
                ));
            }
            inlines.push(Inline::text("A", tiny));
            inlines.extend((0..143).map(|_| Inline::tab(tiny)));
        }
        Paragraph::new(tiny)
            .with_props(ParagraphProps::default().with_tabs(stops.clone()))
            .with_inlines(inlines)
    };
    let leader_glyphs = |painted: &Painted| -> usize {
        items(painted, "glyphs")
            .iter()
            .filter(|run| run.contains("\"\\t\""))
            .map(|run| glyph_xs(run).len())
            .sum()
    };
    let one = fuzzing::paint(&book, &document, &[paragraph()], 1584.0);
    let alone = leader_glyphs(&one);
    assert!(alone > (1 << 18) / 3 && alone <= 1 << 18, "{alone}");
    let three = fuzzing::paint(
        &book,
        &document,
        &[paragraph(), paragraph(), paragraph()],
        1584.0,
    );
    let together = leader_glyphs(&three);
    assert!(together <= 1 << 18, "{together}");
    Ok(())
}
