//! Subsets every face of the input, as a PDF writer embedding it would.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_font::{FontFace, GlyphId};

// libfuzzer-sys installs a panic hook that aborts, so a panic in the subsetter is a
// crash here even though `FontFace::subset` catches it in other builds.
fuzz_target!(|data: &[u8]| {
    let Ok(count) = FontFace::face_count(data) else {
        return;
    };
    for index in 0..count.min(4) {
        let Ok(font) = FontFace::from_bytes(data, index) else {
            continue;
        };
        // Glyphs by character, as text needs them, and the first glyphs by ID.
        let mut glyphs: Vec<GlyphId> = "Hello, Мир! 0123 ÁÉ fi"
            .chars()
            .filter_map(|ch| font.glyph_id(ch))
            .collect();
        glyphs.extend((0..font.glyph_count().min(256)).map(GlyphId::new));
        if let Ok(subset) = font.subset(glyphs.iter().copied()) {
            for glyph in &glyphs {
                assert!(
                    subset.glyph_id(*glyph).is_some(),
                    "glyph {glyph} was dropped"
                );
            }
        }
    }
});
