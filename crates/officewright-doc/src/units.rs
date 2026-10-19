//! Lengths as WordprocessingML writes them, converted to points at the read boundary.

use officewright_core::Pt;

/// A length in twentieths of a point, or with a unit (ST_TwipsMeasure,
/// ST_SignedTwipsMeasure, ST_UniversalMeasure):
/// `1440`, `2.54cm`, `-0.5in`. `signed` allows negative values. `None` when the value is
/// not a length or is out of range for a document.
pub(crate) fn twips_measure(value: &str, signed: bool) -> Option<Pt> {
    measure(value, 20.0, signed)
}

/// A length in half-points, or with a unit (ST_HpsMeasure, ST_SignedHpsMeasure): `24`
/// is 12 pt.
pub(crate) fn half_points_measure(value: &str, signed: bool) -> Option<Pt> {
    measure(value, 2.0, signed)
}

/// A length given as a number of `1 / per_point` points, or as a number with a unit.
fn measure(value: &str, per_point: f64, signed: bool) -> Option<Pt> {
    // Not trimmed: Office takes atomic values only with their whitespace already
    // normalized, so " 720 " is no length to Word (MS-OI29500 2.1.1(a), #243).
    let number_end = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(number_end);
    if number.is_empty() || number.starts_with('+') || number.contains("..") {
        return None;
    }
    let number: f64 = number.parse().ok()?;
    let points = match unit {
        // A value without a unit may have a fraction too.
        "" => number / per_point,
        "pt" => number,
        "in" => number * 72.0,
        "cm" => number * 72.0 / 2.54,
        "mm" => number * 72.0 / 25.4,
        "pc" | "pi" => number * 12.0,
        _ => return None,
    };
    // Lengths in a document fit in 32 bits of twips; anything beyond is damage.
    let in_range = points.is_finite() && points.abs() <= f64::from(i32::MAX) / 20.0;
    (in_range && (signed || points >= 0.0)).then_some(Pt(points))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_twips_and_universal_measures() {
        assert_eq!(twips_measure("1440", false), Some(Pt(72.0)));
        assert_eq!(twips_measure("12240", false), Some(Pt(612.0)));
        assert_eq!(twips_measure("72pt", false), Some(Pt(72.0)));
        assert_eq!(twips_measure("1in", false), Some(Pt(72.0)));
        assert_eq!(twips_measure("1pc", false), Some(Pt(12.0)));
        assert_eq!(twips_measure("1pi", false), Some(Pt(12.0)));
        assert_eq!(twips_measure("-720", true), Some(Pt(-36.0)));
        assert_eq!(twips_measure("-0.5in", true), Some(Pt(-36.0)));
        let cm = twips_measure("2.54cm", false).unwrap();
        assert!((cm.0 - 72.0).abs() < 1e-9);
        let mm = twips_measure("25.4mm", false).unwrap();
        assert!((mm.0 - 72.0).abs() < 1e-9);
    }

    #[test]
    fn reads_half_points() {
        assert_eq!(half_points_measure("24", false), Some(Pt(12.0)));
        assert_eq!(half_points_measure("21", false), Some(Pt(10.5)));
        assert_eq!(half_points_measure("-6", true), Some(Pt(-3.0)));
        assert_eq!(half_points_measure("-6", false), None);
        assert_eq!(half_points_measure("11pt", false), Some(Pt(11.0)));
    }

    #[test]
    fn refuses_a_length_with_whitespace_around_it_as_word_does() {
        for value in [" 720", "720 ", "\t1in", "1in\n", "\u{a0}720"] {
            assert_eq!(twips_measure(value, false), None, "{value:?}");
            assert_eq!(half_points_measure(value, false), None, "{value:?}");
        }
    }

    #[test]
    fn refuses_what_is_not_a_length() {
        for value in [
            "", "pt", "-720", "1 in", "1km", "+5", "1..2", "abc", "1e9", "NaN", "inf",
        ] {
            assert_eq!(twips_measure(value, false), None, "{value:?}");
        }
        assert_eq!(twips_measure("99999999999", true), None);
        assert_eq!(twips_measure("--1", true), None);
    }
}
