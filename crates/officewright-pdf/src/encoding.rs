//! WinAnsiEncoding, the encoding the standard Helvetica font is used with.

/// Returns the WinAnsiEncoding code of `ch`, if it has one (ISO 32000-1, Annex D.2).
///
/// Codes 0x81, 0x8D, 0x8F, 0x90 and 0x9D and the control characters are unused.
pub(crate) fn win_ansi(ch: char) -> Option<u8> {
    let code = match ch {
        // Printable ASCII and Latin-1 keep their code points. Annex D shows 0xA0 (no-break
        // space) with the space glyph and 0xAD (soft hyphen) with the hyphen glyph.
        ' '..='~' | '\u{A0}'..='\u{FF}' => return u8::try_from(ch).ok(),
        '\u{20AC}' => 0x80, // Euro
        '\u{201A}' => 0x82, // quotesinglbase
        '\u{0192}' => 0x83, // florin
        '\u{201E}' => 0x84, // quotedblbase
        '\u{2026}' => 0x85, // ellipsis
        '\u{2020}' => 0x86, // dagger
        '\u{2021}' => 0x87, // daggerdbl
        '\u{02C6}' => 0x88, // circumflex
        '\u{2030}' => 0x89, // perthousand
        '\u{0160}' => 0x8A, // Scaron
        '\u{2039}' => 0x8B, // guilsinglleft
        '\u{0152}' => 0x8C, // OE
        '\u{017D}' => 0x8E, // Zcaron
        '\u{2018}' => 0x91, // quoteleft
        '\u{2019}' => 0x92, // quoteright
        '\u{201C}' => 0x93, // quotedblleft
        '\u{201D}' => 0x94, // quotedblright
        '\u{2022}' => 0x95, // bullet
        '\u{2013}' => 0x96, // endash
        '\u{2014}' => 0x97, // emdash
        '\u{02DC}' => 0x98, // tilde
        '\u{2122}' => 0x99, // trademark
        '\u{0161}' => 0x9A, // scaron
        '\u{203A}' => 0x9B, // guilsinglright
        '\u{0153}' => 0x9C, // oe
        '\u{017E}' => 0x9E, // zcaron
        '\u{0178}' => 0x9F, // Ydieresis
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    fn encode(text: &str) -> crate::Result<Vec<u8>> {
        crate::Font::standard(crate::StandardFont::Helvetica).encode(text)
    }

    #[test]
    fn encodes_latin_text() {
        assert_eq!(
            encode("Grüße €5 — „ok“").unwrap(),
            b"Gr\xFC\xDFe \x805 \x97 \x84ok\x93"
        );
    }

    #[test]
    fn reports_the_first_unencodable_character_and_its_offset() {
        let err = encode("ab Жк").unwrap_err();
        assert!(
            matches!(
                err,
                Error::Unencodable {
                    ch: 'Ж', offset: 3
                }
            ),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "character U+0416 'Ж' at byte 3 is not in WinAnsiEncoding"
        );
    }

    #[test]
    fn rejects_control_characters_and_unused_codes() {
        for ch in [
            '\0', '\t', '\n', '\r', '\u{7F}', '\u{81}', '\u{9F}', '\u{FEFF}',
        ] {
            assert_eq!(win_ansi(ch), None, "{ch:?}");
        }
    }

    #[test]
    fn maps_each_code_to_at_most_one_character() {
        let mut seen = [false; 256];
        for ch in (0..=0x10FFFF).filter_map(char::from_u32) {
            if let Some(code) = win_ansi(ch) {
                assert!(!seen[usize::from(code)], "code {code:#04X} used twice");
                seen[usize::from(code)] = true;
            }
        }
        assert_eq!(seen.iter().filter(|&&used| used).count(), 218);
    }
}
