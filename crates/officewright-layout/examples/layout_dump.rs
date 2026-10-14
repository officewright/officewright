//! Lays out the paragraphs of a text file one under the other in a column and prints
//! what they draw, one item a line, positions and sizes in twips from the column's top
//! left corner: glyph runs with each glyph's place, rectangles, lines and pictures.
//! With `--page`, lays them out on pages instead, with margins of 1 inch, and prints
//! each page and what it draws, positions from the page's top left corner; `--footer`
//! adds a footer to every page, "Page N of M", centred. With the feature `pdf` too,
//! `--pdf FILE` writes the pages as a PDF instead.
//!
//! ```text
//! cargo run -p officewright-layout --features fuzzing --example layout_dump -- \
//!     FONT_DIR FAMILY WIDTH_PT FILE [--size PT] [--justify]
//!     [--page HEIGHT_PT [--footer] [--pdf FILE]]
//! ```
//!
//! Each line of FILE is a paragraph; in it, `\t` is a tab, `\n` a line break, `\p` a
//! page break, text between `_` is underlined and text between `*` is bold. For example
//! `tests/fonts Calibri 468 notes.txt --justify --page 792` lays the file out in
//! Carlito, justified, on Letter pages with 1 inch margins.

use std::error::Error;

use officewright_core::{PageSize, Pt};
use officewright_font::FontBook;
use officewright_layout::fuzzing;
use officewright_layout::ir::{
    Alignment, BreakClear, Document, Field, FieldKind, HeaderFooterSet, Inline, PageSetup,
    Paragraph, ParagraphProps, RunFonts, RunStyle, RunStyleId, Section, Underline,
};

fn main() -> Result<(), Box<dyn Error>> {
    let usage = "usage: layout_dump FONT_DIR FAMILY WIDTH_PT FILE [--size PT] [--justify] \
                 [--page HEIGHT_PT [--footer] [--pdf FILE]]";
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(family), Some(width), Some(file)) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        return Err(usage.into());
    };
    let width: f64 = width.parse()?;
    let mut size = 11.0;
    let mut alignment = Alignment::Start;
    let mut page: Option<f64> = None;
    let mut footer = false;
    let mut pdf: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => size = args.next().ok_or(usage)?.parse()?,
            "--justify" => alignment = Alignment::Both,
            "--page" => page = Some(args.next().ok_or(usage)?.parse()?),
            "--footer" => footer = true,
            "--pdf" => pdf = Some(args.next().ok_or(usage)?),
            _ => return Err(usage.into()),
        }
    }
    // Only pages are written as a PDF.
    if pdf.is_some() && page.is_none() {
        return Err(usage.into());
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
    if let Some(height) = page {
        let setup = PageSetup::new(PageSize::new(Pt(width + 144.0), Pt(height)));
        let blocks = paragraphs.into_iter().map(Into::into).collect::<Vec<_>>();
        let mut section = Section::new(setup).with_blocks(blocks);
        if footer {
            let field = |kind| Inline::Field(Field::new(kind, styles.plain));
            let centred = ParagraphProps::default().with_alignment(Alignment::Center);
            let numbers = Paragraph::new(styles.plain)
                .with_props(centred)
                .with_inlines([
                    Inline::text("Page ", styles.plain),
                    field(FieldKind::Page),
                    Inline::text(" of ", styles.plain),
                    field(FieldKind::NumPages),
                ]);
            section =
                section.with_footers(HeaderFooterSet::default().with_primary([numbers.into()]));
        }
        document.add_section(section);
        if let Some(file) = pdf {
            return write_pdf(&document, &book, &file);
        }
        let laid = fuzzing::paginate(&book, &document, 10_000)?;
        for page in laid.pages() {
            let (width, height) = page.size();
            println!("page {} ({width} x {height} pt)", page.number());
            print!("{}", page.text());
        }
        for entry in laid.report().iter() {
            eprintln!("{:?}: {}", entry.kind(), entry.message());
        }
        return Ok(());
    }
    let painted = fuzzing::paint(&book, &document, &paragraphs, width);
    print!("{}", painted.text());
    for entry in painted.report().iter() {
        eprintln!("{:?}: {}", entry.kind(), entry.message());
    }
    Ok(())
}

/// Renders `document` with the fonts of `book` into the PDF `file`, and prints what the
/// layout reported.
#[cfg(feature = "pdf")]
fn write_pdf(document: &Document, book: &FontBook, file: &str) -> Result<(), Box<dyn Error>> {
    // The fonts are those of `book`, from FONT_DIR: the options' font settings only make
    // a book.
    let options = officewright_layout::RenderOptions::new();
    let rendered = officewright_layout::render_pdf(document, book, &options)?;
    rendered.pdf().save(file)?;
    for entry in rendered.report() {
        eprintln!("{:?}: {}", entry.kind(), entry.message());
    }
    Ok(())
}

/// Without the feature `pdf`, there is no PDF to write.
#[cfg(not(feature = "pdf"))]
fn write_pdf(_: &Document, _: &FontBook, _: &str) -> Result<(), Box<dyn Error>> {
    Err("--pdf needs the feature pdf".into())
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
            '\\' if matches!(chars.peek(), Some('t' | 'n' | 'p')) => chars.next(),
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
        let current = style(underlined, bold);
        match (ch, piece) {
            ('_', _) => underlined = !underlined,
            ('*', _) => bold = !bold,
            (_, Some('t')) => inlines.push(Inline::tab(current)),
            (_, Some('n')) => inlines.push(Inline::line_break(BreakClear::None, current)),
            (_, Some('p')) => inlines.push(Inline::page_break(current)),
            _ => {}
        }
    }
    if !text.is_empty() {
        inlines.push(Inline::text(text, style(underlined, bold)));
    }
    inlines
}
