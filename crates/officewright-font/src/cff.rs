//! A check of CFF data that the subsetter cannot handle yet.
//!
//! subsetter 0.2.6 keeps the real-number operands of the DICTs it rewrites as `f32` and
//! writes them back from their decimal form. A real too large for an `f32` becomes
//! infinity, which it cannot write, and it panics with `unreachable!()` (in
//! `RealNumber::write`, `src/cff/number.rs`). `FontFace::subset` would turn that panic
//! into an error, but the panic hook still prints it, and the fuzz targets, whose hook
//! aborts, would report it on every run. Such a face is damaged, so it is refused here,
//! before the subsetter runs. Remove this once a subsetter release returns an error.

use std::ops::Range;

use skrifa::raw::ps::cff::dict::{self, Entry};
use skrifa::raw::ps::cff::index::Index;
use skrifa::raw::types::Tag;
use skrifa::raw::{FontRef, TableProvider};

use crate::error::{Cause, Error, Result};

/// The most characters a real number may have; the subsetter rejects longer ones.
const MAX_REAL_LEN: usize = 64;

/// Fails if a DICT of the face's `CFF ` table has a real number that is finite in the
/// font but too large for an `f32`: the Top DICTs, the Font DICTs of a CID-keyed font
/// and their Private DICTs, every DICT the subsetter rewrites. Structures that cannot be
/// read are left to the subsetter, which reports them as errors.
pub(crate) fn check_reals(font: &FontRef<'_>, face: u32) -> Result<()> {
    let Some(data) = font.table_data(Tag::new(b"CFF ")) else {
        return Ok(());
    };
    let data = data.as_bytes();
    let Ok(cff) = font.cff() else {
        return Ok(());
    };
    let top_dicts = cff.top_dicts();
    let mut dicts: Vec<&[u8]> = Vec::new();
    let mut privates: Vec<Range<usize>> = Vec::new();
    for top in (0..top_dicts.count() as usize).filter_map(|i| top_dicts.get(i)) {
        dicts.push(top);
        for entry in dict::entries(top, None).flatten() {
            match entry {
                Entry::PrivateDictRange(range) => privates.push(range),
                Entry::FdArrayOffset(offset) => {
                    let Some(Ok(font_dicts)) = data.get(offset..).map(|d| Index::new(d, false))
                    else {
                        continue;
                    };
                    for font_dict in
                        (0..font_dicts.count() as usize).filter_map(|i| font_dicts.get(i))
                    {
                        dicts.push(font_dict);
                        for entry in dict::entries(font_dict, None).flatten() {
                            if let Entry::PrivateDictRange(range) = entry {
                                privates.push(range);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    dicts.extend(privates.into_iter().filter_map(|range| data.get(range)));
    for dict in dicts {
        if let Some(real) = overflowing_real(dict) {
            return Err(Error::MalformedTable {
                face,
                table: "CFF ",
                source: Cause::new(format!(
                    "the real number {real} in a DICT is too large to subset"
                )),
            });
        }
    }
    Ok(())
}

/// The first real-number operand in DICT data that reads as a finite number but is
/// infinite as an `f32`, in its decimal form.
fn overflowing_real(dict: &[u8]) -> Option<String> {
    let mut at = 0;
    while let Some(&b0) = dict.get(at) {
        at += 1;
        match b0 {
            // An operator; 12 starts a two-byte one. Like the subsetter, the reserved
            // bytes 22 to 27, 31 and 255 count as operators too.
            12 => at += 1,
            0..=27 | 31 | 255 => {}
            28 => at += 2,
            29 => at += 4,
            30 => {
                let (real, len) = real(dict.get(at..)?);
                at += len;
                if let Some(real) = real {
                    let value: Option<f32> = real.parse().ok();
                    if value.is_some_and(|value| value.is_infinite()) {
                        return Some(real);
                    }
                }
            }
            32..=246 => {}
            247..=254 => at += 1,
        }
    }
    None
}

/// Reads the nibbles of a real number (CFF specification, Table 5) after its prefix
/// byte, as the subsetter does. Returns its decimal form, `None` if the subsetter would
/// reject it, and the number of bytes it takes.
fn real(data: &[u8]) -> (Option<String>, usize) {
    let mut text = String::new();
    let mut valid = true;
    for (i, &byte) in data.iter().enumerate() {
        for nibble in [byte >> 4, byte & 0xF] {
            match nibble {
                0..=9 => text.push(char::from(b'0' + nibble)),
                0xA => text.push('.'),
                0xB => text.push('E'),
                0xC => text.push_str("E-"),
                0xE => text.push('-'),
                0xF => return (valid.then_some(text), i + 1),
                // 0xD is reserved.
                _ => valid = false,
            }
        }
        if text.len() > MAX_REAL_LEN {
            valid = false;
        }
    }
    (None, data.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_numbers_as_the_subsetter_does() {
        // 0.039625, -2.25, 1.5E-3, from the CFF specification's encoding.
        assert_eq!(
            real(&[0x0A, 0x03, 0x96, 0x25, 0xFF]),
            (Some("0.039625".into()), 5)
        );
        assert_eq!(real(&[0xE2, 0xA2, 0x5F]), (Some("-2.25".into()), 3));
        assert_eq!(real(&[0x1A, 0x5C, 0x3F]), (Some("1.5E-3".into()), 3));
        // A reserved nibble, and no end.
        assert_eq!(real(&[0x1D, 0xFF]), (None, 2));
        assert_eq!(real(&[0x12, 0x34]), (None, 2));
    }

    #[test]
    fn finds_reals_too_large_for_an_f32() {
        // BlueScale 0.039625 (12 9), then a real 1E99999 with operator BlueShift (12 10).
        let fine = [30, 0x0A, 0x03, 0x96, 0x25, 0xFF, 12, 9];
        assert_eq!(overflowing_real(&fine), None);
        let huge = [
            &fine[..],
            &[139, 28, 0, 1, 30, 0x1B, 0x99, 0x99, 0x9F, 12, 10],
        ]
        .concat();
        assert_eq!(overflowing_real(&huge).as_deref(), Some("1E99999"));
        // -4E38 is past f32::MAX too; 3E38 is not.
        assert_eq!(
            overflowing_real(&[30, 0xE4, 0xB3, 0x8F, 1]).as_deref(),
            Some("-4E38")
        );
        assert_eq!(overflowing_real(&[30, 0x3B, 0x38, 0xFF, 1]), None);
        // A real cut off by the end of the data is left to the subsetter.
        assert_eq!(overflowing_real(&[30, 0x1B, 0x99]), None);
        // Reserved bytes are operators, and reading goes on after them.
        let after_reserved = [38, 31, 22, 255, 30, 0x88, 0x06, 0x82, 0xB7, 0x05, 0xFB, 24];
        assert_eq!(
            overflowing_real(&after_reserved).as_deref(),
            Some("880682E705")
        );
    }
}
