//! How much work HarfRust does the first time it shapes with a face's layout tables,
//! counted before the face is given to it, so that a damaged face cannot hold the layout
//! up for seconds (issue #255).
//!
//! HarfRust (pinned at 0.14.0) reads `GSUB`, `GPOS` and `GDEF` lazily, and checks nothing
//! before: HarfBuzz checks a table within a budget and drops it when the check takes more,
//! HarfRust has no such check. The first time HarfRust applies a lookup, it reads every
//! subtable of it (`LookupInfo::new`, `SubtableInfo::new`) and walks what each subtable's
//! offsets lead to: the coverage, every pair-value record of every pair set, every rule of
//! every rule set and, for each rule of a class-based context, every glyph of its class
//! definition. When it first shapes, it also walks the coverage of every mark glyph set of
//! `GDEF` (`GdefCache::new`). Lookups, subtables, pair sets and coverages may be shared
//! through offsets, and HarfRust walks a shared one again for each reference, so a few
//! kilobytes can ask for billions of steps. And every subtable a glyph may match costs
//! time again for each glyph of each run.
//!
//! This module counts those walks as HarfRust 0.14.0 makes them, with three limits per
//! table: references, one per lookup-list entry and per subtable of it, at most
//! [`MAX_REFERENCES`] or one per [`BYTES_PER_REFERENCE`] bytes; work, one per element
//! walked, at most [`MAX_WORK`] or [`WORK_PER_BYTE`] per byte; and the references whose
//! first coverage holds one glyph, at most [`MAX_REFERENCES_PER_GLYPH`] for any glyph. It
//! counts every lookup, not only those a plan applies. The cost of each distinct structure is worked out once, and
//! counting stops at a limit, so the check takes time in proportion to the table and the
//! limits. When HarfRust is upgraded, the walks counted here must be checked against it
//! again.
//!
//! The limits were calibrated on 755 layout tables of 435 font files (`tests/fonts`, a
//! Linux and a Windows installation, CJK and variable collections included): the most
//! references were 5,740 in one table and 0.1 per byte; the most work 1.11 million in one
//! table and 5.2 per byte, in a table of 8 KB; the most references whose coverage holds
//! one glyph 812. Faces built at the limits take at most 0.25 s to read the first time,
//! and at most about a millisecond per glyph to apply (most of it subtables whose digest
//! a glyph matches without being in its coverage), which is HarfRust's own cost for so
//! many subtables.

use std::collections::HashMap;

/// The most subtable references a layout table may hold, whatever its size.
pub(crate) const MAX_REFERENCES: u64 = 32_768;

/// A layout table larger than this many bytes times [`MAX_REFERENCES`] may hold one
/// reference per this many bytes.
pub(crate) const BYTES_PER_REFERENCE: u64 = 16;

/// The most subtable references of a layout table whose first coverage holds any one
/// glyph: HarfRust may try each of them for each such glyph of every run.
pub(crate) const MAX_REFERENCES_PER_GLYPH: u64 = 4_096;

/// The most work the walks of a layout table may take, whatever its size.
pub(crate) const MAX_WORK: u64 = 1 << 23;

/// A layout table larger than [`MAX_WORK`] divided by this may take this much work per
/// byte.
pub(crate) const WORK_PER_BYTE: u64 = 4;

/// The most ligature sets and ligatures HarfRust walks in a ligature substitution before
/// it gives up and takes every glyph (`MAX_LIGATURE_CACHE_WORK` in harfrust 0.14.0).
const MAX_LIGATURE_WORK: u64 = 16_384;

/// The widest span of glyphs HarfRust keeps a mark glyph set of as a page of bits
/// (`MARK_GLYPH_SET_PAGE_BITS` in harfrust 0.14.0).
const MARK_SET_PAGE: u64 = 512;

/// Whether HarfRust may be given face `index` of the font file `data`: its `GSUB`,
/// `GPOS` and `GDEF` tables each appear at most once, and the work HarfRust does with each
/// of them stays within the limits. A file that is no font, or a face it does not have, is
/// left to HarfRust, which does not read it either.
pub(crate) fn within_budget(data: &[u8], index: u32) -> bool {
    let Some(directory) = table_directory(data, index) else {
        return true;
    };
    let tables = u16_at(data, directory.saturating_add(4)).unwrap_or(0);
    let mut seen: Vec<&[u8; 4]> = Vec::new();
    for i in 0..usize::from(tables) {
        let record = directory.saturating_add(12 + 16 * i);
        let Some(tag) = data
            .get(record..record.saturating_add(4))
            .and_then(|tag| <&[u8; 4]>::try_from(tag).ok())
        else {
            break;
        };
        if !matches!(tag, b"GSUB" | b"GPOS" | b"GDEF") {
            continue;
        }
        // A tag names one table: which of several records HarfRust reads is not defined.
        if seen.contains(&tag) {
            return false;
        }
        seen.push(tag);
        let table = u32_at(data, record.saturating_add(8))
            .zip(u32_at(data, record.saturating_add(12)))
            .and_then(|(offset, length)| {
                let offset = usize::try_from(offset).ok()?;
                data.get(offset..offset.checked_add(usize::try_from(length).ok()?)?)
            });
        // A table the file does not hold, HarfRust does not read either.
        let Some(table) = table else {
            continue;
        };
        let mut counter = Counter::new(table, tag == b"GSUB");
        let within = if tag == b"GDEF" {
            counter.mark_sets()
        } else {
            counter.lookups()
        };
        if !within {
            return false;
        }
    }
    true
}

/// Where the table directory of face `index` of the font file `data` starts; `None` for a
/// face a collection does not have.
fn table_directory(data: &[u8], index: u32) -> Option<usize> {
    if data.get(..4) != Some(b"ttcf".as_slice()) {
        return (index == 0).then_some(0);
    }
    if index >= u32_at(data, 8)? {
        return None;
    }
    let entry = usize::try_from(index)
        .ok()?
        .checked_mul(4)?
        .checked_add(12)?;
    usize::try_from(u32_at(data, entry)?).ok()
}

/// Counts the walks of one layout table.
struct Counter<'a> {
    table: &'a [u8],
    gsub: bool,
    references: u64,
    work: u64,
    max_references: u64,
    max_work: u64,
    /// The work of each subtable, by lookup type and place.
    subtables: HashMap<(u16, usize), u64>,
    /// The glyphs of each class definition, by place.
    class_defs: HashMap<usize, u64>,
    /// The subtable references that test each coverage first, by its place.
    first_coverages: HashMap<usize, u64>,
}

impl<'a> Counter<'a> {
    fn new(table: &'a [u8], gsub: bool) -> Self {
        let len = u64::try_from(table.len()).unwrap_or(u64::MAX);
        Self {
            table,
            gsub,
            references: 0,
            work: 0,
            max_references: MAX_REFERENCES.max(len / BYTES_PER_REFERENCE),
            max_work: MAX_WORK.max(len.saturating_mul(WORK_PER_BYTE)),
            subtables: HashMap::new(),
            class_defs: HashMap::new(),
            first_coverages: HashMap::new(),
        }
    }

    /// Adds a reference that takes `work`; returns whether the table is still within its
    /// limits.
    fn add(&mut self, references: u64, work: u64) -> bool {
        self.references = self.references.saturating_add(references);
        self.work = self.work.saturating_add(work);
        self.references <= self.max_references && self.work <= self.max_work
    }

    /// The lookups of a `GSUB` or `GPOS` table: each entry of the lookup list, and each
    /// subtable of its lookup. Returns whether they stay within the limits.
    fn lookups(&mut self) -> bool {
        let Some(list) = self.offset(0, 8) else {
            return true;
        };
        let count = u16_at(self.table, list).unwrap_or(0);
        for entry in 0..usize::from(count) {
            let Some(lookup) = self.offset(list, 2 + 2 * entry) else {
                continue;
            };
            if !self.add(1, 0) {
                return false;
            }
            let kind = u16_at(self.table, lookup).unwrap_or(0);
            for subtable in self.offsets(lookup, 4, 6) {
                let work = self.subtable(kind, subtable, false);
                if !self.add(1, work) {
                    return false;
                }
                if let Some(coverage) = self.first_coverage(kind, subtable, false) {
                    *self.first_coverages.entry(coverage).or_default() += 1;
                }
            }
        }
        // No glyph can be held by more references than there are.
        let references: u64 = self.first_coverages.values().sum();
        references <= MAX_REFERENCES_PER_GLYPH
            || self.most_references_per_glyph() <= MAX_REFERENCES_PER_GLYPH
    }

    /// The coverage HarfRust tests a glyph against first for the subtable at `at` of
    /// lookup type `kind`: the one its digest comes from.
    fn first_coverage(&self, kind: u16, at: usize, extended: bool) -> Option<usize> {
        let extension = if self.gsub { 7 } else { 9 };
        if kind == extension {
            let target = u16_at(self.table, at.saturating_add(2))?;
            let offset = usize::try_from(u32_at(self.table, at.saturating_add(4))?).ok()?;
            return (target != extension && !extended)
                .then(|| self.first_coverage(target, at.checked_add(offset)?, true))
                .flatten();
        }
        match (self.gsub, kind, u16_at(self.table, at)?) {
            (true, 5, 3) | (false, 7, 3) => self.offset(at, 6),
            (true, 6, 3) | (false, 8, 3) => {
                let backtrack = usize::from(u16_at(self.table, at.saturating_add(2))?);
                self.offset(at, 6 + 2 * backtrack)
            }
            _ => self.offset(at, 2),
        }
    }

    /// The most subtable references whose first coverage holds any one glyph. Only called
    /// within the work limit, which counts every coverage this walks.
    fn most_references_per_glyph(&self) -> u64 {
        let mut listed = vec![0_u64; 1 << 16];
        // Ranges add their count at their first glyph and take it back after their last.
        let mut from = vec![0_u64; (1 << 16) + 1];
        let mut until = vec![0_u64; (1 << 16) + 1];
        for (&at, &references) in &self.first_coverages {
            let count = usize::from(u16_at(self.table, at.saturating_add(2)).unwrap_or(0));
            match u16_at(self.table, at) {
                Some(1) if self.fits(at, 4 + 2 * count) => {
                    for i in 0..count {
                        let glyph = usize::from(u16_at(self.table, at + 4 + 2 * i).unwrap_or(0));
                        if let Some(slot) = listed.get_mut(glyph) {
                            *slot = slot.saturating_add(references);
                        }
                    }
                }
                Some(2) if self.fits(at, 4 + 6 * count) => {
                    for i in 0..count {
                        let record = at + 4 + 6 * i;
                        let start = usize::from(u16_at(self.table, record).unwrap_or(0));
                        let end = usize::from(u16_at(self.table, record + 2).unwrap_or(0));
                        if end >= start {
                            if let Some(slot) = from.get_mut(start) {
                                *slot = slot.saturating_add(references);
                            }
                            if let Some(slot) = until.get_mut(end + 1) {
                                *slot = slot.saturating_add(references);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let mut in_ranges: u64 = 0;
        let mut most = 0;
        for glyph in 0..listed.len() {
            in_ranges = in_ranges
                .saturating_add(from.get(glyph).copied().unwrap_or(0))
                .saturating_sub(until.get(glyph).copied().unwrap_or(0));
            let here = in_ranges.saturating_add(listed.get(glyph).copied().unwrap_or(0));
            most = most.max(here);
        }
        most
    }

    /// The mark glyph sets of a `GDEF` table (version 1.2 and later): the coverage of each.
    /// Returns whether they stay within the limits.
    fn mark_sets(&mut self) -> bool {
        if u32_at(self.table, 0).is_none_or(|version| version < 0x0001_0002) {
            return true;
        }
        // The one nullable offset of the header this counts.
        let Some(sets) = self.offset(0, 12).filter(|&sets| sets != 0) else {
            return true;
        };
        if u16_at(self.table, sets) != Some(1) {
            return true;
        }
        let count = usize::from(u16_at(self.table, sets.saturating_add(2)).unwrap_or(0));
        if !self.fits(sets, 4 + 4 * count) {
            return true;
        }
        for set in 0..count {
            let coverage = u32_at(self.table, sets + 4 + 4 * set)
                .and_then(|offset| usize::try_from(offset).ok())
                .and_then(|offset| sets.checked_add(offset));
            let work = coverage.map_or(0, |coverage| self.mark_set(coverage));
            if !self.add(0, 1 + work) {
                return false;
            }
        }
        true
    }

    /// The work of a mark glyph set's coverage at `at`: a page of bits when its first and
    /// last glyphs are near, every glyph of each range of it then; else a digest of it.
    fn mark_set(&self, at: usize) -> u64 {
        let format = u16_at(self.table, at);
        let count = usize::from(u16_at(self.table, at.saturating_add(2)).unwrap_or(0));
        match format {
            Some(1) if self.fits(at, 4 + 2 * count) => count as u64,
            Some(2) if self.fits(at, 4 + 6 * count) => (0..count)
                .map(|range| {
                    let record = at + 4 + 6 * range;
                    let span = self.span(record);
                    1 + span.min(MARK_SET_PAGE)
                })
                .sum(),
            _ => 0,
        }
    }

    /// The work of the subtable at `at` of lookup type `kind`: what HarfRust walks the
    /// first time it reads it. An extension subtable is the one it leads to.
    fn subtable(&mut self, kind: u16, at: usize, extended: bool) -> u64 {
        if let Some(&work) = self.subtables.get(&(kind, at)) {
            return work;
        }
        let extension = if self.gsub { 7 } else { 9 };
        let format = u16_at(self.table, at).unwrap_or(0);
        let work = if kind == extension {
            let target = u16_at(self.table, at.saturating_add(2));
            let offset = u32_at(self.table, at.saturating_add(4))
                .and_then(|offset| usize::try_from(offset).ok())
                .and_then(|offset| at.checked_add(offset));
            match (target, offset) {
                (Some(target), Some(offset)) if target != extension && !extended => {
                    1 + self.subtable(target, offset, true)
                }
                _ => 0,
            }
        } else {
            match (self.gsub, kind, format) {
                // Single, multiple, alternate and reverse chained substitution; single
                // positioning: the coverage's digest.
                (true, 1 | 2 | 3 | 8, _) | (false, 1, _) => self.coverage_at(at, 2),
                // Ligatures: the second glyphs, twice, up to HarfRust's own limit.
                (true, 4, _) => self.coverage_at(at, 2) + 2 * self.ligatures(at),
                // Pairs of glyphs: every pair-value record of every pair set, twice.
                (false, 2, 1) => {
                    let pairs: u64 = self
                        .offsets(at, 8, 10)
                        .into_iter()
                        .map(|set| 1 + u64::from(u16_at(self.table, set).unwrap_or(0)))
                        .sum();
                    self.coverage_at(at, 2)
                        .saturating_add(pairs.saturating_mul(2))
                }
                (false, 2, _) => self.coverage_at(at, 2),
                // Cursive attachment: its coverage, twice.
                (false, 3, _) => 2 * self.coverage_at(at, 2),
                // Mark attachment: both coverages.
                (false, 4..=6, _) => self.coverage_at(at, 2) + self.coverage_at(at, 4),
                // Contexts by glyphs: every rule.
                (true, 5, 1) | (false, 7, 1) | (true, 6, 1) | (false, 8, 1) => {
                    self.coverage_at(at, 2) + self.rules(at, 4, 6)
                }
                // Contexts by classes: every rule, the class definition's glyphs for each,
                // and every rule again for the rule sets' digests.
                (true, 5, 2) | (false, 7, 2) => {
                    let class_def = self.offset(at, 4).map_or(0, |at| self.class_def(at));
                    self.class_rules(at, 6, 8, class_def)
                }
                (true, 6, 2) | (false, 8, 2) => {
                    let input = self.offset(at, 6).map_or(0, |at| self.class_def(at));
                    let ahead = self.offset(at, 8).map_or(0, |at| self.class_def(at));
                    self.class_rules(at, 10, 12, input.max(ahead))
                }
                // Contexts by coverages: the first two.
                (true, 5, 3) | (false, 7, 3) => self
                    .offsets(at, 2, 6)
                    .into_iter()
                    .take(2)
                    .map(|coverage| self.coverage(coverage))
                    .sum(),
                (true, 6, 3) | (false, 8, 3) => self.chained_coverages(at),
                _ => 0,
            }
        };
        self.subtables.insert((kind, at), work);
        work
    }

    /// The work of a subtable at `at` with context rules by classes, its rule sets'
    /// count at `count_at` and their offsets from `first`, each rule walking
    /// `class_glyphs` glyphs; and its coverage.
    fn class_rules(&mut self, at: usize, count_at: usize, first: usize, class_glyphs: u64) -> u64 {
        let rules = self.rules(at, count_at, first);
        self.coverage_at(at, 2)
            .saturating_add(rules.saturating_mul(class_glyphs.saturating_add(2)))
    }

    /// The rule sets of a context subtable at `at` and the rules in them. A rule set's
    /// offset may be null, and then there is none.
    fn rules(&self, at: usize, count_at: usize, first: usize) -> u64 {
        self.offsets(at, count_at, first)
            .into_iter()
            .filter(|&set| set != at)
            .map(|set| 1 + u64::from(u16_at(self.table, set).unwrap_or(0)))
            .sum()
    }

    /// The ligature sets and ligatures of a ligature substitution at `at` that HarfRust
    /// walks, up to its limit.
    fn ligatures(&self, at: usize) -> u64 {
        let sets = self.offsets(at, 4, 6);
        let mut work = sets.len() as u64;
        for set in sets {
            if work > MAX_LIGATURE_WORK {
                break;
            }
            work += u64::from(u16_at(self.table, set).unwrap_or(0));
        }
        work.min(MAX_LIGATURE_WORK + 1)
    }

    /// The input and look-ahead coverages a chained context by coverages at `at` takes its
    /// digests from.
    fn chained_coverages(&self, at: usize) -> u64 {
        let backtrack = usize::from(u16_at(self.table, at.saturating_add(2)).unwrap_or(0));
        let input_count = 4 + 2 * backtrack;
        let input = self.offsets(at, input_count, input_count + 2);
        let mut work: u64 = input.iter().take(2).map(|&c| self.coverage(c)).sum();
        if input.len() <= 1 {
            let ahead_count = input_count + 2 + 2 * input.len();
            work += self
                .offsets(at, ahead_count, ahead_count + 2)
                .first()
                .map_or(0, |&c| self.coverage(c));
        }
        work
    }

    /// The work of the coverage an offset at `at + field` leads to.
    fn coverage_at(&self, at: usize, field: usize) -> u64 {
        self.offset(at, field)
            .map_or(0, |coverage| self.coverage(coverage))
    }

    /// The work of a coverage's digest: one step per glyph, or per range.
    fn coverage(&self, at: usize) -> u64 {
        let count = usize::from(u16_at(self.table, at.saturating_add(2)).unwrap_or(0));
        match u16_at(self.table, at) {
            Some(1) if self.fits(at, 4 + 2 * count) => count as u64,
            Some(2) if self.fits(at, 4 + 6 * count) => count as u64,
            _ => 0,
        }
    }

    /// The glyphs of the class definition at `at`, which HarfRust walks one by one.
    fn class_def(&mut self, at: usize) -> u64 {
        if let Some(&glyphs) = self.class_defs.get(&at) {
            return glyphs;
        }
        let glyphs = match u16_at(self.table, at) {
            Some(1) => {
                let count = usize::from(u16_at(self.table, at.saturating_add(4)).unwrap_or(0));
                if self.fits(at, 6 + 2 * count) {
                    count as u64
                } else {
                    0
                }
            }
            Some(2) => {
                let count = usize::from(u16_at(self.table, at.saturating_add(2)).unwrap_or(0));
                if self.fits(at, 4 + 6 * count) {
                    (0..count).map(|range| self.span(at + 4 + 6 * range)).sum()
                } else {
                    0
                }
            }
            _ => 0,
        };
        self.class_defs.insert(at, glyphs);
        glyphs
    }

    /// The glyphs from the first to the last glyph of the range record at `at`.
    fn span(&self, at: usize) -> u64 {
        let start = u16_at(self.table, at).unwrap_or(0);
        let end = u16_at(self.table, at.saturating_add(2)).unwrap_or(0);
        u64::from(end.saturating_sub(start)) + u64::from(end >= start)
    }

    /// The structure an `Offset16` at `at + field` leads to, from `at`; `None` when the
    /// table does not hold the offset. Offset 0 leads to `at` itself: read-fonts reads
    /// most offsets of layout tables as never null, and so does HarfRust.
    fn offset(&self, at: usize, field: usize) -> Option<usize> {
        let offset = u16_at(self.table, at.checked_add(field)?)?;
        Some(at.saturating_add(usize::from(offset)))
    }

    /// The structures the array of `Offset16` at `at + first` leads to, from `at`, its
    /// count at `at + count_at`; none when the array does not fit in the table.
    fn offsets(&self, at: usize, count_at: usize, first: usize) -> Vec<usize> {
        let count = usize::from(u16_at(self.table, at.saturating_add(count_at)).unwrap_or(0));
        if !self.fits(at, first + 2 * count) {
            return Vec::new();
        }
        (0..count)
            .filter_map(|i| u16_at(self.table, at + first + 2 * i))
            .map(|offset| at + usize::from(offset))
            .collect()
    }

    /// Whether `len` bytes from `at` are in the table.
    fn fits(&self, at: usize, len: usize) -> bool {
        at.checked_add(len)
            .is_some_and(|end| end <= self.table.len())
    }
}

/// The big-endian `u16` at byte `at` of `data`.
fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    let bytes = data.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes(bytes.try_into().ok()?))
}

/// The big-endian `u32` at byte `at` of `data`.
fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{collection, minimal, with_table};

    fn be(values: &[u16]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect()
    }

    /// A `GSUB` or `GPOS` table without scripts or features whose lookup list names one
    /// lookup of type `kind` `entries` times; the lookup refers `subtables` times to
    /// `subtable`, which follows it, or, when the offsets cannot reach past their own
    /// array, to offset 0: the lookup itself.
    fn shared(kind: u16, entries: u16, subtables: u16, subtable: &[u8]) -> Vec<u8> {
        // Version 1.0; the script list at 10, the feature list at 12, both empty; the
        // lookup list at 14.
        let mut table = be(&[1, 0, 10, 12, 14, 0, 0, entries]);
        let lookup = u16::try_from(2 + 2 * u32::from(entries)).unwrap();
        for _ in 0..entries {
            table.extend(be(&[lookup]));
        }
        table.extend(be(&[kind, 0, subtables]));
        let after = u16::try_from(6 + 2 * u32::from(subtables)).unwrap_or(0);
        for _ in 0..subtables {
            table.extend(be(&[after]));
        }
        table.extend(subtable);
        table
    }

    /// A coverage of glyphs 0 to `glyphs` − 1, as a list.
    fn coverage(glyphs: u16) -> Vec<u8> {
        let mut table = be(&[1, glyphs]);
        table.extend(be(&(0..glyphs).collect::<Vec<_>>()));
        table
    }

    /// A single positioning (format 1, no value) of the glyphs of `coverage`.
    fn single(glyphs: u16) -> Vec<u8> {
        let mut table = be(&[1, 6, 0]);
        table.extend(coverage(glyphs));
        table
    }

    /// A pair positioning (format 1) of glyph 0 whose `sets` pair-set offsets all lead to
    /// one pair set of `pairs` records.
    fn pairs(sets: u16, pairs: u16) -> Vec<u8> {
        let coverage_at = 10 + 2 * sets;
        let mut table = be(&[1, coverage_at, 4, 0, sets]);
        for _ in 0..sets {
            table.extend(be(&[coverage_at + 6]));
        }
        table.extend(coverage(1));
        table.extend(be(&[pairs]));
        for glyph in 0..pairs {
            table.extend(be(&[glyph, 0]));
        }
        table
    }

    fn within(tag: &[u8; 4], table: &[u8]) -> bool {
        within_budget(&with_table(minimal(), tag, table), 0)
    }

    #[test]
    fn references_are_limited_whatever_they_lead_to() {
        // One lookup entry and 32,767 references to one subtable: 32,768 references.
        assert!(within(b"GPOS", &shared(1, 1, 32_767, &single(1))));
        assert!(!within(b"GPOS", &shared(1, 1, 32_768, &single(1))));
        // The shape of issue #255's input: many entries for one lookup of many subtables.
        assert!(!within(b"GPOS", &shared(1, 1_000, 1_000, &single(1))));
        assert!(!within(b"GSUB", &shared(1, 1_000, 1_000, &single(1))));
        // The same in a table of 16 bytes per reference may hold more.
        let mut large = shared(1, 1, 32_768, &single(1));
        large.resize(16 * 32_769, 0);
        assert!(within(b"GPOS", &large));
    }

    #[test]
    fn every_pair_of_shared_pair_sets_is_work() {
        // 1 + 2 × 4,095 × 1,024 = 8,386,561 steps, within 2^23; one set more is not.
        assert!(within(b"GPOS", &shared(2, 1, 1, &pairs(4_095, 1_023))));
        assert!(!within(b"GPOS", &shared(2, 1, 1, &pairs(4_096, 1_023))));
        // Behind an extension (GPOS type 9), one step more for the extension itself.
        let extension = |inner: Vec<u8>| {
            let mut table = be(&[1, 2]);
            table.extend(8u32.to_be_bytes());
            table.extend(inner);
            shared(9, 1, 1, &table)
        };
        assert!(within(b"GPOS", &extension(pairs(4_095, 1_023))));
        assert!(!within(b"GPOS", &extension(pairs(4_096, 1_023))));
    }

    #[test]
    fn a_shared_coverage_is_work_for_each_reference() {
        // 279 × 30,000 = 8,370,000 steps; 280 × 30,000 = 8,400,000 is too many.
        assert!(within(b"GPOS", &shared(1, 1, 279, &single(30_000))));
        assert!(!within(b"GPOS", &shared(1, 1, 280, &single(30_000))));
        assert!(!within(b"GSUB", &shared(1, 1, 280, &single(30_000))));
    }

    #[test]
    fn each_rule_by_classes_walks_the_class_definition() {
        // A context by classes (GSUB 5, format 2) whose one rule set lists one rule
        // `rules` times, with a class definition of one range of 65,535 glyphs: each
        // rule walks it, (1 + rules) × (65,535 + 2) steps.
        let context = |rules: u16| {
            // Coverage at 10, class definition at 16, the rule set at 26.
            let mut table = be(&[2, 10, 16, 1, 26]);
            table.extend(coverage(1));
            table.extend(be(&[2, 1, 0, 65_534, 1]));
            table.extend(be(&[rules]));
            for _ in 0..rules {
                table.extend(be(&[2 + 2 * rules]));
            }
            // A rule of one glyph and no lookups.
            table.extend(be(&[1, 0]));
            table
        };
        assert!(within(b"GSUB", &shared(5, 1, 1, &context(126))));
        assert!(!within(b"GSUB", &shared(5, 1, 1, &context(127))));
    }

    #[test]
    fn contexts_by_coverages_walk_their_first_coverages() {
        // GSUB 5 format 3: two coverages, both one list of 30,000 glyphs: 60,000 steps.
        let mut context = be(&[3, 2, 0, 10, 10]);
        context.extend(coverage(30_000));
        assert!(within(b"GSUB", &shared(5, 1, 139, &context)));
        assert!(!within(b"GSUB", &shared(5, 1, 140, &context)));
        // GSUB 6 format 3: no backtrack, one input coverage, then the look-ahead's first.
        let mut chained = be(&[3, 0, 1, 14, 1, 14, 0]);
        chained.extend(coverage(30_000));
        assert!(within(b"GSUB", &shared(6, 1, 139, &chained)));
        assert!(!within(b"GSUB", &shared(6, 1, 140, &chained)));
    }

    #[test]
    fn references_whose_coverage_holds_one_glyph_are_limited() {
        // A pair positioning of glyph 2 alone, `references` times.
        let pair = pairs(1, 1);
        assert!(within(b"GPOS", &shared(2, 1, 4_096, &pair)));
        assert!(!within(b"GPOS", &shared(2, 1, 4_097, &pair)));
        // Over several lookup entries, and with a coverage of one range, glyphs 0 to 9.
        let mut ranged = be(&[1, 6, 0]);
        ranged.extend(be(&[2, 1, 0, 9, 0]));
        assert!(within(b"GPOS", &shared(1, 4, 1_024, &ranged)));
        assert!(!within(b"GPOS", &shared(1, 4, 1_025, &ranged)));
        // Ranges that do not overlap do not add up.
        let mut apart = be(&[1, 6, 0]);
        apart.extend(be(&[2, 2, 0, 9, 0, 10, 19, 10]));
        assert!(within(b"GPOS", &shared(1, 4, 1_024, &apart)));
        assert!(!within(b"GPOS", &shared(1, 4, 1_025, &apart)));
        // Behind an extension, its first coverage is the subtable's.
        let mut extended = be(&[1, 2]);
        extended.extend(8u32.to_be_bytes());
        extended.extend(pair.clone());
        assert!(!within(b"GPOS", &shared(9, 1, 4_097, &extended)));
        // Contexts by coverages: the first input coverage.
        let mut context = be(&[3, 1, 0, 8]);
        context.extend(coverage(3));
        assert!(within(b"GSUB", &shared(5, 1, 4_096, &context)));
        assert!(!within(b"GSUB", &shared(5, 1, 4_097, &context)));
        // A chained one: past its backtrack coverage, which holds glyph 3000 alone.
        let mut chained = be(&[3, 1, 14, 1, 20, 0, 0]);
        chained.extend(be(&[1, 1, 3_000]));
        chained.extend(coverage(3));
        assert!(within(b"GSUB", &shared(6, 1, 4_096, &chained)));
        assert!(!within(b"GSUB", &shared(6, 1, 4_097, &chained)));
    }

    #[test]
    fn ligatures_are_work_up_to_harfrust_s_own_limit() {
        // A ligature substitution whose 20,000 ligature-set offsets lead to one set of
        // 20,000 ligatures: HarfRust stops after 16,384 sets and ligatures, twice.
        let sets: u16 = 20_000;
        let set_at = 6 + 2 * sets + 6;
        let mut table = be(&[1, 6 + 2 * sets, sets]);
        for _ in 0..sets {
            table.extend(be(&[set_at]));
        }
        table.extend(coverage(1));
        table.extend(be(&[sets]));
        for _ in 0..sets {
            table.extend(be(&[2 + 2 * sets]));
        }
        // A ligature of glyph 1 from glyph 0 and glyph 1.
        table.extend(be(&[1, 2, 1]));
        // (1 + 2 × 16,385) × 256 = 8,389,376 is past the limit; 255 references are not.
        assert!(within(b"GSUB", &shared(4, 1, 255, &table)));
        assert!(!within(b"GSUB", &shared(4, 1, 256, &table)));
    }

    #[test]
    fn mark_glyph_sets_of_gdef_are_work() {
        // GDEF 1.2 whose `sets` mark glyph sets all lead to one coverage of 30,000 glyphs.
        let gdef = |sets: u16| {
            let mut table = be(&[1, 2, 0, 0, 0, 0, 14, 1, sets]);
            for _ in 0..sets {
                table.extend((4 + 4 * u32::from(sets)).to_be_bytes());
            }
            table.extend(coverage(30_000));
            table
        };
        assert!(within(b"GDEF", &gdef(279)));
        assert!(!within(b"GDEF", &gdef(280)));
        // GDEF 1.0 has no mark glyph sets.
        let mut old = gdef(280);
        old[3] = 0;
        assert!(within(b"GDEF", &old));
    }

    #[test]
    fn a_layout_table_named_twice_refuses_the_face() {
        let small = shared(1, 1, 1, &single(1));
        let font = with_table(minimal(), b"GPOS", &small);
        assert!(within_budget(&font, 0));
        assert!(!within_budget(&with_table(font, b"GPOS", &small), 0));
    }

    #[test]
    fn a_table_is_as_long_as_its_record_says() {
        // The subtable offsets of the lookup end at the last 2 bytes of the table: cut
        // there, the lookup cannot be read, though the bytes follow in the file.
        let table = shared(1, 1, 40_000, &[]);
        let font = with_table(minimal(), b"GPOS", &table);
        assert!(!within_budget(&font, 0));
        let record = (0..usize::from(u16::from_be_bytes([font[4], font[5]])))
            .map(|i| 12 + 16 * i)
            .find(|&record| &font[record..record + 4] == b"GPOS")
            .unwrap();
        let mut cut = font.clone();
        let length = (table.len() - 2) as u32;
        cut[record + 12..record + 16].copy_from_slice(&length.to_be_bytes());
        cut.extend([0; 4]);
        assert!(within_budget(&cut, 0));
    }

    #[test]
    fn each_face_of_a_collection_has_its_own_tables() {
        let damaged = with_table(minimal(), b"GPOS", &shared(1, 1, 32_768, &single(1)));
        let fonts = collection(&[minimal(), damaged]);
        assert!(within_budget(&fonts, 0));
        assert!(!within_budget(&fonts, 1));
        // A face the collection does not have has no directory.
        assert_eq!(table_directory(&fonts, 2), None);
        assert_eq!(table_directory(&minimal(), 1), None);
        assert!(within_budget(b"not a font", 0));
    }
}
