//! Laying out a paragraph for a width: its lines, and the space above and below it.

// reason: the layout itself uses this once pagination (plan PR 8) lays out pages; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use officewright_core::Pt;

use crate::fonts::Fonts;
use crate::ir::{CompatFlag, Document, Paragraph, ParagraphProps};
use crate::itemize::{Itemized, itemize, shape_runs};
use crate::lines::{Breaker, Input, Line};
use crate::report::Location;
use crate::shape::Shaper;

/// The space automatic spacing (`w:beforeAutospacing`, `w:afterAutospacing`) puts
/// above or below a paragraph, whatever its font size and spacing: Word's renders of
/// `E-autospacing.docx` (11 and 20 pt text, with and without explicit spacing).
const AUTO_SPACING: Pt = Pt(14.0);

/// A paragraph laid out for a width.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParagraphLayout {
    /// The width of the column it is laid out for.
    pub(crate) width: Pt,
    /// Its text and pieces, as the lines refer to them.
    pub(crate) itemized: Itemized,
    pub(crate) lines: Vec<Line>,
}

impl ParagraphLayout {
    /// The height of the lines.
    pub(crate) fn height(&self) -> Pt {
        Pt(self.lines.iter().map(|line| line.height.0).sum())
    }
}

/// Lays out `paragraph` of `document` in a column `width` wide; `location` is its place,
/// for the report.
pub(crate) fn lay_out(
    document: &Document,
    paragraph: &Paragraph,
    width: Pt,
    fonts: &mut Fonts<'_>,
    shaper: &mut Shaper,
    location: &Location,
) -> ParagraphLayout {
    let itemized = itemize(document, paragraph.inlines(), fonts, location);
    let shaped = shape_runs(document, &itemized, fonts, shaper, location);
    let input = Input {
        document,
        paragraph,
        width,
        location,
    };
    let lines = Breaker::new(input, &itemized, &shaped, fonts, shaper).lines();
    ParagraphLayout {
        width,
        itemized,
        lines,
    }
}

/// The space above a paragraph with `props` that follows one with `previous`, in a
/// document with `document`'s settings.
///
/// Automatic spacing is 14 pt, and between two paragraphs that both have it, the
/// space is that of one: the space below the first. The legacy option
/// `doNotUseHTMLParagraphAutoSpacing` turns automatic spacing off, so that the explicit
/// values apply (`E-autospacing-nohtml.docx`). Contextual spacing leaves the space out
/// next to a paragraph of the same style (ECMA-376 Part 1, §17.3.1.9).
pub(crate) fn space_before(
    document: &Document,
    previous: Option<&ParagraphProps>,
    props: &ParagraphProps,
) -> Pt {
    let auto = auto_spacing(document);
    let spacing = props.spacing();
    if let Some(previous) = previous {
        if props.contextual_spacing() && previous.style_key() == props.style_key() {
            return Pt(0.0);
        }
        if auto && spacing.before_auto() && previous.spacing().after_auto() {
            return Pt(0.0);
        }
    }
    if auto && spacing.before_auto() {
        AUTO_SPACING
    } else {
        spacing.before()
    }
}

/// The space below a paragraph with `props` that one with `next` follows; see
/// [`space_before`].
pub(crate) fn space_after(
    document: &Document,
    props: &ParagraphProps,
    next: Option<&ParagraphProps>,
) -> Pt {
    let spacing = props.spacing();
    if let Some(next) = next {
        if props.contextual_spacing() && next.style_key() == props.style_key() {
            return Pt(0.0);
        }
    }
    if auto_spacing(document) && spacing.after_auto() {
        AUTO_SPACING
    } else {
        spacing.after()
    }
}

/// Whether automatic spacing is on in `document`.
fn auto_spacing(document: &Document) -> bool {
    !document
        .settings()
        .has_compat_flag(&CompatFlag::DoNotUseHTMLParagraphAutoSpacing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{DocumentSettings, Spacing, StyleKey};

    fn spaced(before: f64, after: f64, before_auto: bool, after_auto: bool) -> ParagraphProps {
        ParagraphProps::default().with_spacing(
            Spacing::default()
                .with_before(Pt(before))
                .with_after(Pt(after))
                .with_before_auto(before_auto)
                .with_after_auto(after_auto),
        )
    }

    /// The space between paragraphs `a` and `b`.
    fn between(document: &Document, a: &ParagraphProps, b: &ParagraphProps) -> f64 {
        space_after(document, a, Some(b)).0 + space_before(document, Some(a), b).0
    }

    #[test]
    fn automatic_spacing_is_14_points_and_one_between_two() {
        // E-autospacing: shaded 20 pt lines 33.96 pt apart around an automatic
        // paragraph, 14 pt between two automatic ones, explicit values ignored.
        let document = Document::default();
        let plain = spaced(0.0, 0.0, false, false);
        let auto = spaced(0.0, 0.0, true, true);
        assert_eq!(between(&document, &plain, &auto), 14.0);
        assert_eq!(between(&document, &auto, &plain), 14.0);
        assert_eq!(between(&document, &auto, &auto), 14.0);
        let explicit = spaced(24.0, 24.0, true, true);
        assert_eq!(between(&document, &plain, &explicit), 14.0);
        assert_eq!(between(&document, &explicit, &plain), 14.0);
        let before_only = spaced(0.0, 0.0, true, false);
        assert_eq!(between(&document, &before_only, &plain), 0.0);
        assert_eq!(space_before(&document, None, &auto), Pt(14.0));
    }

    #[test]
    fn the_legacy_option_turns_automatic_spacing_off() {
        // E-autospacing-nohtml: the explicit values apply, 0 when there are none.
        let settings = DocumentSettings::default()
            .with_compat_flags([CompatFlag::DoNotUseHTMLParagraphAutoSpacing]);
        let document = Document::new(settings);
        let plain = spaced(0.0, 0.0, false, false);
        let auto = spaced(0.0, 0.0, true, true);
        assert_eq!(between(&document, &plain, &auto), 0.0);
        assert_eq!(between(&document, &auto, &auto), 0.0);
        let explicit = spaced(24.0, 24.0, true, true);
        assert_eq!(between(&document, &plain, &explicit), 24.0);
        assert_eq!(between(&document, &explicit, &plain), 24.0);
    }

    #[test]
    fn contextual_spacing_leaves_out_the_space_next_to_the_same_style() {
        let document = Document::default();
        let style = |key, contextual| {
            spaced(6.0, 10.0, false, false)
                .with_style_key(StyleKey::new(key))
                .with_contextual_spacing(contextual)
        };
        // Both sides ask for it: no space at all.
        assert_eq!(between(&document, &style(1, true), &style(1, true)), 0.0);
        // Only the second: its space above goes, the first's below stays.
        assert_eq!(between(&document, &style(1, false), &style(1, true)), 10.0);
        // Only the first: its space below goes.
        assert_eq!(between(&document, &style(1, true), &style(1, false)), 6.0);
        // Different styles: both stay.
        assert_eq!(between(&document, &style(1, true), &style(2, true)), 16.0);
    }
}
