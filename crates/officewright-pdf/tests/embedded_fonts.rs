//! Embedding real fonts: the OFL fonts in tests/fonts/ at the repository root, which keep
//! their own licenses (THIRD-PARTY-NOTICES.md), and officewright-font's minimal test
//! font. Neither is part of this crate, so this test is not in the published crate.
//!
//! The PDF files themselves are checked by external tools in tests/pdf-output.

use std::path::{Path, PathBuf};

use officewright_font::{FontBook, FontFace, FontRequest, FontResolver, ResolutionKind};
use officewright_pdf::{
    Compression, Document, EmbeddingNote, Error, Font, PageSize, Pt, StandardFont, text_to_pdf,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> TestResult<Vec<u8>> {
    Ok(std::fs::read(repository().join(path))?)
}

fn liberation_sans() -> TestResult<Vec<u8>> {
    read("tests/fonts/liberation/LiberationSans-Regular.ttf")
}

fn noto_sans() -> TestResult<Vec<u8>> {
    read("tests/fonts/noto/NotoSans-Regular.otf")
}

/// officewright-font's test font: 1000 units per em, space (glyph 1, 250 units wide),
/// `A` (glyph 2, 600), acute (glyph 3) and `Á` (glyph 4).
fn minimal() -> TestResult<Vec<u8>> {
    read("crates/officewright-font/tests/data/minimal.ttf")
}

/// A stream of a PDF file.
struct Stream {
    object: usize,
    dictionary: String,
    /// Where its data is in the file.
    range: std::ops::Range<usize>,
    /// The data, decompressed if it is Flate-compressed.
    data: Vec<u8>,
}

/// The streams of a PDF file this crate wrote.
fn streams(pdf: &[u8]) -> TestResult<Vec<Stream>> {
    let mut streams = Vec::new();
    let mut at = 0;
    while let Some(found) = pdf
        .get(at..)
        .and_then(|rest| rest.windows(8).position(|w| w == b"\nstream\n"))
    {
        let head = String::from_utf8_lossy(pdf.get(..at + found).ok_or("bad offset")?);
        let (before, dictionary) = head.rsplit_once(" 0 obj\n").ok_or("no object")?;
        let object = before
            .rsplit(|c: char| !c.is_ascii_digit())
            .next()
            .ok_or("no object number")?
            .parse()?;
        let (_, length) = dictionary.split_once("/Length ").ok_or("no /Length")?;
        let length: usize = length
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .ok_or("no length")?
            .parse()?;
        let start = at + found + 8;
        let data = pdf.get(start..start + length).ok_or("short stream")?;
        let data = if dictionary.contains("/Filter /FlateDecode") {
            miniz_oxide::inflate::decompress_to_vec_zlib(data).map_err(|err| format!("{err:?}"))?
        } else {
            data.to_vec()
        };
        streams.push(Stream {
            object,
            dictionary: dictionary.to_owned(),
            range: start..start + length,
            data,
        });
        at = start + length;
    }
    Ok(streams)
}

/// The PDF file of `doc` as text, with the data of every stream decompressed, so that
/// page content and ToUnicode maps can be read.
fn pdf_text(doc: &Document) -> TestResult<String> {
    let pdf = doc.to_bytes()?;
    let mut text = Vec::new();
    let mut at = 0;
    for stream in streams(&pdf)? {
        text.extend_from_slice(pdf.get(at..stream.range.start).ok_or("bad offset")?);
        text.extend_from_slice(&stream.data);
        at = stream.range.end;
    }
    text.extend_from_slice(pdf.get(at..).ok_or("bad offset")?);
    Ok(String::from_utf8_lossy(&text).into_owned())
}

fn one_line(font: &Font, text: &str) -> TestResult<Document> {
    let mut doc = Document::new();
    doc.add_page(PageSize::A4)?
        .add_text(Pt(72.0), Pt(700.0), font, Pt(12.0), text)?;
    Ok(doc)
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// Where table `tag` starts in a single font file.
fn table(font: &[u8], tag: &[u8; 4]) -> Option<usize> {
    let tables = usize::from(u16_at(font, 4)?);
    let record = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&record| font.get(record..record + 4) == Some(&tag[..]))?;
    usize::try_from(u32_at(font, record + 8)?).ok()
}

/// The font with `fsType` in its `OS/2` table set to `fs_type`.
fn with_fs_type(mut font: Vec<u8>, fs_type: u16) -> TestResult<Vec<u8>> {
    let os2 = table(&font, b"OS/2").ok_or("no OS/2 table")?;
    // Bits 8 and 9 count from version 2 of the table.
    assert!(u16_at(&font, os2).ok_or("short OS/2 table")? >= 2);
    font.get_mut(os2 + 8..os2 + 10)
        .ok_or("short OS/2 table")?
        .copy_from_slice(&fs_type.to_be_bytes());
    Ok(font)
}

/// The font with a `name` table whose only name is `postscript` as its PostScript name
/// (name ID 6), added at the end of the file in place of its own.
fn with_postscript_name(mut font: Vec<u8>, postscript: &str) -> TestResult<Vec<u8>> {
    let tables = usize::from(u16_at(&font, 4).ok_or("short font")?);
    let record = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&record| font.get(record..record + 4) == Some(&b"name"[..]))
        .ok_or("no name table")?;
    let text: Vec<u8> = postscript
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    // Format 0, one record: Windows, Unicode BMP, US English, name ID 6, at offset 0 of
    // the strings, which start after the 6-byte header and the 12-byte record.
    let mut name = Vec::new();
    for value in [0, 1, 18, 3, 1, 0x0409, 6, u16::try_from(text.len())?, 0] {
        name.extend_from_slice(&u16::to_be_bytes(value));
    }
    name.extend_from_slice(&text);
    font.resize(font.len().next_multiple_of(4), 0);
    let offset = u32::try_from(font.len())?;
    let length = u32::try_from(name.len())?;
    font.extend_from_slice(&name);
    font.get_mut(record + 8..record + 16)
        .ok_or("short table directory")?
        .copy_from_slice(&[offset.to_be_bytes(), length.to_be_bytes()].concat());
    Ok(font)
}

/// The names of fonts in `pdf`, as written after `/BaseFont` and `/FontName`, without
/// their slash.
fn font_names(pdf: &str) -> Vec<&str> {
    pdf.split("/BaseFont /")
        .skip(1)
        .chain(pdf.split("/FontName /").skip(1))
        .filter_map(|rest| rest.split(' ').next())
        .collect()
}

/// A collection of the two test fonts, Liberation Sans then Noto Sans, built here
/// rather than committed.
fn collection() -> TestResult<Vec<u8>> {
    let fonts = [liberation_sans()?, noto_sans()?];
    let mut ttc = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x02".to_vec();
    let mut start = 12 + 4 * fonts.len();
    for font in &fonts {
        ttc.extend_from_slice(&u32::try_from(start)?.to_be_bytes());
        start += font.len();
    }
    for font in &fonts {
        let shift = u32::try_from(ttc.len())?;
        let mut font = font.clone();
        let tables = usize::from(u16_at(&font, 4).ok_or("short font")?);
        for record in 0..tables {
            let at = 12 + 16 * record + 8;
            let offset = u32_at(&font, at).ok_or("short table directory")? + shift;
            font.get_mut(at..at + 4)
                .ok_or("short table directory")?
                .copy_from_slice(&offset.to_be_bytes());
        }
        ttc.extend_from_slice(&font);
    }
    Ok(ttc)
}

/// The CIDs of every `<...> Tj` string in `pdf`.
fn shown_cids(pdf: &str) -> Vec<Vec<u16>> {
    pdf.split('<')
        .filter_map(|piece| piece.split_once("> Tj"))
        .map(|(hex, _)| {
            hex.as_bytes()
                .chunks(4)
                .filter_map(|cid| u16::from_str_radix(std::str::from_utf8(cid).ok()?, 16).ok())
                .collect()
        })
        .collect()
}

#[test]
fn embeds_truetype_fonts_as_cid_fonts() -> TestResult {
    let font = Font::from_bytes(liberation_sans()?, 0)?;
    assert_eq!(font.name(), "LiberationSans");
    assert_eq!(font.embedding_notes(), []);
    let doc = one_line(&font, "Привет, мир")?;
    let pdf = pdf_text(&doc)?;
    for part in [
        "/Subtype /Type0",
        "/Encoding /Identity-H",
        "/Subtype /CIDFontType2",
        "/Registry (Adobe) /Ordering (Identity) /Supplement 0",
        "/CIDToGIDMap /Identity",
        "/FontFile2",
        "/ToUnicode",
    ] {
        assert!(pdf.contains(part), "no {part}");
    }
    // A subset's name has a tag of six capitals.
    let (_, name) = pdf.split_once("/BaseFont /").ok_or("no BaseFont")?;
    let (tag, rest) = name.split_once('+').ok_or("no subset tag")?;
    assert!(
        tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()),
        "{tag}"
    );
    assert!(rest.starts_with("LiberationSans "), "{rest}");
    // Eleven characters, nine different glyphs (и and р twice).
    assert!(pdf.contains("9 beginbfchar"), "{pdf}");
    // The font program is compressed, and as a TrueType font program it gives its
    // length again, uncompressed (ISO 32000-1, Table 127). It decompresses to the
    // program of an uncompressed file, a font of its own.
    let program = |doc: &Document| -> TestResult<(String, Vec<u8>)> {
        let program = streams(&doc.to_bytes()?)?
            .into_iter()
            .find(|stream| stream.object == 8)
            .ok_or("no font program")?;
        Ok((program.dictionary, program.data))
    };
    let (dictionary, compressed) = program(&doc)?;
    assert!(dictionary.contains("/Filter /FlateDecode"), "{dictionary}");
    assert!(
        dictionary.ends_with(&format!(" /Length1 {} >>", compressed.len())),
        "{dictionary}"
    );
    let mut plain = doc.clone();
    plain.set_compression(Compression::None);
    let (dictionary, uncompressed) = program(&plain)?;
    assert!(!dictionary.contains("/Filter"), "{dictionary}");
    assert_eq!(compressed, uncompressed);
    FontFace::from_bytes(compressed, 0)?;
    // Without characters that share a glyph, the file is PDF 1.4.
    assert!(pdf.starts_with("%PDF-1.4\n"));
    Ok(())
}

#[test]
fn a_subset_tag_tells_glyph_sets_and_fonts_apart() -> TestResult {
    let tag = |data: Vec<u8>, text: &str| -> TestResult<String> {
        let pdf = pdf_text(&one_line(&Font::from_bytes(data, 0)?, text)?)?;
        let (_, name) = pdf.split_once("/BaseFont /").ok_or("no BaseFont")?;
        Ok(name.get(..6).ok_or("short name")?.to_owned())
    };
    let ab = tag(liberation_sans()?, "АБ")?;
    assert_eq!(ab, tag(liberation_sans()?, "БА")?);
    assert_ne!(ab, tag(liberation_sans()?, "АВ")?);
    // Another font with the same name and glyph IDs: Liberation Sans with another
    // advance width for А (its hmtx entry, an advance and a side bearing per glyph).
    let mut other = liberation_sans()?;
    let a = FontFace::from_bytes(liberation_sans()?, 0)?
        .glyph_id('А')
        .ok_or("no А")?;
    let hmtx = table(&other, b"hmtx").ok_or("no hmtx table")?;
    let advance = hmtx + 4 * usize::from(a.to_u16());
    other
        .get_mut(advance..advance + 2)
        .ok_or("short hmtx table")?
        .copy_from_slice(&1000u16.to_be_bytes());
    assert_ne!(ab, tag(other, "АБ")?);
    Ok(())
}

#[test]
fn widths_and_metrics_are_the_fonts_own() -> TestResult {
    let face = FontFace::from_bytes(liberation_sans()?, 0)?;
    let text = "AWЖ.";
    let pdf = pdf_text(&one_line(&Font::from_face(face.clone())?, text)?)?;
    // The subset numbers the glyphs from 1 in the order of their IDs in the font, and
    // /W gives their advances in 1/1000 em: Liberation Sans has 2048 units per em.
    let mut glyphs: Vec<_> = text
        .chars()
        .map(|ch| face.glyph_id(ch).ok_or("no glyph"))
        .collect::<Result<_, _>>()?;
    glyphs.sort();
    let (_, widths) = pdf.split_once("/W [1 [").ok_or("no /W")?;
    let (widths, _) = widths.split_once("]]").ok_or("no end of /W")?;
    let widths: Vec<f64> = widths
        .split(' ')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    assert_eq!(widths.len(), glyphs.len());
    for (width, glyph) in widths.iter().zip(&glyphs) {
        let advance = f64::from(face.advance_width(*glyph).ok_or("no advance")?);
        assert!(
            (width - advance * 1000.0 / 2048.0).abs() < 0.0001,
            "{width} {advance}"
        );
    }
    // The descriptor's metrics, also in 1/1000 em: the head table's bounding box
    // (-1114, -621, 2666, 2007), the hhea ascender and descender (1854, -434) and the
    // OS/2 cap height (1409).
    assert!(
        pdf.contains(
            "/Flags 4 /FontBBox [-543.9453 -303.2227 1301.7578 979.9805] /ItalicAngle 0 \
             /Ascent 905.2734 /Descent -211.9141 /CapHeight 687.9883 /StemV 88"
        ),
        "{pdf}"
    );
    Ok(())
}

#[test]
fn embeds_cff_fonts_as_cid_font_type_0() -> TestResult {
    let font = Font::from_bytes(noto_sans()?, 0)?;
    assert_eq!(font.name(), "NotoSans-Regular");
    let pdf = pdf_text(&one_line(&font, "Καλημέρα")?)?;
    for part in [
        "/Subtype /CIDFontType0 ",
        "/FontFile3",
        "/Subtype /CIDFontType0C",
        "+NotoSans-Regular-Identity-H /Encoding /Identity-H",
    ] {
        assert!(pdf.contains(part), "no {part}");
    }
    assert!(!pdf.contains("/CIDToGIDMap"));
    Ok(())
}

#[test]
fn embeds_a_face_of_a_collection() -> TestResult {
    let noto = Font::from_bytes(collection()?, 1)?;
    assert_eq!(noto.name(), "NotoSans-Regular");
    assert!(pdf_text(&one_line(&noto, "Ж")?)?.contains("/CIDFontType0C"));
    Ok(())
}

#[test]
fn text_width_uses_the_embedded_fonts_advances() -> TestResult {
    // minimal.ttf's A is 600/1000 em wide, Helvetica's 667/1000.
    let font = Font::from_bytes(minimal()?, 0)?;
    assert_eq!(font.text_width("AAA", Pt(10.0))?, Pt(18.0));
    let helvetica = Font::standard(StandardFont::Helvetica);
    assert_eq!(helvetica.text_width("AAA", Pt(10.0))?, Pt(20.01));

    // Noto Sans's widths, each glyph's advance read from its hmtx table.
    let face = FontFace::from_bytes(noto_sans()?, 0)?;
    let text = "Wide Жж Ωω";
    let mut units = 0;
    for ch in text.chars() {
        let glyph = face.glyph_id(ch).ok_or("no glyph")?;
        units += u32::from(face.advance_width(glyph).ok_or("no advance")?);
    }
    let expected = f64::from(units) * 12.0 / f64::from(face.units_per_em());
    let noto = Font::from_face(face)?;
    let width = noto.text_width(text, Pt(12.0))?;
    assert!((width.0 - expected).abs() < 1e-9, "{width:?} != {expected}");
    assert_ne!(
        noto.text_width("Wide", Pt(12.0))?,
        helvetica.text_width("Wide", Pt(12.0))?
    );
    Ok(())
}

#[test]
fn text_to_pdf_wraps_with_the_embedded_fonts_advances() -> TestResult {
    // An A4 line is 595.28 - 2 × 72 = 451.28 pt wide. At 10 pt minimal.ttf's A is 6 pt
    // wide, so 75 fit on a line; Helvetica's is 6.67 pt, which would give 67.
    let font = Font::from_bytes(minimal()?, 0)?;
    let doc = text_to_pdf(&"A".repeat(100), PageSize::A4, &font, Pt(10.0))?;
    let lines: Vec<usize> = shown_cids(&pdf_text(&doc)?).iter().map(Vec::len).collect();
    assert_eq!(lines, [75, 25]);

    // Wrapping at spaces measures the space's own width too: "A " is 6 + 2.5 pt, so 53
    // words fit (53 × 8.5 - 2.5 = 448 pt); with Helvetica's 6.67 + 2.78 pt only 47 would.
    let words = "A ".repeat(60);
    let doc = text_to_pdf(words.trim_end(), PageSize::A4, &font, Pt(10.0))?;
    let lines: Vec<usize> = shown_cids(&pdf_text(&doc)?).iter().map(Vec::len).collect();
    assert_eq!(lines, [53 * 2 - 1, 7 * 2 - 1]);
    Ok(())
}

#[test]
fn embeds_each_font_once_with_the_glyphs_of_every_page() -> TestResult {
    let liberation = Font::from_bytes(liberation_sans()?, 0)?;
    // The same data loaded again is the same font.
    let again = Font::from_bytes(liberation_sans()?, 0)?;
    assert_eq!(liberation, again);
    let helvetica = Font::standard(StandardFont::Helvetica);
    let mut doc = Document::new();
    doc.add_page(PageSize::A4)?
        .add_text(Pt(72.0), Pt(700.0), &liberation, Pt(12.0), "аб")?;
    let page = doc.add_page(PageSize::A4)?;
    page.add_text(Pt(72.0), Pt(700.0), &helvetica, Pt(12.0), "Latin")?;
    page.add_text(Pt(72.0), Pt(680.0), &again, Pt(12.0), "бв")?;
    let pdf = pdf_text(&doc)?;
    assert_eq!(pdf.matches("/Subtype /Type0").count(), 1);
    assert_eq!(pdf.matches("/BaseFont /Helvetica").count(), 1);
    // а, б and в: the subset has the glyphs of both pages.
    assert!(pdf.contains("3 beginbfchar"), "{pdf}");
    // The second page uses both fonts.
    assert!(pdf.contains("/Font << /F1 4 0 R /F2 3 0 R >>"), "{pdf}");
    Ok(())
}

/// minimal.ttf with Á and ´ mapped to the glyph of A, as fonts do for look-alike
/// characters (Arial maps Latin Ə and Cyrillic Ә to one glyph). Its cmap has one segment
/// per character, and idDelta is the glyph minus the character.
fn shared_glyph_font() -> TestResult<Font> {
    let deltas = |accent: u16, acute: u16| -> Vec<u8> {
        [(0x20u16, 1u16), (0x41, 2), (0xB4, accent), (0xC1, acute)]
            .iter()
            .flat_map(|&(code, glyph)| glyph.wrapping_sub(code).to_be_bytes())
            .collect()
    };
    let (from, to) = (deltas(3, 4), deltas(2, 2));
    let mut data = minimal()?;
    let at = data
        .windows(from.len())
        .position(|window| window == from)
        .ok_or("no cmap deltas")?;
    data.get_mut(at..at + to.len())
        .ok_or("short cmap")?
        .copy_from_slice(&to);
    Ok(Font::from_bytes(data, 0)?)
}

#[test]
fn a_shared_glyph_keeps_each_character_for_text_extraction() -> TestResult {
    let font = shared_glyph_font()?;
    let pdf = pdf_text(&one_line(&font, "AÁA")?)?;
    // Replacement text in marked content is a PDF 1.5 feature.
    assert!(pdf.starts_with("%PDF-1.5\n"));
    // One glyph: the ToUnicode map gives the character it showed first, A, and Á is
    // shown in a span with its own ActualText.
    assert!(
        pdf.contains("1 beginbfchar\n<0001> <0041>\nendbfchar"),
        "{pdf}"
    );
    assert!(
        pdf.contains("<0001> Tj\n/Span << /ActualText <FEFF00C1> >> BDC <0001> Tj EMC\n<0001> Tj"),
        "{pdf}"
    );
    Ok(())
}

#[test]
fn shows_long_text_with_strings_readers_handle() -> TestResult {
    // 40,001 characters shown with their glyphs' own mappings, then 20,000 with
    // replacement text: more than the 32,767 bytes of a string in PDF 1.5. Neither part
    // repeats every 16,382 characters, so a piece out of place shows.
    let font = shared_glyph_font()?;
    let mapped: String = "AA ".chars().cycle().take(40_001).collect();
    let replaced: String = "Á´´".chars().cycle().take(20_000).collect();
    let pdf = pdf_text(&one_line(&font, &format!("{mapped}{replaced}"))?)?;
    let (_, content) = pdf.split_once("BT\n").ok_or("no text object")?;
    let (content, _) = content.split_once("\nET").ok_or("no end of text")?;
    // The hexadecimal digits of a string.
    let digits = |string: &str| -> TestResult<String> {
        let (_, rest) = string.split_once('<').ok_or("no string")?;
        let (digits, _) = rest.split_once('>').ok_or("no end of string")?;
        Ok(digits.to_owned())
    };
    // The CIDs of the Tj strings, and the replacement text, as written.
    let (mut shown, mut actual) = (Vec::new(), Vec::new());
    for line in content.lines().skip(2) {
        let cids = match line.split_once("/ActualText ") {
            Some((_, span)) => {
                actual.push(digits(span)?);
                let (_, cids) = span.split_once(">> BDC ").ok_or("no BDC")?;
                digits(cids)?
            }
            None => digits(line)?,
        };
        shown.push(cids);
    }
    // Pieces of 16,382 characters: the CIDs take 32,764 bytes, the replacement text
    // 32,766 with its byte order mark.
    let bytes = |strings: &[String]| strings.iter().map(|s| s.len() / 2).collect::<Vec<_>>();
    assert_eq!(bytes(&shown), [32764, 32764, 14474, 32764, 7236]);
    assert_eq!(bytes(&actual), [32766, 7238]);
    // Together the strings show the text in order: A and the accented characters share
    // a glyph, the space has its own.
    let all = shown.concat();
    let (a, space) = (all.get(..4).ok_or("short")?, all.get(8..12).ok_or("short")?);
    assert_ne!(a, space);
    let cid = |ch: char| if ch == ' ' { space } else { a };
    let expected: String = mapped.chars().chain(replaced.chars()).map(cid).collect();
    assert!(all == expected, "the CIDs differ from the text's");
    let text: String = actual
        .iter()
        .map(|units| units.strip_prefix("FEFF").ok_or("no byte order mark"))
        .collect::<Result<_, _>>()?;
    let expected: String = replaced
        .encode_utf16()
        .map(|u| format!("{u:04X}"))
        .collect();
    assert!(
        text == expected,
        "the replacement text differs from the text"
    );
    Ok(())
}

#[test]
fn refuses_characters_an_embedded_font_cannot_show() -> TestResult {
    let font = Font::from_bytes(liberation_sans()?, 0)?;
    let mut doc = Document::new();
    let page = doc.add_page(PageSize::A4)?;
    let cases = [
        (
            "ok שלום",
            "character U+05E9 'ש' at byte 3 is in the Hebrew script, which needs shaping or \
             right-to-left layout; that is not supported yet",
        ),
        (
            "a\u{202E}b",
            "character U+202E '\\u{202e}' at byte 1 is a bidirectional formatting character, \
             and right-to-left layout is not supported yet",
        ),
        (
            "a\tb",
            "character U+0009 '\\t' at byte 1 is a control character",
        ),
        (
            "Ж\u{1100}",
            "character U+1100 'ᄀ' at byte 2 is a conjoining Hangul jamo; only precomposed \
             Hangul syllables can be drawn without shaping",
        ),
        (
            "\u{E000}",
            "character U+E000 '\\u{e000}' at byte 0 is unassigned or for private use",
        ),
        (
            "Ж漢",
            "font LiberationSans has no glyph for character U+6F22 '漢' at byte 2",
        ),
    ];
    for (text, message) in cases {
        let err = page
            .add_text(Pt(72.0), Pt(700.0), &font, Pt(12.0), text)
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::UnsupportedCharacter { .. } | Error::MissingGlyph { .. }
            ),
            "{err:?}"
        );
        assert_eq!(err.to_string(), message);
        assert!(matches!(
            officewright_core::Error::from(err),
            officewright_core::Error::Unsupported(_)
        ));
        let err = font.text_width(text, Pt(10.0)).unwrap_err();
        assert_eq!(err.to_string(), message);
    }
    // Nothing was added: the page uses no font.
    assert!(!pdf_text(&doc)?.contains("/Font"));
    Ok(())
}

#[test]
fn follows_the_fonts_embedding_license() -> TestResult {
    for fs_type in [0x0002, 0x0202, 0x0200] {
        let err = Font::from_bytes(with_fs_type(liberation_sans()?, fs_type)?, 0).unwrap_err();
        let message = if fs_type == 0x0200 {
            "the license of font LiberationSans allows embedding only its bitmaps, not its \
             outlines"
        } else {
            "the license of font LiberationSans does not allow embedding it"
        };
        assert!(
            matches!(err, Error::EmbeddingNotAllowed { .. }),
            "{fs_type:#06x}: {err:?}"
        );
        assert_eq!(err.to_string(), message, "{fs_type:#06x}");
    }
    let notes = |fs_type| -> TestResult<Vec<EmbeddingNote>> {
        let font = Font::from_bytes(with_fs_type(liberation_sans()?, fs_type)?, 0)?;
        Ok(font.embedding_notes().to_vec())
    };
    assert_eq!(notes(0x0000)?, []);
    assert_eq!(notes(0x0004)?, [EmbeddingNote::PreviewAndPrint]);
    assert_eq!(notes(0x0008)?, [EmbeddingNote::Editable]);
    assert_eq!(
        notes(0x0104)?,
        [EmbeddingNote::PreviewAndPrint, EmbeddingNote::WholeFont]
    );
    assert_eq!(
        EmbeddingNote::WholeFont.to_string(),
        "its license forbids subsetting, so the whole font is embedded"
    );
    Ok(())
}

#[test]
fn embeds_every_glyph_of_a_font_that_must_not_be_subset() -> TestResult {
    let whole = Font::from_bytes(with_fs_type(liberation_sans()?, 0x0100)?, 0)?;
    let subset = Font::from_bytes(liberation_sans()?, 0)?;
    let whole_pdf = one_line(&whole, "Ж")?.to_bytes()?;
    let subset_pdf = one_line(&subset, "Ж")?.to_bytes()?;
    let text = String::from_utf8_lossy(&whole_pdf);
    // A whole font keeps its name, and its glyph IDs are the font's own.
    assert!(
        text.contains("/BaseFont /LiberationSans "),
        "no untagged name"
    );
    assert!(!text.contains("+LiberationSans"));
    let face = FontFace::from_bytes(liberation_sans()?, 0)?;
    let zhe = face.glyph_id('Ж').ok_or("no Ж")?;
    assert_eq!(shown_cids(&text), [vec![zhe.to_u16()]]);
    assert!(whole_pdf.len() > 20 * subset_pdf.len());
    Ok(())
}

#[test]
fn cuts_font_names_to_what_readers_accept() -> TestResult {
    // ISO 32000-1, Annex C: a reader has to accept names of up to 127 bytes. Every name
    // of the font in the file takes that much here: the font's own name is cut after
    // 120 bytes in a TrueType subset, which adds a tag of seven, after 109 in a CFF
    // subset, whose Type 0 font adds "-Identity-H" too, and after 127 in a whole font.
    let long = "Long".repeat(50);
    let cases = [
        (minimal()?, None, 120, [127, 127, 127]),
        (noto_sans()?, None, 109, [127, 116, 116]),
        (liberation_sans()?, Some(0x0100), 127, [127, 127, 127]),
    ];
    for (data, fs_type, kept, lengths) in cases {
        let mut data = with_postscript_name(data, &long)?;
        if let Some(fs_type) = fs_type {
            data = with_fs_type(data, fs_type)?;
        }
        let font = Font::from_bytes(data, 0)?;
        assert_eq!(font.name(), &long[..kept]);
        let pdf = pdf_text(&one_line(&font, "A A")?)?;
        // The Type 0 font's /BaseFont, the CIDFont's and the descriptor's /FontName.
        let names = font_names(&pdf);
        assert_eq!(
            names.iter().map(|name| name.len()).collect::<Vec<_>>(),
            lengths
        );
        for name in names {
            assert!(name.contains(&long[..kept]), "{name}");
        }
    }
    // A name at the limit is kept whole.
    let font = Font::from_bytes(with_postscript_name(minimal()?, &long[..120])?, 0)?;
    assert_eq!(font.name(), &long[..120]);
    Ok(())
}

#[test]
fn reports_fonts_it_cannot_use() -> TestResult {
    let err = Font::from_bytes(&b"not a font"[..], 0).unwrap_err();
    assert!(matches!(err, Error::Font { path: None, .. }), "{err:?}");
    assert_eq!(err.to_string(), "cannot use the font data");
    let core = officewright_core::Error::from(err);
    assert!(
        matches!(&core, officewright_core::Error::Malformed(message)
            if message.starts_with("cannot use the font data: data is not a TrueType")),
        "{core}"
    );

    // A face the file does not have is the caller's mistake, not damage.
    let err = Font::from_bytes(liberation_sans()?, 3).unwrap_err();
    assert!(matches!(err, Error::Font { .. }), "{err:?}");
    let core = officewright_core::Error::from(err);
    assert!(
        matches!(core, officewright_core::Error::InvalidArgument(_)),
        "{core}"
    );

    let missing = repository().join("tests/fonts/no-such-font.ttf");
    let err = Font::from_file(&missing, 0).unwrap_err();
    assert!(matches!(err, Error::Read { .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!("cannot read {}", missing.display())
    );

    let not_a_font = repository().join("tests/fonts/liberation/LICENSE");
    let err = Font::from_file(&not_a_font, 0).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("cannot use the font in {}", not_a_font.display())
    );
    Ok(())
}

#[test]
fn equal_documents_give_equal_files() -> TestResult {
    let make = || -> TestResult<Vec<u8>> {
        let font = Font::from_bytes(noto_sans()?, 0)?;
        let text = "Съешь же ещё этих мягких булок";
        Ok(text_to_pdf(text, PageSize::A4, &font, Pt(11.0))?.to_bytes()?)
    };
    assert_eq!(make()?, make()?);
    Ok(())
}

#[test]
fn from_file_reads_the_same_font_as_from_bytes() -> TestResult {
    let path = repository().join("tests/fonts/liberation/LiberationSans-Regular.ttf");
    let from_file = Font::from_file(&path, 0)?;
    assert_eq!(from_file, Font::from_bytes(liberation_sans()?, 0)?);
    let face = FontFace::from_bytes(liberation_sans()?, 0)?;
    assert_eq!(from_file, Font::from_face(face)?);
    Ok(())
}

/// A book of the fonts in tests/fonts, without the system fonts, so the results are the
/// same on every machine.
fn test_fonts() -> TestResult<FontBook> {
    let mut book = FontBook::new();
    book.add_dir(repository().join("tests/fonts"))?;
    Ok(book)
}

#[test]
fn resolves_families_and_reports_the_substitutions() -> TestResult {
    let book = test_fonts()?;
    let mut resolver = FontResolver::new(&book);
    let liberation = Font::resolve(&mut resolver, &FontRequest::new("Liberation Sans"))?;
    assert_eq!(liberation.name(), "LiberationSans");
    // Windows compares family names without regard to case, and so does Word.
    let noto = Font::resolve(&mut resolver, &FontRequest::new("noto sans"))?;
    assert_eq!(noto.name(), "NotoSans-Regular");
    assert!(resolver.substitutions().is_empty());

    // Arial's metric-compatible substitute is the same font as Liberation Sans.
    assert_eq!(
        Font::resolve(&mut resolver, &FontRequest::new("Arial"))?,
        liberation
    );
    // The bold face of tests/fonts is used as it is.
    let bold = Font::resolve(
        &mut resolver,
        &FontRequest::new("Liberation Sans").bold(true),
    )?;
    assert_eq!(bold.name(), "LiberationSans-Bold");
    // Noto Sans has no bold face there, so its regular face is drawn bold, and the
    // report says so.
    let noto_bold = Font::resolve(&mut resolver, &FontRequest::new("Noto Sans").bold(true))?;
    assert_eq!(noto_bold, noto);
    let calibri = Font::resolve(&mut resolver, &FontRequest::new("Calibri"))?;
    assert_eq!(calibri.name(), "Carlito-Regular");
    // Verdana has no metric-compatible substitute, so a common sans-serif font stands
    // in, and the report says so.
    let verdana = Font::resolve(&mut resolver, &FontRequest::new("Verdana"))?;
    assert_eq!(verdana, liberation);
    let report: Vec<String> = resolver
        .substitutions()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        report,
        [
            "\"Arial\" → \"Liberation Sans\" (metric-compatible substitute)",
            "\"Noto Sans\" bold → \"Noto Sans\" with synthetic bold",
            "\"Calibri\" → \"Carlito\" (metric-compatible substitute)",
            "\"Verdana\" → \"Liberation Sans\" (fallback; not installed)",
        ]
    );
    let fallback = resolver.substitutions().get(3).ok_or("no Verdana")?;
    assert_eq!(fallback.kind(), ResolutionKind::Fallback);
    assert_eq!(fallback.uninstalled_substitute(), None);
    Ok(())
}

#[test]
fn reports_a_family_nothing_stands_in_for() -> TestResult {
    let book = FontBook::new();
    let mut resolver = FontResolver::new(&book);
    let err = Font::resolve(&mut resolver, &FontRequest::new("Calibri")).unwrap_err();
    assert!(matches!(err, Error::FontNotFound { .. }), "{err:?}");
    assert_eq!(err.to_string(), "no font found for family \"Calibri\"");
    assert!(matches!(
        officewright_core::Error::from(err),
        officewright_core::Error::Unsupported(_)
    ));
    let report = resolver.substitutions().first().ok_or("no report")?;
    assert_eq!(report.to_string(), "\"Calibri\" → no font available");
    Ok(())
}

#[test]
fn reports_a_chosen_font_file_that_went_away() -> TestResult {
    let dir = std::env::temp_dir().join(format!("officewright-pdf-gone-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("minimal.ttf");
    std::fs::write(&path, minimal()?)?;
    let mut book = FontBook::new();
    book.add_dir(&dir)?;
    std::fs::remove_file(&path)?;
    let mut resolver = FontResolver::new(&book);
    let err = Font::resolve(&mut resolver, &FontRequest::new("Officewright Test")).unwrap_err();
    std::fs::remove_dir_all(&dir)?;
    assert!(matches!(err, Error::Font { path: Some(_), .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!("cannot use the font in {}", path.display())
    );
    Ok(())
}
