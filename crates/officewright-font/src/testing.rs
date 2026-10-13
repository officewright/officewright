//! Builds small TrueType fonts for tests, so most tests need no font files.
//!
//! The glyphs: 0 `.notdef` (a box), 1 space (empty), 2 `A` (a triangle), 3 acute (a
//! small triangle) and 4 `Á`, a composite of `A` and the acute. The character map
//! covers U+0020, U+0041, U+00B4 and U+00C1, or U+F020, U+F041, U+F0B4 and U+F0C1 in a
//! symbol font.

use std::collections::BTreeMap;

/// Advance widths of glyphs 0 to 4.
pub(crate) const ADVANCES: [u16; 5] = [500, 250, 600, 300, 600];

/// The font's characters and their glyphs.
pub(crate) const CHARS: [(char, u16); 4] = [(' ', 1), ('A', 2), ('\u{B4}', 3), ('\u{C1}', 4)];

#[derive(Clone)]
pub(crate) struct TestFont {
    family: String,
    subfamily: String,
    typographic: Option<(String, String)>,
    localized_families: Vec<(u16, String)>,
    names: Vec<(u16, String)>,
    weight: u16,
    weight_class: Option<u16>,
    mac_style: Option<u16>,
    italic: bool,
    fs_type: u16,
    os2_version: u16,
    units_per_em: u16,
    symbol: bool,
    removed: Vec<[u8; 4]>,
    replaced: Vec<([u8; 4], Vec<u8>)>,
}

impl TestFont {
    pub(crate) fn new(family: &str) -> Self {
        Self {
            family: family.to_owned(),
            subfamily: "Regular".to_owned(),
            typographic: None,
            localized_families: Vec::new(),
            names: Vec::new(),
            weight: 400,
            weight_class: None,
            mac_style: None,
            italic: false,
            fs_type: 0,
            os2_version: 4,
            units_per_em: 1000,
            symbol: false,
            removed: Vec::new(),
            replaced: Vec::new(),
        }
    }

    /// Sets the subfamily name, the weight class and whether the face is italic; the
    /// style flags in `OS/2` and `head` follow.
    pub(crate) fn style(mut self, subfamily: &str, weight: u16, italic: bool) -> Self {
        self.subfamily = subfamily.to_owned();
        self.weight = weight;
        self.italic = italic;
        self
    }

    /// Adds a typographic family and subfamily (name IDs 16 and 17).
    pub(crate) fn typographic(mut self, family: &str, subfamily: &str) -> Self {
        self.typographic = Some((family.to_owned(), subfamily.to_owned()));
        self
    }

    /// Adds a family name in another language (a Windows language ID).
    pub(crate) fn localized_family(mut self, language: u16, family: &str) -> Self {
        self.localized_families.push((language, family.to_owned()));
        self
    }

    /// Adds an English name record, such as the subfamily name of a variable font's
    /// named instance (IDs 256 and up).
    pub(crate) fn name_record(mut self, id: u16, text: &str) -> Self {
        self.names.push((id, text.to_owned()));
        self
    }

    /// Writes this `usWeightClass` instead of the style's weight; the style flags still
    /// follow the style.
    pub(crate) fn weight_class(mut self, weight_class: u16) -> Self {
        self.weight_class = Some(weight_class);
        self
    }

    /// Writes this `macStyle` in `head` instead of the one the style implies.
    pub(crate) fn mac_style(mut self, mac_style: u16) -> Self {
        self.mac_style = Some(mac_style);
        self
    }

    pub(crate) fn fs_type(mut self, fs_type: u16, os2_version: u16) -> Self {
        self.fs_type = fs_type;
        self.os2_version = os2_version;
        self
    }

    pub(crate) fn units_per_em(mut self, units_per_em: u16) -> Self {
        self.units_per_em = units_per_em;
        self
    }

    /// Makes a symbol font: a Windows symbol `cmap` subtable at U+F0xx.
    pub(crate) fn symbol(mut self) -> Self {
        self.symbol = true;
        self
    }

    pub(crate) fn without_table(mut self, tag: &[u8; 4]) -> Self {
        self.removed.push(*tag);
        self
    }

    pub(crate) fn with_table(mut self, tag: &[u8; 4], data: Vec<u8>) -> Self {
        self.replaced.push((*tag, data));
        self
    }

    pub(crate) fn build(&self) -> Vec<u8> {
        let mut tables = BTreeMap::new();
        let (glyf, loca) = glyf_and_loca();
        tables.insert(*b"glyf", glyf);
        tables.insert(*b"loca", loca);
        tables.insert(*b"head", self.head());
        tables.insert(*b"hhea", hhea());
        tables.insert(*b"hmtx", hmtx());
        tables.insert(*b"maxp", maxp());
        tables.insert(*b"cmap", self.cmap());
        tables.insert(*b"OS/2", self.os2());
        tables.insert(*b"name", self.name());
        tables.insert(*b"post", self.post());
        for tag in &self.removed {
            tables.remove(tag);
        }
        for (tag, data) in &self.replaced {
            tables.insert(*tag, data.clone());
        }
        assemble(tables)
    }

    fn head_mac_style(&self) -> u16 {
        self.mac_style
            .unwrap_or(u16::from(self.weight >= 600) | (u16::from(self.italic) << 1))
    }

    fn head(&self) -> Vec<u8> {
        let mut t = Vec::new();
        put_u32(&mut t, 0x0001_0000); // version
        put_u32(&mut t, 0x0001_0000); // fontRevision
        put_u32(&mut t, 0); // checksumAdjustment, set by `assemble`
        put_u32(&mut t, 0x5F0F_3CF5); // magicNumber
        put_u16(&mut t, 0x000B); // flags
        put_u16(&mut t, self.units_per_em);
        t.extend_from_slice(&[0; 16]); // created, modified
        for v in [0i16, 0, 600, 850] {
            put_i16(&mut t, v); // xMin, yMin, xMax, yMax
        }
        put_u16(&mut t, self.head_mac_style());
        put_u16(&mut t, 8); // lowestRecPPEM
        put_i16(&mut t, 2); // fontDirectionHint
        put_i16(&mut t, 1); // indexToLocFormat: long
        put_i16(&mut t, 0); // glyphDataFormat
        t
    }

    fn cmap(&self) -> Vec<u8> {
        let base = if self.symbol { 0xF000 } else { 0 };
        let mut segments: Vec<(u16, u16)> = CHARS
            .iter()
            .map(|&(ch, glyph)| (base + ch as u16, glyph))
            .collect();
        segments.push((0xFFFF, 0));
        let seg_count = segments.len() as u16;
        let log2 = 15 - seg_count.leading_zeros() as u16;
        let search_range = 2 << log2;
        let mut sub = Vec::new();
        put_u16(&mut sub, 4); // format
        put_u16(&mut sub, 16 + 8 * seg_count); // length
        put_u16(&mut sub, 0); // language
        put_u16(&mut sub, seg_count * 2);
        put_u16(&mut sub, search_range);
        put_u16(&mut sub, log2);
        put_u16(&mut sub, seg_count * 2 - search_range);
        for &(code, _) in &segments {
            put_u16(&mut sub, code); // endCode
        }
        put_u16(&mut sub, 0); // reservedPad
        for &(code, _) in &segments {
            put_u16(&mut sub, code); // startCode
        }
        for &(code, glyph) in &segments {
            let delta = if code == 0xFFFF {
                1
            } else {
                glyph.wrapping_sub(code)
            };
            put_u16(&mut sub, delta); // idDelta
        }
        for _ in &segments {
            put_u16(&mut sub, 0); // idRangeOffset
        }
        let records: &[(u16, u16)] = if self.symbol {
            &[(3, 0)]
        } else {
            &[(0, 3), (3, 1)]
        };
        let mut t = Vec::new();
        put_u16(&mut t, 0); // version
        put_u16(&mut t, records.len() as u16);
        let offset = 4 + 8 * records.len() as u32;
        for &(platform, encoding) in records {
            put_u16(&mut t, platform);
            put_u16(&mut t, encoding);
            put_u32(&mut t, offset);
        }
        t.extend_from_slice(&sub);
        t
    }

    fn os2(&self) -> Vec<u8> {
        let mut selection = 0u16;
        if self.italic {
            selection |= 0x0001;
        }
        if self.weight >= 600 {
            selection |= 0x0020;
        }
        if selection == 0 {
            selection = 0x0040; // REGULAR
        }
        let mut t = Vec::new();
        put_u16(&mut t, self.os2_version);
        put_i16(&mut t, 450); // xAvgCharWidth
        put_u16(&mut t, self.weight_class.unwrap_or(self.weight));
        put_u16(&mut t, 5); // usWidthClass
        put_u16(&mut t, self.fs_type);
        for v in [650i16, 600, 0, 75, 650, 600, 0, 350, 50, 250] {
            put_i16(&mut t, v); // sub- and superscript, strikeout
        }
        put_i16(&mut t, 0); // sFamilyClass
        t.extend_from_slice(&[0; 10]); // panose
        for v in [0b11u32, 0, 0, 0] {
            put_u32(&mut t, v); // ulUnicodeRange1-4: Basic Latin, Latin-1 Supplement
        }
        t.extend_from_slice(b"OWRT"); // achVendID
        put_u16(&mut t, selection);
        put_u16(&mut t, 0x20); // usFirstCharIndex
        put_u16(&mut t, 0xC1); // usLastCharIndex
        put_i16(&mut t, 800); // sTypoAscender
        put_i16(&mut t, -200); // sTypoDescender
        put_i16(&mut t, 90); // sTypoLineGap
        put_u16(&mut t, 900); // usWinAscent
        put_u16(&mut t, 250); // usWinDescent
        if self.os2_version >= 1 {
            put_u32(&mut t, 1); // ulCodePageRange1: Latin 1
            put_u32(&mut t, 0);
        }
        if self.os2_version >= 2 {
            put_i16(&mut t, 500); // sxHeight
            put_i16(&mut t, 700); // sCapHeight
            put_u16(&mut t, 0); // usDefaultChar
            put_u16(&mut t, 0x20); // usBreakChar
            put_u16(&mut t, 0); // usMaxContext
        }
        t
    }

    fn name(&self) -> Vec<u8> {
        let full = if self.subfamily == "Regular" {
            self.family.clone()
        } else {
            format!("{} {}", self.family, self.subfamily)
        };
        let postscript: String = format!("{}-{}", self.family, self.subfamily)
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        // (language, name ID, string), sorted as the name table requires.
        let mut records = vec![
            (0x0409, 1, self.family.clone()),
            (0x0409, 2, self.subfamily.clone()),
            (0x0409, 4, full),
            (0x0409, 6, postscript),
        ];
        if let Some((family, subfamily)) = &self.typographic {
            records.push((0x0409, 16, family.clone()));
            records.push((0x0409, 17, subfamily.clone()));
        }
        for (language, family) in &self.localized_families {
            records.push((*language, 1, family.clone()));
        }
        for (id, text) in &self.names {
            records.push((0x0409, *id, text.clone()));
        }
        records.sort_by_key(|&(language, id, _)| (language, id));
        let mut storage = Vec::new();
        let mut t = Vec::new();
        put_u16(&mut t, 0); // format
        put_u16(&mut t, records.len() as u16);
        put_u16(&mut t, 6 + 12 * records.len() as u16); // storageOffset
        for (language, id, text) in &records {
            let encoded: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
            put_u16(&mut t, 3); // platform: Windows
            put_u16(&mut t, 1); // encoding: Unicode BMP
            put_u16(&mut t, *language);
            put_u16(&mut t, *id);
            put_u16(&mut t, encoded.len() as u16);
            put_u16(&mut t, storage.len() as u16);
            storage.extend_from_slice(&encoded);
        }
        t.extend_from_slice(&storage);
        t
    }

    fn post(&self) -> Vec<u8> {
        let mut t = Vec::new();
        put_u32(&mut t, 0x0003_0000); // version 3: no glyph names
        let angle: i32 = if self.italic { -12 << 16 } else { 0 };
        put_u32(&mut t, angle as u32); // italicAngle, 16.16
        put_i16(&mut t, -100); // underlinePosition
        put_i16(&mut t, 50); // underlineThickness
        t.extend_from_slice(&[0; 20]); // isFixedPitch, memory usage
        t
    }
}

pub(crate) fn hhea() -> Vec<u8> {
    let mut t = Vec::new();
    put_u32(&mut t, 0x0001_0000); // version
    put_i16(&mut t, 800); // ascender
    put_i16(&mut t, -200); // descender
    put_i16(&mut t, 90); // lineGap
    put_u16(&mut t, 600); // advanceWidthMax
    put_i16(&mut t, 0); // minLeftSideBearing
    put_i16(&mut t, 0); // minRightSideBearing
    put_i16(&mut t, 600); // xMaxExtent
    put_i16(&mut t, 1); // caretSlopeRise
    put_i16(&mut t, 0); // caretSlopeRun
    t.extend_from_slice(&[0; 10]); // caretOffset, reserved
    put_i16(&mut t, 0); // metricDataFormat
    put_u16(&mut t, ADVANCES.len() as u16); // numberOfHMetrics
    t
}

fn hmtx() -> Vec<u8> {
    let left_side_bearings = [50i16, 0, 0, 100, 0];
    let mut t = Vec::new();
    for (advance, lsb) in ADVANCES.iter().zip(left_side_bearings) {
        put_u16(&mut t, *advance);
        put_i16(&mut t, lsb);
    }
    t
}

fn maxp() -> Vec<u8> {
    let mut t = Vec::new();
    put_u32(&mut t, 0x0001_0000); // version 1.0
    for v in [ADVANCES.len() as u16, 4, 1, 6, 2, 2, 0, 0, 0, 0, 0, 0, 2, 1] {
        put_u16(&mut t, v);
    }
    t
}

/// A simple glyph with one contour of on-curve points.
fn simple_glyph(points: &[(i16, i16)]) -> Vec<u8> {
    let xs = points.iter().map(|p| p.0);
    let ys = points.iter().map(|p| p.1);
    let mut t = Vec::new();
    put_i16(&mut t, 1); // numberOfContours
    for v in [
        xs.clone().min().unwrap_or(0),
        ys.clone().min().unwrap_or(0),
        xs.max().unwrap_or(0),
        ys.max().unwrap_or(0),
    ] {
        put_i16(&mut t, v);
    }
    put_u16(&mut t, points.len() as u16 - 1); // endPtsOfContours
    put_u16(&mut t, 0); // instructionLength
    t.extend(std::iter::repeat_n(0x01u8, points.len())); // flags: on curve, 16-bit deltas
    let (mut x, mut y) = (0i16, 0i16);
    let mut dys = Vec::new();
    for &(px, py) in points {
        put_i16(&mut t, px - x);
        dys.push(py - y);
        (x, y) = (px, py);
    }
    for dy in dys {
        put_i16(&mut t, dy);
    }
    t
}

fn glyf_and_loca() -> (Vec<u8>, Vec<u8>) {
    let notdef = simple_glyph(&[(50, 0), (50, 700), (450, 700), (450, 0)]);
    let space = Vec::new();
    let a = simple_glyph(&[(0, 0), (300, 700), (600, 0)]);
    let acute = simple_glyph(&[(100, 600), (200, 750), (250, 700)]);
    let mut aacute = Vec::new();
    put_i16(&mut aacute, -1); // composite
    for v in [0i16, 0, 600, 850] {
        put_i16(&mut aacute, v);
    }
    // ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES | MORE_COMPONENTS, glyph 2 at (0, 0)
    put_u16(&mut aacute, 0x0023);
    put_u16(&mut aacute, 2);
    put_i16(&mut aacute, 0);
    put_i16(&mut aacute, 0);
    // ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES, glyph 3 at (100, 100)
    put_u16(&mut aacute, 0x0003);
    put_u16(&mut aacute, 3);
    put_i16(&mut aacute, 100);
    put_i16(&mut aacute, 100);
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for glyph in [notdef, space, a, acute, aacute] {
        put_u32(&mut loca, glyf.len() as u32);
        glyf.extend_from_slice(&glyph);
        while glyf.len() % 4 != 0 {
            glyf.push(0);
        }
    }
    put_u32(&mut loca, glyf.len() as u32);
    (glyf, loca)
}

/// Writes the table directory and the tables, with checksums.
fn assemble(tables: BTreeMap<[u8; 4], Vec<u8>>) -> Vec<u8> {
    let count = tables.len() as u16;
    let log2 = 15 - count.leading_zeros() as u16;
    let search_range = 16 << log2;
    let mut font = Vec::new();
    put_u32(&mut font, 0x0001_0000);
    put_u16(&mut font, count);
    put_u16(&mut font, search_range);
    put_u16(&mut font, log2);
    put_u16(&mut font, count * 16 - search_range);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    let mut head_offset = None;
    for (tag, data) in &tables {
        if tag == b"head" {
            head_offset = Some(offset);
        }
        font.extend_from_slice(tag);
        put_u32(&mut font, checksum(data));
        put_u32(&mut font, offset as u32);
        put_u32(&mut font, data.len() as u32);
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    font.extend_from_slice(&body);
    if let Some(at) = head_offset {
        if font.len() >= at + 12 {
            let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&font));
            font[at + 8..at + 12].copy_from_slice(&adjustment.to_be_bytes());
        }
    }
    font
}

/// An `fvar` table: axes as (tag, minimum, default, maximum), and named instances as
/// (subfamily name ID, coordinates).
pub(crate) fn fvar(axes: &[(&[u8; 4], f64, f64, f64)], instances: &[(u16, &[f64])]) -> Vec<u8> {
    let fixed = |v: f64| ((v * 65536.0).round() as i32) as u32;
    let mut t = Vec::new();
    put_u16(&mut t, 1); // majorVersion
    put_u16(&mut t, 0); // minorVersion
    put_u16(&mut t, 16); // axesArrayOffset
    put_u16(&mut t, 2); // reserved
    put_u16(&mut t, axes.len() as u16);
    put_u16(&mut t, 20); // axisSize
    put_u16(&mut t, instances.len() as u16);
    put_u16(&mut t, 4 + 4 * axes.len() as u16); // instanceSize
    for (tag, min, default, max) in axes {
        t.extend_from_slice(*tag);
        for v in [*min, *default, *max] {
            put_u32(&mut t, fixed(v));
        }
        put_u16(&mut t, 0); // flags
        put_u16(&mut t, 256); // axisNameID
    }
    for (name_id, coordinates) in instances {
        put_u16(&mut t, *name_id);
        put_u16(&mut t, 0); // flags
        for v in *coordinates {
            put_u32(&mut t, fixed(*v));
        }
    }
    t
}

/// A `name` table from records (platform, encoding, language, name ID, text), in
/// UTF-16BE for the Unicode and Windows platforms and in single bytes for Macintosh.
pub(crate) fn name_table(records: &[(u16, u16, u16, u16, &str)]) -> Vec<u8> {
    let mut t = Vec::new();
    let mut storage = Vec::new();
    put_u16(&mut t, 0); // format
    put_u16(&mut t, records.len() as u16);
    put_u16(&mut t, 6 + 12 * records.len() as u16); // storageOffset
    for &(platform, encoding, language, id, text) in records {
        let encoded: Vec<u8> = if platform == 1 {
            text.bytes().collect()
        } else {
            text.encode_utf16().flat_map(u16::to_be_bytes).collect()
        };
        for v in [platform, encoding, language, id] {
            put_u16(&mut t, v);
        }
        put_u16(&mut t, encoded.len() as u16);
        put_u16(&mut t, storage.len() as u16);
        storage.extend_from_slice(&encoded);
    }
    t.extend_from_slice(&storage);
    t
}

/// A `cmap` table with one Windows Unicode BMP subtable in format 4, made of `segments`
/// as given, `(startCode, endCode, idDelta)` each, with no glyph ID array, and the
/// closing segment at U+FFFF after them. Nothing is checked, so a test can build a
/// damaged subtable.
pub(crate) fn cmap_format4(segments: &[(u16, u16, u16)]) -> Vec<u8> {
    let segments: Vec<_> = segments
        .iter()
        .map(|&(start, end, delta)| (start, end, delta, 0))
        .collect();
    cmap_of(&[(3, 1, format4(&segments, &[]))])
}

/// A format 4 subtable of `segments`, `(startCode, endCode, idDelta, idRangeOffset)`
/// each, then the closing segment at U+FFFF, and `glyph_ids` as its glyph ID array.
/// Nothing is checked.
pub(crate) fn format4(segments: &[(u16, u16, u16, u16)], glyph_ids: &[u16]) -> Vec<u8> {
    let mut segments = segments.to_vec();
    segments.push((0xFFFF, 0xFFFF, 1, 0));
    let seg_count = segments.len() as u16;
    let log2 = 15 - seg_count.leading_zeros() as u16;
    let search_range = 2 << log2;
    let mut sub = Vec::new();
    put_u16(&mut sub, 4); // format
    put_u16(&mut sub, 16 + 8 * seg_count + 2 * glyph_ids.len() as u16); // length
    put_u16(&mut sub, 0); // language
    put_u16(&mut sub, seg_count * 2);
    put_u16(&mut sub, search_range);
    put_u16(&mut sub, log2);
    put_u16(&mut sub, (seg_count * 2).saturating_sub(search_range));
    for &(_, end, _, _) in &segments {
        put_u16(&mut sub, end);
    }
    put_u16(&mut sub, 0); // reservedPad
    for &(start, _, _, _) in &segments {
        put_u16(&mut sub, start);
    }
    for &(_, _, delta, _) in &segments {
        put_u16(&mut sub, delta);
    }
    for &(_, _, _, offset) in &segments {
        put_u16(&mut sub, offset);
    }
    for &glyph in glyph_ids {
        put_u16(&mut sub, glyph);
    }
    sub
}

/// A `cmap` table with one Windows Unicode full subtable in format 12 (each group maps
/// a range to consecutive glyphs) or 13 (each group maps a range to one glyph), made of
/// `groups`, `(startCharCode, endCharCode, glyphID)` each, as given.
pub(crate) fn cmap_groups(format: u16, groups: &[(u32, u32, u32)]) -> Vec<u8> {
    cmap_of(&[(3, 10, groups_subtable(format, groups))])
}

/// A format 12 or 13 subtable of `groups`, as given.
pub(crate) fn groups_subtable(format: u16, groups: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut sub = Vec::new();
    put_u16(&mut sub, format);
    put_u16(&mut sub, 0); // reserved
    put_u32(&mut sub, 16 + 12 * groups.len() as u32); // length
    put_u32(&mut sub, 0); // language
    put_u32(&mut sub, groups.len() as u32);
    for &(start, end, glyph) in groups {
        put_u32(&mut sub, start);
        put_u32(&mut sub, end);
        put_u32(&mut sub, glyph);
    }
    sub
}

/// A `cmap` table with an encoding record `(platformID, encodingID, subtable)` each, in
/// the order given.
pub(crate) fn cmap_of(records: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    let mut t = Vec::new();
    put_u16(&mut t, 0); // version
    put_u16(&mut t, records.len() as u16);
    let mut offset = 4 + 8 * records.len() as u32;
    for (platform, encoding, subtable) in records {
        put_u16(&mut t, *platform);
        put_u16(&mut t, *encoding);
        put_u32(&mut t, offset);
        offset += subtable.len() as u32;
    }
    for (_, _, subtable) in records {
        t.extend_from_slice(subtable);
    }
    t
}

/// Builds a font collection (`ttcf`) from single fonts.
pub(crate) fn collection(fonts: &[Vec<u8>]) -> Vec<u8> {
    let mut ttc = Vec::new();
    ttc.extend_from_slice(b"ttcf");
    put_u32(&mut ttc, 0x0001_0000);
    put_u32(&mut ttc, fonts.len() as u32);
    let mut start = 12 + 4 * fonts.len();
    for font in fonts {
        put_u32(&mut ttc, start as u32);
        start += font.len();
    }
    for font in fonts {
        let shift = ttc.len() as u32;
        let mut font = font.clone();
        let count = usize::from(u16::from_be_bytes([font[4], font[5]]));
        for record in 0..count {
            let at = 12 + 16 * record + 8;
            let offset = u32::from_be_bytes(font[at..at + 4].try_into().unwrap());
            font[at..at + 4].copy_from_slice(&(offset + shift).to_be_bytes());
        }
        ttc.extend_from_slice(&font);
    }
    ttc
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}
