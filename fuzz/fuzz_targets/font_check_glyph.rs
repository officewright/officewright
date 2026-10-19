//! Checks glyphs of every face of the input for subsetting, one at a time, and compares
//! with subsetting them: a glyph checked first on a face passes exactly when a subset of
//! it alone succeeds, and the glyphs a face's checks pass subset together, unless they
//! take more together than one subset may.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_font::{Error, FontFace, GlyphId};

// libfuzzer-sys installs a panic hook that aborts, so a panic in the subsetter is a
// crash here even though `FontFace::subset` catches it in other builds.
fuzz_target!(|data: &[u8]| {
    let Ok(count) = FontFace::face_count(data) else {
        return;
    };
    for index in 0..count.min(2) {
        let Ok(font) = FontFace::from_bytes(data, index) else {
            continue;
        };
        // Glyphs by character, as text needs them, and the first glyphs by ID.
        let mut glyphs: Vec<GlyphId> = "Aé fi".chars().filter_map(|ch| font.glyph_id(ch)).collect();
        glyphs.extend((0..font.glyph_count().min(6)).map(GlyphId::new));
        glyphs.sort_unstable_by_key(|glyph| glyph.to_u16());
        glyphs.dedup();
        for &glyph in &glyphs {
            let Ok(fresh) = FontFace::from_bytes(data, index) else {
                continue;
            };
            let checked = fresh.check_subset_glyph(glyph);
            let subset = fresh.subset([glyph]);
            assert_eq!(
                checked.is_ok(),
                subset.is_ok(),
                "glyph {glyph}: checked {checked:?}, subset {:?}",
                subset.err()
            );
        }
        let passed: Vec<GlyphId> = glyphs
            .iter()
            .copied()
            .filter(|&glyph| font.check_subset_glyph(glyph).is_ok())
            .collect();
        // `.notdef` passes unless the face itself cannot be subset.
        if passed.is_empty() {
            continue;
        }
        match font.subset(passed.iter().copied()) {
            Ok(_) | Err(Error::TableOverLimit { .. }) => {}
            Err(err) => panic!("the glyphs {passed:?} passed, but their subset failed: {err}"),
        }
    }
});
