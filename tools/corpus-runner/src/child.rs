//! The child: opens one file, renders it into PDF when asked, and prints one line of JSON
//! about it.

use std::path::PathBuf;
use std::process::ExitCode;

use officewright_doc::{Document, Error, LayoutReport, RenderOptions, ReportKind};
use serde::{Deserialize, Serialize};

/// What the child prints.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Opened {
    /// `opened` or `error`.
    pub status: String,
    /// For `error`: the kind (`io`, `malformed`, `limit`, `unsupported`) and message.
    pub error_kind: Option<String>,
    pub error: Option<String>,
    /// Characters of the document's text, and its diagnostics.
    pub chars: Option<u64>,
    pub diagnostics: Option<u64>,
    /// The child's peak resident memory in KiB, from /proc/self/status (Linux).
    pub peak_rss_kb: Option<u64>,
    /// With `--render`: why the opened document could not be rendered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_error: Option<String>,
    /// With `--render`: what the PDF shows otherwise than Word would, entry by entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_report: Option<Vec<LayoutEntry>>,
}

/// An entry of the layout's report: its kind, as the CLI names it in its JSON report
/// (`font-substituted`, `feature-unsupported`, ...), where it is, and what it says.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LayoutEntry {
    pub kind: String,
    pub location: String,
    pub message: String,
}

/// What the child renders the document into, with `--render PDF`: the PDF to write, laid
/// out with the fonts of each `--font-dir` and none of the system's, so that a run gives
/// the same pages on every machine.
#[derive(Debug, Default, PartialEq)]
pub struct Render {
    pub pdf: PathBuf,
    pub font_dirs: Vec<PathBuf>,
}

/// The child's arguments: `FILE [--render PDF [--font-dir DIR]...]`.
pub fn parse(args: &[String]) -> Result<(String, Option<Render>), String> {
    let mut args = args.iter();
    let file = args.next().ok_or("open needs a file")?.clone();
    let mut pdf = None;
    let mut font_dirs = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--render" => pdf = Some(PathBuf::from(value("--render")?)),
            "--font-dir" => font_dirs.push(PathBuf::from(value("--font-dir")?)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    match pdf {
        Some(pdf) => Ok((file, Some(Render { pdf, font_dirs }))),
        None if font_dirs.is_empty() => Ok((file, None)),
        None => Err("--font-dir needs --render".to_owned()),
    }
}

/// Makes the child misbehave, for the runner's own tests: `panic`, `hang`, `abort` or
/// `allocation` (prints what Rust prints when an allocation fails, then aborts).
pub const MISBEHAVE: &str = "CORPUS_RUNNER_MISBEHAVE";

pub fn main(args: &[String]) -> ExitCode {
    let (file, render) = match parse(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("corpus-runner open: {message}");
            return ExitCode::from(2);
        }
    };
    misbehave();
    let mut opened = match Document::open(&file) {
        Ok(document) => {
            let mut opened = Opened {
                status: "opened".to_owned(),
                error_kind: None,
                error: None,
                chars: Some(document.text().chars().count() as u64),
                diagnostics: Some(document.diagnostics().len() as u64),
                peak_rss_kb: None,
                render_error: None,
                layout_report: None,
            };
            if let Some(render) = &render {
                match render_pdf(&document, render) {
                    Ok(report) => opened.layout_report = Some(entries(&report)),
                    Err(error) => opened.render_error = Some(error),
                }
            }
            opened
        }
        Err(error) => Opened {
            status: "error".to_owned(),
            error_kind: Some(kind(&error).to_owned()),
            error: Some(error.to_string()),
            chars: None,
            diagnostics: None,
            peak_rss_kb: None,
            render_error: None,
            layout_report: None,
        },
    };
    // After the rendering, so that its memory counts.
    opened.peak_rss_kb = peak_rss_kb();
    match serde_json::to_string(&opened) {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("corpus-runner: cannot write the result: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Renders `document` into `render.pdf`, making its folder; its report, or why it failed
/// with each cause.
fn render_pdf(document: &Document, render: &Render) -> Result<LayoutReport, String> {
    if let Some(folder) = render.pdf.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("cannot make {}: {error}", folder.display()))?;
    }
    let mut options = RenderOptions::new().with_system_fonts(false);
    for dir in &render.font_dirs {
        options = options.with_font_dir(dir);
    }
    document
        .save_pdf_path(&render.pdf, &options)
        .map_err(|error| with_causes(&error))
}

/// `error` with each of its causes.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message = format!("{message}: {cause}");
        source = cause.source();
    }
    message
}

/// The entries of `report`, each kind by the name the CLI's JSON report gives it.
fn entries(report: &LayoutReport) -> Vec<LayoutEntry> {
    report
        .entries()
        .iter()
        .map(|entry| LayoutEntry {
            kind: kind_name(entry.kind()).to_owned(),
            location: entry.location().to_string(),
            message: entry.message().to_owned(),
        })
        .collect()
}

/// The name of a report kind, as `officewright docx-to-pdf --report` writes it; `other`
/// for a kind this version does not know.
pub fn kind_name(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::FontSubstituted => "font-substituted",
        ReportKind::GlyphMissing => "glyph-missing",
        ReportKind::FeatureUnsupported => "feature-unsupported",
        ReportKind::FieldCached => "field-cached",
        ReportKind::CompatIgnored => "compat-ignored",
        ReportKind::ValueClamped => "value-clamped",
        ReportKind::Overflow => "overflow",
        ReportKind::FontEmbedding => "font-embedding",
        _ => "other",
    }
}

fn kind(error: &Error) -> &'static str {
    match error {
        Error::Io { .. } => "io",
        Error::Malformed { .. } => "malformed",
        Error::LimitExceeded { .. } => "limit",
        Error::Unsupported(_) => "unsupported",
        _ => "other",
    }
}

/// VmHWM of /proc/self/status, the peak resident set size, in KiB.
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn misbehave() {
    match std::env::var(MISBEHAVE).as_deref() {
        // reason: the runner's tests need a child that panics like a library bug would.
        #[allow(clippy::panic)]
        Ok("panic") => panic!("misbehaving on purpose"),
        Ok("hang") => std::thread::sleep(std::time::Duration::from_secs(3600)),
        Ok("abort") => std::process::abort(),
        Ok("allocation") => {
            eprintln!("memory allocation of 4294967296 bytes failed");
            std::process::abort()
        }
        _ => {}
    }
}
