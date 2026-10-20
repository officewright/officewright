//! What a document keeps of its pages until it is written (#350): a binary of its own, as
//! it measures the memory of the whole process. It reads officewright-font's test font,
//! which is not part of this crate, so this test is not in the published crate. Linux
//! only, where `/proc` gives the memory.
#![cfg(target_os = "linux")]

use std::path::Path;

use officewright_font::GlyphId;
use officewright_pdf::{Document, Font, GlyphRun, PageSize, PlacedGlyph, Pt};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The resident memory of the process in bytes, from `/proc/self/statm`.
fn resident() -> TestResult<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm")?;
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .ok_or("no resident size")?
        .parse()?;
    // The page size of every Linux machine CI runs on.
    Ok(pages * 4096)
}

/// A page of text as a layout engine draws it: 40 lines of 80 glyphs, a cluster each.
fn draw_page(doc: &mut Document, font: &Font) -> TestResult {
    let page = doc.add_page(PageSize::A4)?;
    for line in 0..40 {
        let y = 800.0 - 18.0 * f64::from(line);
        let mut run = GlyphRun::new(font, Pt(10.0), Pt(40.0), Pt(y));
        for i in 0..80 {
            // A and the space of the test font, 6 and 2.5 pt wide at 10 pt.
            let (text, glyph) = if i % 6 == 5 { (" ", 1) } else { ("A", 2) };
            let x = f64::from(i) * 6.0;
            run.push_cluster(
                text,
                [PlacedGlyph::new(GlyphId::new(glyph), Pt(x), Pt(0.0))],
            );
        }
        page.draw_glyphs(&run)?;
    }
    Ok(())
}

#[test]
fn a_document_keeps_its_pages_encoded() -> TestResult {
    let data = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../officewright-font/tests/data/minimal.ttf"),
    )?;
    let font = Font::from_bytes(data, 0)?;
    let mut doc = Document::new();
    draw_page(&mut doc, &font)?;
    let before = resident()?;
    for _ in 0..400 {
        draw_page(&mut doc, &font)?;
    }
    let grown = resident()?.saturating_sub(before);
    // Kept as drawn, each page's runs took about 230 KB, 92 MB for the 400 pages; encoded,
    // its content stream takes about 14 KB.
    assert!(grown < 32 << 20, "the pages took {grown} bytes");
    // And the document is written as drawn.
    let pdf = doc.to_bytes()?;
    assert!(pdf.starts_with(b"%PDF-1.4\n"));
    Ok(())
}
