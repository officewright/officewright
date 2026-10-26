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
//! The DICTs the subsetter writes back come from one reading of the table with subsetter
//! 0.2.6's rules (`Table::parse` in `src/cff/mod.rs`, `src/cff/index.rs` and
//! `src/cff/dict/`), the one the walk of the glyphs below uses, rather than from another
//! parser, which could reject operands it accepts: the first DICT of the Top DICT INDEX,
//! and the Private DICT its last Private operator points to for a SID-keyed font, or for
//! a CID-keyed one the Font DICTs of the FDArray its last FDArray operator points to, up to
//! the first that cannot be read, each with the Private DICT its last Private operator
//! points to. That is more than the subsetter writes back: of the Top DICT only FontMatrix
//! and FontBBox, and of a CID-keyed font only the Font DICTs the kept glyphs use. A real
//! number past `f32::MAX` has no place in any of these, so finding one anywhere refuses
//! the face. Where the subsetter fails to parse the table, it writes nothing back, and
//! nothing is refused here.
//!
//! The subsetter reads the Private DICT of every Private operator of a Font DICT, to find
//! its subroutines, and so does this check. Font DICTs may repeat the operator and share
//! one large Private DICT, which would cost their number times its size to read here and
//! in the subsetter (<https://github.com/officewright/officewright/issues/271>); so the
//! sizes are added up first, up to the first Private operator the subsetter cannot take,
//! where it stops, and the face is refused past [`MAX_PRIVATE_BYTES`], a limit of the
//! library.
//!
//! A CID-keyed font's FDSelect of format 3 gives the Font DICT of each glyph by ranges of
//! glyphs. The subsetter finds the Font DICT of each glyph it keeps by going through the
//! ranges in order, twice, and the walk below does so once, so a font of one range per
//! glyph costs time quadratic in the glyphs kept
//! (<https://github.com/officewright/officewright/issues/278>); the face is refused past
//! [`MAX_FD_SELECT_RANGES`], a limit of the library, after the Font DICTs are counted.
//!
//! Before it decompiles a glyph, the subsetter also copies the local subroutines of every
//! Font DICT into a list of its own, also where Font DICTs share one Subrs INDEX, so a
//! small table could make it allocate hundreds of megabytes
//! (<https://github.com/officewright/officewright/issues/284>). The check counts them as
//! the subsetter collects them and refuses the face past [`MAX_LOCAL_SUBRS`], a limit of
//! the library, before it walks a glyph.
//!
//! subsetter 0.2.6 also counts the stem hints of each glyph it keeps in a `u16`
//! (`count_hints` in `src/cff/charstring.rs`), as it decompiles the glyph's charstring
//! and the subroutines it calls. A glyph that declares more than 65,535 hints overflows
//! it: with overflow checks it panics, and without them the count wraps and the hint
//! masks after it are misread (<https://github.com/typst/subsetter/issues/33>). The Type 2
//! format allows 96 hints in a glyph, so such a glyph is damage, and [`check`]
//! refuses it, walking the kept glyphs as the subsetter does.
//!
//! The same check refuses a CID-keyed font with more Font DICTs than subsetter 0.2.6
//! takes ([`MAX_FONT_DICTS`]), on which it panics.
//!
//! The subsetter decompiles every subroutine each time a glyph calls it, and keeps the
//! whole program of every kept glyph in memory, about 30 bytes for each operand and
//! operator it reads. Subroutines that call each other many times make that exponential
//! in the depth of the calls (<https://github.com/officewright/officewright/issues/235>),
//! within the format's limits on the length of a charstring and the depth of calls. So the
//! same walk counts the operands and operators it reads, and the bytes of each hint mask,
//! which the subsetter copies, counting a hint mask operator twice, as the subsetter keeps
//! it as two instructions, and refuses the face once the kept glyphs together take more
//! than [`MAX_STEPS`]: a limit of the library, since such a font is within the format.

use std::cell::OnceCell;
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

/// The FontName operator (12 38) of a Font DICT.
const FONT_NAME: u16 = 12 << 8 | 38;

/// The number of standard strings (CFF specification, Appendix A); a string ID past them
/// names a string of the String INDEX.
const STANDARD_STRINGS: u16 = 391;

/// The FDSelect operator (12 37).
const FD_SELECT: u16 = 12 << 8 | 37;

/// The Subrs operator of a Private DICT: the offset of its local subroutines from the
/// start of the DICT.
const SUBRS: u16 = 19;

/// The most stem hints the subsetter can count in one glyph.
const MAX_HINTS: u32 = 0xFFFF;

/// How deep the subsetter follows subroutine calls.
const MAX_CALL_DEPTH: u8 = 64;

/// The most operands, operators and hint mask bytes the walk may read in the charstrings
/// of the glyphs one subset keeps, reading a subroutine again for every call, and counting
/// a hint mask operator twice, since the subsetter keeps it as two instructions.
///
/// Measured on real fonts kept whole: Noto Sans (3,884 glyphs) reads 271,888, New
/// Computer Modern Math (7,041) 930,772, Noto Sans CJK SC (65,535) 16,082,053, on which
/// the subsetter peaks at 550 MB, its Black weight 18,085,581, and Noto Serif CJK SC
/// 31,768,357, on which the subsetter peaks at 890 MB; the most for one glyph was 3,056.
/// A document reaches the limit only with about half the glyphs of the largest CJK fonts
/// or more, but such a font kept whole, as one that forbids subsetting is, can be refused.
/// A limit that admitted it would let a crafted font cost the subsetter well over 1 GiB:
/// it spends about 25 to 30 bytes for each unit counted.
pub(crate) const MAX_STEPS: u32 = 1 << 24;

/// How much decompiling the glyphs one subset keeps may read: [`MAX_STEPS`], or less
/// where the budget the subset is made within has less left.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limit {
    steps: u32,
    /// Whether the budget, not [`MAX_STEPS`], sets `steps`.
    budget: bool,
}

impl Limit {
    /// The limit of a subset made with `left` units left of its budget.
    pub(crate) fn within(left: u64) -> Self {
        match u32::try_from(left) {
            Ok(steps) if steps < MAX_STEPS => Self {
                steps,
                budget: true,
            },
            _ => Self {
                steps: MAX_STEPS,
                budget: false,
            },
        }
    }
}

/// Fails if a DICT the subsetter writes back (`programs`' `written`) has a real number
/// that is finite in the font but too large for an `f32`, or a Font DICT's FontMatrix
/// that the subsetter scales past it.
fn check_reals(programs: &Programs<'_>, face: u32) -> Result<()> {
    for dict in &programs.written {
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
    if let Some(real) = overflowing_font_matrix(&programs.scaled) {
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

/// The first entry of the FontMatrix of one of `font_dicts` that the subsetter scales past
/// the `f32` range, in its decimal form, looking at every FontMatrix in a Font DICT,
/// although the subsetter scales only the Font DICTs of the glyphs it keeps and the last
/// FontMatrix of each: an entry past `f32::MAX` / 1000 is damage wherever it is. When the
/// Top DICT of a CID-keyed font has no FontMatrix, subsetter 0.2.6 writes one of 0.001
/// there and multiplies each Font DICT's by 1000
/// (`rewrite_font_dict_index` in `src/cff/dict/font_dict.rs`), in `f64` and then back to
/// `f32`; an entry past `f32::MAX` / 1000 becomes infinite and reaches the same
/// `unreachable!()` as a real that is infinite already
/// (<https://github.com/typst/subsetter/issues/31>). Only the first four entries are
/// scaled; integers stay far within the range.
fn overflowing_font_matrix(font_dicts: &[&[u8]]) -> Option<String> {
    font_dicts
        .iter()
        .flat_map(|font_dict| operators(font_dict))
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
#[cfg(test)]
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
    /// The operand as the subsetter's `as_i32` takes it: an integer, or a real without a
    /// fraction.
    fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Integer(value) => Some(*value),
            Self::Real(text) => {
                let value: f32 = text.parse().ok()?;
                if value.fract() != 0.0 {
                    return None;
                }
                // The subsetter converts the same way, saturating.
                Some(value as i32)
            }
        }
    }

    /// The operand as an offset or size, as the subsetter's `as_u32` takes it.
    fn as_usize(&self) -> Option<usize> {
        usize::try_from(self.as_i32()?).ok()
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

/// The most Font DICTs subsetter 0.2.6 takes in a CID-keyed font, one fewer than the 256
/// that FDSelect's one-byte numbers can address. With debug assertions it asserts at most
/// 255 lists of local subroutines before it decompiles a glyph
/// (<https://github.com/typst/subsetter/issues/34>). Without them it panics too when the
/// kept glyphs use 256 Font DICTs: it numbers the Font DICTs it keeps in a `u8`, with
/// `expect("remapper was overflowed")` (`Remapper::remap` in its `src/remapper.rs`). A
/// release that only allows 256 in the assertion would still panic there.
const MAX_FONT_DICTS: usize = 255;

/// The most bytes of Private DICTs that the Private operators of a CID-keyed font's Font
/// DICTs may point at in all, counted with repeats. Real fonts come to a few hundred:
/// Noto Sans CJK SC and Noto Serif CJK SC have 18 Font DICTs and 526 to 605 bytes.
const MAX_PRIVATE_BYTES: usize = 1 << 20;

/// The most local subroutines the subsetter may collect for the Font DICTs of a face, counted
/// once for each Font DICT, also where several share one Subrs INDEX. subsetter 0.2.6
/// collects the subroutines of every Font DICT into a list of its own, 16 bytes for each,
/// before it decompiles a glyph (`subset_charstrings` in its `src/cff/mod.rs`), so 255 Font
/// DICTs that share an INDEX of 65,535 would take 267 MB of a table of under 100 KB
/// (<https://github.com/officewright/officewright/issues/284>); this is 4 MiB. Real fonts
/// come to tens of thousands: Noto Sans CJK SC 45,206 over 18 Font DICTs, its Black weight
/// 47,857, Noto Serif CJK SC 54,004.
const MAX_LOCAL_SUBRS: usize = 1 << 18;

/// The most ranges an FDSelect of format 3 may declare. subsetter 0.2.6 finds the Font
/// DICT of each glyph it keeps by going through the ranges in order, twice, and the walk
/// does it once, so a font kept whole costs up to 65,535 times this. Real fonts declare a
/// few hundred: Noto Sans CJK SC 118, Noto Serif CJK SC 221.
const MAX_FD_SELECT_RANGES: usize = 2048;

/// Fails if the subsetter would panic on the `CFF ` table it takes from `font` (its file
/// `data`) when it keeps `kept`, or would spend more on it than the library allows:
/// when the table is CID-keyed with more Font DICTs than it takes, Private DICTs past
/// [`MAX_PRIVATE_BYTES`] or an FDSelect of more than [`MAX_FD_SELECT_RANGES`] ranges;
/// when a DICT it writes back has a real number too large for it; when its Font DICTs have
/// more than [`MAX_LOCAL_SUBRS`] local subroutines in all; when a glyph it decompiles
/// declares more stem hints than it can count; or if decompiling the glyphs takes more
/// than `limit`. Where the subsetter fails on the table before it writes or
/// decompiles anything, the table is left to it. The subsetter decompiles the glyphs in
/// the order it numbers them, `kept` (`.notdef` first, then each glyph once), and stops
/// at the first it cannot read; so does this check, leaving that error to the subsetter.
///
/// Returns how many operands, operators and hint mask bytes decompiling the glyphs reads:
/// 0 without a `CFF ` table the subsetter takes.
pub(crate) fn check(
    font: &FontRef<'_>,
    data: &[u8],
    face: u32,
    kept: &[u16],
    limit: Limit,
) -> Result<u32> {
    let Some(cff) = subsetter_table(font, data, b"CFF ") else {
        return Ok(0);
    };
    let programs = match Programs::read(cff) {
        Some(Read::Programs(programs)) => programs,
        // The subsetter's limit, refused before any glyph is walked: the subsetter
        // asserts before it decompiles one. A table damaged elsewhere too is refused here
        // as well, where the subsetter would fail on the damage instead.
        Some(Read::FontDicts(count)) => {
            return Err(Error::SubsetUnsupported {
                face,
                source: Cause::new(format!(
                    "the CFF table has {count} Font DICTs, more than the subsetter takes \
                     ({MAX_FONT_DICTS})"
                )),
            });
        }
        Some(Read::FdSelectRanges(ranges)) => {
            return Err(Error::TableOverLimit {
                face,
                table: "CFF ",
                source: Cause::new(format!(
                    "the FDSelect declares {ranges} ranges, more than the \
                     {MAX_FD_SELECT_RANGES} read before subsetting"
                )),
            });
        }
        Some(Read::PrivateBytes(bytes)) => {
            return Err(Error::TableOverLimit {
                face,
                table: "CFF ",
                source: Cause::new(format!(
                    "the Font DICTs point at {bytes} bytes of Private DICTs, more than the \
                     {MAX_PRIVATE_BYTES} read before subsetting"
                )),
            });
        }
        None => return Ok(0),
    };
    check_reals(&programs, face)?;
    let subrs = programs.collected_local_subrs();
    if subrs > MAX_LOCAL_SUBRS {
        return Err(Error::TableOverLimit {
            face,
            table: "CFF ",
            source: Cause::new(format!(
                "the Font DICTs have {subrs} local subroutines in all, more than the \
                 {MAX_LOCAL_SUBRS} the subsetter may collect"
            )),
        });
    }
    let mut steps = limit.steps;
    for &glyph in kept {
        match programs.walk(glyph, &mut steps) {
            Ok(()) => {}
            Err(Stop::Limit) => {
                let limit = match limit {
                    Limit {
                        steps,
                        budget: true,
                    } => format!(
                        "the {steps} operands, operators and hint mask bytes left of its \
                         budget"
                    ),
                    Limit { steps, .. } => {
                        format!("{steps} operands, operators and hint mask bytes")
                    }
                };
                return Err(Error::TableOverLimit {
                    face,
                    table: "CFF ",
                    source: Cause::new(format!(
                        "decompiling the glyphs to keep, up to glyph {glyph}, reads more \
                         than {limit}"
                    )),
                });
            }
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
            Err(Stop::Unreadable | Stop::Unsupported) => break,
        }
    }
    Ok(limit.steps.saturating_sub(steps))
}

/// The most operands, operators and hint mask bytes that the checks of one face's glyphs
/// ([`check_glyph`]) may read in all, those of glyphs that fail included: four times what
/// one subset may read.
pub(crate) const MAX_CHECK_WORK: u32 = 4 * MAX_STEPS;

/// What checking a face's CFF glyphs one at a time needs of its `CFF ` table, read once:
/// where its INDEXes are in the table, the font names of its Font DICTs, and the numbers
/// of subroutines checks have counted, since counting them takes as long as there are
/// subroutines.
#[derive(Debug)]
pub(crate) struct CffGlyphs {
    charstrings: Place,
    strings: Place,
    global: Subrs,
    /// The local subroutines of a SID-keyed font, or of each Font DICT of a CID-keyed one.
    local: Vec<Subrs>,
    /// Where a CID-keyed font's FDSelect starts in the table.
    fd_select: Option<usize>,
    /// The string ID of the font name each Font DICT of a CID-keyed font gives, if any.
    font_names: Vec<Option<u16>>,
}

/// Where an INDEX is in the table: its offsets and its data.
#[derive(Debug)]
struct Place {
    offsets: Range<usize>,
    offset_size: usize,
    data: Range<usize>,
}

/// The place of an INDEX of subroutines, and how many the subsetter collects, once counted.
#[derive(Debug)]
struct Subrs {
    place: Place,
    count: Option<usize>,
}

impl Place {
    /// Where `index`, read from `cff`, is in it.
    fn of(cff: &[u8], index: &Index<'_>) -> Self {
        Self {
            offsets: within(cff, index.offsets),
            offset_size: index.offset_size,
            data: within(cff, index.data),
        }
    }

    fn index<'a>(&self, cff: &'a [u8]) -> Index<'a> {
        Index {
            offsets: cff.get(self.offsets.clone()).unwrap_or_default(),
            offset_size: self.offset_size,
            data: cff.get(self.data.clone()).unwrap_or_default(),
        }
    }
}

/// Where `part`, a slice of `table` or an empty one, is in `table`.
fn within(table: &[u8], part: &[u8]) -> Range<usize> {
    let start = part.as_ptr().addr().wrapping_sub(table.as_ptr().addr());
    match start.checked_add(part.len()) {
        Some(end) if !part.is_empty() && end <= table.len() => start..end,
        _ => 0..0,
    }
}

impl CffGlyphs {
    /// Reads them from the `CFF ` table the subsetter takes from `font` (its file `data`),
    /// `None` without one or where the subsetter would not take it.
    pub(crate) fn read(font: &FontRef<'_>, data: &[u8]) -> Option<(Range<usize>, Self)> {
        let range = crate::subset::subsetter_table_range(font, data, b"CFF ")?;
        let cff = data.get(range.clone())?;
        let Read::Programs(programs) = Programs::read(cff)? else {
            return None;
        };
        let subrs = |subrs: &Subroutines<'_>| Subrs {
            place: Place::of(cff, &subrs.index),
            count: None,
        };
        let glyphs = Self {
            charstrings: Place::of(cff, &programs.charstrings),
            strings: Place::of(cff, &programs.strings),
            global: subrs(&programs.global),
            local: programs.local.iter().map(subrs).collect(),
            fd_select: programs
                .fd_select
                .map(|fd_select| within(cff, fd_select).start),
            font_names: programs.font_names,
        };
        Some((range, glyphs))
    }
}

/// Fails if the subsetter, which takes `cff`, the `CFF ` table of face `face`, when it
/// keeps `.notdef` alone, would fail on it when it also keeps `glyph`: where it cannot
/// decompile the glyph, the glyph declares more stem hints than it can count, or, in a
/// CID-keyed font, the glyph's Font DICT names its font by a string the String INDEX does
/// not have (`get_sid_remapper` in the subsetter's `src/cff/mod.rs`). Only those are left
/// to check: everything else the subsetter reads of the table it reads for `.notdef`
/// alone, or for every glyph it keeps alike. `glyphs` is what [`CffGlyphs::read`] read of
/// the table.
///
/// Decompiling the glyph may read no more than `steps`, nor than is left of `work`,
/// which every glyph checked takes what it read off. Returns what it read, which a glyph
/// that passes every check takes off the steps of the next.
pub(crate) fn check_glyph(
    cff: &[u8],
    glyphs: &mut CffGlyphs,
    face: u32,
    glyph: u16,
    steps: u32,
    work: &mut u32,
) -> Result<u32> {
    let damaged = |reason: &str| Error::Subset {
        face,
        source: Cause::new(format!("glyph {glyph}: {reason}")),
    };
    let font_dict = match glyphs.fd_select {
        Some(at) => font_dict(cff.get(at..).unwrap_or_default(), glyph),
        None => Some(0),
    };
    // The work left bounds the walk where it is less than the steps left.
    let by_work = *work < steps;
    let limit = steps.min(*work);
    let mut left = limit;
    let walked = match font_dict.and_then(|font_dict| glyphs.local.get_mut(usize::from(font_dict)))
    {
        Some(local) => {
            let global = glyphs.global.subroutines(cff);
            let local_subrs = local.subroutines(cff);
            let charstrings = glyphs.charstrings.index(cff);
            let walked = walk_glyph(&charstrings, &global, &local_subrs, glyph, &mut left);
            glyphs.global.count = global.count.get().copied();
            local.count = local_subrs.count.get().copied();
            walked
        }
        None => Err(Stop::Unreadable),
    };
    let read = limit - left;
    *work -= read;
    match walked {
        Ok(()) => {}
        Err(Stop::Limit) if by_work => {
            return Err(Error::TableOverLimit {
                face,
                table: "CFF ",
                source: Cause::new(format!(
                    "checking glyph {glyph} reads more than the {limit} operands, operators \
                     and hint mask bytes left of the {MAX_CHECK_WORK} the checks of one face \
                     may read"
                )),
            });
        }
        Err(Stop::Limit) => {
            return Err(Error::TableOverLimit {
                face,
                table: "CFF ",
                source: Cause::new(format!(
                    "decompiling glyph {glyph} reads more than the {limit} operands, \
                     operators and hint mask bytes left of what one subset may read, after \
                     the glyphs checked before it"
                )),
            });
        }
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
        Err(Stop::Unsupported) => {
            return Err(Error::SubsetUnsupported {
                face,
                source: Cause::new(format!(
                    "glyph {glyph} composes an accented character with endchar, which the \
                     subsetter does not implement"
                )),
            });
        }
        Err(Stop::Unreadable) => {
            return Err(damaged(
                "its charstring cannot be decompiled as the subsetter decompiles it",
            ));
        }
    }
    let sid = font_dict
        .filter(|_| glyphs.fd_select.is_some())
        .and_then(|font_dict| *glyphs.font_names.get(usize::from(font_dict))?);
    if let Some(sid) = sid {
        let strings = glyphs.strings.index(cff);
        let string = sid
            .checked_sub(STANDARD_STRINGS)
            .map(|at| strings.get(usize::from(at)));
        if let Some(None) = string {
            return Err(damaged(&format!(
                "its Font DICT names its font by string {sid}, which the String INDEX does \
                 not have"
            )));
        }
    }
    Ok(read)
}

impl Subrs {
    /// The subroutines, with their number if it was counted.
    fn subroutines<'a>(&self, cff: &'a [u8]) -> Subroutines<'a> {
        let subrs = Subroutines::new(self.place.index(cff));
        if let Some(count) = self.count {
            let _ = subrs.count.set(count);
        }
        subrs
    }
}

/// What [`Programs::read`] found.
enum Read<'a> {
    Programs(Box<Programs<'a>>),
    /// A CID-keyed font with this many Font DICTs, more than [`MAX_FONT_DICTS`].
    FontDicts(usize),
    /// Font DICTs whose Private operators point at this many bytes of Private DICTs in
    /// all, more than [`MAX_PRIVATE_BYTES`].
    PrivateBytes(usize),
    /// An FDSelect of format 3 that declares this many ranges, more than
    /// [`MAX_FD_SELECT_RANGES`].
    FdSelectRanges(usize),
}

/// What the subsetter decompiles glyphs with, read as `Table::parse` in its
/// `src/cff/mod.rs` reads it.
struct Programs<'a> {
    charstrings: Index<'a>,
    /// The String INDEX.
    strings: Index<'a>,
    /// The string ID of the font name each Font DICT of a CID-keyed font gives, if any.
    font_names: Vec<Option<u16>>,
    global: Subroutines<'a>,
    /// The local subroutines of a SID-keyed font, or of each Font DICT of a CID-keyed one.
    local: Vec<Subroutines<'a>>,
    /// A CID-keyed font's FDSelect, from its format byte on.
    fd_select: Option<&'a [u8]>,
    /// The DICTs whose real numbers the subsetter may write back: the Top DICT, and the
    /// top-level Private DICT of a SID-keyed font whose subroutines it can read, or each
    /// Font DICT of a CID-keyed one with the Private DICT its last Private operator points
    /// to, also of Font DICTs no kept glyph uses.
    written: Vec<&'a [u8]>,
    /// The Font DICTs of a CID-keyed font whose Top DICT has no FontMatrix: the subsetter
    /// scales theirs by 1000.
    scaled: Vec<&'a [u8]>,
}

impl<'a> Programs<'a> {
    /// Reads them from `cff`, or `None` where the subsetter fails before it decompiles
    /// a glyph. Only what decides which programs are walked is read as the subsetter reads
    /// it; where a damaged table makes the subsetter fail on something else (another
    /// operator of a DICT, an FDSelect range past the glyph), this check may refuse the
    /// face first, with an error of its own: both are errors.
    ///
    /// The Font DICTs of a CID-keyed font are counted as the subsetter counts them, up to
    /// the first it cannot read, before their Private DICTs are read: every Font DICT may
    /// point at one Private DICT, so reading them all would cost their number times its
    /// size, for a font the subsetter does not take.
    fn read(cff: &'a [u8]) -> Option<Read<'a>> {
        if cff.first() != Some(&1) {
            return None;
        }
        let names = usize::from(*cff.get(2)?);
        let (top_dicts, after_top_dicts) = parsed_index(cff, skip_index(cff, names)?)?;
        let top_dict = top_dicts.get(0)?;
        let top = operators(top_dict);
        let mut written = vec![top_dict];
        let mut scaled = Vec::new();
        // Of an operator given twice, the subsetter keeps the last.
        let last = |operator: u16| {
            top.iter()
                .rev()
                .find(|(found, _)| *found == operator)
                .map(|(_, operands)| operands.as_slice())
        };
        let (strings, after_strings) = parsed_index(cff, after_top_dicts)?;
        let (global, _) = parsed_index(cff, after_strings)?;
        let (charstrings, _) = parsed_index(cff, offset(last(CHAR_STRINGS)?)?)?;
        let glyph_count = charstrings.len();
        if glyph_count == 0 || glyph_count > usize::from(u16::MAX) {
            return None;
        }
        let mut font_names = Vec::new();
        let (local, fd_select) = if top.iter().any(|(operator, _)| *operator == ROS) {
            let (font_dicts, _) = parsed_index(cff, offset(last(FD_ARRAY)?)?)?;
            let fd_select = cff.get(offset(last(FD_SELECT)?)?..)?;
            let ranges = match fd_select.first()? {
                // A font DICT number for each glyph.
                0 => {
                    fd_select.get(1..glyph_count.checked_add(1)?)?;
                    0
                }
                // Ranges of glyphs; the subsetter goes through them for every glyph it keeps,
                // twice, and so does the walk, once. The subsetter takes a table whose count
                // is cut off by its end, and still asserts on the Font DICTs, so they are
                // counted below all the same.
                3 => u16_at(fd_select, 1).map_or(0, usize::from),
                _ => return None,
            };
            let count = (0..font_dicts.len())
                .take_while(|&i| font_dicts.get(i).is_some())
                .count();
            // The subsetter's own limit first, then the library's.
            if count > MAX_FONT_DICTS {
                return Some(Read::FontDicts(count));
            }
            if ranges > MAX_FD_SELECT_RANGES {
                return Some(Read::FdSelectRanges(ranges));
            }
            let font_dicts: Vec<&[u8]> = (0..count).filter_map(|i| font_dicts.get(i)).collect();
            // `parse_font_dict` reads the Private DICT of every Private operator of a Font
            // DICT, and so does this check; Font DICTs may repeat the operator and share
            // one large Private DICT. Their sizes are added up before any is read, up to
            // the first Private operator the subsetter cannot take (no range, or one past the
            // end of the table): it fails there, after reading the Private DICTs before it.
            let mut privates = Vec::new();
            let mut bytes = 0usize;
            let mut taken = true;
            'font_dicts: for font_dict in &font_dicts {
                let mut ranges = Vec::new();
                for (operator, operands) in operators(font_dict) {
                    if operator == PRIVATE {
                        let Some(private) =
                            range(&operands).filter(|private| private.end <= cff.len())
                        else {
                            taken = false;
                            break 'font_dicts;
                        };
                        bytes = bytes.saturating_add(private.len());
                        ranges.push(private);
                    }
                }
                privates.push(ranges);
            }
            if bytes > MAX_PRIVATE_BYTES {
                return Some(Read::PrivateBytes(bytes));
            }
            if !taken {
                return None;
            }
            let mut local = Vec::new();
            for (&font_dict, ranges) in font_dicts.iter().zip(privates) {
                // The last Private operator of the Font DICT gives its subroutines and the
                // Private DICT the subsetter writes back.
                let mut subrs = Subroutines::none();
                let mut private = None;
                for range in ranges {
                    private = Some(cff.get(range.clone())?);
                    subrs = local_subrs(cff, range)?;
                }
                written.push(font_dict);
                written.extend(private);
                local.push(subrs);
            }
            font_names = font_dicts
                .iter()
                .map(|font_dict| font_name(font_dict))
                .collect();
            if last(FONT_MATRIX).is_none() {
                scaled = font_dicts;
            }
            (local, Some(fd_select))
        } else {
            // A SID-keyed font whose local subroutines cannot be read has none, and the
            // subsetter drops its Private DICT then (`parse_sid_metadata` in its
            // `src/cff/sid_font.rs`), so that is not written back either.
            let private = last(PRIVATE).and_then(range);
            let subrs = private
                .clone()
                .and_then(|private| Some((cff.get(private.clone())?, local_subrs(cff, private)?)));
            let subrs = match subrs {
                Some((private, subrs)) => {
                    written.push(private);
                    subrs
                }
                None => Subroutines::none(),
            };
            (vec![subrs], None)
        };
        Some(Read::Programs(Box::new(Self {
            charstrings,
            strings,
            font_names,
            global: Subroutines::new(global),
            local,
            fd_select,
            written,
            scaled,
        })))
    }

    /// How many local subroutines the subsetter collects for all Font DICTs: those it can
    /// read of each one's Subrs INDEX, counted again for each Font DICT that shares it.
    /// Where the INDEXes declare no more than [`MAX_LOCAL_SUBRS`] in all, that is all
    /// that is counted; otherwise each INDEX is read once.
    fn collected_local_subrs(&self) -> usize {
        let declared = self
            .local
            .iter()
            .fold(0usize, |sum, subrs| sum.saturating_add(subrs.index.len()));
        if declared <= MAX_LOCAL_SUBRS {
            return declared;
        }
        // Each INDEX is read once, by the first Font DICT that has it: 255 Font DICTs that
        // share an INDEX of 65,535 would otherwise read it 255 times, the work the bound is
        // there to spare the subsetter.
        self.local
            .iter()
            .enumerate()
            .fold(0usize, |sum, (i, subrs)| {
                let first = self
                    .local
                    .iter()
                    .take(i)
                    .find(|other| std::ptr::eq(other.index.offsets, subrs.index.offsets))
                    .unwrap_or(subrs);
                sum.saturating_add(first.len())
            })
    }

    /// Walks the charstring of `glyph` as the subsetter decompiles it, reading at most
    /// `steps` operands, operators and hint mask bytes, and takes those it read off
    /// `steps`.
    fn walk(&self, glyph: u16, steps: &mut u32) -> std::result::Result<(), Stop> {
        let font_dict = match self.fd_select {
            Some(fd_select) => font_dict(fd_select, glyph).ok_or(Stop::Unreadable)?,
            None => 0,
        };
        let local = self
            .local
            .get(usize::from(font_dict))
            .ok_or(Stop::Unreadable)?;
        walk_glyph(&self.charstrings, &self.global, local, glyph, steps)
    }
}

/// Walks the charstring of `glyph` among `charstrings`, with the `global` and `local`
/// subroutines it may call, as the subsetter decompiles it, reading at most `steps`
/// operands, operators and hint mask bytes, and takes those it read off `steps`.
fn walk_glyph<'a>(
    charstrings: &Index<'a>,
    global: &'a Subroutines<'a>,
    local: &'a Subroutines<'a>,
    glyph: u16,
    steps: &mut u32,
) -> std::result::Result<(), Stop> {
    let charstring = charstrings
        .get(usize::from(glyph))
        .ok_or(Stop::Unreadable)?;
    let mut walk = Walk {
        global,
        local,
        stack: Vec::new(),
        hints: 0,
        mask_bytes: 0,
        steps: *steps,
    };
    let walked = walk.run(charstring, 1);
    *steps = walk.steps;
    walked.map(|_| ())
}

/// The subroutines of the Private DICT at `private`, which the subsetter finds by the
/// first Subrs operator of the DICT: none without one, or when its operand is no offset
/// it can read (`parse_subr_offset` in its `src/cff/dict/private_dict.rs`); `None` where
/// it fails.
fn local_subrs(cff: &[u8], private: Range<usize>) -> Option<Subroutines<'_>> {
    let start = private.start;
    let operators = operators(cff.get(private)?);
    let Some(offset) = operators
        .iter()
        .find(|(operator, _)| *operator == SUBRS)
        .and_then(|(_, operands)| offset(operands))
    else {
        return Some(Subroutines::none());
    };
    let (subrs, _) = parsed_index(cff, start.checked_add(offset)?)?;
    Some(Subroutines::new(subrs))
}

/// The string ID that the last FontName operator of a Font DICT gives, as the subsetter's
/// `parse_font_dict` keeps it (`parse_sid`: one operand, a whole number from 0 to 65535);
/// `None` without one. A Font DICT whose last FontName is no string ID makes the
/// subsetter fail on the table whatever it keeps.
fn font_name(font_dict: &[u8]) -> Option<u16> {
    let operators = operators(font_dict);
    let (_, operands) = operators
        .iter()
        .rev()
        .find(|(operator, _)| *operator == FONT_NAME)?;
    let [sid] = operands.as_slice() else {
        return None;
    };
    u16::try_from(sid.as_i32()?).ok()
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

/// Subroutines as the subsetter collects them from their INDEX: the objects up to the
/// first it cannot read. They are counted when a glyph first calls one, and not copied,
/// so that Font DICTs that share many subroutines cost only what the walk reaches.
struct Subroutines<'a> {
    index: Index<'a>,
    /// How many the subsetter collects, once counted.
    count: OnceCell<usize>,
}

impl<'a> Subroutines<'a> {
    fn new(index: Index<'a>) -> Self {
        Self {
            index,
            count: OnceCell::new(),
        }
    }

    fn none() -> Self {
        Self::new(Index {
            offsets: &[],
            offset_size: 1,
            data: &[],
        })
    }

    /// How many the subsetter collects.
    fn len(&self) -> usize {
        *self.count.get_or_init(|| {
            (0..self.index.len())
                .take_while(|&i| self.index.get(i).is_some())
                .count()
        })
    }

    fn get(&self, i: usize) -> Option<&'a [u8]> {
        if i < self.len() {
            self.index.get(i)
        } else {
            None
        }
    }
}

/// The subroutine a call operand `index` names among `subrs`, after the bias for their
/// number (Type 2 Charstring Format, 4.7).
fn subroutine<'a>(subrs: &Subroutines<'a>, index: i32) -> Option<&'a [u8]> {
    let bias = match subrs.len() {
        0..1240 => 107,
        1240..33900 => 1131,
        _ => 32768,
    };
    let index = usize::try_from(index.checked_add(bias)?).ok()?;
    subrs.get(index)
}

/// Why a walk stopped before the end of a glyph.
enum Stop {
    /// The glyph declares more stem hints than the subsetter can count.
    Overflow,
    /// The subsetter fails on the glyph with an error of its own.
    Unreadable,
    /// The glyph uses what the subsetter does not implement: `endchar` with the four
    /// operands of an accented character.
    Unsupported,
    /// The glyphs walked so far take more operands, operators and hint mask bytes than
    /// the walk may read.
    Limit,
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
    global: &'a Subroutines<'a>,
    local: &'a Subroutines<'a>,
    /// Each operand as an `i32`, `None` for one with a fraction.
    stack: Vec<Option<i32>>,
    hints: u32,
    /// The length of a hint mask, set at the first.
    mask_bytes: usize,
    /// How many more operands, operators and hint mask bytes the walk may read.
    steps: u32,
}

impl Walk<'_> {
    fn run(&mut self, charstring: &[u8], depth: u8) -> std::result::Result<End, Stop> {
        if depth > MAX_CALL_DEPTH {
            return Err(Stop::Unreadable);
        }
        let mut at = 0;
        while let Some(&b0) = charstring.get(at) {
            self.steps = self.steps.checked_sub(1).ok_or(Stop::Limit)?;
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
                    // The subsetter keeps the operator and its mask as two instructions, and
                    // copies the mask's bytes: one more besides the operator, and the bytes.
                    self.steps = u32::try_from(self.mask_bytes)
                        .ok()
                        .and_then(|bytes| bytes.checked_add(1))
                        .and_then(|cost| self.steps.checked_sub(cost))
                        .ok_or(Stop::Limit)?;
                }
                // endchar; with four operands it would be an accented character, which
                // the subsetter does not implement.
                14 if self.stack.len() >= 4 => return Err(Stop::Unsupported),
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

    /// The DICTs [`Programs::read`] finds that the subsetter writes back.
    fn written(cff: &[u8]) -> Vec<&[u8]> {
        match Programs::read(cff) {
            Some(Read::Programs(programs)) => programs.written,
            _ => panic!("the table is not read"),
        }
    }

    /// The Top DICT of `cff`: the first of the Top DICT INDEX, after the header and the
    /// Name INDEX.
    fn top_dict(cff: &[u8]) -> Option<&[u8]> {
        cff.get(2)
            .and_then(|&header_size| skip_index(cff, usize::from(header_size)))
            .and_then(|after_names| index(cff, after_names))
            .and_then(|top_dicts| top_dicts.get(0))
    }

    /// The offset `at` that a five-byte integer written as `PLACEHOLDER` in `cff` points
    /// to: `bytes` appended to the table, and every such integer replaced by their offset.
    fn appended(mut cff: Vec<u8>, bytes: &[u8]) -> Vec<u8> {
        let at = int5(cff.len());
        let placeholder = int5(PLACEHOLDER);
        let places: Vec<usize> = (0..cff.len().saturating_sub(4))
            .filter(|&i| cff[i..i + 5] == placeholder[..])
            .collect();
        assert!(!places.is_empty(), "no placeholder");
        for i in places {
            cff[i..i + 5].copy_from_slice(&at);
        }
        cff.extend(bytes);
        cff
    }

    /// A five-byte integer that [`appended`] replaces.
    const PLACEHOLDER: usize = 0x0BAD_F00D;

    #[test]
    fn reads_the_dicts_the_subsetter_writes_back() {
        // A SID-keyed font: the Top DICT and its Private DICT.
        let sid = cff_table(&[], &[], &PRIVATE_ZERO, None);
        let found = written(&sid);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1], &PRIVATE_ZERO[..]);
        // A CID-keyed font: the Top DICT, and the Font DICT with its Private DICT.
        let cid = cff_table(&ROS_ADOBE_IDENTITY, &[], &PRIVATE_ZERO, Some(&[139, 20]));
        let found = written(&cid);
        assert_eq!(found.len(), 3);
        assert_eq!(found[1][..2], [139, 20]);
        assert_eq!(found[2], &PRIVATE_ZERO[..]);
        // Only the first Top DICT is read: a second one with 1E99999 does not count.
        let second = Spec {
            glyphs: glyphs_with(4, vec![14]),
            local: vec![vec![]],
            second_top: PRIVATE_INFINITE.to_vec(),
            ..Spec::default()
        }
        .build();
        assert!(!subsetter_panics(second.clone(), &[4]));
        subset_glyphs(second, &[4]).unwrap();
    }

    #[test]
    fn reads_only_the_dicts_the_subsetter_reads() {
        // Of two Private operators in the Top DICT, the subsetter keeps the last: the first
        // may point at a Private DICT with 1E99999.
        let first = [int5(PRIVATE_INFINITE.len()), int5(PLACEHOLDER), vec![18]].concat();
        let sid = |top_prefix: Vec<u8>| {
            Spec {
                glyphs: glyphs_with(4, vec![14]),
                local: vec![vec![]],
                top_prefix,
                ..Spec::default()
            }
            .build()
        };
        let cff = appended(sid(first.clone()), &PRIVATE_INFINITE);
        assert!(!subsetter_panics(cff.clone(), &[4]));
        subset_glyphs(cff, &[4]).unwrap();
        // A table without ROS is SID-keyed, and the subsetter reads no FDArray, even one
        // the Top DICT points to, with 1E99999 in its Font DICT.
        let fd_index = [vec![0, 1, 1, 1, 7], PRIVATE_INFINITE.to_vec()].concat();
        let cff = appended(sid([int5(PLACEHOLDER), vec![12, 36]].concat()), &fd_index);
        assert!(!subsetter_panics(cff.clone(), &[4]));
        subset_glyphs(cff, &[4]).unwrap();
        // Of two FDArray operators, the subsetter keeps the last.
        let format3 = vec![3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        let cid = |top_prefix: Vec<u8>, font_dict_prefix: Vec<u8>, private: Subrs| {
            Spec {
                glyphs: glyphs_with(4, vec![14]),
                local: vec![vec![], vec![]],
                fd_select: Some(format3.clone()),
                top_prefix,
                font_dict_prefix,
                subrs: private,
                ..Spec::default()
            }
            .build()
        };
        let cff = appended(
            cid(
                [int5(PLACEHOLDER), vec![12, 36]].concat(),
                vec![],
                Subrs::One,
            ),
            &fd_index,
        );
        assert!(!subsetter_panics(cff.clone(), &[4]));
        subset_glyphs(cff, &[4]).unwrap();
        // Of two Private operators in a Font DICT, the subsetter reads both but writes back
        // only the last.
        let cff = appended(cid(vec![], first, Subrs::One), &PRIVATE_INFINITE);
        assert_eq!(written(&cff).len(), 5);
        assert!(!subsetter_panics(cff.clone(), &[4]));
        subset_glyphs(cff, &[4]).unwrap();
        // Font DICTs from the first that cannot be read on are not read: of three, the
        // second ends before it starts, so only the first and its Private DICT are.
        let mut cff = Spec {
            glyphs: glyphs_with(4, vec![14]),
            local: vec![vec![]; 3],
            fd_select: Some(format3.clone()),
            ..Spec::default()
        }
        .build();
        let top = operators(top_dict(&cff).unwrap());
        let at = top
            .iter()
            .find(|(operator, _)| *operator == FD_ARRAY)
            .and_then(|(_, operands)| offset(operands))
            .unwrap();
        assert_eq!(written(&cff).len(), 7);
        let size = usize::from(cff[at + 2]);
        let place = |i: usize| at + 3 + i * size..at + 3 + (i + 1) * size;
        let start = cff[place(1)]
            .iter()
            .fold(0usize, |value, &byte| value << 8 | usize::from(byte));
        let before = (start - 1).to_be_bytes();
        cff[place(2)].copy_from_slice(&before[before.len() - size..]);
        assert_eq!(written(&cff).len(), 3);
    }

    #[test]
    fn leaves_the_reals_of_a_table_the_subsetter_cannot_parse_to_it() {
        // A Font DICT with 1E99999 is refused, but not when the FDSelect has format 5,
        // which the subsetter cannot parse: it fails before it writes anything.
        let cff = cff_table(
            &ROS_ADOBE_IDENTITY,
            &[],
            &PRIVATE_ZERO,
            Some(&PRIVATE_INFINITE),
        );
        let err = subset_with(cff.clone(), false).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "CFF ", .. }),
            "{err}"
        );
        let mut cff = cff;
        let top = operators(top_dict(&cff).unwrap());
        let at = top
            .iter()
            .find(|(operator, _)| *operator == FD_SELECT)
            .and_then(|(_, operands)| offset(operands))
            .unwrap();
        cff[at] = 5;
        assert!(!subsetter_panics(cff.clone(), &[]));
        let err = subset_with(cff, false).unwrap_err();
        assert!(matches!(err, Error::Subset { .. }), "{err}");
        // A SID-keyed font whose local subroutines cannot be read is still read: the
        // subsetter subsets it, so 1E99999 in its Top DICT and a glyph of more stem hints
        // than it counts, through global subroutines, are refused.
        let unreadable = |glyphs: Vec<Vec<u8>>, global: Vec<u8>, top_prefix: Vec<u8>| {
            Spec {
                glyphs,
                global,
                local: vec![vec![]],
                subrs: Subrs::PastTheEnd,
                top_prefix,
                ..Spec::default()
            }
            .build()
        };
        let real = unreadable(glyphs_with(4, vec![14]), vec![], PRIVATE_INFINITE.to_vec());
        let err = subset_glyphs(real, &[4]).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "CFF ", .. }),
            "{err}"
        );
        let hints = unreadable(
            glyphs_with(1, hinted_glyph(65_536, 29, false)),
            index_of(&hint_subrs(29)),
            vec![],
        );
        assert!(refusal(subset_glyphs(hints, &[1])).starts_with("glyph 1 declares"));
        // A SID-keyed font whose subroutines cannot be read: the subsetter drops its Private
        // DICT and does not write it back, so 1E99999 there does not count.
        let private = [&PRIVATE_INFINITE[..], &int5(1_000_000), &[19]].concat();
        let sid = cff_table(&[], &[], &private, None);
        assert!(!subsetter_panics(sid.clone(), &[]));
        subset_with(sid, false).unwrap();
        // The last Private DICT of a Font DICT is written back, so 1E99999 there is refused.
        let cff = cff_table(&ROS_ADOBE_IDENTITY, &[], &PRIVATE_INFINITE, Some(&[]));
        let err = subset_with(cff, false).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "CFF ", .. }),
            "{err}"
        );
    }

    #[test]
    fn bounds_the_ranges_of_an_fd_select() {
        // An FDSelect of format 3 for the five glyphs, one range each, then empty ranges
        // that start at the sentinel, 5: 2,048 ranges are read, 2,049 are refused.
        let fd_select = |ranges: u16| {
            let mut select = vec![3];
            select.extend(ranges.to_be_bytes());
            for first in (0..5).chain(std::iter::repeat_n(5, usize::from(ranges) - 5)) {
                select.extend(u16::to_be_bytes(first));
                select.push(0);
            }
            select.extend(5u16.to_be_bytes());
            select
        };
        let font = |ranges, font_dicts| {
            Spec {
                glyphs: glyphs_with(4, vec![14]),
                local: vec![vec![]; font_dicts],
                fd_select: Some(fd_select(ranges)),
                ..Spec::default()
            }
            .build()
        };
        assert!(!subsetter_panics(font(2048, 1), &[4]));
        subset_glyphs(font(2048, 1), &[4]).unwrap();
        let err = subset_glyphs(font(2049, 1), &[4]).unwrap_err();
        assert!(
            matches!(err, Error::TableOverLimit { table: "CFF ", .. }),
            "{err}"
        );
        assert_eq!(
            std::error::Error::source(&err).unwrap().to_string(),
            "the FDSelect declares 2049 ranges, more than the 2048 read before subsetting"
        );
        // The subsetter's own limit on Font DICTs comes first.
        let err = subset_glyphs(font(2049, 256), &[4]).unwrap_err();
        assert!(matches!(err, Error::SubsetUnsupported { .. }), "{err}");
    }

    #[test]
    fn counts_the_font_dicts_when_the_fd_select_is_cut_off() {
        // An FDSelect of format 3 at the end of the table, cut off after its format or
        // within its count of ranges: the subsetter takes the table, and with 256 Font
        // DICTs it panics (subsetter#34), so the count of Font DICTs still refuses it.
        // With one Font DICT the table is left to the subsetter, which finds it damaged;
        // a count it cannot read is no count past the limit.
        let format3 = [3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        for (font_dicts, cut) in [(256, &[3][..]), (256, &[3, 0]), (1, &[3]), (1, &[3, 0])] {
            let mut cff = programs(
                &glyphs_with(4, vec![14]),
                &[],
                &vec![vec![]; font_dicts],
                Some(&format3),
            );
            // The FDSelect operator, a five-byte integer and 12 37, in the Top DICT.
            let at = cff
                .windows(7)
                .position(|w| w[0] == 29 && w[5..] == [12, 37])
                .unwrap();
            let offset = int5(cff.len());
            cff.splice(at..at + 5, offset);
            cff.extend(cut);
            if font_dicts == 1 {
                let err = subset_glyphs(cff, &[4]).unwrap_err();
                assert!(matches!(err, Error::Subset { .. }), "{cut:?}: {err}");
                continue;
            }
            if cfg!(debug_assertions) {
                assert!(subsetter_panics(cff.clone(), &[4]), "{cut:?}");
            }
            let err = subset_glyphs(cff, &[4]).unwrap_err();
            assert!(
                matches!(err, Error::SubsetUnsupported { .. }),
                "{cut:?}: {err}"
            );
        }
    }

    /// A CID-keyed table of `font_dicts` Font DICTs whose local subroutines are
    /// `local`, the first `local.len()` Font DICTs' each, and none for the others.
    fn with_local_subrs(local: Vec<Vec<Vec<u8>>>, font_dicts: usize) -> Vec<u8> {
        let mut all = local;
        all.resize(font_dicts, vec![]);
        Spec {
            glyphs: glyphs_with(4, vec![14]),
            local: all,
            fd_select: Some(vec![3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5]),
            ..Spec::default()
        }
        .build()
    }

    /// Points the Private operator of every Font DICT of `cff` from Font DICT `from` on,
    /// made by [`Spec`] with one Private operator each, at Font DICT `from`'s Private
    /// DICT, and returns where that is.
    fn sharing_a_private(cff: &mut [u8], from: usize) -> usize {
        // Each Font DICT ends in the size of its Private DICT, 6, its offset and 18.
        let size = int5(6);
        let at: Vec<usize> = (0..cff.len().saturating_sub(10))
            .filter(|&i| cff[i..i + 5] == size[..] && cff[i + 5] == 29 && cff[i + 10] == 18)
            .map(|i| i + 5)
            .collect();
        let shared = cff[at[from]..at[from] + 5].to_vec();
        for &i in &at[from..] {
            cff[i..i + 5].copy_from_slice(&shared);
        }
        usize::try_from(i32::from_be_bytes(shared[1..].try_into().unwrap())).unwrap()
    }

    #[test]
    fn bounds_the_local_subroutines_the_subsetter_collects() {
        let message = |count: usize| {
            format!(
                "the Font DICTs have {count} local subroutines in all, more than the 262144 \
                 the subsetter may collect"
            )
        };
        // Eight Font DICTs of 32,768 each come to the limit; one more subroutine is past it.
        let subrs = vec![vec![11]; 32768];
        let at_limit = with_local_subrs(vec![subrs.clone(); 8], 8);
        check_with_limit(at_limit.clone(), &[4], 1 << 20).unwrap();
        subset_glyphs(at_limit, &[4]).unwrap();
        let mut local = vec![subrs; 8];
        local.push(vec![vec![11]]);
        let past = with_local_subrs(local, 9);
        assert_eq!(
            over_limit(check_with_limit(past, &[4], 1 << 20)),
            message(262_145)
        );

        // Font DICTs that share one Subrs INDEX make the subsetter collect it for each.
        let mut shared = with_local_subrs(vec![vec![vec![11]; 65535]], 4);
        sharing_a_private(&mut shared, 0);
        check_with_limit(shared, &[4], 1 << 20).unwrap();
        let mut shared = with_local_subrs(vec![vec![vec![11]; 65535]], 5);
        sharing_a_private(&mut shared, 0);
        assert_eq!(
            over_limit(check_with_limit(shared.clone(), &[4], 1 << 20)),
            message(327_675)
        );
        // The INDEX is read once, by the first Font DICT, however many share it.
        let Some(Read::Programs(programs)) = Programs::read(&shared) else {
            panic!("the table is not read");
        };
        assert_eq!(programs.collected_local_subrs(), 327_675);
        assert!(programs.local[0].count.get().is_some());
        assert!(
            programs.local[1..]
                .iter()
                .all(|subrs| subrs.count.get().is_none())
        );
        // Also where the Font DICTs that share an INDEX come after one with its own.
        let mut after = with_local_subrs(vec![vec![vec![11]; 10], vec![vec![11]; 65535]], 5);
        sharing_a_private(&mut after, 1);
        let Some(Read::Programs(programs)) = Programs::read(&after) else {
            panic!("the table is not read");
        };
        assert_eq!(programs.collected_local_subrs(), 262_150);
        assert!(
            programs.local[..2]
                .iter()
                .all(|subrs| subrs.count.get().is_some())
        );
        assert!(
            programs.local[2..]
                .iter()
                .all(|subrs| subrs.count.get().is_none())
        );
        // The count comes before any glyph is walked.
        assert_eq!(
            over_limit(check_with_limit(shared, &[4], 0)),
            message(327_675)
        );

        // The subsetter collects only up to the first subroutine it cannot read: a shared
        // INDEX that declares 65,535 but holds 10 is no count past the limit.
        let mut damaged = with_local_subrs(vec![vec![]], 5);
        let private = sharing_a_private(&mut damaged, 0);
        let subrs_at = damaged.len() - private;
        let mut index = vec![0xFF, 0xFF, 1];
        index.extend(1..=11);
        index.resize(3 + 65535, 0);
        index.push(11);
        index.extend([11; 10]);
        damaged.extend(index);
        damaged[private..private + 5].copy_from_slice(&int5(subrs_at));
        check_with_limit(damaged, &[4], 1 << 20).unwrap();
    }

    #[test]
    fn bounds_the_private_dicts_the_font_dicts_point_at() {
        // 128 Font DICTs, each with a first Private operator pointing at one Private DICT
        // of `size` bytes ("139 6" pairs) and then its own of 6: 128 × 8,192 bytes is
        // 1 MiB, which passes, and 128 × 8,194 is past it.
        let format3 = vec![3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        let font = |count: usize, prefix: Vec<u8>, size: usize| {
            let cff = Spec {
                glyphs: glyphs_with(4, vec![14]),
                local: vec![vec![]; count],
                fd_select: Some(format3.clone()),
                font_dict_prefix: prefix,
                ..Spec::default()
            }
            .build();
            appended(cff, &[139, 6].repeat(size / 2))
        };
        let shared = |size: usize| [int5(size), int5(PLACEHOLDER), vec![18]].concat();
        let within = font(128, shared(8186), 8186);
        assert!(!subsetter_panics(within.clone(), &[4]));
        subset_glyphs(within, &[4]).unwrap();
        let err = subset_glyphs(font(128, shared(8188), 8188), &[4]).unwrap_err();
        assert!(
            matches!(err, Error::TableOverLimit { table: "CFF ", .. }),
            "{err}"
        );
        assert_eq!(
            std::error::Error::source(&err).unwrap().to_string(),
            "the Font DICTs point at 1048832 bytes of Private DICTs, more than the 1048576 \
             read before subsetting"
        );
        // One Font DICT that repeats the Private operator 300 times, also when a Private
        // operator the subsetter cannot take ("0 Private", one operand) follows them: the
        // subsetter reads the 300 before it fails on that one.
        for after in [vec![], vec![139, 18]] {
            let prefix = [shared(3500).repeat(300), after].concat();
            let err = subset_glyphs(font(1, prefix, 3500), &[4]).unwrap_err();
            assert!(
                matches!(err, Error::TableOverLimit { table: "CFF ", .. }),
                "{err}"
            );
        }
        // A Private DICT that runs past the end of the table is not counted: the subsetter
        // fails on it with an error of its own, and so it does here.
        let past = font(1, shared(2_000_000), 6);
        assert!(!subsetter_panics(past.clone(), &[4]));
        let err = subset_glyphs(past, &[4]).unwrap_err();
        assert!(matches!(err, Error::Subset { .. }), "{err}");
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
        /// "1000000 Subrs": subroutines past the end of the table, which the subsetter
        /// cannot parse.
        PastTheEnd,
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
        /// A second Top DICT, which the subsetter does not read; none if empty.
        second_top: Vec<u8>,
        /// Written first in each Font DICT.
        font_dict_prefix: Vec<u8>,
        /// Written first in the last Font DICT instead, when not empty.
        last_font_dict_prefix: Vec<u8>,
    }

    impl Spec {
        fn build(&self) -> Vec<u8> {
            let cid = self.fd_select.is_some();
            let private_dict: Vec<u8> = match self.subrs {
                Subrs::One => [int5(6), vec![19]].concat(),
                Subrs::Twice => [int5(12), vec![19], int5(1_000_000), vec![19]].concat(),
                Subrs::Unreadable => vec![139, 139, 19],
                Subrs::PastTheEnd => [int5(1_000_000), vec![19]].concat(),
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
            let last = self.local.len().saturating_sub(1);
            let font_dict = |i: usize, first_at: usize, private_at: usize| {
                let mut dict = if i == last && !self.last_font_dict_prefix.is_empty() {
                    self.last_font_dict_prefix.clone()
                } else {
                    self.font_dict_prefix.clone()
                };
                if self.two_private {
                    dict.extend([int5(private_len), int5(first_at), vec![18]].concat());
                }
                dict.extend([int5(private_len), int5(private_at), vec![18]].concat());
                dict
            };
            let head = vec![1, 0, 4, 1];
            let names = index_of(&[b"A".to_vec()]);
            let top_dicts = |first: Vec<u8>| {
                if self.second_top.is_empty() {
                    index_of(&[first])
                } else {
                    index_of(&[first, self.second_top.clone()])
                }
            };
            let top_len = top_dicts(top_dict(0, 0, 0, 0)).len();
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
                let dicts: Vec<Vec<u8>> =
                    (0..self.local.len()).map(|i| font_dict(i, 0, 0)).collect();
                index_of(&dicts).len()
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
            cff.extend(top_dicts(top_dict(
                charstrings_at,
                fd_array_at,
                fd_select_at,
                private_at[0],
            )));
            cff.extend(strings);
            cff.extend(global);
            cff.extend(charstrings);
            if cid {
                cff.extend_from_slice(&fd_select);
                let dicts: Vec<Vec<u8>> = private_at
                    .iter()
                    .enumerate()
                    .map(|(i, &at)| font_dict(i, private_at[0], at))
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
        panics_with(
            cff,
            &subsetter::GlyphRemapper::new_from_glyphs_sorted(glyphs),
        )
    }

    /// Whether subsetter 0.2.6 itself panics on the test font with `cff` as its outlines,
    /// kept to the glyphs of `remapper`.
    fn panics_with(cff: Vec<u8>, remapper: &subsetter::GlyphRemapper) -> bool {
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        std::panic::catch_unwind(|| subsetter::subset(&font, 0, remapper)).is_err()
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

    #[test]
    fn refuses_too_many_font_dicts_before_it_walks_a_glyph() {
        // The subsetter asserts before it decompiles a glyph, so a kept glyph or .notdef
        // it cannot read (callsubr without local subroutines) does not stop the refusal.
        let format3 = [3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        for at in [4, 0] {
            let cff = programs(
                &glyphs_with(at, vec![139, 10]),
                &[],
                &vec![vec![]; 256],
                Some(&format3),
            );
            if cfg!(debug_assertions) {
                assert!(subsetter_panics(cff.clone(), &[4]), "glyph {at}");
            }
            let refused = subset_glyphs(cff, &[4]).err();
            assert!(
                matches!(refused, Some(Error::SubsetUnsupported { .. })),
                "glyph {at}: {refused:?}"
            );
        }
        // The count in the message is the font's.
        let cff = programs(
            &glyphs_with(4, vec![14]),
            &[],
            &vec![vec![]; 300],
            Some(&format3),
        );
        let Some(Error::SubsetUnsupported { source, .. }) = subset_glyphs(cff, &[4]).err() else {
            panic!("300 Font DICTs not refused");
        };
        assert!(source.to_string().contains("300 Font DICTs"), "{source}");
    }

    #[test]
    fn counts_font_dicts_up_to_the_first_it_cannot_read() {
        // 257 Font DICTs, of which object 200 (counting from 0) ends before it starts: the
        // subsetter takes the first 200, does not panic, and subsets.
        let format3 = [3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        let mut cff = programs(
            &glyphs_with(4, vec![14]),
            &[],
            &vec![vec![]; 257],
            Some(&format3),
        );
        let top = operators(top_dict(&cff).unwrap());
        let at = top
            .iter()
            .find(|(operator, _)| *operator == FD_ARRAY)
            .and_then(|(_, operands)| offset(operands))
            .unwrap();
        let size = usize::from(cff[at + 2]);
        let place = |i: usize| at + 3 + i * size..at + 3 + (i + 1) * size;
        let read = |cff: &[u8], i: usize| {
            cff[place(i)]
                .iter()
                .fold(0usize, |value, &byte| value << 8 | usize::from(byte))
        };
        let start = read(&cff, 200);
        let end = (start - (read(&cff, 201) - start)).to_be_bytes();
        cff[place(201)].copy_from_slice(&end[end.len() - size..]);
        assert!(!subsetter_panics(cff.clone(), &[4]));
        subset_glyphs(cff, &[4]).unwrap();
    }

    #[test]
    fn counts_font_dicts_before_it_checks_the_reals() {
        // A real number past f32 in the Top DICT, which the reals check refuses: with 255
        // Font DICTs it does, with 256 the count refuses the font first, before the reals
        // check reads the Private DICT of every Font DICT.
        let format3 = vec![3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        for count in [255, 256] {
            let cff = Spec {
                glyphs: glyphs_with(4, vec![14]),
                local: vec![vec![]; count],
                fd_select: Some(format3.clone()),
                top_prefix: PRIVATE_INFINITE.to_vec(),
                ..Spec::default()
            }
            .build();
            let refused = subset_glyphs(cff, &[4]).unwrap_err();
            if count > MAX_FONT_DICTS {
                assert!(
                    matches!(refused, Error::SubsetUnsupported { .. }),
                    "{refused}"
                );
            } else {
                assert!(
                    matches!(refused, Error::MalformedTable { table: "CFF ", .. }),
                    "{refused}"
                );
            }
        }
    }

    #[test]
    fn calls_no_subroutine_past_the_first_it_cannot_read() {
        // The subsetter collects the first 1,239 global subroutines of `broken_global`;
        // object 1,240 can be read on its own, but a call to it (operand 1,133) fails in
        // the subsetter with an error of its own. The hints after the call are never
        // counted, so the check leaves the glyph to the subsetter.
        let glyph = [vec![28, 4, 109, 29], hinted_glyph(65_536, 29, false)].concat();
        let cff = Spec {
            glyphs: glyphs_with(4, glyph),
            global: broken_global(),
            local: vec![vec![]],
            ..Spec::default()
        }
        .build();
        assert!(!subsetter_panics(cff.clone(), &[4]));
        let err = subset_glyphs(cff, &[4]).unwrap_err();
        assert!(matches!(err, Error::Subset { .. }), "{err}");
    }

    #[test]
    fn counts_font_dicts_before_it_reads_their_private_dicts() {
        // Every Font DICT points at a Private DICT whose subroutines lie past the end of
        // the table. The subsetter fails to parse such a table, at any count, with an
        // error of its own. The check counts first, so that Font DICTs sharing one large
        // Private DICT cost no more than their count to refuse: 256 are refused for their
        // number, 255 are left to the subsetter.
        let format3 = vec![3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        for count in [255, 256] {
            let cff = Spec {
                glyphs: glyphs_with(4, vec![14]),
                local: vec![vec![]; count],
                fd_select: Some(format3.clone()),
                subrs: Subrs::PastTheEnd,
                ..Spec::default()
            }
            .build();
            assert!(!subsetter_panics(cff.clone(), &[4]), "{count}");
            let refused = subset_glyphs(cff, &[4]).unwrap_err();
            if count > MAX_FONT_DICTS {
                assert!(
                    matches!(refused, Error::SubsetUnsupported { .. }),
                    "{refused}"
                );
            } else {
                assert!(matches!(refused, Error::Subset { .. }), "{refused}");
            }
        }
    }

    /// [`check`] with a limit of `limit` steps of its own on the test font with `cff` as
    /// its outlines, keeping `glyphs`.
    fn check_with_limit(cff: Vec<u8>, glyphs: &[u16], limit: u32) -> Result<()> {
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        let font_ref = FontRef::new(&font).unwrap();
        let limit = Limit {
            steps: limit,
            budget: false,
        };
        // In the order the subsetter takes them, as `FontFace::subset` passes them.
        let kept: Vec<u16> = subsetter::GlyphRemapper::new_from_glyphs_sorted(glyphs)
            .remapped_gids()
            .collect();
        check(&font_ref, &font, 0, &kept, limit).map(|_| ())
    }

    /// The message of a refusal by [`check`] for its limit.
    fn over_limit(result: Result<()>) -> String {
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::TableOverLimit { table: "CFF ", .. }),
            "{err}"
        );
        std::error::Error::source(&err).unwrap().to_string()
    }

    #[test]
    fn bounds_what_decompiling_the_kept_glyphs_reads() {
        // Global subroutine 0 is two operands, rmoveto and return: 4 operands and
        // operators. Glyph 1 calls it twice, with an operand and callgsubr each time, and
        // ends: 13. .notdef's endchar makes 14.
        let twice = vec![32, 29, 32, 29, 14];
        let mut glyphs = glyphs_with(1, twice.clone());
        glyphs[2] = twice;
        let cff = programs(&glyphs, &[vec![139, 139, 21, 11]], &[vec![]], None);
        check_with_limit(cff.clone(), &[1], 14).unwrap();
        assert_eq!(
            over_limit(check_with_limit(cff.clone(), &[1], 13)),
            "decompiling the glyphs to keep, up to glyph 1, reads more than 13 operands, \
             operators and hint mask bytes"
        );
        // The kept glyphs share the limit: glyph 2 takes 13 more.
        check_with_limit(cff.clone(), &[2, 1], 27).unwrap();
        assert!(over_limit(check_with_limit(cff.clone(), &[2, 1], 26)).contains("up to glyph 2,"));
        // .notdef is walked first, also when it is not asked for.
        assert!(over_limit(check_with_limit(cff, &[1], 0)).contains("up to glyph 0,"));

        // Nine stem hints (18 operands and hstem: 19), a hintmask, counted twice, and its two
        // bytes (4), and endchar.
        let mut hinted = vec![139; 18];
        hinted.extend([1, 19, 0xFF, 0x80, 14]);
        let cff = programs(&glyphs_with(3, hinted), &[], &[vec![]], None);
        check_with_limit(cff.clone(), &[3], 25).unwrap();
        assert!(over_limit(check_with_limit(cff.clone(), &[3], 24)).contains("up to glyph 3,"));
        // The limit also runs out within the hint mask's own cost.
        assert!(over_limit(check_with_limit(cff, &[3], 23)).contains("up to glyph 3,"));

        // Local subroutines: of a SID-keyed font (an operand, callsubr, 4 and endchar), and
        // of a CID-keyed font's Font DICT 1, which glyphs 3 and 4 use.
        let local = vec![vec![139, 139, 21, 11]];
        let cff = programs(
            &glyphs_with(3, vec![32, 10, 14]),
            &[],
            std::slice::from_ref(&local),
            None,
        );
        check_with_limit(cff.clone(), &[3], 8).unwrap();
        assert!(over_limit(check_with_limit(cff, &[3], 7)).contains("up to glyph 3,"));
        let format3 = [3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        let cff = programs(
            &glyphs_with(4, vec![32, 10, 32, 10, 14]),
            &[],
            &[vec![], local],
            Some(&format3),
        );
        check_with_limit(cff.clone(), &[4], 14).unwrap();
        assert!(over_limit(check_with_limit(cff, &[4], 13)).contains("up to glyph 4,"));
    }

    #[test]
    fn checks_the_limit_on_the_way_subset_checks() {
        // Global subroutine 0 returns; 1 calls 0 2,000 times; 2 calls 1 2,000 times, which
        // reads 12,006,001 operands and operators. A glyph that calls 2 twice is past the
        // limit, one that calls it once is not. `check` is what `FontFace::subset`
        // calls; the subsetter itself never runs here, also if the limit were broken.
        let calls = |subr: u8, times: usize| {
            let mut program = [subr + 32, 29].repeat(times);
            program.push(11);
            program
        };
        let global = [vec![11], calls(0, 2000), calls(1, 2000)];
        let check = |glyph: Vec<u8>| {
            let cff = programs(&glyphs_with(1, glyph), &global, &[vec![]], None);
            let font = crate::testing::TestFont::new("Test Sans")
                .with_table(b"CFF ", cff)
                .without_table(b"glyf")
                .without_table(b"loca")
                .build();
            let font_ref = FontRef::new(&font).unwrap();
            check(&font_ref, &font, 0, &[0, 1], Limit::within(u64::MAX)).map(|_| ())
        };
        assert_eq!(
            over_limit(check(vec![34, 29, 34, 29, 14])),
            "decompiling the glyphs to keep, up to glyph 1, reads more than 16777216 \
             operands, operators and hint mask bytes"
        );
        check(vec![34, 29, 14]).unwrap();
    }

    /// The test font with `cff` as its outlines, subset to `glyphs` within `budget`.
    fn subset_within(
        cff: Vec<u8>,
        glyphs: &[u16],
        budget: &mut crate::subset::SubsetBudget,
    ) -> Result<crate::subset::Subset> {
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        let glyphs = glyphs.iter().map(|&glyph| crate::font::GlyphId::new(glyph));
        crate::font::FontFace::from_bytes(font, 0)
            .unwrap()
            .subset_within(glyphs, budget)
    }

    #[test]
    fn subsets_share_a_budget() {
        use crate::subset::SubsetBudget;

        // Glyph 1 calls global subroutine 0 twice: with .notdef, decompiling it reads 14
        // operands and operators (`bounds_what_decompiling_the_kept_glyphs_reads`).
        let mut glyphs = glyphs_with(1, vec![32, 29, 32, 29, 14]);
        glyphs[2] = vec![139, 139, 21, 14];
        let cff = programs(&glyphs, &[vec![139, 139, 21, 11]], &[vec![]], None);
        let mut budget = SubsetBudget::new(100);
        subset_within(cff.clone(), &[1], &mut budget).unwrap();
        assert_eq!(budget.left(), 86);
        // Each subset takes what it reads; glyph 2 reads 4, glyph 1 the same 13 again.
        subset_within(cff.clone(), &[2, 1, 1], &mut budget).unwrap();
        assert_eq!(budget.left(), 68);

        // A subset may read all that is left, but no more; a refused one takes nothing.
        let mut budget = SubsetBudget::new(14);
        subset_within(cff.clone(), &[1], &mut budget).unwrap();
        assert_eq!(budget.left(), 0);
        let mut budget = SubsetBudget::new(13);
        let err = subset_within(cff.clone(), &[1], &mut budget).unwrap_err();
        assert!(
            matches!(err, Error::TableOverLimit { table: "CFF ", .. }),
            "{err}"
        );
        assert_eq!(
            std::error::Error::source(&err).unwrap().to_string(),
            "decompiling the glyphs to keep, up to glyph 1, reads more than the 13 operands, \
             operators and hint mask bytes left of its budget"
        );
        assert_eq!(budget.left(), 13);
        let mut budget = SubsetBudget::new(0);
        let err = subset_within(cff, &[], &mut budget).unwrap_err();
        assert!(
            over_limit(Err(err)).contains("up to glyph 0, reads more than the 0 operands"),
            "a budget of 0"
        );

        // A subset that the check lets through and the subsetter refuses takes nothing:
        // the check stops at glyph 1, at an operator the subsetter does not know (12 0),
        // having read 5 units: 1 of .notdef and 4 of glyph 1, which comes before glyph 2.
        let mut glyphs = glyphs_with(1, vec![139, 139, 21, 12, 0]);
        glyphs[2] = vec![139, 139, 21, 14];
        let cff = programs(&glyphs, &[], &[vec![]], None);
        let mut budget = SubsetBudget::new(100);
        let err = subset_within(cff, &[2, 1], &mut budget).unwrap_err();
        assert!(matches!(err, Error::Subset { .. }), "{err}");
        assert_eq!(budget.left(), 100);

        // TrueType outlines take nothing.
        let truetype = crate::testing::TestFont::new("Test Sans").build();
        let truetype = crate::font::FontFace::from_bytes(truetype, 0).unwrap();
        let mut budget = SubsetBudget::new(100);
        truetype
            .subset_within([crate::font::GlyphId::new(2)], &mut budget)
            .unwrap();
        assert_eq!(budget.left(), 100);
    }

    #[test]
    fn a_budget_larger_than_the_limit_of_a_subset_keeps_that_limit() {
        // The font of `checks_the_limit_on_the_way_subset_checks` whose glyph 1 calls a
        // subroutine that reads 12,006,001 operands and operators twice, past the limit of
        // one subset. The check refuses it before the subsetter runs.
        let calls = |subr: u8, times: usize| {
            let mut program = [subr + 32, 29].repeat(times);
            program.push(11);
            program
        };
        let global = [vec![11], calls(0, 2000), calls(1, 2000)];
        let cff = programs(
            &glyphs_with(1, vec![34, 29, 34, 29, 14]),
            &global,
            &[vec![]],
            None,
        );
        let mut budget = crate::subset::SubsetBudget::new(1 << 40);
        let err = subset_within(cff, &[1], &mut budget).unwrap_err();
        assert_eq!(
            over_limit(Err(err)),
            "decompiling the glyphs to keep, up to glyph 1, reads more than 16777216 \
             operands, operators and hint mask bytes"
        );
        assert_eq!(budget.left(), 1 << 40);
    }

    #[test]
    fn the_limit_of_a_subset_is_the_budget_left_below_its_own() {
        let limit = |left: u64| {
            let limit = Limit::within(left);
            (limit.steps, limit.budget)
        };
        assert_eq!(limit(0), (0, true));
        assert_eq!(limit(u64::from(MAX_STEPS) - 1), (MAX_STEPS - 1, true));
        assert_eq!(limit(u64::from(MAX_STEPS)), (MAX_STEPS, false));
        assert_eq!(limit(u64::from(u32::MAX) + 1), (MAX_STEPS, false));
        assert_eq!(limit(u64::MAX), (MAX_STEPS, false));
    }

    #[test]
    fn leaves_a_glyph_it_cannot_read_to_the_subsetter_before_the_limit() {
        // Glyph 1 has an operator the subsetter does not know (12 0); glyph 2 alone is
        // past the limit. The subsetter fails on glyph 1 first, so the check leaves it.
        let mut glyphs = glyphs_with(1, vec![12, 0]);
        glyphs[2] = vec![139; 40].into_iter().chain([21, 14]).collect();
        let cff = programs(&glyphs, &[], &[vec![]], None);
        check_with_limit(cff.clone(), &[1, 2], 10).unwrap();
        assert!(over_limit(check_with_limit(cff, &[2], 10)).contains("up to glyph 2,"));
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

    /// [`subset_glyphs`] with the glyphs numbered in the order given.
    fn subset_glyphs_in_order(cff: Vec<u8>, glyphs: &[u16]) -> Result<crate::subset::Subset> {
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        crate::font::FontFace::from_bytes(font, 0)
            .unwrap()
            .subset_in_order_within(
                glyphs.iter().map(|&glyph| crate::font::GlyphId::new(glyph)),
                &mut crate::SubsetBudget::new(u64::MAX),
            )
    }

    /// The message of a refusal by [`check`] for damage.
    fn refusal(result: Result<crate::subset::Subset>) -> String {
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "CFF ", .. }),
            "{err}"
        );
        std::error::Error::source(&err).unwrap().to_string()
    }

    /// What checking each glyph of the test font with `cff` as its outlines gives, each
    /// on a face of its own; see [`crate::testing::subset_checks`].
    fn glyph_checks(cff: Vec<u8>) -> Vec<std::result::Result<(), String>> {
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        crate::testing::subset_checks(&font)
    }

    #[test]
    fn checks_each_glyph_as_the_subsetter_decompiles_it() {
        let cff = |at, glyph| programs(&glyphs_with(at, glyph), &hint_subrs(29), &[vec![]], None);
        assert_eq!(glyph_checks(cff(1, vec![14])), vec![Ok(()); 5]);
        // An operator the subsetter does not know.
        assert_eq!(
            glyph_checks(cff(2, vec![12, 0, 14]))[1..],
            [
                Ok(()),
                Err(
                    "glyph 2: its charstring cannot be decompiled as the subsetter \
                     decompiles it"
                        .to_owned()
                ),
                Ok(()),
                Ok(()),
            ]
        );
        // An accented character composed with endchar, which it does not implement.
        assert_eq!(
            glyph_checks(cff(3, vec![139, 139, 139, 139, 14]))[3],
            Err(
                "glyph 3 composes an accented character with endchar, which the subsetter \
                 does not implement"
                    .to_owned()
            )
        );
        // More stem hints than it counts.
        assert_eq!(
            glyph_checks(cff(4, hinted_glyph(65_536, 29, false)))[4],
            Err(
                "glyph 4 declares more than 65535 stem hints, more than the subsetter can \
                 count"
                    .to_owned()
            )
        );
        // .notdef is checked for the face, and fails every glyph.
        let checks = glyph_checks(cff(0, vec![12, 0, 14]));
        assert!(checks.iter().all(|check| check.is_err()), "{checks:?}");
        // The hmtx and post entries are read as for TrueType outlines.
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff(1, vec![14]))
            .with_table(b"post", crate::testing::post_naming(4))
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        assert_eq!(
            crate::testing::subset_checks(&font)[3..],
            [
                Ok(()),
                Err(
                    "glyph 4: the post table names 4 glyphs, so it has no name for this one"
                        .to_owned()
                ),
            ]
        );
    }

    #[test]
    fn checks_the_font_name_of_each_glyphs_font_dict() {
        // Glyphs 0 to 2 in Font DICT 0, 3 and 4 in Font DICT 1, which names its font by
        // `sid` (12 38). The String INDEX has "Adobe" (391) and "Identity" (392).
        let format3 = [3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5];
        let cff = |sid: u16| {
            Spec {
                glyphs: glyphs_with(1, vec![14]),
                local: vec![vec![], vec![]],
                fd_select: Some(format3.to_vec()),
                last_font_dict_prefix: [vec![28], sid.to_be_bytes().to_vec(), vec![12, 38]]
                    .concat(),
                ..Spec::default()
            }
            .build()
        };
        for sid in [0, 390, 391, 392] {
            assert_eq!(glyph_checks(cff(sid)), vec![Ok(()); 5], "string {sid}");
        }
        // Of two FontName operators, the subsetter keeps the last.
        let named = |first: u16, last: u16| {
            Spec {
                glyphs: glyphs_with(1, vec![14]),
                local: vec![vec![], vec![]],
                fd_select: Some(format3.to_vec()),
                last_font_dict_prefix: [
                    vec![28],
                    first.to_be_bytes().to_vec(),
                    vec![12, 38, 28],
                    last.to_be_bytes().to_vec(),
                    vec![12, 38],
                ]
                .concat(),
                ..Spec::default()
            }
            .build()
        };
        assert_eq!(glyph_checks(named(393, 391)), vec![Ok(()); 5]);
        assert!(glyph_checks(named(391, 393))[3].is_err());
        let unnamed = |glyph| {
            Err(format!(
                "glyph {glyph}: its Font DICT names its font by string 393, which the String \
                 INDEX does not have"
            ))
        };
        assert_eq!(
            glyph_checks(cff(393)),
            [Ok(()), Ok(()), Ok(()), unnamed(3), unnamed(4)]
        );
    }

    #[test]
    fn keeps_the_subroutine_counts_for_the_next_checks() {
        // Glyph 1 calls global subroutine 0 (operand -107, bias 107), glyph 2 local
        // subroutine 0.
        let mut glyphs = glyphs_with(1, vec![32, 29, 14]);
        glyphs[2] = vec![32, 10, 14];
        let cff = programs(
            &glyphs,
            &[vec![139, 139, 21, 11]],
            &[vec![vec![139, 139, 21, 11]]],
            None,
        );
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        let font_ref = FontRef::new(&font).unwrap();
        let (range, mut glyphs) = CffGlyphs::read(&font_ref, &font).unwrap();
        let cff = &font[range];
        let mut work = MAX_CHECK_WORK;
        check_glyph(cff, &mut glyphs, 0, 1, MAX_STEPS, &mut work).unwrap();
        assert_eq!(glyphs.global.count, Some(1));
        assert_eq!(glyphs.local[0].count, None);
        // Glyph 2 calls local subroutine 0, of one.
        check_glyph(cff, &mut glyphs, 0, 2, MAX_STEPS, &mut work).unwrap();
        assert_eq!(glyphs.local[0].count, Some(1));
        // The next checks take the numbers kept rather than counting again.
        glyphs.global.count = Some(0);
        glyphs.local[0].count = Some(0);
        assert!(check_glyph(cff, &mut glyphs, 0, 1, MAX_STEPS, &mut work).is_err());
        assert!(check_glyph(cff, &mut glyphs, 0, 2, MAX_STEPS, &mut work).is_err());
    }

    #[test]
    fn a_glyph_that_fails_takes_nothing_off_the_steps_of_the_others() {
        // Global subroutine 0 is two operands, rmoveto and return; glyphs 1 and 2 call it
        // twice (13 each), glyph 3 is rmoveto and endchar (4).
        let twice = vec![32, 29, 32, 29, 14];
        let mut glyphs = glyphs_with(1, twice.clone());
        glyphs[2] = twice;
        glyphs[3] = vec![139, 139, 21, 14];
        let cff = programs(&glyphs, &[vec![139, 139, 21, 11]], &[vec![]], None);
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        let font_ref = FontRef::new(&font).unwrap();
        let (range, mut read) = CffGlyphs::read(&font_ref, &font).unwrap();
        let cff = &font[range];
        let reason = |result: Result<()>| {
            let err = result.unwrap_err();
            assert!(matches!(err, Error::TableOverLimit { .. }), "{err}");
            std::error::Error::source(&err).unwrap().to_string()
        };
        // Glyph 1 reads 13 within 20 steps; glyph 2 does not within 7, and glyph 3 reads 4.
        // Work counts what every glyph checked read.
        let reason = |result: Result<u32>| reason(result.map(|_| ()));
        let mut work = 100;
        assert_eq!(
            check_glyph(cff, &mut read, 0, 1, 20, &mut work).unwrap(),
            13
        );
        assert_eq!(work, 87);
        assert!(
            reason(check_glyph(cff, &mut read, 0, 2, 7, &mut work))
                .starts_with("decompiling glyph 2 reads more than the 7 operands,")
        );
        assert_eq!(work, 80);
        assert_eq!(check_glyph(cff, &mut read, 0, 3, 7, &mut work).unwrap(), 4);
        assert_eq!(work, 76);
        // Where the work left is less than the steps, it bounds the walk.
        let mut work = 10;
        assert_eq!(
            reason(check_glyph(cff, &mut read, 0, 1, 100, &mut work)),
            format!(
                "checking glyph 1 reads more than the 10 operands, operators and hint mask \
                 bytes left of the {MAX_CHECK_WORK} the checks of one face may read"
            )
        );
        assert_eq!(work, 0);
        assert!(check_glyph(cff, &mut read, 0, 3, 100, &mut work).is_err());
    }

    #[test]
    fn a_glyph_that_fails_after_its_walk_takes_no_steps_either() {
        // Global subroutine 2 reads 12,006,001 operands and operators; glyphs 1, 3 and 4
        // call it once, so two of them together pass the limit of one subset.
        let calls = |subr: u8, times: usize| {
            let mut program = [subr + 32, 29].repeat(times);
            program.push(11);
            program
        };
        let global = [vec![11], calls(0, 2000), calls(1, 2000)];
        let mut glyphs = glyphs_with(1, vec![34, 29, 14]);
        glyphs[3] = vec![34, 29, 14];
        glyphs[4] = vec![34, 29, 14];
        let face = |cff: Vec<u8>, post: Option<Vec<u8>>| {
            let mut font = crate::testing::TestFont::new("Test Sans")
                .with_table(b"CFF ", cff)
                .without_table(b"glyf")
                .without_table(b"loca");
            if let Some(post) = post {
                font = font.with_table(b"post", post);
            }
            crate::font::FontFace::from_bytes(font.build(), 0).unwrap()
        };
        let glyph = crate::font::GlyphId::new;
        // Glyph 3, in Font DICT 1, fails on the font name of its Font DICT, string 393.
        let cid = Spec {
            glyphs: glyphs.clone(),
            global: index_of(&global),
            local: vec![vec![], vec![]],
            fd_select: Some(vec![3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5]),
            last_font_dict_prefix: vec![28, 1, 137, 12, 38],
            ..Spec::default()
        }
        .build();
        let font = face(cid, None);
        assert!(font.check_subset_glyph(glyph(3)).is_err());
        font.check_subset_glyph(glyph(1)).unwrap();
        // Glyph 4 fails on its name in a post table that names four glyphs.
        let sid = programs(&glyphs, &global, &[vec![]], None);
        let font = face(sid, Some(crate::testing::post_naming(4)));
        assert!(font.check_subset_glyph(glyph(4)).is_err());
        font.check_subset_glyph(glyph(1)).unwrap();
    }

    #[test]
    fn checks_of_a_face_share_the_limit_of_a_subset() {
        // Global subroutine 2 reads 12,006,001 operands and operators (see
        // `checks_the_limit_on_the_way_subset_checks`); glyphs 1 and 2 each call it once.
        let calls = |subr: u8, times: usize| {
            let mut program = [subr + 32, 29].repeat(times);
            program.push(11);
            program
        };
        let global = [vec![11], calls(0, 2000), calls(1, 2000)];
        let mut glyphs = glyphs_with(1, vec![34, 29, 14]);
        glyphs[2] = vec![34, 29, 14];
        glyphs[3] = vec![139, 139, 21, 14];
        let cff = programs(&glyphs, &global, &[vec![]], None);
        let font = crate::testing::TestFont::new("Test Sans")
            .with_table(b"CFF ", cff)
            .without_table(b"glyf")
            .without_table(b"loca")
            .build();
        let face = |font: &[u8]| crate::font::FontFace::from_bytes(font.to_vec(), 0).unwrap();
        // Each passes alone. (The subsetter is not run on them: it would take hundreds of
        // megabytes.)
        face(&font)
            .check_subset_glyph(crate::font::GlyphId::new(2))
            .unwrap();
        let face = face(&font);
        face.check_subset_glyph(crate::font::GlyphId::new(1))
            .unwrap();
        // Together they read more than one subset may.
        let err = face
            .check_subset_glyph(crate::font::GlyphId::new(2))
            .unwrap_err();
        assert!(
            matches!(err, Error::TableOverLimit { table: "CFF ", .. }),
            "{err}"
        );
        assert!(
            std::error::Error::source(&err)
                .unwrap()
                .to_string()
                .starts_with("decompiling glyph 2 reads more than the 4771211 operands,"),
            "{err:?}"
        );
        assert!(face.subset([1, 2].map(crate::font::GlyphId::new)).is_err());
        // Glyph 2 failed and took nothing off: glyph 3 passes. Glyph 1 still passes, and so
        // does .notdef.
        face.check_subset_glyph(crate::font::GlyphId::new(3))
            .unwrap();
        face.check_subset_glyph(crate::font::GlyphId::new(1))
            .unwrap();
        face.check_subset_glyph(crate::font::GlyphId::new(0))
            .unwrap();
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
            assert!(subsetter_panics(cff.clone(), &[2, 1]));
        }
        // Numbered in the order given, glyph 2 comes first, and the walk stops there as the
        // subsetter's does, leaving its error to it.
        let refused = subset_glyphs_in_order(cff.clone(), &[2, 1]).unwrap_err();
        assert!(matches!(refused, Error::Subset { .. }), "{refused}");
        if cfg!(debug_assertions) {
            let remapper = subsetter::GlyphRemapper::new_from_glyphs(&[2, 1]);
            assert!(!panics_with(cff, &remapper));
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
        let bytes = |len: usize| index_of(&vec![vec![11]; len]);
        fn subrs(bytes: &[u8]) -> Subroutines<'_> {
            Subroutines::new(index(bytes, 0).unwrap())
        }
        let (b1239, b1240, b33900) = (bytes(1239), bytes(1240), bytes(33_900));
        assert!(subroutine(&subrs(&b1239), -107).is_some());
        assert!(subroutine(&subrs(&b1239), -108).is_none());
        assert!(subroutine(&subrs(&b1240), -1131).is_some());
        assert!(subroutine(&subrs(&b1240), 109).is_none());
        assert!(subroutine(&subrs(&b33900), -32768).is_some());
        assert!(subroutine(&subrs(&b33900), i32::MAX).is_none());
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
