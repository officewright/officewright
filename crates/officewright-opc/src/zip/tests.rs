use super::*;
use crate::testing::{Layout, TestEntry, build, set16, set32, zip};

const TEXT: &[u8] = b"<w:t>Hello, world</w:t><w:t>Hello, world</w:t><w:t>Hello, world</w:t>";

fn open(bytes: &[u8]) -> Result<Archive> {
    Archive::new(bytes, &Limits::new())
}

fn read_all(archive: &Archive) -> Result<Vec<(String, Vec<u8>)>> {
    archive
        .entries()
        .iter()
        .map(|entry| Ok((entry.name().to_owned(), archive.read(entry)?)))
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

/// Offset of an entry's data, after its 30-byte local header and name.
fn data_offset(layout: &Layout, index: usize, name: &str) -> usize {
    layout.local[index] + 30 + name.len()
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
    let deflated = &archive.entries()[1];
    assert_eq!(deflated.size(), TEXT.len() as u64);
    assert!(deflated.compressed_size() < deflated.size());
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
    ]);
    let archive = open(&bytes).unwrap();
    assert_eq!(read_all(&archive).unwrap()[0].1, TEXT);
}

#[test]
fn accepts_bytes_after_the_end_record() {
    let mut bytes = zip(&[TestEntry::stored("a", b"a")]);
    bytes.push(0);
    assert_eq!(open(&bytes).unwrap().entries().len(), 1);
}

#[test]
fn ignores_a_fake_end_record_in_the_comment() {
    let mut comment = b"PK\x05\x06".to_vec();
    comment.extend_from_slice(&[0xff; 18]);
    let layout = build(&[TestEntry::stored("a", b"a")], &comment);
    assert_eq!(open(&layout.bytes).unwrap().entries()[0].name(), "a");
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
    set32(&mut layout.bytes, layout.central[0] + 20, covering);
    set32(&mut layout.bytes, layout.central[0] + 24, covering);
    assert_eq!(zip_message(open(&layout.bytes)), "entries a and b overlap");
}

#[test]
fn rejects_an_entry_that_runs_into_the_central_directory() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set32(&mut layout.bytes, layout.central[0] + 20, 100);
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
    let result = Archive::new(bytes.as_slice(), &Limits::new().with_max_entries(1));
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
    let archive = Archive::new(bytes.as_slice(), &limits).unwrap();
    let err = archive.read(&archive.entries()[0]).unwrap_err();
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
    let archive = open(&layout.bytes).unwrap();
    assert!(matches!(
        archive.read(&archive.entries()[0]),
        Err(Error::Unsupported(message)) if message.starts_with("zip entry a uses compression method 12")
    ));
}

#[test]
fn rejects_a_crc_mismatch() {
    let mut layout = build(&[TestEntry::stored("a", b"a")], b"");
    set32(&mut layout.bytes, layout.central[0] + 16, 0x1234_5678);
    let archive = open(&layout.bytes).unwrap();
    assert_eq!(
        entry_message(archive.read(&archive.entries()[0])),
        format!(
            "crc-32 is {:08x} but the archive declares 12345678",
            crc32(b"a")
        )
    );
}

#[test]
fn rejects_deflate_data_larger_than_declared() {
    let mut layout = build(&[TestEntry::deflated("a", TEXT)], b"");
    set32(&mut layout.bytes, layout.central[0] + 24, 10);
    let archive = open(&layout.bytes).unwrap();
    assert_eq!(
        entry_message(archive.read(&archive.entries()[0])),
        "inflates to more than the 10 bytes it declares"
    );
}

#[test]
fn rejects_deflate_data_smaller_than_declared() {
    let mut layout = build(&[TestEntry::deflated("a", TEXT)], b"");
    set32(
        &mut layout.bytes,
        layout.central[0] + 24,
        TEXT.len() as u32 + 1,
    );
    let archive = open(&layout.bytes).unwrap();
    assert_eq!(
        entry_message(archive.read(&archive.entries()[0])),
        format!(
            "inflates to {} bytes but declares {}",
            TEXT.len(),
            TEXT.len() + 1
        )
    );
}

#[test]
fn rejects_sizes_deflate_cannot_reach() {
    let mut layout = build(&[TestEntry::deflated("a", TEXT)], b"");
    let compressed = open(&layout.bytes).unwrap().entries()[0].compressed_size();
    let impossible = compressed * MAX_DEFLATE_RATIO + 1;
    set32(&mut layout.bytes, layout.central[0] + 24, impossible as u32);
    let archive = open(&layout.bytes).unwrap();
    assert_eq!(
        entry_message(archive.read(&archive.entries()[0])),
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
    let archive = open(&layout.bytes).unwrap();
    assert_eq!(
        entry_message(archive.read(&archive.entries()[0])),
        "deflate data is corrupt"
    );

    let mut layout = build(&[TestEntry::deflated("a", TEXT)], b"");
    let compressed = open(&layout.bytes).unwrap().entries()[0].compressed_size() as u32;
    set32(&mut layout.bytes, layout.central[0] + 20, compressed / 2);
    let archive = open(&layout.bytes).unwrap();
    assert_eq!(
        entry_message(archive.read(&archive.entries()[0])),
        "deflate data is truncated"
    );
}

#[test]
fn rejects_stored_entries_with_two_sizes() {
    let mut layout = build(&[TestEntry::stored("a", b"ab")], b"");
    set32(&mut layout.bytes, layout.central[0] + 24, 1);
    let archive = open(&layout.bytes).unwrap();
    assert_eq!(
        entry_message(archive.read(&archive.entries()[0])),
        "is stored in 2 bytes but declares 1 bytes"
    );
}

#[test]
fn computes_the_zip_crc32() {
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}
