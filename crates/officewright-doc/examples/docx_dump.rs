//! Prints what officewright reads from Word documents.
//!
//! ```text
//! cargo run -p officewright-doc --example docx_dump -- FILE.docx
//! cargo run -p officewright-doc --example docx_dump -- --runs FILE.docx
//! cargo run --release -p officewright-doc --example docx_dump -- --summary FILE.docx...
//! ```
//!
//! With one file it prints the theme's fonts, the compatibility mode, the sections with
//! their page setup,
//! columns and headers and footers, and their blocks. With
//! `--runs` it also prints:
//! - the style, alignment and list of each paragraph, with the list's label template, and
//!   its formatting in effect, and the formatting of its list label;
//! - every run, with the properties written on it and its formatting in effect;
//! - the style, width and layout of each table, and the width and merging of its cells.
//!
//! With `--summary` it opens every file and prints a line for each that does not open,
//! then how many did, how much text they hold and how many runs' formatting was
//! resolved, and the errors by kind.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use officewright_doc::{
    Block, CharacterFormat, ColorValue, Document, Error, Formats, Inline, Numbering,
    ParagraphFormat, Run, RunContent,
};

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
    let theme_fonts = document
        .theme()
        .and_then(|theme| theme.fonts())
        .map(|fonts| {
            format!(
                "{} / {}",
                fonts.major().latin().unwrap_or("?"),
                fonts.minor().latin().unwrap_or("?")
            )
        });
    println!(
        "theme fonts (headings / body): {}; {} fonts declared",
        theme_fonts.as_deref().unwrap_or("no theme"),
        document.fonts().len()
    );
    let settings = document.settings();
    println!(
        "compatibility mode {} ({}), {} legacy options set",
        settings.effective_compatibility_mode().value(),
        if settings.compatibility_mode().is_some() {
            "written"
        } else {
            "none written, Word 2007's"
        },
        settings
            .compatibility_options()
            .filter(|&(_, on)| on)
            .count()
    );
    for (n, section) in document.sections().enumerate() {
        let properties = section.properties();
        let size = properties
            .page_size()
            .map(|size| format!("{} × {} pt", size.width().0, size.height().0));
        println!(
            "section {n}: page {}, start {:?}, {} columns, {} headers, {} footers{}",
            size.as_deref().unwrap_or("not given"),
            properties.start(),
            properties
                .columns()
                .and_then(|columns| columns.count())
                .unwrap_or(1),
            properties.headers().len(),
            properties.footers().len(),
            if properties.title_page() == Some(true) {
                ", title page"
            } else {
                ""
            }
        );
        let numbering = document.numbering();
        let mut formats = document.formats();
        for block in section.blocks() {
            print_block(numbering, &mut formats, block, 1, runs);
        }
    }
    for diagnostic in document.diagnostics() {
        println!("diagnostic: {diagnostic}");
    }
}

fn print_block<'a>(
    numbering: Numbering<'a>,
    formats: &mut Formats<'a>,
    block: Block<'a>,
    depth: usize,
    runs: bool,
) {
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
                if let Some(reference) = properties.numbering() {
                    written.push_str(&format!(
                        " list={:?} level={:?}",
                        reference.instance(),
                        reference.level()
                    ));
                    // The label as the list's definition writes it, without overrides.
                    let level = reference
                        .instance()
                        .and_then(|id| numbering.instance(id)?.definition())
                        .zip(reference.level().and_then(|l| usize::try_from(l).ok()))
                        .and_then(|(definition, level)| definition.level(level));
                    if let Some(level) = level {
                        written.push_str(&format!(
                            " label={:?} format={:?}",
                            level.text().unwrap_or_default(),
                            level.number_format()
                        ));
                    }
                }
                if !written.is_empty() {
                    println!("{indent}  properties:{written}");
                }
                println!(
                    "{indent}  in effect: {}",
                    paragraph_in_effect(&formats.paragraph(paragraph))
                );
                if let Some(label) = formats.list_label(paragraph) {
                    println!("{indent}  label in effect: {}", in_effect(&label));
                }
                for run in paragraph.runs() {
                    println!("{indent}  run {:?}{}", run.text(), describe(run));
                    println!("{indent}    in effect: {}", in_effect(&formats.run(run)));
                }
            }
        }
        Block::Table(table) => {
            println!("{indent}table, {} grid columns", table.grid_columns().len());
            if runs {
                let properties = table.properties();
                println!(
                    "{indent}  properties: style={:?} width={:?} layout={:?} position written={}",
                    properties.style(),
                    properties.width(),
                    properties.layout(),
                    properties.position().is_some()
                );
            }
            for (r, row) in table.rows().enumerate() {
                for (c, cell) in row.cells().enumerate() {
                    if runs {
                        let properties = cell.properties();
                        println!(
                            "{indent}  cell {r},{c}: width={:?} span={:?} vertical merge={:?}",
                            properties.width(),
                            properties.grid_span(),
                            properties.vertical_merge()
                        );
                    } else {
                        println!("{indent}  cell {r},{c}");
                    }
                    for block in cell.blocks() {
                        print_block(numbering, formats, block, depth + 2, runs);
                    }
                }
            }
        }
        Block::ContentControl(control) => {
            println!("{indent}content control");
            for block in control.blocks() {
                print_block(numbering, formats, block, depth + 1, runs);
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
    let mut formatted = 0;
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
                formatted += resolve_every_run(&document);
            }
            Err(err) => {
                println!("error {file}: {err}");
                *kinds.entry(kind(&err)).or_default() += 1;
            }
        }
    }
    println!(
        "\n{opened} of {} files opened, {characters} characters of text, {formatted} runs \
         formatted, {diagnostics} diagnostics",
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

/// Resolves the formatting of every run of the body, in tables and content controls too,
/// and gives how many there were.
fn resolve_every_run(document: &Document) -> usize {
    let mut formats = document.formats();
    let mut count = 0;
    let mut blocks: Vec<Block<'_>> = document.sections().flat_map(|s| s.blocks()).collect();
    while let Some(block) = blocks.pop() {
        match block {
            Block::Paragraph(paragraph) => {
                let _ = formats.paragraph(paragraph);
                let _ = formats.paragraph_mark(paragraph);
                for run in paragraph.runs() {
                    let _ = formats.run(run);
                    count += 1;
                }
            }
            Block::Table(table) => {
                for row in table.rows() {
                    for cell in row.cells() {
                        blocks.extend(cell.blocks());
                    }
                }
            }
            Block::ContentControl(control) => blocks.extend(control.blocks()),
            _ => {}
        }
    }
    count
}

/// The formatting of a paragraph in effect, briefly.
fn paragraph_in_effect(format: &ParagraphFormat) -> String {
    let mut out = format!(
        "{:?} indent {}/{}/{} pt spacing {}/{} pt line {:?}",
        format.justification(),
        format.start_indent().0,
        format.end_indent().0,
        format.first_line_indent().0,
        format.before_spacing().0,
        format.after_spacing().0,
        format.line_spacing()
    );
    if let Some(list) = format.list() {
        out.push_str(&format!(
            " list {} level {} from {}",
            list.instance(),
            list.level_number(),
            list.start()
        ));
    }
    if !format.tabs().is_empty() {
        out.push_str(&format!(" {} tabs", format.tabs().len()));
    }
    out
}

/// The formatting of a run in effect, briefly.
fn in_effect(format: &CharacterFormat) -> String {
    let mut out = format!("{} {}pt", format.fonts().ascii(), format.size().0);
    for (name, on) in [
        ("bold", format.bold()),
        ("italic", format.italic()),
        ("caps", format.caps()),
        ("small-caps", format.small_caps()),
        ("strike", format.strike()),
        ("hidden", format.hidden()),
    ] {
        if on {
            out.push_str(&format!(" {name}"));
        }
    }
    if let ColorValue::Rgb(red, green, blue) = format.color() {
        out.push_str(&format!(" color={red:02X}{green:02X}{blue:02X}"));
    }
    if let Some(underline) = format.underline() {
        out.push_str(&format!(" underline={underline:?}"));
    }
    out
}
