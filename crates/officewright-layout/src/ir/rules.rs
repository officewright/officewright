//! The rules `Document::validate` and `Document::clamp` apply to single fields. A check
//! rejects what no correct producer emits, such as a negative length where
//! WordprocessingML only has unsigned ones; a clamp brings a value that a document may
//! hold, but Word does not accept, within Word's limits.

use officewright_core::Pt;

use super::walk::{Checker, Clamper};
use crate::Result;

/// The limit Word sets on most lengths it stores in twips: 22 inches, 31680 twips.
pub(crate) const WORD_MAX_LENGTH: Pt = Pt(31680.0 / 20.0);

/// The largest length Word stores as a `short` of twips: 32767 twips.
pub(crate) const WORD_MAX_SHORT_LENGTH: Pt = Pt(i16::MAX as f64 / 20.0);

/// The length must not be negative.
pub(crate) fn non_negative(value: &Pt, checker: &mut Checker<'_>) -> Result<()> {
    if value.0 < 0.0 {
        return Err(checker.error(format!("{} pt is negative", value.0)));
    }
    Ok(())
}

/// The length, if there is one, must not be negative.
pub(crate) fn non_negative_if_set(value: &Option<Pt>, checker: &mut Checker<'_>) -> Result<()> {
    match value {
        Some(value) => non_negative(value, checker),
        None => Ok(()),
    }
}

/// Brings `value` within `min` to `max` and reports the change, naming the value as
/// `what`. A value that is not a number is left alone: `validate` rejects it.
pub(crate) fn limit(value: &mut Pt, min: Pt, max: Pt, what: &str, clamper: &mut Clamper) {
    let clamped = if value.0 < min.0 {
        min
    } else if value.0 > max.0 {
        max
    } else {
        return;
    };
    clamper.clamped(format!(
        "{what} {} pt is outside Word's range of {} to {} pt and becomes {} pt",
        value.0, min.0, max.0, clamped.0
    ));
    *value = clamped;
}

/// Word restricts a tab stop's position to ±22 inches (MS-OI29500 2.1.61 a).
pub(crate) fn tab_position(value: &mut Pt, clamper: &mut Clamper) {
    let max = WORD_MAX_LENGTH;
    limit(value, Pt(-max.0), max, "tab stop position", clamper);
}

/// Word restricts the spacing added after each character to ±22 inches (MS-OI29500
/// 2.1.96 b).
pub(crate) fn letter_spacing(value: &mut Pt, clamper: &mut Clamper) {
    let max = WORD_MAX_LENGTH;
    limit(value, Pt(-max.0), max, "letter spacing", clamper);
}

/// Word restricts the font size from which text is kerned to 3277 half-points
/// (MS-OI29500 2.1.83 a); negative sizes are rejected by `validate`.
pub(crate) fn kerning(value: &mut Option<Pt>, clamper: &mut Clamper) {
    if let Some(value) = value {
        limit(
            value,
            Pt(0.0),
            Pt(3277.0 / 2.0),
            "kerning threshold",
            clamper,
        );
    }
}

/// Word takes font sizes from 2 to 3277 half-points (ST_HpsMeasure, MS-OI29500 2.1.544
/// a); negative sizes are rejected by `validate`.
pub(crate) fn font_size(value: &mut Pt, clamper: &mut Clamper) {
    limit(value, Pt(1.0), Pt(3277.0 / 2.0), "font size", clamper);
}

/// Word takes the distance between a border and the text from 0 to 31 pt
/// (ST_PointMeasure, MS-OI29500 2.1.550 a); negative distances are rejected by `validate`.
pub(crate) fn border_space(value: &mut Pt, clamper: &mut Clamper) {
    limit(value, Pt(0.0), Pt(31.0), "border distance", clamper);
}

/// Word stores the distance between automatic tab stops as a `short` of twips, at least
/// 0 (MS-OI29500 2.1.399 a); negative distances are rejected by `validate`.
pub(crate) fn default_tab_stop(value: &mut Pt, clamper: &mut Clamper) {
    limit(
        value,
        Pt(0.0),
        WORD_MAX_SHORT_LENGTH,
        "default tab stop",
        clamper,
    );
}

/// The left, right, header, footer and gutter margins are at most 22 inches in Word
/// (MS-OI29500 2.1.218 a); negative ones are rejected by `validate`.
pub(crate) fn margin(value: &mut Pt, clamper: &mut Clamper) {
    limit(value, Pt(0.0), WORD_MAX_LENGTH, "margin", clamper);
}

/// The top and bottom margins are between -22 and 22 inches in Word (MS-OI29500
/// 2.1.218 b). A negative one fixes the edge of the body text, which the header or
/// footer may overlap (ECMA-376 Part 1, §17.6.11).
pub(crate) fn vertical_margin(value: &mut Pt, clamper: &mut Clamper) {
    let max = WORD_MAX_LENGTH;
    limit(value, Pt(-max.0), max, "margin", clamper);
}

/// Word takes a character scale (`w:w`) from 1 to 600 % (MS-OI29500 2.1.560 a). The
/// schema allows 0 as well; this reads it as 100 %, as the DOCX reader does, after
/// LibreOffice, which fixed tdf#106001 by treating such widths as normal. A Word reference
/// render of a rule document is to confirm it.
pub(crate) fn text_scale(value: &mut u16, clamper: &mut Clamper) {
    let clamped = match *value {
        0 => 100,
        601.. => 600,
        _ => return,
    };
    clamper.clamped(format!(
        "character scale {} % is outside Word's range of 1 to 600 % and becomes {clamped} %",
        *value
    ));
    *value = clamped;
}
