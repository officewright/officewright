//! The fonts of a layout run: which face each family and style resolves to, the faces
//! loaded once each, and the faces that have a character the chosen one lacks.

// reason: the layout itself uses this from PR 6 (paragraph layout) on; until then only
// the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use officewright_font::{FaceId, FontBook, FontRequest, FontResolver, ResolutionKind};
use unicode_script::UnicodeScript;

use crate::report::{LayoutReport, Location, ReportKind};
use crate::shape::ShapingFace;

/// The face a family and style resolve to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Choice {
    /// `None` when the book has no font at all.
    pub(crate) face: Option<FaceId>,
    /// The face is not bold, so bold is drawn by stroking the outlines.
    pub(crate) synthetic_bold: bool,
    /// The face is not italic, so italic is drawn by slanting the glyphs.
    pub(crate) synthetic_italic: bool,
    /// The face is the family itself (or the font Windows takes for its name), not a
    /// substitute or a fallback.
    pub(crate) installed: bool,
}

/// The fonts of a layout run.
pub(crate) struct Fonts<'a> {
    resolver: FontResolver<'a>,
    /// Every face loaded so far; `None` for one whose file could not be read.
    faces: HashMap<FaceId, Option<Arc<ShapingFace>>>,
    /// The faces that have a character, best first, by character, style and the first
    /// word of the requested family.
    covering: HashMap<(char, bool, bool, String), Vec<FaceId>>,
    /// The families and the fallbacks reported for them.
    fallbacks: HashSet<(String, String)>,
    /// The characters no font has, by script.
    missing: BTreeMap<&'static str, BTreeSet<char>>,
    /// What is reported once per layout run.
    reported: HashSet<String>,
    report: LayoutReport,
}

impl<'a> Fonts<'a> {
    pub(crate) fn new(book: &'a FontBook) -> Self {
        Self {
            resolver: FontResolver::new(book),
            faces: HashMap::new(),
            covering: HashMap::new(),
            missing: BTreeMap::new(),
            fallbacks: HashSet::new(),
            reported: HashSet::new(),
            report: LayoutReport::default(),
        }
    }

    /// The face for `family` in bold and italic, as the resolver chooses it.
    pub(crate) fn choose(&mut self, family: &str, bold: bool, italic: bool) -> Choice {
        let request = FontRequest::new(family).bold(bold).italic(italic);
        let resolution = self.resolver.resolve(&request);
        Choice {
            face: resolution.face(),
            synthetic_bold: resolution.synthetic_bold(),
            synthetic_italic: resolution.synthetic_italic(),
            // The family itself, or the font Windows takes for its name; a synthesized
            // style does not make it another font.
            installed: matches!(
                resolution.kind(),
                ResolutionKind::Exact | ResolutionKind::Alias
            ),
        }
    }

    /// A loaded face, ready to shape with.
    pub(crate) fn face(&mut self, id: FaceId) -> Option<Arc<ShapingFace>> {
        let book = self.resolver.book();
        let face = self.faces.entry(id).or_insert_with(|| {
            book.load(id)
                .ok()
                .map(|face| Arc::new(ShapingFace::new(face)))
        });
        if face.is_none() {
            let name = book.face(id).and_then(|face| face.path()).map_or_else(
                || "a font in memory".to_owned(),
                |path| path.display().to_string(),
            );
            let message = format!("cannot read {name}; its text is drawn in other fonts");
            if self.reported.insert(message.clone()) {
                self.report
                    .push(ReportKind::FontSubstituted, Location::default(), message);
            }
        }
        face.clone()
    }

    /// Whether face `id` has a glyph for `ch`; a symbol font has one for U+0020 to
    /// U+00FF where it has one for U+F020 to U+F0FF, as Windows finds them.
    pub(crate) fn has_glyph(&mut self, id: FaceId, ch: char) -> bool {
        self.face(id)
            .is_some_and(|face| face.face().glyph_id(ch).is_some())
    }

    /// A face that has `ch`, for text that asked for `family` in bold and italic: first
    /// the faces of the same style (bold or not, italic or not), then the others; within
    /// each, those whose family shares the requested family's first word ("Noto Sans
    /// CJK" for "Noto Sans") before the rest; then the book's order. Bold or italic that
    /// the face lacks is drawn synthetically.
    pub(crate) fn covering(
        &mut self,
        ch: char,
        family: &str,
        bold: bool,
        italic: bool,
    ) -> Option<Choice> {
        let book = self.resolver.book();
        let first_word = family
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_lowercase();
        let key = (ch, bold, italic, first_word.clone());
        let candidates = self.covering.entry(key).or_insert_with(|| {
            let mut faces = book.faces_covering(ch);
            faces.sort_by_key(|&id| {
                book.face(id).map_or((true, true), |face| {
                    let style = (face.weight() >= 600) == bold && face.is_italic() == italic;
                    let same_family = !first_word.is_empty()
                        && face
                            .names()
                            .family()
                            .and_then(|name| name.split_whitespace().next())
                            .is_some_and(|word| word.to_lowercase() == first_word);
                    (!style, !same_family)
                })
            });
            faces
        });
        let candidates = candidates.clone();
        let id = candidates.into_iter().find(|&id| self.has_glyph(id, ch))?;
        let book = self.resolver.book();
        let (weight, is_italic) = book
            .face(id)
            .map_or((400, false), |face| (face.weight(), face.is_italic()));
        Some(Choice {
            face: Some(id),
            synthetic_bold: bold && weight < 600,
            synthetic_italic: italic && !is_italic,
            installed: false,
        })
    }

    /// Reports that characters of text in `family` are drawn in the face `fallback`, once
    /// per family and fallback, naming the first such character.
    pub(crate) fn report_fallback(
        &mut self,
        family: &str,
        fallback: FaceId,
        ch: char,
        location: &Location,
    ) {
        let name = self
            .resolver
            .book()
            .face(fallback)
            .and_then(|face| face.names().family())
            .unwrap_or("another font")
            .to_owned();
        if self.fallbacks.insert((family.to_lowercase(), name.clone())) {
            self.report.push(
                ReportKind::FontSubstituted,
                location.clone(),
                format!(
                    "\"{family}\" has no glyph for some characters, such as U+{:04X} '{}'; they \
                     are drawn in \"{name}\"",
                    u32::from(ch),
                    ch.escape_debug()
                ),
            );
        }
    }

    /// Counts `ch` as drawn with the missing-glyph shape because no font has it; the
    /// report gives one entry per script.
    pub(crate) fn missing(&mut self, ch: char) {
        let script = ch.script().full_name();
        self.missing.entry(script).or_default().insert(ch);
    }

    /// Adds `message` to the report under `kind` once per layout run.
    pub(crate) fn report_once(&mut self, kind: ReportKind, location: &Location, message: String) {
        if self.reported.insert(message.clone()) {
            self.report.push(kind, location.clone(), message);
        }
    }

    /// The report: what happened while choosing fonts, then every substitution the
    /// resolver made.
    pub(crate) fn into_report(self) -> LayoutReport {
        let mut report = self.report;
        for (script, chars) in &self.missing {
            let Some(&first) = chars.first() else {
                continue;
            };
            let count = chars.len();
            let characters = if count == 1 {
                "character"
            } else {
                "characters"
            };
            report.push(
                ReportKind::GlyphMissing,
                Location::default(),
                format!(
                    "no font has {count} {characters} of the {script} script, such as U+{:04X} \
                     '{}'; the missing-glyph shape is drawn",
                    u32::from(first),
                    first.escape_debug()
                ),
            );
        }
        for resolution in self.resolver.substitutions() {
            report.push(
                ReportKind::FontSubstituted,
                Location::default(),
                resolution.to_string(),
            );
        }
        report
    }
}
