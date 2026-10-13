//! Reads the input as a font file and shapes a fixed set of texts with each face, as the
//! layout does, with kerning and ligatures off and on. The texts reach the lookups of the
//! `GSUB`, `GPOS` and `GDEF` tables for Latin, Cyrillic, Greek, combining marks, Arabic,
//! Devanagari and Han, so the mutations of a seed font land in tables nothing else reads.
//! The library catches no panic here, so one in HarfRust or read-fonts surfaces.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use officewright_font::FontFace;
use officewright_layout::fuzzing::check_shaping;

const TEXTS: &[&str] = &[
    "office AVATAR Wave To fi fl ffi",
    "Съешь же ещё этих мягких французских булок",
    "Ξεσκεπάζω την ψυχοφθόρα βδελυγμία",
    "e\u{301} a\u{308}\u{323} n\u{303}",
    "سلام علیکم",
    "नमस्ते दुनिया",
    "中文字体 日本語",
    "👍🏽 \u{200D} \u{FE0F}",
];

fuzz_target!(|data: &[u8]| {
    let data: Arc<[u8]> = data.into();
    let Ok(count) = FontFace::face_count(&data) else {
        return;
    };
    for index in 0..count.min(2) {
        let Ok(face) = FontFace::from_bytes(Arc::clone(&data), index) else {
            continue;
        };
        if let Err(problem) = check_shaping(&face, TEXTS) {
            panic!("face {index}: {problem}");
        }
    }
});
