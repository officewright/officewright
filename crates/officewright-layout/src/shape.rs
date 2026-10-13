//! Shaping: turning a run of text in one face into positioned glyphs, with HarfRust. No
//! HarfRust type leaves this module.
//!
//! A run is shaped in the face's design units, left to right; the layout scales the
//! result to the font size when it places it.

// reason: the layout itself shapes text from PR 6 (paragraph layout) on; until then only
// the tests and the `fuzzing` module call the shaper.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use harfrust::{
    Buffer, BufferFlags, Direction, Feature, Language, Script, ShapeOptions, ShapePlan, ShaperFont,
    Tag,
};
use officewright_core::Pt;
use officewright_font::{FontFace, GlyphId};

use crate::ir::{Ligatures, RunStyle};

/// A face prepared for shaping: the face, and HarfRust's view of the same bytes.
#[derive(Clone)]
pub(crate) struct ShapingFace {
    /// Tells faces apart in the shaping cache.
    id: u64,
    face: FontFace,
    /// `None` when HarfRust cannot read the face; its runs are shaped from the character
    /// map and the advances alone.
    font: Option<harfrust::Font>,
}

impl ShapingFace {
    /// The face.
    pub(crate) fn face(&self) -> &FontFace {
        &self.face
    }

    pub(crate) fn new(face: FontFace) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // The bytes are shared with the face, not copied.
        let bytes: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(FaceBytes(face.clone()));
        let font = harfrust::Font::new(harfrust::font::Blob::from(bytes), face.index());
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            face,
            font,
        }
    }
}

/// A face's bytes, for HarfRust.
struct FaceBytes(FontFace);

impl AsRef<[u8]> for FaceBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.data()
    }
}

/// The OpenType features a run asks for, from its formatting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Features {
    /// Kerning, when the run's size reaches its kerning threshold (`w:kern`).
    pub(crate) kerning: bool,
    /// The ligatures of `w14:ligatures`.
    pub(crate) ligatures: Ligatures,
    /// Contextual alternates (`w14:cntxtAlts`).
    pub(crate) contextual_alternates: bool,
}

impl Features {
    /// The features a run of `style` asks for at `size`: kerning from the size its
    /// `w:kern` gives, the ligatures and contextual alternates it names.
    pub(crate) fn of(style: &RunStyle, size: Pt) -> Self {
        Self {
            kerning: style.kern_min().is_some_and(|min| size.0 >= min.0),
            ligatures: style.ligatures(),
            contextual_alternates: style.contextual_alternates(),
        }
    }

    /// The features as HarfRust takes them for a run in `script`. The kinds of ligatures
    /// of `w14:ligatures` are the OpenType features of the same names ([MS-DOCX]
    /// ST_Ligatures; ISO/IEC 14496-22): standard `liga`, contextual `clig`, historical
    /// `hlig`, discretionary `dlig`; contextual alternates (`calt`) are a setting of their
    /// own. The features a script needs (`ccmp`, `locl`, `mark`, `rlig` and the like)
    /// keep HarfRust's defaults.
    ///
    /// Word turns kerning, standard and contextual ligatures and contextual alternates off
    /// unless the run asks for them only in the scripts its standard shaping covers
    /// (Latin, Greek, Cyrillic, and characters common to scripts). Other scripts need
    /// some of them: Arabic fonts keep letter connections in `calt`, and Word kerns
    /// Arabic whatever the run says, so there those features keep HarfRust's defaults.
    /// The historical and discretionary ligatures follow the run in every script.
    fn opentype(self, script: Option<Script>) -> Vec<Feature> {
        let feature = |tag: &[u8; 4], on: bool| Feature::new(Tag::new(tag), u32::from(on), ..);
        let mut features = vec![
            feature(b"hlig", self.ligatures.historical()),
            feature(b"dlig", self.ligatures.discretional()),
        ];
        let standard = script.is_none_or(|script| STANDARD_SCRIPTS.contains(&script));
        if standard {
            features.extend([
                feature(b"kern", self.kerning),
                feature(b"liga", self.ligatures.standard()),
                feature(b"clig", self.ligatures.contextual()),
                feature(b"calt", self.contextual_alternates),
            ]);
        }
        features
    }
}

/// The scripts in which kerning and the optional ligatures follow the run: those that
/// Microsoft's "standard" shaping covers (Latin, Greek, Cyrillic, Armenian, Georgian,
/// Runic and Ogham, "Developing OpenType Fonts for Standard Scripts"), and characters
/// common to scripts.
const STANDARD_SCRIPTS: [Script; 10] = [
    Script::LATIN,
    Script::GREEK,
    Script::CYRILLIC,
    Script::ARMENIAN,
    Script::GEORGIAN,
    Script::RUNIC,
    Script::OGHAM,
    Script::COMMON,
    Script::INHERITED,
    Script::UNKNOWN,
];

/// What a run of text is shaped as, apart from its face and text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct ShapeRequest {
    pub(crate) features: Features,
    /// The script, as an ISO 15924 code such as `Latn`; `None` to take it from the text.
    pub(crate) script: Option<[u8; 4]>,
    /// The language, as a BCP 47 tag.
    pub(crate) language: Option<String>,
}

/// A run of text shaped in one face, in the face's design units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShapedRun {
    glyphs: Vec<ShapedGlyph>,
    /// Whether the run was shaped from the character map and advances alone, because
    /// HarfRust could not read the face.
    nominal: bool,
    /// How many glyphs the face's layout tables named that the face does not have; each
    /// was replaced by the missing-glyph shape.
    replaced: usize,
}

impl ShapedRun {
    /// The glyphs, left to right.
    pub(crate) fn glyphs(&self) -> &[ShapedGlyph] {
        &self.glyphs
    }

    pub(crate) fn is_nominal(&self) -> bool {
        self.nominal
    }

    pub(crate) fn replaced_glyphs(&self) -> usize {
        self.replaced
    }
}

/// A glyph of a shaped run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShapedGlyph {
    pub(crate) glyph: GlyphId,
    /// The byte offset in the run's text of the first character the glyph stands for.
    pub(crate) cluster: usize,
    pub(crate) x_advance: i32,
    pub(crate) x_offset: i32,
    pub(crate) y_offset: i32,
    /// Whether breaking the text at this glyph's cluster could shape the two sides
    /// differently, so that they must be shaped again.
    pub(crate) unsafe_to_break: bool,
}

/// Shapes runs and keeps the results, so that text that repeats (headers, list labels,
/// table headings) is shaped once per layout run.
pub(crate) struct Shaper {
    cache: HashMap<CacheKey, Arc<ShapedRun>>,
    /// HarfRust's plans: which lookups of a face a script, language and features use.
    /// Making one reads the face's layout tables, so each is made once.
    plans: Plans,
    /// About how many bytes the cache holds, by [`cost`].
    cached_bytes: usize,
    /// About how many bytes the cache holds at most; when a run would pass it, the cache
    /// starts empty again.
    capacity: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    face: u64,
    request: ShapeRequest,
    text: String,
}

impl Shaper {
    /// How many bytes the cache holds at most by default.
    pub(crate) const DEFAULT_CAPACITY: usize = 64 << 20;

    pub(crate) fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            cache: HashMap::new(),
            plans: Plans::default(),
            cached_bytes: 0,
            capacity,
        }
    }

    /// Shapes `text` in `face`, left to right.
    pub(crate) fn shape(
        &mut self,
        face: &ShapingFace,
        text: &str,
        request: &ShapeRequest,
    ) -> Arc<ShapedRun> {
        let key = CacheKey {
            face: face.id,
            request: request.clone(),
            text: text.to_owned(),
        };
        if let Some(run) = self.cache.get(&key) {
            return Arc::clone(run);
        }
        let run = Arc::new(shape(face, text, request, &mut self.plans));
        let bytes = cost(&key, &run);
        if self.cached_bytes.saturating_add(bytes) > self.capacity {
            self.cache.clear();
            self.cached_bytes = 0;
        }
        if bytes <= self.capacity {
            self.cached_bytes += bytes;
            self.cache.insert(key, Arc::clone(&run));
        }
        run
    }
}

/// About how many bytes a cache entry takes: its glyphs, the text and the language it
/// keeps, and the key, the run and the map slot themselves.
fn cost(key: &CacheKey, run: &ShapedRun) -> usize {
    const ENTRY: usize = std::mem::size_of::<CacheKey>()
        + std::mem::size_of::<ShapedRun>()
        + std::mem::size_of::<Arc<ShapedRun>>()
        + 2 * std::mem::size_of::<usize>();
    let language = key.request.language.as_ref().map_or(0, String::len);
    run.glyphs
        .len()
        .saturating_mul(std::mem::size_of::<ShapedGlyph>())
        .saturating_add(key.text.len())
        .saturating_add(language)
        .saturating_add(ENTRY)
}

/// Shaping plans by face, script, language and features.
#[derive(Default)]
struct Plans {
    plans: HashMap<PlanKey, Arc<ShapePlan>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PlanKey {
    face: u64,
    script: Option<Script>,
    language: Option<String>,
    features: Features,
}

impl Plans {
    /// The most plans kept; a layout run has a few per face, so this is reached only by
    /// documents with very many faces or languages, which then start afresh.
    const CAPACITY: usize = 1024;

    fn plan(
        &mut self,
        key: PlanKey,
        font: &harfrust::Font,
        language: Option<&Language>,
        features: &[Feature],
    ) -> Arc<ShapePlan> {
        if let Some(plan) = self.plans.get(&key) {
            return Arc::clone(plan);
        }
        if self.plans.len() >= Self::CAPACITY {
            self.plans.clear();
        }
        let plan = Arc::new(ShapePlan::new(
            font,
            Direction::LeftToRight,
            key.script,
            language,
            features,
        ));
        self.plans.insert(key, Arc::clone(&plan));
        plan
    }
}

/// Shapes without the cache of runs.
fn shape(face: &ShapingFace, text: &str, request: &ShapeRequest, plans: &mut Plans) -> ShapedRun {
    let Some(font) = &face.font else {
        return shape_nominal(&face.face, text);
    };
    let mut buffer = Buffer::new();
    buffer.push_str(text);
    buffer.set_direction(Direction::LeftToRight);
    // A run is shaped with no context: the layout splits text into runs where the face,
    // script or formatting changes, and shapes each one whole.
    buffer.set_flags(BufferFlags::BEGINNING_OF_TEXT | BufferFlags::END_OF_TEXT);
    // A code that is not a script would keep the script from being guessed from the text.
    let script = request
        .script
        .and_then(|code| Script::from_iso15924_tag(Tag::new(&code)))
        .filter(|script| *script != Script::UNKNOWN);
    let language = request
        .language
        .as_deref()
        .and_then(|tag| Language::from_str(tag).ok());
    buffer.set_script(script);
    buffer.set_language(language.clone());
    // Fills in the script from the text when the request does not give one.
    buffer.guess_segment_properties();
    let features = request.features.opentype(buffer.script());
    let shaper = ShaperFont::new(font);
    let key = PlanKey {
        face: face.id,
        script: buffer.script(),
        language: request.language.clone(),
        features: request.features,
    };
    let plan = plans.plan(key, font, language.as_ref(), &features);
    let options = ShapeOptions::new().features(&features).plan(Some(&plan));
    if harfrust::shape(&shaper, &mut buffer, options).is_err() {
        // HarfRust refuses a buffer only for reasons this code rules out (no direction,
        // a plan for another script); shaping nominally keeps the text all the same.
        return shape_nominal(&face.face, text);
    }
    if !buffer.allocation_successful() {
        // The font made HarfRust run out of room: more than 256 glyphs per character or
        // more than 4096 lookup steps per character (at least 65536 of each). The buffer
        // then holds a half-substituted state, so the run is shaped nominally.
        return shape_nominal(&face.face, text);
    }
    let raw = buffer
        .glyph_infos()
        .iter()
        .zip(buffer.glyph_positions())
        .map(|(info, position)| RawGlyph {
            id: info.glyph_id,
            cluster: info.cluster,
            x_advance: position.x_advance,
            x_offset: position.x_offset,
            y_offset: position.y_offset,
            unsafe_to_break: info.unsafe_to_break(),
        });
    let (glyphs, replaced) = place(raw, face.face.glyph_count(), text.len());
    ShapedRun {
        glyphs,
        nominal: false,
        replaced,
    }
}

/// A glyph as HarfRust returns it, before it is checked against the face.
struct RawGlyph {
    id: u32,
    cluster: u32,
    x_advance: i32,
    x_offset: i32,
    y_offset: i32,
    unsafe_to_break: bool,
}

/// Turns HarfRust's glyphs into the run's: a glyph id the face does not have becomes the
/// missing-glyph shape and is counted, and a cluster stays within the text.
fn place(
    raw: impl Iterator<Item = RawGlyph>,
    glyph_count: u16,
    text_len: usize,
) -> (Vec<ShapedGlyph>, usize) {
    let mut replaced = 0;
    let glyphs = raw
        .map(|raw| {
            let glyph = u16::try_from(raw.id)
                .ok()
                .filter(|id| *id < glyph_count)
                .map_or_else(
                    || {
                        replaced += 1;
                        GlyphId::NOTDEF
                    },
                    GlyphId::new,
                );
            ShapedGlyph {
                glyph,
                cluster: usize::try_from(raw.cluster)
                    .unwrap_or(text_len)
                    .min(text_len),
                x_advance: raw.x_advance,
                x_offset: raw.x_offset,
                y_offset: raw.y_offset,
                unsafe_to_break: raw.unsafe_to_break,
            }
        })
        .collect();
    (glyphs, replaced)
}

/// Shapes from the character map and the advances alone: one glyph per character, the
/// missing-glyph shape for a character the face does not map.
fn shape_nominal(face: &FontFace, text: &str) -> ShapedRun {
    let map = face.char_map();
    let glyphs = text
        .char_indices()
        .map(|(cluster, ch)| {
            let glyph = map.map(ch).unwrap_or(GlyphId::NOTDEF);
            ShapedGlyph {
                glyph,
                cluster,
                x_advance: face.advance_width(glyph).map_or(0, i32::from),
                x_offset: 0,
                y_offset: 0,
                unsafe_to_break: false,
            }
        })
        .collect();
    ShapedRun {
        glyphs,
        nominal: true,
        replaced: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The font crate's own test font: glyphs 0 `.notdef`, 1 space, 2 `A`, 3 acute and
    /// 4 `Á`, advances 500, 250, 600, 300 and 600.
    fn minimal() -> FontFace {
        let data = include_bytes!("../../officewright-font/tests/data/minimal.ttf");
        FontFace::from_bytes(data.to_vec(), 0).expect("the test font")
    }

    fn ids(run: &ShapedRun) -> Vec<(u16, usize, i32)> {
        run.glyphs()
            .iter()
            .map(|glyph| (glyph.glyph.to_u16(), glyph.cluster, glyph.x_advance))
            .collect()
    }

    #[test]
    fn a_face_harfrust_cannot_read_is_shaped_from_its_character_map() {
        let face = minimal();
        let run = shape_nominal(&face, "A \u{C1}B");
        assert!(run.is_nominal());
        // B is not in the font: the missing-glyph shape, with its advance.
        assert_eq!(
            ids(&run),
            [(2, 0, 600), (1, 1, 250), (4, 2, 600), (0, 4, 500)]
        );

        let without_layout = ShapingFace {
            id: 0,
            face: face.clone(),
            font: None,
        };
        let shaped = shape(
            &without_layout,
            "A",
            &ShapeRequest::default(),
            &mut Plans::default(),
        );
        assert!(shaped.is_nominal());
        let with_layout = ShapingFace::new(face);
        let shaped = shape(
            &with_layout,
            "A \u{C1}",
            &ShapeRequest::default(),
            &mut Plans::default(),
        );
        assert!(!shaped.is_nominal());
        assert_eq!(ids(&shaped), [(2, 0, 600), (1, 1, 250), (4, 2, 600)]);
        assert_eq!(shaped.replaced_glyphs(), 0);
    }

    #[test]
    fn repeated_text_is_shaped_once() {
        let face = ShapingFace::new(minimal());
        let mut shaper = Shaper::new();
        let request = ShapeRequest::default();
        let first = shaper.shape(&face, "AA", &request);
        assert!(Arc::ptr_eq(&first, &shaper.shape(&face, "AA", &request)));
        // Other features, another face or other text are shaped anew.
        let kerned = ShapeRequest {
            features: Features {
                kerning: true,
                ..Features::default()
            },
            ..ShapeRequest::default()
        };
        assert!(!Arc::ptr_eq(&first, &shaper.shape(&face, "AA", &kerned)));
        let language = ShapeRequest {
            language: Some("ro".into()),
            ..ShapeRequest::default()
        };
        assert!(!Arc::ptr_eq(&first, &shaper.shape(&face, "AA", &language)));
        let other = ShapingFace::new(minimal());
        assert!(!Arc::ptr_eq(&first, &shaper.shape(&other, "AA", &request)));
    }

    #[test]
    fn the_cache_holds_at_most_its_capacity() {
        let face = ShapingFace::new(minimal());
        let request = ShapeRequest::default();
        let entry = |text: &str| {
            let key = CacheKey {
                face: face.id,
                request: request.clone(),
                text: text.to_owned(),
            };
            cost(&key, &shape(&face, text, &request, &mut Plans::default()))
        };
        // Room for "AA" and "AAA" exactly.
        let mut shaper = Shaper::with_capacity(entry("AA") + entry("AAA"));
        let two = shaper.shape(&face, "AA", &request);
        let three = shaper.shape(&face, "AAA", &request);
        assert_eq!(shaper.cached_bytes, shaper.capacity);
        assert!(Arc::ptr_eq(&two, &shaper.shape(&face, "AA", &request)));
        assert!(Arc::ptr_eq(&three, &shaper.shape(&face, "AAA", &request)));
        // One more entry does not fit: the cache starts again with it.
        let one = shaper.shape(&face, "A", &request);
        assert_eq!(shaper.cached_bytes, entry("A"));
        assert!(Arc::ptr_eq(&one, &shaper.shape(&face, "A", &request)));
        assert!(!Arc::ptr_eq(&two, &shaper.shape(&face, "AA", &request)));
        // A run larger than the whole cache is not kept.
        let mut small = Shaper::with_capacity(entry("A"));
        let long = small.shape(&face, "AAAA", &request);
        assert_eq!(small.cached_bytes, 0);
        assert!(!Arc::ptr_eq(&long, &small.shape(&face, "AAAA", &request)));
    }

    #[test]
    fn an_entry_costs_its_glyphs_its_text_and_its_language() {
        let face = ShapingFace::new(minimal());
        let entry = |text: &str, language: Option<&str>| {
            let request = ShapeRequest {
                language: language.map(str::to_owned),
                ..ShapeRequest::default()
            };
            let key = CacheKey {
                face: face.id,
                request: request.clone(),
                text: text.to_owned(),
            };
            cost(&key, &shape(&face, text, &request, &mut Plans::default()))
        };
        let glyph = std::mem::size_of::<ShapedGlyph>();
        // Two more characters: two more glyphs and two more bytes of text.
        assert_eq!(entry("AAAA", None) - entry("AA", None), 2 * glyph + 2);
        // A language adds its tag.
        assert_eq!(entry("AA", Some("ro")) - entry("AA", None), 2);
        // An empty run costs the entry itself: key, run, `Arc` and map slot.
        let fixed = std::mem::size_of::<CacheKey>()
            + std::mem::size_of::<ShapedRun>()
            + std::mem::size_of::<Arc<ShapedRun>>()
            + 2 * std::mem::size_of::<usize>();
        assert_eq!(entry("", None), fixed);
    }

    /// The features a request turns into, by tag.
    fn features(features: Features, script: Option<Script>) -> Vec<(String, u32)> {
        let mut list: Vec<(String, u32)> = features
            .opentype(script)
            .iter()
            .map(|feature| (feature.tag.to_string(), feature.value))
            .collect();
        list.sort();
        list
    }

    #[test]
    fn each_kind_of_ligature_is_its_own_opentype_feature() {
        let off = Features::default();
        let pairs = |kern, liga, clig, calt, hlig, dlig| {
            let mut list: Vec<(String, u32)> = [
                ("calt", calt),
                ("clig", clig),
                ("dlig", dlig),
                ("hlig", hlig),
                ("kern", kern),
                ("liga", liga),
            ]
            .iter()
            .map(|(tag, value)| ((*tag).to_owned(), *value))
            .collect();
            list.sort();
            list
        };
        assert_eq!(features(off, None), pairs(0, 0, 0, 0, 0, 0));
        let on = |change: fn(Ligatures) -> Ligatures| Features {
            ligatures: change(Ligatures::default()),
            ..off
        };
        assert_eq!(
            features(on(|l| l.with_standard(true)), None),
            pairs(0, 1, 0, 0, 0, 0)
        );
        assert_eq!(
            features(on(|l| l.with_contextual(true)), None),
            pairs(0, 0, 1, 0, 0, 0)
        );
        assert_eq!(
            features(on(|l| l.with_historical(true)), None),
            pairs(0, 0, 0, 0, 1, 0)
        );
        assert_eq!(
            features(on(|l| l.with_discretional(true)), None),
            pairs(0, 0, 0, 0, 0, 1)
        );
        let kerned = Features {
            kerning: true,
            ..off
        };
        assert_eq!(
            features(kerned, Some(Script::LATIN)),
            pairs(1, 0, 0, 0, 0, 0)
        );
        let alternates = Features {
            contextual_alternates: true,
            ..off
        };
        assert_eq!(
            features(alternates, Some(Script::CYRILLIC)),
            pairs(0, 0, 0, 1, 0, 0)
        );
    }

    #[test]
    fn scripts_beyond_the_standard_ones_keep_kerning_and_their_ligatures() {
        let features = features(Features::default(), Some(Script::ARABIC));
        assert_eq!(
            features,
            [("dlig".to_owned(), 0), ("hlig".to_owned(), 0)],
            "kerning, liga, clig and calt are left to the shaper"
        );
    }

    #[test]
    fn glyphs_beyond_the_face_become_the_missing_glyph() {
        let raw = |id, cluster| RawGlyph {
            id,
            cluster,
            x_advance: 10,
            x_offset: 1,
            y_offset: 2,
            unsafe_to_break: true,
        };
        let (glyphs, replaced) = place(
            [raw(4, 0), raw(5, 1), raw(70_000, 2), raw(1, 100)].into_iter(),
            5,
            10,
        );
        let ids: Vec<(u16, usize)> = glyphs
            .iter()
            .map(|glyph| (glyph.glyph.to_u16(), glyph.cluster))
            .collect();
        // 5 is beyond a face of five glyphs, 70000 beyond any; a cluster stays in the text.
        assert_eq!(ids, [(4, 0), (0, 1), (0, 2), (1, 10)]);
        assert_eq!(replaced, 2);
        assert!(glyphs.iter().all(|glyph| glyph.x_advance == 10
            && (glyph.x_offset, glyph.y_offset) == (1, 2)
            && glyph.unsafe_to_break));
    }
}
