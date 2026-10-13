//! The parent: finds the files, opens each in a child process, writes the report and
//! compares it with the baseline.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::Options;
use crate::report::{self, Baseline, ChildExit, FileResult, Report, Status};

/// The extensions of the Word documents a directory is searched for.
const WORD_EXTENSIONS: [&str; 4] = ["docx", "docm", "dotx", "dotm"];

pub fn main(options: &Options) -> ExitCode {
    match run(options) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("corpus-runner: {message}");
            ExitCode::from(2)
        }
    }
}

/// Runs over the files; `Ok(false)` when the run fails.
fn run(options: &Options) -> Result<bool, String> {
    // Absolute but not canonical: a corpus directory may be a symbolic link to elsewhere.
    let root = std::path::absolute(&options.root)
        .map_err(|error| format!("root {}: {error}", options.root.display()))?;
    let expected = match &options.expected {
        Some(path) => report::parse_expected(&read(path)?)
            .map_err(|error| format!("{}: {error}", path.display()))?,
        None => BTreeMap::new(),
    };
    let mut found = Vec::new();
    for path in &options.paths {
        collect(path, &mut found)?;
    }
    let mut files = found
        .into_iter()
        .map(|file| relative(&root, &file).map(|name| (name, file)))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    files.dedup_by(|a, b| a.0 == b.0);
    if files.is_empty() {
        return Err("no Word documents under the paths".to_owned());
    }
    let launcher = Launcher::new(options)?;

    let started = Instant::now();
    let report = Report::new(run_all(&launcher, &files, &expected, options)?);
    print_summary(&report, started.elapsed());
    for file in &report.files {
        if file.status == Status::Opened && expected.contains_key(&file.path) {
            println!(
                "note: {} opened; remove it from the expected errors",
                file.path
            );
        }
    }
    if let Some(path) = &options.report {
        write_json(path, &report)?;
    }

    let mut passed = true;
    let failures = report
        .files
        .iter()
        .filter(|file| file.status.is_robustness_failure())
        .count();
    if failures > 0 {
        println!(
            "FAILED: {failures} files made the library panic, hang, run out of memory or crash"
        );
        passed = false;
    }
    if let Some(path) = &options.baseline {
        if options.update_baseline {
            write_json(path, &Baseline::of(&report))?;
            println!("wrote the baseline {}", path.display());
        } else {
            let baseline: Baseline = serde_json::from_str(&read(path)?)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if baseline.format != 1 {
                return Err(format!(
                    "{}: baseline format {}, this runner reads format 1",
                    path.display(),
                    baseline.format
                ));
            }
            let changes = report::compare(&baseline, &report);
            for line in &changes.worse {
                println!("worse than the baseline: {line}");
            }
            for line in &changes.better {
                println!("better than the baseline: {line}");
            }
            if !changes.worse.is_empty() {
                println!(
                    "FAILED: {} files are worse than the baseline",
                    changes.worse.len()
                );
                passed = false;
            } else if !changes.better.is_empty() && !options.allow_better {
                println!(
                    "FAILED: {} files are better than the baseline; accept them by running with \
                     --update-baseline and committing {}",
                    changes.better.len(),
                    path.display()
                );
                passed = false;
            }
        }
    }
    Ok(passed)
}

/// Adds the file, or the Word documents under the directory, sorted. A path named on the
/// command line is followed if it is a symbolic link.
fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.is_dir() {
        walk(path, files)
    } else {
        // A file named on the command line is run whatever its extension.
        files.push(path.to_path_buf());
        Ok(())
    }
}

/// The Word documents under the directory. A symbolic link to a document is followed (a
/// broken one is run, and reported as an error); a link to a directory is not, so a link
/// cannot make the walk loop.
fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect()
        })
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    entries.sort();
    for entry in entries {
        let link = std::fs::symlink_metadata(&entry)
            .map_err(|error| format!("{}: {error}", entry.display()))?;
        if link.is_dir() {
            walk(&entry, files)?;
        } else if is_word_document(&entry)
            && !std::fs::metadata(&entry).is_ok_and(|target| target.is_dir())
        {
            files.push(entry);
        }
    }
    Ok(())
}

fn is_word_document(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            WORD_EXTENSIONS
                .iter()
                .any(|word| extension.eq_ignore_ascii_case(word))
        })
}

/// The file's path under the root, with forward slashes, the same on every system.
fn relative(root: &Path, file: &Path) -> Result<String, String> {
    let full = std::path::absolute(file).map_err(|error| format!("{}: {error}", file.display()))?;
    let outside = || {
        format!(
            "{} is not under the root {}; pass --root",
            file.display(),
            root.display()
        )
    };
    let rest = full.strip_prefix(root).map_err(|_| outside())?;
    // std::path::absolute keeps "..": a path through it may lead out of the root.
    if rest
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(outside());
    }
    let parts: Vec<String> = rest
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    Ok(parts.join("/"))
}

/// How a child is started: this program, or `prlimit` running it under an address-space
/// limit.
pub struct Launcher {
    program: PathBuf,
    leading: Vec<std::ffi::OsString>,
}

impl Launcher {
    fn new(options: &Options) -> Result<Self, String> {
        let exe = std::env::current_exe()
            .map_err(|error| format!("cannot find this program to start children: {error}"))?;
        if !options.enforce_memory_limit {
            return Ok(Launcher {
                program: exe,
                leading: Vec::new(),
            });
        }
        // The limit is set on each child, not on the runner, whose threads reserve address
        // space of their own.
        if !cfg!(target_os = "linux") {
            return Err("--enforce-memory-limit needs Linux".to_owned());
        }
        let found = Command::new("prlimit")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !found {
            return Err("--enforce-memory-limit needs prlimit (util-linux)".to_owned());
        }
        let bytes = options.memory_limit_mb.saturating_mul(1024 * 1024);
        Ok(Launcher {
            program: PathBuf::from("prlimit"),
            leading: vec![
                format!("--as={bytes}").into(),
                "--".into(),
                exe.into_os_string(),
            ],
        })
    }
}

/// Opens every file in a child process, `options.jobs` at a time. A failure of the runner
/// itself, not of a file, stops the run.
fn run_all(
    launcher: &Launcher,
    files: &[(String, PathBuf)],
    expected: &BTreeMap<String, String>,
    options: &Options,
) -> Result<Vec<FileResult>, String> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(files.len()));
    let fatal = Mutex::new(None);
    let work = || {
        while let Some((name, file)) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
            if fatal
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some()
            {
                break;
            }
            let (exit, elapsed) = match run_child(launcher, file, options.timeout) {
                Ok(done) => done,
                Err(error) => {
                    *fatal.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
                    break;
                }
            };
            let milliseconds = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
            let mut result = report::classify(
                name,
                &exit,
                expected.get(name).map(String::as_str),
                milliseconds,
            );
            report::check_memory(&mut result, options.memory_limit_mb);
            results
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(result);
        }
    };
    std::thread::scope(|scope| {
        let started = (0..options.jobs.min(files.len()))
            .filter(|_| {
                std::thread::Builder::new()
                    .spawn_scoped(scope, work)
                    .is_ok()
            })
            .count();
        if started == 0 {
            work();
        }
    });
    if let Some(error) = fatal.into_inner().unwrap_or_else(PoisonError::into_inner) {
        return Err(error);
    }
    Ok(results.into_inner().unwrap_or_else(PoisonError::into_inner))
}

/// Runs `corpus-runner open FILE`, stopping it after the time-out. `Err` is a failure of
/// the runner: the child could not be watched.
fn run_child(
    launcher: &Launcher,
    file: &Path,
    timeout: Duration,
) -> Result<(ChildExit, Duration), String> {
    let started = Instant::now();
    let child = Command::new(&launcher.program)
        .args(&launcher.leading)
        .arg("open")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            let exit = ChildExit {
                stderr: format!("cannot start the child: {error}"),
                ..ChildExit::default()
            };
            return Ok((exit, started.elapsed()));
        }
    };
    // The pipes are drained while the child runs, so a full pipe cannot block it.
    let (stdout, stderr) = match (
        child.stdout.take().map(drain),
        child.stderr.take().map(drain),
    ) {
        (Some(Ok(stdout)), Some(Ok(stderr))) => (stdout, stderr),
        _ => {
            // Killing fails only when the child has already exited.
            let _ = child.kill();
            let _ = child.wait();
            return Err("cannot start a thread to read a child's output".to_owned());
        }
    };
    let mut timed_out = false;
    let mut pause = Duration::from_millis(1);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= timeout => {
                timed_out = true;
                // Killing fails only when the child has already exited.
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(20));
            }
            Err(_) => {
                // The child's state is unknown: stop it, so the pipes close.
                let _ = child.kill();
                break child.wait().ok();
            }
        }
    };
    let elapsed = started.elapsed();
    let joined = |thread: JoinHandle<String>| thread.join().unwrap_or_default();
    let exit = ChildExit {
        success: status.is_some_and(|status| status.success()),
        code: status.and_then(|status| status.code()),
        stdout: joined(stdout),
        stderr: joined(stderr),
        timed_out,
    };
    Ok((exit, elapsed))
}

fn drain(mut pipe: impl Read + Send + 'static) -> std::io::Result<JoinHandle<String>> {
    std::thread::Builder::new().spawn(move || {
        let mut bytes = Vec::new();
        // A read error ends the output early; what was read is still classified.
        let _ = pipe.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn print_summary(report: &Report, elapsed: Duration) {
    let counts: Vec<String> = report
        .summary
        .iter()
        .map(|(status, count)| format!("{count} {}", report::name(*status)))
        .collect();
    println!(
        "corpus-runner: {} files in {:.2} s: {}",
        report.files.len(),
        elapsed.as_secs_f64(),
        counts.join(", ")
    );
    let chars: u64 = report.files.iter().filter_map(|file| file.chars).sum();
    let diagnostics: u64 = report
        .files
        .iter()
        .filter_map(|file| file.diagnostics)
        .sum();
    println!("{chars} characters of text, {diagnostics} diagnostics");
    if let Some(slowest) = report.files.iter().max_by_key(|file| file.duration_ms) {
        println!("slowest: {} ms, {}", slowest.duration_ms, slowest.path);
    }
    if let Some(largest) = report
        .files
        .iter()
        .filter(|file| file.peak_rss_kb.is_some())
        .max_by_key(|file| file.peak_rss_kb)
    {
        let kb = largest.peak_rss_kb.unwrap_or_default();
        println!("peak memory: {} MiB, {}", kb / 1024, largest.path);
    }
    for file in &report.files {
        if file.status != Status::Opened {
            println!(
                "  {:<16} {}: {}",
                report::name(file.status),
                file.path,
                file.expected_reason
                    .as_deref()
                    .or(file.error.as_deref())
                    .unwrap_or("")
            );
        }
    }
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut json = serde_json::to_string_pretty(value)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    json.push('\n');
    std::fs::write(path, json).map_err(|error| format!("{}: {error}", path.display()))
}
