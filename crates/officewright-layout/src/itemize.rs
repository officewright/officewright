//! Itemizing a paragraph: its text in one string, cut into runs that each have one
//! formatting, one face and one script, so that each run can be shaped whole; and the
//! inlines that are not text, in order.

// reason: the layout itself uses this from PR 6 (paragraph layout) on; until then only
// the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::ops::Range;
use std::sync::Arc;

use officewright_core::Pt;
use officewright_font::FaceId;
use unicode_script::{Script, UnicodeScript};

use crate::fonts::{Choice, Fonts};
use crate::ir::{BreakKind, Document, Inline, RunFonts, RunStyle, RunStyleId};
use crate::report::{Location, ReportKind};
use crate::shape::{Features, ShapeRequest, ShapedRun, Shaper, combining_class};
use crate::slot::{Slot, slot};
use crate::symbol::SymbolFont;

/// The size of small capitals, as a fraction of the run's size. Hypothesis until a Word
/// reference shows it (`E-smallcaps.docx`).
pub(crate) const SMALL_CAPS_SCALE: f64 = 0.8;

/// The most bytes of text a run has: a longer one is cut where a cluster starts, after a
/// space where there is one, so that shaping time and memory stay bounded per run.
pub(crate) const MAX_RUN_BYTES: usize = 4096;

/// A paragraph's content, itemized.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Itemized {
    /// The text as it is drawn: capitals where the formatting asks for them, symbol
    /// codes as the characters they stand for.
    pub(crate) text: String,
    /// The text as the document has it, which the PDF gives for copying and searching.
    pub(crate) source: String,
    pub(crate) items: Vec<Item>,
}

/// A piece of a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Item {
    Text(TextRun),
    Tab(RunStyleId),
    Break(BreakKind, RunStyleId),
    /// An inline object, by its path.
    Object(InlinePath),
    /// An anchored object, by its path; it takes no room in the line.
    Anchored(InlinePath),
    /// A note reference, by its path.
    NoteReference(InlinePath, RunStyleId),
    /// A note's own number or a separator, which the notes layout fills in.
    NoteMark(RunStyleId),
}

/// Where an inline is: its position among the paragraph's inlines, then, for an inline in
/// a field's result, its position there, and so on into fields within fields.
pub(crate) type InlinePath = Vec<usize>;

/// A run of text in one formatting, face and script.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextRun {
    /// Its bytes of [`Itemized::text`].
    pub(crate) range: Range<usize>,
    /// Its bytes of [`Itemized::source`].
    pub(crate) source: Range<usize>,
    pub(crate) style: RunStyleId,
    /// The face; `None` when there is no font at all, and nothing can be drawn.
    pub(crate) face: Option<FaceId>,
    pub(crate) size: Pt,
    /// The size the formatting gives, before small capitals reduce it: what kerning
    /// compares with its threshold.
    pub(crate) formatted_size: Pt,
    pub(crate) synthetic_bold: bool,
    pub(crate) synthetic_italic: bool,
    /// The script, as an ISO 15924 code such as `Latn`; `Zyyy` for text that only has
    /// characters common to scripts.
    pub(crate) script: [u8; 4],
    /// The language, as a BCP 47 tag.
    pub(crate) language: Option<String>,
}

/// What a character of the text is drawn with.
#[derive(Clone, Debug, PartialEq)]
struct Look {
    style: RunStyleId,
    face: Option<FaceId>,
    size: Pt,
    /// The size the formatting gives, before small capitals reduce it.
    formatted_size: Pt,
    synthetic_bold: bool,
    synthetic_italic: bool,
    /// Shared between the characters of a language, as each character has a look.
    language: Option<Arc<str>>,
}

/// A character on its way into a run.
struct Placed {
    /// Its bytes of the drawn text.
    text: Range<usize>,
    /// Its bytes of the source text.
    source: Range<usize>,
    look: Look,
    script: Script,
}

/// Itemizes `inlines` of a paragraph of `document`. Fonts are chosen through `fonts`,
/// which also collects what to report; `location` is the paragraph's.
pub(crate) fn itemize(
    document: &Document,
    inlines: &[Inline],
    fonts: &mut Fonts<'_>,
    location: &Location,
) -> Itemized {
    let mut builder = Builder {
        document,
        fonts,
        location,
        out: Itemized::default(),
        pending: Vec::new(),
        path: Vec::new(),
        languages: Vec::new(),
        shaper: Shaper::with_capacity(0),
    };
    builder.inlines(inlines);
    builder.flush();
    builder.out
}

struct Builder<'d, 'f, 'a> {
    document: &'d Document,
    fonts: &'f mut Fonts<'a>,
    location: &'d Location,
    out: Itemized,
    /// The characters since the last item that is not text.
    pending: Vec<Placed>,
    /// The path of the inline being itemized.
    path: InlinePath,
    /// The languages seen, shared by the looks that have them.
    languages: Vec<Arc<str>>,
    /// Shapes the text of a run cut for its length, to find where its clusters start.
    shaper: Shaper,
}

/// Characters that stay with the character before them, in its run and face: joiners,
/// variation selectors, the soft hyphen, bidirectional controls, marks of the Inherited
/// script and every mark with a canonical combining class (Hebrew points, Arabic
/// harakat, viramas). Spacing marks of combining class 0, such as Devanagari vowel
/// signs, take the font chosen for them.
pub(crate) fn attaches(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{E0100}'..='\u{E01EF}'
    ) || ch.script() == Script::Inherited
        || combining_class(ch) > 0
}

impl Builder<'_, '_, '_> {
    fn inlines(&mut self, inlines: &[Inline]) {
        for (index, inline) in inlines.iter().enumerate() {
            self.path.push(index);
            self.inline(inline);
            self.path.pop();
        }
    }

    fn inline(&mut self, inline: &Inline) {
        let path = self.path.clone();
        match inline {
            Inline::Text { text, style, .. } => self.text(text, *style),
            Inline::Symbol {
                font, code, style, ..
            } => self.symbol(font, *code, *style),
            Inline::SoftHyphen { style, .. } => self.text("\u{00AD}", *style),
            Inline::NoBreakHyphen { style, .. } => self.text("\u{2011}", *style),
            // A field shows its stored result. The fields the layout computes (PAGE,
            // NUMPAGES, SECTIONPAGES) are itemized again with their value by
            // pagination (PR 8), which then replaces their result.
            Inline::Field(field) => self.inlines(field.result()),
            Inline::Tab { style, .. } => self.item(Item::Tab(*style)),
            Inline::Break { kind, style, .. } => self.item(Item::Break(*kind, *style)),
            Inline::Object(_) => self.item(Item::Object(path)),
            Inline::Anchored(_) => self.item(Item::Anchored(path)),
            Inline::NoteReference { style, .. } => {
                self.item(Item::NoteReference(path, *style));
            }
            Inline::NoteNumber { style, .. }
            | Inline::Separator { style, .. }
            | Inline::ContinuationSeparator { style, .. } => {
                self.item(Item::NoteMark(*style));
            }
        }
    }

    fn item(&mut self, item: Item) {
        self.flush();
        self.out.items.push(item);
    }

    fn style(&self, id: RunStyleId) -> RunStyle {
        self.document.run_style(id).cloned().unwrap_or_default()
    }

    /// Adds `text` in style `id`.
    fn text(&mut self, text: &str, id: RunStyleId) {
        let style = self.style(id);
        self.text_in(text, id, &style);
    }

    /// Adds `text` in style `id`, with the formatting `style`.
    fn text_in(&mut self, text: &str, id: RunStyleId, style: &RunStyle) {
        let turkic = style
            .language()
            .latin()
            .is_some_and(|tag| crate::slot::language_is(tag, &["tr", "az"]));
        for ch in text.chars() {
            let source_start = self.out.source.len();
            self.out.source.push(ch);
            let source = source_start..self.out.source.len();
            let lowercase = ch.is_lowercase();
            let upper = style.caps() || (style.small_caps() && lowercase);
            let drawn: Vec<char> = if upper {
                uppercase(ch, turkic)
            } else {
                vec![ch]
            };
            let small = style.small_caps() && !style.caps() && lowercase;
            // The characters drawn for one character of the source (ŉ in capitals is ʼN)
            // stay together, in the look and script of the first.
            let mut drawn = drawn.into_iter();
            if let Some(first) = drawn.next() {
                self.character(first, source.clone(), id, style, small);
            }
            for ch in drawn {
                match self.pending.last() {
                    Some(previous) => {
                        let (look, script) = (previous.look.clone(), previous.script);
                        self.push_as(ch, source.clone(), look, script);
                    }
                    None => self.character(ch, source.clone(), id, style, small),
                }
            }
        }
    }

    /// Adds symbol `code` of `font` (`w:sym`).
    fn symbol(&mut self, font: &str, code: u16, id: RunStyleId) {
        let style = self.style(id);
        let code = u32::from(code);
        // A code that is neither a symbol font's (U+F020 to U+F0FF) nor a byte is a
        // Unicode character in that font.
        if !(0xF000..=0xF0FF).contains(&code) && code > 0xFF {
            if let Some(ch) = char::from_u32(code) {
                let fonts = RunFonts::new(font);
                self.text_in(&ch.to_string(), id, &style.clone().with_fonts(fonts));
                return;
            }
        }
        let (bold, italic) = (style.bold(), style.italic());
        let choice = self.fonts.choose(font, bold, italic);
        let source_start = self.out.source.len();
        if let Some(ch) = char::from_u32(code) {
            self.out.source.push(ch);
        }
        let source = source_start..self.out.source.len();
        // The font itself: a symbol font maps its codes at U+F020 to U+F0FF, another font
        // has them as characters U+0020 to U+00FF.
        let low = code & 0xFF;
        let codes = [0xF000 | low, low].into_iter().filter_map(char::from_u32);
        if let (true, Some(face)) = (choice.installed, choice.face) {
            for ch in codes {
                if self.fonts.has_glyph(face, ch) {
                    let look =
                        self.look(id, &style, choice, style.size(), style.size(), Slot::Ascii);
                    self.push(ch, source, look);
                    return;
                }
            }
        }
        // A symbol font that is not installed: its code as the Unicode character it
        // stands for, from a font that has it.
        let unicode = SymbolFont::of(font)
            .filter(|_| !choice.installed)
            .and_then(|symbol| symbol.unicode(code));
        match unicode {
            Some(ch) => {
                self.fonts.report_once(
                    ReportKind::FontSubstituted,
                    self.location,
                    format!(
                        "\"{font}\" is not installed: its symbols are drawn as the Unicode \
                         characters they stand for"
                    ),
                );
                let face = self.face_for(ch, choice, &style, bold, italic, font);
                let look = self.look(id, &style, face, style.size(), style.size(), Slot::Ascii);
                self.push(ch, source, look);
            }
            None => {
                let why = if choice.installed {
                    "the font has no such symbol"
                } else {
                    "the font is not installed"
                };
                self.fonts.report_once(
                    ReportKind::GlyphMissing,
                    self.location,
                    format!("symbol {code:04X} of \"{font}\" cannot be drawn: {why}"),
                );
                let ch = char::from_u32(0xF000 | low).unwrap_or('\u{FFFD}');
                let look = self.look(id, &style, choice, style.size(), style.size(), Slot::Ascii);
                self.push(ch, source, look);
            }
        }
    }

    /// The look of a character in `style`, in the face of `choice`.
    fn look(
        &mut self,
        id: RunStyleId,
        style: &RunStyle,
        choice: Choice,
        size: Pt,
        formatted_size: Pt,
        slot: Slot,
    ) -> Look {
        let language = match slot {
            Slot::EastAsia => style.language().east_asia(),
            Slot::ComplexScript => style.language().bidi(),
            Slot::Ascii | Slot::HighAnsi => style.language().latin(),
        };
        let language =
            language.map(
                |tag| match self.languages.iter().find(|known| &***known == tag) {
                    Some(known) => Arc::clone(known),
                    None => {
                        let tag: Arc<str> = Arc::from(tag);
                        self.languages.push(Arc::clone(&tag));
                        tag
                    }
                },
            );
        Look {
            style: id,
            face: choice.face,
            size,
            formatted_size,
            synthetic_bold: choice.synthetic_bold,
            synthetic_italic: choice.synthetic_italic,
            language,
        }
    }

    /// Adds `ch`, chosen a face by Word's rule, falling back to faces that have it.
    fn character(
        &mut self,
        ch: char,
        source: Range<usize>,
        id: RunStyleId,
        style: &RunStyle,
        small: bool,
    ) {
        if attaches(ch) {
            if let Some(previous) = self.pending.last() {
                // In its base's run, face and script.
                let (look, script) = (previous.look.clone(), previous.script);
                self.push_as(ch, source, look, script);
                return;
            }
        }
        let slot = slot(ch, style);
        let complex = slot == Slot::ComplexScript;
        let (bold, italic) = if complex {
            (style.bold_complex_script(), style.italic_complex_script())
        } else {
            (style.bold(), style.italic())
        };
        let formatted_size = if complex {
            style.size_complex_script()
        } else {
            style.size()
        };
        let size = if small {
            Pt(formatted_size.0 * SMALL_CAPS_SCALE)
        } else {
            formatted_size
        };
        let family = slot.family(style.fonts()).to_owned();
        let choice = self.fonts.choose(&family, bold, italic);
        // A symbol font that is not installed: its codes stand for Unicode characters,
        // drawn from a font that has them.
        let symbol = SymbolFont::of(&family).filter(|_| !choice.installed);
        let ch = match symbol.and_then(|symbol| symbol.unicode(u32::from(ch))) {
            Some(mapped) => {
                self.fonts.report_once(
                    ReportKind::FontSubstituted,
                    self.location,
                    format!(
                        "\"{family}\" is not installed: its symbols are drawn as the Unicode \
                         characters they stand for"
                    ),
                );
                mapped
            }
            None => ch,
        };
        let face = self.face_for(ch, choice, style, bold, italic, &family);
        let look = self.look(id, style, face, size, formatted_size, slot);
        self.push(ch, source, look);
    }

    /// The face that draws `ch`: the chosen one if it has the character, else another
    /// of the run's four fonts, else any font that has it; else the chosen one, which
    /// draws its missing-glyph shape.
    fn face_for(
        &mut self,
        ch: char,
        choice: Choice,
        style: &RunStyle,
        bold: bool,
        italic: bool,
        family: &str,
    ) -> Choice {
        let Some(face) = choice.face else {
            return choice;
        };
        if ch.is_whitespace() || ch.is_control() || self.fonts.has_glyph(face, ch) {
            return choice;
        }
        for other in Slot::ALL {
            let other_family = other.family(style.fonts());
            if crate::slot::same_family(other_family, family) {
                continue;
            }
            let other = self.fonts.choose(other_family, bold, italic);
            if let Some(face) = other.face.filter(|&face| self.fonts.has_glyph(face, ch)) {
                self.fonts.report_fallback(family, face, ch, self.location);
                return other;
            }
        }
        if let Some(covering) = self.fonts.covering(ch, family, bold, italic) {
            if let Some(face) = covering.face {
                self.fonts.report_fallback(family, face, ch, self.location);
            }
            return covering;
        }
        self.fonts.missing(ch, self.location);
        choice
    }

    fn push(&mut self, ch: char, source: Range<usize>, look: Look) {
        self.push_as(ch, source, look, ch.script());
    }

    fn push_as(&mut self, ch: char, source: Range<usize>, look: Look, script: Script) {
        let start = self.out.text.len();
        self.out.text.push(ch);
        self.pending.push(Placed {
            text: start..self.out.text.len(),
            source,
            look,
            script,
        });
    }

    /// Turns the pending characters into runs.
    fn flush(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        let scripts = resolve_scripts(&pending);
        let same = |a: usize, b: usize| {
            pending.get(a).map(|p| &p.look) == pending.get(b).map(|p| &p.look)
                && scripts.get(a) == scripts.get(b)
        };
        let mut start = 0;
        while start < pending.len() {
            let mut end = start + 1;
            let mut bytes = pending.get(start).map_or(0, |placed| placed.text.len());
            let mut too_long = false;
            while end < pending.len() && same(start, end) {
                let len = pending.get(end).map_or(0, |placed| placed.text.len());
                if bytes + len > MAX_RUN_BYTES {
                    too_long = true;
                    break;
                }
                bytes += len;
                end += 1;
            }
            // A run cut for its length ends where a cluster starts, as HarfRust forms
            // clusters in the run's face: a mark, a joiner or the rest of an Indic
            // syllable stays with its base, which the next run would otherwise draw after
            // a dotted circle. Nor is it cut between the characters drawn for one
            // character of the source (SS for ß). It ends after its last space that is
            // followed by such a start, else at the last such start. In a face HarfRust
            // cannot read, only marks and joiners stay with their base.
            if too_long {
                let script = scripts.get(start).copied().unwrap_or(Script::Common);
                let clusters = self.cluster_starts(&pending, start..end + 1, script);
                let text_at = |i: usize| {
                    pending
                        .get(i)
                        .and_then(|placed| self.out.text.get(placed.text.clone()))
                        .and_then(|text| text.chars().next())
                };
                let starts_cluster = |i: usize| {
                    let joins = text_at(i).is_some_and(attaches);
                    let same_source =
                        pending.get(i).map(|p| &p.source) == pending.get(i - 1).map(|p| &p.source);
                    let shaped = clusters.as_ref().is_none_or(|starts| {
                        pending
                            .get(i)
                            .is_some_and(|placed| starts.binary_search(&placed.text.start).is_ok())
                    });
                    !joins && !same_source && shaped
                };
                let after_space = (start + 1..=end).rev().find(|&i| {
                    text_at(i - 1).is_some_and(char::is_whitespace) && starts_cluster(i)
                });
                let cluster = (start + 1..=end).rev().find(|&i| starts_cluster(i));
                if let Some(cut) = after_space.or(cluster) {
                    end = cut;
                }
            }
            let (Some(first), Some(last)) = (pending.get(start), pending.get(end - 1)) else {
                break;
            };
            let script = scripts.get(start).copied().unwrap_or(Script::Common);
            self.out.items.push(Item::Text(TextRun {
                range: first.text.start..last.text.end,
                source: first.source.start..last.source.end,
                style: first.look.style,
                face: first.look.face,
                size: first.look.size,
                synthetic_bold: first.look.synthetic_bold,
                synthetic_italic: first.look.synthetic_italic,
                script: iso15924(script),
                formatted_size: first.look.formatted_size,
                language: first.look.language.as_deref().map(str::to_owned),
            }));
            start = end;
        }
    }
}

impl Builder<'_, '_, '_> {
    /// The bytes of the drawn text at which HarfRust starts a cluster when it shapes
    /// the characters `range` of `pending`, which have one look, in their face, script
    /// and language; `None` without a face HarfRust can read.
    fn cluster_starts(
        &mut self,
        pending: &[Placed],
        range: Range<usize>,
        script: Script,
    ) -> Option<Vec<usize>> {
        let first = pending.get(range.start)?;
        let last = pending.get(range.end.checked_sub(1)?)?;
        let face = self.fonts.face(first.look.face?)?;
        let text = self.out.text.get(first.text.start..last.text.end)?;
        let request = ShapeRequest {
            features: Features::default(),
            script: Some(iso15924(script)).filter(|code| code != b"Zyyy"),
            language: first.look.language.as_deref().map(str::to_owned),
        };
        let starts = self.shaper.cluster_starts(&face, text, &request)?;
        Some(starts.into_iter().map(|at| at + first.text.start).collect())
    }
}

/// The uppercase of `ch`, the Turkic way for i in Turkish and Azerbaijani.
fn uppercase(ch: char, turkic: bool) -> Vec<char> {
    if turkic && ch == 'i' {
        return vec!['\u{0130}'];
    }
    ch.to_uppercase().collect()
}

/// The script of each character, with characters common to scripts and inherited ones
/// taking the script of the text around them: the one before, or at the start the one
/// after.
fn resolve_scripts(placed: &[Placed]) -> Vec<Script> {
    let neutral =
        |script: Script| matches!(script, Script::Common | Script::Inherited | Script::Unknown);
    let mut scripts: Vec<Script> = placed.iter().map(|p| p.script).collect();
    let mut previous = None;
    for script in &mut scripts {
        if neutral(*script) {
            if let Some(previous) = previous {
                *script = previous;
            }
        } else {
            previous = Some(*script);
        }
    }
    let mut next = None;
    for script in scripts.iter_mut().rev() {
        if neutral(*script) {
            if let Some(next) = next {
                *script = next;
            }
        } else {
            next = Some(*script);
        }
    }
    scripts
}

/// The ISO 15924 code of a script; `Zyyy` for common characters.
fn iso15924(script: Script) -> [u8; 4] {
    let mut code = *b"Zyyy";
    if let Some(name) = script.short_name().as_bytes().get(..4) {
        code.copy_from_slice(name);
    }
    code
}

/// Shapes the text runs of `itemized`, in order: one shaped run per [`Item::Text`],
/// `None` for a run without a face. What could not be shaped as the face asks is
/// reported once per face.
pub(crate) fn shape_runs(
    document: &Document,
    itemized: &Itemized,
    fonts: &mut Fonts<'_>,
    shaper: &mut Shaper,
    location: &Location,
) -> Vec<Option<Arc<ShapedRun>>> {
    let mut shaped = Vec::new();
    for item in &itemized.items {
        let Item::Text(run) = item else {
            continue;
        };
        let face = run.face.and_then(|id| fonts.face(id));
        let Some(face) = face else {
            shaped.push(None);
            continue;
        };
        let style = document.run_style(run.style).cloned().unwrap_or_default();
        let request = request(run, &style);
        let text = itemized.text.get(run.range.clone()).unwrap_or_default();
        let result = shaper.shape(&face, text, &request);
        let name = face.face().names().family().unwrap_or("a font").to_owned();
        report_shaping(fonts, &result, &name, location);
        shaped.push(Some(result));
    }
    shaped
}

/// What a run of `style` asks the shaper for: the features of its formatting at the size
/// the formatting gives, its script unless it has only common characters, its language.
pub(crate) fn request(run: &TextRun, style: &RunStyle) -> ShapeRequest {
    ShapeRequest {
        features: Features::of(style, run.formatted_size),
        script: Some(run.script).filter(|code| code != b"Zyyy"),
        language: run.language.clone(),
    }
}

/// Reports, once per face, a run shaped without the face's layout tables or with glyphs
/// the tables name but the face lacks.
fn report_shaping(fonts: &mut Fonts<'_>, result: &ShapedRun, name: &str, location: &Location) {
    if result.is_nominal() {
        fonts.report_once(
            ReportKind::FeatureUnsupported,
            location,
            format!(
                "the layout tables of \"{name}\" cannot be used; its text is drawn \
                     without kerning, ligatures or mark positioning"
            ),
        );
    }
    if result.replaced_glyphs() > 0 {
        fonts.report_once(
            ReportKind::GlyphMissing,
            location,
            format!(
                "the layout tables of \"{name}\" name glyphs it does not have; the \
                     missing-glyph shape is drawn instead"
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use officewright_font::FontBook;

    use super::*;
    use crate::ir::{BreakClear, Field, FieldKind, Language, RunFonts};
    use crate::report::LayoutReport;
    use crate::testing::{book, minimal, renamed};

    const TEST: &str = "Officewright Test";

    /// The test font without a glyph for Á, under another family name: its cmap maps
    /// each character by an idDelta, and Á's now leads to glyph 0, "not mapped".
    fn without_a_acute(family: &str) -> Vec<u8> {
        let font = minimal();
        let delta = |glyph: u16, code: u16| glyph.wrapping_sub(code).to_be_bytes();
        let from = [
            delta(1, 0x20),
            delta(2, 0x41),
            delta(3, 0xB4),
            delta(4, 0xC1),
        ]
        .concat();
        let to = [
            delta(1, 0x20),
            delta(2, 0x41),
            delta(3, 0xB4),
            delta(0, 0xC1),
        ]
        .concat();
        let at = font
            .windows(from.len())
            .position(|w| w == from)
            .expect("the deltas");
        let mut font = font;
        font[at..at + to.len()].copy_from_slice(&to);
        renamed(font, family)
    }

    struct Setup {
        book: FontBook,
        document: Document,
    }

    impl Setup {
        fn new(fonts: Vec<Vec<u8>>) -> Self {
            Self {
                book: book(fonts),
                document: Document::default(),
            }
        }

        fn style(&mut self, style: RunStyle) -> RunStyleId {
            self.document.add_run_style(style)
        }

        fn run(&self, inlines: &[Inline]) -> (Itemized, LayoutReport, Vec<String>) {
            let mut fonts = Fonts::new(&self.book);
            let itemized = itemize(&self.document, inlines, &mut fonts, &Location::default());
            let families = itemized
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Text(run) => Some(run.face.map_or_else(String::new, |id| {
                        self.book
                            .face(id)
                            .and_then(|face| face.names().family())
                            .unwrap_or_default()
                            .to_owned()
                    })),
                    _ => None,
                })
                .collect();
            (itemized, fonts.into_report(), families)
        }
    }

    fn plain(family: &str) -> RunStyle {
        RunStyle::default().with_fonts(RunFonts::new(family))
    }

    fn runs(itemized: &Itemized) -> Vec<(&str, &str, f64)> {
        itemized
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Text(run) => Some((
                    itemized.text.get(run.range.clone()).unwrap(),
                    itemized.source.get(run.source.clone()).unwrap(),
                    run.size.0,
                )),
                _ => None,
            })
            .collect()
    }

    fn messages(report: &LayoutReport) -> Vec<(ReportKind, &str)> {
        report
            .iter()
            .map(|entry| (entry.kind(), entry.message()))
            .collect()
    }

    #[test]
    fn cuts_runs_where_the_formatting_changes() {
        let mut setup = Setup::new(vec![minimal()]);
        let regular = setup.style(plain(TEST));
        let bold = setup.style(plain(TEST).with_bold(true).with_size(Pt(12.0)));
        let (itemized, report, families) = setup.run(&[
            Inline::text("A A", regular),
            Inline::text("A\u{C1}", bold),
            Inline::text("A", bold),
        ]);
        assert_eq!(
            runs(&itemized),
            [("A A", "A A", 10.0), ("A\u{C1}A", "A\u{C1}A", 12.0)]
        );
        assert_eq!(families, [TEST, TEST]);
        let Some(Item::Text(bold_run)) = itemized.items.get(1) else {
            panic!("{itemized:?}");
        };
        // The book has no bold face: bold is synthetic.
        assert!(bold_run.synthetic_bold && !bold_run.synthetic_italic);
        assert_eq!(bold_run.script, *b"Latn");
        assert_eq!(
            messages(&report),
            [(
                ReportKind::FontSubstituted,
                "\"Officewright Test\" bold → \"Officewright Test\" with synthetic bold"
            )]
        );
    }

    #[test]
    fn draws_capitals_and_small_capitals_with_the_source_text_kept() {
        let mut setup = Setup::new(vec![minimal()]);
        let caps = setup.style(plain(TEST).with_caps(true));
        let small = setup.style(plain(TEST).with_small_caps(true));
        let (itemized, _, _) = setup.run(&[Inline::text("\u{E1}a", caps)]);
        assert_eq!(runs(&itemized), [("\u{C1}A", "\u{E1}a", 10.0)]);
        // Small capitals: capitals stay, lowercase letters become smaller capitals.
        let (itemized, _, _) = setup.run(&[Inline::text("Aa\u{E1}A", small)]);
        assert_eq!(
            runs(&itemized),
            [
                ("A", "A", 10.0),
                ("A\u{C1}", "a\u{E1}", 8.0),
                ("A", "A", 10.0)
            ]
        );
        // Turkish and Azerbaijani capitalize i with its dot; ß becomes two letters.
        let turkish = setup.style(
            plain(TEST)
                .with_caps(true)
                .with_language(Language::default().with_latin(Some("tr-TR".into()))),
        );
        let (itemized, _, _) = setup.run(&[Inline::text("i\u{DF}", turkish)]);
        assert_eq!(itemized.text, "\u{130}SS");
        assert_eq!(itemized.source, "i\u{DF}");
    }

    #[test]
    fn falls_back_to_another_font_of_the_run_then_to_any_font_that_has_the_character() {
        // Lite lacks Á; Zzzz has it and comes first in the book; Test has it too.
        let mut setup = Setup::new(vec![
            renamed(minimal(), "Officewright Zzzz"),
            without_a_acute("Officewright Lite"),
            minimal(),
        ]);
        // The East Asian font of the run is Test: it draws Á before any other font.
        let fonts = RunFonts::new("Officewright Lite").with_east_asia(TEST);
        let own = setup.style(RunStyle::default().with_fonts(fonts));
        let (itemized, report, families) = setup.run(&[Inline::text("A\u{C1}A", own)]);
        assert_eq!(
            runs(&itemized),
            [
                ("A", "A", 10.0),
                ("\u{C1}", "\u{C1}", 10.0),
                ("A", "A", 10.0)
            ]
        );
        assert_eq!(families, ["Officewright Lite", TEST, "Officewright Lite"]);
        // The fallback is reported, once for the family and the font it falls back to.
        assert_eq!(
            messages(&report),
            [(
                ReportKind::FontSubstituted,
                "\"Officewright Lite\" has no glyph for some characters, such as U+00C1 'Á'; \
                 they are drawn in \"Officewright Test\""
            )]
        );
        // No font of the run has it: the book's first font that does.
        let lite = setup.style(plain("Officewright Lite"));
        let (_, report, families) = setup.run(&[Inline::text("\u{C1}\u{C1}", lite)]);
        assert_eq!(families, ["Officewright Zzzz"]);
        assert_eq!(report.len(), 1, "{report:?}");
    }

    #[test]
    fn prefers_a_fallback_of_the_same_style() {
        // Zzzz comes first in the book, but Bold is bold, as the text asks.
        let weight = 700u16.to_be_bytes();
        let mut setup = Setup::new(vec![
            renamed(minimal(), "Officewright Zzzz"),
            crate::testing::patched(renamed(minimal(), "Officewright Bold"), b"OS/2", 4, &weight),
            without_a_acute("Officewright Lite"),
        ]);
        let bold = setup.style(plain("Officewright Lite").with_bold(true));
        let (_, _, families) = setup.run(&[Inline::text("\u{C1}", bold)]);
        assert_eq!(families, ["Officewright Bold"]);
        let regular = setup.style(plain("Officewright Lite"));
        let (_, _, families) = setup.run(&[Inline::text("\u{C1}", regular)]);
        assert_eq!(families, ["Officewright Zzzz"]);
    }

    #[test]
    fn keeps_spaces_the_face_lacks_in_the_face() {
        // The test font has U+0020 only; an em space stays in it, and the shaper gives it
        // a space's width, as it does for spaces a font lacks.
        let mut setup = Setup::new(vec![minimal(), renamed(minimal(), "Officewright Zzzz")]);
        let style = setup.style(plain(TEST));
        let (itemized, report, families) = setup.run(&[Inline::text("A\u{2003}A", style)]);
        assert_eq!(runs(&itemized).len(), 1);
        assert_eq!(families, [TEST]);
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn prefers_a_fallback_of_the_same_family_name() {
        let mut setup = Setup::new(vec![
            renamed(minimal(), "Zzzzzzzzzzzz Bold"),
            without_a_acute("Officewright Lite"),
            renamed(minimal(), "Officewright Zzzz"),
        ]);
        let lite = setup.style(plain("Officewright Lite"));
        let (_, _, families) = setup.run(&[Inline::text("\u{C1}", lite)]);
        assert_eq!(families, ["Officewright Zzzz"]);
    }

    #[test]
    fn reports_a_character_no_font_has_and_draws_the_missing_glyph_shape() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let (itemized, report, families) = setup.run(&[Inline::text("AB\u{4E2D}BC", style)]);
        // B, C and 中 stay in the chosen face; 中 is another script. The report counts
        // the characters of each script once.
        assert_eq!(families, [TEST, TEST, TEST]);
        assert_eq!(runs(&itemized).len(), 3);
        assert_eq!(
            messages(&report),
            [
                (
                    ReportKind::GlyphMissing,
                    "no font has 1 character of the Han script, such as U+4E2D '\u{4E2D}'; the \
                     missing-glyph shape is drawn"
                ),
                (
                    ReportKind::GlyphMissing,
                    "no font has 2 characters of the Latin script, such as U+0042 'B'; the \
                     missing-glyph shape is drawn"
                ),
            ]
        );
    }

    #[test]
    fn draws_symbols_of_missing_symbol_fonts_as_unicode() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let symbol_text = setup.style(plain("Symbol"));
        let (itemized, report, _) = setup.run(&[
            Inline::symbol("Wingdings", 0xF0A7, style),
            Inline::symbol("Symbol", 0xF0B7, style),
            Inline::text("a", symbol_text),
            Inline::symbol("Wingdings", 0xF041, style),
        ]);
        assert_eq!(itemized.text, "\u{25AA}\u{2022}\u{3B1}\u{F041}");
        assert_eq!(itemized.source, "\u{F0A7}\u{F0B7}a\u{F041}");
        let kinds: Vec<_> = messages(&report);
        assert!(kinds.contains(&(
            ReportKind::FontSubstituted,
            "\"Wingdings\" is not installed: its symbols are drawn as the Unicode characters they stand for"
        )), "{kinds:?}");
        assert!(
            kinds.contains(&(
                ReportKind::GlyphMissing,
                "symbol F041 of \"Wingdings\" cannot be drawn: the font is not installed"
            )),
            "{kinds:?}"
        );
    }

    #[test]
    fn keeps_marks_and_joiners_with_the_character_before_them() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let other = setup.style(plain(TEST).with_size(Pt(20.0)));
        let (itemized, _, _) = setup.run(&[
            Inline::text("A", style),
            Inline::text("\u{301}\u{200D}", other),
            Inline::soft_hyphen(other),
            Inline::text("A", other),
        ]);
        assert_eq!(
            runs(&itemized),
            [
                ("A\u{301}\u{200D}\u{AD}", "A\u{301}\u{200D}\u{AD}", 10.0),
                ("A", "A", 20.0)
            ]
        );
    }

    #[test]
    fn cuts_runs_where_the_script_changes_and_gives_neutral_characters_to_their_neighbours() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let (itemized, _, _) = setup.run(&[Inline::text("- A, \u{3B1}\u{3B2} A", style)]);
        let scripts: Vec<(&str, [u8; 4])> = itemized
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Text(run) => {
                    Some((itemized.text.get(run.range.clone()).unwrap(), run.script))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            scripts,
            [
                ("- A, ", *b"Latn"),
                ("\u{3B1}\u{3B2} ", *b"Grek"),
                ("A", *b"Latn")
            ]
        );
        let (itemized, _, _) = setup.run(&[Inline::text("12 - 3", style)]);
        let Some(Item::Text(run)) = itemized.items.first() else {
            panic!("{itemized:?}");
        };
        assert_eq!(run.script, *b"Zyyy");
    }

    #[test]
    fn cuts_long_runs_after_a_space() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let text = "AA ".repeat(2000);
        let (itemized, _, _) = setup.run(&[Inline::text(&text, style)]);
        let runs = runs(&itemized);
        assert_eq!(
            runs.len(),
            2,
            "{:?}",
            runs.iter().map(|r| r.0.len()).collect::<Vec<_>>()
        );
        assert!(runs.iter().all(|run| run.0.len() <= MAX_RUN_BYTES));
        assert!(runs[0].0.ends_with(' '));
        assert_eq!(runs.iter().map(|run| run.0).collect::<String>(), text);
        // Without a space, at the limit.
        let text = "A".repeat(5000);
        let (itemized, _, _) = setup.run(&[Inline::text(&text, style)]);
        let lengths: Vec<usize> = super::tests::runs(&itemized)
            .iter()
            .map(|run| run.0.len())
            .collect();
        assert_eq!(lengths, [4096, 904]);
    }

    #[test]
    fn keeps_the_pieces_that_are_not_text_in_order() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let field = Field::new(FieldKind::Other("DATE".into()), style)
            .with_result([Inline::text("A", style)]);
        let (itemized, _, _) = setup.run(&[
            Inline::text("A", style),
            Inline::tab(style),
            Inline::Field(field),
            Inline::line_break(BreakClear::None, style),
            Inline::no_break_hyphen(style),
        ]);
        assert_eq!(itemized.items.len(), 5, "{itemized:?}");
        assert!(matches!(itemized.items[0], Item::Text(_)));
        assert_eq!(itemized.items[1], Item::Tab(style));
        assert!(matches!(itemized.items[2], Item::Text(_)));
        assert!(matches!(
            itemized.items[3],
            Item::Break(BreakKind::Line(BreakClear::None), _)
        ));
        assert_eq!(itemized.text, "AA\u{2011}");
    }

    #[test]
    fn complex_script_text_takes_its_own_size_and_style() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(
            plain(TEST)
                .with_complex_script(true)
                .with_size(Pt(10.0))
                .with_size_complex_script(Pt(14.0))
                .with_bold_complex_script(true)
                .with_language(Language::default().with_bidi(Some("ar-SA".into()))),
        );
        let (itemized, _, _) = setup.run(&[Inline::text("A", style)]);
        let Some(Item::Text(run)) = itemized.items.first() else {
            panic!("{itemized:?}");
        };
        assert_eq!(run.size, Pt(14.0));
        assert!(run.synthetic_bold);
        assert_eq!(run.language.as_deref(), Some("ar-SA"));
    }

    #[test]
    fn shapes_each_run_with_the_features_its_formatting_asks_for() {
        let mut setup = Setup::new(vec![minimal()]);
        let kerned = setup.style(
            plain(TEST)
                .with_size(Pt(14.0))
                .with_kern_min(Some(Pt(12.0))),
        );
        let unkerned = setup.style(
            plain(TEST)
                .with_size(Pt(10.0))
                .with_kern_min(Some(Pt(12.0))),
        );
        let inlines = [Inline::text("A\u{C1}", kerned), Inline::text("A", unkerned)];
        let mut fonts = Fonts::new(&setup.book);
        let itemized = itemize(&setup.document, &inlines, &mut fonts, &Location::default());
        let mut shaper = Shaper::new();
        let shaped = shape_runs(
            &setup.document,
            &itemized,
            &mut fonts,
            &mut shaper,
            &Location::default(),
        );
        let ids: Vec<Vec<u16>> = shaped
            .iter()
            .map(|run| {
                run.as_ref()
                    .unwrap()
                    .glyphs()
                    .iter()
                    .map(|g| g.glyph.to_u16())
                    .collect()
            })
            .collect();
        assert_eq!(ids, [vec![2, 4], vec![2]]);
        assert!(Features::of(setup.document.run_style(kerned).unwrap(), Pt(14.0)).kerning);
        assert!(!Features::of(setup.document.run_style(unkerned).unwrap(), Pt(10.0)).kerning);
        assert!(!Features::of(&plain(TEST), Pt(72.0)).kerning);
    }

    fn first_run(itemized: &Itemized) -> &TextRun {
        match itemized.items.first() {
            Some(Item::Text(run)) => run,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_fallback_face_draws_bold_and_italic_it_lacks_synthetically() {
        let mut setup = Setup::new(vec![
            without_a_acute("Officewright Lite"),
            renamed(minimal(), "Officewright Zzzz"),
        ]);
        let bold = setup.style(plain("Officewright Lite").with_bold(true).with_italic(true));
        let (itemized, _, families) = setup.run(&[Inline::text("\u{C1}", bold)]);
        assert_eq!(families, ["Officewright Zzzz"]);
        let run = first_run(&itemized);
        assert!(run.synthetic_bold && run.synthetic_italic, "{run:?}");
        // A face of the asked style draws it as it is.
        let mut setup = Setup::new(vec![
            without_a_acute("Officewright Lite"),
            crate::testing::italic(renamed(minimal(), "Officewright Zzzz")),
        ]);
        let italic = setup.style(plain("Officewright Lite").with_italic(true));
        let (itemized, _, _) = setup.run(&[Inline::text("\u{C1}", italic)]);
        assert!(!first_run(&itemized).synthetic_italic);
    }

    #[test]
    fn prefers_the_same_style_over_the_same_family_name() {
        // Zzzz shares the family's first word but is upright; the other is italic.
        let mut setup = Setup::new(vec![
            renamed(minimal(), "Officewright Zzzz"),
            crate::testing::italic(renamed(minimal(), "Zzzzzzzzzzzzz Ita")),
            without_a_acute("Officewright Lite"),
        ]);
        let italic = setup.style(plain("Officewright Lite").with_italic(true));
        let (_, _, families) = setup.run(&[Inline::text("\u{C1}", italic)]);
        assert_eq!(families, ["Zzzzzzzzzzzzz Ita"]);
    }

    #[test]
    fn gives_objects_in_field_results_their_path() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let note = setup
            .document
            .add_note(crate::ir::Note::new(crate::ir::NoteKind::Footnote, []));
        let inner = Field::new(FieldKind::Other("REF".into()), style).with_result([
            Inline::text("A", style),
            Inline::note_reference(note, false, style),
        ]);
        let outer =
            Field::new(FieldKind::Other("IF".into()), style).with_result([Inline::Field(inner)]);
        let (itemized, _, _) = setup.run(&[
            Inline::note_reference(note, false, style),
            Inline::text("A", style),
            Inline::Field(outer),
        ]);
        let paths: Vec<&InlinePath> = itemized
            .items
            .iter()
            .filter_map(|item| match item {
                Item::NoteReference(path, _) => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(paths, [&vec![0], &vec![2, 0, 1]]);
    }

    #[test]
    fn cuts_long_runs_only_where_a_cluster_starts() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        // e and a combining acute, 3 bytes, with no space: the cut must not start a run
        // with the mark.
        let text = "e\u{301}".repeat(2000);
        let (itemized, _, _) = setup.run(&[Inline::text(&text, style)]);
        let pieces = runs(&itemized);
        assert!(pieces.len() > 1);
        for (text, _, _) in &pieces {
            assert!(text.len() <= MAX_RUN_BYTES);
            assert!(!text.starts_with('\u{301}'));
        }
        // ß drawn as SS in capitals: both S stay in one run, and the runs' source does not
        // overlap.
        let caps = setup.style(plain(TEST).with_caps(true));
        // An A first, so that the limit falls between the two S of one ß.
        let text = format!("A{}", "\u{DF}".repeat(3000));
        let (itemized, _, _) = setup.run(&[Inline::text(&text, caps)]);
        let mut end = 0;
        for item in &itemized.items {
            let Item::Text(run) = item else { continue };
            assert_eq!(run.source.start, end, "{run:?}");
            end = run.source.end;
            // ß is two bytes, and so is SS: a run cut between two S would not match.
            assert_eq!(run.range.len(), run.source.len());
        }
        assert_eq!(end, itemized.source.len());
    }

    #[test]
    fn keeps_marks_with_a_combining_class_and_bidi_controls_with_their_base() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let other = setup.style(plain(TEST).with_size(Pt(20.0)));
        // A Hebrew point (class 10) is of the Hebrew script, not Inherited.
        let (itemized, _, _) = setup.run(&[
            Inline::text("A", style),
            Inline::text("\u{5B0}\u{202A}", other),
        ]);
        assert_eq!(
            runs(&itemized),
            [("A\u{5B0}\u{202A}", "A\u{5B0}\u{202A}", 10.0)]
        );
    }

    #[test]
    fn keeps_installed_symbol_fonts() {
        // A font named Symbol that has A (U+0041), but not U+F041 or U+F0B7.
        let symbol = crate::testing::with_family(minimal(), "Symbol");
        let mut setup = Setup::new(vec![minimal(), symbol]);
        let style = setup.style(plain(TEST));
        let symbol_text = setup.style(plain("Symbol"));
        let (itemized, report, families) = setup.run(&[
            Inline::text("A", symbol_text),
            Inline::symbol("Symbol", 0x0041, style),
        ]);
        // Text and the symbol code stay A, drawn in the installed Symbol.
        assert_eq!(itemized.text, "AA");
        assert_eq!(families, ["Symbol", "Symbol"]);
        assert!(report.is_empty(), "{report:?}");
        let (_, report, _) = setup.run(&[Inline::symbol("Symbol", 0xF0B7, style)]);
        assert_eq!(
            messages(&report),
            [(
                ReportKind::GlyphMissing,
                "symbol F0B7 of \"Symbol\" cannot be drawn: the font has no such symbol"
            )]
        );
    }

    #[test]
    fn reads_symbol_codes_that_are_unicode_characters_as_such() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let (itemized, _, _) = setup.run(&[
            Inline::symbol(TEST, 0x00C1, style),
            Inline::symbol(TEST, 0x2022, style),
            Inline::symbol("Wingdings", 0xF071, style),
        ]);
        // Á of the font itself, U+2022 as a character, Wingdings' shadowed box as Unicode.
        assert_eq!(itemized.text, "\u{C1}\u{2022}\u{2751}");
    }

    #[test]
    fn capitals_win_over_small_capitals() {
        let mut setup = Setup::new(vec![minimal()]);
        let both = setup.style(plain(TEST).with_caps(true).with_small_caps(true));
        let (itemized, _, _) = setup.run(&[Inline::text("a", both)]);
        assert_eq!(runs(&itemized), [("A", "a", 10.0)]);
    }

    #[test]
    fn east_asian_text_takes_the_east_asian_language() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(
            plain(TEST).with_language(
                Language::default()
                    .with_latin(Some("en-US".into()))
                    .with_east_asia(Some("ja-JP".into())),
            ),
        );
        let (itemized, _, _) = setup.run(&[Inline::text("A\u{6F22}", style)]);
        let languages: Vec<Option<&str>> = itemized
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Text(run) => Some(run.language.as_deref()),
                _ => None,
            })
            .collect();
        assert_eq!(languages, [Some("en-US"), Some("ja-JP")]);
    }

    #[test]
    fn capitalizes_i_the_turkic_way_by_the_primary_language() {
        let mut setup = Setup::new(vec![minimal()]);
        for (tag, expected) in [
            ("az-Latn-AZ", "\u{130}"),
            ("TR", "\u{130}"),
            ("trv", "I"),
            ("en", "I"),
        ] {
            let style = setup.style(
                plain(TEST)
                    .with_caps(true)
                    .with_language(Language::default().with_latin(Some(tag.into()))),
            );
            let (itemized, _, _) = setup.run(&[Inline::text("i", style)]);
            assert_eq!(itemized.text, expected, "{tag}");
        }
    }

    #[test]
    fn kerning_compares_the_formatted_size_with_the_threshold() {
        let mut setup = Setup::new(vec![minimal()]);
        // Small capitals of a 12 pt run kerned from 12 pt: drawn at 9.6 pt, kerned all
        // the same, as the capitals next to them.
        let id = setup.style(
            plain(TEST)
                .with_size(Pt(12.0))
                .with_kern_min(Some(Pt(12.0)))
                .with_small_caps(true)
                .with_ligatures(crate::ir::Ligatures::default().with_standard(true))
                .with_contextual_alternates(true)
                .with_language(Language::default().with_latin(Some("ro".into()))),
        );
        let (itemized, _, _) = setup.run(&[Inline::text("a", id)]);
        let run = first_run(&itemized);
        assert_eq!(run.size, Pt(12.0 * SMALL_CAPS_SCALE));
        let asked = request(run, setup.document.run_style(id).unwrap());
        assert!(asked.features.kerning);
        assert!(asked.features.ligatures.standard());
        assert!(asked.features.contextual_alternates);
        assert_eq!(asked.script, Some(*b"Latn"));
        assert_eq!(asked.language.as_deref(), Some("ro"));
        // Below the threshold, no kerning; common characters only, no script.
        let small = setup.style(
            plain(TEST)
                .with_size(Pt(11.99))
                .with_kern_min(Some(Pt(12.0))),
        );
        let (itemized, _, _) = setup.run(&[Inline::text("12", small)]);
        let asked = request(
            first_run(&itemized),
            setup.document.run_style(small).unwrap(),
        );
        assert!(!asked.features.kerning);
        assert_eq!(asked.script, None);
    }

    #[test]
    fn reports_runs_shaped_without_the_faces_layout_tables() {
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let location = Location::default();
        report_shaping(&mut fonts, &ShapedRun::for_tests(false, 0), "F", &location);
        report_shaping(&mut fonts, &ShapedRun::for_tests(true, 0), "F", &location);
        report_shaping(&mut fonts, &ShapedRun::for_tests(true, 2), "F", &location);
        let report = fonts.into_report();
        assert_eq!(
            messages(&report),
            [
                (
                    ReportKind::FeatureUnsupported,
                    "the layout tables of \"F\" cannot be used; its text is drawn without \
                     kerning, ligatures or mark positioning"
                ),
                (
                    ReportKind::GlyphMissing,
                    "the layout tables of \"F\" name glyphs it does not have; the \
                     missing-glyph shape is drawn instead"
                ),
            ]
        );
    }

    #[test]
    fn an_installed_symbol_font_stays_in_a_style_it_lacks() {
        // Symbol is installed, but not in bold: it draws bold synthetically, and its codes
        // are its own, not the Unicode characters they stand for.
        let symbol = crate::testing::with_family(minimal(), "Symbol");
        let mut setup = Setup::new(vec![minimal(), symbol]);
        let bold_symbol = setup.style(plain("Symbol").with_bold(true));
        let bold = setup.style(plain(TEST).with_bold(true));
        let (itemized, _, families) = setup.run(&[
            Inline::text("A", bold_symbol),
            Inline::symbol("Symbol", 0x0041, bold),
        ]);
        assert_eq!(itemized.text, "AA");
        assert_eq!(families, ["Symbol", "Symbol"]);
        assert!(first_run(&itemized).synthetic_bold);
    }

    #[test]
    fn keeps_the_capitals_of_one_character_together() {
        // ŉ in capitals is ʼN. The rule gives ʼ (a spacing modifier letter) the East
        // Asian font with the hint, and N the ASCII one; they stay one cluster all the
        // same, in the font of the first.
        let mut setup = Setup::new(vec![minimal(), renamed(minimal(), "Officewright Zzzz")]);
        let fonts = RunFonts::new(TEST)
            .with_east_asia("Officewright Zzzz")
            .with_hint(crate::ir::FontHint::EastAsia);
        let style = setup.style(RunStyle::default().with_fonts(fonts).with_caps(true));
        let (itemized, _, families) = setup.run(&[Inline::text("\u{149}", style)]);
        assert_eq!(runs(&itemized), [("\u{2BC}N", "\u{149}", 10.0)]);
        assert_eq!(families, ["Officewright Zzzz"]);
    }

    #[test]
    fn cuts_long_runs_where_harfrust_starts_a_cluster() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        // Thai sara i (U+0E34) and Devanagari vowel sign i (U+093F) are marks of combining
        // class 0; the limit falls before one of them, after its base.
        for pair in ["\u{E01}\u{E34}", "\u{915}\u{93F}"] {
            let text = pair.repeat(2000);
            let (itemized, _, _) = setup.run(&[Inline::text(&text, style)]);
            let pieces = runs(&itemized);
            assert!(pieces.len() > 1);
            for (text, _, _) in &pieces {
                assert!(text.len() <= MAX_RUN_BYTES);
                assert!(
                    text.starts_with(|ch: char| pair.starts_with(ch)),
                    "{pair:?}"
                );
            }
        }
        // A space followed by a mark is no place to cut either: the mark belongs to the
        // space. The run ends at the next cluster instead.
        let text = format!("{} \u{E34}\u{E01}", "\u{E01}".repeat(1364));
        let (itemized, _, _) = setup.run(&[Inline::text(&text, style)]);
        let pieces = runs(&itemized);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces.get(1).map(|piece| piece.0), Some("\u{E01}"));
        // After a space followed by a base, it is.
        let text = format!("{} \u{E01}\u{E34}\u{E01}", "\u{E01}".repeat(1363));
        let (itemized, _, _) = setup.run(&[Inline::text(&text, style)]);
        let pieces = runs(&itemized);
        assert_eq!(
            pieces.get(1).map(|piece| piece.0),
            Some("\u{E01}\u{E34}\u{E01}")
        );
    }

    #[test]
    fn orders_fallbacks_for_each_requested_family() {
        // Both families lack Á; each falls back to the face that shares its first word,
        // although the other comes first in the book.
        let mut setup = Setup::new(vec![
            without_a_acute("Alphaaaaaaaa Lite"),
            without_a_acute("Betaaaaaaaaa Lite"),
            renamed(minimal(), "Alphaaaaaaaa Zzzz"),
            renamed(minimal(), "Betaaaaaaaaa Zzzz"),
        ]);
        let alpha = setup.style(plain("Alphaaaaaaaa Lite"));
        let beta = setup.style(plain("Betaaaaaaaaa Lite"));
        let (_, _, families) =
            setup.run(&[Inline::text("\u{C1}", alpha), Inline::text("\u{C1}", beta)]);
        assert_eq!(families, ["Alphaaaaaaaa Zzzz", "Betaaaaaaaaa Zzzz"]);
    }

    #[test]
    fn reports_missing_characters_at_the_paragraph_of_the_first() {
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let book = &setup.book;
        let mut fonts = Fonts::new(book);
        let at = |index: usize| {
            let mut location = Location::default();
            location.push(crate::report::Step::Field("blocks"));
            location.push(crate::report::Step::Index(index));
            location
        };
        for index in [3, 5] {
            itemize(
                &setup.document,
                &[Inline::text("\u{4E2D}\u{6587}", style)],
                &mut fonts,
                &at(index),
            );
        }
        let report = fonts.into_report();
        let entry = report.iter().next().unwrap();
        assert_eq!(entry.kind(), ReportKind::GlyphMissing);
        assert_eq!(entry.location(), &at(3));
        assert_eq!(report.len(), 1, "{report:?}");
    }

    #[test]
    fn keeps_format_characters_off_the_start_of_a_cut_run() {
        // HarfRust starts a cluster at these, but they belong to the character before.
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        for ch in ['\u{AD}', '\u{200B}', '\u{200C}', '\u{2060}'] {
            let text = format!("{}{ch}AAAA", "A".repeat(4095));
            let (itemized, _, _) = setup.run(&[Inline::text(&text, style)]);
            let pieces = runs(&itemized);
            assert_eq!(pieces.len(), 2, "{ch:?}");
            for (text, _, _) in &pieces {
                assert!(text.starts_with('A'), "{ch:?}");
            }
        }
    }

    #[test]
    fn cuts_long_runs_in_a_face_harfrust_cannot_read_where_a_cluster_starts() {
        // Without HarfRust's clusters, marks and joiners still stay with their base, and the
        // capitals of one character together.
        let mut setup = Setup::new(vec![minimal()]);
        let style = setup.style(plain(TEST));
        let caps = setup.style(plain(TEST).with_caps(true));
        let mut fonts = Fonts::new(&setup.book);
        let ids: Vec<_> = setup.book.faces().map(|(id, _)| id).collect();
        for id in ids {
            fonts.load_unreadable(id);
        }
        let text = "e\u{301}".repeat(2000);
        let inlines = [Inline::text(&text, style)];
        let itemized = itemize(&setup.document, &inlines, &mut fonts, &Location::default());
        let pieces = runs(&itemized);
        assert!(pieces.len() > 1);
        for (text, _, _) in &pieces {
            assert!(text.starts_with('e'));
        }
        let text = format!("A{}", "\u{DF}".repeat(3000));
        let inlines = [Inline::text(&text, caps)];
        let itemized = itemize(&setup.document, &inlines, &mut fonts, &Location::default());
        let mut end = 0;
        for item in &itemized.items {
            let Item::Text(run) = item else { continue };
            assert_eq!(run.source.start, end, "{run:?}");
            assert_eq!(run.range.len(), run.source.len());
            end = run.source.end;
        }
        assert_eq!(end, itemized.source.len());
        let shaped = shape_runs(
            &setup.document,
            &itemized,
            &mut fonts,
            &mut Shaper::new(),
            &Location::default(),
        );
        assert!(shaped.iter().flatten().all(|run| run.is_nominal()));
    }
}
