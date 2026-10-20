//! Drawing glyph runs in real fonts: Liberation Sans from tests/fonts/ at the repository
//! root, which keeps its own license (THIRD-PARTY-NOTICES.md), and officewright-font's
//! minimal test font. Neither is part of this crate, so this test is not in the published
//! crate.
//!
//! The positions are checked the way a PDF reader finds them: by running the text
//! operators of the content stream with the widths of the font's `/W` array.
//! tests/pdf-output checks the files with external tools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use officewright_font::FontFace;
use officewright_pdf::{
    Color, Compression, Document, Error, Font, GlyphId, GlyphRun, PageSize, PlacedGlyph, Pt,
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

/// officewright-font's test font: 1000 units per em, space (glyph 1, 250 units wide),
/// `A` (glyph 2, 600), acute (glyph 3) and `Á` (glyph 4).
fn minimal() -> TestResult<Vec<u8>> {
    read("crates/officewright-font/tests/data/minimal.ttf")
}

const SPACE: GlyphId = GlyphId::new(1);
const A: GlyphId = GlyphId::new(2);
const ACUTE: GlyphId = GlyphId::new(3);
const A_ACUTE: GlyphId = GlyphId::new(4);

fn glyph(id: GlyphId, x: f64, y: f64) -> PlacedGlyph {
    PlacedGlyph::new(id, Pt(x), Pt(y))
}

/// The PDF of a document with one page holding `runs`, uncompressed so that its content
/// and ToUnicode map can be read.
fn pdf_of(runs: &[GlyphRun]) -> TestResult<String> {
    let mut doc = Document::new();
    let page = doc.add_page(PageSize::A4)?;
    for run in runs {
        page.draw_glyphs(run)?;
    }
    pdf_text(&mut doc)
}

fn pdf_text(doc: &mut Document) -> TestResult<String> {
    doc.set_compression(Compression::None);
    Ok(String::from_utf8_lossy(&doc.to_bytes()?).into_owned())
}

/// The content stream of the first page: the first stream of the file.
fn content(pdf: &str) -> TestResult<&str> {
    let (_, stream) = pdf.split_once("stream\n").ok_or("no stream")?;
    let (content, _) = stream.split_once("\nendstream").ok_or("no endstream")?;
    Ok(content)
}

/// The glyph widths of the `/W` array of the first CIDFont, in 1/1000 em, by CID.
fn widths(pdf: &str) -> TestResult<HashMap<u16, f64>> {
    let (_, w) = pdf.split_once("/W [").ok_or("no /W")?;
    let spaced = w.replace('[', " [ ").replace(']', " ] ");
    let mut tokens = spaced.split_whitespace();
    let mut widths = HashMap::new();
    while let Some(start) = tokens.next() {
        if start == "]" {
            break;
        }
        let start: u16 = start.parse()?;
        if tokens.next() != Some("[") {
            return Err("no width array".into());
        }
        let run = tokens.by_ref().take_while(|&width| width != "]");
        for (cid, width) in (start..).zip(run) {
            widths.insert(cid, width.parse()?);
        }
    }
    Ok(widths)
}

/// A glyph drawn by a content stream: its CID and the position of its origin on the
/// page.
#[derive(Debug)]
struct Drawn {
    cid: u16,
    x: f64,
    y: f64,
}

/// Runs the text operators of `content` as a reader does (ISO 32000-1, 9.4) and returns
/// every glyph drawn. Only the operators this crate writes are understood, and text
/// matrices only of the form `1 0 k 1 e f`.
fn draw(content: &str, widths: &HashMap<u16, f64>) -> TestResult<Vec<Drawn>> {
    let mut drawn = Vec::new();
    let (mut size, mut scale, mut rise) = (0.0, 1.0, 0.0);
    let (mut skew, mut x0, mut y0, mut tx) = (0.0, 0.0, 0.0, 0.0);
    let mut operands: Vec<String> = Vec::new();
    let tokens = content
        .replace('[', " [ ")
        .replace(']', " ] ")
        .replace('<', " <")
        .replace('>', "> ");
    let number = |operand: Option<&String>| -> TestResult<f64> {
        Ok(operand.ok_or("missing operand")?.parse()?)
    };
    for token in tokens.split_whitespace() {
        if token.starts_with(|c: char| c.is_ascii_alphabetic() || c == '\'') && token != "TJ" {
            let n = operands.len();
            match token {
                "Tf" => size = number(operands.get(n - 1))?,
                "Tz" => scale = number(operands.get(n - 1))? / 100.0,
                "Ts" => rise = number(operands.get(n - 1))?,
                "Tm" => {
                    assert_eq!(
                        operands.get(n - 6..n - 4),
                        Some(&["1".into(), "0".into()][..])
                    );
                    assert_eq!(operands.get(n - 3).map(String::as_str), Some("1"));
                    skew = number(operands.get(n - 4))?;
                    x0 = number(operands.get(n - 2))?;
                    y0 = number(operands.get(n - 1))?;
                    tx = 0.0;
                }
                "Td" | "Tj" | "Tc" | "Tw" | "TL" | "T" | "'" => {
                    return Err(format!("unexpected operator {token}").into());
                }
                _ => {}
            }
            operands.clear();
        } else if token == "TJ" {
            let mut array = false;
            for operand in &operands {
                match operand.as_str() {
                    "[" => array = true,
                    "]" => array = false,
                    hex if array && hex.starts_with('<') => {
                        let hex = hex.trim_start_matches('<').trim_end_matches('>');
                        for at in (0..hex.len()).step_by(4) {
                            let cid = u16::from_str_radix(hex.get(at..at + 4).ok_or("odd")?, 16)?;
                            drawn.push(Drawn {
                                cid,
                                x: x0 + tx + skew * rise,
                                y: y0 + rise,
                            });
                            let width = widths.get(&cid).ok_or("no width")?;
                            tx += width / 1000.0 * size * scale;
                        }
                    }
                    adjustment if array => {
                        let adjustment: f64 = adjustment.parse()?;
                        assert!(adjustment.abs() <= 32767.0, "{adjustment}");
                        tx -= adjustment / 1000.0 * size * scale;
                    }
                    _ => {}
                }
            }
            operands.clear();
        } else {
            operands.push(token.to_owned());
        }
    }
    Ok(drawn)
}

/// Asserts that the glyphs of `runs`, drawn on one page, are the runs' glyphs and land
/// where the runs put them, slant included, to within 0.001 pt.
fn assert_positions(runs: &[GlyphRun]) -> TestResult {
    assert_positions_within(runs, 1e-3)
}

/// As [`assert_positions`], to within `tolerance` points.
fn assert_positions_within(runs: &[GlyphRun], tolerance: f64) -> TestResult {
    let pdf = pdf_of(runs)?;
    let drawn = draw(content(&pdf)?, &widths(&pdf)?)?;
    let expected: Vec<(f64, f64)> = runs
        .iter()
        .flat_map(|run| {
            run.glyphs().iter().map(|glyph| {
                (
                    run.x().0 + glyph.x().0 + run.skew() * glyph.y().0,
                    run.y().0 + glyph.y().0,
                )
            })
        })
        .collect();
    assert_eq!(drawn.len(), expected.len(), "{drawn:?}");
    // Each glyph has a CID of its own.
    let glyphs = runs
        .iter()
        .flat_map(|run| run.glyphs().iter().map(|g| g.glyph()));
    let mut cids = HashMap::new();
    for (glyph, drawn) in glyphs.zip(&drawn) {
        assert_eq!(
            *cids.entry(glyph).or_insert(drawn.cid),
            drawn.cid,
            "{glyph:?}"
        );
    }
    let mut distinct: Vec<u16> = cids.values().copied().collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), cids.len());
    for (glyph, (x, y)) in drawn.iter().zip(expected) {
        assert!(
            (glyph.x - x).abs() < tolerance && (glyph.y - y).abs() < tolerance,
            "{glyph:?} should be at ({x}, {y})"
        );
    }
    Ok(())
}

/// The glyph of `ch` in `face` and its advance width at `size`.
fn glyph_of(face: &FontFace, ch: char, size: f64) -> TestResult<(GlyphId, f64)> {
    let glyph = face.char_map().map(ch).ok_or("no glyph")?;
    let advance = f64::from(face.advance_width(glyph).ok_or("no advance")?);
    Ok((glyph, advance * size / f64::from(face.units_per_em())))
}

/// A run of `text` in Liberation Sans, one glyph per character, with `extra` points
/// added after every glyph, so the positions differ from the advances.
fn spaced_run(data: &[u8], text: &str, size: f64, extra: f64) -> TestResult<GlyphRun> {
    let face = FontFace::from_bytes(data.to_vec(), 0)?;
    let font = Font::from_bytes(data.to_vec(), 0)?;
    let mut run = GlyphRun::new(&font, Pt(size), Pt(72.0), Pt(700.0));
    let mut x = 0.0;
    for ch in text.chars() {
        let (id, advance) = glyph_of(&face, ch, size)?;
        run.push_cluster(ch.encode_utf8(&mut [0; 4]), [glyph(id, x, 0.0)]);
        x += advance + extra;
    }
    Ok(run)
}

#[test]
fn places_every_glyph_where_the_run_puts_it() -> TestResult {
    let data = liberation_sans()?;
    for (size, extra) in [(12.0, 0.0), (11.0, 0.37), (9.5, -1.25), (72.0, 15.0)] {
        assert_positions(&[spaced_run(&data, "Typewriter, AVA.", size, extra)?])?;
    }
    // Scaled, slanted and stroked runs, glyphs raised and lowered, and glyphs drawn
    // backwards, as right-to-left text is.
    let font = Font::from_bytes(data.clone(), 0)?;
    let face = FontFace::from_bytes(data, 0)?;
    let (t, _) = glyph_of(&face, 'T', 12.0)?;
    let (o, _) = glyph_of(&face, 'o', 12.0)?;
    for (scale, skew) in [(1.0, 0.0), (1.5, 0.0), (0.37, 0.2), (2.0, -0.3333)] {
        let mut run = GlyphRun::new(&font, Pt(12.0), Pt(100.25), Pt(500.5))
            .with_horizontal_scale(scale)
            .with_skew(skew)
            .with_stroke(Pt(0.4));
        run.push_cluster("T", [glyph(t, 0.0, 0.0)]);
        run.push_cluster("o", [glyph(o, 5.123_456, 0.0)]);
        run.push_cluster("o", [glyph(o, 11.0, 4.5), glyph(o, 11.0, -2.25)]);
        run.push_cluster("T", [glyph(t, 30.0, 4.5)]);
        run.push_cluster("o", [glyph(o, 8.0, 0.0)]);
        run.push_cluster("T", [glyph(t, -20.0, 0.0)]);
        assert_positions(&[run])?;
    }
    Ok(())
}

#[test]
fn places_glyphs_too_far_apart_for_an_adjustment_with_a_text_matrix() -> TestResult {
    // At 0.01 pt a glyph 300 pt further on is 30 million thousandths of the em away,
    // beyond the largest real number every reader handles.
    let font = Font::from_bytes(minimal()?, 0)?;
    let mut run =
        GlyphRun::new(&font, Font::MIN_SIZE, Pt(-32767.0), Pt(0.0)).with_horizontal_scale(0.01);
    for (i, x) in [0.0, 300.0, 300.001, 32767.0, 0.5].into_iter().enumerate() {
        run.push_cluster(&i.to_string(), [glyph(A, x, 0.0)]);
    }
    assert_positions(&[run])?;
    // At 1 pt, an A is 0.6 pt wide: an adjustment of 32767 thousandths of the em reaches
    // 32.767 pt further on, and one more is too far.
    let mut run = GlyphRun::new(&font, Pt(1.0), Pt(0.0), Pt(0.0));
    for x in [0.0, 33.367, 66.735] {
        run.push_cluster("A", [glyph(A, x, 0.0)]);
    }
    assert_positions(std::slice::from_ref(&run))?;
    let pdf = pdf_of(&[run])?;
    assert_eq!(
        content(&pdf)?,
        "q\n0 0 0 rg\nBT\n/F1 1 Tf\n1 0 0 1 0 0 Tm\n[<0001> -32767 <0001>] TJ\n\
         1 0 0 1 66.735 0 Tm\n[<0001>] TJ\nET\nQ\n"
    );
    // 10 pt between the glyphs of 0.01 pt text: every glyph gets a text matrix of its own.
    let run = spaced_run(&liberation_sans()?, "far apart", 0.01, 10.0)?;
    assert_positions(std::slice::from_ref(&run))?;
    assert_eq!(content(&pdf_of(&[run])?)?.matches(" Tm\n").count(), 9);
    Ok(())
}

#[test]
fn splits_arrays_and_strings_at_the_limits_readers_handle() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    // Every glyph 1 pt after the end of the one before: an adjustment and a string each.
    let mut spaced = GlyphRun::new(&font, Pt(1.0), Pt(-32000.0), Pt(0.0));
    // Every glyph where the one before ends: one string. The space is 250/1000 em wide,
    // 0.0625 pt at this size, so the positions add up exactly.
    let mut packed = GlyphRun::new(&font, Pt(0.25), Pt(-32000.0), Pt(10.0));
    for i in 0..20_000 {
        spaced.push_cluster("A", [glyph(A, f64::from(i) * 1.6, 0.0)]);
    }
    for i in 0..40_000 {
        packed.push_cluster(" ", [glyph(SPACE, f64::from(i) * 0.0625, 0.0)]);
    }
    let runs = [spaced, packed];
    assert_positions(&runs)?;
    let pdf = pdf_of(&runs)?;
    let (mut longest_array, mut longest_string) = (0, 0);
    for array in content(&pdf)?
        .split("] TJ")
        .filter(|array| array.contains('['))
    {
        let (_, elements) = array.rsplit_once('[').ok_or("no array")?;
        longest_array = longest_array.max(elements.split_whitespace().count());
        for string in elements.split_whitespace().filter(|e| e.starts_with('<')) {
            // Two hexadecimal digits per byte, and the angle brackets.
            longest_string = longest_string.max((string.len() - 2) / 2);
        }
    }
    // Both limits were reached, and kept: 8191 elements (PDF 1.4) and 32767 bytes (PDF
    // 1.5, which replacement text needs).
    assert_eq!((longest_array, longest_string), (8191, 32766));
    Ok(())
}

#[test]
fn notdef_keeps_cid_0_wherever_it_is_drawn() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    let mut run = GlyphRun::new(&font, Pt(10.0), Pt(72.0), Pt(700.0));
    // .notdef first, as a layout draws it for a character no font has.
    run.push_cluster("\u{FFFF}", [glyph(GlyphId::NOTDEF, 0.0, 0.0)]);
    run.push_cluster("A", [glyph(A, 6.0, 0.0)]);
    let pdf = pdf_of(&[run])?;
    // Every subset keeps .notdef as glyph 0, so the A is CID 1 and has its own width.
    assert!(content(&pdf)?.contains("<0000"), "{pdf}");
    assert_eq!(widths(&pdf)?.get(&1), Some(&600.0), "{pdf}");
    Ok(())
}

#[test]
fn writes_the_run_state_inside_its_own_graphics_state() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    let mut plain = GlyphRun::new(&font, Pt(10.0), Pt(72.0), Pt(700.0));
    plain.push_cluster("AA", [glyph(A, 0.0, 0.0), glyph(A, 6.0, 0.0)]);
    let mut styled = GlyphRun::new(&font, Pt(12.5), Pt(72.0), Pt(680.0))
        .with_color(Color::rgb(255, 0, 51))
        .with_horizontal_scale(1.5)
        .with_skew(0.25)
        .with_stroke(Pt(0.5));
    styled.push_cluster("A", [glyph(A, 0.0, 0.0)]);
    styled.push_cluster("A", [glyph(A, 12.0, 3.0)]);
    styled.push_cluster(" ", [glyph(SPACE, 20.0, 3.0)]);
    let pdf = pdf_of(&[plain, styled])?;
    // The plain run's cluster has two glyphs, so it has replacement text, and its
    // glyphs follow each other with no adjustment: 600/1000 em at 10 pt is 6 pt.
    // The subset numbers the glyphs in the order they are first shown in: the A is CID 1,
    // and the space, glyph 1 of the font, CID 2. The styled run's A is
    // 600 × 12.5 × 1.5 / 1000 = 11.25 pt wide, so the next A, 0.75 pt further on, needs an
    // adjustment of -0.75 × 1000 / 18.75 = -40 thousandths of the em, and the space, 3.25
    // pt back, one of 173.3333.
    assert_eq!(
        content(&pdf)?,
        "q\n0 0 0 rg\nBT\n/F1 10 Tf\n\
         /Span << /ActualText <FEFF00410041> >> BDC\n1 0 0 1 72 700 Tm\n[<00010001>] TJ\nEMC\n\
         ET\nQ\n\
         q\n1 0 0.2 rg\n1 0 0.2 RG\n0.5 w\nBT\n/F1 12.5 Tf\n150 Tz\n2 Tr\n\
         1 0 0.25 1 72 680 Tm\n[<0001>] TJ\n3 Ts\n[-40 <0001> 173.3333 <0002>] TJ\nET\nQ\n",
        "{pdf}"
    );
    assert!(pdf.starts_with("%PDF-1.5\n"));
    Ok(())
}

#[test]
fn a_ligature_maps_to_all_its_characters() -> TestResult {
    let data = liberation_sans()?;
    let face = FontFace::from_bytes(data.clone(), 0)?;
    let font = Font::from_bytes(data, 0)?;
    // The ligature glyph a shaper puts in place of f and i.
    let (fi, advance) = glyph_of(&face, '\u{FB01}', 12.0)?;
    let (n, _) = glyph_of(&face, 'n', 12.0)?;
    let mut run = GlyphRun::new(&font, Pt(12.0), Pt(72.0), Pt(700.0));
    run.push_cluster("fi", [glyph(fi, 0.0, 0.0)]);
    run.push_cluster("n", [glyph(n, advance, 0.0)]);
    let pdf = pdf_of(&[run])?;
    // The subset numbers the glyphs in the order they are first shown in: the ligature,
    // which comes after n in the font, first.
    assert!(
        pdf.contains("2 beginbfchar\n<0001> <00660069>\n<0002> <006E>\nendbfchar"),
        "{pdf}"
    );
    // ToUnicode alone gives the text: no replacement text, so PDF 1.4.
    assert!(!pdf.contains("/ActualText"), "{pdf}");
    assert!(pdf.starts_with("%PDF-1.4\n"));
    Ok(())
}

#[test]
fn text_a_glyph_cannot_map_gets_replacement_text() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    let mut run = GlyphRun::new(&font, Pt(10.0), Pt(72.0), Pt(700.0));
    // The first text a glyph shows alone is its ToUnicode entry.
    run.push_cluster("A", [glyph(A, 0.0, 0.0)]);
    // The same glyph for other text, as small capitals are.
    run.push_cluster("a", [glyph(A, 6.0, 0.0)]);
    // A base and its mark for one character.
    run.push_cluster("Á", [glyph(A, 12.0, 0.0), glyph(ACUTE, 12.0, 1.0)]);
    // Glyphs that stand for no text, such as tab leaders.
    run.push_cluster("", [glyph(SPACE, 18.0, 0.0)]);
    // Long text: 32 UTF-16 code units fit in a ToUnicode entry; the next cluster's glyph
    // already has one, so its text is replaced.
    let long = "x".repeat(32);
    run.push_cluster(&long, [glyph(A_ACUTE, 20.0, 0.0)]);
    run.push_cluster(&format!("{long}y"), [glyph(A_ACUTE, 26.0, 0.0)]);
    let pdf = pdf_of(&[run])?;
    let utf16 = |text: &str| -> String {
        text.encode_utf16()
            .map(|unit| format!("{unit:04X}"))
            .collect()
    };
    let cmap = format!(
        "2 beginbfchar\n<0001> <0041>\n<0004> <{}>\nendbfchar",
        utf16(&long)
    );
    // The space and the acute show no text alone, so they have no entry.
    assert!(pdf.contains(&cmap), "{pdf}");
    let content = content(&pdf)?;
    for text in ["a", "Á", "", &format!("{long}y")] {
        let span = format!("/Span << /ActualText <FEFF{}> >> BDC\n", utf16(text));
        assert_eq!(content.matches(&span).count(), 1, "{text}: {content}");
    }
    assert_eq!(content.matches("BDC").count(), 4);
    assert_eq!(content.matches("EMC").count(), 4);
    assert!(pdf.starts_with("%PDF-1.5\n"));
    // Every glyph shown is embedded with its width, those without text too.
    let mut cids: Vec<u16> = widths(&pdf)?.into_keys().collect();
    cids.sort_unstable();
    assert_eq!(cids, [1, 2, 3, 4]);
    Ok(())
}

#[test]
fn the_tounicode_length_limit_counts_utf16_code_units() -> TestResult {
    let data = liberation_sans()?;
    let face = FontFace::from_bytes(data.clone(), 0)?;
    let font = Font::from_bytes(data, 0)?;
    // Four glyphs, each used for one cluster only, so no entry is taken already.
    let [a, b, c, d] = ['a', 'b', 'c', 'd'].map(|ch| glyph_of(&face, ch, 10.0).map(|(id, _)| id));
    let mut run = GlyphRun::new(&font, Pt(10.0), Pt(72.0), Pt(700.0));
    // U+1D400 takes two UTF-16 code units: 16 of them are 32 units, 17 are 34.
    let bold = |count: usize| "\u{1D400}".repeat(count);
    let cases = [
        (a?, "x".repeat(32), true),
        (b?, "x".repeat(33), false),
        (c?, bold(16), true),
        (d?, bold(17), false),
    ];
    for (i, (id, text, _)) in cases.iter().enumerate() {
        run.push_cluster(text, [glyph(*id, 10.0 * i as f64, 0.0)]);
    }
    let pdf = pdf_of(&[run])?;
    let utf16 = |text: &str| -> String {
        text.encode_utf16()
            .map(|unit| format!("{unit:04X}"))
            .collect()
    };
    for (_, text, mapped) in &cases {
        let entry = format!("> <{}>\n", utf16(text));
        let span = format!("/ActualText <FEFF{}>", utf16(text));
        assert_eq!(pdf.contains(&entry), *mapped, "{text}");
        assert_eq!(pdf.contains(&span), !*mapped, "{text}");
    }
    assert!(pdf.contains("2 beginbfchar"), "{pdf}");
    Ok(())
}

#[test]
fn a_cluster_text_must_fit_in_a_string() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    let run = |units: usize| {
        let mut run = GlyphRun::new(&font, Pt(10.0), Pt(72.0), Pt(700.0));
        run.push_cluster(&"x".repeat(units), [glyph(A, 0.0, 0.0)]);
        run
    };
    // 16382 code units, after the byte order mark, are 32766 bytes.
    let pdf = pdf_of(&[run(16382)])?;
    let (_, span) = pdf.split_once("/ActualText <").ok_or("no span")?;
    let (hex, _) = span.split_once('>').ok_or("no end")?;
    assert_eq!(hex.len() / 2, 32766);
    let mut doc = Document::new();
    let err = doc
        .add_page(PageSize::A4)?
        .draw_glyphs(&run(16383))
        .err()
        .ok_or("too long")?;
    assert_eq!(
        err.to_string(),
        "cluster 0 of the glyph run has 16383 UTF-16 code units of text, more than the 16382 \
         its replacement text can hold"
    );
    // The limit counts UTF-16 code units: U+1D400 takes two, so 8191 of them fit and
    // 8192 do not.
    let wide = |count: usize| {
        let mut run = GlyphRun::new(&font, Pt(10.0), Pt(72.0), Pt(700.0));
        run.push_cluster(&"\u{1D400}".repeat(count), [glyph(A, 0.0, 0.0)]);
        run
    };
    let pdf = pdf_of(&[wide(8191)])?;
    let (_, span) = pdf.split_once("/ActualText <").ok_or("no span")?;
    let (hex, _) = span.split_once('>').ok_or("no end")?;
    assert_eq!(hex.len() / 2, 32766);
    let mut doc = Document::new();
    let err = doc
        .add_page(PageSize::A4)?
        .draw_glyphs(&wide(8192))
        .err()
        .ok_or("too long")?;
    assert!(
        matches!(
            err,
            Error::ClusterTooLong {
                cluster: 0,
                units: 16384
            }
        ),
        "{err:?}"
    );
    Ok(())
}

#[test]
fn a_replaced_cluster_encloses_only_its_own_glyphs() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    let mut run = GlyphRun::new(&font, Pt(10.0), Pt(72.0), Pt(700.0));
    run.push_cluster("A", [glyph(A, 0.0, 0.0)]);
    run.push_cluster("AA", [glyph(A, 6.0, 0.0), glyph(A, 12.0, 0.0)]);
    run.push_cluster("A", [glyph(A, 18.0, 0.0)]);
    let pdf = pdf_of(&[run])?;
    assert_eq!(
        content(&pdf)?,
        "q\n0 0 0 rg\nBT\n/F1 10 Tf\n1 0 0 1 72 700 Tm\n[<0001>] TJ\n\
         /Span << /ActualText <FEFF00410041> >> BDC\n[<00010001>] TJ\nEMC\n[<0001>] TJ\nET\nQ\n"
    );
    Ok(())
}

#[test]
fn keeps_positions_to_a_ten_thousandth_of_a_point() -> TestResult {
    let data = liberation_sans()?;
    let face = FontFace::from_bytes(data.clone(), 0)?;
    let font = Font::from_bytes(data, 0)?;
    // Positions a few thousandths of a point off the advances, at 72 pt: adjustments of a
    // fraction of a thousandth of the em.
    let mut run = GlyphRun::new(&font, Pt(72.0), Pt(-32000.0), Pt(0.0));
    let mut x = 0.0;
    for (i, ch) in "Typewriter".chars().enumerate() {
        let (id, advance) = glyph_of(&face, ch, 72.0)?;
        run.push_cluster(ch.encode_utf8(&mut [0; 4]), [glyph(id, x, 0.0)]);
        x += advance + [0.002, -0.003, 0.0045][i % 3];
    }
    assert_positions_within(&[run], 1e-4)?;
    // Forty n's at 1000 pt, stretched by an awkward scale: the widths of the /W array
    // (four decimals of 556.15234375) and the scale written with Tz (123.4568) are not
    // exact, and the drift they would build up has to be corrected.
    let scale = 1.234_567_89;
    let (n, advance) = glyph_of(&face, 'n', 1000.0)?;
    let mut run =
        GlyphRun::new(&font, Pt(1000.0), Pt(-32000.0), Pt(0.0)).with_horizontal_scale(scale);
    for i in 0..40 {
        run.push_cluster("n", [glyph(n, f64::from(i) * advance * scale, 0.0)]);
    }
    assert_positions_within(&[run], 1e-3)?;
    Ok(())
}

#[test]
fn text_and_runs_in_one_font_share_its_tounicode_entries() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    let mut doc = Document::new();
    // Text first: the run's other text for the same glyph needs replacement text.
    let page = doc.add_page(PageSize::A4)?;
    page.add_text(Pt(72.0), Pt(720.0), &font, Pt(10.0), "A")?;
    let mut run = GlyphRun::new(&font, Pt(10.0), Pt(72.0), Pt(700.0));
    run.push_cluster("a", [glyph(A, 0.0, 0.0)]);
    run.push_cluster("A", [glyph(A, 6.0, 0.0)]);
    page.draw_glyphs(&run)?;
    // A run first: the text's character needs replacement text.
    let page = doc.add_page(PageSize::A4)?;
    let mut run = GlyphRun::new(&font, Pt(10.0), Pt(72.0), Pt(700.0));
    run.push_cluster("x", [glyph(A_ACUTE, 0.0, 0.0)]);
    page.draw_glyphs(&run)?;
    page.add_text(Pt(72.0), Pt(680.0), &font, Pt(10.0), "Á")?;
    let pdf = pdf_text(&mut doc)?;
    assert_eq!(pdf.matches("/Subtype /Type0").count(), 1);
    assert!(
        pdf.contains("2 beginbfchar\n<0001> <0041>\n<0002> <0078>\nendbfchar"),
        "{pdf}"
    );
    assert_eq!(pdf.matches("/ActualText").count(), 2, "{pdf}");
    assert!(pdf.contains("/ActualText <FEFF0061>"), "{pdf}");
    assert!(pdf.contains("/ActualText <FEFF00C1>"), "{pdf}");
    Ok(())
}

#[test]
fn an_empty_run_draws_nothing() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    let mut doc = Document::new();
    doc.add_page(PageSize::A4)?
        .draw_glyphs(&GlyphRun::new(&font, Pt(10.0), Pt(0.0), Pt(0.0)))?;
    let pdf = pdf_text(&mut doc)?;
    assert!(!pdf.contains("/Font"), "{pdf}");
    assert!(pdf.contains("/Length 0 >>"), "{pdf}");
    Ok(())
}

#[test]
fn rejects_runs_that_cannot_be_drawn_and_leaves_the_page_unchanged() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    let run = |size: f64, x: f64, y: f64| -> GlyphRun {
        let mut run = GlyphRun::new(&font, Pt(size), Pt(x), Pt(y));
        run.push_cluster("A", [glyph(A, 0.0, 0.0)]);
        run
    };
    let with_glyph = |placed: PlacedGlyph| -> GlyphRun {
        let mut run = run(10.0, 0.0, 0.0);
        run.push_cluster("x", [placed]);
        run
    };
    let mut empty_cluster = run(10.0, 0.0, 0.0);
    empty_cluster.push_cluster("x", []);
    let cases: Vec<(GlyphRun, &str)> = vec![
        (
            with_glyph(glyph(GlyphId::new(5), 0.0, 0.0)),
            "font OfficewrightTest-Regular has no glyph 5",
        ),
        (empty_cluster, "cluster 1 of the glyph run has no glyphs"),
        (
            run(0.0, 0.0, 0.0),
            "font size 0 pt is outside the 0.01 to 32767 pt range",
        ),
        (
            run(10.0, 32767.5, 0.0),
            "position (32767.5, 0) pt is outside the -32767 to 32767 pt range",
        ),
        (
            with_glyph(glyph(A, 0.0, f64::NAN)),
            "position (0, NaN) pt is outside the -32767 to 32767 pt range",
        ),
        (
            with_glyph(glyph(A, 40000.0, 0.0)),
            "position (40000, 0) pt is outside the -32767 to 32767 pt range",
        ),
        (
            run(10.0, 0.0, 0.0).with_horizontal_scale(0.0),
            "horizontal scale 0 is outside the 0.01 to 10 range",
        ),
        (
            run(10.0, 0.0, 0.0).with_horizontal_scale(10.5),
            "horizontal scale 10.5 is outside the 0.01 to 10 range",
        ),
        (
            run(10.0, 0.0, 0.0).with_skew(-1.5),
            "skew -1.5 is outside the -1 to 1 range",
        ),
        (
            run(10.0, 0.0, 0.0).with_skew(f64::INFINITY),
            "skew inf is outside the -1 to 1 range",
        ),
        (
            run(10.0, 0.0, 0.0).with_stroke(Pt(-0.5)),
            "stroke width -0.5 is outside the 0 to 32767 range",
        ),
        // The rise, a glyph's vertical offset, is written as it is, and so is the
        // origin's vertical position, even where the glyph's position is in range.
        (
            {
                let mut run = GlyphRun::new(&font, Pt(10.0), Pt(0.0), Pt(-32000.0));
                run.push_cluster("A", [glyph(A, 0.0, 40000.0)]);
                run
            },
            "position (0, 40000) pt is outside the -32767 to 32767 pt range",
        ),
        (
            {
                let mut run = GlyphRun::new(&font, Pt(10.0), Pt(0.0), Pt(40000.0));
                run.push_cluster("A", [glyph(A, 0.0, -10000.0)]);
                run
            },
            "position (0, 40000) pt is outside the -32767 to 32767 pt range",
        ),
    ];
    // A glyph inside the range on its own but outside it from the origin.
    let mut far = run(10.0, 32000.0, 0.0);
    far.push_cluster("x", [glyph(A, 1000.0, 0.0)]);
    let mut doc = Document::new();
    let page = doc.add_page(PageSize::A4)?;
    for (run, message) in cases {
        let err = page.draw_glyphs(&run).err().ok_or(message)?;
        assert_eq!(err.to_string(), message);
    }
    let err = page.draw_glyphs(&far).err().ok_or("far")?;
    assert!(matches!(err, Error::InvalidPosition { .. }), "{err:?}");
    // Nothing was drawn, and the font was not added.
    let pdf = pdf_text(&mut doc)?;
    assert!(!pdf.contains("/Font"), "{pdf}");
    // The extremes are accepted.
    let mut extremes = GlyphRun::new(&font, Font::MAX_SIZE, Pt(-32767.0), Pt(32767.0))
        .with_horizontal_scale(10.0)
        .with_skew(-1.0)
        .with_stroke(Pt(32767.0));
    extremes.push_cluster("A", [glyph(A, 32767.0, -32767.0)]);
    let mut doc = Document::new();
    doc.add_page(PageSize::A4)?.draw_glyphs(&extremes)?;
    Ok(())
}

#[test]
fn a_run_reports_what_it_was_built_with() -> TestResult {
    let font = Font::from_bytes(minimal()?, 0)?;
    let mut run = GlyphRun::new(&font, Pt(9.0), Pt(1.0), Pt(2.0))
        .with_color(Color::WHITE)
        .with_horizontal_scale(0.5)
        .with_skew(0.1)
        .with_stroke(Pt(0.3));
    assert_eq!(run.clusters().count(), 0);
    run.push_cluster("ÁA", [glyph(A_ACUTE, 0.0, 0.0), glyph(A, 3.0, 0.0)]);
    run.push_cluster("", [glyph(SPACE, 6.0, 0.0)]);
    assert_eq!(run.font(), &font);
    assert_eq!((run.size(), run.x(), run.y()), (Pt(9.0), Pt(1.0), Pt(2.0)));
    assert_eq!(run.color(), Color::WHITE);
    assert_eq!(run.horizontal_scale(), 0.5);
    assert_eq!(run.skew(), 0.1);
    assert_eq!(run.stroke(), Some(Pt(0.3)));
    assert_eq!(run.glyphs().len(), 3);
    let clusters: Vec<(&str, usize)> = run
        .clusters()
        .map(|(text, glyphs)| (text, glyphs.len()))
        .collect();
    assert_eq!(clusters, [("ÁA", 2), ("", 1)]);
    let defaults = GlyphRun::new(&font, Pt(9.0), Pt(1.0), Pt(2.0));
    assert_eq!(defaults.color(), Color::BLACK);
    assert_eq!(defaults.horizontal_scale(), 1.0);
    assert_eq!(defaults.skew(), 0.0);
    assert_eq!(defaults.stroke(), None);
    Ok(())
}
