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
    Ok(())
}

#[test]
fn paths_outside_the_root_and_bad_options_are_refused() -> io::Result<()> {
    let corpus = Corpus::new("refused")?;
    let (code, output) = corpus.run(&["--root", "docs/nested", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("is not under the root"), "{output}");
    let (code, output) =
        corpus.run(&["--root", "docs/nested", "docs/nested/../good.docx"], None)?;
    assert_eq!(code, 2, "a path through .. leaves the root: {output}");
    assert!(output.contains("is not under the root"), "{output}");
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
/// Through pdftotext and pdftoppm, which check.sh requires on Linux and macOS.
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
