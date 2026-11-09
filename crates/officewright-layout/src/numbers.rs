//! Numbers as text, in the formats of page numbers, fields and list labels.

use crate::ir::NumberFormat;

/// The marks of [`NumberFormat::Chicago`], in order; after the last they start again,
/// doubled, then tripled.
const CHICAGO: [char; 4] = ['*', '\u{2020}', '\u{2021}', '\u{00A7}'];

/// The longest a letter number is written: Word repeats one letter for each 26 past the
/// first round, up to 30 letters (780 is `ZZZ…Z`, 781 cannot be represented, as Word's
/// references `E-number-formats` and `E-number-formats-letters` show).
const MAX_LETTERS: u32 = 30;

/// The most thousands a Roman numeral writes, as repeated `M`, so that no number makes a
/// long text. Word writes 10000 as ten `M` (`E-number-formats-letters`); where it stops
/// is not known.
const MAX_THOUSANDS: u32 = 64;

/// The most times a Chicago mark is repeated, so that no number makes a long text. Word
/// writes page 117 as thirty `*` (`E-number-formats`), so the marks go on past 116,
/// although MS-OI29500 2.1.548 k says Word writes no sequence number past it; how far is
/// not known.
const MAX_MARKS: u32 = 64;

/// The largest number written in words. A hypothesis from Microsoft's knowledge base
/// article Q97591 (Word 97), whose limits for letters and Roman numerals Word's
/// references show to be out of date, until a reference shows this one.
const MAX_WORDS: u32 = 999_999;

/// What Word writes for a number its format cannot represent.
pub(crate) const UNREPRESENTABLE: &str = "Error! Number cannot be represented in specified format.";

/// The language of numbers in words and of ordinals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Words {
    /// English, which the layout also writes for every language it has no words for.
    #[default]
    English,
    /// German.
    German,
}

impl Words {
    /// The language of a BCP 47 tag, and whether the layout has words for it: English
    /// for `en` and for no tag, German for `de`.
    pub(crate) fn of(tag: Option<&str>) -> (Self, bool) {
        let primary = tag.map(|tag| {
            tag.split(['-', '_'])
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
        });
        match primary.as_deref() {
            None | Some("en" | "") => (Self::English, true),
            Some("de") => (Self::German, true),
            Some(_) => (Self::English, false),
        }
    }
}

/// A number as its format writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Written {
    /// As the format says.
    Text(String),
    /// In decimal, since the layout does not write the format.
    Decimal(String),
    /// Not at all: the format cannot represent the number, and Word writes
    /// [`UNREPRESENTABLE`] in a field.
    Unrepresentable,
}

/// `number` written in `format`, and whether it is written in decimal instead, which the
/// caller reports: a format the layout does not write, or a number the format cannot
/// represent. Words and ordinals are English.
pub(crate) fn number_text(number: u32, format: &NumberFormat) -> (String, bool) {
    match written(number, format, Words::English) {
        Written::Text(text) => (text, false),
        Written::Decimal(text) => (text, true),
        Written::Unrepresentable => (number.to_string(), true),
    }
}

/// Whether `format` writes words or suffixes of a language.
pub(crate) fn in_language(format: &NumberFormat) -> bool {
    matches!(
        format,
        NumberFormat::Ordinal
            | NumberFormat::CardinalText
            | NumberFormat::OrdinalText
            | NumberFormat::DollarText
    )
}

/// `number` written in `format`, with words and ordinals in the language `words`.
pub(crate) fn written(number: u32, format: &NumberFormat, words: Words) -> Written {
    let text = match format {
        NumberFormat::Decimal => number.to_string(),
        NumberFormat::DecimalZero => format!("{number:02}"),
        NumberFormat::NumberInDash => format!("- {number} -"),
        NumberFormat::LowerLetter => return letters(number, false),
        NumberFormat::UpperLetter => return letters(number, true),
        NumberFormat::LowerRoman => roman(number).to_lowercase(),
        NumberFormat::UpperRoman => roman(number),
        NumberFormat::Ordinal => match words {
            Words::English => ordinal(number),
            Words::German => format!("{number}."),
        },
        NumberFormat::CardinalText | NumberFormat::OrdinalText | NumberFormat::DollarText
            if number > MAX_WORDS =>
        {
            return Written::Unrepresentable;
        }
        NumberFormat::CardinalText => in_words(number, false, words),
        NumberFormat::OrdinalText => in_words(number, true, words),
        NumberFormat::DollarText => {
            let and = match words {
                Words::English => "and",
                // A hypothesis until a Word reference shows German dollar text.
                Words::German => "und",
            };
            format!("{} {and} 00/100", in_words(number, false, words))
        }
        // Word 97 wrote no hex number past 32,767 (Q97591); whether Word still stops there
        // is not known.
        NumberFormat::Hex => format!("{number:X}"),
        NumberFormat::Chicago => chicago(number),
        NumberFormat::Bullet | NumberFormat::None => String::new(),
        NumberFormat::Other(_) => return Written::Decimal(number.to_string()),
    };
    Written::Text(text)
}

/// `number` as letters: a to z, then aa to zz, then aaa, one letter repeated once more
/// for each round of 26, up to [`MAX_LETTERS`]; 0 as nothing, as Word writes it.
fn letters(number: u32, upper: bool) -> Written {
    if number == 0 {
        return Written::Text(String::new());
    }
    let index = (number - 1) % 26;
    let repeats = (number - 1) / 26 + 1;
    if repeats > MAX_LETTERS {
        return Written::Unrepresentable;
    }
    let base = if upper { b'A' } else { b'a' };
    let letter = char::from(base + index as u8);
    Written::Text(std::iter::repeat_n(letter, repeats as usize).collect())
}

/// `number` in capital Roman numerals, thousands as repeated `M` up to
/// [`MAX_THOUSANDS`]; 0 as nothing, as Word writes it.
fn roman(number: u32) -> String {
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
    let mut rest = number.min(MAX_THOUSANDS * 1000 + 999);
    let mut text = String::new();
    for (value, part) in PARTS {
        while rest >= value {
            text.push_str(part);
            rest -= value;
        }
    }
    text
}

/// `number` with its English ordinal suffix: 0th, 1st, 2nd, 3rd, 4th, 11th, 12th, 13th,
/// 21st.
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

/// `number` in Chicago marks: *, †, ‡, §, then the same doubled, tripled, up to
/// [`MAX_MARKS`] times; 0 as decimal.
fn chicago(number: u32) -> String {
    if number == 0 {
        return number.to_string();
    }
    let index = ((number - 1) % 4) as usize;
    let repeats = ((number - 1) / 4 + 1).min(MAX_MARKS);
    let mark = CHICAGO.get(index).copied().unwrap_or('*');
    std::iter::repeat_n(mark, repeats as usize).collect()
}

/// The English words of 0 to 19.
const ENGLISH_ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];

/// The English words of the tens from 20 to 90, from index 2.
const ENGLISH_TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// The German words of 0 to 19.
const GERMAN_ONES: [&str; 20] = [
    "null",
    "eins",
    "zwei",
    "drei",
    "vier",
    "fünf",
    "sechs",
    "sieben",
    "acht",
    "neun",
    "zehn",
    "elf",
    "zwölf",
    "dreizehn",
    "vierzehn",
    "fünfzehn",
    "sechzehn",
    "siebzehn",
    "achtzehn",
    "neunzehn",
];

/// The German words of the tens from 20 to 90, from index 2.
const GERMAN_TENS: [&str; 10] = [
    "", "", "zwanzig", "dreißig", "vierzig", "fünfzig", "sechzig", "siebzig", "achtzig", "neunzig",
];

/// `number`, at most [`MAX_WORDS`], in words, as a cardinal (`twenty-seven`) or an
/// ordinal (`twenty-seventh`).
fn in_words(number: u32, ordinal: bool, words: Words) -> String {
    match words {
        Words::English => english(number, ordinal),
        Words::German => german(number, ordinal),
    }
}

/// `number` in English words as Word writes them: `one thousand six hundred sixty-five`,
/// with no `and`, and the ordinal by its last word (`sixty-fifth`, `four thousandth`).
fn english(number: u32, ordinal: bool) -> String {
    fn below_thousand(number: u32, parts: &mut Vec<String>) {
        let (hundreds, rest) = (number / 100, number % 100);
        if hundreds > 0 {
            parts.push(english_one(hundreds).to_owned());
            parts.push("hundred".to_owned());
        }
        if rest >= 20 {
            let tens = ENGLISH_TENS
                .get((rest / 10) as usize)
                .copied()
                .unwrap_or_default();
            match rest % 10 {
                0 => parts.push(tens.to_owned()),
                one => parts.push(format!("{tens}-{}", english_one(one))),
            }
        } else if rest > 0 {
            parts.push(english_one(rest).to_owned());
        }
    }
    let mut parts = Vec::new();
    if number == 0 {
        parts.push(english_one(0).to_owned());
    }
    let (thousands, rest) = (number / 1000, number % 1000);
    if thousands > 0 {
        below_thousand(thousands, &mut parts);
        parts.push("thousand".to_owned());
    }
    below_thousand(rest, &mut parts);
    if ordinal {
        if let Some(last) = parts.last_mut() {
            // The last word, or the last of a hyphenated pair, becomes the ordinal.
            let cut = last.rfind('-').map_or(0, |at| at + 1);
            let word = last.split_off(cut);
            last.push_str(&english_ordinal(&word));
        }
    }
    parts.join(" ")
}

/// The English word of 0 to 19.
fn english_one(number: u32) -> &'static str {
    ENGLISH_ONES
        .get(number as usize)
        .copied()
        .unwrap_or_default()
}

/// The ordinal of one English number word: `first`, `twelfth`, `twentieth`, `hundredth`.
fn english_ordinal(word: &str) -> String {
    match word {
        "one" => "first".to_owned(),
        "two" => "second".to_owned(),
        "three" => "third".to_owned(),
        "five" => "fifth".to_owned(),
        "eight" => "eighth".to_owned(),
        "nine" => "ninth".to_owned(),
        "twelve" => "twelfth".to_owned(),
        _ => match word.strip_suffix('y') {
            Some(stem) => format!("{stem}ieth"),
            None => format!("{word}th"),
        },
    }
}

/// The German word of 0 to 19 inside a longer number: 1 is `eins` without its `s`.
fn german_inner(number: u32) -> &'static str {
    let word = GERMAN_ONES
        .get(number as usize)
        .copied()
        .unwrap_or_default();
    if number == 1 {
        word.strip_suffix('s').unwrap_or(word)
    } else {
        word
    }
}

/// `number` in German words as one word with a capital, as Word writes them
/// (`Siebenundzwanzig`, `Siebenundzwanzigste`). Word's references show only 27; the
/// forms of hundreds and thousands (`einhundert`, `eintausend`) and the ordinals of 1 to
/// 19 follow German grammar, a hypothesis until a reference shows Word's.
fn german(number: u32, ordinal: bool) -> String {
    fn below_hundred(number: u32, text: &mut String) {
        if number < 20 {
            text.push_str(german_inner(number));
            return;
        }
        let one = number % 10;
        if one > 0 {
            text.push_str(german_inner(one));
            text.push_str("und");
        }
        text.push_str(
            GERMAN_TENS
                .get((number / 10) as usize)
                .copied()
                .unwrap_or_default(),
        );
    }
    fn below_thousand(number: u32, text: &mut String) {
        let (hundreds, rest) = (number / 100, number % 100);
        if hundreds > 0 {
            below_hundred(hundreds, text);
            text.push_str("hundert");
        }
        if rest > 0 {
            below_hundred(rest, text);
        }
    }
    let mut text = String::new();
    let (thousands, rest) = (number / 1000, number % 1000);
    if thousands > 0 {
        below_thousand(thousands, &mut text);
        text.push_str("tausend");
    }
    below_thousand(rest, &mut text);
    let last = number % 100;
    if number == 0 {
        text.push_str(if ordinal { "nullte" } else { "null" });
    } else if ordinal {
        match last {
            // 1 to 19 take -te, four of them with a stem of their own; the rest -ste.
            1..=19 => {
                let one = german_inner(last);
                if let Some(stem) = text.strip_suffix(one) {
                    text = stem.to_owned();
                }
                match last {
                    1 => text.push_str("erste"),
                    3 => text.push_str("dritte"),
                    7 => text.push_str("siebte"),
                    8 => text.push_str("achte"),
                    _ => {
                        text.push_str(one);
                        text.push_str("te");
                    }
                }
            }
            _ => text.push_str("ste"),
        }
    } else if last == 1 {
        // 1 at the end is `eins`.
        text.push('s');
    }
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
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
        // Zero has no letter or numeral (E-number-formats, case 01).
        assert_eq!(text(0, NumberFormat::UpperRoman), "");
        assert_eq!(text(0, NumberFormat::LowerRoman), "");
        assert_eq!(text(0, NumberFormat::UpperLetter), "");
        assert_eq!(text(0, NumberFormat::LowerLetter), "");
        assert_eq!(text(0, NumberFormat::Chicago), "0");
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
        // 64 thousands and 64 marks at most.
        assert!(text(u32::MAX, NumberFormat::UpperRoman).len() < 100);
        assert_eq!(text(u32::MAX, NumberFormat::Chicago), "\u{2021}".repeat(64));
        assert_eq!(text(u32::MAX, NumberFormat::Hex), "FFFFFFFF");
        assert_eq!(
            written(u32::MAX, &NumberFormat::UpperLetter, Words::English),
            Written::Unrepresentable
        );
    }

    /// Each line of Word's references `E-number-formats` and `E-number-formats-letters`:
    /// a number and what Word writes for it in each format, words and ordinals in English.
    #[test]
    fn numbers_are_written_as_word_writes_them() {
        use NumberFormat::*;
        let error = UNREPRESENTABLE;
        let cases: [(u32, NumberFormat, &str); 67] = [
            (0, Decimal, "0"),
            (0, NumberInDash, "- 0 -"),
            (0, UpperLetter, ""),
            (0, LowerRoman, ""),
            (0, Ordinal, "0th"),
            (0, CardinalText, "zero"),
            (0, OrdinalText, "zeroth"),
            (0, Hex, "0"),
            (0, DollarText, "zero and 00/100"),
            (27, NumberInDash, "- 27 -"),
            (27, UpperLetter, "AA"),
            (27, LowerLetter, "aa"),
            (27, UpperRoman, "XXVII"),
            (27, LowerRoman, "xxvii"),
            (27, Ordinal, "27th"),
            (27, CardinalText, "twenty-seven"),
            (27, OrdinalText, "twenty-seventh"),
            (27, Hex, "1B"),
            (27, DollarText, "twenty-seven and 00/100"),
            (53, UpperLetter, "AAA"),
            (53, UpperRoman, "LIII"),
            (53, Ordinal, "53rd"),
            (53, CardinalText, "fifty-three"),
            (53, OrdinalText, "fifty-third"),
            (53, Hex, "35"),
            (53, DollarText, "fifty-three and 00/100"),
            (1665, NumberInDash, "- 1665 -"),
            (1665, UpperLetter, error),
            (1665, LowerLetter, error),
            (1665, UpperRoman, "MDCLXV"),
            (1665, Ordinal, "1665th"),
            (1665, CardinalText, "one thousand six hundred sixty-five"),
            (1665, OrdinalText, "one thousand six hundred sixty-fifth"),
            (1665, Hex, "681"),
            (
                1665,
                DollarText,
                "one thousand six hundred sixty-five and 00/100",
            ),
            (4000, UpperRoman, "MMMM"),
            (4000, CardinalText, "four thousand"),
            (4000, OrdinalText, "four thousandth"),
            (4000, Hex, "FA0"),
            (4000, DollarText, "four thousand and 00/100"),
            (4999, UpperRoman, "MMMMCMXCIX"),
            (4999, LowerRoman, "mmmmcmxcix"),
            (4999, CardinalText, "four thousand nine hundred ninety-nine"),
            (4999, OrdinalText, "four thousand nine hundred ninety-ninth"),
            (4999, Hex, "1387"),
            (116, Chicago, &"\u{a7}".repeat(29)),
            (117, Chicago, &"*".repeat(30)),
            (9, UpperRoman, "IX"),
            (9, LowerLetter, "i"),
            (9, CardinalText, "nine"),
            (520, UpperLetter, &"Z".repeat(20)),
            (520, LowerRoman, "dxx"),
            (780, LowerLetter, &"z".repeat(30)),
            (780, UpperRoman, "DCCLXXX"),
            (781, UpperLetter, error),
            (781, LowerLetter, error),
            (781, LowerRoman, "dcclxxxi"),
            (1040, UpperLetter, error),
            (1040, UpperRoman, "MXL"),
            (1300, LowerLetter, error),
            (1300, UpperRoman, "MCCC"),
            (1664, UpperLetter, error),
            (1664, UpperRoman, "MDCLXIV"),
            (5000, LowerLetter, error),
            (5000, UpperRoman, "MMMMM"),
            (10000, UpperRoman, "MMMMMMMMMM"),
            (10000, LowerRoman, "mmmmmmmmmm"),
        ];
        for (number, format, word) in cases {
            let ours = match written(number, &format, Words::English) {
                Written::Text(text) => text,
                Written::Decimal(text) => format!("decimal {text}"),
                Written::Unrepresentable => UNREPRESENTABLE.to_owned(),
            };
            assert_eq!(ours, word, "{number} in {format:?}");
        }
    }

    #[test]
    fn german_ordinals_and_words_are_written_as_word_writes_them() {
        // E-number-formats, case 02.
        let german = |format| match written(27, &format, Words::German) {
            Written::Text(text) => text,
            other => panic!("{other:?}"),
        };
        assert_eq!(german(NumberFormat::Ordinal), "27.");
        assert_eq!(german(NumberFormat::OrdinalText), "Siebenundzwanzigste");
        assert_eq!(german(NumberFormat::CardinalText), "Siebenundzwanzig");
    }

    #[test]
    fn german_words_follow_german_grammar() {
        let words: Vec<(String, String)> = [
            0, 1, 3, 7, 8, 11, 16, 17, 20, 21, 30, 100, 101, 1000, 1665, 999_999,
        ]
        .into_iter()
        .map(|n| (german(n, false), german(n, true)))
        .collect();
        let expected = [
            ("Null", "Nullte"),
            ("Eins", "Erste"),
            ("Drei", "Dritte"),
            ("Sieben", "Siebte"),
            ("Acht", "Achte"),
            ("Elf", "Elfte"),
            ("Sechzehn", "Sechzehnte"),
            ("Siebzehn", "Siebzehnte"),
            ("Zwanzig", "Zwanzigste"),
            ("Einundzwanzig", "Einundzwanzigste"),
            ("Dreißig", "Dreißigste"),
            ("Einhundert", "Einhundertste"),
            ("Einhunderteins", "Einhunderterste"),
            ("Eintausend", "Eintausendste"),
            (
                "Eintausendsechshundertfünfundsechzig",
                "Eintausendsechshundertfünfundsechzigste",
            ),
            (
                "Neunhundertneunundneunzigtausendneunhundertneunundneunzig",
                "Neunhundertneunundneunzigtausendneunhundertneunundneunzigste",
            ),
        ];
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(cardinal, ordinal)| (cardinal.to_owned(), ordinal.to_owned()))
            .collect();
        assert_eq!(words, expected);
    }

    #[test]
    fn english_words_have_every_ordinal_form() {
        let ordinals: Vec<String> = [1, 2, 3, 5, 8, 9, 12, 20, 40, 100, 101, 112, 999_999]
            .into_iter()
            .map(|n| english(n, true))
            .collect();
        assert_eq!(
            ordinals,
            [
                "first",
                "second",
                "third",
                "fifth",
                "eighth",
                "ninth",
                "twelfth",
                "twentieth",
                "fortieth",
                "one hundredth",
                "one hundred first",
                "one hundred twelfth",
                "nine hundred ninety-nine thousand nine hundred ninety-ninth",
            ]
        );
    }

    #[test]
    fn words_stop_at_a_million() {
        // Microsoft's knowledge base article Q97591 gives 999,999 as the largest.
        for format in [
            NumberFormat::CardinalText,
            NumberFormat::OrdinalText,
            NumberFormat::DollarText,
        ] {
            for words in [Words::English, Words::German] {
                assert!(matches!(written(999_999, &format, words), Written::Text(_)));
                assert_eq!(written(1_000_000, &format, words), Written::Unrepresentable);
            }
        }
        // Lists and notes write such a number in decimal and say so.
        assert_eq!(
            number_text(1_000_000, &NumberFormat::CardinalText),
            ("1000000".to_owned(), true)
        );
    }

    #[test]
    fn languages_are_read_from_their_tags() {
        assert_eq!(Words::of(None), (Words::English, true));
        assert_eq!(Words::of(Some("en-GB")), (Words::English, true));
        assert_eq!(Words::of(Some("DE-at")), (Words::German, true));
        assert_eq!(Words::of(Some("de")), (Words::German, true));
        assert_eq!(Words::of(Some("fr-FR")), (Words::English, false));
        assert_eq!(Words::of(Some("deu")), (Words::English, false));
    }
}
