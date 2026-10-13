//! Which characters a face maps to a glyph.

use skrifa::raw::tables::cmap::{Cmap, Cmap4, CmapSubtable, PlatformId};
use skrifa::raw::{FontData, FontRead};

/// The characters a face's character map maps to a glyph, as sorted ranges of code
/// points that neither overlap nor touch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Coverage(Vec<(u32, u32)>);

/// What reading character maps may still cost: a step for each segment, group and
/// glyph-array entry looked at, and the ranges kept.
pub(crate) struct Allowance {
    pub(crate) steps: u64,
    pub(crate) ranges: usize,
}

/// The highest code point.
const LAST: u32 = char::MAX as u32;

impl Coverage {
    /// Nothing at all.
    pub(crate) const NOTHING: Self = Self(Vec::new());

    /// The characters the `cmap` table in `data` maps to a glyph from 1 to
    /// `glyph_count - 1`, through the subtable
    /// [`FontFace::char_map`](crate::FontFace::char_map) uses, as its lookups map them.
    ///
    /// The work is a step per segment or group, plus one per glyph-array entry of a
    /// format 4 subtable, so it grows with the table rather than with the characters it
    /// maps; it is taken from `allowance`. `None` if the table cannot be read, if its
    /// subtable is damaged in a way that makes a lookup disagree with its segments or
    /// groups (out of order or overlapping), or if it would go past `allowance`.
    pub(crate) fn of_cmap(
        data: &[u8],
        glyph_count: u16,
        allowance: &mut Allowance,
    ) -> Option<Self> {
        let cmap = Cmap::read(FontData::new(data)).ok()?;
        let Some(subtable) = chosen_subtable(&cmap) else {
            return Some(Self::NOTHING);
        };
        let glyphs = u64::from(glyph_count);
        let mut ranges = Ranges {
            ranges: Vec::new(),
            max: allowance.ranges,
        };
        match subtable {
            CmapSubtable::Format4(table) => format4(&table, glyphs, allowance, &mut ranges)?,
            CmapSubtable::Format12(table) => {
                let mut previous = None;
                for group in table.groups() {
                    allowance.steps = allowance.steps.checked_sub(1)?;
                    let (start, end) = in_order(
                        &mut previous,
                        group.start_char_code(),
                        group.end_char_code(),
                    )?;
                    // Glyphs follow the code points, modulo 2^32 as the lookup computes them.
                    let base = u64::from(group.start_glyph_id());
                    for (from, to) in valid_offsets(base, 1 << 32, glyphs, u64::from(end - start)) {
                        ranges.push(start, from, to)?;
                    }
                }
            }
            CmapSubtable::Format13(table) => {
                let mut previous = None;
                for group in table.groups() {
                    allowance.steps = allowance.steps.checked_sub(1)?;
                    let (start, end) = in_order(
                        &mut previous,
                        group.start_char_code(),
                        group.end_char_code(),
                    )?;
                    // Every code point of the group maps to the same glyph.
                    if (1..glyphs).contains(&u64::from(group.glyph_id())) {
                        ranges.push(start, 0, u64::from(end - start))?;
                    }
                }
            }
            _ => return Some(Self::NOTHING),
        }
        allowance.ranges -= ranges.ranges.len();
        Some(Self(ranges.ranges))
    }

    /// Whether the face maps `ch`.
    pub(crate) fn contains(&self, ch: char) -> bool {
        let code_point = u32::from(ch);
        // The ranges that start at or before the code point; the last of them is the one
        // that can hold it.
        let before = self.0.partition_point(|&(start, _)| start <= code_point);
        before
            .checked_sub(1)
            .and_then(|last| self.0.get(last))
            .is_some_and(|&(_, end)| code_point <= end)
    }

    /// The number of ranges.
    pub(crate) fn range_count(&self) -> usize {
        self.0.len()
    }
}

/// The subtable `skrifa`'s `Charmap` chooses, and so [`FontFace::char_map`]: the encoding
/// records are searched from the last; a Windows symbol subtable wins over every other,
/// a subtable for the whole of Unicode over one for its Basic Multilingual Plane, and
/// among subtables of the same kind the last. Only formats 4, 12 and 13 are read.
///
/// [`FontFace::char_map`]: crate::FontFace::char_map
fn chosen_subtable<'a>(cmap: &Cmap<'a>) -> Option<CmapSubtable<'a>> {
    let mut chosen: Option<(u8, CmapSubtable<'a>)> = None;
    for record in cmap.encoding_records().iter().rev() {
        let kind = match (record.platform_id(), record.encoding_id()) {
            // Unicode variation sequences: not a map of code points.
            (PlatformId::Unicode, 5) => continue,
            (PlatformId::Windows, 0) => 3,
            (PlatformId::Windows, 10) | (PlatformId::Unicode, 4 | 6) => 2,
            (PlatformId::ISO, _) | (PlatformId::Unicode, _) | (PlatformId::Windows, 1) => 1,
            _ => continue,
        };
        let Ok(subtable) = record.subtable(cmap.offset_data()) else {
            continue;
        };
        let read = matches!(
            subtable,
            CmapSubtable::Format4(_) | CmapSubtable::Format12(_) | CmapSubtable::Format13(_)
        );
        if read && chosen.as_ref().is_none_or(|(best, _)| kind > *best) {
            chosen = Some((kind, subtable));
        }
    }
    chosen.map(|(_, subtable)| subtable)
}

/// The code points of a format 4 subtable.
fn format4(
    table: &Cmap4<'_>,
    glyphs: u64,
    allowance: &mut Allowance,
    ranges: &mut Ranges,
) -> Option<()> {
    let segments = usize::from(table.seg_count_x2() / 2);
    let (starts, ends) = (table.start_code(), table.end_code());
    let (deltas, offsets) = (table.id_delta(), table.id_range_offsets());
    let array = table.glyph_id_array();
    let mut previous = None;
    for segment in 0..segments {
        allowance.steps = allowance.steps.checked_sub(1)?;
        let start = u32::from(starts.get(segment)?.get());
        let end = u32::from(ends.get(segment)?.get());
        let (start, end) = in_order(&mut previous, start, end)?;
        // idDelta is signed; modulo 65536 it is the same as its bits read unsigned.
        let delta = u64::from(deltas.get(segment)?.get() as u16);
        let offset = usize::from(offsets.get(segment)?.get());
        if offset == 0 {
            // The glyph is the code point plus the delta, modulo 65536.
            let base = (u64::from(start) + delta) % (1 << 16);
            for (from, to) in valid_offsets(base, 1 << 16, glyphs, u64::from(end - start)) {
                ranges.push(start, from, to)?;
            }
            continue;
        }
        // The glyph comes from the glyph ID array, at a place that grows with the code
        // point, so the segment ends where the array does.
        for code_point in start..=end {
            let past = usize::try_from(code_point - start).ok()?;
            let at = (offset / 2 + past).saturating_sub(segments - segment);
            let Some(glyph) = array.get(at) else {
                break;
            };
            allowance.steps = allowance.steps.checked_sub(1)?;
            let glyph = u64::from(glyph.get());
            if glyph != 0 && (1..glyphs).contains(&((glyph + delta) % (1 << 16))) {
                ranges.push(code_point, 0, 0)?;
            }
        }
    }
    Some(())
}

/// `start` and `end` of a segment or group, if it comes after `previous` without
/// overlapping it, as the lookup's binary search needs; it becomes `previous`.
fn in_order(previous: &mut Option<u32>, start: u32, end: u32) -> Option<(u32, u32)> {
    if start > end || previous.is_some_and(|previous| previous >= start) {
        return None;
    }
    *previous = Some(end);
    // Nothing past the last code point can be asked for.
    (start <= LAST).then_some((start, end.min(LAST)))
}

/// The offsets `k` from 0 to `last` for which `(base + k) % modulus`, a glyph, is from 1
/// to `glyphs - 1`: at most two ranges, as the glyphs can wrap around the modulus.
fn valid_offsets(
    base: u64,
    modulus: u64,
    glyphs: u64,
    last: u64,
) -> impl Iterator<Item = (u64, u64)> {
    // The offset at which the glyph is 1, and the number of valid glyphs from there.
    let first = (modulus + 1 - base % modulus) % modulus;
    let count = glyphs.saturating_sub(1);
    let (head, tail) = if count == 0 {
        (None, None)
    } else if first + count <= modulus {
        (Some((first, first + count - 1)), None)
    } else {
        (
            Some((first, modulus - 1)),
            Some((0, first + count - 1 - modulus)),
        )
    };
    [tail, head]
        .into_iter()
        .flatten()
        .filter_map(move |(from, to)| (from <= last).then_some((from, to.min(last))))
}

/// Ranges of code points being collected, in order, up to `max` of them.
struct Ranges {
    ranges: Vec<(u32, u32)>,
    max: usize,
}

impl Ranges {
    /// Adds `start + from` to `start + to`; `None` if that makes more than `max` ranges.
    fn push(&mut self, start: u32, from: u64, to: u64) -> Option<()> {
        let from = u32::try_from(u64::from(start) + from).ok()?;
        let to = u32::try_from(u64::from(start) + to).ok()?;
        if let Some((_, last)) = self.ranges.last_mut() {
            if from <= last.saturating_add(1) {
                *last = (*last).max(to);
                return Some(());
            }
        }
        if self.ranges.len() >= self.max {
            return None;
        }
        self.ranges.push((from, to));
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_offsets_whose_glyphs_exist() {
        let offsets = |base, modulus, glyphs, last| -> Vec<(u64, u64)> {
            valid_offsets(base, modulus, glyphs, last).collect()
        };
        // Glyph = 10 + k; glyphs 1 to 4 exist: none from offset 0 on.
        assert_eq!(offsets(10, 1 << 16, 5, 100), []);
        // Glyph = 0 + k: offsets 1 to 4.
        assert_eq!(offsets(0, 1 << 16, 5, 100), [(1, 4)]);
        assert_eq!(offsets(0, 1 << 16, 5, 2), [(1, 2)]);
        // Glyph = 65534 + k, modulo 65536: 65534, 65535, 0, 1, 2…: offsets 3 to 6.
        assert_eq!(offsets(65534, 1 << 16, 5, 100), [(3, 6)]);
        // Glyph = 2 + k: k = 0 to 2 give 2 to 4, and past the wrap k = 65535 gives 1.
        assert_eq!(offsets(2, 1 << 16, 5, 1 << 16), [(0, 2), (65535, 65535)]);
        // Format 12 wraps modulo 2^32.
        assert_eq!(offsets(u64::from(u32::MAX), 1 << 32, 3, 10), [(2, 3)]);
        // A font with only glyph 0 maps nothing.
        assert_eq!(offsets(0, 1 << 16, 1, 100), []);
    }

    #[test]
    fn keeps_ranges_in_order_and_merges_neighbours() {
        let mut ranges = Ranges {
            ranges: Vec::new(),
            max: 2,
        };
        ranges.push(0x41, 0, 1).unwrap();
        ranges.push(0x43, 0, 0).unwrap();
        ranges.push(0x100, 0, 0).unwrap();
        assert_eq!(ranges.ranges, [(0x41, 0x43), (0x100, 0x100)]);
        assert_eq!(ranges.push(0x200, 0, 0), None);
        let coverage = Coverage(ranges.ranges);
        for ch in ['A', 'B', 'C', '\u{100}'] {
            assert!(coverage.contains(ch), "{ch:?}");
        }
        for ch in ['@', 'D', '\u{FF}', '\u{101}', '\0', char::MAX] {
            assert!(!coverage.contains(ch), "{ch:?}");
        }
        assert!(!Coverage::NOTHING.contains('A'));
        assert_eq!(Coverage::NOTHING.range_count(), 0);
    }

    #[test]
    fn segments_and_groups_must_be_in_order() {
        let mut previous = None;
        assert_eq!(in_order(&mut previous, 0x41, 0x44), Some((0x41, 0x44)));
        assert_eq!(in_order(&mut previous, 0x44, 0x50), None);
        let mut previous = None;
        assert_eq!(in_order(&mut previous, 0x50, 0x41), None);
        let mut previous = Some(0x10);
        assert_eq!(in_order(&mut previous, 0x11, 0x11_0005), Some((0x11, LAST)));
    }

    #[test]
    fn spends_and_respects_the_allowance() {
        let cmap = crate::testing::cmap_groups(13, &[(0x41, 0x41, 1), (0x43, 0x43, 1)]);
        let mut spare = Allowance {
            steps: 100,
            ranges: 100,
        };
        let coverage = Coverage::of_cmap(&cmap, 5, &mut spare).unwrap();
        assert_eq!(coverage.0, [(0x41, 0x41), (0x43, 0x43)]);
        assert_eq!((spare.steps, spare.ranges), (98, 98));
        let mut short = Allowance {
            steps: 1,
            ranges: 10,
        };
        assert_eq!(Coverage::of_cmap(&cmap, 5, &mut short), None);
        let mut narrow = Allowance {
            steps: 10,
            ranges: 1,
        };
        assert_eq!(Coverage::of_cmap(&cmap, 5, &mut narrow), None);
    }
}
