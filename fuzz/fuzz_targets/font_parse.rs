//! Reads the input as a font file the way the library does: the font index first, with
//! the coverage of each face it indexed, then every face, with all of its metrics and its
//! character map.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use officewright_font::{FontBook, FontFace, FontOrigin, FontRequest, GlyphId};

/// Characters from every plane a character map can cover, and a symbol font's.
const CHARS: [char; 8] = [
    '\0',
    ' ',
    'A',
    'Ж',
    '中',
    '\u{F041}',
    '\u{1F600}',
    char::MAX,
];

fuzz_target!(|data: &[u8]| {
    let data: Arc<[u8]> = data.into();

    // The index reads only the table directory and a few tables, through its own path.
    let mut book = FontBook::new();
    if book
        .add_bytes(Arc::clone(&data), FontOrigin::Document)
        .is_ok()
    {
        // The coverage reads each face's cmap through a path of its own. A face it says
        // covers a character must map it.
        for ch in CHARS {
            let covering = book.faces_covering(ch);
            for &id in covering.iter().take(4) {
                if let Ok(font) = book.load(id) {
                    assert!(font.glyph_id(ch).is_some(), "{ch:?} covered but not mapped");
                }
            }
        }
        let family = book
            .faces()
            .find_map(|(_, face)| face.names().family().map(str::to_owned));
        if let Some(family) = family {
            let resolution = book.resolve(&FontRequest::new(family).bold(true));
            if let Some(id) = resolution.face() {
                let _ = book.load(id);
            }
        }
    }

    let Ok(count) = FontFace::face_count(&data) else {
        return;
    };
    for index in 0..count.min(4) {
        let Ok(font) = FontFace::from_bytes(Arc::clone(&data), index) else {
            continue;
        };
        let glyph_count = font.glyph_count();
        assert!(glyph_count > 0);
        for glyph in 0..glyph_count {
            assert!(font.advance_width(GlyphId::new(glyph)).is_some());
        }
        let map = font.char_map();
        for ch in CHARS {
            if let Some(glyph) = map.map(ch) {
                assert!(
                    glyph.to_u16() < glyph_count,
                    "{ch:?} maps past the last glyph"
                );
            }
        }
        let _ = (
            font.names(),
            font.metrics(),
            font.embedding(),
            font.bounding_box(),
        );
    }
});
