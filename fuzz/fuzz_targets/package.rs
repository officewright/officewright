//! Builds an OOXML package around the input and opens it, so that the parsers of
//! `[Content_Types].xml` and of relationships parts, part names and target resolution see
//! mutated content. Inside a real archive every such mutation fails the entry's CRC-32
//! before any of them runs, and libFuzzer does not fix checksums.
//!
//! The input is split at 0xFF bytes, which UTF-8 text never contains, into
//! `[Content_Types].xml`, `_rels/.rels`, the name of one more part, that part, and its
//! relationships part. They are stored, with correct checksums, in a ZIP archive.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_opc::{Limits, Package, PartName};

fuzz_target!(|data: &[u8]| {
    let mut fields = data.split(|&b| b == 0xFF);
    let mut next = || fields.next().unwrap_or_default();
    let (content_types, package_rels, name, part, part_rels) =
        (next(), next(), next(), next(), next());
    let name = String::from_utf8_lossy(name);
    let name = name.trim_start_matches('/');
    let rels_name = match name.rsplit_once('/') {
        Some((folder, file)) => format!("{folder}/_rels/{file}.rels"),
        None => format!("_rels/{name}.rels"),
    };
    let mut entries: Vec<(&[u8], &[u8])> = vec![
        (b"[Content_Types].xml", content_types),
        (b"_rels/.rels", package_rels),
    ];
    if !name.is_empty() {
        entries.push((name.as_bytes(), part));
        entries.push((rels_name.as_bytes(), part_rels));
    }

    let limits = Limits::new()
        .with_max_part_size(1 << 20)
        .with_max_total_size(4 << 20);
    if let Ok(package) = Package::from_bytes(zip(&entries), &limits) {
        let relationships = std::iter::once(package.relationships())
            .chain(package.parts().map(|part| part.relationships()));
        for relationships in relationships {
            for relationship in relationships {
                if let Some(target) = relationship.target_part() {
                    check_name(&target);
                    assert!(target.as_str().len() <= limits.max_part_name_length() + 1);
                    let _ = package.part(&target).map(|part| part.read());
                }
            }
        }
        for part in package.parts() {
            assert_eq!(
                package.part(part.name()).map(|p| p.name()),
                Some(part.name())
            );
            let _ = (part.content_type(), part.read());
        }
    }
    if let Ok(name) = PartName::new(&format!("/{name}")) {
        check_name(&name);
    }
});

/// What every part name the crate hands out satisfies.
fn check_name(name: &PartName) {
    let text = name.as_str();
    assert!(text.starts_with('/'), "{text}");
    assert!(
        !text
            .split('/')
            .skip(1)
            .any(|s| s.is_empty() || s == "." || s == ".."),
        "{text}"
    );
    assert_eq!(PartName::new(text).as_ref().ok(), Some(name));
}

/// A ZIP archive with the entries stored, not compressed.
fn zip(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for &(name, data) in entries {
        let name = name.get(..usize::from(u16::MAX)).unwrap_or(name);
        let offset = out.len() as u32;
        let crc = crc32(data);
        // Signature, version needed, flags, method 0, time, date 1980-01-01.
        out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        // Version made by, version needed, flags, method 0, time, date.
        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        for header in [&mut out, &mut central] {
            header.extend_from_slice(&crc.to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&[0, 0]);
        }
        // Comment length, disk, internal and external attributes, offset.
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
        out.extend_from_slice(name);
        out.extend_from_slice(data);
    }
    let directory_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    let count = entries.len() as u16;
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
