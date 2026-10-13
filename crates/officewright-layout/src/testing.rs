//! Fonts for the unit tests: officewright-font's own test font, and copies of it with a
//! table changed.

use officewright_font::{FontBook, FontFace, FontOrigin};

/// officewright-font's test font "Officewright Test": 1000 units per em; glyphs 0
/// `.notdef`, 1 space, 2 `A`, 3 acute and 4 `Á`; `usWinAscent` 900, `usWinDescent` 250;
/// typographic and `hhea` ascender 800, descender -200, line gap 90; `USE_TYPO_METRICS`
/// not set.
pub(crate) fn minimal() -> Vec<u8> {
    include_bytes!("../../officewright-font/tests/data/minimal.ttf").to_vec()
}

/// Where table `tag` starts.
fn table(font: &[u8], tag: &[u8; 4]) -> usize {
    let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
    (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&record| &font[record..record + 4] == tag)
        .map(|record| {
            u32::from_be_bytes(font[record + 8..record + 12].try_into().unwrap()) as usize
        })
        .expect("the table")
}

/// `font` with the bytes at `offset` of table `tag` replaced by `bytes`.
pub(crate) fn patched(mut font: Vec<u8>, tag: &[u8; 4], offset: usize, bytes: &[u8]) -> Vec<u8> {
    let at = table(&font, tag) + offset;
    font[at..at + bytes.len()].copy_from_slice(bytes);
    font
}

/// `font` with `USE_TYPO_METRICS` (bit 7 of `fsSelection`) set.
pub(crate) fn with_typo_metrics(font: Vec<u8>) -> Vec<u8> {
    let at = table(&font, b"OS/2") + 62;
    let selection = u16::from_be_bytes([font[at], font[at + 1]]) | 0x80;
    patched(font, b"OS/2", 62, &selection.to_be_bytes())
}

/// `font` with the family name changed from "Officewright Test" to another of the same
/// length, so that a book can hold both.
pub(crate) fn renamed(font: Vec<u8>, family: &str) -> Vec<u8> {
    let from: Vec<u8> = "Officewright Test"
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    let to: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    assert_eq!(
        from.len(),
        to.len(),
        "the new family name must have 17 characters"
    );
    let mut font = font;
    let mut at = 0;
    while let Some(found) = font[at..]
        .windows(from.len())
        .position(|w| w == from.as_slice())
    {
        font[at + found..at + found + to.len()].copy_from_slice(&to);
        at += found + to.len();
    }
    font
}

pub(crate) fn face(font: Vec<u8>) -> FontFace {
    FontFace::from_bytes(font, 0).expect("the test font")
}

/// A book of the given fonts.
pub(crate) fn book(fonts: Vec<Vec<u8>>) -> FontBook {
    let mut book = FontBook::new();
    for font in fonts {
        book.add_bytes(font, FontOrigin::Document)
            .expect("the test font");
    }
    book
}
