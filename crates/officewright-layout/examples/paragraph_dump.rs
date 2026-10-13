//! Prints how the layout itemizes and shapes a paragraph: its runs, each with its text,
//! the font that draws it, its size, script and line height, and what the layout
//! reports about the fonts.
//!
//! ```text
//! cargo run -p officewright-layout --features fuzzing --example paragraph_dump -- \
//!     FONT_DIR FAMILY TEXT [--size PT] [--bold] [--italic] [--caps] [--small-caps] \
//!     [--symbol FONT CODE]
//! ```
//!
//! For example `tests/fonts Calibri "Добрый день Ԕ" --small-caps` draws Calibri in
//! Carlito and Ԕ, which Carlito lacks, in Noto Sans; `--symbol Wingdings F0A7` adds a
//! list bullet of a symbol font that is not installed.

use std::error::Error;

use officewright_core::Pt;
use officewright_font::FontBook;
use officewright_layout::fuzzing;
use officewright_layout::ir::{Document, Inline, RunFonts, RunStyle};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: paragraph_dump FONT_DIR FAMILY TEXT [--size PT] [--bold] [--italic] \
                 [--caps] [--small-caps] [--symbol FONT CODE]";
    let (Some(dir), Some(family), Some(text)) = (args.next(), args.next(), args.next()) else {
        return Err(usage.into());
    };
    let mut style = RunStyle::default().with_fonts(RunFonts::new(&family));
    let mut symbols = Vec::new();
    while let Some(arg) = args.next() {
        style = match arg.as_str() {
            "--size" => style.with_size(Pt(args.next().ok_or(usage)?.parse()?)),
            "--bold" => style.with_bold(true),
            "--italic" => style.with_italic(true),
            "--caps" => style.with_caps(true),
            "--small-caps" => style.with_small_caps(true),
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
    let mut inlines = vec![Inline::text(text, id)];
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
    Ok(())
}
