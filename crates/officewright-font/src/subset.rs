use std::any::Any;
use std::fmt;
use std::panic::{self, UnwindSafe};

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

impl FontFace {
    /// Reduces the face to `glyphs`, plus `.notdef` and the components of composite
    /// glyphs, for embedding in a PDF.
    ///
    /// This does not check [`embedding`](Self::embedding) permissions; the caller
    /// decides what to do with a font whose license forbids embedding or subsetting.
    /// A variable font is reduced to its default instance.
    ///
    /// # Errors
    ///
    /// [`Error::GlyphOutOfRange`] if a glyph is not in the face,
    /// [`Error::UnsupportedOutlines`] for a face without outlines or with CFF2
    /// outlines, and [`Error::Subset`] or [`Error::SubsetterPanicked`] if the face is
    /// damaged in a way that only subsetting finds.
    pub fn subset(&self, glyphs: impl IntoIterator<Item = GlyphId>) -> Result<Subset> {
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
        let remapper = GlyphRemapper::new_from_glyphs_sorted(&ids);
        let data = self.data();
        let data = catch_subsetter_panic(face, || subsetter::subset(data, face, &remapper))?
            .map_err(|err| Error::Subset {
                face,
                source: Cause::new(err.to_string()),
            })?;
        // The output is read back: a subset the font parser cannot read would only
        // fail later, in a PDF viewer.
        let glyph_count = FontRef::new(&data)
            .and_then(|font| font.maxp())
            .map(|maxp| maxp.num_glyphs())
            .map_err(|err| Error::Subset {
                face,
                source: Cause::new(format!("the subset font is unreadable: {err}")),
            })?;
        Ok(Subset {
            data,
            remapper,
            glyph_count,
            outline_format,
        })
    }
}

/// Runs the subsetter and turns a panic into [`Error::SubsetterPanicked`].
///
/// subsetter 0.2.6 still has `unwrap()`, `unreachable!()` and unchecked arithmetic on
/// paths that malformed fonts can reach; fuzzing found an overflow panic in its `name`
/// table code (<https://github.com/typst/subsetter/issues/30>). A damaged font must
/// give `Err`, never a panic, also inside a dependency, so the panic is caught here
/// until upstream returns errors on those paths; panics that fuzzing finds are reported
/// there. Catching relies on unwinding, like the C ABI: built with `-C panic=abort` the
/// process aborts instead. The default panic hook still prints the message to stderr,
/// since a library must not replace the process-wide hook.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestFont;

    fn font() -> FontFace {
        FontFace::from_bytes(TestFont::new("Test Sans").build(), 0).unwrap()
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

    /// A `name` table with one record whose offset plus length exceeds 65535.
    pub(crate) fn name_table_with_overflowing_record() -> Vec<u8> {
        let mut name = Vec::new();
        for v in [0u16, 1, 18, 3, 1, 0x0409, 1, 4, 0xFFFE] {
            name.extend_from_slice(&v.to_be_bytes());
        }
        name.extend_from_slice(&[0, b'A', 0, b'B']);
        name
    }

    #[test]
    fn a_name_record_past_the_u16_range_is_an_error() {
        // subsetter 0.2.6 adds the record's offset and length as u16 and panics on
        // the overflow in builds with overflow checks (found by fuzz/font_subset,
        // https://github.com/typst/subsetter/issues/30).
        let data = TestFont::new("Test Sans")
            .with_table(b"name", name_table_with_overflowing_record())
            .build();
        let font = FontFace::from_bytes(data, 0).unwrap();
        let err = font.subset([]).unwrap_err();
        assert!(
            matches!(err, Error::SubsetterPanicked { .. } | Error::Subset { .. }),
            "{err}"
        );
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
