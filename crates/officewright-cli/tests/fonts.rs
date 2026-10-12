//! Runs `officewright text-to-pdf --font` with officewright-font's minimal test font:
//! 1000 units per em, with glyphs for the space, `A`, the acute accent and `Á`.

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
