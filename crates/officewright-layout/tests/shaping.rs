//! Shaping with the test fonts of the repository (`tests/fonts`, under their own licenses,
//! so this file is left out of the published crate). It reaches the shaper through the
//! `fuzzing` feature, which `cargo test --all-features` turns on.
#![cfg(feature = "fuzzing")]

use std::error::Error;

use officewright_font::FontFace;
use officewright_layout::fuzzing::{Settings, Shaped, check_shaping, shape};
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

fn ids(shaped: &Shaped) -> Vec<u16> {
    shaped.glyphs().iter().map(|glyph| glyph.id()).collect()
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

fn with_ligatures(change: fn(Ligatures) -> Ligatures) -> Settings {
    Settings::new().with_ligatures(change(Ligatures::default()))
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
fn standard_ligatures_join_letters_only_when_asked() -> TestResult {
    let face = carlito()?;
    let plain = shape(&face, "office", &Settings::new());
    assert_eq!(clusters(&plain), [0, 1, 2, 3, 4, 5]);

    let joined = shape(&face, "office", &with_ligatures(|l| l.with_standard(true)));
    // Carlito joins "ffi" into one glyph, which stands for the bytes 1 to 3.
    assert_eq!(clusters(&joined), [0, 1, 4, 5]);
    assert!(!joined.is_nominal());
    // Its "ffi" is a standard ligature (`liga`), not a contextual one (`clig`).
    let contextual = shape(
        &face,
        "office",
        &with_ligatures(|l| l.with_contextual(true)),
    );
    assert_eq!(clusters(&contextual), [0, 1, 2, 3, 4, 5]);
    Ok(())
}

#[test]
fn discretionary_ligatures_join_letters_only_when_asked() -> TestResult {
    let face = carlito()?;
    let plain = shape(&face, "st ct", &Settings::new());
    assert_eq!(clusters(&plain), [0, 1, 2, 3, 4]);
    // Carlito has discretionary ligatures for "st" and "ct".
    let joined = shape(
        &face,
        "st ct",
        &with_ligatures(|l| l.with_discretional(true)),
    );
    assert_eq!(clusters(&joined), [0, 2, 3]);
    Ok(())
}

#[test]
fn kerning_applies_only_when_asked() -> TestResult {
    for face in [liberation_sans()?, noto_sans()?] {
        let family = face.names().family().unwrap_or("?").to_owned();
        let plain = shape(&face, "AVATAR", &Settings::new());
        // Without kerning, every glyph moves the pen by its advance width, and the text
        // may break anywhere without being shaped again.
        for glyph in plain.glyphs() {
            let advance = face.advance_width(glyph.id().into()).map(i32::from);
            assert_eq!(Some(glyph.x_advance()), advance, "{family}");
            assert!(!glyph.unsafe_to_break(), "{family}");
        }
        let kerned = shape(&face, "AVATAR", &Settings::new().with_kerning(true));
        assert!(total(&kerned) < total(&plain), "{family}");
        assert_eq!(clusters(&kerned), clusters(&plain));
    }
    // With kerning, a break between two kerned letters needs the run shaped again.
    let kerned = shape(
        &liberation_sans()?,
        "AVATAR",
        &Settings::new().with_kerning(true),
    );
    let flags: Vec<bool> = kerned
        .glyphs()
        .iter()
        .map(|glyph| glyph.unsafe_to_break())
        .collect();
    assert_eq!(flags, [false, true, true, true, true, false]);
    Ok(())
}

#[test]
fn clusters_are_byte_offsets_in_the_text() -> TestResult {
    let shaped = shape(&liberation_sans()?, "Жук ёж", &Settings::new());
    assert_eq!(clusters(&shaped), [0, 2, 4, 6, 7, 9]);
    assert!(ids(&shaped).iter().all(|id| *id != 0));
    Ok(())
}

#[test]
fn a_letter_with_a_combining_accent_takes_the_composed_glyph() -> TestResult {
    let face = noto_sans()?;
    let composed = shape(&face, "\u{E9}", &Settings::new());
    let decomposed = shape(&face, "e\u{301}", &Settings::new());
    assert_eq!(ids(&decomposed), ids(&composed));
    assert_eq!(clusters(&decomposed), [0]);
    Ok(())
}

#[test]
fn an_accent_without_a_composed_glyph_is_placed_over_its_letter() -> TestResult {
    // Liberation Sans has no "x" with an acute: the accent is a glyph of its own, in the
    // letter's cluster, that takes no room and sits where the font's marks put it.
    let shaped = shape(&liberation_sans()?, "x\u{301}", &Settings::new());
    let glyphs = shaped.glyphs();
    assert_eq!(clusters(&shaped), [0, 0]);
    let accent = glyphs.get(1).ok_or("no accent glyph")?;
    assert_eq!(accent.x_advance(), 0);
    assert_eq!((accent.x_offset(), accent.y_offset()), (-249, -340));
    Ok(())
}

#[test]
fn the_language_chooses_the_forms_of_letters() -> TestResult {
    let face = noto_sans()?;
    // Romanian writes ş and ţ with a comma below, which Noto Sans has as `locl` forms
    // for Romanian in the Latin script.
    let plain = shape(&face, "\u{15F}\u{163}", &Settings::new());
    let romanian = shape(
        &face,
        "\u{15F}\u{163}",
        &Settings::new().with_language("ro"),
    );
    assert_ne!(ids(&plain), ids(&romanian));
    let latin = shape(
        &face,
        "\u{15F}\u{163}",
        &Settings::new().with_language("ro").with_script("Latn"),
    );
    assert_eq!(ids(&latin), ids(&romanian));
    // In a script whose language systems lack Romanian, the forms stay the plain ones.
    let cyrillic = shape(
        &face,
        "\u{15F}\u{163}",
        &Settings::new().with_language("ro").with_script("Cyrl"),
    );
    assert_eq!(ids(&cyrillic), ids(&plain));
    Ok(())
}

#[test]
fn a_character_the_face_lacks_takes_the_missing_glyph() -> TestResult {
    let shaped = shape(&liberation_sans()?, "a中b", &Settings::new());
    let ids = ids(&shaped);
    assert_eq!(ids.len(), 3);
    assert_eq!(ids.get(1), Some(&0));
    assert!(ids.first() != Some(&0) && ids.get(2) != Some(&0));
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
