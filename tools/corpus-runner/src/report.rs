//! Results, the report, expected errors and the baseline.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::child::Opened;

/// How one file ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Opened,
    ExpectedError,
    UnexpectedError,
    Panic,
    Timeout,
    Memory,
    Crash,
}

impl Status {
    /// Whether this is a robustness failure: the library must never do this on any input.
    pub fn is_robustness_failure(self) -> bool {
        matches!(
            self,
            Status::Panic | Status::Timeout | Status::Memory | Status::Crash
        )
    }

    /// Opened is best, then an expected error, then an unexpected one, then the rest.
    fn rank(self) -> u8 {
        match self {
            Status::Opened => 3,
            Status::ExpectedError => 2,
            Status::UnexpectedError => 1,
            Status::Panic | Status::Timeout | Status::Memory | Status::Crash => 0,
        }
    }
}

/// One file in the report.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FileResult {
    /// Relative to the run's root, with forward slashes.
    pub path: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Why the error is expected, from the expected-errors file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_reason: Option<String>,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rss_kb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chars: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<u64>,
}

/// What a child process did: its exit, what it printed, and whether it was stopped.
#[derive(Debug, Default)]
pub struct ChildExit {
    pub success: bool,
    /// The exit code, `None` when a signal ended the child.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// The status of a file from its child's exit; `expected` is the reason the expected-errors
/// file gives for it, if it lists it.
pub fn classify(
    path: &str,
    exit: &ChildExit,
    expected: Option<&str>,
    duration_ms: u64,
) -> FileResult {
    let mut result = FileResult {
        path: path.to_owned(),
        status: Status::Crash,
        error: None,
        expected_reason: None,
        duration_ms,
        peak_rss_kb: None,
        chars: None,
        diagnostics: None,
    };
    let first_line = |text: &str| {
        text.lines()
            .find(|l| !l.trim().is_empty())
            .map(str::to_owned)
    };
    if exit.timed_out {
        result.status = Status::Timeout;
        result.error = Some("no result within the time-out".to_owned());
    } else if exit.stderr.contains("memory allocation of") {
        result.status = Status::Memory;
        result.error = first_line(&exit.stderr);
    } else if exit.stderr.contains("panicked at") {
        result.status = Status::Panic;
        let message = exit
            .stderr
            .lines()
            .skip_while(|l| !l.contains("panicked at"))
            .nth(1);
        result.error = message
            .map(str::to_owned)
            .or_else(|| first_line(&exit.stderr));
    } else if exit.success {
        match serde_json::from_str::<Opened>(exit.stdout.trim()) {
            Ok(opened) => {
                result.peak_rss_kb = opened.peak_rss_kb;
                if opened.status == "opened" {
                    result.status = Status::Opened;
                    result.chars = opened.chars;
                    result.diagnostics = opened.diagnostics;
                } else {
                    let kind = opened.error_kind.unwrap_or_default();
                    let message = opened.error.unwrap_or_default();
                    result.error = Some(format!("{kind}: {message}"));
                    result.status = Status::UnexpectedError;
                    if let Some(reason) = expected {
                        result.status = Status::ExpectedError;
                        result.expected_reason = Some(reason.to_owned());
                    }
                }
            }
            Err(error) => result.error = Some(format!("unreadable result: {error}")),
        }
    } else {
        result.error = Some(match exit.code {
            Some(code) => format!("exited with code {code}"),
            None => "ended by a signal".to_owned(),
        });
        if let Some(line) = first_line(&exit.stderr) {
            result.error = Some(line);
        }
    }
    result
}

/// Turns a result into `memory` when the child's peak memory exceeded the limit.
pub fn check_memory(result: &mut FileResult, limit_mb: u64) {
    if let Some(kb) = result.peak_rss_kb {
        if kb > limit_mb.saturating_mul(1024) && !result.status.is_robustness_failure() {
            result.status = Status::Memory;
            result.error = Some(format!(
                "peak memory {} MiB, over the limit of {limit_mb} MiB",
                kb / 1024
            ));
            result.expected_reason = None;
        }
    }
}

/// The expected-errors file: one `path<TAB>reason` per line, `#` comments, paths relative
/// to the run's root.
pub fn parse_expected(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut expected = BTreeMap::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((path, reason)) = line.split_once('\t') else {
            return Err(format!("line {}: not `path<TAB>reason`", number + 1));
        };
        if path.is_empty() || reason.trim().is_empty() {
            return Err(format!(
                "line {}: a path and a reason are both needed",
                number + 1
            ));
        }
        if expected
            .insert(path.to_owned(), reason.trim().to_owned())
            .is_some()
        {
            return Err(format!("line {}: {path} is listed twice", number + 1));
        }
    }
    Ok(expected)
}

/// The run's report.
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub format: u32,
    pub tool: String,
    pub summary: BTreeMap<Status, usize>,
    pub files: Vec<FileResult>,
}

impl Report {
    pub fn new(mut files: Vec<FileResult>) -> Self {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut summary = BTreeMap::new();
        for file in &files {
            *summary.entry(file.status).or_insert(0) += 1;
        }
        Report {
            format: 1,
            tool: format!("corpus-runner {}", env!("CARGO_PKG_VERSION")),
            summary,
            files,
        }
    }
}

/// The baseline: the status of every file, which is what a later run must not get worse
/// at. Durations and memory vary between machines and are left out.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Baseline {
    pub format: u32,
    pub files: BTreeMap<String, Status>,
}

impl Baseline {
    pub fn of(report: &Report) -> Self {
        Baseline {
            format: 1,
            files: report
                .files
                .iter()
                .map(|f| (f.path.clone(), f.status))
                .collect(),
        }
    }
}

/// How a run differs from its baseline, one line per file.
#[derive(Debug, Default)]
pub struct Changes {
    /// Files whose status got worse, or changed between two failures, and files missing
    /// from the run or from the baseline.
    pub worse: Vec<String>,
    /// Files whose status got better.
    pub better: Vec<String>,
}

pub fn compare(baseline: &Baseline, report: &Report) -> Changes {
    let mut changes = Changes::default();
    let now: BTreeMap<&str, &FileResult> = report
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    for (path, &before) in &baseline.files {
        let Some(file) = now.get(path.as_str()) else {
            changes
                .worse
                .push(format!("{path}: in the baseline but not in this run"));
            continue;
        };
        if file.status == before {
            continue;
        }
        let line = format!(
            "{path}: {} in the baseline, now {}{}",
            name(before),
            name(file.status),
            file.error
                .as_deref()
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        );
        if file.status.rank() > before.rank() {
            changes.better.push(line);
        } else {
            changes.worse.push(line);
        }
    }
    let known: BTreeSet<&str> = baseline.files.keys().map(String::as_str).collect();
    for file in &report.files {
        if !known.contains(file.path.as_str()) {
            changes.worse.push(format!(
                "{}: not in the baseline ({}); run with --update-baseline and commit it",
                file.path,
                name(file.status)
            ));
        }
    }
    changes
}

pub fn name(status: Status) -> &'static str {
    match status {
        Status::Opened => "opened",
        Status::ExpectedError => "expected-error",
        Status::UnexpectedError => "unexpected-error",
        Status::Panic => "panic",
        Status::Timeout => "timeout",
        Status::Memory => "memory",
        Status::Crash => "crash",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(stdout: &str, stderr: &str, success: bool) -> ChildExit {
        ChildExit {
            success,
            code: Some(if success { 0 } else { 101 }),
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            timed_out: false,
        }
    }

    #[test]
    fn an_opened_document_carries_its_counts() {
        let line = r#"{"status":"opened","error_kind":null,"error":null,"chars":42,"diagnostics":1,"peak_rss_kb":2048}"#;
        let result = classify("a.docx", &exit(line, "", true), None, 7);
        assert_eq!(result.status, Status::Opened);
        assert_eq!(
            (result.chars, result.diagnostics, result.peak_rss_kb),
            (Some(42), Some(1), Some(2048))
        );
        assert_eq!(result.duration_ms, 7);
    }

    #[test]
    fn errors_are_expected_only_when_listed() {
        let line = r#"{"status":"error","error_kind":"unsupported","error":"encrypted","chars":null,"diagnostics":null,"peak_rss_kb":null}"#;
        let unlisted = classify("a.docx", &exit(line, "", true), None, 0);
        assert_eq!(unlisted.status, Status::UnexpectedError);
        assert_eq!(unlisted.error.as_deref(), Some("unsupported: encrypted"));
        let listed = classify(
            "a.docx",
            &exit(line, "", true),
            Some("encrypted test file"),
            0,
        );
        assert_eq!(listed.status, Status::ExpectedError);
        assert_eq!(
            listed.expected_reason.as_deref(),
            Some("encrypted test file")
        );
    }

    #[test]
    fn failures_of_the_child_are_told_apart() {
        let panic = "\nthread 'main' panicked at src/x.rs:1:1:\nindex out of bounds\nnote: ...";
        let result = classify("a", &exit("", panic, false), None, 0);
        assert_eq!(
            (result.status, result.error.as_deref()),
            (Status::Panic, Some("index out of bounds"))
        );
        let memory = classify(
            "a",
            &exit("", "memory allocation of 8 bytes failed\n", false),
            None,
            0,
        );
        assert_eq!(memory.status, Status::Memory);
        let timeout = ChildExit {
            timed_out: true,
            ..ChildExit::default()
        };
        assert_eq!(classify("a", &timeout, None, 0).status, Status::Timeout);
        let signal = ChildExit {
            code: None,
            ..ChildExit::default()
        };
        let crash = classify("a", &signal, None, 0);
        assert_eq!(
            (crash.status, crash.error.as_deref()),
            (Status::Crash, Some("ended by a signal"))
        );
        let garbage = classify("a", &exit("not json", "", true), None, 0);
        assert_eq!(garbage.status, Status::Crash);
    }

    #[test]
    fn the_expected_errors_file_is_strict() {
        let text = "# comment\n\ncorpus/a.docx\tencrypted\ncorpus/b.docx\t binary Word file \n";
        let expected = parse_expected(text).unwrap();
        assert_eq!(
            expected.get("corpus/b.docx").map(String::as_str),
            Some("binary Word file")
        );
        assert!(parse_expected("a.docx encrypted").is_err());
        assert!(parse_expected("a.docx\t").is_err());
        assert!(parse_expected("a.docx\tx\na.docx\ty").is_err());
    }

    fn file(path: &str, status: Status) -> FileResult {
        FileResult {
            path: path.to_owned(),
            status,
            error: None,
            expected_reason: None,
            duration_ms: 0,
            peak_rss_kb: None,
            chars: None,
            diagnostics: None,
        }
    }

    #[test]
    fn changes_against_the_baseline() {
        let before = Report::new(vec![
            file("a", Status::Opened),
            file("b", Status::ExpectedError),
            file("c", Status::UnexpectedError),
            file("d", Status::Opened),
            file("f", Status::Opened),
            file("g", Status::Panic),
        ]);
        let baseline = Baseline::of(&before);
        let after = Report::new(vec![
            file("a", Status::Panic),
            file("b", Status::ExpectedError),
            file("c", Status::Opened),
            file("e", Status::Opened),
            file("f", Status::ExpectedError),
            file("g", Status::Timeout),
        ]);
        let changes = compare(&baseline, &after);
        assert_eq!(changes.worse.len(), 5, "{:?}", changes.worse);
        assert!(changes.worse[0].starts_with("a: opened in the baseline, now panic"));
        assert!(changes.worse[1].starts_with("d: in the baseline but not in this run"));
        assert!(changes.worse[2].starts_with("f: opened in the baseline, now expected-error"));
        assert!(changes.worse[3].starts_with("g: panic in the baseline, now timeout"));
        assert!(changes.worse[4].starts_with("e: not in the baseline"));
        assert_eq!(
            changes.better,
            ["c: unexpected-error in the baseline, now opened"]
        );
        let same = compare(&baseline, &before);
        assert!(same.worse.is_empty() && same.better.is_empty());
    }

    #[test]
    fn memory_over_the_limit_fails_the_file() {
        let mut result = file("a", Status::Opened);
        result.peak_rss_kb = Some(1024 * 1024);
        check_memory(&mut result, 1024);
        assert_eq!(result.status, Status::Opened);
        result.peak_rss_kb = Some(1024 * 1024 + 1);
        result.expected_reason = Some("listed".to_owned());
        check_memory(&mut result, 1024);
        assert_eq!(result.status, Status::Memory);
        assert_eq!(result.expected_reason, None);
        assert_eq!(
            result.error.as_deref(),
            Some("peak memory 1024 MiB, over the limit of 1024 MiB")
        );
    }

    #[test]
    fn the_report_is_sorted_and_counted() {
        let report = Report::new(vec![
            file("b", Status::Opened),
            file("a", Status::Opened),
            file("c", Status::Panic),
        ]);
        assert_eq!(report.files[0].path, "a");
        assert_eq!(report.summary.get(&Status::Opened), Some(&2));
        let json = serde_json::to_string(&Baseline::of(&report)).unwrap();
        assert_eq!(
            json,
            r#"{"format":1,"files":{"a":"opened","b":"opened","c":"panic"}}"#
        );
    }
}
