//! Whether a glyph of a face can go into a subset, checked one glyph at a time without
//! subsetting the face for each.
//!
//! The subsetter (subset 0.2.6) reads most of a face whatever glyphs it keeps, and only a
//! few things for each glyph. A face is subset once with `.notdef` alone, which settles
//! the first part; each glyph then needs only what the subsetter reads for it, read here
//! the way it reads it: its `glyf` entry and those of its components, or its `CFF `
//! charstring and Font DICT; its `hmtx` entry; and its glyph name index in a `post` table
//! of version 2.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Mutex, PoisonError};

use crate::error::{Cause, Error, Result};
use crate::font::{FontFace, GlyphId};
use crate::subset::{SubsetBudget, subsetter_table_range};

/// The most components of composite glyphs that the checks of one face follow in all,
/// counted again for each glyph whose check reaches them. Each glyph found sound is not
/// followed again, so real fonts, whose composites have a few components and nest a few
/// levels deep, follow a few for each glyph checked; the limit bounds a crafted face
/// whose glyphs reach many components each.
const MAX_COMPONENTS: u32 = 1 << 22;

/// What the checks of a face's glyphs found so far, shared by the clones of the face.
#[derive(Default)]
pub(crate) struct SubsetCheck(Mutex<Option<State>>);

struct State {
    /// The tables the checks of each glyph read, if subsetting the face with `.notdef`
    /// alone succeeds, else its error.
    face: std::result::Result<Tables, Error>,
    /// The glyphs found sound, with all the glyphs their components use: a bit each.
    sound: Vec<u64>,
    /// The glyphs found damaged, with their errors.
    damaged: HashMap<u16, Error>,
    /// How many operands, operators and hint mask bytes decompiling the CFF glyphs still
    /// to check may read: one subset may read no more for all its glyphs.
    steps: u32,
    /// How many more components of composite glyphs the checks may follow.
    components: u32,
    /// The numbers of CFF subroutines the checks have counted.
    subroutines: crate::cff::SubroutineCounts,
}

/// Where the tables are that the subsetter reads for each glyph it keeps, found as it
/// finds them.
struct Tables {
    /// The `glyf` table and its `loca`, if the subsetter takes the TrueType outlines;
    /// else it takes the `CFF ` table.
    glyf: Option<Glyf>,
    /// The `hmtx` table, if the subsetter finds one.
    hmtx: Option<Hmtx>,
    /// The number of glyph name indexes of a `post` table of version 2.
    post: Option<u16>,
}

struct Glyf {
    loca: Range<usize>,
    glyf: Range<usize>,
    /// Whether `loca` has 32-bit offsets.
    long: bool,
}

struct Hmtx {
    hmtx: Range<usize>,
    /// `numberOfHMetrics` as the subsetter reads it from `hhea`.
    metrics: u16,
}

impl SubsetCheck {
    /// Checks `glyph` of `face`, which this check belongs to; see
    /// [`FontFace::check_subset_glyph`].
    pub(crate) fn check(&self, face: &FontFace, glyph: GlyphId) -> Result<()> {
        let glyph_count = face.glyph_count();
        if glyph.to_u16() >= glyph_count {
            return Err(Error::GlyphOutOfRange {
                face: face.index(),
                glyph,
                glyph_count,
            });
        }
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let state = state.get_or_insert_with(|| State::new(face, MAX_COMPONENTS));
        state.check(face, glyph.to_u16())
    }
}

impl State {
    /// Subsets `face` with `.notdef` alone, and finds the tables the glyphs' checks read;
    /// the checks may follow `components` components of composite glyphs.
    fn new(face: &FontFace, components: u32) -> Self {
        let mut budget = SubsetBudget::new(u64::MAX);
        let tables = face
            .subset_within(std::iter::empty(), &mut budget)
            .and_then(|_| Tables::read(face));
        // What decompiling `.notdef` read, of what one subset may read.
        let read = u64::MAX - budget.left();
        let steps = u64::from(crate::cff::MAX_STEPS).saturating_sub(read);
        Self {
            face: tables,
            sound: vec![0; 1024],
            damaged: HashMap::new(),
            steps: u32::try_from(steps).unwrap_or(0),
            components,
            subroutines: crate::cff::SubroutineCounts::default(),
        }
    }

    fn check(&mut self, face: &FontFace, glyph: u16) -> Result<()> {
        let tables = match &self.face {
            Ok(tables) => tables,
            Err(err) => return Err(err.duplicate()),
        };
        if glyph == 0 || is_marked(&self.sound, glyph) {
            return Ok(());
        }
        if let Some(err) = self.damaged.get(&glyph) {
            return Err(err.duplicate());
        }
        let index = face.index();
        let data = face.data();
        let checked = match &tables.glyf {
            Some(glyf) => {
                let checked = check_closure(
                    tables,
                    glyf,
                    data,
                    glyph,
                    &mut self.sound,
                    &self.damaged,
                    &mut self.components,
                );
                // A damaged component is kept too, so that the glyphs that use it fail
                // without reading it again.
                if let Err(Failure::Glyph(component, reason)) = &checked {
                    if *component != glyph {
                        let failure = Failure::Glyph(*component, reason.clone());
                        self.damaged
                            .insert(*component, failure.error(index, *component));
                    }
                }
                checked.map_err(|failure| failure.error(index, glyph))
            }
            None => crate::font::font_ref(data, index)
                .and_then(|font| {
                    crate::cff::check_glyph(
                        &font,
                        data,
                        index,
                        glyph,
                        &mut self.steps,
                        &mut self.subroutines,
                    )
                })
                .and_then(|()| {
                    tables
                        .metrics(data, glyph)
                        .map_err(|reason| Failure::Glyph(glyph, reason).error(index, glyph))
                }),
        };
        match &checked {
            Ok(()) => mark(&mut self.sound, glyph),
            Err(err) => {
                self.damaged.insert(glyph, err.duplicate());
            }
        }
        checked
    }
}

fn is_marked(bits: &[u64], glyph: u16) -> bool {
    bits.get(usize::from(glyph / 64))
        .is_some_and(|word| word & 1 << (glyph % 64) != 0)
}

fn mark(bits: &mut [u64], glyph: u16) {
    if let Some(word) = bits.get_mut(usize::from(glyph / 64)) {
        *word |= 1 << (glyph % 64);
    }
}

/// Why a TrueType glyph cannot go into a subset.
enum Failure {
    /// Glyph `.0`, the glyph checked or one of the components it uses, is damaged.
    Glyph(u16, String),
    /// Glyph `.0` was found damaged before.
    Known(u16),
    /// Following the components went past [`MAX_COMPONENTS`].
    Limit,
}

impl Failure {
    /// The error for `glyph` of face `face`.
    fn error(self, face: u32, glyph: u16) -> Error {
        let damaged = |text: String| Error::Subset {
            face,
            source: Cause::new(text),
        };
        match self {
            Self::Glyph(at, reason) if at == glyph => damaged(format!("glyph {glyph}: {reason}")),
            Self::Glyph(at, reason) => damaged(format!(
                "glyph {glyph} uses glyph {at} as a component, and glyph {at}: {reason}"
            )),
            Self::Known(at) => damaged(format!(
                "glyph {glyph} uses glyph {at} as a component, which cannot go into a subset"
            )),
            Self::Limit => Error::TableOverLimit {
                face,
                table: "glyf",
                source: Cause::new(format!(
                    "checking glyph {glyph} follows more than the {MAX_COMPONENTS} components \
                     of composite glyphs the checks of one face may follow"
                )),
            },
        }
    }
}

/// Checks `glyph` and every glyph its components use, as the subsetter's glyph closure
/// and its `glyf`, `hmtx` and `post` code read them, taking the glyphs marked in `sound`
/// and those in `damaged` as found. Where the check succeeds, it marks every glyph it
/// reached as sound: the glyphs their components use were all reached too.
fn check_closure(
    tables: &Tables,
    glyf: &Glyf,
    data: &[u8],
    glyph: u16,
    sound: &mut [u64],
    damaged: &HashMap<u16, Error>,
    components: &mut u32,
) -> std::result::Result<(), Failure> {
    let mut reached = vec![glyph];
    let mut seen = HashSet::from([glyph]);
    let mut at = 0;
    while let Some(&next) = reached.get(at) {
        at += 1;
        if is_marked(sound, next) {
            continue;
        }
        if next != glyph && damaged.contains_key(&next) {
            return Err(Failure::Known(next));
        }
        let found = glyf
            .components(data, next)
            .and_then(|found| tables.metrics(data, next).map(|()| found))
            .map_err(|reason| Failure::Glyph(next, reason))?;
        for component in found {
            *components = components.checked_sub(1).ok_or(Failure::Limit)?;
            if seen.insert(component) {
                reached.push(component);
            }
        }
    }
    for glyph in reached {
        mark(sound, glyph);
    }
    Ok(())
}

impl Tables {
    /// Finds the tables of `face` as the subsetter finds them. Subsetting the face with
    /// `.notdef` alone has succeeded, so those it needs are there and readable.
    fn read(face: &FontFace) -> Result<Self> {
        let data = face.data();
        let font = crate::font::font_ref(data, face.index())?;
        let table = |tag: &[u8; 4]| subsetter_table_range(&font, data, tag);
        let unreadable = || Error::Subset {
            face: face.index(),
            source: Cause::new("the tables the subsetter reads for each glyph cannot be read"),
        };
        let glyf = match table(b"glyf") {
            Some(glyf) => {
                let loca = table(b"loca").ok_or_else(unreadable)?;
                let head = table(b"head")
                    .and_then(|head| data.get(head))
                    .ok_or_else(unreadable)?;
                // indexToLocFormat, at byte 50 of `head`.
                let long = i16_at(head, 50).ok_or_else(unreadable)? != 0;
                Some(Glyf { loca, glyf, long })
            }
            None => None,
        };
        let hmtx = match table(b"hmtx") {
            Some(hmtx) => {
                let metrics = table(b"hhea")
                    .and_then(|hhea| data.get(hhea))
                    .and_then(number_of_h_metrics)
                    .ok_or_else(unreadable)?;
                Some(Hmtx { hmtx, metrics })
            }
            None => None,
        };
        let post = match table(b"post").and_then(|post| data.get(post)) {
            Some(post) if u32_at(post, 0) == Some(0x0002_0000) => {
                // numGlyphs, after the 32 bytes of the header.
                Some(u16_at(post, 32).ok_or_else(unreadable)?)
            }
            _ => None,
        };
        Ok(Self { glyf, hmtx, post })
    }

    /// Fails where the subsetter cannot read the `hmtx` entry or the `post` glyph name
    /// index of `glyph`, with the reason.
    fn metrics(&self, data: &[u8], glyph: u16) -> std::result::Result<(), String> {
        if let Some(Hmtx { hmtx, metrics }) = &self.hmtx {
            let hmtx = data.get(hmtx.clone()).unwrap_or_default();
            let (glyph, metrics) = (usize::from(glyph), usize::from(*metrics));
            // An advance width and a left side bearing for the first glyphs, then a left
            // side bearing alone.
            let (at, len) = if glyph < metrics {
                (glyph * 4, 4)
            } else {
                (metrics * 4 + (glyph - metrics) * 2, 2)
            };
            if hmtx.len() < at + len {
                return Err("it has no entry in the hmtx table".to_owned());
            }
        }
        if let Some(count) = self.post {
            if glyph >= count {
                return Err(format!(
                    "the post table names {count} glyphs, so it has no name for this one"
                ));
            }
        }
        Ok(())
    }
}

/// `numberOfHMetrics`, at byte 34 of `hhea`, as the subsetter reads it: it skips the 34
/// bytes before only where there are 34, and reads a shorter table from its start.
fn number_of_h_metrics(hhea: &[u8]) -> Option<u16> {
    let at = if hhea.len() >= 34 { 34 } else { 0 };
    u16_at(hhea, at)
}

impl Glyf {
    /// The components of `glyph` as the subsetter reads its `glyf` entry: none for an
    /// empty or simple glyph; fails where the entry cannot be read, with the reason.
    fn components(&self, data: &[u8], glyph: u16) -> std::result::Result<Vec<u16>, String> {
        let loca = data.get(self.loca.clone()).unwrap_or_default();
        let glyf = data.get(self.glyf.clone()).unwrap_or_default();
        let offset = |n: usize| {
            if self.long {
                u32_at(loca, 4 * n).and_then(|offset| usize::try_from(offset).ok())
            } else {
                u16_at(loca, 2 * n).map(|offset| 2 * usize::from(offset))
            }
        };
        let entry = offset(usize::from(glyph))
            .zip(offset(usize::from(glyph) + 1))
            .and_then(|(from, to)| glyf.get(from..to))
            .ok_or_else(|| "its loca entries do not give a range of the glyf table".to_owned())?;
        if entry.is_empty() {
            return Ok(Vec::new());
        }
        let contours = i16_at(entry, 0)
            .ok_or_else(|| "its glyf entry is too short for a glyph header".to_owned())?;
        if contours >= 0 {
            return Ok(Vec::new());
        }
        composite_components(entry)
            .ok_or_else(|| "its composite glyph description is cut off".to_owned())
    }
}

/// The components of a composite glyph description, read as the subsetter's
/// `remap_component_glyph` (`src/glyf.rs`) reads it, `None` where it fails.
fn composite_components(entry: &[u8]) -> Option<Vec<u16>> {
    const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
    const WE_HAVE_A_SCALE: u16 = 0x0008;
    const MORE_COMPONENTS: u16 = 0x0020;
    const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
    const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
    // numberOfContours and the bounding box.
    let mut at = 10;
    entry.get(..at)?;
    let mut components = Vec::new();
    loop {
        let flags = u16_at(entry, at)?;
        components.push(u16_at(entry, at + 2)?);
        at += 4;
        at += if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            4
        } else {
            2
        };
        at += if flags & WE_HAVE_A_SCALE != 0 {
            2
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            4
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            8
        } else {
            0
        };
        // The arguments and the scale are read whole.
        entry.get(..at)?;
        if flags & MORE_COMPONENTS == 0 {
            // The instructions after the last component are copied as they are.
            return Some(components);
        }
    }
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_be_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestFont, glyph_entries, hhea, hmtx, post_naming, subset_checks};

    /// The test font with `entries` as its `glyf` entries, at the offsets their lengths
    /// give, unpadded.
    fn with_glyphs(entries: &[Vec<u8>]) -> TestFont {
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for entry in entries {
            loca.extend_from_slice(&u32::try_from(glyf.len()).unwrap().to_be_bytes());
            glyf.extend_from_slice(entry);
        }
        loca.extend_from_slice(&u32::try_from(glyf.len()).unwrap().to_be_bytes());
        TestFont::new("Check Sans")
            .with_table(b"glyf", glyf)
            .with_table(b"loca", loca)
    }

    /// The composite glyph 4, `Á`, with its second component `component` instead of the
    /// acute.
    fn aacute_with(component: u16) -> Vec<u8> {
        let mut entry = glyph_entries()[4].clone();
        let at = entry.len() - 6;
        entry[at..at + 2].copy_from_slice(&component.to_be_bytes());
        entry
    }

    #[test]
    fn passes_every_glyph_of_a_sound_face() {
        let checks = subset_checks(&TestFont::new("Check Sans").build());
        assert_eq!(checks, vec![Ok(()); 5]);
        let checks = subset_checks(&with_glyphs(&glyph_entries()).build());
        assert_eq!(checks, vec![Ok(()); 5]);
    }

    #[test]
    fn fails_a_glyph_whose_glyf_entry_cannot_be_read_and_the_glyphs_that_use_it() {
        // The acute's entry is one byte: too short for numberOfContours.
        let mut entries = glyph_entries();
        entries[3] = vec![0xFF];
        assert_eq!(
            subset_checks(&with_glyphs(&entries).build()),
            [
                Ok(()),
                Ok(()),
                Ok(()),
                Err("glyph 3: its glyf entry is too short for a glyph header".to_owned()),
                Err(
                    "glyph 4 uses glyph 3 as a component, and glyph 3: its glyf entry is too \
                     short for a glyph header"
                        .to_owned()
                ),
            ]
        );
        // A composite whose last component is cut off: the acute's offset is one byte short.
        let mut entries = glyph_entries();
        entries[4].pop();
        let checks = subset_checks(&with_glyphs(&entries).build());
        assert_eq!(checks[..4], [Ok(()), Ok(()), Ok(()), Ok(())]);
        assert_eq!(
            checks[4],
            Err("glyph 4: its composite glyph description is cut off".to_owned())
        );
        // A component past the glyphs, which `loca` has no entry for.
        let mut entries = glyph_entries();
        entries[4] = aacute_with(7);
        assert_eq!(
            subset_checks(&with_glyphs(&entries).build())[4],
            Err(
                "glyph 4 uses glyph 7 as a component, and glyph 7: its loca entries do not \
                 give a range of the glyf table"
                    .to_owned()
            )
        );
    }

    #[test]
    fn fails_a_composite_that_cycles_only_where_a_glyph_is_damaged() {
        // `Á` uses itself: the subsetter keeps it once.
        let mut entries = glyph_entries();
        entries[4] = aacute_with(4);
        assert_eq!(
            subset_checks(&with_glyphs(&entries).build()),
            vec![Ok(()); 5]
        );
    }

    #[test]
    fn fails_every_glyph_of_a_face_whose_notdef_cannot_go_into_a_subset() {
        let mut entries = glyph_entries();
        entries[0] = vec![0xFF];
        let checks = subset_checks(&with_glyphs(&entries).build());
        assert_eq!(checks.len(), 5);
        assert!(checks.iter().all(std::result::Result::is_err), "{checks:?}");
        // The error is the subset's, the same for every glyph.
        assert!(checks.iter().all(|check| check == &checks[0]), "{checks:?}");
    }

    #[test]
    fn fails_a_glyph_without_an_hmtx_entry_or_post_name() {
        // Two full metrics, then left side bearings for glyphs 2 and 3 only.
        let mut hhea = hhea();
        let at = hhea.len() - 2;
        hhea[at..].copy_from_slice(&2u16.to_be_bytes());
        let mut short = hmtx()[..8].to_vec();
        short.extend_from_slice(&[0, 0, 0, 100]);
        let font = TestFont::new("Check Sans")
            .with_table(b"hhea", hhea)
            .with_table(b"hmtx", short);
        if crate::FontFace::from_bytes(font.build(), 0).is_ok() {
            let checks = subset_checks(&font.build());
            assert_eq!(checks[..4], [Ok(()), Ok(()), Ok(()), Ok(())]);
            assert_eq!(
                checks[4],
                Err("glyph 4: it has no entry in the hmtx table".to_owned())
            );
        }
        // A `post` table of version 2 that names three glyphs.
        let checks = subset_checks(
            &TestFont::new("Check Sans")
                .with_table(b"post", post_naming(3))
                .build(),
        );
        assert_eq!(checks[..3], [Ok(()), Ok(()), Ok(())]);
        let unnamed = |glyph| {
            Err(format!(
                "glyph {glyph}: the post table names 3 glyphs, so it has no name for this one"
            ))
        };
        assert_eq!(checks[3], unnamed(3));
        // Glyph 4's own entry is read before its components'.
        assert_eq!(checks[4], unnamed(4));
    }

    #[test]
    fn keeps_what_it_found_for_the_clones_of_a_face() {
        let mut entries = glyph_entries();
        entries[3] = vec![0xFF];
        let face = crate::FontFace::from_bytes(with_glyphs(&entries).build(), 0).unwrap();
        let reason = |result: Result<()>| {
            std::error::Error::source(&result.unwrap_err())
                .unwrap()
                .to_string()
        };
        assert!(reason(face.check_subset_glyph(GlyphId::new(3))).starts_with("glyph 3: "));
        // Glyph 3 was found damaged, so glyph 4, checked on a clone, is not read again.
        let clone = face.clone();
        assert_eq!(
            reason(clone.check_subset_glyph(GlyphId::new(4))),
            "glyph 4 uses glyph 3 as a component, which cannot go into a subset"
        );
        assert_eq!(
            reason(face.check_subset_glyph(GlyphId::new(4))),
            reason(clone.check_subset_glyph(GlyphId::new(4)))
        );
        clone.check_subset_glyph(GlyphId::new(2)).unwrap();
        // A damaged component found by checking a glyph that uses it is kept too: the
        // space, made a composite of the acute, does not read it again.
        let mut space = glyph_entries()[4].clone();
        space.truncate(space.len() - 8);
        let at = space.len() - 6;
        space[at - 2..at].copy_from_slice(&0x0003u16.to_be_bytes());
        space[at..at + 2].copy_from_slice(&3u16.to_be_bytes());
        entries[1] = space;
        let face = crate::FontFace::from_bytes(with_glyphs(&entries).build(), 0).unwrap();
        assert!(
            reason(face.check_subset_glyph(GlyphId::new(4)))
                .ends_with("and glyph 3: its glyf entry is too short for a glyph header")
        );
        assert_eq!(
            reason(face.check_subset_glyph(GlyphId::new(1))),
            "glyph 1 uses glyph 3 as a component, which cannot go into a subset"
        );
        assert!(matches!(
            face.check_subset_glyph(GlyphId::new(5)),
            Err(Error::GlyphOutOfRange { glyph_count: 5, .. })
        ));
    }

    #[test]
    fn reads_the_number_of_h_metrics_as_the_subsetter_does() {
        assert_eq!(number_of_h_metrics(&hhea()), Some(5));
        // A table too short to skip to it is read from its start: version 1.0 is 0x0001.
        assert_eq!(number_of_h_metrics(&hhea()[..33]), Some(1));
        assert_eq!(number_of_h_metrics(&[0]), None);
    }

    #[test]
    fn bounds_the_components_it_follows() {
        let face = crate::FontFace::from_bytes(TestFont::new("Check Sans").build(), 0).unwrap();
        // `Á` has two components: with room for one, its check fails, and no glyph is
        // taken as sound.
        let mut state = State::new(&face, 1);
        let err = state.check(&face, 4).unwrap_err();
        assert!(
            matches!(err, Error::TableOverLimit { table: "glyf", .. }),
            "{err}"
        );
        assert_eq!(
            std::error::Error::source(&err).unwrap().to_string(),
            "checking glyph 4 follows more than the 4194304 components of composite glyphs \
             the checks of one face may follow"
        );
        assert!(!is_marked(&state.sound, 2));
        state.check(&face, 2).unwrap();
        // With room for two, it passes, and its components are sound without another check.
        let mut state = State::new(&face, 2);
        state.check(&face, 4).unwrap();
        assert!(is_marked(&state.sound, 2) && is_marked(&state.sound, 3));
        assert_eq!(state.components, 0);
        state.check(&face, 4).unwrap();
    }
}
