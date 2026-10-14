//! Renders a Word document into PDF and prints the report of what the PDF shows
//! otherwise than Word would, for checking the conversion by hand.
//!
//! ```text
//! cargo run -p officewright-doc --features pdf --example docx_to_pdf -- \
//!     IN.docx OUT.pdf [--font-dir DIR]... [--no-system-fonts] [--max-pages N]
//! ```
//!
//! It prints the number of pages, then each entry of the report: where in the document,
//! what kind, and what happened.

use std::process::ExitCode;

use officewright_doc::{Document, RenderOptions};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!(
            "usage: docx_to_pdf IN.docx OUT.pdf [--font-dir DIR]... [--no-system-fonts] [--max-pages N]"
        );
        return ExitCode::FAILURE;
    };
    let mut options = RenderOptions::new();
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--font-dir", Some(dir)) => options = options.with_font_dir(dir),
            ("--max-pages", Some(n)) => match n.parse() {
                Ok(n) => options = options.with_max_pages(n),
                Err(_) => {
                    eprintln!("--max-pages takes a number, not {n}");
                    return ExitCode::FAILURE;
                }
            },
            ("--no-system-fonts", next) => {
                options = options.with_system_fonts(false);
                if let Some(next) = next {
                    eprintln!("unexpected argument {next}");
                    return ExitCode::FAILURE;
                }
            }
            (other, _) => {
                eprintln!("unknown or incomplete argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let document = match Document::open(&input) {
        Ok(document) => document,
        Err(err) => {
            eprintln!("{input}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let (pdf, report) = match document.save_pdf_bytes(&options) {
        Ok(rendered) => rendered,
        Err(err) => {
            eprintln!("{input}: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = std::fs::write(&output, &pdf) {
        eprintln!("{output}: {err}");
        return ExitCode::FAILURE;
    }
    println!("{output}: {} bytes", pdf.len());
    for entry in report.iter() {
        println!("{:?} {entry}", entry.kind());
    }
    ExitCode::SUCCESS
}
