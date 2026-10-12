//! Builds ZIP archives and packages for tests, including ones that tests then break.

use crate::zip::crc32;

/// One entry for [`build`].
#[derive(Clone)]
pub(crate) struct TestEntry {
    name: String,
    data: Vec<u8>,
    deflate: bool,
    zip64: bool,
    data_descriptor: bool,
    descriptor_signature: bool,
    trailing: Vec<u8>,
}

impl TestEntry {
    pub(crate) fn stored(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.into(),
            data: data.into(),
            deflate: false,
            zip64: false,
            data_descriptor: false,
            descriptor_signature: true,
            trailing: Vec::new(),
        }
    }

    pub(crate) fn deflated(name: &str, data: &[u8]) -> Self {
        Self {
            deflate: true,
            ..Self::stored(name, data)
        }
    }

    /// Stores sizes and offset in the zip64 extra field, and adds the zip64 end records.
    pub(crate) fn zip64(self) -> Self {
        Self {
            zip64: true,
            ..self
        }
    }

    /// Puts CRC-32 and sizes in a data descriptor after the data, zeros in the local header.
    pub(crate) fn data_descriptor(self) -> Self {
        Self {
            data_descriptor: true,
            ..self
        }
    }

    /// Writes the data descriptor without its optional signature.
    pub(crate) fn without_descriptor_signature(self) -> Self {
        Self {
            descriptor_signature: false,
            ..self
        }
    }

    /// Appends bytes to the stored (compressed) data and counts them in its size.
    pub(crate) fn trailing(self, bytes: &[u8]) -> Self {
        Self {
            trailing: bytes.into(),
            ..self
        }
    }
}

/// A built archive and the offsets of its records, for tests that patch them.
pub(crate) struct Layout {
    pub(crate) bytes: Vec<u8>,
    /// Local header of each entry.
    pub(crate) local: Vec<usize>,
    /// Central directory record of each entry.
    pub(crate) central: Vec<usize>,
    /// The end of central directory record.
    pub(crate) end: usize,
}

pub(crate) fn zip(entries: &[TestEntry]) -> Vec<u8> {
    build(entries, b"").bytes
}

pub(crate) fn build(entries: &[TestEntry], comment: &[u8]) -> Layout {
    let mut out = Vec::new();
    let mut local = Vec::new();
    let mut stored = Vec::new();
    for entry in entries {
        let mut data = if entry.deflate {
            miniz_oxide::deflate::compress_to_vec(&entry.data, 6)
        } else {
            entry.data.clone()
        };
        data.extend_from_slice(&entry.trailing);
        let crc = crc32(&entry.data);
        local.push(out.len());
        let extra = if entry.zip64 {
            zip64_extra(&[entry.data.len() as u64, data.len() as u64])
        } else {
            Vec::new()
        };
        put32(&mut out, 0x0403_4b50);
        put16(&mut out, if entry.zip64 { 45 } else { 20 });
        put16(&mut out, flags(entry));
        put16(&mut out, method(entry));
        put16(&mut out, 0);
        put16(&mut out, 0x21);
        if entry.data_descriptor {
            out.extend_from_slice(&[0; 12]);
        } else {
            put32(&mut out, crc);
            put32(&mut out, size32(entry, data.len()));
            put32(&mut out, size32(entry, entry.data.len()));
        }
        put16(&mut out, entry.name.len() as u16);
        put16(&mut out, extra.len() as u16);
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&extra);
        out.extend_from_slice(&data);
        if entry.data_descriptor {
            if entry.descriptor_signature {
                put32(&mut out, 0x0807_4b50);
            }
            put32(&mut out, crc);
            if entry.zip64 {
                put64(&mut out, data.len() as u64);
                put64(&mut out, entry.data.len() as u64);
            } else {
                put32(&mut out, data.len() as u32);
                put32(&mut out, entry.data.len() as u32);
            }
        }
        stored.push((crc, data.len()));
    }

    let directory = out.len();
    let mut central = Vec::new();
    for ((entry, &offset), &(crc, compressed)) in entries.iter().zip(&local).zip(&stored) {
        central.push(out.len());
        let extra = if entry.zip64 {
            zip64_extra(&[entry.data.len() as u64, compressed as u64, offset as u64])
        } else {
            Vec::new()
        };
        put32(&mut out, 0x0201_4b50);
        put16(&mut out, if entry.zip64 { 45 } else { 20 });
        put16(&mut out, if entry.zip64 { 45 } else { 20 });
        put16(&mut out, flags(entry));
        put16(&mut out, method(entry));
        put16(&mut out, 0);
        put16(&mut out, 0x21);
        put32(&mut out, crc);
        put32(&mut out, size32(entry, compressed));
        put32(&mut out, size32(entry, entry.data.len()));
        put16(&mut out, entry.name.len() as u16);
        put16(&mut out, extra.len() as u16);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put32(&mut out, 0);
        put32(&mut out, size32(entry, offset));
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&extra);
    }
    let directory_size = out.len() - directory;

    let zip64 = entries.iter().any(|entry| entry.zip64);
    if zip64 {
        let record = out.len();
        put32(&mut out, 0x0606_4b50);
        put64(&mut out, 44);
        put16(&mut out, 45);
        put16(&mut out, 45);
        put32(&mut out, 0);
        put32(&mut out, 0);
        put64(&mut out, entries.len() as u64);
        put64(&mut out, entries.len() as u64);
        put64(&mut out, directory_size as u64);
        put64(&mut out, directory as u64);
        put32(&mut out, 0x0706_4b50);
        put32(&mut out, 0);
        put64(&mut out, record as u64);
        put32(&mut out, 1);
    }
    let end = out.len();
    let count = if zip64 {
        u16::MAX
    } else {
        entries.len() as u16
    };
    put32(&mut out, 0x0605_4b50);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, count);
    put16(&mut out, count);
    put32(
        &mut out,
        if zip64 {
            u32::MAX
        } else {
            directory_size as u32
        },
    );
    put32(&mut out, if zip64 { u32::MAX } else { directory as u32 });
    put16(&mut out, comment.len() as u16);
    out.extend_from_slice(comment);
    Layout {
        bytes: out,
        local,
        central,
        end,
    }
}

fn flags(entry: &TestEntry) -> u16 {
    let utf8 = 1 << 11;
    if entry.data_descriptor {
        utf8 | 1 << 3
    } else {
        utf8
    }
}

fn method(entry: &TestEntry) -> u16 {
    if entry.deflate { 8 } else { 0 }
}

fn size32(entry: &TestEntry, value: usize) -> u32 {
    if entry.zip64 { u32::MAX } else { value as u32 }
}

fn zip64_extra(values: &[u64]) -> Vec<u8> {
    let mut extra = Vec::new();
    put16(&mut extra, 1);
    put16(&mut extra, (values.len() * 8) as u16);
    for &value in values {
        put64(&mut extra, value);
    }
    extra
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn set16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn set32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

pub(crate) const PACKAGE_RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

pub(crate) const DOCUMENT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p></w:body></w:document>"#;

/// The smallest WordprocessingML package: content types, the package relationships and
/// a main part with one paragraph. Stored, not deflated, so the bytes do not depend on
/// the compressor; `tests/data/hello.docx` holds them for the doc examples.
pub(crate) fn hello_docx() -> Vec<u8> {
    zip(&[
        TestEntry::stored("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        TestEntry::stored("_rels/.rels", PACKAGE_RELATIONSHIPS.as_bytes()),
        TestEntry::stored("word/document.xml", DOCUMENT.as_bytes()),
    ])
}

#[test]
fn hello_fixture_matches_its_builder() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/hello.docx");
    if std::env::var_os("OW_UPDATE_FIXTURES").is_some() {
        std::fs::write(path, hello_docx()).unwrap();
    }
    assert!(
        std::fs::read(path).unwrap() == hello_docx(),
        "{path} is stale; regenerate it with OW_UPDATE_FIXTURES=1 cargo test -p officewright-opc"
    );
}
