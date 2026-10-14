//! Pagination: the body text of every section flowed onto pages, each page with the
//! display list of what it draws.

// reason: the render entry point (plan PR 8c) uses this; until then only the tests and
// the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::collections::HashSet;
use std::ops::Range;

use officewright_core::Pt;

use crate::display::{DisplayItem, LeaderBudget, Painting, Point};
use crate::error::{Error, Result};
use crate::fonts::Fonts;
use crate::ir::{
    Block, CompatFlag, CompatMode, Document, MAX_TABLE_DEPTH, PageSetup, PageVerticalAlign,
    Paragraph, Section, SectionStart,
};
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
    /// The parts of paragraphs on it, top to bottom.
    pub(crate) fragments: Vec<Fragment>,
    pub(crate) items: Vec<DisplayItem>,
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
    let mut pager = Pager {
        document,
        fonts,
        shaper,
        max_pages,
        pages: Vec::new(),
        cursor: 0.0,
        above: None,
        next_number: 1,
        leaders: LeaderBudget::default(),
    };
    let lines = pager.run(&flow)?;
    if pager.pages.is_empty() {
        pager.new_page(0, &PageSetup::default())?;
    }
    Ok(Paginated {
        pages: pager.pages,
        lines,
    })
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
}

impl Pager<'_, '_, '_> {
    /// Places every paragraph of `flow`, section by section. Returns the number of lines
    /// of each.
    fn run(&mut self, flow: &[Flowed<'_>]) -> Result<Vec<usize>> {
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
            let layout = match layouts.get_mut(index) {
                Some(Some(layout)) => layout,
                Some(slot) => slot.insert(paragraph::lay_out(
                    self.document,
                    flowed.paragraph,
                    self.body()?.width,
                    self.fonts,
                    self.shaper,
                    &flowed.location,
                )),
                None => break,
            };
            let keeps_space = kept_top || index == 0 || flowed.first_in_cell;
            let (first_page, ends_with_break) = self.place(flow, index, layout, keeps_space)?;
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
                    self.new_page(index, section.page())?;
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
        if self.pages.len() >= self.max_pages {
            return Err(Error::TooManyPages {
                limit: self.max_pages,
            });
        }
        let body = body(setup);
        self.pages.push(Page {
            section,
            width: setup.size().width(),
            height: setup.size().height(),
            number: self.next_number,
            body,
            fragments: Vec::new(),
            items: Vec::new(),
        });
        self.next_number = self.next_number.saturating_add(1);
        self.cursor = body.top.0;
        self.above = None;
        Ok(())
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
        BreakClear, Cell, Inline, LineSpacing, Margins, ParagraphProps, Row, RunFonts, RunStyle,
        RunStyleId, Spacing, Table,
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

    #[test]
    fn a_document_without_sections_has_one_empty_page() {
        let (laid, _) = laid(&Document::default(), 100);
        let laid = laid.expect("pages");
        assert_eq!(laid.pages.len(), 1);
        assert!(laid.pages[0].fragments.is_empty());
    }
}
