//! Runs the `officewright` binary.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output};

fn officewright(args: &[&dyn AsRef<OsStr>]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_officewright"))
        .args(args.iter().map(|arg| arg.as_ref()))
        .output()
}

/// An empty directory for one test.
fn scratch(test: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("officewright-cli-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[test]
fn prints_version() -> io::Result<()> {
    let out = officewright(&[&"--version"])?;
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        concat!("officewright ", env!("CARGO_PKG_VERSION"), "\n")
    );
    Ok(())
}

#[test]
fn rejects_unknown_argument() -> io::Result<()> {
    assert_eq!(officewright(&[&"--bogus"])?.status.code(), Some(2));
    Ok(())
}

#[test]
fn shows_help_without_a_command() -> io::Result<()> {
    let out = officewright(&[])?;
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("text-to-pdf"));
    Ok(())
}

#[test]
fn converts_text_to_pdf() -> io::Result<()> {
    let dir = scratch("converts")?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "Hello, world\n\tGrüße\n")?;
    let out = officewright(&[&"text-to-pdf", &input, &output])?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    let pdf = fs::read(&output)?;
    assert!(pdf.starts_with(b"%PDF-1.4\n"));
    assert!(String::from_utf8_lossy(&pdf).contains("/MediaBox [0 0 595.2756 841.8898]"));
    fs::remove_dir_all(&dir)
}

#[test]
fn uses_the_requested_page_and_font_size() -> io::Result<()> {
    let dir = scratch("sizes")?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "x")?;
    let out = officewright(&[
        &"text-to-pdf",
        &"--page-size",
        &"letter",
        &"--font-size",
        &"9.5",
        &input,
        &output,
    ])?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pdf = String::from_utf8_lossy(&fs::read(&output)?).into_owned();
    assert!(pdf.contains("/MediaBox [0 0 612 792]"));
    assert!(pdf.contains("/F1 9.5 Tf"));
    fs::remove_dir_all(&dir)
}

#[test]
fn reports_the_line_and_column_of_an_unsupported_character() -> io::Result<()> {
    let dir = scratch("unsupported")?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "ok\nab Жк")?;
    let out = officewright(&[&"text-to-pdf", &input, &output])?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "error: {}:2:4: character U+0416 'Ж' at byte 6 is not in WinAnsiEncoding\n",
            input.display()
        )
    );
    assert!(!output.exists());
    fs::remove_dir_all(&dir)
}

#[test]
fn rejects_input_that_is_not_utf8() -> io::Result<()> {
    let dir = scratch("utf8")?;
    let input = dir.join("in.txt");
    fs::write(&input, b"abc\xFF")?;
    let out = officewright(&[&"text-to-pdf", &input, &dir.join("out.pdf")])?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!("error: {} is not valid UTF-8 at byte 3\n", input.display())
    );
    fs::remove_dir_all(&dir)
}

#[test]
fn reports_a_missing_input_file() -> io::Result<()> {
    let dir = scratch("missing")?;
    let input = dir.join("missing.txt");
    let out = officewright(&[&"text-to-pdf", &input, &dir.join("out.pdf")])?;
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with(&format!("error: cannot read {}: ", input.display())),
        "{stderr}"
    );
    fs::remove_dir_all(&dir)
}

#[test]
fn reports_an_output_it_cannot_write() -> io::Result<()> {
    let dir = scratch("unwritable")?;
    let input = dir.join("in.txt");
    let output = dir.join("no-such-dir").join("out.pdf");
    fs::write(&input, "x")?;
    let out = officewright(&[&"text-to-pdf", &input, &output])?;
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with(&format!("error: cannot write {}: ", output.display())),
        "{stderr}"
    );
    fs::remove_dir_all(&dir)
}

#[test]
fn rejects_font_sizes_that_are_not_positive_numbers() -> io::Result<()> {
    let dir = scratch("font-size")?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "x")?;
    for size in ["abc", "0", "-1", "nan", "inf"] {
        let out = officewright(&[&"text-to-pdf", &"--font-size", &size, &input, &output])?;
        assert_eq!(out.status.code(), Some(2), "{size}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("expected a number of points from 0.01 to 32767"),
            "{stderr}"
        );
    }
    assert!(!output.exists());
    fs::remove_dir_all(&dir)
}

/// A path in the repository.
fn repository(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

#[test]
fn converts_a_word_document_to_pdf_and_writes_its_report() -> io::Result<()> {
    let dir = scratch("docx")?;
    let output = dir.join("hello.pdf");
    let report = dir.join("report.json");
    let out = officewright(&[
        &"docx-to-pdf",
        &repository("crates/officewright-doc/tests/data/hello.docx"),
        &output,
        &"--font-dir",
        &repository("tests/fonts"),
        &"--no-system-fonts",
        &"--report",
        &report,
    ])?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(fs::read(&output)?.starts_with(b"%PDF-"));
    // Calibri, which the fonts given lack, is laid out in Carlito: a note, and an entry.
    let note = "note: document: \"Calibri\" → \"Carlito\" (metric-compatible substitute)\n";
    assert_eq!(stderr, note);
    assert_eq!(
        fs::read_to_string(&report)?,
        "{\"entries\": [\n    {\"kind\": \"font-substituted\", \"location\": \"document\", \
         \"message\": \"\\\"Calibri\\\" → \\\"Carlito\\\" (metric-compatible substitute)\"}\n]}\n"
    );
    fs::remove_dir_all(&dir)
}

#[test]
fn a_word_document_takes_its_fonts_from_files_too() -> io::Result<()> {
    // Carlito from its file, and no folder: the same substitution.
    let dir = scratch("docx-font")?;
    let output = dir.join("hello.pdf");
    let out = officewright(&[
        &"docx-to-pdf",
        &repository("crates/officewright-doc/tests/data/hello.docx"),
        &output,
        &"--font",
        &repository("tests/fonts/carlito/Carlito-Regular.ttf"),
        &"--no-system-fonts",
    ])?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("\"Carlito\""), "{stderr}");
    assert!(String::from_utf8_lossy(&fs::read(&output)?).contains("Carlito-Regular"));
    fs::remove_dir_all(&dir)
}

#[test]
fn a_word_document_past_the_page_limit_is_refused() -> io::Result<()> {
    let dir = scratch("docx-pages")?;
    let output = dir.join("pages.pdf");
    let out = officewright(&[
        &"docx-to-pdf",
        &repository("tests/corpus/word-rules/E-space-before-top.docx"),
        &output,
        &"--font-dir",
        &repository("tests/fonts"),
        &"--no-system-fonts",
        &"--max-pages",
        &"1",
    ])?;
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error: ") && stderr.contains("pages"),
        "{stderr}"
    );
    assert!(!output.exists());
    // A limit of 0 pages is a usage error.
    let usage = officewright(&[&"docx-to-pdf", &"in.docx", &output, &"--max-pages", &"0"])?;
    assert_eq!(usage.status.code(), Some(2));
    fs::remove_dir_all(&dir)
}

#[test]
fn a_file_that_is_no_word_document_is_refused() -> io::Result<()> {
    let dir = scratch("docx-not")?;
    let input = dir.join("in.docx");
    fs::write(&input, "not a zip")?;
    let out = officewright(&[&"docx-to-pdf", &input, &dir.join("out.pdf")])?;
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("error: "), "{stderr}");
    fs::remove_dir_all(&dir)
}
