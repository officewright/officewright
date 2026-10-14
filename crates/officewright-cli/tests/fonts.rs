//! Runs `officewright text-to-pdf --font` with officewright-font's minimal test font
//! (1000 units per em, with glyphs for the space, `A`, the acute accent and `Á`) and, by
//! family name, with the OFL fonts in tests/fonts at the repository root, and
//! `officewright docx-to-pdf` on Word documents of the repository with those fonts. They
//! keep their own licenses (THIRD-PARTY-NOTICES.md), so this test is not in the published
//! crate.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn officewright(args: &[&dyn AsRef<OsStr>]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_officewright"))
        .args(args.iter().map(|arg| arg.as_ref()))
        .output()
}

/// An empty directory for one test.
fn scratch(test: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "officewright-cli-fonts-{}-{test}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn minimal_font() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../officewright-font/tests/data/minimal.ttf")
}

/// A copy of the minimal font in `dir` with `fsType` (what its license allows) set to
/// `fs_type`.
fn minimal_font_with_fs_type(dir: &Path, fs_type: u16) -> io::Result<PathBuf> {
    let mut font = fs::read(minimal_font())?;
    let u32_at = |at: usize| -> Option<usize> {
        let bytes = font.get(at..at + 4)?.try_into().ok()?;
        usize::try_from(u32::from_be_bytes(bytes)).ok()
    };
    let tables = font
        .get(4..6)
        .and_then(|n| n.try_into().ok())
        .map_or(0, |n| usize::from(u16::from_be_bytes(n)));
    let os2 = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&record| font.get(record..record + 4) == Some(&b"OS/2"[..]))
        .and_then(|record| u32_at(record + 8))
        .ok_or_else(|| io::Error::other("no OS/2 table"))?;
    font.get_mut(os2 + 8..os2 + 10)
        .ok_or_else(|| io::Error::other("short OS/2 table"))?
        .copy_from_slice(&fs_type.to_be_bytes());
    let path = dir.join(format!("font-{fs_type:04x}.ttf"));
    fs::write(&path, font)?;
    Ok(path)
}

#[test]
fn embeds_the_font_given_with_font() -> io::Result<()> {
    let dir = scratch("embeds")?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "A Á\nÁA")?;
    let out = officewright(&[&"text-to-pdf", &"--font", &minimal_font(), &input, &output])?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    let pdf = String::from_utf8_lossy(&fs::read(&output)?).into_owned();
    assert!(pdf.contains("+OfficewrightTest-Regular /Encoding /Identity-H"));
    assert!(pdf.contains("/FontFile2"));
    fs::remove_dir_all(&dir)
}

#[test]
fn reports_where_the_font_has_no_glyph() -> io::Result<()> {
    let dir = scratch("missing-glyph")?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "A A\nAЖ")?;
    let out = officewright(&[&"text-to-pdf", &"--font", &minimal_font(), &input, &output])?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "error: {}:2:2: font OfficewrightTest-Regular has no glyph for character U+0416 \
             'Ж' at byte 5\n",
            input.display()
        )
    );
    assert!(!output.exists());

    fs::write(&input, "A\nAא")?;
    let out = officewright(&[&"text-to-pdf", &"--font", &minimal_font(), &input, &output])?;
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "error: {}:2:2: character U+05D0 'א' at byte 3 is in the Hebrew script, which \
             needs shaping or right-to-left layout; that is not supported yet\n",
            input.display()
        )
    );
    fs::remove_dir_all(&dir)
}

#[test]
fn warns_about_what_the_font_license_restricts() -> io::Result<()> {
    let dir = scratch("license")?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "A")?;
    // Preview & Print embedding, no subsetting.
    let font = minimal_font_with_fs_type(&dir, 0x0104)?;
    let out = officewright(&[&"text-to-pdf", &"--font", &font, &input, &output])?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "warning: font OfficewrightTest-Regular: its license allows the embedded font only \
         for viewing and printing the document\n\
         warning: font OfficewrightTest-Regular: its license forbids subsetting, so the whole \
         font is embedded\n"
    );
    assert!(output.exists());

    // Restricted license embedding: no output.
    fs::remove_file(&output)?;
    let font = minimal_font_with_fs_type(&dir, 0x0002)?;
    let out = officewright(&[&"text-to-pdf", &"--font", &font, &input, &output])?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "error: the license of font OfficewrightTest-Regular does not allow embedding it\n"
    );
    assert!(!output.exists());
    fs::remove_dir_all(&dir)
}

#[test]
fn reports_fonts_it_cannot_use() -> io::Result<()> {
    let dir = scratch("bad-font")?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "A")?;
    let out = officewright(&[&"text-to-pdf", &"--font", &input, &input, &output])?;
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with(&format!(
            "error: cannot use the font in {}: data is not a TrueType or OpenType font or font \
             collection: ",
            input.display()
        )),
        "{stderr}"
    );

    let missing = dir.join("missing.ttf");
    let out = officewright(&[&"text-to-pdf", &"--font", &missing, &input, &output])?;
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with(&format!("error: cannot read {}: ", missing.display())),
        "{stderr}"
    );
    assert!(!output.exists());
    fs::remove_dir_all(&dir)
}

fn test_fonts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fonts")
}

/// Runs text-to-pdf with `--font family`, looking it up only in tests/fonts, so the
/// result is the same on every machine.
fn convert_by_name(test: &str, family: &str) -> io::Result<(Output, Vec<u8>, PathBuf)> {
    let dir = scratch(test)?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "Привет, мир")?;
    let fonts = test_fonts();
    let out = officewright(&[
        &"text-to-pdf",
        &"--font",
        &family,
        &"--font-dir",
        &fonts,
        &"--no-system-fonts",
        &input,
        &output,
    ])?;
    let pdf = fs::read(&output).unwrap_or_default();
    Ok((out, pdf, dir))
}

#[test]
fn looks_up_a_family_name_in_the_font_folders() -> io::Result<()> {
    let (out, pdf, dir) = convert_by_name("by-name", "liberation sans")?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(String::from_utf8_lossy(&pdf).contains("+LiberationSans /Encoding /Identity-H"));
    fs::remove_dir_all(&dir)
}

#[test]
fn warns_about_a_substituted_family() -> io::Result<()> {
    let (out, pdf, dir) = convert_by_name("substitute", "Arial")?;
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "warning: font \"Arial\" → \"Liberation Sans\" (metric-compatible substitute)\n"
    );
    assert!(String::from_utf8_lossy(&pdf).contains("+LiberationSans"));
    fs::remove_dir_all(&dir)
}

#[test]
fn reports_a_family_nothing_stands_in_for() -> io::Result<()> {
    let dir = scratch("not-found")?;
    let input = dir.join("in.txt");
    let output = dir.join("out.pdf");
    fs::write(&input, "A")?;
    let out = officewright(&[
        &"text-to-pdf",
        &"--font",
        &"Calibri",
        &"--no-system-fonts",
        &input,
        &output,
    ])?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "error: no font found for family \"Calibri\": no fonts were found in --font-dir\n"
    );
    assert!(!output.exists());
    // --font-dir is for family names; with a font file it is a mistake.
    let out = officewright(&[
        &"text-to-pdf",
        &"--font",
        &minimal_font(),
        &"--font-dir",
        &test_fonts(),
        &input,
        &output,
    ])?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "error: --font-dir and --no-system-fonts apply to a font family name, not a font file\n"
    );
    fs::remove_dir_all(&dir)
}

#[test]
fn takes_a_font_file_name_or_relative_path_as_a_file() -> io::Result<()> {
    let dir = scratch("file-names")?;
    fs::write(dir.join("in.txt"), "A")?;
    // Run in the scratch directory, so these are relative names: one that ends like a
    // font file, and one that is a path without an extension. Neither exists, and neither
    // is taken as a family name.
    for (font, message) in [
        ("missing.ttf", "error: cannot read missing.ttf: "),
        ("MISSING.OTC", "error: cannot read MISSING.OTC: "),
        ("sub/missing", "error: cannot read sub/missing: "),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_officewright"))
            .current_dir(&dir)
            .args(["text-to-pdf", "--font", font, "in.txt", "out.pdf"])
            .output()?;
        assert_eq!(out.status.code(), Some(1), "{font}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with(message), "{font}: {stderr}");
    }
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
    fs::remove_dir_all(&dir)
}
