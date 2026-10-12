//! Prints what officewright reads from Word documents.
//!
//! ```text
//! cargo run -p officewright-doc --example docx_dump -- FILE.docx
//! cargo run --release -p officewright-doc --example docx_dump -- --summary FILE.docx...
//! ```
//!
//! With one file it prints the sections, their page setup and their paragraphs. With
//! `--summary` it opens every file and prints a line for each that does not open,
//! then how many did, by error.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use officewright_doc::{Block, Document, Error, Inline, RunContent};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((flag, files)) if flag == "--summary" && !files.is_empty() => summary(files),
        Some((file, [])) => match Document::open(file) {
            Ok(document) => {
                dump(&document);
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("{file}: {err}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: docx_dump FILE | docx_dump --summary FILE...");
            ExitCode::FAILURE
        }
    }
}

fn dump(document: &Document) {
    println!("{:?}, {:?}", document.kind(), document.conformance());
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
            print_block(block, 1);
        }
    }
    for diagnostic in document.diagnostics() {
        println!("diagnostic: {diagnostic}");
    }
}

fn print_block(block: Block<'_>, depth: usize) {
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
        }
        Block::Other(other) => println!("{indent}kept as written: {}", other.local_name()),
        _ => println!("{indent}(a kind of block this example does not know)"),
    }
}

fn summary(files: &[String]) -> ExitCode {
    let mut opened = 0;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut diagnostics = 0;
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
                let _ = document.text();
            }
            Err(err) => {
                println!("error {file}: {err}");
                *kinds.entry(kind(&err)).or_default() += 1;
            }
        }
    }
    println!(
        "\n{opened} of {} files opened, {diagnostics} diagnostics",
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
