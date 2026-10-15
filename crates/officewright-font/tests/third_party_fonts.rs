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
fn keeps_every_glyph_of_a_real_cff_font() -> TestResult {
    // As a font that forbids subsetting is kept. Decompiling Noto Sans whole reads
    // 271,888 operands, operators and hint mask bytes, far within the limit on that work.
    let font = noto_sans()?;
    let subset = font.subset((0..font.glyph_count()).map(GlyphId::new))?;
    assert_eq!(subset.glyph_count(), font.glyph_count());
    Ok(())
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
    // Carlito and Liberation Sans, Serif and Mono in four styles each, and Noto Sans.
    assert_eq!(book.add_dir(fonts_dir()).unwrap(), 17);
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

/// Bold and italic requests for the fonts the corpus uses find the twins' own styles, as
/// Word finds the installed families, instead of synthesizing them from the regular face.
#[test]
fn bold_and_italic_resolve_to_the_twins_own_styles() -> TestResult {
    let mut book = FontBook::new();
    book.add_dir(fonts_dir())?;
    let mut resolver = FontResolver::new(&book);
    for (family, twin) in [
        ("Calibri", "Carlito"),
        ("Arial", "Liberation Sans"),
        ("Times New Roman", "Liberation Serif"),
        ("Courier New", "Liberation Mono"),
    ] {
        for (bold, italic, style) in [
            (false, false, "Regular"),
            (true, false, "Bold"),
            (false, true, "Italic"),
            (true, true, "Bold Italic"),
        ] {
            let request = FontRequest::new(family).bold(bold).italic(italic);
            let resolution = resolver.resolve(&request).clone();
            let what = format!("{family} {style}: {resolution:?}");
            assert_eq!(
                resolution.kind(),
                ResolutionKind::MetricCompatible,
                "{what}"
            );
            assert!(
                !resolution.synthetic_bold() && !resolution.synthetic_italic(),
                "{what}"
            );
            let face = book.load(resolution.face().ok_or("no face")?)?;
            assert_eq!(face.names().family(), Some(twin), "{what}");
            assert_eq!(face.names().subfamily(), Some(style), "{what}");
        }
    }
    Ok(())
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

/// `font` with the directory record of `tag` renamed `new_tag` and pointed at `table`,
/// appended to the end of the file.
fn with_record(
    mut font: Vec<u8>,
    tag: &[u8; 4],
    new_tag: &[u8; 4],
    table: &[u8],
) -> TestResult<Vec<u8>> {
    let tables = usize::from(u16::from_be_bytes(
        font.get(4..6).ok_or("short")?.try_into()?,
    ));
    let record = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&at| font.get(at..at + 4) == Some(&tag[..]))
        .ok_or("no such table")?;
    font.resize(font.len().next_multiple_of(4), 0);
    let offset = u32::try_from(font.len())?;
    let length = u32::try_from(table.len())?;
    font.extend_from_slice(table);
    let checksum = font.get(record + 4..record + 8).ok_or("short directory")?;
    let entry = [
        &new_tag[..],
        checksum,
        &offset.to_be_bytes(),
        &length.to_be_bytes(),
    ]
    .concat();
    font.get_mut(record..record + 16)
        .ok_or("short directory")?
        .copy_from_slice(&entry);
    Ok(font)
}

#[test]
fn checks_the_cff_table_the_subsetter_takes() -> TestResult {
    // Noto Sans with a second `CFF ` record: its `GDEF` record, renamed, after the
    // first. The subsetter finds a table by bisection, which here takes the second;
    // the font parser takes the first. One of the two tables has the real number
    // 1E99999 the subsetter cannot write. In either order the subsetter never meets
    // it: exactly one order is refused, before the subsetter runs, and the other
    // subsets.
    let noto = read("noto/NotoSans-Regular.otf")?;
    let clean = table(&noto, b"CFF ")?.to_vec();
    let at = clean
        .windows(5)
        .position(|window| window == [0x1e, 0x46, 0x87, 0x5c, 0x6f])
        .ok_or("no real number")?;
    let mut infinite = clean.clone();
    infinite
        .get_mut(at..at + 5)
        .ok_or("short table")?
        .copy_from_slice(&[0x1e, 0x1b, 0x99, 0x99, 0x9f]);
    let mut refused = 0;
    for (first, second) in [(&infinite, &clean), (&clean, &infinite)] {
        let mut data = with_record(noto.clone(), b"GDEF", b"CFF ", second)?;
        let cff = data
            .windows(clean.len())
            .position(|window| window == clean.as_slice())
            .ok_or("no CFF table")?;
        data.get_mut(cff..cff + first.len())
            .ok_or("short font")?
            .copy_from_slice(first);
        match FontFace::from_bytes(data, 0)?.subset([]) {
            Ok(_) => {}
            Err(officewright_font::Error::MalformedTable { table: "CFF ", .. }) => refused += 1,
            Err(err) => return Err(format!("{err}").into()),
        }
    }
    assert_eq!(refused, 1);
    Ok(())
}

#[test]
fn checks_cff_reals_where_the_subsetter_takes_cff_for_truetype() -> TestResult {
    // Noto Sans with the real number 1E99999 in its CFF table, and its GDEF and GPOS
    // records renamed `glyf` and `loca`. The directory is no longer sorted: the font
    // parser looks tables up one by one, finds both and takes the outlines for TrueType,
    // while the subsetter's bisection misses `glyf` and takes the CFF table. The CFF
    // check runs whatever the parser says, so the face is refused before the subsetter
    // meets the number.
    let mut data = read("noto/NotoSans-Regular.otf")?;
    let at = data
        .windows(5)
        .position(|window| window == [0x1e, 0x46, 0x87, 0x5c, 0x6f])
        .ok_or("no real number")?;
    data.get_mut(at..at + 5)
        .ok_or("short font")?
        .copy_from_slice(&[0x1e, 0x1b, 0x99, 0x99, 0x9f]);
    let tables = usize::from(u16::from_be_bytes(
        data.get(4..6).ok_or("short")?.try_into()?,
    ));
    for (tag, new_tag) in [(b"GDEF", b"glyf"), (b"GPOS", b"loca")] {
        let record = (0..tables)
            .map(|i| 12 + 16 * i)
            .find(|&at| data.get(at..at + 4) == Some(&tag[..]))
            .ok_or("no such table")?;
        data.get_mut(record..record + 4)
            .ok_or("short directory")?
            .copy_from_slice(new_tag);
    }
    let font = FontFace::from_bytes(data, 0)?;
    assert_eq!(font.outline_format(), Some(OutlineFormat::TrueType));
    let err = font.subset([]).err().ok_or("the subset succeeded")?;
    assert!(
        matches!(
            err,
            officewright_font::Error::MalformedTable { table: "CFF ", .. }
        ),
        "{err}"
    );
    Ok(())
}

#[test]
fn refuses_name_tables_the_subsetter_cannot_write_in_cff_fonts_too() -> TestResult {
    // Noto Sans with a `name` table whose one record ends 65538 bytes into the strings.
    let mut name = Vec::new();
    for value in [0u16, 1, 18, 3, 1, 0x0409, 1, 4, 0xFFFE] {
        name.extend_from_slice(&value.to_be_bytes());
    }
    name.resize(18 + 0x1_0002, 0);
    let data = with_record(read("noto/NotoSans-Regular.otf")?, b"name", b"name", &name)?;
    let err = FontFace::from_bytes(data, 0)?
        .subset([])
        .err()
        .ok_or("the subset succeeded")?;
    assert!(
        matches!(
            err,
            officewright_font::Error::MalformedTable { table: "name", .. }
        ),
        "{err}"
    );
    Ok(())
}

#[test]
fn finds_the_faces_that_cover_a_character() -> TestResult {
    let mut book = FontBook::new();
    book.add_dir(fonts_dir())?;
    let families = |ch: char| -> Vec<String> {
        book.faces_covering(ch)
            .into_iter()
            .filter_map(|id| book.face(id)?.names().family().map(str::to_owned))
            .collect()
    };
    let latin = families('A');
    for family in ["Liberation Sans", "Carlito"] {
        assert!(
            latin.iter().any(|name| name == family),
            "{family}: {latin:?}"
        );
    }
    assert!(families('Ж').iter().any(|name| name == "Liberation Sans"));
    assert!(families('\u{4E00}').is_empty());
    Ok(())
}

#[test]
fn coverage_agrees_with_the_character_map() -> TestResult {
    let mut book = FontBook::new();
    book.add_dir(fonts_dir())?;
    assert!(book.len() >= 5);
    // Every character of the Basic Multilingual Plane and a few beyond, for every face.
    let chars: Vec<char> = (0..=0xFFFF_u32)
        .chain([0x1_F600, 0x2_0000, 0x10_FFFF])
        .filter_map(char::from_u32)
        .collect();
    for (id, info) in book.faces() {
        let face = book.load(id)?;
        let map = face.char_map();
        let mut mapped = 0;
        for &ch in &chars {
            let covered = book.faces_covering(ch).contains(&id);
            assert_eq!(
                covered,
                map.map(ch).is_some(),
                "{:?} {ch:?}",
                info.names().family()
            );
            mapped += usize::from(covered);
        }
        assert!(mapped > 200, "{:?} maps {mapped}", info.names().family());
    }
    Ok(())
}

/// A table of a font file, found through its table directory.
fn table<'a>(font: &'a [u8], tag: &[u8; 4]) -> TestResult<&'a [u8]> {
    let u16_at = |at: usize| -> TestResult<usize> {
        Ok(usize::from(u16::from_be_bytes(
            font.get(at..at + 2).ok_or("short")?.try_into()?,
        )))
    };
    let u32_at = |at: usize| -> TestResult<usize> {
        Ok(usize::try_from(u32::from_be_bytes(
            font.get(at..at + 4).ok_or("short")?.try_into()?,
        ))?)
    };
    for record in (0..u16_at(4)?).map(|i| 12 + 16 * i) {
        if font.get(record..record + 4) == Some(&tag[..]) {
            let (offset, len) = (u32_at(record + 8)?, u32_at(record + 12)?);
            return Ok(font.get(offset..offset + len).ok_or("short table")?);
        }
    }
    Err(format!("no {} table", String::from_utf8_lossy(tag)).into())
}

#[test]
fn reads_underline_and_strikeout_as_the_tables_store_them() -> TestResult {
    let i16_at = |table: &[u8], at: usize| -> TestResult<i16> {
        Ok(i16::from_be_bytes(
            table.get(at..at + 2).ok_or("short")?.try_into()?,
        ))
    };
    for path in [
        "liberation/LiberationSans-Regular.ttf",
        "liberation/LiberationSerif-Regular.ttf",
        "liberation/LiberationMono-Regular.ttf",
        "carlito/Carlito-Regular.ttf",
        "noto/NotoSans-Regular.otf",
    ] {
        let data = read(path)?;
        let face = FontFace::from_bytes(data.clone(), 0)?;
        // post: underlinePosition at byte 8, underlineThickness at 10; OS/2:
        // yStrikeoutSize at 26, yStrikeoutPosition at 28.
        let post = table(&data, b"post")?;
        let os2 = table(&data, b"OS/2")?;
        let underline = face.underline_metrics().ok_or("no underline")?;
        assert_eq!(
            (underline.position(), underline.thickness()),
            (i16_at(post, 8)?, i16_at(post, 10)?),
            "{path}"
        );
        let strikeout = face.strikeout_metrics().ok_or("no strikeout")?;
        assert_eq!(
            (strikeout.position(), strikeout.thickness()),
            (i16_at(os2, 28)?, i16_at(os2, 26)?),
            "{path}"
        );
        // Real fonts put the underline below the baseline and the strikeout above it.
        assert!(
            underline.position() < 0 && strikeout.position() > 0,
            "{path}"
        );
    }
    Ok(())
}
