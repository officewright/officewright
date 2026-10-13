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
    for (how, status) in [
        ("panic", "panic"),
        ("allocation", "memory"),
        ("abort", "crash"),
        ("hang", "timeout"),
    ] {
        let (code, output) = corpus.run(&["--timeout", "1", "docs/good.docx"], Some(how))?;
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
    let (code, output) = corpus.run(&["docs/notes-missing"], None)?;
    assert_eq!(code, 2, "{output}");
    let (code, output) = corpus.run(&["--jobs", "many", "docs"], None)?;
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("usage:"), "{output}");
    Ok(())
}
