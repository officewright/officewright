use officewright_core::{PageSize, Pt};

use super::properties::{Border, NumberFormat, xml_enum};
use super::table_properties::{TextDirection, VerticalJustification};

/// The properties of a section as written in the document (`w:sectPr`, ECMA-376 Part 1,
/// §17.6): page setup, how the section starts, its headers and footers, columns,
/// numbering of pages and lines, page borders, notes and the document grid. A value the
/// document does not give is `None`.
#[doc(alias = "PageSetup")]
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SectionProperties {
    pub(crate) page_width: Option<Pt>,
    pub(crate) page_height: Option<Pt>,
    pub(crate) orientation: Option<Orientation>,
    pub(crate) paper_code: Option<i64>,
    pub(crate) margins: PageMargins,
    pub(crate) start: Option<SectionStart>,
    pub(crate) headers: Vec<HeaderFooterReference>,
    pub(crate) footers: Vec<HeaderFooterReference>,
    pub(crate) footnotes: Option<NoteProperties>,
    pub(crate) endnotes: Option<NoteProperties>,
    pub(crate) page_borders: Option<PageBorders>,
    pub(crate) line_numbering: Option<LineNumbering>,
    pub(crate) page_numbering: Option<PageNumbering>,
    pub(crate) columns: Option<Columns>,
    pub(crate) form_protection: Option<bool>,
    pub(crate) vertical_justification: Option<VerticalJustification>,
    pub(crate) no_endnote: Option<bool>,
    pub(crate) title_page: Option<bool>,
    pub(crate) text_direction: Option<TextDirection>,
    pub(crate) right_to_left: Option<bool>,
    pub(crate) right_to_left_gutter: Option<bool>,
    pub(crate) grid: Option<DocumentGrid>,
}

impl SectionProperties {
    /// The page size, when the document gives both width and height.
    pub fn page_size(&self) -> Option<PageSize> {
        Some(PageSize::new(self.page_width?, self.page_height?))
    }

    /// The page width as written.
    pub fn page_width(&self) -> Option<Pt> {
        self.page_width
    }

    /// The page height as written.
    pub fn page_height(&self) -> Option<Pt> {
        self.page_height
    }

    /// The orientation as written (`w:orient`).
    pub fn orientation(&self) -> Option<Orientation> {
        self.orientation
    }

    /// The page margins as written.
    pub fn margins(&self) -> &PageMargins {
        &self.margins
    }

    /// How the section starts relative to the one before (`w:type`).
    pub fn start(&self) -> Option<SectionStart> {
        self.start
    }

    /// The printer's code for the paper size (`w:code` of `w:pgSz`).
    pub fn paper_code(&self) -> Option<i64> {
        self.paper_code
    }

    /// The headers the section names (`w:headerReference`), in document order.
    pub fn headers(&self) -> &[HeaderFooterReference] {
        &self.headers
    }

    /// The footers the section names (`w:footerReference`), in document order.
    pub fn footers(&self) -> &[HeaderFooterReference] {
        &self.footers
    }

    /// Where footnotes go and how they are numbered (`w:footnotePr`).
    pub fn footnotes(&self) -> Option<&NoteProperties> {
        self.footnotes.as_ref()
    }

    /// Where endnotes go and how they are numbered (`w:endnotePr`).
    pub fn endnotes(&self) -> Option<&NoteProperties> {
        self.endnotes.as_ref()
    }

    /// The borders around the pages (`w:pgBorders`).
    pub fn page_borders(&self) -> Option<&PageBorders> {
        self.page_borders.as_ref()
    }

    /// How lines are numbered in the margin (`w:lnNumType`).
    pub fn line_numbering(&self) -> Option<&LineNumbering> {
        self.line_numbering.as_ref()
    }

    /// How pages are numbered (`w:pgNumType`).
    pub fn page_numbering(&self) -> Option<&PageNumbering> {
        self.page_numbering.as_ref()
    }

    /// The text columns (`w:cols`).
    pub fn columns(&self) -> Option<&Columns> {
        self.columns.as_ref()
    }

    /// Only the form fields of the section can be edited (`w:formProt`).
    pub fn form_protection(&self) -> Option<bool> {
        self.form_protection
    }

    /// How the text is placed between the top and bottom margins (`w:vAlign`).
    pub fn vertical_justification(&self) -> Option<VerticalJustification> {
        self.vertical_justification
    }

    /// Endnotes are not placed at the end of this section, but carried to the next
    /// (`w:noEndnote`).
    pub fn no_endnote(&self) -> Option<bool> {
        self.no_endnote
    }

    /// The first page has its own header and footer (`w:titlePg`).
    pub fn title_page(&self) -> Option<bool> {
        self.title_page
    }

    /// The direction of the text (`w:textDirection`).
    pub fn text_direction(&self) -> Option<TextDirection> {
        self.text_direction
    }

    /// The section runs from right to left: columns, and the page's start side
    /// (`w:bidi`).
    pub fn right_to_left(&self) -> Option<bool> {
        self.right_to_left
    }

    /// The gutter is on the right side of the page (`w:rtlGutter`).
    pub fn right_to_left_gutter(&self) -> Option<bool> {
        self.right_to_left_gutter
    }

    /// The grid that lines and East Asian characters snap to (`w:docGrid`).
    pub fn grid(&self) -> Option<&DocumentGrid> {
        self.grid.as_ref()
    }
}

xml_enum! {
    /// Which pages of a section a header or footer is for (ST_HdrFtr).
    HeaderFooterKind {
        /// The pages that have no other.
        Default = "default",
        /// Even pages, when the document sets different odd and even headers.
        Even = "even",
        /// The first page, when the section has a title page.
        First = "first",
    }
}

/// A header or footer a section names (`w:headerReference`, `w:footerReference`), as
/// written: the relationship of the main part that leads to its part.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct HeaderFooterReference {
    pub(crate) kind: Option<HeaderFooterKind>,
    pub(crate) relationship_id: Box<str>,
}

impl HeaderFooterReference {
    /// Which pages it is for (`w:type`).
    pub fn kind(&self) -> Option<HeaderFooterKind> {
        self.kind
    }

    /// The id of the relationship to its part (`r:id`).
    pub fn relationship_id(&self) -> &str {
        &self.relationship_id
    }
}

xml_enum! {
    /// Where notes are placed (ST_FtnPos; endnotes take the last two values only).
    NotePosition {
        /// At the bottom of the page (footnotes only).
        PageBottom = "pageBottom",
        /// Below the text of the page (footnotes only).
        BeneathText = "beneathText",
        /// At the end of the section.
        SectionEnd = "sectEnd",
        /// At the end of the document.
        DocumentEnd = "docEnd",
    }
}

xml_enum! {
    /// When note numbers start again (ST_RestartNumber).
    NoteRestart {
        /// Never.
        Continuous = "continuous",
        /// In each section.
        EachSection = "eachSect",
        /// On each page.
        EachPage = "eachPage",
    }
}

/// Where the footnotes or endnotes of a section go and how they are numbered
/// (`w:footnotePr`, `w:endnotePr`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NoteProperties {
    pub(crate) position: Option<NotePosition>,
    pub(crate) number_format: Option<NumberFormat>,
    pub(crate) start: Option<i64>,
    pub(crate) restart: Option<NoteRestart>,
}

impl NoteProperties {
    /// Where the notes are placed (`w:pos`).
    pub fn position(&self) -> Option<NotePosition> {
        self.position
    }

    /// How their numbers are written (`w:numFmt`).
    pub fn number_format(&self) -> Option<NumberFormat> {
        self.number_format
    }

    /// The first number (`w:numStart`).
    pub fn start(&self) -> Option<i64> {
        self.start
    }

    /// When numbering starts again (`w:numRestart`).
    pub fn restart(&self) -> Option<NoteRestart> {
        self.restart
    }
}

xml_enum! {
    /// Whether page borders are drawn in front of or behind the text (ST_PageBorderZOrder).
    PageBorderZOrder {
        Front = "front",
        Back = "back",
    }
}

xml_enum! {
    /// On which pages of a section the page borders are drawn (ST_PageBorderDisplay).
    PageBorderDisplay {
        AllPages = "allPages",
        FirstPage = "firstPage",
        NotFirstPage = "notFirstPage",
    }
}

xml_enum! {
    /// What the distances of page borders are measured from (ST_PageBorderOffset).
    PageBorderOffset {
        /// The edge of the page.
        Page = "page",
        /// The text.
        Text = "text",
    }
}

/// The borders around the pages of a section (`w:pgBorders`). The schema's defaults,
/// when nothing is written: in front of the text, measured from the text.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct PageBorders {
    pub(crate) top: Option<Border>,
    pub(crate) left: Option<Border>,
    pub(crate) bottom: Option<Border>,
    pub(crate) right: Option<Border>,
    pub(crate) z_order: Option<PageBorderZOrder>,
    pub(crate) display: Option<PageBorderDisplay>,
    pub(crate) offset_from: Option<PageBorderOffset>,
}

impl PageBorders {
    /// At the top.
    pub fn top(&self) -> Option<&Border> {
        self.top.as_ref()
    }

    /// On the left.
    pub fn left(&self) -> Option<&Border> {
        self.left.as_ref()
    }

    /// At the bottom.
    pub fn bottom(&self) -> Option<&Border> {
        self.bottom.as_ref()
    }

    /// On the right.
    pub fn right(&self) -> Option<&Border> {
        self.right.as_ref()
    }

    /// In front of or behind the text (`w:zOrder`).
    pub fn z_order(&self) -> Option<PageBorderZOrder> {
        self.z_order
    }

    /// On which pages (`w:display`).
    pub fn display(&self) -> Option<PageBorderDisplay> {
        self.display
    }

    /// What the distances are measured from (`w:offsetFrom`).
    pub fn offset_from(&self) -> Option<PageBorderOffset> {
        self.offset_from
    }
}

xml_enum! {
    /// When line numbers start again (ST_LineNumberRestart).
    LineNumberRestart {
        NewPage = "newPage",
        NewSection = "newSection",
        Continuous = "continuous",
    }
}

/// How the lines of a section are numbered in the margin (`w:lnNumType`). The schema's
/// defaults, when nothing is written: start at 1, again on each page.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct LineNumbering {
    pub(crate) count_by: Option<i64>,
    pub(crate) start: Option<i64>,
    pub(crate) distance: Option<Pt>,
    pub(crate) restart: Option<LineNumberRestart>,
}

impl LineNumbering {
    /// Every how many lines a number is shown (`w:countBy`).
    pub fn count_by(&self) -> Option<i64> {
        self.count_by
    }

    /// The first number (`w:start`).
    pub fn start(&self) -> Option<i64> {
        self.start
    }

    /// The distance between the numbers and the text (`w:distance`).
    pub fn distance(&self) -> Option<Pt> {
        self.distance
    }

    /// When numbering starts again (`w:restart`).
    pub fn restart(&self) -> Option<LineNumberRestart> {
        self.restart
    }
}

xml_enum! {
    /// What separates a chapter number from a page number (ST_ChapterSep).
    ChapterSeparator {
        Hyphen = "hyphen",
        Period = "period",
        Colon = "colon",
        EmDash = "emDash",
        EnDash = "enDash",
    }
}

/// How the pages of a section are numbered (`w:pgNumType`). The schema's defaults, when
/// nothing is written: decimal numbers, a hyphen after a chapter number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PageNumbering {
    pub(crate) format: Option<NumberFormat>,
    pub(crate) start: Option<i64>,
    pub(crate) chapter_style: Option<i64>,
    pub(crate) chapter_separator: Option<ChapterSeparator>,
}

impl PageNumbering {
    /// How page numbers are written (`w:fmt`).
    pub fn format(&self) -> Option<NumberFormat> {
        self.format
    }

    /// The number of the section's first page (`w:start`); without it numbering goes on
    /// from the section before.
    pub fn start(&self) -> Option<i64> {
        self.start
    }

    /// The heading level whose number is the chapter number shown with page numbers
    /// (`w:chapStyle`).
    pub fn chapter_style(&self) -> Option<i64> {
        self.chapter_style
    }

    /// What separates the chapter number from the page number (`w:chapSep`).
    pub fn chapter_separator(&self) -> Option<ChapterSeparator> {
        self.chapter_separator
    }
}

/// The text columns of a section (`w:cols`). The schema's defaults, when nothing is
/// written: one column, 36 pt apart.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Columns {
    pub(crate) count: Option<i64>,
    pub(crate) space: Option<Pt>,
    pub(crate) equal_width: Option<bool>,
    pub(crate) separator: Option<bool>,
    pub(crate) columns: Vec<Column>,
}

impl Columns {
    /// How many columns (`w:num`).
    pub fn count(&self) -> Option<i64> {
        self.count
    }

    /// The space between columns of equal width (`w:space`).
    pub fn space(&self) -> Option<Pt> {
        self.space
    }

    /// The columns are equally wide (`w:equalWidth`).
    pub fn equal_width(&self) -> Option<bool> {
        self.equal_width
    }

    /// A line is drawn between columns (`w:sep`).
    pub fn separator(&self) -> Option<bool> {
        self.separator
    }

    /// Each column as written (`w:col`), for columns that are not equally wide.
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }
}

/// One text column (`w:col`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Column {
    pub(crate) width: Option<Pt>,
    pub(crate) space: Option<Pt>,
}

impl Column {
    /// The width (`w:w`).
    pub fn width(&self) -> Option<Pt> {
        self.width
    }

    /// The space after the column (`w:space`).
    pub fn space(&self) -> Option<Pt> {
        self.space
    }
}

xml_enum! {
    /// What the document grid applies to (ST_DocGrid).
    DocumentGridKind {
        /// No grid.
        Default = "default",
        /// Lines snap to the grid.
        Lines = "lines",
        /// Lines and characters snap to the grid.
        LinesAndChars = "linesAndChars",
        /// Characters snap to the grid, lines do not.
        SnapToChars = "snapToChars",
    }
}

/// The grid that lines and East Asian characters of a section snap to (`w:docGrid`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct DocumentGrid {
    pub(crate) kind: Option<DocumentGridKind>,
    pub(crate) line_pitch: Option<Pt>,
    pub(crate) character_space: Option<i64>,
}

impl DocumentGrid {
    /// What snaps to the grid (`w:type`).
    pub fn kind(&self) -> Option<DocumentGridKind> {
        self.kind
    }

    /// The distance between lines of the grid (`w:linePitch`, written in twentieths of a
    /// point).
    pub fn line_pitch(&self) -> Option<Pt> {
        self.line_pitch
    }

    /// The extra space added to the width of characters on the grid, as written
    /// (`w:charSpace`).
    pub fn character_space(&self) -> Option<i64> {
        self.character_space
    }
}

/// The page margins of a section (`w:pgMar`), as written.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct PageMargins {
    pub(crate) top: Option<Pt>,
    pub(crate) bottom: Option<Pt>,
    pub(crate) left: Option<Pt>,
    pub(crate) right: Option<Pt>,
    pub(crate) header: Option<Pt>,
    pub(crate) footer: Option<Pt>,
    pub(crate) gutter: Option<Pt>,
}

impl PageMargins {
    /// From the top edge of the page to the text. A negative value means the text
    /// starts there even when the header is taller.
    pub fn top(&self) -> Option<Pt> {
        self.top
    }

    /// From the bottom edge of the page to the text. A negative value means the text
    /// ends there even when the footer is taller.
    pub fn bottom(&self) -> Option<Pt> {
        self.bottom
    }

    /// From the left edge of the page to the text.
    pub fn left(&self) -> Option<Pt> {
        self.left
    }

    /// From the right edge of the page to the text.
    pub fn right(&self) -> Option<Pt> {
        self.right
    }

    /// From the top edge of the page to the header.
    pub fn header(&self) -> Option<Pt> {
        self.header
    }

    /// From the bottom edge of the page to the footer.
    pub fn footer(&self) -> Option<Pt> {
        self.footer
    }

    /// Extra space for binding, added to the left margin (or the top one, depending on
    /// the document settings).
    pub fn gutter(&self) -> Option<Pt> {
        self.gutter
    }
}

/// The orientation of the pages of a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Orientation {
    /// Taller than wide.
    Portrait,
    /// Wider than tall.
    Landscape,
}

/// How a section starts relative to the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SectionStart {
    /// On a new page.
    NextPage,
    /// On the same page.
    Continuous,
    /// On the next even page.
    EvenPage,
    /// On the next odd page.
    OddPage,
    /// In the next column.
    NextColumn,
}
