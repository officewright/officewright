//! Numbers as text, in the formats of page numbers, fields and list labels.

// reason: page numbers (plan PR 8b) use this first, and list labels (PR 11) after; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use crate::ir::NumberFormat;

/// The marks of [`NumberFormat::Chicago`], in order; after the last they start again,
/// doubled, then tripled.
const CHICAGO: [char; 4] = ['*', '\u{2020}', '\u{2021}', '\u{00A7}'];

/// The longest a letter or Chicago number is written: Word repeats one mark for each 26 (or
/// 4) past the first round, so a large number would make a long text. Hypothesis until a
/// Word reference (`E-number-formats.docx`) shows what Word writes there.
const MAX_REPEATS: u32 = 64;

/// `number` written in `format`, and whether the format is one the layout writes as
/// decimal instead, which the caller reports.
pub(crate) fn number_text(number: u32, format: &NumberFormat) -> (String, bool) {
    let text = match format {
        NumberFormat::Decimal => number.to_string(),
        NumberFormat::DecimalZero => format!("{number:02}"),
        NumberFormat::LowerLetter => letters(number, false),
        NumberFormat::UpperLetter => letters(number, true),
        NumberFormat::LowerRoman => roman(number).to_lowercase(),
        NumberFormat::UpperRoman => roman(number),
        NumberFormat::Ordinal => ordinal(number),
        NumberFormat::Chicago => chicago(number),
        NumberFormat::Bullet | NumberFormat::None => String::new(),
        NumberFormat::Other(_) => return (number.to_string(), true),
    };
    (text, false)
}

/// `number` as letters: a to z, then aa to zz, then aaa, one letter repeated once more
/// for each round of 26; 0 as decimal.
fn letters(number: u32, upper: bool) -> String {
    if number == 0 {
        return number.to_string();
    }
    let index = (number - 1) % 26;
    let repeats = ((number - 1) / 26 + 1).min(MAX_REPEATS);
    let base = if upper { b'A' } else { b'a' };
    let letter = char::from(base + index as u8);
    std::iter::repeat_n(letter, repeats as usize).collect()
}

/// `number` in capital Roman numerals, thousands as repeated `M`; 0 as decimal.
fn roman(number: u32) -> String {
    if number == 0 {
        return number.to_string();
    }
    const PARTS: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    // Thousands past the bound of repeats are not written: Word's fields stop far below.
    let mut rest = number.min(MAX_REPEATS * 1000 + 999);
    let mut text = String::new();
    for (value, part) in PARTS {
        while rest >= value {
            text.push_str(part);
            rest -= value;
        }
    }
    text
}

/// `number` with its English ordinal suffix: 1st, 2nd, 3rd, 4th, 11th, 12th, 13th, 21st.
fn ordinal(number: u32) -> String {
    // Each suffix after a digit that takes it, as it is read: the digit is left out.
    let example = match (number % 10, number % 100) {
        (_, 11..=13) => "4th",
        (1, _) => "1st",
        (2, _) => "2nd",
        (3, _) => "3rd",
        _ => "4th",
    };
    format!("{number}{}", example.get(1..).unwrap_or_default())
}

/// `number` in Chicago marks: *, †, ‡, §, then the same doubled, tripled; 0 as decimal.
fn chicago(number: u32) -> String {
    if number == 0 {
        return number.to_string();
    }
    let index = ((number - 1) % 4) as usize;
    let repeats = ((number - 1) / 4 + 1).min(MAX_REPEATS);
    let mark = CHICAGO.get(index).copied().unwrap_or('*');
    std::iter::repeat_n(mark, repeats as usize).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(number: u32, format: NumberFormat) -> String {
        number_text(number, &format).0
    }

    #[test]
    fn numbers_are_written_in_each_format() {
        assert_eq!(text(7, NumberFormat::Decimal), "7");
        assert_eq!(text(7, NumberFormat::DecimalZero), "07");
        assert_eq!(text(123, NumberFormat::DecimalZero), "123");
        let letters: Vec<String> = [1, 26, 27, 52, 53]
            .into_iter()
            .map(|n| text(n, NumberFormat::LowerLetter))
            .collect();
        assert_eq!(letters, ["a", "z", "aa", "zz", "aaa"]);
        assert_eq!(text(28, NumberFormat::UpperLetter), "BB");
        let romans: Vec<String> = [1, 4, 9, 14, 40, 90, 400, 1994, 3999, 4000]
            .into_iter()
            .map(|n| text(n, NumberFormat::UpperRoman))
            .collect();
        assert_eq!(
            romans,
            [
                "I",
                "IV",
                "IX",
                "XIV",
                "XL",
                "XC",
                "CD",
                "MCMXCIV",
                "MMMCMXCIX",
                "MMMM"
            ]
        );
        assert_eq!(text(12, NumberFormat::LowerRoman), "xii");
        let ordinals: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111]
            .into_iter()
            .map(|n| text(n, NumberFormat::Ordinal))
            .collect();
        assert_eq!(
            ordinals,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st",
                "111th"
            ]
        );
        let marks: Vec<String> = [1, 2, 3, 4, 5, 8]
            .into_iter()
            .map(|n| text(n, NumberFormat::Chicago))
            .collect();
        assert_eq!(
            marks,
            ["*", "\u{2020}", "\u{2021}", "\u{a7}", "**", "\u{a7}\u{a7}"]
        );
        assert_eq!(text(3, NumberFormat::None), "");
        assert_eq!(text(3, NumberFormat::Bullet), "");
        // Zero has no letter, numeral or mark.
        assert_eq!(text(0, NumberFormat::UpperRoman), "0");
        assert_eq!(text(0, NumberFormat::LowerLetter), "0");
    }

    #[test]
    fn other_formats_are_decimal_and_say_so() {
        assert_eq!(
            number_text(5, &NumberFormat::Other("hebrew1".into())),
            ("5".to_owned(), true)
        );
        assert_eq!(
            number_text(5, &NumberFormat::Decimal),
            ("5".to_owned(), false)
        );
    }

    #[test]
    fn large_numbers_stay_short() {
        let long = text(u32::MAX, NumberFormat::UpperLetter);
        assert_eq!(long.len(), MAX_REPEATS as usize);
        assert!(text(u32::MAX, NumberFormat::UpperRoman).len() < 100);
        assert_eq!(
            text(u32::MAX, NumberFormat::Chicago).chars().count(),
            MAX_REPEATS as usize
        );
    }
}
