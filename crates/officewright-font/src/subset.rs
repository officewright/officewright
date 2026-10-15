use std::any::Any;
use std::fmt;
use std::ops::Range;
use std::panic::{self, UnwindSafe};

use skrifa::raw::types::Tag;
use skrifa::raw::{FontRef, TableProvider};
use subsetter::GlyphRemapper;

use crate::error::{Cause, Error, Result};
use crate::face::OutlineFormat;
use crate::font::{FontFace, GlyphId};

/// A font reduced to the glyphs a document uses, for embedding in a PDF.
///
/// The glyphs are renumbered from 0 in the order of their original IDs, with `.notdef`
/// first; glyphs that composite glyphs are built from are kept as well. The font is
/// meant for a PDF CIDFont whose CIDs are the new glyph IDs (`CIDToGIDMap /Identity`):
/// it has no `cmap` or `OS/2` table, and CFF fonts are converted to CID-keyed
/// CFF. It is not a general-purpose font file.
///
/// ```
/// use officewright_font::FontFace;
///
/// let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/minimal.ttf"))?;
/// let font = FontFace::from_bytes(data, 0)?;
/// let a = font.glyph_id('A').expect("the font has A");
/// let subset = font.subset([a])?;
/// assert_eq!(subset.glyph_id(a).map(u16::from), Some(1));
/// assert!(subset.data().len() < font.data().len());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct Subset {
    data: Vec<u8>,
    remapper: GlyphRemapper,
    glyph_count: u16,
    outline_format: OutlineFormat,
    /// Where the `CFF ` table is in `data`, for CFF outlines.
    cff: Option<Range<usize>>,
}

impl Subset {
    /// The font data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The font data, without copying it.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// The ID in the subset of a glyph that was asked for, by its ID in the original
    /// font; `None` for a glyph that was not asked for.
    pub fn glyph_id(&self, original: GlyphId) -> Option<GlyphId> {
        self.remapper.get(original.to_u16()).map(GlyphId::new)
    }

    /// The number of glyphs in the subset, including `.notdef` and the components of
    /// composite glyphs.
    pub fn glyph_count(&self) -> u16 {
        self.glyph_count
    }

    /// How the glyph outlines are stored, the same as in the original font.
    pub fn outline_format(&self) -> OutlineFormat {
        self.outline_format
    }

    /// For CFF outlines, the `CFF ` table: the CID-keyed font program that a PDF embeds
    /// as `FontFile3` with subtype `CIDFontType0C` (ISO 32000-1, 9.9). `None` for
    /// TrueType outlines, which a PDF embeds as the whole [`data`](Self::data).
    pub fn cff_table(&self) -> Option<&[u8]> {
        self.data.get(self.cff.clone()?)
    }
}

impl fmt::Debug for Subset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subset")
            .field("len", &self.data.len())
            .field("glyph_count", &self.glyph_count)
            .field("outline_format", &self.outline_format)
            .finish_non_exhaustive()
    }
}

/// The work that several subsets may take together, such as the subsets of the fonts one
/// PDF file embeds.
///
/// Reducing a face with CFF outlines decompiles the charstrings of the glyphs it keeps,
/// reading every subroutine again for each call, and the work and memory that takes grow
/// with what it reads: about 30 bytes for each operand, operator and hint mask byte while
/// the subset is made, and about 5 bytes for each in the subset made. One subset may read
/// at most 16,777,216 of them, a limit of the library; a budget bounds what several
/// subsets that are kept together read in all. [`FontFace::subset_within`] takes what a
/// subset reads off the budget, and refuses a subset that would read more than is left.
/// TrueType outlines take nothing from it.
///
/// ```
/// use officewright_font::{FontFace, SubsetBudget};
///
/// let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/minimal.ttf"))?;
/// let font = FontFace::from_bytes(data, 0)?;
/// let a = font.glyph_id('A').expect("the font has A");
/// let mut budget = SubsetBudget::new(1_000_000);
/// let subset = font.subset_within([a], &mut budget)?;
/// assert_eq!(subset.glyph_id(a).map(u16::from), Some(1));
/// // The font has TrueType outlines.
/// assert_eq!(budget.left(), 1_000_000);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubsetBudget {
    left: u64,
}

impl SubsetBudget {
    /// A budget of `units` operands, operators and hint mask bytes.
    pub fn new(units: u64) -> Self {
        Self { left: units }
    }

    /// The units left.
    pub fn left(&self) -> u64 {
        self.left
    }
}

impl FontFace {
    /// Reduces the face to `glyphs`, plus `.notdef` and the components of composite
    /// glyphs, for embedding in a PDF.
    ///
    /// This does not check [`embedding`](Self::embedding) permissions; the caller
    /// decides what to do with a font whose license forbids embedding or subsetting.
    /// A variable font is reduced to its default instance. Several subsets kept together
    /// are bounded together by [`subset_within`](Self::subset_within).
    ///
    /// # Errors
    ///
    /// [`Error::GlyphOutOfRange`] if a glyph is not in the face,
    /// [`Error::UnsupportedOutlines`] for a face without outlines or with CFF2
    /// outlines, [`Error::MalformedTable`] for a `CFF ` table with a real number the
    /// subsetter cannot write back or a glyph with more stem hints than it can count, or
    /// a `name` table whose offsets it cannot compute,
    /// [`Error::TableOverLimit`] for a `name` table whose records to keep point at more
    /// than 1 MiB of strings, `CFF ` Font DICTs that point at more than 1 MiB of Private
    /// DICTs, an FDSelect of more than 2,048 ranges, or glyphs whose `CFF ` charstrings
    /// take more than 16,777,216 operands, operators and hint mask bytes to decompile,
    /// [`Error::SubsetUnsupported`] if the face uses
    /// something the subsetter does not implement yet, and [`Error::Subset`] or
    /// [`Error::SubsetterPanicked`] if the face is damaged in a way that only subsetting
    /// finds.
    pub fn subset(&self, glyphs: impl IntoIterator<Item = GlyphId>) -> Result<Subset> {
        self.subset_within(glyphs, &mut SubsetBudget::new(u64::MAX))
    }

    /// [`subset`](Self::subset) within `budget`: decompiling the CFF glyphs to keep may
    /// read no more than is left of it, nor than the limit of one subset, and what it
    /// reads is taken off the budget once the subset is made. A subset that fails takes
    /// nothing.
    ///
    /// # Errors
    ///
    /// Those of [`subset`](Self::subset), and [`Error::TableOverLimit`] if decompiling
    /// the glyphs to keep reads more than is left of `budget`.
    pub fn subset_within(
        &self,
        glyphs: impl IntoIterator<Item = GlyphId>,
        budget: &mut SubsetBudget,
    ) -> Result<Subset> {
        let face = self.index();
        let outline_format = match self.outline_format() {
            Some(format @ (OutlineFormat::TrueType | OutlineFormat::Cff)) => format,
            format => return Err(Error::UnsupportedOutlines { face, format }),
        };
        let glyph_count = self.glyph_count();
        let mut ids = Vec::new();
        for glyph in glyphs {
            if glyph.to_u16() >= glyph_count {
                return Err(Error::GlyphOutOfRange {
                    face,
                    glyph,
                    glyph_count,
                });
            }
            ids.push(glyph.to_u16());
        }
        // What subsetter 0.2.6 would panic on is refused first, in the tables it takes.
        // It reads `CFF ` only when it finds no `glyf` table, which is decided by its
        // lookup, not by the outline format the font parser reports.
        let font = crate::font::font_ref(self.data(), face)?;
        crate::name_offsets::check(&font, self.data(), face)?;
        let read = if subsetter_table(&font, self.data(), b"glyf").is_none() {
            let limit = crate::cff::Limit::within(budget.left);
            crate::cff::check(&font, self.data(), face, &ids, limit)?
        } else {
            0
        };
        let remapper = GlyphRemapper::new_from_glyphs_sorted(&ids);
        let data = self.data();
        let data = catch_subsetter_panic(face, || subsetter::subset(data, face, &remapper))?
            .map_err(|err| subsetter_error(face, &err))?;
        // The output is read back: a subset the font parser cannot read would only
        // fail later, in a PDF viewer.
        let unreadable = |reason: String| Error::Subset {
            face,
            source: Cause::new(format!("the subset font is unreadable: {reason}")),
        };
        let font = FontRef::new(&data).map_err(|err| unreadable(err.to_string()))?;
        let glyph_count = font
            .maxp()
            .map(|maxp| maxp.num_glyphs())
            .map_err(|err| unreadable(err.to_string()))?;
        let cff = match outline_format {
            OutlineFormat::Cff => Some(
                table_range(&font, b"CFF ", data.len())
                    .ok_or_else(|| unreadable("it has no CFF table".to_owned()))?,
            ),
            _ => None,
        };
        budget.left = budget.left.saturating_sub(u64::from(read));
        Ok(Subset {
            data,
            remapper,
            glyph_count,
            outline_format,
            cff,
        })
    }
}

/// Where the table `tag` is in the font data of `len` bytes, if it is there whole.
fn table_range(font: &FontRef<'_>, tag: &[u8; 4], len: usize) -> Option<Range<usize>> {
    let record = font
        .table_directory()
        .table_records()
        .iter()
        .find(|record| record.tag() == Tag::new(tag))?;
    let start = usize::try_from(record.offset()).ok()?;
    let end = start.checked_add(usize::try_from(record.length()).ok()?)?;
    (end <= len).then_some(start..end)
}

/// The table `tag` of `font`, in its file `data`, that subsetter 0.2.6 takes: the record
/// `binary_search_by` finds among the records of the table directory in their order,
/// sorted or not, comparing tags as their four bytes. Built with the same standard
/// library, the same search over the same tags takes the same record, also where a tag
/// is repeated; the parser's own lookup may take another. A subsetter release that looks
/// tables up otherwise (its main branch sorts the records first) needs this changed with
/// it; the tests of repeated `name` and `CFF ` records fail until it is.
pub(crate) fn subsetter_table<'a>(
    font: &FontRef<'_>,
    data: &'a [u8],
    tag: &[u8; 4],
) -> Option<&'a [u8]> {
    let records = font.table_directory().table_records();
    let found = records
        .binary_search_by(|record| record.tag().to_be_bytes().cmp(tag))
        .ok()?;
    let record = records.get(found)?;
    let start = usize::try_from(record.offset()).ok()?;
    let end = start.checked_add(usize::try_from(record.length()).ok()?)?;
    data.get(start..end)
}

/// Runs the subsetter and turns a panic into [`Error::SubsetterPanicked`].
///
/// subsetter 0.2.6 still has `unwrap()`, `unreachable!()` and unchecked arithmetic on
/// paths that malformed fonts can reach. A damaged font must give `Err`, never a panic,
/// also inside a dependency, so the panic is caught here until upstream returns errors
/// on those paths; panics that fuzzing finds are reported there. The four found so far
/// are not left to this guard: the checks in `name_offsets.rs` and `cff.rs` refuse the
/// faces before the subsetter runs, since the panic hook would still print them. They
/// are an overflow in its `name` table code
/// (<https://github.com/typst/subsetter/issues/30>), `unreachable!()` for a CFF real
/// number too large for an `f32` (<https://github.com/typst/subsetter/issues/31>), an
/// overflow of a glyph's stem-hint count
/// (<https://github.com/typst/subsetter/issues/33>), and a CID-keyed font with 256 Font
/// DICTs or more, on which a `debug_assert!` fails and, when the kept glyphs use 256, a
/// `u8` counter overflows in any build
/// (<https://github.com/typst/subsetter/issues/34>).
/// Catching relies on unwinding, like the C ABI: built with `-C panic=abort` the process
/// aborts instead. The default panic hook still prints the message to stderr, since a
/// library must not replace the process-wide hook.
fn catch_subsetter_panic<T>(face: u32, run: impl FnOnce() -> T + UnwindSafe) -> Result<T> {
    panic::catch_unwind(run).map_err(|payload| Error::SubsetterPanicked {
        face,
        message: panic_message(payload.as_ref()),
    })
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic without a message".to_owned()
    }
}

/// The error for what the subsetter reported about face `face`: a feature it does not
/// implement yet makes the face unsupported, anything else damaged.
fn subsetter_error(face: u32, err: &subsetter::Error) -> Error {
    let source = Cause::new(err.to_string());
    match err {
        subsetter::Error::Unimplemented => Error::SubsetUnsupported { face, source },
        _ => Error::Subset { face, source },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestFont;

    fn font() -> FontFace {
        FontFace::from_bytes(TestFont::new("Test Sans").build(), 0).unwrap()
    }

    #[test]
    fn what_the_subsetter_lacks_is_unsupported_and_damage_is_malformed() {
        let unsupported = subsetter_error(2, &subsetter::Error::Unimplemented);
        assert!(matches!(
            unsupported,
            Error::SubsetUnsupported { face: 2, .. }
        ));
        match officewright_core::Error::from(unsupported) {
            officewright_core::Error::Unsupported(message) => assert_eq!(
                message,
                "cannot subset font face 2: it uses a feature the subsetter does not support \
                 yet: unsupported feature in font"
            ),
            other => panic!("{other:?}"),
        }
        for err in [
            subsetter::Error::MalformedFont,
            subsetter::Error::CFFError,
            subsetter::Error::OverflowError,
        ] {
            let damaged = subsetter_error(0, &err);
            assert!(matches!(damaged, Error::Subset { face: 0, .. }), "{err}");
            assert!(
                matches!(
                    officewright_core::Error::from(damaged),
                    officewright_core::Error::Malformed(_)
                ),
                "{err}"
            );
        }
    }

    #[test]
    fn keeps_requested_glyphs_and_renumbers_them() {
        let font = font();
        let a = font.glyph_id('A').unwrap();
        let space = font.glyph_id(' ').unwrap();
        let subset = font.subset([a, space, a]).unwrap();
        assert_eq!(subset.glyph_id(GlyphId::NOTDEF), Some(GlyphId::NOTDEF));
        assert_eq!(subset.glyph_id(space), Some(GlyphId::new(1)));
        assert_eq!(subset.glyph_id(a), Some(GlyphId::new(2)));
        assert_eq!(subset.glyph_id(GlyphId::new(3)), None);
        assert_eq!(subset.glyph_count(), 3);
        assert_eq!(subset.outline_format(), OutlineFormat::TrueType);
        assert_eq!(subset.cff_table(), None);
        let out = FontRef::new(subset.data()).unwrap();
        assert!(
            out.cmap().is_err(),
            "the PDF supplies the character mapping"
        );
        // Read back, the subset has the original's advances under the new IDs.
        let reread = FontFace::from_bytes(subset.data().to_vec(), 0).unwrap();
        assert_eq!(reread.glyph_count(), 3);
        assert_eq!(
            reread.advance_width(GlyphId::new(1)),
            font.advance_width(space)
        );
        assert_eq!(reread.advance_width(GlyphId::new(2)), font.advance_width(a));
    }

    #[test]
    fn keeps_the_components_of_composite_glyphs() {
        let font = font();
        let aacute = font.glyph_id('\u{C1}').unwrap();
        let subset = font.subset([aacute]).unwrap();
        assert_eq!(subset.glyph_id(aacute), Some(GlyphId::new(1)));
        // .notdef, Á, and its components A and acute.
        assert_eq!(subset.glyph_count(), 4);
    }

    #[test]
    fn an_empty_subset_has_only_notdef() {
        let subset = font().subset([]).unwrap();
        assert_eq!(subset.glyph_count(), 1);
    }

    #[test]
    fn rejects_glyphs_the_font_does_not_have() {
        let err = font().subset([GlyphId::new(5)]).unwrap_err();
        assert!(
            matches!(err, Error::GlyphOutOfRange { glyph, glyph_count: 5, .. } if glyph.to_u16() == 5),
            "{err}"
        );
    }

    #[test]
    fn rejects_faces_without_outlines() {
        let font = FontFace::from_bytes(
            TestFont::new("Test Sans")
                .without_table(b"glyf")
                .without_table(b"loca")
                .build(),
            0,
        )
        .unwrap();
        assert_eq!(font.outline_format(), None);
        let err = font.subset([]).unwrap_err();
        assert!(
            matches!(err, Error::UnsupportedOutlines { format: None, .. }),
            "{err}"
        );
    }

    #[test]
    fn a_panic_in_the_subsetter_becomes_an_error() {
        let err = catch_subsetter_panic(3, || -> u8 { panic!("index out of bounds") }).unwrap_err();
        assert!(
            matches!(&err, Error::SubsetterPanicked { face: 3, message } if message == "index out of bounds"),
            "{err}"
        );
        assert_eq!(
            err.to_string(),
            "cannot subset font face 3: the subsetter crashed on its data (index out of bounds)"
        );
        assert_eq!(catch_subsetter_panic(0, || 7).unwrap(), 7);
    }

    #[test]
    fn damaged_fonts_give_errors_not_panics() {
        let data = TestFont::new("Test Sans").build();
        for at in 0..data.len() {
            for value in [0x00, 0x7F, 0xFF] {
                let mut damaged = data.clone();
                damaged[at] = value;
                let Ok(font) = FontFace::from_bytes(damaged, 0) else {
                    continue;
                };
                let glyphs = (0..font.glyph_count()).map(GlyphId::new);
                if let Err(err) = font.subset(glyphs) {
                    assert!(
                        !matches!(err, Error::SubsetterPanicked { .. }),
                        "byte {at} = {value:#04x}: {err}"
                    );
                }
            }
        }
    }
}
