//! Measures each metric-compatible substitute the resolver takes against Word.
//!
//! `tests/corpus/word-rules/E-line-fonts.docx` sets ten lines per font at 11 and 12 pt,
//! and the `geometry.json` of its Word reference render holds the box of every line and
//! word as pdftotext reads them from Word's PDF. For each Microsoft font in it that the
//! resolver replaces with a metric-compatible font from tests/fonts, the free font must
//! set every word and line of that font as wide as Word set it, and space the lines as
//! far apart.
//!
//! How close is close enough comes from the render itself: Word also set Liberation Sans
//! and Noto Sans, which are in tests/fonts, and how far their boxes are from what their
//! own advance widths give is how precisely the render measures a font.
//!
//! The fonts keep their own licenses (THIRD-PARTY-NOTICES.md), so this test is not in
//! the published crate.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use officewright_font::{FontBook, FontFace, FontRequest, FontResolver, ResolutionKind};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// The Microsoft fonts the resolver replaces with a metric-compatible font, with the
/// font it takes. `resolve.rs` checks that its table lists the same pairs.
const PAIRS: [(&str, &str); 4] = [
    ("Calibri", "Carlito"),
    ("Arial", "Liberation Sans"),
    ("Times New Roman", "Liberation Serif"),
    ("Courier New", "Liberation Mono"),
];

/// Word places lines on a grid of 1/600 inch in its PDF: the distances between the lines
/// of one font in `geometry.json` are multiples of `GRID`, except at most one per
/// paragraph that is up to `OFF_GRID` off. The first and the last of `n` lines are each
/// within half a step of where their line height puts them, so the mean distance is
/// known to `(GRID + OFF_GRID) / (n - 1)`.
const GRID: f64 = 0.12;

/// How far the one distance per paragraph that is off the grid is off: 0.02 or 0.03 pt.
const OFF_GRID: f64 = 0.03;

/// The lines `E-line-fonts` sets of each font at each size, as one paragraph.
const LINES: u32 = 10;

/// The most that Word's boxes for a font in tests/fonts may be off from its advance
/// widths before the comparison is taken to be broken (a font in tests/fonts that is not
/// the version Word had, say) rather than a measure of precision. The render shows up to
/// 0.375 pt, a few steps of Word's grid.
const PRECISION_LIMIT: f64 = 0.5;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The fonts in tests/fonts, without the system fonts, so the results are the same on
/// every machine.
fn test_fonts() -> TestResult<FontBook> {
    let mut book = FontBook::new();
    book.add_dir(repository().join("tests/fonts"))?;
    Ok(book)
}

/// A line of `geometry.json`.
struct Line {
    page: u32,
    top: f64,
    text: String,
    width: f64,
    words: Vec<(String, f64)>,
}

/// The lines of `E-line-fonts` by font family and size. Each line starts with them, as
/// in "Calibri 11 pt, line 01: Hamburgefonts gjpqy".
type Rows = BTreeMap<String, BTreeMap<u32, Vec<Line>>>;

fn rows() -> TestResult<Rows> {
    let path = repository().join("tests/corpus/reference/word-rules/E-line-fonts/geometry.json");
    let geometry = Json::parse(&std::fs::read_to_string(path)?)?;
    if geometry.get("format")?.number()? != 1.0 {
        return Err("geometry.json is not in format 1".into());
    }
    let box_width = |value: &Json| -> TestResult<f64> {
        match value.get("box")?.array()? {
            [left, _, right, _] => Ok(right.number()? - left.number()?),
            _ => Err("a box is not four numbers".into()),
        }
    };
    let mut rows = Rows::new();
    for page in geometry.get("pages")?.array()? {
        let number = page.get("number")?.number()?.to_string().parse::<u32>()?;
        for line in page.get("lines")?.array()? {
            let text = line.get("text")?.string()?;
            let Some((family, size)) = text
                .split_once(" pt, line ")
                .and_then(|(start, _)| start.rsplit_once(' '))
            else {
                continue;
            };
            let top = match line.get("box")?.array()? {
                [_, top, _, _] => top.number()?,
                _ => return Err("a box is not four numbers".into()),
            };
            let mut words = Vec::new();
            for word in line.get("words")?.array()? {
                words.push((word.get("text")?.string()?.to_owned(), box_width(word)?));
            }
            rows.entry(family.to_owned())
                .or_default()
                .entry(size.parse()?)
                .or_default()
                .push(Line {
                    page: number,
                    top,
                    text: text.to_owned(),
                    width: box_width(line)?,
                    words,
                });
        }
    }
    Ok(rows)
}

/// The width of `text` in `face` at `size` pt: the sum of the advance widths, without
/// kerning, as Word sets text by default.
fn width(face: &FontFace, text: &str, size: f64) -> TestResult<f64> {
    let mut units = 0_u32;
    for ch in text.chars() {
        let glyph = face
            .glyph_id(ch)
            .ok_or_else(|| format!("{:?} has no glyph for {ch:?}", face.names().full_name()))?;
        units += u32::from(
            face.advance_width(glyph)
                .ok_or("glyph without an advance")?,
        );
    }
    Ok(f64::from(units) * size / f64::from(face.units_per_em()))
}

/// The single line height GDI gives a font, which Word uses: `usWinAscent +
/// usWinDescent` plus the external leading, the part of the `hhea` line height that the
/// Windows metrics leave out ("Baseline to Baseline Distances" in Recommendations for
/// OpenType Fonts, OpenType 1.9.1).
fn line_height(face: &FontFace, size: f64) -> TestResult<f64> {
    let metrics = face.metrics();
    let win = metrics.win().ok_or("no OS/2 table")?;
    let hhea = metrics.hhea();
    let win = i32::from(win.ascent()) + i32::from(win.descent());
    let hhea =
        i32::from(hhea.ascender()) - i32::from(hhea.descender()) + i32::from(hhea.line_gap());
    let external_leading = (hhea - win).max(0);
    Ok(f64::from(win + external_leading) * size / f64::from(face.units_per_em()))
}

/// The face the resolver takes for `family` from tests/fonts, and how it took it.
fn resolve(book: &FontBook, family: &str) -> TestResult<(ResolutionKind, FontFace)> {
    let resolution = FontResolver::new(book)
        .resolve(&FontRequest::new(family))
        .clone();
    let face = book.load(resolution.face().ok_or("no face")?)?;
    Ok((resolution.kind(), face))
}

/// What a box of `geometry.json` holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Span {
    Word,
    Line,
}

/// How far a face's advance widths are from Word's boxes for the lines of a family:
/// one entry per word and per line, with a description and the difference in pt. The
/// lines of a size differ only in their numbers, so they share a description.
fn width_differences(
    face: &FontFace,
    sizes: &BTreeMap<u32, Vec<Line>>,
) -> TestResult<Vec<(Span, String, f64)>> {
    let mut differences = Vec::new();
    for (&size, lines) in sizes {
        let size = f64::from(size);
        for line in lines {
            for (word, word_width) in &line.words {
                let ours = width(face, word, size)?;
                let what = format!("{size} pt, word {word:?}");
                differences.push((Span::Word, what, ours - word_width));
            }
            let ours = width(face, &line.text, size)?;
            let what = format!("{size} pt, whole lines");
            differences.push((Span::Line, what, ours - line.width));
        }
    }
    Ok(differences)
}

/// The largest difference in `differences` for boxes of `span`.
fn largest(differences: &[(Span, String, f64)], span: Span) -> f64 {
    differences
        .iter()
        .filter(|(kind, _, _)| *kind == span)
        .map(|(_, _, difference)| difference.abs())
        .fold(0.0, f64::max)
}

#[test]
fn the_resolver_takes_the_measured_fonts() -> TestResult {
    let book = test_fonts()?;
    for (family, substitute) in PAIRS {
        let (kind, face) = resolve(&book, family)?;
        assert_eq!(kind, ResolutionKind::MetricCompatible, "{family}");
        assert_eq!(face.names().family(), Some(substitute), "{family}");
    }
    // Every font Word set in E-line-fonts that the resolver has a metric-compatible
    // substitute for is measured: its substitute is in tests/fonts and in PAIRS.
    let mut resolver = FontResolver::new(&book);
    for family in rows()?.keys() {
        let resolution = resolver.resolve(&FontRequest::new(family.as_str()));
        assert_eq!(
            resolution.uninstalled_substitute(),
            None,
            "the metric-compatible substitute for {family} is not in tests/fonts"
        );
        if resolution.kind() == ResolutionKind::MetricCompatible {
            assert!(PAIRS.iter().any(|(name, _)| name == family), "{family}");
        }
    }
    Ok(())
}

#[test]
fn substitutes_set_text_as_wide_as_word_set_the_original() -> TestResult {
    let book = test_fonts()?;
    let rows = rows()?;
    // Fonts Word set as themselves: how precisely the render measures a font.
    let mut precision = (0.0_f64, 0.0_f64);
    for family in ["Liberation Sans", "Noto Sans"] {
        let (kind, face) = resolve(&book, family)?;
        assert_eq!(kind, ResolutionKind::Exact, "{family}");
        let sizes = rows.get(family).ok_or(family)?;
        assert_eq!(
            sizes.keys().copied().collect::<Vec<_>>(),
            [11, 12],
            "{family}"
        );
        let differences = width_differences(&face, sizes)?;
        precision.0 = precision.0.max(largest(&differences, Span::Word));
        precision.1 = precision.1.max(largest(&differences, Span::Line));
    }
    assert!(
        precision.0 > 0.0 && precision.0 <= PRECISION_LIMIT,
        "words: {precision:?}"
    );
    assert!(
        precision.1 > 0.0 && precision.1 <= PRECISION_LIMIT,
        "lines: {precision:?}"
    );

    let mut failures = String::new();
    for (family, _) in PAIRS {
        let (_, face) = resolve(&book, family)?;
        let name = face.names().family().unwrap_or("?").to_owned();
        let sizes = rows.get(family).ok_or(family)?;
        assert_eq!(
            sizes.keys().copied().collect::<Vec<_>>(),
            [11, 12],
            "{family}"
        );
        // Report the largest difference for each word and for the lines of each size.
        let mut worst = BTreeMap::<String, (f64, f64)>::new();
        for (span, what, difference) in width_differences(&face, sizes)? {
            let limit = match span {
                Span::Word => precision.0,
                Span::Line => precision.1,
            };
            if difference.abs() > limit {
                let entry = worst.entry(what).or_insert((difference, limit));
                if difference.abs() > entry.0.abs() {
                    entry.0 = difference;
                }
            }
        }
        for (what, (difference, limit)) in worst {
            writeln!(
                failures,
                "{name} for {family}, {what}: {difference:+.3} pt (Word's own fonts are \
                 within {limit:.3} pt)"
            )?;
        }
    }
    assert!(failures.is_empty(), "{failures}");
    Ok(())
}

#[test]
fn substitutes_space_lines_as_word_spaced_the_original() -> TestResult {
    let book = test_fonts()?;
    let rows = rows()?;
    let mut failures = String::new();
    // Word spaced Liberation Sans and Noto Sans by GDI's line height too, Noto Sans
    // although it asks for its typographic metrics (USE_TYPO_METRICS).
    let substitutes = PAIRS.iter().map(|(family, _)| (*family, true));
    let measured = substitutes.chain([("Liberation Sans", false), ("Noto Sans", false)]);
    for (family, substitute) in measured {
        let (_, face) = resolve(&book, family)?;
        let name = face.names().family().unwrap_or("?").to_owned();
        // Word honours USE_TYPO_METRICS for some fonts and not for others, so the line
        // height of a substitute that sets it cannot be compared yet.
        assert!(
            !(substitute && face.metrics().use_typo_metrics()),
            "{name}, the substitute for {family}, sets USE_TYPO_METRICS: its line height \
             cannot be compared with Word's until it is known whether Word honours that flag \
             for it"
        );
        let sizes = rows.get(family).ok_or(family)?;
        assert_eq!(
            sizes.keys().copied().collect::<Vec<_>>(),
            [11, 12],
            "{family}"
        );
        for (&size, lines) in sizes {
            // The tolerance comes from the number of lines, so a reference whose lines
            // pdftotext merged or dropped, or that a page break split, would loosen it:
            // each font and size must have its ten lines, in order, on one page.
            // A line without a number counts as its whole text, so it fails the check too.
            let numbers: Vec<&str> = lines
                .iter()
                .map(|line| {
                    line.text
                        .split_once(" pt, line ")
                        .and_then(|(_, rest)| rest.split_once(':'))
                        .map_or(line.text.as_str(), |(number, _)| number)
                })
                .collect();
            let expected: Vec<String> = (1..=LINES).map(|n| format!("{n:02}")).collect();
            assert_eq!(numbers, expected, "{family} at {size} pt: the lines");
            assert!(
                lines.windows(2).all(|pair| pair[0].page == pair[1].page),
                "{family} at {size} pt: lines on more than one page"
            );
            let (first, last) = lines
                .first()
                .zip(lines.last())
                .ok_or_else(|| format!("{family} at {size} pt: no lines"))?;
            let ours = line_height(&face, f64::from(size))?;
            let intervals = f64::from(LINES - 1);
            let words = (last.top - first.top) / intervals;
            let limit = (GRID + OFF_GRID) / intervals;
            if (ours - words).abs() > limit {
                writeln!(
                    failures,
                    "{name} for {family} at {size} pt: lines {ours:.3} pt apart, Word's \
                     {words:.3} pt (measured to {limit:.3} pt)"
                )?;
            }
        }
    }
    assert!(failures.is_empty(), "{failures}");
    Ok(())
}

/// A JSON value, enough to read `geometry.json` without a dependency.
enum Json {
    /// `true`, `false` or `null`, which the test does not read.
    Literal,
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn parse(text: &str) -> TestResult<Self> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            at: 0,
        };
        let value = parser.value()?;
        parser.space();
        if parser.at != parser.bytes.len() {
            return Err(format!("text after the JSON value at byte {}", parser.at).into());
        }
        Ok(value)
    }

    fn get(&self, key: &str) -> TestResult<&Self> {
        match self {
            Self::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
                .ok_or_else(|| format!("no member {key:?}").into()),
            _ => Err(format!("not an object, looking for {key:?}").into()),
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

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next(&mut self) -> TestResult<u8> {
        let byte = self.peek().ok_or("unexpected end of JSON")?;
        self.at += 1;
        Ok(byte)
    }

    fn expect(&mut self, expected: u8) -> TestResult {
        match self.next()? {
            byte if byte == expected => Ok(()),
            byte => Err(format!(
                "expected {:?}, found {:?} at byte {}",
                char::from(expected),
                char::from(byte),
                self.at - 1
            )
            .into()),
        }
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn value(&mut self) -> TestResult<Json> {
        self.space();
        match self.peek().ok_or("unexpected end of JSON")? {
            b'{' => {
                self.at += 1;
                let mut members = Vec::new();
                self.space();
                if self.peek() == Some(b'}') {
                    self.at += 1;
                    return Ok(Json::Object(members));
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    self.space();
                    self.expect(b':')?;
                    members.push((key, self.value()?));
                    self.space();
                    match self.next()? {
                        b',' => {}
                        b'}' => return Ok(Json::Object(members)),
                        _ => return Err(format!("bad object at byte {}", self.at - 1).into()),
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.peek() == Some(b']') {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    self.space();
                    match self.next()? {
                        b',' => {}
                        b']' => return Ok(Json::Array(items)),
                        _ => return Err(format!("bad array at byte {}", self.at - 1).into()),
                    }
                }
            }
            b'"' => Ok(Json::String(self.string()?)),
            b't' => self.literal("true"),
            b'f' => self.literal("false"),
            b'n' => self.literal("null"),
            _ => {
                let start = self.at;
                while matches!(
                    self.peek(),
                    Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                ) {
                    self.at += 1;
                }
                let number = std::str::from_utf8(self.bytes.get(start..self.at).ok_or("")?)?;
                Ok(Json::Number(number.parse().map_err(|_| {
                    format!("bad number {number:?} at byte {start}")
                })?))
            }
        }
    }

    fn literal(&mut self, word: &str) -> TestResult<Json> {
        if self.bytes.get(self.at..self.at + word.len()) == Some(word.as_bytes()) {
            self.at += word.len();
            Ok(Json::Literal)
        } else {
            Err(format!("bad literal at byte {}", self.at).into())
        }
    }

    fn string(&mut self) -> TestResult<String> {
        self.expect(b'"')?;
        let mut string = String::new();
        loop {
            // Quotes and backslashes are ASCII, so they never split a UTF-8 sequence.
            let start = self.at;
            while !matches!(self.peek(), Some(b'"' | b'\\') | None) {
                self.at += 1;
            }
            string.push_str(std::str::from_utf8(
                self.bytes.get(start..self.at).ok_or("")?,
            )?);
            match self.next()? {
                b'"' => return Ok(string),
                _ => {
                    let escaped = match self.next()? {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let unit = self.hex()?;
                            let code = if (0xD800..0xDC00).contains(&unit) {
                                self.expect(b'\\')?;
                                self.expect(b'u')?;
                                let low = self.hex()?;
                                if !(0xDC00..0xE000).contains(&low) {
                                    return Err("unpaired surrogate in a JSON string".into());
                                }
                                0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
                            } else {
                                unit
                            };
                            char::from_u32(code).ok_or("bad \\u escape in a JSON string")?
                        }
                        _ => return Err(format!("bad escape at byte {}", self.at - 1).into()),
                    };
                    string.push(escaped);
                }
            }
        }
    }

    fn hex(&mut self) -> TestResult<u32> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .ok_or("short \\u escape")?;
        self.at += 4;
        Ok(u32::from_str_radix(std::str::from_utf8(digits)?, 16)?)
    }
}
