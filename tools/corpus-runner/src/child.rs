//! The child: opens one file and prints one line of JSON about it.

use std::process::ExitCode;

use officewright_doc::{Document, Error};
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
}

/// Makes the child misbehave, for the runner's own tests: `panic`, `hang`, `abort` or
/// `allocation` (prints what Rust prints when an allocation fails, then aborts).
pub const MISBEHAVE: &str = "CORPUS_RUNNER_MISBEHAVE";

pub fn main(file: &str) -> ExitCode {
    misbehave();
    let opened = match Document::open(file) {
        Ok(document) => Opened {
            status: "opened".to_owned(),
            error_kind: None,
            error: None,
            chars: Some(document.text().chars().count() as u64),
            diagnostics: Some(document.diagnostics().len() as u64),
            peak_rss_kb: peak_rss_kb(),
        },
        Err(error) => Opened {
            status: "error".to_owned(),
            error_kind: Some(kind(&error).to_owned()),
            error: Some(error.to_string()),
            chars: None,
            diagnostics: None,
            peak_rss_kb: peak_rss_kb(),
        },
    };
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
