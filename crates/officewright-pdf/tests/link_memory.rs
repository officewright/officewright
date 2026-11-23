//! What a link that spans many lines costs (#581): a binary of its own, as it measures the
//! memory of the whole process. Linux only, where `/proc` gives the memory.
#![cfg(target_os = "linux")]

use std::sync::Mutex;

use officewright_pdf::{Document, LinkTarget, PageSize, Pt};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Held by each test while it measures, as the memory is the whole process's.
static MEASURING: Mutex<()> = Mutex::new(());

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

#[test]
fn a_link_over_many_lines_keeps_and_writes_its_target_once() -> TestResult {
    // A URI of 32,000 bytes, the most a PDF string holds less a little, on 16,000 lines
    // of two pages: one annotation for each line.
    let _measuring = MEASURING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let uri = format!("https://example.com/{}", "a".repeat(32_000 - 20));
    let target = LinkTarget::Uri(uri.clone());
    let mut doc = Document::new();
    let before = resident()?;
    for _ in 0..2 {
        let page = doc.add_page(PageSize::A4)?;
        for line in 0..8000_u32 {
            let y = 800.0 - 0.1 * f64::from(line);
            page.add_link(Pt(72.0), Pt(y), Pt(400.0), Pt(0.1), &target)?;
        }
    }
    let grown = resident()?.saturating_sub(before);
    // A copy of the URI for each line took 512 MB; the links themselves take about 1 MB.
    assert!(grown < 16 << 20, "the links took {grown} bytes");
    let pdf = doc.to_bytes()?;
    // Each annotation about 90 bytes and its cross-reference entry 20, the URI once.
    assert!(pdf.len() < 4 << 20, "the PDF is {} bytes", pdf.len());
    let written = pdf
        .windows(uri.len())
        .filter(|window| *window == uri.as_bytes())
        .count();
    assert_eq!(written, 1);
    Ok(())
}

#[test]
fn links_to_many_targets_keep_each_once() -> TestResult {
    // 1000 links, each to a URI of its own of 32,000 bytes: 32 MB of URIs.
    let _measuring = MEASURING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let uris: Vec<String> = (0..1000)
        .map(|i| format!("https://example.com/{i:04}/{}", "a".repeat(32_000 - 25)))
        .collect();
    let mut doc = Document::new();
    let page = doc.add_page(PageSize::A4)?;
    let before = resident()?;
    for (i, uri) in uris.iter().enumerate() {
        let y = 800.0 - 0.5 * i as f64;
        page.add_link(
            Pt(72.0),
            Pt(y),
            Pt(400.0),
            Pt(0.5),
            &LinkTarget::Uri(uri.clone()),
        )?;
    }
    let grown = resident()?.saturating_sub(before);
    // Each URI kept once; a second copy of each would take 64 MB.
    assert!(grown < 48 << 20, "the links took {grown} bytes");
    Ok(())
}
