//! Embeds every face of every font file under the given directories in a PDF, and counts
//! what happened, for checking font embedding over a corpus by hand:
//!
//! ```text
//! cargo run -p officewright-pdf --example embed_report -- OUT_DIR FONT_DIR...
//! ```
//!
//! For each face it draws the characters of a sample text in the Latin, Greek and
//! Cyrillic scripts that the face has, and writes the PDF to OUT_DIR, where qpdf and
//! pdffonts can check them. It prints every face that could not be embedded, and a
//! summary by outcome.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use officewright_font::FontFace;
use officewright_pdf::{Document, Font, PageSize, Pt};

const SAMPLE: &str = "The quick brown fox, Грüße, Съешь же ещё этих булок, Ξεσκεπάζω 0123 €";

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let out = PathBuf::from(
        args.next()
            .ok_or("usage: embed_report OUT_DIR FONT_DIR...")?,
    );
    fs::create_dir_all(&out)?;
    let mut files = Vec::new();
    for dir in args {
        collect(Path::new(&dir), &mut files);
    }
    files.sort();
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    let mut written = 0;
    for file in &files {
        let Ok(data) = fs::read(file) else {
            *outcomes.entry("unreadable file".to_owned()).or_default() += 1;
            continue;
        };
        let Ok(count) = FontFace::face_count(&data) else {
            *outcomes.entry("not a font".to_owned()).or_default() += 1;
            continue;
        };
        for index in 0..count {
            let outcome = match embed(&data, index) {
                Ok(pdf) => {
                    written += 1;
                    fs::write(out.join(format!("{written:05}.pdf")), pdf)?;
                    "embedded".to_owned()
                }
                Err(err) => {
                    let cause = std::error::Error::source(&err)
                        .map_or_else(String::new, |cause| format!(": {cause}"));
                    println!("{}, face {index}: {err}{cause}", file.display());
                    kind(&err)
                }
            };
            *outcomes.entry(outcome).or_default() += 1;
        }
    }
    println!("{} files", files.len());
    for (outcome, count) in outcomes {
        println!("{count:6} {outcome}");
    }
    Ok(())
}

/// The PDF with the sample text in face `index` of `data`.
fn embed(data: &[u8], index: u32) -> Result<Vec<u8>, officewright_pdf::Error> {
    let font = Font::from_bytes(data, index)?;
    let face = FontFace::from_bytes(data, index)
        .map_err(|source| officewright_pdf::Error::Font { path: None, source })?;
    let text: String = SAMPLE
        .chars()
        .filter(|&ch| ch == ' ' || face.glyph_id(ch).is_some())
        .collect();
    let mut doc = Document::new();
    doc.add_page(PageSize::A4)?
        .add_text(Pt(72.0), Pt(770.0), &font, Pt(12.0), &text)?;
    doc.to_bytes()
}

/// The outcome an error stands for in the summary.
fn kind(err: &officewright_pdf::Error) -> String {
    match err {
        officewright_pdf::Error::EmbeddingNotAllowed { .. } => "license forbids embedding".into(),
        officewright_pdf::Error::Font { source, .. }
        | officewright_pdf::Error::Embed { source, .. } => match source {
            officewright_font::Error::UnsupportedOutlines { .. }
            | officewright_font::Error::SubsetUnsupported { .. } => {
                "not supported by the subsetter yet".into()
            }
            officewright_font::Error::SubsetterPanicked { .. } => "the subsetter panicked".into(),
            _ => "damaged".into(),
        },
        officewright_pdf::Error::MissingGlyph { .. } => "no glyph for a space".into(),
        other => format!("other: {other}"),
    }
}

/// Adds the font files under `dir` to `files`.
fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ["ttf", "otf", "ttc", "otc"]
                    .iter()
                    .any(|known| ext.eq_ignore_ascii_case(known))
            })
        {
            files.push(path);
        }
    }
}
