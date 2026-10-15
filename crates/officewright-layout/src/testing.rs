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

/// `font` with a new `name` table that names the family `family`, appended at the end of
/// the file; the old table stays, unused.
pub(crate) fn with_family(font: Vec<u8>, family: &str) -> Vec<u8> {
    let names = [
        (1u16, family.to_owned()),
        (2, "Regular".to_owned()),
        (4, family.to_owned()),
        (6, family.replace(' ', "")),
    ];
    let mut strings = Vec::new();
    let mut records = Vec::new();
    for (id, text) in &names {
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        for value in [
            3u16,
            1,
            0x409,
            *id,
            bytes.len() as u16,
            strings.len() as u16,
        ] {
            records.extend_from_slice(&value.to_be_bytes());
        }
        strings.extend_from_slice(&bytes);
    }
    let mut table = Vec::new();
    for value in [0u16, names.len() as u16, (6 + records.len()) as u16] {
        table.extend_from_slice(&value.to_be_bytes());
    }
    table.extend_from_slice(&records);
    table.extend_from_slice(&strings);
    let mut font = font;
    while font.len() % 4 != 0 {
        font.push(0);
    }
    let offset = font.len() as u32;
    font.extend_from_slice(&table);
    let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
    let record = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&record| &font[record..record + 4] == b"name")
        .expect("a name table");
    font[record + 8..record + 12].copy_from_slice(&offset.to_be_bytes());
    font[record + 12..record + 16].copy_from_slice(&(table.len() as u32).to_be_bytes());
    font
}

/// `font` (the test font) with its glyph of `A` mapped from `.` instead, in both
/// subtables of its `cmap`, which share one format 4 table: a font with a period and no
/// `A`.
pub(crate) fn with_period(font: Vec<u8>) -> Vec<u8> {
    let cmap = table(&font, b"cmap");
    let subtable =
        cmap + u32::from_be_bytes(font[cmap + 8..cmap + 12].try_into().unwrap()) as usize;
    let segments_x2 = usize::from(u16::from_be_bytes([font[subtable + 6], font[subtable + 7]]));
    // The second segment is `A` alone, to glyph 2 by its delta.
    let end = subtable + 14 + 2;
    let start = subtable + 16 + segments_x2 + 2;
    let delta = subtable + 16 + 2 * segments_x2 + 2;
    assert_eq!(
        &font[start..start + 2],
        &0x41u16.to_be_bytes(),
        "the segment of A"
    );
    let mut font = font;
    font[end..end + 2].copy_from_slice(&0x2Eu16.to_be_bytes());
    font[start..start + 2].copy_from_slice(&0x2Eu16.to_be_bytes());
    font[delta..delta + 2].copy_from_slice(&(2i16 - 0x2E).to_be_bytes());
    font
}

/// `font` marked italic in `OS/2` `fsSelection` and `head` `macStyle`.
pub(crate) fn italic(font: Vec<u8>) -> Vec<u8> {
    let at = table(&font, b"OS/2") + 62;
    let selection = (u16::from_be_bytes([font[at], font[at + 1]]) | 1) & !0x40;
    let font = patched(font, b"OS/2", 62, &selection.to_be_bytes());
    let at = table(&font, b"head") + 44;
    let style = u16::from_be_bytes([font[at], font[at + 1]]) | 2;
    patched(font, b"head", 44, &style.to_be_bytes())
}

/// `font` with a new `name` table whose family is "Test", followed by `copies` records
/// of 32768 bytes each that all point at one run of strings: from 32 copies on, the
/// strings the records to keep point at pass the 1 MiB that subsetting reads, and
/// `officewright-pdf` cannot embed the font.
pub(crate) fn with_name_copies(font: Vec<u8>, copies: u16) -> Vec<u8> {
    let mut font = font;
    font.resize(font.len().next_multiple_of(4), 0);
    let count = copies + 1;
    let mut name: Vec<u8> = [0u16, count, 6 + 12 * count]
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect();
    let mut record = |id: u16, length: u16, offset: u16| {
        for value in [3, 1, 0x0409, id, length, offset] {
            name.extend(u16::to_be_bytes(value));
        }
    };
    record(1, 8, 0);
    for _ in 0..copies {
        record(0, 32_768, 8);
    }
    name.extend("Test".encode_utf16().flat_map(u16::to_be_bytes));
    name.extend([0, b'A'].repeat(16_384));
    let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
    let entry = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&entry| &font[entry..entry + 4] == b"name")
        .expect("a name table");
    let offset = font.len() as u32;
    font[entry + 8..entry + 12].copy_from_slice(&offset.to_be_bytes());
    font[entry + 12..entry + 16].copy_from_slice(&(name.len() as u32).to_be_bytes());
    font.extend(name);
    font
}
