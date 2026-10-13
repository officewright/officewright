//! Shaping with the test fonts of the repository (`tests/fonts`, under their own licenses,
//! so this file is left out of the published crate). It reaches the shaper through the
//! `fuzzing` feature, which `cargo test --all-features` turns on.
#![cfg(feature = "fuzzing")]

use std::error::Error;

use officewright_font::FontFace;
use officewright_layout::fuzzing::{Shaped, check_shaping, shape};
use officewright_layout::ir::Ligatures;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn font(path: &str) -> TestResult<FontFace> {
    let path = format!("{}/../../tests/fonts/{path}", env!("CARGO_MANIFEST_DIR"));
    let data = std::fs::read(&path).map_err(|err| format!("{path}: {err}"))?;
    Ok(FontFace::from_bytes(data, 0)?)
}

fn carlito() -> TestResult<FontFace> {
    font("carlito/Carlito-Regular.ttf")
}

fn liberation_sans() -> TestResult<FontFace> {
    font("liberation/LiberationSans-Regular.ttf")
}

fn noto_sans() -> TestResult<FontFace> {
    font("noto/NotoSans-Regular.otf")
}

fn clusters(shaped: &Shaped) -> Vec<usize> {
    shaped
        .glyphs()
        .iter()
        .map(|glyph| glyph.cluster())
        .collect()
}

fn total(shaped: &Shaped) -> i64 {
    shaped
        .glyphs()
        .iter()
        .map(|glyph| i64::from(glyph.x_advance()))
        .sum()
}

/// The texts the `shape_font` fuzz target shapes, so the fonts here are known to pass.
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

#[test]
fn ligatures_join_letters_only_when_asked() -> TestResult {
    let face = carlito()?;
    let plain = shape(&face, "office", false, Ligatures::default(), None);
    assert_eq!(clusters(&plain), [0, 1, 2, 3, 4, 5]);

    let joined = shape(
        &face,
        "office",
        false,
        Ligatures::default().with_standard(true),
        None,
    );
    // Carlito joins "ffi" into one glyph, which stands for the bytes 1 to 3.
    assert_eq!(clusters(&joined), [0, 1, 4, 5]);
    assert!(!joined.is_nominal());
    // Contextual alternates alone do not join the letters.
    let contextual = shape(
        &face,
        "office",
        false,
        Ligatures::default().with_contextual(true),
        None,
    );
    assert_eq!(clusters(&contextual), [0, 1, 2, 3, 4, 5]);
    Ok(())
}

#[test]
fn kerning_applies_only_when_asked() -> TestResult {
    for face in [liberation_sans()?, noto_sans()?] {
        let plain = shape(&face, "AVATAR", false, Ligatures::default(), None);
        // Without kerning, every glyph moves the pen by its advance width.
        for glyph in plain.glyphs() {
            let advance = face.advance_width(glyph.id().into()).map(i32::from);
            assert_eq!(Some(glyph.x_advance()), advance);
        }
        let kerned = shape(&face, "AVATAR", true, Ligatures::default(), None);
        assert!(
            total(&kerned) < total(&plain),
            "{}",
            face.names().family().unwrap_or("?")
        );
        assert_eq!(clusters(&kerned), clusters(&plain));
    }
    Ok(())
}

#[test]
fn clusters_are_byte_offsets_in_the_text() -> TestResult {
    let shaped = shape(
        &liberation_sans()?,
        "Жук ёж",
        false,
        Ligatures::default(),
        None,
    );
    assert_eq!(clusters(&shaped), [0, 2, 4, 6, 7, 9]);
    assert!(shaped.glyphs().iter().all(|glyph| glyph.id() != 0));
    Ok(())
}

#[test]
fn a_letter_with_a_combining_accent_takes_the_composed_glyph() -> TestResult {
    let face = noto_sans()?;
    let composed = shape(&face, "\u{E9}", false, Ligatures::default(), None);
    let decomposed = shape(&face, "e\u{301}", false, Ligatures::default(), None);
    assert_eq!(decomposed.glyphs().len(), 1);
    assert_eq!(decomposed.glyphs()[0].id(), composed.glyphs()[0].id());
    assert_eq!(clusters(&decomposed), [0]);
    Ok(())
}

#[test]
fn a_character_the_face_lacks_takes_the_missing_glyph() -> TestResult {
    let shaped = shape(
        &liberation_sans()?,
        "a中b",
        false,
        Ligatures::default(),
        None,
    );
    let ids: Vec<u16> = shaped.glyphs().iter().map(|glyph| glyph.id()).collect();
    assert_eq!(ids.len(), 3);
    assert_eq!(ids[1], 0);
    assert!(ids[0] != 0 && ids[2] != 0);
    assert_eq!(clusters(&shaped), [0, 1, 4]);
    assert_eq!(shaped.replaced_glyphs(), 0);
    Ok(())
}

#[test]
fn every_test_font_keeps_what_the_layout_relies_on() -> TestResult {
    for face in [carlito()?, liberation_sans()?, noto_sans()?] {
        check_shaping(&face, TEXTS)
            .map_err(|problem| format!("{}: {problem}", face.names().family().unwrap_or("?")))?;
    }
    Ok(())
}
