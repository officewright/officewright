//! What the tests with Word's reference renders share: the repository, the test fonts,
//! and the lines of a rule document's `geometry.json`.

// reason: each test file uses a part of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use officewright_core::Pt;
use officewright_font::FontBook;
use officewright_layout::ir::{
    Document, Indent, Inline, LineSpacing, Paragraph, ParagraphProps, RunFonts, RunStyle, Spacing,
    TabStop,
};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn book() -> TestResult<FontBook> {
    let mut book = FontBook::new();
    book.add_dir(repository().join("tests/fonts"))?;
    Ok(book)
}

/// A line of Word's reference render: its text and the boxes of its words, in points
/// from the top left of the page.
pub struct WordLine {
    pub page: usize,
    pub text: String,
    pub top: f64,
    pub words: Vec<(String, f64, f64)>,
}

/// The lines of `geometry.json` of rule document `name`.
pub fn word_lines(name: &str) -> TestResult<Vec<WordLine>> {
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

/// Just enough JSON for `geometry.json`.
#[derive(Debug)]
pub enum Json {
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
    Other,
}

impl Json {
    pub fn parse(text: &str) -> TestResult<Self> {
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

    pub fn get(&self, key: &str) -> TestResult<&Self> {
        match self {
            Self::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
                .ok_or_else(|| format!("no {key}").into()),
            _ => Err(format!("not an object looking for {key}").into()),
        }
    }

    pub fn array(&self) -> TestResult<&[Self]> {
        match self {
            Self::Array(items) => Ok(items),
            _ => Err("not an array".into()),
        }
    }

    pub fn number(&self) -> TestResult<f64> {
        match self {
            Self::Number(number) => Ok(*number),
            _ => Err("not a number".into()),
        }
    }

    pub fn string(&self) -> TestResult<&str> {
        match self {
            Self::String(string) => Ok(string),
            _ => Err("not a string".into()),
        }
    }
}

/// A paragraph built from `seed`, as the fuzz target `layout_shape` builds them, with
/// line and page breaks, inline objects and kerning added; and its column's width.
pub fn random_paragraph(seed: u64, document: &mut Document) -> (Paragraph, f64) {
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
