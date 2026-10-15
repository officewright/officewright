//! Sections: page setup, columns, headers and footers, and the blocks of the body.

use officewright_core::{PageSize, Pt};

use super::macros::{ir_struct, no_rules};
use super::note::{NoteProps, endnote_position, footnote_position};
use super::paragraph::Block;
use super::rules::{self, WORD_MAX_LENGTH};
use super::walk::{Checker, Clamper};
use crate::Result;

ir_struct! {
    /// A part of the document with its own page setup, columns, headers and footers.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Section {
        /// The size and margins of its pages.
        ref page: PageSetup => with_page;
        /// The columns of its body text.
        ref columns: Columns => with_columns;
        /// Where the section starts.
        copy start: SectionStart => with_start;
        /// Its headers, after the producer has filled in those it inherits from the
        /// previous section.
        ref headers: HeaderFooterSet => with_headers;
        /// Its footers, inherited like the headers.
        ref footers: HeaderFooterSet => with_footers;
        /// Whether its first page has a header and footer of its own (`w:titlePg`).
        copy title_page: bool => with_title_page;
        /// How its pages are numbered.
        ref page_numbering: PageNumbering => with_page_numbering;
        /// Where the body text sits between the top and bottom margins. Only
        /// [`Top`](PageVerticalAlign::Top) is laid out yet.
        copy vertical_align: PageVerticalAlign => with_vertical_align;
        /// How its footnotes are numbered and placed.
        ref footnotes: NoteProps => with_footnotes, clamp footnote_position;
        /// How its endnotes are numbered and placed.
        ref endnotes: NoteProps => with_endnotes, check endnote_position;
        /// The body text, in reading order.
        vec blocks: Block => with_blocks;
    }
}

impl Section {
    /// A section of one column on pages of this setup, starting on a new page, with no
    /// headers, footers or blocks, and notes as a document has them when it says
    /// nothing.
    pub fn new(page: PageSetup) -> Self {
        Self {
            page,
            columns: Columns::default(),
            start: SectionStart::NextPage,
            headers: HeaderFooterSet::default(),
            footers: HeaderFooterSet::default(),
            title_page: false,
            page_numbering: PageNumbering::default(),
            vertical_align: PageVerticalAlign::Top,
            footnotes: NoteProps::footnotes(),
            endnotes: NoteProps::endnotes(),
            blocks: Vec::new(),
        }
    }
}

ir_struct! {
    /// The size and margins of a section's pages.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct PageSetup {
        /// The size of the page, as the document gives it: a landscape page is wider than
        /// it is high.
        copy size: PageSize => with_size, check page_size_non_negative;
        /// The orientation the page is printed in. The size already reflects it.
        copy orientation: Orientation => with_orientation;
        /// The margins.
        copy margins: Margins => with_margins;
    }
    clamp page_size_within_margins;
}

impl PageSetup {
    /// A portrait page of this size with margins of 1 inch on every side, headers and
    /// footers half an inch from the edge, and no gutter.
    pub fn new(size: PageSize) -> Self {
        Self {
            size,
            orientation: Orientation::Portrait,
            margins: Margins::default(),
        }
    }
}

impl Default for PageSetup {
    /// A US Letter page with the margins of [`new`](Self::new).
    fn default() -> Self {
        Self::new(PageSize::LETTER)
    }
}

fn page_size_non_negative(size: &PageSize, checker: &mut Checker<'_>) -> Result<()> {
    if size.width().0 < 0.0 || size.height().0 < 0.0 {
        return Err(checker.error(format!(
            "page size {} by {} pt is negative",
            size.width().0,
            size.height().0
        )));
    }
    Ok(())
}

/// Word requires a page's width and height to be at least the margins across it and at
/// most 22 inches (MS-OI29500 2.1.220 b). The margins are clamped first, each to at most
/// 22 inches, so when they add up to more than that the page is as large as Word allows.
/// Word also counts the widths of explicit columns towards the minimum width; columns
/// are not laid out yet, so they are left out here.
fn page_size_within_margins(setup: &mut PageSetup, clamper: &mut Clamper) {
    let margins = setup.margins;
    let across = margins.left.0 + margins.right.0 + margins.gutter.0;
    let down = margins.top.0.abs() + margins.bottom.0.abs();
    // A margin that is not a number gives no bound; `validate` rejects it.
    if !across.is_finite() || !down.is_finite() {
        return;
    }
    let mut width = setup.size.width();
    let mut height = setup.size.height();
    let max = WORD_MAX_LENGTH;
    rules::limit(
        &mut width,
        Pt(across.min(max.0)),
        max,
        "page width",
        clamper,
    );
    rules::limit(
        &mut height,
        Pt(down.min(max.0)),
        max,
        "page height",
        clamper,
    );
    setup.size = PageSize::new(width, height);
}

ir_struct! {
    /// The margins of a page.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Margins {
        /// From the top edge to the body text. A negative value fixes the body's top
        /// edge at its absolute value even when the header is taller (ECMA-376 Part 1,
        /// §17.6.11).
        copy top: Pt = Pt(72.0) => with_top, clamp rules::vertical_margin;
        /// From the bottom edge to the body text; negative like [`top`](Self::top).
        copy bottom: Pt = Pt(72.0) => with_bottom, clamp rules::vertical_margin;
        /// From the left edge to the text.
        copy left: Pt = Pt(72.0) => with_left, check rules::non_negative, clamp rules::margin;
        /// From the right edge to the text.
        copy right: Pt = Pt(72.0) => with_right, check rules::non_negative, clamp rules::margin;
        /// From the top edge to the header.
        copy header: Pt = Pt(36.0) => with_header, check rules::non_negative, clamp rules::margin;
        /// From the bottom edge to the footer.
        copy footer: Pt = Pt(36.0) => with_footer, check rules::non_negative, clamp rules::margin;
        /// Extra room on the binding side, added to the left margin.
        copy gutter: Pt = Pt(0.0) => with_gutter, check rules::non_negative, clamp rules::margin;
    }
}

/// The orientation a page is printed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Orientation {
    /// Upright.
    #[default]
    Portrait,
    /// Turned on its side.
    Landscape,
}

ir_struct! {
    /// The columns of a section's body text. Only one column is laid out yet.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Columns {
        /// The number of columns of equal width, when [`widths`](Self::widths) is empty.
        copy count: u16 = 1 => with_count;
        /// The space between columns of equal width. ECMA-376 gives half an inch when a
        /// document does not say.
        copy spacing: Pt = Pt(36.0) => with_spacing, check rules::non_negative;
        /// Columns of their own widths, from the start edge; empty for columns of equal
        /// width.
        vec widths: Column = Vec::new() => with_widths;
        /// Whether a line is drawn between the columns.
        copy separator: bool = false => with_separator;
    }
    clamp column_count;
}

/// Word allows 1 to 45 columns (MS-OI29500 2.1.213 a), and requires the number of
/// explicit columns to be that count (2.1.213 c).
fn column_count(columns: &mut Columns, clamper: &mut Clamper) {
    const MAX: u16 = 45;
    if columns.count == 0 || columns.count > MAX {
        let clamped = columns.count.clamp(1, MAX);
        clamper.clamped(format!(
            "{} columns are outside Word's range of 1 to {MAX} and become {clamped}",
            columns.count
        ));
        columns.count = clamped;
    }
    if columns.widths.len() > usize::from(MAX) {
        clamper.clamped(format!(
            "{} columns of their own widths are more than Word's {MAX}; the columns after \
             the first {MAX} are left out",
            columns.widths.len()
        ));
        columns.widths.truncate(usize::from(MAX));
    }
}

ir_struct! {
    /// A column of its own width.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Column {
        /// The width of the column.
        copy width: Pt = Pt(0.0) => with_width, check rules::non_negative;
        /// The space after the column, before the next one.
        copy spacing: Pt = Pt(0.0) => with_spacing, check rules::non_negative;
    }
}

/// Where a section starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SectionStart {
    /// On a new page.
    #[default]
    NextPage,
    /// On the same page, below the previous section.
    Continuous,
    /// On a page whose number is even: on the next page, numbered one more when its
    /// number would be odd, with a blank page for the skipped number when the section
    /// continues the numbering of the one before.
    EvenPage,
    /// On a page whose number is odd, as [`EvenPage`](Self::EvenPage) is on an even one.
    OddPage,
    /// In the next column.
    NextColumn,
}

ir_struct! {
    /// The headers or the footers of a section. An empty list means an empty header or
    /// footer, which takes no room.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct HeaderFooterSet {
        /// For every page the other two do not cover (`w:type="default"`).
        vec primary: Block => with_primary;
        /// For the first page of the section, when it has a title page.
        vec first: Block => with_first;
        /// For even pages, when the document has headers and footers for them.
        vec even: Block => with_even;
    }
}

ir_struct! {
    /// How a section's pages are numbered.
    #[derive(Clone, Debug, PartialEq)]
    pub struct PageNumbering {
        /// How page numbers are written. Decimal when a document does not say
        /// (MS-OI29500 2.1.219 c).
        ref format: NumberFormat = NumberFormat::Decimal => with_format;
        /// The number of the section's first page; `None` continues from the previous
        /// section.
        copy start: Option<u32> = None => with_start;
        /// The heading level whose number is put before page numbers, as in `2-5`. Not
        /// laid out yet.
        copy chapter_style: Option<u8> = None => with_chapter_style;
    }
}

/// How numbers are written: of pages, notes and list items.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NumberFormat {
    /// 1, 2, 3.
    #[default]
    Decimal,
    /// 01, 02, 03: at least two digits.
    DecimalZero,
    /// a, b, c, …, z, aa, bb.
    LowerLetter,
    /// A, B, C, …, Z, AA, BB.
    UpperLetter,
    /// i, ii, iii.
    LowerRoman,
    /// I, II, III.
    UpperRoman,
    /// 1st, 2nd, 3rd.
    Ordinal,
    /// *, †, ‡, §, then the same doubled.
    Chicago,
    /// A bullet: the template of the list level, without a number.
    Bullet,
    /// No number.
    None,
    /// Any other format, by its name in WordprocessingML, such as `hebrew1`. Laid out as
    /// decimal and reported.
    Other(String),
}

/// Where a section's body text sits between the top and bottom margins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PageVerticalAlign {
    /// At the top.
    #[default]
    Top,
    /// Centred.
    Center,
    /// Spread so the first line is at the top and the last at the bottom.
    Both,
    /// At the bottom.
    Bottom,
}

no_rules!(Orientation, SectionStart, NumberFormat, PageVerticalAlign);
