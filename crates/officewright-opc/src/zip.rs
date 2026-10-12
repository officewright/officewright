//! Reading ZIP archives, the container of every OOXML package.
//!
//! Only what ECMA-376 Part 2, Annex C lets a package use is read: stored and deflated
//! entries, ZIP64 sizes and offsets, a single disk and no encryption. Every offset and
//! size from the archive is checked against the input before use, and the archive is
//! rejected when:
//!
//! - the central directory and a local header disagree on an entry's name or method, so
//!   that every reader of the file sees the same entries;
//! - two entries overlap, the trick behind non-recursive zip bombs;
//! - two entries have the same name;
//! - an entry decompresses to more or fewer bytes than it declares, or fails its CRC-32.
//!
//! Entries are decompressed only when [read](Archive::read), and never past their declared
//! size or [`Limits::max_part_size`].
//!
//! ```
//! use officewright_opc::{Limits, zip::Archive};
//!
//! let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/hello.docx"));
//! let archive = Archive::new(bytes.as_slice(), &Limits::new())?;
//! let entry = archive
//!     .entries()
//!     .iter()
//!     .find(|entry| entry.name() == "word/document.xml")
//!     .expect("the test document has a main part");
//! let xml = archive.read(entry)?;
//! assert!(xml.starts_with(b"<?xml"));
//! # Ok::<(), officewright_opc::Error>(())
//! ```

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use miniz_oxide::inflate::TINFLStatus;

use crate::{Error, Limit, Limits, Result};

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4b50;
const ZIP64_END_OF_CENTRAL_DIRECTORY: u32 = 0x0606_4b50;
const ZIP64_LOCATOR: u32 = 0x0706_4b50;

const LOCAL_HEADER_LEN: usize = 30;
const CENTRAL_HEADER_LEN: usize = 46;
const END_LEN: usize = 22;
const ZIP64_END_LEN: usize = 56;
const ZIP64_LOCATOR_LEN: usize = 20;

const ZIP64_EXTRA_FIELD: u16 = 0x0001;

const STORED: u16 = 0;
const DEFLATED: u16 = 8;

const FLAG_ENCRYPTED: u16 = 1;
const FLAG_STRONG_ENCRYPTION: u16 = 1 << 6;

/// DEFLATE cannot expand data more than 1032 times (a 258-byte match in 2 bits), so an
/// entry that declares more is lying about its size.
const MAX_DEFLATE_RATIO: u64 = 1032;

/// A ZIP archive held in memory, with its central directory read and checked.
pub struct Archive {
    data: Arc<[u8]>,
    entries: Vec<Entry>,
    max_entry_size: u64,
}

/// One entry of an [`Archive`].
#[derive(Clone, Debug)]
pub struct Entry {
    name: String,
    method: u16,
    crc32: u32,
    compressed_size: u64,
    size: u64,
    data_offset: usize,
}

impl Archive {
    /// Reads the central directory of the archive in `data` and checks every entry's
    /// local header.
    ///
    /// # Errors
    ///
    /// [`Error::Zip`] if the archive is malformed, [`Error::Unsupported`] for encryption
    /// or archives split over several disks, and [`Error::LimitExceeded`] for more
    /// entries than [`Limits::max_entries`].
    pub fn new(data: impl Into<Arc<[u8]>>, limits: &Limits) -> Result<Self> {
        let data: Arc<[u8]> = data.into();
        let end = find_end(&data)?;
        if end.entries > limits.max_entries() {
            return Err(Error::LimitExceeded {
                limit: Limit::Entries,
                maximum: limits.max_entries(),
                context: "zip archive".into(),
            });
        }
        // Every central directory record takes at least 46 bytes, which also bounds the
        // allocations below by the size of the input.
        let directory_end = end.directory_offset + end.directory_size;
        if end.entries.saturating_mul(CENTRAL_HEADER_LEN as u64) > end.directory_size as u64 {
            return Err(zip_error(
                end.directory_offset,
                format!(
                    "the central directory is too small for {} entries",
                    end.entries
                ),
            ));
        }
        let count = usize::try_from(end.entries).unwrap_or(usize::MAX);
        let mut entries = Vec::with_capacity(count);
        let mut spans = Vec::with_capacity(count);
        let mut names = HashSet::with_capacity(count);
        let mut at = end.directory_offset;
        for _ in 0..count {
            let record = read_central_record(&data, at, directory_end)?;
            if !names.insert(record.entry.name.clone()) {
                return Err(zip_error(
                    at,
                    format!("duplicate entry name {}", record.entry.name),
                ));
            }
            let (entry, span) = check_local_header(&data, record, end.directory_offset)?;
            entries.push(entry);
            spans.push(span);
            at = span.next_record;
        }
        if at != directory_end {
            return Err(zip_error(
                at,
                "unexpected bytes after the last central directory record",
            ));
        }
        check_overlaps(&mut spans, &entries)?;
        Ok(Self {
            data,
            entries,
            max_entry_size: limits.max_part_size(),
        })
    }

    /// The entries in the order of the central directory.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Decompresses an entry of this archive and checks its size and CRC-32.
    ///
    /// # Errors
    ///
    /// [`Error::LimitExceeded`] if the entry is larger than [`Limits::max_part_size`],
    /// [`Error::Entry`] if its data is corrupt, and [`Error::Unsupported`] for a
    /// compression method other than stored or deflated.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        if entry.size > self.max_entry_size {
            return Err(Error::LimitExceeded {
                limit: Limit::PartSize,
                maximum: self.max_entry_size,
                context: format!("zip entry {}", entry.name),
            });
        }
        let raw = usize::try_from(entry.compressed_size)
            .ok()
            .and_then(|len| {
                self.data
                    .get(entry.data_offset..entry.data_offset.checked_add(len)?)
            })
            .ok_or_else(|| entry_error(entry, "data lies outside the archive"))?;
        let data = match entry.method {
            STORED => {
                if entry.compressed_size != entry.size {
                    return Err(entry_error(
                        entry,
                        format!(
                            "is stored in {} bytes but declares {} bytes",
                            entry.compressed_size, entry.size
                        ),
                    ));
                }
                raw.to_vec()
            }
            DEFLATED => inflate(entry, raw)?,
            method => {
                return Err(Error::Unsupported(format!(
                    "zip entry {} uses compression method {method}; OOXML packages use only \
                     stored and deflated entries",
                    entry.name
                )));
            }
        };
        let crc = crc32(&data);
        if crc != entry.crc32 {
            return Err(entry_error(
                entry,
                format!(
                    "crc-32 is {crc:08x} but the archive declares {:08x}",
                    entry.crc32
                ),
            ));
        }
        Ok(data)
    }
}

impl fmt::Debug for Archive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Archive")
            .field("len", &self.data.len())
            .field("entries", &self.entries)
            .finish_non_exhaustive()
    }
}

impl Entry {
    /// The entry name as stored in the archive, such as `word/document.xml`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The declared uncompressed size in bytes. [`Archive::read`] fails if the data
    /// does not match it.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The size of the stored, possibly compressed, data in bytes.
    pub fn compressed_size(&self) -> u64 {
        self.compressed_size
    }
}

/// Where the central directory is, from the end of central directory record.
struct End {
    entries: u64,
    directory_offset: usize,
    directory_size: usize,
}

/// A central directory record before its local header is checked.
struct CentralRecord<'a> {
    entry: Entry,
    raw_name: &'a [u8],
    flags: u16,
    header_offset: usize,
    next_record: usize,
}

/// The bytes an entry occupies, from its local header to the end of its data.
#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
    index: usize,
    next_record: usize,
}

/// Finds the end of central directory record, searching back from the end of the input
/// over the at most 65,535 bytes of archive comment. Some writers leave bytes after the
/// record (a real presentation in the POI corpus has one), so the last record whose
/// central directory checks out wins.
fn find_end(data: &[u8]) -> Result<End> {
    let Some(last) = data.len().checked_sub(END_LEN) else {
        return Err(zip_error(0, "the input is too short to be a zip archive"));
    };
    let first = last.saturating_sub(usize::from(u16::MAX));
    let mut first_error = None;
    for pos in (first..=last).rev() {
        if le32(data, pos) != Some(END_OF_CENTRAL_DIRECTORY) {
            continue;
        }
        match read_end(data, pos) {
            Ok(end) => return Ok(end),
            Err(err) => {
                first_error.get_or_insert(err);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| zip_error(data.len(), "no end of central directory record")))
}

fn read_end(data: &[u8], pos: usize) -> Result<End> {
    let comment = usize::from(read16(data, pos + 20)?);
    if pos + END_LEN + comment > data.len() {
        return Err(zip_error(pos, "the archive comment runs past the end"));
    }
    let locator = pos
        .checked_sub(ZIP64_LOCATOR_LEN)
        .filter(|&at| le32(data, at) == Some(ZIP64_LOCATOR));
    let (disk, directory_disk, disk_entries, entries, size, offset, directory_end) =
        if let Some(locator) = locator {
            let record_disk = read32(data, locator + 4)?;
            let record = read64(data, locator + 8)?;
            let disks = read32(data, locator + 16)?;
            if record_disk != 0 || disks > 1 {
                return Err(multi_volume());
            }
            let record = usize::try_from(record)
                .ok()
                .filter(|&at| {
                    at.checked_add(ZIP64_END_LEN)
                        .is_some_and(|end| end <= locator)
                })
                .ok_or_else(|| {
                    zip_error(
                        locator,
                        "the zip64 end of central directory locator points out of range",
                    )
                })?;
            if read32(data, record)? != ZIP64_END_OF_CENTRAL_DIRECTORY {
                return Err(zip_error(
                    record,
                    "bad zip64 end of central directory signature",
                ));
            }
            (
                read32(data, record + 16)?,
                read32(data, record + 20)?,
                read64(data, record + 24)?,
                read64(data, record + 32)?,
                read64(data, record + 40)?,
                read64(data, record + 48)?,
                record,
            )
        } else {
            (
                u32::from(read16(data, pos + 4)?),
                u32::from(read16(data, pos + 6)?),
                u64::from(read16(data, pos + 8)?),
                u64::from(read16(data, pos + 10)?),
                u64::from(read32(data, pos + 12)?),
                u64::from(read32(data, pos + 16)?),
                pos,
            )
        };
    if disk != 0 || directory_disk != 0 || disk_entries != entries {
        return Err(multi_volume());
    }
    let directory = usize::try_from(offset)
        .ok()
        .zip(usize::try_from(size).ok())
        .filter(|&(offset, size)| offset.checked_add(size) == Some(directory_end))
        .ok_or_else(|| {
            zip_error(
                pos,
                "the central directory does not end where the end of central directory \
                 record begins",
            )
        })?;
    Ok(End {
        entries,
        directory_offset: directory.0,
        directory_size: directory.1,
    })
}

fn read_central_record(data: &[u8], at: usize, directory_end: usize) -> Result<CentralRecord<'_>> {
    if read32(data, at)? != CENTRAL_HEADER {
        return Err(zip_error(at, "bad central directory record signature"));
    }
    let flags = read16(data, at + 8)?;
    let method = read16(data, at + 10)?;
    let crc32 = read32(data, at + 16)?;
    let compressed_size = read32(data, at + 20)?;
    let size = read32(data, at + 24)?;
    let name_len = usize::from(read16(data, at + 28)?);
    let extra_len = usize::from(read16(data, at + 30)?);
    let comment_len = usize::from(read16(data, at + 32)?);
    let disk = read16(data, at + 34)?;
    let header_offset = read32(data, at + 42)?;
    let name_at = at + CENTRAL_HEADER_LEN;
    let next_record = name_at + name_len + extra_len + comment_len;
    if next_record > directory_end {
        return Err(zip_error(
            at,
            "a central directory record runs past the end of the central directory",
        ));
    }
    let raw_name = slice(data, name_at, name_len)?;
    let extra = slice(data, name_at + name_len, extra_len)?;
    let name = std::str::from_utf8(raw_name)
        .map_err(|_| zip_error(name_at, "an entry name is not valid UTF-8"))?
        .to_owned();

    let mut size = u64::from(size);
    let mut compressed_size = u64::from(compressed_size);
    let mut header_offset = u64::from(header_offset);
    let mut disk = u32::from(disk);
    // Saturated fields are in the zip64 extra field, in this order (APPNOTE 4.5.3).
    if size == u64::from(u32::MAX)
        || compressed_size == u64::from(u32::MAX)
        || header_offset == u64::from(u32::MAX)
        || disk == u32::from(u16::MAX)
    {
        let missing = || zip_error(at, format!("entry {name} lacks its zip64 extra field"));
        let mut field = zip64_extra_field(extra).ok_or_else(missing)?;
        for value in [&mut size, &mut compressed_size, &mut header_offset] {
            if *value == u64::from(u32::MAX) {
                *value = le64(field, 0).ok_or_else(missing)?;
                field = field.get(8..).ok_or_else(missing)?;
            }
        }
        if disk == u32::from(u16::MAX) {
            disk = le32(field, 0).ok_or_else(missing)?;
        }
    }
    if disk != 0 {
        return Err(multi_volume());
    }
    if flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION) != 0 {
        return Err(encrypted(&name));
    }
    let header_offset = usize::try_from(header_offset).map_err(|_| {
        zip_error(
            at,
            format!("entry {name} has its local header out of range"),
        )
    })?;
    Ok(CentralRecord {
        entry: Entry {
            name,
            method,
            crc32,
            compressed_size,
            size,
            data_offset: 0,
        },
        raw_name,
        flags,
        header_offset,
        next_record,
    })
}

/// Checks that the local header agrees with the central directory and returns the entry
/// with its data offset, and the bytes it occupies.
fn check_local_header(
    data: &[u8],
    record: CentralRecord<'_>,
    directory_offset: usize,
) -> Result<(Entry, Span)> {
    let CentralRecord {
        mut entry,
        raw_name,
        flags,
        header_offset: at,
        next_record,
    } = record;
    if read32(data, at)? != LOCAL_HEADER {
        return Err(zip_error(
            at,
            format!("bad local header signature for entry {}", entry.name),
        ));
    }
    let local_flags = read16(data, at + 6)?;
    let local_method = read16(data, at + 8)?;
    let name_len = usize::from(read16(data, at + 26)?);
    let extra_len = usize::from(read16(data, at + 28)?);
    if slice(data, at + LOCAL_HEADER_LEN, name_len)? != raw_name {
        return Err(zip_error(
            at,
            format!(
                "the local header of entry {} names a different entry",
                entry.name
            ),
        ));
    }
    if local_method != entry.method {
        return Err(zip_error(
            at,
            format!(
                "the local header of entry {} gives compression method {local_method}, the \
                 central directory {}",
                entry.name, entry.method
            ),
        ));
    }
    if (local_flags | flags) & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION) != 0 {
        return Err(encrypted(&entry.name));
    }
    let data_offset = at + LOCAL_HEADER_LEN + name_len + extra_len;
    let end = usize::try_from(entry.compressed_size)
        .ok()
        .and_then(|len| data_offset.checked_add(len))
        .filter(|&end| end <= directory_offset)
        .ok_or_else(|| {
            zip_error(
                at,
                format!("entry {} runs into the central directory", entry.name),
            )
        })?;
    entry.data_offset = data_offset;
    Ok((
        entry,
        Span {
            start: at,
            end,
            index: 0,
            next_record,
        },
    ))
}

/// Rejects entries whose bytes overlap. Overlapping entries let a small archive expand
/// to far more data than it holds, and different readers disagree on them.
fn check_overlaps(spans: &mut [Span], entries: &[Entry]) -> Result<()> {
    for (index, span) in spans.iter_mut().enumerate() {
        span.index = index;
    }
    spans.sort_unstable_by_key(|span| span.start);
    for pair in spans.windows(2) {
        if let [first, second] = pair {
            if first.end > second.start {
                let name = |span: &Span| entries.get(span.index).map_or("", |e| e.name.as_str());
                return Err(zip_error(
                    second.start,
                    format!("entries {} and {} overlap", name(first), name(second)),
                ));
            }
        }
    }
    Ok(())
}

fn inflate(entry: &Entry, raw: &[u8]) -> Result<Vec<u8>> {
    if entry.size == 0 && raw.is_empty() {
        return Ok(Vec::new());
    }
    if entry.size > entry.compressed_size.saturating_mul(MAX_DEFLATE_RATIO) {
        return Err(entry_error(
            entry,
            format!(
                "declares {} bytes, more than {} bytes of deflate data can hold",
                entry.size, entry.compressed_size
            ),
        ));
    }
    let size = usize::try_from(entry.size)
        .map_err(|_| entry_error(entry, "is too large for this platform"))?;
    match miniz_oxide::inflate::decompress_to_vec_with_limit(raw, size) {
        Ok(data) if data.len() == size => Ok(data),
        Ok(data) => Err(entry_error(
            entry,
            format!("inflates to {} bytes but declares {size}", data.len()),
        )),
        Err(err) => Err(entry_error(
            entry,
            match err.status {
                TINFLStatus::HasMoreOutput => {
                    format!("inflates to more than the {size} bytes it declares")
                }
                TINFLStatus::NeedsMoreInput | TINFLStatus::FailedCannotMakeProgress => {
                    "deflate data is truncated".into()
                }
                _ => "deflate data is corrupt".into(),
            },
        )),
    }
}

/// The data of the zip64 extended information extra field, if present.
fn zip64_extra_field(mut extra: &[u8]) -> Option<&[u8]> {
    while extra.len() >= 4 {
        let id = le16(extra, 0)?;
        let len = usize::from(le16(extra, 2)?);
        let field = extra.get(4..4 + len)?;
        if id == ZIP64_EXTRA_FIELD {
            return Some(field);
        }
        extra = extra.get(4 + len..)?;
    }
    None
}

fn zip_error(offset: usize, message: impl Into<String>) -> Error {
    Error::Zip {
        offset: offset as u64,
        message: message.into(),
    }
}

fn entry_error(entry: &Entry, message: impl Into<String>) -> Error {
    Error::Entry {
        name: entry.name.clone(),
        message: message.into(),
    }
}

fn multi_volume() -> Error {
    Error::Unsupported("zip archives split over several disks are not supported".into())
}

fn encrypted(name: &str) -> Error {
    Error::Unsupported(format!("zip entry {name} is encrypted"))
}

fn slice(data: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    at.checked_add(len)
        .and_then(|end| data.get(at..end))
        .ok_or_else(|| zip_error(at, "unexpected end of the archive"))
}

fn read16(data: &[u8], at: usize) -> Result<u16> {
    le16(data, at).ok_or_else(|| zip_error(at, "unexpected end of the archive"))
}

fn read32(data: &[u8], at: usize) -> Result<u32> {
    le32(data, at).ok_or_else(|| zip_error(at, "unexpected end of the archive"))
}

fn read64(data: &[u8], at: usize) -> Result<u64> {
    le64(data, at).ok_or_else(|| zip_error(at, "unexpected end of the archive"))
}

fn le16(data: &[u8], at: usize) -> Option<u16> {
    let bytes = data.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

fn le32(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn le64(data: &[u8], at: usize) -> Option<u64> {
    let bytes = data.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

/// The CRC-32 of ZIP (ISO 3309, reflected polynomial 0xEDB88320).
pub(crate) fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &byte| {
        let index = usize::from((crc as u8) ^ byte);
        CRC_TABLE.get(index).copied().unwrap_or_default() ^ (crc >> 8)
    })
}

static CRC_TABLE: [u32; 256] = crc_table();

#[allow(clippy::indexing_slicing)] // reason: `byte` runs over 0..256, the table length
const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut byte = 0;
    while byte < 256 {
        let mut crc = byte as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[byte] = crc;
        byte += 1;
    }
    table
}

#[cfg(test)]
mod tests;
