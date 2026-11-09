//! The numbers of footnotes and endnotes.

use std::collections::HashMap;

use crate::ir::{
    Block, Document, Inline, MAX_TABLE_DEPTH, NoteId, NoteKind, NoteRestart, Paragraph,
};
use crate::numbers::number_text;

/// The number each referenced note shows, as text, and the formats and restarts of them
/// that the layout writes otherwise than the document says.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct NoteNumbers {
    numbers: HashMap<NoteId, String>,
    /// Whether a number's format is one the layout writes as decimal instead.
    pub(crate) decimal: bool,
    /// Whether a section restarts the numbers of its footnotes on each page, which the
    /// layout numbers through the section until it places footnotes on their pages.
    pub(crate) each_page: bool,
}

impl NoteNumbers {
    /// The numbers of the notes of `document`, in the order of their references in the
    /// text, each from its section's numbering of its kind (ECMA-376 Part 1, §17.11):
    /// from its start, again at each section for those that restart there. A reference
    /// with a custom mark (`w:customMarkFollows`) takes no number, a hypothesis until a
    /// Word render; nor does one in a text box, which Word ignores (MS-OI29500 2.1.305 a),
    /// nor a second reference to a note.
    pub(crate) fn new(document: &Document) -> Self {
        let mut numbers = Self::default();
        let mut next = Counters::default();
        for section in document.sections() {
            for (kind, props) in [
                (NoteKind::Footnote, section.footnotes()),
                (NoteKind::Endnote, section.endnotes()),
            ] {
                let restart =
                    props.restart() != NoteRestart::Continuous || next.get(kind).is_none();
                if restart {
                    next.set(kind, u32::from(props.start()));
                }
                if kind == NoteKind::Footnote && props.restart() == NoteRestart::EachPage {
                    numbers.each_page = true;
                }
            }
            let mut walk = Walk {
                document,
                section,
                numbers: &mut numbers,
                next: &mut next,
            };
            walk.blocks(section.blocks(), 0);
        }
        numbers
    }

    /// The text of the number of `note`; `None` for a note no reference numbers.
    pub(crate) fn text(&self, note: NoteId) -> Option<&str> {
        self.numbers.get(&note).map(String::as_str)
    }
}

/// The next number of each kind of note.
#[derive(Default)]
struct Counters {
    footnote: Option<u32>,
    endnote: Option<u32>,
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
}

/// Numbers the references of one section's blocks.
struct Walk<'a, 'd> {
    document: &'d Document,
    section: &'d crate::ir::Section,
    numbers: &'a mut NoteNumbers,
    next: &'a mut Counters,
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

    /// Numbers the references of `inlines`, those of the stored result of a field the
    /// layout does not compute too, one field deep, as the itemizer lays results out.
    fn inlines(&mut self, inlines: &[Inline], results: bool) {
        for inline in inlines {
            match inline {
                Inline::NoteReference {
                    note, custom_mark, ..
                } if !custom_mark => self.reference(*note),
                Inline::Field(field) if results && !crate::itemize::computed(field.kind()) => {
                    self.inlines(field.result(), false);
                }
                // Objects are not walked into: a text box's references are ignored, as Word
                // ignores them.
                _ => {}
            }
        }
    }

    fn reference(&mut self, note: NoteId) {
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
        let number = self.next.get(kind).unwrap_or(u32::from(props.start()));
        self.next.set(kind, number.saturating_add(1));
        let (text, decimal) = number_text(number, props.format());
        self.numbers.decimal |= decimal;
        self.numbers.numbers.insert(note, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        Cell, Field, FieldKind, InlineObject, Note, NoteProps, NumberFormat, ObjectContent,
        PageSetup, Row, RunStyle, RunStyleId, Section, Size, Table, TextBox,
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
        let reference = |note, custom| Inline::note_reference(note, custom, style);
        // The second note's reference comes first, in a table; the third's in the stored
        // result of a field; a reference to the first again takes no number.
        let cell = Cell::default().with_blocks([holding(style, vec![reference(second, false)])]);
        let table: Block = Table::default()
            .with_rows([Row::default().with_cells([cell])])
            .into();
        let field = Field::new(FieldKind::Other("QUOTE".into()), style)
            .with_result(vec![reference(third, false)]);
        let in_box = Inline::Object(InlineObject::new(
            ObjectContent::text_box(
                TextBox::new().with_blocks([holding(style, vec![reference(boxed, false)])]),
            ),
            Size::new(Pt(10.0), Pt(10.0)),
        ));
        let paragraph = holding(
            style,
            vec![
                reference(first, false),
                Inline::Field(field),
                reference(end, false),
                reference(custom, true),
                in_box,
                reference(first, false),
            ],
        );
        document.add_section(Section::new(PageSetup::default()).with_blocks([table, paragraph]));
        let numbers = NoteNumbers::new(&document);
        // Footnotes 1, 2, 3 in text order; the endnote i, by its own default numbering; a
        // custom mark and a reference in a text box take no number.
        assert_eq!(numbers.text(second), Some("1"));
        assert_eq!(numbers.text(first), Some("2"));
        assert_eq!(numbers.text(third), Some("3"));
        assert_eq!(numbers.text(end), Some("i"));
        assert_eq!(numbers.text(custom), None);
        assert_eq!(numbers.text(boxed), None);
        assert!(!numbers.decimal && !numbers.each_page);
    }

    #[test]
    fn a_section_s_numbering_gives_the_start_the_format_and_where_numbers_start_again() {
        let mut document = Document::default();
        let style = document.add_run_style(RunStyle::default());
        let notes: Vec<NoteId> = (0..6)
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
        // V and VI; on through the next section, which numbers continuously, VII and VIII;
        // then from its start in a section that starts again: 3 and 4.
        for (props, range) in [(roman.clone(), 0..2), (roman, 2..4), (restarting, 4..6)] {
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
                Some("4")
            ]
        );
        // Starting again on each page is numbered through the section, and noted.
        let mut document = Document::default();
        let style = document.add_run_style(RunStyle::default());
        let id = document.add_note(Note::new(NoteKind::Footnote, []));
        document.add_section(
            Section::new(PageSetup::default())
                .with_footnotes(NoteProps::footnotes().with_restart(NoteRestart::EachPage))
                .with_blocks([holding(
                    style,
                    vec![Inline::note_reference(id, false, style)],
                )]),
        );
        let numbers = NoteNumbers::new(&document);
        assert!(numbers.each_page);
        assert_eq!(numbers.text(id), Some("1"));
    }
}
