//! The vertical metrics of a face at a size: how far its lines reach above and below the
//! baseline, which sets the height of a line of text.

// reason: the layout itself uses this once pagination (plan PR 8) lays out pages; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use officewright_core::Pt;
use officewright_font::{FontFace, OutlineFormat};

/// When the typographic metrics of the `OS/2` table take the place of its Windows ones:
/// when the font sets `USE_TYPO_METRICS` (bit 7 of `fsSelection`), and the rule says so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum TypoRule {
    /// For fonts with TrueType outlines only. Word's line heights in
    /// `tests/corpus/word-rules/E-line-fonts.docx` show it: Aptos and Cascadia Mono
    /// (TrueType, flag set) keep their typographic line height (13.429 pt and 12.776 pt at
    /// 11 pt), Noto Sans (CFF, flag set) its Windows one (16.707 pt, not 14.98). Whether
    /// the outline format or something else of those fonts decides is open until a Word
    /// run with a TrueType build of Noto Sans.
    #[default]
    TrueTypeOnly,
    /// For every font that sets the flag, as the OpenType specification asks.
    Always,
    /// Never: the Windows metrics, as GDI used them.
    Never,
}

/// How far a face's lines reach, in points at a font size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LineMetrics {
    /// Above the baseline.
    pub(crate) ascent: Pt,
    /// Below the baseline, as a positive distance.
    pub(crate) descent: Pt,
    /// The space a line adds below the descent; Windows' external leading.
    pub(crate) leading: Pt,
}

impl LineMetrics {
    /// The height of a single-spaced line.
    pub(crate) fn height(self) -> Pt {
        Pt(self.ascent.0 + self.descent.0 + self.leading.0)
    }
}

/// The line metrics of `face` at `size`, as Word takes them: from the `OS/2` table's
/// `usWinAscent` and `usWinDescent`, with the external leading GDI adds when the
/// `hhea` line is taller ("Recommendations for OpenType fonts", baseline to baseline
/// distances); or from the typographic metrics when `rule` honours `USE_TYPO_METRICS`.
/// A face without an `OS/2` table has the `hhea` metrics. Values are not rounded.
pub(crate) fn line_metrics(face: &FontFace, size: Pt, rule: TypoRule) -> LineMetrics {
    let metrics = face.metrics();
    let scale = size.0 / f64::from(face.units_per_em().max(1));
    let scaled = |units: f64| Pt(units * scale);
    let hhea = metrics.hhea();
    let typo_rule = match rule {
        TypoRule::Always => true,
        TypoRule::Never => false,
        TypoRule::TrueTypeOnly => face.outline_format() == Some(OutlineFormat::TrueType),
    };
    if let (Some(typo), true) = (metrics.typo(), typo_rule && metrics.use_typo_metrics()) {
        return LineMetrics {
            ascent: scaled(f64::from(typo.ascender())),
            descent: scaled(-f64::from(typo.descender())),
            leading: scaled(f64::from(typo.line_gap()).max(0.0)),
        };
    }
    match metrics.win() {
        Some(win) => {
            let (ascent, descent) = (f64::from(win.ascent()), f64::from(win.descent()));
            let hhea_line = f64::from(hhea.ascender()) - f64::from(hhea.descender())
                + f64::from(hhea.line_gap());
            LineMetrics {
                ascent: scaled(ascent),
                descent: scaled(descent),
                leading: scaled((hhea_line - (ascent + descent)).max(0.0)),
            }
        }
        None => LineMetrics {
            ascent: scaled(f64::from(hhea.ascender())),
            descent: scaled(-f64::from(hhea.descender())),
            leading: scaled(f64::from(hhea.line_gap()).max(0.0)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{face, minimal, patched, with_typo_metrics};

    fn at_10(font: Vec<u8>, rule: TypoRule) -> (f64, f64, f64) {
        let metrics = line_metrics(&face(font), Pt(10.0), rule);
        let round = |pt: Pt| (pt.0 * 1e6).round() / 1e6;
        (
            round(metrics.ascent),
            round(metrics.descent),
            round(metrics.leading),
        )
    }

    #[test]
    fn takes_the_windows_metrics_and_the_external_leading() {
        // usWinAscent 900 and usWinDescent 250 of 1000 units at 10 pt; the hhea line,
        // 800 + 200 + 90, is shorter, so no external leading.
        for rule in [TypoRule::TrueTypeOnly, TypoRule::Always, TypoRule::Never] {
            assert_eq!(at_10(minimal(), rule), (9.0, 2.5, 0.0), "{rule:?}");
        }
        // An hhea line gap of 300 makes the hhea line 1300 units, 150 more than the
        // Windows one: external leading.
        let tall = patched(minimal(), b"hhea", 8, &300i16.to_be_bytes());
        assert_eq!(at_10(tall, TypoRule::default()), (9.0, 2.5, 1.5));
        // The size scales everything.
        let metrics = line_metrics(&face(minimal()), Pt(22.0), TypoRule::default());
        assert!((metrics.height().0 - 25.3).abs() < 1e-9, "{metrics:?}");
    }

    #[test]
    fn honours_use_typo_metrics_by_the_rule() {
        let font = with_typo_metrics(minimal());
        // A TrueType font: the typographic metrics, line gap included.
        assert_eq!(at_10(font.clone(), TypoRule::TrueTypeOnly), (8.0, 2.0, 0.9));
        assert_eq!(at_10(font.clone(), TypoRule::Always), (8.0, 2.0, 0.9));
        assert_eq!(at_10(font, TypoRule::Never), (9.0, 2.5, 0.0));
    }

    #[test]
    fn a_line_is_as_high_as_its_three_parts() {
        let metrics = LineMetrics {
            ascent: Pt(9.0),
            descent: Pt(2.5),
            leading: Pt(0.75),
        };
        assert_eq!(metrics.height(), Pt(12.25));
    }
}
