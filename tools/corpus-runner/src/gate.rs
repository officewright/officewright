//! `corpus-runner gate`: the release criteria of Milestone 4 over a run's report, printed as
//! a markdown table with the target, the value on main and the value on the branch.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::content;
use crate::features;
use crate::metrics::Scores;
use crate::report::{FileResult, Report, Status};

/// The fonts of `tests/fonts`, which Word had too when it rendered the references: a
/// document in these only is laid out with Word's fonts (class S).
const SAME_FONTS: [&str; 5] = [
    "Carlito",
    "Liberation Mono",
    "Liberation Sans",
    "Liberation Serif",
    "Noto Sans",
];

/// Fonts whose metric twin among `SAME_FONTS` Word's renders confirmed (class T).
const TWINNED: [&str; 4] = ["Arial", "Calibri", "Courier New", "Times New Roman"];

/// How a document's fonts compare with Word's: the same (S), confirmed metric twins (T),
/// neither (N), or not known: its reference has no formatting export, or an incomplete one
/// (without the runs, or cut short) in which every font named is S or T.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Same,
    Twin,
    Other,
    Unknown,
}

pub fn class(scores: &Scores, same_fonts: bool) -> Class {
    if same_fonts {
        // The release run lays out with the fonts Word used.
        return Class::Same;
    }
    let Some(fonts) = &scores.word_fonts else {
        return Class::Unknown;
    };
    let same = |font: &String| SAME_FONTS.contains(&font.as_str());
    if !fonts
        .iter()
        .all(|font| same(font) || TWINNED.contains(&font.as_str()))
    {
        // Fonts the export left out cannot take this back.
        Class::Other
    } else if scores.word_fonts_complete != Some(true) {
        // The runs, or paragraphs after the time budget, may hold a font of class N.
        Class::Unknown
    } else if fonts.iter().all(same) {
        Class::Same
    } else {
        Class::Twin
    }
}

/// A number to compare with main, whether more of it is better, and by how much it may
/// move before it counts as worse (timings and memory vary from run to run).
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    pub value: f64,
    pub higher_is_better: bool,
    pub tolerance: f64,
}

impl Key {
    fn higher(value: f64) -> Self {
        Key {
            value,
            higher_is_better: true,
            tolerance: 0.0,
        }
    }

    fn lower(value: f64) -> Self {
        Key {
            value,
            higher_is_better: false,
            tolerance: 0.0,
        }
    }

    fn count(value: usize) -> Self {
        Key::lower(value as f64)
    }

    fn worse_than(&self, before: &Key) -> bool {
        if self.higher_is_better {
            self.value < before.value - self.tolerance
        } else {
            self.value > before.value + self.tolerance
        }
    }
}

/// One criterion's value on one report.
#[derive(Clone, Debug, PartialEq)]
pub struct Measure {
    pub text: String,
    /// Whether the target is met; `None` when there was nothing to measure.
    pub pass: Option<bool>,
    /// The numbers compared with main: the branch is worse when any of them is.
    pub keys: Vec<Key>,
}

impl Measure {
    fn counted(text: String, failures: usize) -> Self {
        Measure {
            text,
            pass: Some(failures == 0),
            keys: vec![Key::count(failures)],
        }
    }

    fn unmeasured(text: &str) -> Self {
        Measure {
            text: text.to_owned(),
            pass: None,
            keys: Vec::new(),
        }
    }

    /// Worse than main: a number worse beyond its tolerance, or nothing measured where main
    /// measured something.
    pub fn worse_than(&self, before: &Measure) -> bool {
        if self.keys.is_empty() {
            return !before.keys.is_empty();
        }
        self.keys
            .iter()
            .zip(&before.keys)
            .any(|(after, before)| after.worse_than(before))
    }
}

/// The criteria: number, name, target and to which documents it applies.
pub const CRITERIA: [(&str, &str, &str); 12] = [
    (
        "Crashes",
        "0 panics, time-outs, memory failures",
        "every file",
    ),
    (
        "Refusals",
        "every error expected; every reference document opened and scored",
        "every file",
    ),
    (
        "Valid output",
        "`qpdf --check` clean, every font embedded",
        "every scored PDF",
    ),
    (
        "Page count",
        "≥ 95 % as Word's, all within ±1, `E-*` 100 %",
        "S, T",
    ),
    (
        "`text_score`",
        "≥ 0.99 per document",
        "every scored document",
    ),
    (
        "`line_agreement`",
        "≥ 0.98 overall, ≥ 0.95 per document, `E-*` 1.0",
        "S, T",
    ),
    (
        "Positions",
        "dx, dy median ≤ 0.5 pt and p95 ≤ 2 pt per document",
        "S, T",
    ),
    ("`page_break_agreement`", "≥ 0.97 overall", "S, T"),
    (
        "SSIM",
        "pages ≥ 0.90 and mean ≥ 0.95 (S); 0.88 and 0.93 (T)",
        "S, T",
    ),
    (
        "Speed and memory",
        "every document < 5 s, peak < 200 MB",
        "every file",
    ),
    (
        "Report",
        "every font of Word's text drawn or its substitution reported; all content the \
         layout does not lay out reported",
        "fonts: reference documents; content: every rendered document",
    ),
    (
        "Required features",
        "no entry degrading a required feature",
        "reference documents",
    ),
];

/// The largest peak memory criterion 10 allows, in bytes (200 MB).
const MEMORY_LIMIT: u64 = 200_000_000;

/// The time criterion 10 allows a document, in milliseconds.
const TIME_LIMIT_MS: u64 = 5_000;

/// A scored document with its file name and class.
struct Scored<'a> {
    name: &'a str,
    scores: &'a Scores,
    class: Class,
}

impl Scored<'_> {
    /// A rule document of `tests/corpus/word-rules`: they are named `E-…`.
    fn is_rule(&self) -> bool {
        self.name.starts_with("E-")
    }
}

fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// What a report lacks of main's: files main's report has and it does not, and reference
/// documents main scored or tried to score that it did not.
#[derive(Debug, Default, PartialEq)]
pub struct Missing {
    pub files: Vec<String>,
    pub references: Vec<String>,
}

impl Missing {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.references.is_empty()
    }
}

/// A file the runner tried to score: one with a Word reference.
fn is_reference(file: &FileResult) -> bool {
    file.scores.is_some() || file.score_error.is_some()
}

/// What `report` lacks of `main`. A report made over fewer paths, or without the
/// references, would otherwise leave main's failing documents out of every row. Files that
/// only the report has are new documents and count for nothing here.
pub fn missing(report: &Report, main: &Report) -> Missing {
    let files: HashMap<&str, &FileResult> = report
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let mut out = Missing::default();
    for before in &main.files {
        match files.get(before.path.as_str()) {
            None => out.files.push(before.path.clone()),
            Some(file) if is_reference(before) && !is_reference(file) => {
                out.references.push(before.path.clone());
            }
            Some(_) => {}
        }
    }
    out
}

/// The value of every criterion on the report.
pub fn measure(report: &Report, same_fonts: bool) -> Vec<Measure> {
    measure_against(report, None, same_fonts)
}

/// The value of every criterion on the report, which row 2 compares with main's: files and
/// reference documents of main that the report lacks are refusals too.
pub fn measure_against(report: &Report, main: Option<&Report>, same_fonts: bool) -> Vec<Measure> {
    let scored: Vec<Scored> = report
        .files
        .iter()
        .filter_map(|file| {
            let scores = file.scores.as_ref()?;
            Some(Scored {
                name: file_name(&file.path),
                scores,
                class: class(scores, same_fonts),
            })
        })
        .collect();
    let gated: Vec<&Scored> = scored
        .iter()
        .filter(|doc| matches!(doc.class, Class::Same | Class::Twin))
        .collect();
    let mut out = Vec::new();

    // 1. Crashes.
    let crashes = report
        .files
        .iter()
        .filter(|file| file.status.is_robustness_failure())
        .count();
    out.push(Measure::counted(crashes.to_string(), crashes));

    // 2. Refusals: an unexpected error anywhere, a document that opened but could not be
    // rendered, and a reference document that did not open or could not be scored, which
    // would also leave the rows below with fewer documents than main.
    let unexpected = report
        .files
        .iter()
        .filter(|file| file.status == Status::UnexpectedError)
        .count();
    let refused = report
        .files
        .iter()
        .filter(|file| {
            (file.scores.is_some() || file.score_error.is_some()) && file.status != Status::Opened
        })
        .count();
    let unscored = report
        .files
        .iter()
        .filter(|file| file.score_error.is_some() && file.status == Status::Opened)
        .count();
    let unrendered = report
        .files
        .iter()
        .filter(|file| file.render_error.is_some())
        .count();
    let lacking = main.map(|main| missing(report, main)).unwrap_or_default();
    let mut text = format!(
        "{unexpected} unexpected errors, {unrendered} not rendered, {refused} reference \
         documents not opened, {unscored} not scored"
    );
    if !lacking.is_empty() {
        text.push_str(&format!(
            "; of main's, {} files missing and {} reference documents not scored",
            lacking.files.len(),
            lacking.references.len()
        ));
    }
    let lacked = lacking.files.len() + lacking.references.len();
    out.push(Measure {
        text,
        pass: Some(unexpected + unrendered + refused + unscored + lacked == 0),
        keys: vec![
            Key::count(unexpected),
            Key::count(unrendered),
            Key::count(refused + unscored),
            Key::count(lacked),
        ],
    });

    // 3. Valid output.
    let checked: Vec<&Scored> = scored
        .iter()
        .filter(|doc| doc.scores.pdf_valid.is_some())
        .collect();
    if checked.is_empty() {
        out.push(Measure::unmeasured("no PDF checked"));
    } else {
        let bad = checked
            .iter()
            .filter(|doc| {
                doc.scores.pdf_valid == Some(false)
                    || doc
                        .scores
                        .fonts_not_embedded
                        .as_ref()
                        .is_some_and(|fonts| !fonts.is_empty())
            })
            .count();
        out.push(Measure::counted(
            format!(
                "{bad} of {} PDFs invalid or with a font not embedded",
                checked.len()
            ),
            bad,
        ));
    }

    // 4. Page count.
    if gated.is_empty() {
        out.push(Measure::unmeasured("no S or T document scored"));
    } else {
        let same = gated
            .iter()
            .filter(|doc| doc.scores.pages == doc.scores.ref_pages)
            .count();
        let far = gated
            .iter()
            .filter(|doc| doc.scores.pages.abs_diff(doc.scores.ref_pages) > 1)
            .count();
        let rules: Vec<&&Scored> = gated.iter().filter(|doc| doc.is_rule()).collect();
        let rules_same = rules
            .iter()
            .filter(|doc| doc.scores.pages == doc.scores.ref_pages)
            .count();
        let share = same as f64 / gated.len() as f64;
        out.push(Measure {
            text: format!(
                "{:.1} % same ({same}/{}), {far} off by more than one, `E-*` {rules_same}/{}",
                share * 100.0,
                gated.len(),
                rules.len()
            ),
            pass: Some(share >= 0.95 && far == 0 && rules_same == rules.len()),
            keys: vec![
                Key::higher(round(share)),
                Key::count(far),
                Key::count(rules.len() - rules_same),
            ],
        });
    }

    // 5. text_score, every class.
    if scored.is_empty() {
        out.push(Measure::unmeasured("no document scored"));
    } else {
        let below = scored
            .iter()
            .filter(|doc| doc.scores.text_score < 0.99)
            .count();
        let lowest = scored
            .iter()
            .map(|doc| doc.scores.text_score)
            .fold(1.0, f64::min);
        let mut measure = Measure::counted(
            format!(
                "{below} of {} below 0.99 (lowest {lowest:.4})",
                scored.len()
            ),
            below,
        );
        measure.keys.push(Key::higher(round(lowest)));
        out.push(measure);
    }

    // 6. line_agreement.
    let lines: usize = gated.iter().map(|doc| doc.scores.lines).sum();
    if lines == 0 {
        out.push(Measure::unmeasured("no line of an S or T document"));
    } else {
        let agreeing: usize = gated.iter().map(|doc| doc.scores.lines_agreeing).sum();
        let overall = agreeing as f64 / lines as f64;
        let below = gated
            .iter()
            .filter(|doc| doc.scores.line_agreement < 0.95)
            .count();
        let rules_below = gated
            .iter()
            .filter(|doc| doc.is_rule() && doc.scores.line_agreement < 1.0)
            .count();
        out.push(Measure {
            text: format!(
                "{overall:.4} overall, {below} documents below 0.95, {rules_below} `E-*` below 1.0"
            ),
            pass: Some(overall >= 0.98 && below == 0 && rules_below == 0),
            keys: vec![
                Key::higher(round(overall)),
                Key::count(below),
                Key::count(rules_below),
            ],
        });
    }

    // 7. Positions.
    let placed: Vec<&&Scored> = gated
        .iter()
        .filter(|doc| doc.scores.words_matched > 0)
        .collect();
    if placed.is_empty() {
        out.push(Measure::unmeasured("no matched word in an S or T document"));
    } else {
        let value = |v: Option<f64>| v.unwrap_or(0.0);
        let off = placed
            .iter()
            .filter(|doc| {
                let s = doc.scores;
                value(s.dx_median) > 0.5
                    || value(s.dy_median) > 0.5
                    || value(s.dx_p95) > 2.0
                    || value(s.dy_p95) > 2.0
            })
            .count();
        let worst = |pick: fn(&Scores) -> Option<f64>| {
            placed
                .iter()
                .map(|doc| value(pick(doc.scores)))
                .fold(0.0, f64::max)
        };
        let (dx, dy) = (worst(|s| s.dx_p95), worst(|s| s.dy_p95));
        let mut measure = Measure::counted(
            format!(
                "{off} of {} documents off (largest p95: dx {dx} pt, dy {dy} pt)",
                placed.len()
            ),
            off,
        );
        // The percentiles are rounded to 0.001 pt already.
        measure.keys.extend([Key::lower(dx), Key::lower(dy)]);
        out.push(measure);
    }

    // 8. page_break_agreement.
    let breaks: usize = gated.iter().map(|doc| doc.scores.page_breaks).sum();
    if breaks == 0 {
        out.push(Measure::unmeasured("no page break in an S or T document"));
    } else {
        let agreeing: usize = gated
            .iter()
            .map(|doc| doc.scores.page_breaks_agreeing)
            .sum();
        let overall = agreeing as f64 / breaks as f64;
        out.push(Measure {
            text: format!("{overall:.4} ({agreeing}/{breaks} breaks)"),
            pass: Some(overall >= 0.97),
            keys: vec![Key::higher(round(overall))],
        });
    }

    // 9. SSIM.
    if gated.is_empty() {
        out.push(Measure::unmeasured("no S or T document scored"));
    } else {
        let below = gated
            .iter()
            .filter(|doc| {
                let (page, mean) = if doc.class == Class::Same {
                    (0.90, 0.95)
                } else {
                    (0.88, 0.93)
                };
                doc.scores.ssim_min < page || doc.scores.ssim_mean < mean
            })
            .count();
        let lowest = gated
            .iter()
            .map(|doc| doc.scores.ssim_min)
            .fold(1.0, f64::min);
        let mut measure = Measure::counted(
            format!(
                "{below} of {} documents below (lowest page {lowest:.4})",
                gated.len()
            ),
            below,
        );
        measure.keys.push(Key::higher(round(lowest)));
        out.push(measure);
    }

    // 10. Speed and memory: of opening and rendering when the run rendered, else of
    // opening only.
    let rendered = report
        .files
        .iter()
        .any(|file| file.layout_report.is_some() || file.render_error.is_some());
    let slowest = report
        .files
        .iter()
        .map(|file| file.duration_ms)
        .max()
        .unwrap_or(0);
    let peak = report
        .files
        .iter()
        .filter_map(|file| file.peak_rss_kb)
        .max()
        .map(|kb| kb.saturating_mul(1024));
    let mut keys = vec![Key {
        // Milliseconds: the same file takes a different time on every run.
        tolerance: 500.0,
        ..Key::lower(slowest as f64)
    }];
    if let Some(bytes) = peak {
        keys.push(Key {
            tolerance: 16.0 * 1024.0 * 1024.0,
            ..Key::lower(bytes as f64)
        });
    }
    out.push(Measure {
        text: format!(
            "slowest {:.2} s, peak {}{}",
            slowest as f64 / 1000.0,
            peak.map_or("not measured".to_owned(), |bytes| format!(
                "{:.0} MB",
                bytes as f64 / 1e6
            )),
            if rendered { "" } else { " (opening only)" }
        ),
        pass: Some(slowest < TIME_LIMIT_MS && peak.is_none_or(|bytes| bytes < MEMORY_LIMIT)),
        keys,
    });

    // 11. Report: what the PDF shows otherwise than Word does without an entry saying so.
    let silent = silences(report);
    if silent.fonts_checked + silent.content_checked == 0 {
        out.push(Measure::unmeasured(
            "no document checked: nothing rendered, or a report without the fonts and content",
        ));
    } else {
        let (fonts, content): (Vec<&Silence>, Vec<&Silence>) = silent
            .found
            .iter()
            .partition(|found| matches!(found.what, Unreported::Font(_)));
        let mut text = format!(
            "{} fonts unreported in {} documents, of {} checked; {} kinds of content \
             unreported in {} documents, of {} checked",
            fonts.len(),
            documents(&fonts),
            silent.fonts_checked,
            content.len(),
            documents(&content),
            silent.content_checked
        );
        let mut keys = vec![Key::count(fonts.len()), Key::count(content.len())];
        if let Some(main) = main {
            let new = silent.new_against(&silences(main));
            text.push_str(&format!("; {new} new against main"));
            keys.push(Key::count(new));
        } else {
            keys.push(Key::count(0));
        }
        out.push(Measure {
            text,
            pass: Some(silent.found.is_empty()),
            keys,
        });
    }

    // 12. Required features: entries of reference documents that degrade one.
    let degraded = degradations(report);
    if degraded.checked == 0 {
        out.push(Measure::unmeasured("no rendered reference document"));
    } else {
        let documents: BTreeSet<&str> = degraded
            .found
            .iter()
            .map(|(path, _)| path.as_str())
            .collect();
        let mut by_feature: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, feature) in &degraded.found {
            let label = if feature.starts_with(UNCLASSIFIED) {
                "unclassified"
            } else {
                feature.as_str()
            };
            *by_feature.entry(label).or_default() += 1;
        }
        let mut text = format!(
            "{} of {} reference documents",
            documents.len(),
            degraded.checked
        );
        if !by_feature.is_empty() {
            let counts: Vec<String> = by_feature
                .iter()
                .map(|(feature, count)| format!("{feature} {count}"))
                .collect();
            text.push_str(&format!(" ({})", counts.join(", ")));
        }
        let mut keys = vec![
            Key::count(documents.len()),
            Key::count(degraded.found.len()),
        ];
        if let Some(main) = main {
            let before = degradations(main).found;
            let new = degraded.found.difference(&before).count();
            text.push_str(&format!("; {new} new against main"));
            keys.push(Key::count(new));
        } else {
            keys.push(Key::count(0));
        }
        out.push(Measure {
            text,
            pass: Some(documents.is_empty()),
            keys,
        });
    }
    out
}

/// What row 11 finds unreported in a document.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unreported {
    /// A font of Word's text that our PDF does not embed and no `font-substituted` entry
    /// names.
    Font(String),
    /// A kind of content that no `feature-unsupported` entry names.
    Content(String),
    /// The document's markup could not be read for its content.
    Unreadable(String),
}

impl std::fmt::Display for Unreported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unreported::Font(font) => write!(f, "font {font}"),
            Unreported::Content(kind) => write!(f, "{kind}"),
            Unreported::Unreadable(error) => write!(f, "markup not read: {error}"),
        }
    }
}

/// One thing row 11 finds, and the file it is in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Silence {
    pub path: String,
    pub what: Unreported,
}

/// Row 11 over a report: what it finds, and how many documents it could check for fonts
/// and for content.
#[derive(Debug, Default)]
pub struct Silences {
    pub found: Vec<Silence>,
    pub fonts_checked: usize,
    pub content_checked: usize,
    /// The files whose fonts, and whose content, were checked, for comparing with main.
    fonts_of: BTreeSet<String>,
    content_of: BTreeSet<String>,
}

impl Silences {
    /// How many findings this report has that `main` does not, among the documents main
    /// checked the same way: a report of an older runner, which did not check them, adds
    /// none.
    fn new_against(&self, main: &Silences) -> usize {
        let before: BTreeSet<&Silence> = main.found.iter().collect();
        self.found
            .iter()
            .filter(|found| {
                let checked = match found.what {
                    Unreported::Font(_) => &main.fonts_of,
                    Unreported::Content(_) | Unreported::Unreadable(_) => &main.content_of,
                };
                checked.contains(&found.path) && !before.contains(found)
            })
            .count()
    }
}

fn documents(found: &[&Silence]) -> usize {
    found
        .iter()
        .map(|found| found.path.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

/// A font's name as `pdffonts` gives it, or a family's, in lower case without spaces and
/// punctuation, for comparing the two.
fn squeezed(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Whether our PDF embeds a font of `family`: one whose PostScript name, before its style
/// (`LiberationSans-Bold`), is the family's name without spaces, or that with Microsoft's
/// `PSMT`, `MT` or `PS` after it (`TimesNewRomanPSMT`, `TimesNewRomanPS-BoldMT`).
fn embeds(pdf_fonts: &[String], family: &str) -> bool {
    let family = squeezed(family);
    !family.is_empty()
        && pdf_fonts.iter().any(|font| {
            let base = font.split('-').next().unwrap_or(font);
            [squeezed(font), squeezed(base)].iter().any(|name| {
                name == &family
                    || name
                        .strip_prefix(family.as_str())
                        .is_some_and(|rest| matches!(rest, "psmt" | "mt" | "ps"))
            })
        })
}

/// The family a `font-substituted` entry names first, in its quotes: the requested one of
/// a substitution (`"Calibri" → "Carlito" (...)`), the font of a fallback for some
/// characters, or a symbol font that is not installed.
fn substituted(message: &str) -> Option<&str> {
    let rest = message.strip_prefix('"')?;
    rest.split_once('"').map(|(family, _)| family)
}

/// The stories of Word's formatting export whose fonts row 11 does not check, with the
/// words of an entry that says the layout leaves their text out: comments, which Word's
/// references do not show either, and stories the report says are not laid out.
const STORIES_LEFT_OUT: [(&str, &[&str]); 4] = [
    ("comments", &[]),
    ("endnotes", &["endnotes are not laid out"]),
    (
        "footnotes",
        &[
            "footnotes referenced in table cells are not laid out",
            "their notes are left out",
        ],
    ),
    (
        "text-boxes",
        &[
            "a group of vml shapes is left out",
            "a vml picture's text (v:textbox) is left out",
            "a drawing that shows no picture",
            "floating objects in text boxes are not drawn",
        ],
    ),
];

/// Row 11 over a report.
pub fn silences(report: &Report) -> Silences {
    let mut out = Silences::default();
    for file in &report.files {
        let Some(entries) = &file.layout_report else {
            continue;
        };
        let fonts = file.scores.as_ref().and_then(|scores| {
            Some((scores.word_text_fonts.as_ref()?, scores.pdf_fonts.as_ref()?))
        });
        if let Some((word_fonts, pdf_fonts)) = fonts {
            out.fonts_checked += 1;
            out.fonts_of.insert(file.path.clone());
            let reported: BTreeSet<String> = entries
                .iter()
                .filter(|entry| entry.kind == "font-substituted")
                .filter_map(|entry| substituted(&entry.message))
                .map(str::to_lowercase)
                .collect();
            let left_out = |story: &str| {
                STORIES_LEFT_OUT
                    .iter()
                    .find(|(name, _)| *name == story)
                    .is_some_and(|(_, words)| {
                        words.is_empty()
                            || entries.iter().any(|entry| {
                                entry.kind == "feature-unsupported"
                                    && content::names(&entry.message, words)
                            })
                    })
            };
            let not_shown = file.stories_not_shown.as_deref().unwrap_or_default();
            let shown: BTreeSet<&String> = word_fonts
                .iter()
                .filter(|(story, _)| !left_out(story) && !not_shown.contains(story))
                .flat_map(|(_, fonts)| fonts)
                .collect();
            for font in shown {
                if !reported.contains(&font.to_lowercase()) && !embeds(pdf_fonts, font) {
                    out.found.push(Silence {
                        path: file.path.clone(),
                        what: Unreported::Font(font.clone()),
                    });
                }
            }
        }
        if let Some(error) = &file.content_error {
            out.content_checked += 1;
            out.content_of.insert(file.path.clone());
            out.found.push(Silence {
                path: file.path.clone(),
                what: Unreported::Unreadable(error.clone()),
            });
        } else if let Some(kinds) = &file.content {
            out.content_checked += 1;
            out.content_of.insert(file.path.clone());
            for kind in kinds {
                let Some(words) = content::words(kind) else {
                    continue;
                };
                let named = entries.iter().any(|entry| {
                    entry.kind == "feature-unsupported" && content::names(&entry.message, words)
                });
                if !named {
                    out.found.push(Silence {
                        path: file.path.clone(),
                        what: Unreported::Content(kind.clone()),
                    });
                }
            }
        }
    }
    out
}

/// How row 12 names an entry no line of `features::MESSAGES` classes: this, then its
/// message.
const UNCLASSIFIED: &str = "unclassified: ";

/// Row 12 over a report: the reference documents it could check, and each document with
/// a required feature an entry degrades (`unclassified: ` and the message for an entry
/// that is not classed).
pub struct Degradations {
    pub checked: usize,
    pub found: BTreeSet<(String, String)>,
}

pub fn degradations(report: &Report) -> Degradations {
    let mut out = Degradations {
        checked: 0,
        found: BTreeSet::new(),
    };
    for file in report.files.iter().filter(|file| is_reference(file)) {
        let Some(entries) = &file.layout_report else {
            continue;
        };
        out.checked += 1;
        for entry in entries {
            let feature = match features::classify(&entry.kind, &entry.message) {
                features::Class::Required(feature) => feature.to_owned(),
                features::Class::Unclassified => format!("{UNCLASSIFIED}{}", entry.message),
                features::Class::NotRequired => continue,
            };
            out.found.insert((file.path.clone(), feature));
        }
    }
    out
}

/// The lines above the table that name what rows 11 and 12 find, by the paths of the
/// report, so that a miss can be read without opening the report.
pub fn findings(report: &Report) -> Vec<String> {
    let mut lines = Vec::new();
    let silent = silences(report);
    if !silent.found.is_empty() {
        let items: Vec<String> = silent
            .found
            .iter()
            .map(|found| format!("{} ({})", found.path, found.what))
            .collect();
        lines.push(format!("Row 11, not reported: {}", listed(&items)));
    }
    let degraded = degradations(report);
    let mut by_feature: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (path, feature) in &degraded.found {
        by_feature
            .entry(feature.as_str())
            .or_default()
            .push(path.clone());
    }
    for (feature, files) in by_feature {
        lines.push(format!("Row 12, {feature}: {}", listed(&files)));
    }
    lines
}

/// Up to twenty items, and how many more there are.
fn listed(items: &[String]) -> String {
    let shown: Vec<&str> = items.iter().take(20).map(String::as_str).collect();
    let mut line = shown.join(", ");
    if items.len() > shown.len() {
        line.push_str(&format!(" and {} more", items.len() - shown.len()));
    }
    line
}

/// How the documents divide, and which scored ones the render criteria leave out, for the
/// lines above the table.
pub fn census(report: &Report, same_fonts: bool) -> String {
    let mut counts = [0usize; 4];
    let mut left_out = Vec::new();
    let mut partial = 0;
    for file in &report.files {
        let Some(scores) = file.scores.as_ref() else {
            continue;
        };
        let class = class(scores, same_fonts);
        if class == Class::Unknown && scores.word_fonts.is_some() {
            partial += 1;
        }
        let index = match class {
            Class::Same => 0,
            Class::Twin => 1,
            Class::Other => 2,
            Class::Unknown => 3,
        };
        if index >= 2 {
            left_out.push(file_name(&file.path));
        }
        if let Some(count) = counts.get_mut(index) {
            *count += 1;
        }
    }
    let [same, twin, other, unknown] = counts;
    let unscored = report
        .files
        .iter()
        .filter(|file| file.score_error.is_some())
        .count();
    let without_runs = if partial > 0 {
        format!(" ({partial} without the fonts of runs)")
    } else {
        String::new()
    };
    let mut line = format!(
        "{} files, {} scored against Word's references: S {same}, T {twin}, N {other}, \
         no class {unknown}{without_runs}; {unscored} reference documents not scored",
        report.files.len(),
        same + twin + other + unknown
    );
    if !left_out.is_empty() {
        let shown: Vec<&str> = left_out.iter().take(20).copied().collect();
        line.push_str(&format!(
            ". Not gated by rows 4 and 6 to 9 (N or no class): {}",
            shown.join(", ")
        ));
        if left_out.len() > shown.len() {
            line.push_str(&format!(" and {} more", left_out.len() - shown.len()));
        }
    }
    line
}

fn met(pass: Option<bool>) -> &'static str {
    match pass {
        Some(true) => "yes",
        Some(false) => "**no**",
        None => "–",
    }
}

/// The table: with a main report, its values next to the branch's, and a branch value worse
/// than main's marked.
pub fn table(branch: &[Measure], main: Option<&[Measure]>) -> String {
    let mut out = String::new();
    if main.is_some() {
        out.push_str("| # | Criterion | Target | Applies to | main | branch | Met |\n");
        out.push_str("|---|---|---|---|---|---|---|\n");
    } else {
        out.push_str("| # | Criterion | Target | Applies to | Value | Met |\n");
        out.push_str("|---|---|---|---|---|---|\n");
    }
    for (index, ((name, target, applies), measure)) in CRITERIA.iter().zip(branch).enumerate() {
        let number = index + 1;
        match main.and_then(|main| main.get(index)) {
            Some(before) => {
                let mark = if measure.worse_than(before) {
                    " **(worse than main)**"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "| {number} | {name} | {target} | {applies} | {} | {}{mark} | {} |\n",
                    before.text,
                    measure.text,
                    met(measure.pass)
                ));
            }
            None => out.push_str(&format!(
                "| {number} | {name} | {target} | {applies} | {} | {} |\n",
                measure.text,
                met(measure.pass)
            )),
        }
    }
    out
}

/// Whether every criterion that could be measured is met.
pub fn passes(measures: &[Measure]) -> bool {
    measures.iter().all(|measure| measure.pass != Some(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(pages: usize, text: f64, lines: usize, agreeing: usize, fonts: &[&str]) -> Scores {
        Scores {
            pages,
            ref_pages: 2,
            text_score: text,
            lines,
            lines_agreeing: agreeing,
            line_agreement: if lines == 0 {
                1.0
            } else {
                agreeing as f64 / lines as f64
            },
            words: 10,
            words_matched: 10,
            dx_median: Some(0.1),
            dx_p95: Some(0.4),
            dy_median: Some(0.2),
            dy_p95: Some(1.5),
            page_breaks: 1,
            page_breaks_agreeing: 1,
            page_break_agreement: Some(1.0),
            ssim: vec![0.97, 0.96],
            ssim_mean: 0.965,
            ssim_min: 0.96,
            pdf_valid: Some(true),
            fonts_not_embedded: Some(Vec::new()),
            word_fonts: Some(fonts.iter().map(|font| (*font).to_owned()).collect()),
            word_fonts_complete: Some(true),
            word_text_fonts: Some(BTreeMap::from([(
                "main".to_owned(),
                fonts.iter().map(|font| (*font).to_owned()).collect(),
            )])),
            // Every font of the text embedded, under its PostScript name.
            pdf_fonts: Some(fonts.iter().map(|font| font.replace(' ', "")).collect()),
        }
    }

    fn file(path: &str, status: Status, scores: Option<Scores>) -> FileResult {
        FileResult {
            path: path.to_owned(),
            status,
            error: None,
            expected_reason: None,
            duration_ms: 120,
            peak_rss_kb: Some(50_000),
            chars: None,
            diagnostics: None,
            scores,
            score_error: None,
            render_error: None,
            layout_report: None,
            content: None,
            content_error: None,
            stories_not_shown: None,
        }
    }

    fn good() -> Report {
        Report::new(vec![
            file(
                "c/E-rule.docx",
                Status::Opened,
                Some(scores(2, 1.0, 40, 40, &["Carlito"])),
            ),
            file(
                "c/letter.docx",
                Status::Opened,
                Some(scores(2, 0.995, 100, 99, &["Calibri", "Carlito"])),
            ),
            // Class N: its pages and lines are not gated.
            file(
                "c/poster.docx",
                Status::Opened,
                Some(scores(5, 1.0, 10, 1, &["Aptos"])),
            ),
            file(
                "c/old.docx",
                Status::Opened,
                Some(Scores {
                    word_fonts: None,
                    ..scores(2, 1.0, 1, 1, &[])
                }),
            ),
            file("c/plain.docx", Status::ExpectedError, None),
        ])
    }

    #[test]
    fn documents_are_classed_by_the_fonts_word_used() {
        let s = |fonts: &[&str]| scores(1, 1.0, 1, 1, fonts);
        assert_eq!(class(&s(&["Carlito", "Noto Sans"]), false), Class::Same);
        assert_eq!(
            class(&s(&["Calibri", "Liberation Serif"]), false),
            Class::Twin
        );
        assert_eq!(class(&s(&["Calibri", "Aptos"]), false), Class::Other);
        assert_eq!(class(&s(&["(unresolved)"]), false), Class::Other);
        assert_eq!(
            class(
                &Scores {
                    word_fonts: None,
                    ..s(&[])
                },
                false
            ),
            Class::Unknown
        );
        assert_eq!(
            class(&s(&["Aptos"]), true),
            Class::Same,
            "the release run has Word's fonts"
        );
        // Without the fonts of the runs, S and T are not known; N is.
        let partial = |fonts: &[&str], complete: Option<bool>| Scores {
            word_fonts_complete: complete,
            ..s(fonts)
        };
        for complete in [Some(false), None] {
            assert_eq!(
                class(&partial(&["Carlito"], complete), false),
                Class::Unknown
            );
            assert_eq!(
                class(&partial(&["Calibri"], complete), false),
                Class::Unknown
            );
            assert_eq!(
                class(&partial(&["Calibri", "Aptos"], complete), false),
                Class::Other
            );
        }
        assert_eq!(
            class(&partial(&["Calibri"], Some(false)), true),
            Class::Same
        );
    }

    /// The census says how many documents have no class only because their export lacks
    /// the fonts of the runs (#133).
    #[test]
    fn the_census_counts_documents_without_the_fonts_of_runs() {
        let mut report = good();
        scores_of(&mut report, "c/letter.docx").word_fonts_complete = Some(false);
        let measures = measure(&report, false);
        assert_eq!(
            census(&report, false),
            "5 files, 4 scored against Word's references: S 1, T 0, N 1, no class 2 (1 without \
             the fonts of runs); 0 reference documents not scored. Not gated by rows 4 and 6 to \
             9 (N or no class): letter.docx, old.docx, poster.docx"
        );
        assert_eq!(
            measures[3].text,
            "100.0 % same (1/1), 0 off by more than one, `E-*` 1/1"
        );
    }

    #[test]
    fn a_good_report_meets_every_measured_criterion() {
        let report = good();
        let measures = measure(&report, false);
        assert_eq!(measures.len(), CRITERIA.len());
        assert!(passes(&measures), "{measures:#?}");
        assert_eq!(measures[4].text, "0 of 4 below 0.99 (lowest 0.9950)");
        assert_eq!(
            measures[3].text,
            "100.0 % same (2/2), 0 off by more than one, `E-*` 1/1"
        );
        assert_eq!(
            measures[5].text,
            "0.9929 overall, 0 documents below 0.95, 0 `E-*` below 1.0"
        );
        assert_eq!(measures[7].text, "1.0000 (2/2 breaks)");
        assert_eq!((measures[10].pass, measures[11].pass), (None, None));
        assert_eq!(
            census(&report, false),
            "5 files, 4 scored against Word's references: S 1, T 1, N 1, no class 1; \
             0 reference documents not scored. Not gated by rows 4 and 6 to 9 (N or no \
             class): old.docx, poster.docx"
        );
    }

    fn named<'a>(report: &'a mut Report, path: &str) -> &'a mut FileResult {
        report
            .files
            .iter_mut()
            .find(|file| file.path == path)
            .unwrap()
    }

    fn scores_of<'a>(report: &'a mut Report, path: &str) -> &'a mut Scores {
        named(report, path).scores.as_mut().unwrap()
    }

    #[test]
    fn misses_are_found() {
        let mut report = good();
        let rule = scores_of(&mut report, "c/E-rule.docx");
        (rule.lines_agreeing, rule.line_agreement) = (39, 0.975); // an E-* line broken elsewhere
        let letter = scores_of(&mut report, "c/letter.docx");
        letter.pages = 4; // two pages more than Word
        letter.dy_p95 = Some(2.5);
        letter.ssim_min = 0.87; // T allows 0.88
        letter.fonts_not_embedded = Some(vec!["Helvetica".to_owned()]);
        scores_of(&mut report, "c/poster.docx").text_score = 0.5; // text is gated in every class
        named(&mut report, "c/plain.docx").status = Status::Panic;
        report
            .files
            .push(file("c/broken.docx", Status::UnexpectedError, None));
        let mut refused = file("c/refused.docx", Status::UnexpectedError, None);
        refused.score_error = Some("no PDF of ours".to_owned());
        report.files.push(refused);
        let mut unrendered = file("c/unrendered.docx", Status::Opened, None);
        unrendered.render_error = Some("cannot write out.pdf".to_owned());
        report.files.push(unrendered);
        let measures = measure(&report, false);
        let failed: Vec<usize> = measures
            .iter()
            .enumerate()
            .filter(|(_, measure)| measure.pass == Some(false))
            .map(|(index, _)| index + 1)
            .collect();
        assert_eq!(failed, [1, 2, 3, 4, 5, 6, 7, 9]);
        assert_eq!(
            measures[1].text,
            "2 unexpected errors, 1 not rendered, 1 reference documents not opened, 0 not scored"
        );
    }

    #[test]
    fn the_table_marks_values_worse_than_main() {
        let main = measure(&good(), false);
        let mut report = good();
        named(&mut report, "c/letter.docx")
            .scores
            .as_mut()
            .unwrap()
            .lines_agreeing = 90;
        let branch = measure(&report, false);
        let table = table(&branch, Some(&main));
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 2 + CRITERIA.len());
        assert!(
            lines[0].starts_with("| # | Criterion | Target | Applies to | main | branch | Met |")
        );
        assert!(
            lines[7].contains("0.9929 overall")
                && lines[7].contains("(worse than main)")
                && lines[7].ends_with("| **no** |"),
            "{}",
            lines[7]
        );
        assert!(!lines[3].contains("worse"), "{}", lines[3]);
        assert!(lines[12].ends_with("| – |"));
        // Timings within half a second of main's are noise, not a regression.
        let mut slower = good();
        named(&mut slower, "c/letter.docx").duration_ms = 600;
        let timed = super::table(&measure(&slower, false), Some(&main));
        assert!(!timed.lines().nth(11).unwrap().contains("worse"), "{timed}");
        named(&mut slower, "c/letter.docx").duration_ms = 700;
        let timed = super::table(&measure(&slower, false), Some(&main));
        assert!(
            timed.lines().nth(11).unwrap().contains("(worse than main)"),
            "{timed}"
        );
        let alone = super::table(&branch, None);
        assert!(alone.starts_with("| # | Criterion | Target | Applies to | Value | Met |"));
    }

    /// A document that opens but cannot be rendered fails row 2 and is worse than main
    /// there, whether or not it has a reference.
    #[test]
    fn a_document_that_cannot_be_rendered_is_worse() {
        let main = measure(&good(), false);
        let mut report = good();
        let mut unrendered = file("c/no-reference.docx", Status::Opened, None);
        unrendered.render_error = Some("cannot write out.pdf".to_owned());
        report.files.push(unrendered);
        let branch = measure(&report, false);
        let row = table(&branch, Some(&main))
            .lines()
            .nth(3)
            .unwrap()
            .to_owned();
        assert!(
            row.contains("1 not rendered") && row.contains("(worse than main)"),
            "{row}"
        );
        assert_eq!(branch[1].pass, Some(false));
    }

    /// A reference document that opens but drops out of scoring fails row 2 and is worse
    /// than main there, instead of leaving the rows below with one document fewer.
    #[test]
    fn a_document_that_is_no_longer_scored_is_worse() {
        let main = measure(&good(), false);
        let mut report = good();
        let letter = named(&mut report, "c/letter.docx");
        letter.scores = None;
        letter.score_error = Some("pdftoppm failed".to_owned());
        let branch = measure(&report, false);
        assert_eq!(branch[1].pass, Some(false));
        assert!(
            branch[1].text.ends_with("1 not scored"),
            "{}",
            branch[1].text
        );
        assert!(branch[1].worse_than(&main[1]));
        assert!(census(&report, false).contains("1 reference documents not scored"));
        // A reference document that did not open counts even if a PDF of it was scored.
        let mut stale = good();
        named(&mut stale, "c/letter.docx").status = Status::ExpectedError;
        let measures = measure(&stale, false);
        assert_eq!(measures[1].pass, Some(false));
        assert!(
            measures[1]
                .text
                .contains("1 reference documents not opened"),
            "{}",
            measures[1].text
        );
        // A row that main measured and the branch cannot is worse too.
        let unmeasured = measure(
            &Report::new(vec![file("c/a.docx", Status::Opened, None)]),
            false,
        );
        assert!(unmeasured[5].worse_than(&main[5]));
        assert!(!main[5].worse_than(&unmeasured[5]));
    }

    /// A branch report made over fewer files, or without the references, cannot hide main's
    /// documents: what it lacks of main's counts in row 2 (#130).
    #[test]
    fn what_the_branch_lacks_of_main_is_refused() {
        let main_report = good();
        let main = measure(&main_report, false);
        let mut report = good();
        report.files.retain(|file| file.path != "c/letter.docx");
        named(&mut report, "c/E-rule.docx").scores = None;
        assert_eq!(
            missing(&report, &main_report),
            Missing {
                files: vec!["c/letter.docx".to_owned()],
                references: vec!["c/E-rule.docx".to_owned()],
            }
        );
        let branch = measure_against(&report, Some(&main_report), false);
        assert_eq!(branch[1].pass, Some(false));
        assert!(branch[1].worse_than(&main[1]));
        assert_eq!(
            branch[1].text,
            "0 unexpected errors, 0 not rendered, 0 reference documents not opened, 0 not \
             scored; of main's, \
             1 files missing and 1 reference documents not scored"
        );
        assert!(
            passes(&measure(&report, false)),
            "alone, the report looks clean"
        );
        // Files and references only the branch has are new, not refusals.
        let mut grown = good();
        grown.files.push(file(
            "c/new.docx",
            Status::Opened,
            Some(scores(2, 1.0, 1, 1, &["Carlito"])),
        ));
        let plain = named(&mut grown, "c/plain.docx");
        (plain.status, plain.scores) = (Status::Opened, Some(scores(2, 1.0, 1, 1, &["Carlito"])));
        assert!(missing(&grown, &main_report).is_empty());
        let measures = measure_against(&grown, Some(&main_report), false);
        assert!(
            !measures[1].text.contains("of main's"),
            "{}",
            measures[1].text
        );
        assert!(!measures[1].worse_than(&main[1]));
    }

    /// A reference document of main's that the branch did not try to score is lacking even
    /// when the branch has the file: a branch report made without the references (#130).
    #[test]
    fn a_reference_main_tried_and_the_branch_did_not_is_refused() {
        let mut main_report = good();
        let mut tried = file("c/tried.docx", Status::Opened, None);
        tried.score_error = Some("no PDF of ours".to_owned());
        main_report.files.push(tried);
        let mut report = good();
        report
            .files
            .push(file("c/tried.docx", Status::Opened, None));
        named(&mut report, "c/letter.docx").scores = None;
        let lacking = missing(&report, &main_report);
        assert_eq!(
            lacking,
            Missing {
                files: Vec::new(),
                references: vec!["c/letter.docx".to_owned(), "c/tried.docx".to_owned()],
            }
        );
        assert!(!lacking.is_empty());
        let branch = measure_against(&report, Some(&main_report), false);
        assert_eq!(branch[1].pass, Some(false));
        assert!(
            branch[1]
                .text
                .ends_with("of main's, 0 files missing and 2 reference documents not scored"),
            "{}",
            branch[1].text
        );
        // A reference the branch tried and failed to score is a refusal once, not lacking too.
        let mut failed = good();
        named(&mut failed, "c/letter.docx").scores = None;
        named(&mut failed, "c/letter.docx").score_error = Some("pdftoppm failed".to_owned());
        assert!(missing(&failed, &good()).is_empty());
        let measures = measure_against(&failed, Some(&good()), false);
        assert_eq!(
            measures[1].text,
            "0 unexpected errors, 0 not rendered, 0 reference documents not opened, 1 not scored"
        );
    }

    /// Rows 7 and 9 take their largest and lowest values from S and T documents only, like
    /// their counts.
    #[test]
    fn rows_7_and_9_compare_only_gated_documents() {
        let main = measure(&good(), false);
        let mut report = good();
        let poster = scores_of(&mut report, "c/poster.docx"); // class N
        (poster.dx_p95, poster.dy_p95, poster.ssim_min) = (Some(5.0), Some(5.0), 0.5);
        let old = scores_of(&mut report, "c/old.docx"); // no class
        (old.dx_p95, old.dy_p95, old.ssim_min) = (Some(5.0), Some(5.0), 0.5);
        let branch = measure(&report, false);
        for index in [6, 8] {
            assert_eq!(branch[index].text, main[index].text, "row {}", index + 1);
            assert!(!branch[index].worse_than(&main[index]), "row {}", index + 1);
        }
    }

    /// Rows 5, 7 and 9 print the lowest or largest value, so a worse one is marked even when
    /// no document crosses a threshold (#131).
    #[test]
    fn a_worse_lowest_or_largest_value_is_marked() {
        let main = measure(&good(), false);
        let compare = |row: usize, change: &dyn Fn(&mut Report)| {
            let mut report = good();
            change(&mut report);
            let branch = measure(&report, false);
            let index = row - 1;
            (branch[index].pass, branch[index].worse_than(&main[index]))
        };
        // 5: the lowest text_score, 0.9950 on main.
        let text =
            |value: f64| move |r: &mut Report| scores_of(r, "c/letter.docx").text_score = value;
        assert_eq!(compare(5, &text(0.9949)), (Some(true), true));
        assert_eq!(compare(5, &text(0.9951)), (Some(true), false));
        // 7: the largest p95, dx 0.4 pt and dy 1.5 pt on main.
        let dx =
            |value: f64| move |r: &mut Report| scores_of(r, "c/E-rule.docx").dx_p95 = Some(value);
        let dy =
            |value: f64| move |r: &mut Report| scores_of(r, "c/letter.docx").dy_p95 = Some(value);
        assert_eq!(compare(7, &dx(0.401)), (Some(true), true));
        assert_eq!(compare(7, &dy(1.501)), (Some(true), true));
        assert_eq!(compare(7, &dy(1.0)), (Some(true), false));
        // 9: the lowest page, 0.96 on main.
        let page =
            |value: f64| move |r: &mut Report| scores_of(r, "c/letter.docx").ssim_min = value;
        assert_eq!(compare(9, &page(0.9599)), (Some(true), true));
        assert_eq!(compare(9, &page(0.97)), (Some(true), false));
    }

    /// Twenty documents in class S, made by `make`, which sees the index of each.
    fn twenty(make: impl Fn(usize, &mut Scores)) -> Vec<Measure> {
        let files = (0..20)
            .map(|index| {
                let mut s = scores(2, 1.0, 100, 100, &["Carlito"]);
                make(index, &mut s);
                file(&format!("c/d{index:02}.docx"), Status::Opened, Some(s))
            })
            .collect();
        measure(&Report::new(files), false)
    }

    /// Each threshold, met exactly and missed by a little.
    #[test]
    fn thresholds_are_met_at_their_bounds() {
        let row = |number: usize, make: &dyn Fn(usize, &mut Scores)| twenty(make)[number - 1].pass;
        // 4: 19 of 20 = 95 % same pages; 18 of 20 is not enough.
        assert_eq!(
            row(4, &|i, s| if i == 0 {
                s.pages = 3
            }),
            Some(true)
        );
        assert_eq!(
            row(4, &|i, s| if i < 2 {
                s.pages = 3
            }),
            Some(false)
        );
        // 5: 0.99 per document.
        assert_eq!(
            row(5, &|i, s| if i == 0 {
                s.text_score = 0.99
            }),
            Some(true)
        );
        assert_eq!(
            row(5, &|i, s| if i == 0 {
                s.text_score = 0.9899
            }),
            Some(false)
        );
        // 6: 0.95 per document (the corpus stays above 0.98).
        let lines = |agreeing: usize, fraction: f64| {
            move |i: usize, s: &mut Scores| {
                if i == 0 {
                    (s.lines_agreeing, s.line_agreement) = (agreeing, fraction);
                }
            }
        };
        assert_eq!(row(6, &lines(95, 0.95)), Some(true));
        assert_eq!(row(6, &lines(94, 0.94)), Some(false));
        // 4: every rule document E-* has Word's page count, whatever the share.
        let rule_off = |i: usize, s: &mut Scores| {
            if i == 0 {
                s.pages = 3;
            }
        };
        let with_rule = |make: &dyn Fn(usize, &mut Scores), rule: bool| {
            let files = (0..20)
                .map(|index| {
                    let mut s = scores(2, 1.0, 100, 100, &["Carlito"]);
                    make(index, &mut s);
                    let name = if rule && index == 0 {
                        "E-rule".to_owned()
                    } else {
                        format!("d{index:02}")
                    };
                    file(&format!("c/{name}.docx"), Status::Opened, Some(s))
                })
                .collect();
            measure(&Report::new(files), false)[3].pass
        };
        assert_eq!(
            with_rule(&rule_off, false),
            Some(true),
            "19 of 20 is enough"
        );
        assert_eq!(
            with_rule(&rule_off, true),
            Some(false),
            "not for a rule document"
        );
        // 7: medians 0.5 pt and p95 2 pt.
        assert_eq!(
            row(7, &|_, s| (s.dx_median, s.dy_median, s.dy_p95) =
                (Some(0.5), Some(0.5), Some(2.0))),
            Some(true)
        );
        assert_eq!(
            row(7, &|i, s| if i == 0 {
                s.dx_median = Some(0.501)
            }),
            Some(false)
        );
        assert_eq!(
            row(7, &|i, s| if i == 0 {
                s.dy_median = Some(0.501)
            }),
            Some(false)
        );
        assert_eq!(
            row(7, &|_, s| (s.dx_p95, s.dy_p95) = (Some(2.0), Some(2.0))),
            Some(true)
        );
        assert_eq!(
            row(7, &|i, s| if i == 0 {
                s.dx_p95 = Some(2.001)
            }),
            Some(false)
        );
        assert_eq!(
            row(7, &|i, s| if i == 0 {
                s.dy_p95 = Some(2.001)
            }),
            Some(false)
        );
        // 8: 0.97 over all breaks: 97 of 100 is enough, 96 is not.
        let breaks = |missed: usize| {
            move |i: usize, s: &mut Scores| {
                s.page_breaks = 5;
                s.page_breaks_agreeing = if i < missed { 4 } else { 5 };
            }
        };
        assert_eq!(row(8, &breaks(3)), Some(true));
        assert_eq!(row(8, &breaks(4)), Some(false));
        // 9: class S, every page 0.90 and the mean 0.95.
        assert_eq!(
            row(9, &|_, s| (s.ssim_min, s.ssim_mean) = (0.90, 0.95)),
            Some(true)
        );
        assert_eq!(
            row(9, &|i, s| if i == 0 {
                s.ssim_min = 0.8999
            }),
            Some(false)
        );
        assert_eq!(
            row(9, &|i, s| if i == 0 {
                s.ssim_mean = 0.9499
            }),
            Some(false)
        );
        // 9: class T, 0.02 lower.
        let twin = |page: f64, mean: f64| {
            move |_: usize, s: &mut Scores| {
                s.word_fonts = Some(vec!["Calibri".to_owned()]);
                (s.ssim_min, s.ssim_mean) = (page, mean);
            }
        };
        assert_eq!(row(9, &twin(0.88, 0.93)), Some(true));
        assert_eq!(row(9, &twin(0.8799, 0.93)), Some(false));
        assert_eq!(row(9, &twin(0.88, 0.9299)), Some(false));
    }

    #[test]
    fn speed_and_memory_have_their_bounds_and_tolerances() {
        let row = |ms: u64, kb: u64| {
            let mut f = file("c/a.docx", Status::Opened, None);
            (f.duration_ms, f.peak_rss_kb) = (ms, Some(kb));
            measure(&Report::new(vec![f]), false).swap_remove(9)
        };
        // 200 MB is 195,312.5 KiB.
        assert_eq!(row(4_999, 195_312).pass, Some(true));
        assert_eq!(row(5_000, 195_312).pass, Some(false));
        assert_eq!(row(4_999, 195_313).pass, Some(false));
        assert!(row(100, 50_000).text.ends_with("peak 51 MB (opening only)"));
        // Once the run renders, opening and rendering together.
        let mut rendered = file("c/a.docx", Status::Opened, None);
        (rendered.duration_ms, rendered.peak_rss_kb) = (100, Some(50_000));
        rendered.layout_report = Some(Vec::new());
        let row10 = measure(&Report::new(vec![rendered]), false).swap_remove(9);
        assert!(row10.text.ends_with("peak 51 MB"), "{}", row10.text);
        // Memory: 16 MiB more than main is noise, more than that is worse.
        let main = row(100, 50_000);
        assert!(!row(100, 50_000 + 16 * 1024).worse_than(&main));
        assert!(row(100, 50_000 + 16 * 1024 + 1).worse_than(&main));
    }

    #[test]
    fn nothing_scored_leaves_the_render_criteria_unmeasured() {
        let report = Report::new(vec![file("c/a.docx", Status::Opened, None)]);
        let measures = measure(&report, false);
        assert!(passes(&measures));
        for index in [2, 3, 4, 5, 6, 7, 8, 10, 11] {
            assert_eq!(measures[index].pass, None, "criterion {}", index + 1);
        }
    }

    fn entry(kind: &str, message: &str) -> crate::child::LayoutEntry {
        crate::child::LayoutEntry {
            kind: kind.to_owned(),
            location: "document".to_owned(),
            message: message.to_owned(),
        }
    }

    const CALIBRI: &str = "\"Calibri\" → \"Carlito\" (metric-compatible substitute)";
    const DRAWING: &str = "a drawing that shows no picture (a shape, chart or group) is left out";

    /// A rendered reference document: Word's text in `fonts`, our PDF embedding
    /// `pdf_fonts`, holding `content`, and its layout report `entries`.
    fn rendered(
        path: &str,
        fonts: &[&str],
        pdf_fonts: &[&str],
        content: &[&str],
        entries: &[(&str, &str)],
    ) -> FileResult {
        let mut scores = scores(2, 1.0, 10, 10, fonts);
        scores.pdf_fonts = Some(pdf_fonts.iter().map(|font| (*font).to_owned()).collect());
        let mut file = file(path, Status::Opened, Some(scores));
        file.content = Some(content.iter().map(|kind| (*kind).to_owned()).collect());
        file.layout_report = Some(
            entries
                .iter()
                .map(|(kind, message)| entry(kind, message))
                .collect(),
        );
        file
    }

    #[test]
    fn fonts_of_word_s_text_are_drawn_or_reported() {
        let row = |files: Vec<FileResult>| measure(&Report::new(files), false).swap_remove(10);
        let letter = |entries: &[(&str, &str)]| {
            rendered(
                "c/letter.docx",
                &["Calibri", "Carlito", "Times New Roman"],
                &["Carlito-Bold", "TimesNewRomanPSMT"],
                &[],
                entries,
            )
        };
        let reported = row(vec![letter(&[("font-substituted", CALIBRI)])]);
        assert_eq!(reported.pass, Some(true), "{}", reported.text);
        assert_eq!(
            reported.text,
            "0 fonts unreported in 0 documents, of 1 checked; 0 kinds of content unreported \
             in 0 documents, of 1 checked"
        );
        // Calibri is neither in our PDF nor named by a substitution.
        let silent = row(vec![letter(&[(
            "font-substituted",
            "\"Arial\" → \"Liberation Sans\" (metric-compatible substitute)",
        )])]);
        assert_eq!(silent.pass, Some(false));
        assert!(
            silent.text.starts_with("1 fonts unreported in 1 documents"),
            "{}",
            silent.text
        );
        // Only a substitution names a font: a feature entry that quotes it does not.
        let other_kind = row(vec![letter(&[("feature-unsupported", CALIBRI)])]);
        assert_eq!(other_kind.pass, Some(false));
        // A family is told from another that starts like it.
        let symbols = rendered(
            "c/s.docx",
            &["Noto Sans"],
            &["NotoSansSymbols-Regular"],
            &[],
            &[],
        );
        assert_eq!(row(vec![symbols]).pass, Some(false));
        // Without Word's fonts of the text or our PDF's, the fonts are not checked.
        let mut old = letter(&[]);
        old.scores.as_mut().unwrap().word_text_fonts = None;
        let unchecked = row(vec![old]);
        assert!(
            unchecked.text.contains("of 0 checked; "),
            "{}",
            unchecked.text
        );
        assert_eq!(unchecked.pass, Some(true));
    }

    /// The fonts of a story the layout leaves out, by the report's own entry, or of the
    /// comments, which Word's references do not show, are not checked.
    #[test]
    fn fonts_of_stories_left_out_are_not_checked() {
        let row = |stories: &[(&str, &[&str])], entries: &[(&str, &str)]| {
            let mut file = rendered("c/notes.docx", &[], &["Carlito-Regular"], &[], entries);
            file.scores.as_mut().unwrap().word_text_fonts = Some(
                stories
                    .iter()
                    .map(|(story, fonts)| {
                        (
                            (*story).to_owned(),
                            fonts.iter().map(|font| (*font).to_owned()).collect(),
                        )
                    })
                    .collect(),
            );
            silences(&Report::new(vec![file])).found
        };
        let endnotes: &[(&str, &[&str])] = &[("main", &["Carlito"]), ("endnotes", &["Cambria"])];
        assert_eq!(
            row(endnotes, &[]),
            [Silence {
                path: "c/notes.docx".to_owned(),
                what: Unreported::Font("Cambria".to_owned()),
            }]
        );
        let left_out = (
            "feature-unsupported",
            "endnotes are not laid out yet: their texts are left out",
        );
        assert!(row(endnotes, &[left_out]).is_empty());
        // Only an unsupported feature's entry says so.
        assert_eq!(
            row(endnotes, &[("overflow", "endnotes are not laid out yet")]).len(),
            1
        );
        assert!(row(&[("main", &["Carlito"]), ("comments", &["Cambria"])], &[]).is_empty());
        // A font of a story that is laid out is checked even when another story has it.
        assert_eq!(
            row(
                &[("main", &["Cambria"]), ("endnotes", &["Cambria"])],
                &[left_out]
            )
            .len(),
            1
        );
    }

    /// Each entry STORIES_LEFT_OUT names leaves its story's fonts unchecked, and the stories
    /// of types the scan says no section shows are not checked either.
    #[test]
    fn every_story_left_out_and_not_shown_goes_unchecked() {
        let found = |story: &str, entries: &[(&str, &str)], not_shown: Option<Vec<String>>| {
            let mut file = rendered("c/a.docx", &[], &["Carlito-Regular"], &[], entries);
            file.scores.as_mut().unwrap().word_text_fonts = Some(BTreeMap::from([
                ("main".to_owned(), vec!["Carlito".to_owned()]),
                (story.to_owned(), vec!["Cambria".to_owned()]),
            ]));
            file.stories_not_shown = not_shown;
            silences(&Report::new(vec![file])).found.len()
        };
        for (story, words) in STORIES_LEFT_OUT {
            for word in words {
                assert_eq!(
                    found(story, &[("feature-unsupported", word)], None),
                    0,
                    "{word}"
                );
            }
            if !words.is_empty() {
                assert_eq!(found(story, &[], None), 1, "{story}");
            }
        }
        for story in ["first-page-header", "even-pages-footer"] {
            assert_eq!(found(story, &[], None), 1, "{story}");
            assert_eq!(
                found(story, &[], Some(vec![story.to_owned()])),
                0,
                "{story}"
            );
            assert_eq!(
                found(story, &[], Some(vec!["primary-header".to_owned()])),
                1,
                "{story}"
            );
        }
    }

    /// A finding counts as new only where main checked the same thing: fonts where main
    /// checked fonts, content where it checked content.
    #[test]
    fn findings_are_new_only_where_main_checked_them() {
        let finding = |what: Unreported| Silence {
            path: "c/a.docx".to_owned(),
            what,
        };
        let branch = Silences {
            found: vec![
                finding(Unreported::Font("Calibri".to_owned())),
                finding(Unreported::Content("chart".to_owned())),
            ],
            ..Silences::default()
        };
        let fonts_only = Silences {
            fonts_of: BTreeSet::from(["c/a.docx".to_owned()]),
            ..Silences::default()
        };
        let content_only = Silences {
            content_of: BTreeSet::from(["c/a.docx".to_owned()]),
            ..Silences::default()
        };
        assert_eq!(branch.new_against(&fonts_only), 1);
        assert_eq!(branch.new_against(&content_only), 1);
        assert_eq!(branch.new_against(&Silences::default()), 0);
    }

    #[test]
    fn long_lists_are_cut_at_twenty() {
        let items: Vec<String> = (1..=23).map(|n| format!("d{n}")).collect();
        let line = listed(&items);
        assert!(line.starts_with("d1, d2, "), "{line}");
        assert!(line.ends_with("d20 and 3 more"), "{line}");
        assert_eq!(listed(&items[..20]), items[..20].join(", "));
    }

    #[test]
    fn a_pdf_font_is_matched_to_its_family() {
        let fonts = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert!(embeds(&fonts(&["LiberationSans-Bold"]), "Liberation Sans"));
        assert!(embeds(&fonts(&["LiberationSans"]), "liberation sans"));
        assert!(embeds(&fonts(&["TimesNewRomanPSMT"]), "Times New Roman"));
        assert!(embeds(&fonts(&["ArialMT"]), "Arial"));
        assert!(embeds(&fonts(&["MS-Gothic"]), "MS Gothic"));
        assert!(embeds(
            &fonts(&["LibreBarcode128-Regular"]),
            "Libre Barcode 128"
        ));
        // Microsoft's bold and italic faces put PS before the style.
        assert!(embeds(
            &fonts(&["TimesNewRomanPS-BoldMT"]),
            "Times New Roman"
        ));
        assert!(embeds(
            &fonts(&["TimesNewRomanPS-ItalicMT"]),
            "Times New Roman"
        ));
        assert!(embeds(&fonts(&["CourierNewPS-BoldMT"]), "Courier New"));
        assert!(embeds(&fonts(&["Arial-BoldMT"]), "Arial"));
        assert!(!embeds(&fonts(&["TimesNewRomanPS-BoldMT"]), "Times"));
        assert!(!embeds(&fonts(&["NotoSansSymbols-Regular"]), "Noto Sans"));
        assert!(!embeds(&fonts(&["ArialNarrow"]), "Arial"));
        assert!(!embeds(&fonts(&["Carlito-Regular"]), "Calibri"));
        assert!(!embeds(&fonts(&["Carlito"]), "(unresolved)"));
        assert!(!embeds(&[], "Carlito"));
        assert!(!embeds(&fonts(&["X"]), ""));
    }

    #[test]
    fn a_substitution_names_its_first_quoted_family() {
        assert_eq!(substituted(CALIBRI), Some("Calibri"));
        assert_eq!(
            substituted("\"Noto Sans\" has no glyph for some characters, such as U+25CF '●'"),
            Some("Noto Sans")
        );
        assert_eq!(
            substituted("cannot read \"X\"; its text is drawn in other fonts"),
            None
        );
        assert_eq!(substituted("\"unterminated"), None);
    }

    #[test]
    fn content_the_layout_does_not_lay_out_is_reported() {
        let row = |files: Vec<FileResult>| measure(&Report::new(files), false).swap_remove(10);
        let poster = |entries: &[(&str, &str)]| {
            rendered(
                "c/poster.docx",
                &[],
                &[],
                &["chart", "text wrapping"],
                entries,
            )
        };
        let named = row(vec![poster(&[
            ("feature-unsupported", DRAWING),
            (
                "feature-unsupported",
                "text does not wrap around floating objects yet: it runs over or under them",
            ),
        ])]);
        assert_eq!(named.pass, Some(true), "{}", named.text);
        let one = row(vec![poster(&[("feature-unsupported", DRAWING)])]);
        assert_eq!(one.pass, Some(false));
        assert!(
            one.text
                .ends_with("1 kinds of content unreported in 1 documents, of 1 checked"),
            "{}",
            one.text
        );
        assert_eq!(
            findings(&Report::new(vec![poster(&[(
                "feature-unsupported",
                DRAWING
            )])])),
            ["Row 11, not reported: c/poster.docx (text wrapping)"]
        );
        // Only an unsupported feature's entry names content.
        let overflow = row(vec![poster(&[
            ("feature-unsupported", DRAWING),
            ("overflow", "text does not wrap around floating objects"),
        ])]);
        assert_eq!(overflow.pass, Some(false));
        // A kind of another runner's version is not checked.
        let unknown = row(vec![rendered("c/a.docx", &[], &[], &["hologram"], &[])]);
        assert_eq!(unknown.pass, Some(true));
        // Markup that could not be read fails the row.
        let mut unread = rendered("c/a.docx", &[], &[], &[], &[]);
        unread.content = None;
        unread.content_error = Some("/word/document.xml: not well-formed".to_owned());
        let unreadable = row(vec![unread.clone()]);
        assert_eq!(unreadable.pass, Some(false));
        assert_eq!(
            findings(&Report::new(vec![unread])),
            [
                "Row 11, not reported: c/a.docx (markup not read: /word/document.xml: not well-formed)"
            ]
        );
        // A document that was not rendered has no report to check.
        let mut unrendered = poster(&[]);
        unrendered.layout_report = None;
        assert_eq!(row(vec![unrendered]).pass, None);
    }

    /// Row 11 counts what the branch leaves unreported that main reported, so that a new
    /// silence shows even when another one goes; a main report of an older runner, which
    /// did not check, adds none.
    #[test]
    fn row_11_compares_with_main() {
        let doc = |content: &[&str]| rendered("c/a.docx", &["Carlito"], &["Carlito"], content, &[]);
        let main_report = Report::new(vec![doc(&["chart"])]);
        let main = measure(&main_report, false);
        let branch_report = Report::new(vec![doc(&["columns"])]);
        let branch = measure_against(&branch_report, Some(&main_report), false);
        assert!(
            branch[10].text.ends_with("; 1 new against main"),
            "{}",
            branch[10].text
        );
        assert!(branch[10].worse_than(&main[10]));
        let same = measure_against(&main_report, Some(&main_report), false);
        assert!(same[10].text.ends_with("; 0 new against main"));
        assert!(!same[10].worse_than(&main[10]));
        // Main without content: nothing new, and nothing measured there to be worse than.
        let mut older = doc(&[]);
        older.content = None;
        older.scores.as_mut().unwrap().word_text_fonts = None;
        let older_report = Report::new(vec![older]);
        let before = measure(&older_report, false);
        assert_eq!(before[10].pass, None);
        let against_older = measure_against(&branch_report, Some(&older_report), false);
        assert!(against_older[10].text.ends_with("; 0 new against main"));
        assert!(!against_older[10].worse_than(&before[10]));
    }

    #[test]
    fn entries_degrading_a_required_feature_fail_row_12() {
        let borders = ("feature-unsupported", "paragraph borders are not drawn yet");
        let wrap = (
            "feature-unsupported",
            "text does not wrap around floating objects yet: it runs over or under them",
        );
        let unknown = (
            "feature-unsupported",
            "a block-level w:altChunk is left out",
        );
        let row = |files: Vec<FileResult>| measure(&Report::new(files), false).swap_remove(11);
        let clean = row(vec![rendered("c/a.docx", &[], &[], &[], &[wrap])]);
        assert_eq!(clean.pass, Some(true));
        assert_eq!(clean.text, "0 of 1 reference documents");
        let files = vec![
            rendered("c/a.docx", &[], &[], &[], &[borders, borders, wrap]),
            rendered("c/b.docx", &[], &[], &[], &[unknown, borders]),
            // Not a reference document: row 12 does not look at it.
            {
                let mut file = rendered("c/c.docx", &[], &[], &[], &[borders]);
                file.scores = None;
                file
            },
        ];
        let report = Report::new(files);
        let measures = measure(&report, false);
        assert_eq!(measures[11].pass, Some(false));
        assert_eq!(
            measures[11].text,
            "2 of 2 reference documents (paragraph borders 2, unclassified 1)"
        );
        assert_eq!(
            findings(&report),
            [
                "Row 12, paragraph borders: c/a.docx, c/b.docx",
                "Row 12, unclassified: a block-level w:altChunk is left out: c/b.docx",
            ]
        );
        let none = Report::new(vec![file("c/a.docx", Status::Opened, None)]);
        assert_eq!(measure(&none, false)[11].pass, None);
    }

    /// Row 12 marks the branch worse for a degradation main does not have, even when the
    /// branch removes another.
    #[test]
    fn row_12_compares_with_main() {
        let borders = ("feature-unsupported", "paragraph borders are not drawn yet");
        let notes = ("feature-unsupported", "endnotes are not laid out yet");
        let main_report = Report::new(vec![rendered("c/a.docx", &[], &[], &[], &[borders])]);
        let main = measure(&main_report, false);
        let traded = Report::new(vec![rendered("c/a.docx", &[], &[], &[], &[notes])]);
        let branch = measure_against(&traded, Some(&main_report), false);
        assert!(
            branch[11].text.ends_with("; 1 new against main"),
            "{}",
            branch[11].text
        );
        assert!(branch[11].worse_than(&main[11]));
        let fixed = Report::new(vec![rendered("c/a.docx", &[], &[], &[], &[])]);
        let better = measure_against(&fixed, Some(&main_report), false);
        assert!(better[11].text.ends_with("; 0 new against main"));
        assert!(!better[11].worse_than(&main[11]));
        assert_eq!(better[11].pass, Some(true));
    }
}
