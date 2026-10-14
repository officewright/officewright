use std::borrow::Cow;

use super::chars::is_char;
use super::reader::{Reader, error_at};
use crate::{Limits, Result};

/// The text of an XML part, decoded and checked, ready to [read](Input::reader).
///
/// ECMA-376 Part 2 allows XML parts in UTF-8 and UTF-16 only. A byte order mark selects
/// the encoding; without one, UTF-16 is recognised by its first characters `<?` (XML 1.0,
/// Appendix F) and anything else is read as UTF-8, whatever its declaration says. Line
/// ends are normalised to `\n` (XML 1.0, section 2.11) and every character is checked to
/// be one that XML allows, so that the [`Reader`] works on clean text.
#[derive(Clone, Debug)]
pub struct Input<'b> {
    text: Cow<'b, str>,
}

impl<'b> Input<'b> {
    /// Decodes the bytes of an XML part.
    ///
    /// # Errors
    ///
    /// [`Error::Xml`](crate::Error::Xml) for bytes that are not valid UTF-8 or UTF-16,
    /// or for a character XML does not allow.
    pub fn decode(bytes: &'b [u8]) -> Result<Self> {
        // UTF-16 is decoded with its line ends normalised, into a string of the size it
        // needs: a part decodes with at most one copy of its text besides its bytes.
        let text = match bytes {
            [0xEF, 0xBB, 0xBF, rest @ ..] => normalize(utf8(rest)?),
            [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes)?,
            [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes)?,
            [0x3C, 0x00, 0x3F, 0x00, ..] => utf16(bytes, u16::from_le_bytes)?,
            [0x00, 0x3C, 0x00, 0x3F, ..] => utf16(bytes, u16::from_be_bytes)?,
            _ => normalize(utf8(bytes)?),
        };
        check(&text)?;
        Ok(Self { text })
    }

    /// The decoded text.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// A reader over the text.
    pub fn reader(&self, limits: &Limits) -> Reader<'_> {
        Reader::new(&self.text, limits)
    }

    /// The room the text owns, 0 when it is borrowed from the bytes.
    #[cfg(test)]
    pub(super) fn capacity(&self) -> usize {
        match &self.text {
            Cow::Borrowed(_) => 0,
            Cow::Owned(text) => text.capacity(),
        }
    }
}

fn utf8(bytes: &[u8]) -> Result<Cow<'_, str>> {
    std::str::from_utf8(bytes)
        .map(Cow::Borrowed)
        .map_err(|err| {
            let valid = bytes
                .get(..err.valid_up_to())
                .and_then(|valid| std::str::from_utf8(valid).ok())
                .unwrap_or_default();
            error_at(valid, valid.len(), "invalid UTF-8")
        })
}

/// UTF-16 decoded, with its line ends normalised as it goes.
fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> Result<Cow<'static, str>> {
    let units = || {
        bytes
            .chunks_exact(2)
            .map(|pair| <[u8; 2]>::try_from(pair).map_or(0, unit))
    };
    // The length in UTF-8: a surrogate pair is 4 bytes, so each half counts 2. Line ends
    // that normalisation shortens leave a byte or so unused.
    let length = units()
        .map(|unit| match unit {
            0..=0x7F => 1,
            0x80..=0x7FF | 0xD800..=0xDFFF => 2,
            _ => 3,
        })
        .sum();
    let mut text = String::with_capacity(length);
    let mut after_carriage_return = false;
    for c in char::decode_utf16(units()) {
        match c {
            Ok('\r') => text.push('\n'),
            Ok('\n') if after_carriage_return => {}
            Ok(c) => text.push(c),
            Err(_) => return Err(error_at(&text, text.len(), "invalid UTF-16")),
        }
        after_carriage_return = c == Ok('\r');
    }
    if bytes.len() % 2 != 0 {
        return Err(error_at(
            &text,
            text.len(),
            "UTF-16 text has an odd number of bytes",
        ));
    }
    Ok(Cow::Owned(text))
}

/// Line ends normalised to `\n` (XML 1.0, section 2.11); a copy only when the text has a
/// carriage return.
fn normalize(text: Cow<'_, str>) -> Cow<'_, str> {
    if !text.contains('\r') {
        return text;
    }
    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            normalized.push('\n');
            chars.next_if_eq(&'\n');
        } else {
            normalized.push(c);
        }
    }
    Cow::Owned(normalized)
}

/// Checks that every character is one XML allows.
fn check(text: &str) -> Result<()> {
    match text.char_indices().find(|(_, c)| !is_char(*c)) {
        Some((at, c)) => Err(error_at(
            text,
            at,
            format!("character U+{:04X} is not allowed in XML", u32::from(c)),
        )),
        None => Ok(()),
    }
}
