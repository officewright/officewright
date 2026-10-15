//! Runs the corpus runner as a program over small corpora in temporary directories.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A temporary corpus with one Word document that opens and one that does not.
struct Corpus {
    dir: PathBuf,
}

impl Corpus {
    fn new(name: &str) -> io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("corpus-runner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs/nested"))?;
        let document = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/word-rules/E-table-indent-15.docx");
        std::fs::copy(document, dir.join("docs/good.docx"))?;
        std::fs::write(dir.join("docs/nested/BROKEN.DOCX"), b"not a zip archive")?;
        std::fs::write(dir.join("docs/notes.txt"), b"not a Word document")?;
        Ok(Corpus { dir })
    }

    fn run(&self, args: &[&str], misbehave: Option<&str>) -> io::Result<(i32, String)> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corpus-runner"));
        command.current_dir(&self.dir).arg("run").args(args);
        match misbehave {
            Some(how) => command.env("CORPUS_RUNNER_MISBEHAVE", how),
            None => command.env_remove("CORPUS_RUNNER_MISBEHAVE"),
        };
        let Output {
            status,
            stdout,
            stderr,
        } = command.output()?;
        let text =
            String::from_utf8_lossy(&stdout).into_owned() + &String::from_utf8_lossy(&stderr);
        Ok((status.code().unwrap_or(-1), text))
    }

    fn read(&self, name: &str) -> io::Result<String> {
        std::fs::read_to_string(self.dir.join(name))
    }

    fn write(&self, name: &str, text: &str) -> io::Result<()> {
        std::fs::write(self.dir.join(name), text)
    }
}

impl Drop for Corpus {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn word_documents_are_found_and_reported() -> io::Result<()> {
    let corpus = Corpus::new("report")?;
    let (code, output) = corpus.run(&["--report", "report.json", "docs"], None)?;
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("2 files in"), "{output}");
    assert!(output.contains("1 opened, 1 unexpected-error"), "{output}");
    assert!(
        output.contains("unexpected-error docs/nested/BROKEN.DOCX: malformed: zip archive"),
        "{output}"
    );
    let report: serde_json::Value = serde_json::from_str(&corpus.read("report.json")?).unwrap();
    let files = report["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["path"], "docs/good.docx");
    assert_eq!(files[0]["status"], "opened");
    assert!(files[0]["chars"].as_u64().unwrap() > 0);
    assert_eq!(files[1]["status"], "unexpected-error");
    assert_eq!(report["summary"]["opened"], 1);
    Ok(())
}

#[test]
fn listed_errors_are_expected() -> io::Result<()> {
    let corpus = Corpus::new("expected")?;
    corpus.write(
        "expected.tsv",
        "# reasons\ndocs/nested/BROKEN.DOCX\tdeliberately broken\n",
    )?;
    let (code, output) = corpus.run(&["--expected", "expected.tsv", "docs"], None)?;
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("1 opened, 1 expected-error"), "{output}");
    assert!(
        output.contains("docs/nested/BROKEN.DOCX: deliberately broken"),
        "{output}"
    );

    corpus.write("expected.tsv", "docs/good.docx\tbroken, once\n")?;
    let (code, output) = corpus.run(&["--expected", "expected.tsv", "docs"], None)?;
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("note: docs/good.docx opened; remove it from the expected errors"));

    corpus.write("expected.tsv", "docs/good.docx without a tab\n")?;
    let (code, output) = corpus.run(&["--expected", "expected.tsv", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("expected.tsv: line 1"), "{output}");
    Ok(())
}

#[test]
fn a_misbehaving_child_is_a_result_and_fails_the_run() -> io::Result<()> {
    let corpus = Corpus::new("misbehave")?;
    // The default time-out leaves a slow machine room to start a child; only the hang waits.
    for (how, timeout, status) in [
        ("panic", "30", "panic"),
        ("allocation", "30", "memory"),
        ("abort", "30", "crash"),
        ("hang", "1", "timeout"),
    ] {
        let started = std::time::Instant::now();
        let (code, output) = corpus.run(&["--timeout", timeout, "docs/good.docx"], Some(how))?;
        assert!(
            started.elapsed().as_secs() < 25,
            "{how} took {:?}",
            started.elapsed()
        );
        assert_eq!(code, 1, "{how}: {output}");
        assert!(output.contains("1 files in"), "{how}: {output}");
        assert!(output.contains(&format!(": 1 {status}")), "{how}: {output}");
        assert!(
            output.contains("FAILED: 1 files made the library"),
            "{how}: {output}"
        );
    }
    let (_, output) = corpus.run(&["docs/good.docx"], Some("panic"))?;
    assert!(
        output.contains("docs/good.docx: misbehaving on purpose"),
        "{output}"
    );
    Ok(())
}

#[test]
fn memory_over_the_limit_fails_the_file() -> io::Result<()> {
    let corpus = Corpus::new("memory")?;
    let (code, output) = corpus.run(&["--memory-limit", "0", "docs/good.docx"], None)?;
    if cfg!(target_os = "linux") {
        assert_eq!(code, 1, "{output}");
        assert!(output.contains("over the limit of 0 MiB"), "{output}");
        // The summary rounds the peak as the file's message does.
        let peak = |prefix: &str| {
            output
                .lines()
                .find_map(|line| line.split(prefix).nth(1)?.split(" MiB").next())
                .map(str::to_owned)
        };
        assert_eq!(peak("peak memory: "), peak("peak memory "), "{output}");
    } else {
        // Peak memory is measured on Linux only.
        assert_eq!(code, 0, "{output}");
    }
    Ok(())
}

#[test]
fn the_baseline_holds_every_status() -> io::Result<()> {
    let corpus = Corpus::new("baseline")?;
    let base = ["--baseline", "baseline.json"];
    let (code, output) = corpus.run(&[&base[..], &["docs"]].concat(), None)?;
    assert_eq!(code, 2, "a missing baseline is an error: {output}");

    let (code, output) = corpus.run(&[&base[..], &["--update-baseline", "docs"]].concat(), None)?;
    assert_eq!(code, 0, "{output}");
    assert_eq!(
        corpus.read("baseline.json")?,
        "{\n  \"format\": 1,\n  \"files\": {\n    \"docs/good.docx\": \"opened\",\n    \
         \"docs/nested/BROKEN.DOCX\": \"unexpected-error\"\n  }\n}\n"
    );
    let (code, output) = corpus.run(&[&base[..], &["docs"]].concat(), None)?;
    assert_eq!(code, 0, "{output}");

    // Worse: a document that opened panics.
    let (code, output) = corpus.run(&[&base[..], &["docs"]].concat(), Some("panic"))?;
    assert_eq!(code, 1, "{output}");
    assert!(
        output
            .contains("worse than the baseline: docs/good.docx: opened in the baseline, now panic")
    );
    assert!(
        output.contains("FAILED: 2 files worse than the baseline"),
        "{output}"
    );

    // Better: an error becomes expected, which must be accepted into the baseline.
    corpus.write(
        "expected.tsv",
        "docs/nested/BROKEN.DOCX\tdeliberately broken\n",
    )?;
    let expected = ["--expected", "expected.tsv"];
    let (code, output) = corpus.run(&[&base[..], &expected[..], &["docs"]].concat(), None)?;
    assert_eq!(code, 1, "{output}");
    assert!(
        output.contains("better than the baseline: docs/nested/BROKEN.DOCX"),
        "{output}"
    );
    assert!(
        output.contains("FAILED: 1 file better than the baseline"),
        "{output}"
    );
    assert!(output.contains("--update-baseline"), "{output}");
    // The nightly run over the external corpora fails only on worse files.
    let allow = ["--allow-better"];
    let (code, output) = corpus.run(
        &[&base[..], &expected[..], &allow[..], &["docs"]].concat(),
        None,
    )?;
    assert_eq!(code, 0, "{output}");
    assert!(
        output.contains("better than the baseline: docs/nested/BROKEN.DOCX"),
        "{output}"
    );
    // A regression that is no crash: the document that opened no longer does.
    let good = std::fs::read(corpus.dir.join("docs/good.docx"))?;
    corpus.write("docs/good.docx", "not a zip archive any more")?;
    let (code, output) = corpus.run(&[&base[..], &allow[..], &["docs"]].concat(), None)?;
    assert_eq!(code, 1, "worse still fails: {output}");
    assert!(
        output.contains(
            "worse than the baseline: docs/good.docx: opened in the baseline, now unexpected-error"
        ),
        "{output}"
    );
    std::fs::write(corpus.dir.join("docs/good.docx"), good)?;

    // A file the baseline does not know, and one it knows that is gone.
    std::fs::rename(
        corpus.dir.join("docs/good.docx"),
        corpus.dir.join("docs/renamed.docx"),
    )
    .unwrap();
    let (code, output) = corpus.run(&[&base[..], &["docs"]].concat(), None)?;
    assert_eq!(code, 1, "{output}");
    assert!(
        output.contains("docs/good.docx: in the baseline but not in this run"),
        "{output}"
    );
    assert!(
        output.contains("docs/renamed.docx: not in the baseline (opened)"),
        "{output}"
    );
    assert!(
        output.contains("FAILED: 2 files worse than the baseline"),
        "{output}"
    );
    // A file gone from the run that the baseline also has scores for: one file, two changes.
    corpus.write(
        "baseline.json",
        "{\"format\": 1, \"files\": {\"docs/good.docx\": \"opened\", \"docs/nested/BROKEN.DOCX\": \"unexpected-error\", \"docs/renamed.docx\": \"opened\"},
            \"scores\": {\"docs/good.docx\": {\"text_score\": 1.0}}}",
    )?;
    let (code, output) = corpus.run(&[&base[..], &["docs"]].concat(), None)?;
    assert_eq!(code, 1, "{output}");
    assert!(
        output.contains("FAILED: 1 file (2 changes) worse than the baseline"),
        "{output}"
    );
    Ok(())
}

#[test]
fn paths_outside_the_root_and_bad_options_are_refused() -> io::Result<()> {
    let corpus = Corpus::new("refused")?;
    let (code, output) = corpus.run(&["--root", "docs/nested", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("is not under the root"), "{output}");
    // ".." is resolved by name, the same on every system: a path through it that leaves the
    // root is not under it (#68).
    let (code, output) =
        corpus.run(&["--root", "docs/nested", "docs/nested/../good.docx"], None)?;
    assert_eq!(code, 2, "a path through .. leaves the root: {output}");
    assert!(output.contains("is not under the root"), "{output}");
    Ok(())
}

/// A path through ".." that stays inside the root is the file it names, and so is a root
/// through "..", also from a subfolder (#68).
#[test]
fn paths_through_dots_inside_the_root_are_resolved() -> io::Result<()> {
    let corpus = Corpus::new("dots")?;
    let path_of = |name: &str| -> io::Result<serde_json::Value> {
        let report: serde_json::Value = serde_json::from_str(&corpus.read(name)?).unwrap();
        Ok(report["files"][0]["path"].clone())
    };
    let (code, output) = corpus.run(&["--report", "a.json", "docs/nested/../good.docx"], None)?;
    assert_eq!(code, 0, "{output}");
    assert_eq!(path_of("a.json")?, "docs/good.docx");
    let (code, output) = corpus.run(
        &[
            "--root",
            "docs/nested/..",
            "--report",
            "b.json",
            "docs/good.docx",
        ],
        None,
    )?;
    assert_eq!(code, 0, "{output}");
    assert_eq!(path_of("b.json")?, "good.docx");
    let from_docs = Command::new(env!("CARGO_BIN_EXE_corpus-runner"))
        .current_dir(corpus.dir.join("docs"))
        .args([
            "run",
            "--root",
            "..",
            "--report",
            "../c.json",
            "../docs/good.docx",
        ])
        .output()?;
    assert_eq!(
        from_docs.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&from_docs.stderr)
    );
    assert_eq!(path_of("c.json")?, "docs/good.docx");
    // Two spellings of one file are one file.
    let (code, output) = corpus.run(
        &[
            "--report",
            "d.json",
            "docs/good.docx",
            "docs/nested/../good.docx",
        ],
        None,
    )?;
    assert_eq!(code, 0, "{output}");
    let report: serde_json::Value = serde_json::from_str(&corpus.read("d.json")?).unwrap();
    assert_eq!(report["files"].as_array().unwrap().len(), 1, "{report}");
    Ok(())
}

/// A symbolic link before ".." does not make the child open another file than the one the
/// report names: the path is resolved by name before it is opened.
#[cfg(unix)]
#[test]
fn the_file_opened_is_the_file_named_after_a_link() -> io::Result<()> {
    let corpus = Corpus::new("link")?;
    // docs/link points into a folder whose good.docx is broken; by name,
    // docs/link/../good.docx is docs/good.docx, which opens.
    std::fs::create_dir_all(corpus.dir.join("elsewhere/inner"))?;
    std::fs::write(corpus.dir.join("elsewhere/good.docx"), b"not a zip archive")?;
    std::os::unix::fs::symlink(
        corpus.dir.join("elsewhere/inner"),
        corpus.dir.join("docs/link"),
    )?;
    let (code, output) = corpus.run(&["--report", "r.json", "docs/link/../good.docx"], None)?;
    assert_eq!(code, 0, "{output}");
    let report: serde_json::Value = serde_json::from_str(&corpus.read("r.json")?).unwrap();
    assert_eq!(report["files"][0]["path"], "docs/good.docx");
    assert_eq!(report["files"][0]["status"], "opened", "{report}");
    Ok(())
}

#[test]
fn bad_options_are_refused() -> io::Result<()> {
    let corpus = Corpus::new("options")?;
    let (code, output) = corpus.run(&["docs/notes-missing"], None)?;
    assert_eq!(code, 2, "{output}");
    let (code, output) = corpus.run(&["--allow-better", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    let (code, output) = corpus.run(&["--jobs", "many", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("usage:"), "{output}");
    Ok(())
}

#[test]
fn a_baseline_of_another_format_is_refused() -> io::Result<()> {
    let corpus = Corpus::new("format")?;
    corpus.write("baseline.json", "{\"format\": 2, \"files\": {}}")?;
    let (code, output) = corpus.run(&["--baseline", "baseline.json", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(
        output.contains("baseline format 2, this runner reads format 1"),
        "{output}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn links_are_followed_when_named_and_to_documents() -> io::Result<()> {
    use std::os::unix::fs::symlink;
    let corpus = Corpus::new("links")?;
    let dir = &corpus.dir;
    std::fs::create_dir_all(dir.join("walked"))?;
    symlink(dir.join("docs/good.docx"), dir.join("walked/linked.docx"))?;
    symlink(dir.join("docs"), dir.join("walked/linked-dir"))?;
    symlink(dir.join("docs"), dir.join("walked/linked-dir.docx"))?;
    symlink(
        dir.join("docs/missing.docx"),
        dir.join("walked/broken.docx"),
    )?;
    symlink(dir.join("docs"), dir.join("named"))?;
    // Inside a walked directory: the link to a document is run, the broken one is reported,
    // the link to a directory is not walked.
    let (code, output) = corpus.run(&["walked"], None)?;
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("2 files in"), "{output}");
    assert!(
        output.contains("unexpected-error walked/broken.docx: io:"),
        "{output}"
    );
    assert!(!output.contains("linked-dir"), "{output}");
    // A link named on the command line is walked.
    let (code, output) = corpus.run(&["named"], None)?;
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("2 files in"), "{output}");
    assert!(output.contains("named/nested/BROKEN.DOCX"), "{output}");
    Ok(())
}

#[test]
fn documents_with_a_reference_are_scored_or_say_why_not() -> io::Result<()> {
    let corpus = Corpus::new("references")?;
    let reference = corpus.dir.join("references/docs/good");
    std::fs::create_dir_all(&reference)?;
    std::fs::write(
        reference.join("geometry.json"),
        r#"{"format": 1, "document": "docs/good.docx", "pages": []}"#,
    )?;
    let options = [
        "--references",
        "references",
        "--ours",
        "ours",
        "--report",
        "report.json",
    ];
    let (code, output) = corpus.run(&[&options[..], &["docs"]].concat(), None)?;
    assert_eq!(code, 0, "{output}");
    assert!(
        output.contains("not scored       docs/good.docx: no PDF of ours at ours"),
        "{output}"
    );
    let report: serde_json::Value = serde_json::from_str(&corpus.read("report.json")?).unwrap();
    assert!(
        report["files"][0]["score_error"]
            .as_str()
            .unwrap()
            .starts_with("no PDF of ours")
    );
    assert!(
        report["files"][1].get("score_error").is_none(),
        "no reference, nothing to score"
    );
    // A path through ".." finds its reference too, and so does a reference folder named
    // through "..": the paths are compared resolved.
    let dots = [
        &["--references", "docs/../references", "--ours", "ours"][..],
        &["docs/nested/../good.docx"],
    ]
    .concat();
    let (code, output) = corpus.run(&dots, None)?;
    assert_eq!(code, 0, "{output}");
    assert!(
        output.contains("not scored       docs/good.docx: no PDF of ours at ours"),
        "{output}"
    );

    // A baseline that holds scores fails a run that does not score.
    corpus.write(
        "baseline.json",
        r#"{"format": 1, "files": {"docs/good.docx": "opened", "docs/nested/BROKEN.DOCX": "unexpected-error"},
            "scores": {"docs/good.docx": {"text_score": 1.0}}}"#,
    )?;
    let (code, output) = corpus.run(&["--baseline", "baseline.json", "docs"], None)?;
    assert_eq!(code, 1, "{output}");
    assert!(
        output.contains("docs/good.docx: scored in the baseline, now not scored in this run"),
        "{output}"
    );

    // A reference that names a path out of the corpus is refused.
    std::fs::write(
        reference.join("geometry.json"),
        r#"{"format": 1, "document": "../escape.docx", "pages": []}"#,
    )?;
    let (code, output) = corpus.run(&[&options[..], &["docs"]].concat(), None)?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("is not a plain path"), "{output}");
    let (code, output) = corpus.run(&["--references", "references", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(
        output.contains("--references and --ours go together"),
        "{output}"
    );
    Ok(())
}

#[test]
fn score_takes_its_two_options_in_either_order() -> io::Result<()> {
    let corpus = Corpus::new("score")?;
    for args in [
        ["score", "--ours", "docs/x.pdf", "--reference", "missing"],
        ["score", "--reference", "missing", "--ours", "docs/x.pdf"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_corpus-runner"))
            .current_dir(&corpus.dir)
            .args(args)
            .output()?;
        let text = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{text}");
        assert!(
            text.contains("geometry.json"),
            "the reference is read first: {text}"
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_corpus-runner"))
        .current_dir(&corpus.dir)
        .args(["score", "--ours", "docs/x.pdf"])
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
    Ok(())
}

/// A branch report made over fewer paths than main's cannot hide main's failing document
/// (#130): the gate counts it in row 2, names it, and fails.
#[test]
fn the_gate_refuses_a_branch_report_that_lacks_files_of_main() -> io::Result<()> {
    let corpus = Corpus::new("gate")?;
    let (_, output) = corpus.run(&["--report", "main.json", "docs"], None)?;
    assert!(output.contains("1 unexpected-error"), "{output}");
    let (code, output) = corpus.run(&["--report", "branch.json", "docs/good.docx"], None)?;
    assert_eq!(code, 0, "{output}");
    let gate = |args: &[&str]| -> io::Result<(Option<i32>, String)> {
        let output = Command::new(env!("CARGO_BIN_EXE_corpus-runner"))
            .current_dir(&corpus.dir)
            .arg("gate")
            .args(args)
            .output()?;
        Ok((
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr),
        ))
    };
    let (code, text) = gate(&["branch.json"])?;
    assert_eq!(code, Some(0), "alone, the narrower report passes: {text}");
    let (code, text) = gate(&["--main", "main.json", "branch.json"])?;
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("Files of main the branch report lacks: docs/nested/BROKEN.DOCX"),
        "{text}"
    );
    let refusals = text
        .lines()
        .find(|line| line.starts_with("| 2 |"))
        .unwrap_or_default();
    assert!(
        refusals.contains("of main's, 1 files missing") && refusals.contains("(worse than main)"),
        "{refusals}"
    );
    Ok(())
}

/// A branch report made without the references cannot hide main's reference documents
/// (#130): the gate counts them in row 2, names them, and fails.
#[test]
fn the_gate_refuses_a_branch_report_made_without_the_references() -> io::Result<()> {
    let corpus = Corpus::new("gate-references")?;
    let reference = corpus.dir.join("references/docs/good");
    std::fs::create_dir_all(&reference)?;
    std::fs::write(
        reference.join("geometry.json"),
        r#"{"format": 1, "document": "docs/good.docx", "pages": []}"#,
    )?;
    let main = [
        "--references",
        "references",
        "--ours",
        "ours",
        "--report",
        "main.json",
        "docs/good.docx",
    ];
    let (code, output) = corpus.run(&main, None)?;
    assert_eq!(code, 0, "{output}");
    let (code, output) = corpus.run(&["--report", "branch.json", "docs/good.docx"], None)?;
    assert_eq!(code, 0, "{output}");
    let output = Command::new(env!("CARGO_BIN_EXE_corpus-runner"))
        .current_dir(&corpus.dir)
        .args(["gate", "--main", "main.json", "branch.json"])
        .output()?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains(
            "Reference documents of main the branch report did not score: docs/good.docx"
        ),
        "{text}"
    );
    assert!(
        text.contains("of main's, 0 files missing and 1 reference documents not scored"),
        "{text}"
    );
    Ok(())
}

/// Whether Word's font table is complete reaches the report, and the gate reads it from
/// there (#133): an export without the runs gives no class, a complete one gives a class.
/// Scoring needs poppler and qpdf, which check.sh requires on Linux and macOS and the MSRV
/// job installs.
#[cfg(unix)]
#[test]
fn the_report_says_whether_word_s_fonts_are_all_known() -> io::Result<()> {
    use officewright_pdf::{Document, Font, PageSize, Pt, StandardFont};
    let corpus = Corpus::new("word-fonts")?;
    let reference = corpus.dir.join("references/docs/good");
    std::fs::create_dir_all(&reference)?;
    std::fs::write(
        reference.join("geometry.json"),
        r#"{"format": 1, "document": "docs/good.docx", "page_count": 0, "pages": []}"#,
    )?;
    std::fs::create_dir_all(corpus.dir.join("ours/docs"))?;
    let mut doc = Document::new();
    doc.add_page(PageSize::A4)
        .unwrap()
        .add_text(
            Pt(72.0),
            Pt(770.0),
            &Font::standard(StandardFont::Helvetica),
            Pt(12.0),
            "Ours",
        )
        .unwrap();
    std::fs::write(
        corpus.dir.join("ours/docs/good.pdf"),
        doc.to_bytes().unwrap(),
    )?;
    let run = |runs: bool| -> io::Result<(serde_json::Value, String)> {
        std::fs::write(
            reference.join("formatting.json"),
            format!(r#"{{"complete": true, "runs": {runs}, "fonts": [{{"name": "Carlito"}}]}}"#),
        )?;
        let options = [
            "--references",
            "references",
            "--ours",
            "ours",
            "--report",
            "report.json",
            "docs/good.docx",
        ];
        let (code, output) = corpus.run(&options, None)?;
        assert_eq!(code, 0, "{output}");
        let report: serde_json::Value = serde_json::from_str(&corpus.read("report.json")?).unwrap();
        let gate = Command::new(env!("CARGO_BIN_EXE_corpus-runner"))
            .current_dir(&corpus.dir)
            .args(["gate", "report.json"])
            .output()?;
        Ok((
            report["files"][0]["scores"].clone(),
            String::from_utf8_lossy(&gate.stdout).into_owned(),
        ))
    };
    let (scores, census) = run(false)?;
    assert_eq!(
        scores["word_fonts"],
        serde_json::json!(["Carlito"]),
        "{scores}"
    );
    assert_eq!(scores["word_fonts_complete"], false, "{scores}");
    assert!(
        census.contains("S 0, T 0, N 0, no class 1 (1 without the fonts of runs)"),
        "{census}"
    );
    let (scores, census) = run(true)?;
    assert_eq!(scores["word_fonts_complete"], true, "{scores}");
    assert!(census.contains("S 1, T 0, N 0, no class 0;"), "{census}");
    Ok(())
}

#[test]
fn documents_are_rendered_where_the_scoring_finds_them() -> io::Result<()> {
    let corpus = Corpus::new("render")?;
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fonts");
    let fonts = fonts.to_str().unwrap();

    // Without references: each document that opens is rendered under its name.
    let (code, output) = corpus.run(
        &[
            "--render",
            "pdfs",
            "--font-dir",
            fonts,
            "--report",
            "report.json",
            "docs",
        ],
        None,
    )?;
    assert_eq!(code, 0, "{output}");
    assert!(std::fs::read(corpus.dir.join("pdfs/docs/good.pdf"))?.starts_with(b"%PDF-"));
    assert!(!corpus.dir.join("pdfs/docs/nested/BROKEN.pdf").exists());
    let report: serde_json::Value = serde_json::from_str(&corpus.read("report.json")?).unwrap();
    let entries = report["files"][0]["layout_report"].as_array().unwrap();
    assert!(
        entries.iter().all(|entry| entry["kind"].is_string()
            && entry["location"].is_string()
            && entry["message"].is_string()),
        "{entries:?}"
    );
    // Laid out with the fonts given: Calibri becomes Carlito from tests/fonts.
    assert!(
        entries
            .iter()
            .any(|entry| entry["kind"] == "font-substituted"
                && entry["message"].as_str().unwrap().contains("Carlito")),
        "{entries:?}"
    );
    assert!(report["files"][1].get("layout_report").is_none());

    // With references in docs/references (a group, then a folder per document), whose
    // documents are named from docs: rendered where the scoring looks, pdfs/good.pdf, and
    // scored from there.
    let reference = corpus.dir.join("docs/references/group/good");
    std::fs::create_dir_all(&reference)?;
    std::fs::write(
        reference.join("geometry.json"),
        r#"{"format": 1, "document": "good.docx", "pages": []}"#,
    )?;
    let (_, output) = corpus.run(
        &[
            "--render",
            "pdfs",
            "--font-dir",
            fonts,
            "--references",
            "docs/references",
            "--report",
            "scored.json",
            "docs",
        ],
        None,
    )?;
    assert!(corpus.dir.join("pdfs/good.pdf").is_file(), "{output}");
    let scored: serde_json::Value = serde_json::from_str(&corpus.read("scored.json")?).unwrap();
    assert_scored(&scored["files"][0], &output);
    Ok(())
}

/// That `file` of a report was scored from the PDF this run rendered. Scoring needs
/// poppler, which check.sh requires on Linux and macOS. Windows has no official build: there
/// a scoring that fails where poppler is missing still shows that it looked at this run's
/// PDF, and a poppler someone installed scores it.
fn assert_scored(file: &serde_json::Value, output: &str) {
    if file["scores"].is_object() {
        return;
    }
    let why = file["score_error"].as_str().unwrap_or_default();
    assert!(
        !cfg!(unix)
            && !why.is_empty()
            && !why.starts_with("no PDF of ours")
            && why != "our PDF was not rendered",
        "{file}\n{output}"
    );
}

#[test]
fn a_document_that_opens_but_cannot_be_rendered_fails_the_run() -> io::Result<()> {
    let corpus = Corpus::new("unrendered")?;
    // A file where the folder of the PDFs would go.
    corpus.write("blocked", "not a folder")?;
    let (code, output) = corpus.run(&["--render", "blocked", "docs"], None)?;
    assert_eq!(code, 1, "{output}");
    assert!(
        output.contains("not rendered: docs/good.docx: cannot make"),
        "{output}"
    );
    assert!(
        output.contains("FAILED: 1 files opened but could not be rendered"),
        "{output}"
    );
    // Fonts to render with need something to render.
    let (code, output) = corpus.run(&["--font-dir", "fonts", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("--font-dir needs --render"), "{output}");
    Ok(())
}

#[test]
fn a_run_of_statuses_only_keeps_the_baseline_s_scores() -> io::Result<()> {
    let corpus = Corpus::new("statuses")?;
    let baseline = r#"{"format": 1, "files": {"docs/good.docx": "opened", "docs/nested/BROKEN.DOCX": "unexpected-error"},
        "scores": {"docs/good.docx": {"text_score": 1.0}}}"#;
    corpus.write("baseline.json", baseline)?;
    // Not scored, but compared by status only: it passes.
    let (code, output) = corpus.run(
        &["--baseline", "baseline.json", "--statuses-only", "docs"],
        None,
    )?;
    assert_eq!(code, 0, "{output}");
    // Updating it keeps the scores it cannot measure.
    let (code, output) = corpus.run(
        &[
            "--baseline",
            "baseline.json",
            "--statuses-only",
            "--update-baseline",
            "docs",
        ],
        None,
    )?;
    assert_eq!(code, 0, "{output}");
    let written: serde_json::Value = serde_json::from_str(&corpus.read("baseline.json")?).unwrap();
    assert_eq!(written["scores"]["docs/good.docx"]["text_score"], 1.0);
    // It takes no references, and needs a baseline.
    let (code, output) = corpus.run(
        &[
            "--baseline",
            "baseline.json",
            "--statuses-only",
            "--references",
            "r",
            "--ours",
            "o",
            "docs",
        ],
        None,
    )?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("takes no --references"), "{output}");
    let (code, output) = corpus.run(&["--statuses-only", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("need --baseline"), "{output}");
    Ok(())
}

#[test]
fn a_run_of_statuses_only_still_fails_on_statuses() -> io::Result<()> {
    let corpus = Corpus::new("statuses-fail")?;
    let cases = [
        // A status changed for the worse.
        (
            r#"{"format": 1, "files": {"docs/good.docx": "opened", "docs/nested/BROKEN.DOCX": "opened"}}"#,
            "docs/nested/BROKEN.DOCX: opened in the baseline, now unexpected-error",
        ),
        // A file of the baseline is gone.
        (
            r#"{"format": 1, "files": {"docs/good.docx": "opened", "docs/nested/BROKEN.DOCX": "unexpected-error", "docs/gone.docx": "opened"}}"#,
            "docs/gone.docx: in the baseline but not in this run",
        ),
        // A file the baseline does not know.
        (
            r#"{"format": 1, "files": {"docs/good.docx": "opened"}}"#,
            "docs/nested/BROKEN.DOCX: not in the baseline",
        ),
    ];
    for (baseline, why) in cases {
        corpus.write("baseline.json", baseline)?;
        let (code, output) = corpus.run(
            &["--baseline", "baseline.json", "--statuses-only", "docs"],
            None,
        )?;
        assert_eq!(code, 1, "{output}");
        assert!(output.contains(why), "{why}\n{output}");
    }
    Ok(())
}

#[test]
fn two_documents_are_never_rendered_to_one_pdf() -> io::Result<()> {
    let corpus = Corpus::new("clash")?;
    std::fs::copy(
        corpus.dir.join("docs/good.docx"),
        corpus.dir.join("docs/good.dotx"),
    )?;
    let (code, output) = corpus.run(&["--render", "pdfs", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(
        output.contains("docs/good.docx and docs/good.dotx would both be rendered to"),
        "{output}"
    );
    assert!(!corpus.dir.join("pdfs").exists());
    Ok(())
}

#[test]
fn a_file_to_run_is_never_rendered_over() -> io::Result<()> {
    // Issue #257: a file named on the command line runs whatever its extension, and the
    // target of notes.pdf rendered into "." is notes.pdf itself; that of Notes.PDF is
    // Notes.pdf, the same file where names do not tell case apart.
    let corpus = Corpus::new("own-target")?;
    for name in ["notes.pdf", "Notes.PDF"] {
        std::fs::copy(corpus.dir.join("docs/good.docx"), corpus.dir.join(name))?;
        let before = std::fs::read(corpus.dir.join(name))?;
        let (code, output) = corpus.run(&["--render", ".", name], None)?;
        assert_eq!(code, 2, "{name}: {output}");
        assert!(output.contains("a file to run"), "{name}: {output}");
        assert_eq!(std::fs::read(corpus.dir.join(name))?, before, "{name}");
    }
    // Nor over another file to run: docs/good.docx is rendered to pdfs/docs/good.pdf.
    std::fs::create_dir_all(corpus.dir.join("pdfs/docs"))?;
    std::fs::copy(
        corpus.dir.join("docs/good.docx"),
        corpus.dir.join("pdfs/docs/good.pdf"),
    )?;
    let before = std::fs::read(corpus.dir.join("pdfs/docs/good.pdf"))?;
    let (code, output) = corpus.run(
        &["--render", "pdfs", "docs/good.docx", "pdfs/docs/good.pdf"],
        None,
    )?;
    assert_eq!(code, 2, "{output}");
    assert!(
        output.contains("docs/good.docx would be rendered to") && output.contains("a file to run"),
        "{output}"
    );
    assert_eq!(
        std::fs::read(corpus.dir.join("pdfs/docs/good.pdf"))?,
        before
    );
    // Nor through a link to the folder it is in.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(".", corpus.dir.join("here"))?;
        let (code, output) = corpus.run(&["--render", "here", "notes.pdf"], None)?;
        assert_eq!(code, 2, "{output}");
        assert!(output.contains("a file to run"), "{output}");
        assert_eq!(std::fs::read(corpus.dir.join("notes.pdf"))?.len(), {
            std::fs::read(corpus.dir.join("docs/good.docx"))?.len()
        });
        // A link's `..` is the parent of where it leads: jump/.. is elsewhere, not here,
        // so notes.pdf is rendered there.
        std::fs::create_dir_all(corpus.dir.join("elsewhere/inner"))?;
        std::os::unix::fs::symlink("elsewhere/inner", corpus.dir.join("jump"))?;
        let (code, output) = corpus.run(&["--render", "jump/..", "notes.pdf"], None)?;
        assert_eq!(code, 0, "{output}");
        assert!(corpus.dir.join("elsewhere/notes.pdf").exists(), "{output}");
        // Inputs are where they are on disk too: one named through a link, and one that is
        // itself a link to its target.
        let notes = std::fs::read(corpus.dir.join("notes.pdf"))?;
        let (code, output) = corpus.run(&["--render", ".", "here/notes.pdf"], None)?;
        assert_eq!(code, 2, "{output}");
        assert!(output.contains("a file to run"), "{output}");
        assert_eq!(std::fs::read(corpus.dir.join("notes.pdf"))?, notes);
        std::fs::create_dir_all(corpus.dir.join("store"))?;
        std::fs::copy(
            corpus.dir.join("docs/good.docx"),
            corpus.dir.join("store/a.pdf"),
        )?;
        std::os::unix::fs::symlink("store/a.pdf", corpus.dir.join("a.docx"))?;
        let stored = std::fs::read(corpus.dir.join("store/a.pdf"))?;
        let (code, output) = corpus.run(&["--render", "store", "a.docx"], None)?;
        assert_eq!(code, 2, "{output}");
        assert!(output.contains("a file to run"), "{output}");
        assert_eq!(std::fs::read(corpus.dir.join("store/a.pdf"))?, stored);
        // A folder of the target that does not exist yet, under a link: link/new/.. is the
        // link's folder, where the second file to run is.
        std::os::unix::fs::symlink("store", corpus.dir.join("link"))?;
        std::fs::create_dir_all(corpus.dir.join("store/docs"))?;
        std::fs::copy(
            corpus.dir.join("docs/good.docx"),
            corpus.dir.join("store/docs/good.pdf"),
        )?;
        let (code, output) = corpus.run(
            &[
                "--render",
                "link/new/..",
                "docs/good.docx",
                "store/docs/good.pdf",
            ],
            None,
        )?;
        assert_eq!(code, 2, "{output}");
        assert!(output.contains("a file to run"), "{output}");
        assert_eq!(
            std::fs::read(corpus.dir.join("store/docs/good.pdf"))?,
            stored
        );
        assert!(!corpus.dir.join("store/new").exists());
        // A `..` that climbs out of a folder that does not exist comes back to folders
        // that do, where a link can follow: missing/../here is here, a link to ".".
        let (code, output) = corpus.run(&["--render", "missing/../here", "notes.pdf"], None)?;
        assert_eq!(code, 2, "{output}");
        assert!(output.contains("a file to run"), "{output}");
        assert_eq!(std::fs::read(corpus.dir.join("notes.pdf"))?, notes);
        assert!(!corpus.dir.join("missing").exists());
        // The target itself a link to the file to run, after such a climb: removing
        // missing/../b.pdf finds nothing while `missing` does not exist, and writing it
        // would follow the link.
        std::fs::copy(corpus.dir.join("docs/good.docx"), corpus.dir.join("b.docx"))?;
        std::os::unix::fs::symlink("b.docx", corpus.dir.join("b.pdf"))?;
        let b = std::fs::read(corpus.dir.join("b.docx"))?;
        let (code, output) = corpus.run(&["--render", "missing/..", "b.docx"], None)?;
        assert_eq!(code, 2, "{output}");
        assert!(output.contains("a file to run"), "{output}");
        assert_eq!(std::fs::read(corpus.dir.join("b.docx"))?, b);
        assert!(!corpus.dir.join("missing").exists());
    }
    // Rendered elsewhere, it runs.
    let (code, output) = corpus.run(&["--render", "out", "notes.pdf"], None)?;
    assert_eq!(code, 0, "{output}");
    assert!(corpus.dir.join("out/notes.pdf").exists(), "{output}");
    Ok(())
}

#[test]
fn a_pdf_of_an_earlier_run_is_never_scored() -> io::Result<()> {
    let corpus = Corpus::new("stale")?;
    let reference = corpus.dir.join("docs/references/group/good");
    std::fs::create_dir_all(&reference)?;
    std::fs::write(
        reference.join("geometry.json"),
        r#"{"format": 1, "document": "good.docx", "pages": []}"#,
    )?;
    let args = [
        "--render",
        "pdfs",
        "--references",
        "docs/references",
        "--report",
        "report.json",
        "docs",
    ];
    let good = |corpus: &Corpus| -> io::Result<serde_json::Value> {
        let report: serde_json::Value = serde_json::from_str(&corpus.read("report.json")?).unwrap();
        Ok(report["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == "docs/good.docx")
            .unwrap()
            .clone())
    };
    let unscored = |file: &serde_json::Value| {
        assert!(
            file.get("scores").is_none_or(serde_json::Value::is_null),
            "{file}"
        );
        assert_eq!(file["score_error"], "our PDF was not rendered");
    };
    let (_, output) = corpus.run(&args, None)?;
    assert!(corpus.dir.join("pdfs/good.pdf").is_file(), "{output}");
    let first = good(&corpus)?;
    assert_scored(&first, &output);
    // Rendered without font folders and without the system's fonts: Calibri has no font,
    // on every machine.
    assert!(
        first["layout_report"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["message"] == "\"Calibri\" → no font available"),
        "{first}"
    );
    // The child dies before it renders: the earlier PDF is gone, and nothing is scored.
    let (code, output) = corpus.run(&args, Some("panic"))?;
    assert_eq!(code, 1, "{output}");
    assert!(!corpus.dir.join("pdfs/good.pdf").exists(), "{output}");
    unscored(&good(&corpus)?);
    // The document opens but its rendering fails (a font folder that does not exist):
    // not scored either, though it opened.
    corpus.run(&args, None)?;
    let mut failing = args.to_vec();
    failing.splice(0..0, ["--font-dir", "no-fonts"]);
    let (code, output) = corpus.run(&failing, None)?;
    assert_eq!(code, 1, "{output}");
    assert!(output.contains("not rendered: docs/good.docx"), "{output}");
    let file = good(&corpus)?;
    assert_eq!(file["status"], "opened", "{file}");
    unscored(&file);
    // What stands at the target and cannot be removed stops the run, rather than be
    // taken for this run's PDF.
    std::fs::create_dir_all(corpus.dir.join("pdfs/good.pdf/inside"))?;
    let (code, output) = corpus.run(&args, None)?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("of an earlier run"), "{output}");
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn memory_is_measured_after_the_rendering() -> io::Result<()> {
    let corpus = Corpus::new("render-memory")?;
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fonts");
    let peak = |report: &str| -> io::Result<u64> {
        let report: serde_json::Value = serde_json::from_str(&corpus.read(report)?).unwrap();
        Ok(report["files"][0]["peak_rss_kb"].as_u64().unwrap())
    };
    corpus.run(&["--report", "opened.json", "docs"], None)?;
    let (_, output) = corpus.run(
        &[
            "--render",
            "pdfs",
            "--font-dir",
            fonts.to_str().unwrap(),
            "--report",
            "rendered.json",
            "docs",
        ],
        None,
    )?;
    // Rendering reads the fonts of tests/fonts, megabytes the opening alone never holds.
    let (opened, rendered) = (peak("opened.json")?, peak("rendered.json")?);
    assert!(
        rendered > opened + 1024,
        "{opened} KiB, then {rendered} KiB\n{output}"
    );
    Ok(())
}
