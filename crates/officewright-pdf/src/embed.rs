//! Embedded TrueType and OpenType fonts, written as Type 0 fonts whose descendant is a
//! CIDFont (ISO 32000-1, 9.7) with the Identity-H encoding: text is a string of
//! two-byte glyph IDs of the embedded subset.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use officewright_font::{GlyphId, Subset};

use crate::document::number;
use crate::font::{Embedded, Lookup};
use crate::glyphs::MAX_STRING_BYTES;
use crate::stream::stream_object;
use crate::{Compression, Error, Result};

/// How many objects an embedded font has after its font dictionary: the CIDFont, the
/// font descriptor, the font program and the ToUnicode CMap.
pub(crate) const EXTRA_OBJECTS: usize = 4;

/// The glyphs a document shows in an embedded font, each with the text its ToUnicode
/// entry gives: the first text it was shown alone for, if any.
#[derive(Clone, Debug, Default)]
pub(crate) struct Usage {
    glyphs: BTreeMap<u16, Option<String>>,
}

impl Usage {
    /// Records that `glyph` was shown alone for `text`, which becomes its ToUnicode entry
    /// unless it has one already.
    pub(crate) fn add(&mut self, glyph: u16, text: &str) {
        let entry = self.glyphs.entry(glyph).or_default();
        if entry.is_none() {
            *entry = Some(text.to_owned());
        }
    }

    /// Records that `glyph` was shown together with other glyphs for some text, which
    /// gives it no ToUnicode entry.
    pub(crate) fn add_unmapped(&mut self, glyph: u16) {
        self.glyphs.entry(glyph).or_default();
    }

    /// The text of the ToUnicode entry of `glyph`.
    pub(crate) fn text(&self, glyph: u16) -> Option<&str> {
        self.glyphs.get(&glyph).and_then(Option::as_deref)
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

    /// The glyphs shown and their text.
    pub(crate) fn usage(&self) -> &Usage {
        &self.usage
    }

    /// The glyph's ID in the subset, which is its CID.
    pub(crate) fn cid(&self, glyph: u16) -> u16 {
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
    ///
    /// Long text is shown with several strings, each within the 32767 bytes readers
    /// have to handle in PDF 1.5 (ISO 32000-1, Annex C); one `Tj` after another moves
    /// on as one would.
    pub(crate) fn show(&self, out: &mut Vec<u8>, lookup: &Lookup<'_>, text: &str) -> Result<bool> {
        let mut runs: Vec<(bool, Vec<(u16, char)>)> = Vec::new();
        for (offset, ch) in text.char_indices() {
            let glyph = lookup.glyph(ch, offset)?.id;
            let mapped = self.usage.text(glyph) == Some(&*ch.encode_utf8(&mut [0; 4]));
            let cid = self.cid(glyph);
            match runs.last_mut() {
                Some((last, run)) if *last == mapped => run.push((cid, ch)),
                _ => runs.push((mapped, vec![(cid, ch)])),
            }
        }
        if runs.is_empty() {
            out.extend_from_slice(b"<> Tj");
        }
        let pieces = runs
            .iter()
            .flat_map(|(mapped, run)| pieces(run).map(move |piece| (*mapped, piece)));
        for (i, (mapped, piece)) in pieces.enumerate() {
            if i > 0 {
                out.push(b'\n');
            }
            let mut cids = String::from("<");
            for (cid, _) in piece {
                let _ = write!(cids, "{cid:04X}");
            }
            cids.push('>');
            if mapped {
                out.extend_from_slice(format!("{cids} Tj").as_bytes());
            } else {
                let actual: String = piece.iter().map(|&(_, ch)| ch).collect();
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
            Some(_) => format!("{}{CMAP_SUFFIX}", self.base_font),
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

    /// The advance width of `glyph` in 1/1000 em, as the `/W` array gives it.
    pub(crate) fn width(&self, glyph: u16) -> f64 {
        let width = number(self.advance(glyph));
        width.parse().unwrap_or_else(|_| self.advance(glyph))
    }

    /// The advance width of `glyph` in 1/1000 em.
    fn advance(&self, glyph: u16) -> f64 {
        let advance = self
            .font
            .face
            .advance_width(GlyphId::new(glyph))
            .unwrap_or(0);
        self.scale(f64::from(advance))
    }

    /// The `/W` array: the width of each glyph shown, in runs of consecutive CIDs.
    fn widths(&self) -> String {
        let mut runs: Vec<(u16, Vec<String>)> = Vec::new();
        for &glyph in self.usage.glyphs.keys() {
            let cid = self.cid(glyph);
            let width = number(self.advance(glyph));
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

    /// The ToUnicode CMap: for each glyph shown alone, the text it showed first
    /// (ISO 32000-1, 9.10.3).
    fn to_unicode(&self) -> String {
        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
             /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
             /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
             1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        );
        let entries: Vec<(u16, &str)> = self
            .usage
            .glyphs
            .iter()
            .filter_map(|(&glyph, text)| Some((self.cid(glyph), text.as_deref()?)))
            .collect();
        // A bfchar section holds at most 100 entries (Adobe Technical Note 5411, 1.4.1).
        for chunk in entries.chunks(100) {
            let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
            for &(cid, text) in chunk {
                let utf16: String = text
                    .encode_utf16()
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
pub(crate) fn text_string(text: &str) -> String {
    let mut out = String::from("<FEFF");
    for unit in text.encode_utf16() {
        let _ = write!(out, "{unit:04X}");
    }
    out.push('>');
    out
}

/// The most UTF-16 code units of text in a piece of a run that [`Prepared::show`] writes
/// with one string. Its replacement text, two bytes a unit after a two-byte byte order
/// mark, then fits in a string, and so do its CIDs, two bytes for each character, which
/// has one or two units.
const MAX_PIECE_UNITS: usize = (MAX_STRING_BYTES - 2) / 2;

/// `run` cut into pieces of at most [`MAX_PIECE_UNITS`] UTF-16 code units of text; an
/// empty run is one empty piece.
fn pieces(run: &[(u16, char)]) -> impl Iterator<Item = &[(u16, char)]> {
    let mut rest = run;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let mut units = 0;
        let end = rest
            .iter()
            .position(|&(_, ch)| {
                units += ch.len_utf16();
                units > MAX_PIECE_UNITS
            })
            .unwrap_or(rest.len());
        let (piece, after) = rest.split_at(end);
        rest = after;
        done = rest.is_empty();
        Some(piece)
    })
}

/// `name` as a PDF name object: bytes outside the regular characters are written as
/// `#` and two hexadecimal digits (ISO 32000-1, 7.3.5).
pub(crate) fn pdf_name(name: &str) -> String {
    let mut out = String::from("/");
    for byte in name.bytes() {
        if regular(byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "#{byte:02X}");
        }
    }
    out
}

/// Whether `byte` stands for itself in a PDF name, rather than as `#` and two
/// hexadecimal digits: a printable ASCII character other than a delimiter or `#`
/// (ISO 32000-1, 7.2.2 and 7.3.5).
fn regular(byte: u8) -> bool {
    matches!(byte, b'!'..=b'~')
        && !matches!(
            byte,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' | b'#'
        )
}

/// The longest name, in bytes, that a reader has to accept (ISO 32000-1, Annex C).
///
/// The standard counts a name's bytes after its `#xx` escapes are decoded (7.3.5); the
/// names of embedded fonts are kept within it as written, escapes and all, which is
/// within it by either count.
const MAX_NAME_BYTES: usize = 127;

/// The letters of a subset tag.
const SUBSET_TAG_LETTERS: usize = 6;

/// What the name of a CIDFontType0's Type 0 font adds to the CIDFont's: the name of its
/// CMap (ISO 32000-1, 9.7.6.1).
const CMAP_SUFFIX: &str = "-Identity-H";

/// The start of a font's PostScript name `name` that its PDF names can hold: with the
/// subset tag unless the `whole` font is embedded, and with the CMap name for CFF
/// outlines (`cff`), every name the font has in the file takes at most
/// [`MAX_NAME_BYTES`] as written. A name is cut between characters, so never inside the
/// escapes of one. Null characters, which a PDF name cannot hold even escaped
/// (ISO 32000-1, 7.3.5), are left out first.
pub(crate) fn fit_font_name(name: &str, whole: bool, cff: bool) -> String {
    let name: String = name.chars().filter(|&ch| ch != '\0').collect();
    let mut room = MAX_NAME_BYTES;
    if !whole {
        // The tag and the plus sign after it.
        room -= SUBSET_TAG_LETTERS + 1;
    }
    if cff {
        room -= CMAP_SUFFIX.len();
    }
    let mut used = 0;
    let end = name
        .char_indices()
        .find(|&(_, ch)| {
            used += ch
                .encode_utf8(&mut [0; 4])
                .bytes()
                .map(|byte| if regular(byte) { 1 } else { 3 })
                .sum::<usize>();
            used > room
        })
        .map_or(name.len(), |(at, _)| at);
    name.get(..end).unwrap_or(&name).to_owned()
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
    (0..SUBSET_TAG_LETTERS)
        .map(|i| char::from(b'A' + ((hash >> (10 * i)) % 26) as u8))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_runs_where_their_strings_would_pass_the_limit() {
        let lengths = |run: &[(u16, char)]| -> Vec<usize> {
            // The pieces follow one another and together are the run.
            assert_eq!(pieces(run).collect::<Vec<_>>().concat(), run);
            pieces(run).map(<[_]>::len).collect()
        };
        // Every element different, so a piece in the wrong place shows.
        let numbered = |count: usize, ch: char| -> Vec<(u16, char)> {
            (0..count).map(|i| (i as u16, ch)).collect()
        };
        // One unit a character: 16,382 of them a piece.
        assert_eq!(lengths(&numbered(40_000, 'A')), [16_382, 16_382, 7_236]);
        // Characters outside the Basic Multilingual Plane take two units.
        assert_eq!(lengths(&numbered(10_000, '\u{1F600}')), [8_191, 1_809]);
        let mixed: Vec<(u16, char)> = [(1, 'A'), (1, '\u{1F600}')].repeat(6_000);
        // 5,460 pairs of three units and one A: 16,381 units; the next character makes 16,383.
        assert_eq!(lengths(&mixed), [10_921, 1_079]);
        assert_eq!(lengths(&[]), [0]);
    }

    #[test]
    fn escapes_names() {
        assert_eq!(pdf_name("LiberationSans"), "/LiberationSans");
        assert_eq!(pdf_name("A B#(1)/é"), "/A#20B#23#281#29#2F#C3#A9");
    }

    #[test]
    fn cuts_font_names_to_the_longest_name_readers_accept() {
        let long = "Long".repeat(50);
        // A subset's tag and plus sign take 7 of the 127 bytes, a CFF font's CMap name 11.
        assert_eq!(fit_font_name(&long, false, false), &long[..120]);
        assert_eq!(fit_font_name(&long, true, false), &long[..127]);
        assert_eq!(fit_font_name(&long, false, true), &long[..109]);
        assert_eq!(fit_font_name(&long, true, true), &long[..116]);
        // At the limit a name stays whole; a byte more and its last character goes.
        assert_eq!(fit_font_name(&long[..120], false, false), &long[..120]);
        assert_eq!(fit_font_name(&long[..121], false, false), &long[..120]);
        assert_eq!(fit_font_name("", false, true), "");
        // Bytes written as escapes count three each: 40 number signs fill 120 bytes.
        let signs = "#".repeat(50);
        assert_eq!(fit_font_name(&signs, false, false), &signs[..40]);
        // é is two bytes, six as written; a name is never cut inside a character.
        let accents = format!("{}é", "A".repeat(114));
        assert_eq!(fit_font_name(&accents, false, false), accents);
        let accents = format!("{}é", "A".repeat(115));
        assert_eq!(fit_font_name(&accents, false, false), &accents[..115]);
        // Null characters are left out, wherever they are, before the name is cut.
        assert_eq!(fit_font_name("Foo\0Bar", false, false), "FooBar");
        assert_eq!(fit_font_name("\0Foo\0\0", true, true), "Foo");
        let nulls = format!("{}\0{}", "A".repeat(119), "B".repeat(10));
        assert_eq!(
            fit_font_name(&nulls, false, false),
            format!("{}B", "A".repeat(119))
        );
        for name in [long.as_str(), &signs, &accents, &"é".repeat(30), "a/b(c)"] {
            for (whole, cff) in [(false, false), (true, false), (false, true), (true, true)] {
                let fit = fit_font_name(name, whole, cff);
                let tag = if whole { 0 } else { 7 };
                let suffix = if cff { CMAP_SUFFIX.len() } else { 0 };
                // Without the slash.
                let written = pdf_name(&fit).len() - 1;
                assert!(
                    written + tag + suffix <= MAX_NAME_BYTES,
                    "{name:?} {whole} {cff}"
                );
                // Another character would not have fitted.
                if let Some(next) = name[fit.len()..].chars().next() {
                    let longer = &name[..fit.len() + next.len_utf8()];
                    assert!(pdf_name(longer).len() - 1 + tag + suffix > MAX_NAME_BYTES);
                }
            }
        }
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
