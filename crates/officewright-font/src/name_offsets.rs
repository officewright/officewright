//! A check of `name` tables whose offsets the subsetter cannot compute yet, or whose
//! strings it would take too long to go through.
//!
//! subsetter 0.2.6 writes a version 0 `name` table anew with the records it keeps: the
//! Unicode ones (platform 0, or platform 3 with encoding 0, 1 or 10) with name IDs 0 to 9
//! and 11 to 14, each distinct string stored once. It computes the offsets in `u16`
//! (`subset_table` and the code that writes the table, in `src/name.rs`), which overflows
//! when a kept record's string ends past byte 65535 of the string storage, when the kept
//! strings take more than 65535 bytes, or when more than 5460 kept records push the
//! storage past byte 65535 of the table. With overflow checks, as in debug builds, that
//! panics (<https://github.com/typst/subsetter/issues/30>); without them the subset fails
//! or, worse, has wrong offsets in its `name` table. No real font has a table like that,
//! so such a face is refused here, before the subsetter runs. The rules about offsets go
//! once a subsetter release computes them without overflow; the rule about the size of
//! the strings below stays, since the subsetter still hashes every string it keeps.
//!
//! The table is read the way subsetter 0.2.6 reads it rather than with another parser,
//! which could reject tables it accepts: a version, a record count, the offset of the
//! string storage, every record, then the records it keeps in order, each string looked
//! up in the storage. Tables it cannot read, or that end its work with an error before
//! an overflow, are left to it. The table is found as the subsetter finds it, by
//! `binary_search_by` over the records of the table directory in their order, sorted or
//! not: built with the same standard library, the same search over the same tags takes
//! the same record, also among several `name` records. Checking each of those instead
//! would let a small file make this check read one table thousands of times. A
//! subsetter release that looks tables up otherwise (its main branch sorts the records
//! first) needs this lookup changed with it; the test of several `name` records fails
//! until it is.
//!
//! One rule is ours: the strings of the records to keep may take at most 1 MiB in all,
//! counted with repeats. Both this check and the subsetter hash each of those strings to
//! store it once, and records may share strings, so 65,535 records of one string of 64 KB
//! made each hash 4 GB (issue #249). Real fonts' come to a few kilobytes. The rule is
//! checked as soon as the records are read, from their lengths, before any string is
//! looked up: a table over it is refused even where the subsetter would stop at a string
//! outside the storage. Tables whose records cannot be read are still left to it, which
//! hashes nothing then.

use std::collections::HashSet;

use skrifa::raw::FontRef;

use crate::error::{Cause, Error, Result};
use crate::subset::subsetter_table;

/// The largest offset the subsetter can compute.
const MAX_OFFSET: usize = 0xFFFF;

/// The bytes of a name record.
const RECORD_SIZE: usize = 12;

/// The bytes before the records: version, count and storage offset.
const HEADER_SIZE: usize = 6;

/// The most bytes the strings of the records to keep may take in all, counted with
/// repeats: what this check and the subsetter hash.
const MAX_KEPT_STRING_BYTES: usize = 1 << 20;

/// Fails if the subsetter, given the `name` table it would take for face `face` in its
/// font file `data`, would overflow its offsets or hash more than
/// [`MAX_KEPT_STRING_BYTES`] of strings.
pub(crate) fn check(font: &FontRef<'_>, data: &[u8], face: u32) -> Result<()> {
    match subsetter_table(font, data, b"name").and_then(refusal) {
        Some(Refusal::Overflow(reason)) => Err(Error::MalformedTable {
            face,
            table: "name",
            source: Cause::new(reason),
        }),
        // A table within the format, past our bound on the work.
        Some(Refusal::OverLimit(reason)) => Err(Error::TableOverLimit {
            face,
            table: "name",
            source: Cause::new(reason),
        }),
        None => Ok(()),
    }
}

/// Why a `name` table is refused.
enum Refusal {
    /// The subsetter would overflow its offsets.
    Overflow(String),
    /// The strings of the records to keep take more than [`MAX_KEPT_STRING_BYTES`].
    OverLimit(String),
}

/// Why the `name` table `table` is refused: subsetter 0.2.6 would overflow on it, or it
/// points at more than [`MAX_KEPT_STRING_BYTES`] of strings to keep.
fn refusal(table: &[u8]) -> Option<Refusal> {
    let read = |at: usize| -> Option<u16> {
        Some(u16::from_be_bytes(
            table.get(at..at.checked_add(2)?)?.try_into().ok()?,
        ))
    };
    if read(0)? != 0 {
        // Kept as it is.
        return None;
    }
    let count = usize::from(read(2)?);
    let storage = table.get(usize::from(read(4)?)..)?;
    // (platform, encoding, name ID, length, offset); the subsetter reads every record
    // before it keeps any.
    let records = (0..count)
        .map(|i| {
            let at = HEADER_SIZE + RECORD_SIZE * i;
            Some((
                read(at)?,
                read(at + 2)?,
                read(at + 6)?,
                read(at + 8)?,
                read(at + 10)?,
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    let keeps = |platform: u16, encoding: u16, id: u16| {
        let unicode = platform == 0 || (platform == 3 && matches!(encoding, 0 | 1 | 10));
        unicode && matches!(id, 0..=9 | 11..=14)
    };
    let referenced: usize = records
        .iter()
        .filter(|&&(platform, encoding, id, ..)| keeps(platform, encoding, id))
        .map(|&(.., length, _)| usize::from(length))
        .sum();
    if referenced > MAX_KEPT_STRING_BYTES {
        return Some(Refusal::OverLimit(format!(
            "the name records to keep point at {referenced} bytes of strings, more than the \
             {MAX_KEPT_STRING_BYTES} read before subsetting"
        )));
    }
    let mut kept = 0;
    let mut strings = HashSet::new();
    let mut stored = 0;
    for (i, &(platform, encoding, id, length, offset)) in records.iter().enumerate() {
        if !keeps(platform, encoding, id) {
            continue;
        }
        kept += 1;
        let (offset, length) = (usize::from(offset), usize::from(length));
        let end = offset + length;
        if end > MAX_OFFSET {
            return Some(Refusal::Overflow(format!(
                "the string of name record {i} ends {end} bytes into the string storage, \
                 past the {MAX_OFFSET} that the subsetter can handle"
            )));
        }
        // The subsetter ends with an error here.
        let string = storage.get(offset..end)?;
        if strings.insert(string) {
            stored += length;
            if stored > MAX_OFFSET {
                return Some(Refusal::Overflow(format!(
                    "the strings of the name records to keep take more than the \
                     {MAX_OFFSET} bytes that the subsetter can handle"
                )));
            }
        }
    }
    // The storage follows the records.
    if HEADER_SIZE + RECORD_SIZE * kept > MAX_OFFSET {
        return Some(Refusal::Overflow(format!(
            "{kept} name records to keep, more than the {} that the subsetter can handle",
            (MAX_OFFSET - HEADER_SIZE) / RECORD_SIZE
        )));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontFace;
    use crate::testing::{TestFont, name_table};

    /// A version 0 `name` table of `records` (platform, encoding, name ID, length,
    /// offset), with `storage` after them; with more records than a 16-bit storage
    /// offset can pass, the storage starts at byte 65535, among the records.
    fn raw_name_table(records: &[(u16, u16, u16, u16, u16)], storage: &[u8]) -> Vec<u8> {
        let mut table = Vec::new();
        let count = u16::try_from(records.len()).unwrap();
        let storage_offset = u16::try_from(6 + 12 * records.len()).unwrap_or(u16::MAX);
        for value in [0, count, storage_offset] {
            table.extend_from_slice(&value.to_be_bytes());
        }
        for &(platform, encoding, id, length, offset) in records {
            for value in [platform, encoding, 0x0409, id, length, offset] {
                table.extend_from_slice(&value.to_be_bytes());
            }
        }
        table.extend_from_slice(storage);
        table
    }

    /// Why the test font with `name` as its `name` table is refused as damaged, by a rule
    /// about the subsetter's offsets; `None` if it subsets. Any other error fails the test.
    fn subset_error(name: Vec<u8>) -> Option<String> {
        refusal_of(name, false)
    }

    /// Why it is refused as past the 1 MiB rule, a limit of ours; `None` if it subsets.
    fn limit_error(name: Vec<u8>) -> Option<String> {
        refusal_of(name, true)
    }

    fn refusal_of(name: Vec<u8>, limit: bool) -> Option<String> {
        let data = TestFont::new("Test Sans").with_table(b"name", name).build();
        let font = FontFace::from_bytes(data, 0).unwrap();
        match font.subset([]) {
            Ok(_) => None,
            Err(err @ Error::MalformedTable { table: "name", .. }) if !limit => {
                Some(std::error::Error::source(&err).unwrap().to_string())
            }
            Err(err @ Error::TableOverLimit { table: "name", .. }) if limit => {
                Some(std::error::Error::source(&err).unwrap().to_string())
            }
            Err(err) => panic!("{err}"),
        }
    }

    #[test]
    fn refuses_a_string_that_ends_past_the_subsetters_offsets() {
        // Found by fuzz/font_subset: offset 0xFFFE plus length 4 overflows the subsetter's
        // u16 (https://github.com/typst/subsetter/issues/30).
        let storage = vec![0; 0x1_0002];
        let err = subset_error(raw_name_table(&[(3, 1, 1, 4, 0xFFFE)], &storage));
        assert_eq!(
            err.as_deref(),
            Some(
                "the string of name record 0 ends 65538 bytes into the string storage, past \
                 the 65535 that the subsetter can handle"
            )
        );
        // Up to byte 65535 is fine; one byte further is not.
        assert_eq!(
            subset_error(raw_name_table(&[(3, 1, 1, 4, 0xFFFB)], &storage)),
            None
        );
        assert!(
            subset_error(raw_name_table(&[(3, 1, 1, 2, 0xFFFE)], &storage))
                .is_some_and(|err| err.contains("ends 65536 bytes"))
        );
        // Records the subsetter drops are not looked at: Macintosh, Windows symbol
        // encoding, name IDs 10 and 15 and up.
        for (platform, encoding, id) in [(1, 0, 1), (3, 2, 1), (3, 1, 10), (0, 3, 15)] {
            let table = raw_name_table(&[(platform, encoding, id, 4, 0xFFFE)], &storage);
            assert_eq!(subset_error(table), None, "{platform} {encoding} {id}");
        }
        // The Unicode encodings it keeps.
        for (platform, encoding, id) in [(0, 3, 0), (3, 0, 9), (3, 10, 14), (3, 1, 11)] {
            let table = raw_name_table(&[(platform, encoding, id, 4, 0xFFFE)], &storage);
            assert!(subset_error(table).is_some(), "{platform} {encoding} {id}");
        }
    }

    #[test]
    fn refuses_strings_to_keep_that_take_more_than_the_subsetters_offsets() {
        // Two different strings of 40000 bytes that overlap in the storage.
        let storage: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let two = [(3, 1, 1, 40_000, 0), (3, 1, 4, 40_000, 10_000)];
        assert_eq!(
            subset_error(raw_name_table(&two, &storage)).as_deref(),
            Some(
                "the strings of the name records to keep take more than the 65535 bytes \
                 that the subsetter can handle"
            )
        );
        // Up to 65535 bytes is fine.
        let fits = [(3, 1, 1, 30_000, 0), (3, 1, 4, 35_535, 1)];
        assert_eq!(subset_error(raw_name_table(&fits, &storage)), None);
        let over = [(3, 1, 1, 30_001, 0), (3, 1, 4, 35_535, 1)];
        assert!(subset_error(raw_name_table(&over, &storage)).is_some());
        // The same string twice is stored once.
        let same = [(3, 1, 1, 40_000, 0), (3, 1, 4, 40_000, 0)];
        assert_eq!(subset_error(raw_name_table(&same, &storage)), None);
        // A string stored once counts once, also before a new one: 40000 bytes twice,
        // then 20000 more, take 60000.
        let then_new = [
            (3, 1, 1, 40_000, 0),
            (3, 1, 4, 40_000, 0),
            (3, 1, 5, 20_000, 1),
        ];
        assert_eq!(subset_error(raw_name_table(&then_new, &storage)), None);
        // So is an equal string elsewhere in the storage.
        let zeros = vec![0; 50_000];
        assert_eq!(subset_error(raw_name_table(&two, &zeros)), None);
    }

    #[test]
    fn refuses_strings_to_keep_of_more_than_1_mib_counted_with_repeats() {
        // 32 records of one string of 32768 bytes, which the subsetter would store once,
        // come to 1 MiB; a record of one byte more is too much. Every Unicode encoding
        // the subsetter keeps counts.
        let storage = vec![0; 32_768];
        let records = |extra: Option<u16>| {
            let mut records = vec![(3, 1, 1, 32_768, 0); 28];
            records.extend([
                (0, 3, 2, 32_768, 0),
                (3, 0, 4, 32_768, 0),
                (3, 10, 6, 32_768, 0),
                (0, 4, 13, 32_768, 0),
            ]);
            records.extend(extra.map(|length| (3, 1, 4, length, 0)));
            records
        };
        assert_eq!(subset_error(raw_name_table(&records(None), &storage)), None);
        // A limit of ours, not damage: the C ABI's LIMIT_EXCEEDED.
        let data = TestFont::new("Test Sans")
            .with_table(b"name", raw_name_table(&records(Some(1)), &storage))
            .build();
        let err = FontFace::from_bytes(data, 0)
            .unwrap()
            .subset([])
            .unwrap_err();
        assert!(
            matches!(err, Error::TableOverLimit { table: "name", .. }),
            "{err}"
        );
        assert!(matches!(
            officewright_core::Error::from(err),
            officewright_core::Error::LimitExceeded(_)
        ));
        assert_eq!(
            limit_error(raw_name_table(&records(Some(1)), &storage)).as_deref(),
            Some(
                "the name records to keep point at 1048577 bytes of strings, more than the \
                 1048576 read before subsetting"
            )
        );
        // Records the subsetter drops do not count: Macintosh, Windows symbol encoding,
        // name IDs 10 and 15 and up.
        let mut dropped = records(None);
        dropped.extend([
            (1, 0, 1, 32_768, 0),
            (3, 2, 1, 32_768, 0),
            (3, 1, 10, 32_768, 0),
            (0, 3, 15, 32_768, 0),
        ]);
        assert_eq!(subset_error(raw_name_table(&dropped, &storage)), None);
        // The rule comes first: here the first string to keep is outside the storage,
        // though within the subsetter's offsets, where it would stop with an error of its
        // own; without the rule, the table was left to it.
        let mut outside = records(Some(1));
        outside.insert(0, (3, 1, 1, 2, 40_000));
        assert!(
            limit_error(raw_name_table(&outside, &storage))
                .is_some_and(|err| err.contains("1048579 bytes"))
        );
    }

    #[test]
    fn refuses_more_records_to_keep_than_the_subsetters_offsets_reach() {
        // 6 + 12 × 5460 = 65526 fits, one record more does not.
        let records = |count: usize| vec![(3, 1, 1, 0, 0); count];
        assert_eq!(subset_error(raw_name_table(&records(5460), &[])), None);
        assert_eq!(
            subset_error(raw_name_table(&records(5461), &[])).as_deref(),
            Some("5461 name records to keep, more than the 5460 that the subsetter can handle")
        );
        // Records it drops do not count.
        let mut mixed = records(5460);
        mixed.extend(vec![(1, 0, 1, 0, 0); 100]);
        assert_eq!(subset_error(raw_name_table(&mixed, &[])), None);
    }

    /// Subsets the test font with `name` as its `name` table, which this check leaves to
    /// the subsetter: it subsets, or the subsetter reports an error, but never panics. A
    /// face the font parser refuses never reaches the subsetter.
    fn left_to_the_subsetter(name: Vec<u8>) {
        let data = TestFont::new("Test Sans").with_table(b"name", name).build();
        let Ok(font) = FontFace::from_bytes(data, 0) else {
            return;
        };
        match font.subset([]) {
            Ok(_) | Err(Error::Subset { .. }) => {}
            Err(err) => panic!("{err}"),
        }
    }

    #[test]
    fn leaves_tables_the_subsetter_stops_at_to_it() {
        // A string outside the storage ends the subsetter's work with an error before it
        // reaches the overflowing record after it.
        let storage = vec![0; 0x1_0002];
        let mut table = raw_name_table(&[(3, 1, 1, 4, 0), (3, 1, 2, 4, 0xFFFE)], &storage);
        table.truncate(table.len() - storage.len() + 2);
        left_to_the_subsetter(table);
        // A version 1 table is kept as it is.
        let mut table = raw_name_table(&[(3, 1, 1, 4, 0xFFFE)], &storage);
        table[1] = 1;
        left_to_the_subsetter(table);
        // A record past the end of the table, after one that would overflow: the
        // subsetter cannot read the table. (The storage starts at byte 6, so that only
        // the record is missing.)
        let mut table = raw_name_table(&[(3, 1, 1, 4, 0xFFFE), (3, 1, 2, 0, 0)], &[]);
        table[4..6].copy_from_slice(&6u16.to_be_bytes());
        assert!(matches!(refusal(&table), Some(Refusal::Overflow(_))));
        assert!(refusal(&table[..table.len() - 1]).is_none());
        left_to_the_subsetter(table[..table.len() - 1].to_vec());
        // An ordinary table.
        left_to_the_subsetter(name_table(&[(3, 1, 0x0409, 1, "Test Sans")]));
    }

    #[test]
    fn reads_the_strings_where_the_storage_offset_puts_them() {
        // The storage starts at byte 6, among the records, as the table says, so the
        // first record's string, 20 bytes into it, is there, and the overflowing record
        // after it is reached. Read from the end of the records, the first string would
        // be missing and the subsetter would stop before the overflow.
        let mut table = raw_name_table(&[(3, 1, 1, 4, 20), (3, 1, 2, 4, 0xFFFE)], &[0; 10]);
        table[4..6].copy_from_slice(&6u16.to_be_bytes());
        assert_eq!(
            subset_error(table).as_deref(),
            Some(
                "the string of name record 1 ends 65538 bytes into the string storage, past \
                 the 65535 that the subsetter can handle"
            )
        );
    }

    #[test]
    fn checks_the_name_table_of_the_face_being_subset() {
        // A collection of a sound face and one whose `name` table overflows: each face is
        // checked against its own table.
        let bad = raw_name_table(&[(3, 1, 1, 4, 0xFFFE)], &vec![0; 0x1_0002]);
        let collection = crate::testing::collection(&[
            TestFont::new("Test Sans").build(),
            TestFont::new("Test Sans").with_table(b"name", bad).build(),
        ]);
        FontFace::from_bytes(collection.clone(), 0)
            .unwrap()
            .subset([])
            .unwrap();
        let err = FontFace::from_bytes(collection, 1)
            .unwrap()
            .subset([])
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::MalformedTable {
                    face: 1,
                    table: "name",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn checks_the_name_table_the_subsetter_takes() {
        // Two to four `name` records, one of them with the overflow, in each position:
        // the bisection takes the same position every time, so exactly one position is
        // refused, and the subsetter never meets the overflow (in a test build it would
        // panic). The tables after the first are built as `namf`, `namg` and `namh`,
        // which sort after `name`, and renamed in the table directory.
        let good = name_table(&[(3, 1, 0x0409, 1, "Test Sans")]);
        let bad = raw_name_table(&[(3, 1, 1, 4, 0xFFFE)], &vec![0; 0x1_0002]);
        let tags = [b"name", b"namf", b"namg", b"namh"];
        for count in 2..=4 {
            let mut refused = 0;
            for bad_at in 0..count {
                let mut font = TestFont::new("Test Sans");
                for (i, tag) in tags.iter().take(count).enumerate() {
                    let table = if i == bad_at { &bad } else { &good };
                    font = font.with_table(tag, table.clone());
                }
                let mut data = font.build();
                let tables = usize::from(u16::from_be_bytes([data[4], data[5]]));
                for at in (0..tables).map(|i| 12 + 16 * i) {
                    if tags[1..].iter().any(|tag| data[at..at + 4] == **tag) {
                        data[at..at + 4].copy_from_slice(b"name");
                    }
                }
                let font = FontFace::from_bytes(data, 0).unwrap();
                match font.subset([]) {
                    Ok(_) => {}
                    Err(Error::MalformedTable { table: "name", .. }) => refused += 1,
                    Err(err) => panic!("{count} records, bad at {bad_at}: {err}"),
                }
            }
            assert_eq!(refused, 1, "{count} records");
        }
        // An unsorted directory: a second `name` record after `post`, at the end, built
        // as `zzzz`. The bisection need not take the last of them.
        let mut refused = 0;
        for (first, last) in [(&good, &bad), (&bad, &good)] {
            let mut data = TestFont::new("Test Sans")
                .with_table(b"name", first.clone())
                .with_table(b"zzzz", last.clone())
                .build();
            let tables = usize::from(u16::from_be_bytes([data[4], data[5]]));
            let record = (0..tables)
                .map(|i| 12 + 16 * i)
                .find(|&at| &data[at..at + 4] == b"zzzz")
                .unwrap();
            data[record..record + 4].copy_from_slice(b"name");
            let font = FontFace::from_bytes(data, 0).unwrap();
            match font.subset([]) {
                Ok(_) => {}
                Err(Error::MalformedTable { table: "name", .. }) => refused += 1,
                Err(err) => panic!("unsorted: {err}"),
            }
        }
        assert_eq!(refused, 1, "unsorted");
    }
}
