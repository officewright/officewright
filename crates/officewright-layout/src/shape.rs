//! Shaping: turning a run of text in one face into positioned glyphs, with HarfRust. No
//! HarfRust type leaves this module.
//!
//! A run is shaped in the face's design units, left to right; the layout scales the
//! result to the font size when it places it.

// reason: the layout shapes text from PR 5a on; until then only the tests and the
// `fuzzing` module call the shaper.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use harfrust::{Buffer, BufferFlags, Direction, Feature, Language, ShapeOptions, ShaperFont, Tag};
use officewright_font::{FontFace, GlyphId};

use crate::ir::Ligatures;

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
}

impl Features {
    /// The features as HarfRust takes them. Word applies kerning and the optional
    /// ligatures only when the run asks for them, so each is set on or off explicitly;
    /// the features a script needs (`ccmp`, `locl`, `mark`, `rlig` and the like) keep
    /// HarfRust's defaults.
    fn opentype(self) -> [Feature; 6] {
        let feature = |tag: &[u8; 4], on: bool| Feature::new(Tag::new(tag), u32::from(on), ..);
        [
            feature(b"kern", self.kerning),
            feature(b"liga", self.ligatures.standard()),
            feature(b"clig", self.ligatures.standard()),
            feature(b"calt", self.ligatures.contextual()),
            feature(b"hlig", self.ligatures.historical()),
            feature(b"dlig", self.ligatures.discretional()),
        ]
    }
}

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
    /// The glyphs held in the cache.
    cached_glyphs: usize,
    /// The most glyphs the cache holds; when a run would pass it, the cache starts empty
    /// again.
    capacity: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    face: u64,
    request: ShapeRequest,
    text: String,
}

impl Shaper {
    /// The most glyphs the cache holds by default: tens of megabytes at most.
    pub(crate) const DEFAULT_CAPACITY: usize = 1 << 20;

    pub(crate) fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            cache: HashMap::new(),
            cached_glyphs: 0,
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
        let run = Arc::new(shape(face, text, request));
        let glyphs = run.glyphs.len();
        if self.cached_glyphs.saturating_add(glyphs) > self.capacity {
            self.cache.clear();
            self.cached_glyphs = 0;
        }
        if glyphs <= self.capacity {
            self.cached_glyphs += glyphs;
            self.cache.insert(key, Arc::clone(&run));
        }
        run
    }
}

/// Shapes without the cache.
fn shape(face: &ShapingFace, text: &str, request: &ShapeRequest) -> ShapedRun {
    let Some(font) = &face.font else {
        return shape_nominal(&face.face, text);
    };
    let mut buffer = Buffer::new();
    buffer.push_str(text);
    buffer.set_direction(Direction::LeftToRight);
    // A run is shaped with no context: the layout splits text into runs where the face,
    // script or formatting changes, and shapes each one whole.
    buffer.set_flags(BufferFlags::BEGINNING_OF_TEXT | BufferFlags::END_OF_TEXT);
    let script = request
        .script
        .and_then(|code| harfrust::Script::from_iso15924_tag(Tag::new(&code)));
    let language = request
        .language
        .as_deref()
        .and_then(|tag| Language::from_str(tag).ok());
    buffer.set_script(script);
    buffer.set_language(language);
    // Fills in the script from the text when the request does not give one.
    buffer.guess_segment_properties();
    let features = request.features.opentype();
    let shaper = ShaperFont::new(font);
    if harfrust::shape(
        &shaper,
        &mut buffer,
        ShapeOptions::new().features(&features),
    )
    .is_err()
    {
        // HarfRust refuses a buffer only for reasons this code rules out (no direction,
        // a plan for another script); shaping nominally keeps the text all the same.
        return shape_nominal(&face.face, text);
    }

    let glyph_count = face.face.glyph_count();
    let mut replaced = 0;
    let glyphs = buffer
        .glyph_infos()
        .iter()
        .zip(buffer.glyph_positions())
        .map(|(info, position)| {
            let glyph = u16::try_from(info.glyph_id)
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
                cluster: usize::try_from(info.cluster)
                    .unwrap_or(text.len())
                    .min(text.len()),
                x_advance: position.x_advance,
                x_offset: position.x_offset,
                y_offset: position.y_offset,
                unsafe_to_break: info.unsafe_to_break(),
            }
        })
        .collect();
    ShapedRun {
        glyphs,
        nominal: false,
        replaced,
    }
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
        let shaped = shape(&without_layout, "A", &ShapeRequest::default());
        assert!(shaped.is_nominal());
        let with_layout = ShapingFace::new(face);
        let shaped = shape(&with_layout, "A \u{C1}", &ShapeRequest::default());
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
        let other = ShapingFace::new(minimal());
        assert!(!Arc::ptr_eq(&first, &shaper.shape(&other, "AA", &request)));
    }

    #[test]
    fn the_cache_holds_at_most_its_capacity_in_glyphs() {
        let face = ShapingFace::new(minimal());
        let request = ShapeRequest::default();
        let mut shaper = Shaper::with_capacity(5);
        let two = shaper.shape(&face, "AA", &request);
        assert_eq!(shaper.cached_glyphs, 2);
        // Four more glyphs would make six: the cache starts again with them.
        let four = shaper.shape(&face, "AAAA", &request);
        assert_eq!(shaper.cached_glyphs, 4);
        assert!(Arc::ptr_eq(&four, &shaper.shape(&face, "AAAA", &request)));
        let two_again = shaper.shape(&face, "AA", &request);
        assert!(!Arc::ptr_eq(&two, &two_again));
        assert_eq!(shaper.cached_glyphs, 2);
        // A run longer than the whole cache is not kept.
        let six = shaper.shape(&face, "AAAAAA", &request);
        assert_eq!(shaper.cached_glyphs, 0);
        assert!(!Arc::ptr_eq(&six, &shaper.shape(&face, "AAAAAA", &request)));
    }
}
