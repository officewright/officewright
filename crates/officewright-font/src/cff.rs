//! Checks of CFF data that the subsetter cannot handle yet.
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
//!
//! subsetter 0.2.6 also counts the stem hints of each glyph it keeps in a `u16`
//! (`count_hints` in `src/cff/charstring.rs`), as it decompiles the glyph's charstring
//! and the subroutines it calls. A glyph that declares more than 65,535 hints overflows
//! it: with overflow checks it panics, and without them the count wraps and the hint
//! masks after it are misread (<https://github.com/typst/subsetter/issues/33>). The Type 2
//! format allows 96 hints in a glyph, so such a glyph is damage, and [`check_hints`]
//! refuses it, walking the kept glyphs as the subsetter does.

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

/// The CharStrings operator: the offset of the CharStrings INDEX.
const CHAR_STRINGS: u16 = 17;

/// The FDSelect operator (12 37).
const FD_SELECT: u16 = 12 << 8 | 37;

/// The Subrs operator of a Private DICT: the offset of its local subroutines from the
/// start of the DICT.
const SUBRS: u16 = 19;

/// The most stem hints the subsetter can count in one glyph.
const MAX_HINTS: u32 = 0xFFFF;

/// How deep the subsetter follows subroutine calls.
const MAX_CALL_DEPTH: u8 = 64;

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
/// FontMatrix of each: an entry past `f32::MAX` / 1000 is damage wherever it is. When the
/// Top DICT of a CID-keyed font has no FontMatrix, subsetter 0.2.6 writes one of 0.001
/// there and multiplies each Font DICT's by 1000
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

/// The most Font DICTs subsetter 0.2.6 takes in a CID-keyed font: it asserts at most 255
/// lists of local subroutines, one fewer than the 256 that FDSelect's one-byte numbers
/// can address (<https://github.com/typst/subsetter/issues/34>).
const MAX_FONT_DICTS: usize = 255;

/// Fails if the subsetter would panic on the `CFF ` table it takes from `font` (its file
/// `data`) when it keeps `glyphs`: when the table is CID-keyed with more Font DICTs than
/// it takes, or when a glyph it decompiles declares more stem hints than it can count.
/// The subsetter decompiles `.notdef` and then the kept glyphs in order, and stops at the
/// first it cannot read; so does this check, leaving that error to the subsetter.
pub(crate) fn check_programs(
    font: &FontRef<'_>,
    data: &[u8],
    face: u32,
    glyphs: &[u16],
) -> Result<()> {
    let Some(cff) = subsetter_table(font, data, b"CFF ") else {
        return Ok(());
    };
    let Some(programs) = Programs::read(cff) else {
        return Ok(());
    };
    // A well-formed font, so the subsetter's limit, not damage.
    if programs.local.len() > MAX_FONT_DICTS {
        return Err(Error::SubsetUnsupported {
            face,
            source: Cause::new(format!(
                "the CFF table has {} Font DICTs, more than the subsetter takes \
                 ({MAX_FONT_DICTS})",
                programs.local.len()
            )),
        });
    }
    let mut kept = glyphs.to_vec();
    kept.sort_unstable();
    kept.dedup();
    for glyph in std::iter::once(0).chain(kept.into_iter().filter(|&glyph| glyph != 0)) {
        match programs.walk(glyph) {
            Ok(()) => {}
            Err(Stop::Overflow) => {
                return Err(Error::MalformedTable {
                    face,
                    table: "CFF ",
                    source: Cause::new(format!(
                        "glyph {glyph} declares more than 65535 stem hints, more than the \
                         subsetter can count"
                    )),
                });
            }
            Err(Stop::Unreadable) => return Ok(()),
        }
    }
    Ok(())
}

/// What the subsetter decompiles glyphs with, read as `Table::parse` in its
/// `src/cff/mod.rs` reads it.
struct Programs<'a> {
    charstrings: Index<'a>,
    global: Vec<&'a [u8]>,
    /// The local subroutines of a SID-keyed font, or of each Font DICT of a CID-keyed one.
    local: Vec<Vec<&'a [u8]>>,
    /// A CID-keyed font's FDSelect, from its format byte on.
    fd_select: Option<&'a [u8]>,
}

impl<'a> Programs<'a> {
    /// Reads them from `cff`, or `None` where the subsetter fails before it decompiles
    /// a glyph. Only what decides which programs are walked is read as the subsetter reads
    /// it; where a damaged table makes the subsetter fail on something else (another
    /// operator of a DICT, an FDSelect range past the glyph), this check may refuse the
    /// face first, with an error of its own: both are errors.
    fn read(cff: &'a [u8]) -> Option<Self> {
        if cff.first() != Some(&1) {
            return None;
        }
        let names = usize::from(*cff.get(2)?);
        let (top_dicts, after_top_dicts) = parsed_index(cff, skip_index(cff, names)?)?;
        let top = operators(top_dicts.get(0)?);
        // Of an operator given twice, the subsetter keeps the last.
        let last = |operator: u16| {
            top.iter()
                .rev()
                .find(|(found, _)| *found == operator)
                .map(|(_, operands)| operands.as_slice())
        };
        let (_, after_strings) = parsed_index(cff, after_top_dicts)?;
        let (global, _) = parsed_index(cff, after_strings)?;
        let (charstrings, _) = parsed_index(cff, offset(last(CHAR_STRINGS)?)?)?;
        let glyph_count = charstrings.len();
        if glyph_count == 0 || glyph_count > usize::from(u16::MAX) {
            return None;
        }
        let (local, fd_select) = if top.iter().any(|(operator, _)| *operator == ROS) {
            let (font_dicts, _) = parsed_index(cff, offset(last(FD_ARRAY)?)?)?;
            let fd_select = cff.get(offset(last(FD_SELECT)?)?..)?;
            match fd_select.first()? {
                // A font DICT number for each glyph.
                0 => {
                    fd_select.get(1..glyph_count.checked_add(1)?)?;
                }
                3 => {}
                _ => return None,
            }
            let mut local = Vec::new();
            for font_dict in objects(&font_dicts) {
                // The last Private operator of the Font DICT gives its subroutines.
                let mut subrs = Vec::new();
                for (operator, operands) in operators(font_dict) {
                    if operator == PRIVATE {
                        subrs = local_subrs(cff, range(&operands)?)?;
                    }
                }
                local.push(subrs);
            }
            (local, Some(fd_select))
        } else {
            // A SID-keyed font whose local subroutines cannot be read has none.
            let subrs = last(PRIVATE)
                .and_then(range)
                .and_then(|private| local_subrs(cff, private))
                .unwrap_or_default();
            (vec![subrs], None)
        };
        Some(Self {
            charstrings,
            global: objects(&global),
            local,
            fd_select,
        })
    }

    /// Walks the charstring of `glyph` as the subsetter decompiles it.
    fn walk(&self, glyph: u16) -> std::result::Result<(), Stop> {
        let font_dict = match self.fd_select {
            Some(fd_select) => font_dict(fd_select, glyph).ok_or(Stop::Unreadable)?,
            None => 0,
        };
        let local = self
            .local
            .get(usize::from(font_dict))
            .ok_or(Stop::Unreadable)?;
        let charstring = self
            .charstrings
            .get(usize::from(glyph))
            .ok_or(Stop::Unreadable)?;
        let mut walk = Walk {
            global: &self.global,
            local,
            stack: Vec::new(),
            hints: 0,
            mask_bytes: 0,
        };
        walk.run(charstring, 1).map(|_| ())
    }
}

/// The subroutines of the Private DICT at `private`, which the subsetter finds by the
/// first Subrs operator of the DICT: none without one, or when its operand is no offset
/// it can read (`parse_subr_offset` in its `src/cff/dict/private_dict.rs`); `None` where
/// it fails.
fn local_subrs(cff: &[u8], private: Range<usize>) -> Option<Vec<&[u8]>> {
    let start = private.start;
    let operators = operators(cff.get(private)?);
    let Some(offset) = operators
        .iter()
        .find(|(operator, _)| *operator == SUBRS)
        .and_then(|(_, operands)| offset(operands))
    else {
        return Some(Vec::new());
    };
    let (subrs, _) = parsed_index(cff, start.checked_add(offset)?)?;
    Some(objects(&subrs))
}

/// The Font DICT of `glyph` in an FDSelect, as the subsetter's `font_dict_index` finds it.
fn font_dict(fd_select: &[u8], glyph: u16) -> Option<u8> {
    let (&format, rest) = fd_select.split_first()?;
    if format == 0 {
        return rest.get(usize::from(glyph)).copied();
    }
    // Format 3: ranges of a first glyph and a Font DICT, then a sentinel glyph.
    let ranges = u16_at(rest, 0)?;
    if ranges == 0 {
        return None;
    }
    let mut first = u16_at(rest, 2)?;
    let mut dict = *rest.get(4)?;
    let mut at = 5;
    for _ in 0..ranges {
        let next = u16_at(rest, at)?;
        if (first..next).contains(&glyph) {
            return Some(dict);
        }
        dict = *rest.get(at + 2)?;
        first = next;
        at += 3;
    }
    None
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]))
}

/// The INDEX at `at` and where it ends, as the subsetter's `parse_index` reads one: one
/// whose last offset is 0 is empty, and its data must fit in the table.
fn parsed_index(cff: &[u8], at: usize) -> Option<(Index<'_>, usize)> {
    let (mut index, end) = Index::head(cff, at)?;
    let Some(len) = index.len().checked_add(1).and_then(|n| index.offset(n - 1)) else {
        let empty = Index {
            offsets: &[],
            offset_size: 1,
            data: &[],
        };
        return Some((empty, end));
    };
    let data_end = end.checked_add(len)?;
    index.data = cff.get(end..data_end)?;
    Some((index, data_end))
}

/// The objects of an INDEX up to the first that cannot be read, as the subsetter collects
/// subroutines.
fn objects<'a>(index: &Index<'a>) -> Vec<&'a [u8]> {
    (0..index.len()).map_while(|i| index.get(i)).collect()
}

/// The subroutine a call operand `index` names among `subrs`, after the bias for their
/// number (Type 2 Charstring Format, 4.7).
fn subroutine<'a>(subrs: &[&'a [u8]], index: i32) -> Option<&'a [u8]> {
    let bias = match subrs.len() {
        0..1240 => 107,
        1240..33900 => 1131,
        _ => 32768,
    };
    let index = usize::try_from(index.checked_add(bias)?).ok()?;
    subrs.get(index).copied()
}

/// Why a walk stopped before the end of a glyph.
enum Stop {
    /// The glyph declares more stem hints than the subsetter can count.
    Overflow,
    /// The subsetter fails on the glyph with an error of its own.
    Unreadable,
}

/// How a charstring or subroutine ended.
enum End {
    Return,
    EndChar,
}

/// A glyph's charstring and the subroutines it calls, walked as subsetter 0.2.6's
/// `Decompiler` (`src/cff/charstring.rs`) decompiles them, keeping only what it needs
/// to count stem hints: the operands, and the hints so far.
struct Walk<'a> {
    global: &'a [&'a [u8]],
    local: &'a [&'a [u8]],
    /// Each operand as an `i32`, `None` for one with a fraction.
    stack: Vec<Option<i32>>,
    hints: u32,
    /// The length of a hint mask, set at the first.
    mask_bytes: usize,
}

impl Walk<'_> {
    fn run(&mut self, charstring: &[u8], depth: u8) -> std::result::Result<End, Stop> {
        if depth > MAX_CALL_DEPTH {
            return Err(Stop::Unreadable);
        }
        let mut at = 0;
        while let Some(&b0) = charstring.get(at) {
            if matches!(b0, 28 | 32..=255) {
                let (operand, len) = charstring_number(charstring.get(at..).unwrap_or_default())
                    .ok_or(Stop::Unreadable)?;
                if self.stack.len() == MAX_OPERANDS {
                    return Err(Stop::Unreadable);
                }
                self.stack.push(operand);
                at += len;
                continue;
            }
            at += 1;
            let operator = if b0 == 12 {
                let b1 = *charstring.get(at).ok_or(Stop::Unreadable)?;
                at += 1;
                12 << 8 | u16::from(b1)
            } else {
                u16::from(b0)
            };
            match operator {
                // hstem, vstem, hstemhm and vstemhm.
                1 | 3 | 18 | 23 => self.count_hints()?,
                // Path operators, and hflex, flex, hflex1 and flex1.
                4..=8 | 21 | 22 | 24..=27 | 30 | 31 | 0x0C22..=0x0C25 => self.stack.clear(),
                // return
                11 => return Ok(End::Return),
                // callsubr and callgsubr
                10 | 29 => {
                    let index = self.stack.pop().flatten().ok_or(Stop::Unreadable)?;
                    let subrs = if operator == 29 {
                        self.global
                    } else {
                        self.local
                    };
                    let subr = subroutine(subrs, index).ok_or(Stop::Unreadable)?;
                    if let End::EndChar = self.run(subr, depth + 1)? {
                        return Ok(End::EndChar);
                    }
                }
                // hintmask and cntrmask: the first may declare vstem hints of its own.
                19 | 20 => {
                    if self.mask_bytes == 0 {
                        self.count_hints()?;
                        self.mask_bytes = usize::try_from(self.hints.div_ceil(8))
                            .map_err(|_| Stop::Unreadable)?;
                    }
                    at = at
                        .checked_add(self.mask_bytes)
                        .filter(|&end| end <= charstring.len())
                        .ok_or(Stop::Unreadable)?;
                }
                // endchar; with four operands it would be an accented character, which
                // the subsetter does not implement.
                14 if self.stack.len() >= 4 => return Err(Stop::Unreadable),
                14 => return Ok(End::EndChar),
                _ => return Err(Stop::Unreadable),
            }
        }
        Ok(End::Return)
    }

    /// Counts the operands of a stem operator, two for each hint, and takes them off.
    fn count_hints(&mut self) -> std::result::Result<(), Stop> {
        let operands = u32::try_from(self.stack.len()).map_err(|_| Stop::Unreadable)?;
        self.stack.clear();
        self.hints += operands / 2;
        if self.hints > MAX_HINTS {
            return Err(Stop::Overflow);
        }
        Ok(())
    }
}

/// A charstring number at the start of `data` as the subsetter reads it, as an `i32`
/// (`None` for a 16.16 fixed number with a fraction), and its length.
fn charstring_number(data: &[u8]) -> Option<(Option<i32>, usize)> {
    match *data.first()? {
        28 => Some((
            Some(i32::from(i16::from_be_bytes([
                *data.get(1)?,
                *data.get(2)?,
            ]))),
            3,
        )),
        b0 @ 32..=246 => Some((Some(i32::from(b0) - 139), 1)),
        b0 @ 247..=250 => Some((
            Some((i32::from(b0) - 247) * 256 + i32::from(*data.get(1)?) + 108),
            2,
        )),
        b0 @ 251..=254 => Some((
            Some(-(i32::from(b0) - 251) * 256 - i32::from(*data.get(1)?) - 108),
            2,
        )),
        255 => {
            let bytes: [u8; 4] = data.get(1..5)?.try_into().ok()?;
            // As the subsetter's `as_i32` converts it: in `f32`, a whole number only.
            let value = i32::from_be_bytes(bytes) as f32 / 65536.0;
            let whole = (value.fract() == 0.0).then_some(value as i32);
            Some((whole, 5))
        }
        _ => None,
    }
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
    /// offsets and `top_after` after them, with a Private DICT holding `private`, and,
    /// when `font_dict` is given, an FDArray of one Font DICT holding it and pointing at
    /// that Private DICT. Offsets are five-byte integers, so the layout does not depend on
    /// them.
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

    /// An INDEX of `items`, with the smallest offset size that holds them.
    fn index_of(items: &[Vec<u8>]) -> Vec<u8> {
        if items.is_empty() {
            return vec![0, 0];
        }
        let mut offsets = vec![1];
        for item in items {
            offsets.push(offsets.last().unwrap() + item.len());
        }
        let size = (1..=4)
            .find(|&size| *offsets.last().unwrap() < 1 << (8 * size))
            .unwrap();
        let mut out = u16::try_from(items.len()).unwrap().to_be_bytes().to_vec();
        out.push(size as u8);
        for offset in offsets {
            out.extend_from_slice(&offset.to_be_bytes()[8 - size..]);
        }
        for item in items {
            out.extend_from_slice(item);
        }
        out
    }

    /// A five-byte integer operand of a DICT.
    fn int5(value: usize) -> Vec<u8> {
        [&[29][..], &(value as i32).to_be_bytes()].concat()
    }

    /// How each Private DICT of a table gives its local subroutines.
    #[derive(Clone, Copy, Default)]
    enum Subrs {
        /// "6 Subrs": the subroutines right after the DICT.
        #[default]
        One,
        /// "12 Subrs 1000000 Subrs": the subsetter takes the first.
        Twice,
        /// "0 0 Subrs": two operands, no offset the subsetter can read, so it takes the
        /// DICT to have no subroutines.
        Unreadable,
    }

    /// A CFF table to build. With one entry in `local`, it is SID-keyed and that entry
    /// holds its local subroutines; with an `fd_select`, it is CID-keyed and `local` holds
    /// those of each Font DICT. Offsets are five-byte integers, so the layout does not
    /// depend on them.
    #[derive(Clone, Default)]
    struct Spec {
        glyphs: Vec<Vec<u8>>,
        /// The Global Subrs INDEX as written; empty for none.
        global: Vec<u8>,
        local: Vec<Vec<Vec<u8>>>,
        fd_select: Option<Vec<u8>>,
        subrs: Subrs,
        /// Written first in the Top DICT, before the operators the table needs, which
        /// come later and so are the ones the subsetter keeps.
        top_prefix: Vec<u8>,
        /// Each Font DICT gives a Private operator pointing at Font DICT 0's Private DICT
        /// before the one pointing at its own.
        two_private: bool,
        /// An empty String INDEX.
        no_strings: bool,
    }

    impl Spec {
        fn build(&self) -> Vec<u8> {
            let cid = self.fd_select.is_some();
            let private_dict: Vec<u8> = match self.subrs {
                Subrs::One => [int5(6), vec![19]].concat(),
                Subrs::Twice => [int5(12), vec![19], int5(1_000_000), vec![19]].concat(),
                Subrs::Unreadable => vec![139, 139, 19],
            };
            let private_len = private_dict.len();
            let privates: Vec<Vec<u8>> = self
                .local
                .iter()
                .map(|subrs| [private_dict.clone(), index_of(subrs)].concat())
                .collect();
            let top_dict =
                |charstrings: usize, fd_array: usize, fd_select_at: usize, private_at: usize| {
                    let mut dict = self.top_prefix.clone();
                    if cid {
                        dict.extend(ROS_ADOBE_IDENTITY);
                    }
                    dict.extend(int5(charstrings));
                    dict.push(17);
                    if cid {
                        dict.extend(int5(fd_array));
                        dict.extend([12, 36]);
                        dict.extend(int5(fd_select_at));
                        dict.extend([12, 37]);
                    } else {
                        dict.extend(int5(private_len));
                        dict.extend(int5(private_at));
                        dict.push(18);
                    }
                    dict
                };
            let font_dict = |first_at: usize, private_at: usize| {
                let mut dict = Vec::new();
                if self.two_private {
                    dict.extend([int5(private_len), int5(first_at), vec![18]].concat());
                }
                dict.extend([int5(private_len), int5(private_at), vec![18]].concat());
                dict
            };
            let head = vec![1, 0, 4, 1];
            let names = index_of(&[b"A".to_vec()]);
            let top_len = index_of(&[top_dict(0, 0, 0, 0)]).len();
            let strings = if self.no_strings {
                index_of(&[])
            } else {
                index_of(&[b"Adobe".to_vec(), b"Identity".to_vec()])
            };
            let global = if self.global.is_empty() {
                index_of(&[])
            } else {
                self.global.clone()
            };
            let charstrings = index_of(&self.glyphs);
            let charstrings_at = head.len() + names.len() + top_len + strings.len() + global.len();
            let fd_select_at = charstrings_at + charstrings.len();
            let fd_select = self.fd_select.clone().unwrap_or_default();
            let fd_array_at = fd_select_at + fd_select.len();
            let fd_array_len = if cid {
                index_of(&vec![font_dict(0, 0); self.local.len()]).len()
            } else {
                0
            };
            let mut private_at = Vec::new();
            let mut at = fd_array_at + fd_array_len;
            for private in &privates {
                private_at.push(at);
                at += private.len();
            }
            let mut cff = head;
            cff.extend(names);
            cff.extend(index_of(&[top_dict(
                charstrings_at,
                fd_array_at,
                fd_select_at,
                private_at[0],
            )]));
            cff.extend(strings);
            cff.extend(global);
            cff.extend(charstrings);
            if cid {
                cff.extend_from_slice(&fd_select);
                let dicts: Vec<Vec<u8>> = private_at
                    .iter()
                    .map(|&at| font_dict(private_at[0], at))
                    .collect();
                cff.extend(index_of(&dicts));
            }
            for private in privates {
                cff.extend(private);
            }
            cff
        }
    }

    /// A CFF table with the charstrings `glyphs` and the global subroutines `global`. With
    /// one entry in `local`, it is SID-keyed and that entry holds its local subroutines;
    /// with an `fd_select`, it is CID-keyed and `local` holds those of each Font DICT.
    /// Each Private DICT is "6 Subrs", its subroutines right after it.
    fn programs(
        glyphs: &[Vec<u8>],
        global: &[Vec<u8>],
        local: &[Vec<Vec<u8>>],
        fd_select: Option<&[u8]>,
    ) -> Vec<u8> {
        Spec {
            glyphs: glyphs.to_vec(),
            global: index_of(global),
            local: local.to_vec(),
            fd_select: fd_select.map(<[u8]>::to_vec),
            ..Spec::default()
        }
        .build()
    }

    /// Subroutines that declare stem hints: 0 is 24 hints; 1 calls 0 60 times (1,440);
    /// 2 calls 1 45 times (64,800). With fewer than 1,240 subroutines the bias is 107, so
    /// subroutine `i` is called with the operand `i` - 107, the byte `i` + 32.
    fn hint_subrs(call: u8) -> Vec<Vec<u8>> {
        stem_subrs(call, 1)
    }

    /// The same, declaring the hints with the stem operator `stem`: hstem (1), vstem (3),
    /// hstemhm (18) or vstemhm (23).
    fn stem_subrs(call: u8, stem: u8) -> Vec<Vec<u8>> {
        let mut first = vec![139; 48];
        first.extend([stem, 11]);
        let calls = |subr: u8, times: usize| {
            let mut subr = [subr + 32, call].repeat(times);
            subr.push(11);
            subr
        };
        vec![first, calls(0, 60), calls(1, 45)]
    }

    /// A charstring that declares `hints` stem hints, at least 65,520: subroutine 2, then 0
    /// 30 times (65,520 so far), then the rest with one hstem, or with the first hintmask
    /// and its mask when `mask` is set; then endchar. `call` is callsubr (10) or
    /// callgsubr (29).
    fn hinted_glyph(hints: u32, call: u8, mask: bool) -> Vec<u8> {
        [hinted(hints, call, if mask { 19 } else { 1 }), vec![14]].concat()
    }

    /// The program of [`hinted_glyph`] without its endchar, the last hints declared by
    /// `last`: hstem (1), or the first hintmask (19) or cntrmask (20) with its mask.
    fn hinted(hints: u32, call: u8, last: u8) -> Vec<u8> {
        let rest = usize::try_from(hints - 65_520).unwrap();
        let mut program = vec![34, call];
        program.extend([32, call].repeat(30));
        program.extend(vec![139; 2 * rest]);
        program.push(last);
        if last != 1 {
            program.extend(vec![0xFF; usize::try_from(hints.div_ceil(8)).unwrap()]);
        }
        program
    }

    /// 1,241 global subroutines, the hint subroutines first, of which the subsetter can
    /// read only the first 1,239: the end of object 1,239 comes before its start. With
    /// 1,239 the bias is 107; with 1,240 or more it would be 1,131.
    fn broken_global() -> Vec<u8> {
        let mut subrs = hint_subrs(29);
        subrs.resize(1241, vec![11]);
        let mut index = index_of(&subrs);
        let size = usize::from(index[2]);
        let at = |i: usize| 3 + i * size;
        let start = index[at(1239)..at(1240)]
            .iter()
            .fold(0usize, |value, &byte| value << 8 | usize::from(byte));
        let end = (start - 1).to_be_bytes();
        index[at(1240)..at(1241)].copy_from_slice(&end[end.len() - size..]);
        index
    }

    /// Two operands for each path operator, the four flex operators (12 34 to 12 37)
    /// among them, which the subsetter takes off the stack without counting hints.
    fn drawn() -> Vec<u8> {
        let mut path = Vec::new();
        for operator in [21, 22, 4, 5, 6, 7, 8, 24, 25, 26, 27, 30, 31] {
            path.extend([139, 139, operator]);
        }
        for flex in 34..=37 {
            path.extend([139, 139, 12, flex]);
        }
        path
    }

    /// The five glyphs of the test font: `glyph` as glyph `at`, the others endchar.
    fn glyphs_with(at: usize, glyph: Vec<u8>) -> Vec<Vec<u8>> {
        let mut glyphs = vec![vec![14]; crate::testing::ADVANCES.len()];
        glyphs[at] = glyph;
        glyphs
    }

    /// Whether subsetter 0.2.6 itself panics on the test font with `cff` as its outlines,
    /// kept to `glyphs`.
    fn subsetter_panics(cff: Vec<u8>, glyphs: &[u16]) -> bool {
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        let remapper = subsetter::GlyphRemapper::new_from_glyphs_sorted(glyphs);
        std::panic::catch_unwind(|| subsetter::subset(&font, 0, &remapper)).is_err()
    }

    #[test]
    fn refuses_more_font_dicts_than_the_subsetter_takes() {
        // Glyphs 0 to 2 in Font DICT 0, 3 and 4 in Font DICT 1, of 255 or 256.
        let format3 = [3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        for count in [255, 256] {
            let cff = programs(
                &glyphs_with(4, vec![14]),
                &[],
                &vec![vec![]; count],
                Some(&format3),
            );
            let refused = subset_glyphs(cff.clone(), &[4]).err();
            if count > MAX_FONT_DICTS {
                let Some(Error::SubsetUnsupported { source, .. }) = refused else {
                    panic!("{count} Font DICTs: {refused:?}");
                };
                assert!(source.to_string().contains("256 Font DICTs"), "{source}");
            } else {
                assert!(refused.is_none(), "{count} Font DICTs: {refused:?}");
            }
            // The subsetter's assertion holds only with debug assertions.
            if cfg!(debug_assertions) {
                assert_eq!(subsetter_panics(cff, &[4]), count > 255, "{count}");
            }
        }
    }

    /// The test font with `cff` as its outlines, subset to `glyphs`.
    fn subset_glyphs(cff: Vec<u8>, glyphs: &[u16]) -> Result<crate::subset::Subset> {
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        crate::font::FontFace::from_bytes(font, 0)
            .unwrap()
            .subset(glyphs.iter().map(|&glyph| crate::font::GlyphId::new(glyph)))
    }

    /// The message of a refusal by [`check_hints`].
    fn refusal(result: Result<crate::subset::Subset>) -> String {
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "CFF ", .. }),
            "{err}"
        );
        std::error::Error::source(&err).unwrap().to_string()
    }

    #[test]
    fn refuses_a_glyph_with_more_stem_hints_than_the_subsetter_counts() {
        for mask in [false, true] {
            let font = |hints| {
                programs(
                    &glyphs_with(1, hinted_glyph(hints, 29, mask)),
                    &hint_subrs(29),
                    &[vec![]],
                    None,
                )
            };
            // 65,535 hints fit in the subsetter's count; one more does not.
            subset_glyphs(font(65_535), &[1]).unwrap();
            assert_eq!(
                refusal(subset_glyphs(font(65_536), &[1])),
                "glyph 1 declares more than 65535 stem hints, more than the subsetter can count",
                "mask {mask}"
            );
            // Only the glyphs the subset keeps are walked, and .notdef always is.
            subset_glyphs(font(65_536), &[2, 3]).unwrap();
            let notdef = programs(
                &glyphs_with(0, hinted_glyph(65_536, 29, mask)),
                &hint_subrs(29),
                &[vec![]],
                None,
            );
            assert!(refusal(subset_glyphs(notdef, &[2])).starts_with("glyph 0 declares"));
            // After drawing with every path operator.
            let drawn = |hints| {
                let glyph = [drawn(), hinted_glyph(hints, 29, mask)].concat();
                programs(&glyphs_with(1, glyph), &hint_subrs(29), &[vec![]], None)
            };
            subset_glyphs(drawn(65_535), &[1]).unwrap();
            assert!(refusal(subset_glyphs(drawn(65_536), &[1])).starts_with("glyph 1 declares"));
        }
    }

    #[test]
    fn ends_a_glyph_at_endchar_in_a_subroutine() {
        // Global subroutine 3 is endchar, which ends the glyph that calls it: the hints
        // after the call are never decompiled.
        let mut subrs = hint_subrs(29);
        subrs.push(vec![14]);
        let glyph = [vec![35, 29], hinted_glyph(65_536, 29, false)].concat();
        let font = programs(&glyphs_with(1, glyph), &subrs, &[vec![]], None);
        subset_glyphs(font, &[1]).unwrap();
    }

    #[test]
    fn counts_the_stem_hints_of_local_subroutines() {
        // A SID-keyed font, its Private DICT's subroutines.
        let font = |hints| {
            programs(
                &glyphs_with(3, hinted_glyph(hints, 10, false)),
                &[],
                &[hint_subrs(10)],
                None,
            )
        };
        subset_glyphs(font(65_535), &[3]).unwrap();
        assert!(refusal(subset_glyphs(font(65_536), &[3])).starts_with("glyph 3 declares"));
        // A CID-keyed font: glyphs 0 to 2 use Font DICT 0, which has no subroutines, and
        // 3 and 4 use Font DICT 1, whose subroutines declare the hints; FDSelect format 3
        // (two ranges and the sentinel), and format 0 (a Font DICT for each glyph).
        let format3 = [3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        let format0 = [0, 0, 0, 0, 1, 1];
        for fd_select in [&format3[..], &format0[..]] {
            let font = |hints| {
                programs(
                    &glyphs_with(4, hinted_glyph(hints, 10, false)),
                    &[],
                    &[vec![], hint_subrs(10)],
                    Some(fd_select),
                )
            };
            subset_glyphs(font(65_535), &[4]).unwrap();
            assert!(refusal(subset_glyphs(font(65_536), &[4])).starts_with("glyph 4 declares"));
            subset_glyphs(font(65_536), &[1, 2, 3]).unwrap();
        }
    }

    #[test]
    fn leaves_glyphs_the_subsetter_cannot_read_to_the_subsetter() {
        // An operator the subsetter does not know (12 0) before the hints, and endchar with
        // the four operands of an accented character: it fails on the glyph before it
        // counts the hints, with an error of its own.
        for before in [vec![12, 0], vec![139, 139, 139, 139, 14]] {
            let glyph = [before, hinted_glyph(65_536, 29, false)].concat();
            let font = programs(&glyphs_with(1, glyph), &hint_subrs(29), &[vec![]], None);
            let err = subset_glyphs(font, &[1]).unwrap_err();
            assert!(!matches!(err, Error::MalformedTable { .. }), "{err}");
        }
        // A glyph before it that the subsetter cannot read, the last one a hintmask without
        // its mask: it stops there.
        for before in [
            vec![12, 0],
            vec![139, 139, 139, 139, 14],
            vec![139, 139, 19],
        ] {
            let mut glyphs = glyphs_with(2, hinted_glyph(65_536, 29, false));
            glyphs[1] = before;
            let font = programs(&glyphs, &hint_subrs(29), &[vec![]], None);
            let err = subset_glyphs(font, &[1, 2]).unwrap_err();
            assert!(!matches!(err, Error::MalformedTable { .. }), "{err}");
        }
    }

    #[test]
    fn agrees_with_the_subsetter_on_when_its_hint_count_overflows() {
        // The subsetter itself panics exactly where the check refuses, on fonts around the
        // limit built every way above. It panics only with overflow checks, which come
        // with debug assertions, as in `cargo test`; without them its count wraps.
        let mut cases: Vec<(String, Vec<u8>, u16, bool)> = Vec::new();
        for hints in [65_534, 65_535, 65_536, 65_537, 65_543] {
            for mask in [false, true] {
                for (cff, glyph) in [
                    (
                        programs(
                            &glyphs_with(1, hinted_glyph(hints, 29, mask)),
                            &hint_subrs(29),
                            &[vec![]],
                            None,
                        ),
                        1,
                    ),
                    (
                        programs(
                            &glyphs_with(3, hinted_glyph(hints, 10, mask)),
                            &[],
                            &[hint_subrs(10)],
                            None,
                        ),
                        3,
                    ),
                    (
                        programs(
                            &glyphs_with(0, hinted_glyph(hints, 29, mask)),
                            &hint_subrs(29),
                            &[vec![]],
                            None,
                        ),
                        0,
                    ),
                    (
                        programs(
                            &glyphs_with(2, [drawn(), hinted_glyph(hints, 29, mask)].concat()),
                            &hint_subrs(29),
                            &[vec![]],
                            None,
                        ),
                        2,
                    ),
                ] {
                    cases.push((
                        format!("{hints} hints, mask {mask}"),
                        cff,
                        glyph,
                        hints > 65_535,
                    ));
                }
            }
        }
        let format3 = vec![3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        for hints in [65_535, 65_536] {
            let over = hints > 65_535;
            // The other stem operators, and cntrmask.
            for (stem, last) in [(3, 1), (18, 1), (23, 1), (1, 20)] {
                let glyph = [hinted(hints, 29, last), vec![14]].concat();
                cases.push((
                    format!("{hints} hints, stem {stem}, last {last}"),
                    programs(
                        &glyphs_with(1, glyph),
                        &stem_subrs(29, stem),
                        &[vec![]],
                        None,
                    ),
                    1,
                    over,
                ));
            }
            // A second hintmask after more stems: only the first counts its operands.
            let mut glyph = vec![139, 139, 1, 19, 0xFF];
            glyph.extend(hinted(hints - 1, 29, 1));
            glyph.extend([139, 139, 19, 0xFF, 14]);
            cases.push((
                format!("{hints} hints, second mask"),
                programs(&glyphs_with(1, glyph), &hint_subrs(29), &[vec![]], None),
                1,
                over,
            ));
            // A second hintmask keeps the length of the first, one byte for one hint,
            // and the hints after it overflow or not.
            let mut glyph = vec![139, 139, 1, 19, 0xFF];
            glyph.extend(hinted(65_529, 29, 1));
            glyph.extend([19, 0xFF]);
            glyph.extend(vec![139; 2 * usize::try_from(hints - 65_530).unwrap()]);
            glyph.extend([1, 14]);
            cases.push((
                format!("{hints} hints, after a second mask"),
                programs(&glyphs_with(1, glyph), &hint_subrs(29), &[vec![]], None),
                1,
                over,
            ));
            // A first hintmask at no hints has a mask of no bytes, so the next one is a
            // first mask too and counts its implicit hints.
            let glyph = [vec![19], hinted(hints, 29, 19), vec![14]].concat();
            cases.push((
                format!("{hints} hints, after a mask of no hints"),
                programs(&glyphs_with(1, glyph), &hint_subrs(29), &[vec![]], None),
                1,
                over,
            ));
            // CID-keyed: Font DICT 1's own subroutines; or none, its Subrs operand being
            // no offset, with the hints in the global ones; or its last Private operator.
            cases.push((
                format!("{hints} hints, CID"),
                programs(
                    &glyphs_with(4, hinted_glyph(hints, 10, false)),
                    &[],
                    &[vec![], hint_subrs(10)],
                    Some(&format3),
                ),
                4,
                over,
            ));
            cases.push((
                format!("{hints} hints, CID, unreadable Subrs"),
                Spec {
                    glyphs: glyphs_with(4, hinted_glyph(hints, 29, false)),
                    global: index_of(&hint_subrs(29)),
                    local: vec![vec![], vec![]],
                    fd_select: Some(format3.clone()),
                    subrs: Subrs::Unreadable,
                    ..Spec::default()
                }
                .build(),
                4,
                over,
            ));
            cases.push((
                format!("{hints} hints, CID, two Private"),
                Spec {
                    glyphs: glyphs_with(4, hinted_glyph(hints, 10, false)),
                    local: vec![vec![vec![11]; 3], hint_subrs(10)],
                    fd_select: Some(format3.clone()),
                    two_private: true,
                    ..Spec::default()
                }
                .build(),
                4,
                over,
            ));
            // The first of two Subrs operators; the last of two CharStrings operators,
            // the first pointing at the header; an empty String INDEX before the global
            // subroutines; a global subroutine the subsetter cannot read.
            cases.push((
                format!("{hints} hints, two Subrs"),
                Spec {
                    glyphs: glyphs_with(3, hinted_glyph(hints, 10, false)),
                    local: vec![hint_subrs(10)],
                    subrs: Subrs::Twice,
                    ..Spec::default()
                }
                .build(),
                3,
                over,
            ));
            let global = Spec {
                glyphs: glyphs_with(1, hinted_glyph(hints, 29, false)),
                global: index_of(&hint_subrs(29)),
                local: vec![vec![]],
                ..Spec::default()
            };
            cases.push((
                format!("{hints} hints, two CharStrings"),
                Spec {
                    top_prefix: [int5(0), vec![17]].concat(),
                    ..global.clone()
                }
                .build(),
                1,
                over,
            ));
            cases.push((
                format!("{hints} hints, no strings"),
                Spec {
                    no_strings: true,
                    ..global.clone()
                }
                .build(),
                1,
                over,
            ));
            cases.push((
                format!("{hints} hints, broken global subroutine"),
                Spec {
                    global: broken_global(),
                    ..global
                }
                .build(),
                1,
                over,
            ));
        }
        // Hints declared in a subroutine 60 or 61 calls deep: the deepest call is at depth
        // 64, which the subsetter follows, or 65, where it stops with an error of its own.
        for (chain, refused) in [(60, true), (61, false)] {
            let mut global = hint_subrs(29);
            for link in 0..chain - 1 {
                global.push(vec![32 + 3 + link + 1, 29, 11]);
            }
            global.push([hinted(65_536, 29, 1), vec![11]].concat());
            cases.push((
                format!("{chain} calls deep"),
                programs(
                    &glyphs_with(1, vec![32 + 3, 29, 14]),
                    &global,
                    &[vec![]],
                    None,
                ),
                1,
                refused,
            ));
        }
        assert_eq!(cases.len(), 40 + 2 * 14 + 2);
        for (name, cff, glyph, expected) in cases {
            let refused = matches!(
                subset_glyphs(cff.clone(), &[glyph]),
                Err(Error::MalformedTable { .. })
            );
            assert_eq!(refused, expected, "{name}");
            if cfg!(debug_assertions) {
                assert_eq!(refused, subsetter_panics(cff, &[glyph]), "{name}");
            }
        }
        // The kept glyphs are walked in the subsetter's order, sorted: glyph 1 overflows
        // before glyph 2, which the subsetter cannot read, is reached, also when the
        // subset names them 2, 1.
        let mut glyphs = glyphs_with(1, hinted_glyph(65_536, 29, false));
        glyphs[2] = vec![12, 0];
        let cff = programs(&glyphs, &hint_subrs(29), &[vec![]], None);
        assert!(matches!(
            subset_glyphs(cff.clone(), &[2, 1]),
            Err(Error::MalformedTable { .. })
        ));
        if cfg!(debug_assertions) {
            assert!(subsetter_panics(cff, &[2, 1]));
        }
        // `.notdef`, which every subset keeps, is walked first: it overflows before
        // glyph 1, which the subsetter cannot read, is reached.
        let mut glyphs = glyphs_with(0, hinted_glyph(65_536, 29, false));
        glyphs[1] = vec![12, 0];
        let cff = programs(&glyphs, &hint_subrs(29), &[vec![]], None);
        assert!(matches!(
            subset_glyphs(cff.clone(), &[1]),
            Err(Error::MalformedTable { .. })
        ));
        if cfg!(debug_assertions) {
            assert!(subsetter_panics(cff, &[1]));
        }
    }

    #[test]
    fn reads_charstring_numbers_and_subroutine_biases_as_the_subsetter_does() {
        assert_eq!(charstring_number(&[28, 0xFF, 0x9C]), Some((Some(-100), 3)));
        assert_eq!(charstring_number(&[139]), Some((Some(0), 1)));
        assert_eq!(charstring_number(&[247, 0]), Some((Some(108), 2)));
        assert_eq!(charstring_number(&[254, 0xFF]), Some((Some(-1131), 2)));
        // 16.16 fixed: 2.0 is whole, 1.5 is not.
        assert_eq!(charstring_number(&[255, 0, 2, 0, 0]), Some((Some(2), 5)));
        assert_eq!(charstring_number(&[255, 0, 1, 0x80, 0]), Some((None, 5)));
        assert_eq!(charstring_number(&[28, 1]), None);
        // The bias grows at 1,240 and 33,900 subroutines.
        let subrs = |len: usize| vec![&[11u8][..]; len];
        assert!(subroutine(&subrs(1239), -107).is_some());
        assert!(subroutine(&subrs(1239), -108).is_none());
        assert!(subroutine(&subrs(1240), -1131).is_some());
        assert!(subroutine(&subrs(1240), 109).is_none());
        assert!(subroutine(&subrs(33_900), -32768).is_some());
        assert!(subroutine(&subrs(33_900), i32::MAX).is_none());
        // FDSelect format 3: glyphs 0 to 2 in Font DICT 0, 3 and 4 in 1, 5 beyond the
        // sentinel; no ranges finds nothing.
        let format3 = [3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        let found: Vec<_> = (0..6).map(|glyph| font_dict(&format3, glyph)).collect();
        assert_eq!(found, [Some(0), Some(0), Some(0), Some(1), Some(1), None]);
        assert_eq!(font_dict(&[3, 0, 0, 0, 0, 0], 0), None);
        assert_eq!(font_dict(&[0, 7, 8], 1), Some(8));
        assert_eq!(font_dict(&[0, 7, 8], 2), None);
    }
}
