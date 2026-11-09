//! Footnotes on the pages of their references: laid out at the width of the body, under
//! the separator, at the bottom of the body or beneath its text (design 6.8).
//!
//! When the pager places a line, the footnotes it references and no page holds yet go
//! on its page with it, and their room is taken from the bottom of the body: a line
//! whose footnotes do not fit below it moves to the next page with them. The notes are
//! stacked as the paragraphs of a header are, with the space between them; the space
//! before the separator and after the last note are not counted. Hypotheses until Word
//! renders `E-footnote-two-notes.docx` and `E-footnote-no-room.docx`: the spacing, and
//! that a note is never cut, which Word does where a note does not fit (15c).

use std::rc::Rc;

use officewright_core::Pt;

use super::{
    Content, EPSILON, Flowed, FramePart, Page, PageSink, Pager, Walk, page_box,
    report_list_formats, stack_space,
};
use crate::display::{DisplayItem, Float, PageBox, Painting, Point};
use crate::error::Result;
use crate::ir::{Block, Inline, NoteId, NoteKind, NotePosition, PageSetup, Paragraph, Table};
use crate::itemize::{FieldValues, Item};
use crate::lines::{PieceKind, inline_at};
use crate::lists::Counters;
use crate::paragraph::ParagraphLayout;
use crate::report::{Location, ReportKind, Step};
use crate::tables;

/// A footnote's content laid out, or the separator's: its paragraphs and tables, each with
/// its top from the top of the first, and the height from the top of the first to the
/// bottom of the last.
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
}

/// A footnote on a page: which, its layout, its top from the top of the notes' room, and
/// the room the notes take down to its bottom, the separator's included.
struct PageNote<'d> {
    note: NoteId,
    layout: Rc<NoteLayout<'d>>,
    top: f64,
    room: f64,
}

/// The footnotes on a page, in the order of their references, under the separator.
#[derive(Default)]
pub(super) struct PageNotes<'d> {
    notes: Vec<PageNote<'d>>,
    separator: Option<Rc<NoteLayout<'d>>>,
}

impl PageNotes<'_> {
    /// The room the notes take, the separator's included; none without notes.
    pub(super) fn room(&self) -> f64 {
        self.notes.last().map_or(0.0, |note| note.room)
    }

    /// How many notes it holds.
    pub(super) fn len(&self) -> usize {
        self.notes.len()
    }

    /// Keeps the first `count` notes.
    pub(super) fn truncate(&mut self, count: usize) {
        self.notes.truncate(count);
    }

    /// The notes it holds.
    pub(super) fn notes(&self) -> impl Iterator<Item = NoteId> + '_ {
        self.notes.iter().map(|placed| placed.note)
    }

    fn holds(&self, note: NoteId) -> bool {
        self.notes.iter().any(|placed| placed.note == note)
    }
}

/// The footnotes laid out so far in a pass: each by its note, the width it was laid out
/// at, and the number of the page it was laid out for when it has a `PAGE` field.
pub(super) type NoteLayouts<'d> =
    std::collections::HashMap<(NoteId, u64, Option<u32>), Rc<NoteLayout<'d>>>;

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
    /// less the room its footnotes take.
    pub(super) fn text_bottom(&self) -> Result<f64> {
        let room = self.footnotes.last().map_or(0.0, PageNotes::room);
        Ok(self.body()?.bottom.0 - room)
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
            if footnote && !notes.contains(note) && !self.placed(*note) {
                notes.push(*note);
            }
        }
        notes
    }

    /// Whether a page holds `note` already.
    fn placed(&self, note: NoteId) -> bool {
        self.notes_done.contains(&note) || self.footnotes.iter().any(|page| page.holds(note))
    }

    /// How many of the lines of `layout`, the layout of paragraph `paragraph` of section
    /// `section`, from `first` on fit on the current page from `top` with the footnotes
    /// they reference, and whether the last of those ends in a page or column break.
    pub(super) fn fitting_with_notes(
        &mut self,
        (section, paragraph): (usize, &Paragraph),
        layout: &ParagraphLayout,
        first: usize,
        top: f64,
    ) -> Result<(usize, bool)> {
        let bottom = self.body()?.bottom.0;
        let (mut room, mut tail) = match self.footnotes.last().and_then(|page| page.notes.last()) {
            Some(last) => (last.room, Some(Rc::clone(&last.layout))),
            None => (0.0, None),
        };
        let mut taken: Vec<NoteId> = Vec::new();
        let mut y = top;
        let mut count = 0;
        for (index, line) in layout.lines.iter().enumerate().skip(first) {
            for note in self.line_footnotes(paragraph, layout, index) {
                if taken.contains(&note) {
                    continue;
                }
                taken.push(note);
                let laid = self.note_layout(note, section)?;
                room = self.room_after(room, tail.as_deref(), &laid, section)?.1;
                tail = Some(laid);
            }
            if y + line.height.0 > bottom - room + EPSILON {
                break;
            }
            y += line.height.0;
            count += 1;
            if matches!(
                line.end,
                crate::lines::LineEnd::PageBreak | crate::lines::LineEnd::ColumnBreak
            ) {
                return Ok((count, true));
            }
        }
        Ok((count, false))
    }

    /// The top of `note` in the notes' room and the room down to its bottom, the notes
    /// taking `room` down to the bottom of `tail` before it, or nothing when it is the
    /// first, which goes under the separator of section `section`.
    fn room_after(
        &mut self,
        room: f64,
        tail: Option<&NoteLayout<'d>>,
        note: &NoteLayout<'d>,
        section: usize,
    ) -> Result<(f64, f64)> {
        let (room, gap) = match tail {
            Some(tail) => (
                room,
                note.first()
                    .map_or(0.0, |below| self.gap(tail.last(), below)),
            ),
            None => {
                let separator = self.separator_layout(section)?;
                let gap = note
                    .first()
                    .map_or(0.0, |below| self.gap(separator.last(), below));
                (separator.height, gap)
            }
        };
        let top = room + gap;
        Ok((top, top + note.height))
    }

    /// The space between `above` and `below` in a stack of notes.
    fn gap(&self, above: Option<&Flowed<'_>>, below: &Flowed<'_>) -> f64 {
        above.map_or(0.0, |above| stack_space(self.document, Some(above), below))
    }

    /// Puts on the current page the footnotes that `lines` of `layout`, the layout of
    /// paragraph `paragraph` of section `section`, reference and no page holds yet.
    pub(super) fn add_footnotes(
        &mut self,
        (section, paragraph): (usize, &Paragraph),
        layout: &ParagraphLayout,
        lines: std::ops::Range<usize>,
    ) -> Result<()> {
        for line in lines {
            for note in self.line_footnotes(paragraph, layout, line) {
                let laid = self.note_layout(note, section)?;
                let (room, tail) = match self.footnotes.last().and_then(|page| page.notes.last()) {
                    Some(last) => (last.room, Some(Rc::clone(&last.layout))),
                    None => (0.0, None),
                };
                let (top, room) = self.room_after(room, tail.as_deref(), &laid, section)?;
                let separator = match tail {
                    Some(_) => None,
                    None => Some(self.separator_layout(section)?),
                };
                if let Some(page) = self.footnotes.last_mut() {
                    if separator.is_some() {
                        page.separator = separator;
                    }
                    page.notes.push(PageNote {
                        note,
                        layout: laid,
                        top,
                        room,
                    });
                }
            }
        }
        Ok(())
    }

    /// The content of footnote `note`, referenced in section `section`, laid out at the
    /// width of the current page's body, with its number: the layout of it laid out
    /// before at that width, unless it has a `PAGE` field, which is laid out again for each
    /// page.
    fn note_layout(&mut self, note: NoteId, section: usize) -> Result<Rc<NoteLayout<'d>>> {
        let width = self.body()?.width.0.to_bits();
        // A note laid out for no page has no `PAGE` field.
        if let Some(laid) = self.note_layouts.get(&(note, width, None)) {
            return Ok(Rc::clone(laid));
        }
        let blocks: &'d [Block] = self.document.note(note).map_or(&[], |ir| ir.blocks());
        let page = super::field_kinds(blocks, 0)
            .0
            .then(|| self.field_values(section).page);
        let key = (note, width, page);
        if let Some(laid) = self.note_layouts.get(&key) {
            return Ok(Rc::clone(laid));
        }
        let mut location = Location::default();
        location.push(Step::Field("notes"));
        location.push(Step::Index(note.index()));
        location.push(Step::Field("blocks"));
        let laid = Rc::new(self.lay_out_note(blocks, location, section, Some(note))?);
        self.note_layouts.insert(key, Rc::clone(&laid));
        Ok(laid)
    }

    /// The separator of footnotes laid out at the width of the current page's body, as
    /// the footnotes of section `section` show it.
    fn separator_layout(&mut self, section: usize) -> Result<Rc<NoteLayout<'d>>> {
        let width = self.body()?.width.0.to_bits();
        if let Some(laid) = self.separators.get(&width) {
            return Ok(Rc::clone(laid));
        }
        let blocks = self
            .document
            .note_separators(NoteKind::Footnote)
            .separator();
        let mut location = Location::default();
        location.push(Step::Field("footnote_separators"));
        location.push(Step::Field("separator"));
        let laid = Rc::new(self.lay_out_note(blocks, location, section, None)?);
        self.separators.insert(width, Rc::clone(&laid));
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
    /// above its text, so that notes too tall for the page run past the bottom of the
    /// body, where the pager reported them.
    pub(super) fn paint_footnotes(&mut self, page: &mut Page, notes: &PageNotes<'d>) {
        if notes.notes.is_empty() {
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
        if let Some(separator) = &notes.separator {
            let origin = Point { x, y: Pt(top) };
            self.paint_note(separator, origin, page_box, (&mut items, &mut floats));
        }
        for placed in &notes.notes {
            let origin = Point {
                x,
                y: Pt(top + placed.top),
            };
            self.paint_note(&placed.layout, origin, page_box, (&mut items, &mut floats));
        }
        page.items.extend(items);
        page.floats.extend(floats);
    }

    /// Adds to `items` what `note` draws, its top left corner at `origin`, and to `floats`
    /// its floating objects.
    fn paint_note(
        &mut self,
        note: &NoteLayout<'d>,
        origin: Point,
        page: PageBox,
        (items, floats): (&mut Vec<DisplayItem>, &mut Vec<Float>),
    ) {
        for (part, laid, top) in &note.parts {
            let at = Point {
                x: origin.x,
                y: Pt(origin.y.0 + top),
            };
            match (laid, &part.content) {
                (FramePart::Paragraph(layout, _), Content::Paragraph(paragraph, _)) => {
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
                    let lines = 0..layout.lines.len();
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

    /// Reports, once, footnotes that run past the bottom of the body of the page of their
    /// reference, on which their line is alone.
    pub(super) fn report_notes_past_bottom(&mut self, location: &Location) {
        self.fonts.report_once(
            ReportKind::Overflow,
            location,
            "footnotes too tall for the page of their reference run past the bottom of its \
             body: notes are not cut across pages yet"
                .to_owned(),
        );
    }
}
