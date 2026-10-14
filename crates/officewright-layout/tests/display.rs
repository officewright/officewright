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
    Alignment, ColorValue, Document, ImageData, ImageFormat, Inline, InlineObject, ObjectContent,
    Paragraph, ParagraphProps, Picture, RunFonts, RunStyle, RunStyleId, Shading, ShadingPattern,
    Size, TabKind, TabLeader, TabStop, Underline, UnderlineKind, VerticalPosition,
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
    // A highlight is a rectangle behind the text, as high as the font reaches.
    let painted = paint(&book, &document, &[paragraph(highlighted)]);
    let text = painted.text();
    assert!(text.starts_with("rect @0,"), "{text}");
    let rect = items(&painted, "rect").first().copied().unwrap_or_default();
    // From the ascent above the baseline (usWinAscent 1854 of 2048) to the descent below
    // it (usWinDescent 434).
    assert_eq!(
        at(rect).1,
        BASELINE - (1854.0 / 2048.0 * 11.0 * 20.0_f64).round() as i64
    );
    assert_eq!(
        dims(rect).1,
        (2288.0 / 2048.0 * 11.0 * 20.0_f64).round() as i64
    );
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

#[test]
fn dot_leaders_fill_a_tab_and_end_at_its_stop() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(base());
    let props = ParagraphProps::default().with_tabs([TabStop::default()
        .with_position(Pt(200.0))
        .with_kind(TabKind::End)
        .with_leader(TabLeader::Dot)]);
    let paragraph = Paragraph::new(style).with_props(props).with_inlines([
        Inline::text("Chapter", style),
        Inline::tab(style),
        Inline::text("12", style),
    ]);
    let painted = paint(&book, &document, &[paragraph]);
    let runs = items(&painted, "glyphs");
    assert_eq!(runs.len(), 3, "{}", painted.text());
    let leader = runs.get(1).copied().unwrap_or_default();
    assert!(leader.contains("\"\\t\""), "{leader}");
    // The dots: the period of Liberation Sans, 569 units, 3.06 pt at 11 pt.
    let dots = glyph_xs(leader);
    assert!(dots.len() > 30);
    // Evenly spaced.
    let dot = (569.0 / 2048.0 * 11.0 * 20.0_f64).round() as i64;
    for pair in dots.windows(2) {
        let step = pair
            .get(1)
            .zip(pair.first())
            .map(|(b, a)| b - a)
            .unwrap_or_default();
        assert!((step - dot).abs() <= 1, "{dots:?}");
    }
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &Paragraph::new(style)
            .with_props(
                ParagraphProps::default().with_tabs([TabStop::default()
                    .with_position(Pt(200.0))
                    .with_kind(TabKind::End)]),
            )
            .with_inlines([
                Inline::text("Chapter", style),
                Inline::tab(style),
                Inline::text("12", style),
            ]),
        300.0,
    );
    let tab = laid
        .lines()
        .first()
        .and_then(|line| line.tabs().first())
        .copied()
        .ok_or("no tab")?;
    let origin = at(leader).0;
    let last = dots.last().copied().unwrap_or_default();
    // The last dot ends where the tab ends, within rounding to twips.
    assert!(
        (origin + last + dot - (tab.1 * 20.0).round() as i64).abs() <= 1,
        "{leader}"
    );
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
    // Liberation Sans 11 pt lines are 12.65 pt, 253 twips, high.
    assert_eq!(
        bars,
        [((400, 0), (400, 253)), ((400, 253), (400, 506))],
        "{}",
        painted.text()
    );
    Ok(())
}
