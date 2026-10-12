//! Real fonts from tests/fonts/ at the repository root. They keep their own licenses
//! (THIRD-PARTY-NOTICES.md), so this test is not in the published crate.
//!
//! The expected values were read with fontTools 4.66 (`TTFont`), an independent
//! implementation.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use officewright_font::{
    EmbeddingLicense, FontBook, FontFace, FontOrigin, FontRequest, FontResolver, GenericFamily,
    GlyphId, OutlineFormat, ResolutionKind,
};

fn fonts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fonts")
}

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn read(path: &str) -> TestResult<Vec<u8>> {
    Ok(std::fs::read(fonts_dir().join(path))?)
}

fn liberation_sans() -> TestResult<FontFace> {
    Ok(FontFace::from_bytes(
        read("liberation/LiberationSans-Regular.ttf")?,
        0,
    )?)
}

fn noto_sans() -> TestResult<FontFace> {
    Ok(FontFace::from_bytes(read("noto/NotoSans-Regular.otf")?, 0)?)
}

/// The text the subsets are made for: Latin, Cyrillic, a currency sign, a ligature.
const TEXT: &str = "Hello, Мир! 0123 € fi";

#[test]
fn reads_liberation_sans() -> TestResult {
    let font = liberation_sans()?;
    assert_eq!(font.outline_format(), Some(OutlineFormat::TrueType));
    assert_eq!(font.units_per_em(), 2048);
    assert_eq!(font.glyph_count(), 2620);
    let names = font.names();
    assert_eq!(names.family(), Some("Liberation Sans"));
    assert_eq!(names.subfamily(), Some("Regular"));
    assert_eq!(names.postscript_name(), Some("LiberationSans"));
    assert_eq!(names.families().collect::<Vec<_>>(), ["Liberation Sans"]);
    let metrics = font.metrics();
    let hhea = metrics.hhea();
    assert_eq!(
        (hhea.ascender(), hhea.descender(), hhea.line_gap()),
        (1854, -434, 67)
    );
    let typo = metrics.typo().unwrap();
    assert_eq!(
        (typo.ascender(), typo.descender(), typo.line_gap()),
        (1491, -431, 307)
    );
    let win = metrics.win().unwrap();
    assert_eq!((win.ascent(), win.descent()), (1854, 434));
    assert!(!metrics.use_typo_metrics());
    assert_eq!(
        (font.cap_height(), font.x_height()),
        (Some(1409), Some(1082))
    );
    let bbox = font.bounding_box();
    assert_eq!(
        (bbox.x_min(), bbox.y_min(), bbox.x_max(), bbox.y_max()),
        (-1114, -621, 2666, 2007)
    );
    assert_eq!(font.weight(), 400);
    assert_eq!(font.embedding().license(), EmbeddingLicense::Installable);
    // Glyph and advance of each character; 'A' is 1366/2048 em wide, as in Arial.
    for (ch, glyph, advance) in [
        ('A', 36, 1366),
        (' ', 3, 569),
        ('Ж', 967, 1891),
        ('€', 2088, 1139),
    ] {
        let id = font.glyph_id(ch).unwrap();
        assert_eq!(id, GlyphId::new(glyph), "{ch}");
        assert_eq!(font.advance_width(id), Some(advance), "{ch}");
    }
    Ok(())
}

#[test]
fn reads_noto_sans_with_cff_outlines() -> TestResult {
    let font = noto_sans()?;
    assert_eq!(font.outline_format(), Some(OutlineFormat::Cff));
    assert_eq!(font.units_per_em(), 1000);
    assert_eq!(font.glyph_count(), 3884);
    assert_eq!(font.names().family(), Some("Noto Sans"));
    assert_eq!(font.names().full_name(), Some("Noto Sans Regular"));
    assert_eq!(font.names().postscript_name(), Some("NotoSans-Regular"));
    let metrics = font.metrics();
    assert!(metrics.use_typo_metrics());
    let win = metrics.win().unwrap();
    assert_eq!((win.ascent(), win.descent()), (1124, 395));
    for (ch, glyph, advance) in [('A', 36, 639), ('Ж', 430, 905), ('€', 538, 572)] {
        let id = font.glyph_id(ch).unwrap();
        assert_eq!(id, GlyphId::new(glyph), "{ch}");
        assert_eq!(font.advance_width(id), Some(advance), "{ch}");
    }
    Ok(())
}

/// Subsets `font` for `TEXT` and checks the result against the original.
fn check_subset(font: &FontFace) -> TestResult {
    let glyphs: Vec<GlyphId> = TEXT.chars().filter_map(|ch| font.glyph_id(ch)).collect();
    let subset = font.subset(glyphs.iter().copied())?;
    assert_eq!(Some(subset.outline_format()), font.outline_format());
    assert!(
        subset.data().len() * 10 < font.data().len(),
        "subset of {} bytes from {}",
        subset.data().len(),
        font.data().len()
    );
    // Renumbered in the order of the original IDs, after .notdef, with the original
    // advances.
    let reread = FontFace::from_bytes(subset.data().to_vec(), 0)?;
    let mut sorted = glyphs.clone();
    sorted.sort();
    sorted.dedup();
    for (new, original) in sorted.iter().enumerate() {
        let new = GlyphId::new(new as u16 + 1);
        assert_eq!(subset.glyph_id(*original), Some(new));
        assert_eq!(reread.advance_width(new), font.advance_width(*original));
    }
    assert!(subset.glyph_count() as usize > sorted.len());
    Ok(())
}

#[test]
fn subsets_truetype_and_cff_fonts() -> TestResult {
    check_subset(&liberation_sans()?)?;
    check_subset(&noto_sans()?)
}

#[test]
fn reads_a_collection_of_real_fonts() -> TestResult {
    // A .ttc of two real fonts, built here rather than committed.
    let fonts = [
        read("liberation/LiberationSans-Regular.ttf")?,
        read("noto/NotoSans-Regular.otf")?,
    ];
    let mut ttc = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x02".to_vec();
    let mut start = 12 + 4 * fonts.len();
    for font in &fonts {
        ttc.extend_from_slice(&(start as u32).to_be_bytes());
        start += font.len();
    }
    for font in &fonts {
        let shift = ttc.len() as u32;
        let mut font = font.clone();
        let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
        for record in 0..tables {
            let at = 12 + 16 * record + 8;
            let offset = u32::from_be_bytes(font[at..at + 4].try_into().unwrap());
            font[at..at + 4].copy_from_slice(&(offset + shift).to_be_bytes());
        }
        ttc.extend_from_slice(&font);
    }
    let ttc: Arc<[u8]> = ttc.into();
    assert_eq!(FontFace::face_count(&ttc).unwrap(), 2);
    let noto = FontFace::from_bytes(Arc::clone(&ttc), 1).unwrap();
    assert_eq!(noto.names().family(), Some("Noto Sans"));
    check_subset(&noto)?;
    let mut book = FontBook::new();
    assert_eq!(book.add_bytes(ttc, FontOrigin::User)?, 2);
    Ok(())
}

#[test]
fn resolves_arial_to_liberation_sans() {
    let mut book = FontBook::new();
    assert_eq!(book.add_dir(fonts_dir()).unwrap(), 2);
    assert!(book.skipped().is_empty(), "{:?}", book.skipped());
    let mut resolver = FontResolver::new(&book);
    let arial = resolver.resolve(&FontRequest::new("Arial")).clone();
    assert_eq!(arial.kind(), ResolutionKind::MetricCompatible);
    let font = book.load(arial.face().unwrap()).unwrap();
    assert_eq!(font.names().family(), Some("Liberation Sans"));
    let georgia = resolver.resolve(&FontRequest::new("Georgia").generic(GenericFamily::Serif));
    // No serif font in tests/fonts, so nothing fits.
    assert_eq!(georgia.kind(), ResolutionKind::Missing);
    let noto = resolver.resolve(&FontRequest::new("noto sans"));
    assert_eq!(noto.kind(), ResolutionKind::Exact);
    assert_eq!(resolver.substitutions().len(), 2);
}
