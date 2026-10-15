//! Runs officewright over corpora of documents and compares the results with a baseline.
//!
//! ```text
//! corpus-runner run [--root DIR] [--report FILE]
//!                   [--baseline FILE [--update-baseline | --allow-better] [--statuses-only]]
//!                   [--expected FILE] [--timeout SECONDS] [--memory-limit MIB]
//!                   [--enforce-memory-limit] [--jobs N]
//!                   [--render DIR [--font-dir DIR]...] [--references DIR [--ours DIR]]
//!                   PATH...
//! corpus-runner score --ours FILE.pdf --reference DIR
//! ```
//!
//! Every Word document under the paths (`.docx`, `.docm`, `.dotx`, `.dotm`) is opened in a
//! child process of its own (this program with `open FILE`), so a panic, a hang or a memory
//! blow-up in one file is a result for that file, not the end of the run. A path named here
//! is followed if it is a symbolic link; inside a directory, links to documents are followed
//! and links to directories are not. A child that
//! takes longer than the time-out (30 s) is stopped. Paths in the report, the baseline and
//! the expected-errors file are relative to the root (by default the current directory)
//! with forward slashes. Each file ends as
//!
//! - `opened`: the document opened;
//! - `expected-error`: it did not, and the expected-errors file lists it with a reason
//!   (encrypted, a binary Word file named `.docx`, a deliberately broken file);
//! - `unexpected-error`: it did not, and it is not listed;
//! - `panic`, `timeout`, `memory` (the peak memory exceeded the memory limit, 1 GiB, as
//!   measured on Linux; or an allocation failed under that limit, which
//!   `--enforce-memory-limit` sets on each child with `prlimit`, as `nightly.sh` does) or
//!   `crash` (ended by a signal, or by an exit the child does not make).
//!
//! The report (`--report`) records every file with its status, error, duration, peak memory
//! (Linux), characters of text and diagnostics. The run fails when a file panics, hangs,
//! runs out of memory or crashes. With `--baseline` it also fails when any file's status
//! differs from the baseline's, or the set of files does: worse is a regression, and better
//! is accepted by running with `--update-baseline`, which writes the baseline, and committing
//! it, so that progress shows in the diff. With `--allow-better` only worse files fail, for
//! the nightly run over the external corpora, whose baseline is updated by hand.
//!
//! With `--references tests/corpus/reference --ours DIR`, every document that has a
//! reference folder (its `geometry.json` names the document, relative to the folder above
//! `reference/`) is also scored: our PDF of it, `DIR/<that path>.pdf`, against Word's
//! pages and geometry. The scores (page counts, text, line endings, word positions, page
//! breaks, SSIM per page) go into the report, and the baseline keeps the fractions among
//! them: a drop of more than 0.01 fails, as a rise does until it is accepted. `score`
//! prints the same metrics for one PDF and one reference folder. Both need pdftotext and
//! pdftoppm (poppler).
//!
//! With `--render DIR`, each child also renders the document it opened into PDF, with the
//! fonts of each `--font-dir` and none of the system's, so that a run gives the same pages
//! on every machine. The PDF goes to `DIR/<path>.pdf`, the path relative to the folder above
//! `--references` when the document lies under it, else relative to the root. The report
//! records what the layout reported for each document, or why it could not be rendered.
//! With `--references` too, the PDFs rendered are the ones scored, unless `--ours` names
//! others.
//!
//! `--statuses-only` compares only the files' statuses with the baseline, and keeps the
//! baseline's scores with `--update-baseline`: for a machine that cannot score, such as
//! Windows, which has no official poppler build.
#![forbid(unsafe_code)]

mod align;
mod child;
mod gate;
mod geometry;
mod metrics;
mod raster;
mod report;
mod run;
mod ssim;
mod text;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "usage: corpus-runner run [--root DIR] [--report FILE] \
[--baseline FILE [--update-baseline | --allow-better] [--statuses-only]] [--expected FILE] \
[--timeout SECONDS] \
[--memory-limit MIB] [--enforce-memory-limit] [--jobs N] [--render DIR [--font-dir DIR]...] \
[--references DIR [--ours DIR]] PATH...
       corpus-runner score --ours FILE.pdf --reference DIR
       corpus-runner gate [--main MAIN.json] [--same-fonts] REPORT.json
       corpus-runner open FILE [--render PDF [--font-dir DIR]...]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((command, rest)) if command == "open" => child::main(rest),
        Some((command, rest)) if command == "score" => score(rest),
        Some((command, rest)) if command == "gate" => gate(rest),
        Some((command, rest)) if command == "run" => match parse(rest) {
            Ok(options) => run::main(&options),
            Err(message) => {
                eprintln!("corpus-runner: {message}\n{USAGE}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Prints the metrics of one PDF of ours against one reference folder, as JSON.
fn score(args: &[String]) -> ExitCode {
    let (ours, reference) = match args {
        [a, ours, b, reference] if a == "--ours" && b == "--reference" => (ours, reference),
        [b, reference, a, ours] if a == "--ours" && b == "--reference" => (ours, reference),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let scored = metrics::Rendering::reference(std::path::Path::new(reference)).and_then(|word| {
        let ours = metrics::Rendering::pdf(std::path::Path::new(ours))?;
        serde_json::to_string_pretty(&metrics::score(&ours, &word))
            .map_err(|error| error.to_string())
    });
    match scored {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("corpus-runner: {message}");
            ExitCode::from(2)
        }
    }
}

/// Prints the release criteria over a run's report as a markdown table, next to main's
/// when given; fails when a criterion that could be measured is missed.
fn gate(args: &[String]) -> ExitCode {
    let mut main_report = None;
    let mut same_fonts = false;
    let mut branch_report = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--main" => match args.next() {
                Some(path) => main_report = Some(path),
                None => {
                    eprintln!("corpus-runner: --main needs a report\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--same-fonts" => same_fonts = true,
            path if !path.starts_with("--") && branch_report.is_none() => branch_report = Some(arg),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(branch_report) = branch_report else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let read = |path: &String| -> Result<report::Report, String> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
        serde_json::from_str(&text).map_err(|error| format!("{path}: {error}"))
    };
    let reports =
        read(branch_report).and_then(|branch| Ok((branch, main_report.map(read).transpose()?)));
    let (branch, main) = match reports {
        Ok(reports) => reports,
        Err(message) => {
            eprintln!("corpus-runner: {message}");
            return ExitCode::from(2);
        }
    };
    let measures = gate::measure_against(&branch, main.as_ref(), same_fonts);
    let before = main.as_ref().map(|main| gate::measure(main, same_fonts));
    match &main {
        Some(main) => {
            println!("main: {}\n", gate::census(main, same_fonts));
            println!("branch: {}\n", gate::census(&branch, same_fonts));
            let lacking = gate::missing(&branch, main);
            if !lacking.files.is_empty() {
                println!(
                    "Files of main the branch report lacks: {}\n",
                    lacking.files.join(", ")
                );
            }
            if !lacking.references.is_empty() {
                println!(
                    "Reference documents of main the branch report did not score: {}\n",
                    lacking.references.join(", ")
                );
            }
        }
        None => println!("{}\n", gate::census(&branch, same_fonts)),
    }
    print!("{}", gate::table(&measures, before.as_deref()));
    if gate::passes(&measures) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// What `run` was asked to do.
#[derive(Debug)]
pub struct Options {
    /// Files, and directories searched for Word documents.
    pub paths: Vec<PathBuf>,
    /// What the paths in the report, baseline and expected errors are relative to.
    pub root: PathBuf,
    /// Where to write the JSON report.
    pub report: Option<PathBuf>,
    /// The baseline to compare with, or to write with `update_baseline`.
    pub baseline: Option<PathBuf>,
    /// Write the baseline instead of comparing with it.
    pub update_baseline: bool,
    /// Files better than the baseline do not fail the run.
    pub allow_better: bool,
    /// The expected-errors file.
    pub expected: Option<PathBuf>,
    /// How long a child may take for one file.
    pub timeout: Duration,
    /// The most memory a child may use for one file, in MiB (measured on Linux).
    pub memory_limit_mb: u64,
    /// Also start each child under that address-space limit (Linux, with prlimit).
    pub enforce_memory_limit: bool,
    /// How many children run at once.
    pub jobs: usize,
    /// The reference folders (`tests/corpus/reference`).
    pub references: Option<PathBuf>,
    /// The folder of our PDFs to score against them, named like the documents.
    pub ours: Option<PathBuf>,
    /// The folder each child renders the document it opened into, as a PDF.
    pub render: Option<PathBuf>,
    /// The fonts to render with.
    pub font_dirs: Vec<PathBuf>,
    /// Compare only the statuses with the baseline, and keep its scores when updating it.
    pub statuses_only: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        paths: Vec::new(),
        root: PathBuf::from("."),
        report: None,
        baseline: None,
        update_baseline: false,
        allow_better: false,
        expected: None,
        timeout: Duration::from_secs(30),
        memory_limit_mb: 1024,
        enforce_memory_limit: false,
        jobs: std::thread::available_parallelism().map_or(1, |n| n.get()),
        references: None,
        ours: None,
        render: None,
        font_dirs: Vec::new(),
        statuses_only: false,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--root" => options.root = PathBuf::from(value("--root")?),
            "--report" => options.report = Some(PathBuf::from(value("--report")?)),
            "--baseline" => options.baseline = Some(PathBuf::from(value("--baseline")?)),
            "--expected" => options.expected = Some(PathBuf::from(value("--expected")?)),
            "--references" => options.references = Some(PathBuf::from(value("--references")?)),
            "--ours" => options.ours = Some(PathBuf::from(value("--ours")?)),
            "--render" => options.render = Some(PathBuf::from(value("--render")?)),
            "--font-dir" => options.font_dirs.push(PathBuf::from(value("--font-dir")?)),
            "--update-baseline" => options.update_baseline = true,
            "--statuses-only" => options.statuses_only = true,
            "--allow-better" => options.allow_better = true,
            "--enforce-memory-limit" => options.enforce_memory_limit = true,
            "--timeout" => {
                let seconds = value("--timeout")?;
                let seconds: u64 = seconds
                    .parse()
                    .map_err(|_| format!("--timeout {seconds} is not a number of seconds"))?;
                options.timeout = Duration::from_secs(seconds.max(1));
            }
            "--memory-limit" => {
                let limit = value("--memory-limit")?;
                options.memory_limit_mb = limit
                    .parse()
                    .map_err(|_| format!("--memory-limit {limit} is not a number of MiB"))?;
            }
            "--jobs" => {
                let jobs = value("--jobs")?;
                options.jobs = jobs
                    .parse::<usize>()
                    .map_err(|_| format!("--jobs {jobs} is not a number"))?
                    .max(1);
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path => options.paths.push(PathBuf::from(path)),
        }
    }
    if options.paths.is_empty() {
        return Err("no paths to run over".to_owned());
    }
    if (options.update_baseline || options.allow_better || options.statuses_only)
        && options.baseline.is_none()
    {
        return Err(
            "--update-baseline, --allow-better and --statuses-only need --baseline".to_owned(),
        );
    }
    if options.statuses_only && options.references.is_some() {
        return Err("--statuses-only compares no scores, so it takes no --references".to_owned());
    }
    // The PDFs rendered are the ones scored, unless --ours names others.
    if options.references.is_some() && options.ours.is_none() {
        options.ours = options.render.clone();
    }
    if options.references.is_some() != options.ours.is_some() {
        return Err("--references and --ours go together; --render stands for --ours".to_owned());
    }
    if !options.font_dirs.is_empty() && options.render.is_none() {
        return Err("--font-dir needs --render".to_owned());
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn options_are_parsed() {
        let options = parse(&strings(&[
            "--timeout",
            "5",
            "--jobs",
            "2",
            "--baseline",
            "b.json",
            "--update-baseline",
            "a",
            "b",
        ]))
        .unwrap();
        assert_eq!(options.timeout, Duration::from_secs(5));
        assert_eq!(options.jobs, 2);
        assert!(options.update_baseline);
        assert_eq!(options.paths, [PathBuf::from("a"), PathBuf::from("b")]);
    }

    #[test]
    fn bad_options_are_refused() {
        for args in [
            &["--timeout", "soon", "a"][..],
            &["--jobs"][..],
            &["--update-baseline", "a"][..],
            &["--nonsense", "a"][..],
            &["--ours", "pdfs", "a"][..],
            &[][..],
        ] {
            assert!(parse(&strings(args)).is_err(), "{args:?}");
        }
    }
}
