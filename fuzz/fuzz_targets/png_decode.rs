//! Reads PNG files as `Image::from_bytes` does, and writes a PDF that draws them: the
//! chunk reader, the decompression and its size checks, unfiltering, Adam7 interlacing
//! and the split into colour and alpha.
//!
//! Each chunk's CRC is written anew before reading, so that a mutation of a chunk reaches
//! the code behind the CRC check; the unit tests check the CRC check itself.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_pdf::{Document, Image, PageSize, Pt};

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The CRC-32 of PNG chunks (PNG specification, annex D).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// `data` with the CRC of every whole chunk corrected.
fn with_crcs(data: &[u8]) -> Vec<u8> {
    let mut data = data.to_vec();
    let mut at = SIGNATURE.len();
    while let Some(length) = data
        .get(at..at + 4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_be_bytes)
        .and_then(|length| usize::try_from(length).ok())
    {
        let end = at.saturating_add(8).saturating_add(length);
        let Some(crc) = data.get(at + 4..end).map(crc32) else {
            break;
        };
        let Some(slot) = data.get_mut(end..end + 4) else {
            break;
        };
        slot.copy_from_slice(&crc.to_be_bytes());
        at = end + 4;
    }
    data
}

fuzz_target!(|data: &[u8]| {
    if !data.starts_with(SIGNATURE) {
        return;
    }
    let Ok(image) = Image::from_bytes(with_crcs(data)) else {
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
