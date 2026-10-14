//! What both [`FontFace`](crate::FontFace) and the font index read from a face: names,
//! style, embedding permissions and the outline format.

use std::fmt;

use skrifa::raw::tables::head::{Head, MacStyle};
use skrifa::raw::tables::name::Name;
use skrifa::raw::tables::os2::{Os2, SelectionFlags};

/// How the glyph outlines of a face are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OutlineFormat {
    /// Quadratic outlines in the `glyf` and `loca` tables (a `.ttf` file).
    TrueType,
    /// Cubic outlines in a `CFF ` table (an `.otf` file).
    Cff,
    /// Cubic, possibly variable outlines in a `CFF2` table.
    Cff2,
}

impl fmt::Display for OutlineFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TrueType => "TrueType",
            Self::Cff => "CFF",
            Self::Cff2 => "CFF2",
        })
    }
}

/// What the font's license, as recorded in the `fsType` field of the `OS/2` table,
/// allows a document that embeds the font to do with it.
///
/// A face without an `OS/2` table has no restrictions. The PDF writer decides what to
/// do with a face whose permissions forbid embedding or subsetting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EmbeddingPermissions {
    license: EmbeddingLicense,
    no_subsetting: bool,
    bitmap_only: bool,
}

/// The embedding permission level of a font (bits 0 to 3 of `fsType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EmbeddingLicense {
    /// The font may be embedded and permanently installed by the receiver.
    #[default]
    Installable,
    /// The font may be embedded and used to view and edit the document.
    Editable,
    /// The font may be embedded and used to view and print the document.
    PreviewAndPrint,
    /// The font must not be embedded.
    Restricted,
}

impl EmbeddingPermissions {
    /// Reads `fsType` from an `OS/2` table of the given version.
    pub(crate) fn from_fs_type(fs_type: u16, os2_version: u16) -> Self {
        // OpenType, OS/2 fsType: bits 0 to 3 should be mutually exclusive; if several
        // are set, the least restrictive permission takes precedence, and Restricted
        // License embedding takes effect only when the sub-field is exactly 2.
        let license = if fs_type & 0x0008 != 0 {
            EmbeddingLicense::Editable
        } else if fs_type & 0x0004 != 0 {
            EmbeddingLicense::PreviewAndPrint
        } else if fs_type & 0x000F == 0x0002 {
            EmbeddingLicense::Restricted
        } else {
            EmbeddingLicense::Installable
        };
        // Bits 8 and 9 exist from version 2; versions 0 and 1 must ignore bits 4 to 15.
        let flags = if os2_version >= 2 { fs_type } else { 0 };
        Self {
            license,
            no_subsetting: flags & 0x0100 != 0,
            bitmap_only: flags & 0x0200 != 0,
        }
    }

    /// The permission level.
    pub fn license(self) -> EmbeddingLicense {
        self.license
    }

    /// Whether the font may be embedded at all.
    pub fn allows_embedding(self) -> bool {
        self.license != EmbeddingLicense::Restricted
    }

    /// Whether the font may be embedded as a subset; if not, it must be embedded whole.
    pub fn allows_subsetting(self) -> bool {
        !self.no_subsetting
    }

    /// Whether only the font's bitmaps, not its outlines, may be embedded.
    pub fn bitmap_only(self) -> bool {
        self.bitmap_only
    }
}

/// The names of a face from its `name` table.
///
/// Word refers to fonts by the legacy family name (name ID 1), the one Windows GDI
/// uses: "Calibri Light" is a family of its own there, while its typographic family
/// (name ID 16) is "Calibri". Documents may also use a localized family name, so every
/// language's family name is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontNames {
    family: Option<String>,
    subfamily: Option<String>,
    full_name: Option<String>,
    postscript_name: Option<String>,
    typographic_family: Option<String>,
    typographic_subfamily: Option<String>,
    families: Vec<String>,
    typographic_families: Vec<String>,
    full_names: Vec<String>,
}

/// The most localized variants of one name kept per face; real fonts have a few dozen.
/// The bound keeps a malicious name table from making the index quadratic.
const MAX_LOCALIZED_NAMES: usize = 64;

/// The longest name string read, in bytes: 512 UTF-16 code units, far more than any name
/// a face is looked up or embedded by (Windows takes 31 characters of a family name, a
/// PostScript name has at most 63). Longer strings are skipped, which bounds the work of
/// reading a table: without it, 5,460 records of 64 KB each made a face take a second to
/// load, and records that repeat long strings differing only at their end made each new
/// one be compared in full with every name kept.
const MAX_NAME_BYTES: u16 = 1024;

impl FontNames {
    pub(crate) fn read(name: &Name<'_>) -> Self {
        let data = name.string_data();
        let mut names = Self::default();
        // Rank of the string currently in each English slot (lower is better).
        let mut ranks = [u8::MAX; 6];
        for record in name.name_record() {
            let id = record.name_id().to_u16();
            let slot = match id {
                1 => 0,
                2 => 1,
                4 => 2,
                6 => 3,
                16 => 4,
                17 => 5,
                _ => continue,
            };
            if record.length() > MAX_NAME_BYTES {
                continue;
            }
            let Ok(string) = record.string(data) else {
                continue;
            };
            let text: String = string.chars().collect();
            let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\0');
            if text.is_empty() {
                continue;
            }
            let rank = english_rank(record.platform_id(), record.language_id());
            if let Some(best) = ranks.get_mut(slot) {
                if rank < *best {
                    *best = rank;
                    let english = match slot {
                        0 => &mut names.family,
                        1 => &mut names.subfamily,
                        2 => &mut names.full_name,
                        3 => &mut names.postscript_name,
                        4 => &mut names.typographic_family,
                        _ => &mut names.typographic_subfamily,
                    };
                    *english = Some(text.to_owned());
                }
            }
            match id {
                1 => push_unique(&mut names.families, text),
                4 => push_unique(&mut names.full_names, text),
                16 => push_unique(&mut names.typographic_families, text),
                _ => {}
            }
        }
        names
    }

    /// The family name (name ID 1) in English, or in the first language the font has.
    pub fn family(&self) -> Option<&str> {
        self.family.as_deref()
    }

    /// The subfamily name (name ID 2), such as "Bold Italic".
    pub fn subfamily(&self) -> Option<&str> {
        self.subfamily.as_deref()
    }

    /// The full name (name ID 4), such as "Arial Bold Italic".
    pub fn full_name(&self) -> Option<&str> {
        self.full_name.as_deref()
    }

    /// The PostScript name (name ID 6), such as "Arial-BoldItalicMT".
    pub fn postscript_name(&self) -> Option<&str> {
        self.postscript_name.as_deref()
    }

    /// The typographic family name (name ID 16), if it differs from the family name.
    pub fn typographic_family(&self) -> Option<&str> {
        self.typographic_family.as_deref()
    }

    /// The typographic subfamily name (name ID 17), such as "Light".
    pub fn typographic_subfamily(&self) -> Option<&str> {
        self.typographic_subfamily.as_deref()
    }

    /// The family name (name ID 1) in every language the font has.
    pub fn families(&self) -> impl ExactSizeIterator<Item = &str> {
        self.families.iter().map(String::as_str)
    }

    /// The typographic family name (name ID 16) in every language the font has.
    pub fn typographic_families(&self) -> impl ExactSizeIterator<Item = &str> {
        self.typographic_families.iter().map(String::as_str)
    }

    /// The full name (name ID 4) in every language the font has.
    pub fn full_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.full_names.iter().map(String::as_str)
    }
}

/// How well a name record serves as the English name; lower is better.
pub(crate) fn english_rank(platform: u16, language: u16) -> u8 {
    match platform {
        // Windows, English (United States), then any other English locale.
        3 if language == 0x0409 => 0,
        3 if language & 0x03FF == 0x0009 => 1,
        // Unicode platform: no language.
        0 => 2,
        // Macintosh, English.
        1 if language == 0 => 3,
        _ => 4,
    }
}

fn push_unique(names: &mut Vec<String>, name: &str) {
    if names.len() < MAX_LOCALIZED_NAMES && !names.iter().any(|n| n == name) {
        names.push(name.to_owned());
    }
}

/// The properties of a face that the font index needs.
#[derive(Clone, Debug)]
pub(crate) struct FaceProperties {
    pub(crate) names: FontNames,
    pub(crate) weight: u16,
    pub(crate) italic: bool,
    pub(crate) embedding: EmbeddingPermissions,
    pub(crate) outlines: Option<OutlineFormat>,
    pub(crate) variable: bool,
}

impl FaceProperties {
    pub(crate) fn read(
        head: &Head<'_>,
        os2: Option<&Os2<'_>>,
        name: Option<&Name<'_>>,
        has_table: impl Fn(&[u8; 4]) -> bool,
    ) -> Self {
        let mac_style = head.mac_style();
        let mac_bold = mac_style.contains(MacStyle::BOLD);
        let mac_italic = mac_style.contains(MacStyle::ITALIC);
        let (weight, italic, embedding) = match os2 {
            Some(os2) => {
                let selection = os2.fs_selection();
                let bold = selection.contains(SelectionFlags::BOLD) || mac_bold;
                let weight = match os2.us_weight_class() {
                    weight @ 1..=1000 => weight,
                    _ => default_weight(bold),
                };
                let italic = selection.intersects(SelectionFlags::ITALIC | SelectionFlags::OBLIQUE)
                    || mac_italic;
                let embedding = EmbeddingPermissions::from_fs_type(os2.fs_type(), os2.version());
                (weight, italic, embedding)
            }
            None => (
                default_weight(mac_bold),
                mac_italic,
                EmbeddingPermissions::default(),
            ),
        };
        let outlines = if has_table(b"glyf") && has_table(b"loca") {
            Some(OutlineFormat::TrueType)
        } else if has_table(b"CFF ") {
            Some(OutlineFormat::Cff)
        } else if has_table(b"CFF2") {
            Some(OutlineFormat::Cff2)
        } else {
            None
        };
        Self {
            names: name.map(FontNames::read).unwrap_or_default(),
            weight,
            italic,
            embedding,
            outlines,
            variable: has_table(b"fvar"),
        }
    }
}

fn default_weight(bold: bool) -> u16 {
    if bold { 700 } else { 400 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn least_restrictive_permission_wins() {
        let editable_and_print = EmbeddingPermissions::from_fs_type(0x000C, 2);
        assert_eq!(editable_and_print.license(), EmbeddingLicense::Editable);
        assert_eq!(
            EmbeddingPermissions::from_fs_type(0x0004, 4).license(),
            EmbeddingLicense::PreviewAndPrint
        );
        let restricted = EmbeddingPermissions::from_fs_type(0x0002, 4);
        assert_eq!(restricted.license(), EmbeddingLicense::Restricted);
        assert!(!restricted.allows_embedding());
        assert!(EmbeddingPermissions::from_fs_type(0, 4).allows_embedding());
    }

    #[test]
    fn restricted_needs_exactly_bit_1() {
        // Bit 0 is reserved; with it set the sub-field is 3, not 2.
        assert_eq!(
            EmbeddingPermissions::from_fs_type(0x0003, 4).license(),
            EmbeddingLicense::Installable
        );
    }

    #[test]
    fn subsetting_and_bitmap_flags_need_version_2() {
        let v4 = EmbeddingPermissions::from_fs_type(0x0300, 4);
        assert!(!v4.allows_subsetting());
        assert!(v4.bitmap_only());
        let v1 = EmbeddingPermissions::from_fs_type(0x0300, 1);
        assert!(v1.allows_subsetting());
        assert!(!v1.bitmap_only());
    }

    fn names(records: &[(u16, u16, u16, u16, &str)]) -> FontNames {
        let data = crate::testing::name_table(records);
        use skrifa::raw::{FontData, FontRead};
        FontNames::read(&Name::read(FontData::new(&data)).unwrap())
    }

    /// A raw `name` table: Windows, Unicode BMP, US English records of name ID 1, each
    /// (offset, length) into `storage`.
    fn raw_name_table(records: &[(u16, u16)], storage: &[u8]) -> Vec<u8> {
        let count = u16::try_from(records.len()).unwrap();
        let mut table = Vec::new();
        for value in [0, count, 6 + 12 * count] {
            table.extend(value.to_be_bytes());
        }
        for &(offset, length) in records {
            for value in [3, 1, 0x409, 1, length, offset] {
                table.extend(u16::to_be_bytes(value));
            }
        }
        table.extend_from_slice(storage);
        table
    }

    fn read_raw(table: &[u8]) -> FontNames {
        use skrifa::raw::{FontData, FontRead};
        FontNames::read(&Name::read(FontData::new(table)).unwrap())
    }

    #[test]
    fn skips_names_longer_than_1024_bytes() {
        let utf16 = |text: String| {
            text.encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<u8>>()
        };
        let at_limit = utf16("A".repeat(512));
        let past_limit = utf16("B".repeat(513));
        let storage = [past_limit.clone(), at_limit.clone()].concat();
        let names = read_raw(&raw_name_table(&[(0, 1026), (1026, 1024)], &storage));
        assert_eq!(names.family(), Some("A".repeat(512).as_str()));
        assert_eq!(names.families().count(), 1);
    }

    #[test]
    fn reads_tables_built_to_be_slow_quickly() {
        // 5,460 records, as many as a table whose strings start within 64 KB can have,
        // each pointing at the same string of 65,534 bytes; and 5,460 records whose first
        // 63 strings of 1,024 bytes differ only at their end, the rest repeating the 63rd,
        // so that each is compared in full with every name kept.
        let long = "A"
            .repeat(32_767)
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<u8>>();
        let same = raw_name_table(&vec![(0, 65_534); 5460], &long);
        let tails: String = (0..64)
            .map(|i| char::from_u32(0x4E00 + i).unwrap())
            .collect();
        let storage = format!("{}{tails}", "A".repeat(511))
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<u8>>();
        let records: Vec<(u16, u16)> = (0..5460u16).map(|i| (2 * i.min(62) + 2, 1024)).collect();
        let differing = raw_name_table(&records, &storage);
        let started = std::time::Instant::now();
        let (same, differing) = (read_raw(&same), read_raw(&differing));
        // Unbounded, the first took seconds to read even in a release build.
        assert!(
            started.elapsed().as_secs_f64() < 1.0,
            "{:?}",
            started.elapsed()
        );
        assert_eq!(same.family(), None);
        assert_eq!(differing.families().count(), 63);
    }

    #[test]
    fn the_first_name_of_the_best_rank_wins() {
        let names = names(&[(3, 1, 0x409, 1, "First"), (3, 1, 0x409, 1, "Second")]);
        assert_eq!(names.family(), Some("First"));
        assert_eq!(names.families().collect::<Vec<_>>(), ["First", "Second"]);
    }

    #[test]
    fn names_are_trimmed_and_empty_ones_skipped() {
        let names = names(&[(3, 1, 0x409, 1, " Padded\0"), (3, 1, 0x409, 2, " \0 ")]);
        assert_eq!(names.family(), Some("Padded"));
        assert_eq!(names.subfamily(), None);
    }

    #[test]
    fn unicode_platform_names_rank_between_windows_and_mac() {
        let mac_and_unicode = [(1, 0, 0, 1, "Mac"), (0, 3, 0, 1, "Unicode")];
        assert_eq!(names(&mac_and_unicode).family(), Some("Unicode"));
        let mut all = mac_and_unicode.to_vec();
        all.push((3, 1, 0x809, 1, "British"));
        assert_eq!(names(&all).family(), Some("British"));
    }

    #[test]
    fn localized_names_are_bounded() {
        let texts: Vec<String> = (0..70).map(|i| format!("Family {i}")).collect();
        let records: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, text)| (3, 1, 0x400 + i as u16, 1, text.as_str()))
            .collect();
        assert_eq!(names(&records).families().len(), MAX_LOCALIZED_NAMES);
    }

    #[test]
    fn english_names_are_preferred() {
        assert!(english_rank(3, 0x0409) < english_rank(3, 0x0809));
        assert!(english_rank(3, 0x0809) < english_rank(1, 0));
        assert!(english_rank(1, 0) < english_rank(3, 0x0804));
    }
}
