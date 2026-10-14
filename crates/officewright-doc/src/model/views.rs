//! The public view of the document tree: borrowed, copyable handles onto the nodes of a
//! [`Document`].

use officewright_core::Pt;

use super::drawing::Drawing;
use super::xml::Element;
use super::xml::Span;
use super::{
    ControlLevel, Document, HeaderFooterKind, HyperlinkData, Kind, Node, NoteType, ParagraphId,
    ParagraphProperties, RevisionData, RunItem, RunProperties, SectionData, SectionProperties,
    StoryData, StoryKind, TableCellProperties, TableId, TableProperties, TableRowProperties, text,
};

/// A section of a document: its page setup and the blocks laid out with it.
#[derive(Clone, Copy, Debug)]
pub struct Section<'a> {
    document: &'a Document,
    data: &'a SectionData,
}

impl<'a> Section<'a> {
    pub(super) fn new(document: &'a Document, data: &'a SectionData) -> Self {
        Self { document, data }
    }

    /// The section's properties as written in the document.
    pub fn properties(self) -> &'a SectionProperties {
        &self.data.properties
    }

    /// The paragraphs, tables and other blocks of the section, in document order. A
    /// content control that a section break falls inside is split between the two
    /// sections (see [`ContentControl`]).
    pub fn blocks(self) -> impl Iterator<Item = Block<'a>> + 'a {
        blocks(self.document, &self.data.blocks)
    }

    /// The header of this kind in effect for the section: its own, or the one the
    /// closest earlier section names, as each kind is inherited on its own (ECMA-376
    /// Part 1, §17.10.5, as Word reads it: MS-OI29500 §17.10.5(b)). Which kind a page shows depends on the title page and on the
    /// document's different odd and even headers.
    pub fn header(self, kind: HeaderFooterKind) -> Option<HeaderFooter<'a>> {
        HeaderFooter::at(self.document, *self.data.headers.get(kind_index(kind))?)
    }

    /// The footer of this kind in effect for the section; see [`header`](Self::header).
    pub fn footer(self, kind: HeaderFooterKind) -> Option<HeaderFooter<'a>> {
        HeaderFooter::at(self.document, *self.data.footers.get(kind_index(kind))?)
    }
}

/// The place of a kind of header or footer in a section's lists of them.
pub(crate) fn kind_index(kind: HeaderFooterKind) -> usize {
    match kind {
        HeaderFooterKind::Default => 0,
        HeaderFooterKind::First => 1,
        HeaderFooterKind::Even => 2,
    }
}

/// A header or a footer: a story of its own part, which sections name. Two are equal
/// when they are the same story of the same document, as for sections that inherit one.
#[derive(Clone, Copy, Debug)]
pub struct HeaderFooter<'a> {
    document: &'a Document,
    data: &'a StoryData,
    index: u32,
}

impl PartialEq for HeaderFooter<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.document, other.document) && self.index == other.index
    }
}

impl Eq for HeaderFooter<'_> {}

impl std::hash::Hash for HeaderFooter<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(self.document, state);
        self.index.hash(state);
    }
}

impl<'a> HeaderFooter<'a> {
    fn at(document: &'a Document, index: Option<u32>) -> Option<Self> {
        let index = index?;
        let data = document.stories.get(usize::try_from(index).ok()?)?;
        matches!(data.kind, StoryKind::Header | StoryKind::Footer).then_some(Self {
            document,
            data,
            index,
        })
    }

    /// Where it is among the document's stories: the same for every section that has
    /// it.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn index(self) -> u32 {
        self.index
    }

    /// Whether it is a header, not a footer.
    pub fn is_header(self) -> bool {
        matches!(self.data.kind, StoryKind::Header)
    }

    /// Its paragraphs, tables and other blocks.
    pub fn blocks(self) -> impl Iterator<Item = Block<'a>> + 'a {
        blocks(self.document, &self.data.blocks)
    }
}

/// A footnote or an endnote: a story of the notes part, which runs refer to by its id.
#[derive(Clone, Copy, Debug)]
pub struct Note<'a> {
    document: &'a Document,
    data: &'a StoryData,
}

impl<'a> Note<'a> {
    pub(crate) fn at(document: &'a Document, index: u32) -> Option<Self> {
        let data = document.stories.get(usize::try_from(index).ok()?)?;
        matches!(data.kind, StoryKind::Note { .. }).then_some(Self { document, data })
    }

    /// Its id (`w:id`), which [`RunContent::FootnoteReference`] and
    /// [`RunContent::EndnoteReference`] name.
    pub fn id(self) -> i64 {
        match self.data.kind {
            StoryKind::Note { id, .. } => id,
            _ => 0,
        }
    }

    /// Whether it is an endnote, not a footnote.
    pub fn is_endnote(self) -> bool {
        matches!(self.data.kind, StoryKind::Note { endnote: true, .. })
    }

    /// What it is (`w:type`): a note the text refers to, or a separator.
    pub fn note_type(self) -> NoteType {
        match self.data.kind {
            StoryKind::Note { note_type, .. } => note_type,
            _ => NoteType::Normal,
        }
    }

    /// Its paragraphs, tables and other blocks.
    pub fn blocks(self) -> impl Iterator<Item = Block<'a>> + 'a {
        blocks(self.document, &self.data.blocks)
    }
}

/// Something that stacks vertically in a section, a table cell or a content control.
///
/// More kinds of block may be added; an element read as [`Block::Other`] today may get
/// a variant of its own in a minor version.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Block<'a> {
    /// A paragraph.
    Paragraph(Paragraph<'a>),
    /// A table.
    Table(Table<'a>),
    /// A content control (`w:sdt`) around blocks.
    ContentControl(ContentControl<'a>),
    /// An element kept without being understood, such as a bookmark.
    Other(Opaque<'a>),
}

/// A paragraph: runs of text and what surrounds them, such as hyperlinks.
#[derive(Clone, Copy, Debug)]
pub struct Paragraph<'a> {
    document: &'a Document,
    index: u32,
    node: &'a Node,
}

impl<'a> Paragraph<'a> {
    pub(super) fn at(document: &'a Document, index: u32) -> Option<Self> {
        let node = document.node(index)?;
        matches!(node.kind, Kind::Paragraph { .. }).then_some(Self {
            document,
            index,
            node,
        })
    }

    pub(super) fn node(self) -> &'a Node {
        self.node
    }

    pub(super) fn document(self) -> &'a Document {
        self.document
    }

    /// The paragraph's formatting in effect, with its style, its list level and the
    /// document defaults; see [`Document::formats`], which is faster for many
    /// paragraphs.
    pub fn format(self) -> super::ParagraphFormat {
        super::Formats::new(self.document).paragraph(self)
    }

    /// The paragraph's id, to find it again with [`Document::paragraph`].
    pub fn id(self) -> ParagraphId {
        self.document.paragraph_id(self.index)
    }

    /// The runs, hyperlinks and other inline content, in document order.
    pub fn inlines(self) -> impl Iterator<Item = Inline<'a>> + 'a {
        inlines(self.document, &self.node.children)
    }

    /// Every run of the paragraph in document order, including the runs inside
    /// hyperlinks, content controls, fields and tracked changes.
    pub fn runs(self) -> impl Iterator<Item = Run<'a>> + 'a {
        let document = self.document;
        descendants(document, &self.node.children).filter_map(move |index| {
            let node = document.node(index)?;
            matches!(node.kind, Kind::Run { .. }).then_some(Run { document, node })
        })
    }

    /// The paragraph properties written on the paragraph (its `w:pPr`), with those of
    /// its mark, without those of its style or the document defaults.
    pub fn properties(self) -> ParagraphProperties {
        let document = self.document;
        self.node
            .properties
            .iter()
            .map(|&span| document.element(span))
            .find(|element| element.is(&document.names.w, "pPr"))
            .map_or_else(ParagraphProperties::default, |p_pr| {
                crate::docx::properties::paragraph_properties(
                    p_pr,
                    &document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The paragraph's text, as [`Document::text`] gives it, without the line feed.
    pub fn text(self) -> String {
        let mut out = String::new();
        text::inlines(self.document, &self.node.children, &mut out);
        out
    }
}

/// A table: rows of cells laid out on a grid of columns.
#[derive(Clone, Copy, Debug)]
pub struct Table<'a> {
    document: &'a Document,
    index: u32,
    node: &'a Node,
}

impl<'a> Table<'a> {
    pub(super) fn at(document: &'a Document, index: u32) -> Option<Self> {
        let node = document.node(index)?;
        matches!(node.kind, Kind::Table { .. }).then_some(Self {
            document,
            index,
            node,
        })
    }

    /// The table's id, to find it again with [`Document::table`].
    pub fn id(self) -> TableId {
        self.document.table_id(self.index)
    }

    /// The widths of the grid columns as written (`w:tblGrid`); `None` for a column
    /// without a valid width.
    pub fn grid_columns(self) -> impl ExactSizeIterator<Item = Option<Pt>> + 'a {
        let columns: &'a [Option<Pt>] = match &self.node.kind {
            Kind::Table { columns } => columns,
            _ => &[],
        };
        columns.iter().copied()
    }

    /// The table properties written on the table (its `w:tblPr`), without those of its
    /// style.
    pub fn properties(self) -> TableProperties {
        property_element(self.document, self.node, "tblPr").map_or_else(
            TableProperties::default,
            |tbl_pr| {
                crate::docx::table_properties::table_properties(
                    tbl_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            },
        )
    }

    /// The rows, in document order, including rows inside content controls around them.
    pub fn rows(self) -> impl Iterator<Item = TableRow<'a>> + 'a {
        let document = self.document;
        through_containers(document, &self.node.children).filter_map(move |index| {
            let node = document.node(index)?;
            matches!(node.kind, Kind::Row).then_some(TableRow { document, node })
        })
    }
}

/// A row of a [`Table`].
#[derive(Clone, Copy, Debug)]
pub struct TableRow<'a> {
    document: &'a Document,
    node: &'a Node,
}

impl<'a> TableRow<'a> {
    /// The row properties written on the row (its `w:trPr`).
    pub fn properties(self) -> TableRowProperties {
        property_element(self.document, self.node, "trPr").map_or_else(
            TableRowProperties::default,
            |tr_pr| {
                crate::docx::table_properties::row_properties(
                    tr_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            },
        )
    }

    /// The table properties the row sets for itself, in place of the table's (its
    /// `w:tblPrEx`): the standard notes they are written mostly for legacy documents
    /// and where two tables were joined (§17.4.61).
    pub fn table_property_exceptions(self) -> TableProperties {
        property_element(self.document, self.node, "tblPrEx").map_or_else(
            TableProperties::default,
            |tbl_pr_ex| {
                crate::docx::table_properties::table_properties(
                    tbl_pr_ex,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            },
        )
    }

    /// The cells, in document order, including cells inside content controls around
    /// them.
    pub fn cells(self) -> impl Iterator<Item = TableCell<'a>> + 'a {
        let document = self.document;
        through_containers(document, &self.node.children).filter_map(move |index| {
            let node = document.node(index)?;
            matches!(node.kind, Kind::Cell).then_some(TableCell { document, node })
        })
    }
}

/// A cell of a [`TableRow`].
#[derive(Clone, Copy, Debug)]
pub struct TableCell<'a> {
    document: &'a Document,
    node: &'a Node,
}

impl<'a> TableCell<'a> {
    /// The cell properties written on the cell (its `w:tcPr`).
    pub fn properties(self) -> TableCellProperties {
        property_element(self.document, self.node, "tcPr").map_or_else(
            TableCellProperties::default,
            |tc_pr| {
                crate::docx::table_properties::cell_properties(
                    tc_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            },
        )
    }

    /// The paragraphs, tables and other blocks in the cell.
    pub fn blocks(self) -> impl Iterator<Item = Block<'a>> + 'a {
        blocks(self.document, &self.node.children)
    }
}

/// A content control (`w:sdt`): a region of a document that a form or a template
/// marks, around blocks or around inline content.
///
/// A section break inside a content control around blocks ends the section after its
/// paragraph, as in Word, so such a control shows in both sections: once with the
/// blocks before the break and once, with the same properties, with the rest.
#[derive(Clone, Copy, Debug)]
pub struct ContentControl<'a> {
    document: &'a Document,
    node: &'a Node,
}

impl<'a> ContentControl<'a> {
    /// The blocks inside a content control that is a [`Block`]; nothing for one that
    /// is an [`Inline`].
    pub fn blocks(self) -> impl Iterator<Item = Block<'a>> + 'a {
        let is_block = self.level() == Some(ControlLevel::Block);
        blocks(self.document, &self.node.children).filter(move |_| is_block)
    }

    /// The inline content inside a content control that is an [`Inline`]; nothing for
    /// one that is a [`Block`].
    pub fn inlines(self) -> impl Iterator<Item = Inline<'a>> + 'a {
        let is_inline = self.level() == Some(ControlLevel::Inline);
        inlines(self.document, &self.node.children).filter(move |_| is_inline)
    }

    fn level(self) -> Option<ControlLevel> {
        match self.node.kind {
            Kind::ContentControl { level } => Some(level),
            _ => None,
        }
    }
}

/// Content inside a paragraph.
///
/// More kinds may be added; an element read as [`Inline::Other`] today may get a variant
/// of its own in a minor version.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Inline<'a> {
    /// A run of text with one set of character properties.
    Run(Run<'a>),
    /// A hyperlink around inline content.
    Hyperlink(Hyperlink<'a>),
    /// A content control around inline content.
    ContentControl(ContentControl<'a>),
    /// A tracked insertion, deletion or move around inline content.
    Revision(Revision<'a>),
    /// A simple field (`w:fldSimple`): an instruction and its last result.
    SimpleField(SimpleField<'a>),
    /// An element kept without being understood, such as a bookmark or an equation.
    Other(Opaque<'a>),
}

/// A run: content with one set of character properties.
#[doc(alias = "TextRange")]
#[derive(Clone, Copy, Debug)]
pub struct Run<'a> {
    document: &'a Document,
    node: &'a Node,
}

impl<'a> Run<'a> {
    pub(super) fn node(self) -> &'a Node {
        self.node
    }

    pub(super) fn document(self) -> &'a Document {
        self.document
    }

    /// The run's formatting in effect, with its styles and the document defaults; see
    /// [`Document::formats`], which is faster for many runs.
    pub fn format(self) -> super::CharacterFormat {
        super::Formats::new(self.document).run(self)
    }

    /// What the run contains, in document order.
    pub fn content(self) -> impl Iterator<Item = RunContent<'a>> + 'a {
        let document = self.document;
        let items: &'a [RunItem] = match &self.node.kind {
            Kind::Run { content } => content,
            _ => &[],
        };
        items
            .iter()
            .map(move |item| RunContent::new(document, item))
    }

    /// The character properties written on the run (its `w:rPr`), without those of its
    /// styles or the document defaults.
    pub fn properties(self) -> RunProperties {
        let document = self.document;
        self.node
            .properties
            .iter()
            .map(|&span| document.element(span))
            .find(|element| element.is(&document.names.w, "rPr"))
            .map_or_else(RunProperties::default, |r_pr| {
                crate::docx::properties::run_properties(r_pr, &document.names, &mut |_, _, _| {})
            })
    }

    /// The run's text: its text with tabs and line breaks, without deleted text and
    /// field codes.
    pub fn text(self) -> String {
        let mut out = String::new();
        if let Kind::Run { content } = &self.node.kind {
            text::run(content, &mut out);
        }
        out
    }
}

/// One item of a [`Run`].
///
/// More kinds may be added; an element read as [`RunContent::Other`] today may get a
/// variant of its own in a minor version.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum RunContent<'a> {
    /// Text (`w:t`), as written: white space at its ends is kept whether or not the
    /// element says `xml:space="preserve"`.
    Text(&'a str),
    /// Text deleted with change tracking on (`w:delText`).
    DeletedText(&'a str),
    /// The instruction of a complex field (`w:instrText`).
    FieldCode(&'a str),
    /// A deleted field instruction (`w:delInstrText`).
    DeletedFieldCode(&'a str),
    /// A tab character.
    Tab,
    /// A line, page or column break.
    Break(Break),
    /// A carriage return, a line break that never clears floating objects.
    CarriageReturn,
    /// A character from a symbol font (`w:sym`), such as a Wingdings bullet.
    #[non_exhaustive]
    Symbol {
        /// The font, as written.
        font: Option<&'a str>,
        /// The character code, as written: for symbol fonts usually in U+F000–U+F0FF.
        char: char,
    },
    /// Where a complex field begins, where its instruction ends and its result begins,
    /// or where it ends.
    FieldChar(FieldCharKind),
    /// A hyphen that does not allow a line break.
    NoBreakHyphen,
    /// An optional hyphen, shown only where the line breaks.
    SoftHyphen,
    /// The mark of the footnote with this id.
    FootnoteReference(i64),
    /// The mark of the endnote with this id.
    EndnoteReference(i64),
    /// The anchor of the comment with this id.
    CommentReference(i64),
    /// The number of the note this content is in (`w:footnoteRef`, `w:endnoteRef`).
    NoteNumber,
    /// The short line a separator note draws above the notes of a page
    /// (`w:separator`).
    Separator,
    /// The full-width line above notes continued from the previous page
    /// (`w:continuationSeparator`).
    ContinuationSeparator,
    /// A drawing (`w:drawing`): a picture, in the line or floating, or a shape, chart or
    /// other graphic, which shows as no picture.
    Drawing(Drawing<'a>),
    /// Where Word's last layout broke the page; a hint, not a break.
    LastRenderedPageBreak,
    /// An element kept without being understood, such as a VML picture (`w:pict`) or an
    /// embedded object.
    Other(Opaque<'a>),
}

impl<'a> RunContent<'a> {
    fn new(document: &'a Document, item: &'a RunItem) -> Self {
        match item {
            RunItem::Text(text) => Self::Text(text),
            RunItem::DeletedText(text) => Self::DeletedText(text),
            RunItem::FieldCode(text) => Self::FieldCode(text),
            RunItem::DeletedFieldCode(text) => Self::DeletedFieldCode(text),
            RunItem::Tab => Self::Tab,
            RunItem::Break(b) => Self::Break(*b),
            RunItem::CarriageReturn => Self::CarriageReturn,
            RunItem::Symbol { font, char } => Self::Symbol {
                font: font.as_deref(),
                char: *char,
            },
            RunItem::FieldChar { kind, .. } => Self::FieldChar(*kind),
            RunItem::NoBreakHyphen => Self::NoBreakHyphen,
            RunItem::SoftHyphen => Self::SoftHyphen,
            RunItem::FootnoteReference(id) => Self::FootnoteReference(*id),
            RunItem::EndnoteReference(id) => Self::EndnoteReference(*id),
            RunItem::CommentReference(id) => Self::CommentReference(*id),
            RunItem::NoteNumber => Self::NoteNumber,
            RunItem::Separator => Self::Separator,
            RunItem::ContinuationSeparator => Self::ContinuationSeparator,
            RunItem::Drawing { element, image } => {
                Self::Drawing(Drawing::new(document, *element, *image))
            }
            RunItem::LastRenderedPageBreak => Self::LastRenderedPageBreak,
            RunItem::Other(span) => Self::Other(Opaque::new(document, *span)),
        }
    }
}

/// A break in a run (`w:br`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Break {
    kind: BreakKind,
    clear: BreakClear,
}

impl Break {
    pub(crate) fn new(kind: BreakKind, clear: BreakClear) -> Self {
        Self { kind, clear }
    }

    /// What the break ends.
    pub fn kind(self) -> BreakKind {
        self.kind
    }

    /// For a line break, past which floating objects the next line starts.
    pub fn clear(self) -> BreakClear {
        self.clear
    }
}

/// What a [`Break`] ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BreakKind {
    /// The line (`textWrapping`, the default).
    Line,
    /// The page.
    Page,
    /// The column.
    Column,
}

/// Past which floating objects the line after a line break starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BreakClear {
    /// None: the next line starts right below (the default).
    None,
    /// Objects on the left.
    Left,
    /// Objects on the right.
    Right,
    /// All objects.
    All,
}

/// Which mark of a complex field a [`RunContent::FieldChar`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FieldCharKind {
    /// The field begins; its instruction follows.
    Begin,
    /// The instruction ends; the field's last result follows.
    Separate,
    /// The field ends.
    End,
}

/// A hyperlink around inline content.
#[derive(Clone, Copy, Debug)]
pub struct Hyperlink<'a> {
    document: &'a Document,
    node: &'a Node,
    data: &'a HyperlinkData,
}

impl<'a> Hyperlink<'a> {
    /// Where the hyperlink leads outside the document, such as a web address, as
    /// written in the relationship it names. officewright never follows it.
    pub fn target(self) -> Option<&'a str> {
        self.data
            .target
            .map(|target| self.document.strings.get(target))
    }

    /// The bookmark the hyperlink leads to inside the document.
    pub fn anchor(self) -> Option<&'a str> {
        self.data.anchor.as_deref()
    }

    /// The inline content the hyperlink spans.
    pub fn inlines(self) -> impl Iterator<Item = Inline<'a>> + 'a {
        inlines(self.document, &self.node.children)
    }
}

/// A tracked change around inline content.
#[derive(Clone, Copy, Debug)]
pub struct Revision<'a> {
    document: &'a Document,
    node: &'a Node,
    data: &'a RevisionData,
}

impl<'a> Revision<'a> {
    /// What the change did.
    pub fn kind(self) -> RevisionKind {
        self.data.kind
    }

    /// Who made it, as written.
    pub fn author(self) -> Option<&'a str> {
        self.data.author.as_deref()
    }

    /// When, as written (an ISO 8601 date and time).
    pub fn date(self) -> Option<&'a str> {
        self.data.date.as_deref()
    }

    /// The inline content the change spans.
    pub fn inlines(self) -> impl Iterator<Item = Inline<'a>> + 'a {
        inlines(self.document, &self.node.children)
    }
}

/// What a [`Revision`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RevisionKind {
    /// Inserted content (`w:ins`).
    Insertion,
    /// Deleted content (`w:del`).
    Deletion,
    /// Content moved away from here (`w:moveFrom`).
    MoveFrom,
    /// Content moved here (`w:moveTo`).
    MoveTo,
}

/// A simple field (`w:fldSimple`).
#[derive(Clone, Copy, Debug)]
pub struct SimpleField<'a> {
    document: &'a Document,
    node: &'a Node,
    instruction: &'a str,
}

impl<'a> SimpleField<'a> {
    /// The field instruction, such as `PAGE` or `DATE \@ "d MMMM yyyy"`.
    pub fn instruction(self) -> &'a str {
        self.instruction
    }

    /// The field's last result, as Word saved it.
    pub fn inlines(self) -> impl Iterator<Item = Inline<'a>> + 'a {
        inlines(self.document, &self.node.children)
    }
}

/// An element kept as it was read, because officewright does not represent it (yet).
#[derive(Clone, Copy, Debug)]
pub struct Opaque<'a> {
    document: &'a Document,
    span: Span,
}

impl<'a> Opaque<'a> {
    pub(super) fn new(document: &'a Document, span: Span) -> Self {
        Self { document, span }
    }

    /// The element's namespace URI; empty for an element in no namespace.
    pub fn namespace(self) -> &'a str {
        self.document.element(self.span).namespace()
    }

    /// The element's local name, such as `bookmarkStart`.
    pub fn local_name(self) -> &'a str {
        self.document.element(self.span).local_name()
    }

    /// The element and everything in it as XML, with the namespace declarations it
    /// needs.
    pub fn to_xml(self) -> String {
        self.document.element(self.span).to_xml()
    }
}

fn blocks<'a>(document: &'a Document, children: &'a [u32]) -> impl Iterator<Item = Block<'a>> + 'a {
    through_wrappers(document, children).filter_map(move |index| {
        let node = document.node(index)?;
        Some(match &node.kind {
            Kind::Paragraph { .. } => Block::Paragraph(Paragraph {
                document,
                index,
                node,
            }),
            Kind::Table { .. } => Block::Table(Table {
                document,
                index,
                node,
            }),
            Kind::ContentControl { .. } => Block::ContentControl(ContentControl { document, node }),
            Kind::Other(span) => Block::Other(Opaque::new(document, *span)),
            _ => return None,
        })
    })
}

fn inlines<'a>(
    document: &'a Document,
    children: &'a [u32],
) -> impl Iterator<Item = Inline<'a>> + 'a {
    through_wrappers(document, children).filter_map(move |index| {
        let node = document.node(index)?;
        Some(match &node.kind {
            Kind::Run { .. } => Inline::Run(Run { document, node }),
            Kind::Hyperlink(data) => Inline::Hyperlink(Hyperlink {
                document,
                node,
                data,
            }),
            Kind::ContentControl { .. } => {
                Inline::ContentControl(ContentControl { document, node })
            }
            Kind::Revision(data) => Inline::Revision(Revision {
                document,
                node,
                data,
            }),
            Kind::SimpleField { instruction } => Inline::SimpleField(SimpleField {
                document,
                node,
                instruction,
            }),
            Kind::Other(span) => Inline::Other(Opaque::new(document, *span)),
            _ => return None,
        })
    })
}

/// The nodes in `children`, with each wrapper replaced by its own children.
fn through_wrappers<'a>(
    document: &'a Document,
    children: &'a [u32],
) -> impl Iterator<Item = u32> + 'a {
    walk(document, children, |kind| matches!(kind, Kind::Wrapper))
}

/// The nodes in `children`, with each wrapper and content control replaced by its own
/// children: how a table sees rows and a row sees cells.
pub(super) fn through_containers<'a>(
    document: &'a Document,
    children: &'a [u32],
) -> impl Iterator<Item = u32> + 'a {
    walk(document, children, |kind| {
        matches!(kind, Kind::Wrapper | Kind::ContentControl { .. })
    })
}

/// Every node below `children`, depth first in document order.
fn descendants<'a>(document: &'a Document, children: &'a [u32]) -> impl Iterator<Item = u32> + 'a {
    let mut stack = vec![children.iter()];
    std::iter::from_fn(move || {
        loop {
            let next = stack.last_mut()?.next();
            match next {
                Some(&index) => {
                    if let Some(node) = document.node(index) {
                        stack.push(node.children.iter());
                    }
                    return Some(index);
                }
                None => {
                    stack.pop();
                }
            }
        }
    })
}

/// The nodes in `children`, descending without recursion into the nodes `open` selects
/// instead of yielding them.
fn walk<'a>(
    document: &'a Document,
    children: &'a [u32],
    open: impl Fn(&Kind) -> bool + 'a,
) -> impl Iterator<Item = u32> + 'a {
    let mut stack = vec![children.iter()];
    std::iter::from_fn(move || {
        loop {
            let next = stack.last_mut()?.next();
            match next {
                Some(&index) => match document.node(index) {
                    Some(node) if open(&node.kind) => stack.push(node.children.iter()),
                    Some(_) => return Some(index),
                    None => {}
                },
                None => {
                    stack.pop();
                }
            }
        }
    })
}

/// The property element of a node with this local name in the main namespace.
fn property_element<'a>(document: &'a Document, node: &Node, local: &str) -> Option<Element<'a>> {
    node.properties
        .iter()
        .map(|&span| document.element(span))
        .find(|element| element.is(&document.names.w, local))
}
