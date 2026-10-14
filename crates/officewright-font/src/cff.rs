//! A check of CFF data that the subsetter cannot handle yet.
//!
//! subsetter 0.2.6 keeps the real-number operands of the DICTs it rewrites as `f32` and
//! writes them back from their decimal form. A real too large for an `f32` becomes
//! infinity, which it cannot write, and it panics with `unreachable!()` (in
//! `RealNumber::write`, `src/cff/number.rs`;
//! <https://github.com/typst/subsetter/issues/31>). `FontFace::subset` would turn that
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

use crate::error::{Cause, Error, Result};
use crate::subset::subsetter_table;

/// The most characters a real number may have; the subsetter rejects longer ones.
const MAX_REAL_LEN: usize = 64;

/// The most operands of an operator the subsetter keeps.
const MAX_OPERANDS: usize = 48;

/// The Private operator: the size and offset of a Private DICT.
const PRIVATE: u16 = 18;

/// The FDArray operator (12 36): the offset of the Font DICT INDEX.
const FD_ARRAY: u16 = 12 << 8 | 36;

/// The ROS operator (12 30), which makes a font CID-keyed.
const ROS: u16 = 12 << 8 | 30;

/// The FontMatrix operator (12 7).
const FONT_MATRIX: u16 = 12 << 8 | 7;

/// Fails if a DICT the subsetter rewrites, in the `CFF ` table it takes from `font` (its
/// file `data`), has a real number that is finite in the font but too large for an
/// `f32`. Structures that cannot be read are left to the subsetter, which reports them as
/// errors.
pub(crate) fn check_reals(font: &FontRef<'_>, data: &[u8], face: u32) -> Result<()> {
    let Some(cff) = subsetter_table(font, data, b"CFF ") else {
        return Ok(());
    };
    for dict in dicts(cff) {
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
    if let Some(real) = overflowing_font_matrix(cff) {
        return Err(Error::MalformedTable {
            face,
            table: "CFF ",
            source: Cause::new(format!(
                "the FontMatrix entry {real} of a Font DICT is too large to subset"
            )),
        });
    }
    Ok(())
}

/// The first entry of a Font DICT's FontMatrix that the subsetter scales past the `f32`
/// range, in its decimal form, looking at every Font DICT and every FontMatrix in one,
/// although the subsetter scales only the Font DICTs of the glyphs it keeps and the last
/// FontMatrix of each: an entry past `f32::MAX` / 1000 is damage wherever it is. When
/// the Top DICT of a CID-keyed font has no FontMatrix,
/// subsetter 0.2.6 writes one of 0.001 there and multiplies each Font DICT's by 1000
/// (`rewrite_font_dict_index` in `src/cff/dict/font_dict.rs`), in `f64` and then back to
/// `f32`; an entry past `f32::MAX` / 1000 becomes infinite and reaches the same
/// `unreachable!()` as a real that is infinite already
/// (<https://github.com/typst/subsetter/issues/31>). Only the first four entries are
/// scaled; integers stay far within the range.
fn overflowing_font_matrix(cff: &[u8]) -> Option<String> {
    let top = operators(top_dict(cff)?);
    let cid_keyed = top.iter().any(|(operator, _)| *operator == ROS);
    let own_matrix = top.iter().any(|(operator, _)| *operator == FONT_MATRIX);
    if !cid_keyed || own_matrix {
        return None;
    }
    font_dicts(cff, &top)
        .into_iter()
        .flat_map(operators)
        .filter(|(operator, operands)| *operator == FONT_MATRIX && operands.len() == 6)
        .flat_map(|(_, operands)| operands.into_iter().take(4))
        .find_map(|operand| match operand {
            Operand::Real(text) => {
                let value: f32 = text.parse().ok()?;
                ((f64::from(value) * 1000.0) as f32)
                    .is_infinite()
                    .then_some(text)
            }
            Operand::Integer(_) => None,
        })
}

/// The DICTs of a `CFF ` table that the subsetter reads and rewrites.
fn dicts(cff: &[u8]) -> Vec<&[u8]> {
    let mut dicts = Vec::new();
    let Some(top) = top_dict(cff) else {
        return dicts;
    };
    dicts.push(top);
    let top = operators(top);
    for (operator, operands) in &top {
        if *operator == PRIVATE {
            dicts.extend(range(operands).and_then(|range| cff.get(range)));
        }
    }
    for font_dict in font_dicts(cff, &top) {
        dicts.push(font_dict);
        for (operator, operands) in operators(font_dict) {
            if operator == PRIVATE {
                dicts.extend(range(&operands).and_then(|range| cff.get(range)));
            }
        }
    }
    dicts
}

/// The Top DICT the subsetter reads: the first of the Top DICT INDEX, which follows the
/// header (whose third byte is its size) and the Name INDEX.
fn top_dict(cff: &[u8]) -> Option<&[u8]> {
    cff.get(2)
        .and_then(|&header_size| skip_index(cff, usize::from(header_size)))
        .and_then(|after_names| index(cff, after_names))
        .and_then(|top_dicts| top_dicts.get(0))
}

/// The Font DICTs of the FDArray that the Top DICT's operators `top` point to.
fn font_dicts<'a>(cff: &'a [u8], top: &[(u16, Vec<Operand>)]) -> Vec<&'a [u8]> {
    top.iter()
        .filter(|(operator, _)| *operator == FD_ARRAY)
        .filter_map(|(_, operands)| offset(operands).and_then(|at| index(cff, at)))
        .flat_map(|font_dicts| {
            (0..font_dicts.len())
                .filter_map(|i| font_dicts.get(i))
                .collect::<Vec<_>>()
        })
        .collect()
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
/// if the subsetter would reject it (its nibbles, or a text such as "", "." or "1E" that
/// does not parse as an `f32`, which makes the subsetter stop reading the DICT), and the
/// number of bytes it takes; `None` overall if the data ends before the number does.
fn real(data: &[u8]) -> Option<(Option<String>, usize)> {
    let mut text = String::new();
    let mut valid = true;
    for (i, &byte) in data.iter().enumerate() {
        for nibble in [byte >> 4, byte & 0xF] {
            if nibble == 0xF {
                let readable = valid && text.parse::<f32>().is_ok();
                return Some((readable.then_some(text), i + 1));
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
    use crate::face::OutlineFormat;

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

    /// A CFF table of one glyph (`.notdef`, endchar) whose Top DICT has `top` before its
    /// offsets and `top_after` after them, with a Private DICT holding `private`, and, when `font_dict` is given, an
    /// FDArray of one Font DICT holding it and pointing at that Private DICT. Offsets are
    /// five-byte integers, so the layout does not depend on them.
    fn cff_table(
        top: &[u8],
        top_after: &[u8],
        private: &[u8],
        font_dict: Option<&[u8]>,
    ) -> Vec<u8> {
        fn index(items: &[&[u8]]) -> Vec<u8> {
            if items.is_empty() {
                return vec![0, 0];
            }
            let mut out = vec![0, items.len() as u8, 1, 1];
            let mut end = 1;
            for item in items {
                end += item.len() as u8;
                out.push(end);
            }
            for item in items {
                out.extend_from_slice(item);
            }
            out
        }
        let int5 = |value: usize| [&[29][..], &(value as i32).to_be_bytes()].concat();
        let top_dict = |charstrings: usize, fd_array: usize, fd_select: usize, at: usize| {
            let mut dict = top.to_vec();
            dict.extend(int5(charstrings));
            dict.push(17);
            if font_dict.is_some() {
                dict.extend(int5(fd_array));
                dict.extend([12, 36]);
                dict.extend(int5(fd_select));
                dict.extend([12, 37]);
            } else {
                dict.extend(int5(private.len()));
                dict.extend(int5(at));
                dict.push(18);
            }
            dict.extend_from_slice(top_after);
            dict
        };
        let font_dict_of = |at: usize| {
            let mut dict = font_dict.unwrap_or_default().to_vec();
            dict.extend(int5(private.len()));
            dict.extend(int5(at));
            dict.push(18);
            dict
        };
        let head = [1, 0, 4, 1];
        let names = index(&[b"A"]);
        let strings = index(&[b"Adobe", b"Identity"]);
        let charstrings_data = index(&[&[14]]);
        let fd_select = [0, 0];
        let top_len = index(&[&top_dict(0, 0, 0, 0)]).len();
        let charstrings = head.len() + names.len() + top_len + strings.len() + 2;
        let fd_select_at = charstrings + charstrings_data.len();
        let fd_array = fd_select_at
            + if font_dict.is_some() {
                fd_select.len()
            } else {
                0
            };
        let font_dicts_len = if font_dict.is_some() {
            index(&[&font_dict_of(0)]).len()
        } else {
            0
        };
        let private_at = fd_array + font_dicts_len;
        let mut cff = head.to_vec();
        cff.extend(names);
        cff.extend(index(&[&top_dict(
            charstrings,
            fd_array,
            fd_select_at,
            private_at,
        )]));
        cff.extend(strings);
        cff.extend([0, 0]);
        cff.extend(charstrings_data);
        if font_dict.is_some() {
            cff.extend(fd_select);
            cff.extend(index(&[&font_dict_of(private_at)]));
        }
        cff.extend_from_slice(private);
        cff
    }

    /// ROS Adobe-Identity-0: strings 391 and 392 of the String INDEX, and 0.
    const ROS_ADOBE_IDENTITY: [u8; 7] = [248, 27, 248, 28, 139, 12, 30];

    /// FontMatrix [`first` 0 0 0.001 0 0], `first` a real number's bytes.
    fn font_matrix(first: &[u8]) -> Vec<u8> {
        let thousandth = [30, 0x0A, 0x00, 0x1F];
        [first, &[139, 139], &thousandth, &[139, 139, 12, 7]].concat()
    }

    /// The real numbers 1E36, 1E30 and 0.001.
    const E36: [u8; 4] = [30, 0x1B, 0x36, 0xFF];
    const E30: [u8; 4] = [30, 0x1B, 0x30, 0xFF];
    const THOUSANDTH: [u8; 4] = [30, 0x0A, 0x00, 0x1F];

    /// A Private DICT with defaultWidthX 0, and one with defaultWidthX 1E99999.
    const PRIVATE_ZERO: [u8; 2] = [139, 20];
    const PRIVATE_INFINITE: [u8; 6] = [30, 0x1B, 0x99, 0x99, 0x9F, 20];

    #[test]
    fn refuses_font_matrices_the_subsetter_scales_past_f32() {
        // A CID-keyed font without a FontMatrix in its Top DICT: the subsetter multiplies
        // the Font DICT's by 1000, and 1E36 becomes infinite.
        let cid = |matrix: &[u8]| cff_table(&ROS_ADOBE_IDENTITY, &[], &PRIVATE_ZERO, Some(matrix));
        let err = subset_with(cid(&font_matrix(&E36)), false).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "CFF ", .. }),
            "{err}"
        );
        assert_eq!(
            std::error::Error::source(&err).unwrap().to_string(),
            "the FontMatrix entry 1E36 of a Font DICT is too large to subset"
        );
        // Entries 1 to 3 are scaled too.
        for at in 1..4 {
            let mut entries = vec![vec![139]; 6];
            entries[3] = THOUSANDTH.to_vec();
            entries[at] = E36.to_vec();
            let matrix = [entries.concat(), vec![12, 7]].concat();
            let err = subset_with(cid(&matrix), false).unwrap_err();
            assert!(
                matches!(err, Error::MalformedTable { table: "CFF ", .. }),
                "{at}: {err}"
            );
        }
        // At the limit: 3.40282357E35 times 1000 still rounds to f32::MAX, 3.402824E35
        // does not.
        let below = [30, 0x3A, 0x40, 0x28, 0x23, 0x57, 0xB3, 0x5F];
        let above = [30, 0x3A, 0x40, 0x28, 0x24, 0xB3, 0x5F];
        subset_with(cid(&font_matrix(&below)), false).unwrap();
        let err = subset_with(cid(&font_matrix(&above)), false).unwrap_err();
        assert_eq!(
            std::error::Error::source(&err).unwrap().to_string(),
            "the FontMatrix entry 3.402824E35 of a Font DICT is too large to subset"
        );
        // A real the subsetter cannot read, "", "." or "1E", ends its reading of the Top
        // DICT, so a FontMatrix after it does not count, and the Font DICT's is scaled.
        for unreadable in [&[30, 0xFF][..], &[30, 0xAF], &[30, 0x1B, 0xFF]] {
            let after = [unreadable, &font_matrix(&THOUSANDTH)].concat();
            let table = cff_table(
                &ROS_ADOBE_IDENTITY,
                &after,
                &PRIVATE_ZERO,
                Some(&font_matrix(&E36)),
            );
            let err = subset_with(table, false).unwrap_err();
            assert!(
                matches!(err, Error::MalformedTable { table: "CFF ", .. }),
                "{err}"
            );
        }
        // 1E30 times 1000 fits; so do the usual 0.001.
        subset_with(cid(&font_matrix(&E30)), false).unwrap();
        subset_with(cid(&font_matrix(&THOUSANDTH)), false).unwrap();
        // With a FontMatrix of its own in the Top DICT, the Font DICT's is written as it
        // is, and 1E36 fits in an f32.
        let top = [&ROS_ADOBE_IDENTITY[..], &font_matrix(&THOUSANDTH)].concat();
        let own = cff_table(&top, &[], &PRIVATE_ZERO, Some(&font_matrix(&E36)));
        subset_with(own, false).unwrap();
        // The translation is not scaled: 1E36 as its first entry fits.
        let thousandth = [30, 0x0A, 0x00, 0x1F];
        let translated = [
            &thousandth[..],
            &[139, 139],
            &thousandth,
            &E36,
            &[139, 12, 7],
        ]
        .concat();
        subset_with(cid(&translated), false).unwrap();
        // A font that is not CID-keyed: the subsetter does not read the FDArray.
        let sid = cff_table(&[], &[], &PRIVATE_ZERO, Some(&font_matrix(&E36)));
        subset_with(sid, false).unwrap();
    }

    #[test]
    fn checks_the_cff_table_only_where_the_subsetter_reads_it() {
        // A CFF table whose Private DICT has 1E99999 is refused as the outlines.
        let infinite = cff_table(&[], &[], &PRIVATE_INFINITE, None);
        let err = subset_with(infinite.clone(), false).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "CFF ", .. }),
            "{err}"
        );
        // Next to a `glyf` table, the subsetter takes TrueType outlines and never reads
        // it, so the face subsets.
        let subset = subset_with(infinite.clone(), true).unwrap();
        assert_eq!(subset.outline_format(), OutlineFormat::TrueType);
        // `loca` alone does not make the subsetter take TrueType outlines.
        let font = crate::testing::TestFont::new("Test Sans")
            .without_table(b"glyf")
            .with_table(b"CFF ", infinite)
            .build();
        let err = crate::font::FontFace::from_bytes(font, 0)
            .unwrap()
            .subset([])
            .unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "CFF ", .. }),
            "{err}"
        );
    }

    /// Subsets the test font with `cff` as its outlines, and `glyf` and `loca` kept or
    /// not.
    fn subset_with(cff: Vec<u8>, truetype: bool) -> Result<crate::subset::Subset> {
        let mut font = crate::testing::TestFont::new("Test Sans").with_table(b"CFF ", cff);
        if !truetype {
            font = font.without_table(b"glyf").without_table(b"loca");
        }
        crate::font::FontFace::from_bytes(font.build(), 0)
            .unwrap()
            .subset([])
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
