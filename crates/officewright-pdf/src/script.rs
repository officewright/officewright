//! Which characters an embedded font can show one glyph per character.
//!
//! Text is drawn without shaping (no ligatures, contextual forms or mark positioning)
//! and without bidirectional layout. That is correct only for scripts whose characters
//! each map to one glyph laid out left to right, so every other script is refused
//! rather than drawn wrong. The scripts come from the Unicode Script property (UAX #24),
//! as the `unicode-script` crate provides it.

use unicode_script::{Script, UnicodeScript};

/// Why a character cannot be drawn without shaping or bidirectional layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A control character (general category Cc).
    Control,
    /// An explicit bidirectional formatting character.
    Bidi,
    /// A conjoining Hangul jamo, which only shaping assembles into syllables.
    Jamo,
    /// A character of a script that needs shaping or right-to-left layout.
    Script(Script),
    /// An unassigned or private-use code point, or a noncharacter.
    Unknown,
}

/// Fails unless `ch` can be drawn as one glyph, left to right.
///
/// Allowed are the scripts that need no shaping: Latin, Greek, Cyrillic, Armenian,
/// Georgian, Han, Hiragana, Katakana and precomposed Hangul syllables, and the Common
/// and Inherited characters (punctuation, digits, symbols, combining marks, which are
/// drawn where the font puts them).
pub(crate) fn check(ch: char) -> Result<(), Refusal> {
    if ch.is_control() {
        return Err(Refusal::Control);
    }
    if is_bidi_control(ch) {
        return Err(Refusal::Bidi);
    }
    if is_conjoining_jamo(ch) {
        return Err(Refusal::Jamo);
    }
    match ch.script() {
        Script::Latin
        | Script::Greek
        | Script::Cyrillic
        | Script::Armenian
        | Script::Georgian
        | Script::Han
        | Script::Hiragana
        | Script::Katakana
        | Script::Hangul
        | Script::Common
        | Script::Inherited => Ok(()),
        Script::Unknown => Err(Refusal::Unknown),
        script => Err(Refusal::Script(script)),
    }
}

/// The explicit bidirectional formatting characters, the `Bidi_Control` property of
/// Unicode's PropList.txt (UAX #9, 2.1 to 2.6): ALM, LRM, RLM, LRE to RLO, LRI to PDI.
/// Their script is Common, so the script check alone would let them through.
fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// The conjoining jamo blocks: Hangul Jamo, Hangul Jamo Extended-A and Extended-B.
/// Their script is Hangul, but a syllable written with them needs shaping. The
/// compatibility jamo (U+3130 to U+318F) and halfwidth jamo stand alone and are allowed.
fn is_conjoining_jamo(ch: char) -> bool {
    matches!(
        ch,
        '\u{1100}'..='\u{11FF}' | '\u{A960}'..='\u{A97F}' | '\u{D7B0}'..='\u{D7FF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_scripts_that_need_no_shaping() {
        for ch in [
            'A', 'ß', 'Ę', 'Ω', 'ά', 'Ж', 'ї', 'Ա', 'ა', '漢', 'あ', 'ア', '한', 'ㄱ', '1', ',',
            ' ', '€', '\u{A0}', '\u{301}', '\u{200D}', '\u{FE0F}',
        ] {
            assert_eq!(check(ch), Ok(()), "U+{:04X}", u32::from(ch));
        }
    }

    #[test]
    fn refuses_scripts_that_need_shaping_or_right_to_left_layout() {
        for (ch, script) in [
            ('א', Script::Hebrew),
            ('ب', Script::Arabic),
            ('क', Script::Devanagari),
            ('ก', Script::Thai),
            ('ក', Script::Khmer),
            ('ᠠ', Script::Mongolian),
            ('ㄅ', Script::Bopomofo),
        ] {
            assert_eq!(check(ch), Err(Refusal::Script(script)), "{ch}");
        }
    }

    #[test]
    fn refuses_controls_bidi_formatting_jamo_and_unassigned_code_points() {
        for ch in ['\0', '\t', '\n', '\u{7F}', '\u{85}'] {
            assert_eq!(check(ch), Err(Refusal::Control), "U+{:04X}", u32::from(ch));
        }
        for ch in [
            '\u{61C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}',
        ] {
            assert_eq!(check(ch), Err(Refusal::Bidi), "U+{:04X}", u32::from(ch));
        }
        for ch in ['\u{1100}', '\u{11FF}', '\u{A960}', '\u{D7FB}'] {
            assert_eq!(check(ch), Err(Refusal::Jamo), "U+{:04X}", u32::from(ch));
        }
        for ch in ['\u{378}', '\u{E000}', '\u{F8FF}', '\u{FFFF}', '\u{10FFFF}'] {
            assert_eq!(check(ch), Err(Refusal::Unknown), "U+{:04X}", u32::from(ch));
        }
    }
}
