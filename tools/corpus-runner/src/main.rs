//! Runs officewright over corpora of documents and compares the results with a baseline.
//!
//! ```text
//! corpus-runner run [--root DIR] [--report FILE] [--baseline FILE [--update-baseline]]
//!                   [--expected FILE] [--timeout SECONDS] [--memory-limit MIB]
//!                   [--enforce-memory-limit] [--jobs N] PATH...
//! ```
//!
//! Every Word document under the paths (`.docx`, `.docm`, `.dotx`, `.dotm`) is opened in a
//! child process of its own (this program with `open FILE`), so a panic, a hang or a memory
//! blow-up in one file is a result for that file, not the end of the run. A child that
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
//! it, so that progress shows in the diff.
#![forbid(unsafe_code)]

mod child;
mod report;
mod run;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "usage: corpus-runner run [--root DIR] [--report FILE] \
[--baseline FILE [--update-baseline]] [--expected FILE] [--timeout SECONDS] \
[--memory-limit MIB] [--enforce-memory-limit] [--jobs N] PATH...
       corpus-runner open FILE";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((command, [file])) if command == "open" => child::main(file),
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
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        paths: Vec::new(),
        root: PathBuf::from("."),
        report: None,
        baseline: None,
        update_baseline: false,
        expected: None,
        timeout: Duration::from_secs(30),
        memory_limit_mb: 1024,
        enforce_memory_limit: false,
        jobs: std::thread::available_parallelism().map_or(1, |n| n.get()),
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
            "--update-baseline" => options.update_baseline = true,
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
    if options.update_baseline && options.baseline.is_none() {
        return Err("--update-baseline needs --baseline".to_owned());
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
            &[][..],
        ] {
            assert!(parse(&strings(args)).is_err(), "{args:?}");
        }
    }
}
