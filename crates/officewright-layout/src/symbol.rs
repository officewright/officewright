//! Symbol fonts: the codes of Symbol and Wingdings as Unicode characters, for when the
//! fonts themselves are not installed, as on every Linux machine.
//!
//! The Symbol table is derived from `symbol.txt`, "Adobe Symbol Encoding to Unicode",
//! table version 1.0 of 2011 July 12, in Unicode's mapping files (MAPPINGS/VENDORS/ADOBE).
//! Its notice, which must stay attached:
//!
//! > Copyright (c) 1991-2011 Unicode, Inc. All Rights reserved.
//! >
//! > This file is provided as-is by Unicode, Inc. (The Unicode Consortium). No claims are
//! > made as to fitness for any particular purpose. No warranties of any kind are
//! > expressed or implied. The recipient agrees to determine applicability of
//! > information provided. If this file has been provided on magnetic media by Unicode,
//! > Inc., the sole remedy for any claim will be exchange of defective media within 90
//! > days of receipt.
//! >
//! > Unicode, Inc. hereby grants the right to freely use the information supplied in
//! > this file in the creation of products supporting the Unicode Standard, and to make
//! > copies of this file in any form for internal or external distribution as long as
//! > this notice remains attached.
//!
//! See THIRD-PARTY-NOTICES.md in the repository; `tests/symbol_table.rs` checks the table
//! against the file, which is kept in `data/` and left out of the published crate.

// reason: the layout itself uses this once pagination (plan PR 8) lays out pages; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

/// The characters of the Symbol encoding (Adobe's table): where Adobe gives two, the
/// Greek letter, else the first; codes Adobe maps only to its Corporate Use Subarea are
/// left out, apart from those of [`SYMBOL_PRIVATE`].
#[rustfmt::skip]
const SYMBOL: &[(u8, char)] = &[
    (0x20, '\u{0020}'), // SPACE
    (0x21, '\u{0021}'), // EXCLAMATION MARK
    (0x22, '\u{2200}'), // FOR ALL
    (0x23, '\u{0023}'), // NUMBER SIGN
    (0x24, '\u{2203}'), // THERE EXISTS
    (0x25, '\u{0025}'), // PERCENT SIGN
    (0x26, '\u{0026}'), // AMPERSAND
    (0x27, '\u{220B}'), // CONTAINS AS MEMBER
    (0x28, '\u{0028}'), // LEFT PARENTHESIS
    (0x29, '\u{0029}'), // RIGHT PARENTHESIS
    (0x2A, '\u{2217}'), // ASTERISK OPERATOR
    (0x2B, '\u{002B}'), // PLUS SIGN
    (0x2C, '\u{002C}'), // COMMA
    (0x2D, '\u{2212}'), // MINUS SIGN
    (0x2E, '\u{002E}'), // FULL STOP
    (0x2F, '\u{002F}'), // SOLIDUS
    (0x30, '\u{0030}'), // DIGIT ZERO
    (0x31, '\u{0031}'), // DIGIT ONE
    (0x32, '\u{0032}'), // DIGIT TWO
    (0x33, '\u{0033}'), // DIGIT THREE
    (0x34, '\u{0034}'), // DIGIT FOUR
    (0x35, '\u{0035}'), // DIGIT FIVE
    (0x36, '\u{0036}'), // DIGIT SIX
    (0x37, '\u{0037}'), // DIGIT SEVEN
    (0x38, '\u{0038}'), // DIGIT EIGHT
    (0x39, '\u{0039}'), // DIGIT NINE
    (0x3A, '\u{003A}'), // COLON
    (0x3B, '\u{003B}'), // SEMICOLON
    (0x3C, '\u{003C}'), // LESS-THAN SIGN
    (0x3D, '\u{003D}'), // EQUALS SIGN
    (0x3E, '\u{003E}'), // GREATER-THAN SIGN
    (0x3F, '\u{003F}'), // QUESTION MARK
    (0x40, '\u{2245}'), // APPROXIMATELY EQUAL TO
    (0x41, '\u{0391}'), // GREEK CAPITAL LETTER ALPHA
    (0x42, '\u{0392}'), // GREEK CAPITAL LETTER BETA
    (0x43, '\u{03A7}'), // GREEK CAPITAL LETTER CHI
    (0x44, '\u{0394}'), // GREEK CAPITAL LETTER DELTA
    (0x45, '\u{0395}'), // GREEK CAPITAL LETTER EPSILON
    (0x46, '\u{03A6}'), // GREEK CAPITAL LETTER PHI
    (0x47, '\u{0393}'), // GREEK CAPITAL LETTER GAMMA
    (0x48, '\u{0397}'), // GREEK CAPITAL LETTER ETA
    (0x49, '\u{0399}'), // GREEK CAPITAL LETTER IOTA
    (0x4A, '\u{03D1}'), // GREEK THETA SYMBOL
    (0x4B, '\u{039A}'), // GREEK CAPITAL LETTER KAPPA
    (0x4C, '\u{039B}'), // GREEK CAPITAL LETTER LAMDA
    (0x4D, '\u{039C}'), // GREEK CAPITAL LETTER MU
    (0x4E, '\u{039D}'), // GREEK CAPITAL LETTER NU
    (0x4F, '\u{039F}'), // GREEK CAPITAL LETTER OMICRON
    (0x50, '\u{03A0}'), // GREEK CAPITAL LETTER PI
    (0x51, '\u{0398}'), // GREEK CAPITAL LETTER THETA
    (0x52, '\u{03A1}'), // GREEK CAPITAL LETTER RHO
    (0x53, '\u{03A3}'), // GREEK CAPITAL LETTER SIGMA
    (0x54, '\u{03A4}'), // GREEK CAPITAL LETTER TAU
    (0x55, '\u{03A5}'), // GREEK CAPITAL LETTER UPSILON
    (0x56, '\u{03C2}'), // GREEK SMALL LETTER FINAL SIGMA
    (0x57, '\u{03A9}'), // GREEK CAPITAL LETTER OMEGA
    (0x58, '\u{039E}'), // GREEK CAPITAL LETTER XI
    (0x59, '\u{03A8}'), // GREEK CAPITAL LETTER PSI
    (0x5A, '\u{0396}'), // GREEK CAPITAL LETTER ZETA
    (0x5B, '\u{005B}'), // LEFT SQUARE BRACKET
    (0x5C, '\u{2234}'), // THEREFORE
    (0x5D, '\u{005D}'), // RIGHT SQUARE BRACKET
    (0x5E, '\u{22A5}'), // UP TACK
    (0x5F, '\u{005F}'), // LOW LINE
    (0x61, '\u{03B1}'), // GREEK SMALL LETTER ALPHA
    (0x62, '\u{03B2}'), // GREEK SMALL LETTER BETA
    (0x63, '\u{03C7}'), // GREEK SMALL LETTER CHI
    (0x64, '\u{03B4}'), // GREEK SMALL LETTER DELTA
    (0x65, '\u{03B5}'), // GREEK SMALL LETTER EPSILON
    (0x66, '\u{03C6}'), // GREEK SMALL LETTER PHI
    (0x67, '\u{03B3}'), // GREEK SMALL LETTER GAMMA
    (0x68, '\u{03B7}'), // GREEK SMALL LETTER ETA
    (0x69, '\u{03B9}'), // GREEK SMALL LETTER IOTA
    (0x6A, '\u{03D5}'), // GREEK PHI SYMBOL
    (0x6B, '\u{03BA}'), // GREEK SMALL LETTER KAPPA
    (0x6C, '\u{03BB}'), // GREEK SMALL LETTER LAMDA
    (0x6D, '\u{03BC}'), // GREEK SMALL LETTER MU
    (0x6E, '\u{03BD}'), // GREEK SMALL LETTER NU
    (0x6F, '\u{03BF}'), // GREEK SMALL LETTER OMICRON
    (0x70, '\u{03C0}'), // GREEK SMALL LETTER PI
    (0x71, '\u{03B8}'), // GREEK SMALL LETTER THETA
    (0x72, '\u{03C1}'), // GREEK SMALL LETTER RHO
    (0x73, '\u{03C3}'), // GREEK SMALL LETTER SIGMA
    (0x74, '\u{03C4}'), // GREEK SMALL LETTER TAU
    (0x75, '\u{03C5}'), // GREEK SMALL LETTER UPSILON
    (0x76, '\u{03D6}'), // GREEK PI SYMBOL
    (0x77, '\u{03C9}'), // GREEK SMALL LETTER OMEGA
    (0x78, '\u{03BE}'), // GREEK SMALL LETTER XI
    (0x79, '\u{03C8}'), // GREEK SMALL LETTER PSI
    (0x7A, '\u{03B6}'), // GREEK SMALL LETTER ZETA
    (0x7B, '\u{007B}'), // LEFT CURLY BRACKET
    (0x7C, '\u{007C}'), // VERTICAL LINE
    (0x7D, '\u{007D}'), // RIGHT CURLY BRACKET
    (0x7E, '\u{223C}'), // TILDE OPERATOR
    (0xA0, '\u{20AC}'), // EURO SIGN
    (0xA1, '\u{03D2}'), // GREEK UPSILON WITH HOOK SYMBOL
    (0xA2, '\u{2032}'), // PRIME
    (0xA3, '\u{2264}'), // LESS-THAN OR EQUAL TO
    (0xA4, '\u{2044}'), // FRACTION SLASH
    (0xA5, '\u{221E}'), // INFINITY
    (0xA6, '\u{0192}'), // LATIN SMALL LETTER F WITH HOOK
    (0xA7, '\u{2663}'), // BLACK CLUB SUIT
    (0xA8, '\u{2666}'), // BLACK DIAMOND SUIT
    (0xA9, '\u{2665}'), // BLACK HEART SUIT
    (0xAA, '\u{2660}'), // BLACK SPADE SUIT
    (0xAB, '\u{2194}'), // LEFT RIGHT ARROW
    (0xAC, '\u{2190}'), // LEFTWARDS ARROW
    (0xAD, '\u{2191}'), // UPWARDS ARROW
    (0xAE, '\u{2192}'), // RIGHTWARDS ARROW
    (0xAF, '\u{2193}'), // DOWNWARDS ARROW
    (0xB0, '\u{00B0}'), // DEGREE SIGN
    (0xB1, '\u{00B1}'), // PLUS-MINUS SIGN
    (0xB2, '\u{2033}'), // DOUBLE PRIME
    (0xB3, '\u{2265}'), // GREATER-THAN OR EQUAL TO
    (0xB4, '\u{00D7}'), // MULTIPLICATION SIGN
    (0xB5, '\u{221D}'), // PROPORTIONAL TO
    (0xB6, '\u{2202}'), // PARTIAL DIFFERENTIAL
    (0xB7, '\u{2022}'), // BULLET
    (0xB8, '\u{00F7}'), // DIVISION SIGN
    (0xB9, '\u{2260}'), // NOT EQUAL TO
    (0xBA, '\u{2261}'), // IDENTICAL TO
    (0xBB, '\u{2248}'), // ALMOST EQUAL TO
    (0xBC, '\u{2026}'), // HORIZONTAL ELLIPSIS
    (0xBF, '\u{21B5}'), // DOWNWARDS ARROW WITH CORNER LEFTWARDS
    (0xC0, '\u{2135}'), // ALEF SYMBOL
    (0xC1, '\u{2111}'), // BLACK-LETTER CAPITAL I
    (0xC2, '\u{211C}'), // BLACK-LETTER CAPITAL R
    (0xC3, '\u{2118}'), // SCRIPT CAPITAL P
    (0xC4, '\u{2297}'), // CIRCLED TIMES
    (0xC5, '\u{2295}'), // CIRCLED PLUS
    (0xC6, '\u{2205}'), // EMPTY SET
    (0xC7, '\u{2229}'), // INTERSECTION
    (0xC8, '\u{222A}'), // UNION
    (0xC9, '\u{2283}'), // SUPERSET OF
    (0xCA, '\u{2287}'), // SUPERSET OF OR EQUAL TO
    (0xCB, '\u{2284}'), // NOT A SUBSET OF
    (0xCC, '\u{2282}'), // SUBSET OF
    (0xCD, '\u{2286}'), // SUBSET OF OR EQUAL TO
    (0xCE, '\u{2208}'), // ELEMENT OF
    (0xCF, '\u{2209}'), // NOT AN ELEMENT OF
    (0xD0, '\u{2220}'), // ANGLE
    (0xD1, '\u{2207}'), // NABLA
    (0xD5, '\u{220F}'), // N-ARY PRODUCT
    (0xD6, '\u{221A}'), // SQUARE ROOT
    (0xD7, '\u{22C5}'), // DOT OPERATOR
    (0xD8, '\u{00AC}'), // NOT SIGN
    (0xD9, '\u{2227}'), // LOGICAL AND
    (0xDA, '\u{2228}'), // LOGICAL OR
    (0xDB, '\u{21D4}'), // LEFT RIGHT DOUBLE ARROW
    (0xDC, '\u{21D0}'), // LEFTWARDS DOUBLE ARROW
    (0xDD, '\u{21D1}'), // UPWARDS DOUBLE ARROW
    (0xDE, '\u{21D2}'), // RIGHTWARDS DOUBLE ARROW
    (0xDF, '\u{21D3}'), // DOWNWARDS DOUBLE ARROW
    (0xE0, '\u{25CA}'), // LOZENGE
    (0xE1, '\u{2329}'), // LEFT-POINTING ANGLE BRACKET
    (0xE5, '\u{2211}'), // N-ARY SUMMATION
    (0xF1, '\u{232A}'), // RIGHT-POINTING ANGLE BRACKET
    (0xF2, '\u{222B}'), // INTEGRAL
    (0xF3, '\u{2320}'), // TOP HALF INTEGRAL
    (0xF5, '\u{2321}'), // BOTTOM HALF INTEGRAL
];

/// Symbol codes that Adobe's table maps only to its Corporate Use Subarea, and the
/// standard character each stands for: the serif and sans-serif forms of the registered,
/// copyright and trademark signs. The bracket, brace, radical and arrow extenders have no
/// such character and are left out. This table is ours, not Adobe's.
const SYMBOL_PRIVATE: [(u8, char); 6] = [
    (0xD2, '\u{00AE}'),
    (0xD3, '\u{00A9}'),
    (0xD4, '\u{2122}'),
    (0xE2, '\u{00AE}'),
    (0xE3, '\u{00A9}'),
    (0xE4, '\u{2122}'),
];

/// Wingdings codes of the bullets Word offers for lists, and the Unicode character of the
/// nearest shape. Ours, from the glyphs Word draws; other Wingdings codes have none.
const WINGDINGS: [(u8, char); 10] = [
    (0x6C, '\u{25CF}'), // black circle
    (0x6E, '\u{25A0}'), // black square
    (0x6F, '\u{25A1}'), // white square
    (0x71, '\u{2751}'), // lower right shadowed white square
    (0x75, '\u{25C6}'), // black diamond
    (0x76, '\u{2756}'), // black diamond minus white X
    (0xA7, '\u{25AA}'), // black small square
    (0xA8, '\u{25FB}'), // white medium square
    (0xD8, '\u{27A2}'), // three-D top-lighted rightwards arrowhead
    (0xFC, '\u{2713}'), // check mark
];

/// A symbol font whose codes have Unicode characters here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SymbolFont {
    /// Symbol, Adobe's encoding.
    Symbol,
    /// Wingdings, its list bullets.
    Wingdings,
}

impl SymbolFont {
    /// The symbol font of this family, if it is one.
    pub(crate) fn of(family: &str) -> Option<Self> {
        if family.eq_ignore_ascii_case("Symbol") {
            Some(Self::Symbol)
        } else if family.eq_ignore_ascii_case("Wingdings") {
            Some(Self::Wingdings)
        } else {
            None
        }
    }

    /// The Unicode character of a code of this font: the code itself (U+0020 to
    /// U+00FF) or as symbol fonts map it, U+F020 to U+F0FF.
    pub(crate) fn unicode(self, code: u32) -> Option<char> {
        let code = match code {
            0x20..=0xFF => code,
            0xF020..=0xF0FF => code - 0xF000,
            _ => return None,
        };
        let code = u8::try_from(code).ok()?;
        let find = |table: &[(u8, char)]| {
            table
                .iter()
                .find(|(entry, _)| *entry == code)
                .map(|(_, ch)| *ch)
        };
        match self {
            Self::Symbol => find(SYMBOL).or_else(|| find(&SYMBOL_PRIVATE)),
            Self::Wingdings => find(&WINGDINGS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_symbol_codes_as_adobes_table_has_them() {
        let symbol = SymbolFont::of("Symbol").unwrap();
        for (code, expected) in [
            (0x61, '\u{03B1}'), // alpha
            (0xF061, '\u{03B1}'),
            (0xB7, '\u{2022}'), // bullet, the list bullet of Word
            (0xF0B7, '\u{2022}'),
            (0x44, '\u{0394}'), // Delta: the Greek letter, not the increment
            (0x57, '\u{03A9}'), // Omega, not the ohm sign
            (0x6D, '\u{03BC}'), // mu, not the micro sign
            (0x20, ' '),        // space, not the no-break space
            (0xA4, '\u{2044}'), // fraction slash, Adobe's first
            (0xA5, '\u{221E}'), // infinity
            (0xD2, '\u{00AE}'), // Adobe's serif registered sign, ours to U+00AE
            (0xE3, '\u{00A9}'), // sans-serif copyright sign
            (0xD4, '\u{2122}'),
        ] {
            assert_eq!(symbol.unicode(code), Some(expected), "{code:04X}");
        }
        // Extenders have no character, nor do codes outside the font.
        for code in [
            0x60, 0xBD, 0xBE, 0xE6, 0xFE, 0x1F, 0x100, 0xF01F, 0xF100, 0xFFFF,
        ] {
            assert_eq!(symbol.unicode(code), None, "{code:04X}");
        }
        assert_eq!(SYMBOL.len(), 160);
    }

    #[test]
    fn reads_the_wingdings_bullets_only() {
        let wingdings = SymbolFont::of("Wingdings").unwrap();
        assert_eq!(wingdings.unicode(0xF0A7), Some('\u{25AA}'));
        assert_eq!(wingdings.unicode(0xA7), Some('\u{25AA}'));
        assert_eq!(wingdings.unicode(0xF0FC), Some('\u{2713}'));
        assert_eq!(wingdings.unicode(0xF0D8), Some('\u{27A2}'));
        assert_eq!(wingdings.unicode(0xF076), Some('\u{2756}'));
        assert_eq!(wingdings.unicode(0xF041), None);
        assert_eq!(wingdings.unicode(0xF0B7), None);
    }

    #[test]
    fn knows_symbol_fonts_by_family_in_any_case() {
        assert_eq!(SymbolFont::of("SYMBOL"), Some(SymbolFont::Symbol));
        assert_eq!(SymbolFont::of("wingdings"), Some(SymbolFont::Wingdings));
        assert_eq!(SymbolFont::of("Wingdings 2"), None);
        assert_eq!(SymbolFont::of("Arial"), None);
    }
}
