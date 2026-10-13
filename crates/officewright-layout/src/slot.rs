//! Which of a run's four fonts a character takes: Word's rule, MS-OI29500 2.1.88 b (the
//! same note is MS-OE376 for Part 4 §2.3.2.24, rFonts).

// reason: the layout itself uses this from PR 6 (paragraph layout) on; until then only
// the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use crate::ir::{EastAsiaCharset, FontHint, RunFonts, RunStyle};

/// One of the four fonts of a run (`w:rFonts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    /// `w:ascii`.
    Ascii,
    /// `w:hAnsi`.
    HighAnsi,
    /// `w:eastAsia`.
    EastAsia,
    /// `w:cs`.
    ComplexScript,
}

impl Slot {
    /// The four slots, in the order a character falls back from one to the others.
    pub(crate) const ALL: [Slot; 4] = [
        Slot::Ascii,
        Slot::HighAnsi,
        Slot::EastAsia,
        Slot::ComplexScript,
    ];

    /// The family `fonts` names for this slot.
    pub(crate) fn family(self, fonts: &RunFonts) -> &str {
        match self {
            Self::Ascii => fonts.ascii(),
            Self::HighAnsi => fonts.high_ansi(),
            Self::EastAsia => fonts.east_asia(),
            Self::ComplexScript => fonts.complex_script(),
        }
    }
}

/// How a range of characters chooses its slot, by a row of the note's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    Ascii,
    HighAnsi,
    EastAsia,
    /// East Asian with `w:hint="eastAsia"`, else `hAnsi`, the table's default.
    HintEastAsia,
    /// Latin-1 Supplement: `hAnsi`, with East Asian exceptions.
    Latin1,
    /// Latin Extended-A and -B and IPA Extensions: East Asian with the hint when the run
    /// is Chinese or the East Asian font's character set is Chinese.
    ChineseByHint,
    /// Latin Extended Additional: East Asian with the hint when the run is Chinese.
    ChineseLanguageByHint,
    /// Alphabetic Presentation Forms: East Asian with the hint up to U+FB1C, `ascii`
    /// from U+FB1D (the Hebrew forms), `hAnsi` otherwise.
    PresentationForms,
}

/// The note's table, row by row (start, end, rule). Characters in no row take `hAnsi`.
const TABLE: &[(u32, u32, Rule)] = &[
    (0x0000, 0x007F, Rule::Ascii),                 // Basic Latin
    (0x00A0, 0x00FF, Rule::Latin1),                // Latin-1 Supplement
    (0x0100, 0x017F, Rule::ChineseByHint),         // Latin Extended-A
    (0x0180, 0x024F, Rule::ChineseByHint),         // Latin Extended-B
    (0x0250, 0x02AF, Rule::ChineseByHint),         // IPA Extensions
    (0x02B0, 0x02FF, Rule::HintEastAsia),          // Spacing Modifier Letters
    (0x0300, 0x036F, Rule::HintEastAsia),          // Combining Diacritical Marks
    (0x0370, 0x03CF, Rule::HintEastAsia),          // Greek
    (0x0400, 0x04FF, Rule::HintEastAsia),          // Cyrillic
    (0x0590, 0x05FF, Rule::Ascii),                 // Hebrew
    (0x0600, 0x06FF, Rule::Ascii),                 // Arabic
    (0x0700, 0x074F, Rule::Ascii),                 // Syriac
    (0x0750, 0x077F, Rule::Ascii),                 // Arabic Supplement
    (0x0780, 0x07BF, Rule::Ascii),                 // Thaana
    (0x1100, 0x11FF, Rule::EastAsia),              // Hangul Jamo
    (0x1E00, 0x1EFF, Rule::ChineseLanguageByHint), // Latin Extended Additional
    (0x2000, 0x206F, Rule::HintEastAsia),          // General Punctuation
    (0x2070, 0x209F, Rule::HintEastAsia),          // Superscripts and Subscripts
    (0x20A0, 0x20CF, Rule::HintEastAsia),          // Currency Symbols
    (0x20D0, 0x20FF, Rule::HintEastAsia),          // Combining Diacritical Marks for Symbols
    (0x2100, 0x214F, Rule::HintEastAsia),          // Letter-like Symbols
    (0x2150, 0x218F, Rule::HintEastAsia),          // Number Forms
    (0x2190, 0x21FF, Rule::HintEastAsia),          // Arrows
    (0x2200, 0x22FF, Rule::HintEastAsia),          // Mathematical Operators
    (0x2300, 0x23FF, Rule::HintEastAsia),          // Miscellaneous Technical
    (0x2400, 0x243F, Rule::HintEastAsia),          // Control Pictures
    (0x2440, 0x245F, Rule::HintEastAsia),          // Optical Character Recognition
    (0x2460, 0x24FF, Rule::HintEastAsia),          // Enclosed Alphanumerics
    (0x2500, 0x257F, Rule::HintEastAsia),          // Box Drawing
    (0x2580, 0x259F, Rule::HintEastAsia),          // Block Elements
    (0x25A0, 0x25FF, Rule::HintEastAsia),          // Geometric Shapes
    (0x2600, 0x26FF, Rule::HintEastAsia),          // Miscellaneous Symbols
    (0x2700, 0x27BF, Rule::HintEastAsia),          // Dingbats
    (0x2E80, 0x2EFF, Rule::HintEastAsia),          // CJK Radicals Supplement
    (0x2F00, 0x2FDF, Rule::EastAsia),              // Kangxi Radicals
    (0x2FF0, 0x2FFF, Rule::EastAsia),              // Ideographic Description Characters
    (0x3000, 0x303F, Rule::EastAsia),              // CJK Symbols and Punctuation
    (0x3040, 0x309F, Rule::EastAsia),              // Hiragana
    (0x30A0, 0x30FF, Rule::EastAsia),              // Katakana
    (0x3100, 0x312F, Rule::EastAsia),              // Bopomofo
    (0x3130, 0x318F, Rule::EastAsia),              // Hangul Compatibility Jamo
    (0x3190, 0x319F, Rule::EastAsia),              // Kanbun
    (0x3200, 0x32FF, Rule::EastAsia),              // Enclosed CJK Letters and Months
    (0x3300, 0x33FF, Rule::EastAsia),              // CJK Compatibility
    (0x3400, 0x4DBF, Rule::EastAsia),              // CJK Unified Ideographs Extension A
    (0x4E00, 0x9FAF, Rule::EastAsia),              // CJK Unified Ideographs
    (0xA000, 0xA48F, Rule::EastAsia),              // Yi Syllables
    (0xA490, 0xA4CF, Rule::EastAsia),              // Yi Radicals
    (0xAC00, 0xD7AF, Rule::EastAsia),              // Hangul Syllables
    (0xD800, 0xDB7F, Rule::EastAsia),              // High Surrogates
    (0xDB80, 0xDBFF, Rule::EastAsia),              // High Private Use Surrogates
    (0xDC00, 0xDFFF, Rule::EastAsia),              // Low Surrogates
    (0xE000, 0xF8FF, Rule::HintEastAsia),          // Private Use Area
    (0xF900, 0xFAFF, Rule::EastAsia),              // CJK Compatibility Ideographs
    (0xFB00, 0xFB4F, Rule::PresentationForms),     // Alphabetic Presentation Forms
    (0xFB50, 0xFDFF, Rule::Ascii),                 // Arabic Presentation Forms-A
    (0xFE30, 0xFE4F, Rule::EastAsia),              // CJK Compatibility Forms
    (0xFE50, 0xFE6F, Rule::EastAsia),              // Small Form Variants
    (0xFE70, 0xFEFE, Rule::Ascii),                 // Arabic Presentation Forms-B
    (0xFF00, 0xFFEF, Rule::EastAsia),              // Halfwidth and Fullwidth Forms
    // The note counts UTF-16 code units: a character above U+FFFF is two surrogates,
    // which the three rows above give to the East Asian font. A `char` is never a
    // surrogate, so this row stands for them.
    (0x1_0000, 0x10_FFFF, Rule::EastAsia),
];

/// The Latin-1 characters that take the East Asian font with `w:hint="eastAsia"`.
const LATIN1_HINT: [(u32, u32); 11] = [
    (0xA1, 0xA1),
    (0xA4, 0xA4),
    (0xA7, 0xA8),
    (0xAA, 0xAA),
    (0xAD, 0xAD),
    (0xAF, 0xAF),
    (0xB0, 0xB4),
    (0xB6, 0xBA),
    (0xBC, 0xBF),
    (0xD7, 0xD7),
    (0xF7, 0xF7),
];

/// The Latin-1 characters that take the East Asian font with the hint when the run is
/// Chinese.
const LATIN1_CHINESE: [(u32, u32); 6] = [
    (0xE0, 0xE1),
    (0xE8, 0xEA),
    (0xEC, 0xED),
    (0xF2, 0xF3),
    (0xF9, 0xFA),
    (0xFC, 0xFC),
];

/// Whether two family names name the same family, as Word compares them: ignoring case
/// and the spaces around them.
pub(crate) fn same_family(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Whether the BCP 47 tag `tag` is of one of `languages`, by its primary subtag in any
/// case: `zh-Hant` and `ZH-cn` are `zh`, `zha` (Zhuang) is not.
pub(crate) fn language_is(tag: &str, languages: &[&str]) -> bool {
    let primary = tag.split(['-', '_']).next().unwrap_or_default();
    languages
        .iter()
        .any(|language| primary.eq_ignore_ascii_case(language))
}

fn within(code: u32, ranges: &[(u32, u32)]) -> bool {
    ranges
        .iter()
        .any(|&(start, end)| (start..=end).contains(&code))
}

/// The slot of `ch` in a run of `style`.
pub(crate) fn slot(ch: char, style: &RunStyle) -> Slot {
    let fonts = style.fonts();
    // A complex script or right-to-left run takes its complex script font throughout.
    if style.complex_script() || style.rtl() {
        return Slot::ComplexScript;
    }
    // Times New Roman for East Asian text means none was chosen, and when the two Latin
    // fonts agree, everything takes the ASCII one.
    if same_family(fonts.east_asia(), "Times New Roman")
        && same_family(fonts.ascii(), fonts.high_ansi())
    {
        return Slot::Ascii;
    }
    let code = u32::from(ch);
    let hint = fonts.hint() == FontHint::EastAsia;
    let chinese = style
        .language()
        .east_asia()
        .is_some_and(|tag| language_is(tag, &["zh"]));
    let chinese_charset = matches!(
        fonts.east_asia_charset(),
        EastAsiaCharset::ChineseBig5 | EastAsiaCharset::Gb2312
    );
    let rule = TABLE
        .iter()
        .find(|(start, end, _)| (*start..=*end).contains(&code))
        .map_or(Rule::HighAnsi, |(_, _, rule)| *rule);
    let east_asia_if = |condition: bool| {
        if condition {
            Slot::EastAsia
        } else {
            Slot::HighAnsi
        }
    };
    match rule {
        Rule::Ascii => Slot::Ascii,
        Rule::HighAnsi => Slot::HighAnsi,
        Rule::EastAsia => Slot::EastAsia,
        Rule::HintEastAsia => east_asia_if(hint),
        Rule::Latin1 => east_asia_if(
            hint && (within(code, &LATIN1_HINT) || (chinese && within(code, &LATIN1_CHINESE))),
        ),
        // The note: "If hint is eastAsia, and the language of the run is either Chinese
        // Traditional or Chinese Simplified, or the character set of the eastAsia font is
        // Chinese5 or GB2312 then eastAsia font is used." Read as the hint and either
        // condition; the other reading, the language with the hint or the character set
        // alone, awaits a Word reference that tells them apart.
        Rule::ChineseByHint => east_asia_if(hint && (chinese || chinese_charset)),
        Rule::ChineseLanguageByHint => east_asia_if(hint && chinese),
        Rule::PresentationForms if code >= 0xFB1D => Slot::Ascii,
        Rule::PresentationForms => east_asia_if(hint),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Language;

    /// A style whose four fonts differ, so Word's Times New Roman rule does not apply.
    fn style() -> RunStyle {
        RunStyle::default().with_fonts(
            RunFonts::default()
                .with_ascii("A")
                .with_high_ansi("H")
                .with_east_asia("E")
                .with_complex_script("C"),
        )
    }

    fn hinted() -> RunStyle {
        let style = style();
        let fonts = style.fonts().clone().with_hint(FontHint::EastAsia);
        style.with_fonts(fonts)
    }

    fn chinese(style: RunStyle) -> RunStyle {
        style.with_language(Language::default().with_east_asia(Some("zh-CN".into())))
    }

    fn charset(style: RunStyle, charset: EastAsiaCharset) -> RunStyle {
        let fonts = style.fonts().clone().with_east_asia_charset(charset);
        style.with_fonts(fonts)
    }

    fn at(code: u32, style: &RunStyle) -> Slot {
        slot(char::from_u32(code).unwrap(), style)
    }

    use Slot::{Ascii as A, ComplexScript as C, EastAsia as E, HighAnsi as H};

    /// The note's table, written out again: for the first and last character of each
    /// block, the slot without the hint and with it (and no Chinese).
    const NOTE: &[(u32, u32, Slot, Slot)] = &[
        (0x0000, 0x007F, A, A),
        (0x00A0, 0x00FF, H, H), // the hinted exceptions are tested apart
        (0x0100, 0x017F, H, H),
        (0x0180, 0x024F, H, H),
        (0x0250, 0x02AF, H, H),
        (0x02B0, 0x02FF, H, E),
        (0x0300, 0x036F, H, E),
        (0x0370, 0x03CF, H, E),
        (0x0400, 0x04FF, H, E),
        (0x0590, 0x05FF, A, A),
        (0x0600, 0x06FF, A, A),
        (0x0700, 0x074F, A, A),
        (0x0750, 0x077F, A, A),
        (0x0780, 0x07BF, A, A),
        (0x1100, 0x11FF, E, E),
        (0x1E00, 0x1EFF, H, H),
        (0x2000, 0x206F, H, E),
        (0x2070, 0x209F, H, E),
        (0x20A0, 0x20CF, H, E),
        (0x20D0, 0x20FF, H, E),
        (0x2100, 0x214F, H, E),
        (0x2150, 0x218F, H, E),
        (0x2190, 0x21FF, H, E),
        (0x2200, 0x22FF, H, E),
        (0x2300, 0x23FF, H, E),
        (0x2400, 0x243F, H, E),
        (0x2440, 0x245F, H, E),
        (0x2460, 0x24FF, H, E),
        (0x2500, 0x257F, H, E),
        (0x2580, 0x259F, H, E),
        (0x25A0, 0x25FF, H, E),
        (0x2600, 0x26FF, H, E),
        (0x2700, 0x27BF, H, E),
        (0x2E80, 0x2EFF, H, E),
        (0x2F00, 0x2FDF, E, E),
        (0x2FF0, 0x2FFF, E, E),
        (0x3000, 0x303F, E, E),
        (0x3040, 0x309F, E, E),
        (0x30A0, 0x30FF, E, E),
        (0x3100, 0x312F, E, E),
        (0x3130, 0x318F, E, E),
        (0x3190, 0x319F, E, E),
        (0x3200, 0x32FF, E, E),
        (0x3300, 0x33FF, E, E),
        (0x3400, 0x4DBF, E, E),
        (0x4E00, 0x9FAF, E, E),
        (0xA000, 0xA48F, E, E),
        (0xA490, 0xA4CF, E, E),
        (0xAC00, 0xD7AF, E, E),
        (0xE000, 0xF8FF, H, E),
        (0xF900, 0xFAFF, E, E),
        (0xFB00, 0xFB1C, H, E),
        (0xFB1D, 0xFB4F, A, A),
        (0xFB50, 0xFDFF, A, A),
        (0xFE30, 0xFE4F, E, E),
        (0xFE50, 0xFE6F, E, E),
        (0xFE70, 0xFEFE, A, A),
        (0xFF00, 0xFFEF, E, E),
        // Above U+FFFF: two surrogates, which the note gives to East Asian fonts.
        (0x1_0000, 0x1_0000, E, E),
        (0x1_F600, 0x1_F600, E, E),
        (0x10_FFFF, 0x10_FFFF, E, E),
        // Blocks the note does not list: hAnsi.
        (0x0080, 0x009F, H, H),
        (0x0E00, 0x0E7F, H, H),
        (0x2FE0, 0x2FEF, H, H),
        (0x3FFF0, 0x3FFF0, E, E),
        (0xFFF0, 0xFFFD, H, H),
    ];

    #[test]
    fn follows_every_row_of_the_note() {
        for &(start, end, plain, with_hint) in NOTE {
            for code in [start, end] {
                assert_eq!(at(code, &style()), plain, "U+{code:04X}");
                assert_eq!(at(code, &hinted()), with_hint, "U+{code:04X} hinted");
            }
        }
    }

    #[test]
    fn takes_the_latin_1_exceptions_with_the_hint() {
        let hinted = hinted();
        // U+00A1, A4, A7 to A8, AA, AD, AF, B0 to B4, B6 to U+00BA, BC to BF, D7 and F7.
        let east_asian = [
            0xA1, 0xA4, 0xA7, 0xA8, 0xAA, 0xAD, 0xAF, 0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB6, 0xB7,
            0xB8, 0xB9, 0xBA, 0xBC, 0xBD, 0xBE, 0xBF, 0xD7, 0xF7,
        ];
        for code in 0xA0..=0xFF {
            let expected = if east_asian.contains(&code) { E } else { H };
            assert_eq!(at(code, &hinted), expected, "U+{code:04X}");
        }
        // With Chinese, accented vowels too; without the hint, none.
        let chinese_vowels = [
            0xE0, 0xE1, 0xE8, 0xE9, 0xEA, 0xEC, 0xED, 0xF2, 0xF3, 0xF9, 0xFA, 0xFC,
        ];
        for code in 0xA0..=0xFF {
            let expected = if east_asian.contains(&code) || chinese_vowels.contains(&code) {
                E
            } else {
                H
            };
            assert_eq!(
                at(code, &chinese(hinted.clone())),
                expected,
                "U+{code:04X} zh"
            );
            assert_eq!(at(code, &chinese(style())), H, "U+{code:04X} zh, no hint");
        }
    }

    #[test]
    fn takes_latin_extensions_east_asian_only_for_chinese_with_the_hint() {
        for code in [0x0100, 0x017F, 0x0180, 0x024F, 0x0250, 0x02AF] {
            assert_eq!(at(code, &chinese(hinted())), E, "U+{code:04X}");
            assert_eq!(at(code, &charset(hinted(), EastAsiaCharset::Gb2312)), E);
            assert_eq!(
                at(code, &charset(hinted(), EastAsiaCharset::ChineseBig5)),
                E
            );
            assert_eq!(at(code, &charset(hinted(), EastAsiaCharset::Other)), H);
            assert_eq!(at(code, &chinese(style())), H);
        }
        // Latin Extended Additional depends on the language alone.
        for code in [0x1E00, 0x1EFF] {
            assert_eq!(at(code, &chinese(hinted())), E);
            assert_eq!(at(code, &charset(hinted(), EastAsiaCharset::Gb2312)), H);
        }
        // The character set alone, without the hint, does not count.
        assert_eq!(at(0x0100, &charset(style(), EastAsiaCharset::Gb2312)), H);
        // A Chinese language tag in any case; another language, Zhuang (zha) among
        // them, is not Chinese.
        let zhuang = hinted().with_language(Language::default().with_east_asia(Some("zha".into())));
        assert_eq!(at(0x0100, &zhuang), H);
        let traditional =
            hinted().with_language(Language::default().with_east_asia(Some("ZH-tw".into())));
        assert_eq!(at(0x0100, &traditional), E);
        let japanese =
            hinted().with_language(Language::default().with_east_asia(Some("ja-JP".into())));
        assert_eq!(at(0x0100, &japanese), H);
    }

    #[test]
    fn complex_script_and_right_to_left_runs_take_the_complex_script_font() {
        for code in [0x41, 0x4E00, 0x05D0] {
            assert_eq!(at(code, &style().with_complex_script(true)), C);
            assert_eq!(at(code, &style().with_rtl(true)), C);
        }
    }

    #[test]
    fn times_new_roman_for_east_asian_text_with_equal_latin_fonts_means_ascii() {
        let fonts = RunFonts::new("Calibri").with_east_asia("Times New Roman");
        let style = RunStyle::default().with_fonts(fonts.clone());
        for code in [0x41, 0xE9, 0x4E00, 0xAC00, 0x1_F600] {
            assert_eq!(at(code, &style), A, "U+{code:04X}");
        }
        // Unless the Latin fonts differ.
        let style = RunStyle::default().with_fonts(fonts.with_high_ansi("Cambria"));
        assert_eq!(at(0x4E00, &style), E);
        // The default run, all Times New Roman, takes ASCII throughout.
        assert_eq!(at(0x4E00, &RunStyle::default()), A);
    }

    #[test]
    fn compares_family_names_ignoring_case_and_surrounding_spaces() {
        let fonts = RunFonts::new("Calibri")
            .with_high_ansi(" calibri ")
            .with_east_asia("times new roman");
        assert_eq!(at(0x4E00, &RunStyle::default().with_fonts(fonts)), A);
        assert!(same_family("Times New Roman", " TIMES new roman"));
        assert!(!same_family("Times New Roman", "Times New Roman PS"));
    }

    #[test]
    fn names_the_family_of_each_slot() {
        let style = style();
        let families: Vec<&str> = Slot::ALL
            .iter()
            .map(|slot| slot.family(style.fonts()))
            .collect();
        assert_eq!(families, ["A", "H", "E", "C"]);
    }
}
