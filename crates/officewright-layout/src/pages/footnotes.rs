//! Footnotes on the pages of their references: laid out at the width of the body, under
//! the separator, at the bottom of the body or beneath its text (design 6.8).
//!
//! When the pager places a line, the footnotes it references and no page holds yet go
//! on its page with it, and their room is taken from the bottom of the body. The notes are
//! stacked as the paragraphs of a header are, with the space between them and the space
//! after the last, as Word's render of `E-footnote-two-notes.docx` shows (case 03: two
//! notes of 6 pt before and after end 6 pt above the bottom of the body, 6 pt apart and 6
//! pt under the separator). The space after the last note is kept only as far as the text
//! leaves room for it: like the space after the last paragraph of a page's text, it never
//! moves a line to the next page, and where the text takes it the notes end at the bottom
//! of the body. That, and that the space before the separator is not counted, are
//! hypotheses until a Word render.
//!
//! A footnote that does not fit whole below its line is cut, as Word's renders of
//! `E-footnote-long.docx` and `E-footnote-no-room.docx` show: what fits stays under the
//! line, down to the bottom of the body, and the rest goes on at the top of the next
//! page's notes, under the continuation separator, before the notes of that page's own
//! references. A note is cut between its parts, never within a table, or between the
//! lines of a paragraph where the paragraph's widow control lets it, which leaves at least
//! two of its lines on each side (`E-footnote-no-room.docx` cases 01 and 03 cut, 02 does
//! not); the continuation notice goes under the part cut (`E-footnote-long-notice.docx`).
//! The page then takes no more text, and the notes the line references after the one cut
//! go wholly on the next page. Hypotheses until a Word render: keep rules other than widow
//! control do not hold inside notes, the notice follows the part cut with its own space
//! before only, the rest of a note on the next page is cut again only where it does not fit
//! that page's whole body, its text taking what is left, and a blank page that an odd or
//! even section start puts in takes none of it.
//!
//! A line whose note cannot start on the page, where not even its first part fits, moves
//! to the next page with its notes in Word 2013 and later (mode 15: `E-footnote-no-room.docx`
//! cases 02 and 04). Word 2010 (mode 14) keeps the line where it is and puts the whole note
//! on the next page, under the continuation separator, and the page takes no more text (note
//! 21 of `business/form-ec-state-aid-notification-de` on its page 15, note 27 of `-el` on its
//! page 19, whose next line goes to page 20), and so do the older modes here. In those
//! renders the note that cannot start is the
//! first on its page in mode 15 and follows others in mode 14, so whether the mode or the
//! notes before it decide is a hypothesis until a Word render. Where nothing moves the line
//! further, on a page that holds nothing else, its notes that cannot start there go on the
//! next page in every mode.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use officewright_core::Pt;

use super::{
    Content, EPSILON, Flowed, FramePart, Page, PageSink, Pager, Walk, page_box,
    report_list_formats, stack_space, trailing_space,
};
use crate::display::{DisplayItem, Float, PageBox, Painting, Point};
use crate::error::Result;
use crate::ir::{
    Block, CompatMode, Inline, NoteId, NoteKind, NotePosition, PageSetup, Paragraph, Table,
};
use crate::itemize::{FieldValues, Item};
use crate::lines::{PieceKind, inline_at};
use crate::lists::Counters;
use crate::paragraph::ParagraphLayout;
use crate::report::{Location, ReportKind, Step};
use crate::tables;

/// A place in the content of a note: before line `line` of part `part`, where a table has
/// only line 0, or, past the last part, its end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct At {
    part: usize,
    line: usize,
}

/// A footnote's content laid out, or a separator's or the continuation notice's: its
/// paragraphs and tables, each with its top from the top of the first, and the height
/// from the top of the first to the bottom of the last.
pub(super) struct NoteLayout<'d> {
    parts: Vec<(Flowed<'d>, FramePart, f64)>,
    height: f64,
}

impl<'d> NoteLayout<'d> {
    fn first(&self) -> Option<&Flowed<'d>> {
        self.parts.first().map(|(part, ..)| part)
    }

    fn last(&self) -> Option<&Flowed<'d>> {
        self.parts.last().map(|(part, ..)| part)
    }

    /// Its end.
    fn end(&self) -> At {
        At {
            part: self.parts.len(),
            line: 0,
        }
    }

    /// The top of `at` from the top of the content.
    fn top(&self, at: At) -> f64 {
        match self.parts.get(at.part) {
            Some((_, FramePart::Paragraph(layout, _), top)) => {
                let above: f64 = layout
                    .lines
                    .iter()
                    .take(at.line)
                    .map(|line| line.height.0)
                    .sum();
                top + above
            }
            Some((_, FramePart::Table(_), top)) => *top,
            None => self.height,
        }
    }

    /// The bottom of what ends at `at`: of the part before it where it starts a part.
    fn bottom(&self, at: At) -> f64 {
        match at
            .part
            .checked_sub(1)
            .filter(|_| at.line == 0)
            .and_then(|before| self.parts.get(before))
        {
            Some((_, laid, top)) => top + laid.height(),
            None => self.top(at),
        }
    }

    /// The height of what is between `from` and `to`.
    fn between(&self, from: At, to: At) -> f64 {
        if to <= from {
            0.0
        } else {
            (self.bottom(to) - self.top(from)).max(0.0)
        }
    }

    /// The part that what starts at `from` starts with, none where it goes on within one.
    fn starting(&self, from: At) -> Option<&Flowed<'d>> {
        self.parts
            .get(from.part)
            .filter(|_| from.line == 0)
            .map(|(part, ..)| part)
    }

    /// The part that what ends at `to` ends with, none where it ends within one.
    fn ending(&self, to: At) -> Option<&Flowed<'d>> {
        to.part
            .checked_sub(1)
            .filter(|_| to.line == 0)
            .and_then(|before| self.parts.get(before))
            .map(|(part, ..)| part)
    }

    /// The place one line of a paragraph past `from`, or past the table it starts.
    fn step(&self, from: At) -> At {
        match self.parts.get(from.part) {
            Some((_, FramePart::Paragraph(layout, _), _)) if from.line + 1 < layout.lines.len() => {
                At {
                    part: from.part,
                    line: from.line + 1,
                }
            }
            _ => At {
                part: from.part + 1,
                line: 0,
            },
        }
    }

    /// How far the content goes from `from` in `room`: to its end where the rest fits, else
    /// to before the first table or line that does not, or, where `widow` keeps the widow
    /// control of the paragraph cut, never leaving fewer than two of its lines on either
    /// side, or than one on this side where the paragraph goes on from before `from`, as the
    /// lines of the text are kept. `from` where nothing fits.
    fn cut(&self, from: At, room: f64, widow: bool) -> At {
        let start = self.top(from);
        for (index, (part, laid, top)) in self.parts.iter().enumerate().skip(from.part) {
            let first = if index == from.part { from.line } else { 0 };
            match laid {
                FramePart::Table(drawn) => {
                    if top + drawn.height.0 - start > room + EPSILON {
                        return At {
                            part: index,
                            line: 0,
                        };
                    }
                }
                FramePart::Paragraph(layout, _) => {
                    let mut y = self.top(At {
                        part: index,
                        line: first,
                    }) - start;
                    let rest = layout.lines.len().saturating_sub(first);
                    let mut fit = 0;
                    for line in layout.lines.iter().skip(first) {
                        if y + line.height.0 > room + EPSILON {
                            break;
                        }
                        y += line.height.0;
                        fit += 1;
                    }
                    if fit < rest {
                        let mut take = fit;
                        if widow && part.paragraph().is_some_and(|p| p.props().widow_control()) {
                            if rest - take < 2 {
                                take = rest.saturating_sub(2);
                            }
                            let least = if first == 0 { 2 } else { 1 };
                            if take < least {
                                take = 0;
                            }
                        }
                        return At {
                            part: index,
                            line: first + take,
                        };
                    }
                }
            }
        }
        self.end()
    }
}

/// What of a footnote is on a page: which note, its layout, the part from `from` to `to`,
/// which is none where the note goes wholly on the next page, whether its reference is on
/// a page before, its top and its bottom from the top of the notes' room, and the space
/// after it, which the room ends with when it is the last.
struct PageNote<'d> {
    note: NoteId,
    layout: Rc<NoteLayout<'d>>,
    from: At,
    to: At,
    continued: bool,
    top: f64,
    bottom: f64,
    after: f64,
}

impl PageNote<'_> {
    /// Whether none of what is left of the note is on the page: a note with no content
    /// is, with its separator.
    fn is_empty(&self) -> bool {
        self.to <= self.from && self.from < self.layout.end()
    }

    /// Whether the note ends on the page.
    fn ends(&self) -> bool {
        self.to >= self.layout.end()
    }
}

/// The footnotes on a page, in the order of their references, under the separator, the
/// rest of notes cut on the page before first, and under a note cut on the page the
/// continuation notice, with its top and the space after it.
#[derive(Default)]
pub(super) struct PageNotes<'d> {
    notes: Vec<PageNote<'d>>,
    separator: Option<Rc<NoteLayout<'d>>>,
    notice: Option<(Rc<NoteLayout<'d>>, f64, f64)>,
}

impl<'d> PageNotes<'d> {
    /// The last note drawn on the page.
    fn last_drawn(&self) -> Option<&PageNote<'d>> {
        self.notes.iter().rev().find(|note| !note.is_empty())
    }

    /// The room the notes take, the separator's included, and the space after the last;
    /// none without notes.
    pub(super) fn room(&self) -> f64 {
        match &self.notice {
            Some((notice, top, after)) => top + notice.height + after,
            None => self
                .last_drawn()
                .map_or(0.0, |note| note.bottom + note.after),
        }
    }

    /// The room the notes need: their room without the space after the last.
    pub(super) fn needed(&self) -> f64 {
        match &self.notice {
            Some((notice, top, _)) => top + notice.height,
            None => self.last_drawn().map_or(0.0, |note| note.bottom),
        }
    }

    /// Whether the page takes no more text: a note on it goes on to the next page.
    pub(super) fn closed(&self) -> bool {
        self.notes.iter().any(|note| !note.ends())
    }

    /// Whether its first notes go on from the page before.
    pub(super) fn continues(&self) -> bool {
        self.notes.first().is_some_and(|note| note.continued)
    }

    /// How many notes it holds.
    pub(super) fn len(&self) -> usize {
        self.notes.len()
    }

    /// Keeps the first `count` notes, and returns those of the others whose references
    /// are on the page.
    pub(super) fn truncate(&mut self, count: usize) -> Vec<NoteId> {
        let dropped = self
            .notes
            .drain(count.min(self.notes.len())..)
            .filter(|placed| !placed.continued)
            .map(|placed| placed.note)
            .collect();
        if !self.closed() {
            self.notice = None;
        }
        dropped
    }

    /// The notes it holds.
    pub(super) fn notes(&self) -> impl Iterator<Item = NoteId> + '_ {
        self.notes.iter().map(|placed| placed.note)
    }

    /// The notes whose references are on the page, those going on from the page before
    /// left out.
    pub(super) fn referenced(&self) -> impl Iterator<Item = NoteId> + '_ {
        self.notes
            .iter()
            .filter(|placed| !placed.continued)
            .map(|placed| placed.note)
    }

    /// What of its notes goes on to the next page.
    pub(super) fn going_on(&self) -> Vec<Waiting<'d>> {
        self.notes
            .iter()
            .filter(|placed| !placed.ends())
            .map(|placed| Waiting {
                note: placed.note,
                layout: Rc::clone(&placed.layout),
                from: placed.to,
                continued: true,
            })
            .collect()
    }

    /// The last note drawn, under which the next goes.
    fn tail(&self) -> Option<Tail<'d>> {
        self.last_drawn().map(|note| Tail {
            layout: Rc::clone(&note.layout),
            to: note.to,
            bottom: note.bottom,
        })
    }
}

/// A footnote, or the rest of one, that goes on a page: which, its layout, where what goes
/// starts, and whether its reference is on a page before.
#[derive(Clone)]
pub(super) struct Waiting<'d> {
    note: NoteId,
    layout: Rc<NoteLayout<'d>>,
    from: At,
    continued: bool,
}

/// The last note drawn in a page's notes: its layout, where what is drawn of it ends, and
/// its bottom from the top of the notes' room.
#[derive(Clone)]
struct Tail<'d> {
    layout: Rc<NoteLayout<'d>>,
    to: At,
    bottom: f64,
}

/// What of footnotes goes on a page: the notes, the separator above them where they are
/// the first drawn there, the continuation notice where one is cut, and the last drawn.
struct Plan<'d> {
    notes: Vec<PageNote<'d>>,
    separator: Option<Rc<NoteLayout<'d>>>,
    notice: Option<(Rc<NoteLayout<'d>>, f64, f64)>,
    tail: Option<Tail<'d>>,
}

/// How notes that do not fit go on a page: whether one that cannot start there may go
/// wholly on the next page (`defer`), and whether the first on the page puts at least a line
/// or a table there all the same (`force`), past the bottom where it is taller than that.
#[derive(Clone, Copy)]
struct Rules {
    defer: bool,
    force: bool,
}

/// The special notes of footnotes, from Word's footnote types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Special {
    Separator,
    ContinuationSeparator,
    ContinuationNotice,
}

/// The footnotes laid out so far in a pass, by note, while their page may still change:
/// each layout with the width it was laid out at, and the number of the page it was laid
/// out for when it has a `PAGE` field.
pub(super) type NoteLayouts<'d> = HashMap<NoteId, Vec<(u64, Option<u32>, Rc<NoteLayout<'d>>)>>;

/// The special notes laid out, by the bits of their width.
pub(super) type Specials<'d> = HashMap<(u64, Special), Rc<NoteLayout<'d>>>;

/// Whether `table` references a footnote in a cell, nested tables' included, where the
/// notes are not laid out yet.
pub(super) fn has_footnote(document: &crate::ir::Document, table: &Table, depth: usize) -> bool {
    table
        .rows()
        .iter()
        .flat_map(|row| row.cells())
        .flat_map(|cell| cell.blocks())
        .any(|block| match block {
            Block::Paragraph(paragraph) => references_footnote(document, paragraph.inlines(), true),
            Block::Table(table) => {
                depth < crate::ir::MAX_TABLE_DEPTH && has_footnote(document, table, depth + 1)
            }
        })
}

/// Whether `inlines` reference a footnote, and with `results` the stored results of their
/// fields that the layout does not compute, one field deep, as the itemizer lays them out.
fn references_footnote(document: &crate::ir::Document, inlines: &[Inline], results: bool) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::NoteReference { note, .. } => document
            .note(*note)
            .is_some_and(|note| note.kind() == NoteKind::Footnote),
        Inline::Field(field) if results && !crate::itemize::computed(field.kind()) => {
            references_footnote(document, field.result(), false)
        }
        _ => false,
    })
}

impl<'d, S: PageSink> Pager<'d, '_, '_, S> {
    /// The bottom of what the body text of the current page may take: its body's bottom,
    /// less the room its footnotes need, without the space after the last, and no lower
    /// than its text where a note on it goes on to the next page.
    pub(super) fn text_bottom(&self) -> Result<f64> {
        let notes = self.footnotes.last();
        let bottom = self.body()?.bottom.0 - notes.map_or(0.0, PageNotes::needed);
        Ok(if notes.is_some_and(PageNotes::closed) {
            bottom.min(self.cursor)
        } else {
            bottom
        })
    }

    /// The top of the room the footnotes of the current page take at the bottom of its
    /// body, the space after the last included, as far as nothing below its text takes it
    /// yet.
    pub(super) fn notes_top(&self) -> Result<f64> {
        let room = self.footnotes.last().map_or(0.0, PageNotes::room);
        Ok(self.body()?.bottom.0 - room)
    }

    /// Whether the current page holds notes that go on from the page before, which take
    /// its room before its text.
    pub(super) fn holds_continued_notes(&self) -> bool {
        self.footnotes.last().is_some_and(PageNotes::continues)
    }

    /// The footnotes that line `line` of `layout`, the layout of `paragraph`, references
    /// and no page holds yet, in order.
    fn line_footnotes(
        &self,
        paragraph: &Paragraph,
        layout: &ParagraphLayout,
        line: usize,
    ) -> Vec<NoteId> {
        let mut notes = Vec::new();
        let mut seen = HashSet::new();
        let Some(line) = layout.lines.get(line) else {
            return notes;
        };
        for piece in &line.pieces {
            let PieceKind::Object { item, height: None } = piece.kind else {
                continue;
            };
            let Some(Item::NoteReference(path, _)) = layout.itemized.items.get(item) else {
                continue;
            };
            let Some(Inline::NoteReference { note, .. }) = inline_at(paragraph.inlines(), path)
            else {
                continue;
            };
            let footnote = self
                .document
                .note(*note)
                .is_some_and(|ir| ir.kind() == NoteKind::Footnote);
            if footnote && !self.placed.contains(note) && seen.insert(*note) {
                notes.push(*note);
            }
        }
        notes
    }

    /// `notes`, referenced in section `section`, waiting whole for a page.
    fn waiting(&mut self, notes: Vec<NoteId>, section: usize) -> Result<Vec<Waiting<'d>>> {
        notes
            .into_iter()
            .map(|note| {
                Ok(Waiting {
                    note,
                    layout: self.note_layout(note, section)?,
                    from: At::default(),
                    continued: false,
                })
            })
            .collect()
    }

    /// How many of the lines of `layout`, the layout of paragraph `paragraph` of section
    /// `section`, from `first` on fit on the current page from `top` with the footnotes
    /// they reference, and whether the last of those ends in a page or column break. The
    /// last may be one whose notes are cut, or, before Word 2013, go wholly on the next
    /// page: the page takes no line after it.
    pub(super) fn fitting_with_notes(
        &mut self,
        (section, paragraph): (usize, &Paragraph),
        layout: &ParagraphLayout,
        first: usize,
        top: f64,
    ) -> Result<(usize, bool)> {
        let bottom = self.body()?.bottom.0;
        let Some(page) = self.footnotes.last() else {
            return Ok((0, false));
        };
        if page.closed() {
            return Ok((0, false));
        }
        let mut tail = page.tail();
        let rules = Rules {
            defer: self.document.settings().compat_mode() < CompatMode::Word2013,
            force: false,
        };
        let mut taken: HashSet<NoteId> = HashSet::new();
        let mut y = top;
        let mut count = 0;
        for (index, line) in layout.lines.iter().enumerate().skip(first) {
            let end = tail.as_ref().map_or(0.0, |tail| tail.bottom);
            if y + line.height.0 > bottom - end + EPSILON {
                break;
            }
            let notes: Vec<NoteId> = self
                .line_footnotes(paragraph, layout, index)
                .into_iter()
                .filter(|note| taken.insert(*note))
                .collect();
            let mut closes = false;
            if !notes.is_empty() {
                let waiting = self.waiting(notes, section)?;
                let room = bottom - y - line.height.0;
                let Some(plan) = self.plan(tail.clone(), waiting, room, section, rules)? else {
                    break;
                };
                closes = plan.notes.iter().any(|note| !note.ends());
                tail = plan.tail;
            }
            y += line.height.0;
            count += 1;
            let breaks = matches!(
                line.end,
                crate::lines::LineEnd::PageBreak | crate::lines::LineEnd::ColumnBreak
            );
            if breaks || closes {
                return Ok((count, breaks));
            }
        }
        Ok((count, false))
    }

    /// The space between `above`, the last part of what is above, none where that ends
    /// within a paragraph or nothing is, and `below`, the first part of what is below, none
    /// where that goes on within a paragraph.
    fn gap(&self, above: Option<&Flowed<'_>>, below: Option<&Flowed<'_>>) -> f64 {
        match (above, below) {
            (Some(above), Some(below)) => stack_space(self.document, Some(above), below),
            (Some(above), None) => trailing_space(self.document, above),
            (None, _) => 0.0,
        }
    }

    /// Where `waiting` go on the current page, the notes of section `section`, under
    /// `tail`, the last note drawn there, and within `room` from the top of the notes'
    /// room: each whole that fits, else cut, the notes after the one cut wholly on the
    /// next page, as are those that cannot start where `rules` let them; `None` where one
    /// cannot start and they do not.
    fn plan(
        &mut self,
        mut tail: Option<Tail<'d>>,
        waiting: Vec<Waiting<'d>>,
        room: f64,
        section: usize,
        rules: Rules,
    ) -> Result<Option<Plan<'d>>> {
        let mut plan = Plan {
            notes: Vec::with_capacity(waiting.len()),
            separator: None,
            notice: None,
            tail: None,
        };
        // Whether a note goes on to the next page: those after it go there wholly.
        let mut closed = false;
        for Waiting {
            note,
            layout,
            from,
            continued,
        } in waiting
        {
            let base = tail.as_ref().map_or(0.0, |tail| tail.bottom);
            if closed {
                plan.notes.push(PageNote {
                    note,
                    layout,
                    from,
                    to: from,
                    continued,
                    top: base,
                    bottom: base,
                    after: 0.0,
                });
                continue;
            }
            let (top, separator) = match &tail {
                Some(tail) => (
                    tail.bottom + self.gap(tail.layout.ending(tail.to), layout.starting(from)),
                    None,
                ),
                None => {
                    let special = if continued {
                        Special::ContinuationSeparator
                    } else {
                        Special::Separator
                    };
                    let separator = self.special_layout(special, section)?;
                    let gap = self.gap(separator.last(), layout.starting(from));
                    (separator.height + gap, Some(separator))
                }
            };
            let end = layout.end();
            let mut to = end;
            if top + layout.between(from, end) > room + EPSILON {
                let notice = self.special_layout(Special::ContinuationNotice, section)?;
                let before = notice
                    .first()
                    .map_or(0.0, |first| stack_space(self.document, None, first));
                let notice_room = notice.first().map_or(0.0, |_| before + notice.height);
                let left = room - top - notice_room;
                to = layout.cut(from, left, true);
                if to == from && rules.force && tail.is_none() {
                    to = layout.cut(from, left, false);
                    if to == from {
                        to = layout.step(from);
                    }
                    if top + layout.between(from, to) > room + EPSILON {
                        self.report_note_past_bottom(note);
                    }
                }
                if to == from && !rules.defer {
                    return Ok(None);
                }
                if from < to && to < end && notice.first().is_some() {
                    let after = notice
                        .last()
                        .map_or(0.0, |last| trailing_space(self.document, last));
                    let bottom = top + layout.between(from, to);
                    plan.notice = Some((notice, bottom + before, after));
                }
            }
            closed = to < end;
            if to <= from && from < end {
                plan.notes.push(PageNote {
                    note,
                    layout,
                    from,
                    to: from,
                    continued,
                    top: base,
                    bottom: base,
                    after: 0.0,
                });
                continue;
            }
            let bottom = top + layout.between(from, to);
            let after = match layout.last() {
                Some(last) if to == end => trailing_space(self.document, last),
                _ => 0.0,
            };
            plan.separator = plan.separator.or(separator);
            tail = Some(Tail {
                layout: Rc::clone(&layout),
                to,
                bottom,
            });
            plan.notes.push(PageNote {
                note,
                layout,
                from,
                to,
                continued,
                top,
                bottom,
                after,
            });
        }
        plan.tail = tail;
        Ok(Some(plan))
    }

    /// Puts `plan` on the current page; what of its notes goes on waits for the next.
    fn apply(&mut self, plan: Plan<'d>) {
        let Some(page) = self.footnotes.last_mut() else {
            return;
        };
        if plan.separator.is_some() {
            page.separator = plan.separator;
        }
        if plan.notice.is_some() {
            page.notice = plan.notice;
        }
        for placed in plan.notes {
            if !placed.continued {
                self.placed.insert(placed.note);
            }
            if !placed.ends() {
                self.pending.push(Waiting {
                    note: placed.note,
                    layout: Rc::clone(&placed.layout),
                    from: placed.to,
                    continued: true,
                });
            }
            page.notes.push(placed);
        }
    }

    /// Puts on the current page the footnotes that `lines` of `layout`, the layout of
    /// paragraph `paragraph` of section `section`, reference and no page holds yet, under
    /// the lines, which end at `end`: whole where they fit, else cut or wholly on the next
    /// page.
    pub(super) fn add_footnotes(
        &mut self,
        (section, paragraph): (usize, &Paragraph),
        layout: &ParagraphLayout,
        lines: std::ops::Range<usize>,
        end: f64,
    ) -> Result<()> {
        let mut seen = HashSet::new();
        let mut notes = Vec::new();
        for line in lines {
            for note in self.line_footnotes(paragraph, layout, line) {
                if seen.insert(note) {
                    notes.push(note);
                }
            }
        }
        if notes.is_empty() {
            return Ok(());
        }
        let waiting = self.waiting(notes, section)?;
        let room = self.body()?.bottom.0 - end;
        let tail = self.footnotes.last().and_then(PageNotes::tail);
        let rules = Rules {
            defer: true,
            force: false,
        };
        if let Some(plan) = self.plan(tail, waiting, room, section, rules)? {
            self.apply(plan);
        }
        Ok(())
    }

    /// Puts at the top of the notes of the current page, a page of section `section`,
    /// what of the notes of the page before goes on: as much as its body holds, and at
    /// least a line or a table.
    pub(super) fn continue_footnotes(&mut self, section: usize) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let waiting = std::mem::take(&mut self.pending);
        let body = self.body()?;
        let rules = Rules {
            defer: true,
            force: true,
        };
        let room = body.bottom.0 - body.top.0;
        if let Some(plan) = self.plan(None, waiting, room, section, rules)? {
            self.apply(plan);
        }
        Ok(())
    }

    /// Takes again, after going back to a checkpoint, what of the notes of the current
    /// page goes on to the next.
    pub(super) fn reset_continued_footnotes(&mut self) {
        self.pending = self
            .footnotes
            .last()
            .map(PageNotes::going_on)
            .unwrap_or_default();
    }

    /// The content of footnote `note`, referenced in section `section`, laid out at the
    /// width of the current page's body, with its number: the layout of it laid out
    /// before at that width, unless it has a `PAGE` field, which is laid out again for each
    /// page.
    fn note_layout(&mut self, note: NoteId, section: usize) -> Result<Rc<NoteLayout<'d>>> {
        let width = self.body()?.width.0.to_bits();
        let laid_out = |page: Option<u32>, layouts: &NoteLayouts<'d>| {
            layouts.get(&note).and_then(|laid| {
                laid.iter()
                    .find(|(at, on, _)| (*at, *on) == (width, page))
                    .map(|(.., laid)| Rc::clone(laid))
            })
        };
        // A note laid out for no page has no `PAGE` field.
        if let Some(laid) = laid_out(None, &self.note_layouts) {
            return Ok(laid);
        }
        let blocks: &'d [Block] = self.document.note(note).map_or(&[], |ir| ir.blocks());
        let page = super::field_kinds(blocks, 0)
            .0
            .then(|| self.field_values(section).page);
        if let Some(laid) = laid_out(page, &self.note_layouts) {
            return Ok(laid);
        }
        let mut location = Location::default();
        location.push(Step::Field("notes"));
        location.push(Step::Index(note.index()));
        location.push(Step::Field("blocks"));
        let laid = Rc::new(self.lay_out_note(blocks, location, section, Some(note))?);
        self.note_layouts
            .entry(note)
            .or_default()
            .push((width, page, Rc::clone(&laid)));
        Ok(laid)
    }

    /// Special note `special` of footnotes laid out at the width of the current page's
    /// body, as the footnotes of section `section` show it.
    fn special_layout(&mut self, special: Special, section: usize) -> Result<Rc<NoteLayout<'d>>> {
        let key = (self.body()?.width.0.to_bits(), special);
        if let Some(laid) = self.specials.get(&key) {
            return Ok(Rc::clone(laid));
        }
        let separators = self.document.note_separators(NoteKind::Footnote);
        let (blocks, field) = match special {
            Special::Separator => (separators.separator(), "separator"),
            Special::ContinuationSeparator => (
                separators.continuation_separator(),
                "continuation_separator",
            ),
            Special::ContinuationNotice => {
                (separators.continuation_notice(), "continuation_notice")
            }
        };
        let mut location = Location::default();
        location.push(Step::Field("footnote_separators"));
        location.push(Step::Field(field));
        let laid = Rc::new(self.lay_out_note(blocks, location, section, None)?);
        self.specials.insert(key, Rc::clone(&laid));
        Ok(laid)
    }

    /// `blocks` of a note laid out one under the other with the space between them, at
    /// the width of the current page's body, with the field values of section `section`
    /// on it and the number of `note`. Their lists count from their start, as those of a
    /// header do: a hypothesis until a Word render.
    fn lay_out_note(
        &mut self,
        blocks: &'d [Block],
        mut location: Location,
        section: usize,
        note: Option<NoteId>,
    ) -> Result<NoteLayout<'d>> {
        let width = self.body()?.width;
        let mut flowed = Vec::new();
        let mut counters = Counters::default();
        let mut walk = Walk {
            document: self.document,
            section,
            location: &mut location,
            counters: &mut counters,
            out: &mut flowed,
        };
        walk.blocks(blocks)?;
        report_list_formats(&counters, self.fonts);
        let values = FieldValues {
            note,
            ..self.field_values(section)
        };
        let mut parts: Vec<(Flowed<'d>, FramePart, f64)> = Vec::new();
        let mut height = 0.0;
        for part in flowed {
            if let Some((above, ..)) = parts.last() {
                height += stack_space(self.document, Some(above), &part);
            }
            let laid = self.frame_part(&part, &values, width);
            let top = height;
            height += laid.height();
            parts.push((part, laid, top));
        }
        Ok(NoteLayout { parts, height })
    }

    /// Adds to `page` what its footnotes `notes` draw: at the bottom of its body, or
    /// beneath its text where its section places them so, under the separator; never
    /// above its text, so that the space after the last note gives way to the text, and
    /// a line or table of a note too tall for the page runs past the bottom of the body,
    /// where the pager reported it.
    pub(super) fn paint_footnotes(&mut self, page: &mut Page, notes: &PageNotes<'d>) {
        if notes.last_drawn().is_none() {
            return;
        }
        let text_bottom = page
            .fragments
            .iter()
            .map(|fragment| fragment.top.0 + fragment.height.0)
            .fold(page.body.top.0, f64::max);
        let section = self.document.sections().get(page.section);
        let position = section.map_or(NotePosition::PageBottom, |section| {
            section.footnotes().position()
        });
        let top = match position {
            NotePosition::BeneathText => text_bottom,
            _ => (page.body.bottom.0 - notes.room()).max(text_bottom),
        };
        let setup = section.map_or_else(PageSetup::default, |section| *section.page());
        let page_box = page_box(&setup, page.number);
        let x = page.body.left;
        let mut items = Vec::new();
        let mut floats = Vec::new();
        let mut paint = |layout: &NoteLayout<'d>, span: (At, At), y: f64| {
            let origin = Point { x, y: Pt(top + y) };
            self.paint_note(layout, span, origin, page_box, (&mut items, &mut floats));
        };
        if let Some(separator) = &notes.separator {
            paint(separator, (At::default(), separator.end()), 0.0);
        }
        for placed in notes.notes.iter().filter(|placed| !placed.is_empty()) {
            paint(&placed.layout, (placed.from, placed.to), placed.top);
        }
        if let Some((notice, y, _)) = &notes.notice {
            paint(notice, (At::default(), notice.end()), *y);
        }
        page.items.extend(items);
        page.floats.extend(floats);
    }

    /// Adds to `items` what `note` draws from `from` to `to`, the top left corner of that
    /// at `origin`, and to `floats` its floating objects.
    fn paint_note(
        &mut self,
        note: &NoteLayout<'d>,
        (from, to): (At, At),
        origin: Point,
        page: PageBox,
        (items, floats): (&mut Vec<DisplayItem>, &mut Vec<Float>),
    ) {
        let start = note.top(from);
        for (index, (part, laid, _)) in note.parts.iter().enumerate().skip(from.part) {
            if index > to.part || (index == to.part && to.line == 0) {
                break;
            }
            let first = if index == from.part { from.line } else { 0 };
            let at = Point {
                x: origin.x,
                y: Pt(origin.y.0
                    + note.top(At {
                        part: index,
                        line: first,
                    })
                    - start),
            };
            match (laid, &part.content) {
                (FramePart::Paragraph(layout, _), Content::Paragraph(paragraph, _)) => {
                    let last = if index == to.part {
                        to.line.min(layout.lines.len())
                    } else {
                        layout.lines.len()
                    };
                    tables::report_borders(paragraph, &part.location, self.fonts);
                    let mut painting = Painting {
                        document: self.document,
                        paragraph,
                        layout,
                        fonts: self.fonts,
                        location: &part.location,
                        background: None,
                        leaders: &mut self.leaders,
                        page: Some(page),
                        floats: Vec::new(),
                        // Hypothesis: in notes, as no reference shows them, bar tabs reach
                        // only through the lines.
                        bar_reach: (0.0, 0.0),
                    };
                    let lines = first..last;
                    painting.paint_shading(lines.clone(), at, items);
                    painting.paint(lines, at, items);
                    floats.extend(painting.floats);
                }
                (FramePart::Table(drawn), _) => {
                    let mut table = drawn.items.clone();
                    tables::shift(&mut table, at.x.0, at.y.0);
                    items.extend(table);
                }
                (FramePart::Paragraph(..), Content::Table(..)) => {}
            }
        }
    }

    /// Reports, once, a line or table of footnote `note` too tall for the body of the page
    /// that the rest of the note goes on to, which runs past its bottom.
    fn report_note_past_bottom(&mut self, note: NoteId) {
        let mut location = Location::default();
        location.push(Step::Field("notes"));
        location.push(Step::Index(note.index()));
        self.fonts.report_once(
            ReportKind::Overflow,
            &location,
            "a line or table of a footnote taller than the body of its page runs past its \
             bottom"
                .to_owned(),
        );
    }
}
