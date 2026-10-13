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
use crate::ir::{BreakKind, Document, Inline, RunStyle, RunStyleId};
use crate::report::{Location, ReportKind};
use crate::shape::{Features, ShapeRequest, ShapedRun, Shaper};
use crate::slot::{Slot, slot};
use crate::symbol::SymbolFont;

/// The size of small capitals, as a fraction of the run's size. Hypothesis until a Word
/// reference shows it (`E-smallcaps.docx`).
pub(crate) const SMALL_CAPS_SCALE: f64 = 0.8;

/// The most bytes of text a run has: a longer one is cut, at a space where there is one,
/// so that shaping time and memory stay bounded per run.
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
    /// An inline object, by its position among the paragraph's inlines.
    Object(usize),
    /// An anchored object, by its position among the paragraph's inlines; it takes no
    /// room in the line.
    Anchored(usize),
    /// A note reference, by its position among the paragraph's inlines.
    NoteReference(usize, RunStyleId),
    /// A note's own number or a separator, which the notes layout fills in.
    NoteMark(RunStyleId),
}

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
    synthetic_bold: bool,
    synthetic_italic: bool,
    language: Option<String>,
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
}

/// Characters that stay with the character before them, in its run and face: joiners,
/// variation selectors, the soft hyphen, bidirectional controls and combining marks.
fn attaches(ch: char) -> bool {
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
}

impl Builder<'_, '_, '_> {
    fn inlines(&mut self, inlines: &[Inline]) {
        for (index, inline) in inlines.iter().enumerate() {
            match inline {
                Inline::Text { text, style, .. } => self.text(text, *style),
                Inline::Symbol {
                    font, code, style, ..
                } => self.symbol(font, *code, *style),
                Inline::SoftHyphen { style, .. } => self.text("\u{00AD}", *style),
                Inline::NoBreakHyphen { style, .. } => self.text("\u{2011}", *style),
                Inline::Field(field) => self.inlines(field.result()),
                Inline::Tab { style, .. } => self.item(Item::Tab(*style)),
                Inline::Break { kind, style, .. } => self.item(Item::Break(*kind, *style)),
                Inline::Object(_) => self.item(Item::Object(index)),
                Inline::Anchored(_) => self.item(Item::Anchored(index)),
                Inline::NoteReference { style, .. } => {
                    self.item(Item::NoteReference(index, *style));
                }
                Inline::NoteNumber { style, .. }
                | Inline::Separator { style, .. }
                | Inline::ContinuationSeparator { style, .. } => {
                    self.item(Item::NoteMark(*style));
                }
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
        let turkic = style
            .language()
            .latin()
            .is_some_and(|tag| tag.starts_with("tr") || tag.starts_with("az"));
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
            for drawn in drawn {
                self.character(drawn, source.clone(), id, &style, small);
            }
        }
    }

    /// Adds symbol `code` of `font` (`w:sym`).
    fn symbol(&mut self, font: &str, code: u16, id: RunStyleId) {
        let style = self.style(id);
        let (bold, italic) = (style.bold(), style.italic());
        let choice = self.fonts.choose(font, bold, italic);
        let private = char::from_u32(0xF000 | u32::from(code & 0xFF));
        let source_start = self.out.source.len();
        if let Some(ch) = char::from_u32(u32::from(code)) {
            self.out.source.push(ch);
        }
        let source = source_start..self.out.source.len();
        // The font itself, which maps its codes at U+F020 to U+F0FF.
        if let (true, Some(face), Some(ch)) = (choice.installed, choice.face, private) {
            if self.fonts.has_glyph(face, ch) {
                let look = self.look(id, &style, choice, style.size(), Slot::Ascii);
                self.push(ch, source, look);
                return;
            }
        }
        // A symbol font that is not installed: its code as the Unicode character it
        // stands for, from a font that has it.
        let unicode = SymbolFont::of(font).and_then(|symbol| symbol.unicode(u32::from(code)));
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
                let look = self.look(id, &style, face, style.size(), Slot::Ascii);
                self.push(ch, source, look);
            }
            None => {
                self.fonts.report_once(
                    ReportKind::GlyphMissing,
                    self.location,
                    format!(
                        "symbol {code:04X} of \"{font}\" cannot be drawn: the font is not installed"
                    ),
                );
                let ch = private.unwrap_or('\u{FFFD}');
                let look = self.look(id, &style, choice, style.size(), Slot::Ascii);
                self.push(ch, source, look);
            }
        }
    }

    /// The look of a character in `style`, in the face of `choice`.
    fn look(&self, id: RunStyleId, style: &RunStyle, choice: Choice, size: Pt, slot: Slot) -> Look {
        let language = match slot {
            Slot::EastAsia => style.language().east_asia(),
            Slot::ComplexScript => style.language().bidi(),
            Slot::Ascii | Slot::HighAnsi => style.language().latin(),
        };
        Look {
            style: id,
            face: choice.face,
            size,
            synthetic_bold: choice.synthetic_bold,
            synthetic_italic: choice.synthetic_italic,
            language: language.map(str::to_owned),
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
                let look = previous.look.clone();
                self.push(ch, source, look);
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
        let mut size = if complex {
            style.size_complex_script()
        } else {
            style.size()
        };
        if small {
            size = Pt(size.0 * SMALL_CAPS_SCALE);
        }
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
        let look = self.look(id, style, face, size, slot);
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
            if other_family == family {
                continue;
            }
            let other = self.fonts.choose(other_family, bold, italic);
            if other
                .face
                .is_some_and(|face| self.fonts.has_glyph(face, ch))
            {
                return other;
            }
        }
        if let Some(covering) = self.fonts.covering(ch, family, bold, italic) {
            return Choice {
                face: Some(covering),
                synthetic_bold: false,
                synthetic_italic: false,
                installed: false,
            };
        }
        self.fonts.report_once(
            ReportKind::GlyphMissing,
            self.location,
            format!(
                "no font has U+{:04X} '{}'; the missing-glyph shape is drawn",
                u32::from(ch),
                ch.escape_debug()
            ),
        );
        choice
    }

    fn push(&mut self, ch: char, source: Range<usize>, look: Look) {
        let start = self.out.text.len();
        self.out.text.push(ch);
        self.pending.push(Placed {
            text: start..self.out.text.len(),
            source,
            look,
            script: ch.script(),
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
            // A run cut for its length ends after its last space, where it has one.
            if too_long {
                let after_space = (start + 1..end).rev().find(|&i| {
                    pending
                        .get(i - 1)
                        .and_then(|placed| self.out.text.get(placed.text.clone()))
                        .is_some_and(|text| text.chars().all(char::is_whitespace))
                });
                if let Some(cut) = after_space {
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
                language: first.look.language.clone(),
            }));
            start = end;
        }
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
    let mut code = [b'Z', b'y', b'y', b'y'];
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
        let request = ShapeRequest {
            features: Features::of(&style, run.size),
            script: Some(run.script).filter(|code| code != b"Zyyy"),
            language: run.language.clone(),
        };
        let text = itemized.text.get(run.range.clone()).unwrap_or_default();
        let result = shaper.shape(&face, text, &request);
        let name = face.face().names().family().unwrap_or("a font").to_owned();
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
        shaped.push(Some(result));
    }
    shaped
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
        assert!(report.is_empty(), "{report:?}");
        // No font of the run has it: the book's first font that does.
        let lite = setup.style(plain("Officewright Lite"));
        let (_, _, families) = setup.run(&[Inline::text("\u{C1}", lite)]);
        assert_eq!(families, ["Officewright Zzzz"]);
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
        let (itemized, report, families) = setup.run(&[Inline::text("AB\u{4E2D}B", style)]);
        // B and 中 stay in the chosen face; 中 is another script.
        assert_eq!(families, [TEST, TEST, TEST]);
        assert_eq!(runs(&itemized).len(), 3);
        assert_eq!(
            messages(&report),
            [
                (
                    ReportKind::GlyphMissing,
                    "no font has U+0042 'B'; the missing-glyph shape is drawn"
                ),
                (
                    ReportKind::GlyphMissing,
                    "no font has U+4E2D '\u{4E2D}'; the missing-glyph shape is drawn"
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
}
