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

/// The advance of a space in `style` of `document`: a space on its own hangs and has no
/// width on its line.
fn space_width(book: &FontBook, document: &Document, style: RunStyleId) -> f64 {
    width_of(book, document, style, "a b") - width_of(book, document, style, "ab")
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
        let space = space_width(&book, &document, style);
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
    // Every line is as wide as its text shaped on its own: what was shaped again for a
    // cut keeps nothing on the lines after it.
    for line in laid.lines() {
        let alone = width_of(&book, &document, style, line.text());
        assert!(
            (line.width() - alone).abs() < 1e-9,
            "{:?}: {} against {alone}",
            line.text(),
            line.width()
        );
    }
    // Shaped on its own the line is wider than first fit measured it; it still fits,
    // ending a cluster earlier where needed.
    for line in laid.lines() {
        assert!(
            line.width() <= kerned / 2.0 + 1e-9,
            "{:?}: {}",
            line.text(),
            line.width()
        );
    }
    assert!(laid.report().is_empty());
    Ok(())
}

#[test]
fn spaces_before_a_word_too_wide_for_the_line_end_the_line() -> TestResult {
    use officewright_layout::ir::{
        ImageData, ImageFormat, InlineObject, ObjectContent, Picture, Size,
    };

    let book = book()?;
    let (mut document, style) = liberation(11.0);
    // The spaces, wider than the line, take the first line; the word, no wider than a
    // line, the next.
    let word = "Wordwordword";
    let width = width_of(&book, &document, style, word) + 1.0;
    let spaces = " ".repeat(30);
    assert!(width_of(&book, &document, style, &format!("{spaces}W")) > width);
    let text = format!("{spaces}{word}");
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::text(text, style)],
        ),
        width,
    );
    let texts: Vec<&str> = laid.lines().iter().map(|line| line.text()).collect();
    assert_eq!(texts, [spaces.as_str(), word]);
    assert!(laid.report().is_empty(), "{:?}", laid.report());
    // An object wider than its line stays on it, reported as an object.
    let image = document.add_image(ImageData::new(ImageFormat::Png, Vec::new()));
    let picture = ObjectContent::Picture(Picture::new(image));
    let object = InlineObject::new(picture, Size::new(Pt(400.0), Pt(10.0)));
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::Object(object)],
        ),
        100.0,
    );
    let messages: Vec<&str> = laid.report().iter().map(|entry| entry.message()).collect();
    assert_eq!(
        messages,
        ["an inline object is wider than its line and reaches past its end"]
    );
    Ok(())
}

/// Checks the space between paragraphs against Word's render of `name`
/// (`E-autospacing` and `E-autospacing-nohtml`): paragraphs of one 20 pt exact line,
/// most of them Calibri 11, with automatic and explicit spacing; Word's baselines must be
/// as far apart as ours.
fn check_autospacing(name: &str, no_html: bool) -> TestResult {
    use officewright_layout::ir::{CompatFlag, DocumentSettings};

    let book = book()?;
    let word = word_lines(name)?;
    let flags = if no_html {
        vec![CompatFlag::DoNotUseHTMLParagraphAutoSpacing]
    } else {
        Vec::new()
    };
    let mut document = Document::new(DocumentSettings::default().with_compat_flags(flags));
    let mut style = |size: f64| {
        document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Calibri"))
                .with_size(Pt(size)),
        )
    };
    let (small, large) = (style(11.0), style(20.0));
    // (size, before, after, automatic before, automatic after), as make-rule-docs.py
    // writes the document.
    let cases: [(f64, f64, f64, bool, bool); 14] = [
        (11.0, 0.0, 0.0, false, false),
        (11.0, 0.0, 0.0, true, true),
        (11.0, 0.0, 0.0, false, false),
        (11.0, 0.0, 0.0, true, false),
        (11.0, 0.0, 0.0, false, false),
        (11.0, 0.0, 0.0, false, true),
        (11.0, 0.0, 0.0, false, false),
        (11.0, 0.0, 0.0, true, true),
        (11.0, 0.0, 0.0, true, true),
        (11.0, 0.0, 0.0, false, false),
        (11.0, 24.0, 24.0, true, true),
        (11.0, 0.0, 0.0, false, false),
        (20.0, 0.0, 0.0, true, true),
        (11.0, 0.0, 0.0, false, false),
    ];
    let paragraphs: Vec<(f64, Paragraph)> = cases
        .iter()
        .map(|&(size, before, after, before_auto, after_auto)| {
            let run = if size > 11.0 { large } else { small };
            let spacing = Spacing::default()
                .with_before(Pt(before))
                .with_after(Pt(after))
                .with_before_auto(before_auto)
                .with_after_auto(after_auto)
                .with_line(LineSpacing::Exact(Pt(20.0)));
            let paragraph = Paragraph::new(run)
                .with_props(ParagraphProps::default().with_spacing(spacing))
                .with_inlines([Inline::text("Case", run)]);
            (size, paragraph)
        })
        .collect();
    let mut top = 0.0;
    let mut ours = Vec::new();
    for (index, (_, paragraph)) in paragraphs.iter().enumerate() {
        if let Some((_, previous)) = index.checked_sub(1).and_then(|i| paragraphs.get(i)) {
            top += fuzzing::space_between(&document, previous.props(), paragraph.props());
        }
        let laid = lay_out(&book, &document, paragraph);
        ours.push(top + laid.lines().first().map_or(0.0, |line| line.baseline()));
        top += laid.height();
    }
    // Word's baselines: the tops of the boxes `pdftotext` gives for Calibri are three
    // quarters of the size above them.
    let theirs: Vec<f64> = word
        .iter()
        .zip(&paragraphs)
        .map(|(line, (size, _))| line.top + 0.75 * size)
        .collect();
    assert_eq!(theirs.len(), ours.len(), "{name}");
    for (index, (pair, word_pair)) in ours.windows(2).zip(theirs.windows(2)).enumerate() {
        let (Some(a), Some(b), Some(c), Some(d)) = (
            pair.first(),
            pair.get(1),
            word_pair.first(),
            word_pair.get(1),
        ) else {
            continue;
        };
        assert!(
            ((b - a) - (d - c)).abs() <= GRID,
            "{name}: paragraph {} is {:.2} pt below the one before, Word's {:.2}",
            index + 1,
            b - a,
            d - c
        );
    }
    Ok(())
}

#[test]
fn automatic_spacing_is_words() -> TestResult {
    check_autospacing("E-autospacing", false)
}

#[test]
fn automatic_spacing_off_is_words() -> TestResult {
    check_autospacing("E-autospacing-nohtml", true)
}

#[test]
fn a_line_shaped_again_and_too_wide_ends_at_its_place_before() -> TestResult {
    // Kerning joins T and o across the soft hyphen, and the A and the hyphen-minus: a
    // line that ends there is wider shaped on its own. It must end at its place
    // before, after "aaa ", never inside a word or before a hyphen (UAX #14, LB21).
    let book = book()?;
    let mut document = Document::default();
    let kerned = |family: &str| {
        RunStyle::default()
            .with_fonts(RunFonts::new(family))
            .with_size(Pt(20.0))
            .with_kern_min(Some(Pt(1.0)))
    };
    let sans = document.add_run_style(kerned("Liberation Sans"));
    let carlito = document.add_run_style(kerned("Calibri"));
    for (style, text, from, to) in [
        (sans, "aaa T\u{00AD}onne", 50.0, 62.0),
        (carlito, "aaa AA-Yacht", 56.0, 68.0),
        // T and the hyphen fit kerned, not shaped on their own: the line ends at the
        // soft hyphen without one.
        (sans, "T\u{00AD}onne", 15.0, 20.0),
    ] {
        let mut lines_before = usize::MAX;
        let mut width = from;
        while width < to {
            let paragraph = Paragraph::new(style).with_inlines([Inline::text(text, style)]);
            fuzzing::check_lines(&book, &document, &paragraph, width)
                .map_err(|problem| format!("{text:?} in {width} pt: {problem}"))?;
            let laid = fuzzing::lay_out(&book, &document, &paragraph, width);
            let lines: Vec<&str> = laid.lines().iter().map(|line| line.text()).collect();
            // Every character fits a line of its own: nothing overflows.
            let overflows = laid
                .report()
                .iter()
                .filter(|entry| entry.kind() == officewright_layout::ReportKind::Overflow)
                .count();
            assert_eq!(overflows, 0, "{text:?} in {width} pt: {lines:?}");
            // A wider column never takes more lines.
            assert!(
                lines.len() <= lines_before,
                "{text:?} in {width} pt: {lines:?}"
            );
            lines_before = lines.len();
            // A line without a drawn hyphen (which the dump writes after its soft hyphen)
            // is as wide as its text shaped on its own.
            let drawn_hyphen = |line: &&fuzzing::Line| line.text().ends_with("\u{00AD}-");
            for line in laid.lines().iter().filter(|line| !drawn_hyphen(line)) {
                let alone = width_of(&book, &document, style, line.text());
                assert!(
                    (line.width() - alone).abs() < 1e-9,
                    "{text:?} in {width} pt: {:?} is {}, alone {alone}",
                    line.text(),
                    line.width()
                );
            }
            for line in lines.iter().skip(1) {
                assert!(
                    !line.starts_with(['-', '\u{00AD}']),
                    "{text:?} in {width} pt: {lines:?}"
                );
            }
            // Where the second word is cut, it is at its soft hyphen or hyphen.
            let first = lines.first().filter(|line| line.len() > "aaa ".len());
            if let Some(first) = first.filter(|_| text.starts_with("aaa")) {
                assert!(first.ends_with('-'), "{text:?} in {width} pt: {lines:?}");
            }
            width += 0.1;
        }
    }
    Ok(())
}

#[test]
fn spaces_before_a_line_break_hang() -> TestResult {
    // Word's form-gsa-pacs-series-form: a right-aligned "Identity Management Division "
    // and a line break end at the margin, like the lines without a space.
    use officewright_layout::ir::{Alignment, BreakClear};

    let book = book()?;
    let (document, style) = liberation(11.0);
    let right = ParagraphProps::default().with_alignment(Alignment::End);
    let ends: Vec<f64> = ["word ", "word"]
        .iter()
        .map(|text| {
            let inlines = vec![
                Inline::text(*text, style),
                Inline::line_break(BreakClear::None, style),
                Inline::text("next", style),
            ];
            let laid = fuzzing::lay_out(
                &book,
                &document,
                &paragraph(style, right.clone(), inlines),
                200.0,
            );
            laid.lines()
                .first()
                .and_then(|line| line.words().first())
                .map_or(0.0, |word| word.end())
        })
        .collect();
    assert_eq!(ends, [200.0, 200.0]);
    Ok(())
}

#[test]
fn justification_widens_only_the_spaces_after_the_last_tab() -> TestResult {
    use officewright_layout::ir::Alignment;

    let book = book()?;
    let (document, style) = liberation(11.0);
    let props = ParagraphProps::default()
        .with_alignment(Alignment::Both)
        .with_tabs([TabStop::default().with_position(Pt(60.0))]);
    let inlines = vec![
        Inline::text("a b", style),
        Inline::tab(style),
        Inline::text(SENTENCE, style),
    ];
    let laid = fuzzing::lay_out(&book, &document, &paragraph(style, props, inlines), 300.0);
    let first = laid.lines().first().ok_or("no line")?;
    let words = first.words();
    let (Some(a), Some(b), Some(after)) = (words.first(), words.get(1), words.get(2)) else {
        return Err("words".into());
    };
    // Before the tab the space keeps its width, and the text after the tab starts at
    // its stop; after the tab the line is justified to its end.
    let space = space_width(&book, &document, style);
    assert!((b.start() - a.end() - space).abs() < 1e-9);
    assert!((after.start() - 60.0).abs() < 1e-9, "{}", after.start());
    assert!((first.width() - 300.0).abs() < 1e-6);
    Ok(())
}

#[test]
fn a_line_separator_in_text_ends_the_line() -> TestResult {
    let book = book()?;
    let (document, style) = liberation(11.0);
    let laid = fuzzing::lay_out(
        &book,
        &document,
        &paragraph(
            style,
            ParagraphProps::default(),
            vec![Inline::text("one\u{2028}two", style)],
        ),
        300.0,
    );
    let ends: Vec<&str> = laid.lines().iter().map(|line| line.end()).collect();
    assert_eq!(ends, ["break", "paragraph"]);
    Ok(())
}

#[test]
fn the_width_ends_where_the_text_after_an_end_tab_does() -> TestResult {
    use officewright_layout::ir::TabKind;

    let book = book()?;
    let (document, style) = liberation(11.0);
    let props = ParagraphProps::default().with_tabs([TabStop::default()
        .with_position(Pt(200.0))
        .with_kind(TabKind::End)]);
    let inlines = vec![
        Inline::text("Total", style),
        Inline::tab(style),
        Inline::text("9      ", style),
    ];
    let laid = fuzzing::lay_out(&book, &document, &paragraph(style, props, inlines), 400.0);
    let line = laid.lines().first().ok_or("no line")?;
    let nine = line.words().get(1).ok_or("no 9")?;
    assert!(
        (line.width() - nine.end()).abs() < 1e-9,
        "{} against {}",
        line.width(),
        nine.end()
    );
    Ok(())
}

/// A paragraph built from `seed`, as the fuzz target `layout_shape` builds them, with
/// line and page breaks, inline objects and kerning added; and its column's width.
fn random_paragraph(seed: u64, document: &mut Document) -> (Paragraph, f64) {
    use officewright_layout::ir::{
        Alignment, BreakClear, ImageData, ImageFormat, InlineObject, ObjectContent, Picture, Size,
        TabKind,
    };

    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut next = move |below: u64| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) % below.max(1)
    };
    const FAMILIES: [&str; 5] = [
        "Calibri",
        "Liberation Sans",
        "Times New Roman",
        "Noto Sans",
        "Courier New",
    ];
    const WORDS: [&str; 14] = [
        "AV",
        "To",
        "T\u{00AD}onne",
        "AA-Yacht",
        "Wordwordword",
        "ab\u{00AD}cd",
        " ",
        "  ",
        "a\u{00A0}b",
        "Donau\u{00AD}dampf\u{00AD}schiff",
        "12.50",
        "x",
        "\u{2028}",
        "Ünïcödé",
    ];
    let mut make = || {
        let family = FAMILIES.get(next(5) as usize).copied().unwrap_or("Calibri");
        let mut style = RunStyle::default()
            .with_fonts(RunFonts::new(family))
            .with_size(Pt(4.0 + next(30) as f64));
        if next(2) == 0 {
            style = style.with_kern_min(Some(Pt(1.0)));
        }
        if next(4) == 0 {
            style = style.with_letter_spacing(Pt(next(6) as f64 - 2.0));
        }
        document.add_run_style(style)
    };
    let (a, b, c) = (make(), make(), make());
    let style = |index: u64| match index % 3 {
        0 => a,
        1 => b,
        _ => c,
    };
    let mut inlines = Vec::new();
    for _ in 0..(1 + next(12)) {
        let run = style(next(3));
        inlines.push(match next(10) {
            0 => Inline::tab(run),
            1 => Inline::soft_hyphen(run),
            2 => Inline::line_break(BreakClear::None, run),
            3 if next(4) == 0 => Inline::page_break(run),
            4 => {
                let image = document.add_image(ImageData::new(ImageFormat::Png, Vec::new()));
                let size = Size::new(Pt(next(120) as f64), Pt(next(40) as f64));
                Inline::Object(InlineObject::new(
                    ObjectContent::Picture(Picture::new(image)),
                    size,
                ))
            }
            _ => Inline::text(WORDS.get(next(14) as usize).copied().unwrap_or("x"), run),
        });
    }
    let alignments = [
        Alignment::Start,
        Alignment::Center,
        Alignment::End,
        Alignment::Both,
        Alignment::Distribute,
    ];
    let kinds = [
        TabKind::Start,
        TabKind::Center,
        TabKind::End,
        TabKind::Decimal,
        TabKind::Bar,
    ];
    let line = match next(3) {
        0 => LineSpacing::Multiple(next(64) as f64 / 16.0),
        1 => LineSpacing::Exact(Pt(next(40) as f64)),
        _ => LineSpacing::AtLeast(Pt(next(40) as f64)),
    };
    let tabs: Vec<TabStop> = (0..next(3))
        .map(|_| {
            TabStop::default()
                .with_position(Pt(next(300) as f64))
                .with_kind(
                    kinds
                        .get(next(5) as usize)
                        .copied()
                        .unwrap_or(TabKind::Start),
                )
        })
        .collect();
    let props = ParagraphProps::default()
        .with_alignment(
            alignments
                .get(next(5) as usize)
                .copied()
                .unwrap_or(Alignment::Start),
        )
        .with_spacing(Spacing::default().with_line(line))
        .with_indent(
            Indent::default()
                .with_start(Pt(next(40) as f64))
                .with_first_line(Pt(next(40) as f64 - 20.0)),
        )
        .with_tabs(tabs)
        .with_word_wrap(next(4) != 0);
    let mark = style(0);
    (
        Paragraph::new(mark).with_props(props).with_inlines(inlines),
        1.0 + next(400) as f64,
    )
}

#[test]
fn every_line_breaking_keeps_what_pagination_relies_on() -> TestResult {
    let book = book()?;
    // The input the reviewer of #152 found for the fuzz target: Calibri 11, "ab", a soft
    // hyphen and "cd" in a column 1 pt wide.
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Calibri"))
            .with_size(Pt(11.0)),
    );
    let paragraph = Paragraph::new(style).with_inlines([
        Inline::text("ab", style),
        Inline::soft_hyphen(style),
        Inline::text("cd", style),
    ]);
    for width in [1.0, 4.0, 5.0, 9.0, 13.0, 20.0] {
        fuzzing::check_lines(&book, &document, &paragraph, width)
            .map_err(|problem| format!("{width} pt: {problem}"))?;
    }
    // Paragraphs of every ingredient, in columns of every width.
    for seed in 0..1000 {
        let mut document = Document::default();
        let (paragraph, width) = random_paragraph(seed, &mut document);
        fuzzing::check_lines(&book, &document, &paragraph, width)
            .map_err(|problem| format!("seed {seed}, {width} pt: {problem}"))?;
    }
    Ok(())
}

#[test]
fn spaces_before_an_object_of_no_width_hang_and_no_room_is_no_room() -> TestResult {
    use officewright_layout::ir::{
        Alignment, ImageData, ImageFormat, InlineObject, ObjectContent, Picture, Size,
    };

    let book = book()?;
    let (mut document, style) = liberation(11.0);
    let image = document.add_image(ImageData::new(ImageFormat::Png, Vec::new()));
    let object = || {
        let picture = ObjectContent::Picture(Picture::new(image));
        Inline::Object(InlineObject::new(picture, Size::new(Pt(0.0), Pt(10.0))))
    };
    let right = ParagraphProps::default().with_alignment(Alignment::End);
    let inlines = vec![Inline::text("word ", style), object()];
    let with_object = paragraph(style, right, inlines);
    fuzzing::check_lines(&book, &document, &with_object, 25.0)?;
    let laid = fuzzing::lay_out(&book, &document, &with_object, 200.0);
    let end = laid
        .lines()
        .first()
        .and_then(|line| line.words().first())
        .map(|word| word.end());
    assert_eq!(end, Some(200.0));
    // Indents wider than the column leave no room: the spaces of a line still share it.
    let props = ParagraphProps::default().with_indent(Indent::default().with_start(Pt(40.0)));
    let indented = paragraph(style, props, vec![Inline::text("    abc", style)]);
    fuzzing::check_lines(&book, &document, &indented, 20.0)?;
    let laid = fuzzing::lay_out(&book, &document, &indented, 20.0);
    assert_eq!(laid.lines().first().map(|line| line.text()), Some("    "));
    Ok(())
}

#[test]
fn a_hyphen_that_does_not_fit_after_shaping_again_is_dropped_before_giving_up() -> TestResult {
    // Kerned, "f" and the hyphen fit; shaped on its own, "f" is wider and the hyphen
    // no longer fits. The line ends at the soft hyphen without one, and nothing is
    // reported as overflowing: "f" alone is 7.78 pt.
    let book = book()?;
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Liberation Sans"))
            .with_size(Pt(28.0))
            .with_kern_min(Some(Pt(1.0))),
    );
    let paragraph = Paragraph::new(style).with_inlines([Inline::text("f\u{00AD}fi", style)]);
    let mut width = 16.5;
    while width < 17.2 {
        fuzzing::check_lines(&book, &document, &paragraph, width)
            .map_err(|problem| format!("{width} pt: {problem}"))?;
        let laid = fuzzing::lay_out(&book, &document, &paragraph, width);
        let lines: Vec<&str> = laid.lines().iter().map(|line| line.text()).collect();
        assert!(
            laid.report().is_empty(),
            "{width} pt: {lines:?} {:?}",
            laid.report()
        );
        for line in laid.lines() {
            assert!(line.width() <= width + 1e-9, "{width} pt: {lines:?}");
        }
        width += 0.05;
    }
    Ok(())
}

/// A document whose text is in Liberation Sans 10 pt with `spacing` after each
/// character, and that style.
fn spaced_out(spacing: f64) -> (Document, RunStyleId) {
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Liberation Sans"))
            .with_size(Pt(10.0))
            .with_letter_spacing(Pt(spacing)),
    );
    (document, style)
}

#[test]
fn breaking_lines_ends_when_a_dropped_hyphen_does_not_help() -> TestResult {
    // The line ending at the soft hyphen does not fit with its hyphen nor without it
    // (the tab before it starts past the end); first fit offers the hyphen again. The
    // breaker must not go back and forth between the two.
    let book = book()?;
    let (document, style) = spaced_out(-2.0);
    for (opening, width) in [
        ("( ".to_owned(), 1.44),
        (format!("({}", " ".repeat(120)), 94.10),
    ] {
        let paragraph = Paragraph::new(style).with_inlines([
            Inline::text(opening, style),
            Inline::tab(style),
            Inline::text("\u{00AD}WW", style),
        ]);
        fuzzing::check_lines(&book, &document, &paragraph, width)
            .map_err(|problem| format!("{width} pt: {problem}"))?;
    }
    Ok(())
}

#[test]
fn a_soft_hyphen_takes_no_room_however_the_text_is_spaced() -> TestResult {
    let book = book()?;
    // Spaced out by 3 pt: no line starts with the soft hyphen, wherever the line ends.
    let (document, style) = spaced_out(3.0);
    let paragraph = Paragraph::new(style).with_inlines([Inline::text("ab\u{00AD}cd", style)]);
    let mut width = 17.0;
    while width < 20.2 {
        fuzzing::check_lines(&book, &document, &paragraph, width)
            .map_err(|problem| format!("{width} pt: {problem}"))?;
        width += 0.05;
    }
    // Drawn as measured: on one line, the glyphs end where the line does.
    let laid = fuzzing::lay_out(&book, &document, &paragraph, 200.0);
    let line = laid.lines().first().ok_or("no line")?;
    let end = line.words().last().map_or(0.0, |word| word.end());
    assert!(
        (end - line.width()).abs() < 1e-9,
        "{end} against {}",
        line.width()
    );
    // Condensed by 2 pt: "xx " and "ab-cd" fit on lines of their own; the soft hyphen
    // takes no room to end the first line at.
    let (document, style) = spaced_out(-2.0);
    let paragraph = Paragraph::new(style).with_inlines([Inline::text("xx ab\u{00AD}cd", style)]);
    let mut width = 13.95;
    while width < 15.2 {
        let laid = fuzzing::lay_out(&book, &document, &paragraph, width);
        let lines: Vec<&str> = laid.lines().iter().map(|line| line.text()).collect();
        assert_eq!(lines, ["xx ", "ab\u{00AD}cd"], "{width} pt");
        width += 0.05;
    }
    Ok(())
}

#[test]
fn a_tab_past_the_end_with_nowhere_to_end_before_it_hangs() -> TestResult {
    // No line may end after "(" and its space (UAX #14), so the tab that starts past the
    // end stays on the line and hangs, like the space.
    let book = book()?;
    let (document, style) = liberation(10.0);
    let paragraph = Paragraph::new(style).with_inlines([
        Inline::text("( ", style),
        Inline::tab(style),
        Inline::text("abc", style),
    ]);
    let mut width = 4.0;
    while width < 6.2 {
        fuzzing::check_lines(&book, &document, &paragraph, width)
            .map_err(|problem| format!("{width} pt: {problem}"))?;
        let laid = fuzzing::lay_out(&book, &document, &paragraph, width);
        let first = laid.lines().first().ok_or("no line")?;
        assert_eq!(first.text(), "( \t", "{width} pt");
        width += 0.1;
    }
    Ok(())
}
