use std::borrow::Cow;

use super::chars::is_char;
use super::reader::{Reader, error_at};
use crate::{Limits, Result};

/// The text of an XML part, decoded and checked, ready to [read](Input::reader).
///
/// ECMA-376 Part 2 allows XML parts in UTF-8 and UTF-16 only. A byte order mark selects
/// the encoding; without one, UTF-16 is recognised by its first characters `<?` (XML 1.0,
/// Appendix F) and anything else is read as UTF-8. Line ends are normalised to `\n` (XML
/// 1.0, section 2.11) and every character is checked to be one that XML allows, so that
/// the [`Reader`] works on clean text.
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
        let text = match bytes {
            [0xEF, 0xBB, 0xBF, rest @ ..] => utf8(rest)?,
            [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes)?,
            [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes)?,
            [0x3C, 0x00, 0x3F, 0x00, ..] => utf16(bytes, u16::from_le_bytes)?,
            [0x00, 0x3C, 0x00, 0x3F, ..] => utf16(bytes, u16::from_be_bytes)?,
            _ => utf8(bytes)?,
        };
        Ok(Self {
            text: normalize(text)?,
        })
    }

    /// The decoded text.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// A reader over the text.
    pub fn reader(&self, limits: &Limits) -> Reader<'_> {
        Reader::new(&self.text, limits)
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

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> Result<Cow<'static, str>> {
    let mut text = String::with_capacity(bytes.len() / 2);
    let units = bytes
        .chunks_exact(2)
        .map(|pair| <[u8; 2]>::try_from(pair).map_or(0, unit));
    for c in char::decode_utf16(units) {
        match c {
            Ok(c) => text.push(c),
            Err(_) => return Err(error_at(&text, text.len(), "invalid UTF-16")),
        }
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

fn normalize(text: Cow<'_, str>) -> Result<Cow<'_, str>> {
    let mut carriage_return = false;
    for (at, c) in text.char_indices() {
        if c == '\r' {
            carriage_return = true;
        } else if !is_char(c) {
            return Err(error_at(
                &text,
                at,
                format!("character U+{:04X} is not allowed in XML", u32::from(c)),
            ));
        }
    }
    if !carriage_return {
        return Ok(text);
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
    Ok(Cow::Owned(normalized))
}
