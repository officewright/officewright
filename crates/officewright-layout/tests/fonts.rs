//! Fonts in the layout, with the test fonts of tests/fonts/ at the repository root, which
//! keep their own licenses (THIRD-PARTY-NOTICES.md) and are not part of this crate, so
//! this test is not in the published crate: line heights against Word's, and the font
//! each character of a paragraph is drawn in.
#![cfg(feature = "fuzzing")]

use std::path::{Path, PathBuf};

use officewright_core::Pt;
use officewright_font::{FontBook, FontFace};
use officewright_layout::fuzzing::{self, TypoMetrics};
use officewright_layout::ir::{Document, Inline, RunFonts, RunStyle};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn face(path: &str) -> TestResult<FontFace> {
    let data = std::fs::read(repository().join("tests/fonts").join(path))?;
    Ok(FontFace::from_bytes(data, 0)?)
}

/// Word's line pitch for each paragraph of `E-line-fonts.docx`, by the start of its
/// lines' text ("Calibri 11 pt"): the mean distance between the tops of its ten lines in
/// the reference PDF, from `geometry.json`.
fn word_pitches() -> TestResult<Vec<(String, f64)>> {
    let path = repository().join("tests/corpus/reference/word-rules/E-line-fonts/geometry.json");
    let json = std::fs::read_to_string(path)?;
    let mut lines = json.lines();
    let mut tops: Vec<(String, f64)> = Vec::new();
    while let Some(line) = lines.next() {
        let Some(text) = line.trim().strip_prefix("\"text\": \"") else {
            continue;
        };
        let Some((paragraph, _)) = text.split_once(", line ") else {
            continue;
        };
        // "box": [ then x, then the top.
        let _ = lines.next();
        let _ = lines.next();
        let top: f64 = lines
            .next()
            .ok_or("no top")?
            .trim()
            .trim_end_matches(',')
            .parse()?;
        tops.push((paragraph.to_owned(), top));
    }
    let mut pitches: Vec<(String, f64)> = Vec::new();
    for (name, _) in &tops {
        if pitches.iter().any(|(known, _)| known == name) {
            continue;
        }
        let paragraph: Vec<f64> = tops
            .iter()
            .filter(|(other, _)| other == name)
            .map(|(_, top)| *top)
            .collect();
        let (Some(first), Some(last)) = (paragraph.first(), paragraph.last()) else {
            continue;
        };
        pitches.push((name.clone(), (last - first) / (paragraph.len() - 1) as f64));
    }
    Ok(pitches)
}

#[test]
fn line_heights_are_words_for_the_metric_twins() -> TestResult {
    let pitches = word_pitches()?;
    let twins = [
        ("Calibri", "carlito/Carlito-Regular.ttf"),
        ("Arial", "liberation/LiberationSans-Regular.ttf"),
        ("Times New Roman", "liberation/LiberationSerif-Regular.ttf"),
        ("Courier New", "liberation/LiberationMono-Regular.ttf"),
        ("Liberation Sans", "liberation/LiberationSans-Regular.ttf"),
        // A CFF font that sets USE_TYPO_METRICS: Word takes its Windows metrics.
        ("Noto Sans", "noto/NotoSans-Regular.otf"),
    ];
    let mut checked = 0;
    for (word_font, file) in twins {
        let face = face(file)?;
        for size in [11.0, 12.0] {
            let name = format!("{word_font} {size} pt");
            let (_, word) = pitches
                .iter()
                .find(|(paragraph, _)| *paragraph == name)
                .ok_or_else(|| format!("no paragraph {name}"))?;
            let ours = fuzzing::line_metrics(&face, size, TypoMetrics::default()).height;
            // pdftotext gives the tops to about 0.12 pt; the mean of nine steps is good
            // to about 0.02 pt.
            assert!(
                (ours - word).abs() < 0.02,
                "{name}: ours {ours:.4}, Word {word:.4}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 12);
    Ok(())
}

#[test]
fn use_typo_metrics_counts_for_truetype_fonts_only() -> TestResult {
    // Noto Sans (CFF) sets the flag; its typographic line is 11 % shorter.
    let noto = face("noto/NotoSans-Regular.otf")?;
    let windows = fuzzing::line_metrics(&noto, 11.0, TypoMetrics::default()).height;
    let typo = fuzzing::line_metrics(&noto, 11.0, TypoMetrics::Always).height;
    assert!((windows - 16.709).abs() < 0.01, "{windows}");
    assert!((typo - 14.982).abs() < 0.01, "{typo}");
    // Liberation Sans (TrueType) with the flag set takes its typographic metrics.
    let mut data =
        std::fs::read(repository().join("tests/fonts/liberation/LiberationSans-Regular.ttf"))?;
    set_use_typo_metrics(&mut data)?;
    let flagged = FontFace::from_bytes(data, 0)?;
    let plain = face("liberation/LiberationSans-Regular.ttf")?;
    let ours = fuzzing::line_metrics(&flagged, 11.0, TypoMetrics::default());
    let typo = fuzzing::line_metrics(&flagged, 11.0, TypoMetrics::Always);
    let windows = fuzzing::line_metrics(&plain, 11.0, TypoMetrics::default());
    assert_eq!(ours, typo);
    assert_ne!(ours, windows);
    Ok(())
}

/// Sets `USE_TYPO_METRICS` in a single font's `OS/2` table.
fn set_use_typo_metrics(font: &mut [u8]) -> TestResult {
    let u16_at = |at: usize| -> Option<u16> {
        Some(u16::from_be_bytes(font.get(at..at + 2)?.try_into().ok()?))
    };
    let tables = usize::from(u16_at(4).ok_or("no table count")?);
    let record = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&record| font.get(record..record + 4) == Some(&b"OS/2"[..]))
        .ok_or("no OS/2")?;
    let os2 = u32::from_be_bytes(
        font.get(record + 8..record + 12)
            .ok_or("short record")?
            .try_into()?,
    );
    let selection = font
        .get_mut(usize::try_from(os2)? + 63)
        .ok_or("short OS/2")?;
    *selection |= 0x80;
    Ok(())
}

fn book() -> TestResult<FontBook> {
    let mut book = FontBook::new();
    book.add_dir(repository().join("tests/fonts"))?;
    Ok(book)
}

/// The family of each run of a paragraph of `text` in `style`, with the run's text.
fn families(
    book: &FontBook,
    style: RunStyle,
    inlines: impl Fn(officewright_layout::ir::RunStyleId) -> Vec<Inline>,
) -> Vec<(String, String)> {
    let mut document = Document::default();
    let id = document.add_run_style(style);
    let paragraph = fuzzing::paragraph(book, &document, &inlines(id));
    paragraph
        .runs()
        .iter()
        .map(|run| {
            (
                run.text().to_owned(),
                run.face().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[test]
fn draws_each_character_in_a_font_that_has_it() -> TestResult {
    let book = book()?;
    let calibri = RunStyle::default().with_fonts(RunFonts::new("Calibri"));
    // Calibri is drawn in Carlito; Ԕ (Komi Lje) is not in Carlito, but in Noto Sans; 中
    // is in no test font.
    let runs = families(&book, calibri.clone(), |id| {
        vec![Inline::text("Добрый день Ԕ", id)]
    });
    assert_eq!(
        runs,
        [
            ("Добрый день ".to_owned(), "Carlito".to_owned()),
            ("Ԕ".to_owned(), "Noto Sans".to_owned()),
        ]
    );
    let mut document = Document::default();
    let id = document.add_run_style(calibri);
    let paragraph = fuzzing::paragraph(&book, &document, &[Inline::text("a\u{4E2D}", id)]);
    let messages: Vec<&str> = paragraph
        .report()
        .iter()
        .map(|entry| entry.message())
        .collect();
    assert!(
        messages.contains(&"no font has U+4E2D '中'; the missing-glyph shape is drawn"),
        "{messages:?}"
    );
    assert!(
        messages.contains(&"\"Calibri\" → \"Carlito\" (metric-compatible substitute)"),
        "{messages:?}"
    );
    Ok(())
}

#[test]
fn draws_list_bullets_of_missing_symbol_fonts() -> TestResult {
    let book = book()?;
    let calibri = RunStyle::default().with_fonts(RunFonts::new("Calibri"));
    let runs = families(&book, calibri, |id| {
        vec![
            Inline::symbol("Symbol", 0xF0B7, id),
            Inline::symbol("Wingdings", 0xF0A7, id),
            Inline::symbol("Wingdings", 0xF0FC, id),
        ]
    });
    let text: String = runs.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(text, "\u{2022}\u{25AA}\u{2713}");
    // Every bullet comes from a font that has it.
    assert!(
        runs.iter().all(|(_, family)| !family.is_empty()),
        "{runs:?}"
    );
    Ok(())
}

#[test]
fn every_paragraph_keeps_what_the_layout_relies_on() -> TestResult {
    let book = book()?;
    let texts = [
        "The quick brown fox jumps over the lazy dog. ffi fl",
        "Съешь же ещё этих мягких французских булок, да выпей чаю.",
        "Ξεσκεπάζω την ψυχοφθόρα βδελυγμία.",
        "مرحبا بالعالم",
        "漢字かなカナ 한국어",
        "e\u{301} a\u{308}\u{332} \u{1F600} \u{200D}\u{FE0F}",
    ];
    let styles = [
        RunStyle::default().with_fonts(RunFonts::new("Calibri")),
        RunStyle::default()
            .with_fonts(RunFonts::new("Times New Roman"))
            .with_bold(true)
            .with_caps(true),
        RunStyle::default()
            .with_fonts(RunFonts::new("Courier New"))
            .with_small_caps(true)
            .with_size(Pt(9.0)),
        RunStyle::default()
            .with_fonts(RunFonts::new("Arial"))
            .with_complex_script(true),
    ];
    for style in styles {
        let mut document = Document::default();
        let id = document.add_run_style(style);
        for text in texts {
            fuzzing::check_paragraph(&book, &document, &[Inline::text(text, id)])
                .map_err(|err| format!("{text}: {err}"))?;
        }
    }
    Ok(())
}
