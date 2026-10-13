//! Lines of paragraphs against Word's: where lines end, how high they are, where their
//! words and tabs are. With the test fonts of tests/fonts/ at the repository root, which
//! keep their own licenses (THIRD-PARTY-NOTICES.md) and are not part of this crate, and
//! Word's reference renders of tests/corpus/word-rules/; so this test is not in the
//! published crate. The rule documents are in Calibri, whose metrics Carlito shares.
#![cfg(feature = "fuzzing")]

use std::path::{Path, PathBuf};

use officewright_core::Pt;
use officewright_font::FontBook;
use officewright_layout::fuzzing::{self, LaidOut};
use officewright_layout::ir::{
    Document, Indent, Inline, LineSpacing, Paragraph, ParagraphProps, RunFonts, RunStyle,
    RunStyleId, Spacing, TabStop,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The width of the text of the rule documents: A4 with 1 inch margins.
const TEXT_WIDTH: f64 = (11906.0 - 2.0 * 1440.0) / 20.0;
/// The left margin of the rule documents.
const MARGIN: f64 = 72.0;
/// How far Word's lines may be from ours: the printer's grid of 600 dots per inch, on
/// which two lines' places are rounded, and the rounding of the coordinates `pdftotext`
/// writes.
const GRID: f64 = 0.15;
/// How far Word's words may be from ours along a line, from the line's first word: Word
/// starts each word on the printer's grid (the glyphs within it keep their advances
/// unrounded), which along whole lines of `E-tab-hanging` keeps them within 0.15 pt of
/// Carlito's advances.
const WORD_GRID: f64 = 0.2;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn book() -> TestResult<FontBook> {
    let mut book = FontBook::new();
    book.add_dir(repository().join("tests/fonts"))?;
    Ok(book)
}

/// A line of Word's reference render: its text and the boxes of its words, in points
/// from the top left of the page.
struct WordLine {
    page: usize,
    text: String,
    top: f64,
    words: Vec<(String, f64, f64)>,
}

/// The lines of `geometry.json` of rule document `name`.
fn word_lines(name: &str) -> TestResult<Vec<WordLine>> {
    let path = repository()
        .join("tests/corpus/reference/word-rules")
        .join(name)
        .join("geometry.json");
    let json = std::fs::read_to_string(path)?;
    let value: Json = Json::parse(&json)?;
    let mut lines = Vec::new();
    for page in value.get("pages")?.array()? {
        let number = page.get("number")?.number()? as usize;
        for line in page.get("lines")?.array()? {
            let words = line
                .get("words")?
                .array()?
                .iter()
                .map(|word| -> TestResult<(String, f64, f64)> {
                    let bbox = word.get("box")?.array()?;
                    let x = |index: usize| -> TestResult<f64> {
                        bbox.get(index).ok_or("short box")?.number()
                    };
                    Ok((word.get("text")?.string()?.to_owned(), x(0)?, x(2)?))
                })
                .collect::<TestResult<Vec<_>>>()?;
            let bbox = line.get("box")?.array()?;
            let top = bbox.get(1).ok_or("short box")?.number()?;
            let text = line.get("text")?.string()?.to_owned();
            // pdftotext cuts a line at a wide gap, such as a tab's, into lines of the
            // same top: they are one line.
            if let Some(last) = lines
                .last_mut()
                .filter(|last: &&mut WordLine| last.page == number && (last.top - top).abs() < 0.5)
            {
                last.text = format!("{} {text}", last.text);
                last.words.extend(words);
                last.words.sort_by(|a, b| a.1.total_cmp(&b.1));
                continue;
            }
            lines.push(WordLine {
                page: number,
                text,
                top,
                words,
            });
        }
    }
    Ok(lines)
}

/// A document with Calibri 11, the rule documents' default, and its run style.
fn calibri(size: f64) -> (Document, RunStyleId) {
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Calibri"))
            .with_size(Pt(size))
            .with_size_complex_script(Pt(size)),
    );
    (document, style)
}

fn lay_out(book: &FontBook, document: &Document, paragraph: &Paragraph) -> LaidOut {
    fuzzing::lay_out(book, document, paragraph, TEXT_WIDTH)
}

/// A paragraph of `lines`, separated by line breaks, with `spacing`.
fn broken_lines(style: RunStyleId, lines: &[String], spacing: LineSpacing) -> Paragraph {
    let mut inlines = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            inlines.push(Inline::line_break(Default::default(), style));
        }
        inlines.push(Inline::text(line.clone(), style));
    }
    Paragraph::new(style)
        .with_props(ParagraphProps::default().with_spacing(Spacing::default().with_line(spacing)))
        .with_inlines(inlines)
}

/// The baselines of the lines of `paragraphs`, laid out one under the other from 0.
fn baselines(book: &FontBook, document: &Document, paragraphs: &[Paragraph]) -> Vec<f64> {
    let mut top = 0.0;
    let mut baselines = Vec::new();
    for paragraph in paragraphs {
        for line in lay_out(book, document, paragraph).lines() {
            baselines.push(top + line.baseline());
            top += line.height();
        }
    }
    baselines
}

/// Checks the line spacing of rule document `name` (`E-line-exact` and the others):
/// a reference line, then for each value a paragraph of four lines and a reference
/// line, all in Calibri 11; Word's lines must be as far apart as ours.
fn check_line_rule(
    name: &str,
    label: &str,
    values: &[(f64, LineSpacing)],
    tolerance: f64,
) -> TestResult {
    let book = book()?;
    let word = word_lines(name)?;
    let (document, style) = calibri(11.0);
    let single = LineSpacing::Multiple(1.0);
    let mut paragraphs = vec![broken_lines(
        style,
        &["Reference line above the first case, single line spacing.".to_owned()],
        single,
    )];
    for &(value, spacing) in values {
        let case = (1..=4)
            .map(|n| format!("{label} {value}, line {n}: Hamburgefonts gjpqy"))
            .collect::<Vec<_>>();
        paragraphs.push(broken_lines(style, &case, spacing));
        paragraphs.push(broken_lines(
            style,
            &[format!("Reference line after {label} {value}.")],
            single,
        ));
    }
    let ours = baselines(&book, &document, &paragraphs);
    // Word's lines of the cases, on its first page, in order; the tops of lines of one
    // font and size are their baselines moved by the same amount.
    // Only the texts differ from ours, which does not move the lines.
    let tops: Vec<f64> = word
        .iter()
        .take_while(|line| line.page == 1 && !line.text.contains(" with a"))
        .map(|line| line.top)
        .collect();
    assert!(tops.len() >= 10, "{name}: {} lines of Word's", tops.len());
    for (index, (pair, word_pair)) in ours.windows(2).zip(tops.windows(2)).enumerate() {
        let (Some(a), Some(b), Some(c), Some(d)) = (
            pair.first(),
            pair.get(1),
            word_pair.first(),
            word_pair.get(1),
        ) else {
            continue;
        };
        let (ours, words) = (b - a, d - c);
        assert!(
            (ours - words).abs() <= tolerance,
            "{name}: line {} is {ours:.3} pt below the one before, Word's {words:.3}",
            index + 1
        );
    }
    Ok(())
}

#[test]
fn exact_line_spacing_is_words() -> TestResult {
    // Word's baseline of 8 pt lines is 0.15 to 0.19 pt lower than 80 % of the height,
    // which fits its 12, 20 and 30 pt lines within 0.05 pt: open until more sizes are
    // rendered, so the tolerance here is wider than the printer's grid.
    let values = [8.0, 12.0, 20.0, 30.0].map(|pt| (pt, LineSpacing::Exact(Pt(pt))));
    check_line_rule("E-line-exact", "exact", &values, 0.2)
}

#[test]
fn at_least_line_spacing_is_words() -> TestResult {
    let values = [6.0, 12.0, 14.0, 20.0, 30.0].map(|pt| (pt, LineSpacing::AtLeast(Pt(pt))));
    check_line_rule("E-line-at-least", "atLeast", &values, GRID)
}

#[test]
fn multiple_line_spacing_is_words() -> TestResult {
    let values = [120.0, 200.0, 240.0, 259.0, 276.0, 360.0, 480.0, 720.0]
        .map(|line| (line, LineSpacing::Multiple(line / 240.0)));
    check_line_rule("E-line-multiple", "multiple", &values, GRID)
}

#[test]
fn tabs_and_the_wrapped_line_are_where_word_puts_them() -> TestResult {
    let book = book()?;
    let word = word_lines("E-tab-hanging")?;
    let (document, style) = calibri(11.0);
    let rest = " The text after the tab shows where the tab ended, and the wrapped second line \
                shows where the hanging indent puts the text of the following lines.";
    let cases: [(&str, &str, Indent, Vec<TabStop>); 5] = [
        (
            "Case A: typed label, hanging indent 50 pt, no custom tab stops.",
            "1.1",
            Indent::default()
                .with_start(Pt(50.0))
                .with_first_line(Pt(-50.0)),
            Vec::new(),
        ),
        (
            "Case C: typed label wider than the 50 pt hanging indent.",
            "Clause 12.3.4",
            Indent::default()
                .with_start(Pt(50.0))
                .with_first_line(Pt(-50.0)),
            Vec::new(),
        ),
        (
            "Case D: typed label, hanging indent 50 pt, custom tab stop at 75 pt.",
            "1.1",
            Indent::default()
                .with_start(Pt(50.0))
                .with_first_line(Pt(-50.0)),
            vec![TabStop::default().with_position(Pt(75.0))],
        ),
        (
            "Case E: typed label, first-line indent 25 pt, no hanging indent.",
            "1.1",
            Indent::default().with_first_line(Pt(25.0)),
            Vec::new(),
        ),
        (
            "Case F: typed label, left indent 100 pt, hanging 50 pt.",
            "1.1",
            Indent::default()
                .with_start(Pt(100.0))
                .with_first_line(Pt(-50.0)),
            Vec::new(),
        ),
    ];
    for (case, label, indent, tabs) in cases {
        let paragraph = Paragraph::new(style)
            .with_props(
                ParagraphProps::default()
                    .with_indent(indent)
                    .with_tabs(tabs),
            )
            .with_inlines([
                Inline::text(label, style),
                Inline::tab(style),
                Inline::text(format!("{case}{rest}"), style),
            ]);
        let ours = lay_out(&book, &document, &paragraph);
        let words_line = word
            .iter()
            .position(|line| line.text.contains(&case[..7]))
            .ok_or("case not in Word's render")?;
        // Word's lines of the case: the label's line, which holds the case's first
        // words, and the next.
        let theirs: Vec<&WordLine> = word.iter().skip(words_line).take(2).collect();
        let lines = ours.lines();
        assert!(lines.len() >= 2, "{case}: {} lines", lines.len());
        for (line, word_line) in lines.iter().zip(&theirs) {
            let ours: Vec<(&str, f64)> = line
                .words()
                .iter()
                .map(|word| (word.text(), word.start() + MARGIN))
                .collect();
            // The first word where Word's is, the others as far from it as Word's.
            let (Some(first), Some(word_first)) = (ours.first(), word_line.words.first()) else {
                continue;
            };
            assert!(
                (first.1 - word_first.1).abs() <= GRID,
                "{case}: the line starts at {:.2}, Word's at {:.2}",
                first.1,
                word_first.1
            );
            for (word, (text, start, _)) in ours.iter().zip(&word_line.words) {
                assert_eq!(word.0, text, "{case}: words of a line");
                let (ours, theirs) = (word.1 - first.1, start - word_first.1);
                assert!(
                    (ours - theirs).abs() <= WORD_GRID,
                    "{case}: {:?} {ours:.2} pt after the line's start, Word's {theirs:.2}",
                    word.0
                );
            }
            assert_eq!(
                line.words().len(),
                word_line.words.len(),
                "{case}: the line \"{}\" ends where Word's \"{}\" does",
                line.text(),
                word_line.text
            );
        }
    }
    Ok(())
}

#[test]
fn advances_are_not_rounded() -> TestResult {
    // E-advance-rounding: 100 'i', 100 '.', 30 'm' and 50 '0' in Calibri at four sizes;
    // rounding each advance to twips would make 100 'i' at 10 pt 0.67 pt wider.
    let book = book()?;
    let word = word_lines("E-advance-rounding-15")?;
    for line in &word {
        let Some((run, label)) = line.text.split_once(' ') else {
            continue;
        };
        if !label.starts_with("Calibri") {
            continue;
        }
        let size: f64 = label
            .trim_start_matches("Calibri ")
            .split(' ')
            .next()
            .ok_or("no size")?
            .parse()?;
        let (document, style) = calibri(size);
        let paragraph = Paragraph::new(style).with_inlines([Inline::text(run, style)]);
        let ours = lay_out(&book, &document, &paragraph);
        let ours = ours
            .lines()
            .first()
            .and_then(|line| line.words().first())
            .ok_or("no word")?;
        let theirs = line.words.first().ok_or("no word")?;
        let (ours, theirs) = (ours.end() - ours.start(), theirs.2 - theirs.1);
        assert!(
            (ours - theirs).abs() <= 0.4,
            "{label}: {ours:.3} pt wide, Word's {theirs:.3}"
        );
    }
    Ok(())
}

/// Just enough JSON for `geometry.json`.
#[derive(Debug)]
enum Json {
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
    Other,
}

impl Json {
    fn parse(text: &str) -> TestResult<Self> {
        let mut chars = text.chars().peekable();
        let value = Self::value(&mut chars)?;
        Ok(value)
    }

    fn value(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> TestResult<Self> {
        Self::skip(chars);
        match chars.peek().copied().ok_or("end of json")? {
            '{' => {
                chars.next();
                let mut members = Vec::new();
                loop {
                    Self::skip(chars);
                    if chars.peek() == Some(&'}') {
                        chars.next();
                        break;
                    }
                    let Self::String(key) = Self::value(chars)? else {
                        return Err("key".into());
                    };
                    Self::skip(chars);
                    if chars.next() != Some(':') {
                        return Err("colon".into());
                    }
                    members.push((key, Self::value(chars)?));
                    Self::skip(chars);
                    if chars.peek() == Some(&',') {
                        chars.next();
                    }
                }
                Ok(Self::Object(members))
            }
            '[' => {
                chars.next();
                let mut items = Vec::new();
                loop {
                    Self::skip(chars);
                    if chars.peek() == Some(&']') {
                        chars.next();
                        break;
                    }
                    items.push(Self::value(chars)?);
                    Self::skip(chars);
                    if chars.peek() == Some(&',') {
                        chars.next();
                    }
                }
                Ok(Self::Array(items))
            }
            '"' => {
                chars.next();
                let mut string = String::new();
                while let Some(ch) = chars.next() {
                    match ch {
                        '"' => return Ok(Self::String(string)),
                        '\\' => match chars.next().ok_or("escape")? {
                            'u' => {
                                let hex: String = chars.by_ref().take(4).collect();
                                let code = u32::from_str_radix(&hex, 16)?;
                                string.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                            }
                            'n' => string.push('\n'),
                            't' => string.push('\t'),
                            other => string.push(other),
                        },
                        other => string.push(other),
                    }
                }
                Err("unterminated string".into())
            }
            ch if ch == '-' || ch.is_ascii_digit() => {
                let mut number = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch == '-'
                        || ch == '+'
                        || ch == '.'
                        || ch == 'e'
                        || ch == 'E'
                        || ch.is_ascii_digit()
                    {
                        number.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                Ok(Self::Number(number.parse()?))
            }
            _ => {
                while chars.peek().is_some_and(|ch| ch.is_ascii_alphabetic()) {
                    chars.next();
                }
                Ok(Self::Other)
            }
        }
    }

    fn skip(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
        while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
            chars.next();
        }
    }

    fn get(&self, key: &str) -> TestResult<&Self> {
        match self {
            Self::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
                .ok_or_else(|| format!("no {key}").into()),
            _ => Err(format!("not an object looking for {key}").into()),
        }
    }

    fn array(&self) -> TestResult<&[Self]> {
        match self {
            Self::Array(items) => Ok(items),
            _ => Err("not an array".into()),
        }
    }

    fn number(&self) -> TestResult<f64> {
        match self {
            Self::Number(number) => Ok(*number),
            _ => Err("not a number".into()),
        }
    }

    fn string(&self) -> TestResult<&str> {
        match self {
            Self::String(string) => Ok(string),
            _ => Err("not a string".into()),
        }
    }
}

/// A document whose text is in Liberation Sans at `size`, and that style.
fn liberation(size: f64) -> (Document, RunStyleId) {
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Liberation Sans"))
            .with_size(Pt(size)),
    );
    (document, style)
}

/// A paragraph of `inlines` with `props`.
fn paragraph(style: RunStyleId, props: ParagraphProps, inlines: Vec<Inline>) -> Paragraph {
    Paragraph::new(style)
        .with_props(props)
        .with_inlines(inlines)
}

/// The width of `text` in `style` of `document` on a line of its own.
fn width_of(book: &FontBook, document: &Document, style: RunStyleId, text: &str) -> f64 {
    let alone = paragraph(
        style,
        ParagraphProps::default(),
        vec![Inline::text(text, style)],
    );
    fuzzing::lay_out(book, document, &alone, 10_000.0)
        .lines()
        .first()
        .map_or(0.0, |line| line.width())
}

const SENTENCE: &str = "The quick brown fox jumps over the lazy dog and keeps running \
                        through the long grass until the evening comes over the hills.";

#[test]
fn lines_end_at_the_last_word_that_fits_and_spaces_hang() -> TestResult {
    let book = book()?;
    let (document, style) = liberation(11.0);
    let width = 150.0;
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::text(SENTENCE, style)],
        ),
        width,
    );
    let lines = laid.lines();
    assert!(lines.len() > 3);
    let text: String = lines.iter().map(|line| line.text()).collect();
    assert_eq!(text, SENTENCE, "every character is on one line");
    for pair in lines.windows(2) {
        let (Some(line), Some(next)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        assert!(
            line.width() <= width,
            "{:?} is {} wide",
            line.text(),
            line.width()
        );
        assert_eq!(line.end(), "wrap");
        // The space at the end hangs: the width ends at the last word.
        assert!(line.text().ends_with(' '));
        let last = line.words().last().ok_or("no word")?;
        assert!((line.width() - last.end()).abs() < 1e-9);
        // The next line's first word would not have fitted.
        let first = next.words().first().ok_or("no word")?;
        let space = width_of(&book, &document, style, " ");
        assert!(line.width() + space + (first.end() - first.start()) > width);
    }
    assert_eq!(lines.last().map(|line| line.end()), Some("paragraph"));
    // Many spaces at the end of a line hang past it, however many.
    let spaces = format!("word{} next", " ".repeat(200));
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::text(spaces, style)],
        ),
        width,
    );
    let first = laid.lines().first().ok_or("no line")?;
    assert_eq!(first.words().len(), 1);
    assert!(first.width() < 30.0);
    assert!(laid.report().is_empty());
    Ok(())
}

#[test]
fn a_word_wider_than_the_line_breaks_where_it_reaches_the_end() -> TestResult {
    let book = book()?;
    let (document, style) = liberation(11.0);
    let word = "Donaudampfschifffahrtsgesellschaftskapitän";
    let width = width_of(&book, &document, style, word) / 2.5;
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::text(word, style)],
        ),
        width,
    );
    let lines = laid.lines();
    assert_eq!(lines.len(), 3);
    for line in lines {
        assert!(line.width() <= width + 1e-9);
    }
    // Each line is full: the next character would not have fitted.
    let first = lines.first().ok_or("no line")?;
    let next_char = lines
        .get(1)
        .and_then(|line| line.text().chars().next())
        .ok_or("no next")?;
    let next_width = width_of(&book, &document, style, &next_char.to_string());
    assert!(first.width() + next_width > width);
    assert_eq!(
        lines.iter().map(|line| line.text()).collect::<String>(),
        word
    );
    // A word fits on no line of a column narrower than its first character: one
    // character a line, and the overflow is reported.
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::text("WW", style)],
        ),
        1.0,
    );
    assert_eq!(laid.lines().len(), 2);
    let kinds: Vec<_> = laid.report().iter().map(|entry| entry.kind()).collect();
    assert_eq!(kinds, [officewright_layout::ReportKind::Overflow]);
    Ok(())
}

#[test]
fn a_line_that_ends_at_a_soft_hyphen_shows_a_hyphen() -> TestResult {
    let book = book()?;
    let (document, style) = liberation(11.0);
    let text = "Donau\u{00AD}dampf\u{00AD}schiff";
    let head = width_of(&book, &document, style, "Donaudampf-");
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::text(text, style)],
        ),
        head + 1.0,
    );
    let lines: Vec<&str> = laid.lines().iter().map(|line| line.text()).collect();
    assert_eq!(lines, ["Donau\u{00AD}dampf\u{00AD}-", "schiff"]);
    let first = laid.lines().first().ok_or("no line")?;
    assert!(
        (first.width() - head).abs() < 0.01,
        "{} against {head}",
        first.width()
    );
    // Without the room for the hyphen, the line ends at the soft hyphen before.
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::text(text, style)],
        ),
        head - 0.5,
    );
    let first = laid.lines().first().ok_or("no line")?;
    assert_eq!(first.text(), "Donau\u{00AD}-");
    Ok(())
}

fn aligned(alignment: officewright_layout::ir::Alignment) -> ParagraphProps {
    ParagraphProps::default().with_alignment(alignment)
}

#[test]
fn justified_lines_fill_the_width_but_the_last() -> TestResult {
    use officewright_layout::ir::{Alignment, BreakClear, CompatFlag, DocumentSettings};

    let book = book()?;
    let (document, style) = liberation(11.0);
    let width = 200.0;
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            aligned(Alignment::Both),
            vec![Inline::text(SENTENCE, style)],
        ),
        width,
    );
    let lines = laid.lines();
    let (last, wrapped) = lines.split_last().ok_or("no line")?;
    for line in wrapped {
        assert!(
            (line.width() - width).abs() < 1e-6,
            "{:?}: {}",
            line.text(),
            line.width()
        );
        let last_word = line.words().last().ok_or("no word")?;
        assert!((last_word.end() - width).abs() < 1e-6);
        // The spaces between the words are all as wide.
        let gaps: Vec<f64> = line
            .words()
            .windows(2)
            .filter_map(|pair| Some(pair.get(1)?.start() - pair.first()?.end()))
            .collect();
        let (min, max) = gaps.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &gap| {
            (lo.min(gap), hi.max(gap))
        });
        assert!(max - min < 1e-6, "{gaps:?}");
    }
    assert!(
        last.width() < width - 10.0,
        "the last line is not justified"
    );
    // A line that ends in a line break is justified, unless the document says not to.
    let inlines = vec![
        Inline::text("one two three", style),
        Inline::line_break(BreakClear::None, style),
        Inline::text("four", style),
    ];
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(style, aligned(Alignment::Both), inlines.clone()),
        width,
    );
    assert!((laid.lines().first().ok_or("no line")?.width() - width).abs() < 1e-6);
    let settings =
        DocumentSettings::default().with_compat_flags([CompatFlag::DoNotExpandShiftReturn]);
    let mut legacy = Document::new(settings);
    let style = legacy.add_run_style(document.run_style(style).cloned().ok_or("no style")?);
    let inlines: Vec<Inline> = vec![
        Inline::text("one two three", style),
        Inline::line_break(BreakClear::None, style),
        Inline::text("four", style),
    ];
    let laid = fuzzing::lay_out(
        &book,
        &legacy,
        &paragraph(style, aligned(Alignment::Both), inlines),
        width,
    );
    assert!(laid.lines().first().ok_or("no line")?.width() < width - 50.0);
    // Nor is a line that ends in a page break.
    let inlines = vec![
        Inline::text("one two three", style),
        Inline::page_break(style),
    ];
    let laid = fuzzing::lay_out(
        &book,
        &legacy,
        &paragraph(style, aligned(Alignment::Both), inlines),
        width,
    );
    assert_eq!(laid.lines().len(), 1);
    assert!(laid.lines().first().ok_or("no line")?.width() < width - 50.0);
    Ok(())
}

#[test]
fn distributed_lines_fill_the_width_the_last_too_and_centred_lines_are_centred() -> TestResult {
    use officewright_layout::ir::Alignment;

    let book = book()?;
    let (document, style) = liberation(11.0);
    let width = 200.0;
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            aligned(Alignment::Distribute),
            vec![Inline::text("abc def", style)],
        ),
        width,
    );
    let line = laid.lines().first().ok_or("no line")?;
    assert!((line.width() - width).abs() < 1e-6);
    let last = line.words().last().ok_or("no word")?;
    assert!(
        (last.end() - width).abs() < 1e-6,
        "no room after the last character"
    );
    let natural = width_of(&book, &document, style, "Centred");
    for (alignment, start) in [
        (Alignment::Center, (width - natural) / 2.0),
        (Alignment::End, width - natural),
        (Alignment::Start, 0.0),
    ] {
        let laid = fuzzing::lay_out(
            &book,
            &document,
            &paragraph(
                style,
                aligned(alignment),
                vec![Inline::text("Centred  ", style)],
            ),
            width,
        );
        let word = laid
            .lines()
            .first()
            .and_then(|line| line.words().first())
            .ok_or("no word")?;
        assert!(
            (word.start() - start).abs() < 1e-6,
            "{alignment:?}: {}",
            word.start()
        );
    }
    Ok(())
}

#[test]
fn end_centre_and_decimal_tabs_align_the_text_after_them() -> TestResult {
    use officewright_layout::ir::TabKind;

    let book = book()?;
    let (document, style) = liberation(11.0);
    let stop = |kind| {
        ParagraphProps::default()
            .with_tabs([TabStop::default().with_position(Pt(200.0)).with_kind(kind)])
    };
    let lay = |kind, after: &str| {
        fuzzing::lay_out(
            &book,
            &document,
            &paragraph(
                style,
                stop(kind),
                vec![
                    Inline::text("Total", style),
                    Inline::tab(style),
                    Inline::text(after, style),
                ],
            ),
            400.0,
        )
    };
    let words = |laid: &LaidOut| -> Vec<(String, f64, f64)> {
        laid.lines()
            .first()
            .map(|line| {
                line.words()
                    .iter()
                    .map(|word| (word.text().to_owned(), word.start(), word.end()))
                    .collect()
            })
            .unwrap_or_default()
    };
    // End: the text ends at the stop.
    let laid = lay(TabKind::End, "1234.50");
    let amount = words(&laid).pop().ok_or("no amount")?;
    assert!((amount.2 - 200.0).abs() < 1e-6, "{amount:?}");
    // Centre: the text is centred on the stop.
    let laid = lay(TabKind::Center, "1234.50");
    let amount = words(&laid).pop().ok_or("no amount")?;
    assert!(
        ((amount.1 + amount.2) / 2.0 - 200.0).abs() < 1e-6,
        "{amount:?}"
    );
    // Decimal: the separator is at the stop, and without one the end of the number.
    let laid = lay(TabKind::Decimal, "1234.50");
    let amount = words(&laid).pop().ok_or("no amount")?;
    let whole = width_of(&book, &document, style, "1234");
    assert!((amount.1 + whole - 200.0).abs() < 1e-6, "{amount:?}");
    let laid = lay(TabKind::Decimal, "1234 EUR");
    let number = words(&laid).get(1).cloned().ok_or("no number")?;
    assert!((number.2 - 200.0).abs() < 1e-6, "{number:?}");
    // The tab spans from the end of "Total" to the start of the text after it.
    let laid = lay(TabKind::End, "9");
    let line = laid.lines().first().ok_or("no line")?;
    let (Some(total), Some(nine), Some(tab)) = (
        line.words().first(),
        line.words().get(1),
        line.tabs().first(),
    ) else {
        return Err("pieces".into());
    };
    assert!((tab.0 - total.end()).abs() < 1e-6 && (tab.1 - nine.start()).abs() < 1e-6);
    Ok(())
}

#[test]
fn a_tab_whose_stop_is_past_the_end_reaches_the_end_and_the_text_wraps() -> TestResult {
    let book = book()?;
    let (document, style) = liberation(11.0);
    let props = ParagraphProps::default().with_tabs([TabStop::default().with_position(Pt(300.0))]);
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            props,
            vec![
                Inline::text("Name", style),
                Inline::tab(style),
                Inline::text("Value", style),
            ],
        ),
        200.0,
    );
    let lines = laid.lines();
    assert_eq!(lines.len(), 2);
    let tab = lines
        .first()
        .and_then(|line| line.tabs().first())
        .ok_or("no tab")?;
    assert!((tab.1 - 200.0).abs() < 1e-6);
    assert_eq!(lines.get(1).map(|line| line.text()), Some("Value"));
    Ok(())
}

#[test]
fn empty_paragraphs_and_a_final_line_break_have_the_marks_height() -> TestResult {
    use officewright_layout::ir::BreakClear;

    let book = book()?;
    let mut document = Document::default();
    let small = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Liberation Sans"))
            .with_size(Pt(10.0)),
    );
    let big = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Liberation Sans"))
            .with_size(Pt(20.0)),
    );
    // Liberation Sans: usWinAscent 1854, usWinDescent 434 and an external leading of 67
    // units of 2048 a line.
    let line = |size: f64| size * 2355.0 / 2048.0;
    let laid = fuzzing::lay_out(&book, &document, &Paragraph::new(big), 200.0);
    assert_eq!(laid.lines().len(), 1);
    assert!((laid.height() - line(20.0)).abs() < 1e-9);
    // The mark's height counts on the paragraph's last line, not on the others.
    let inlines = vec![
        Inline::text("one", small),
        Inline::line_break(BreakClear::None, small),
        Inline::text("two", small),
    ];
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(big, ParagraphProps::default(), inlines),
        200.0,
    );
    let heights: Vec<f64> = laid.lines().iter().map(|line| line.height()).collect();
    assert_eq!(heights.len(), 2);
    assert!((heights.first().copied().unwrap_or_default() - line(10.0)).abs() < 1e-9);
    assert!((heights.get(1).copied().unwrap_or_default() - line(20.0)).abs() < 1e-9);
    // A line break at the end makes an empty line, with the mark.
    let inlines = vec![
        Inline::text("one", small),
        Inline::line_break(BreakClear::None, small),
    ];
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(small, ParagraphProps::default(), inlines),
        200.0,
    );
    let ends: Vec<&str> = laid.lines().iter().map(|line| line.end()).collect();
    assert_eq!(ends, ["break", "paragraph"]);
    // A page break at the end does not: the mark stays on its line.
    let inlines = vec![Inline::text("one", small), Inline::page_break(small)];
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(small, ParagraphProps::default(), inlines),
        200.0,
    );
    let ends: Vec<&str> = laid.lines().iter().map(|line| line.end()).collect();
    assert_eq!(ends, ["page break"]);
    Ok(())
}

#[test]
fn hidden_text_superscripts_and_objects() -> TestResult {
    use officewright_layout::ir::{
        ImageData, ImageFormat, InlineObject, ObjectContent, Picture, Size, VerticalPosition,
    };

    let book = book()?;
    let (mut document, style) = liberation(10.0);
    let base = document.run_style(style).cloned().ok_or("no style")?;
    let hidden = document.add_run_style(base.clone().with_hidden(true));
    let superscript =
        document.add_run_style(base.clone().with_vertical(VerticalPosition::Superscript));
    // Hidden text, and the tab of a hidden run, take no room.
    let inlines = vec![
        Inline::text("seen ", style),
        Inline::text("unseen ", hidden),
        Inline::tab(hidden),
        Inline::text("seen", style),
    ];
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(style, ParagraphProps::default(), inlines),
        400.0,
    );
    assert_eq!(
        laid.lines().first().map(|line| line.text()),
        Some("seen seen")
    );
    // A superscript is smaller, and the line keeps the height of its full size.
    let inlines = vec![Inline::text("x", style), Inline::text("2", superscript)];
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(style, ParagraphProps::default(), inlines),
        400.0,
    );
    let line = laid.lines().first().ok_or("no line")?;
    let two = line.words().first().ok_or("no word")?;
    let full_two = width_of(&book, &document, style, "x2") - width_of(&book, &document, style, "x");
    assert!(
        ((two.end() - two.start()) - width_of(&book, &document, style, "x") - full_two * 0.65)
            .abs()
            < 1e-6
    );
    assert!((line.height() - 10.0 * 2355.0 / 2048.0).abs() < 1e-9);
    // An inline object is as wide as it is, and a tall one makes the line taller.
    let image = document.add_image(ImageData::new(ImageFormat::Png, Vec::new()));
    let object = InlineObject::new(
        ObjectContent::Picture(Picture::new(image)),
        Size::new(Pt(40.0), Pt(30.0)),
    );
    let inlines = vec![
        Inline::text("a", style),
        Inline::Object(object),
        Inline::text("b", style),
    ];
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(style, ParagraphProps::default(), inlines),
        400.0,
    );
    let line = laid.lines().first().ok_or("no line")?;
    let (Some(a), Some(b)) = (line.words().first(), line.words().get(1)) else {
        return Err("words".into());
    };
    assert!((b.start() - a.end() - 40.0).abs() < 1e-9);
    // The object sits on the baseline, below the external leading of the text.
    let leading = 10.0 * 67.0 / 2048.0;
    assert!(
        (line.baseline() - (leading + 30.0)).abs() < 1e-9,
        "{}",
        line.baseline()
    );
    Ok(())
}

#[test]
fn a_line_cut_where_kerning_joins_two_glyphs_is_shaped_again() -> TestResult {
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Liberation Sans"))
            .with_size(Pt(20.0))
            .with_kern_min(Some(Pt(1.0))),
    );
    let word = "AVAVAVAVAVAVAVAVAVAV";
    let kerned = width_of(&book, &document, style, word);
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::text(word, style)],
        ),
        kerned / 2.0,
    );
    let first = laid.lines().first().ok_or("no line")?;
    // The first line, shaped on its own: its last glyph is not kerned against the
    // first glyph of the next line.
    let alone = width_of(&book, &document, style, first.text());
    assert!(
        (first.width() - alone).abs() < 1e-9,
        "{} against {alone}",
        first.width()
    );
    Ok(())
}
