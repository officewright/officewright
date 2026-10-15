//! Prints how the layout itemizes and shapes a paragraph: its runs, each with its text,
//! the font that draws it, its size, script and line height, and what the layout
//! reports about the fonts. With `--width`, also the lines the paragraph breaks into in
//! a column that wide: each line's text, height and baseline, and where its words start
//! and end.
//!
//! ```text
//! cargo run -p officewright-layout --features fuzzing --example paragraph_dump -- \
//!     FONT_DIR FAMILY TEXT [--size PT] [--bold] [--italic] [--caps] [--small-caps] \
//!     [--symbol FONT CODE] [--width PT] [--justify]
//! ```
//!
//! For example `tests/fonts Calibri "Добрый день Ԕ" --small-caps` draws Calibri in
//! Carlito and Ԕ, which Carlito lacks, in Noto Sans; `--symbol Wingdings F0A7` adds a
//! list bullet of a symbol font that is not installed. In TEXT, `\t` is a tab and `\n`
//! a line break.

use std::error::Error;

use officewright_core::Pt;
use officewright_font::FontBook;
use officewright_layout::fuzzing;
use officewright_layout::ir::{
    Alignment, BreakClear, Document, Inline, Paragraph, ParagraphProps, RunFonts, RunStyle,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: paragraph_dump FONT_DIR FAMILY TEXT [--size PT] [--bold] [--italic] \
                 [--caps] [--small-caps] [--symbol FONT CODE] [--width PT] [--justify]";
    let (Some(dir), Some(family), Some(text)) = (args.next(), args.next(), args.next()) else {
        return Err(usage.into());
    };
    let mut style = RunStyle::default().with_fonts(RunFonts::new(family.as_str()));
    let mut symbols = Vec::new();
    let mut width = None;
    let mut alignment = Alignment::Start;
    while let Some(arg) = args.next() {
        style = match arg.as_str() {
            "--size" => style.with_size(Pt(args.next().ok_or(usage)?.parse()?)),
            "--bold" => style.with_bold(true),
            "--italic" => style.with_italic(true),
            "--caps" => style.with_caps(true),
            "--small-caps" => style.with_small_caps(true),
            "--width" => {
                width = Some(args.next().ok_or(usage)?.parse::<f64>()?);
                style
            }
            "--justify" => {
                alignment = Alignment::Both;
                style
            }
            "--symbol" => {
                let font = args.next().ok_or(usage)?;
                let code = u16::from_str_radix(&args.next().ok_or(usage)?, 16)?;
                symbols.push((font, code));
                style
            }
            _ => return Err(usage.into()),
        };
    }
    let mut book = FontBook::new();
    book.add_dir(&dir)?;
    let mut document = Document::default();
    let id = document.add_run_style(style);
    let mut inlines = Vec::new();
    for (index, line) in text.split("\\n").enumerate() {
        if index > 0 {
            inlines.push(Inline::line_break(BreakClear::None, id));
        }
        for (index, piece) in line.split("\\t").enumerate() {
            if index > 0 {
                inlines.push(Inline::tab(id));
            }
            if !piece.is_empty() {
                inlines.push(Inline::text(piece, id));
            }
        }
    }
    for (font, code) in symbols {
        inlines.push(Inline::symbol(font, code, id));
    }
    let paragraph = fuzzing::paragraph(&book, &document, &inlines);
    println!("text\tsource\tfont\tsize\tscript\tsynthetic\tglyphs\tline height");
    for run in paragraph.runs() {
        let synthetic = match (run.synthetic_bold(), run.synthetic_italic()) {
            (true, true) => "bold italic",
            (true, false) => "bold",
            (false, true) => "italic",
            (false, false) => "-",
        };
        println!(
            "{:?}\t{:?}\t{}\t{}\t{}\t{synthetic}\t{}\t{}",
            run.text(),
            run.source(),
            run.face().unwrap_or("(none)"),
            run.size(),
            run.script(),
            run.glyphs().len(),
            run.line()
                .map_or_else(|| "-".to_owned(), |line| format!("{:.3}", line.height)),
        );
    }
    for entry in paragraph.report().iter() {
        println!("{:?}: {}", entry.kind(), entry.message());
    }
    if let Some(width) = width {
        let props = ParagraphProps::default().with_alignment(alignment);
        let paragraph = Paragraph::new(id).with_props(props).with_inlines(inlines);
        let laid = fuzzing::lay_out(&book, &document, &paragraph, width);
        println!("\nlines in {width} pt:");
        for line in laid.lines() {
            println!(
                "{:?}\twidth {:.2}\theight {:.3}\tbaseline {:.3}\tends: {}",
                line.text(),
                line.width(),
                line.height(),
                line.baseline(),
                line.end()
            );
            for word in line.words() {
                println!(
                    "    {:8.2} {:8.2}  {}",
                    word.start(),
                    word.end(),
                    word.text()
                );
            }
        }
    }
    Ok(())
}
