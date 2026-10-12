//! The public view of the document tree: borrowed, copyable handles onto the nodes of a
//! [`Document`].

use super::xml::Span;
use super::{Document, Kind, Node, ParagraphId, RunItem, SectionData, SectionProperties, text};

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

    /// The paragraphs, tables and other blocks of the section, in document order.
    pub fn blocks(self) -> impl Iterator<Item = Block<'a>> + 'a {
        blocks(self.document, &self.data.blocks)
    }
}

/// Something that stacks vertically in a section.
///
/// More kinds of block may be added; an element read as [`Block::Other`] today may get
/// a variant of its own in a minor version.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Block<'a> {
    /// A paragraph.
    Paragraph(Paragraph<'a>),
    /// An element kept without being understood, such as a table or a bookmark.
    Other(Opaque<'a>),
}

/// A paragraph: runs of text and the content kept around them.
#[derive(Clone, Copy, Debug)]
pub struct Paragraph<'a> {
    document: &'a Document,
    index: u32,
    node: &'a Node,
}

impl<'a> Paragraph<'a> {
    pub(super) fn at(document: &'a Document, index: u32) -> Option<Self> {
        let node = document.node(index)?;
        matches!(node.kind, Kind::Paragraph).then_some(Self {
            document,
            index,
            node,
        })
    }

    /// The paragraph's id, to find it again with [`Document::paragraph`].
    pub fn id(self) -> ParagraphId {
        self.document.paragraph_id(self.index)
    }

    /// The runs and other inline content, in document order.
    pub fn inlines(self) -> impl Iterator<Item = Inline<'a>> + 'a {
        inlines(self.document, &self.node.children)
    }

    /// The runs of the paragraph, in document order.
    pub fn runs(self) -> impl Iterator<Item = Run<'a>> + 'a {
        self.inlines().filter_map(|inline| match inline {
            Inline::Run(run) => Some(run),
            _ => None,
        })
    }

    /// The paragraph's text, as [`Document::text`] gives it, without the line feed.
    pub fn text(self) -> String {
        let mut out = String::new();
        text::inlines(self.document, &self.node.children, &mut out);
        out
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
    /// An element kept without being understood, such as a hyperlink, a bookmark or an
    /// equation.
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
    /// Where Word's last layout broke the page; a hint, not a break.
    LastRenderedPageBreak,
    /// An element kept without being understood, such as a drawing.
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
    children.iter().filter_map(move |&index| {
        let node = document.node(index)?;
        Some(match &node.kind {
            Kind::Paragraph => Block::Paragraph(Paragraph {
                document,
                index,
                node,
            }),
            Kind::Other(span) => Block::Other(Opaque::new(document, *span)),
            Kind::Run { .. } => return None,
        })
    })
}

fn inlines<'a>(
    document: &'a Document,
    children: &'a [u32],
) -> impl Iterator<Item = Inline<'a>> + 'a {
    children.iter().filter_map(move |&index| {
        let node = document.node(index)?;
        Some(match &node.kind {
            Kind::Run { .. } => Inline::Run(Run { document, node }),
            Kind::Other(span) => Inline::Other(Opaque::new(document, *span)),
            Kind::Paragraph => return None,
        })
    })
}
