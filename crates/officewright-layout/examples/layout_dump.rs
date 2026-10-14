//! Lays out the paragraphs of a text file one under the other in a column and prints
//! what they draw, one item a line, positions and sizes in twips from the column's top
//! left corner: glyph runs with each glyph's place, rectangles, lines and pictures.
//!
//! ```text
//! cargo run -p officewright-layout --features fuzzing --example layout_dump -- \
//!     FONT_DIR FAMILY WIDTH_PT FILE [--size PT] [--justify]
//! ```
//!
//! Each line of FILE is a paragraph; in it, `\t` is a tab, `\n` a line break, text
//! between `_` is underlined and text between `*` is bold. For example
//! `tests/fonts Calibri 468 notes.txt --justify` lays the file out in Carlito, justified,
//! in the 6.5 inch column of a Letter page with 1 inch margins.

use std::error::Error;

use officewright_core::Pt;
use officewright_font::FontBook;
use officewright_layout::fuzzing;
use officewright_layout::ir::{
    Alignment, BreakClear, Document, Inline, Paragraph, ParagraphProps, RunFonts, RunStyle,
    RunStyleId, Underline,
};

fn main() -> Result<(), Box<dyn Error>> {
    let usage = "usage: layout_dump FONT_DIR FAMILY WIDTH_PT FILE [--size PT] [--justify]";
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(family), Some(width), Some(file)) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        return Err(usage.into());
    };
    let width: f64 = width.parse()?;
    let mut size = 11.0;
    let mut alignment = Alignment::Start;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => size = args.next().ok_or(usage)?.parse()?,
            "--justify" => alignment = Alignment::Both,
            _ => return Err(usage.into()),
        }
    }
    let mut book = FontBook::new();
    book.add_dir(&dir)?;
    let mut document = Document::default();
    let plain = RunStyle::default()
        .with_fonts(RunFonts::new(&family))
        .with_size(Pt(size));
    let styles = Styles {
        plain: document.add_run_style(plain.clone()),
        underlined: document
            .add_run_style(plain.clone().with_underline(Some(Underline::default()))),
        bold: document.add_run_style(plain.clone().with_bold(true)),
        both: document.add_run_style(
            plain
                .with_bold(true)
                .with_underline(Some(Underline::default())),
        ),
    };
    let text = std::fs::read_to_string(&file)?;
    let props = ParagraphProps::default().with_alignment(alignment);
    let paragraphs: Vec<Paragraph> = text
        .lines()
        .map(|line| {
            Paragraph::new(styles.plain)
                .with_props(props.clone())
                .with_inlines(inlines(line, &styles))
        })
        .collect();
    let painted = fuzzing::paint(&book, &document, &paragraphs, width);
    print!("{}", painted.text());
    for entry in painted.report().iter() {
        eprintln!("{:?}: {}", entry.kind(), entry.message());
    }
    Ok(())
}

/// The run styles of the file's markup.
struct Styles {
    plain: RunStyleId,
    underlined: RunStyleId,
    bold: RunStyleId,
    both: RunStyleId,
}

/// The inlines of a line of the file: text, tabs (`\t`) and line breaks (`\n`), in the
/// styles `_` and `*` switch.
fn inlines(line: &str, styles: &Styles) -> Vec<Inline> {
    let mut inlines = Vec::new();
    let (mut underlined, mut bold) = (false, false);
    let mut text = String::new();
    let style = |underlined: bool, bold: bool| match (underlined, bold) {
        (false, false) => styles.plain,
        (true, false) => styles.underlined,
        (false, true) => styles.bold,
        (true, true) => styles.both,
    };
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        let piece = match ch {
            '_' | '*' => None,
            '\\' if matches!(chars.peek(), Some('t' | 'n')) => {
                let next = chars.next();
                Some(next == Some('t'))
            }
            _ => {
                text.push(ch);
                continue;
            }
        };
        if !text.is_empty() {
            inlines.push(Inline::text(
                std::mem::take(&mut text),
                style(underlined, bold),
            ));
        }
        match (ch, piece) {
            ('_', _) => underlined = !underlined,
            ('*', _) => bold = !bold,
            (_, Some(true)) => inlines.push(Inline::tab(style(underlined, bold))),
            (_, Some(false)) => {
                inlines.push(Inline::line_break(
                    BreakClear::None,
                    style(underlined, bold),
                ));
            }
            _ => {}
        }
    }
    if !text.is_empty() {
        inlines.push(Inline::text(text, style(underlined, bold)));
    }
    inlines
}
