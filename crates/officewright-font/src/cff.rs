//! A check of CFF data that the subsetter cannot handle yet.
//!
//! subsetter 0.2.6 keeps the real-number operands of the DICTs it rewrites as `f32` and
//! writes them back from their decimal form. A real too large for an `f32` becomes
//! infinity, which it cannot write, and it panics with `unreachable!()` (in
//! `RealNumber::write`, `src/cff/number.rs`;
//! <https://github.com/typst/subsetter/issues/32>). `FontFace::subset` would turn that
//! panic into an error, but the panic hook still prints it, and the fuzz targets, whose
//! hook aborts, would report it on every run. Such a face is damaged, so it is refused
//! here, before the subsetter runs. Remove this once a subsetter release returns an
//! error.
//!
//! To find every DICT the subsetter rewrites, the table is read the way subsetter 0.2.6
//! reads it (`Table::parse` in `src/cff/mod.rs`, `src/cff/index.rs` and `src/cff/dict/`)
//! rather than with another parser, which could reject operands it accepts: the first
//! DICT of the Top DICT INDEX, the Private DICT it points to, and each Font DICT of the
//! FDArray with its Private DICT. That is a little more than the subsetter writes back:
//! it rewrites only FontMatrix and FontBBox of the Top DICT, the top-level Private DICT
//! only of a SID-keyed font and the FDArray only of a CID-keyed one. A real number past
//! `f32::MAX` has no place in any of these, so finding one anywhere refuses the face.

use std::ops::Range;

use skrifa::raw::FontRef;
use skrifa::raw::types::Tag;

use crate::error::{Cause, Error, Result};

/// The most characters a real number may have; the subsetter rejects longer ones.
const MAX_REAL_LEN: usize = 64;

/// The most operands of an operator the subsetter keeps.
const MAX_OPERANDS: usize = 48;

/// The Private operator: the size and offset of a Private DICT.
const PRIVATE: u16 = 18;

/// The FDArray operator (12 36): the offset of the Font DICT INDEX.
const FD_ARRAY: u16 = 12 << 8 | 36;

/// Fails if a DICT the subsetter rewrites has a real number that is finite in the font
/// but too large for an `f32`. Structures that cannot be read are left to the subsetter,
/// which reports them as errors.
pub(crate) fn check_reals(font: &FontRef<'_>, face: u32) -> Result<()> {
    let Some(data) = font.table_data(Tag::new(b"CFF ")) else {
        return Ok(());
    };
    for dict in dicts(data.as_bytes()) {
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

/// The DICTs of a `CFF ` table that the subsetter reads and rewrites.
fn dicts(cff: &[u8]) -> Vec<&[u8]> {
    let mut dicts = Vec::new();
    // The header's third byte is its size, where the Name INDEX starts; the Top DICT
    // INDEX follows it, and the subsetter reads only its first DICT.
    let top = cff
        .get(2)
        .and_then(|&header_size| skip_index(cff, usize::from(header_size)))
        .and_then(|after_names| index(cff, after_names))
        .and_then(|top_dicts| top_dicts.get(0));
    let Some(top) = top else {
        return dicts;
    };
    dicts.push(top);
    for (operator, operands) in operators(top) {
        match operator {
            PRIVATE => dicts.extend(range(&operands).and_then(|range| cff.get(range))),
            FD_ARRAY => {
                let Some(font_dicts) = offset(&operands).and_then(|at| index(cff, at)) else {
                    continue;
                };
                for font_dict in (0..font_dicts.len()).filter_map(|i| font_dicts.get(i)) {
                    dicts.push(font_dict);
                    for (operator, operands) in operators(font_dict) {
                        if operator == PRIVATE {
                            dicts.extend(range(&operands).and_then(|range| cff.get(range)));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    dicts
}

/// An INDEX (CFF specification, 5) as the subsetter reads it.
struct Index<'a> {
    /// The offsets, each `offset_size` bytes, counting from 1.
    offsets: &'a [u8],
    offset_size: usize,
    /// The data after the offsets, as long as the last offset says.
    data: &'a [u8],
}

impl<'a> Index<'a> {
    /// Reads the head of the INDEX at `at`: the offsets, and where they end.
    fn head(cff: &'a [u8], at: usize) -> Option<(Self, usize)> {
        let count = cff.get(at..at.checked_add(2)?)?;
        let count = usize::from(u16::from_be_bytes([*count.first()?, *count.get(1)?]));
        if count == 0 {
            let empty = Self {
                offsets: &[],
                offset_size: 1,
                data: &[],
            };
            return Some((empty, at + 2));
        }
        let offset_size = usize::from(*cff.get(at + 2)?);
        if !(1..=4).contains(&offset_size) {
            return None;
        }
        let end = (count + 1).checked_mul(offset_size)?.checked_add(at + 3)?;
        let offsets = cff.get(at + 3..end)?;
        let head = Self {
            offsets,
            offset_size,
            data: &[],
        };
        Some((head, end))
    }

    /// The number of objects.
    fn len(&self) -> usize {
        (self.offsets.len() / self.offset_size).saturating_sub(1)
    }

    /// Offset `i` from the start of the data.
    fn offset(&self, i: usize) -> Option<usize> {
        let at = i.checked_mul(self.offset_size)?;
        let bytes = self.offsets.get(at..at.checked_add(self.offset_size)?)?;
        let value = bytes
            .iter()
            .fold(0usize, |value, &byte| value << 8 | usize::from(byte));
        value.checked_sub(1)
    }

    /// The data of object `i`.
    fn get(&self, i: usize) -> Option<&'a [u8]> {
        self.data
            .get(self.offset(i)?..self.offset(i.checked_add(1)?)?)
    }
}

/// The INDEX at `at`.
fn index(cff: &[u8], at: usize) -> Option<Index<'_>> {
    let (mut index, end) = Index::head(cff, at)?;
    if let Some(len) = index.len().checked_add(1).and_then(|n| index.offset(n - 1)) {
        index.data = cff.get(end..end.checked_add(len)?)?;
    }
    Some(index)
}

/// Where the INDEX at `at` ends, as the subsetter skips one: if its data would run past
/// the table, only its offsets are skipped.
fn skip_index(cff: &[u8], at: usize) -> Option<usize> {
    let (index, end) = Index::head(cff, at)?;
    let data_end = index
        .len()
        .checked_add(1)
        .and_then(|n| index.offset(n - 1))
        .and_then(|len| end.checked_add(len))
        .filter(|&data_end| data_end <= cff.len());
    Some(data_end.unwrap_or(end))
}

/// An operand as the subsetter reads it.
#[derive(Clone, Debug, PartialEq)]
enum Operand {
    Integer(i32),
    /// A real number in its decimal form.
    Real(String),
}

impl Operand {
    /// The operand as an offset or size, as the subsetter's `as_u32` takes it: an integer,
    /// or a real without a fraction.
    fn as_usize(&self) -> Option<usize> {
        let value = match self {
            Self::Integer(value) => *value,
            Self::Real(text) => {
                let value: f32 = text.parse().ok()?;
                if value.fract() != 0.0 {
                    return None;
                }
                // The subsetter converts the same way, saturating.
                value as i32
            }
        };
        usize::try_from(value).ok()
    }
}

/// The range of the Private operator's operands, size then offset.
fn range(operands: &[Operand]) -> Option<Range<usize>> {
    let [size, offset] = operands else {
        return None;
    };
    let start = offset.as_usize()?;
    Some(start..start.checked_add(size.as_usize()?)?)
}

/// The offset of an operator that takes one.
fn offset(operands: &[Operand]) -> Option<usize> {
    let [offset] = operands else {
        return None;
    };
    offset.as_usize()
}

/// The operators of DICT data with their operands, as the subsetter's `DictionaryParser`
/// reads them: reading stops at an operand it cannot read, an operator keeps at most 48
/// operands, and operands after the last operator are not written back, so they are
/// left out.
fn operators(dict: &[u8]) -> Vec<(u16, Vec<Operand>)> {
    let mut operators = Vec::new();
    let mut operands = Vec::new();
    let mut at = 0;
    while let Some(&b0) = dict.get(at) {
        at += 1;
        let operand = match b0 {
            12 => {
                let Some(&b1) = dict.get(at) else {
                    break;
                };
                at += 1;
                operators.push((12 << 8 | u16::from(b1), std::mem::take(&mut operands)));
                continue;
            }
            // Like the subsetter, the reserved bytes 22 to 27, 31 and 255 are operators.
            0..=27 | 31 | 255 => {
                operators.push((u16::from(b0), std::mem::take(&mut operands)));
                continue;
            }
            28 => {
                let Some(&[hi, lo]) = dict.get(at..at + 2) else {
                    break;
                };
                at += 2;
                Operand::Integer(i32::from(i16::from_be_bytes([hi, lo])))
            }
            29 => {
                let Some(&[b1, b2, b3, b4]) = dict.get(at..at + 4) else {
                    break;
                };
                at += 4;
                Operand::Integer(i32::from_be_bytes([b1, b2, b3, b4]))
            }
            30 => {
                let Some((Some(text), len)) = dict.get(at..).and_then(real) else {
                    break;
                };
                at += len;
                Operand::Real(text)
            }
            32..=246 => Operand::Integer(i32::from(b0) - 139),
            247..=254 => {
                let Some(&b1) = dict.get(at) else {
                    break;
                };
                at += 1;
                let b1 = i32::from(b1);
                if b0 <= 250 {
                    Operand::Integer((i32::from(b0) - 247) * 256 + b1 + 108)
                } else {
                    Operand::Integer(-(i32::from(b0) - 251) * 256 - b1 - 108)
                }
            }
        };
        if operands.len() < MAX_OPERANDS {
            operands.push(operand);
        }
    }
    operators
}

/// The first real-number operand the subsetter would write back from DICT data that is
/// finite in the font but infinite as an `f32`, in its decimal form.
fn overflowing_real(dict: &[u8]) -> Option<String> {
    operators(dict)
        .into_iter()
        .flat_map(|(_, operands)| operands)
        .find_map(|operand| match operand {
            Operand::Real(text) => {
                let value: Option<f32> = text.parse().ok();
                value.is_some_and(f32::is_infinite).then_some(text)
            }
            Operand::Integer(_) => None,
        })
}

/// Reads the nibbles of a real number (CFF specification, Table 5) after its prefix
/// byte, as the subsetter's `RealNumber::parse` does. Returns its decimal form, `None`
/// if the subsetter would reject it, and the number of bytes it takes; `None` overall if
/// the data ends before the number does.
fn real(data: &[u8]) -> Option<(Option<String>, usize)> {
    let mut text = String::new();
    let mut valid = true;
    for (i, &byte) in data.iter().enumerate() {
        for nibble in [byte >> 4, byte & 0xF] {
            if nibble == 0xF {
                return Some((valid.then_some(text), i + 1));
            }
            // As `parse_float_nibble`: no character past the 64th, and E- needs two.
            let room = MAX_REAL_LEN.saturating_sub(text.len());
            match nibble {
                _ if room == 0 || (nibble == 0xC && room < 2) => valid = false,
                0..=9 => text.push(char::from(b'0' + nibble)),
                0xA => text.push('.'),
                0xB => text.push('E'),
                0xC => text.push_str("E-"),
                0xE => text.push('-'),
                // 0xD is reserved.
                _ => valid = false,
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_numbers_as_the_subsetter_does() {
        // 0.039625, -2.25, 1.5E-3, from the CFF specification's encoding.
        assert_eq!(
            real(&[0x0A, 0x03, 0x96, 0x25, 0xFF]),
            Some((Some("0.039625".into()), 5))
        );
        assert_eq!(real(&[0xE2, 0xA2, 0x5F]), Some((Some("-2.25".into()), 3)));
        assert_eq!(real(&[0x1A, 0x5C, 0x3F]), Some((Some("1.5E-3".into()), 3)));
        // A reserved nibble, and no end.
        assert_eq!(real(&[0x1D, 0xFF]), Some((None, 2)));
        assert_eq!(real(&[0x12, 0x34]), None);
        // 64 characters are accepted and 65 are not, even when the end is in the same
        // byte as the 65th; E- does not fit in the 64th place.
        let ones = |bytes: usize, last: u8| [vec![0x11; bytes], vec![last]].concat();
        let text = |data: &[u8]| real(data).and_then(|(text, _)| text);
        assert_eq!(text(&ones(32, 0xFF)).map(|text| text.len()), Some(64));
        assert_eq!(text(&ones(32, 0x1F)), None);
        assert_eq!(text(&[ones(31, 0x1C), vec![0xFF]].concat()), None);
        assert_eq!(
            text(&[ones(31, 0xC1), vec![0xFF]].concat()).map(|text| text.len()),
            None
        );
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
        // Cut off by the end of the data, or after the last operator: not written back.
        assert_eq!(overflowing_real(&[30, 0x1B, 0x99]), None);
        assert_eq!(
            overflowing_real(&[139, 1, 30, 0x1B, 0x99, 0x99, 0x9F]),
            None
        );
        // After an operand the subsetter cannot read, it reads no further.
        assert_eq!(
            overflowing_real(&[30, 0x1D, 0xFF, 1, 30, 0x1B, 0x99, 0x99, 0x9F, 1]),
            None
        );
        // Reserved bytes are operators, and reading goes on after them.
        let after_reserved = [38, 31, 22, 255, 30, 0x88, 0x06, 0x82, 0xB7, 0x05, 0xFB, 24];
        assert_eq!(
            overflowing_real(&after_reserved).as_deref(),
            Some("880682E705")
        );
    }

    #[test]
    fn finds_offsets_written_as_integers_or_whole_reals() {
        // 36 190208 Private, the offset once as an integer and once as a real.
        let integer = operators(&[0xAF, 0x1D, 0x00, 0x02, 0xE7, 0x00, 0x12]);
        let real = operators(&[0xAF, 0x1E, 0x19, 0x02, 0x08, 0xFF, 0x12]);
        for operators in [integer, real] {
            assert_eq!(operators.len(), 1, "{operators:?}");
            assert_eq!(operators[0].0, PRIVATE);
            assert_eq!(range(&operators[0].1), Some(190_208..190_244));
        }
        // A fraction, a negative number or a third operand is not a range.
        assert_eq!(
            range(&operators(&[0xAF, 0x1E, 0x1A, 0x5F, 0x12])[0].1),
            None
        );
        assert_eq!(range(&operators(&[0xAF, 0x8A, 0x12])[0].1), None);
        assert_eq!(range(&operators(&[0xAF, 0xAF, 0xAF, 0x12])[0].1), None);
    }

    #[test]
    fn reads_the_first_top_dict_and_the_dicts_it_points_to() {
        // A CFF table: a header of 4 bytes, a Name INDEX with one name, and a Top DICT
        // INDEX with "5 30 Private" pointing at a Private DICT of 5 bytes at offset 30,
        // then a second Top DICT, which the subsetter does not read.
        let mut cff = vec![1, 0, 4, 1];
        cff.extend([0, 1, 1, 1, 2, b'A']);
        let top = [144, 30 + 139, 18];
        cff.extend([0, 2, 1, 1, 4, 5]);
        cff.extend(top);
        cff.push(99);
        cff.resize(30, 0);
        let private = [30, 0x1B, 0x99, 0x99, 0x9F, 1];
        cff.extend(private);
        assert_eq!(dicts(&cff), [&top[..], &private[..5]]);
    }

    #[test]
    fn reads_the_font_dicts_of_a_cid_keyed_font() {
        // The Top DICT says "20 FDArray"; the Font DICT INDEX there has one DICT,
        // "1 30 Private", whose Private DICT is the byte 139 at offset 30.
        let mut cff = vec![1, 0, 4, 1];
        cff.extend([0, 0]); // no names
        let top = [20 + 139, 12, 36];
        cff.extend([0, 1, 1, 1, 4]);
        cff.extend(top);
        cff.resize(20, 0);
        let font_dict = [140, 30 + 139, 18];
        cff.extend([0, 1, 1, 1, 4]);
        cff.extend(font_dict);
        cff.resize(30, 0);
        cff.push(139);
        assert_eq!(dicts(&cff), [&top[..], &font_dict[..], &[139][..]]);
    }
}
