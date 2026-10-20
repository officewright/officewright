//! Pagination: the body text of every section flowed onto pages, each page with the
//! display list of what it draws.

// reason: the render entry point (plan PR 8c) uses this; until then only the tests and
// the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use officewright_core::Pt;

use crate::display::{self, DisplayItem, Float, LeaderBudget, PageBox, Painting, Point};
use crate::error::{Error, Result};
use crate::fonts::Fonts;
use crate::ir::{
    Block, CompatFlag, CompatMode, Document, FieldKind, Inline, MAX_TABLE_DEPTH, PageSetup,
    PageVerticalAlign, Paragraph, ParagraphProps, Section, SectionStart, Table,
};
use crate::itemize::{self, FieldValues};
use crate::lines::{LineEnd, PieceKind};
use crate::lists::{Counters, Label};
use crate::paragraph::{self, ParagraphLayout};
use crate::report::{Location, ReportKind, Step};
use crate::shape::Shaper;
use crate::tables::{self, Context, Drawn, Prepared};

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
    /// What it draws: its header and footer with their floating objects behind or in
    /// front of them, then its body; once it goes to the sink, the body's floating
    /// objects too, behind or in front of the body's text. The header and footer are a
    /// layer under the body, its floating objects behind the text included, as a public
    /// description of Word's layers says (Microsoft Q&A, "the header/footer layer, which
    /// will always be behind everything"). Hypothesis until a Word render (#314).
    pub(crate) items: Vec<DisplayItem>,
    /// How many of `items` are the header's and footer's.
    pub(crate) frame_items: usize,
    /// The floating objects its body anchors, until they are stacked into `items`.
    pub(crate) floats: Vec<Float>,
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

/// Lines of a paragraph, or bands of a table, on a page.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fragment {
    /// The paragraph or table, by its place in the flow: every paragraph and table of
    /// the body in reading order.
    pub(crate) paragraph: usize,
    /// Its lines, or for a table its bands: rows that a vertically merged cell ties
    /// together.
    pub(crate) lines: Range<usize>,
    /// The top of its first line.
    pub(crate) top: Pt,
    /// The height of its lines.
    pub(crate) height: Pt,
}

/// What an item of the flow is.
pub(crate) enum Content<'a> {
    /// A paragraph, with its list label when it is a list item, as the counters of its
    /// story make it.
    Paragraph(&'a Paragraph, Option<Label>),
    /// A table, with the list labels of its paragraphs, nested tables' included, in
    /// reading order.
    Table(&'a Table, Vec<Option<Label>>),
}

/// A paragraph or a table of the flow, with where it comes from.
pub(crate) struct Flowed<'a> {
    /// Its section, by its place in the document.
    pub(crate) section: usize,
    pub(crate) content: Content<'a>,
    pub(crate) location: Location,
}

impl<'a> Flowed<'a> {
    /// The paragraph, unless it is a table.
    pub(crate) fn paragraph(&self) -> Option<&'a Paragraph> {
        match self.content {
            Content::Paragraph(paragraph, _) => Some(paragraph),
            Content::Table(..) => None,
        }
    }
}

/// The paragraphs and tables of the body of every section of `document`, in reading
/// order.
pub(crate) fn flow<'a>(document: &'a Document, fonts: &mut Fonts<'_>) -> Result<Vec<Flowed<'a>>> {
    let mut out = Vec::new();
    let mut location = Location::default();
    // The lists of the body count on from section to section.
    let mut counters = Counters::default();
    location.push(Step::Field("sections"));
    for (index, section) in document.sections().iter().enumerate() {
        location.push(Step::Index(index));
        location.push(Step::Field("blocks"));
        let mut walk = Walk {
            document,
            section: index,
            location: &mut location,
            counters: &mut counters,
            out: &mut out,
        };
        walk.blocks(section.blocks())?;
        location.pop();
        location.pop();
    }
    report_list_formats(&counters, fonts);
    Ok(out)
}

/// A walk over the blocks of a section for [`flow`].
struct Walk<'a, 'w> {
    document: &'a Document,
    section: usize,
    location: &'w mut Location,
    /// The counters of the lists of the story walked.
    counters: &'w mut Counters,
    out: &'w mut Vec<Flowed<'a>>,
}

/// Reports, once, at the first of them, labels with a number written in decimal because
/// its format is one the layout does not write.
fn report_list_formats(counters: &Counters, fonts: &mut Fonts<'_>) {
    if let Some(location) = &counters.unsupported_format {
        fonts.report_once(
            ReportKind::FeatureUnsupported,
            location,
            "list numbers in a format the layout does not write are written in decimal".to_owned(),
        );
    }
    if let Some(location) = &counters.long_label {
        fonts.report_once(
            ReportKind::ValueClamped,
            location,
            format!(
                "list labels, and the templates they are written from, longer than {} \
                 characters are cut there",
                crate::lists::MAX_LABEL_CHARS
            ),
        );
    }
}

impl<'a> Walk<'a, '_> {
    /// Adds the paragraphs and tables of `blocks`.
    fn blocks(&mut self, blocks: &'a [Block]) -> Result<()> {
        for (index, block) in blocks.iter().enumerate() {
            self.location.push(Step::Index(index));
            let content = match block {
                Block::Paragraph(paragraph) => Content::Paragraph(paragraph, self.label(paragraph)),
                Block::Table(table) => {
                    let mut labels = Vec::new();
                    self.table_labels(table, 0, &mut labels)?;
                    Content::Table(table, labels)
                }
            };
            self.out.push(Flowed {
                section: self.section,
                content,
                location: self.location.clone(),
            });
            self.location.pop();
        }
        Ok(())
    }

    /// The list label of `paragraph`, at the walk's place, which advances its list.
    fn label(&mut self, paragraph: &Paragraph) -> Option<Label> {
        paragraph
            .list()
            .and_then(|item| self.counters.label(self.document, &item, self.location))
    }

    /// Adds to `labels` those of the paragraphs of `table`, nested `depth` deep in other
    /// tables, in reading order, nested tables' included: the lists count through table
    /// cells as through the text around them.
    fn table_labels(
        &mut self,
        table: &Table,
        depth: usize,
        labels: &mut Vec<Option<Label>>,
    ) -> Result<()> {
        if depth >= MAX_TABLE_DEPTH {
            return Err(Error::NestingTooDeep {
                location: self.location.clone(),
                limit: MAX_TABLE_DEPTH,
            });
        }
        self.location.push(Step::Field("rows"));
        for (row_index, row) in table.rows().iter().enumerate() {
            self.location.push(Step::Index(row_index));
            self.location.push(Step::Field("cells"));
            for (cell_index, cell) in row.cells().iter().enumerate() {
                self.location.push(Step::Index(cell_index));
                self.location.push(Step::Field("blocks"));
                for (index, block) in cell.blocks().iter().enumerate() {
                    self.location.push(Step::Index(index));
                    match block {
                        Block::Paragraph(paragraph) => labels.push(self.label(paragraph)),
                        Block::Table(table) => self.table_labels(table, depth + 1, labels)?,
                    }
                    self.location.pop();
                }
                self.location.pop();
                self.location.pop();
            }
            self.location.pop();
            self.location.pop();
        }
        self.location.pop();
        Ok(())
    }
}
/// The body of a document laid out on pages.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Paginated {
    pub(crate) pages: Vec<Page>,
    /// The number of lines of every paragraph of the flow, and of bands of every table.
    pub(crate) lines: Vec<usize>,
}

/// Where laid-out pages go, one at a time and in order, as soon as nothing can change them
/// any more, so that a document's pages are not all held at once (#332).
pub(crate) trait PageSink {
    /// Takes `page`, the next one, when the layout has made `made` pages so far; `fonts`
    /// are those of the layout.
    fn take(&mut self, page: Page, made: usize, fonts: &mut Fonts<'_>) -> Result<()>;
}

/// Every page, kept.
impl PageSink for Vec<Page> {
    fn take(&mut self, page: Page, _: usize, _: &mut Fonts<'_>) -> Result<()> {
        self.push(page);
        Ok(())
    }
}

/// What a layout of the pages hands on.
struct Laid<S> {
    /// The sink of the last layout pass, which has taken every page.
    sink: S,
    /// The number of lines of every paragraph of the flow, and of bands of every table.
    lines: Vec<usize>,
    /// The most pages held at once before they went to the sink, and the most paragraph
    /// layouts.
    #[cfg_attr(not(test), allow(dead_code))] // reason: read by the tests of that bound only
    most_held: (usize, usize),
    /// How many times a header or footer was laid out in the last pass.
    #[cfg_attr(not(test), allow(dead_code))] // reason: read by the tests of that bound only
    frames_laid: usize,
    /// The pieces of lines painting headers and footers walked in the last pass.
    #[cfg_attr(not(test), allow(dead_code))] // reason: read by the tests of that bound only
    frames_walked: usize,
}

/// Lays out the body of every section of `document` on pages, at most `max_pages` of
/// them. A document without sections has one empty page of the default setup.
pub(crate) fn paginate(
    document: &Document,
    fonts: &mut Fonts<'_>,
    shaper: &mut Shaper,
    max_pages: usize,
) -> Result<Paginated> {
    let laid = paginate_with(
        document,
        fonts,
        shaper,
        max_pages,
        (MAX_FRAME_DRAWN, MAX_FRAME_LAID),
        Vec::new,
    )?;
    Ok(Paginated {
        pages: laid.sink,
        lines: laid.lines,
    })
}

/// [`paginate`], handing every page on to a sink as soon as nothing can change it any more:
/// a sink that `sink` makes for each layout pass, of which that of the last pass, which
/// has taken every page, is returned. A pass that `NUMPAGES` or `SECTIONPAGES` makes
/// necessary drops the sink of the one before.
pub(crate) fn paginate_into<S: PageSink>(
    document: &Document,
    fonts: &mut Fonts<'_>,
    shaper: &mut Shaper,
    max_pages: usize,
    sink: impl FnMut() -> S,
) -> Result<S> {
    paginate_with(
        document,
        fonts,
        shaper,
        max_pages,
        (MAX_FRAME_DRAWN, MAX_FRAME_LAID),
        sink,
    )
    .map(|laid| laid.sink)
}

/// [`paginate_into`], with headers and footers drawing at most `max_frame_drawn` in all,
/// by [`drawn`] and what painting walks, and laying out at most `max_frame_laid` in a pass,
/// by [`layout_cost`].
fn paginate_with<S: PageSink>(
    document: &Document,
    fonts: &mut Fonts<'_>,
    shaper: &mut Shaper,
    max_pages: usize,
    (max_frame_drawn, max_frame_laid): (usize, usize),
    mut sink: impl FnMut() -> S,
) -> Result<Laid<S>> {
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
            sink: sink(),
            taken: 0,
            taken_by_section: vec![0; document.sections().len()],
            keep_from: usize::MAX,
            most_held: 0,
            layouts_held: 0,
            most_layouts: 0,
            frames_laid: 0,
            frames_walked: 0,
            cursor: 0.0,
            above: None,
            next_number: 1,
            leaders: LeaderBudget::default(),
            frame_budget: max_frame_drawn,
            frame_laid_budget: max_frame_laid,
            totals: totals.clone(),
            frames: HashMap::new(),
        };
        let lines = pager.run(&flow)?;
        if pager.page_count() == 0 {
            pager.new_page(0, &PageSetup::default())?;
        }
        pager.hand_on(usize::MAX)?;
        let found = Totals::of(pager.taken, &pager.taken_by_section, document);
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
            return Ok(Laid {
                sink: pager.sink,
                lines,
                most_held: (pager.most_held, pager.most_layouts),
                frames_laid: pager.frames_laid,
                frames_walked: pager.frames_walked,
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
    /// The counts of a document of `pages` pages, `by_section` of them in each section of
    /// `document`: all of them, but one less when the first section has a title page and
    /// numbers its pages from 0 (MS-OI29500 2.1.489 a). A section counts the blank page of
    /// its odd or even start, and not the page it starts on when it is continuous:
    /// hypotheses until Word shows them (#298, `E-section-continuous-headers.docx`).
    fn of(pages: usize, by_section: &[u32], document: &Document) -> Self {
        let count = |section: usize| by_section.get(section).copied().unwrap_or(0);
        let all = u32::try_from(pages).unwrap_or(u32::MAX);
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
    document
        .sections()
        .iter()
        .any(|section| section_field_kinds(section).1)
}

/// [`field_kinds`] of the body and every header and footer of `section`.
fn section_field_kinds(section: &Section) -> (bool, bool) {
    let sets = [section.headers(), section.footers()];
    let parts = sets
        .iter()
        .flat_map(|set| [set.primary(), set.first(), set.even()]);
    std::iter::once(section.blocks())
        .chain(parts)
        .map(|blocks| field_kinds(blocks, 0))
        .fold((false, false), |kinds, (page, count)| {
            (kinds.0 || page, kinds.1 || count)
        })
}

/// What `item` costs of the budget of what headers and footers draw: one for the item,
/// and for each cluster of glyphs its glyphs, or the characters of the text it stands for
/// where those are more, since painting copies that text into the PDF on every page (a
/// face without a space deletes zero-width characters, so one glyph can stand for many).
fn drawn(item: &DisplayItem) -> usize {
    match item {
        DisplayItem::Glyphs(run) => run
            .clusters
            .iter()
            .map(|cluster| cluster.glyphs.len().max(cluster.text.chars().count()))
            .fold(1, usize::saturating_add),
        _ => 1,
    }
}

/// Whether a `PAGE` field of `layout`, paragraph `index` of the flow, shows text in a line
/// after those on `first`, the page its first lines are on.
fn field_past_first_page(layout: &ParagraphLayout, first: Option<&Page>, index: usize) -> bool {
    let on_first = first
        .and_then(|page| page.fragments.iter().find(|f| f.paragraph == index))
        .map_or(0, |fragment| fragment.lines.end);
    if on_first >= layout.lines.len() {
        return false;
    }
    // Lines hold the drawn text in order: a field that starts where the first page's
    // text ends, or later, is on a later page.
    let first_end = layout
        .lines
        .iter()
        .take(on_first)
        .flat_map(|line| &line.pieces)
        .filter_map(|piece| match &piece.kind {
            PieceKind::Text { text, .. } => Some(text.end),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    layout
        .itemized
        .page_fields
        .iter()
        .any(|field| !field.is_empty() && field.start >= first_end)
}

/// Whether `flowed` has a `PAGE` field, whose value depends on the page it lands on.
fn has_page_field(paragraph: &Paragraph) -> bool {
    paragraph
        .inlines()
        .iter()
        .any(|inline| matches!(inline, Inline::Field(field) if *field.kind() == FieldKind::Page))
}

/// Whether `paragraph` has a `PAGE` field, and whether it has a page count (`NUMPAGES`,
/// `SECTIONPAGES`).
fn paragraph_field_kinds(paragraph: &Paragraph) -> (bool, bool) {
    let mut kinds = (false, false);
    for inline in paragraph.inlines() {
        if let Inline::Field(field) = inline {
            match field.kind() {
                FieldKind::Page => kinds.0 = true,
                FieldKind::NumPages | FieldKind::SectionPages => kinds.1 = true,
                _ => {}
            }
        }
    }
    kinds
}

/// Whether `blocks` have a `PAGE` field, and whether they have a page count
/// (`NUMPAGES`, `SECTIONPAGES`), at any depth of tables.
fn field_kinds(blocks: &[Block], depth: usize) -> (bool, bool) {
    let mut kinds = (false, false);
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => {
                let (page, count) = paragraph_field_kinds(paragraph);
                kinds = (kinds.0 || page, kinds.1 || count);
            }
            Block::Table(table) if depth < MAX_TABLE_DEPTH => {
                for cell in table.rows().iter().flat_map(|row| row.cells()) {
                    let (page, count) = field_kinds(cell.blocks(), depth + 1);
                    kinds = (kinds.0 || page, kinds.1 || count);
                }
            }
            Block::Table(_) => {}
        }
    }
    kinds
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

/// The most work of a header or footer that is laid out and drawn: a paragraph counts
/// one, a table its paragraphs, rows, cells and grid columns, nested tables' included
/// ([`tables::work`]); more is left out, and reported, so that a frame drawn on every
/// page stays bounded.
const MAX_FRAME_WORK: usize = 256;

/// The most glyphs and shapes the headers and footers of a document draw in all, on every
/// page together, with the characters glyphs stand for where those are more ([`drawn`]),
/// and the pieces of their lines that painting walks, where those are more; past it they
/// are left out, and reported. A frame is drawn on every page, so without it a
/// short document of many pages could draw without bound, and one whose lines hold much
/// that draws nothing, such as tabs or breaks, would cost the whole of it on every page
/// (#384).
const MAX_FRAME_DRAWN: usize = 1 << 20;

/// The most the headers and footers of a document lay out in a pass, on every page
/// together, by [`layout_cost`]; past it they are left out as past [`MAX_FRAME_DRAWN`]. A
/// frame with a `PAGE` field is laid out again for every page, so without it one that lays
/// out much and draws little, such as one of many tabs or breaks, would cost the whole of
/// it on every page (#374).
const MAX_FRAME_LAID: usize = 1 << 20;

/// A header or footer as each pass prepares it once: the paragraphs that may be drawn,
/// in order (the first [`MAX_FRAME_WORK`] of a header, the last of a footer), each
/// with whether it has a field the layout computes; whether any of them has a `PAGE`
/// field; whether it is a header; and where it is, for the report.
struct Candidates<'d> {
    parts: Vec<(Flowed<'d>, bool)>,
    page_field: bool,
    header: bool,
    location: Location,
}

/// A header or footer laid out for a page: its candidates; the paragraphs drawn, each by
/// its place among the candidates, with its layout and its top from the frame's top; its
/// height; and, when it has a `PAGE` field, the field values it was laid out with.
struct Frame<'d> {
    candidates: Rc<Candidates<'d>>,
    parts: Vec<(usize, Rc<FramePart>, f64)>,
    height: f64,
    values: Option<FieldValues>,
}

/// A paragraph of a header or footer laid out, with the pieces painting it walks (one for
/// each line that has none), or a table of it drawn.
enum FramePart {
    Paragraph(ParagraphLayout, usize),
    Table(Drawn),
}

impl FramePart {
    /// A paragraph's part of `layout`.
    fn paragraph(layout: ParagraphLayout) -> Self {
        let walked = layout
            .lines
            .iter()
            .map(|line| line.pieces.len().max(1))
            .fold(0, usize::saturating_add);
        Self::Paragraph(layout, walked)
    }

    /// Its height.
    fn height(&self) -> f64 {
        match self {
            Self::Paragraph(layout, _) => layout.height().0,
            Self::Table(drawn) => drawn.height.0,
        }
    }
}

/// The space between `above` and `below` in a stack of paragraphs and tables, or above
/// `below` when it comes first: a table has no spacing of its own (design 8.6).
fn stack_space(document: &Document, above: Option<&Flowed<'_>>, below: &Flowed<'_>) -> f64 {
    match (above.and_then(Flowed::paragraph), below.paragraph()) {
        (Some(above), Some(below)) => {
            paragraph::space_between(document, above.props(), below.props()).0
        }
        (None, Some(below)) => paragraph::space_before(document, below.props()).0,
        (Some(above), None) => paragraph::space_after(document, above.props()).0,
        (None, None) => 0.0,
    }
}

/// The space below `last`, the last of a stack.
fn trailing_space(document: &Document, last: &Flowed<'_>) -> f64 {
    last.paragraph()
        .map_or(0.0, |last| paragraph::space_after(document, last.props()).0)
}

/// What is laid out so far, to return to when a keep rule moves paragraphs.
#[derive(Clone, Copy)]
struct Checkpoint {
    /// The paragraph of the flow placed next.
    index: usize,
    pages: usize,
    items: usize,
    fragments: usize,
    floats: usize,
    cursor: f64,
    above: Option<usize>,
    next_number: u32,
    /// The leader glyphs the run could still draw.
    leaders: LeaderBudget,
    /// What headers and footers could still draw by [`drawn`] and painting walk, and what
    /// they could still lay out.
    frame_budget: usize,
    frame_laid_budget: usize,
    /// Whether nothing was on the page yet.
    at_top: bool,
}

/// Where a paragraph's lines, or a table's bands, went.
#[derive(Clone, Copy)]
struct Placed {
    first_page: usize,
    last_page: usize,
}

struct Pager<'d, 'f, 'b, S> {
    document: &'d Document,
    fonts: &'f mut Fonts<'b>,
    shaper: &'f mut Shaper,
    max_pages: usize,
    /// The pages made and not yet handed on to the sink: the last ones.
    pages: Vec<Page>,
    /// Where pages go once nothing can change them.
    sink: S,
    /// How many pages went to the sink, of each section.
    taken: usize,
    taken_by_section: Vec<u32>,
    /// The first page that going back to a checkpoint could still change: where a chain
    /// of paragraphs kept with the next one starts while the chain can still move (it
    /// does not start at the top of a page), or where the paragraph being placed starts
    /// when its `PAGE` fields may lay it out again or it may start such a chain. The pages
    /// before it, and before the last, can go to the sink.
    keep_from: usize,
    /// The most pages held at once.
    most_held: usize,
    /// The paragraph layouts held, and the most at once.
    layouts_held: usize,
    most_layouts: usize,
    /// How many times a header or footer was laid out, and the pieces of lines painting them
    /// walked.
    frames_laid: usize,
    frames_walked: usize,
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
    /// What headers and footers may still draw by [`drawn`] and painting walk, and what they
    /// may still lay out by [`layout_cost`], what [`paginate_with`] is given at first.
    frame_budget: usize,
    frame_laid_budget: usize,
    /// The page counts of the pass before, for `NUMPAGES` and `SECTIONPAGES`.
    totals: Totals,
    /// The last layout of each header and footer, by section, side and kind: kept while
    /// its `PAGE` fields show the same, and for its paragraphs without fields after.
    frames: HashMap<FrameKey, Rc<Frame<'d>>>,
}

/// Which header or footer: its section, whether it is the header, and its kind.
type FrameKey = (usize, bool, Kind);

/// The paragraph above a paragraph on its page and the one after it, by their properties.
type Neighbours<'a> = (Option<&'a ParagraphProps>, Option<&'a ParagraphProps>);

impl<'d, S: PageSink> Pager<'d, '_, '_, S> {
    /// Places every paragraph of `flow`, section by section. Returns the number of lines
    /// of each.
    fn run(&mut self, flow: &[Flowed<'d>]) -> Result<Vec<usize>> {
        let sections = self.document.sections();
        // Each paragraph's layout, with the page number its `PAGE` fields show, while it
        // may be placed again: from where a chain kept with the next paragraph starts while
        // the chain can still move, or from the item placed next. Those before are dropped
        // (`dropped` of them), their number of lines kept, as tables keep their number of
        // bands.
        let mut layouts: Vec<Option<(ParagraphLayout, u32)>> = Vec::new();
        layouts.resize_with(flow.len(), || None);
        let mut dropped = 0;
        let mut line_counts = vec![0; flow.len()];
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
            let (paragraph, label) = match &flowed.content {
                Content::Paragraph(paragraph, label) => (*paragraph, label.as_ref()),
                Content::Table(table, labels) => {
                    // A table, band by band. It ends a chain kept with the next paragraph,
                    // which moves to the next page, once, when the table's first band is
                    // not on the page where the chain's last paragraph ends; and it is kept
                    // with nothing itself yet.
                    let boundary = broke_page || section_here;
                    let ends = chain.filter(|_| !boundary).map(|start| {
                        let previous = index
                            .checked_sub(1)
                            .and_then(|i| placed.get(i).copied().flatten())
                            .map(|previous| previous.last_page);
                        (
                            start,
                            !start.at_top && !moved.contains(&start.index),
                            previous,
                        )
                    });
                    let decided = ends.map(|(_, movable, previous)| (movable, previous));
                    let (first_page, bands) = match (
                        self.place_table((flow, index), (table, labels), decided)?,
                        ends,
                    ) {
                        (Some(table), _) => table,
                        (None, Some((start, ..))) => {
                            // The whole chain to the next page, once.
                            moved.insert(start.index);
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
                        // A table moves only the chain it ends.
                        (None, None) => (self.page_count().saturating_sub(1), 0),
                    };
                    if let Some(count) = line_counts.get_mut(index) {
                        *count = bands;
                    }
                    let here = Placed {
                        first_page,
                        last_page: self.page_count().saturating_sub(1),
                    };
                    if let Some(slot) = placed.get_mut(index) {
                        *slot = Some(here);
                    }
                    chain = None;
                    // Nothing goes back past the next item's start now.
                    self.keep_from = usize::MAX;
                    self.release()?;
                    for slot in layouts.iter_mut().take(index + 1).skip(dropped) {
                        if slot.take().is_some() {
                            self.layouts_held -= 1;
                        }
                    }
                    dropped = dropped.max(index + 1);
                    broke_page = false;
                    index += 1;
                    continue;
                }
            };
            let props = paragraph.props();
            // A page break before a paragraph that already starts a page makes no page.
            // Hypothesis until a Word render (`E-page-break-before-top.docx`).
            if props.page_break_before() && !self.page_is_empty() {
                self.new_page(flowed.section, section.page())?;
                broke_page = true;
            }
            let checkpoint = self.checkpoint(index);
            // Placing goes back here when the paragraph's `PAGE` fields land on another page
            // than the one they were laid out for, and moving a chain it starts goes back
            // here later, unless it starts at the top of a page, from where a chain does not
            // move: its first page stays until then.
            let page_field = has_page_field(paragraph);
            let starts_chain = props.keep_next() && !checkpoint.at_top;
            if page_field || starts_chain {
                self.keep_from = self.keep_from.min(checkpoint.pages.saturating_sub(1));
            }
            let values = self.field_values(flowed.section);
            let (layout, laid_for) = match layouts.get_mut(index) {
                Some(Some((layout, laid_for))) => (layout, laid_for),
                // Fields show the values of the page it is laid out for: where it starts,
                // unless keep and widow rules move it further.
                Some(slot) => {
                    let layout = paragraph::lay_out_with(
                        self.document,
                        (paragraph, label),
                        self.body()?.width,
                        (&mut *self.fonts, &mut *self.shaper),
                        &flowed.location,
                        Some(&values),
                    );
                    self.layouts_held += 1;
                    self.most_layouts = self.most_layouts.max(self.layouts_held);
                    let (layout, laid_for) = slot.insert((layout, values.page));
                    (layout, laid_for)
                }
                None => break,
            };
            let keeps_space = kept_top || index == 0;
            let (mut first_page, mut ends_with_break) =
                self.place(flow, index, layout, keeps_space)?;
            // A `PAGE` field shows the number of the page its paragraph starts on: laid out
            // for the page it began on (or, kept from before a keep chain moved, the page it
            // was first placed on), a paragraph that lands on another page is laid out again
            // for that page, and placed again.
            let landed = self.page_at(first_page).map(|page| page.number);
            if let Some(landed) = landed.filter(|&n| n != *laid_for && page_field) {
                self.restore(checkpoint);
                let values = FieldValues {
                    page: landed,
                    ..values.clone()
                };
                *laid_for = landed;
                *layout = paragraph::lay_out_with(
                    self.document,
                    (paragraph, label),
                    self.body()?.width,
                    (&mut *self.fonts, &mut *self.shaper),
                    &flowed.location,
                    Some(&values),
                );
                (first_page, ends_with_break) = self.place(flow, index, layout, keeps_space)?;
                if self.page_at(first_page).map(|page| page.number) != Some(landed) {
                    self.fonts.report_once(
                        ReportKind::FieldCached,
                        &flowed.location,
                        "a PAGE field in the body text shows the number of a page next to the \
                         one its paragraph starts on"
                            .to_owned(),
                    );
                }
            }
            // A paragraph split over pages is laid out once, for its first: a `PAGE` field
            // in its later lines shows that page's number, where Word shows the number of
            // the page the field is on.
            if page_field && field_past_first_page(layout, self.page_at(first_page), index) {
                self.fonts.report_once(
                    ReportKind::FieldCached,
                    &flowed.location,
                    "a PAGE field of a paragraph split over pages shows the number of the page \
                     the paragraph starts on"
                        .to_owned(),
                );
            }
            if let Some(count) = line_counts.get_mut(index) {
                *count = layout.lines.len();
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
                last_page: self.page_count().saturating_sub(1),
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
                    self.report_split_chain(&flowed.location);
                }
            }
            chain = match chain {
                _ if !props.keep_next() => None,
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
            // Only a chain that can still move goes back past the next paragraph's start:
            // one that does not start at the top of a page. A chain moved once starts again
            // at the top of the page it moved to, so it cannot move twice.
            let movable = chain.filter(|start| !start.at_top);
            self.keep_from = movable.map_or(usize::MAX, |start| start.pages.saturating_sub(1));
            self.release()?;
            let placed_again = movable.map_or(index + 1, |start| start.index);
            for slot in layouts.iter_mut().take(placed_again).skip(dropped) {
                if slot.take().is_some() {
                    self.layouts_held -= 1;
                }
            }
            dropped = dropped.max(placed_again);
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
        // The count the bound is measured by is the layouts really held.
        debug_assert_eq!(
            self.layouts_held,
            layouts.iter().flatten().count(),
            "paragraph layouts held"
        );
        Ok(line_counts)
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
        // Chapter numbers go before the numbers of `PAGE` fields only.
        if section.page_numbering().chapter_style().is_some() && section_field_kinds(section).0 {
            self.fonts.report_once(
                ReportKind::FeatureUnsupported,
                &location,
                "chapter numbers before page numbers are not written".to_owned(),
            );
        }
        // An odd or even start whose first page would have a number of the wrong parity
        // raises the number by one. The skipped number becomes a blank page only when the
        // section continues the numbering of the one before; the first section, and one
        // that starts its numbering again, start on the next page, numbered one more
        // (Word's renders of `E-section-odd.docx` and `E-section-odd-first.docx`, #301).
        // Numbering that starts again at 0 is laid out as before those renders until Word
        // shows it (#298): its number is not raised, and after the first section the
        // skipped number is a blank page.
        let raises = section
            .page_numbering()
            .start()
            .is_some_and(|start| start > 0);
        let wrong_parity = match section.start() {
            SectionStart::OddPage => self.next_number % 2 == 0,
            SectionStart::EvenPage => self.next_number % 2 == 1,
            _ => false,
        };
        let Some(current) = self.pages.last() else {
            if wrong_parity && (raises || section.page_numbering().start().is_none()) {
                self.next_number = self.next_number.saturating_add(1);
            }
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
                if wrong_parity {
                    if raises {
                        self.next_number = self.next_number.saturating_add(1);
                    } else {
                        self.add_page(index, section.page(), true)?;
                    }
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
        let Some((flowed, paragraph)) = flow
            .get(index)
            .and_then(|flowed| Some((flowed, flowed.paragraph()?)))
        else {
            return Ok((self.page_count().saturating_sub(1), false));
        };
        let props = paragraph.props();
        let lines = &layout.lines;
        let count = lines.len();
        let keep_lines = props.keep_lines();
        let widow_control = props.widow_control();
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
            // Space above: between this paragraph and the one above it on the page, or
            // its own below a table; at the top of a page only at the start of the
            // document or of a section. Word's render of `E-space-before-top.docx`:
            // dropped after flowing and after page breaks alike.
            let gap = if first > 0 {
                0.0
            } else if let Some(above) = self.above.filter(|_| !empty).and_then(|i| flow.get(i)) {
                match above.paragraph() {
                    Some(above) => paragraph::space_between(self.document, above.props(), props).0,
                    None => paragraph::space_before(self.document, props).0,
                }
            } else if keeps_space {
                paragraph::space_before(self.document, props).0
            } else {
                0.0
            };
            let here = self.page_count().saturating_sub(1);
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
            // The paragraphs it meets, whose styles decide whether contextual spacing leaves
            // its own spacing out: the one above it on the page, and the next.
            let neighbours = (
                self.above
                    .filter(|_| !empty)
                    .and_then(|i| flow.get(i))
                    .and_then(|above| above.paragraph())
                    .map(Paragraph::props),
                flow.get(index + 1)
                    .and_then(Flowed::paragraph)
                    .map(Paragraph::props),
            );
            let (fit, breaks) = fitting(lines, first, top, body.bottom.0);
            if fit == rest || (breaks && fit > 0) {
                self.put(
                    index,
                    (flowed, paragraph, neighbours),
                    layout,
                    first..first + fit,
                    top,
                );
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
            self.put(
                index,
                (flowed, paragraph, neighbours),
                layout,
                first..first + take,
                top,
            );
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
        (flowed, paragraph, neighbours): (&Flowed<'_>, &Paragraph, Neighbours<'_>),
        layout: &ParagraphLayout,
        lines: Range<usize>,
        top: f64,
    ) {
        let Some(page) = self.pages.last() else {
            return;
        };
        let (left, bottom) = (page.body.left, page.body.bottom.0);
        let height: f64 = layout
            .lines
            .get(lines.clone())
            .unwrap_or(&[])
            .iter()
            .map(|line| line.height.0)
            .sum();
        let mut items = Vec::new();
        tables::report_borders(paragraph, &flowed.location, self.fonts);
        let origin = Point {
            x: left,
            y: Pt(top),
        };
        let page = self.pages.last().map(|page| {
            let section = self.document.sections().get(page.section);
            let setup = section.map_or_else(PageSetup::default, |section| *section.page());
            page_box(&setup, page.number)
        });
        // Bar tabs run through the paragraph's own space before and after, as far as the page
        // lays them out: above, within the gap its lines are placed under, which is none for
        // those that go on after a page; below, within the body, after its last line; neither
        // where contextual spacing leaves it out. Word's render of `E-bar-tab.docx` shows the
        // bar unbroken through case 04's 12 pt before and after, between paragraphs that have
        // the same bar, where this rule and one joining bars across the gaps give the same.
        // Hypotheses until a Word reference: a neighbour without a bar (each bar reaching
        // through its own paragraph's space only), a gap larger than the paragraph's own
        // space, the cut at the body's bottom and a paragraph split over pages.
        let props = paragraph.props();
        let above = paragraph::own_space_before(self.document, props, neighbours.0)
            .0
            .min((top - self.cursor).max(0.0));
        let below = if lines.end >= layout.lines.len() {
            paragraph::own_space_after(self.document, props, neighbours.1)
                .0
                .min((bottom - top - height).max(0.0))
        } else {
            0.0
        };
        let mut painting = Painting {
            document: self.document,
            paragraph,
            layout,
            fonts: self.fonts,
            location: &flowed.location,
            background: None,
            leaders: &mut self.leaders,
            page,
            floats: Vec::new(),
            bar_reach: (above, below),
        };
        painting.paint_shading(lines.clone(), origin, &mut items);
        painting.paint(lines.clone(), origin, &mut items);
        let floats = painting.floats;
        if let Some(page) = self.pages.last_mut() {
            page.items.extend(items);
            page.floats.extend(floats);
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

    /// Places table `index` of the flow, `flowed`, band by band: a band goes to the next
    /// page when it does not fit, unless it starts the page, and it is laid out with the
    /// field values of the page it goes on. Returns the page of its first band and its
    /// number of bands.
    fn place_table(
        &mut self,
        (flow, index): (&[Flowed<'d>], usize),
        (table, labels): (&'d Table, &[Option<Label>]),
        ends: Option<(bool, Option<usize>)>,
    ) -> Result<Option<(usize, usize)>> {
        let Some(flowed) = flow.get(index) else {
            return Ok(Some((self.page_count().saturating_sub(1), 0)));
        };
        let setup = self
            .document
            .sections()
            .get(flowed.section)
            .map_or_else(PageSetup::default, |section| *section.page());
        let width = self.body()?.width;
        // A table that fits its columns to their content measures it once, with the field
        // values of the page it starts on.
        let values = self.field_values(flowed.section);
        let mut measured = tables::Measured::new();
        let mut context = Context {
            document: self.document,
            fonts: &mut *self.fonts,
            shaper: &mut *self.shaper,
            leaders: &mut self.leaders,
            values: Some(&values),
            measured: &mut measured,
        };
        let prepared = Prepared::new(table, labels, width, &flowed.location, &mut context);
        let count = prepared.band_count();
        let mut first_page = None;
        for band in 0..count {
            loop {
                let empty = self.page_is_empty();
                let body = self.body()?;
                // The space below the paragraph above (design 8.6: a table has no
                // spacing of its own).
                let above = self
                    .above
                    .filter(|_| !empty && band == 0)
                    .and_then(|above| flow.get(above))
                    .and_then(Flowed::paragraph);
                let gap = above.map_or(0.0, |above| {
                    paragraph::space_after(self.document, above.props()).0
                });
                let values = self.field_values(flowed.section);
                let mut measured = tables::Measured::new();
                let mut context = Context {
                    document: self.document,
                    fonts: &mut *self.fonts,
                    shaper: &mut *self.shaper,
                    leaders: &mut self.leaders,
                    values: Some(&values),
                    measured: &mut measured,
                };
                let drawn = prepared.band(band, &mut context);
                let top = self.cursor + gap;
                let fits = top + drawn.height.0 <= body.bottom.0 + EPSILON;
                if fits || empty {
                    if !fits {
                        self.fonts.report_once(
                            ReportKind::Overflow,
                            &flowed.location,
                            "table rows are taller than the body of their page".to_owned(),
                        );
                    }
                    let here = self.page_count().saturating_sub(1);
                    if band == 0 {
                        if let Some((movable, previous)) = ends {
                            if previous.is_some_and(|previous| previous != here) {
                                if movable {
                                    // The chain before the table moves; the run places it
                                    // and the table again.
                                    return Ok(None);
                                }
                                self.report_split_chain(&flowed.location);
                            }
                        }
                        // Nothing goes back past the table's first band: the pages before
                        // it go on.
                        self.keep_from = usize::MAX;
                        self.release()?;
                    }
                    let first = *first_page.get_or_insert(here);
                    if here != first && tables::has_header_rows(table) {
                        self.fonts.report_once(
                            ReportKind::FeatureUnsupported,
                            &flowed.location,
                            "table header rows are not repeated on the pages their table \
                             continues on"
                                .to_owned(),
                        );
                    }
                    self.put_band(index, band, drawn, top);
                    break;
                }
                self.new_page(flowed.section, &setup)?;
            }
        }
        if count == 0 {
            first_page = Some(self.page_count().saturating_sub(1));
        }
        Ok(Some((
            first_page.unwrap_or_else(|| self.page_count().saturating_sub(1)),
            count,
        )))
    }

    /// Reports, once, that a chain of paragraphs kept with the next one is split.
    fn report_split_chain(&mut self, location: &Location) {
        self.fonts.report_once(
            ReportKind::Overflow,
            location,
            "paragraphs kept with the next one do not fit on a page together, so they are \
             split"
                .to_owned(),
        );
    }

    /// Puts band `band` of table `index`, `drawn`, on the current page with its top at
    /// `top`.
    fn put_band(&mut self, index: usize, band: usize, drawn: Drawn, top: f64) {
        let Some(page) = self.pages.last_mut() else {
            return;
        };
        let mut items = drawn.items;
        tables::shift(&mut items, page.body.left.0, top);
        page.items.extend(items);
        page.fragments.push(Fragment {
            paragraph: index,
            lines: band..band + 1,
            top: Pt(top),
            height: drawn.height,
        });
        self.cursor = top + drawn.height.0;
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
        if self.page_count() >= self.max_pages {
            return Err(Error::TooManyPages {
                limit: self.max_pages,
            });
        }
        let number = self.next_number;
        let mut body = body(setup);
        let mut items = Vec::new();
        let mut floats = Vec::new();
        if let Some(ir) = self.document.sections().get(section).filter(|_| !blank) {
            // The section's first page is the first of its own: a continuous section starts
            // on a page that keeps the section before, so its title-page header is on its
            // second. Hypothesis until a Word render (`E-section-continuous-headers.docx`).
            // The sink takes no page before a later one is made, so the last page is held:
            // a page that is not blank, or a blank one that an odd or even section start put
            // before that section's first page, after a page of another section.
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
                    "the margins, header and footer of a page leave no room for its body text"
                        .to_owned(),
                );
            }
            let page = page_box(setup, number);
            self.paint_frame(
                &header,
                Point {
                    x: body.left,
                    y: Pt(header_top),
                },
                (&mut items, &mut floats),
                page,
            );
            let footer_top = footer_bottom - footer.height;
            self.paint_frame(
                &footer,
                Point {
                    x: body.left,
                    y: Pt(footer_top),
                },
                (&mut items, &mut floats),
                page,
            );
        }
        // The header's and footer's floating objects are stacked at once with them: nothing
        // adds to their layer.
        let items = display::stack(items, floats);
        self.pages.push(Page {
            section,
            width: setup.size().width(),
            height: setup.size().height(),
            number,
            body,
            fragments: Vec::new(),
            frame_items: items.len(),
            items,
            floats: Vec::new(),
            blank,
        });
        self.most_held = self.most_held.max(self.pages.len());
        self.next_number = self.next_number.saturating_add(1);
        self.cursor = body.top.0;
        self.above = None;
        self.release()
    }

    /// Hands on to the sink the pages that nothing can change any more: those before the
    /// last and before [`Pager::keep_from`].
    fn release(&mut self) -> Result<()> {
        self.hand_on(self.keep_from.min(self.page_count().saturating_sub(1)))
    }

    /// The number of pages made, those handed on to the sink included.
    fn page_count(&self) -> usize {
        self.taken + self.pages.len()
    }

    /// Page `index` of those made, unless it went to the sink.
    fn page_at(&self, index: usize) -> Option<&Page> {
        index
            .checked_sub(self.taken)
            .and_then(|index| self.pages.get(index))
    }

    /// Hands the pages before page `upto` on to the sink.
    fn hand_on(&mut self, upto: usize) -> Result<()> {
        let count = upto.saturating_sub(self.taken).min(self.pages.len());
        let made = self.page_count();
        for mut page in self.pages.drain(..count) {
            if let Some(taken) = self.taken_by_section.get_mut(page.section) {
                *taken = taken.saturating_add(1);
            }
            self.taken += 1;
            // Its body's floating objects go behind or in front of the body's text, over the
            // header and footer, now that nothing can add to them.
            let mut items = std::mem::take(&mut page.items);
            let body = items.split_off(page.frame_items.min(items.len()));
            let floats = std::mem::take(&mut page.floats);
            items.extend(display::stack(body, floats));
            page.items = items;
            self.sink.take(page, made, self.fonts)?;
        }
        Ok(())
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

    /// The header (`header`) or footer of `kind` of section `section` for a page whose
    /// field values are `values`, `room.0` wide on a page `room.1` high: the last layout of
    /// it while its `PAGE` fields show the same; otherwise laid out again, its paragraphs
    /// without fields the layout computes taken from the last layout. A page touches at
    /// most [`MAX_FRAME_WORK`] of its work: the rest is walked once a pass. Past what
    /// headers and footers may draw or lay out, nothing of them is laid out: the last
    /// layout stands in for the room it takes, or an empty one for a frame never laid out,
    /// which is then reported.
    fn cached_frame(
        &mut self,
        ir: &'d Section,
        section: usize,
        header: bool,
        kind: Kind,
        values: &FieldValues,
        room: (Pt, Pt),
    ) -> Result<Rc<Frame<'d>>> {
        let key = (section, header, kind);
        let last = self.frames.get(&key).cloned();
        // Past the budget of what headers and footers draw, nothing more of them is
        // drawn, so none is laid out (#374).
        if self.frame_budget == 0 {
            return Ok(self.stand_in(last, ir, header, kind));
        }
        let candidates = match &last {
            Some(frame) => Rc::clone(&frame.candidates),
            None => Rc::new(self.candidates(ir, section, header, kind)?),
        };
        // Only a `PAGE` field changes from page to page: the number format and the page
        // counts are the same on every page of a section in a pass.
        let shown = candidates.page_field.then(|| values.clone());
        if let Some(frame) = last.as_ref().filter(|frame| frame.values == shown) {
            return Ok(Rc::clone(frame));
        }
        // Past what they may lay out, none is laid out either, and none is drawn any more,
        // since the layouts that stand in show the numbers of other pages.
        if self.frame_laid_budget == 0 {
            self.frame_budget = 0;
            return Ok(self.stand_in(last, ir, header, kind));
        }
        let frame = Rc::new(self.frame(candidates, (values, shown), room, last)?);
        self.frames_laid += 1;
        self.frames.insert(key, Rc::clone(&frame));
        Ok(frame)
    }

    /// What stands in for the header (`header`) or footer of `kind` of `section` past what
    /// headers and footers may draw or lay out: its `last` layout, for the room it takes,
    /// or, for one never laid out, an empty one, which takes none and is reported when the
    /// frame has content (painting reports the others). The empty one is not kept as the
    /// frame's last layout, which it must never be taken for.
    fn stand_in(
        &mut self,
        last: Option<Rc<Frame<'d>>>,
        ir: &Section,
        header: bool,
        kind: Kind,
    ) -> Rc<Frame<'d>> {
        if let Some(last) = last {
            return last;
        }
        if !frame_blocks(ir, header, kind).is_empty() {
            self.report_frames_left_out();
        }
        Rc::new(Frame {
            candidates: Rc::new(Candidates {
                parts: Vec::new(),
                page_field: false,
                header,
                location: Location::default(),
            }),
            parts: Vec::new(),
            height: 0.0,
            values: None,
        })
    }

    /// The [`Candidates`] of the header (`header`) or footer of `kind` of section
    /// `section`. What is past [`MAX_FRAME_WORK`] is left out, and reported.
    fn candidates(
        &mut self,
        ir: &'d Section,
        section: usize,
        header: bool,
        kind: Kind,
    ) -> Result<Candidates<'d>> {
        let blocks = frame_blocks(ir, header, kind);
        let name = match kind {
            Kind::First => "first",
            Kind::Even => "even",
            Kind::Primary => "primary",
        };
        let mut location = Location::default();
        location.push(Step::Field("sections"));
        location.push(Step::Index(section));
        location.push(Step::Field(if header { "headers" } else { "footers" }));
        location.push(Step::Field(name));
        let mut flowed = Vec::new();
        // The lists of each header and footer count from their start: a hypothesis until a
        // Word render, since no reference document has a list in a header or footer.
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
        // A header is laid out from its first paragraph, a footer from its last, which is
        // nearest its distance. A table counts its paragraphs, rows, cells and grid
        // columns, nested tables' too, as it is laid out again on every page where a field
        // in it changes.
        let weight = |part: &Flowed<'_>| match &part.content {
            Content::Paragraph(..) => 1,
            Content::Table(table, _) => tables::table_work(table),
        };
        let mut taken = 0usize;
        let mut fits = |part: &Flowed<'_>| {
            taken = taken.saturating_add(weight(part));
            taken <= MAX_FRAME_WORK
        };
        let kept = if header {
            flowed.iter().take_while(|part| fits(part)).count()
        } else {
            flowed.iter().rev().take_while(|part| fits(part)).count()
        };
        let past = flowed.len().saturating_sub(kept);
        if past > 0 {
            self.report_cut(&location);
            if header {
                flowed.truncate(kept);
            } else {
                flowed.drain(..past);
            }
        }
        let kinds = |part: &Flowed<'_>| match &part.content {
            Content::Paragraph(paragraph, _) => paragraph_field_kinds(paragraph),
            Content::Table(table, _) => table
                .rows()
                .iter()
                .flat_map(|row| row.cells())
                .map(|cell| field_kinds(cell.blocks(), 1))
                .fold((false, false), |a, b| (a.0 || b.0, a.1 || b.1)),
        };
        let parts: Vec<(Flowed<'d>, bool)> = flowed
            .into_iter()
            .map(|part| {
                let fields = kinds(&part) != (false, false);
                (part, fields)
            })
            .collect();
        let page_field = parts.iter().any(|(part, _)| kinds(part).0);
        Ok(Candidates {
            parts,
            page_field,
            header,
            location,
        })
    }

    /// Reports, once, that a header or footer is cut.
    fn report_cut(&mut self, location: &Location) {
        self.fonts.report_once(
            ReportKind::Overflow,
            location,
            format!(
                "a header or footer taller than its page, or of more than \
                 {MAX_FRAME_WORK} paragraphs, table rows, cells and columns together, is cut"
            ),
        );
    }

    /// `candidates` laid out `room.0` wide with the field values `values.0` (of which
    /// `values.1` are those it shows): its paragraphs one under the other with the space
    /// between them, the space before the first and after the last included. The layouts
    /// of its paragraphs without fields are taken from `last` when it has them. Paragraphs
    /// that would start past the page's height `room.1`, spacing included, are left out,
    /// and reported: the last ones of a header, the first ones of a footer, which grows up
    /// from its distance.
    fn frame(
        &mut self,
        candidates: Rc<Candidates<'d>>,
        (values, shown): (&FieldValues, Option<FieldValues>),
        (width, page_height): (Pt, Pt),
        last: Option<Rc<Frame<'d>>>,
    ) -> Result<Frame<'d>> {
        let document = self.document;
        let reusable: HashMap<usize, Rc<FramePart>> = last
            .iter()
            .flat_map(|frame| frame.parts.iter())
            .filter(|(index, _, _)| {
                candidates
                    .parts
                    .get(*index)
                    .is_some_and(|(_, fields)| !fields)
            })
            .map(|(index, part, _)| (*index, Rc::clone(part)))
            .collect();
        let count = candidates.parts.len();
        let order: Vec<usize> = if candidates.header {
            (0..count).collect()
        } else {
            (0..count).rev().collect()
        };
        let mut kept: Vec<(usize, Rc<FramePart>)> = Vec::new();
        let mut taken = 0.0;
        // The part laid out before, next to this one: above it in a header, below it in
        // a footer.
        let mut before: Option<&Flowed<'d>> = None;
        for index in order {
            let Some((part, _)) = candidates.parts.get(index) else {
                continue;
            };
            // The space between it and the part before, or before a header's first and
            // after a footer's last: a part that would start past the page's height is
            // not laid out, nor any after it.
            let gap = match (before, candidates.header) {
                (None, true) => stack_space(document, None, part),
                (None, false) => trailing_space(document, part),
                (Some(above), true) => stack_space(document, Some(above), part),
                (Some(below), false) => stack_space(document, Some(part), below),
            };
            if taken + gap >= page_height.0 {
                self.report_cut(&candidates.location);
                break;
            }
            let laid = match reusable.get(&index) {
                Some(laid) => Rc::clone(laid),
                None => {
                    let cost = match &part.content {
                        Content::Paragraph(paragraph, _) => paragraph_cost(document, paragraph),
                        Content::Table(table, _) => table_cost(document, table),
                    };
                    self.frame_laid_budget = self.frame_laid_budget.saturating_sub(cost);
                    Rc::new(self.frame_part(part, values, width))
                }
            };
            taken += gap + laid.height();
            before = Some(part);
            kept.push((index, laid));
        }
        kept.sort_by_key(|(index, _)| *index);
        let mut parts = Vec::new();
        let mut height = 0.0;
        let mut above: Option<&Flowed<'d>> = None;
        for (index, laid) in kept {
            let Some((part, _)) = candidates.parts.get(index) else {
                continue;
            };
            height += stack_space(document, above, part);
            let top = height;
            height += laid.height();
            above = Some(part);
            parts.push((index, laid, top));
        }
        if let Some(last) = above {
            height += trailing_space(document, last);
        }
        Ok(Frame {
            candidates,
            parts,
            height,
            values: shown,
        })
    }

    /// A paragraph of a header or footer laid out `width` wide, or a table of it drawn,
    /// with the field values `values`.
    fn frame_part(&mut self, part: &Flowed<'d>, values: &FieldValues, width: Pt) -> FramePart {
        match &part.content {
            Content::Paragraph(paragraph, label) => FramePart::paragraph(paragraph::lay_out_with(
                self.document,
                (paragraph, label.as_ref()),
                width,
                (&mut *self.fonts, &mut *self.shaper),
                &part.location,
                Some(values),
            )),
            Content::Table(table, labels) => {
                let mut measured = tables::Measured::new();
                let mut context = Context {
                    document: self.document,
                    fonts: &mut *self.fonts,
                    shaper: &mut *self.shaper,
                    leaders: &mut self.leaders,
                    values: Some(values),
                    measured: &mut measured,
                };
                let prepared = Prepared::new(table, labels, width, &part.location, &mut context);
                FramePart::Table(prepared.whole(&mut context))
            }
        }
    }

    /// Adds to `items` what `frame` draws, its top left corner at `origin`, as far as the
    /// budget of what frames draw goes: a part that would take more than is left is not
    /// drawn, nor is anything of a frame after it. A paragraph costs what it draws, or the
    /// pieces painting walks where those are more.
    fn paint_frame(
        &mut self,
        frame: &Frame<'d>,
        origin: Point,
        (items, floats): (&mut Vec<DisplayItem>, &mut Vec<Float>),
        page: PageBox,
    ) {
        for (index, laid, top) in &frame.parts {
            let Some((part, _)) = frame.candidates.parts.get(*index) else {
                continue;
            };
            let walked = match laid.as_ref() {
                FramePart::Paragraph(_, walked) => *walked,
                FramePart::Table(_) => 0,
            };
            if self.frame_budget > 0 {
                let start = items.len();
                let at = Point {
                    x: origin.x,
                    y: Pt(origin.y.0 + top),
                };
                let mut anchored = Vec::new();
                match (laid.as_ref(), part.paragraph()) {
                    (FramePart::Paragraph(layout, _), Some(paragraph)) => {
                        self.frames_walked = self.frames_walked.saturating_add(walked);
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
                            // Hypothesis: in headers and footers, as no reference shows them,
                            // bar tabs reach only through the lines.
                            bar_reach: (0.0, 0.0),
                        };
                        let lines = 0..layout.lines.len();
                        painting.paint_shading(lines.clone(), at, items);
                        painting.paint(lines, at, items);
                        anchored = painting.floats;
                    }
                    (FramePart::Table(drawn), _) => {
                        let mut table = drawn.items.clone();
                        tables::shift(&mut table, at.x.0, at.y.0);
                        items.extend(table);
                    }
                    (FramePart::Paragraph(..), None) => {}
                }
                let cost: usize = items
                    .iter()
                    .skip(start)
                    .chain(anchored.iter().map(|float| &float.item))
                    .map(drawn)
                    .sum::<usize>()
                    .max(walked);
                if cost <= self.frame_budget {
                    self.frame_budget -= cost;
                    floats.extend(anchored);
                    continue;
                }
                items.truncate(start);
            }
            self.frame_budget = 0;
            self.report_frames_left_out();
            break;
        }
    }

    /// Reports, once, that headers and footers are left out past what they may draw or lay
    /// out on all pages together.
    fn report_frames_left_out(&mut self) {
        self.fonts.report_once(
            ReportKind::Overflow,
            &Location::default(),
            format!(
                "headers and footers are left out past {MAX_FRAME_DRAWN} glyphs, characters, \
                 shapes and pieces painted, or {MAX_FRAME_LAID} characters and pieces laid out, on \
                 all pages together"
            ),
        );
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
            pages: self.page_count(),
            items: page.map_or(0, |page| page.items.len()),
            fragments: page.map_or(0, |page| page.fragments.len()),
            floats: page.map_or(0, |page| page.floats.len()),
            cursor: self.cursor,
            above: self.above,
            next_number: self.next_number,
            leaders: self.leaders,
            frame_budget: self.frame_budget,
            frame_laid_budget: self.frame_laid_budget,
            at_top: self.page_is_empty(),
        }
    }

    /// Goes back to what was laid out at `checkpoint`.
    fn restore(&mut self, checkpoint: Checkpoint) {
        // `keep_from` holds the checkpoint's last page back from the sink.
        debug_assert!(
            checkpoint.pages > self.taken,
            "a checkpoint at page {} goes back past the {} pages handed on",
            checkpoint.pages,
            self.taken
        );
        self.pages
            .truncate(checkpoint.pages.saturating_sub(self.taken));
        if let Some(page) = self.pages.last_mut() {
            page.items.truncate(checkpoint.items);
            page.fragments.truncate(checkpoint.fragments);
            page.floats.truncate(checkpoint.floats);
        }
        self.cursor = checkpoint.cursor;
        self.above = checkpoint.above;
        self.frame_budget = checkpoint.frame_budget;
        self.frame_laid_budget = checkpoint.frame_laid_budget;
        self.next_number = checkpoint.next_number;
        self.leaders = checkpoint.leaders;
    }
}

/// The blocks of the header (`header`) or footer of `kind` of `section`.
fn frame_blocks(section: &Section, header: bool, kind: Kind) -> &[Block] {
    let set = if header {
        section.headers()
    } else {
        section.footers()
    };
    match kind {
        Kind::First => set.first(),
        Kind::Even => set.even(),
        Kind::Primary => set.primary(),
    }
}

/// What laying out `blocks` of `document` again costs, as [`MAX_FRAME_LAID`] counts it:
/// one for each paragraph and each inline, the bytes of their text, the stored results of
/// fields the layout does not compute, and a table's rows, cells and grid columns, nested
/// tables' included. A hidden inline, which is not laid out, costs one.
fn layout_cost(document: &Document, blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(paragraph) => paragraph_cost(document, paragraph),
            Block::Table(table) => table_cost(document, table),
        })
        .fold(0, usize::saturating_add)
}

/// [`layout_cost`] of `paragraph`.
fn paragraph_cost(document: &Document, paragraph: &Paragraph) -> usize {
    paragraph
        .inlines()
        .iter()
        .map(|inline| inline_cost(document, inline))
        .fold(1, usize::saturating_add)
}

/// [`layout_cost`] of `inline`. Headers and footers are laid out with field values, so a
/// field the layout computes shows its number, not its stored result; the result of any
/// other field is laid out, and counts (a result holds no fields).
fn inline_cost(document: &Document, inline: &Inline) -> usize {
    if itemize::hidden(document, inline) {
        return 1;
    }
    match inline {
        Inline::Text { text, .. } => text.len().max(1),
        Inline::Field(field) if !itemize::computed(field.kind()) => field
            .result()
            .iter()
            .map(|inline| inline_cost(document, inline))
            .fold(1, usize::saturating_add),
        _ => 1,
    }
}

/// [`layout_cost`] of `table`.
fn table_cost(document: &Document, table: &Table) -> usize {
    table
        .rows()
        .iter()
        .flat_map(|row| row.cells())
        .map(|cell| layout_cost(document, cell.blocks()).saturating_add(1))
        .fold(0, usize::saturating_add)
        .saturating_add(table.rows().len())
        .saturating_add(table.grid().len())
}

/// The page `setup` gives, numbered `number`, as floating objects are positioned on it.
pub(crate) fn page_box(setup: &PageSetup, number: u32) -> PageBox {
    let body = body(setup);
    PageBox {
        width: setup.size().width().0,
        height: setup.size().height().0,
        margins: [
            body.left.0,
            body.top.0,
            body.left.0 + body.width.0,
            body.bottom.0,
        ],
        odd: number % 2 == 1,
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
        RunFonts, RunStyle, RunStyleId, Spacing, Table, VerticalMerge,
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

    /// The number of every page the sink took, in order, and the most pages and paragraph
    /// layouts held at once.
    fn held(document: &Document) -> (Vec<u32>, (usize, usize)) {
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let laid = paginate_with(
            document,
            &mut fonts,
            &mut shaper,
            1000,
            (MAX_FRAME_DRAWN, MAX_FRAME_LAID),
            Vec::new,
        )
        .expect("pages");
        let numbers = laid.sink.iter().map(|page| page.number).collect();
        (numbers, laid.most_held)
    }

    #[test]
    fn pages_go_to_the_sink_as_soon_as_nothing_can_change_them() {
        // Issue #332: a document's pages are not all held at once, nor the layouts of its
        // paragraphs. Eight 20 pt lines fill a page: 400 paragraphs of a line, and one
        // paragraph of 400 lines, make 50 pages each, held two at a time: the new page and
        // the one before it. A paragraph's layout goes once it is placed.
        let fifty: Vec<u32> = (1..=50).collect();
        let paragraphs = document(|style| {
            one_section(
                (0..400)
                    .map(|_| lines(style, 1, spacing(0.0, 0.0)))
                    .collect(),
            )
        });
        assert_eq!(held(&paragraphs), (fifty.clone(), (2, 1)));
        let paragraph = document(|style| one_section(vec![lines(style, 400, spacing(0.0, 0.0))]));
        assert_eq!(held(&paragraph), (fifty, (2, 1)));
        // A chain of paragraphs kept with the next one holds its pages back while it can
        // still move, since moving it goes back to where it starts: 20 one-line paragraphs
        // kept with the next after 7 others start on page 1 and hold it, with the layouts
        // of the two placed there, until they reach page 2 and move there. Moved once and
        // at the top of a page, the chain cannot move again, so its pages over 2 to 4 go on
        // as they fill, and the 100 paragraphs after it fill 12 more pages, two at a time.
        let chain = document(|style| {
            let mut blocks: Vec<Block> =
                (0..7).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.extend((0..20).map(|_| lines(style, 1, spacing(0.0, 0.0).with_keep_next(true))));
            blocks.extend((0..100).map(|_| lines(style, 1, spacing(0.0, 0.0))));
            one_section(blocks)
        });
        assert_eq!(held(&chain), ((1..=16).collect(), (2, 2)));
        // A chain from the top of the first page cannot move at all: 400 paragraphs, every
        // one kept with the next, go on two pages at a time (#332's document with keepNext
        // everywhere, which held every page until the chain ended).
        let all_kept = document(|style| {
            one_section(
                (0..400)
                    .map(|_| lines(style, 1, spacing(0.0, 0.0).with_keep_next(true)))
                    .collect(),
            )
        });
        assert_eq!(held(&all_kept), ((1..=50).collect(), (2, 1)));
        // Nor can one paragraph of 400 lines kept with the next at the top of page 1: it
        // holds none of its pages while it is placed.
        let one_kept = document(|style| {
            one_section(vec![lines(
                style,
                400,
                spacing(0.0, 0.0).with_keep_next(true),
            )])
        });
        assert_eq!(held(&one_kept), ((1..=50).collect(), (2, 1)));
        // A table of 400 rows of one line goes on its 50 pages the same way: its bands are
        // placed like lines.
        let table = document(|style| {
            let row = || {
                Row::default().with_cells(vec![Cell::default().with_blocks([lines(
                    style,
                    1,
                    spacing(0.0, 0.0),
                )])])
            };
            one_section(vec![
                Table::default()
                    .with_grid([Pt(50.0)])
                    .with_rows((0..400).map(|_| row()).collect::<Vec<_>>())
                    .into(),
            ])
        });
        assert_eq!(held(&table), ((1..=50).collect(), (2, 0)));
        // A heading kept with the table after it holds its page only until the table's
        // first row is placed: then the table goes on two pages at a time too.
        let kept = document(|style| {
            let row = || {
                Row::default().with_cells(vec![Cell::default().with_blocks([lines(
                    style,
                    1,
                    spacing(0.0, 0.0),
                )])])
            };
            one_section(vec![
                lines(style, 1, spacing(0.0, 0.0)),
                lines(style, 1, spacing(0.0, 0.0).with_keep_next(true)),
                Table::default()
                    .with_grid([Pt(50.0)])
                    .with_rows((0..400).map(|_| row()).collect::<Vec<_>>())
                    .into(),
            ])
        });
        let (numbers, (pages, _)) = held(&kept);
        assert_eq!((numbers.len(), pages), (51, 2));
    }

    /// A table of `rows` rows of one cell of one line, over a column 100 pt wide; the
    /// first row a header row with `header`.
    fn table_of(style: RunStyleId, rows: usize, header: bool) -> Block {
        let row = |n: usize| {
            Row::default()
                .with_header(header && n == 0)
                .with_cells(vec![Cell::default().with_blocks([lines(
                    style,
                    1,
                    spacing(0.0, 0.0),
                )])])
        };
        Table::default()
            .with_grid([Pt(100.0)])
            .with_rows((0..rows).map(row).collect::<Vec<_>>())
            .into()
    }

    #[test]
    fn a_paragraph_kept_with_a_table_goes_with_its_first_row() {
        // Seven lines, then a heading kept with the next on the last line of page 1, then a
        // table whose first row is two lines: the row does not fit, so the heading goes to
        // page 2 with it.
        let moved = document(|style| {
            let mut blocks: Vec<Block> =
                (0..7).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(lines(style, 1, spacing(0.0, 0.0).with_keep_next(true)));
            let cell = Cell::default().with_blocks([lines(style, 2, spacing(0.0, 0.0))]);
            blocks.push(
                Table::default()
                    .with_grid([Pt(100.0)])
                    .with_rows(vec![Row::default().with_cells(vec![cell])])
                    .into(),
            );
            one_section(blocks)
        });
        let moved = pages(&moved);
        assert_eq!(moved[0].len(), 7);
        assert_eq!(moved[1], [(7, 0..1, 20.0), (8, 0..1, 40.0)]);
        // A heading at the top of a page cannot move: the table's first row of eight lines
        // goes to the next page without it, and the split is reported.
        let split = document(|style| {
            let cell = Cell::default().with_blocks([lines(style, 8, spacing(0.0, 0.0))]);
            one_section(vec![
                lines(style, 1, spacing(0.0, 0.0).with_keep_next(true)),
                Table::default()
                    .with_grid([Pt(100.0)])
                    .with_rows(vec![Row::default().with_cells(vec![cell])])
                    .into(),
            ])
        });
        let (laid, report) = laid(&split, 100);
        assert_eq!(laid.expect("pages").pages.len(), 2);
        assert!(
            messages(&report)
                .iter()
                .any(|message| message.starts_with("paragraphs kept with the next one")),
            "{report:?}"
        );
    }

    #[test]
    fn header_rows_that_are_not_repeated_are_reported() {
        // A table with a header row that goes on to page 2, where Word repeats the header
        // row: not done yet, and reported. One that ends on its first page, and one without
        // header rows, report nothing.
        let note = |rows: usize, header: bool| {
            let document = document(|style| one_section(vec![table_of(style, rows, header)]));
            let (_, report) = laid(&document, 100);
            messages(&report)
                .iter()
                .any(|message| message.starts_with("table header rows are not repeated"))
        };
        assert!(note(12, true));
        assert!(!note(4, true));
        assert!(!note(12, false));
    }

    #[test]
    fn a_table_takes_the_space_of_the_paragraphs_around_it() {
        // Design 8.6: a table has no spacing of its own. Below a paragraph with 30 pt after
        // it, the table starts 30 pt lower; the paragraph after it, with 10 pt before it,
        // starts 10 pt below the table, not 30.
        let document = document(|style| {
            one_section(vec![
                lines(style, 1, spacing(0.0, 30.0)),
                table_of(style, 1, false),
                lines(style, 1, spacing(10.0, 0.0)),
            ])
        });
        assert_eq!(
            pages(&document),
            [vec![(0, 0..1, 20.0), (1, 0..1, 70.0), (2, 0..1, 100.0)]]
        );
    }

    #[test]
    fn a_header_s_table_takes_the_space_after_the_paragraph_above_it() {
        // A header of a paragraph with 10 pt after it, then a table: the table's text is
        // 30 pt below the paragraph's, its line and the space after it.
        let document = document(|style| {
            let paragraph = text_paragraph(style, "P", spacing(0.0, 10.0));
            let cell = Cell::default().with_blocks([text_paragraph(style, "T", spacing(0.0, 0.0))]);
            let table = Table::default()
                .with_grid([Pt(100.0)])
                .with_rows(vec![Row::default().with_cells(vec![cell])]);
            vec![
                Section::new(close_frames())
                    .with_headers(
                        HeaderFooterSet::default().with_primary([paragraph, table.into()]),
                    )
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let pages = laid.expect("pages").pages;
        let baseline = |text: &str| {
            pages[0].items.iter().find_map(|item| match item {
                DisplayItem::Glyphs(run)
                    if run
                        .clusters
                        .iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        == text =>
                {
                    Some(run.origin.y.0)
                }
                _ => None,
            })
        };
        let (Some(p), Some(t)) = (baseline("P"), baseline("T")) else {
            panic!("{:?}", texts(&pages[0]));
        };
        assert!((t - p - 30.0).abs() < 1e-9, "{p} {t}");
    }

    /// The entries of `kind` in `report` whose message starts with `what`, as their locations.
    fn entries_at(report: &LayoutReport, kind: ReportKind, what: &str) -> Vec<String> {
        report
            .entries()
            .iter()
            .filter(|entry| entry.kind() == kind && entry.message().starts_with(what))
            .map(|entry| entry.location().to_string())
            .collect()
    }

    #[test]
    fn what_the_pdf_leaves_out_is_reported() {
        use crate::ir::{
            AnchoredObject, DocumentSettings, HorizontalAnchor, Hyphenation, ImageData,
            ImageFormat, Note, NoteKind, ObjectContent, Picture, Size, VerticalAnchor,
        };

        // Issue #303: each feature left out has its entry, once, however often it occurs,
        // at the first place it occurs.
        let floating = "floating pictures are not drawn yet";
        let notes = "footnotes and endnotes are not laid out yet";
        let right_to_left = "right-to-left text is laid out from left to right";
        let hyphenation = "automatic hyphenation is not done";
        let all = [floating, notes, right_to_left, hyphenation];
        let at = |report: &LayoutReport, what: &str| {
            entries_at(report, ReportKind::FeatureUnsupported, what)
        };
        let settings =
            DocumentSettings::default().with_hyphenation(Hyphenation::default().with_auto(true));
        let mut document = Document::new(settings);
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let rtl = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_rtl(true),
        );
        let image = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
        let note = document.add_note(Note::new(NoteKind::Footnote, []));
        let anchored = || {
            Inline::anchored(AnchoredObject::new(
                ObjectContent::Picture(Picture::new(image)),
                Size::new(Pt(10.0), Pt(10.0)),
                HorizontalAnchor::default(),
                VerticalAnchor::default(),
            ))
        };
        let paragraph =
            |inlines: Vec<Inline>| -> Block { Paragraph::new(style).with_inlines(inlines).into() };
        let long = "A ".repeat(60);
        document.add_section(Section::new(setup()).with_blocks(vec![
            paragraph(vec![Inline::text(&long, style)]),
            paragraph(vec![
                anchored(),
                Inline::text("A", style),
                Inline::note_reference(note, false, style),
            ]),
            paragraph(vec![
                anchored(),
                Inline::text("\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}", style),
                Inline::note_reference(note, false, style),
            ]),
            paragraph(vec![Inline::text("Latin", rtl)]),
        ]));
        let (result, report) = laid(&document, 100);
        result.expect("pages");
        // Floating pictures are drawn: no entry says they are not.
        assert_eq!(at(&report, floating), [""; 0], "{:?}", messages(&report));
        for (what, place) in [
            (notes, "sections[0].blocks[1]"),
            (right_to_left, "sections[0].blocks[2]"),
            (hyphenation, "sections[0].blocks[0]"),
        ] {
            assert_eq!(
                at(&report, what),
                [place],
                "{what}: {:?}",
                messages(&report)
            );
        }
        // Right-to-left text is told by its characters, wherever they are in a run, and by
        // the run's `w:rtl` on its own; digits that keep their stored order are not.
        for (text, run_rtl, reported) in [
            ("\u{05E9}\u{05DC}\u{05D5}\u{05DD}", false, true),
            ("Hello \u{05E9}\u{05DC}\u{05D5}\u{05DD}", false, true),
            ("Latin", true, true),
            ("Latin", false, false),
            ("a\u{200F}b", false, true),
            ("a\u{202B}b", false, true),
            ("\u{202E}abc", false, true),
            ("a\u{2067}b", false, true),
            ("\u{061B}", false, true),
            ("\u{061F}", false, true),
            ("\u{0640}", false, true),
            ("\u{0661}\u{0662}\u{0663}", false, false),
            ("\u{06F1}\u{06F2}\u{06F3}", false, false),
            ("\u{0661}\u{066B}\u{0665}", false, false),
            ("\u{0665}\u{0660}\u{066A}", false, false),
            ("\u{0600}\u{0661}\u{0662}", false, false),
            ("1\u{FB29}2", false, false),
            ("\u{0601}", false, false),
            ("\u{0604}", false, false),
            ("\u{0609}", false, false),
            ("\u{060A}", false, false),
            ("\u{066C}", false, false),
            ("\u{0890}", false, false),
            ("\u{0891}", false, false),
            ("\u{10D30}\u{10D31}", false, false),
            ("\u{10D40}\u{10D41}", false, false),
            ("\u{0710}", false, true),
            ("\u{0780}", false, true),
            ("\u{07CA}", false, true),
            ("\u{10900}\u{10901}", false, true),
        ] {
            let mut only = Document::default();
            let style = only.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_rtl(run_rtl),
            );
            only.add_section(Section::new(setup()).with_blocks(vec![
                Paragraph::new(style)
                    .with_inlines([Inline::text(text, style)])
                    .into(),
            ]));
            let (_, report) = laid(&only, 100);
            assert_eq!(
                at(&report, right_to_left).len(),
                usize::from(reported),
                "{text:?}: {:?}",
                messages(&report)
            );
        }
        // Text from left to right, with nothing floating, no notes and no automatic
        // hyphenation, reports none of them.
        let plain = self::document(|style| {
            vec![Section::new(setup()).with_blocks(vec![lines(style, 2, spacing(0.0, 0.0))])]
        });
        let (_, report) = laid(&plain, 100);
        for what in all {
            assert_eq!(
                at(&report, what),
                [""; 0],
                "{what}: {:?}",
                messages(&report)
            );
        }
    }

    #[test]
    fn automatic_hyphenation_is_reported_only_where_a_line_wraps_in_a_paragraph_allowing_it() {
        use crate::ir::{DocumentSettings, Hyphenation};

        // Word hyphenates only where a line wraps, in a paragraph that does not suppress
        // hyphenation, in a document that turns it on (Part 1 §17.3.1.34, §17.15.1.10).
        // LibreOffice saves `w:autoHyphenation` on with `w:suppressAutoHyphens` in every
        // paragraph's defaults (poi/unicode-path.docx), and a paragraph that fits on its
        // line, or ends its lines with breaks, has nothing to hyphenate.
        let what = "automatic hyphenation is not done";
        let long = "A ".repeat(60);
        let laid_with = |auto: bool, paragraphs: [(&str, bool); 3]| {
            let settings = DocumentSettings::default()
                .with_hyphenation(Hyphenation::default().with_auto(auto));
            let mut document = Document::new(settings);
            let style = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(10.0)),
            );
            let blocks: Vec<Block> = paragraphs
                .iter()
                .map(|&(text, suppress)| {
                    let props = spacing(0.0, 0.0).with_suppress_auto_hyphens(suppress);
                    if text.is_empty() {
                        lines(style, 3, props)
                    } else {
                        // A line break where the text has a newline.
                        let mut inlines = Vec::new();
                        for (n, part) in text.split('\n').enumerate() {
                            if n > 0 {
                                inlines.push(Inline::line_break(BreakClear::None, style));
                            }
                            inlines.push(Inline::text(part, style));
                        }
                        Paragraph::new(style)
                            .with_props(props)
                            .with_inlines(inlines)
                            .into()
                    }
                })
                .collect();
            document.add_section(Section::new(setup()).with_blocks(blocks));
            let (result, report) = laid(&document, 100);
            result.expect("pages");
            entries_at(&report, ReportKind::FeatureUnsupported, what)
        };
        let (wraps, fits, broken) = (long.as_str(), "A", "");
        // Only the lines in its middle wrap: a line break before them and after them.
        let middle = format!("A\n{long}\nA");
        assert_eq!(laid_with(true, [(wraps, true); 3]), [""; 0]);
        assert_eq!(
            laid_with(true, [(wraps, true), (wraps, false), (wraps, false)]),
            ["sections[0].blocks[1]"]
        );
        // A paragraph allowing it whose text fits on its line, or whose lines end with
        // line breaks, gets no entry; the first that wraps does.
        assert_eq!(
            laid_with(true, [(fits, false), (broken, false), (wraps, false)]),
            ["sections[0].blocks[2]"]
        );
        assert_eq!(
            laid_with(true, [(fits, false), (broken, false), (fits, false)]),
            [""; 0]
        );
        assert_eq!(
            laid_with(true, [(fits, false), (&middle, false), (fits, false)]),
            ["sections[0].blocks[1]"]
        );
        assert_eq!(laid_with(false, [(wraps, false); 3]), [""; 0]);
    }

    #[test]
    fn a_page_s_box_has_its_section_s_margins_and_the_parity_of_its_number() {
        // setup(): 300 x 200 with margins of 20; a gutter widens the left one.
        let gutter = setup().with_margins(setup().margins().with_gutter(Pt(5.0)));
        let odd = page_box(&gutter, 3);
        assert_eq!(
            (odd.width, odd.height, odd.margins, odd.odd),
            (300.0, 200.0, [25.0, 20.0, 280.0, 180.0], true)
        );
        assert!(!page_box(&gutter, 4).odd);
    }

    #[test]
    fn floating_objects_are_drawn_where_they_are_anchored_behind_or_in_front_of_the_text() {
        use crate::ir::{
            AnchoredObject, HorizontalAnchor, HorizontalPlacement, HorizontalReference, ImageData,
            ImageFormat, ObjectContent, Picture, Size, VerticalAnchor, VerticalPlacement,
            VerticalReference, Wrap,
        };

        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let image = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
        let float = |across: HorizontalReference,
                     x: f64,
                     down: VerticalReference,
                     y: f64,
                     (behind, z_order): (bool, u32)| {
            Inline::anchored(
                AnchoredObject::new(
                    ObjectContent::Picture(Picture::new(image)),
                    Size::new(Pt(30.0), Pt(10.0)),
                    HorizontalAnchor::default()
                        .with_relative_to(across)
                        .with_placement(HorizontalPlacement::Offset(Pt(x))),
                    VerticalAnchor::default()
                        .with_relative_to(down)
                        .with_placement(VerticalPlacement::Offset(Pt(y))),
                )
                .with_behind_text(behind)
                .with_z_order(z_order)
                .with_wrap(if z_order == 5 {
                    Wrap::Square
                } else {
                    Wrap::None
                }),
            )
        };
        // A logo in the header, from the page's corner, in front.
        let logo = float(
            HorizontalReference::Page,
            10.0,
            VerticalReference::Page,
            5.0,
            (false, 3),
        );
        let header = Paragraph::new(style)
            .with_props(spacing(0.0, 0.0))
            .with_inlines([logo, Inline::text("H", style)]);
        // In the body: one behind the text at the margins' corner, one in front from the
        // line's top, 50 pt from the margin, which text would wrap around in Word.
        let body = Paragraph::new(style)
            .with_props(spacing(0.0, 0.0))
            .with_inlines([
                float(
                    HorizontalReference::Margin,
                    0.0,
                    VerticalReference::Margin,
                    0.0,
                    (true, 1),
                ),
                Inline::text("A", style),
                float(
                    HorizontalReference::Margin,
                    50.0,
                    VerticalReference::Line,
                    0.0,
                    (false, 5),
                ),
                Inline::page_break(style),
                Inline::text("B", style),
            ]);
        document.add_section(
            Section::new(close_frames())
                .with_headers(HeaderFooterSet::default().with_primary([header.into()]))
                .with_blocks(vec![body.into()]),
        );
        let (laid, report) = laid(&document, 100);
        let pages = laid.expect("pages").pages;
        let images = |page: &Page| -> Vec<(usize, f64, f64)> {
            page.items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| match item {
                    DisplayItem::Image(image) => Some((index, image.at.x.0, image.at.y.0)),
                    _ => None,
                })
                .collect()
        };
        let first = images(&pages[0]);
        let frame = pages[0].frame_items;
        let last = pages[0].items.len() - 1;
        let line_top = pages[0].body.top.0;
        // The header's layer first: its text, then its logo in front of it. Then the
        // body's: the picture behind its text first, the one in front last.
        assert_eq!(frame, 2);
        assert_eq!(
            first,
            [
                (frame - 1, 10.0, 5.0),
                (frame, 20.0, 20.0),
                (last, 70.0, line_top)
            ]
        );
        // The header's logo is on every page, in front of the header's text and under
        // the body's.
        let frame = pages[1].frame_items;
        assert_eq!(images(&pages[1]), [(frame - 1, 10.0, 5.0)]);
        assert!(pages[1].items.len() > frame);
        assert!(
            messages(&report)
                .iter()
                .any(|message| message.contains("does not wrap around floating objects")),
            "{report:?}"
        );
    }

    #[test]
    fn a_floating_object_moves_with_the_paragraph_that_anchors_it() {
        use crate::ir::{
            AnchoredObject, HorizontalAnchor, ImageData, ImageFormat, ObjectContent, Picture, Size,
            VerticalAnchor,
        };

        // Eight lines fill page 1; the first and the eighth anchor a picture each, the
        // eighth kept with the next paragraph. The next paragraph starts page 2, so the
        // chain moves there, and its picture goes with it: page 1 keeps the first picture
        // and no copy of the other.
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let image = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
        let picture = Inline::anchored(AnchoredObject::new(
            ObjectContent::Picture(Picture::new(image)),
            Size::new(Pt(30.0), Pt(10.0)),
            HorizontalAnchor::default(),
            VerticalAnchor::default(),
        ));
        let mut blocks: Vec<Block> = vec![
            Paragraph::new(style)
                .with_props(spacing(0.0, 0.0))
                .with_inlines([picture.clone(), Inline::text("A", style)])
                .into(),
        ];
        blocks.extend((0..6).map(|_| lines(style, 1, spacing(0.0, 0.0))));
        blocks.push(
            Paragraph::new(style)
                .with_props(spacing(0.0, 0.0).with_keep_next(true))
                .with_inlines([picture, Inline::text("A", style)])
                .into(),
        );
        blocks.push(lines(style, 2, spacing(0.0, 0.0)));
        document.add_section(Section::new(setup()).with_blocks(blocks));
        let (laid, _) = laid(&document, 100);
        let pages = laid.expect("pages").pages;
        let images = |page: &Page| {
            page.items
                .iter()
                .filter(|item| matches!(item, DisplayItem::Image(_)))
                .count()
        };
        assert_eq!(texts(&pages[1]).len(), 3);
        assert_eq!(pages.iter().map(images).collect::<Vec<_>>(), [1, 1]);
    }

    /// A picture 30 x 10 of `document`, placed `across` and `down`, without wrapping.
    fn anchored_picture(
        document: &mut Document,
        across: (
            crate::ir::HorizontalReference,
            crate::ir::HorizontalPlacement,
        ),
        down: (crate::ir::VerticalReference, crate::ir::VerticalPlacement),
    ) -> Inline {
        use crate::ir::{
            AnchoredObject, HorizontalAnchor, ImageData, ImageFormat, ObjectContent, Picture, Size,
            VerticalAnchor,
        };
        let image = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
        Inline::anchored(AnchoredObject::new(
            ObjectContent::Picture(Picture::new(image)),
            Size::new(Pt(30.0), Pt(10.0)),
            HorizontalAnchor::default()
                .with_relative_to(across.0)
                .with_placement(across.1),
            VerticalAnchor::default()
                .with_relative_to(down.0)
                .with_placement(down.1),
        ))
    }

    /// Where the pictures `page` draws are, in drawing order.
    fn picture_places(page: &Page) -> Vec<(f64, f64)> {
        page.items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Image(image) => Some((image.at.x.0, image.at.y.0)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_floating_picture_is_placed_from_where_its_paragraph_line_and_character_are() {
        use crate::ir::{HorizontalPlacement, HorizontalReference, VerticalPlacement};
        use crate::ir::{VerticalReference as Y, Wrap};

        // A paragraph of two exact 20 pt lines at the body's top (20): on its second line,
        // after "A", one picture 5 pt right of its character and at its line's top (40),
        // another at the margin and at its paragraph's top (20), a third, 10 pt high, at
        // the bottom of its line (60, so at 50). None asks for wrapping, so the report says
        // nothing of it.
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let at_line = anchored_picture(
            &mut document,
            (
                HorizontalReference::Character,
                HorizontalPlacement::Offset(Pt(5.0)),
            ),
            (Y::Line, VerticalPlacement::Offset(Pt(0.0))),
        );
        let at_paragraph = anchored_picture(
            &mut document,
            (
                HorizontalReference::Margin,
                HorizontalPlacement::Offset(Pt(0.0)),
            ),
            (Y::Paragraph, VerticalPlacement::Offset(Pt(0.0))),
        );
        let at_line_bottom = anchored_picture(
            &mut document,
            (
                HorizontalReference::Margin,
                HorizontalPlacement::Offset(Pt(0.0)),
            ),
            (
                Y::Line,
                VerticalPlacement::Align(crate::ir::AnchorVerticalAlign::Bottom),
            ),
        );
        assert!(matches!(&at_line, Inline::Anchored(object) if object.wrap() == Wrap::None));
        let paragraph = Paragraph::new(style)
            .with_props(spacing(0.0, 0.0))
            .with_inlines([
                Inline::text("W", style),
                Inline::line_break(BreakClear::None, style),
                Inline::text("A", style),
                at_line,
                at_paragraph,
                at_line_bottom,
                Inline::text("B", style),
            ]);
        document.add_section(Section::new(setup()).with_blocks(vec![paragraph.into()]));
        let (laid, report) = laid(&document, 100);
        let pages = laid.expect("pages").pages;
        // The run after the pictures starts where they are anchored.
        let b = pages[0]
            .items
            .iter()
            .find_map(|item| match item {
                DisplayItem::Glyphs(run)
                    if run.clusters.iter().any(|cluster| cluster.text == "B") =>
                {
                    Some(run.origin.x.0)
                }
                _ => None,
            })
            .expect("the run of B");
        assert!(b > 20.0, "{b}");
        assert_eq!(
            picture_places(&pages[0]),
            [(b + 5.0, 40.0), (20.0, 20.0), (20.0, 50.0)]
        );
        assert!(
            !messages(&report)
                .iter()
                .any(|message| message.contains("does not wrap")),
            "{report:?}"
        );
    }

    #[test]
    fn floating_pictures_take_the_inside_margin_of_their_page_s_parity() {
        use crate::ir::{
            AnchorHorizontalAlign, HorizontalPlacement, HorizontalReference, VerticalPlacement,
            VerticalReference,
        };

        // At the left of the inside margin: the left margin area of odd pages (from 0),
        // the right one of even pages (from 280), in the header and the body alike.
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let inside = |document: &mut Document| {
            anchored_picture(
                document,
                (
                    HorizontalReference::InsideMargin,
                    HorizontalPlacement::Align(AnchorHorizontalAlign::Left),
                ),
                (VerticalReference::Page, VerticalPlacement::Offset(Pt(0.0))),
            )
        };
        let header = Paragraph::new(style)
            .with_props(spacing(0.0, 0.0))
            .with_inlines([inside(&mut document), Inline::text("H", style)]);
        let page = |document: &mut Document, break_before: bool| -> Block {
            Paragraph::new(style)
                .with_props(spacing(0.0, 0.0).with_page_break_before(break_before))
                .with_inlines([inside(document), Inline::text("A", style)])
                .into()
        };
        let blocks = vec![page(&mut document, false), page(&mut document, true)];
        document.add_section(
            Section::new(close_frames())
                .with_headers(HeaderFooterSet::default().with_primary([header.into()]))
                .with_blocks(blocks),
        );
        let (laid, _) = laid(&document, 100);
        let pages = laid.expect("pages").pages;
        let across = |page: &Page| -> Vec<f64> {
            picture_places(page).into_iter().map(|(x, _)| x).collect()
        };
        assert_eq!(across(&pages[0]), [0.0, 0.0]);
        assert_eq!(across(&pages[1]), [280.0, 280.0]);
    }

    #[test]
    fn a_header_part_the_budget_cuts_leaves_out_its_pictures_too() {
        // A header of a logo and "H" costs 3 (the picture, and a run and a glyph): with a
        // budget of 4 it is drawn on page 1; on page 2 the 1 left does not draw it, its
        // logo included.
        use crate::ir::VerticalReference;
        use crate::ir::{HorizontalPlacement, HorizontalReference, VerticalPlacement};

        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let logo = anchored_picture(
            &mut document,
            (
                HorizontalReference::Page,
                HorizontalPlacement::Offset(Pt(10.0)),
            ),
            (VerticalReference::Page, VerticalPlacement::Offset(Pt(5.0))),
        );
        let header = Paragraph::new(style)
            .with_props(spacing(0.0, 0.0))
            .with_inlines([logo, Inline::text("H", style)]);
        let blocks: Vec<Block> = (0..4)
            .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
            .collect();
        document.add_section(
            Section::new(close_frames())
                .with_headers(HeaderFooterSet::default().with_primary([header.into()]))
                .with_blocks(blocks),
        );
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let laid = paginate_with(
            &document,
            &mut fonts,
            &mut shaper,
            100,
            (4, MAX_FRAME_LAID),
            Vec::new,
        )
        .expect("pages");
        let drawn: Vec<usize> = laid
            .sink
            .iter()
            .map(|page| picture_places(page).len())
            .collect();
        assert_eq!(drawn, [1, 0, 0, 0]);
    }

    #[test]
    fn a_header_s_pictures_count_towards_what_headers_draw_in_all() {
        // A header holding only a picture, drawn with a budget of 3: on three of eight
        // pages, then left out and reported.
        use crate::ir::VerticalReference;
        use crate::ir::{HorizontalPlacement, HorizontalReference, VerticalPlacement};

        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let logo = anchored_picture(
            &mut document,
            (
                HorizontalReference::Page,
                HorizontalPlacement::Offset(Pt(10.0)),
            ),
            (VerticalReference::Page, VerticalPlacement::Offset(Pt(5.0))),
        );
        let header = Paragraph::new(style)
            .with_props(spacing(0.0, 0.0))
            .with_inlines([logo]);
        let blocks: Vec<Block> = (0..8)
            .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
            .collect();
        document.add_section(
            Section::new(close_frames())
                .with_headers(HeaderFooterSet::default().with_primary([header.into()]))
                .with_blocks(blocks),
        );
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let laid = paginate_with(
            &document,
            &mut fonts,
            &mut shaper,
            100,
            (3, MAX_FRAME_LAID),
            Vec::new,
        )
        .expect("pages");
        let drawn: Vec<usize> = laid
            .sink
            .iter()
            .map(|page| picture_places(page).len())
            .collect();
        assert_eq!(drawn, [1, 1, 1, 0, 0, 0, 0, 0]);
        let report = fonts.into_report();
        assert!(
            messages(&report)
                .iter()
                .any(|m| m.contains("left out past")),
            "{report:?}"
        );
    }

    /// The tops and bottoms of the lines `page` draws.
    fn drawn_lines(page: &Page) -> Vec<(f64, f64)> {
        page.items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Line(line) => Some((line.from.y.0, line.to.y.0)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bar_tabs_run_through_the_paragraph_s_space_before_and_after() {
        use crate::ir::{TabKind, TabStop};

        // 12 pt before and after, as in E-bar-tab case 04, but between paragraphs without a
        // bar, which no reference shows (there every paragraph has the bar): the bar reaches
        // through the paragraph's own spacing only. Its line is at 52 to 72, 12 pt under the
        // line above it, and the paragraph after it starts 12 pt lower: from 40 to 84.
        let bar = TabStop::default()
            .with_position(Pt(30.0))
            .with_kind(TabKind::Bar);
        let between = document(|style| {
            one_section(vec![
                lines(style, 1, spacing(0.0, 0.0)),
                lines(style, 1, spacing(12.0, 12.0).with_tabs([bar])),
                lines(style, 1, spacing(0.0, 0.0)),
            ])
        });
        let (between, _) = laid(&between, 100);
        assert_eq!(
            drawn_lines(&between.expect("pages").pages[0]),
            [(40.0, 84.0)]
        );
        // Split over two pages, the first part reaches through the space before, kept at
        // the top of the document (from 20, its first line at 32, seven lines to 172), the
        // second through the space after (three lines from 20 to 80, then 12 pt).
        let split = document(|style| {
            one_section(vec![lines(style, 10, spacing(12.0, 12.0).with_tabs([bar]))])
        });
        let (split, _) = laid(&split, 100);
        let split = split.expect("pages");
        assert_eq!(drawn_lines(&split.pages[0]), [(20.0, 172.0)]);
        assert_eq!(drawn_lines(&split.pages[1]), [(20.0, 92.0)]);
        // At the top of a page the space before is dropped, and the bar with it; at the
        // bottom of the body the space after is cut, and the bar with it.
        let at_top = document(|style| {
            one_section(vec![
                lines(style, 8, spacing(0.0, 0.0)),
                lines(style, 1, spacing(12.0, 12.0).with_tabs([bar])),
            ])
        });
        let (at_top, _) = laid(&at_top, 100);
        assert_eq!(
            drawn_lines(&at_top.expect("pages").pages[1]),
            [(20.0, 52.0)]
        );
        let at_bottom = document(|style| {
            one_section(vec![lines(style, 7, spacing(12.0, 12.0).with_tabs([bar]))])
        });
        let (at_bottom, _) = laid(&at_bottom, 100);
        assert_eq!(
            drawn_lines(&at_bottom.expect("pages").pages[0]),
            [(20.0, 180.0)]
        );
        // Where contextual spacing leaves the paragraph's own space out next to one of the
        // same style, the bar does not reach through it: not into the next paragraph's
        // line after a space after left out (the first line at 32 to 52, under its space
        // before kept at the top of the document, and the next at 52 to 72), nor through a
        // space before left out under a paragraph whose own 20 pt after stays (its line at
        // 60 to 80, then its own 12 pt after).
        let contextual = spacing(12.0, 12.0).with_contextual_spacing(true);
        let after_left_out = document(|style| {
            one_section(vec![
                lines(style, 1, contextual.clone().with_tabs([bar])),
                lines(style, 1, spacing(0.0, 0.0).with_contextual_spacing(true)),
            ])
        });
        let (after_left_out, _) = laid(&after_left_out, 100);
        assert_eq!(
            drawn_lines(&after_left_out.expect("pages").pages[0]),
            [(20.0, 52.0)]
        );
        let before_left_out = document(|style| {
            one_section(vec![
                lines(style, 1, spacing(0.0, 20.0)),
                lines(style, 1, contextual.clone().with_tabs([bar])),
            ])
        });
        let (before_left_out, _) = laid(&before_left_out, 100);
        assert_eq!(
            drawn_lines(&before_left_out.expect("pages").pages[0]),
            [(60.0, 92.0)]
        );
        // Under a gap larger than its own space before, the bar reaches through its own 6 pt
        // only: the gap is the 24 pt after of the paragraph above, its line at 64 to 84.
        let larger_gap = document(|style| {
            one_section(vec![
                lines(style, 1, spacing(0.0, 24.0)),
                lines(style, 1, spacing(6.0, 0.0).with_tabs([bar])),
            ])
        });
        let (larger_gap, _) = laid(&larger_gap, 100);
        assert_eq!(
            drawn_lines(&larger_gap.expect("pages").pages[0]),
            [(58.0, 84.0)]
        );
    }

    #[test]
    fn a_bar_tab_over_unshaded_spacing_takes_the_colour_on_the_page() {
        use crate::ir::{ColorValue, Shading, TabKind, TabStop};
        use officewright_core::Color;

        // A paragraph shaded black, with 12 pt before and after: its shading covers its
        // line only, so the bar is white over the line (52 to 72) and black over the
        // spacing above and below it.
        let bar = TabStop::default()
            .with_position(Pt(30.0))
            .with_kind(TabKind::Bar);
        let shaded = spacing(12.0, 12.0).with_tabs([bar]).with_shading(Some(
            Shading::default().with_fill(ColorValue::Rgb(Color::BLACK)),
        ));
        let document = document(|style| {
            one_section(vec![
                lines(style, 1, spacing(0.0, 0.0)),
                lines(style, 1, shaded.clone()),
                lines(style, 1, spacing(0.0, 0.0)),
            ])
        });
        let (laid, _) = laid(&document, 100);
        let laid = laid.expect("pages");
        let segments: Vec<(f64, f64, Color)> = laid.pages[0]
            .items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Line(line) => Some((line.from.y.0, line.to.y.0, line.color)),
                _ => None,
            })
            .collect();
        assert_eq!(
            segments,
            [
                (40.0, 52.0, Color::BLACK),
                (52.0, 72.0, Color::WHITE),
                (72.0, 84.0, Color::BLACK),
            ]
        );
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
        let chain = |style, start: SectionStart, break_before: bool, restart: Option<u32>| {
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
            let numbering = crate::ir::PageNumbering::default().with_start(restart);
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
        let summary = |start: SectionStart, break_before: bool, restart: Option<u32>| {
            let chained = document(|style| chain(style, start, break_before, restart));
            let (laid, _) = laid(&chained, 100);
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
            summary(SectionStart::NextPage, false, Some(10)),
            [
                (0, 1, vec![0, 1, 2, 3, 4, 5, 6]),
                (1, 10, vec![7]),
                (1, 11, vec![8])
            ]
        );
        // On an odd page, numbered from 10: 10 is even, so the section starts page 2
        // numbered 11, with no blank page, since it starts its numbering again (#301).
        assert_eq!(
            summary(SectionStart::OddPage, false, Some(10)),
            [
                (0, 1, vec![0, 1, 2, 3, 4, 5, 6]),
                (1, 11, vec![7]),
                (1, 12, vec![8])
            ]
        );
        // On an odd page, continuing the numbering: page 2 is blank, and the chain starts
        // page 3.
        assert_eq!(
            summary(SectionStart::OddPage, false, None),
            [
                (0, 1, vec![0, 1, 2, 3, 4, 5, 6]),
                (1, 2, vec![]),
                (1, 3, vec![7]),
                (1, 4, vec![8])
            ]
        );
        // A continuous section that numbers its pages from 10: its next page is 10.
        let continuous = summary(SectionStart::Continuous, false, Some(10));
        assert!(continuous.iter().any(|page| page.1 == 10), "{continuous:?}");
        // A page break before: three pages, not four, and the break stays where it is.
        let broken = summary(SectionStart::NextPage, true, Some(10));
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
    fn a_table_row_that_does_not_fit_goes_to_the_next_page_whole() {
        // Rows are not split yet (plan PR 13): a row of three lines after six lines of a
        // body of eight goes whole to page 2, and a row taller than a body takes a page of
        // its own and is reported.
        let row_of = |style, count| -> Block {
            let cell = Cell::default().with_blocks([lines(style, count, spacing(0.0, 0.0))]);
            Table::default()
                .with_grid([Pt(100.0)])
                .with_rows(vec![Row::default().with_cells(vec![cell])])
                .into()
        };
        let moved = document(|style| {
            let mut blocks: Vec<Block> =
                (0..6).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(row_of(style, 3));
            one_section(blocks)
        });
        let moved = pages(&moved);
        assert_eq!(moved[0].last(), Some(&(5, 0..1, 120.0)));
        assert_eq!(moved[1], [(6, 0..1, 20.0)]);
        let tall = document(|style| {
            one_section(vec![lines(style, 1, spacing(0.0, 0.0)), row_of(style, 9)])
        });
        let (laid, report) = laid(&tall, 100);
        let pages: Vec<usize> = laid
            .expect("pages")
            .pages
            .iter()
            .map(|page| page.fragments.len())
            .collect();
        assert_eq!(pages, [1, 1]);
        assert!(
            report
                .entries()
                .iter()
                .any(|entry| entry.kind() == ReportKind::Overflow
                    && entry.message().contains("table rows")),
            "{report:?}"
        );
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
    fn odd_and_even_starts_add_a_blank_page_only_where_the_numbering_continues() {
        // Word's renders of E-section-odd and E-section-odd-first (#301).
        let section = |style, start, restart| {
            Section::new(setup())
                .with_start(start)
                .with_page_numbering(crate::ir::PageNumbering::default().with_start(restart))
                .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))])
        };
        let summary = |document: &Document| -> Vec<(usize, u32, bool)> {
            let (laid, _) = laid(document, 100);
            laid.expect("pages")
                .pages
                .iter()
                .map(|page| (page.section, page.number, page.blank))
                .collect()
        };
        let continuing = document(|style| {
            vec![
                section(style, SectionStart::NextPage, None),
                // Continuing: page 2 would be even, so it is a blank page.
                section(style, SectionStart::OddPage, None),
                // Continuing at 4, even: no blank page.
                section(style, SectionStart::EvenPage, None),
                // Starting again at 1, odd: no blank page.
                section(style, SectionStart::OddPage, Some(1)),
                // Starting again at 1, which is odd: numbered 2, with no blank page.
                section(style, SectionStart::EvenPage, Some(1)),
                // Starting again at 2, which is even: numbered 3, with no blank page.
                section(style, SectionStart::OddPage, Some(2)),
            ]
        });
        assert_eq!(
            summary(&continuing),
            [
                (0, 1, false),
                (1, 2, true),
                (1, 3, false),
                (2, 4, false),
                (3, 1, false),
                (4, 2, false),
                (5, 3, false)
            ]
        );
        // A first section that starts on an even page: its first page is numbered 2, and
        // the numbering goes on from there.
        let first = document(|style| {
            vec![
                section(style, SectionStart::EvenPage, None),
                section(style, SectionStart::NextPage, None),
            ]
        });
        assert_eq!(summary(&first), [(0, 2, false), (1, 3, false)]);
        // A first section that starts on an odd page is on page 1; numbered from 2, it is
        // numbered 3.
        for (restart, number) in [(None, 1), (Some(2), 3)] {
            let odd = document(|style| vec![section(style, SectionStart::OddPage, restart)]);
            assert_eq!(summary(&odd), [(0, number, false)], "{restart:?}");
        }
        // Numbering that starts again at 0 is laid out as before #301 until Word shows it
        // (#298): not raised in the first section, a blank page in a later one.
        let zero = document(|style| {
            vec![
                section(style, SectionStart::OddPage, Some(0)),
                section(style, SectionStart::OddPage, Some(0)),
            ]
        });
        assert_eq!(summary(&zero), [(0, 0, false), (1, 0, true), (1, 1, false)]);
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
    fn tables_go_on_pages_band_by_band() {
        // Two rows of cells of one and two lines: rows 40 pt high. A cell merged over the
        // second and third rows ties them into one band; the paragraph after the table
        // comes below it.
        let document = document(|style| {
            let cell =
                |count| Cell::default().with_blocks([lines(style, count, spacing(0.0, 0.0))]);
            let row = |cells| Row::default().with_cells(cells);
            let merged = |merge| cell(1).with_vertical_merge(merge);
            let table = Table::default()
                .with_grid([Pt(50.0), Pt(50.0)])
                .with_rows(vec![
                    row(vec![cell(1), cell(2)]),
                    row(vec![merged(VerticalMerge::Restart), cell(1)]),
                    row(vec![merged(VerticalMerge::Continue), cell(1)]),
                ]);
            one_section(vec![table.into(), lines(style, 1, spacing(0.0, 0.0))])
        });
        let (laid, report) = laid(&document, 100);
        let laid = laid.expect("pages");
        assert_eq!(laid.lines, [2, 1]);
        let fragments: Vec<(usize, Range<usize>, f64, f64)> = laid.pages[0]
            .fragments
            .iter()
            .map(|f| (f.paragraph, f.lines.clone(), f.top.0, f.height.0))
            .collect();
        assert_eq!(
            fragments,
            [
                (0, 0..1, 20.0, 40.0),
                (0, 1..2, 60.0, 40.0),
                (1, 0..1, 100.0, 20.0)
            ]
        );
        assert!(
            report
                .entries()
                .iter()
                .all(|entry| !entry.message().contains("not laid out")),
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
        // (10 - 15) to 30 pt before the end of the 260 pt column, and 1.5 pt further on
        // each side, as Word shades (E-para-shading).
        let left = Pt(20.0 - 5.0 - 1.5);
        let width = Pt(260.0 - 30.0 + 5.0 + 3.0);
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
        // On a thread of its own, so that passes that never end fail the test.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(laid(&document, 100)));
        let (laid, report) = receiver
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the passes end");
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

    /// A paragraph of `text` in exact 20 pt lines, with `props`.
    fn text_paragraph(style: RunStyleId, text: &str, props: ParagraphProps) -> Block {
        Paragraph::new(style)
            .with_props(props)
            .with_inlines([Inline::text(text, style)])
            .into()
    }

    #[test]
    fn a_page_field_moved_with_its_keep_chain_shows_the_page_it_lands_on() {
        use crate::ir::FieldKind;
        // Seven lines, the field's paragraph kept with the next, and one more line: the
        // chain moves to page 2, after the field's paragraph was laid out for page 1. In
        // the middle of a chain too.
        for chain in [1, 2] {
            let document = document(|style| {
                let mut blocks: Vec<Block> = (0..8 - chain)
                    .map(|_| lines(style, 1, spacing(0.0, 0.0)))
                    .collect();
                if chain == 2 {
                    blocks.push(lines(style, 1, spacing(0.0, 0.0).with_keep_next(true)));
                }
                let Block::Paragraph(field) = fields(style, vec![FieldKind::Page]) else {
                    unreachable!()
                };
                let props = field.props().clone().with_keep_next(true);
                blocks.push(field.with_props(props).into());
                blocks.push(lines(style, 1, spacing(0.0, 0.0)));
                one_section(blocks)
            });
            let (laid, report) = laid(&document, 100);
            let page = &laid.expect("pages").pages[1];
            let expected: &[&str] = if chain == 1 {
                &["2", "A"]
            } else {
                &["A", "2", "A"]
            };
            assert_eq!(texts(page), expected, "chain of {chain}");
            assert!(
                !messages(&report).iter().any(|m| m.contains("PAGE")),
                "{report:?}"
            );
        }
    }

    #[test]
    fn a_page_field_of_a_paragraph_split_over_pages_is_reported() {
        use crate::ir::FieldKind;
        // Seven lines, then a paragraph of lines without widow control, split after its
        // first line, with a `PAGE` field at its end or its start: only the field on page
        // 2, which shows page 1's number, is reported.
        let split = |count: usize, at_start: bool| {
            document(|style| {
                let mut blocks: Vec<Block> =
                    (0..7).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
                let props = spacing(0.0, 0.0).with_widow_control(false);
                let Block::Paragraph(split) = lines(style, count, props) else {
                    unreachable!()
                };
                let mut inlines = split.inlines().to_vec();
                let field = Inline::Field(crate::ir::Field::new(FieldKind::Page, style));
                if at_start {
                    inlines.insert(0, field);
                } else {
                    inlines.push(field);
                }
                blocks.push(split.with_inlines(inlines).into());
                one_section(blocks)
            })
        };
        let reported = |document: &Document| {
            let (laid, report) = laid(document, 100);
            assert_eq!(laid.expect("pages").pages.len(), 2);
            messages(&report)
                .iter()
                .any(|m| m.contains("split over pages"))
        };
        assert!(reported(&split(2, false)));
        assert!(!reported(&split(3, true)));
    }

    #[test]
    fn a_page_field_that_moves_again_when_laid_out_for_its_page_is_reported() {
        use crate::ir::{FieldKind, NumberFormat};
        // A body 12 pt wide and two lines high, where "III" takes two lines and "IV" one.
        // The field's paragraph, kept together, does not fit the last line of page 3 laid
        // out for it; laid out for page 4, it does, and goes back to page 3.
        let document = document(|style| {
            let narrow = PageSetup::new(PageSize::new(Pt(52.0), Pt(80.0))).with_margins(
                Margins::default()
                    .with_top(Pt(20.0))
                    .with_bottom(Pt(20.0))
                    .with_left(Pt(20.0))
                    .with_right(Pt(20.0)),
            );
            let mut blocks: Vec<Block> = (0..5)
                .map(|_| text_paragraph(style, "A", spacing(0.0, 0.0)))
                .collect();
            let roman = Paragraph::new(style)
                .with_props(spacing(0.0, 0.0).with_keep_lines(true))
                .with_inlines([Inline::Field(
                    crate::ir::Field::new(FieldKind::Page, style)
                        .with_format(Some(NumberFormat::UpperRoman)),
                )]);
            blocks.push(roman.into());
            vec![Section::new(narrow).with_blocks(blocks)]
        });
        let (laid, report) = laid(&document, 100);
        assert_eq!(texts(&laid.expect("pages").pages[2]), ["A", "IV"]);
        assert!(
            messages(&report)
                .iter()
                .any(|m| m.contains("next to the one")),
            "{report:?}"
        );
    }

    #[test]
    fn each_section_has_its_own_headers() {
        // A header of one line in the first section, of three in the second: the body of
        // the second section's page starts below its own.
        let document = document(|style| {
            let section = |header: usize| {
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary(frame_of(style, header)))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))])
            };
            vec![section(1), section(3)]
        });
        let (laid, _) = laid(&document, 100);
        let tops: Vec<Pt> = laid
            .expect("pages")
            .pages
            .iter()
            .map(|page| page.body.top)
            .collect();
        assert_eq!(tops, [Pt(30.0), Pt(70.0)]);
    }

    #[test]
    fn a_page_field_in_a_footer_table_shows_each_page() {
        use crate::ir::FieldKind;
        let document = document(|style| {
            let cell = Cell::default().with_blocks([fields(style, vec![FieldKind::Page])]);
            let table: Block = Table::default()
                .with_rows(vec![Row::default().with_cells(vec![cell])])
                .into();
            let blocks: Vec<Block> = (0..3)
                .map(|_| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(true)))
                .collect();
            vec![
                Section::new(close_frames())
                    .with_footers(HeaderFooterSet::default().with_primary(vec![table]))
                    .with_blocks(blocks),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let numbers: Vec<String> = laid
            .expect("pages")
            .pages
            .iter()
            .filter_map(|page| texts(page).first().cloned())
            .collect();
        assert_eq!(numbers, ["1", "2", "3"]);
    }

    #[test]
    fn a_footer_taller_than_its_page_keeps_its_last_paragraphs() {
        // A footer grows up from its distance: what is cut is its top.
        let document = document(|style| {
            let footer = (0..30)
                .map(|n| text_paragraph(style, &format!("F{n}"), spacing(0.0, 0.0)))
                .collect::<Vec<Block>>();
            vec![
                Section::new(close_frames())
                    .with_footers(HeaderFooterSet::default().with_primary(footer))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let drawn = texts(&laid.expect("pages").pages[0]);
        assert!(drawn.iter().any(|text| text == "F29"), "{drawn:?}");
        assert!(!drawn.iter().any(|text| text == "F0"), "{drawn:?}");
    }

    #[test]
    fn a_frame_of_too_many_paragraphs_is_cut() {
        // Exact lines of 0.1 pt: three hundred paragraphs fit the page, 256 are drawn, the
        // first ones of a header and the last ones of a footer.
        let frame = |style| {
            let tiny = ParagraphProps::default()
                .with_spacing(Spacing::default().with_line(LineSpacing::Exact(Pt(0.1))));
            (0..300)
                .map(|n| text_paragraph(style, &format!("F{n}"), tiny.clone()))
                .collect::<Vec<Block>>()
        };
        for header in [true, false] {
            let document = document(|style| {
                let set = HeaderFooterSet::default().with_primary(frame(style));
                let section = Section::new(close_frames());
                let section = if header {
                    section.with_headers(set)
                } else {
                    section.with_footers(set)
                };
                vec![section.with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))])]
            });
            let (laid, report) = laid(&document, 100);
            let drawn = texts(&laid.expect("pages").pages[0]);
            assert_eq!(drawn.len(), MAX_FRAME_WORK + 1);
            let (kept, left_out) = if header {
                ("F0", "F299")
            } else {
                ("F299", "F0")
            };
            assert!(drawn.iter().any(|text| text == kept), "{drawn:?}");
            assert!(!drawn.iter().any(|text| text == left_out), "{drawn:?}");
            assert!(
                messages(&report).iter().any(|m| m.contains("cut")),
                "{report:?}"
            );
        }
    }

    #[test]
    fn a_frame_s_tables_count_their_paragraphs_rows_cells_and_columns_towards_its_bound() {
        // A table is laid out again on every page where a field in it changes, so each of
        // its paragraphs, rows, cells and grid columns counts. Exact lines of 0.1 pt, so
        // that all fit.
        let tiny = ParagraphProps::default()
            .with_spacing(Spacing::default().with_line(LineSpacing::Exact(Pt(0.1))));
        // Rows of one cell of one paragraph, over one column: 3 for each row, and 1.
        let table = |style, rows: usize| -> Block {
            let rows = (0..rows)
                .map(|n| {
                    let text = text_paragraph(style, &format!("T{n}"), tiny.clone());
                    Row::default().with_cells(vec![Cell::default().with_blocks([text])])
                })
                .collect::<Vec<Row>>();
            Table::default()
                .with_grid([Pt(100.0)])
                .with_rows(rows)
                .into()
        };
        let drawn = |header: &dyn Fn(RunStyleId) -> Vec<Block>| {
            let document = document(|style| {
                vec![
                    Section::new(close_frames())
                        .with_headers(HeaderFooterSet::default().with_primary(header(style)))
                        .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
                ]
            });
            let (laid, report) = laid(&document, 100);
            (texts(&laid.expect("pages").pages[0]), messages(&report))
        };
        let has = |texts: &[String], text: &str| texts.iter().any(|drawn| drawn == text);
        let cut = |report: &[String]| report.iter().any(|m| m.contains("is cut"));
        // A table of 84 rows (253) and three paragraphs: 256, all drawn; the next is cut.
        let (texts, report) = drawn(&|style| {
            vec![
                table(style, 84),
                text_paragraph(style, "in", tiny.clone()),
                text_paragraph(style, "in", tiny.clone()),
                text_paragraph(style, "in", tiny.clone()),
                text_paragraph(style, "out", tiny.clone()),
            ]
        });
        assert!(has(&texts, "T0") && has(&texts, "T83") && has(&texts, "in"));
        assert!(!has(&texts, "out"), "{texts:?}");
        assert!(cut(&report), "{report:?}");
        // One paragraph, then a table of 85 rows (256): the table is cut.
        let (texts, _) = drawn(&|style| {
            vec![
                text_paragraph(style, "first", tiny.clone()),
                table(style, 85),
            ]
        });
        assert!(has(&texts, "first"));
        assert!(
            !texts.iter().any(|drawn| drawn.starts_with('T')),
            "{texts:?}"
        );
        // Whatever its paragraphs: one cell over a grid of 300 columns, or a row of 300
        // cells without content, is cut too.
        let (texts, report) = drawn(&|style| {
            vec![
                Table::default()
                    .with_grid(vec![Pt(0.1); 300])
                    .with_rows(vec![Row::default().with_cells(vec![
                        Cell::default().with_blocks([text_paragraph(style, "W", tiny.clone())]),
                    ])])
                    .into(),
            ]
        });
        assert!(!has(&texts, "W") && cut(&report), "{texts:?} {report:?}");
        // A nested table counts the same: inside a cell of a table of one column.
        let (texts, report) = drawn(&|style| {
            let nested = Table::default()
                .with_grid(vec![Pt(0.1); 300])
                .with_rows(vec![Row::default().with_cells(vec![
                    Cell::default().with_blocks([text_paragraph(style, "N", tiny.clone())]),
                ])]);
            vec![
                Table::default()
                    .with_grid([Pt(100.0)])
                    .with_rows(vec![
                        Row::default()
                            .with_cells(vec![Cell::default().with_blocks([nested.into()])]),
                    ])
                    .into(),
            ]
        });
        assert!(!has(&texts, "N") && cut(&report), "{texts:?} {report:?}");
        let (_, report) = drawn(&|_| {
            vec![
                Table::default()
                    .with_grid([Pt(100.0)])
                    .with_rows(vec![Row::default().with_cells(vec![Cell::default(); 300])])
                    .into(),
            ]
        });
        assert!(cut(&report), "{report:?}");
    }

    #[test]
    fn no_header_is_laid_out_past_the_budget_of_what_headers_draw() {
        use crate::ir::FieldKind;

        // Issue #374: a header with a `PAGE` field is laid out again for each page, but
        // past the budget of what headers and footers draw nothing more of it is drawn, so
        // it is not laid out either. A budget of 11 draws the page number on a few of 100
        // pages; the rest keep the room the header takes and lay out none.
        let section = |style, pages: usize| {
            let blocks: Vec<Block> = (0..pages)
                .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
                .collect();
            Section::new(close_frames())
                .with_headers(
                    HeaderFooterSet::default().with_primary([fields(style, vec![FieldKind::Page])]),
                )
                .with_blocks(blocks)
        };
        let lay = |document: &Document| {
            let book = book(vec![minimal()]);
            let mut fonts = Fonts::new(&book);
            let mut shaper = Shaper::new();
            let laid = paginate_with(
                document,
                &mut fonts,
                &mut shaper,
                1000,
                (11, MAX_FRAME_LAID),
                Vec::new,
            )
            .expect("pages");
            (laid, messages(&fonts.into_report()))
        };
        // The pages whose header is drawn: the body's text is "A", the header's a number.
        let numbered = |page: &Page| texts(page).iter().any(|text| text != "A");
        let (laid, report) = lay(&document(|style| vec![section(style, 100)]));
        assert_eq!(laid.sink.len(), 100);
        let drawn = laid.sink.iter().filter(|page| numbered(page)).count();
        assert!(drawn > 0 && drawn < 10, "{drawn}");
        // Laid out: the header of each page that draws it, of the next, whose drawing
        // spends the budget, and the empty footer once.
        assert_eq!(laid.frames_laid, drawn + 2, "{drawn}");
        let tops: Vec<f64> = laid.sink.iter().map(|page| page.body.top.0).collect();
        assert!(tops.iter().all(|&top| top == tops[0]), "{tops:?}");
        assert!(
            report.iter().any(|m| m.contains("left out past")),
            "{report:?}"
        );
        // A section whose header was never laid out before the budget was spent lays out
        // none of its own either.
        let (two, _) = lay(&document(|style| {
            vec![section(style, 50), section(style, 50)]
        }));
        assert_eq!(two.sink.len(), 100);
        assert_eq!(two.frames_laid, laid.frames_laid);
        assert!(!two.sink[50..].iter().any(numbered));
    }

    /// `document` laid out with budgets of `bounds` for what headers and footers draw and
    /// lay out, and the messages of the report.
    fn laid_within(document: &Document, bounds: (usize, usize)) -> (Laid<Vec<Page>>, Vec<String>) {
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let laid = paginate_with(document, &mut fonts, &mut shaper, 1000, bounds, Vec::new)
            .expect("pages");
        (laid, messages(&fonts.into_report()))
    }

    #[test]
    fn a_header_never_laid_out_past_the_budget_is_reported() {
        // Review of #379: section 1 of 5 pages with a header costing 2 a page; a budget of
        // 10 is spent by its headers exactly. A section 2 with a header and a footer of its
        // own has neither laid out nor drawn, and that is reported; one with neither leaves
        // nothing out, and nothing is reported.
        let document = |frames: bool| {
            document(|style| {
                let frame = |text: &str| [text_paragraph(style, text, spacing(0.0, 0.0))];
                let pages = |count: usize| -> Vec<Block> {
                    (0..count)
                        .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
                        .collect()
                };
                let first = Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary(frame("H")))
                    .with_blocks(pages(5));
                let second = Section::new(close_frames()).with_blocks(pages(3));
                let second = if frames {
                    second
                        .with_headers(HeaderFooterSet::default().with_primary(frame("S")))
                        .with_footers(HeaderFooterSet::default().with_primary(frame("T")))
                } else {
                    second
                };
                vec![first, second]
            })
        };
        let left_out = |report: &[String]| report.iter().any(|m| m.contains("left out past"));
        let (laid, report) = laid_within(&document(true), (10, MAX_FRAME_LAID));
        let framed = laid
            .sink
            .iter()
            .filter(|page| texts(page).iter().any(|text| text != "A"))
            .count();
        assert_eq!(framed, 5);
        assert!(left_out(&report), "{report:?}");
        let (_, report) = laid_within(&document(false), (10, MAX_FRAME_LAID));
        assert!(!left_out(&report), "{report:?}");
    }

    #[test]
    fn a_header_field_whose_result_lays_out_much_is_bounded_too() {
        use crate::ir::{Field, FieldKind};

        // Re-check of #379: a header of a `PAGE` field and a `DATE` field whose stored
        // result is 1,000 tabs: with 5,000 to lay out, it is laid out and drawn on five
        // pages, then reported.
        let document = document(|style| {
            let date = Field::new(FieldKind::Other("DATE".into()), style)
                .with_result((0..1000).map(|_| Inline::tab(style)).collect::<Vec<_>>());
            let header = Paragraph::new(style)
                .with_props(spacing(0.0, 0.0))
                .with_inlines(vec![
                    Inline::Field(Field::new(FieldKind::Page, style)),
                    Inline::Field(date),
                ]);
            let blocks: Vec<Block> = (0..100)
                .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
                .collect();
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary([header.into()]))
                    .with_blocks(blocks),
            ]
        });
        let (laid, report) = laid_within(&document, (MAX_FRAME_DRAWN, 5000));
        let numbered = laid
            .sink
            .iter()
            .filter(|page| texts(page).iter().any(|text| text != "A"))
            .count();
        assert_eq!(numbered, 5);
        assert!(
            report.iter().any(|m| m.contains("left out past")),
            "{report:?}"
        );
    }

    #[test]
    fn what_a_header_does_not_lay_out_costs_it_nothing_of_the_bound() {
        use crate::ir::{Field, FieldKind};

        // Second re-check of #379: a header's `PAGE` field shows the page's number, not
        // its stored result, and hidden text is not laid out, so neither counts towards
        // what headers lay out. A stored result of 1,000 tabs, or 10,000 hidden
        // characters, beside the field, with 5,000 to lay out: drawn on all 100 pages.
        let numbered = |hidden_text: bool| {
            let mut document = Document::default();
            let style = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(10.0)),
            );
            let hidden = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(10.0))
                    .with_hidden(true),
            );
            let inlines = if hidden_text {
                vec![
                    Inline::Field(Field::new(FieldKind::Page, style)),
                    Inline::text("x".repeat(10_000), hidden),
                ]
            } else {
                vec![Inline::Field(
                    Field::new(FieldKind::Page, style)
                        .with_result((0..1000).map(|_| Inline::tab(style)).collect::<Vec<_>>()),
                )]
            };
            let header = Paragraph::new(style)
                .with_props(spacing(0.0, 0.0))
                .with_inlines(inlines);
            let blocks: Vec<Block> = (0..100)
                .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
                .collect();
            document.add_section(
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary([header.into()]))
                    .with_blocks(blocks),
            );
            let (laid, report) = laid_within(&document, (MAX_FRAME_DRAWN, 5000));
            assert!(
                !report.iter().any(|m| m.contains("left out past")),
                "{report:?}"
            );
            laid.sink
                .iter()
                .filter(|page| texts(page).iter().any(|text| text != "A"))
                .count()
        };
        assert_eq!(numbered(false), 100);
        assert_eq!(numbered(true), 100);
    }

    #[test]
    fn results_a_header_does_not_lay_out_cost_it_nothing_of_the_bound() {
        use crate::ir::{Field, FieldKind};

        // Third re-check of #379. Beside a header's `PAGE` field: a `NUMPAGES` or `SECTIONPAGES` field whose stored
        // result is 1,000 tabs, or a `DATE` field whose stored result is 10,000 hidden
        // characters. None of them is laid out, so with 5,000 to lay out the header is drawn
        // on all 100 pages, and nothing is reported.
        let numbered = |case: u8| {
            let mut document = Document::default();
            let style = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(10.0)),
            );
            let hidden = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(10.0))
                    .with_hidden(true),
            );
            let tabs = || (0..1000).map(|_| Inline::tab(style)).collect::<Vec<_>>();
            let other = match case {
                0 => Field::new(FieldKind::NumPages, style).with_result(tabs()),
                1 => Field::new(FieldKind::SectionPages, style).with_result(tabs()),
                _ => Field::new(FieldKind::Other("DATE".into()), style)
                    .with_result([Inline::text("x".repeat(10_000), hidden)]),
            };
            let header = Paragraph::new(style)
                .with_props(spacing(0.0, 0.0))
                .with_inlines(vec![
                    Inline::Field(Field::new(FieldKind::Page, style)),
                    Inline::Field(other),
                ]);
            let blocks: Vec<Block> = (0..100)
                .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
                .collect();
            document.add_section(
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary([header.into()]))
                    .with_blocks(blocks),
            );
            let (laid, report) = laid_within(&document, (MAX_FRAME_DRAWN, 5000));
            let count = laid
                .sink
                .iter()
                .filter(|page| texts(page).iter().any(|text| text != "A"))
                .count();
            (count, report.iter().any(|m| m.contains("left out past")))
        };
        assert_eq!(numbered(0), (100, false));
        assert_eq!(numbered(1), (100, false));
        assert_eq!(numbered(2), (100, false));
    }

    #[test]
    fn a_header_table_that_lays_out_much_and_draws_little_is_bounded_too() {
        use crate::ir::FieldKind;

        // Re-check of #379: a header table of a `PAGE` field in one cell and 1,000 tabs in
        // the other costs 1,008 (cells 3 and 1,002, a row, two columns): with 5,000 to lay
        // out, it is laid out and drawn on five pages, then left out and reported.
        let document = document(|style| {
            let tabs = Paragraph::new(style)
                .with_props(spacing(0.0, 0.0))
                .with_inlines((0..1000).map(|_| Inline::tab(style)).collect::<Vec<_>>());
            let table: Block = Table::default()
                .with_grid([Pt(100.0), Pt(100.0)])
                .with_rows(vec![Row::default().with_cells(vec![
                    Cell::default().with_blocks([fields(style, vec![FieldKind::Page])]),
                    Cell::default().with_blocks([tabs.into()]),
                ])])
                .into();
            let blocks: Vec<Block> = (0..100)
                .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
                .collect();
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary([table]))
                    .with_blocks(blocks),
            ]
        });
        let (laid, report) = laid_within(&document, (MAX_FRAME_DRAWN, 5000));
        let numbered = laid
            .sink
            .iter()
            .filter(|page| texts(page).iter().any(|text| text != "A"))
            .count();
        assert_eq!(numbered, 5);
        assert!(
            report.iter().any(|m| m.contains("left out past")),
            "{report:?}"
        );
    }

    #[test]
    fn a_frame_left_out_past_the_layout_bound_is_laid_out_on_a_page_made_again() {
        use crate::ir::FieldKind;

        // Re-check of #379: even and odd headers of a `PAGE` field, footers "F" and "E",
        // and 6 to lay out. Page 1 spends 4. A paragraph of a `PAGE` field laid out for
        // page 1 lands on page 2: placing it first, page 2's header spends the last 2, so
        // its footer, never laid out, gets an empty stand-in. Page 2 made again gets 2 back
        // and lays out its footer.
        let mut document =
            Document::new(crate::ir::DocumentSettings::default().with_even_and_odd_headers(true));
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let mut blocks: Vec<Block> = (0..7).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
        let mut inlines = vec![Inline::Field(crate::ir::Field::new(FieldKind::Page, style))];
        for _ in 0..14 {
            inlines.push(Inline::line_break(BreakClear::None, style));
            inlines.push(Inline::text("A", style));
        }
        blocks.push(
            Paragraph::new(style)
                .with_props(spacing(0.0, 0.0))
                .with_inlines(inlines)
                .into(),
        );
        let page = || [fields(style, vec![FieldKind::Page])];
        document.add_section(
            Section::new(close_frames())
                .with_headers(
                    HeaderFooterSet::default()
                        .with_primary(page())
                        .with_even(page()),
                )
                .with_footers(
                    HeaderFooterSet::default()
                        .with_primary([text_paragraph(style, "F", spacing(0.0, 0.0))])
                        .with_even([text_paragraph(style, "E", spacing(0.0, 0.0))]),
                )
                .with_blocks(blocks),
        );
        let (laid, _) = laid_within(&document, (MAX_FRAME_DRAWN, 6));
        assert!(texts(&laid.sink[1]).iter().any(|text| text == "E"));
    }

    #[test]
    fn a_header_that_lays_out_much_and_draws_little_is_bounded_too() {
        use crate::ir::FieldKind;

        // Review of #379: a header of a `PAGE` field and 1,000 tabs draws one number but
        // lays out a thousand tabs on every page. With 5,000 to lay out, it is laid out for
        // five pages; after that neither laid out nor drawn, and reported.
        let document = document(|style| {
            let mut inlines = vec![Inline::Field(crate::ir::Field::new(FieldKind::Page, style))];
            inlines.extend((0..1000).map(|_| Inline::tab(style)));
            let header = Paragraph::new(style)
                .with_props(spacing(0.0, 0.0))
                .with_inlines(inlines);
            let blocks: Vec<Block> = (0..100)
                .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
                .collect();
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary([header.into()]))
                    .with_blocks(blocks),
            ]
        });
        let (laid, report) = laid_within(&document, (MAX_FRAME_DRAWN, 5000));
        assert_eq!(laid.sink.len(), 100);
        let numbered = laid
            .sink
            .iter()
            .filter(|page| texts(page).iter().any(|text| text != "A"))
            .count();
        assert_eq!(numbered, 5);
        // The five headers, and the empty footer once.
        assert_eq!(laid.frames_laid, 6);
        assert!(
            report.iter().any(|m| m.contains("left out past")),
            "{report:?}"
        );
    }

    #[test]
    fn a_page_laid_out_again_lays_out_its_header_again() {
        use crate::ir::FieldKind;

        // Review of #379: a header of a `PAGE` field, with a budget of 6 (three headers).
        // Seven lines fill page 1 under it; a paragraph of a `PAGE` field and 14 more lines,
        // laid out for page 1, lands on page 2, so pages 2 to 4 are made again: each shows
        // its own number in its header, and page 4, past the budget, none.
        let document = document(|style| {
            let mut blocks: Vec<Block> =
                (0..7).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            let mut inlines = vec![Inline::Field(crate::ir::Field::new(FieldKind::Page, style))];
            for _ in 0..14 {
                inlines.push(Inline::line_break(BreakClear::None, style));
                inlines.push(Inline::text("A", style));
            }
            blocks.push(
                Paragraph::new(style)
                    .with_props(spacing(0.0, 0.0))
                    .with_inlines(inlines)
                    .into(),
            );
            vec![
                Section::new(close_frames())
                    .with_headers(
                        HeaderFooterSet::default()
                            .with_primary([fields(style, vec![FieldKind::Page])]),
                    )
                    .with_blocks(blocks),
            ]
        });
        let first = |bounds| -> Vec<String> {
            let (laid, _) = laid_within(&document, bounds);
            laid.sink
                .iter()
                .filter_map(|page| texts(page).first().cloned())
                .collect()
        };
        assert_eq!(first((6, MAX_FRAME_LAID)), ["1", "2", "3", "A"]);
        // The same with what headers may lay out as the bound: three layouts of the
        // header's paragraph and field (2 each). The page made again gets that back too.
        assert_eq!(first((MAX_FRAME_DRAWN, 6)), ["1", "2", "3", "A"]);
    }

    #[test]
    fn a_header_that_paints_much_and_draws_little_is_bounded() {
        // Issue #384: a header without fields is laid out once, but painted on every page.
        // With 5,000 to paint, each header below is painted on a few of 100 pages, then left
        // out and reported, and painting walks no more once that is spent.
        let painted = |header: &dyn Fn(RunStyleId) -> Vec<Block>| {
            let document = document(|style| {
                let blocks: Vec<Block> = (0..100)
                    .map(|n| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(n > 0)))
                    .collect();
                vec![
                    Section::new(close_frames())
                        .with_headers(HeaderFooterSet::default().with_primary(header(style)))
                        .with_blocks(blocks),
                ]
            });
            let (laid, report) = laid_within(&document, (5000, MAX_FRAME_LAID));
            assert_eq!(laid.sink.len(), 100);
            assert!(
                report.iter().any(|m| m.contains("left out past")),
                "{report:?}"
            );
            let headed = laid
                .sink
                .iter()
                .filter(|page| texts(page).iter().any(|text| text == "H"))
                .count();
            (headed, laid.frames_walked)
        };
        let tiny = || {
            ParagraphProps::default()
                .with_spacing(Spacing::default().with_line(LineSpacing::Exact(Pt(0.1))))
        };
        // "H" and 1,000 tabs on one line draw a run and a glyph, while painting walks
        // 1,001 pieces: four pages, and no more walked than the budget and one header.
        let (headed, walked) = painted(&|style| {
            let mut inlines = vec![Inline::text("H", style)];
            inlines.extend((0..1000).map(|_| Inline::tab(style)));
            vec![
                Paragraph::new(style)
                    .with_props(tiny())
                    .with_inlines(inlines)
                    .into(),
            ]
        });
        assert_eq!(headed, 4);
        assert!(walked <= 5000 + 1001, "{walked}");
        // "H" and 1,000 line breaks: 1,001 lines of one piece each, the same.
        let (headed, _) = painted(&|style| {
            let mut inlines = vec![Inline::text("H", style)];
            for _ in 0..1000 {
                inlines.push(Inline::line_break(BreakClear::None, style));
            }
            vec![
                Paragraph::new(style)
                    .with_props(tiny())
                    .with_inlines(inlines)
                    .into(),
            ]
        });
        assert_eq!(headed, 4);
        // "H", then 200 empty paragraphs, a line each with no piece, each of which counts
        // one: 202 a page, so "H" is painted on 25 pages (on all 100 if they counted none).
        let (headed, _) = painted(&|style| {
            let mut blocks = vec![text_paragraph(style, "H", tiny())];
            blocks.extend((0..200).map(|_| Paragraph::new(style).with_props(tiny()).into()));
            blocks
        });
        assert_eq!(headed, 25);
    }

    #[test]
    fn a_glyph_costs_the_characters_it_stands_for() {
        // Review of #392: painting copies the text a glyph stands for into the PDF on every
        // page, so a cluster costs its characters where those are more than its glyphs. A
        // run of one glyph standing for 1,000 characters costs 1,001.
        let face = book(vec![minimal()])
            .faces()
            .next()
            .map(|(id, _)| id)
            .expect("the test font");
        let run = |text: String, glyphs: usize| {
            DisplayItem::Glyphs(crate::display::GlyphItem {
                face,
                size: Pt(10.0),
                color: officewright_core::Color::BLACK,
                h_scale: 1.0,
                synthetic_bold: false,
                synthetic_italic: false,
                origin: Point {
                    x: Pt(0.0),
                    y: Pt(0.0),
                },
                clusters: vec![crate::display::Cluster {
                    text,
                    glyphs: vec![
                        crate::display::DrawnGlyph {
                            glyph: officewright_font::GlyphId::new(1),
                            x: Pt(0.0),
                            y: Pt(0.0),
                        };
                        glyphs
                    ],
                }],
            })
        };
        assert_eq!(drawn(&run("\u{200b}".repeat(1000), 1)), 1001);
        assert_eq!(drawn(&run("fi".to_owned(), 1)), 3);
        assert_eq!(drawn(&run("A".to_owned(), 3)), 4);
    }

    #[test]
    fn what_headers_and_footers_draw_in_all_is_bounded() {
        // A budget of 11 glyphs and shapes: a header of one "A" (a run and a glyph, 2)
        // is drawn on five pages; the 1 left does not draw the sixth, nor the rest, which
        // is reported.
        let pages_of_breaks = document(|style| {
            let blocks: Vec<Block> = (0..8)
                .map(|_| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(true)))
                .collect();
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary(frame_of(style, 1)))
                    .with_blocks(blocks),
            ]
        });
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let laid = paginate_with(
            &pages_of_breaks,
            &mut fonts,
            &mut shaper,
            100,
            (11, MAX_FRAME_LAID),
            Vec::new,
        )
        .expect("pages");
        let counts: Vec<usize> = laid.sink.iter().map(|page| texts(page).len()).collect();
        assert_eq!(counts, [2, 2, 2, 2, 2, 1, 1, 1]);
        let report = fonts.into_report();
        assert!(
            messages(&report)
                .iter()
                .any(|m| m.contains("left out past")),
            "{report:?}"
        );
        // A page laid out again gives back what its header drew: seven lines fill page 1
        // under its header, and the page of a field laid out for page 1 and landed on page
        // 2 is made twice. A budget of two headers still draws both.
        let moved = document(|style| {
            let mut blocks: Vec<Block> =
                (0..7).map(|_| lines(style, 1, spacing(0.0, 0.0))).collect();
            blocks.push(fields(style, vec![crate::ir::FieldKind::Page]));
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary(frame_of(style, 1)))
                    .with_blocks(blocks),
            ]
        });
        let mut fonts = Fonts::new(&book);
        let laid = paginate_with(
            &moved,
            &mut fonts,
            &mut shaper,
            100,
            (4, MAX_FRAME_LAID),
            Vec::new,
        )
        .expect("pages");
        let headers: Vec<String> = laid
            .sink
            .iter()
            .filter_map(|page| texts(page).first().cloned())
            .collect();
        assert_eq!(headers, ["A", "A"]);
        assert_eq!(texts(&laid.sink[1]), ["A", "2"]);
    }

    #[test]
    fn chapter_numbers_are_reported_only_with_page_fields() {
        use crate::ir::PageNumbering;
        let document = document(|style| {
            vec![
                Section::new(close_frames())
                    .with_page_numbering(PageNumbering::default().with_chapter_style(Some(1)))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (_, report) = laid(&document, 100);
        assert!(
            !messages(&report).iter().any(|m| m.contains("chapter")),
            "{report:?}"
        );
    }

    #[test]
    fn a_footer_keeps_its_paragraphs_in_order_when_laid_out_again() {
        use crate::ir::FieldKind;
        // "X", the page number and "Y": laid out again on every page for its number, with
        // "X" and "Y" kept from the page before.
        let document = document(|style| {
            let footer = vec![
                text_paragraph(style, "X", spacing(0.0, 0.0)),
                fields(style, vec![FieldKind::Page]),
                text_paragraph(style, "Y", spacing(0.0, 0.0)),
            ];
            let blocks: Vec<Block> = (0..3)
                .map(|_| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(true)))
                .collect();
            vec![
                Section::new(close_frames())
                    .with_footers(HeaderFooterSet::default().with_primary(footer))
                    .with_blocks(blocks),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let footers: Vec<Vec<String>> = laid
            .expect("pages")
            .pages
            .iter()
            .map(|page| texts(page).into_iter().take(3).collect())
            .collect();
        assert_eq!(footers, [["X", "1", "Y"], ["X", "2", "Y"], ["X", "3", "Y"]]);
    }

    #[test]
    fn a_section_page_count_alone_is_counted() {
        use crate::ir::FieldKind;
        let document = document(|style| {
            let blocks: Vec<Block> = (0..3)
                .map(|_| lines(style, 1, spacing(0.0, 0.0).with_page_break_before(true)))
                .collect();
            vec![
                Section::new(close_frames())
                    .with_footers(
                        HeaderFooterSet::default()
                            .with_primary(vec![fields(style, vec![FieldKind::SectionPages])]),
                    )
                    .with_blocks(blocks),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let counts: Vec<String> = laid
            .expect("pages")
            .pages
            .iter()
            .filter_map(|page| texts(page).first().cloned())
            .collect();
        assert_eq!(counts, ["3", "3", "3"]);
    }

    #[test]
    fn the_cut_at_the_page_s_height_counts_the_space_between_paragraphs() {
        // Twenty one-line paragraphs of 20 pt with 100 pt before each, on a page 200 pt
        // high: the second would start 220 pt down, and is not laid out. A header keeps
        // its first ones.
        let document = document(|style| {
            let header = (0..20)
                .map(|n| text_paragraph(style, &format!("H{n}"), spacing(100.0, 0.0)))
                .collect::<Vec<Block>>();
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary(header))
                    .with_blocks(vec![text_paragraph(style, "B", spacing(0.0, 0.0))]),
            ]
        });
        let (laid, report) = laid(&document, 100);
        let drawn = texts(&laid.expect("pages").pages[0]);
        assert_eq!(drawn, ["H0", "B"]);
        assert!(
            messages(&report).iter().any(|m| m.contains("cut")),
            "{report:?}"
        );
    }

    #[test]
    fn a_page_field_is_reported_only_where_it_shows_the_wrong_page() {
        use crate::ir::{FieldKind, NumberFormat, PageNumbering};
        // A paragraph without widow control after `before` lines, of lines "A" and a
        // `PAGE` field as `pieces` says, with line breaks between.
        let split = |before: usize, pieces: &[bool], numbering: PageNumbering| {
            document(|style| {
                let mut blocks: Vec<Block> = (0..before)
                    .map(|_| lines(style, 1, spacing(0.0, 0.0)))
                    .collect();
                let mut inlines = Vec::new();
                for (at, field) in pieces.iter().enumerate() {
                    if at > 0 {
                        inlines.push(Inline::line_break(BreakClear::None, style));
                    }
                    inlines.push(if *field {
                        Inline::Field(crate::ir::Field::new(FieldKind::Page, style))
                    } else {
                        Inline::text("A", style)
                    });
                }
                blocks.push(
                    Paragraph::new(style)
                        .with_props(spacing(0.0, 0.0).with_widow_control(false))
                        .with_inlines(inlines)
                        .into(),
                );
                vec![
                    Section::new(setup())
                        .with_page_numbering(numbering.clone())
                        .with_blocks(blocks),
                ]
            })
        };
        let reported = |document: &Document| {
            let (laid, report) = laid(document, 100);
            assert_eq!(laid.expect("pages").pages.len(), 2);
            messages(&report)
                .iter()
                .any(|m| m.contains("split over pages"))
        };
        let decimal = PageNumbering::default();
        // The field starts page 2's text, right after a line break.
        assert!(reported(&split(7, &[false, true], decimal.clone())));
        // The field is on the second of the paragraph's two lines on page 1.
        assert!(!reported(&split(6, &[false, true, false, false], decimal)));
        // On page 2 the field shows nothing: Chicago marks stop at 116.
        let chicago = PageNumbering::default()
            .with_format(NumberFormat::Chicago)
            .with_start(Some(117));
        assert!(!reported(&split(7, &[false, true], chicago)));
    }

    #[test]
    fn a_footer_cut_from_its_top_paints_each_paragraph_with_its_own_properties() {
        use crate::ir::{ColorValue, Shading};
        use officewright_core::Color;
        // Thirty footer paragraphs of 20 pt on a page 200 pt high, only the last shaded:
        // the last ten are drawn, and one shading with them.
        let document = document(|style| {
            let shaded = spacing(0.0, 0.0).with_shading(Some(
                Shading::default().with_fill(ColorValue::Rgb(Color::rgb(0, 0, 255))),
            ));
            let footer = (0..30)
                .map(|n| {
                    let props = if n == 29 {
                        shaded.clone()
                    } else {
                        spacing(0.0, 0.0)
                    };
                    text_paragraph(style, &format!("F{n}"), props)
                })
                .collect::<Vec<Block>>();
            vec![
                Section::new(close_frames())
                    .with_footers(HeaderFooterSet::default().with_primary(footer))
                    .with_blocks(vec![lines(style, 1, spacing(0.0, 0.0))]),
            ]
        });
        let (laid, _) = laid(&document, 100);
        let page = &laid.expect("pages").pages[0];
        let shading = page
            .items
            .iter()
            .filter(|item| matches!(item, DisplayItem::Rect { color, .. } if *color == Color::rgb(0, 0, 255)))
            .count();
        assert_eq!(shading, 1);
        assert!(texts(page).iter().any(|text| text == "F29"));
    }

    #[test]
    fn the_cut_counts_the_spacing_on_each_side_of_a_paragraph() {
        // One-line paragraphs of 20 pt on a page 200 pt high, with 80 pt after every
        // other one; what a frame keeps depends on which space lies between which.
        let frame = |style, last_after: f64| {
            (0..10)
                .map(|n| {
                    let after = if n == 9 {
                        last_after
                    } else if n % 2 == 0 {
                        80.0
                    } else {
                        0.0
                    };
                    text_paragraph(style, &format!("P{n}"), spacing(0.0, after))
                })
                .collect::<Vec<Block>>()
        };
        // A header: P0 to 20 pt, P1 after P0's 80 to 120, P2 to 140; P3 would start at
        // 220.
        let header = document(|style| {
            vec![
                Section::new(close_frames())
                    .with_headers(HeaderFooterSet::default().with_primary(frame(style, 0.0)))
                    .with_blocks(vec![text_paragraph(style, "B", spacing(0.0, 0.0))]),
            ]
        });
        let (header_laid, _) = laid(&header, 100);
        assert_eq!(
            texts(&header_laid.expect("pages").pages[0]),
            ["P0", "P1", "P2", "B"]
        );
        // A footer, from its distance up: P9's 90 after it and P9 to 110, P8's 80 after
        // it and P8 to 210; P7 would start at 210.
        let footer = document(|style| {
            vec![
                Section::new(close_frames())
                    .with_footers(HeaderFooterSet::default().with_primary(frame(style, 90.0)))
                    .with_blocks(vec![text_paragraph(style, "B", spacing(0.0, 0.0))]),
            ]
        });
        let (footer_laid, _) = laid(&footer, 100);
        assert_eq!(
            texts(&footer_laid.expect("pages").pages[0]),
            ["P8", "P9", "B"]
        );
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

    #[test]
    fn lists_count_on_through_the_body_and_afresh_in_each_header() {
        use crate::ir::{
            Level, LevelTextPart, ListDefinition, ListInstance, ListItem, NumberFormat,
        };

        let mut document = Document::default();
        let style = document
            .add_run_style(RunStyle::default().with_fonts(RunFonts::new("Officewright Test")));
        let level = Level::default()
            .with_text([
                LevelTextPart::Placeholder(0),
                LevelTextPart::Literal(".".into()),
            ])
            .with_start(1);
        let other = Level::default()
            .with_text([LevelTextPart::Placeholder(0)])
            .with_format(NumberFormat::Other("hebrew1".into()));
        let list = document.add_list(ListDefinition::default().with_levels([level]));
        let hebrew = document.add_list(ListDefinition::default().with_levels([other]));
        let instance = document.add_list_instance(ListInstance::new(list));
        let hebrew = document.add_list_instance(ListInstance::new(hebrew));
        let item = |instance| -> Block {
            Paragraph::new(style)
                .with_list(Some(ListItem::new(instance, 0, style)))
                .with_inlines([Inline::text("A", style)])
                .into()
        };
        let header = HeaderFooterSet::default().with_primary(vec![item(instance)]);
        document.add_section(
            Section::new(close_frames())
                .with_headers(header.clone())
                .with_blocks(vec![item(instance), item(instance)]),
        );
        document.add_section(
            Section::new(close_frames())
                .with_headers(header)
                .with_blocks(vec![item(instance), item(hebrew)]),
        );
        let (laid, report) = laid(&document, 10);
        let labels: Vec<Vec<String>> = laid
            .expect("pages")
            .pages
            .iter()
            .map(|page| texts(page).into_iter().filter(|text| text != "A").collect())
            .collect();
        // The header of each page counts from its start; the body counts on from section
        // to section; a format the layout does not write is decimal, and reported.
        assert_eq!(labels, [vec!["1.", "1.", "2."], vec!["1.", "3.", "0"]]);
        // The report names the first paragraph whose label it wrote in decimal.
        let places: Vec<String> = report
            .iter()
            .filter(|entry| {
                entry.kind() == ReportKind::FeatureUnsupported
                    && entry.message().contains("list numbers")
            })
            .map(|entry| entry.location().to_string())
            .collect();
        assert_eq!(places, ["sections[1].blocks[1]"], "{report:?}");
        // A header's list in such a format is reported too, at the header's paragraph.
        let mut in_header = Document::default();
        let style = in_header
            .add_run_style(RunStyle::default().with_fonts(RunFonts::new("Officewright Test")));
        let other = Level::default()
            .with_text([LevelTextPart::Placeholder(0)])
            .with_format(NumberFormat::Other("hebrew1".into()));
        let hebrew = in_header.add_list(ListDefinition::default().with_levels([other]));
        let hebrew = in_header.add_list_instance(ListInstance::new(hebrew));
        let header = HeaderFooterSet::default().with_primary(vec![
            Paragraph::new(style)
                .with_list(Some(ListItem::new(hebrew, 0, style)))
                .with_inlines([Inline::text("A", style)])
                .into(),
        ]);
        in_header.add_section(
            Section::new(close_frames())
                .with_headers(header)
                .with_blocks(vec![Paragraph::new(style).into()]),
        );
        let (_, report) = self::laid(&in_header, 10);
        let places: Vec<String> = report
            .iter()
            .filter(|entry| entry.message().contains("list numbers"))
            .map(|entry| entry.location().to_string())
            .collect();
        assert_eq!(places.len(), 1, "{report:?}");
        assert!(places[0].contains("headers"), "{places:?}");
    }

    #[test]
    fn a_cut_label_is_reported_once_at_the_first_paragraph_it_is_cut_in() {
        use crate::ir::{Level, LevelTextPart, ListDefinition, ListInstance, ListItem};

        let mut document = Document::default();
        let style = document
            .add_run_style(RunStyle::default().with_fonts(RunFonts::new("Officewright Test")));
        let level = |text: String| {
            Level::default()
                .with_text([LevelTextPart::Literal(text), LevelTextPart::Placeholder(0)])
        };
        let short = document.add_list(ListDefinition::default().with_levels([level("a".into())]));
        let long = document.add_list(
            ListDefinition::default()
                .with_levels([level("a".repeat(crate::lists::MAX_LABEL_CHARS + 1))]),
        );
        let short = document.add_list_instance(ListInstance::new(short));
        let long = document.add_list_instance(ListInstance::new(long));
        let item = |instance| -> Block {
            Paragraph::new(style)
                .with_list(Some(ListItem::new(instance, 0, style)))
                .into()
        };
        document.add_section(Section::new(close_frames()).with_blocks(vec![
            item(short),
            item(long),
            item(long),
        ]));
        let (_, report) = laid(&document, 100);
        let places: Vec<String> = report
            .iter()
            .filter(|entry| {
                entry.kind() == ReportKind::ValueClamped && entry.message().contains("list labels")
            })
            .map(|entry| entry.location().to_string())
            .collect();
        assert_eq!(places, ["sections[0].blocks[1]"], "{report:?}");
    }
}
