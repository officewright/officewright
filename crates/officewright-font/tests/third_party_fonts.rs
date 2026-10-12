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
    // A CFF subset carries its font program as a CID-keyed CFF table: version 1.0
    // (CFF spec, 6), with the ROS operator (12 30) opening the top DICT (CFF spec, 18).
    match subset.outline_format() {
        OutlineFormat::Cff => {
            let cff = subset.cff_table().ok_or("a CFF subset has a CFF table")?;
            assert_eq!(cff.get(..2), Some(&[1, 0][..]));
            assert!(cff.windows(2).any(|w| w == [12, 30]), "no ROS operator");
        }
        _ => assert_eq!(subset.cff_table(), None),
    }
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
    assert_eq!(book.add_dir(fonts_dir()).unwrap(), 5);
    assert!(book.skipped().is_empty(), "{:?}", book.skipped());
    let mut resolver = FontResolver::new(&book);
    let arial = resolver.resolve(&FontRequest::new("Arial")).clone();
    assert_eq!(arial.kind(), ResolutionKind::MetricCompatible);
    let font = book.load(arial.face().unwrap()).unwrap();
    assert_eq!(font.names().family(), Some("Liberation Sans"));
    // Georgia has no metric-compatible substitute; the common serif font stands in.
    let georgia = resolver
        .resolve(&FontRequest::new("Georgia").generic(GenericFamily::Serif))
        .clone();
    assert_eq!(georgia.kind(), ResolutionKind::Fallback);
    assert_eq!(georgia.family(), Some("Liberation Serif"));
    let noto = resolver.resolve(&FontRequest::new("noto sans"));
    assert_eq!(noto.kind(), ResolutionKind::Exact);
    assert_eq!(resolver.substitutions().len(), 2);
}

#[test]
fn refuses_cff_reals_the_subsetter_cannot_write() -> TestResult {
    /// `data` with the bytes `from` replaced by `to`, which are as long.
    fn patch(mut data: Vec<u8>, from: &[u8], to: &[u8]) -> TestResult<Vec<u8>> {
        let at = data
            .windows(from.len())
            .position(|window| window == from)
            .ok_or("bytes not found")?;
        data.get_mut(at..at + to.len())
            .ok_or("short font")?
            .copy_from_slice(to);
        Ok(data)
    }
    // Noto Sans with the first real number of its Private DICT, 46875E-6 (bytes 1e 46
    // 87 5c 6f), replaced by 1E99999 (1e 1b 99 99 9f), which is infinite as an f32.
    // subsetter 0.2.6 panics writing it back; the face is refused before that.
    let infinite = patch(
        read("noto/NotoSans-Regular.otf")?,
        &[0x1e, 0x46, 0x87, 0x5c, 0x6f],
        &[0x1e, 0x1b, 0x99, 0x99, 0x9f],
    )?;
    // The same, with the Top DICT's "36 190208 Private" giving the offset as the real
    // number 190208 (1e 19 02 08 ff) instead of an integer (1d 00 02 e7 00), which the
    // subsetter reads as well.
    let real_offset = patch(
        infinite.clone(),
        &[0xaf, 0x1d, 0x00, 0x02, 0xe7, 0x00, 0x12],
        &[0xaf, 0x1e, 0x19, 0x02, 0x08, 0xff, 0x12],
    )?;
    for data in [infinite, real_offset] {
        let font = FontFace::from_bytes(data, 0)?;
        let err = font.subset([]).err().ok_or("the subset succeeded")?;
        assert!(
            matches!(
                err,
                officewright_font::Error::MalformedTable { table: "CFF ", .. }
            ),
            "{err}"
        );
        assert_eq!(
            format!(
                "{err}: {}",
                std::error::Error::source(&err).ok_or("no cause")?
            ),
            "font face 0: table 'CFF ' is damaged: the real number 1E99999 in a DICT is too \
             large to subset"
        );
    }
    Ok(())
}
