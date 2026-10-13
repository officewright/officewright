//! Reads PNG files as `Image::from_bytes` does, and writes a PDF that draws them: the
//! chunk reader and its checksums, the decompression and its size checks, unfiltering,
//! Adam7 interlacing and the split into colour and alpha.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_pdf::{Document, Image, PageSize, Pt};

fuzz_target!(|data: &[u8]| {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return;
    }
    let Ok(image) = Image::from_bytes(data) else {
        return;
    };
    // Writing compresses the decoded pixels; large images only slow the fuzzer down.
    if u64::from(image.width()) * u64::from(image.height()) > 1 << 20 {
        return;
    }
    let mut doc = Document::new();
    let Ok(page) = doc.add_page(PageSize::A4) else {
        return;
    };
    if page
        .draw_image(&image, Pt(0.0), Pt(0.0), Pt(100.0), Pt(100.0))
        .is_ok()
    {
        let _ = doc.to_bytes();
    }
});
