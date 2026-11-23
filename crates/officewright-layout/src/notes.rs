//! The numbers of footnotes and endnotes.

use std::collections::{HashMap, HashSet};

use crate::ir::{
    Block, Document, Inline, MAX_TABLE_DEPTH, NoteId, NoteKind, NoteRestart, NumberFormat,
    Paragraph, RunStyleId,
};
use crate::numbers::{Departures, number_text};

/// The number each referenced note shows, as text, and the formats and restarts of them
/// that the layout writes otherwise than the document says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NoteNumbers {
    numbers: HashMap<NoteId, String>,
    /// The notes numbered in a section that starts their numbers again on each page: by
    /// the section, the start, the format and the language of their numbers.
    paged: HashMap<NoteId, Paged>,
    /// The notes referenced in headers and footers, where a reference shows no number
    /// unless the body numbers its note.
    framed: HashSet<NoteId>,
    /// How numbers are written otherwise than their formats say: in decimal, or in
    /// English for a language without words.
    pub(crate) departures: Departures,
}

/// How a note in a section that numbers its notes again on each page is numbered.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Paged {
    section: usize,
    start: u32,
    format: NumberFormat,
    language: Option<String>,
}

impl NoteNumbers {
    /// The numbers of the notes of `document`, in the order of their references in the
    /// body's text, each from its section's numbering of its kind (ECMA-376 Part 1,
    /// §17.11): from its start, again at each section for those that restart there, and
    /// in a section that numbers on, its start plus the notes numbered before it in the
    /// document, as Word's render of `E-footnote-numbering.docx` shows (section 3, which
    /// numbers on from 1 after sections of three notes from 5 and of two restarted at 1,
    /// shows 6); in one that restarts on each page, through the section, until a layout
    /// pass places them ([`by_page`](Self::by_page)). A
    /// reference with a custom mark (`w:customMarkFollows`) takes no number and does not
    /// count (§17.11.14); nor does one in a text box, which Word ignores (MS-OI29500
    /// 2.1.302 b and 2.1.305 a), nor a second reference to a note, nor one in the result
    /// of a field the layout computes, which is not laid out. One in hidden text is
    /// numbered, a hypothesis until a Word render. References in headers and footers take
    /// no number and mark their notes: a reference to a marked note that the body does
    /// not number shows none, wherever it stands, which the itemizer reports where the
    /// first such reference is laid out; one to a note the body numbers shows that
    /// number.
    pub(crate) fn new(document: &Document) -> Self {
        let mut numbers = Self::default();
        let mut next = Counters::default();
        for (index, section) in document.sections().iter().enumerate() {
            for (kind, props) in [
                (NoteKind::Footnote, section.footnotes()),
                (NoteKind::Endnote, section.endnotes()),
            ] {
                let start = u32::from(props.start());
                let first = match props.restart() {
                    NoteRestart::Continuous => start.saturating_add(next.counted(kind)),
                    _ => start,
                };
                next.set(kind, first);
            }
            let mut walk = Walk {
                document,
                section,
                index,
                numbers: &mut numbers,
                next: &mut next,
                frames: false,
            };
            walk.blocks(section.blocks(), 0);
            walk.frames = true;
            let sets = [section.headers(), section.footers()];
            for blocks in sets
                .iter()
                .flat_map(|set| [set.primary(), set.first(), set.even()])
            {
                walk.blocks(blocks, 0);
            }
        }
        numbers
    }

    /// The numbers of the notes numbered again on each page, as `pages` hold them: the
    /// notes whose references each page holds, in order, a page after another. On each
    /// page, the notes of each section count from that section's start; a note no page
    /// holds keeps its number. `None` where that changes no number.
    pub(crate) fn by_page(&self, pages: &[Vec<NoteId>]) -> Option<Self> {
        if self.paged.is_empty() {
            return None;
        }
        let mut numbers = self.clone();
        for page in pages {
            let mut next: HashMap<usize, u32> = HashMap::new();
            for note in page {
                let Some(paged) = self.paged.get(note) else {
                    continue;
                };
                let number = next.entry(paged.section).or_insert(paged.start);
                let (text, departures) =
                    number_text(*number, &paged.format, paged.language.as_deref());
                *number = number.saturating_add(1);
                numbers.departures.add(departures);
                numbers.numbers.insert(*note, text);
            }
        }
        (numbers.numbers != self.numbers).then_some(numbers)
    }

    /// Whether a note numbered again on each page is on none of `pages`, the notes each
    /// page holds, so that it keeps the number through its section: an endnote, or a
    /// footnote no page lays out.
    pub(crate) fn unplaced(&self, pages: &[Vec<NoteId>]) -> bool {
        let placed: HashSet<NoteId> = pages.iter().flatten().copied().collect();
        self.paged.keys().any(|note| !placed.contains(note))
    }

    /// The text of the number of `note`; `None` for a note no reference numbers.
    pub(crate) fn text(&self, note: NoteId) -> Option<&str> {
        self.numbers.get(&note).map(String::as_str)
    }

    /// Whether `note` is referenced in a header or footer, so that a reference to it that
    /// shows no number is reported.
    pub(crate) fn framed(&self, note: NoteId) -> bool {
        self.framed.contains(&note)
    }
}

/// The next number of each kind of note, and how many of each are numbered so far.
#[derive(Default)]
struct Counters {
    footnote: Option<u32>,
    endnote: Option<u32>,
    footnotes: u32,
    endnotes: u32,
}

impl Counters {
    fn get(&self, kind: NoteKind) -> Option<u32> {
        match kind {
            NoteKind::Endnote => self.endnote,
            _ => self.footnote,
        }
    }

    fn set(&mut self, kind: NoteKind, value: u32) {
        match kind {
            NoteKind::Endnote => self.endnote = Some(value),
            _ => self.footnote = Some(value),
        }
    }

    /// How many notes of `kind` are numbered so far.
    fn counted(&self, kind: NoteKind) -> u32 {
        match kind {
            NoteKind::Endnote => self.endnotes,
            _ => self.footnotes,
        }
    }

    /// Takes the next number of `kind`, from `start` where none is set, and counts it.
    fn take(&mut self, kind: NoteKind, start: u32) -> u32 {
        let number = self.get(kind).unwrap_or(start);
        self.set(kind, number.saturating_add(1));
        match kind {
            NoteKind::Endnote => self.endnotes = self.endnotes.saturating_add(1),
            _ => self.footnotes = self.footnotes.saturating_add(1),
        }
        number
    }
}

/// Numbers the references of one section's blocks.
struct Walk<'a, 'd> {
    document: &'d Document,
    section: &'d crate::ir::Section,
    /// The section's index.
    index: usize,
    numbers: &'a mut NoteNumbers,
    next: &'a mut Counters,
    /// Whether the blocks walked are a header's or footer's, whose references are marked
    /// instead of numbered.
    frames: bool,
}

impl Walk<'_, '_> {
    fn blocks(&mut self, blocks: &[Block], depth: usize) {
        for block in blocks {
            match block {
                Block::Paragraph(paragraph) => self.paragraph(paragraph),
                Block::Table(table) if depth < MAX_TABLE_DEPTH => {
                    for cell in table.rows().iter().flat_map(|row| row.cells()) {
                        self.blocks(cell.blocks(), depth + 1);
                    }
                }
                Block::Table(_) => {}
            }
        }
    }

    fn paragraph(&mut self, paragraph: &Paragraph) {
        self.inlines(paragraph.inlines(), true);
    }

    /// Numbers the references of `inlines`, or marks them in headers and footers, those of
    /// the stored result of a field the layout does not compute too, one field deep, as
    /// the itemizer lays results out. A text box's references are passed over, as Word
    /// ignores them.
    fn inlines(&mut self, inlines: &[Inline], results: bool) {
        for inline in inlines {
            match inline {
                Inline::NoteReference {
                    note,
                    custom_mark,
                    style,
                } if !custom_mark => {
                    if self.frames {
                        self.numbers.framed.insert(*note);
                    } else {
                        self.reference(*note, *style);
                    }
                }
                Inline::Field(field) if results && !crate::itemize::computed(field.kind()) => {
                    self.inlines(field.result(), false);
                }
                _ => {}
            }
        }
    }

    /// Numbers `note` at its first reference, whose style `style` gives the language of
    /// a number in words: a hypothesis, as fields take theirs, until a Word reference
    /// shows a note number in another language (#546).
    fn reference(&mut self, note: NoteId, style: RunStyleId) {
        if self.numbers.numbers.contains_key(&note) {
            return;
        }
        let Some(kind) = self.document.note(note).map(crate::ir::Note::kind) else {
            return;
        };
        let props = match kind {
            NoteKind::Endnote => self.section.endnotes(),
            _ => self.section.footnotes(),
        };
        let number = self.next.take(kind, u32::from(props.start()));
        let language = self
            .document
            .run_style(style)
            .and_then(|style| style.language().latin());
        if props.restart() == NoteRestart::EachPage {
            self.numbers.paged.insert(
                note,
                Paged {
                    section: self.index,
                    start: u32::from(props.start()),
                    format: props.format().clone(),
                    language: language.map(str::to_owned),
                },
            );
        }
        let (text, departures) = number_text(number, props.format(), language);
        self.numbers.departures.add(departures);
        self.numbers.numbers.insert(note, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        Cell, Field, FieldKind, HeaderFooterSet, InlineObject, Note, NoteProps, NumberFormat,
        ObjectContent, PageSetup, Row, RunStyle, RunStyleId, Section, Size, Table, TextBox,
    };
    use officewright_core::Pt;

    fn holding(style: RunStyleId, inlines: Vec<Inline>) -> Block {
        Paragraph::new(style).with_inlines(inlines).into()
    }

    #[test]
    fn notes_are_numbered_in_the_order_of_their_references() {
        let mut document = Document::default();
        let style = document.add_run_style(RunStyle::default());
        let mut note = |kind| document.add_note(Note::new(kind, []));
        let first = note(NoteKind::Footnote);
        let second = note(NoteKind::Footnote);
        let third = note(NoteKind::Footnote);
        let end = note(NoteKind::Endnote);
        let custom = note(NoteKind::Footnote);
        let boxed = note(NoteKind::Footnote);
        let boxed_in_field = note(NoteKind::Footnote);
        let paged = note(NoteKind::Footnote);
        let headed = note(NoteKind::Footnote);
        let headed_in_field = note(NoteKind::Footnote);
        let headed_in_box = note(NoteKind::Footnote);
        let footed = note(NoteKind::Endnote);
        let evened = note(NoteKind::Footnote);
        let reference = |note, custom| Inline::note_reference(note, custom, style);
        // The second note's reference comes first, in a table; the third's in the stored
        // result of a field; a reference to the first again takes no number, nor does one
        // in the result of a PAGE field, which the layout computes.
        let cell = Cell::default().with_blocks([holding(style, vec![reference(second, false)])]);
        let table: Block = Table::default()
            .with_rows([Row::default().with_cells([cell])])
            .into();
        let field = Field::new(FieldKind::Other("QUOTE".into()), style)
            .with_result(vec![reference(third, false)]);
        let page = Field::new(FieldKind::Page, style).with_result(vec![reference(paged, false)]);
        let quoting =
            |note| Field::new(FieldKind::Other("QUOTE".into()), style).with_result(vec![note]);
        let in_box = |inlines| {
            Inline::Object(InlineObject::new(
                ObjectContent::text_box(TextBox::new().with_blocks([holding(style, inlines)])),
                Size::new(Pt(10.0), Pt(10.0)),
            ))
        };
        let boxes = in_box(vec![
            reference(boxed, false),
            Inline::Field(quoting(reference(boxed_in_field, false))),
        ]);
        let paragraph = holding(
            style,
            vec![
                Inline::Field(page),
                reference(first, false),
                Inline::Field(field),
                reference(end, false),
                reference(custom, true),
                boxes,
                reference(first, false),
            ],
        );
        // References in a header, in a field's result there and in its text box, in a
        // footer for the first page and in a header for even pages.
        let header = holding(
            style,
            vec![
                reference(headed, false),
                Inline::Field(quoting(reference(headed_in_field, false))),
                in_box(vec![reference(headed_in_box, false)]),
            ],
        );
        let footer = holding(style, vec![reference(footed, false)]);
        let even = holding(style, vec![reference(evened, false)]);
        document.add_section(
            Section::new(PageSetup::default())
                .with_headers(
                    HeaderFooterSet::default()
                        .with_primary([header])
                        .with_even([even]),
                )
                .with_footers(HeaderFooterSet::default().with_first([footer]))
                .with_blocks([table, paragraph]),
        );
        let numbers = NoteNumbers::new(&document);
        // Footnotes 1, 2, 3 in text order; the endnote i, by its own default numbering; a
        // custom mark, a reference in a text box and one in a header or footer take no
        // number.
        assert_eq!(numbers.text(second), Some("1"));
        assert_eq!(numbers.text(first), Some("2"));
        assert_eq!(numbers.text(third), Some("3"));
        assert_eq!(numbers.text(end), Some("i"));
        assert_eq!(numbers.text(custom), None);
        let unnumbered = [boxed, boxed_in_field, paged, headed, headed_in_field];
        assert!(unnumbered.iter().all(|&id| numbers.text(id).is_none()));
        assert_eq!(numbers.text(headed_in_box), None);
        assert_eq!(numbers.text(footed), None);
        assert_eq!(numbers.text(evened), None);
        // The references of headers and footers are marked, those of their fields' results
        // too, but not those of their text boxes, nor any of the body's.
        let framed = [headed, headed_in_field, footed, evened];
        assert!(framed.iter().all(|&id| numbers.framed(id)));
        let unframed = [
            first,
            second,
            end,
            custom,
            boxed,
            boxed_in_field,
            headed_in_box,
        ];
        assert!(!unframed.iter().any(|&id| numbers.framed(id)));
        assert!(numbers.departures == Departures::default() && !numbers.unplaced(&[]));
    }

    #[test]
    fn note_numbers_are_in_the_language_of_their_reference_and_say_how_they_depart() {
        use crate::ir::Language;
        let numbers = |format: NumberFormat, start: u16, tag: &str| {
            let mut document = Document::default();
            let style = document.add_run_style(
                RunStyle::default().with_language(Language::default().with_latin(Some(tag.into()))),
            );
            let note = document.add_note(Note::new(NoteKind::Footnote, []));
            let section = Section::new(PageSetup::default())
                .with_footnotes(NoteProps::footnotes().with_start(start).with_format(format))
                .with_blocks([holding(
                    style,
                    vec![Inline::note_reference(note, false, style)],
                )]);
            document.add_section(section);
            let numbers = NoteNumbers::new(&document);
            (numbers.text(note).map(str::to_owned), numbers.departures)
        };
        let none = Departures::default();
        assert_eq!(
            numbers(NumberFormat::OrdinalText, 27, "de-DE"),
            (Some("Siebenundzwanzigste".to_owned()), none)
        );
        assert_eq!(
            numbers(NumberFormat::OrdinalText, 27, "fr-FR"),
            (
                Some("twenty-seventh".to_owned()),
                Departures {
                    language: true,
                    ..none
                }
            )
        );
        assert_eq!(
            numbers(NumberFormat::UpperLetter, 781, "en-US"),
            (
                Some("781".to_owned()),
                Departures {
                    unrepresentable: true,
                    ..none
                }
            )
        );
    }

    #[test]
    fn footnotes_numbered_on_count_no_endnotes_before_them() {
        // Section 0 references an endnote; section 1 numbers its footnotes on from 1: the
        // first footnote of the document shows 1, the endnotes being counted apart.
        let mut document = Document::default();
        let style = document.add_run_style(RunStyle::default());
        let endnote = document.add_note(Note::new(NoteKind::Endnote, []));
        let footnote = document.add_note(Note::new(NoteKind::Footnote, []));
        for note in [endnote, footnote] {
            document.add_section(Section::new(PageSetup::default()).with_blocks([holding(
                style,
                vec![Inline::note_reference(note, false, style)],
            )]));
        }
        let numbers = NoteNumbers::new(&document);
        assert_eq!(numbers.text(endnote), Some("i"));
        assert_eq!(numbers.text(footnote), Some("1"));
    }

    #[test]
    fn notes_numbered_again_on_each_page_are_written_in_the_language_of_their_reference() {
        use crate::ir::Language;
        // A note in German, numbered again on each page in words: on its page it is the
        // first again, "Eins".
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default().with_language(Language::default().with_latin(Some("de-DE".into()))),
        );
        let notes: Vec<NoteId> = (0..2)
            .map(|_| document.add_note(Note::new(NoteKind::Footnote, [])))
            .collect();
        let paged = NoteProps::footnotes()
            .with_format(NumberFormat::CardinalText)
            .with_restart(NoteRestart::EachPage);
        document.add_section(
            Section::new(PageSetup::default())
                .with_footnotes(paged)
                .with_blocks([holding(
                    style,
                    notes
                        .iter()
                        .map(|&id| Inline::note_reference(id, false, style))
                        .collect(),
                )]),
        );
        let numbers = NoteNumbers::new(&document);
        let by_page = numbers
            .by_page(&[vec![notes[0]], vec![notes[1]]])
            .expect("new numbers");
        assert_eq!(by_page.text(notes[1]), Some("Eins"));
    }

    #[test]
    fn notes_numbered_again_on_each_page_count_on_their_pages() {
        // Section 0 numbers its footnotes again on each page, from 2, in lower-case
        // letters; section 1 does too, from 1, in decimal. On the first page, two notes of
        // section 0 and one of section 1, then one of section 0 on the second page; the
        // last note of section 1 is on no page.
        let mut document = Document::default();
        let style = document.add_run_style(RunStyle::default());
        let notes: Vec<NoteId> = (0..5)
            .map(|_| document.add_note(Note::new(NoteKind::Footnote, [])))
            .collect();
        let references = |notes: &[NoteId]| {
            holding(
                style,
                notes
                    .iter()
                    .map(|&id| Inline::note_reference(id, false, style))
                    .collect(),
            )
        };
        let paged = NoteProps::footnotes()
            .with_start(2)
            .with_format(NumberFormat::LowerLetter)
            .with_restart(NoteRestart::EachPage);
        document.add_section(
            Section::new(PageSetup::default())
                .with_footnotes(paged)
                .with_blocks([references(&notes[..3])]),
        );
        document.add_section(
            Section::new(PageSetup::default())
                .with_footnotes(NoteProps::footnotes().with_restart(NoteRestart::EachPage))
                .with_blocks([references(&notes[3..])]),
        );
        let numbers = NoteNumbers::new(&document);
        let texts = |numbers: &NoteNumbers| -> Vec<Option<String>> {
            notes
                .iter()
                .map(|&id| numbers.text(id).map(str::to_owned))
                .collect()
        };
        // Before any page: through each section, b to d, then 1 and 2.
        assert_eq!(
            texts(&numbers),
            ["b", "c", "d", "1", "2"].map(|t| Some(t.to_owned()))
        );
        // Two of section 0 and one of section 1 on the first page, one of section 0 on
        // the second: each section counts from its start on each page, apart from the
        // other, and the note on no page keeps its number.
        let by_page = numbers
            .by_page(&[vec![notes[0], notes[1], notes[3]], vec![notes[2]]])
            .expect("new numbers");
        assert_eq!(
            texts(&by_page),
            ["b", "c", "b", "1", "2"].map(|t| Some(t.to_owned()))
        );
        // The same pages again change nothing; a document numbering on gives none.
        assert_eq!(
            by_page.by_page(&[vec![notes[0], notes[1], notes[3]], vec![notes[2]]]),
            None
        );
        let mut plain = Document::default();
        let style = plain.add_run_style(RunStyle::default());
        let note = plain.add_note(Note::new(NoteKind::Footnote, []));
        plain.add_section(Section::new(PageSetup::default()).with_blocks([holding(
            style,
            vec![Inline::note_reference(note, false, style)],
        )]));
        assert_eq!(NoteNumbers::new(&plain).by_page(&[vec![note]]), None);
    }

    #[test]
    fn a_section_s_numbering_gives_the_start_the_format_and_where_numbers_start_again() {
        let mut document = Document::default();
        let style = document.add_run_style(RunStyle::default());
        let notes: Vec<NoteId> = (0..8)
            .map(|_| document.add_note(Note::new(NoteKind::Footnote, [])))
            .collect();
        let paragraph = |range: std::ops::Range<usize>| {
            let inlines = notes
                .get(range)
                .unwrap_or_default()
                .iter()
                .map(|&id| Inline::note_reference(id, false, style))
                .collect();
            holding(style, inlines)
        };
        let roman = NoteProps::footnotes()
            .with_start(5)
            .with_format(NumberFormat::UpperRoman);
        let restarting = NoteProps::footnotes()
            .with_start(3)
            .with_restart(NoteRestart::EachSection);
        let each_page = NoteProps::footnotes().with_restart(NoteRestart::EachPage);
        // V and VI; on through the next section, which numbers continuously from the same
        // start, VII and VIII; then from its start in a section that starts again: 3 and
        // 4; in a section that numbers continuously from 1 after it, the seventh note of
        // the document, 7, as Word numbers it; from its start in a section that starts
        // again on each page, 1.
        let sections = [
            (roman.clone(), 0..2),
            (roman, 2..4),
            (restarting, 4..6),
            (NoteProps::footnotes(), 6..7),
            (each_page.clone(), 7..8),
        ];
        for (props, range) in sections {
            let section = Section::new(PageSetup::default())
                .with_footnotes(props)
                .with_blocks([paragraph(range)]);
            document.add_section(section);
        }
        let numbers = NoteNumbers::new(&document);
        let texts: Vec<Option<&str>> = notes.iter().map(|&id| numbers.text(id)).collect();
        assert_eq!(
            texts,
            [
                Some("V"),
                Some("VI"),
                Some("VII"),
                Some("VIII"),
                Some("3"),
                Some("4"),
                Some("7"),
                Some("1")
            ]
        );
        // Starting again on each page is numbered through the section until a pass puts
        // the notes on their pages: a note of a kind that starts again so is on no page
        // until then, not a note of the other kind, nor none.
        let each_page_with = |restarting: NoteKind, referenced: Option<NoteKind>| {
            let mut document = Document::default();
            let style = document.add_run_style(RunStyle::default());
            let inlines = referenced
                .map(|kind| {
                    let id = document.add_note(Note::new(kind, []));
                    Inline::note_reference(id, false, style)
                })
                .into_iter()
                .collect();
            let section = Section::new(PageSetup::default());
            let section = match restarting {
                NoteKind::Endnote => {
                    section.with_endnotes(NoteProps::endnotes().with_restart(NoteRestart::EachPage))
                }
                _ => section.with_footnotes(each_page.clone()),
            };
            document.add_section(section.with_blocks([holding(style, inlines)]));
            NoteNumbers::new(&document).unplaced(&[])
        };
        let (footnote, endnote) = (NoteKind::Footnote, NoteKind::Endnote);
        assert!(each_page_with(footnote, Some(footnote)));
        assert!(each_page_with(endnote, Some(endnote)));
        assert!(!each_page_with(footnote, Some(endnote)));
        assert!(!each_page_with(endnote, Some(footnote)));
        assert!(!each_page_with(footnote, None));
    }
}
