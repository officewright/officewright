//! Footnotes and endnotes as IR notes, each added when the text first refers to it, and
//! the special notes Word draws between the text and the notes.

use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::{Step, Translator};
use crate::model::NoteType;

impl Translator<'_> {
    /// The IR note a reference to the note `id` names, added the first time; none,
    /// listed each time, for a reference inside a note, which the IR does not hold, and
    /// for a note that does not exist or is not a normal note.
    pub(super) fn note_reference(&mut self, endnote: bool, id: i64) -> Option<ir::NoteId> {
        let name = if endnote { "endnote" } else { "footnote" };
        if self.in_note {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                format!("a reference to a {name} inside a note is left out")
            });
            return None;
        }
        let document = self.document;
        let note = if endnote {
            document.endnote(id)
        } else {
            document.footnote(id)
        };
        let Some(note) = note.filter(|note| note.note_type() == NoteType::Normal) else {
            let special = note.map(|note| note.note_type());
            self.leave_out(ReportKind::FeatureUnsupported, || match special {
                Some(NoteType::Separator) => {
                    format!("a reference to {name} {id}, which is the separator, is left out")
                }
                Some(NoteType::ContinuationSeparator) => format!(
                    "a reference to {name} {id}, which is the continuation separator, is left out"
                ),
                Some(NoteType::ContinuationNotice) => format!(
                    "a reference to {name} {id}, which is the continuation notice, is left out"
                ),
                _ => format!(
                    "a reference to {name} {id}, which the document does not have, is left out"
                ),
            });
            return None;
        };
        if let Some(&found) = self.notes.get(&(endnote, id)) {
            return Some(found);
        }
        // Places in a note are written from the document's list of notes, and its tables
        // nest from the note, not from where the reference is.
        let path = std::mem::replace(
            &mut self.path,
            vec![Step::Field("notes"), Step::Index(self.notes_added)],
        );
        let depth = std::mem::take(&mut self.table_depth);
        self.in_note = true;
        let blocks = self.blocks(note.blocks());
        self.in_note = false;
        self.table_depth = depth;
        self.path = path;
        let kind = if endnote {
            ir::NoteKind::Endnote
        } else {
            ir::NoteKind::Footnote
        };
        self.notes_added += 1;
        let found = self.ir.add_note(ir::Note::new(kind, blocks));
        self.notes.insert((endnote, id), found);
        Some(found)
    }

    /// The separators of footnotes and of endnotes: the first note of each special type.
    /// Without them, the IR's defaults draw what Word draws.
    pub(super) fn note_separators(&mut self) {
        for (endnote, kind, place) in [
            (false, ir::NoteKind::Footnote, "footnote_separators"),
            (true, ir::NoteKind::Endnote, "endnote_separators"),
        ] {
            let mut parts: [Option<Vec<ir::Block>>; 3] = [None, None, None];
            let document = self.document;
            for note in document.notes().filter(|note| note.is_endnote() == endnote) {
                let (slot, field) = match note.note_type() {
                    NoteType::Separator => (0, "separator"),
                    NoteType::ContinuationSeparator => (1, "continuation_separator"),
                    NoteType::ContinuationNotice => (2, "continuation_notice"),
                    NoteType::Normal => continue,
                };
                if parts.get(slot).is_some_and(Option::is_some) {
                    continue;
                }
                let path =
                    std::mem::replace(&mut self.path, vec![Step::Field(place), Step::Field(field)]);
                self.in_note = true;
                let blocks = self.blocks_in(None, note.blocks());
                self.in_note = false;
                self.path = path;
                if let Some(part) = parts.get_mut(slot) {
                    *part = Some(blocks);
                }
            }
            if parts.iter().all(Option::is_none) {
                continue;
            }
            let [separator, continuation, notice] = parts;
            let mut separators = self.ir.note_separators(kind).clone();
            if let Some(blocks) = separator {
                separators = separators.with_separator(blocks);
            }
            if let Some(blocks) = continuation {
                separators = separators.with_continuation_separator(blocks);
            }
            if let Some(blocks) = notice {
                separators = separators.with_continuation_notice(blocks);
            }
            self.ir.set_note_separators(kind, separators);
        }
    }
}
