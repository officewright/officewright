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
use crate::metrics;
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
    let root = lexical_absolute(&options.root).map_err(|error| format!("root {error}"))?;
    let expected = match &options.expected {
        Some(path) => report::parse_expected(&read(path)?)
            .map_err(|error| format!("{}: {error}", path.display()))?,
        None => BTreeMap::new(),
    };
    let mut found = Vec::new();
    for path in &options.paths {
        // Resolved before the child opens it, so that it opens the file the report names:
        // the system would follow a symbolic link before "..", the name would not.
        collect(&lexical_absolute(path)?, &mut found)?;
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
    let index = reference_index(options)?;
    let mut scoring = BTreeMap::new();
    // The files are absolute already, their "." and ".." resolved by collect's caller.
    for (name, file) in &files {
        if let Some(pair) = index.get(file) {
            scoring.insert(name.clone(), pair.clone());
        }
    }
    let renders = render_targets(options, &files)?;

    let started = Instant::now();
    let report = Report::new(run_all(
        &launcher, &files, &expected, &scoring, &renders, options,
    )?);
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
    let unrendered: Vec<&FileResult> = report
        .files
        .iter()
        .filter(|file| file.render_error.is_some())
        .collect();
    for file in &unrendered {
        println!(
            "not rendered: {}: {}",
            file.path,
            file.render_error.as_deref().unwrap_or_default()
        );
    }
    if !unrendered.is_empty() {
        println!(
            "FAILED: {} files opened but could not be rendered",
            unrendered.len()
        );
        passed = false;
    }
    if let Some(path) = &options.baseline {
        if options.update_baseline {
            let mut baseline = Baseline::of(&report);
            // A run that cannot score keeps the scores the baseline has.
            if options.statuses_only && path.is_file() {
                let old: Baseline = serde_json::from_str(&read(path)?)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                baseline.scores = old.scores;
            }
            write_json(path, &baseline)?;
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
            let changes = if options.statuses_only {
                report::compare_statuses(&baseline, &report)
            } else {
                report::compare(&baseline, &report)
            };
            for line in &changes.worse {
                println!("worse than the baseline: {line}");
            }
            for line in &changes.better {
                println!("better than the baseline: {line}");
            }
            if !changes.worse.is_empty() {
                println!("FAILED: {} worse than the baseline", changes.worse_count());
                passed = false;
            } else if !changes.better.is_empty() && !options.allow_better {
                println!(
                    "FAILED: {} better than the baseline; accept them by running with \
                     --update-baseline and committing {}",
                    changes.better_count(),
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

/// The path made absolute, with "." and ".." resolved by name: not canonical, since a corpus
/// folder may be a symbolic link to elsewhere. std::path::absolute keeps ".." on Unix and
/// resolves it on Windows; this gives the same path on every system, for the root, the
/// files and the reference folders alike.
fn lexical_absolute(path: &Path) -> Result<PathBuf, String> {
    let full = std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut out = PathBuf::new();
    for part in full.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

/// Where `path` is on disk, links followed: the canonical path of the longest part of it
/// that exists, then the rest of it part by part, its `.` and `..` resolved by their
/// names, and canonical again wherever a `..` has climbed back to what exists, since a
/// link can follow there (`missing/../link`).
fn on_disk(path: &Path) -> Result<PathBuf, String> {
    let full = std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let parts: Vec<std::path::Component<'_>> = full.components().collect();
    for existing in (1..=parts.len()).rev() {
        let (head, rest) = parts.split_at(existing);
        let Ok(mut found) = std::fs::canonicalize(head.iter().collect::<PathBuf>()) else {
            continue;
        };
        for part in rest {
            match part {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    found.pop();
                }
                other => found.push(other.as_os_str()),
            }
            if let Ok(real) = std::fs::canonicalize(&found) {
                found = real;
            }
        }
        return Ok(found);
    }
    lexical_absolute(path)
}

/// The file's path under the root, with forward slashes, the same on every system; both
/// are absolute, their "." and ".." already resolved by lexical_absolute.
fn relative(root: &Path, file: &Path) -> Result<String, String> {
    let rest = file.strip_prefix(root).map_err(|_| {
        format!(
            "{} is not under the root {}; pass --root",
            file.display(),
            root.display()
        )
    })?;
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

/// Where each file is rendered with `--render DIR`, by its name: `DIR/<path>.pdf`, the path
/// relative to the folder above `--references` when the file lies under it (where the
/// scoring looks for it), else its name, relative to the root. Empty without `--render`.
/// Refuses two files that would be rendered to one PDF: names that differ only in their
/// extension, or in case, which the file systems of macOS and Windows ignore. Refuses as
/// well a target that is one of the files to run, compared where they are on disk (links
/// followed) and without case: a target is removed before its document runs. A target
/// that is a link to a file to run is refused too, though removing it would remove only
/// the link.
fn render_targets(
    options: &Options,
    files: &[(String, PathBuf)],
) -> Result<BTreeMap<String, PathBuf>, String> {
    let Some(dir) = &options.render else {
        return Ok(BTreeMap::new());
    };
    let corpus = match &options.references {
        Some(references) => lexical_absolute(references)?
            .parent()
            .map(Path::to_path_buf),
        None => None,
    };
    let mut targets = BTreeMap::new();
    for (name, file) in files {
        let full = lexical_absolute(file)?;
        let relative = corpus
            .as_ref()
            .and_then(|corpus| full.strip_prefix(corpus).ok())
            .map_or_else(|| PathBuf::from(name), Path::to_path_buf);
        targets.insert(name.clone(), dir.join(relative).with_extension("pdf"));
    }
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for (name, pdf) in &targets {
        if let Some(other) = seen.insert(pdf.to_string_lossy().to_lowercase(), name) {
            return Err(format!(
                "{other} and {name} would both be rendered to {}",
                pdf.display()
            ));
        }
    }
    // A target is removed before its document runs, so none may be a file to run, in any
    // case of its name either, nor through a link.
    let mut inputs: BTreeMap<String, &str> = BTreeMap::new();
    for (name, file) in files {
        inputs.insert(on_disk(file)?.to_string_lossy().to_lowercase(), name);
    }
    for (name, pdf) in &targets {
        let target = on_disk(pdf)?.to_string_lossy().to_lowercase();
        if let Some(input) = inputs.get(&target) {
            return Err(format!(
                "{name} would be rendered to {}, which is {input}, a file to run",
                pdf.display()
            ));
        }
    }
    Ok(targets)
}

/// Opens every file in a child process, `options.jobs` at a time, rendering it to its
/// target in `renders` if it has one. A failure of the runner itself, not of a file, stops
/// the run.
fn run_all(
    launcher: &Launcher,
    files: &[(String, PathBuf)],
    expected: &BTreeMap<String, String>,
    scoring: &BTreeMap<String, (PathBuf, PathBuf)>,
    renders: &BTreeMap<String, PathBuf>,
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
            let render = renders
                .get(name)
                .map(|pdf| (pdf.as_path(), options.font_dirs.as_slice()));
            if let Some((pdf, _)) = render {
                // A PDF left by an earlier run would be scored if this one is not written.
                if let Err(error) = std::fs::remove_file(pdf) {
                    if std::fs::symlink_metadata(pdf).is_ok() {
                        *fatal.lock().unwrap_or_else(PoisonError::into_inner) = Some(format!(
                            "cannot remove {} of an earlier run: {error}",
                            pdf.display()
                        ));
                        break;
                    }
                }
            }
            let (exit, elapsed) = match run_child(launcher, file, render, options.timeout) {
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
            if let Some((folder, pdf)) = scoring.get(name) {
                let unrendered = render.is_some()
                    && (result.status != Status::Opened || result.render_error.is_some());
                if unrendered {
                    result.score_error = Some("our PDF was not rendered".to_owned());
                } else {
                    match score_file(folder, pdf) {
                        Ok(scores) => result.scores = Some(scores),
                        Err(error) => result.score_error = Some(error),
                    }
                }
            }
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

/// Each document that has a reference folder, by its absolute path: the folder, and where
/// our PDF of it is. A reference's `geometry.json` names its document relative to the
/// folder above the references.
fn reference_index(options: &Options) -> Result<BTreeMap<PathBuf, (PathBuf, PathBuf)>, String> {
    #[derive(serde::Deserialize)]
    struct Named {
        document: String,
    }
    let (Some(references), Some(ours)) = (&options.references, &options.ours) else {
        return Ok(BTreeMap::new());
    };
    let references = lexical_absolute(references)?;
    let corpus = references
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", references.display()))?;
    let folders = |dir: &Path| -> Result<Vec<PathBuf>, String> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .and_then(|entries| {
                entries
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect()
            })
            .map_err(|error| format!("{}: {error}", dir.display()))?;
        found.retain(|path| path.is_dir());
        found.sort();
        Ok(found)
    };
    let mut index = BTreeMap::new();
    for group in folders(&references)? {
        for folder in folders(&group)? {
            let json = folder.join("geometry.json");
            if !json.is_file() {
                continue;
            }
            let named: Named = serde_json::from_str(&read(&json)?)
                .map_err(|error| format!("{}: {error}", json.display()))?;
            let plain = !named.document.is_empty()
                && Path::new(&named.document)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)));
            if !plain {
                return Err(format!(
                    "{}: document {} is not a plain path",
                    json.display(),
                    named.document
                ));
            }
            let pdf = ours.join(Path::new(&named.document).with_extension("pdf"));
            index.insert(corpus.join(&named.document), (folder, pdf));
        }
    }
    Ok(index)
}

fn score_file(reference: &Path, pdf: &Path) -> Result<metrics::Scores, String> {
    if !pdf.is_file() {
        return Err(format!("no PDF of ours at {}", pdf.display()));
    }
    let word = metrics::Rendering::reference(reference)?;
    let ours = metrics::Rendering::pdf(pdf)?;
    let mut scores = metrics::score(&ours, &word);
    let (valid, not_embedded) = metrics::check_pdf(pdf)?;
    scores.pdf_valid = Some(valid);
    scores.fonts_not_embedded = Some(not_embedded);
    scores.read_word_fonts(reference)?;
    Ok(scores)
}

/// Runs `corpus-runner open FILE`, with `--render PDF --font-dir DIR...` when `render`
/// gives the PDF and the font folders, stopping it after the time-out. `Err` is a failure
/// of the runner: the child could not be watched.
fn run_child(
    launcher: &Launcher,
    file: &Path,
    render: Option<(&Path, &[PathBuf])>,
    timeout: Duration,
) -> Result<(ChildExit, Duration), String> {
    let started = Instant::now();
    let mut command = Command::new(&launcher.program);
    command.args(&launcher.leading).arg("open").arg(file);
    if let Some((pdf, font_dirs)) = render {
        command.arg("--render").arg(pdf);
        for dir in font_dirs {
            command.arg("--font-dir").arg(dir);
        }
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    // A child that cannot be started says nothing about the file: like a thread that cannot
    // be started below, it stops the run.
    let mut child = child.map_err(|error| {
        format!(
            "cannot start {} to open {}: {error}",
            launcher.program.display(),
            file.display()
        )
    })?;
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
        // Rounded up, as in a memory failure's message.
        println!("peak memory: {} MiB, {}", kb.div_ceil(1024), largest.path);
    }
    let scored: Vec<&metrics::Scores> = report
        .files
        .iter()
        .filter_map(|file| file.scores.as_ref())
        .collect();
    if !scored.is_empty() {
        let mean = |value: fn(&metrics::Scores) -> f64| {
            scored.iter().map(|scores| value(scores)).sum::<f64>() / scored.len() as f64
        };
        println!(
            "scored {} documents: means text_score {:.4}, line_agreement {:.4}, ssim_mean {:.4}",
            scored.len(),
            mean(|scores| scores.text_score),
            mean(|scores| scores.line_agreement),
            mean(|scores| scores.ssim_mean)
        );
    }
    for file in &report.files {
        if let Some(error) = &file.score_error {
            println!("  {:<16} {}: {error}", "not scored", file.path);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A child that cannot be started is a failure of the runner, not a result of the file
    /// (#68).
    #[test]
    fn a_child_that_cannot_start_stops_the_run() {
        let launcher = Launcher {
            program: PathBuf::from("/nonexistent/corpus-runner-child"),
            leading: Vec::new(),
        };
        let error = run_child(
            &launcher,
            Path::new("docs/a.docx"),
            None,
            Duration::from_secs(5),
        )
        .err()
        .unwrap();
        assert!(
            error.starts_with("cannot start /nonexistent/corpus-runner-child to open docs/a.docx"),
            "{error}"
        );
    }

    /// The whole run stops: no file is skipped and the rest reported as if nothing happened.
    #[test]
    fn the_run_stops_when_a_child_cannot_start() {
        let launcher = Launcher {
            program: PathBuf::from("/nonexistent/corpus-runner-child"),
            leading: Vec::new(),
        };
        let files: Vec<(String, PathBuf)> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                (
                    format!("{name}.docx"),
                    PathBuf::from(format!("docs/{name}.docx")),
                )
            })
            .collect();
        let options = Options {
            paths: Vec::new(),
            root: PathBuf::from("."),
            report: None,
            baseline: None,
            update_baseline: false,
            allow_better: false,
            expected: None,
            timeout: Duration::from_secs(5),
            memory_limit_mb: 1024,
            enforce_memory_limit: false,
            jobs: 2,
            references: None,
            ours: None,
            render: None,
            font_dirs: Vec::new(),
            statuses_only: false,
        };
        let outcome = run_all(
            &launcher,
            &files,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &options,
        );
        let error = outcome.err().unwrap();
        assert!(
            error.starts_with("cannot start /nonexistent/corpus-runner-child"),
            "{error}"
        );
    }

    /// Names that differ only in case would share one PDF where the file system ignores
    /// case, as those of macOS and Windows do.
    #[test]
    fn names_that_differ_only_in_case_are_refused() {
        let files = [
            (
                "docs/Letter.docx".to_owned(),
                PathBuf::from("docs/Letter.docx"),
            ),
            (
                "docs/letter.docx".to_owned(),
                PathBuf::from("docs/letter.docx"),
            ),
        ];
        let options = Options {
            paths: Vec::new(),
            root: PathBuf::from("."),
            report: None,
            baseline: None,
            update_baseline: false,
            allow_better: false,
            expected: None,
            timeout: Duration::from_secs(5),
            memory_limit_mb: 1024,
            enforce_memory_limit: false,
            jobs: 2,
            references: None,
            ours: None,
            render: Some(PathBuf::from("pdfs")),
            font_dirs: Vec::new(),
            statuses_only: false,
        };
        let error = render_targets(&options, &files).unwrap_err();
        assert!(
            error.starts_with("docs/Letter.docx and docs/letter.docx would both be rendered to"),
            "{error}"
        );
        // Without --render nothing is rendered, so nothing clashes.
        let options = Options {
            render: None,
            ..options
        };
        assert!(render_targets(&options, &files).unwrap().is_empty());
    }
}
