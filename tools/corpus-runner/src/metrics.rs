//! The render metrics of one document, our PDF against Word's reference: page counts, text,
//! line endings, word positions, page breaks and the SSIM of every page.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use crate::align;
use crate::geometry::{Geometry, Page};
use crate::raster::{self, Grey};
use crate::ssim;
use crate::text;

/// The distance, in points, of a word that is on another page than in Word's rendering:
/// more than any position criterion allows.
pub const OFF_PAGE: f64 = 1000.0;

/// The resolution of the reference rasters, and of ours.
const DPI: &str = "110";

/// The metrics of one document; fractions are 1 when there is nothing to count.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scores {
    pub pages: usize,
    pub ref_pages: usize,
    /// Per page, the Levenshtein similarity of the normalised text; the mean over the pages
    /// of the longer side, a page missing on one side scoring 0.
    pub text_score: f64,
    /// Reference lines whose first and last words begin and end one of our lines.
    pub lines: usize,
    pub lines_agreeing: usize,
    pub line_agreement: f64,
    /// Reference words, and those matched to ours in order by their text.
    pub words: usize,
    pub words_matched: usize,
    /// Differences of the matched words' left and top edges, in points.
    pub dx_median: Option<f64>,
    pub dx_p95: Option<f64>,
    pub dy_median: Option<f64>,
    pub dy_p95: Option<f64>,
    /// Word's page breaks (the first word of each page after the first), and those before
    /// which we break a page too.
    pub page_breaks: usize,
    pub page_breaks_agreeing: usize,
    pub page_break_agreement: Option<f64>,
    /// SSIM per page of the longer side, a page missing on one side scoring 0.
    pub ssim: Vec<f64>,
    pub ssim_mean: f64,
    pub ssim_min: f64,
    /// `qpdf --check` passed without warnings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf_valid: Option<bool>,
    /// The fonts our PDF uses without embedding them, as `pdffonts` lists them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fonts_not_embedded: Option<Vec<String>>,
    /// The fonts Word used, from the reference's `formatting.json`; `(unresolved)` for a
    /// theme font make-reference.ps1 could not name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_fonts: Option<Vec<String>>,
}

/// The pages of one rendering: its words with their boxes, and its 110 dpi grey rasters.
#[derive(Debug)]
pub struct Rendering {
    pub geometry: Geometry,
    pub rasters: Vec<Grey>,
}

impl Rendering {
    /// A reference folder of `tests/corpus/reference/`: `geometry.json` and
    /// `page-001.png`, `page-002.png` and so on, one per page.
    pub fn reference(folder: &Path) -> Result<Self, String> {
        let read = |name: &str| {
            std::fs::read(folder.join(name))
                .map_err(|error| format!("{}: {error}", folder.join(name).display()))
        };
        let json = read("geometry.json")?;
        let geometry = Geometry::from_reference(&String::from_utf8_lossy(&json))?;
        let rasters = (1..=geometry.pages.len())
            .map(|page| {
                let name = format!("page-{page:03}.png");
                raster::read_png(&read(&name)?).map_err(|error| format!("{name}: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Rendering { geometry, rasters })
    }

    /// A PDF, through `pdftotext -bbox-layout` and `pdftoppm -gray -r 110`, as the
    /// references were made.
    pub fn pdf(pdf: &Path) -> Result<Self, String> {
        let bbox = tool(
            Command::new("pdftotext")
                .arg("-bbox-layout")
                .arg(pdf)
                .arg("-"),
        )?;
        let geometry = Geometry::from_bbox(&String::from_utf8_lossy(&bbox))?;
        let folder = Scratch::new()?;
        let prefix = folder.path.join("page");
        tool(
            Command::new("pdftoppm")
                .args(["-gray", "-r", DPI])
                .arg(pdf)
                .arg(&prefix),
        )?;
        // pdftoppm numbers the pages page-1.pgm or page-01.pgm, after the page count.
        let mut pages: Vec<(usize, PathBuf)> = std::fs::read_dir(&folder.path)
            .map_err(|error| format!("{}: {error}", folder.path.display()))?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let number = name
                    .strip_prefix("page-")?
                    .strip_suffix(".pgm")?
                    .parse()
                    .ok()?;
                Some((number, entry.path()))
            })
            .collect();
        pages.sort();
        let rasters = pages
            .iter()
            .map(|(_, path)| {
                let data =
                    std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
                raster::read_pgm(&data)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Rendering { geometry, rasters })
    }
}

/// Runs a poppler tool and returns what it printed.
fn tool(command: &mut Command) -> Result<Vec<u8>, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .output()
        .map_err(|error| format!("cannot run {program}: {error} (poppler-utils)"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{program} failed: {}", stderr.trim()));
    }
    Ok(output.stdout)
}

/// A temporary folder, removed when dropped.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self, String> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "corpus-runner-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Scratch { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Failing to remove a temporary folder loses nothing.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A word of a rendering in reading order: its page, its line (numbered through the
/// document), whether it begins its page or begins or ends its line, and its edges.
struct Placed {
    page: usize,
    line: usize,
    first_on_page: bool,
    first: bool,
    last: bool,
    x: f64,
    y: f64,
    text: String,
}

fn placed(pages: &[Page]) -> Vec<Placed> {
    let mut out = Vec::new();
    let mut line_number = 0;
    for (page, content) in pages.iter().enumerate() {
        let mut first_on_page = true;
        for line in &content.lines {
            let count = line.words.len();
            for (index, word) in line.words.iter().enumerate() {
                out.push(Placed {
                    page,
                    line: line_number,
                    first_on_page,
                    first: index == 0,
                    last: index + 1 == count,
                    x: word.x_min,
                    y: word.y_min,
                    text: text::normalise(&word.text),
                });
                first_on_page = false;
            }
            line_number += 1;
        }
    }
    out
}

fn page_text(page: Option<&Page>) -> Option<String> {
    let page = page?;
    let words: Vec<&str> = page
        .lines
        .iter()
        .flat_map(|line| line.words.iter().map(|word| word.text.as_str()))
        .collect();
    Some(text::normalise(&words.join(" ")))
}

/// Our rendering against the reference.
pub fn score(ours: &Rendering, reference: &Rendering) -> Scores {
    let (pages, ref_pages) = (ours.geometry.pages.len(), reference.geometry.pages.len());
    let longer = pages.max(ref_pages);

    let text_score = mean((0..longer).map(|page| {
        match (
            page_text(reference.geometry.pages.get(page)),
            page_text(ours.geometry.pages.get(page)),
        ) {
            (Some(a), Some(b)) => text::similarity(&a, &b),
            _ => 0.0,
        }
    }));

    let theirs = placed(&reference.geometry.pages);
    let mine = placed(&ours.geometry.pages);
    let theirs_text: Vec<&str> = theirs.iter().map(|word| word.text.as_str()).collect();
    let mine_text: Vec<&str> = mine.iter().map(|word| word.text.as_str()).collect();
    let mut matched: Vec<Option<&Placed>> = (0..theirs.len()).map(|_| None).collect();
    for (i, j) in align::matches(&theirs_text, &mine_text) {
        if let Some(slot) = matched.get_mut(i) {
            *slot = mine.get(j);
        }
    }

    // A reference line agrees when its first word begins one of our lines and its last word
    // ends the same line.
    let mut lines = 0;
    let mut lines_agreeing = 0;
    let mut first_of_line: Option<Option<&Placed>> = None;
    for (word, ours) in theirs.iter().zip(&matched) {
        if word.first {
            first_of_line = Some(*ours);
        }
        if word.last {
            lines += 1;
            if let (Some(Some(start)), Some(end)) = (first_of_line, ours) {
                if start.first && end.last && start.line == end.line {
                    lines_agreeing += 1;
                }
            }
        }
    }

    let (mut dx, mut dy) = (Vec::new(), Vec::new());
    for (word, ours) in theirs.iter().zip(&matched) {
        if let Some(ours) = ours {
            if ours.page == word.page {
                dx.push((ours.x - word.x).abs());
                dy.push((ours.y - word.y).abs());
            } else {
                dx.push(OFF_PAGE);
                dy.push(OFF_PAGE);
            }
        }
    }

    // Word's page breaks: the first word of every page after the first that has words.
    let mut page_breaks = 0;
    let mut page_breaks_agreeing = 0;
    let mut previous_page = None;
    for (word, ours) in theirs.iter().zip(&matched) {
        if previous_page != Some(word.page) {
            if previous_page.is_some() {
                page_breaks += 1;
                if ours.is_some_and(|ours| ours.page > 0 && ours.first_on_page) {
                    page_breaks_agreeing += 1;
                }
            }
            previous_page = Some(word.page);
        }
    }

    let ssim: Vec<f64> = (0..longer)
        .map(
            |page| match (reference.rasters.get(page), ours.rasters.get(page)) {
                (Some(a), Some(b)) => round(ssim::ssim(a, b)),
                _ => 0.0,
            },
        )
        .collect();

    let fraction = |part: usize, whole: usize| {
        if whole == 0 {
            1.0
        } else {
            round(part as f64 / whole as f64)
        }
    };
    Scores {
        pages,
        ref_pages,
        text_score: round(text_score),
        lines,
        lines_agreeing,
        line_agreement: fraction(lines_agreeing, lines),
        words: theirs.len(),
        words_matched: dx.len(),
        dx_median: percentile(&mut dx.clone(), 50),
        dx_p95: percentile(&mut dx, 95),
        dy_median: percentile(&mut dy.clone(), 50),
        dy_p95: percentile(&mut dy, 95),
        page_breaks,
        page_breaks_agreeing,
        page_break_agreement: (page_breaks > 0)
            .then(|| fraction(page_breaks_agreeing, page_breaks)),
        ssim_mean: round(mean(ssim.iter().copied())),
        ssim_min: ssim.iter().copied().reduce(f64::min).unwrap_or(1.0),
        ssim,
        pdf_valid: None,
        fonts_not_embedded: None,
        word_fonts: None,
    }
}

/// Whether `qpdf --check` passes without warnings, and the fonts `pdffonts` lists as not
/// embedded.
pub fn check_pdf(pdf: &Path) -> Result<(bool, Vec<String>), String> {
    let output = Command::new("qpdf")
        .arg("--check")
        .arg(pdf)
        .output()
        .map_err(|error| format!("cannot run qpdf: {error}"))?;
    let said = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    let valid = output.status.success() && !said.contains("WARNING");
    let listing = tool(Command::new("pdffonts").arg(pdf))?;
    Ok((valid, not_embedded(&String::from_utf8_lossy(&listing))))
}

/// The names in the rows of a `pdffonts` listing whose `emb` column says no.
fn not_embedded(listing: &str) -> Vec<String> {
    let mut lines = listing.lines();
    let Some(column) = lines
        .next()
        .and_then(|header| header.find(" emb "))
        .map(|at| at + 1)
    else {
        return Vec::new();
    };
    lines
        .skip(1)
        .filter(|line| {
            line.get(column..column + 3)
                .is_some_and(|emb| emb.trim() == "no")
        })
        .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
        .collect()
}

/// The fonts Word used for the document, from its reference's `formatting.json` (written
/// by make-reference.ps1 format 3 on); `None` for an older reference.
pub fn word_fonts(reference: &Path) -> Result<Option<Vec<String>>, String> {
    let path = reference.join("formatting.json");
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let fonts = value
        .get("fonts")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{}: no fonts table", path.display()))?;
    let mut names: Vec<String> = fonts
        .iter()
        .map(|font| {
            font.get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(unresolved)")
                .to_owned()
        })
        .collect();
    names.sort();
    names.dedup();
    Ok(Some(names))
}

/// The mean, 1 for no values.
fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, count) = values.fold((0.0, 0usize), |(sum, count), value| {
        (sum + value, count + 1)
    });
    if count == 0 { 1.0 } else { sum / count as f64 }
}

/// The nearest-rank percentile, rounded to 0.001 pt; `None` for no values.
fn percentile(values: &mut [f64], rank: usize) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let index = (rank * values.len()).div_ceil(100).max(1) - 1;
    values
        .get(index)
        .map(|value| (value * 1000.0).round() / 1000.0)
}

fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Line, Word};

    /// A line of (word, x, y).
    type Words<'a> = &'a [(&'a str, f64, f64)];

    /// A rendering from pages of lines of words.
    fn rendering(pages: &[&[Words]]) -> Rendering {
        let geometry = Geometry {
            pages: pages
                .iter()
                .map(|lines| Page {
                    lines: lines
                        .iter()
                        .map(|words| Line {
                            words: words
                                .iter()
                                .map(|&(text, x, y)| Word {
                                    text: text.to_owned(),
                                    x_min: x,
                                    y_min: y,
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        };
        let blank = Grey {
            width: 20,
            height: 20,
            pixels: vec![255; 400],
        };
        Rendering {
            rasters: vec![blank; pages.len()],
            geometry,
        }
    }

    const WORD: &[(&str, f64, f64)] = &[("Lorem", 72.0, 100.0), ("ipsum", 110.0, 100.0)];
    const LINE_2: &[(&str, f64, f64)] = &[
        ("dolor", 72.0, 114.0),
        ("sit", 105.0, 114.0),
        ("amet.", 125.0, 114.0),
    ];
    const PAGE_2: &[(&str, f64, f64)] = &[("Second", 72.0, 72.0), ("page", 120.0, 72.0)];

    #[test]
    fn the_same_rendering_scores_perfectly() {
        let word = rendering(&[&[WORD, LINE_2], &[PAGE_2]]);
        let scores = score(&word, &word);
        assert_eq!((scores.pages, scores.ref_pages), (2, 2));
        assert_eq!(scores.text_score, 1.0);
        assert_eq!(
            (scores.lines, scores.lines_agreeing, scores.line_agreement),
            (3, 3, 1.0)
        );
        assert_eq!((scores.words, scores.words_matched), (7, 7));
        assert_eq!((scores.dx_median, scores.dy_p95), (Some(0.0), Some(0.0)));
        assert_eq!(
            (scores.page_breaks, scores.page_break_agreement),
            (1, Some(1.0))
        );
        assert_eq!(
            (scores.ssim.clone(), scores.ssim_mean, scores.ssim_min),
            (vec![1.0, 1.0], 1.0, 1.0)
        );
    }

    #[test]
    fn a_line_broken_elsewhere_and_a_page_break_moved() {
        let word = rendering(&[&[WORD, LINE_2], &[PAGE_2]]);
        // "dolor" moved up to the first line; the second page starts one line earlier.
        let ours = rendering(&[
            &[&[
                ("Lorem", 72.0, 100.5),
                ("ipsum", 110.0, 100.5),
                ("dolor", 150.0, 100.5),
            ]],
            &[&[("sit", 72.0, 72.0), ("amet.", 92.0, 72.0)], PAGE_2],
        ]);
        let scores = score(&ours, &word);
        assert_eq!(
            (scores.lines, scores.lines_agreeing),
            (3, 1),
            "only the last line agrees"
        );
        assert_eq!(scores.words_matched, 7);
        // "sit" and "amet." moved to another page: off by OFF_PAGE.
        assert_eq!(scores.dy_p95, Some(OFF_PAGE));
        assert_eq!(scores.dy_median, Some(0.5));
        assert_eq!((scores.page_breaks, scores.page_breaks_agreeing), (1, 0));
        assert!(
            scores.text_score < 1.0 && scores.text_score > 0.5,
            "{}",
            scores.text_score
        );
    }

    #[test]
    fn missing_pages_score_zero() {
        let word = rendering(&[&[WORD], &[PAGE_2]]);
        let ours = rendering(&[&[WORD]]);
        let scores = score(&ours, &word);
        assert_eq!((scores.pages, scores.ref_pages), (1, 2));
        assert_eq!(scores.text_score, 0.5);
        assert_eq!(scores.ssim, vec![1.0, 0.0]);
        assert_eq!((scores.ssim_mean, scores.ssim_min), (0.5, 0.0));
        assert_eq!((scores.words, scores.words_matched), (4, 2));
        assert_eq!(scores.page_breaks_agreeing, 0);
    }

    #[test]
    fn nothing_to_count_is_not_a_failure() {
        let empty = rendering(&[&[]]);
        let scores = score(&empty, &empty);
        assert_eq!(
            (scores.line_agreement, scores.text_score, scores.ssim_mean),
            (1.0, 1.0, 1.0)
        );
        assert_eq!(
            (scores.dx_median, scores.page_break_agreement),
            (None, None)
        );
    }

    /// A two-page PDF of ours, its lines `shift` points lower than in the reference's.
    #[cfg(unix)]
    fn pdf(path: &Path, shift: f64) {
        use officewright_pdf::{Document, Font, PageSize, Pt, StandardFont};
        let helvetica = Font::standard(StandardFont::Helvetica);
        let mut doc = Document::new();
        let page = doc.add_page(PageSize::A4).unwrap();
        for (line, text) in [
            "Lorem ipsum dolor sit amet,",
            "consectetur adipiscing elit.",
        ]
        .iter()
        .enumerate()
        {
            page.add_text(
                Pt(72.0),
                Pt(770.0 - 14.0 * line as f64 - shift),
                &helvetica,
                Pt(12.0),
                text,
            )
            .unwrap();
        }
        doc.add_page(PageSize::A4)
            .unwrap()
            .add_text(
                Pt(72.0),
                Pt(770.0 - shift),
                &helvetica,
                Pt(12.0),
                "Second page",
            )
            .unwrap();
        std::fs::write(path, doc.to_bytes().unwrap()).unwrap();
    }

    /// A reference folder of the rendering, laid out as `scripts/corpus-import.py` does.
    #[cfg(unix)]
    fn reference_folder(folder: &Path, rendering: &Rendering) {
        std::fs::create_dir_all(folder).unwrap();
        let pages: Vec<serde_json::Value> = rendering
            .geometry
            .pages
            .iter()
            .map(|page| {
                let lines: Vec<serde_json::Value> = page
                    .lines
                    .iter()
                    .map(|line| {
                        let words: Vec<serde_json::Value> = line
                            .words
                            .iter()
                            .map(|w| serde_json::json!({"text": w.text, "box": [w.x_min, w.y_min, w.x_min + 1.0, w.y_min + 1.0]}))
                            .collect();
                        serde_json::json!({"text": "", "box": [0, 0, 0, 0], "words": words})
                    })
                    .collect();
                serde_json::json!({"width": 595.28, "height": 841.89, "lines": lines})
            })
            .collect();
        let json = serde_json::json!({"format": 1, "document": "x.docx", "page_count": pages.len(), "pages": pages});
        std::fs::write(folder.join("geometry.json"), json.to_string()).unwrap();
        for (index, raster) in rendering.rasters.iter().enumerate() {
            std::fs::write(
                folder.join(format!("page-{:03}.png", index + 1)),
                raster::encode_png(raster),
            )
            .unwrap();
        }
    }

    /// Through pdftotext and pdftoppm, which check.sh requires on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn a_pdf_is_scored_against_a_reference_folder() {
        let scratch = Scratch::new().unwrap();
        let (word_pdf, ours_pdf) = (scratch.path.join("word.pdf"), scratch.path.join("ours.pdf"));
        pdf(&word_pdf, 0.0);
        pdf(&ours_pdf, 3.0);
        let word = Rendering::pdf(&word_pdf).unwrap();
        assert_eq!((word.geometry.pages.len(), word.rasters.len()), (2, 2));
        assert_eq!(word.geometry.pages[0].lines[1].words[0].text, "consectetur");
        // A4 at 110 dpi.
        assert!(
            (909..=910).contains(&word.rasters[0].width)
                && (1286..=1287).contains(&word.rasters[0].height)
        );
        reference_folder(&scratch.path.join("reference"), &word);
        let reference = Rendering::reference(&scratch.path.join("reference")).unwrap();
        assert_eq!(reference.geometry, word.geometry);

        let same = score(&Rendering::pdf(&word_pdf).unwrap(), &reference);
        assert_eq!(
            (same.text_score, same.line_agreement, same.ssim_mean),
            (1.0, 1.0, 1.0)
        );
        assert_eq!(
            (same.dy_p95, same.page_break_agreement),
            (Some(0.0), Some(1.0))
        );

        let shifted = score(&Rendering::pdf(&ours_pdf).unwrap(), &reference);
        assert_eq!((shifted.text_score, shifted.line_agreement), (1.0, 1.0));
        assert_eq!((shifted.dy_median, shifted.dx_p95), (Some(3.0), Some(0.0)));
        // Two lines on a white page: a shift of 3 pt costs SSIM little; positions show it.
        assert!(shifted.ssim_min < 0.999, "{:?}", shifted.ssim);

        std::fs::remove_file(scratch.path.join("reference/page-002.png")).unwrap();
        assert!(
            Rendering::reference(&scratch.path.join("reference"))
                .unwrap_err()
                .contains("page-002.png")
        );
        assert!(
            Rendering::pdf(&scratch.path.join("missing.pdf"))
                .unwrap_err()
                .contains("pdftotext failed")
        );
    }

    #[test]
    fn fonts_not_embedded_come_from_the_emb_column() {
        let listing = "name                                 type              encoding         emb sub uni object ID\n\
------------------------------------ ----------------- ---------------- --- --- --- ---------\n\
ABCDEF+Carlito                       CID TrueType      Identity-H       yes yes yes      9  0\n\
Helvetica                            Type 1            WinAnsi          no  no  no       5  0\n\
TimesNewRomanPSMT                    TrueType          WinAnsi          no  no  yes     12  0\n";
        assert_eq!(not_embedded(listing), ["Helvetica", "TimesNewRomanPSMT"]);
        assert!(not_embedded("").is_empty());
    }

    #[test]
    fn word_fonts_come_from_the_formatting_export() {
        let scratch = Scratch::new().unwrap();
        assert_eq!(word_fonts(&scratch.path).unwrap(), None);
        std::fs::write(
            scratch.path.join("formatting.json"),
            r#"{"fonts": [{"name": "Calibri"}, {"name": null, "theme": "unresolved"}, {"name": "Calibri", "bold": true}]}"#,
        )
        .unwrap();
        assert_eq!(
            word_fonts(&scratch.path).unwrap(),
            Some(vec!["(unresolved)".to_owned(), "Calibri".to_owned()])
        );
        std::fs::write(scratch.path.join("formatting.json"), "{}").unwrap();
        assert!(word_fonts(&scratch.path).is_err());
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let mut values: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&mut values.clone(), 50), Some(10.0));
        assert_eq!(percentile(&mut values, 95), Some(19.0));
        assert_eq!(percentile(&mut [0.12345], 95), Some(0.123));
        assert_eq!(percentile(&mut [], 50), None);
    }
}
