//! Reads JPEG files as `Image::from_bytes` does: the markers, the segments and their
//! lengths, the frame header, Adobe's APP14 segment and every scan header with its
//! components and, in a progressive frame, the coefficients and bits it codes, walking
//! the coded data between the scans; then writes a PDF that draws them, cut after the
//! only scan of a sequential file where what follows is left out.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_pdf::{Document, Image, PageSize, Pt};

fuzz_target!(|data: &[u8]| {
    if !data.starts_with(b"\xFF\xD8") {
        return;
    }
    let Ok(image) = Image::from_bytes(data) else {
        return;
    };
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
