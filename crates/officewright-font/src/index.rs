//! Reads the properties of every face in a font file without reading the whole file.
//!
//! A system has hundreds of fonts, and collections of East Asian fonts weigh tens of
//! megabytes each, so the index reads only the table directories and the `head`,
//! `OS/2`, `name` and `fvar` tables. The same code reads fonts in memory, through a
//! cursor. Every read is bounded, per table and per file, so a hostile file cannot make
//! the index read much more than its own size.

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use skrifa::raw::tables::fvar::Fvar;
use skrifa::raw::tables::name::Name;
use skrifa::raw::types::Tag;
use skrifa::raw::{FontData, FontRead, TableDirectory, TableProvider};

use skrifa::raw::tables::maxp::Maxp;

use crate::coverage::{Allowance, Coverage};
use crate::error::{Cause, Error};
use crate::face::{FaceProperties, english_rank};
use crate::font::{all_records, optional, required};

/// How much of a table the index reads.
#[derive(Clone, Copy)]
enum Bound {
    /// Only this many bytes: the parser looks no further.
    Prefix(u32),
    /// The whole table, which must not be longer.
    Whole(u32),
}

/// The tables the index reads. `head` is 54 bytes and `OS/2` at most 100 (version 5),
/// so longer tables are read only that far. `name` and `fvar` are read whole; a table
/// larger than any real font has makes the face count as damaged.
const TABLES: [(&[u8; 4], &str, Bound); 4] = [
    (b"head", "head", Bound::Prefix(54)),
    (b"OS/2", "OS/2", Bound::Prefix(100)),
    (b"name", "name", Bound::Whole(1 << 20)),
    (b"fvar", "fvar", Bound::Whole(64 << 10)),
];

/// The tables `FontFace::from_bytes` cannot do without; a face that lacks one is not
/// indexed, so every face in the index can be loaded.
const REQUIRED: [(&[u8; 4], &str); 4] = [
    (b"head", "head"),
    (b"maxp", "maxp"),
    (b"hhea", "hhea"),
    (b"hmtx", "hmtx"),
];

/// The most bytes the index reads from one file, table directories included. Real
/// collections need a few megabytes; the bound stops faces that all point at the same
/// large tables from multiplying the reads.
const MAX_READ_PER_FILE: u64 = 64 << 20;

/// The most faces read from one collection; real collections have a few dozen.
const MAX_FACES: u32 = 1024;

/// The most named instances read from one variable font; real ones have a few dozen.
const MAX_INSTANCES: usize = 256;

/// The largest `cmap` table the coverage of a face is read from. Fonts with the whole
/// of Unicode need a few hundred kilobytes.
const MAX_CMAP: u32 = 16 << 20;

/// How much reading the coverage of one file's faces may cost, all faces together.
#[derive(Clone, Copy)]
pub(crate) struct CoverageLimits {
    /// Bytes read, table directories included.
    pub(crate) read: u64,
    /// Segments, groups and glyph-array entries looked at; see [`Coverage::of_cmap`].
    pub(crate) steps: u64,
    /// Ranges of code points kept.
    pub(crate) ranges: usize,
}

impl CoverageLimits {
    /// The limits of a file. Real character maps take a few thousand steps and ranges, a
    /// few hundred thousand bytes; faces that share a `cmap` count once.
    pub(crate) const FILE: Self = Self {
        read: MAX_READ_PER_FILE,
        steps: 1 << 22,
        ranges: 1 << 16,
    };
}

/// The characters each of `faces` of a font file maps to a glyph, as
/// [`FontFace::char_map`](crate::FontFace::char_map) maps them, in the order of `faces`.
///
/// It reads like the index: the table directories, the `cmap` tables and the start of
/// the `maxp` tables, nothing else, within `limits` for the whole file. Faces that share
/// their `cmap` and `maxp` tables, as the faces of a collection usually do, share one
/// coverage. A face that cannot be read, or would go past the limits, covers nothing.
pub(crate) fn read_coverages<R: Read + Seek>(
    reader: &mut R,
    faces: &[u32],
    limits: CoverageLimits,
) -> Vec<Arc<Coverage>> {
    let nothing = Arc::new(Coverage::NOTHING);
    let Ok(file_len) = reader.seek(SeekFrom::End(0)) else {
        return vec![nothing; faces.len()];
    };
    let mut file = BoundedReader {
        reader,
        file_len,
        budget: limits.read,
        exhausted: false,
    };
    let mut allowance = Allowance {
        steps: limits.steps,
        ranges: limits.ranges,
    };
    let mut known = HashMap::new();
    faces
        .iter()
        .map(|&face| {
            read_face_coverage(&mut file, face, &mut allowance, &mut known)
                .unwrap_or_else(|| Arc::clone(&nothing))
        })
        .collect()
}

/// Where a face's `cmap` table and the part of its `maxp` table that is read are in the
/// file.
type TablesKey = (u32, u32, u32, u32);

/// The coverage of one face; `None` if it covers nothing.
fn read_face_coverage<R: Read + Seek>(
    file: &mut BoundedReader<'_, R>,
    face: u32,
    allowance: &mut Allowance,
    known: &mut HashMap<TablesKey, Option<Arc<Coverage>>>,
) -> Option<Arc<Coverage>> {
    let header = file.read_at(0, 12).ok()??;
    let offset = if header.get(..4) == Some(b"ttcf") {
        if face >= be_u32(&header, 8)? {
            return None;
        }
        let offset = file.read_at(12 + 4 * u64::from(face), 4).ok()??;
        u64::from(be_u32(&offset, 0)?)
    } else if face == 0 {
        0
    } else {
        return None;
    };
    let header = file.read_at(offset, 12).ok()??;
    let table_count = be_u16(&header, 4)?;
    let directory = file
        .read_at(offset, 12 + 16 * u64::from(table_count))
        .ok()??;
    let directory = TableDirectory::read(FontData::new(&directory)).ok()?;
    // The tables are found as `FontFace` finds them through read-fonts, so a face covers
    // nothing where its character map maps nothing: only TrueType and OpenType faces;
    // the first record of a tag, the only one when the tags are in order and searched
    // for by bisection; and a record with offset 0, or a table that does not fit in the
    // file, means the table is missing.
    if ![
        0x0001_0000,
        u32::from_be_bytes(*b"OTTO"),
        u32::from_be_bytes(*b"true"),
    ]
    .contains(&directory.sfnt_version())
    {
        return None;
    }
    let record = |tag: &[u8; 4]| {
        directory
            .table_records()
            .iter()
            .find(|record| record.tag() == Tag::new(tag))
            .filter(|record| record.offset() != 0)
            .filter(|record| {
                u64::from(record.offset()) + u64::from(record.length()) <= file.file_len
            })
            .map(|record| (record.offset(), record.length()))
    };
    let (cmap_offset, cmap_len) = record(b"cmap")?;
    let (maxp_offset, maxp_len) = record(b"maxp")?;
    // numGlyphs is at bytes 4 and 5; the parser wants the whole table of its version.
    let maxp_len = maxp_len.min(32);
    let key = (cmap_offset, cmap_len, maxp_offset, maxp_len);
    if let Some(coverage) = known.get(&key) {
        return coverage.clone();
    }
    let coverage = read_tables_coverage(file, key, allowance).map(Arc::new);
    known.insert(key, coverage.clone());
    coverage
}

/// The coverage of the `cmap` and `maxp` tables at `key`.
fn read_tables_coverage<R: Read + Seek>(
    file: &mut BoundedReader<'_, R>,
    (cmap_offset, cmap_len, maxp_offset, maxp_len): TablesKey,
    allowance: &mut Allowance,
) -> Option<Coverage> {
    if cmap_len > MAX_CMAP {
        return None;
    }
    let maxp = file
        .read_at(u64::from(maxp_offset), u64::from(maxp_len))
        .ok()??;
    let glyph_count = Maxp::read(FontData::new(&maxp)).ok()?.num_glyphs();
    let cmap = file
        .read_at(u64::from(cmap_offset), u64::from(cmap_len))
        .ok()??;
    Coverage::of_cmap(&cmap, glyph_count, allowance)
}

/// Why a font file could not be indexed.
#[derive(Debug)]
pub(crate) enum IndexError {
    /// Reading the file failed.
    Io(io::Error),
    /// The file is not a font.
    Font(Error),
}

impl From<io::Error> for IndexError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// What the index keeps of a face.
#[derive(Clone, Debug)]
pub(crate) struct IndexedFace {
    pub(crate) properties: FaceProperties,
    /// The named instances of a variable font, from its `fvar` table.
    pub(crate) instances: Vec<NamedInstance>,
}

/// A named instance of a variable font, such as "SemiBold Condensed".
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NamedInstance {
    /// The subfamily name, in English if the font has it.
    pub(crate) name: String,
    /// The `wght` coordinate, or 400 for a font without a weight axis.
    pub(crate) weight: u16,
    /// Whether the instance is italic or oblique.
    pub(crate) italic: bool,
}

/// What the index read of each face in the file, or why a face could not be read.
pub(crate) type Faces = Vec<(u32, Result<IndexedFace, Error>)>;

/// Indexes every face of a font file or collection.
pub(crate) fn read_faces<R: Read + Seek>(reader: &mut R) -> Result<Faces, IndexError> {
    let file_len = reader.seek(SeekFrom::End(0))?;
    let mut file = BoundedReader {
        reader,
        file_len,
        budget: MAX_READ_PER_FILE,
        exhausted: false,
    };
    let not_a_font = |message: &str| {
        IndexError::Font(Error::NotAFont {
            source: Cause::new(message),
        })
    };
    let header = file
        .read_at(0, 12)?
        .ok_or_else(|| not_a_font("the file is too short"))?;
    if header.get(..4) != Some(b"ttcf") {
        let face = read_face(&mut file, 0, 0)?;
        return match face {
            Err(Error::MalformedFace { source, .. }) => {
                Err(IndexError::Font(Error::NotAFont { source }))
            }
            face => Ok(vec![(0, face)]),
        };
    }
    let count = be_u32(&header, 8).unwrap_or_default();
    if count == 0 || count > MAX_FACES {
        return Err(not_a_font(
            "the collection header gives an invalid number of fonts",
        ));
    }
    let offsets = file
        .read_at(12, u64::from(count) * 4)?
        .ok_or_else(|| not_a_font("the collection header is past the end of the file"))?;
    let mut faces = Vec::new();
    for (index, offset) in (0..count).zip(offsets.chunks_exact(4)) {
        let offset = be_u32(offset, 0).unwrap_or_default();
        faces.push((index, read_face(&mut file, u64::from(offset), index)?));
        // Once the budget is spent, the other faces are reported together.
        if file.exhausted && index + 1 < count {
            let rest = index + 1;
            faces.push((
                rest,
                Err(Error::MalformedFace {
                    face: rest,
                    source: Cause::new(format!(
                        "faces {rest} to {} were not read: the file exceeds the {} MiB the \
                         index reads per file",
                        count - 1,
                        MAX_READ_PER_FILE >> 20
                    )),
                }),
            ));
            break;
        }
    }
    Ok(faces)
}

/// Reads one face. I/O errors abort the whole file; a damaged face gives an `Err` for
/// that face only.
fn read_face<R: Read + Seek>(
    file: &mut BoundedReader<'_, R>,
    offset: u64,
    face: u32,
) -> Result<Result<IndexedFace, Error>, IndexError> {
    let malformed = |message: &str| Error::MalformedFace {
        face,
        source: Cause::new(message),
    };
    let Some(header) = file.read_at(offset, 12)? else {
        return Ok(Err(malformed(&file.refusal("the table directory"))));
    };
    let table_count = be_u16(&header, 4).unwrap_or_default();
    let Some(directory) = file.read_at(offset, 12 + 16 * u64::from(table_count))? else {
        return Ok(Err(malformed(&file.refusal("the table directory"))));
    };
    let directory = match TableDirectory::read(FontData::new(&directory)) {
        Ok(directory) => directory,
        Err(err) => return Ok(Err(malformed(&err.to_string()))),
    };
    // The versions FontRef accepts: TrueType, CFF and Apple's 'true'.
    if ![0x0001_0000, 0x4F54_544F, 0x7472_7565].contains(&directory.sfnt_version()) {
        return Ok(Err(malformed("the sfnt version is unknown")));
    }
    let present: Vec<Tag> = directory.table_records().iter().map(|r| r.tag()).collect();
    for (tag, table) in REQUIRED {
        if !present.contains(&Tag::new(tag)) {
            return Ok(Err(Error::MissingTable { face, table }));
        }
    }
    let mut tables = Vec::new();
    for (tag, table, bound) in TABLES {
        // The first record wins if a damaged directory lists a table twice.
        let Some(record) = directory
            .table_records()
            .iter()
            .find(|record| record.tag() == Tag::new(tag))
        else {
            continue;
        };
        let damaged = |message: &str| Error::MalformedTable {
            face,
            table,
            source: Cause::new(message),
        };
        let len = match bound {
            Bound::Prefix(max) => record.length().min(max),
            Bound::Whole(max) if record.length() > max => {
                return Ok(Err(damaged("the table is implausibly large")));
            }
            Bound::Whole(_) => record.length(),
        };
        let Some(data) = file.read_at(u64::from(record.offset()), u64::from(len))? else {
            return Ok(Err(damaged(&file.refusal("the table"))));
        };
        tables.push((Tag::new(tag), data));
    }
    Ok(index_face(&tables, &present, face))
}

/// The properties of a face from the tables the index read.
fn index_face(tables: &[(Tag, Vec<u8>)], present: &[Tag], face: u32) -> Result<IndexedFace, Error> {
    let tables = TablesRef(tables);
    let head = required(tables.head(), face, "head")?;
    let units_per_em = head.units_per_em();
    if !(16..=16384).contains(&units_per_em) {
        return Err(Error::InvalidUnitsPerEm { face, units_per_em });
    }
    let os2 = optional(tables.os2(), face, "OS/2")?;
    let name = optional(tables.name(), face, "name")?;
    if let Some(name) = &name {
        all_records(name.name_record().len(), name.count(), face, "name")?;
    }
    let fvar = optional(tables.fvar(), face, "fvar")?;
    let properties = FaceProperties::read(&head, os2.as_ref(), name.as_ref(), |tag| {
        present.contains(&Tag::new(tag))
    });
    let instances = match (&fvar, &name) {
        (Some(fvar), Some(name)) => named_instances(fvar, name),
        _ => Vec::new(),
    };
    Ok(IndexedFace {
        properties,
        instances,
    })
}

/// The named instances of a variable font. Instances whose record or name cannot be
/// read are left out.
fn named_instances(fvar: &Fvar<'_>, name: &Name<'_>) -> Vec<NamedInstance> {
    let (Ok(axes), Ok(records)) = (fvar.axes(), fvar.instances()) else {
        return Vec::new();
    };
    let axis = |tag: &[u8; 4]| {
        axes.iter()
            .position(|axis| axis.axis_tag() == Tag::new(tag))
    };
    let (weight_axis, italic_axis, slant_axis) = (axis(b"wght"), axis(b"ital"), axis(b"slnt"));
    let records: Vec<_> = records
        .iter()
        .take(MAX_INSTANCES)
        .filter_map(Result::ok)
        .collect();
    // One pass over the name table for the names the instances use.
    let mut names: HashMap<u16, (u8, String)> = records
        .iter()
        .map(|record| (record.subfamily_name_id.to_u16(), (u8::MAX, String::new())))
        .collect();
    let data = name.string_data();
    for record in name.name_record() {
        let Some(best) = names.get_mut(&record.name_id().to_u16()) else {
            continue;
        };
        let rank = english_rank(record.platform_id(), record.language_id());
        if rank >= best.0 {
            continue;
        }
        let Ok(string) = record.string(data) else {
            continue;
        };
        let text: String = string.chars().collect();
        let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\0');
        if !text.is_empty() {
            *best = (rank, text.to_owned());
        }
    }
    let coordinate =
        |coordinates: &[skrifa::raw::types::BigEndian<skrifa::raw::types::Fixed>],
         axis: Option<usize>| {
            axis.and_then(|axis| coordinates.get(axis))
                .map(|value| value.get().to_f64())
        };
    records
        .iter()
        .filter_map(|record| {
            let (_, name) = names.get(&record.subfamily_name_id.to_u16())?;
            if name.is_empty() {
                return None;
            }
            let weight = coordinate(record.coordinates, weight_axis)
                .map_or(400, |weight| weight.round().clamp(1.0, 1000.0) as u16);
            let words = name.to_lowercase();
            let italic = coordinate(record.coordinates, italic_axis).is_some_and(|v| v >= 0.5)
                || coordinate(record.coordinates, slant_axis).is_some_and(|v| v != 0.0)
                || words
                    .split_whitespace()
                    .any(|w| w == "italic" || w == "oblique");
            Some(NamedInstance {
                name: name.clone(),
                weight,
                italic,
            })
        })
        .collect()
}

struct TablesRef<'a>(&'a [(Tag, Vec<u8>)]);

impl<'a> TableProvider<'a> for TablesRef<'a> {
    fn data_for_tag(&self, tag: Tag) -> Option<FontData<'a>> {
        self.0
            .iter()
            .find(|(known, _)| *known == tag)
            .map(|(_, data)| FontData::new(data))
    }
}

/// Reads parts of a file, within the file and within a total budget.
struct BoundedReader<'r, R> {
    reader: &'r mut R,
    file_len: u64,
    budget: u64,
    /// Whether a read was refused because the budget is spent.
    exhausted: bool,
}

impl<R: Read + Seek> BoundedReader<'_, R> {
    /// Reads `len` bytes at `offset`, `None` if they are not all in the file or the
    /// budget is spent.
    fn read_at(&mut self, offset: u64, len: u64) -> io::Result<Option<Vec<u8>>> {
        let in_file = offset
            .checked_add(len)
            .is_some_and(|end| end <= self.file_len);
        let Ok(size) = usize::try_from(len) else {
            return Ok(None);
        };
        if !in_file {
            return Ok(None);
        }
        if len > self.budget {
            self.exhausted = true;
            return Ok(None);
        }
        self.budget -= len;
        self.reader.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0; size];
        self.reader.read_exact(&mut data)?;
        Ok(Some(data))
    }

    /// Why the last read of `what` was refused.
    fn refusal(&self, what: &str) -> String {
        if self.exhausted {
            format!(
                "{what} is past the {} MiB the index reads per file",
                MAX_READ_PER_FILE >> 20
            )
        } else {
            format!("{what} is past the end of the file")
        }
    }
}

fn be_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::face::OutlineFormat;
    use crate::testing::{self, TestFont};

    fn index(data: &[u8]) -> Result<Faces, IndexError> {
        read_faces(&mut Cursor::new(data))
    }

    #[test]
    fn indexes_a_single_font() {
        let faces = index(&TestFont::new("Test Sans").style("Bold", 700, false).build()).unwrap();
        let [(0, Ok(face))] = faces.as_slice() else {
            panic!("{faces:?}");
        };
        assert_eq!(face.properties.names.family(), Some("Test Sans"));
        assert_eq!(face.properties.weight, 700);
        assert!(!face.properties.italic);
        assert_eq!(face.properties.outlines, Some(OutlineFormat::TrueType));
    }

    #[test]
    fn indexes_every_face_of_a_collection() {
        let ttc = testing::collection(&[
            TestFont::new("Test Sans").build(),
            TestFont::new("Test Serif").build(),
        ]);
        let faces = index(&ttc).unwrap();
        let families: Vec<_> = faces
            .iter()
            .map(|(index, face)| {
                (
                    *index,
                    face.as_ref().unwrap().properties.names.family().unwrap(),
                )
            })
            .collect();
        assert_eq!(families, [(0, "Test Sans"), (1, "Test Serif")]);
    }

    #[test]
    fn a_damaged_face_does_not_hide_the_others() {
        let mut ttc = testing::collection(&[
            TestFont::new("Test Sans").build(),
            TestFont::new("Test Serif").build(),
        ]);
        // Point the second face past the end of the file.
        ttc[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        let faces = index(&ttc).unwrap();
        assert!(faces[0].1.is_ok());
        assert!(matches!(
            faces[1].1,
            Err(Error::MalformedFace { face: 1, .. })
        ));
    }

    #[test]
    fn rejects_files_that_are_not_fonts() {
        for data in [&b""[..], b"hello, world", b"ttcf\0\x01\0\0\0\0\0\0"] {
            assert!(matches!(
                index(data),
                Err(IndexError::Font(Error::NotAFont { .. }))
            ));
        }
    }

    #[test]
    fn agrees_with_font() {
        let data = TestFont::new("Test Sans Light")
            .typographic("Test Sans", "Light")
            .style("Regular", 300, true)
            .fs_type(0x0004, 4)
            .build();
        let font = crate::FontFace::from_bytes(data.clone(), 0).unwrap();
        let faces = index(&data).unwrap();
        let face = faces[0].1.as_ref().unwrap();
        assert_eq!(&face.properties.names, font.names());
        assert_eq!(face.properties.weight, font.weight());
        assert_eq!(face.properties.italic, font.is_italic());
        assert_eq!(face.properties.embedding, font.embedding());
    }

    /// The byte offset of a table's record in a single font's table directory.
    fn record(font: &[u8], tag: &[u8; 4]) -> usize {
        let count = usize::from(u16::from_be_bytes([font[4], font[5]]));
        (0..count)
            .map(|i| 12 + 16 * i)
            .find(|at| &font[*at..*at + 4] == tag)
            .unwrap()
    }

    /// Whether each of `faces` of `data` covers `ch`, within `limits`.
    fn covers_with(data: &[u8], faces: &[u32], ch: char, limits: CoverageLimits) -> Vec<bool> {
        read_coverages(&mut Cursor::new(data), faces, limits)
            .iter()
            .map(|coverage| coverage.contains(ch))
            .collect()
    }

    /// Whether each of `faces` of `data` covers A.
    fn covers_a(data: &[u8], faces: &[u32]) -> Vec<bool> {
        covers_with(data, faces, 'A', CoverageLimits::FILE)
    }

    /// The table directory of face `face` of a collection.
    fn face_offset(ttc: &[u8], face: usize) -> usize {
        u32::from_be_bytes(ttc[12 + 4 * face..16 + 4 * face].try_into().unwrap()) as usize
    }

    #[test]
    fn coverage_reads_only_faces_the_file_has() {
        let font = TestFont::new("Test Sans").build();
        let ttc = testing::collection(&[font.clone(), font.clone()]);
        assert_eq!(covers_a(&ttc, &[0, 1, 2, 7]), [true, true, false, false]);
        assert_eq!(covers_a(&font, &[0, 1]), [true, false]);
        assert_eq!(covers_a(b"not a font", &[0]), [false]);
        // A header that lists one face, followed by the offset of another: only face 0.
        let mut one = ttc.clone();
        one[8..12].copy_from_slice(&1_u32.to_be_bytes());
        assert_eq!(covers_a(&one, &[0, 1]), [true, false]);
    }

    /// A collection of three faces with character maps of their own, each 1,500 groups
    /// of one code point and glyph 1: 1,500 steps and ranges, 18 kB, per face.
    fn three_big_maps() -> Vec<u8> {
        let groups: Vec<(u32, u32, u32)> =
            (0..1500).map(|i| (0x41 + 2 * i, 0x41 + 2 * i, 1)).collect();
        let font = TestFont::new("Test Big")
            .with_table(b"cmap", testing::cmap_groups(12, &groups))
            .build();
        testing::collection(&[font.clone(), font.clone(), font])
    }

    #[test]
    fn the_limits_of_a_file_hold_for_all_its_faces_together() {
        let ttc = three_big_maps();
        let all = [0, 1, 2];
        assert_eq!(
            covers_with(&ttc, &all, 'A', CoverageLimits::FILE),
            [true; 3]
        );
        let limits = |read, steps, ranges| CoverageLimits {
            read,
            steps,
            ranges,
        };
        // Room for two of the maps, whichever limit it is.
        let only_two = [true, true, false];
        assert_eq!(
            covers_with(&ttc, &all, 'A', limits(45_000, 1 << 20, 1 << 20)),
            only_two
        );
        assert_eq!(
            covers_with(&ttc, &all, 'A', limits(1 << 20, 3_100, 1 << 20)),
            only_two
        );
        assert_eq!(
            covers_with(&ttc, &all, 'A', limits(1 << 20, 1 << 20, 3_100)),
            only_two
        );
    }

    #[test]
    fn faces_share_coverage_only_when_they_share_maxp_too() {
        let font = TestFont::new("Test Sans").build();
        let mut ttc = testing::collection(&[font.clone(), font]);
        // Face 1 points at face 0's cmap, but its own maxp says it has 2 glyphs, so A,
        // glyph 2, is not one of them.
        share_table(&mut ttc, b"cmap", 0, 1);
        let second = face_offset(&ttc, 1);
        let maxp = second + record(&font_at(&ttc, second), b"maxp");
        let maxp_at = u32::from_be_bytes(ttc[maxp + 8..maxp + 12].try_into().unwrap()) as usize;
        ttc[maxp_at + 4..maxp_at + 6].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(covers_a(&ttc, &[0, 1]), [true, false]);
        assert_eq!(
            covers_with(&ttc, &[0, 1], ' ', CoverageLimits::FILE),
            [true, true]
        );
    }

    /// The bytes from `offset` on, so `record` reads the table directory there.
    fn font_at(ttc: &[u8], offset: usize) -> Vec<u8> {
        ttc[offset..].to_vec()
    }

    /// Points the `tag` record of face `to` of a collection at the table of face `from`.
    fn share_table(ttc: &mut [u8], tag: &[u8; 4], from: usize, to: usize) {
        let at = |face: usize| {
            let base = face_offset(ttc, face);
            base + record(&font_at(ttc, base), tag)
        };
        let (from, to) = (at(from), at(to));
        let location = ttc[from + 8..from + 16].to_vec();
        ttc[to + 8..to + 16].copy_from_slice(&location);
    }

    /// Sets the offset and the length in the `tag` record of a single font.
    fn set_record(font: &mut [u8], tag: &[u8; 4], offset: u32, len: u32) {
        let at = record(font, tag);
        font[at + 8..at + 12].copy_from_slice(&offset.to_be_bytes());
        font[at + 12..at + 16].copy_from_slice(&len.to_be_bytes());
    }

    /// Whether `font` loads with a character map that maps `ch`.
    fn maps(font: &[u8], ch: char) -> bool {
        crate::FontFace::from_bytes(font.to_vec(), 0)
            .is_ok_and(|face| face.char_map().map(ch).is_some())
    }

    #[test]
    fn a_table_at_offset_0_is_missing() {
        // An OpenType font whose cmap record has offset 0. Read from byte 0 on, the font
        // is a cmap whose header, `OTTO`, makes 0x544F encoding records; the last of them,
        // in a table of filler, is a Windows symbol record that points at a subtable for
        // A and U+F041. read-fonts finds no cmap, so the face maps nothing.
        let records_end = 4 + 8 * 0x544F;
        let subtable = testing::format4(
            &[
                (0x41, 0x41, 1_u16.wrapping_sub(0x41), 0),
                (0xF041, 0xF041, 1_u16.wrapping_sub(0xF041), 0),
            ],
            &[],
        );
        let mut font = TestFont::new("Test Zero")
            .with_table(b"zzzz", vec![0; records_end + subtable.len()])
            .build();
        font[..4].copy_from_slice(b"OTTO");
        let last = records_end - 8;
        font[last..last + 4].copy_from_slice(&[0, 3, 0, 0]);
        font[last + 4..last + 8].copy_from_slice(&(records_end as u32).to_be_bytes());
        font[records_end..records_end + subtable.len()].copy_from_slice(&subtable);
        let mut whole = font.clone();
        set_record(
            &mut whole,
            b"cmap",
            0,
            (records_end + subtable.len()) as u32,
        );
        for ch in ['A', '\u{F041}'] {
            assert!(!maps(&whole, ch));
            assert_eq!(covers_with(&whole, &[0], ch, CoverageLimits::FILE), [false]);
        }
        // Read from byte 0 on, a TrueType font is a maxp that says the font has as many
        // glyphs as tables, 12 here, so A, glyph 2 of 5, would be one of them.
        let mut font = TestFont::new("Test Sans")
            .with_table(b"zzzz", vec![0; 4])
            .with_table(b"yyyy", vec![0; 4])
            .build();
        assert_eq!(u16::from_be_bytes([font[4], font[5]]), 12);
        assert_eq!(covers_a(&font, &[0]), [true]);
        set_record(&mut font, b"maxp", 0, 32);
        assert_eq!(covers_a(&font, &[0]), [false]);
    }

    #[test]
    fn a_table_that_does_not_fit_in_the_file_is_missing() {
        let font = TestFont::new("Test Sans").build();
        let maxp = record(&font, b"maxp");
        let offset = u32::from_be_bytes(font[maxp + 8..maxp + 12].try_into().unwrap());
        // Only the first 32 bytes of maxp are read, but read-fonts wants all of it.
        let mut past = font.clone();
        set_record(&mut past, b"maxp", offset, font.len() as u32 - offset + 1);
        assert!(crate::FontFace::from_bytes(past.clone(), 0).is_err());
        assert_eq!(covers_a(&past, &[0]), [false]);
        let mut fits = font.clone();
        set_record(&mut fits, b"maxp", offset, font.len() as u32 - offset);
        assert!(maps(&fits, 'A'));
        assert_eq!(covers_a(&fits, &[0]), [true]);
    }

    #[test]
    fn coverage_reads_only_truetype_and_opentype_faces() {
        let font = TestFont::new("Test Sans").build();
        for version in [*b"OTTO", *b"true", [0, 1, 0, 0]] {
            let mut font = font.clone();
            font[..4].copy_from_slice(&version);
            assert_eq!(covers_a(&font, &[0]), [true]);
        }
        let mut other = font;
        other[..4].copy_from_slice(b"wOFF");
        assert_eq!(covers_a(&other, &[0]), [false]);
    }

    /// A collection of three faces with format 4 maps of their own, each 1,500
    /// segments of one code point and glyph 1, then a segment of 1,500 code points that
    /// read glyph 1 from the glyph ID array: 3,002 steps per face, with the closing
    /// segment, and 1,501 ranges.
    fn three_format4_maps() -> Vec<u8> {
        let mut segments: Vec<(u16, u16, u16, u16)> = (0..1500)
            .map(|i| {
                let code = 0x41 + 2 * i;
                (code, code, 1_u16.wrapping_sub(code), 0)
            })
            .collect();
        // The array segment is number 1,500 of 1,502 and reads from the array's start.
        segments.push((0x1000, 0x1000 + 1499, 0, 2 * (1502 - 1500)));
        let cmap = testing::cmap_of(&[(3, 1, testing::format4(&segments, &[1; 1500]))]);
        let font = TestFont::new("Test Segments")
            .with_table(b"cmap", cmap)
            .build();
        testing::collection(&[font.clone(), font.clone(), font])
    }

    #[test]
    fn segments_and_glyph_array_entries_are_steps() {
        let ttc = three_format4_maps();
        let all = [0, 1, 2];
        for ch in ['A', '\u{1000}', '\u{15DB}'] {
            assert_eq!(covers_with(&ttc, &all, ch, CoverageLimits::FILE), [true; 3]);
        }
        assert_eq!(
            covers_with(&ttc, &all, '\u{15DC}', CoverageLimits::FILE),
            [false; 3]
        );
        let limits = CoverageLimits {
            read: 1 << 20,
            steps: 6_100,
            ranges: 1 << 20,
        };
        assert_eq!(covers_with(&ttc, &all, 'A', limits), [true, true, false]);
    }

    #[test]
    fn faces_that_share_their_tables_count_once() {
        let mut ttc = three_big_maps();
        for face in [1, 2] {
            share_table(&mut ttc, b"cmap", 0, face);
            share_table(&mut ttc, b"maxp", 0, face);
        }
        // Each limit has room for two maps; all three faces read the same one.
        let limits = CoverageLimits {
            read: 45_000,
            steps: 3_100,
            ranges: 3_100,
        };
        assert_eq!(covers_with(&ttc, &[0, 1, 2], 'A', limits), [true; 3]);
    }

    #[test]
    fn coverage_refuses_huge_character_maps() {
        // The cmap moved to the end of the file, followed by zeros up to `len` bytes, all
        // of them in the file.
        let moved = |len: u32| {
            let mut font = TestFont::new("Test Sans").build();
            let at = record(&font, b"cmap");
            let offset = u32::from_be_bytes(font[at + 8..at + 12].try_into().unwrap()) as usize;
            let old_len = u32::from_be_bytes(font[at + 12..at + 16].try_into().unwrap()) as usize;
            let cmap = font[offset..offset + old_len].to_vec();
            let new_offset = u32::try_from(font.len()).unwrap();
            font[at + 8..at + 12].copy_from_slice(&new_offset.to_be_bytes());
            font[at + 12..at + 16].copy_from_slice(&len.to_be_bytes());
            font.extend_from_slice(&cmap);
            font.resize(new_offset as usize + len as usize, 0);
            font
        };
        assert_eq!(covers_a(&moved(4096), &[0]), [true]);
        assert_eq!(covers_a(&moved(MAX_CMAP), &[0]), [true]);
        assert_eq!(covers_a(&moved(MAX_CMAP + 1), &[0]), [false]);
    }

    fn cause(err: &Error) -> String {
        std::error::Error::source(err)
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    #[test]
    fn collections_have_at_most_1024_faces() {
        let header = |count: u32| {
            let mut ttc = b"ttcf\0\x01\0\0".to_vec();
            ttc.extend_from_slice(&count.to_be_bytes());
            ttc.resize(12 + 4 * count as usize, 0);
            ttc
        };
        assert_eq!(index(&header(1024)).unwrap().len(), 1024);
        let Err(IndexError::Font(err)) = index(&header(1025)) else {
            panic!("1025 faces accepted");
        };
        assert!(cause(&err).contains("invalid number of fonts"), "{err}");
    }

    #[test]
    fn rejects_implausibly_large_tables() {
        let mut font = TestFont::new("Test Sans").build();
        let at = record(&font, b"name") + 12;
        font[at..at + 4].copy_from_slice(&((1u32 << 20) + 1).to_be_bytes());
        let faces = index(&font).unwrap();
        let Err(err) = &faces[0].1 else {
            panic!("a name table over 1 MiB was read");
        };
        assert!(
            matches!(err, Error::MalformedTable { table: "name", .. }),
            "{err}"
        );
        assert!(cause(err).contains("implausibly large"), "{err}");
    }

    #[test]
    fn faces_sharing_large_tables_do_not_multiply_the_reads() {
        // 1024 faces point at one font whose name table is padded to 1 MiB, the most
        // the index reads of it: without a budget the index would read 1 GiB.
        let mut font = TestFont::new("Test Sans").build();
        let at = record(&font, b"name");
        let start = u32::from_be_bytes(font[at + 8..at + 12].try_into().unwrap()) as usize;
        let len = u32::from_be_bytes(font[at + 12..at + 16].try_into().unwrap()) as usize;
        let mut name = font[start..start + len].to_vec();
        name.resize(1 << 20, 0);
        let name_offset = font.len();
        font.extend_from_slice(&name);
        font[at + 8..at + 12].copy_from_slice(&(name_offset as u32).to_be_bytes());
        font[at + 12..at + 16].copy_from_slice(&(1u32 << 20).to_be_bytes());
        let header = 12 + 4 * 1024;
        let mut ttc = b"ttcf\0\x01\0\0\0\0\x04\0".to_vec();
        for _ in 0..1024 {
            ttc.extend_from_slice(&(header as u32).to_be_bytes());
        }
        let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
        for i in 0..tables {
            let at = 12 + 16 * i + 8;
            let offset = u32::from_be_bytes(font[at..at + 4].try_into().unwrap());
            font[at..at + 4].copy_from_slice(&(offset + header as u32).to_be_bytes());
        }
        ttc.extend_from_slice(&font);
        let faces = index(&ttc).unwrap();
        let read = faces.iter().filter(|(_, face)| face.is_ok()).count();
        assert!((1..64).contains(&read), "{read} faces read");
        // The faces left are reported once, not one by one.
        assert_eq!(faces.len(), read + 2);
        let (_, Err(last)) = faces.last().unwrap() else {
            panic!("the last face was read");
        };
        assert!(
            cause(last).ends_with(
                "to 1023 were not read: the file exceeds the 64 MiB the index reads per file"
            ),
            "{last}"
        );
    }

    #[test]
    fn refusals_say_whether_the_file_or_the_budget_ran_out() {
        let data = [0u8; 100];
        let mut cursor = Cursor::new(&data[..]);
        let mut file = BoundedReader {
            reader: &mut cursor,
            file_len: 100,
            budget: 50,
            exhausted: false,
        };
        assert_eq!(file.read_at(90, 10).unwrap().map(|d| d.len()), Some(10));
        assert_eq!(file.read_at(95, 10).unwrap(), None);
        assert_eq!(
            file.refusal("the table"),
            "the table is past the end of the file"
        );
        assert_eq!(file.read_at(0, 41).unwrap(), None);
        assert_eq!(
            file.refusal("the table"),
            "the table is past the 64 MiB the index reads per file"
        );
    }

    #[test]
    fn rejects_unknown_sfnt_versions() {
        let mut font = TestFont::new("Test Sans").build();
        font[..4].copy_from_slice(b"abcd");
        let Err(IndexError::Font(err)) = index(&font) else {
            panic!("an unknown sfnt version was accepted");
        };
        assert!(matches!(err, Error::NotAFont { .. }), "{err}");
        assert!(cause(&err).contains("sfnt version"), "{err}");
    }

    #[test]
    fn indexes_only_faces_that_font_can_load() {
        for table in [b"maxp", b"hhea", b"hmtx"] {
            let font = TestFont::new("Test Sans").without_table(table).build();
            assert!(
                matches!(index(&font).unwrap()[0].1, Err(Error::MissingTable { .. })),
                "{}",
                String::from_utf8_lossy(table)
            );
        }
        let font = TestFont::new("Test Sans").units_per_em(8).build();
        assert!(matches!(
            index(&font).unwrap()[0].1,
            Err(Error::InvalidUnitsPerEm { .. })
        ));
    }

    #[test]
    fn reads_the_named_instances_of_variable_fonts() {
        let axes = [(b"wght", 100.0, 400.0, 900.0), (b"ital", 0.0, 0.0, 1.0)];
        let instances: [(u16, &[f64]); 2] = [(256, &[600.0, 0.0]), (257, &[700.0, 1.0])];
        let font = TestFont::new("Test Var")
            .name_record(256, "SemiBold")
            .name_record(257, "Bold Italic")
            .with_table(b"fvar", testing::fvar(&axes, &instances))
            .build();
        let faces = index(&font).unwrap();
        let face = faces[0].1.as_ref().unwrap();
        assert!(face.properties.variable);
        assert_eq!(
            face.instances,
            [
                NamedInstance {
                    name: "SemiBold".into(),
                    weight: 600,
                    italic: false
                },
                NamedInstance {
                    name: "Bold Italic".into(),
                    weight: 700,
                    italic: true
                },
            ]
        );
    }

    #[test]
    fn a_name_table_must_hold_its_records() {
        // Declares 3 records and holds none, like in FontFace.
        let font = TestFont::new("Test Sans")
            .with_table(b"name", vec![0, 0, 0, 3, 0, 6])
            .build();
        assert!(matches!(
            index(&font).unwrap()[0].1,
            Err(Error::MalformedTable { table: "name", .. })
        ));
    }

    #[test]
    fn instances_are_italic_by_axis_or_name() {
        let axes = [(b"slnt", -12.0, 0.0, 0.0), (b"ital", 0.0, 0.0, 1.0)];
        let instances: [(u16, &[f64]); 4] = [
            (256, &[0.0, 0.0]),
            (257, &[-12.0, 0.0]),
            (258, &[0.0, 1.0]),
            (259, &[0.0, 0.0]),
        ];
        let font = TestFont::new("Test Var")
            .name_record(256, "Upright")
            .name_record(257, "Slanted")
            .name_record(258, "Cursive")
            .name_record(259, "Oblique")
            .with_table(b"fvar", testing::fvar(&axes, &instances))
            .build();
        let faces = index(&font).unwrap();
        let italic: Vec<_> = faces[0]
            .1
            .as_ref()
            .unwrap()
            .instances
            .iter()
            .map(|i| i.italic)
            .collect();
        assert_eq!(italic, [false, true, true, true]);
    }

    #[test]
    fn damaged_fonts_give_errors_not_panics() {
        let data = testing::collection(&[TestFont::new("Test Sans").build()]);
        for len in 0..data.len() {
            let _ = index(&data[..len]);
        }
        for at in 0..data.len() {
            for value in [0x00, 0x7F, 0xFF] {
                let mut damaged = data.clone();
                damaged[at] = value;
                let _ = index(&damaged);
            }
        }
    }
}
