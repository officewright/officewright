use super::*;
use crate::testing::{Layout, TestEntry, build, set16, set32, zip};

const TEXT: &[u8] = b"<w:t>Hello, world</w:t><w:t>Hello, world</w:t><w:t>Hello, world</w:t>";

fn open(bytes: &[u8]) -> Result<Archive> {
    Archive::new(bytes, &Limits::new())
}

fn read_all(archive: &Archive) -> Result<Vec<(String, Vec<u8>)>> {
    archive
        .entries()
        .map(|entry| Ok((entry.name().to_owned(), entry.read()?)))
        .collect()
}

fn zip_message(result: Result<Archive>) -> String {
    match result {
        Err(Error::Zip { message, .. }) => message,
        other => panic!("expected a zip error, got {other:?}"),
    }
}

fn entry_message(result: Result<Vec<u8>>) -> String {
    match result {
        Err(Error::Entry { message, .. }) => message,
        other => panic!("expected an entry error, got {other:?}"),
    }
}

fn read_first(bytes: &[u8]) -> Result<Vec<u8>> {
    let archive = open(bytes).unwrap();
    archive.entry(0).unwrap().read()
}

/// Offset of an entry's data, after its 30-byte local header and name.
fn data_offset(layout: &Layout, index: usize, name: &str) -> usize {
    layout.local[index] + 30 + name.len()
}

/// Sets an entry's compressed and uncompressed sizes in its local header and in the
/// central directory, so that both agree.
fn set_sizes(layout: &mut Layout, index: usize, compressed: u32, size: u32) {
    let (local, central) = (layout.local[index], layout.central[index]);
    set32(&mut layout.bytes, local + 18, compressed);
    set32(&mut layout.bytes, local + 22, size);
    set32(&mut layout.bytes, central + 20, compressed);
    set32(&mut layout.bytes, central + 24, size);
}

/// Sets an entry's CRC-32 in its local header and in the central directory.
fn set_crc(layout: &mut Layout, index: usize, crc: u32) {
    let (local, central) = (layout.local[index], layout.central[index]);
    set32(&mut layout.bytes, local + 14, crc);
    set32(&mut layout.bytes, central + 16, crc);
}

fn central_u32(layout: &Layout, index: usize, at: usize) -> u32 {
    let start = layout.central[index] + at;
    u32::from_le_bytes(layout.bytes[start..start + 4].try_into().unwrap())
}

#[test]
fn reads_stored_and_deflated_entries() {
    let bytes = zip(&[
        TestEntry::stored("a.txt", b"plain"),
        TestEntry::deflated("dir/b.xml", TEXT),
    ]);
    let archive = open(&bytes).unwrap();
    assert_eq!(
        read_all(&archive).unwrap(),
        [
            ("a.txt".to_owned(), b"plain".to_vec()),
            ("dir/b.xml".to_owned(), TEXT.to_vec())
        ]
    );
    let deflated = archive.entry(1).unwrap();
    assert_eq!(deflated.size(), TEXT.len() as u64);
    assert!(deflated.compressed_size() < deflated.size());
    assert!(archive.entry(2).is_none());
    assert_eq!(archive.entries().len(), 2);
}

#[test]
fn reads_empty_entries() {
    let bytes = zip(&[TestEntry::stored("a", b""), TestEntry::deflated("b", b"")]);
    let archive = open(&bytes).unwrap();
    assert_eq!(
        read_all(&archive).unwrap(),
        [("a".to_owned(), vec![]), ("b".to_owned(), vec![])]
    );
}

#[test]
fn reads_a_deflated_entry_without_data() {
    // Some writers give an empty file method 8 and no data at all; LibreOffice's
    // sc/qa/unit/data/xlsx/tdf124525.xlsx has six such entries.
    let mut layout = build(&[TestEntry::deflated("a", b"")], b"");
    set_sizes(&mut layout, 0, 0, 0);
    assert_eq!(read_first(&layout.bytes).unwrap(), b"");
}

#[test]
fn reads_zip64_records_and_extra_fields() {
    let bytes = zip(&[
        TestEntry::deflated("a.xml", TEXT).zip64(),
        TestEntry::stored("b.xml", b"b"),
    ]);
    let archive = open(&bytes).unwrap();
    assert_eq!(
        read_all(&archive).unwrap(),
        [
            ("a.xml".to_owned(), TEXT.to_vec()),
            ("b.xml".to_owned(), b"b".to_vec())
        ]
    );
}

#[test]
fn reads_entries_with_data_descriptors() {
    let bytes = zip(&[
        TestEntry::deflated("a.xml", TEXT).data_descriptor(),
        TestEntry::stored("b.xml", b"b").data_descriptor().zip64(),
        TestEntry::deflated("c.xml", TEXT)
            .data_descriptor()
            .without_descriptor_signature(),
    ]);
    let archive = open(&bytes).unwrap();
    let all = read_all(&archive).unwrap();
    assert_eq!(all[0].1, TEXT);
    assert_eq!(all[1].1, b"b");
    assert_eq!(all[2].1, TEXT);
}

#[test]
fn accepts_bytes_after_the_end_record() {
    let mut bytes = zip(&[TestEntry::stored("a", b"a")]);
    bytes.push(0);
    assert_eq!(open(&bytes).unwrap().entries().len(), 1);
}

#[test]
fn skips_a_fake_end_record_in_the_comment() {
    // The comment holds a copy of the central directory that names "b" and an end
    // record that points at it. That record is the last one in the file and its
    // directory checks out, but its own comment would run past the end, so the real
    // record before it is used.
    let base = build(&[TestEntry::stored("a", b"a")], b"");
    let mut fake_directory = base.bytes[base.central[0]..base.end].to_vec();
    *fake_directory.last_mut().unwrap() = b'b';
    let fake_directory_at = base.end + 22;
    let mut comment = fake_directory.clone();
    comment.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    comment.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
    comment.extend_from_slice(&(fake_directory.len() as u32).to_le_bytes());
    comment.extend_from_slice(&(fake_directory_at as u32).to_le_bytes());
    comment.extend_from_slice(&u16::MAX.to_le_bytes());
    let layout = build(&[TestEntry::stored("a", b"a")], &comment);
    let archive = open(&layout.bytes).unwrap();
    assert_eq!(archive.entry(0).unwrap().name(), "a");
}

#[test]
fn rejects_a_comment_that_runs_past_the_end() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"abc");
    set16(&mut layout.bytes, layout.end + 20, 10);
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "the archive comment runs past the end"
    );
}

#[test]
fn rejects_input_that_is_not_a_zip_archive() {
    assert_eq!(
        zip_message(open(b"short")),
        "the input is too short to be a zip archive"
    );
    assert_eq!(
        zip_message(open(&[b'x'; 100])),
        "no end of central directory record"
    );
}

#[test]
fn rejects_truncated_archives_at_every_length() {
    let bytes = zip(&[
        TestEntry::deflated("a.xml", TEXT),
        TestEntry::stored("b", b"b"),
    ]);
    for len in 0..bytes.len() {
        assert!(open(&bytes[..len]).is_err(), "accepted {len} bytes");
    }
}

#[test]
fn rejects_a_central_directory_that_does_not_reach_the_end_record() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    let offset = u32::from_le_bytes(layout.bytes[layout.end + 16..][..4].try_into().unwrap());
    set32(&mut layout.bytes, layout.end + 16, offset - 1);
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "the central directory does not end where the end of central directory record begins"
    );
}

#[test]
fn rejects_data_before_the_first_entry() {
    // Offsets are not shifted: a self-extracting stub or a concatenated file in front
    // of the archive makes every offset wrong.
    let mut bytes = vec![0; 100];
    bytes.extend(zip(&[TestEntry::stored("a", b"a")]));
    assert_eq!(
        zip_message(open(&bytes)),
        "the central directory does not end where the end of central directory record begins"
    );
}

#[test]
fn rejects_data_before_the_first_entry_with_its_offsets_shifted() {
    // As `zip -A` and Python's zipfile in append mode write a self-extracting archive or
    // a concatenated file: every offset is right for the bytes in front (#98). The same
    // for an archive without entries, whose central directory comes first.
    let mut stub = b"MZ".to_vec();
    stub.resize(1002, 0);
    for entries in [vec![TestEntry::stored("a", b"a")], Vec::new()] {
        let layout = build(&entries, b"");
        let mut bytes = stub.clone();
        bytes.extend(&layout.bytes);
        let shift = |bytes: &mut Vec<u8>, at: usize| {
            let at = stub.len() + at;
            let offset = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            set32(bytes, at, offset + stub.len() as u32);
        };
        for &central in &layout.central {
            shift(&mut bytes, central + 42);
        }
        shift(&mut bytes, layout.end + 16);
        assert_eq!(
            zip_message(open(&bytes)),
            "1002 bytes come before the first entry",
            "{} entries",
            entries.len()
        );
    }
}

#[test]
fn rejects_more_entries_than_the_directory_holds() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set16(&mut layout.bytes, layout.end + 8, 2);
    set16(&mut layout.bytes, layout.end + 10, 2);
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "the central directory is too small for 2 entries"
    );
}

#[test]
fn rejects_bytes_after_the_last_central_record() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    layout.bytes.splice(layout.end..layout.end, [0; 3]);
    let end = layout.end + 3;
    let size = u32::from_le_bytes(layout.bytes[end + 12..][..4].try_into().unwrap());
    set32(&mut layout.bytes, end + 12, size + 3);
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "unexpected bytes after the last central directory record"
    );
}

#[test]
fn rejects_a_central_record_that_runs_past_the_directory() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set16(&mut layout.bytes, layout.central[0] + 28, 1000);
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "a central directory record runs past the end of the central directory"
    );
}

#[test]
fn rejects_bad_signatures() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    layout.bytes[layout.central[0]] = b'X';
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "bad central directory record signature"
    );
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    layout.bytes[layout.local[0]] = b'X';
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "bad local header signature for entry a"
    );
}

#[test]
fn rejects_a_zip64_locator_that_points_out_of_range() {
    let mut layout = build(&[TestEntry::stored("a", b"a").zip64()], b"");
    let locator = layout.end - 20;
    set32(&mut layout.bytes, locator + 8, 0xFFFF_FF00);
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "the zip64 end of central directory locator points out of range"
    );
}

#[test]
fn rejects_a_local_header_that_names_another_entry() {
    let mut layout = build(&[TestEntry::stored("a.xml", b"a")], b"");
    layout.bytes[layout.local[0] + 30] = b'b';
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "the local header of entry a.xml names a different entry"
    );
}

#[test]
fn rejects_a_local_header_with_another_method() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set16(&mut layout.bytes, layout.local[0] + 8, 8);
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "the local header of entry a gives compression method 8, the central directory 0"
    );
}

#[test]
fn rejects_a_local_header_with_another_crc_or_size() {
    // A reader that walks the local headers would see other bytes than one that reads
    // the central directory.
    let central_crc = crc32(b"hello world");
    for (at, value) in [(14, 0xDEAD_BEEF), (18, 5), (22, 5)] {
        let mut layout = build(&[TestEntry::stored("a", b"hello world")], b"");
        set32(&mut layout.bytes, layout.local[0] + at, value);
        let (crc, compressed, size) = match at {
            14 => (0xDEAD_BEEF, 11, 11),
            18 => (central_crc, 5, 11),
            _ => (central_crc, 11, 5),
        };
        assert_eq!(
            zip_message(open(&layout.bytes)),
            format!(
                "the local header of entry a gives crc-32 {crc:08x}, {compressed} compressed \
                 and {size} uncompressed bytes, the central directory {central_crc:08x}, 11 \
                 and 11"
            )
        );
    }
}

#[test]
fn rejects_a_zip64_local_header_with_other_sizes() {
    let mut layout = build(&[TestEntry::stored("a", b"hello world").zip64()], b"");
    // The local zip64 extra field follows the name: id, length, size, compressed size.
    let extra = layout.local[0] + 30 + 1;
    set32(&mut layout.bytes, extra + 4, 5);
    assert!(zip_message(open(&layout.bytes)).starts_with("the local header of entry a gives"));
}

#[test]
fn rejects_a_data_descriptor_that_disagrees() {
    let layout = build(&[TestEntry::deflated("a", TEXT).data_descriptor()], b"");
    let descriptor = data_offset(&layout, 0, "a") + central_u32(&layout, 0, 20) as usize;
    for at in [4, 8, 12] {
        let mut bytes = layout.bytes.clone();
        bytes[descriptor + at] ^= 1;
        assert_eq!(
            zip_message(open(&bytes)),
            "the data descriptor of entry a disagrees with the central directory"
        );
    }
}

#[test]
fn rejects_duplicate_names() {
    let bytes = zip(&[TestEntry::stored("a", b"1"), TestEntry::stored("a", b"2")]);
    assert_eq!(zip_message(open(&bytes)), "duplicate entry name a");
}

#[test]
fn rejects_names_that_are_not_utf8() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    layout.bytes[layout.local[0] + 30] = 0xff;
    layout.bytes[layout.central[0] + 46] = 0xff;
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "an entry name is not valid UTF-8"
    );
}

#[test]
fn rejects_overlapping_entries() {
    // The first entry claims the second one's local header as its own data, like the
    // entries of a non-recursive zip bomb.
    let mut layout = build(
        &[TestEntry::stored("a", b"a"), TestEntry::stored("b", b"b")],
        b"",
    );
    let covering = (layout.local[1] + 31 - data_offset(&layout, 0, "a")) as u32;
    set_sizes(&mut layout, 0, covering, covering);
    assert_eq!(zip_message(open(&layout.bytes)), "entries a and b overlap");
}

#[test]
fn rejects_an_entry_that_runs_into_the_central_directory() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set_sizes(&mut layout, 0, 100, 100);
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "entry a runs into the central directory"
    );
}

#[test]
fn rejects_encrypted_entries() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set16(&mut layout.bytes, layout.central[0] + 8, 1);
    assert!(matches!(
        open(&layout.bytes),
        Err(Error::Unsupported(message)) if message == "zip entry a is encrypted"
    ));
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set16(&mut layout.bytes, layout.local[0] + 6, 1);
    assert!(matches!(open(&layout.bytes), Err(Error::Unsupported(_))));
}

#[test]
fn rejects_archives_split_over_several_disks() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set16(&mut layout.bytes, layout.end + 4, 1);
    assert!(matches!(
        open(&layout.bytes),
        Err(Error::Unsupported(message)) if message.contains("several disks")
    ));
}

#[test]
fn rejects_a_zip64_entry_without_its_extra_field() {
    let mut layout = build(&[TestEntry::stored("a", b"a").zip64()], b"");
    // Renames the extra field so that it is no longer the zip64 one.
    set16(&mut layout.bytes, layout.central[0] + 46 + 1, 0x7075);
    assert_eq!(
        zip_message(open(&layout.bytes)),
        "entry a lacks its zip64 extra field"
    );
}

#[test]
fn rejects_more_entries_than_the_limit() {
    let bytes = zip(&[TestEntry::stored("a", b"a"), TestEntry::stored("b", b"b")]);
    let result = Archive::new(bytes, &Limits::new().with_max_entries(1));
    assert!(matches!(
        result,
        Err(Error::LimitExceeded {
            limit: Limit::Entries,
            maximum: 1,
            ..
        })
    ));
}

#[test]
fn refuses_to_read_an_entry_larger_than_the_limit() {
    let bytes = zip(&[TestEntry::deflated("a", TEXT)]);
    let limits = Limits::new().with_max_part_size(TEXT.len() as u64 - 1);
    let archive = Archive::new(bytes, &limits).unwrap();
    let err = archive.entry(0).unwrap().read().unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "zip entry a: uncompressed size in bytes exceeds the limit of {}",
            TEXT.len() - 1
        )
    );
}

#[test]
fn rejects_unsupported_compression_methods_when_reading() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set16(&mut layout.bytes, layout.local[0] + 8, 12);
    set16(&mut layout.bytes, layout.central[0] + 10, 12);
    assert!(matches!(
        read_first(&layout.bytes),
        Err(Error::Unsupported(message)) if message.starts_with("zip entry a uses compression method 12")
    ));
}

#[test]
fn rejects_a_crc_mismatch() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set_crc(&mut layout, 0, 0x1234_5678);
    assert_eq!(
        entry_message(read_first(&layout.bytes)),
        format!(
            "crc-32 is {:08x} but the archive declares 12345678",
            crc32(b"a")
        )
    );
}

#[test]
fn rejects_deflate_data_larger_than_declared() {
    let mut layout = build(&[TestEntry::deflated("a", TEXT)], b"");
    let compressed = central_u32(&layout, 0, 20);
    set_sizes(&mut layout, 0, compressed, 10);
    assert_eq!(
        entry_message(read_first(&layout.bytes)),
        "inflates to more than the 10 bytes it declares"
    );
}

#[test]
fn rejects_deflate_data_smaller_than_declared() {
    let mut layout = build(&[TestEntry::deflated("a", TEXT)], b"");
    let compressed = central_u32(&layout, 0, 20);
    set_sizes(&mut layout, 0, compressed, TEXT.len() as u32 + 1);
    assert_eq!(
        entry_message(read_first(&layout.bytes)),
        format!(
            "inflates to {} bytes but declares {}",
            TEXT.len(),
            TEXT.len() + 1
        )
    );
}

#[test]
fn rejects_deflate_data_that_ends_before_its_compressed_size() {
    let bytes = zip(&[TestEntry::deflated("a", TEXT).trailing(b"junk")]);
    assert_eq!(
        entry_message(read_first(&bytes)),
        "deflate data ends 4 bytes before its compressed size"
    );
}

#[test]
fn rejects_sizes_deflate_cannot_reach() {
    let mut layout = build(&[TestEntry::deflated("a", TEXT)], b"");
    let compressed = central_u32(&layout, 0, 20);
    let impossible = compressed * MAX_DEFLATE_RATIO as u32 + 1;
    set_sizes(&mut layout, 0, compressed, impossible);
    assert_eq!(
        entry_message(read_first(&layout.bytes)),
        format!(
            "declares {impossible} bytes, more than {compressed} bytes of deflate data can hold"
        )
    );
}

#[test]
fn rejects_corrupt_and_truncated_deflate_data() {
    let mut layout = build(&[TestEntry::deflated("a", TEXT)], b"");
    let start = data_offset(&layout, 0, "a");
    layout.bytes[start] = 0xff; // block type 3, which does not exist
    assert_eq!(
        entry_message(read_first(&layout.bytes)),
        "deflate data is corrupt"
    );

    let mut layout = build(&[TestEntry::deflated("a", TEXT)], b"");
    let compressed = central_u32(&layout, 0, 20);
    set_sizes(&mut layout, 0, compressed / 2, TEXT.len() as u32);
    assert_eq!(
        entry_message(read_first(&layout.bytes)),
        "deflate data is truncated"
    );
}

#[test]
fn rejects_stored_entries_with_two_sizes() {
    let mut layout = build(&[TestEntry::stored("a", b"ab")], b"");
    set_sizes(&mut layout, 0, 2, 1);
    assert_eq!(
        entry_message(read_first(&layout.bytes)),
        "is stored in 2 bytes but declares 1 bytes"
    );
}

#[test]
fn computes_the_zip_crc32() {
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

/// The archive with its central directory records in reverse order, as some writers
/// order the directory differently from the data.
fn reverse_central_directory(layout: &Layout) -> Vec<u8> {
    let mut bounds = layout.central.clone();
    bounds.push(layout.end);
    let records: Vec<&[u8]> = bounds
        .windows(2)
        .map(|pair| &layout.bytes[pair[0]..pair[1]])
        .collect();
    let mut bytes = layout.bytes[..layout.central[0]].to_vec();
    for record in records.iter().rev() {
        bytes.extend_from_slice(record);
    }
    bytes.extend_from_slice(&layout.bytes[layout.end..]);
    bytes
}

#[test]
fn reads_a_central_directory_in_another_order_than_the_entries() {
    let layout = build(
        &[
            TestEntry::stored("a", b"1"),
            TestEntry::deflated("b", TEXT),
            TestEntry::stored("c", b"3"),
        ],
        b"",
    );
    let archive = open(&reverse_central_directory(&layout)).unwrap();
    assert_eq!(
        read_all(&archive).unwrap(),
        [
            ("c".to_owned(), b"3".to_vec()),
            ("b".to_owned(), TEXT.to_vec()),
            ("a".to_owned(), b"1".to_vec())
        ]
    );
}

#[test]
fn finds_overlaps_whatever_the_order_of_the_central_directory() {
    let mut layout = build(
        &[
            TestEntry::stored("a", b"a"),
            TestEntry::stored("b", b"b"),
            TestEntry::stored("c", b"c"),
        ],
        b"",
    );
    let covering = (layout.local[1] + 31 - data_offset(&layout, 0, "a")) as u32;
    set_sizes(&mut layout, 0, covering, covering);
    assert_eq!(
        zip_message(open(&reverse_central_directory(&layout))),
        "entries a and b overlap"
    );
}

#[test]
fn rejects_entries_on_another_disk() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set16(&mut layout.bytes, layout.central[0] + 34, 1);
    assert!(matches!(
        open(&layout.bytes),
        Err(Error::Unsupported(message)) if message.contains("several disks")
    ));
    for (at, value) in [(4, 1), (16, 2)] {
        let mut layout = build(&[TestEntry::stored("a", b"a").zip64()], b"");
        let locator = layout.end - 20;
        set32(&mut layout.bytes, locator + at, value);
        assert!(
            matches!(
                open(&layout.bytes),
                Err(Error::Unsupported(ref message)) if message.contains("several disks")
            ),
            "zip64 locator field at {at}"
        );
    }
}

#[test]
fn opens_as_many_entries_as_the_default_limit_allows() {
    let entries = |count: usize| -> Vec<TestEntry> {
        (0..count)
            .map(|i| {
                let entry = TestEntry::stored(&i.to_string(), b"");
                // More than 65,535 entries need the zip64 end records.
                if i == 0 { entry.zip64() } else { entry }
            })
            .collect()
    };
    let limit = Limits::new().max_entries() as usize;
    assert_eq!(open(&zip(&entries(limit))).unwrap().entries().len(), limit);
    assert!(matches!(
        open(&zip(&entries(limit + 1))),
        Err(Error::LimitExceeded {
            limit: Limit::Entries,
            maximum: 65_536,
            ..
        })
    ));
}
