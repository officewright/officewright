//! Pagination: the body text of every section flowed onto pages, each page with the
//! display list of what it draws.

// reason: the render entry point (plan PR 8c) uses this; until then only the tests and
// the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use officewright_core::Pt;

use crate::display::{DisplayItem, LeaderBudget, Painting, Point};
use crate::error::{Error, Result};
use crate::fonts::Fonts;
use crate::ir::{
    Block, CompatFlag, CompatMode, Document, FieldKind, Inline, MAX_TABLE_DEPTH, PageSetup,
    PageVerticalAlign, Paragraph, Section, SectionStart,
};
use crate::itemize::FieldValues;
use crate::lines::LineEnd;
use crate::paragraph::{self, ParagraphLayout};
use crate::report::{Location, ReportKind, Step};
use crate::shape::Shaper;

/// Lengths closer than this are equal, so that lines that reach the bottom of the body
/// but for rounding still fit.
const EPSILON: f64 = 1e-6;

/// A page: its size, its body area and what it draws.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Page {
    /// The section whose page setup it has, by its place in the document: the one in
    /// effect where it starts.
    pub(crate) section: usize,
    pub(crate) width: Pt,
    pub(crate) height: Pt,
    /// Its number as page numbers show it.
    pub(crate) number: u32,
    pub(crate) body: Body,
    /// The parts of paragraphs of the body on it, top to bottom.
    pub(crate) fragments: Vec<Fragment>,
    /// What it draws: its header and footer, then its body.
    pub(crate) items: Vec<DisplayItem>,
    /// Whether it is a blank page an odd- or even-page section start put in, which has
    /// no header or footer.
    pub(crate) blank: bool,
}

/// The area of a page the body text flows in, in points from the page's top left
/// corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Body {
    pub(crate) left: Pt,
    pub(crate) top: Pt,
    pub(crate) width: Pt,
    pub(crate) bottom: Pt,
}

/// Lines of a paragraph on a page.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fragment {
    /// The paragraph, by its place in the flow: every paragraph of the body in reading
    /// order, those of table cells included.
    pub(crate) paragraph: usize,
    pub(crate) lines: Range<usize>,
    /// The top of its first line.
    pub(crate) top: Pt,
    /// The height of its lines.
    pub(crate) height: Pt,
}

/// A paragraph of the flow, with where it comes from.
pub(crate) struct Flowed<'a> {
    /// Its section, by its place in the document.
    pub(crate) section: usize,
    pub(crate) paragraph: &'a Paragraph,
    pub(crate) location: Location,
    /// Whether it is in a table cell, where keeping lines together and widow control do
    /// not apply (MS-OI29500 2.1.46 a, 2.1.66 a). Keeping with the next, which Word applies
    /// to a cell's row, waits for tables to be laid out (plan PR 12).
    pub(crate) in_cell: bool,
    /// Whether it is the first paragraph of a table cell, which keeps the space above it
    /// at the top of a page.
    pub(crate) first_in_cell: bool,
}

/// The paragraphs of the body of every section of `document`, in reading order. Tables
/// are not laid out yet: the paragraphs of their cells follow one another at the full
/// width, and that is reported.
pub(crate) fn flow<'a>(document: &'a Document, fonts: &mut Fonts<'_>) -> Result<Vec<Flowed<'a>>> {
    let mut out = Vec::new();
    let mut table = None;
    let mut location = Location::default();
    location.push(Step::Field("sections"));
    for (index, section) in document.sections().iter().enumerate() {
        location.push(Step::Index(index));
        location.push(Step::Field("blocks"));
        let mut walk = Walk {
            section: index,
            location: &mut location,
            table: &mut table,
            out: &mut out,
        };
        walk.blocks(section.blocks(), false, 0)?;
        location.pop();
        location.pop();
    }
    if let Some(location) = table {
        fonts.report_once(
            ReportKind::FeatureUnsupported,
            &location,
            "tables are not laid out yet: the paragraphs of their cells follow one another at \
             the full width"
                .to_owned(),
        );
    }
    Ok(out)
}

/// A walk over the blocks of a section for [`flow`].
struct Walk<'a, 'w> {
    section: usize,
    location: &'w mut Location,
    /// The place of the first table.
    table: &'w mut Option<Location>,
    out: &'w mut Vec<Flowed<'a>>,
}

impl<'a> Walk<'a, '_> {
    /// Adds the paragraphs of `blocks`, those of a table cell when `in_cell`, in tables
    /// nested `depth` deep.
    fn blocks(&mut self, blocks: &'a [Block], in_cell: bool, depth: usize) -> Result<()> {
        for (index, block) in blocks.iter().enumerate() {
            self.location.push(Step::Index(index));
            match block {
                Block::Paragraph(paragraph) => self.out.push(Flowed {
                    section: self.section,
                    paragraph,
                    location: self.location.clone(),
                    in_cell,
                    first_in_cell: in_cell && index == 0,
                }),
                Block::Table(table) => {
                    if depth >= MAX_TABLE_DEPTH {
                        return Err(Error::NestingTooDeep {
                            location: self.location.clone(),
                            limit: MAX_TABLE_DEPTH,
                        });
                    }
                    self.table.get_or_insert_with(|| self.location.clone());
                    self.location.push(Step::Field("rows"));
                    for (row_index, row) in table.rows().iter().enumerate() {
                        self.location.push(Step::Index(row_index));
                        self.location.push(Step::Field("cells"));
                        for (cell_index, cell) in row.cells().iter().enumerate() {
                            self.location.push(Step::Index(cell_index));
                            self.location.push(Step::Field("blocks"));
                            self.blocks(cell.blocks(), true, depth + 1)?;
                            self.location.pop();
                            self.location.pop();
                        }
                        self.location.pop();
                        self.location.pop();
                    }
                    self.location.pop();
                }
            }
            self.location.pop();
        }
        Ok(())
    }
}

/// The body of a document laid out on pages.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Paginated {
    pub(crate) pages: Vec<Page>,
    /// The number of lines of every paragraph of the flow.
    pub(crate) lines: Vec<usize>,
}

/// Lays out the body of every section of `document` on pages, at most `max_pages` of
/// them. A document without sections has one empty page of the default setup.
pub(crate) fn paginate(
    document: &Document,
    fonts: &mut Fonts<'_>,
    shaper: &mut Shaper,
    max_pages: usize,
) -> Result<Paginated> {
    let flow = flow(document, fonts)?;
    // `NUMPAGES` and `SECTIONPAGES` depend on the layout they are part of: each pass lays
    // out with the counts the one before found, until they agree.
    let counted = counts_pages(document);
    let mut totals = Totals {
        pages: 1,
        sections: vec![1; document.sections().len()],
    };
    let mut pass = 1;
    loop {
        let mut pager = Pager {
            document,
            fonts: &mut *fonts,
            shaper: &mut *shaper,
            max_pages,
            pages: Vec::new(),
            cursor: 0.0,
            above: None,
            next_number: 1,
            leaders: LeaderBudget::default(),
            totals: totals.clone(),
            frames: HashMap::new(),
        };
        let lines = pager.run(&flow)?;
        if pager.pages.is_empty() {
            pager.new_page(0, &PageSetup::default())?;
        }
        let found = Totals::of(&pager.pages, document);
        if !counted || found == totals || pass == MAX_PASSES {
            if counted && found != totals {
                pager.fonts.report_once(
                    ReportKind::FieldCached,
                    &Location::default(),
                    format!(
                        "the number of pages changed in each of {MAX_PASSES} layouts; NUMPAGES \
                         and SECTIONPAGES show the count of the one before the last"
                    ),
                );
            }
            return Ok(Paginated {
                pages: pager.pages,
                lines,
            });
        }
        totals = found;
        pass += 1;
    }
}

/// The most times a document is laid out for its page counts to settle; a document whose
/// count changes every time (it must oscillate) keeps the last one.
const MAX_PASSES: u32 = 3;

/// The page counts `NUMPAGES` and `SECTIONPAGES` show.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Totals {
    pages: u32,
    /// Of every section.
    sections: Vec<u32>,
}

impl Totals {
    /// The counts of `pages` of `document`: all of them, but one less when the first
    /// section has a title page and numbers its pages from 0 (MS-OI29500 2.1.489 a). A
    /// section counts the blank page of its odd or even start, and not the page it starts
    /// on when it is continuous: hypotheses until Word renders (`E-section-odd.docx`,
    /// `E-section-continuous-headers.docx`).
    fn of(pages: &[Page], document: &Document) -> Self {
        let count = |section: usize| {
            u32::try_from(pages.iter().filter(|page| page.section == section).count())
                .unwrap_or(u32::MAX)
        };
        let all = u32::try_from(pages.len()).unwrap_or(u32::MAX);
        let less = document.sections().first().is_some_and(|section| {
            section.title_page() && section.page_numbering().start() == Some(0)
        });
        Self {
            pages: all.saturating_sub(u32::from(less)),
            sections: (0..document.sections().len()).map(count).collect(),
        }
    }

    fn section(&self, section: usize) -> u32 {
        self.sections.get(section).copied().unwrap_or(1)
    }
}

/// Whether a paragraph of `document`, in its body or in a header or footer, has a
/// `NUMPAGES` or `SECTIONPAGES` field.
fn counts_pages(document: &Document) -> bool {
    fn in_inlines(inlines: &[Inline]) -> bool {
        inlines.iter().any(|inline| {
            matches!(inline, Inline::Field(field)
                if matches!(field.kind(), FieldKind::NumPages | FieldKind::SectionPages))
        })
    }
    fn in_blocks(blocks: &[Block], depth: usize) -> bool {
        blocks.iter().any(|block| match block {
            Block::Paragraph(paragraph) => in_inlines(paragraph.inlines()),
            Block::Table(table) => {
                depth < MAX_TABLE_DEPTH
                    && table.rows().iter().any(|row| {
                        row.cells()
                            .iter()
                            .any(|cell| in_blocks(cell.blocks(), depth + 1))
                    })
            }
        })
    }
    document.sections().iter().any(|section| {
        let sets = [section.headers(), section.footers()];
        in_blocks(section.blocks(), 0)
            || sets.iter().any(|set| {
                in_blocks(set.primary(), 0) || in_blocks(set.first(), 0) || in_blocks(set.even(), 0)
            })
    })
}

/// Whether `flowed` has a `PAGE` field, whose value depends on the page it lands on.
fn has_page_field(flowed: &Flowed<'_>) -> bool {
    flowed
        .paragraph
        .inlines()
        .iter()
        .any(|inline| matches!(inline, Inline::Field(field) if *field.kind() == FieldKind::Page))
}

/// Whether `blocks` have a field whose value the layout computes, at any depth of
/// tables.
fn has_page_fields(blocks: &[Block], depth: usize) -> bool {
    blocks.iter().any(|block| match block {
        Block::Paragraph(paragraph) => paragraph.inlines().iter().any(|inline| {
            matches!(inline, Inline::Field(field) if matches!(
                field.kind(),
                FieldKind::Page | FieldKind::NumPages | FieldKind::SectionPages
            ))
        }),
        Block::Table(table) => {
            depth < MAX_TABLE_DEPTH
                && table.rows().iter().any(|row| {
                    row.cells()
                        .iter()
                        .any(|cell| has_page_fields(cell.blocks(), depth + 1))
                })
        }
    })
}

/// Which of its section's headers and footers a page has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    /// That of the section's first page.
    First,
    /// That of even pages.
    Even,
    /// That of every other page.
    Primary,
}

/// A header or footer laid out for a page: its paragraphs, each with its top from the
/// frame's top, and its height.
struct Frame<'d> {
    parts: Vec<(Flowed<'d>, ParagraphLayout, f64)>,
    height: f64,
}

/// What is laid out so far, to return to when a keep rule moves paragraphs.
#[derive(Clone, Copy)]
struct Checkpoint {
    /// The paragraph of the flow placed next.
    index: usize,
    pages: usize,
    items: usize,
    fragments: usize,
    cursor: f64,
    above: Option<usize>,
    next_number: u32,
    /// The leader glyphs the run could still draw.
    leaders: LeaderBudget,
    /// Whether nothing was on the page yet.
    at_top: bool,
}

/// Where a paragraph's lines went.
#[derive(Clone, Copy)]
struct Placed {
    first_page: usize,
    last_page: usize,
}

struct Pager<'d, 'f, 'b> {
    document: &'d Document,
    fonts: &'f mut Fonts<'b>,
    shaper: &'f mut Shaper,
    max_pages: usize,
    pages: Vec<Page>,
    /// The top of what comes next on the current page: under the last line placed,
    /// without the space below it.
    cursor: f64,
    /// The paragraph of the flow above on this page, whose space below meets the space
    /// above the next one.
    above: Option<usize>,
    /// The number of the next page.
    next_number: u32,
    /// The leader glyphs the run may still draw.
    leaders: LeaderBudget,
    /// The page counts of the pass before, for `NUMPAGES` and `SECTIONPAGES`.
    totals: Totals,
    /// The headers and footers laid out so far, by section, side, kind and the values of
    /// their fields when they have any: each is laid out once, not once a page.
    frames: HashMap<FrameKey, Rc<Frame<'d>>>,
}

/// What a header or footer is laid out for: its section, whether it is the header, its
/// kind, and the values of its fields when it has fields the layout computes.
type FrameKey = (usize, bool, Kind, Option<FieldValues>);

impl<'d> Pager<'d, '_, '_> {
    /// Places every paragraph of `flow`, section by section. Returns the number of lines
    /// of each.
    fn run(&mut self, flow: &[Flowed<'d>]) -> Result<Vec<usize>> {
        let sections = self.document.sections();
        let mut layouts: Vec<Option<ParagraphLayout>> = Vec::new();
        layouts.resize_with(flow.len(), || None);
        let mut placed: Vec<Option<Placed>> = vec![None; flow.len()];
        // The start of the chain of paragraphs kept with the next one that the
        // paragraph placed next would end, and the chains moved to a page of their own
        // once, which are not moved again.
        let mut chain: Option<Checkpoint> = None;
        let mut moved: HashSet<usize> = HashSet::new();
        // Whether the paragraph placed next starts after a page break of its own: a
        // section start, a page break, or a page break before it.
        let mut index = 0;
        let mut section_started: Option<usize> = None;
        let mut broke_page = false;
        // A page break that ended the paragraph before: the next one starts a new page,
        // unless a section start makes one anyway. At the end of the document it makes
        // none, as in Word 2013 and later, which keeps the paragraph mark on the page of
        // the break.
        let mut pending_break = false;
        while let Some(flowed) = flow.get(index) {
            // The sections from the last one started up to this paragraph's, those
            // without paragraphs included.
            let mut kept_top = false;
            let mut section_here = false;
            while section_started.is_none_or(|started| started < flowed.section) {
                section_here = true;
                let next = section_started.map_or(0, |started| started + 1);
                let Some(section) = sections.get(next) else {
                    break;
                };
                if self.start_section(next, section, pending_break)? {
                    kept_top = true;
                    broke_page = true;
                    pending_break = false;
                }
                section_started = Some(next);
            }
            let Some(section) = sections.get(flowed.section) else {
                index += 1;
                continue;
            };
            if pending_break {
                self.new_page(flowed.section, section.page())?;
                pending_break = false;
            }
            let props = flowed.paragraph.props();
            // A page break before a paragraph that already starts a page makes no page.
            // Hypothesis until a Word render (`E-page-break-before-top.docx`).
            if props.page_break_before() && !self.page_is_empty() {
                self.new_page(flowed.section, section.page())?;
                broke_page = true;
            }
            let checkpoint = self.checkpoint(index);
            let values = self.field_values(flowed.section);
            let layout = match layouts.get_mut(index) {
                Some(Some(layout)) => layout,
                // Fields show the values of the page it is laid out for: where it starts,
                // unless keep and widow rules move it further.
                Some(slot) => slot.insert(paragraph::lay_out_with(
                    self.document,
                    flowed.paragraph,
                    self.body()?.width,
                    (&mut *self.fonts, &mut *self.shaper),
                    &flowed.location,
                    Some(&values),
                )),
                None => break,
            };
            let keeps_space = kept_top || index == 0 || flowed.first_in_cell;
            let (mut first_page, mut ends_with_break) =
                self.place(flow, index, layout, keeps_space)?;
            // A `PAGE` field shows the number of the page its paragraph starts on: laid out
            // for the page it began on, a paragraph that keep or widow rules moved is laid
            // out again for the page it landed on, and placed again.
            let landed = self.pages.get(first_page).map(|page| page.number);
            if let Some(landed) = landed.filter(|&n| n != values.page && has_page_field(flowed)) {
                self.restore(checkpoint);
                let values = FieldValues {
                    page: landed,
                    ..values.clone()
                };
                *layout = paragraph::lay_out_with(
                    self.document,
                    flowed.paragraph,
                    self.body()?.width,
                    (&mut *self.fonts, &mut *self.shaper),
                    &flowed.location,
                    Some(&values),
                );
                (first_page, ends_with_break) = self.place(flow, index, layout, keeps_space)?;
                if self.pages.get(first_page).map(|page| page.number) != Some(landed) {
                    self.fonts.report_once(
                        ReportKind::FieldCached,
                        &flowed.location,
                        "a PAGE field in the body text shows the number of a page next to the \
                         one its paragraph starts on"
                            .to_owned(),
                    );
                }
            }
            // A page or column break before its last line: its last lines start a page
            // whatever comes before them.
            let inner_break = layout
                .lines
                .iter()
                .rev()
                .skip(1)
                .any(|line| matches!(line.end, LineEnd::PageBreak | LineEnd::ColumnBreak));
            let here = Placed {
                first_page,
                last_page: self.pages.len().saturating_sub(1),
            };
            if let Some(slot) = placed.get_mut(index) {
                *slot = Some(here);
            }
            // A chain kept with this paragraph must end on the page where it starts. A
            // section start or a page break before this paragraph ends the chain: keeping
            // with the next does not reach past them, so moving a chain never undoes one.
            // Hypothesis until a Word render (`E-keepnext-chain.docx`).
            let boundary = broke_page || section_here;
            if let Some(start) = chain.filter(|_| !boundary) {
                let previous = index
                    .checked_sub(1)
                    .and_then(|i| placed.get(i).copied().flatten());
                let apart = previous.is_some_and(|previous| previous.last_page != here.first_page);
                if apart {
                    if !start.at_top && moved.insert(start.index) {
                        // The whole chain to the next page, once.
                        self.restore(start);
                        chain = None;
                        let section = flow.get(start.index).map_or(0, |f| f.section);
                        let setup = sections
                            .get(section)
                            .map_or_else(PageSetup::default, |s| *s.page());
                        self.new_page(section, &setup)?;
                        index = start.index;
                        broke_page = false;
                        continue;
                    }
                    self.fonts.report_once(
                        ReportKind::Overflow,
                        &flowed.location,
                        "paragraphs kept with the next one do not fit on a page together, so \
                         they are split"
                            .to_owned(),
                    );
                }
            }
            chain = match chain {
                _ if !props.keep_next() || flowed.in_cell => None,
                // Its last lines, kept with the next paragraph, start a page after the
                // break inside it: the chain starts there, at the top of a page, and
                // moving it would only take the lines before the break along.
                _ if inner_break => Some(Checkpoint {
                    at_top: true,
                    ..checkpoint
                }),
                Some(start) if !boundary => Some(start),
                _ => Some(checkpoint),
            };
            broke_page = ends_with_break;
            pending_break = ends_with_break;
            index += 1;
        }
        // Sections after the last paragraph, with none of their own.
        let mut next = section_started.map_or(0, |started| started + 1);
        while let Some(section) = sections.get(next) {
            if self.start_section(next, section, pending_break)? {
                pending_break = false;
            }
            next += 1;
        }
        Ok(layouts
            .iter()
            .map(|layout| layout.as_ref().map_or(0, |layout| layout.lines.len()))
            .collect())
    }

    /// Starts section `index`: on a new page, or on the current one for a continuous
    /// section of the same page setup, unless a page break (`after_break`) ended the
    /// paragraph before. Returns whether it started a new page. The current page keeps
    /// the section it started in: a continuous section's headers and footers apply from
    /// its next page (hypothesis, `E-section-continuous-headers.docx`).
    fn start_section(
        &mut self,
        index: usize,
        section: &Section,
        after_break: bool,
    ) -> Result<bool> {
        let mut location = Location::default();
        location.push(Step::Field("sections"));
        location.push(Step::Index(index));
        if section.columns().count() > 1 {
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                &location,
                "text in columns is laid out in one column".to_owned(),
            );
        }
        if section.vertical_align() != PageVerticalAlign::Top {
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                &location,
                "the body text of a section is laid out from the top of its pages, whatever \
                 its vertical alignment"
                    .to_owned(),
            );
        }
        if let Some(start) = section.page_numbering().start() {
            self.next_number = start;
        }
        if section.page_numbering().chapter_style().is_some() {
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                &location,
                "chapter numbers before page numbers are not written".to_owned(),
            );
        }
        // The first section starts the first page, numbered 1 or as it says, whatever its
        // start: no blank page comes before it for an odd or even start. Hypothesis until a
        // Word render (`E-section-odd.docx`).
        let Some(current) = self.pages.last() else {
            self.new_page(index, section.page())?;
            return Ok(true);
        };
        let same_setup = self
            .document
            .sections()
            .get(current.section)
            .is_some_and(|previous| previous.page() == section.page());
        match section.start() {
            // After a page break, the break makes the new page, when a paragraph follows.
            SectionStart::Continuous if after_break || same_setup => Ok(false),
            SectionStart::Continuous => {
                self.fonts.report_once(
                    ReportKind::FeatureUnsupported,
                    &location,
                    "a continuous section with another page size or other margins starts on a \
                     new page"
                        .to_owned(),
                );
                self.new_page(index, section.page())?;
                Ok(true)
            }
            SectionStart::EvenPage | SectionStart::OddPage => {
                // A blank page first when the next page's number has the wrong parity;
                // it counts. Hypothesis until a Word render (`E-section-odd.docx`).
                let odd = self.next_number % 2 == 1;
                if odd == (section.start() == SectionStart::EvenPage) {
                    self.add_page(index, section.page(), true)?;
                }
                self.new_page(index, section.page())?;
                Ok(true)
            }
            // A next-column start in a section of one column, as every section is laid out,
            // is a next-page one. Hypothesis until a Word render
            // (`E-section-next-column.docx`).
            _ => {
                self.new_page(index, section.page())?;
                Ok(true)
            }
        }
    }

    /// Places the lines of paragraph `index` of `flow`, laid out as `layout`, from the
    /// cursor on; the space above it is kept at the top of a page when `keeps_space`.
    /// Returns the page of its first line, and whether its last line ends in a page or
    /// column break, after which the next paragraph starts on a new page.
    fn place(
        &mut self,
        flow: &[Flowed<'_>],
        index: usize,
        layout: &ParagraphLayout,
        keeps_space: bool,
    ) -> Result<(usize, bool)> {
        let Some(flowed) = flow.get(index) else {
            return Ok((self.pages.len().saturating_sub(1), false));
        };
        let props = flowed.paragraph.props();
        let lines = &layout.lines;
        let count = lines.len();
        let keep_lines = props.keep_lines() && !flowed.in_cell;
        let widow_control = props.widow_control() && !flowed.in_cell;
        let section = self
            .document
            .sections()
            .get(flowed.section)
            .map_or_else(PageSetup::default, |section| *section.page());
        let mut first = 0;
        let mut first_page = None;
        loop {
            let empty = self.page_is_empty();
            let body = self.body()?;
            // Space above: between this paragraph and the one above it on the page; at
            // the top of a page only at the start of the document or of a section, and
            // in a table cell. Word's render of `E-space-before-top.docx`: dropped after
            // flowing and after page breaks alike.
            let gap = if first > 0 {
                0.0
            } else if let Some(above) = self.above.filter(|_| !empty).and_then(|i| flow.get(i)) {
                paragraph::space_between(self.document, above.paragraph.props(), props).0
            } else if keeps_space {
                paragraph::space_before(self.document, props).0
            } else {
                0.0
            };
            let here = self.pages.len().saturating_sub(1);
            let rest = count - first;
            if rest == 0 {
                return Ok((first_page.unwrap_or(here), false));
            }
            // On an empty page, the space above never pushes the first line off the body:
            // it shrinks to the room the line leaves. Ours, not Word's.
            let gap = match lines.get(first) {
                Some(line) if empty => {
                    gap.min((body.bottom.0 - self.cursor - line.height.0).max(0.0))
                }
                _ => gap,
            };
            let top = self.cursor + gap;
            let (fit, breaks) = fitting(lines, first, top, body.bottom.0);
            if fit == rest || (breaks && fit > 0) {
                self.put(index, flowed, layout, first..first + fit, top);
                first_page.get_or_insert(here);
                first += fit;
                if first == count {
                    let ends = lines.last().is_some_and(|line| {
                        matches!(line.end, LineEnd::PageBreak | LineEnd::ColumnBreak)
                    });
                    if ends {
                        self.report_mark_after_break(flowed);
                    }
                    return Ok((first_page.unwrap_or(here), ends));
                }
                // A page break inside the paragraph: the rest on the next page.
                self.new_page(flowed.section, &section)?;
                continue;
            }
            // The paragraph does not fit: split it, or move it. One kept on one page that is
            // taller than a page moves to a new page all the same, and is split there.
            // Hypothesis until a Word render (`E-keeplines-long.docx`).
            if first == 0 && keep_lines && !empty {
                self.new_page(flowed.section, &section)?;
                continue;
            }
            let mut take = fit;
            if widow_control {
                // At least two lines at the bottom of a page and at the top of the next.
                if rest - take < 2 {
                    take = rest.saturating_sub(2);
                }
                let least = if first == 0 { 2 } else { 1 };
                if take < least {
                    take = 0;
                }
            }
            if take == 0 {
                if !empty {
                    self.new_page(flowed.section, &section)?;
                    continue;
                }
                // Nothing moves it further: the lines that fit, at least one.
                take = fit.max(1);
                if fit == 0 {
                    self.fonts.report_once(
                        ReportKind::Overflow,
                        &flowed.location,
                        "a line is taller than the body of its page".to_owned(),
                    );
                }
            }
            self.put(index, flowed, layout, first..first + take, top);
            first_page.get_or_insert(here);
            first += take;
            if first == count {
                return Ok((first_page.unwrap_or(here), false));
            }
            self.new_page(flowed.section, &section)?;
        }
    }

    /// Reports, once, that a page break at the end of a paragraph keeps the paragraph
    /// mark on its page, as Word 2013 and later do, where a document of an older mode
    /// without `splitPgBreakAndParaMark` puts it on the next page.
    fn report_mark_after_break(&mut self, flowed: &Flowed<'_>) {
        let settings = self.document.settings();
        if settings.compat_mode() != CompatMode::Word2013
            && !settings.has_compat_flag(&CompatFlag::SplitPgBreakAndParaMark)
        {
            self.fonts.report_once(
                ReportKind::CompatIgnored,
                &flowed.location,
                "a page break at the end of a paragraph keeps the paragraph mark on its page, as \
                 in Word 2013 and later"
                    .to_owned(),
            );
        }
    }

    /// Puts `lines` of paragraph `index` on the current page, the first with its top at
    /// `top`.
    fn put(
        &mut self,
        index: usize,
        flowed: &Flowed<'_>,
        layout: &ParagraphLayout,
        lines: Range<usize>,
        top: f64,
    ) {
        let Some(page) = self.pages.last() else {
            return;
        };
        let left = page.body.left;
        let height: f64 = layout
            .lines
            .get(lines.clone())
            .unwrap_or(&[])
            .iter()
            .map(|line| line.height.0)
            .sum();
        let mut items = Vec::new();
        self.report_borders(flowed);
        let origin = Point {
            x: left,
            y: Pt(top),
        };
        let mut painting = Painting {
            document: self.document,
            paragraph: flowed.paragraph,
            layout,
            fonts: self.fonts,
            location: &flowed.location,
            background: None,
            leaders: &mut self.leaders,
        };
        painting.paint_shading(lines.clone(), origin, &mut items);
        painting.paint(lines.clone(), origin, &mut items);
        if let Some(page) = self.pages.last_mut() {
            page.items.extend(items);
            page.fragments.push(Fragment {
                paragraph: index,
                lines,
                top: Pt(top),
                height: Pt(height),
            });
        }
        self.cursor = top + height;
        self.above = Some(index);
    }

    /// Adds a page of section `section` with `setup`, and moves the cursor to the top of
    /// its body.
    fn new_page(&mut self, section: usize, setup: &PageSetup) -> Result<()> {
        self.add_page(section, setup, false)
    }

    /// Adds a page of section `section` with `setup`, `blank` or with the header and
    /// footer it has, and moves the cursor to the top of its body: below the header and
    /// above the footer where they reach past the margins, unless a margin is negative,
    /// which fixes it (ECMA-376 Part 1, §17.6.11). Hypothesis until Word renders
    /// (`E-header-tall.docx`, `E-header-space-after.docx`): no gap is added between them,
    /// and the space after the last paragraph of a header or footer is part of its height.
    fn add_page(&mut self, section: usize, setup: &PageSetup, blank: bool) -> Result<()> {
        if self.pages.len() >= self.max_pages {
            return Err(Error::TooManyPages {
                limit: self.max_pages,
            });
        }
        let number = self.next_number;
        let mut body = body(setup);
        let mut items = Vec::new();
        if let Some(ir) = self.document.sections().get(section).filter(|_| !blank) {
            // The section's first page is the first of its own: a continuous section starts
            // on a page that keeps the section before, so its title-page header is on its
            // second. Hypothesis until a Word render (`E-section-continuous-headers.docx`).
            let first = self
                .pages
                .iter()
                .rev()
                .find(|page| !page.blank)
                .is_none_or(|page| page.section != section);
            let kind = if ir.title_page() && first {
                Kind::First
            } else if self.document.settings().even_and_odd_headers() && number % 2 == 0 {
                Kind::Even
            } else {
                Kind::Primary
            };
            let values = FieldValues {
                page: number,
                ..self.field_values(section)
            };
            let margins = setup.margins();
            let room = (body.width, setup.size().height());
            let header = self.cached_frame(ir, section, true, kind, &values, room)?;
            let footer = self.cached_frame(ir, section, false, kind, &values, room)?;
            let header_top = margins.header().0;
            let footer_bottom = setup.size().height().0 - margins.footer().0;
            // An empty header or footer takes no room (design 6.4).
            if margins.top().0 >= 0.0 && !header.parts.is_empty() {
                body.top = Pt(body.top.0.max(header_top + header.height));
            }
            if margins.bottom().0 >= 0.0 && !footer.parts.is_empty() {
                body.bottom = Pt(body.bottom.0.min(footer_bottom - footer.height));
            }
            if body.bottom.0 <= body.top.0 {
                self.fonts.report_once(
                    ReportKind::Overflow,
                    &Location::default(),
                    "the header and footer of a page leave no room for its body text, which \
                     is drawn below them"
                        .to_owned(),
                );
            }
            self.paint_frame(
                &header,
                Point {
                    x: body.left,
                    y: Pt(header_top),
                },
                &mut items,
            );
            let footer_top = footer_bottom - footer.height;
            self.paint_frame(
                &footer,
                Point {
                    x: body.left,
                    y: Pt(footer_top),
                },
                &mut items,
            );
        }
        self.pages.push(Page {
            section,
            width: setup.size().width(),
            height: setup.size().height(),
            number,
            body,
            fragments: Vec::new(),
            items,
            blank,
        });
        self.next_number = self.next_number.saturating_add(1);
        self.cursor = body.top.0;
        self.above = None;
        Ok(())
    }

    /// Reports, once, that the borders of `flowed` are not drawn.
    fn report_borders(&mut self, flowed: &Flowed<'_>) {
        let borders = flowed.paragraph.props().borders();
        let bordered = [
            borders.top(),
            borders.bottom(),
            borders.start(),
            borders.end(),
            borders.between(),
            borders.bar(),
        ]
        .iter()
        .any(Option::is_some);
        if bordered {
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                &flowed.location,
                "paragraph borders are not drawn yet".to_owned(),
            );
        }
    }

    /// The values of the fields of section `section` on the current page.
    fn field_values(&self, section: usize) -> FieldValues {
        let format = self
            .document
            .sections()
            .get(section)
            .map(|section| section.page_numbering().format().clone())
            .unwrap_or_default();
        FieldValues {
            page: self
                .pages
                .last()
                .map_or(self.next_number, |page| page.number),
            format,
            pages: self.totals.pages,
            section_pages: self.totals.section(section),
        }
    }

    /// [`frame`](Self::frame), laid out once for each section, side, kind and, for a frame
    /// with fields the layout computes, field values.
    fn cached_frame(
        &mut self,
        ir: &'d Section,
        section: usize,
        header: bool,
        kind: Kind,
        values: &FieldValues,
        room: (Pt, Pt),
    ) -> Result<Rc<Frame<'d>>> {
        let set = if header { ir.headers() } else { ir.footers() };
        let blocks = match kind {
            Kind::First => set.first(),
            Kind::Even => set.even(),
            Kind::Primary => set.primary(),
        };
        let key = (
            section,
            header,
            kind,
            has_page_fields(blocks, 0).then(|| values.clone()),
        );
        if let Some(frame) = self.frames.get(&key) {
            return Ok(Rc::clone(frame));
        }
        let frame = Rc::new(self.frame(ir, section, header, kind, values, room)?);
        self.frames.insert(key, Rc::clone(&frame));
        Ok(frame)
    }

    /// The header (`header`) or footer of `kind` of section `section`, laid out `room.0`
    /// wide with the field values `values`: its paragraphs one under the other with the
    /// space between them, the space before the first and after the last included. Its
    /// paragraphs past the page's height `room.1` are left out, and that is reported:
    /// nothing of them could be seen.
    fn frame(
        &mut self,
        ir: &'d Section,
        section: usize,
        header: bool,
        kind: Kind,
        values: &FieldValues,
        (width, page_height): (Pt, Pt),
    ) -> Result<Frame<'d>> {
        let set = if header { ir.headers() } else { ir.footers() };
        let (blocks, name) = match kind {
            Kind::First => (set.first(), "first"),
            Kind::Even => (set.even(), "even"),
            Kind::Primary => (set.primary(), "primary"),
        };
        let mut location = Location::default();
        location.push(Step::Field("sections"));
        location.push(Step::Index(section));
        location.push(Step::Field(if header { "headers" } else { "footers" }));
        location.push(Step::Field(name));
        let mut table = None;
        let mut flowed = Vec::new();
        let mut walk = Walk {
            section,
            location: &mut location,
            table: &mut table,
            out: &mut flowed,
        };
        walk.blocks(blocks, false, 0)?;
        if let Some(location) = table {
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                &location,
                "tables are not laid out yet: the paragraphs of their cells follow one another \
                 at the full width"
                    .to_owned(),
            );
        }
        let mut parts = Vec::new();
        let mut height = 0.0;
        let mut above: Option<&'d Paragraph> = None;
        for part in flowed {
            if height >= page_height.0 {
                self.fonts.report_once(
                    ReportKind::Overflow,
                    &part.location,
                    "a header or footer taller than its page is cut at the page's height"
                        .to_owned(),
                );
                break;
            }
            let props = part.paragraph.props();
            height += match above {
                Some(above) => paragraph::space_between(self.document, above.props(), props).0,
                None => paragraph::space_before(self.document, props).0,
            };
            let layout = paragraph::lay_out_with(
                self.document,
                part.paragraph,
                width,
                (&mut *self.fonts, &mut *self.shaper),
                &part.location,
                Some(values),
            );
            let top = height;
            height += layout.height().0;
            above = Some(part.paragraph);
            parts.push((part, layout, top));
        }
        if let Some(last) = above {
            height += paragraph::space_after(self.document, last.props()).0;
        }
        Ok(Frame { parts, height })
    }

    /// Adds to `items` what `frame` draws, its top left corner at `origin`.
    fn paint_frame(&mut self, frame: &Frame<'d>, origin: Point, items: &mut Vec<DisplayItem>) {
        for (part, layout, top) in &frame.parts {
            self.report_borders(part);
            let at = Point {
                x: origin.x,
                y: Pt(origin.y.0 + top),
            };
            let mut painting = Painting {
                document: self.document,
                paragraph: part.paragraph,
                layout,
                fonts: self.fonts,
                location: &part.location,
                background: None,
                leaders: &mut self.leaders,
            };
            let lines = 0..layout.lines.len();
            painting.paint_shading(lines.clone(), at, items);
            painting.paint(lines, at, items);
        }
    }

    /// The body of the current page.
    fn body(&self) -> Result<Body> {
        self.pages
            .last()
            .map(|page| page.body)
            .ok_or_else(|| Error::InvalidDocument {
                location: Location::default(),
                problem: "no page to lay out on".to_owned(),
            })
    }

    /// Whether nothing is on the current page yet.
    fn page_is_empty(&self) -> bool {
        self.pages
            .last()
            .is_none_or(|page| page.fragments.is_empty())
    }

    fn checkpoint(&self, index: usize) -> Checkpoint {
        let page = self.pages.last();
        Checkpoint {
            index,
            pages: self.pages.len(),
            items: page.map_or(0, |page| page.items.len()),
            fragments: page.map_or(0, |page| page.fragments.len()),
            cursor: self.cursor,
            above: self.above,
            next_number: self.next_number,
            leaders: self.leaders,
            at_top: self.page_is_empty(),
        }
    }

    /// Goes back to what was laid out at `checkpoint`.
    fn restore(&mut self, checkpoint: Checkpoint) {
        self.pages.truncate(checkpoint.pages);
        if let Some(page) = self.pages.last_mut() {
            page.items.truncate(checkpoint.items);
            page.fragments.truncate(checkpoint.fragments);
        }
        self.cursor = checkpoint.cursor;
        self.above = checkpoint.above;
        self.next_number = checkpoint.next_number;
        self.leaders = checkpoint.leaders;
    }
}

/// The body of a page of `setup`: inside the margins, the gutter on the left; a
/// negative top or bottom margin is a fixed one of its size (ECMA-376 Part 1, §17.6.11).
pub(crate) fn body(setup: &PageSetup) -> Body {
    let size = setup.size();
    let margins = setup.margins();
    let left = margins.left().0 + margins.gutter().0;
    let right = size.width().0 - margins.right().0;
    let top = margins.top().0.abs();
    let bottom = size.height().0 - margins.bottom().0.abs();
    Body {
        left: Pt(left),
        top: Pt(top),
        width: Pt((right - left).max(0.0)),
        bottom: Pt(bottom),
    }
}

/// How many of `lines` from `first` on fit between `top` and `bottom`, and whether the
/// last of those ends in a page or column break, which ends the page.
fn fitting(lines: &[crate::lines::Line], first: usize, top: f64, bottom: f64) -> (usize, bool) {
    let mut y = top;
    let mut count = 0;
    for line in lines.iter().skip(first) {
        if y + line.height.0 > bottom + EPSILON {
            break;
        }
        y += line.height.0;
        count += 1;
        if matches!(line.end, LineEnd::PageBreak | LineEnd::ColumnBreak) {
            return (count, true);
        }
    }
    (count, false)
}

#[cfg(test)]
mod tests {
    use officewright_core::PageSize;

    use super::*;
    use crate::ir::{
        BreakClear, Cell, HeaderFooterSet, Inline, LineSpacing, Margins, ParagraphProps, Row,
        RunFonts, RunStyle, RunStyleId, Spacing, Table,
    };
    use crate::report::LayoutReport;
    use crate::testing::{book, minimal};

    /// Pages 200 pt high, 300 wide, with 20 pt margins: a body 160 pt high, eight lines
    /// of 20 pt.
    fn setup() -> PageSetup {
        PageSetup::new(PageSize::new(Pt(300.0), Pt(200.0))).with_margins(
            Margins::default()
                .with_top(Pt(20.0))
                .with_bottom(Pt(20.0))
                .with_left(Pt(20.0))
                .with_right(Pt(20.0)),
        )
    }

    /// Spacing of exact 20 pt lines, `before` above and `after` below.
    fn spacing(before: f64, after: f64) -> ParagraphProps {
        ParagraphProps::default().with_spacing(
            Spacing::default()
                .with_line(LineSpacing::Exact(Pt(20.0)))
                .with_before(Pt(before))
                .with_after(Pt(after)),
        )
    }

    /// A paragraph of `lines` lines of `A`, with `props`.
    fn lines(style: RunStyleId, count: usize, props: ParagraphProps) -> Block {
        let mut inlines = Vec::new();
        for line in 0..count {
            if line > 0 {
                inlines.push(Inline::line_break(BreakClear::None, style));
            }
            inlines.push(Inline::text("A", style));
        }
        Paragraph::new(style)
            .with_props(props)
            .with_inlines(inlines)
            .into()
    }

    /// A document of the test font at 10 pt with the sections `make` gives.
    fn document(make: impl Fn(RunStyleId) -> Vec<Section>) -> Document {
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        for section in make(style) {
            document.add_section(section);
        }
        document
    }

    fn laid(document: &Document, max_pages: usize) -> (Result<Paginated>, LayoutReport) {
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let laid = paginate(document, &mut fonts, &mut shaper, max_pages);
        (laid, fonts.into_report())
    }

    /// Every page as the paragraphs, lines and tops of its fragments.
    fn pages(document: &Document) -> Vec<Vec<(usize, Range<usize>, f64)>> {
        let (laid, _) = laid(document, 100);
        laid.expect("pages")
            .pages
            .iter()
            .map(|page| {
                page.fragments
                    .iter()
                    .map(|f| (f.paragraph, f.lines.clone(), f.top.0))
                    .collect()
            })
            .collect()
    }

    fn one_section(blocks: Vec<Block>) -> Vec<Section> {
        vec![Section::new(setup()).with_blocks(blocks)]
    }

    #[test]
    fn paragraphs_flow_onto_pages_and_pages_are_numbered() {
        let document = document(|style| {
            one_section(
                (0..20)
                    .map(|_| lines(style, 1, spacing(0.0, 0.0)))
                    .collect(),
            )
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        assert!(report.is_empty(), "{report:?}");
        let counts: Vec<usize> = laid.pages.iter().map(|p| p.fragments.len()).collect();
        assert_eq!(counts, [8, 8, 4]);
        let numbers: Vec<u32> = laid.pages.iter().map(|p| p.number).collect();
        assert_eq!(numbers, [1, 2, 3]);
        let tops: Vec<f64> = laid.pages[1].fragments.iter().map(|f| f.top.0).collect();
        assert_eq!(tops, [20.0, 40.0, 60.0, 80.0, 100.0, 120.0, 140.0, 160.0]);
        let page = &laid.pages[0];
        assert_eq!((page.width, page.height), (Pt(300.0), Pt(200.0)));
        assert_eq!(
            page.body,
            Body {
                left: Pt(20.0),
                top: Pt(20.0),
                width: Pt(260.0),
                bottom: Pt(180.0)
            }
        );
        // What the lines draw is on their page.
        assert!(
            page.items
                .iter()
                .any(|item| matches!(item, DisplayItem::Glyphs(_)))
        );
    }

    #[test]
    fn space_between_paragraphs_is_the_larger_and_falls_off_at_the_bottom() {
        // 10 below and 15 above make 15. The eighth paragraph's line ends at the body's
        // bottom without the space below it, which may fall off the page.
        let document = document(|style| {
            one_section(vec![
                lines(style, 1, spacing(0.0, 10.0)),
                lines(style, 1, spacing(15.0, 0.0)),
            ])
        });
        assert_eq!(pages(&document), [vec![(0, 0..1, 20.0), (1, 0..1, 55.0)]]);
        let document = document_with_bottom_space();
        let laid = pages(&document);
        assert_eq!(laid.len(), 2);
        assert_eq!(laid[0].len(), 8);
        assert_eq!(laid[1], [(8, 0..1, 20.0)]);
    }

    fn document_with_bottom_space() -> Document {
        document(|style| {
            one_section(
                (0..9)
                    .map(|_| lines(style, 1, spacing(0.0, 0.0)))
                    .enumerate()
                    .map(|(i, block)| {
                        if i == 7 {
                            lines(style, 1, spacing(0.0, 30.0))
                        } else {
                            block
                        }
                    })
                    .collect(),
            )
        })
    }

    #[test]
    fn space_above_is_kept_at_the_top_of_a_page_only_where_a_document_or_section_starts() {
        // Word's render of `E-space-before-top.docx`: kept at the start of the document
        // and of a section on a new page; dropped after flowing, after a page break, and
        // for a page break before.
        let document = document(|style| {
            let mut first = vec![lines(style, 1, spacing(24.0, 0.0))];
            first.extend((0..7).map(|_| lines(style, 1, spacing(0.0, 0.0))));
            // Flows to page 2.
            first.push(lines(style, 1, spacing(24.0, 0.0)));
            // A page break before.
            first.push(lines(
                style,
                1,
                spacing(24.0, 0.0).with_page_break_before(true),
            ));
            // After a paragraph that ends in a page break.
            first.push(
                Paragraph::new(style)
                    .with_props(spacing(0.0, 0.0))
                    .with_inlines([Inline::text("A", style), Inline::page_break(style)])
                    .into(),
            );
            first.push(lines(style, 1, spacing(24.0, 0.0)));
            vec![
                Section::new(setup()).with_blocks(first),
                Section::new(setup()).with_blocks(vec![lines(style, 1, spacing(24.0, 0.0))]),
            ]
        });
        let laid = pages(&document);
        let tops: Vec<f64> = laid.iter().map(|page| page[0].2).collect();
        assert_eq!(tops, [44.0, 20.0, 20.0, 20.0, 44.0], "{laid:?}");
        // The paragraph that ends in a page break shares page 3 with the one before it.
        assert_eq!(laid[2].iter().map(|f| f.0).collect::<Vec<_>>(), [9, 10]);
    }

    #[test]
    fn widow_control_keeps_two_lines_on_each_side_of_a_page_break() {
        let place = |singles: usize, widow_control: bool| {
            let document = document(|style| {
                let mut blocks: Vec<Block> = (0..singles)
                    .map(|_| lines(style, 1, spacing(0.0, 0.0)))
                    .collect();
                blocks.push(lines(
                    style,
                    5,
                    spacing(0.0, 0.0).with_widow_control(widow_control),
                ));
                one_section(blocks)
            });
            pages(&document)
                .iter()
                .map(|page| {
                    page.iter()
                        .filter(|f| f.0 == singles)
                        .map(|f| f.1.len())
                        .sum()
                })
                .collect::<Vec<usize>>()
        };
        // Room for 3 of 5: 3 and 2.
        assert_eq!(place(5, true), [3, 2]);
        // Room for 4 of 5: one would be alone at the top of the next page, so 3 and 2.
        assert_eq!(place(4, true), [3, 2]);
        assert_eq!(place(4, false), [4, 1]);
        // Room for 1 of 5: one would be alone at the bottom, so all five move.
        assert_eq!(place(7, true), [0, 5]);
        assert_eq!(place(7, false), [1, 4]);
    }

    #[test]
    fn keep_lines_moves_a_paragraph_and_splits_one_taller_than_a_page() {
        let document = document(|style| {
            let mut blocks: Vec<Block> =
                (0..6).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(lines(style, 3, spacing(0.0, 0.0).with_keep_lines(true)));
            blocks.push(lines(style, 10, spacing(0.0, 0.0).with_keep_lines(true)));
            one_section(blocks)
        });
        let laid = pages(&document);
        // The three lines go to page 2; the ten lines, taller than a page, start on a new
        // page and are split there.
        assert_eq!(laid[1], [(6, 0..3, 20.0)]);
        assert_eq!(laid[2], [(7, 0..8, 20.0)]);
        assert_eq!(laid[3], [(7, 8..10, 20.0)]);
    }

    #[test]
    fn keep_with_next_moves_the_chain_to_the_page_of_the_next_paragraph() {
        let document = document(|style| {
            let mut blocks: Vec<Block> =
                (0..6).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            // Two headings kept with what follows, which does not fit.
            blocks.push(lines(style, 1, spacing(0.0, 0.0).with_keep_next(true)));
            blocks.push(lines(style, 1, spacing(0.0, 0.0).with_keep_next(true)));
            blocks.push(lines(style, 3, spacing(0.0, 0.0).with_keep_lines(true)));
            one_section(blocks)
        });
        let laid = pages(&document);
        assert_eq!(laid[0].len(), 6);
        assert_eq!(
            laid[1].iter().map(|f| f.0).collect::<Vec<_>>(),
            [6, 7, 8],
            "{laid:?}"
        );
    }

    #[test]
    fn a_chain_kept_with_the_next_that_fits_no_page_is_split_and_reported() {
        let document = document(|style| {
            let mut blocks: Vec<Block> = vec![lines(style, 1, spacing(0.0, 0.0))];
            blocks.extend((0..12).map(|_| lines(style, 1, spacing(0.0, 0.0).with_keep_next(true))));
            blocks.push(lines(style, 1, spacing(0.0, 0.0)));
            one_section(blocks)
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        // Moved once to a page of its own, then split there.
        assert_eq!(laid.pages[0].fragments.len(), 1);
        assert_eq!(laid.pages[1].fragments.len(), 8);
        assert_eq!(laid.pages.len(), 3);
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.kind() == ReportKind::Overflow),
            "{report:?}"
        );
    }

    #[test]
    fn a_chain_kept_with_the_next_that_moves_reports_nothing() {
        let document = document(|style| {
            let mut blocks: Vec<Block> =
                (0..6).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(lines(style, 1, spacing(0.0, 0.0).with_keep_next(true)));
            blocks.push(lines(style, 3, spacing(0.0, 0.0).with_keep_lines(true)));
            one_section(blocks)
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        assert_eq!(laid.pages[1].fragments.len(), 2);
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn a_chain_kept_with_the_next_ends_at_a_section_start_or_a_page_break() {
        // Moving a chain never undoes the page a section starts on, its blank page, its
        // numbering or a page break: keeping with the next does not reach past them.
        let chain = |style, start: SectionStart, break_before: bool| {
            let mut first: Vec<Block> =
                (0..6).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            first.push(lines(style, 1, spacing(0.0, 0.0).with_keep_next(true)));
            let mut second = vec![lines(
                style,
                6,
                spacing(0.0, 0.0)
                    .with_keep_next(true)
                    .with_page_break_before(break_before),
            )];
            second.push(lines(style, 3, spacing(0.0, 0.0).with_keep_lines(true)));
            let numbering = crate::ir::PageNumbering::default().with_start(Some(10));
            if break_before {
                first.extend(second);
                return vec![Section::new(setup()).with_blocks(first)];
            }
            vec![
                Section::new(setup()).with_blocks(first),
                Section::new(setup())
                    .with_start(start)
                    .with_page_numbering(numbering)
                    .with_blocks(second),
            ]
        };
        let summary = |start: SectionStart, break_before: bool| {
            let (laid, _) = laid(&document(|style| chain(style, start, break_before)), 100);
            laid.expect("pages")
                .pages
                .iter()
                .map(|page| {
                    let paragraphs = page.fragments.iter().map(|f| f.paragraph).collect();
                    (page.section, page.number, paragraphs)
                })
                .collect::<Vec<(usize, u32, Vec<usize>)>>()
        };
        // A section on the next page: it starts page 2, numbered 10; its chain moves its
        // last paragraph's companion with it to page 3.
        assert_eq!(
            summary(SectionStart::NextPage, false),
            [
                (0, 1, vec![0, 1, 2, 3, 4, 5, 6]),
                (1, 10, vec![7]),
                (1, 11, vec![8])
            ]
        );
        // On an odd page: page 2 would be even, so it is blank.
        let odd = summary(SectionStart::OddPage, false);
        assert_eq!(odd.len(), 4, "{odd:?}");
        assert_eq!((odd[1].2.len(), odd[2].2.clone()), (0, vec![7]));
        // A continuous section that numbers its pages from 10: its next page is 10.
        let continuous = summary(SectionStart::Continuous, false);
        assert!(continuous.iter().any(|page| page.1 == 10), "{continuous:?}");
        // A page break before: three pages, not four, and the break stays where it is.
        let broken = summary(SectionStart::NextPage, true);
        assert_eq!(broken.len(), 3, "{broken:?}");
        assert_eq!(broken[1].2, [7]);
    }

    #[test]
    fn a_page_break_inside_a_chain_s_paragraph_makes_no_extra_page() {
        // P5, kept with the next, holds "A", a page break and six lines: its last lines
        // start page 2 whatever happens, so moving it would only push "A" onto a page of
        // its own. The three lines kept together that follow do not fit there and go to
        // page 3, and the chain is reported split.
        let with_break = |style, before: usize, after: usize| -> Block {
            let mut inlines = Vec::new();
            for _ in 0..before {
                inlines.push(Inline::text("A", style));
            }
            inlines.push(Inline::page_break(style));
            for line in 0..after {
                if line > 0 {
                    inlines.push(Inline::line_break(BreakClear::None, style));
                }
                inlines.push(Inline::text("A", style));
            }
            Paragraph::new(style)
                .with_props(spacing(0.0, 0.0).with_keep_next(true))
                .with_inlines(inlines)
                .into()
        };
        let inner = document(|style| {
            let mut blocks: Vec<Block> =
                (0..5).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(with_break(style, 1, 6));
            blocks.push(lines(style, 3, spacing(0.0, 0.0).with_keep_lines(true)));
            one_section(blocks)
        });
        let (laid, report) = laid(&inner, 100);
        let laid = laid.expect("pages");
        assert_eq!(laid.pages.len(), 3);
        assert_eq!(laid.pages[0].fragments.last().map(|f| f.paragraph), Some(5));
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.kind() == ReportKind::Overflow),
            "{report:?}"
        );
        // A heading that starts with a page break, kept with ten lines kept together:
        // four pages, as without keeping with the next.
        let leading = document(|style| {
            let mut blocks: Vec<Block> =
                (0..3).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(with_break(style, 0, 1));
            blocks.push(lines(style, 10, spacing(0.0, 0.0).with_keep_lines(true)));
            one_section(blocks)
        });
        assert_eq!(pages(&leading).len(), 4);
        // A column break after two lines, in one column a page break: three pages again.
        let column = document(|style| {
            let mut blocks: Vec<Block> =
                (0..5).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            let mut inlines = vec![
                Inline::text("A", style),
                Inline::line_break(BreakClear::None, style),
                Inline::text("A", style),
                Inline::column_break(style),
            ];
            for line in 0..6 {
                if line > 0 {
                    inlines.push(Inline::line_break(BreakClear::None, style));
                }
                inlines.push(Inline::text("A", style));
            }
            blocks.push(
                Paragraph::new(style)
                    .with_props(spacing(0.0, 0.0).with_keep_next(true))
                    .with_inlines(inlines)
                    .into(),
            );
            blocks.push(lines(style, 3, spacing(0.0, 0.0).with_keep_lines(true)));
            one_section(blocks)
        });
        let laid = pages(&column);
        assert_eq!(laid.len(), 3, "{laid:?}");
        assert_eq!(laid[0].last(), Some(&(5, 0..2, 120.0)));
    }

    #[test]
    fn keeping_with_the_next_waits_in_table_cells() {
        // A cell's paragraph kept with the next stays on page 1 when the paragraph after
        // the table goes to page 2.
        let document = document(|style| {
            let mut blocks: Vec<Block> =
                (0..6).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            let cell = Cell::default().with_blocks([lines(
                style,
                1,
                spacing(0.0, 0.0).with_keep_next(true),
            )]);
            blocks.push(
                Table::default()
                    .with_rows(vec![Row::default().with_cells(vec![cell])])
                    .into(),
            );
            blocks.push(lines(style, 3, spacing(0.0, 0.0).with_keep_lines(true)));
            one_section(blocks)
        });
        let laid = pages(&document);
        assert_eq!(laid[0].last(), Some(&(6, 0..1, 140.0)));
        assert_eq!(laid[1], [(7, 0..3, 20.0)]);
    }

    #[test]
    fn the_first_section_starts_on_page_1_whatever_its_start() {
        let document = document(|style| {
            vec![
                Section::new(setup())
                    .with_start(SectionStart::EvenPage)
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let laid = laid.expect("pages");
        assert_eq!(laid.pages.len(), 1);
        assert_eq!(laid.pages[0].number, 1);
    }

    #[test]
    fn space_above_that_does_not_fit_below_text_moves_its_paragraph() {
        // Under three lines, 200 pt above a paragraph do not fit: it moves to page 2,
        // where the space is dropped at the top, rather than being squeezed in.
        let document = document(|style| {
            let mut blocks: Vec<Block> =
                (0..3).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(lines(style, 1, spacing(200.0, 0.0)));
            one_section(blocks)
        });
        assert_eq!(pages(&document)[1], [(3, 0..1, 20.0)]);
    }

    #[test]
    fn keep_rules_and_widow_control_wait_in_table_cells() {
        // In a cell, a paragraph kept together that does not fit is split, and widow
        // control leaves one line at the top of the next page (MS-OI29500 2.1.46 a,
        // 2.1.66 a); keeping with the next waits for tables.
        let cell_paragraph = |style, count, props: ParagraphProps| -> Block {
            let cell = Cell::default().with_blocks([lines(style, count, props)]);
            Table::default()
                .with_rows(vec![Row::default().with_cells(vec![cell])])
                .into()
        };
        let kept = document(|style| {
            let mut blocks: Vec<Block> =
                (0..6).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(cell_paragraph(
                style,
                3,
                spacing(0.0, 0.0).with_keep_lines(true),
            ));
            one_section(blocks)
        });
        assert_eq!(pages(&kept)[0].last(), Some(&(6, 0..2, 140.0)));
        let widowed = document(|style| {
            let mut blocks: Vec<Block> =
                (0..5).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(cell_paragraph(style, 4, spacing(0.0, 0.0)));
            one_section(blocks)
        });
        assert_eq!(pages(&widowed)[1], [(5, 3..4, 20.0)]);
    }

    #[test]
    fn a_page_break_before_the_first_paragraph_of_a_page_makes_no_page() {
        let document = document(|style| {
            one_section(vec![lines(
                style,
                1,
                spacing(0.0, 0.0).with_page_break_before(true),
            )])
        });
        assert_eq!(pages(&document), [vec![(0, 0..1, 20.0)]]);
    }

    #[test]
    fn a_next_column_section_starts_a_new_page_in_one_column() {
        let document = document(|style| {
            vec![
                Section::new(setup()).with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
                Section::new(setup())
                    .with_start(SectionStart::NextColumn)
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        assert_eq!(pages(&document).len(), 2);
    }

    #[test]
    fn space_above_never_pushes_a_first_line_off_its_page() {
        // 500 pt above the first paragraph of the document, on a body 160 pt high.
        let document = document(|style| one_section(vec![lines(style, 1, spacing(500.0, 0.0))]));
        let (laid, report) = laid(&document, 100);
        assert_eq!(laid.expect("pages").pages[0].fragments[0].top, Pt(160.0));
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn columns_vertical_alignment_and_a_page_break_s_mark_are_reported() {
        use crate::ir::{Columns, CompatFlag, DocumentSettings, PageVerticalAlign};
        let kinds = |settings: DocumentSettings| {
            let mut document = Document::new(settings);
            let style = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(10.0)),
            );
            let ending_in_break = Paragraph::new(style)
                .with_inlines([Inline::text("A", style), Inline::page_break(style)]);
            document.add_section(
                Section::new(setup())
                    .with_columns(Columns::default().with_count(2))
                    .with_vertical_align(PageVerticalAlign::Center)
                    .with_blocks(vec![
                        ending_in_break.into(),
                        lines(style, 1, spacing(0.0, 0.0)),
                    ]),
            );
            let (_, report) = laid(&document, 100);
            report
                .entries()
                .iter()
                .map(|entry| entry.message().to_owned())
                .collect::<Vec<_>>()
        };
        let older = kinds(DocumentSettings::default());
        for expected in ["columns", "vertical alignment", "paragraph mark"] {
            assert!(
                older.iter().any(|message| message.contains(expected)),
                "{older:?}"
            );
        }
        // Word 2013 and later, and documents with `splitPgBreakAndParaMark`, keep the mark
        // with the break themselves.
        let newer = kinds(DocumentSettings::default().with_compat_mode(CompatMode::Word2013));
        assert!(
            !newer
                .iter()
                .any(|message| message.contains("paragraph mark"))
        );
        let flagged = kinds(
            DocumentSettings::default().with_compat_flags([CompatFlag::SplitPgBreakAndParaMark]),
        );
        assert!(
            !flagged
                .iter()
                .any(|message| message.contains("paragraph mark"))
        );
    }

    #[test]
    fn tables_nest_as_deep_as_the_ir_allows() {
        let nested = |depth: usize| {
            document(move |style| {
                // A cell ends with a paragraph, and a table has a grid, as the IR wants.
                let mut block = lines(style, 1, spacing(0.0, 0.0));
                for _ in 0..depth {
                    let blocks = vec![block, lines(style, 1, spacing(0.0, 0.0))];
                    let cell = Cell::default().with_blocks(blocks);
                    block = Table::default()
                        .with_grid([Pt(10.0)])
                        .with_rows(vec![Row::default().with_cells(vec![cell])])
                        .into();
                }
                one_section(vec![block])
            })
        };
        let deepest = nested(MAX_TABLE_DEPTH);
        assert!(deepest.validate().is_ok());
        assert!(laid(&deepest, 100).0.is_ok());
        assert!(matches!(
            laid(&nested(MAX_TABLE_DEPTH + 1), 100).0,
            Err(Error::NestingTooDeep { .. })
        ));
    }

    #[test]
    fn too_many_pages_is_a_limit_exceeded() {
        let core = officewright_core::Error::from(Error::TooManyPages { limit: 7 });
        assert!(
            matches!(&core, officewright_core::Error::LimitExceeded(message) if message.contains('7')),
            "{core:?}"
        );
    }

    #[test]
    fn page_breaks_inside_a_paragraph_continue_it_on_the_next_page() {
        let document = document(|style| {
            one_section(vec![
                Paragraph::new(style)
                    .with_props(spacing(0.0, 0.0))
                    .with_inlines([
                        Inline::text("A", style),
                        Inline::page_break(style),
                        Inline::text("A", style),
                        Inline::column_break(style),
                        Inline::text("A", style),
                    ])
                    .into(),
            ])
        });
        assert_eq!(
            pages(&document),
            [
                vec![(0, 0..1, 20.0)],
                vec![(0, 1..2, 20.0)],
                vec![(0, 2..3, 20.0)]
            ]
        );
    }

    #[test]
    fn sections_start_on_a_new_page_on_the_same_one_or_after_a_blank_page() {
        let document = document(|style| {
            let other = setup().with_size(PageSize::new(Pt(400.0), Pt(200.0)));
            vec![
                Section::new(setup()).with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
                // Continuous with the same setup: on the same page.
                Section::new(setup())
                    .with_start(SectionStart::Continuous)
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
                // On an odd page: page 2 is even, so a blank page comes first.
                Section::new(setup())
                    .with_start(SectionStart::OddPage)
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
                // Continuous with another size: on a new page, reported.
                Section::new(other)
                    .with_start(SectionStart::Continuous)
                    .with_page_numbering(crate::ir::PageNumbering::default().with_start(Some(10)))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        let summary: Vec<(usize, u32, usize, f64)> = laid
            .pages
            .iter()
            .map(|p| (p.section, p.number, p.fragments.len(), p.width.0))
            .collect();
        // The first page keeps the section it started in, though the continuous section
        // 1 starts on it.
        assert_eq!(
            summary,
            [
                (0, 1, 2, 300.0),
                (2, 2, 0, 300.0),
                (2, 3, 1, 300.0),
                (3, 10, 1, 400.0)
            ]
        );
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.kind() == ReportKind::FeatureUnsupported
                    && entry.message().contains("continuous")),
            "{report:?}"
        );
    }

    #[test]
    fn a_page_break_at_the_end_of_a_paragraph_makes_a_page_only_for_what_follows() {
        let ending_in_break = |style| -> Block {
            Paragraph::new(style)
                .with_props(spacing(0.0, 0.0))
                .with_inlines([Inline::text("A", style), Inline::page_break(style)])
                .into()
        };
        // Before a section that starts on a new page: no empty page between. At the end of
        // the document: no empty page after. Before a continuous section, here of another
        // page size: its paragraph starts the new page, of the section's size, and the
        // space above it is dropped there, as after any page break; nothing is reported.
        let document = document(|style| {
            vec![
                Section::new(setup()).with_blocks(vec![ending_in_break(style)]),
                Section::new(setup()).with_blocks(vec![ending_in_break(style)]),
                Section::new(setup().with_size(PageSize::new(Pt(400.0), Pt(200.0))))
                    .with_start(SectionStart::Continuous)
                    .with_blocks(vec![
                        lines(style, 1, spacing(24.0, 0.0)),
                        ending_in_break(style),
                    ]),
            ]
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        assert!(
            !report
                .entries()
                .iter()
                .any(|entry| entry.message().contains("continuous")),
            "{report:?}"
        );
        assert_eq!(laid.pages.last().map(|page| page.width), Some(Pt(400.0)));
        assert_eq!(
            pages(&document),
            [
                vec![(0, 0..1, 20.0)],
                vec![(1, 0..1, 20.0)],
                vec![(2, 0..1, 20.0), (3, 0..1, 40.0)]
            ]
        );
    }

    #[test]
    fn a_line_taller_than_the_body_takes_a_page_of_its_own_and_is_reported() {
        let short =
            setup().with_margins(Margins::default().with_top(Pt(95.0)).with_bottom(Pt(95.0)));
        let document = document(|style| {
            vec![Section::new(short).with_blocks(vec![lines(style, 3, spacing(0.0, 0.0))])]
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        assert_eq!(laid.pages.len(), 3);
        assert!(laid.pages.iter().all(|page| page.fragments.len() == 1));
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.kind() == ReportKind::Overflow),
            "{report:?}"
        );
    }

    #[test]
    fn more_pages_than_allowed_is_an_error() {
        let document = document(|style| {
            one_section(
                (0..20)
                    .map(|_| lines(style, 1, spacing(0.0, 0.0)))
                    .collect(),
            )
        });
        let (two, _) = laid(&document, 2);
        assert!(
            matches!(two, Err(Error::TooManyPages { limit: 2 })),
            "{two:?}"
        );
        assert!(laid(&document, 3).0.is_ok());
    }

    #[test]
    fn tables_are_their_cells_paragraphs_until_they_are_laid_out() {
        let document = document(|style| {
            let cell =
                |count| Cell::default().with_blocks([lines(style, count, spacing(0.0, 0.0))]);
            let table =
                Table::default().with_rows(vec![Row::default().with_cells(vec![cell(1), cell(2)])]);
            one_section(vec![table.into(), lines(style, 1, spacing(0.0, 0.0))])
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        assert_eq!(laid.lines, [1, 2, 1]);
        assert_eq!(laid.pages[0].fragments.len(), 3);
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.kind() == ReportKind::FeatureUnsupported
                    && entry.message().contains("tables")),
            "{report:?}"
        );
    }

    #[test]
    fn paragraph_shading_is_behind_each_page_s_lines_from_the_outermost_indent() {
        use crate::ir::{Border, ColorValue, Indent, ParagraphBorders, Shading};
        let grey = officewright_core::Color::rgb(200, 200, 200);
        let document = document(|style| {
            let mut blocks: Vec<Block> =
                (0..6).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            let shaded = spacing(0.0, 0.0)
                .with_widow_control(false)
                .with_indent(
                    Indent::default()
                        .with_start(Pt(10.0))
                        .with_end(Pt(30.0))
                        .with_first_line(Pt(-15.0)),
                )
                .with_shading(Some(Shading::default().with_fill(ColorValue::Rgb(grey))))
                .with_borders(ParagraphBorders::default().with_top(Some(Border::default())));
            blocks.push(lines(style, 4, shaded));
            one_section(blocks)
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        let boxes: Vec<Vec<(Point, Pt, Pt)>> = laid
            .pages
            .iter()
            .map(|page| {
                page.items
                    .iter()
                    .filter_map(|item| match item {
                        DisplayItem::Rect {
                            at,
                            width,
                            height,
                            color,
                        } if *color == grey => Some((*at, *width, *height)),
                        _ => None,
                    })
                    .collect()
            })
            .collect();
        // Two lines on each page; from 5 pt into the margin side of the start indent
        // (10 - 15) to 30 pt before the end of the 260 pt column.
        let left = Pt(20.0 - 5.0);
        let width = Pt(260.0 - 30.0 + 5.0);
        assert_eq!(
            boxes,
            [
                vec![(
                    Point {
                        x: left,
                        y: Pt(140.0)
                    },
                    width,
                    Pt(40.0)
                )],
                vec![(
                    Point {
                        x: left,
                        y: Pt(20.0)
                    },
                    width,
                    Pt(40.0)
                )]
            ]
        );
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.message().contains("borders")),
            "{report:?}"
        );
    }

    /// A header or footer of `count` lines of 20 pt.
    fn frame_of(style: RunStyleId, count: usize) -> Vec<Block> {
        vec![lines(style, count, spacing(0.0, 0.0))]
    }

    /// The setup of [`setup`] with a header and footer distance of 10 pt.
    fn close_frames() -> PageSetup {
        setup().with_margins(
            Margins::default()
                .with_top(Pt(20.0))
                .with_bottom(Pt(20.0))
                .with_left(Pt(20.0))
                .with_right(Pt(20.0))
                .with_header(Pt(10.0))
                .with_footer(Pt(10.0)),
        )
    }

    /// The texts the glyph runs of `page` stand for, top to bottom.
    fn texts(page: &Page) -> Vec<String> {
        page.items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Glyphs(run) => {
                    Some(run.clusters.iter().map(|c| c.text.as_str()).collect())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn headers_push_the_body_down_and_footers_up_unless_the_margin_is_fixed() {
        let document = |top: f64| {
            document(|style| {
                let margins = close_frames().margins().with_top(Pt(top));
                vec![
                    Section::new(close_frames().with_margins(margins))
                        .with_headers(HeaderFooterSet::default().with_primary(frame_of(style, 3)))
                        .with_footers(HeaderFooterSet::default().with_primary(frame_of(style, 2)))
                        .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
                ]
            })
        };
        let (first, _) = laid(&document(20.0), 100);
        let page = &first.expect("pages").pages[0];
        // The header's 60 pt from 10 pt reach 70, past the top margin; the footer's 40 pt
        // end 10 pt above the page's bottom at 190, so it starts at 150.
        assert_eq!((page.body.top, page.body.bottom), (Pt(70.0), Pt(150.0)));
        assert_eq!(page.fragments[0].top, Pt(70.0));
        // A negative top margin is fixed: the body starts 20 pt down, under the header.
        let (fixed, _) = laid(&document(-20.0), 100);
        assert_eq!(fixed.expect("pages").pages[0].body.top, Pt(20.0));
    }

    #[test]
    fn a_header_s_height_takes_the_space_around_its_paragraphs() {
        // A header line of 20 pt, 3 pt above it and 5 pt below, from 10 pt: the body
        // starts at 38 (hypothesis, `E-header-space-after.docx`).
        let document = document(|style| {
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary(vec![lines(
                        style,
                        1,
                        spacing(3.0, 5.0),
                    )]))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, _) = laid(&document, 100);
        assert_eq!(laid.expect("pages").pages[0].body.top, Pt(38.0));
    }

    #[test]
    fn pages_take_the_first_even_or_primary_header_and_blank_pages_none() {
        // Headers of 1, 2 and 3 lines of 20 pt at 10 pt put the body at 30, 50 and 70.
        let tops = |title_page: bool, even_and_odd: bool| {
            let mut document = Document::new(
                crate::ir::DocumentSettings::default().with_even_and_odd_headers(even_and_odd),
            );
            let style = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(10.0)),
            );
            let headers = HeaderFooterSet::default()
                .with_first(frame_of(style, 1))
                .with_even(frame_of(style, 2))
                .with_primary(frame_of(style, 3));
            let blocks: Vec<Block> = (0..3)
                .map(|_| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(true)))
                .collect();
            document.add_section(
                Section::new(close_frames())
                    .with_title_page(title_page)
                    .with_headers(headers.clone())
                    .with_blocks(blocks),
            );
            // An odd-page section after page 3: page 4 is blank, and page 5 is the first
            // of the section.
            document.add_section(
                Section::new(close_frames())
                    .with_start(SectionStart::OddPage)
                    .with_title_page(title_page)
                    .with_headers(headers)
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            );
            let (laid, _) = laid(&document, 100);
            laid.expect("pages")
                .pages
                .iter()
                .map(|page| (page.body.top.0, page.blank))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            tops(false, false),
            [
                (70.0, false),
                (70.0, false),
                (70.0, false),
                (20.0, true),
                (70.0, false)
            ]
        );
        assert_eq!(
            tops(true, true),
            [
                (30.0, false),
                (50.0, false),
                (70.0, false),
                (20.0, true),
                (30.0, false)
            ]
        );
    }

    #[test]
    fn page_fields_show_the_page_the_pages_and_the_section_s_pages() {
        use crate::ir::{Field, FieldKind, NumberFormat, PageNumbering};
        let field = |kind, style, format: Option<NumberFormat>| {
            Inline::Field(Field::new(kind, style).with_format(format))
        };
        let document = document(|style| {
            let footer = |format| {
                vec![
                    Paragraph::new(style)
                        .with_props(spacing(0.0, 0.0))
                        .with_inlines([
                            field(FieldKind::Page, style, format),
                            Inline::text(" ", style),
                            field(FieldKind::NumPages, style, None),
                            Inline::text(" ", style),
                            field(FieldKind::SectionPages, style, None),
                        ])
                        .into(),
                ]
            };
            let pages = |count: usize| -> Vec<Block> {
                (0..count)
                    .map(|_| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(true)))
                    .collect()
            };
            vec![
                Section::new(close_frames())
                    .with_footers(HeaderFooterSet::default().with_primary(footer(None)))
                    .with_page_numbering(
                        PageNumbering::default().with_format(NumberFormat::UpperRoman),
                    )
                    .with_blocks(pages(2)),
                Section::new(close_frames())
                    .with_footers(
                        HeaderFooterSet::default()
                            .with_primary(footer(Some(NumberFormat::LowerLetter))),
                    )
                    .with_blocks(pages(3)),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let laid = laid.expect("pages");
        let footers: Vec<String> = laid
            .pages
            .iter()
            .map(|page| texts(page).first().cloned().unwrap_or_default())
            .collect();
        // Section 1 numbers its pages in capital Roman numerals; section 2 continues the
        // numbers, and its field writes them as letters.
        assert_eq!(footers, ["I 5 2", "II 5 2", "c 5 3", "d 5 3", "e 5 3"]);
    }

    /// A paragraph of the fields `kinds`, in the test font.
    fn fields(style: RunStyleId, kinds: Vec<crate::ir::FieldKind>) -> Block {
        Paragraph::new(style)
            .with_props(spacing(0.0, 0.0))
            .with_inlines(
                kinds
                    .into_iter()
                    .map(|kind| Inline::Field(crate::ir::Field::new(kind, style))),
            )
            .into()
    }

    /// The messages of `report`.
    fn messages(report: &LayoutReport) -> Vec<String> {
        report
            .entries()
            .iter()
            .map(|entry| entry.message().to_owned())
            .collect()
    }

    #[test]
    fn headers_and_footers_are_drawn_where_their_distances_put_them() {
        // One exact 20 pt line each: the header's from 10 pt down, its baseline at 26;
        // the footer's ending 10 pt above the bottom of the 200 pt page, its baseline at
        // 186.
        let document = document(|style| {
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary(frame_of(style, 1)))
                    .with_footers(HeaderFooterSet::default().with_primary(frame_of(style, 1)))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let page = &laid.expect("pages").pages[0];
        let baselines: Vec<f64> = page
            .items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Glyphs(run) => Some(run.origin.y.0),
                _ => None,
            })
            .collect();
        assert_eq!(
            baselines.get(..2),
            Some(&[26.0, 186.0][..]),
            "{baselines:?}"
        );
    }

    #[test]
    fn a_negative_bottom_margin_is_fixed_whatever_the_footer() {
        let document = document(|style| {
            let margins = close_frames().margins().with_bottom(Pt(-20.0));
            vec![
                Section::new(close_frames().with_margins(margins))
                    .with_footers(HeaderFooterSet::default().with_primary(frame_of(style, 3)))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, _) = laid(&document, 100);
        assert_eq!(laid.expect("pages").pages[0].body.bottom, Pt(180.0));
    }

    #[test]
    fn a_frame_s_paragraphs_have_the_space_between_them() {
        // 5 pt below the first and 8 above the second make 8: 20 + 8 + 20 from 10 pt.
        let document = document(|style| {
            let header = vec![
                lines(style, 1, spacing(0.0, 5.0)),
                lines(style, 1, spacing(8.0, 0.0)),
            ];
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary(header))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, _) = laid(&document, 100);
        assert_eq!(laid.expect("pages").pages[0].body.top, Pt(58.0));
    }

    #[test]
    fn a_page_field_in_the_body_shows_the_page_its_paragraph_lands_on() {
        use crate::ir::FieldKind;
        // Eight lines fill page 1: the paragraph of the field lands on page 2, laid out
        // for page 1 first.
        let document = document(|style| {
            let mut blocks: Vec<Block> =
                (0..8).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(fields(style, vec![FieldKind::Page]));
            one_section(blocks)
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        assert_eq!(texts(&laid.pages[1]), ["2"]);
        assert!(
            !messages(&report).iter().any(|m| m.contains("PAGE")),
            "{report:?}"
        );
    }

    #[test]
    fn page_counts_are_found_wherever_their_fields_are() {
        use crate::ir::FieldKind;
        // NUMPAGES only in a first-page header, only in the body, and only in a table:
        // each shows the pages: three, and four where the field's paragraph is on a page
        // before the three.
        let three = |style| -> Vec<Block> {
            (0..3)
                .map(|_| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(true)))
                .collect()
        };
        let in_header = document(|style| {
            vec![
                Section::new(close_frames())
                    .with_title_page(true)
                    .with_headers(
                        HeaderFooterSet::default()
                            .with_first(vec![fields(style, vec![FieldKind::NumPages])]),
                    )
                    .with_blocks(three(style)),
            ]
        });
        let (header_laid, _) = laid(&in_header, 100);
        assert_eq!(
            texts(&header_laid.expect("pages").pages[0])
                .first()
                .map(String::as_str),
            Some("3")
        );
        let in_body = document(|style| {
            let mut blocks = vec![fields(style, vec![FieldKind::NumPages])];
            blocks.extend(three(style));
            one_section(blocks)
        });
        let (body_laid, _) = laid(&in_body, 100);
        assert_eq!(
            texts(&body_laid.expect("pages").pages[0])
                .first()
                .map(String::as_str),
            Some("4")
        );
        let in_table = document(|style| {
            let cell = Cell::default().with_blocks([fields(style, vec![FieldKind::NumPages])]);
            let mut blocks: Vec<Block> = vec![
                Table::default()
                    .with_rows(vec![Row::default().with_cells(vec![cell])])
                    .into(),
            ];
            blocks.extend(three(style));
            one_section(blocks)
        });
        let (table_laid, _) = laid(&in_table, 100);
        assert_eq!(
            texts(&table_laid.expect("pages").pages[0])
                .first()
                .map(String::as_str),
            Some("4")
        );
    }

    #[test]
    fn page_counts_that_keep_changing_are_reported() {
        use crate::ir::{FieldKind, NumberFormat};
        // A body 12 pt wide and 20 pt high: one line a page. "III" wraps, "IV" does not, so
        // the count goes 1, 3, 4, 3 and never settles.
        let document = document(|style| {
            let narrow = PageSetup::new(PageSize::new(Pt(52.0), Pt(60.0))).with_margins(
                Margins::default()
                    .with_top(Pt(20.0))
                    .with_bottom(Pt(20.0))
                    .with_left(Pt(20.0))
                    .with_right(Pt(20.0)),
            );
            let roman = Paragraph::new(style).with_inlines([Inline::Field(
                crate::ir::Field::new(FieldKind::NumPages, style)
                    .with_format(Some(NumberFormat::UpperRoman)),
            )]);
            vec![Section::new(narrow).with_blocks(vec![
                Paragraph::new(style).with_inlines([Inline::text("A", style)]).into(),
                Paragraph::new(style).with_inlines([Inline::text("A", style)]).into(),
                roman.into(),
            ])]
        });
        let (laid, report) = laid(&document, 100);
        assert!(laid.is_ok());
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.kind() == ReportKind::FieldCached
                    && entry.message().contains("changed")),
            "{report:?}"
        );
    }

    #[test]
    fn what_frames_and_page_numbers_leave_out_is_reported() {
        use crate::ir::{Border, FieldKind, NumberFormat, PageNumbering, ParagraphBorders};
        let document = document(|style| {
            let bordered = lines(
                style,
                1,
                spacing(0.0, 0.0)
                    .with_borders(ParagraphBorders::default().with_bottom(Some(Border::default()))),
            );
            let cell = Cell::default().with_blocks([lines(style, 1, spacing(0.0, 0.0))]);
            let table: Block = Table::default()
                .with_rows(vec![Row::default().with_cells(vec![cell])])
                .into();
            let hebrew = Paragraph::new(style).with_inlines([Inline::Field(
                crate::ir::Field::new(FieldKind::Page, style)
                    .with_format(Some(NumberFormat::Other("hebrew1".into()))),
            )]);
            vec![
                Section::new(close_frames())
                    .with_page_numbering(PageNumbering::default().with_chapter_style(Some(1)))
                    .with_headers(HeaderFooterSet::default().with_primary(vec![bordered, table]))
                    .with_footers(HeaderFooterSet::default().with_primary(vec![hebrew.into()]))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (_, report) = laid(&document, 100);
        let all = messages(&report);
        for expected in ["borders", "tables", "decimal", "chapter"] {
            assert!(
                all.iter().any(|m| m.contains(expected)),
                "{expected}: {all:?}"
            );
        }
    }

    #[test]
    fn a_header_taller_than_its_page_is_cut_and_reported() {
        // Thirty lines of 20 pt on a page 200 pt high: what fits is laid out, and the
        // body has no room.
        let document = document(|style| {
            vec![
                Section::new(close_frames())
                    .with_headers(
                        HeaderFooterSet::default().with_primary(
                            (0..30)
                                .map(|_| lines(style, 1, spacing(0.0, 0.0)))
                                .collect::<Vec<Block>>(),
                        ),
                    )
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, report) = laid(&document, 100);
        let page = &laid.expect("pages").pages[0];
        let header_lines = texts(page).len() - 1;
        assert!(header_lines <= 10, "{header_lines}");
        let all = messages(&report);
        assert!(all.iter().any(|m| m.contains("cut")), "{all:?}");
        assert!(all.iter().any(|m| m.contains("no room")), "{all:?}");
    }

    #[test]
    fn numpages_leaves_out_a_title_page_numbered_0() {
        use crate::ir::{Field, FieldKind, PageNumbering};
        let document = document(|style| {
            let footer = vec![
                Paragraph::new(style)
                    .with_inlines([Inline::Field(Field::new(FieldKind::NumPages, style))])
                    .into(),
            ];
            vec![
                Section::new(close_frames())
                    .with_title_page(true)
                    .with_page_numbering(PageNumbering::default().with_start(Some(0)))
                    .with_footers(
                        HeaderFooterSet::default()
                            .with_first(footer.clone())
                            .with_primary(footer),
                    )
                    .with_blocks(
                        (0..3)
                            .map(|_| {
                                lines(style, 1, spacing(0.0, 0.0).with_page_break_before(true))
                            })
                            .collect::<Vec<Block>>(),
                    ),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let laid = laid.expect("pages");
        let numbers: Vec<u32> = laid.pages.iter().map(|page| page.number).collect();
        assert_eq!(numbers, [0, 1, 2]);
        assert!(
            laid.pages
                .iter()
                .all(|page| texts(page).first().map(String::as_str) == Some("2"))
        );
    }

    #[test]
    fn other_fields_show_their_stored_result_and_are_reported() {
        use crate::ir::{Field, FieldKind};
        let document = document(|style| {
            let date = Field::new(FieldKind::Other("DATE".into()), style)
                .with_result([Inline::text("A A", style)]);
            one_section(vec![
                Paragraph::new(style)
                    .with_inlines([Inline::Field(date)])
                    .into(),
            ])
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        assert_eq!(texts(&laid.pages[0]).concat(), "A A");
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.kind() == ReportKind::FieldCached),
            "{report:?}"
        );
    }

    #[test]
    fn a_document_without_sections_has_one_empty_page() {
        let (laid, _) = laid(&Document::default(), 100);
        let laid = laid.expect("pages");
        assert_eq!(laid.pages.len(), 1);
        assert!(laid.pages[0].fragments.is_empty());
    }
}
