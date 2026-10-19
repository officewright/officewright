use std::fmt;
use std::sync::Arc;

use skrifa::charmap::Charmap;
use skrifa::raw::tables::os2::{Os2, SelectionFlags};
use skrifa::raw::{FileRef, FontRef, ReadError, TableProvider};

use crate::error::{Cause, Error, Result};
use crate::face::{EmbeddingPermissions, FaceProperties, FontNames, OutlineFormat};
use crate::subset_check::SubsetCheck;

/// The index of a glyph in a face.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlyphId(u16);

impl GlyphId {
    /// Glyph 0, `.notdef`, which fonts show for characters they do not have.
    pub const NOTDEF: Self = Self(0);

    /// The glyph with this index.
    pub const fn new(id: u16) -> Self {
        Self(id)
    }

    /// The index.
    pub const fn to_u16(self) -> u16 {
        self.0
    }
}

impl From<u16> for GlyphId {
    fn from(id: u16) -> Self {
        Self(id)
    }
}

impl From<GlyphId> for u16 {
    fn from(glyph: GlyphId) -> Self {
        glyph.0
    }
}

impl fmt::Display for GlyphId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A rectangle in font design units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BoundingBox {
    x_min: i16,
    y_min: i16,
    x_max: i16,
    y_max: i16,
}

impl BoundingBox {
    /// The left edge.
    pub fn x_min(self) -> i16 {
        self.x_min
    }

    /// The bottom edge.
    pub fn y_min(self) -> i16 {
        self.y_min
    }

    /// The right edge.
    pub fn x_max(self) -> i16 {
        self.x_max
    }

    /// The top edge.
    pub fn y_max(self) -> i16 {
        self.y_max
    }
}

/// Ascender, descender and line gap, in font design units. The descender is negative
/// below the baseline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LineMetrics {
    ascender: i16,
    descender: i16,
    line_gap: i16,
}

impl LineMetrics {
    /// Distance from the baseline to the top of the line.
    pub fn ascender(self) -> i16 {
        self.ascender
    }

    /// Distance from the baseline to the bottom of the line, negative below it.
    pub fn descender(self) -> i16 {
        self.descender
    }

    /// Extra space between lines.
    pub fn line_gap(self) -> i16 {
        self.line_gap
    }
}

/// The Windows ascent and descent from the `OS/2` table, in font design units. Both
/// are positive: the descent is measured downwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WinMetrics {
    ascent: u16,
    descent: u16,
}

impl WinMetrics {
    /// `usWinAscent`.
    pub fn ascent(self) -> u16 {
        self.ascent
    }

    /// `usWinDescent`.
    pub fn descent(self) -> u16 {
        self.descent
    }
}

/// The position and thickness of a line drawn through or under text, in font design
/// units, as the font stores them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecorationMetrics {
    position: i16,
    thickness: i16,
}

impl DecorationMetrics {
    /// The distance from the baseline to the top of the line, positive above it.
    pub fn position(self) -> i16 {
        self.position
    }

    /// The thickness of the line.
    pub fn thickness(self) -> i16 {
        self.thickness
    }
}

/// The vertical metrics a font declares, each set as the font has it.
///
/// Fonts carry three sets, and applications disagree on which one to use: Word on
/// Windows lays out lines with the `OS/2` Windows metrics, which GDI reports, other
/// applications use `hhea` or the typographic set. The layout engine picks, so this
/// type does not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VerticalMetrics {
    hhea: LineMetrics,
    typo: Option<LineMetrics>,
    win: Option<WinMetrics>,
    use_typo_metrics: bool,
}

impl VerticalMetrics {
    /// The metrics in the `hhea` table.
    pub fn hhea(self) -> LineMetrics {
        self.hhea
    }

    /// `sTypoAscender`, `sTypoDescender` and `sTypoLineGap` from the `OS/2` table.
    pub fn typo(self) -> Option<LineMetrics> {
        self.typo
    }

    /// `usWinAscent` and `usWinDescent` from the `OS/2` table.
    pub fn win(self) -> Option<WinMetrics> {
        self.win
    }

    /// Whether the font asks for the typographic metrics to be used
    /// (`USE_TYPO_METRICS` in `fsSelection`).
    pub fn use_typo_metrics(self) -> bool {
        self.use_typo_metrics
    }
}

/// One face of a TrueType or OpenType font file or font collection, with the data it
/// was read from.
///
/// All metrics are in font design units; [`units_per_em`](Self::units_per_em) of them
/// make one em. Cloning is cheap: the data and the parsed tables are shared. A `FontFace`
/// is `Send` and `Sync`.
///
/// ```
/// use officewright_font::FontFace;
///
/// let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/minimal.ttf"))?;
/// let font = FontFace::from_bytes(data, 0)?;
/// assert_eq!(font.names().family(), Some("Officewright Test"));
/// let glyph = font.glyph_id('A').expect("the font has A");
/// assert_eq!(font.advance_width(glyph), Some(600));
/// assert_eq!(font.units_per_em(), 1000);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct FontFace(Arc<Inner>);

struct Inner {
    data: Arc<[u8]>,
    index: u32,
    face: Face,
    /// What [`FontFace::check_subset_glyph`] found so far.
    subset_check: SubsetCheck,
}

/// What `FontFace` reads once, when it is created.
struct Face {
    units_per_em: u16,
    advances: Box<[u16]>,
    bounding_box: BoundingBox,
    metrics: VerticalMetrics,
    properties: FaceProperties,
    italic_angle: Option<f64>,
    fixed_pitch: bool,
    cap_height: Option<i16>,
    x_height: Option<i16>,
    underline: Option<DecorationMetrics>,
    strikeout: Option<DecorationMetrics>,
    has_char_map: bool,
    symbol: bool,
}

impl FontFace {
    /// Reads face `index` of a font file or collection. Use 0 for a single font.
    ///
    /// The face must have the `head`, `maxp`, `hhea` and `hmtx` tables; `cmap`,
    /// `OS/2`, `name` and `post` are read if present. Fonts embedded in PDF files, for
    /// example, often have no `cmap`. A table that is present must be readable, with
    /// all the records its header declares: a damaged one makes the face an error
    /// rather than being taken as missing, so that values such as the embedding
    /// permissions are never guessed. Character map subtables in formats or encodings
    /// that are not read are ignored.
    ///
    /// # Errors
    ///
    /// [`Error::NotAFont`] if the data is not a font, [`Error::FaceIndexOutOfRange`] if
    /// there is no such face, and [`Error::MissingTable`], [`Error::MalformedTable`],
    /// [`Error::MalformedFace`] or [`Error::InvalidUnitsPerEm`] if the face is damaged.
    pub fn from_bytes(data: impl Into<Arc<[u8]>>, index: u32) -> Result<Self> {
        let data = data.into();
        let face = Face::read(&font_ref(&data, index)?, index)?;
        Ok(Self(Arc::new(Inner {
            data,
            index,
            face,
            subset_check: SubsetCheck::default(),
        })))
    }

    /// The number of faces in a font file: the number of fonts in a collection, 1 for
    /// a single font.
    ///
    /// # Errors
    ///
    /// [`Error::NotAFont`] if the data is not a font or font collection.
    pub fn face_count(data: &[u8]) -> Result<u32> {
        match file_ref(data)? {
            FileRef::Font(_) => Ok(1),
            FileRef::Collection(collection) => Ok(collection.len()),
        }
    }

    /// The data the face was read from: the whole file, so for a collection all of it.
    pub fn data(&self) -> &[u8] {
        &self.0.data
    }

    /// What [`check_subset_glyph`](Self::check_subset_glyph) found so far, shared by the
    /// clones of the face.
    pub(crate) fn subset_check(&self) -> &SubsetCheck {
        &self.0.subset_check
    }

    /// The index of the face in its file.
    pub fn index(&self) -> u32 {
        self.0.index
    }

    /// The number of font design units per em, between 16 and 16384.
    pub fn units_per_em(&self) -> u16 {
        self.0.face.units_per_em
    }

    /// The number of glyphs, at least 1.
    pub fn glyph_count(&self) -> u16 {
        // The advances were read for every glyph `maxp` declares, which fits in a u16.
        u16::try_from(self.0.face.advances.len()).unwrap_or(u16::MAX)
    }

    /// The advance width of a glyph from the `hmtx` table, `None` if the face has no
    /// such glyph.
    pub fn advance_width(&self, glyph: GlyphId) -> Option<u16> {
        self.0.face.advances.get(usize::from(glyph.0)).copied()
    }

    /// The glyph for a character, `None` if the face does not map it.
    ///
    /// For many lookups, get a [`CharMap`] once with [`char_map`](Self::char_map).
    pub fn glyph_id(&self, ch: char) -> Option<GlyphId> {
        self.char_map().map(ch)
    }

    /// The character map of the face, for looking up glyphs.
    pub fn char_map(&self) -> CharMap<'_> {
        let charmap = font_ref(&self.0.data, self.0.index)
            .map(|font| Charmap::new(&font))
            .unwrap_or_default();
        CharMap {
            charmap,
            glyph_count: self.glyph_count(),
        }
    }

    /// Whether the face has a character map (`cmap` table) with a Unicode or symbol
    /// subtable.
    pub fn has_char_map(&self) -> bool {
        self.0.face.has_char_map
    }

    /// Whether the face is a symbol font: its character map is a Windows symbol
    /// subtable, which maps the characters U+F020 to U+F0FF. Like Windows,
    /// [`glyph_id`](Self::glyph_id) also finds those glyphs at U+0020 to U+00FF.
    pub fn is_symbol(&self) -> bool {
        self.0.face.symbol
    }

    /// The names of the face.
    pub fn names(&self) -> &FontNames {
        &self.0.face.properties.names
    }

    /// The weight class, 1 to 1000; 400 is regular, 700 bold.
    pub fn weight(&self) -> u16 {
        self.0.face.properties.weight
    }

    /// Whether the face is italic or oblique.
    pub fn is_italic(&self) -> bool {
        self.0.face.properties.italic
    }

    /// What the font license allows when the face is embedded in a document.
    pub fn embedding(&self) -> EmbeddingPermissions {
        self.0.face.properties.embedding
    }

    /// How the glyph outlines are stored, `None` for a face without outlines, such as a
    /// bitmap-only font.
    pub fn outline_format(&self) -> Option<OutlineFormat> {
        self.0.face.properties.outlines
    }

    /// Whether the face is a variable font (it has an `fvar` table). Only its default
    /// instance is used.
    pub fn is_variable(&self) -> bool {
        self.0.face.properties.variable
    }

    /// The vertical metrics.
    pub fn metrics(&self) -> VerticalMetrics {
        self.0.face.metrics
    }

    /// The box that encloses all glyphs, from the `head` table.
    pub fn bounding_box(&self) -> BoundingBox {
        self.0.face.bounding_box
    }

    /// The italic angle in degrees counter-clockwise from vertical, from the `post`
    /// table; negative for a font that leans right.
    pub fn italic_angle(&self) -> Option<f64> {
        self.0.face.italic_angle
    }

    /// Whether all glyphs have the same advance width (`isFixedPitch` in `post`).
    pub fn is_fixed_pitch(&self) -> bool {
        self.0.face.fixed_pitch
    }

    /// The height of capital letters, from version 2 of the `OS/2` table on.
    pub fn cap_height(&self) -> Option<i16> {
        self.0.face.cap_height
    }

    /// The height of lowercase letters such as x, from version 2 of the `OS/2` table on.
    pub fn x_height(&self) -> Option<i16> {
        self.0.face.x_height
    }

    /// Where the font puts an underline: `underlinePosition` and `underlineThickness`
    /// from the `post` table, `None` without one. The OpenType specification defines the
    /// position as the top of the underline, not its centre as PostScript's `FontInfo`
    /// does. The values are as stored: a thickness of 0 or a position above the baseline
    /// is returned as it is.
    pub fn underline_metrics(&self) -> Option<DecorationMetrics> {
        self.0.face.underline
    }

    /// Where the font puts a strikeout: `yStrikeoutPosition` and `yStrikeoutSize` from
    /// the `OS/2` table, `None` without one. The position is the top of the stroke, as
    /// for [`underline_metrics`](Self::underline_metrics). The values are as stored.
    pub fn strikeout_metrics(&self) -> Option<DecorationMetrics> {
        self.0.face.strikeout
    }
}

impl fmt::Debug for FontFace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontFace")
            .field("family", &self.names().family())
            .field("subfamily", &self.names().subfamily())
            .field("index", &self.0.index)
            .field("glyph_count", &self.glyph_count())
            .finish_non_exhaustive()
    }
}

/// The character map of a [`FontFace`].
#[derive(Clone)]
pub struct CharMap<'a> {
    charmap: Charmap<'a>,
    glyph_count: u16,
}

impl CharMap<'_> {
    /// The glyph for a character, `None` if the face does not map it.
    pub fn map(&self, ch: char) -> Option<GlyphId> {
        let glyph = u16::try_from(self.charmap.map(ch)?.to_u32()).ok()?;
        // Glyph 0 means "not mapped"; a glyph past the end of the font is a damaged
        // cmap entry and is treated the same.
        (glyph != 0 && glyph < self.glyph_count).then_some(GlyphId(glyph))
    }
}

impl fmt::Debug for CharMap<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CharMap")
            .field("is_symbol", &self.charmap.is_symbol())
            .finish_non_exhaustive()
    }
}

impl Face {
    fn read(font: &FontRef<'_>, face: u32) -> Result<Self> {
        let head = required(font.head(), face, "head")?;
        let units_per_em = head.units_per_em();
        if !(16..=16384).contains(&units_per_em) {
            return Err(Error::InvalidUnitsPerEm { face, units_per_em });
        }
        let maxp = required(font.maxp(), face, "maxp")?;
        let glyph_count = maxp.num_glyphs();
        if glyph_count == 0 {
            return Err(Error::MalformedTable {
                face,
                table: "maxp",
                source: Cause::new("the face has no glyphs"),
            });
        }
        let hhea = required(font.hhea(), face, "hhea")?;
        let hmtx = required(font.hmtx(), face, "hmtx")?;
        if hhea.number_of_h_metrics() == 0 {
            return Err(Error::MalformedTable {
                face,
                table: "hmtx",
                source: Cause::new("numberOfHMetrics is 0"),
            });
        }
        all_records(
            hmtx.h_metrics().len(),
            hhea.number_of_h_metrics(),
            face,
            "hmtx",
        )?;
        // Glyphs past numberOfHMetrics repeat the last advance, so every glyph has one.
        let advances = (0..glyph_count)
            .map(|glyph| {
                hmtx.advance(skrifa::GlyphId::new(u32::from(glyph)))
                    .unwrap_or_default()
            })
            .collect();
        let os2 = optional(font.os2(), face, "OS/2")?;
        let name = optional(font.name(), face, "name")?;
        if let Some(name) = &name {
            all_records(name.name_record().len(), name.count(), face, "name")?;
        }
        let post = optional(font.post(), face, "post")?;
        if let Some(cmap) = optional(font.cmap(), face, "cmap")? {
            all_records(
                cmap.encoding_records().len(),
                cmap.num_tables(),
                face,
                "cmap",
            )?;
        }
        let charmap = Charmap::new(font);
        let properties = FaceProperties::read(&head, os2.as_ref(), name.as_ref(), |tag| {
            font.table_data(skrifa::Tag::new(tag)).is_some()
        });
        Ok(Self {
            units_per_em,
            advances,
            bounding_box: BoundingBox {
                x_min: head.x_min(),
                y_min: head.y_min(),
                x_max: head.x_max(),
                y_max: head.y_max(),
            },
            metrics: VerticalMetrics {
                hhea: LineMetrics {
                    ascender: hhea.ascender().to_i16(),
                    descender: hhea.descender().to_i16(),
                    line_gap: hhea.line_gap().to_i16(),
                },
                typo: os2.as_ref().map(|os2| LineMetrics {
                    ascender: os2.s_typo_ascender(),
                    descender: os2.s_typo_descender(),
                    line_gap: os2.s_typo_line_gap(),
                }),
                win: os2.as_ref().map(|os2| WinMetrics {
                    ascent: os2.us_win_ascent(),
                    descent: os2.us_win_descent(),
                }),
                use_typo_metrics: os2.as_ref().is_some_and(|os2| {
                    os2.fs_selection()
                        .contains(SelectionFlags::USE_TYPO_METRICS)
                }),
            },
            properties,
            italic_angle: post.as_ref().map(|post| post.italic_angle().to_f64()),
            fixed_pitch: post.as_ref().is_some_and(|post| post.is_fixed_pitch() != 0),
            cap_height: os2.as_ref().and_then(Os2::s_cap_height),
            x_height: os2.as_ref().and_then(Os2::sx_height),
            underline: post.as_ref().map(|post| DecorationMetrics {
                position: post.underline_position().to_i16(),
                thickness: post.underline_thickness().to_i16(),
            }),
            strikeout: os2.as_ref().map(|os2| DecorationMetrics {
                position: os2.y_strikeout_position(),
                thickness: os2.y_strikeout_size(),
            }),
            has_char_map: charmap.has_map(),
            symbol: charmap.is_symbol(),
        })
    }
}

fn file_ref(data: &[u8]) -> Result<FileRef<'_>> {
    match FileRef::new(data) {
        // The parser reads a collection whose font offsets do not fit the data as empty.
        Ok(FileRef::Collection(collection)) if collection.is_empty() => Err(Error::NotAFont {
            source: Cause::new("the collection header lists no fonts that fit in the data"),
        }),
        Ok(file) => Ok(file),
        Err(err) => Err(Error::NotAFont {
            source: Cause::new(err.to_string()),
        }),
    }
}

/// The face `index` of a font file or collection.
pub(crate) fn font_ref(data: &[u8], index: u32) -> Result<FontRef<'_>> {
    match file_ref(data)? {
        FileRef::Font(font) if index == 0 => Ok(font),
        FileRef::Font(_) => Err(Error::FaceIndexOutOfRange { index, count: 1 }),
        FileRef::Collection(collection) => {
            let count = collection.len();
            if index >= count {
                return Err(Error::FaceIndexOutOfRange { index, count });
            }
            collection.get(index).map_err(|err| Error::MalformedFace {
                face: index,
                source: Cause::new(err.to_string()),
            })
        }
    }
}

/// Checks that a table has all the records its header declares. The parser reads a
/// record array that runs past the end of its table as empty, which would make a
/// damaged table look like one without records.
pub(crate) fn all_records(
    read: usize,
    declared: u16,
    face: u32,
    table: &'static str,
) -> Result<()> {
    if read == usize::from(declared) {
        Ok(())
    } else {
        Err(Error::MalformedTable {
            face,
            table,
            source: Cause::new(format!(
                "the table declares {declared} records but holds {read}"
            )),
        })
    }
}

/// A table the face cannot do without.
pub(crate) fn required<T>(
    table: std::result::Result<T, ReadError>,
    face: u32,
    name: &'static str,
) -> Result<T> {
    optional(table, face, name)?.ok_or(Error::MissingTable { face, table: name })
}

/// A table the face may lack; if it has it, the table must be readable.
pub(crate) fn optional<T>(
    table: std::result::Result<T, ReadError>,
    face: u32,
    name: &'static str,
) -> Result<Option<T>> {
    match table {
        Ok(table) => Ok(Some(table)),
        Err(ReadError::TableIsMissing(_)) => Ok(None),
        Err(err) => Err(Error::MalformedTable {
            face,
            table: name,
            source: Cause::new(err.to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::EmbeddingLicense;
    use crate::testing::{self, ADVANCES, CHARS, TestFont};

    fn load(font: &TestFont) -> Result<FontFace> {
        FontFace::from_bytes(font.build(), 0)
    }

    #[test]
    fn reads_a_truetype_face() {
        let font = load(&TestFont::new("Test Sans")).unwrap();
        assert_eq!(font.units_per_em(), 1000);
        assert_eq!(font.glyph_count(), 5);
        assert_eq!(font.outline_format(), Some(OutlineFormat::TrueType));
        assert!(!font.is_variable());
        assert!(font.has_char_map());
        assert!(!font.is_symbol());
        assert_eq!(font.names().family(), Some("Test Sans"));
        assert_eq!(font.names().subfamily(), Some("Regular"));
        assert_eq!(font.names().postscript_name(), Some("TestSans-Regular"));
        assert_eq!(font.weight(), 400);
        assert!(!font.is_italic());
        assert_eq!(font.embedding().license(), EmbeddingLicense::Installable);
        assert_eq!(font.italic_angle(), Some(0.0));
        assert!(!font.is_fixed_pitch());
        assert_eq!(font.cap_height(), Some(700));
        assert_eq!(font.x_height(), Some(500));
        let bbox = font.bounding_box();
        assert_eq!(
            (bbox.x_min(), bbox.y_min(), bbox.x_max(), bbox.y_max()),
            (0, 0, 600, 850)
        );
    }

    #[test]
    fn maps_characters_to_glyphs_with_their_advances() {
        let font = load(&TestFont::new("Test Sans")).unwrap();
        let map = font.char_map();
        for (ch, glyph) in CHARS {
            assert_eq!(map.map(ch), Some(GlyphId::new(glyph)), "{ch:?}");
        }
        assert_eq!(map.map('B'), None);
        for (glyph, advance) in ADVANCES.iter().enumerate() {
            assert_eq!(
                font.advance_width(GlyphId::new(glyph as u16)),
                Some(*advance)
            );
        }
        assert_eq!(font.advance_width(GlyphId::new(5)), None);
    }

    #[test]
    fn keeps_all_three_sets_of_vertical_metrics() {
        let metrics = load(&TestFont::new("Test Sans")).unwrap().metrics();
        assert_eq!(metrics.hhea().ascender(), 800);
        assert_eq!(metrics.hhea().descender(), -200);
        assert_eq!(metrics.hhea().line_gap(), 90);
        let typo = metrics.typo().unwrap();
        assert_eq!(
            (typo.ascender(), typo.descender(), typo.line_gap()),
            (800, -200, 90)
        );
        let win = metrics.win().unwrap();
        assert_eq!((win.ascent(), win.descent()), (900, 250));
        assert!(!metrics.use_typo_metrics());
    }

    #[test]
    fn reads_underline_and_strikeout_as_stored() {
        // The test font's post and OS/2 tables: underline at -100, 50 thick; strikeout at
        // 250, 50 thick.
        let font = load(&TestFont::new("Test Sans")).unwrap();
        let underline = font.underline_metrics().unwrap();
        assert_eq!((underline.position(), underline.thickness()), (-100, 50));
        let strikeout = font.strikeout_metrics().unwrap();
        assert_eq!((strikeout.position(), strikeout.thickness()), (250, 50));

        let without_post = load(&TestFont::new("Test Sans").without_table(b"post")).unwrap();
        assert_eq!(without_post.underline_metrics(), None);
        assert!(without_post.strikeout_metrics().is_some());
        let without_os2 = load(&TestFont::new("Test Sans").without_table(b"OS/2")).unwrap();
        assert_eq!(without_os2.strikeout_metrics(), None);
        assert!(without_os2.underline_metrics().is_some());

        // A version 3 post table with an underline above the baseline and no thickness:
        // returned as stored, not corrected.
        let mut post = Vec::new();
        post.extend_from_slice(&0x0003_0000_u32.to_be_bytes());
        post.extend_from_slice(&0_u32.to_be_bytes()); // italicAngle
        post.extend_from_slice(&30_i16.to_be_bytes()); // underlinePosition
        post.extend_from_slice(&0_i16.to_be_bytes()); // underlineThickness
        post.extend_from_slice(&[0; 20]); // isFixedPitch and the memory fields
        let odd = load(&TestFont::new("Test Sans").with_table(b"post", post.clone())).unwrap();
        let underline = odd.underline_metrics().unwrap();
        assert_eq!((underline.position(), underline.thickness()), (30, 0));

        // A post table cut short is a damaged face, not one without an underline.
        post.truncate(10);
        let err = load(&TestFont::new("Test Sans").with_table(b"post", post)).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "post", .. }),
            "{err:?}"
        );
    }

    #[test]
    fn symbol_fonts_map_the_low_range_like_windows() {
        let font = load(&TestFont::new("Test Symbol").symbol()).unwrap();
        assert!(font.is_symbol());
        assert_eq!(font.glyph_id('A'), Some(GlyphId::new(2)));
        assert_eq!(font.glyph_id('\u{F041}'), Some(GlyphId::new(2)));
    }

    #[test]
    fn reads_style_from_os2_and_head() {
        let font = load(&TestFont::new("Test Sans").style("Bold Italic", 700, true)).unwrap();
        assert_eq!(font.weight(), 700);
        assert!(font.is_italic());
        assert_eq!(font.italic_angle(), Some(-12.0));
        // Without OS/2, head.macStyle decides.
        let font = load(
            &TestFont::new("Test Sans")
                .style("Bold", 700, false)
                .without_table(b"OS/2"),
        )
        .unwrap();
        assert_eq!(font.weight(), 700);
        assert_eq!(font.metrics().win(), None);
        assert_eq!(font.cap_height(), None);
        assert_eq!(font.embedding(), EmbeddingPermissions::default());
    }

    #[test]
    fn reads_embedding_permissions() {
        let font = load(&TestFont::new("Test Sans").fs_type(0x0102, 4)).unwrap();
        assert!(!font.embedding().allows_embedding());
        assert!(!font.embedding().allows_subsetting());
    }

    #[test]
    fn reads_every_face_of_a_collection() {
        let regular = TestFont::new("Test Sans").build();
        let bold = TestFont::new("Test Sans").style("Bold", 700, false).build();
        let ttc = testing::collection(&[regular, bold]);
        assert_eq!(FontFace::face_count(&ttc).unwrap(), 2);
        let font = FontFace::from_bytes(ttc.clone(), 1).unwrap();
        assert_eq!(font.index(), 1);
        assert_eq!(font.names().subfamily(), Some("Bold"));
        assert_eq!(font.glyph_id('A'), Some(GlyphId::new(2)));
        assert!(matches!(
            FontFace::from_bytes(ttc, 2),
            Err(Error::FaceIndexOutOfRange { index: 2, count: 2 })
        ));
    }

    #[test]
    fn a_single_font_has_only_face_0() {
        let data = TestFont::new("Test Sans").build();
        assert_eq!(FontFace::face_count(&data).unwrap(), 1);
        assert!(matches!(
            FontFace::from_bytes(data, 1),
            Err(Error::FaceIndexOutOfRange { index: 1, count: 1 })
        ));
    }

    #[test]
    fn rejects_data_that_is_not_a_font() {
        for data in [&b""[..], b"hello, world", b"%PDF-1.4\n"] {
            assert!(matches!(
                FontFace::from_bytes(data, 0),
                Err(Error::NotAFont { .. })
            ));
            assert!(matches!(
                FontFace::face_count(data),
                Err(Error::NotAFont { .. })
            ));
        }
    }

    #[test]
    fn a_collection_whose_fonts_do_not_fit_is_not_a_font() {
        // Two fonts declared but the file ends after one offset; and no fonts at all.
        let short = b"ttcf\0\x01\0\0\0\0\0\x02\0\0\0\x10".to_vec();
        let empty = b"ttcf\0\x01\0\0\0\0\0\0".to_vec();
        for data in [short, empty] {
            assert!(matches!(
                FontFace::face_count(&data),
                Err(Error::NotAFont { .. })
            ));
            assert!(matches!(
                FontFace::from_bytes(data, 0),
                Err(Error::NotAFont { .. })
            ));
        }
    }

    #[test]
    fn a_face_needs_at_least_one_glyph() {
        // maxp version 0.5 with numGlyphs 0.
        let maxp = vec![0, 0, 0x50, 0, 0, 0];
        let err = load(&TestFont::new("Test Sans").with_table(b"maxp", maxp)).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "maxp", .. }),
            "{err}"
        );
    }

    #[test]
    fn reads_style_when_os2_has_no_usable_weight() {
        let weight = |font: TestFont| load(&font).unwrap().weight();
        // Weight class 0 or above 1000: the bold flags decide.
        assert_eq!(
            weight(TestFont::new("T").style("Bold", 700, false).weight_class(0)),
            700
        );
        assert_eq!(weight(TestFont::new("T").weight_class(1001)), 400);
        // macStyle alone marks the face bold.
        assert_eq!(weight(TestFont::new("T").weight_class(0).mac_style(1)), 700);
        assert_eq!(weight(TestFont::new("T").weight_class(0)), 400);
        // A valid weight class wins over the flags.
        assert_eq!(
            weight(TestFont::new("T").weight_class(500).mac_style(1)),
            500
        );
    }

    #[test]
    fn truetype_outlines_need_glyf_and_loca() {
        let font = load(&TestFont::new("Test Sans").without_table(b"loca")).unwrap();
        assert_eq!(font.outline_format(), None);
    }

    #[test]
    fn reports_missing_tables() {
        for (tag, name) in [
            (b"head", "head"),
            (b"maxp", "maxp"),
            (b"hhea", "hhea"),
            (b"hmtx", "hmtx"),
        ] {
            let err = load(&TestFont::new("Test Sans").without_table(tag)).unwrap_err();
            assert!(
                matches!(err, Error::MissingTable { face: 0, table } if table == name),
                "{err}"
            );
        }
    }

    #[test]
    fn optional_tables_may_be_missing() {
        let font = load(
            &TestFont::new("Test Sans")
                .without_table(b"cmap")
                .without_table(b"name")
                .without_table(b"post"),
        )
        .unwrap();
        assert!(!font.has_char_map());
        assert_eq!(font.glyph_id('A'), None);
        assert_eq!(font.names().family(), None);
        assert_eq!(font.italic_angle(), None);
    }

    #[test]
    fn reports_damaged_tables() {
        let err = load(&TestFont::new("Test Sans").with_table(b"hmtx", vec![0; 6])).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "hmtx", .. }),
            "{err}"
        );
        let err = load(&TestFont::new("Test Sans").with_table(b"OS/2", vec![0; 10])).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "OS/2", .. }),
            "{err}"
        );
        let err = load(&TestFont::new("Test Sans").with_table(b"cmap", vec![0; 2])).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "cmap", .. }),
            "{err}"
        );
    }

    #[test]
    fn tables_must_hold_the_records_they_declare() {
        // A cmap declaring 2 encoding records and holding none, one declaring 65535.
        for cmap in [vec![0, 0, 0, 2], vec![0, 0, 0xFF, 0xFF]] {
            let err = load(&TestFont::new("Test Sans").with_table(b"cmap", cmap)).unwrap_err();
            assert!(
                matches!(err, Error::MalformedTable { table: "cmap", .. }),
                "{err}"
            );
        }
        // A name table declaring 3 records and holding none.
        let err = load(&TestFont::new("Test Sans").with_table(b"name", vec![0, 0, 0, 3, 0, 6]))
            .unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "name", .. }),
            "{err}"
        );
        // An hhea that declares no metrics at all.
        let mut hhea = testing::hhea();
        let len = hhea.len();
        hhea[len - 2..].fill(0);
        let err = load(&TestFont::new("Test Sans").with_table(b"hhea", hhea)).unwrap_err();
        assert!(
            std::error::Error::source(&err)
                .is_some_and(|cause| cause.to_string() == "numberOfHMetrics is 0"),
            "{err}"
        );
        // An hmtx with 2 of the 5 metrics hhea declares.
        let err = load(&TestFont::new("Test Sans").with_table(b"hmtx", vec![0; 8])).unwrap_err();
        assert!(
            matches!(err, Error::MalformedTable { table: "hmtx", .. }),
            "{err}"
        );
        assert!(
            std::error::Error::source(&err)
                .is_some_and(|cause| cause.to_string().contains("declares 5 records but holds")),
            "{err}"
        );
    }

    #[test]
    fn rejects_units_per_em_outside_the_opentype_range() {
        for units_per_em in [0, 15, 16385] {
            let err = load(&TestFont::new("Test Sans").units_per_em(units_per_em)).unwrap_err();
            assert!(matches!(err, Error::InvalidUnitsPerEm { .. }), "{err}");
        }
        load(&TestFont::new("Test Sans").units_per_em(16)).unwrap();
    }

    #[test]
    fn keeps_localized_and_typographic_names() {
        let font = load(
            &TestFont::new("Test Sans Light")
                .typographic("Test Sans", "Light")
                .localized_family(0x0804, "测试"),
        )
        .unwrap();
        let names = font.names();
        assert_eq!(names.family(), Some("Test Sans Light"));
        assert_eq!(names.typographic_family(), Some("Test Sans"));
        assert_eq!(names.typographic_subfamily(), Some("Light"));
        assert_eq!(
            names.families().collect::<Vec<_>>(),
            ["Test Sans Light", "测试"]
        );
    }

    #[test]
    fn damaged_fonts_give_errors_not_panics() {
        let data = TestFont::new("Test Sans").build();
        for len in 0..data.len() {
            let _ = FontFace::from_bytes(&data[..len], 0);
        }
        for at in 0..data.len() {
            for value in [0x00, 0x7F, 0xFF] {
                let mut damaged = data.clone();
                damaged[at] = value;
                if let Ok(font) = FontFace::from_bytes(damaged, 0) {
                    for ch in ['A', '\u{C1}', '\u{F041}'] {
                        if let Some(glyph) = font.glyph_id(ch) {
                            assert!(glyph.to_u16() < font.glyph_count());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn minimal_font_file_is_up_to_date() {
        // The doc examples read this file, so they also run from the published crate.
        // Regenerate it with OFFICEWRIGHT_UPDATE_TEST_FONT=1 after changing the builder.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/minimal.ttf");
        let expected = TestFont::new("Officewright Test").build();
        if std::env::var_os("OFFICEWRIGHT_UPDATE_TEST_FONT").is_some() {
            std::fs::write(path, &expected).unwrap();
        }
        assert!(
            std::fs::read(path).unwrap() == expected,
            "{path} is stale; run the tests with OFFICEWRIGHT_UPDATE_TEST_FONT=1"
        );
    }
}
