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
    /// How many operands, operators and hint mask bytes the checks of CFF glyphs may
    /// still read in all, those of glyphs that fail included.
    work: u32,
    /// How many more components of composite glyphs the checks may follow.
    components: u32,
    /// The glyphs found sound whose components may reach a glyph past the face's glyph
    /// count, a bit each: only glyphs whose closure, or `.notdef`'s, reaches one can
    /// take every glyph ID with `.notdef`.
    past: Vec<u64>,
    /// Whether `.notdef`'s components reach a glyph past the face's glyph count.
    notdef_past: bool,
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
    /// Where the `CFF ` table is, and what the checks of its glyphs need of it, if the
    /// subsetter takes the CFF outlines.
    cff: Option<(Range<usize>, crate::cff::CffGlyphs)>,
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
        let mut components = components;
        let notdef_past = match &tables {
            Ok(Tables {
                glyf: Some(glyf), ..
            }) => reaches_past(glyf, face.data(), face.glyph_count(), &mut components),
            _ => false,
        };
        Self {
            face: tables,
            sound: vec![0; 1024],
            damaged: HashMap::new(),
            steps: u32::try_from(steps).unwrap_or(0),
            work: crate::cff::MAX_CHECK_WORK,
            components,
            past: vec![0; 1024],
            notdef_past,
        }
    }

    fn check(&mut self, face: &FontFace, glyph: u16) -> Result<()> {
        let Self {
            face: tables,
            sound,
            damaged,
            steps,
            work,
            components,
            past,
            notdef_past,
        } = self;
        let tables = match tables {
            Ok(tables) => tables,
            Err(err) => return Err(err.duplicate()),
        };
        if glyph == 0 || is_marked(sound, glyph) {
            return Ok(());
        }
        if let Some(err) = damaged.get(&glyph) {
            return Err(err.duplicate());
        }
        let index = face.index();
        let data = face.data();
        let checked = if let Some(glyf) = &tables.glyf {
            let checked = check_closure(
                &ClosureCheck {
                    tables,
                    glyf,
                    data,
                    glyph_count: face.glyph_count(),
                },
                glyph,
                &mut Found {
                    sound,
                    past,
                    damaged,
                    components,
                    notdef_past: *notdef_past,
                },
            );
            // A damaged component is kept too, so that the glyphs that use it fail
            // without reading it again.
            if let Err(Failure::Glyph(component, reason)) = &checked {
                if *component != glyph {
                    let failure = Failure::Glyph(*component, reason.clone());
                    damaged.insert(*component, failure.error(index, *component));
                }
            }
            checked.map_err(|failure| failure.error(index, glyph))
        } else if let Some((range, glyphs)) = &mut tables.cff {
            let cff = data.get(range.clone()).unwrap_or_default();
            // The glyph takes what it read off the steps only once it passed everything.
            crate::cff::check_glyph(cff, glyphs, index, glyph, *steps, work).and_then(|read| {
                tables
                    .metrics(data, glyph)
                    .map_err(|reason| Failure::Glyph(glyph, reason).error(index, glyph))?;
                *steps = steps.saturating_sub(read);
                Ok(())
            })
        } else {
            Err(Error::Subset {
                face: index,
                source: Cause::new("the tables the subsetter reads for each glyph cannot be read"),
            })
        };
        match &checked {
            Ok(()) => mark(sound, glyph),
            Err(err) => {
                damaged.insert(glyph, err.duplicate());
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
    /// The glyph and the glyphs its components use take every glyph ID with `.notdef`.
    AllIds,
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
            Self::AllIds => Error::SubsetUnsupported {
                face,
                source: Cause::new(format!(
                    "glyph {glyph} and the glyphs its components use take all 65536 glyph \
                     IDs with .notdef, more than the subsetter can number"
                )),
            },
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

/// Whether subsetting the TrueType outlines of `face` to `glyphs` would make the subsetter
/// number all 65,536 glyph IDs in its glyph closure, on which it panics
/// ([`closure_overflows`]); false for a face whose `glyf` table it does not take.
pub(crate) fn subset_overflows(face: &FontFace, glyphs: &[u16]) -> bool {
    let data = face.data();
    let Ok(font) = crate::font::font_ref(data, face.index()) else {
        return false;
    };
    let table = |tag: &[u8; 4]| subsetter_table_range(&font, data, tag);
    let (Some(glyf), Some(loca), Some(head)) = (table(b"glyf"), table(b"loca"), table(b"head"))
    else {
        return false;
    };
    let Some(long) = data.get(head).and_then(|head| i16_at(head, 50)) else {
        return false;
    };
    let glyf = Glyf {
        loca,
        glyf,
        long: long != 0,
    };
    // The subsetter keeps `.notdef` first, then the glyphs in order, once each.
    let mut kept: Vec<u16> = glyphs.iter().copied().filter(|&glyph| glyph != 0).collect();
    kept.sort_unstable();
    kept.dedup();
    kept.insert(0, 0);
    closure_overflows(&glyf, data, &kept, None).unwrap_or(false)
}

/// What [`check_closure`] reads a face's glyphs from.
struct ClosureCheck<'a> {
    tables: &'a Tables,
    glyf: &'a Glyf,
    data: &'a [u8],
    glyph_count: u16,
}

/// What the checks of a face's TrueType glyphs found so far.
struct Found<'a> {
    sound: &'a mut [u64],
    past: &'a mut [u64],
    damaged: &'a HashMap<u16, Error>,
    components: &'a mut u32,
    notdef_past: bool,
}

/// Checks `glyph` and every glyph its components use, as the subsetter's glyph closure
/// and its `glyf`, `hmtx` and `post` code read them, taking the glyphs marked in `sound`
/// and those in `damaged` as found. Where the check succeeds, it marks every glyph it
/// reached as sound: the glyphs their components use were all reached too.
///
/// Components may name glyphs past the face's glyph count, which the subsetter keeps
/// where `loca` gives them an entry. Where the glyph's components, or `.notdef`'s, reach
/// one, the glyphs they use might take every glyph ID with `.notdef`, on which the
/// subsetter panics, so that is checked as it numbers them ([`closure_overflows`]),
/// within the components the checks may follow.
fn check_closure(
    face: &ClosureCheck<'_>,
    glyph: u16,
    found: &mut Found<'_>,
) -> std::result::Result<(), Failure> {
    let mut reached = vec![glyph];
    let mut seen = HashSet::from([glyph]);
    let mut reaches_past = false;
    let mut at = 0;
    while let Some(&next) = reached.get(at) {
        at += 1;
        if is_marked(found.sound, next) {
            reaches_past |= is_marked(found.past, next);
            continue;
        }
        if next != glyph && found.damaged.contains_key(&next) {
            return Err(Failure::Known(next));
        }
        let components = face
            .glyf
            .components(face.data, next)
            .and_then(|components| face.tables.metrics(face.data, next).map(|()| components))
            .map_err(|reason| Failure::Glyph(next, reason))?;
        for component in components {
            *found.components = found.components.checked_sub(1).ok_or(Failure::Limit)?;
            reaches_past |= component >= face.glyph_count;
            if seen.insert(component) {
                reached.push(component);
            }
        }
    }
    if reaches_past || found.notdef_past {
        match closure_overflows(face.glyf, face.data, &[0, glyph], Some(found.components)) {
            Some(true) => return Err(Failure::AllIds),
            Some(false) => {}
            None => return Err(Failure::Limit),
        }
    }
    for glyph in reached {
        mark(found.sound, glyph);
        if reaches_past {
            mark(found.past, glyph);
        }
    }
    Ok(())
}

/// Whether the components of `glyph`, followed as far as they go, reach a glyph past
/// `glyph_count`; each component read takes one off `components`, and none are left
/// counts as reaching one.
fn reaches_past(glyf: &Glyf, data: &[u8], glyph_count: u16, components: &mut u32) -> bool {
    let mut seen = vec![0u64; 1024];
    mark(&mut seen, 0);
    let mut stack = vec![0];
    while let Some(glyph) = stack.pop() {
        for component in glyf.components(data, glyph).unwrap_or_default() {
            let Some(left) = components.checked_sub(1) else {
                return true;
            };
            *components = left;
            if component >= glyph_count {
                return true;
            }
            if !is_marked(&seen, component) {
                mark(&mut seen, component);
                stack.push(component);
            }
        }
    }
    false
}

/// Whether the subsetter numbers all 65,536 glyph IDs when it forms the glyph closure of
/// `kept` (`glyf::closure` in its `src/glyf.rs`), on which it panics ("remapper was
/// overflowed"): it takes the glyphs to keep, and each glyph's components as it reaches
/// the glyph, reading them as its `component_glyphs` does, which stops at the first it
/// cannot read; it stops at a glyph whose entry is too short for a header, with an error
/// of its own.
///
/// The subsetter reads a glyph's components again each time it reaches the glyph; by
/// then every one of them has been numbered, so reading them again numbers nothing and
/// fails nowhere, and is skipped here. Each component read takes one off `components`,
/// if given; `None` where none are left.
fn closure_overflows(
    glyf: &Glyf,
    data: &[u8],
    kept: &[u16],
    mut components: Option<&mut u32>,
) -> Option<bool> {
    let mut numbered = vec![0u64; 1024];
    let mut read = vec![0u64; 1024];
    let mut count = 0usize;
    let mut stack: Vec<u16> = kept.to_vec();
    for &glyph in kept {
        if !is_marked(&numbered, glyph) {
            mark(&mut numbered, glyph);
            count += 1;
        }
    }
    while let Some(glyph) = stack.pop() {
        if !is_marked(&numbered, glyph) {
            mark(&mut numbered, glyph);
            count += 1;
        }
        if count > usize::from(u16::MAX) {
            return Some(true);
        }
        if is_marked(&read, glyph) {
            continue;
        }
        mark(&mut read, glyph);
        let Some(entry) = glyf.entry(data, glyph) else {
            continue;
        };
        if entry.is_empty() {
            continue;
        }
        let Some(contours) = i16_at(entry, 0) else {
            return Some(false);
        };
        if contours >= 0 {
            continue;
        }
        let Some(found) = closure_components(entry) else {
            return Some(false);
        };
        if let Some(components) = components.as_deref_mut() {
            let len = u32::try_from(found.len()).unwrap_or(u32::MAX);
            *components = components.checked_sub(len)?;
        }
        stack.extend(found.into_iter().filter(|&c| !is_marked(&numbered, c)));
    }
    Some(false)
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
        // The subsetter reads `CFF ` only when it finds no `glyf` table.
        let cff = match glyf {
            Some(_) => None,
            None => crate::cff::CffGlyphs::read(&font, data),
        };
        Ok(Self {
            glyf,
            hmtx,
            post,
            cff,
        })
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
    /// The `glyf` entry of `glyph`, as the subsetter's `Table::glyph_data` finds it from its
    /// `loca` entries; `None` where they give no range of the table.
    fn entry<'a>(&self, data: &'a [u8], glyph: u16) -> Option<&'a [u8]> {
        let loca = data.get(self.loca.clone()).unwrap_or_default();
        let glyf = data.get(self.glyf.clone()).unwrap_or_default();
        let offset = |n: usize| {
            if self.long {
                u32_at(loca, 4 * n).and_then(|offset| usize::try_from(offset).ok())
            } else {
                u16_at(loca, 2 * n).map(|offset| 2 * usize::from(offset))
            }
        };
        let (from, to) = offset(usize::from(glyph)).zip(offset(usize::from(glyph) + 1))?;
        glyf.get(from..to)
    }

    /// The components of `glyph` as the subsetter reads its `glyf` entry: none for an
    /// empty or simple glyph; fails where the entry cannot be read, with the reason.
    fn components(&self, data: &[u8], glyph: u16) -> std::result::Result<Vec<u16>, String> {
        let entry = self
            .entry(data, glyph)
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

/// The components of a composite glyph description as the subsetter's closure reads them
/// (`component_glyphs` in its `src/glyf.rs`): `None` where the header is cut off, and
/// otherwise the components up to the first whose flags or glyph cannot be read.
fn closure_components(entry: &[u8]) -> Option<Vec<u16>> {
    const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
    const WE_HAVE_A_SCALE: u16 = 0x0008;
    const MORE_COMPONENTS: u16 = 0x0020;
    const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
    const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
    entry.get(..10)?;
    let mut at = 10;
    let mut components = Vec::new();
    // Each read that runs past the entry leaves the place unchanged, as the subsetter's
    // reader does; only the flags and the glyph end the components.
    let skip = |at: &mut usize, len: usize| {
        if entry.get(*at..*at + len).is_some() {
            *at += len;
        }
    };
    loop {
        let (Some(flags), Some(component)) = (u16_at(entry, at), u16_at(entry, at + 2)) else {
            return Some(components);
        };
        at += 4;
        components.push(component);
        if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            skip(&mut at, 2);
            skip(&mut at, 2);
        } else {
            skip(&mut at, 2);
        }
        if flags & WE_HAVE_A_SCALE != 0 {
            skip(&mut at, 2);
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            skip(&mut at, 2);
            skip(&mut at, 2);
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            for _ in 0..4 {
                skip(&mut at, 2);
            }
        }
        if flags & MORE_COMPONENTS == 0 {
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

    /// The table `tag` of the font `data`.
    fn table_of(data: &[u8], tag: &[u8; 4]) -> Vec<u8> {
        let count = usize::from(u16::from_be_bytes([data[4], data[5]]));
        (0..count)
            .map(|i| 12 + 16 * i)
            .find(|&at| &data[at..at + 4] == tag)
            .map(|at| {
                let offset = u32::from_be_bytes(data[at + 8..at + 12].try_into().unwrap()) as usize;
                let len = u32::from_be_bytes(data[at + 12..at + 16].try_into().unwrap()) as usize;
                data[offset..offset + len].to_vec()
            })
            .unwrap()
    }

    /// A font of `entries` as its glyphs: `loca` with 16-bit offsets where `short` (every
    /// entry then of even length), a full `hmtx` and `maxp` and `hhea` counting them.
    fn font_of(entries: &[Vec<u8>], short: bool) -> TestFont {
        let base = TestFont::new("Check Sans").build();
        let count = u16::try_from(entries.len()).unwrap();
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        let offset = |loca: &mut Vec<u8>, at: usize| {
            if short {
                loca.extend_from_slice(&u16::try_from(at / 2).unwrap().to_be_bytes());
            } else {
                loca.extend_from_slice(&u32::try_from(at).unwrap().to_be_bytes());
            }
        };
        for entry in entries {
            offset(&mut loca, glyf.len());
            glyf.extend_from_slice(entry);
        }
        offset(&mut loca, glyf.len());
        let mut head = table_of(&base, b"head");
        head[50..52].copy_from_slice(&i16::from(!short).to_be_bytes());
        let mut maxp = table_of(&base, b"maxp");
        maxp[4..6].copy_from_slice(&count.to_be_bytes());
        let mut hhea = hhea();
        let at = hhea.len() - 2;
        hhea[at..].copy_from_slice(&count.to_be_bytes());
        TestFont::new("Check Sans")
            .with_table(b"glyf", glyf)
            .with_table(b"loca", loca)
            .with_table(b"head", head)
            .with_table(b"maxp", maxp)
            .with_table(b"hhea", hhea)
            .with_table(b"hmtx", [0x01, 0xF4, 0, 0].repeat(entries.len()))
    }

    /// The test font's glyphs, each padded to an even length.
    fn even_entries() -> Vec<Vec<u8>> {
        glyph_entries()
            .into_iter()
            .map(|mut entry| {
                entry.resize(entry.len().next_multiple_of(2), 0);
                entry
            })
            .collect()
    }

    #[test]
    fn reads_loca_with_16_bit_offsets() {
        assert_eq!(
            subset_checks(&font_of(&even_entries(), true).build()),
            vec![Ok(()); 5]
        );
        // The acute's entry ends before it starts: its offset and the next swapped.
        let font = font_of(&even_entries(), true).build();
        let mut loca = table_of(&font, b"loca");
        loca.swap(6, 8);
        loca.swap(7, 9);
        let checks = subset_checks(
            &font_of(&even_entries(), true)
                .with_table(b"loca", loca)
                .build(),
        );
        assert_eq!(
            checks[3],
            Err("glyph 3: its loca entries do not give a range of the glyf table".to_owned())
        );
    }

    #[test]
    fn keeps_what_it_found_of_glyphs_past_the_first_64() {
        // Glyph 1 is damaged and glyph 65 empty: finding 65 sound marks no other glyph.
        let mut entries = vec![Vec::new(); 70];
        entries[0] = glyph_entries()[0].clone();
        entries[1] = vec![0xFF];
        let font = font_of(&entries, false).build();
        let checks = subset_checks(&font);
        assert!(checks[1].is_err() && checks[65].is_ok(), "{checks:?}");
        let face = crate::FontFace::from_bytes(font, 0).unwrap();
        face.check_subset_glyph(GlyphId::new(65)).unwrap();
        assert!(face.check_subset_glyph(GlyphId::new(1)).is_err());
    }

    #[test]
    fn reads_hmtx_entries_whole() {
        // With 4 full metrics, glyph 4 has a side bearing alone, 2 bytes: one byte short
        // fails it. (A table one byte short of its full metrics is not a face.)
        let font = |metrics: u16, len: usize| {
            let mut hhea = hhea();
            let at = hhea.len() - 2;
            hhea[at..].copy_from_slice(&metrics.to_be_bytes());
            let mut hmtx = hmtx();
            hmtx.truncate(len);
            TestFont::new("Check Sans")
                .with_table(b"hhea", hhea)
                .with_table(b"hmtx", hmtx)
                .build()
        };
        assert!(crate::FontFace::from_bytes(font(5, 19), 0).is_err());
        assert_eq!(subset_checks(&font(5, 20)), vec![Ok(()); 5]);
        assert_eq!(subset_checks(&font(4, 18)), vec![Ok(()); 5]);
        assert_eq!(
            subset_checks(&font(4, 17))[4],
            Err("glyph 4: it has no entry in the hmtx table".to_owned())
        );
    }

    #[test]
    fn reads_composite_scales_by_their_size() {
        // `Á` with a scale of each kind on its last component (flags 0x0008, 0x0040 and
        // 0x0080 with their 2, 4 and 8 bytes), and cut one byte short.
        for (flag, size) in [(0x0008u16, 2), (0x0040, 4), (0x0080, 8)] {
            let mut aacute = glyph_entries()[4].clone();
            let at = aacute.len() - 8;
            aacute[at..at + 2].copy_from_slice(&(0x0003 | flag).to_be_bytes());
            aacute.extend(vec![0x40; size]);
            let mut entries = glyph_entries();
            entries[4] = aacute.clone();
            assert_eq!(
                subset_checks(&with_glyphs(&entries).build())[4],
                Ok(()),
                "{flag:#06x}"
            );
            aacute.pop();
            entries[4] = aacute;
            assert_eq!(
                subset_checks(&with_glyphs(&entries).build())[4],
                Err("glyph 4: its composite glyph description is cut off".to_owned()),
                "{flag:#06x}"
            );
        }
        // numberOfContours 0 is a simple glyph, here of its header alone.
        let mut entries = glyph_entries();
        entries[3] = vec![0; 10];
        assert_eq!(
            subset_checks(&with_glyphs(&entries).build()),
            vec![Ok(()); 5]
        );
    }

    /// A composite glyph description of `components`, each with byte arguments.
    fn composite_of(components: &[u16]) -> Vec<u8> {
        let mut entry = vec![0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0];
        for (i, component) in components.iter().enumerate() {
            let flags: u16 = if i + 1 == components.len() {
                0x0002
            } else {
                0x0022
            };
            entry.extend_from_slice(&flags.to_be_bytes());
            entry.extend_from_slice(&component.to_be_bytes());
            entry.extend_from_slice(&[0, 0]);
        }
        entry
    }

    /// A font whose `maxp` counts `count` glyphs, of the glyphs `entries`, which may be
    /// more: `loca` and `hmtx` cover them all.
    fn font_past(entries: &[Vec<u8>], count: usize) -> Vec<u8> {
        let base = font_of(&entries[..count], false);
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for entry in entries {
            loca.extend_from_slice(&u32::try_from(glyf.len()).unwrap().to_be_bytes());
            glyf.extend_from_slice(entry);
        }
        loca.extend_from_slice(&u32::try_from(glyf.len()).unwrap().to_be_bytes());
        let mut hmtx = table_of(&base.build(), b"hmtx");
        hmtx.resize(hmtx.len() + 2 * (entries.len() - count), 0);
        base.with_table(b"glyf", glyf)
            .with_table(b"loca", loca)
            .with_table(b"hmtx", hmtx)
            .build()
    }

    /// 65,536 glyph entries, all empty but `.notdef`'s.
    fn every_id() -> Vec<Vec<u8>> {
        let mut entries = vec![Vec::new(); 65536];
        entries[0] = glyph_entries()[0].clone();
        entries
    }

    const ALL_IDS: &str = "the glyphs its components use take all 65536 glyph IDs with .notdef, \
                           more than the subsetter can number";

    #[test]
    fn fails_a_glyph_whose_components_take_every_glyph_id() {
        // Glyph 1 is a composite of the empty glyphs 2 to `last`, past the glyph count.
        let font = |last: u16| {
            let mut entries = every_id();
            entries[1] = composite_of(&(2..=last).collect::<Vec<_>>());
            font_past(&entries, 2)
        };
        // With .notdef, 65,535 glyph IDs subset; 65,536 do not, and are refused, not left
        // to the subsetter's panic.
        assert_eq!(subset_checks(&font(65534)), [Ok(()), Ok(())]);
        let checks = subset_checks(&font(65535));
        assert_eq!(checks[1], Err(format!("glyph 1 and {ALL_IDS}")));
        let face = crate::FontFace::from_bytes(font(65535), 0).unwrap();
        assert!(matches!(
            face.subset([GlyphId::new(1)]),
            Err(Error::SubsetUnsupported { .. })
        ));
        assert!(matches!(
            face.check_subset_glyph(GlyphId::new(1)),
            Err(Error::SubsetUnsupported { .. })
        ));
        // `.notdef` a composite of glyphs 2 to 65535: alone it subsets, and with glyph 1,
        // a simple glyph, it takes every ID.
        let mut entries = every_id();
        entries[0] = composite_of(&(2..=65535).collect::<Vec<_>>());
        entries[1] = glyph_entries()[2].clone();
        let checks = subset_checks(&font_past(&entries, 2));
        assert_eq!(checks[1], Err(format!("glyph 1 and {ALL_IDS}")));
    }

    #[test]
    fn numbers_glyph_ids_as_the_subsetter_reads_components() {
        // Glyph 1 uses glyphs 3 to 65535 and then glyph 2, which the subsetter reaches
        // first. It numbers a glyph whose entry `loca` gives no range, and the components
        // it can read of an entry cut off: either way every ID, and `subset` refuses it
        // rather than leave it to the subsetter's panic.
        let mut components: Vec<u16> = (3..=65535).collect();
        components.push(2);
        let mut entries = every_id();
        entries[1] = composite_of(&components);
        let no_range = {
            let font = font_past(&entries, 2);
            let mut loca = table_of(&font, b"loca");
            loca[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
            rebuild_with(&font, b"loca", loca)
        };
        let mut cut = entries.clone();
        cut[1].pop();
        for font in [no_range, font_past(&cut, 2)] {
            let face = crate::FontFace::from_bytes(font, 0).unwrap();
            assert!(matches!(
                face.subset([GlyphId::new(1)]),
                Err(Error::SubsetUnsupported { .. })
            ));
        }
        // A glyph too short for a header, reached first, ends the subsetter's closure with
        // an error of its own before it numbers every ID.
        let mut short = entries;
        short[65535] = vec![0xFF];
        let face = crate::FontFace::from_bytes(font_past(&short, 2), 0).unwrap();
        assert!(matches!(
            face.subset([GlyphId::new(1)]),
            Err(Error::Subset { .. })
        ));
    }

    #[test]
    fn remembers_which_sound_glyphs_reach_past_the_glyph_count() {
        // Glyph 2 uses glyphs 3 to 65535, past the count: with .notdef 65,535 IDs, which
        // subset. Glyph 1 uses glyph 2: 65,536. Checked after glyph 2, its check does not
        // read glyph 2 again, and still numbers its closure.
        let mut entries = every_id();
        entries[1] = composite_of(&[2]);
        entries[2] = composite_of(&(3..=65535).collect::<Vec<_>>());
        let font = font_past(&entries, 3);
        let face = crate::FontFace::from_bytes(font.clone(), 0).unwrap();
        face.check_subset_glyph(GlyphId::new(2)).unwrap();
        assert!(matches!(
            face.check_subset_glyph(GlyphId::new(1)),
            Err(Error::SubsetUnsupported { .. })
        ));
        assert_eq!(
            subset_checks(&font)[1..],
            [Err(format!("glyph 1 and {ALL_IDS}")), Ok(())]
        );
    }

    #[test]
    fn numbers_a_closure_reading_each_glyph_once() {
        // Glyph 1 uses glyph 2 20,000 times, and glyph 2 uses glyph 3 as often. The
        // subsetter reads glyph 2 again each time it reaches it; that numbers nothing new,
        // and the numbering reads it once.
        let mut entries = vec![Vec::new(); 4];
        entries[0] = glyph_entries()[0].clone();
        entries[1] = composite_of(&[2; 20_000]);
        entries[2] = composite_of(&[3; 20_000]);
        let face = crate::FontFace::from_bytes(font_of(&entries, false).build(), 0).unwrap();
        let tables = Tables::read(&face).unwrap();
        let glyf = tables.glyf.unwrap();
        let mut components = 100_000;
        assert_eq!(
            closure_overflows(&glyf, face.data(), &[0, 1], Some(&mut components)),
            Some(false)
        );
        assert_eq!(components, 60_000);
    }

    /// The font `data` with its table `tag` replaced by `table`, of the same length.
    fn rebuild_with(data: &[u8], tag: &[u8; 4], table: Vec<u8>) -> Vec<u8> {
        let mut data = data.to_vec();
        let count = usize::from(u16::from_be_bytes([data[4], data[5]]));
        let at = (0..count)
            .map(|i| 12 + 16 * i)
            .find(|&at| &data[at..at + 4] == tag)
            .unwrap();
        let offset = u32::from_be_bytes(data[at + 8..at + 12].try_into().unwrap()) as usize;
        data[offset..offset + table.len()].copy_from_slice(&table);
        data
    }

    #[test]
    fn bounds_the_work_of_glyphs_whose_components_pass_the_glyph_count() {
        // Glyph 1 uses glyph 2,001, past the count; glyphs 2 to 1,999 each use the next.
        // Checking glyph 1 first, then the chain from its end, follows each component once
        // more than the chain has: only glyph 1's check numbers its closure.
        let mut entries = vec![Vec::new(); 2002];
        entries[0] = glyph_entries()[0].clone();
        entries[1] = composite_of(&[2001]);
        for glyph in 2..1999u16 {
            entries[usize::from(glyph)] = composite_of(&[glyph + 1]);
        }
        let face = crate::FontFace::from_bytes(font_past(&entries, 2001), 0).unwrap();
        let mut state = State::new(&face, 10_000);
        state.check(&face, 1).unwrap();
        for glyph in (2..2000).rev() {
            state.check(&face, glyph).unwrap();
        }
        assert!(
            10_000 - state.components < 2_010,
            "{}",
            10_000 - state.components
        );
        // The numbering counts against the components the checks may follow.
        let mut state = State::new(&face, 1);
        assert!(matches!(
            state.check(&face, 1),
            Err(Error::TableOverLimit { .. })
        ));
    }

    #[test]
    fn keeps_what_it_found_of_glyphs_in_each_word_of_bits() {
        // Glyph 40 is empty and glyph 104 damaged: finding 40 sound marks no other glyph.
        let mut entries = vec![Vec::new(); 110];
        entries[0] = glyph_entries()[0].clone();
        entries[104] = vec![0xFF];
        let face = crate::FontFace::from_bytes(font_of(&entries, false).build(), 0).unwrap();
        face.check_subset_glyph(GlyphId::new(40)).unwrap();
        assert!(face.check_subset_glyph(GlyphId::new(104)).is_err());
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
