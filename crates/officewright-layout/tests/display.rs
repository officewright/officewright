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
    // 1 pt at 1 %, stops every 11 pt with dot leaders: each of 143 tabs a line holds
    // stays under the bound of one leader, but together the leaders of three such lines
    // would take 1.7 million glyphs.
    let book = book()?;
    let mut document = Document::default();
    let tiny = styled(&mut document, |style| {
        style.with_size(Pt(1.0)).with_scale(1)
    });
    let stops = (1..=143).map(|stop| {
        TabStop::default()
            .with_position(Pt(f64::from(stop) * 11.0))
            .with_leader(TabLeader::Dot)
    });
    let props = ParagraphProps::default().with_tabs(stops);
    let mut inlines = Vec::new();
    for line in 0..3 {
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
fn line_leaders_are_dashed_for_hyphens_and_heavy_ones_twice_as_thick() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(base());
    let tiny = styled(&mut document, |style| {
        style.with_size(Pt(1.0)).with_scale(1)
    });
    let leader_line = |leader: TabLeader, style: RunStyleId| {
        let props = ParagraphProps::default().with_tabs([TabStop::default()
            .with_position(Pt(1500.0))
            .with_leader(leader)]);
        let paragraph = Paragraph::new(style)
            .with_props(props)
            .with_inlines([Inline::text("A", style), Inline::tab(style)]);
        let painted = fuzzing::paint(&book, &document, &[paragraph], 1584.0);
        items(&painted, "line")
            .first()
            .map(|line| (*line).to_owned())
            .unwrap_or_default()
    };
    // Too long in a tiny font: a hyphen leader is a dashed line.
    assert!(leader_line(TabLeader::Hyphen, tiny).contains(" dashed "));
    // The heavy leader is twice as thick as the underscore leader.
    let width = |line: String| -> i64 {
        line.split(' ')
            .nth(3)
            .and_then(|width| width.parse().ok())
            .unwrap_or(-1)
    };
    let (thin, heavy) = (
        width(leader_line(TabLeader::Underscore, style)),
        width(leader_line(TabLeader::Heavy, style)),
    );
    assert!(thin > 0 && (heavy - 2 * thin).abs() <= 1, "{thin} {heavy}");
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
    // Three paragraphs of 50 dot-leader tabs at 1 pt and 1 %: each alone stays under the
    // bound, together they would pass it.
    let book = book()?;
    let mut document = Document::default();
    let tiny = styled(&mut document, |style| {
        style.with_size(Pt(1.0)).with_scale(1)
    });
    let stops: Vec<TabStop> = (1..=50)
        .map(|stop| {
            TabStop::default()
                .with_position(Pt(f64::from(stop) * 11.0))
                .with_leader(TabLeader::Dot)
        })
        .collect();
    let paragraph = || {
        let mut inlines = vec![Inline::text("A", tiny)];
        inlines.extend((0..50).map(|_| Inline::tab(tiny)));
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
