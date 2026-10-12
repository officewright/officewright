//! A known subsetter panic that the fuzz targets which subset fonts skip.

use std::collections::HashSet;

use skrifa::raw::{FontRef, TableProvider};

/// Whether the face trips <https://github.com/typst/subsetter/issues/30>: subsetter
/// 0.2.6 adds the offsets and lengths of the name records it keeps as u16 and panics
/// when they overflow. `FontFace::subset` turns that panic into an error (see the
/// `a_name_record_past_the_u16_range_is_an_error` test); these inputs are skipped here
/// so that nightly fuzzing keeps reporting crashes that are not known yet. Remove this
/// once a subsetter release fixes the issue.
///
/// This follows `subset` and `subset_table` in subsetter's `src/name.rs`: only a version
/// 0 table is rewritten; records are visited in order, a string outside the storage
/// ends the subset with an error before any later record, and identical strings are
/// stored once.
pub fn hits(data: &[u8], index: u32) -> bool {
    let Ok(name) = FontRef::from_index(data, index).and_then(|font| font.name()) else {
        return false;
    };
    if name.version() != 0 {
        return false;
    }
    let storage = name.string_data().as_bytes();
    let mut stored = HashSet::new();
    let mut stored_len = 0u32;
    for record in name.name_record() {
        let id = record.name_id().to_u16();
        if !record.is_unicode() || !matches!(id, 0..=9 | 11..=14) {
            continue;
        }
        let start = record.string_offset().to_u32();
        let length = u32::from(record.length());
        if start + length > 0xFFFF {
            return true;
        }
        let Some(string) = storage.get(start as usize..(start + length) as usize) else {
            return false;
        };
        if stored.insert(string) {
            stored_len += length;
            if stored_len > 0xFFFF {
                return true;
            }
        }
    }
    false
}
