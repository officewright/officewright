//! Embedded TrueType and OpenType fonts, written as Type 0 fonts whose descendant is a
//! CIDFont (ISO 32000-1, 9.7) with the Identity-H encoding: text is a string of
//! two-byte glyph IDs of the embedded subset.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use officewright_font::{GlyphId, Subset};

use crate::document::number;
use crate::font::{Embedded, Lookup};
use crate::stream::stream_object;
use crate::{Compression, Error, Result};

/// How many objects an embedded font has after its font dictionary: the CIDFont, the
/// font descriptor, the font program and the ToUnicode CMap.
pub(crate) const EXTRA_OBJECTS: usize = 4;

/// The glyphs a document shows in an embedded font, each with the character it showed
/// first: the one its ToUnicode entry gives.
#[derive(Clone, Debug, Default)]
pub(crate) struct Usage {
    glyphs: BTreeMap<u16, char>,
}

impl Usage {
    pub(crate) fn add(&mut self, glyph: u16, ch: char) {
        self.glyphs.entry(glyph).or_insert(ch);
    }
}

/// An embedded font reduced to the glyphs a document shows, ready to be written.
#[derive(Debug)]
pub(crate) struct Prepared<'a> {
    font: &'a Embedded,
    usage: Usage,
    subset: Subset,
    /// The CIDFont's `/BaseFont`: the PostScript name, after a subset tag unless the
    /// whole font is embedded.
    base_font: String,
}

impl<'a> Prepared<'a> {
    /// Subsets `font` to the glyphs in `usage`, or to all of them if its license
    /// forbids subsetting.
    ///
    /// # Errors
    ///
    /// [`Error::Embed`] if subsetting finds the font damaged.
    pub(crate) fn new(font: &'a Embedded, usage: Usage) -> Result<Self> {
        let face = &font.face;
        let glyphs: Vec<GlyphId> = if font.whole {
            (0..face.glyph_count()).map(GlyphId::new).collect()
        } else {
            usage.glyphs.keys().copied().map(GlyphId::new).collect()
        };
        let subset = face.subset(glyphs).map_err(|source| Error::Embed {
            font: font.name.clone(),
            source,
        })?;
        // A subset's name starts with a tag of six capital letters and a plus sign
        // (ISO 32000-1, 9.6.4); a whole font keeps its name.
        let base_font = if font.whole {
            font.name.clone()
        } else {
            format!("{}+{}", subset_tag(&font.name, subset.data()), font.name)
        };
        Ok(Self {
            font,
            usage,
            subset,
            base_font,
        })
    }

    /// The glyph's ID in the subset, which is its CID.
    fn cid(&self, glyph: u16) -> u16 {
        // The subset keeps every glyph it was made from, so .notdef is never written.
        self.subset
            .glyph_id(GlyphId::new(glyph))
            .map_or(0, GlyphId::to_u16)
    }

    /// Writes `text` to a content stream as hex strings of CIDs with `Tj` operators, and
    /// returns whether it needed replacement text.
    ///
    /// Where two characters share a glyph, its ToUnicode entry gives the one shown
    /// first; the other is shown in a marked-content span whose `/ActualText` gives
    /// the character (ISO 32000-1, 14.9.4), so text extraction returns the text as
    /// written. Such a span is a PDF 1.5 feature.
    pub(crate) fn show(&self, out: &mut Vec<u8>, lookup: &Lookup<'_>, text: &str) -> Result<bool> {
        let mut runs: Vec<(bool, Vec<(u16, char)>)> = Vec::new();
        for (offset, ch) in text.char_indices() {
            let glyph = lookup.glyph(ch, offset)?.id;
            let mapped = self.usage.glyphs.get(&glyph) == Some(&ch);
            let cid = self.cid(glyph);
            match runs.last_mut() {
                Some((last, run)) if *last == mapped => run.push((cid, ch)),
                _ => runs.push((mapped, vec![(cid, ch)])),
            }
        }
        if runs.is_empty() {
            out.extend_from_slice(b"<> Tj");
        }
        for (i, (mapped, run)) in runs.iter().enumerate() {
            if i > 0 {
                out.push(b'\n');
            }
            let mut cids = String::from("<");
            for (cid, _) in run {
                let _ = write!(cids, "{cid:04X}");
            }
            cids.push('>');
            if *mapped {
                out.extend_from_slice(format!("{cids} Tj").as_bytes());
            } else {
                let actual: String = run.iter().map(|&(_, ch)| ch).collect();
                out.extend_from_slice(
                    format!(
                        "/Span << /ActualText {} >> BDC {cids} Tj EMC",
                        text_string(&actual)
                    )
                    .as_bytes(),
                );
            }
        }
        Ok(runs.iter().any(|(mapped, _)| !mapped))
    }

    /// The font dictionary, a Type 0 font, whose other objects start at `first`.
    pub(crate) fn dictionary(&self, first: usize) -> Vec<u8> {
        // The Type 0 font's name is the CIDFont's, followed by the CMap's for a
        // CIDFontType0 (ISO 32000-1, 9.7.6.1).
        let name = match self.subset.cff_table() {
            Some(_) => format!("{}-Identity-H", self.base_font),
            None => self.base_font.clone(),
        };
        format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont {} /Encoding /Identity-H \
             /DescendantFonts [{first} 0 R] /ToUnicode {} 0 R >>",
            pdf_name(&name),
            first + 3
        )
        .into_bytes()
    }

    /// The objects after the font dictionary, numbered from `first`: the CIDFont, the
    /// font descriptor, the font program and the ToUnicode CMap.
    pub(crate) fn objects(
        &self,
        first: usize,
        compression: Compression,
    ) -> [Vec<u8>; EXTRA_OBJECTS] {
        let base_font = pdf_name(&self.base_font);
        let (subtype, file, program, program_entries) = match self.subset.cff_table() {
            Some(cff) => (
                "/CIDFontType0",
                "/FontFile3",
                cff,
                " /Subtype /CIDFontType0C".to_owned(),
            ),
            None => {
                let data = self.subset.data();
                // TrueType font programs give their length again (ISO 32000-1, 9.9).
                let entries = format!(" /Length1 {}", data.len());
                ("/CIDFontType2", "/FontFile2", data, entries)
            }
        };
        // For a CIDFontType2 the CIDs are glyph IDs; a CIDFontType0's CID-keyed font
        // program maps them itself.
        let cid_to_gid = if subtype == "/CIDFontType2" {
            " /CIDToGIDMap /Identity"
        } else {
            ""
        };
        let cid_font = format!(
            "<< /Type /Font /Subtype {subtype} /BaseFont {base_font} \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
             /FontDescriptor {} 0 R /W {}{cid_to_gid} >>",
            first + 1,
            self.widths()
        );
        let descriptor = format!(
            "<< /Type /FontDescriptor /FontName {base_font} {} {file} {} 0 R >>",
            self.metrics(),
            first + 2
        );
        [
            cid_font.into_bytes(),
            descriptor.into_bytes(),
            stream_object(&program_entries, program, compression),
            stream_object("", self.to_unicode().as_bytes(), compression),
        ]
    }

    /// Converts font units to 1/1000 em, the unit of glyph space.
    fn scale(&self, units: f64) -> f64 {
        units * 1000.0 / f64::from(self.font.face.units_per_em())
    }

    /// The `/W` array: the width of each glyph shown, in runs of consecutive CIDs.
    fn widths(&self) -> String {
        let mut runs: Vec<(u16, Vec<String>)> = Vec::new();
        for &glyph in self.usage.glyphs.keys() {
            let cid = self.cid(glyph);
            let advance = self
                .font
                .face
                .advance_width(GlyphId::new(glyph))
                .unwrap_or(0);
            let width = number(self.scale(f64::from(advance)));
            match runs.last_mut() {
                Some((start, widths)) if usize::from(*start) + widths.len() == usize::from(cid) => {
                    widths.push(width);
                }
                _ => runs.push((cid, vec![width])),
            }
        }
        let runs: Vec<String> = runs
            .iter()
            .map(|(start, widths)| format!("{start} [{}]", widths.join(" ")))
            .collect();
        format!("[{}]", runs.join(" "))
    }

    /// The font descriptor's entries from the font's metrics (ISO 32000-1, 9.8).
    fn metrics(&self) -> String {
        let face = &self.font.face;
        let bbox = face.bounding_box();
        let hhea = face.metrics().hhea();
        let ascent = self.scale(f64::from(hhea.ascender()));
        let cap_height = face
            .cap_height()
            .map_or(ascent, |height| self.scale(f64::from(height)));
        // Flags (Table 123): the glyphs are addressed by CID, not by a standard Latin
        // encoding, so the font is symbolic.
        let mut flags = 4;
        if face.is_fixed_pitch() {
            flags |= 1;
        }
        if face.is_italic() {
            flags |= 64;
        }
        // TrueType and CFF fonts record no stem width. A viewer uses StemV only to pick
        // a substitute when it cannot use the embedded font, so an estimate from the
        // weight class does: 88 for regular (400), 148 for bold (700).
        let stem_v = f64::from(face.weight()) / 5.0 + 8.0;
        format!(
            "/Flags {flags} /FontBBox [{} {} {} {}] /ItalicAngle {} /Ascent {} /Descent {} \
             /CapHeight {} /StemV {}",
            number(self.scale(f64::from(bbox.x_min()))),
            number(self.scale(f64::from(bbox.y_min()))),
            number(self.scale(f64::from(bbox.x_max()))),
            number(self.scale(f64::from(bbox.y_max()))),
            number(face.italic_angle().unwrap_or(0.0)),
            number(ascent),
            number(self.scale(f64::from(hhea.descender()))),
            number(cap_height),
            number(stem_v)
        )
    }

    /// The ToUnicode CMap: for each glyph shown, the character it showed first
    /// (ISO 32000-1, 9.10.3).
    fn to_unicode(&self) -> String {
        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
             /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
             /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
             1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        );
        let entries: Vec<(u16, char)> = self
            .usage
            .glyphs
            .iter()
            .map(|(&glyph, &ch)| (self.cid(glyph), ch))
            .collect();
        // A bfchar section holds at most 100 entries (Adobe Technical Note 5411, 1.4.1).
        for chunk in entries.chunks(100) {
            let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
            for &(cid, ch) in chunk {
                let mut units = [0; 2];
                let utf16: String = ch
                    .encode_utf16(&mut units)
                    .iter()
                    .map(|unit| format!("{unit:04X}"))
                    .collect();
                let _ = writeln!(cmap, "<{cid:04X}> <{utf16}>");
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend");
        cmap
    }
}

/// A PDF text string for `text` in UTF-16BE with a byte order mark, as a hexadecimal
/// string (ISO 32000-1, 7.9.2.2).
fn text_string(text: &str) -> String {
    let mut out = String::from("<FEFF");
    for unit in text.encode_utf16() {
        let _ = write!(out, "{unit:04X}");
    }
    out.push('>');
    out
}

/// `name` as a PDF name object: bytes outside the regular characters are written as
/// `#` and two hexadecimal digits (ISO 32000-1, 7.3.5).
pub(crate) fn pdf_name(name: &str) -> String {
    let mut out = String::from("/");
    for byte in name.bytes() {
        match byte {
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' | b'#' => {
                let _ = write!(out, "#{byte:02X}");
            }
            b'!'..=b'~' => out.push(char::from(byte)),
            _ => {
                let _ = write!(out, "#{byte:02X}");
            }
        }
    }
    out
}

/// Six capital letters that tell this subset from others (ISO 32000-1, 9.6.4): a hash of
/// the font's name and the subset's data, so different fonts or glyphs give different
/// tags, and equal documents give equal files.
fn subset_tag(name: &str, data: &[u8]) -> String {
    // 64-bit FNV-1a.
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for &byte in name.as_bytes().iter().chain(data) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    (0..6)
        .map(|i| char::from(b'A' + ((hash >> (10 * i)) % 26) as u8))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_names() {
        assert_eq!(pdf_name("LiberationSans"), "/LiberationSans");
        assert_eq!(pdf_name("A B#(1)/é"), "/A#20B#23#281#29#2F#C3#A9");
    }

    #[test]
    fn writes_text_strings_in_utf16() {
        assert_eq!(text_string("µ"), "<FEFF00B5>");
        assert_eq!(text_string("𝄞"), "<FEFFD834DD1E>");
    }

    #[test]
    fn subset_tags_are_six_capitals_and_depend_on_the_font_and_its_data() {
        let tag = subset_tag("Font", b"glyphs");
        assert_eq!(tag.len(), 6);
        assert!(tag.bytes().all(|b| b.is_ascii_uppercase()), "{tag}");
        assert_eq!(tag, subset_tag("Font", b"glyphs"));
        assert_ne!(tag, subset_tag("Font", b"glyphz"));
        assert_ne!(tag, subset_tag("Other", b"glyphs"));
    }
}
