//! `corpus-runner gate`: the release criteria of Milestone 4 over a run's report, printed as
//! a markdown table with the target, the value on main and the value on the branch.

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
/// neither (N), or not known because its reference has no formatting export.
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
    if fonts.iter().all(same) {
        Class::Same
    } else if fonts
        .iter()
        .all(|font| same(font) || TWINNED.contains(&font.as_str()))
    {
        Class::Twin
    } else {
        Class::Other
    }
}

/// One criterion's value on one report.
#[derive(Clone, Debug, PartialEq)]
pub struct Measure {
    pub text: String,
    /// Whether the target is met; `None` when there was nothing to measure.
    pub pass: Option<bool>,
    /// A number to compare with main, whether more of it is better, and by how much it
    /// may move before it counts as worse (timings vary from run to run).
    pub key: Option<f64>,
    pub higher_is_better: bool,
    pub tolerance: f64,
}

impl Measure {
    fn counted(text: String, failures: usize) -> Self {
        Measure {
            text,
            pass: Some(failures == 0),
            key: Some(failures as f64),
            higher_is_better: false,
            tolerance: 0.0,
        }
    }

    fn unmeasured(text: &str) -> Self {
        Measure {
            text: text.to_owned(),
            pass: None,
            key: None,
            higher_is_better: false,
            tolerance: 0.0,
        }
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
        "every error expected; none on a reference document",
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
        "every substitution and unsupported feature reported",
        "every document",
    ),
    (
        "Required features",
        "no degradation of a required feature",
        "reference documents",
    ),
];

/// A scored document with its file name and class.
struct Scored<'a> {
    name: &'a str,
    scores: &'a Scores,
    class: Class,
}

impl Scored<'_> {
    fn is_rule(&self) -> bool {
        self.name.starts_with("E-")
    }
}

fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// The value of every criterion on the report.
pub fn measure(report: &Report, same_fonts: bool) -> Vec<Measure> {
    let scored: Vec<Scored> = report
        .files
        .iter()
        .filter_map(|file| {
            let scores = file.scores.as_ref()?;
            Some(Scored {
                name: file.path.rsplit('/').next().unwrap_or(&file.path),
                scores,
                class: class(scores, same_fonts),
            })
        })
        .collect();
    let gated: Vec<&Scored> = scored
        .iter()
        .filter(|doc| matches!(doc.class, Class::Same | Class::Twin))
        .collect();
    let has_reference = |file: &FileResult| file.scores.is_some() || file.score_error.is_some();
    let mut out = Vec::new();

    // 1. Crashes.
    let crashes = report
        .files
        .iter()
        .filter(|file| file.status.is_robustness_failure())
        .count();
    out.push(Measure::counted(crashes.to_string(), crashes));

    // 2. Refusals.
    let unexpected = report
        .files
        .iter()
        .filter(|file| file.status == Status::UnexpectedError)
        .count();
    let refused = report
        .files
        .iter()
        .filter(|file| has_reference(file) && file.status != Status::Opened)
        .count();
    out.push(Measure::counted(
        format!("{unexpected} unexpected errors, {refused} reference documents not opened"),
        unexpected + refused,
    ));

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
            key: Some(round(share)),
            higher_is_better: true,
            tolerance: 0.0,
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
        out.push(Measure::counted(
            format!(
                "{below} of {} below 0.99 (lowest {lowest:.4})",
                scored.len()
            ),
            below,
        ));
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
                "{:.4} overall, {below} documents below 0.95, {rules_below} `E-*` below 1.0",
                overall
            ),
            pass: Some(overall >= 0.98 && below == 0 && rules_below == 0),
            key: Some(round(overall)),
            higher_is_better: true,
            tolerance: 0.0,
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
        out.push(Measure::counted(
            format!(
                "{off} of {} documents off (largest p95: dx {} pt, dy {} pt)",
                placed.len(),
                worst(|s| s.dx_p95),
                worst(|s| s.dy_p95)
            ),
            off,
        ));
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
            key: Some(round(overall)),
            higher_is_better: true,
            tolerance: 0.0,
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
        out.push(Measure::counted(
            format!(
                "{below} of {} documents below (lowest page {lowest:.4})",
                gated.len()
            ),
            below,
        ));
    }

    // 10. Speed and memory: opening only, until the runner renders.
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
        .max();
    out.push(Measure {
        text: format!(
            "slowest {:.2} s, peak {} (opening only)",
            slowest as f64 / 1000.0,
            peak.map_or("not measured".to_owned(), |kb| format!("{} MiB", kb / 1024))
        ),
        pass: Some(slowest < 5_000 && peak.is_none_or(|kb| kb < 200 * 1000 * 1000 / 1024)),
        key: Some(slowest as f64),
        higher_is_better: false,
        // Milliseconds: the same file takes a different time on every run.
        tolerance: 500.0,
    });

    // 11 and 12 need the layout report.
    out.push(Measure::unmeasured(
        "not measured before the layout report (layout PR 10)",
    ));
    out.push(Measure::unmeasured(
        "not measured before the layout report (layout PR 10)",
    ));
    out
}

/// How the documents divide, for the line above the table.
pub fn census(report: &Report, same_fonts: bool) -> String {
    let mut counts = [0usize; 4];
    for scores in report.files.iter().filter_map(|file| file.scores.as_ref()) {
        let index = match class(scores, same_fonts) {
            Class::Same => 0,
            Class::Twin => 1,
            Class::Other => 2,
            Class::Unknown => 3,
        };
        if let Some(count) = counts.get_mut(index) {
            *count += 1;
        }
    }
    let [same, twin, other, unknown] = counts;
    format!(
        "{} files, {} scored against Word's references: S {same}, T {twin}, N {other}, no class {unknown}",
        report.files.len(),
        same + twin + other + unknown
    )
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
                let worse = match (before.key, measure.key) {
                    (Some(old), Some(new)) if measure.higher_is_better => {
                        new < old - measure.tolerance
                    }
                    (Some(old), Some(new)) => new > old + measure.tolerance,
                    _ => false,
                };
                let mark = if worse { " **(worse than main)**" } else { "" };
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
            "5 files, 4 scored against Word's references: S 1, T 1, N 1, no class 1"
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
            "2 unexpected errors, 1 reference documents not opened"
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

    #[test]
    fn nothing_scored_leaves_the_render_criteria_unmeasured() {
        let report = Report::new(vec![file("c/a.docx", Status::Opened, None)]);
        let measures = measure(&report, false);
        assert!(passes(&measures));
        for index in [2, 3, 4, 5, 6, 7, 8, 10, 11] {
            assert_eq!(measures[index].pass, None, "criterion {}", index + 1);
        }
    }
}
