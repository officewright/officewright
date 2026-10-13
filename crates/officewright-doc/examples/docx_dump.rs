//! Prints what officewright reads from Word documents.
//!
//! ```text
//! cargo run -p officewright-doc --example docx_dump -- FILE.docx
//! cargo run -p officewright-doc --example docx_dump -- --runs FILE.docx
//! cargo run --release -p officewright-doc --example docx_dump -- --summary FILE.docx...
//! ```
//!
//! With one file it prints the sections, their page setup and their blocks, and with
//! `--runs` also the style, alignment and list of each paragraph and every run with the
//! properties written on it. With
//! `--summary` it opens every file and prints a line for each that does not open, then
//! how many did and how much text they hold, and the errors by kind.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use officewright_doc::{Block, Document, Error, Inline, Run, RunContent};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((flag, files)) if flag == "--summary" && !files.is_empty() => summary(files),
        Some((flag, [file])) if flag == "--runs" => open_and_dump(file, true),
        Some((file, [])) => open_and_dump(file, false),
        _ => {
            eprintln!("usage: docx_dump [--runs] FILE | docx_dump --summary FILE...");
            ExitCode::FAILURE
        }
    }
}

fn open_and_dump(file: &str, runs: bool) -> ExitCode {
    match Document::open(file) {
        Ok(document) => {
            dump(&document, runs);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{file}: {err}");
            ExitCode::FAILURE
        }
    }
}

fn dump(document: &Document, runs: bool) {
    println!("{:?}, {:?}", document.kind(), document.conformance());
    let styles = document.styles();
    let defaults = styles.default_run_properties();
    println!(
        "{} styles; default font size {:?}",
        styles.iter().len(),
        defaults.size().map(|size| size.0)
    );
    for (n, section) in document.sections().enumerate() {
        let properties = section.properties();
        let size = properties
            .page_size()
            .map(|size| format!("{} × {} pt", size.width().0, size.height().0));
        println!(
            "section {n}: page {}, start {:?}",
            size.as_deref().unwrap_or("not given"),
            properties.start()
        );
        for block in section.blocks() {
            print_block(block, 1, runs);
        }
    }
    for diagnostic in document.diagnostics() {
        println!("diagnostic: {diagnostic}");
    }
}

fn print_block(block: Block<'_>, depth: usize, runs: bool) {
    let indent = "  ".repeat(depth);
    match block {
        Block::Paragraph(paragraph) => {
            let others = paragraph
                .inlines()
                .filter(|inline| matches!(inline, Inline::Other(_)))
                .count();
            let drawings = paragraph
                .runs()
                .flat_map(|run| run.content())
                .filter(|item| matches!(item, RunContent::Other(_)))
                .count();
            let mut note = String::new();
            if others + drawings > 0 {
                note = format!(" (+{} kept as written)", others + drawings);
            }
            println!("{indent}paragraph {:?}{note}", paragraph.text());
            if runs {
                let properties = paragraph.properties();
                let mut written = String::new();
                if let Some(style) = properties.style() {
                    written.push_str(&format!(" style={style}"));
                }
                if let Some(justification) = properties.justification() {
                    written.push_str(&format!(" justification={justification:?}"));
                }
                if let Some(numbering) = properties.numbering() {
                    written.push_str(&format!(
                        " list={:?} level={:?}",
                        numbering.instance(),
                        numbering.level()
                    ));
                }
                if !written.is_empty() {
                    println!("{indent}  properties:{written}");
                }
                for run in paragraph.runs() {
                    println!("{indent}  run {:?}{}", run.text(), describe(run));
                }
            }
        }
        Block::Table(table) => {
            println!("{indent}table, {} grid columns", table.grid_columns().len());
            for (r, row) in table.rows().enumerate() {
                for (c, cell) in row.cells().enumerate() {
                    println!("{indent}  cell {r},{c}");
                    for block in cell.blocks() {
                        print_block(block, depth + 2, runs);
                    }
                }
            }
        }
        Block::ContentControl(control) => {
            println!("{indent}content control");
            for block in control.blocks() {
                print_block(block, depth + 1, runs);
            }
        }
        Block::Other(other) => println!("{indent}kept as written: {}", other.local_name()),
        _ => println!("{indent}(a kind of block this example does not know)"),
    }
}

fn summary(files: &[String]) -> ExitCode {
    let mut opened = 0;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut diagnostics = 0;
    let mut characters = 0;
    let mut slowest = (Duration::ZERO, "");
    for file in files {
        let start = Instant::now();
        let result = Document::open(file);
        let elapsed = start.elapsed();
        if elapsed > slowest.0 {
            slowest = (elapsed, file);
        }
        match result {
            Ok(document) => {
                opened += 1;
                diagnostics += document.diagnostics().len();
                // Reading the text walks the whole tree.
                characters += document.text().chars().count();
            }
            Err(err) => {
                println!("error {file}: {err}");
                *kinds.entry(kind(&err)).or_default() += 1;
            }
        }
    }
    println!(
        "\n{opened} of {} files opened, {characters} characters of text, {diagnostics} diagnostics",
        files.len()
    );
    for (kind, count) in &kinds {
        println!("{count:>6} {kind}");
    }
    println!("slowest: {:.1?} {}", slowest.0, slowest.1);
    ExitCode::SUCCESS
}

fn kind(err: &Error) -> String {
    match err {
        Error::Io { .. } => "io".into(),
        Error::Malformed { .. } => "malformed".into(),
        Error::LimitExceeded { limit, .. } => format!("limit: {limit}"),
        Error::Unsupported(_) => "unsupported".into(),
        _ => "other".into(),
    }
}

/// The properties written on a run, briefly.
fn describe(run: Run<'_>) -> String {
    let properties = run.properties();
    let mut out = String::new();
    if let Some(style) = properties.style() {
        out.push_str(&format!(" style={style}"));
    }
    if let Some(font) = properties.fonts().ascii() {
        out.push_str(&format!(" font={font}"));
    }
    if let Some(size) = properties.size() {
        out.push_str(&format!(" size={}pt", size.0));
    }
    for (name, value) in [
        ("bold", properties.bold()),
        ("italic", properties.italic()),
        ("caps", properties.caps()),
        ("strike", properties.strike()),
        ("hidden", properties.vanish()),
    ] {
        if let Some(value) = value {
            out.push_str(&format!(" {name}={value}"));
        }
    }
    if let Some(color) = properties.color() {
        out.push_str(&format!(" color={:?}", color.value()));
    }
    if let Some(underline) = properties.underline() {
        out.push_str(&format!(" underline={:?}", underline.style()));
    }
    out
}
