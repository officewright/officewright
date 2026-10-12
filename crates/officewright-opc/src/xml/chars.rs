//! Character classes of XML 1.0 (fifth edition) and Namespaces in XML 1.0.

/// `Char` (production 2): the characters a document may contain.
pub(crate) fn is_char(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    )
}

/// `S` (production 3).
pub(crate) fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// `NameStartChar` (production 4) without the colon: the first character of an `NCName`.
pub(crate) fn is_name_start(c: char) -> bool {
    matches!(
        c,
        'A'..='Z'
            | '_'
            | 'a'..='z'
            | '\u{C0}'..='\u{D6}'
            | '\u{D8}'..='\u{F6}'
            | '\u{F8}'..='\u{2FF}'
            | '\u{370}'..='\u{37D}'
            | '\u{37F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}'
            | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}'
            | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}'
    )
}

/// `NameChar` (production 4a) without the colon.
pub(crate) fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(
            c,
            '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_characters_at_the_range_edges() {
        for c in [
            '\t',
            '\n',
            '\r',
            ' ',
            '\u{D7FF}',
            '\u{E000}',
            '\u{FFFD}',
            '\u{10FFFF}',
        ] {
            assert!(is_char(c), "{c:?}");
        }
        for c in ['\0', '\u{8}', '\u{B}', '\u{1F}', '\u{FFFE}', '\u{FFFF}'] {
            assert!(!is_char(c), "{c:?}");
        }
        assert!(is_name_start('Ж') && is_name_start('中') && is_name_start('_'));
        assert!(!is_name_start('1') && !is_name_start('-') && !is_name_start(':'));
        assert!(is_name_char('1') && is_name_char('-') && is_name_char('\u{B7}'));
        assert!(!is_name_char(':') && !is_name_char(' ') && !is_name_char('\u{D7}'));
    }
}
