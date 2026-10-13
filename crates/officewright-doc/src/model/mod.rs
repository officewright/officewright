//! The document model: what a document contains, independent of the file format it was
//! read from.

pub(crate) mod properties;
pub(crate) mod section;
pub(crate) mod styles;
pub(crate) mod table_properties;
mod text;
mod views;
pub(crate) mod xml;

use std::sync::atomic::{AtomicU64, Ordering};

use officewright_core::Pt;

pub use properties::{
    ArtBorder, Border, BorderStyle, Color, ColorValue, FontHint, Highlight, Indentation,
    Justification, Language, LineSpacing, NumberFormat, NumberingReference, ParagraphBorders,
    ParagraphProperties, RunFonts, RunProperties, Shading, ShadingPattern, Spacing, TabKind,
    TabLeader, TabStop, ThemeColor, ThemeFont, Underline, UnderlineStyle, VerticalPosition,
};
pub use section::{
    ChapterSeparator, Column, Columns, DocumentGrid, DocumentGridKind, HeaderFooterKind,
    HeaderFooterReference, LineNumberRestart, LineNumbering, NotePosition, NoteProperties,
    NoteRestart, Orientation, PageBorderDisplay, PageBorderOffset, PageBorderZOrder, PageBorders,
    PageMargins, PageNumbering, SectionProperties, SectionStart,
};
pub use styles::{Style, StyleKind, Styles};
pub use table_properties::{
    CellBorders, CellMargins, CellMerge, HeightRule, HorizontalAlignment, PositionAnchor,
    RowHeight, TableBorders, TableCellProperties, TableJustification, TableLayout, TableLook,
    TableOverlap, TablePosition, TableProperties, TableRowProperties, TableWidth, TextDirection,
    VerticalAlignment, VerticalJustification,
};
pub use views::{
    Block, Break, BreakClear, BreakKind, ContentControl, FieldCharKind, Hyperlink, Inline, Opaque,
    Paragraph, Revision, RevisionKind, Run, RunContent, Section, SimpleField, Table, TableCell,
    TableRow,
};
use xml::{Element, Span, Strings, Token};

use crate::Diagnostic;

/// A Word document: its sections, and in them paragraphs, tables and what they contain.
///
/// A document is read with [`Document::open`], [`Document::from_bytes`] or
/// [`Document::from_reader`], or with other limits through
/// [`LoadOptions`](crate::LoadOptions). It is `Send` and `Sync`: one
/// document can be read from several threads at once.
///
/// ```
/// # fn main() -> officewright_doc::Result<()> {
/// use officewright_doc::{Block, Document};
///
/// let document = Document::open("tests/data/hello.docx")?;
/// for section in document.sections() {
///     for block in section.blocks() {
///         if let Block::Paragraph(paragraph) = block {
///             println!("{}", paragraph.text());
///         }
///     }
/// }
/// assert_eq!(document.text(), "Hello, world\n");
/// # Ok(())
/// # }
/// ```
#[doc(alias = "Doc")]
pub struct Document {
    id: DocumentId,
    pub(crate) nodes: Vec<Node>,
    /// The tokens of every element kept without being understood, in document order.
    pub(crate) tokens: Vec<Token>,
    pub(crate) strings: Strings,
    pub(crate) names: Names,
    pub(crate) styles: styles::StyleTable,
    pub(crate) sections: Vec<SectionData>,
    pub(crate) kind: DocumentKind,
    pub(crate) conformance: Conformance,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// Where the namespaces the model reads are in a document's strings: WordprocessingML,
/// Transitional and Strict (read alike), and Markup Compatibility.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Names {
    pub(crate) w: [Option<xml::Sym>; 2],
    /// The relationships namespace of attributes such as `r:id`.
    pub(crate) r: [Option<xml::Sym>; 2],
    pub(crate) mc: Option<xml::Sym>,
}

/// What a WordprocessingML package holds, from the content type of its main part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentKind {
    /// A document, `.docx`.
    Document,
    /// A document with macros, `.docm`. The macros are kept and never run.
    MacroEnabledDocument,
    /// A template, `.dotx`.
    Template,
    /// A template with macros, `.dotm`.
    MacroEnabledTemplate,
}

/// Which of the two conformance classes of ISO/IEC 29500 the document is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Conformance {
    /// Transitional, what Word writes by default.
    Transitional,
    /// Strict, with its own namespaces.
    Strict,
}

/// Identifies the document a node id belongs to, so that an id is never looked up in
/// another document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DocumentId(u64);

impl DocumentId {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Identifies a paragraph of a [`Document`], and only of that document: looking it up
/// in another one gives `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParagraphId {
    document: DocumentId,
    index: u32,
}

/// Identifies a table of a [`Document`], and only of that document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TableId {
    document: DocumentId,
    index: u32,
}

impl Document {
    pub(crate) fn new() -> Self {
        Self {
            id: DocumentId::next(),
            nodes: Vec::new(),
            tokens: Vec::new(),
            strings: Strings::default(),
            names: Names::default(),
            styles: styles::StyleTable::default(),
            sections: Vec::new(),
            kind: DocumentKind::Document,
            conformance: Conformance::Transitional,
            diagnostics: Vec::new(),
        }
    }

    /// Opens the document in a file with the default [`LoadOptions`](crate::LoadOptions).
    ///
    /// # Errors
    ///
    /// As [`LoadOptions::open`](crate::LoadOptions::open).
    pub fn open(path: impl AsRef<std::path::Path>) -> crate::Result<Self> {
        crate::LoadOptions::new().open(path)
    }

    /// Opens the document in `data` with the default
    /// [`LoadOptions`](crate::LoadOptions).
    ///
    /// # Errors
    ///
    /// As [`LoadOptions::open_bytes`](crate::LoadOptions::open_bytes).
    pub fn from_bytes(data: impl Into<Vec<u8>>) -> crate::Result<Self> {
        crate::LoadOptions::new().open_bytes(data)
    }

    /// Reads a document to its end and opens it with the default
    /// [`LoadOptions`](crate::LoadOptions).
    ///
    /// # Errors
    ///
    /// As [`LoadOptions::open_reader`](crate::LoadOptions::open_reader).
    pub fn from_reader(reader: impl std::io::Read) -> crate::Result<Self> {
        crate::LoadOptions::new().open_reader(reader)
    }

    /// The sections, in document order. Every document has at least one.
    pub fn sections(&self) -> impl ExactSizeIterator<Item = Section<'_>> {
        self.sections.iter().map(|data| Section::new(self, data))
    }

    /// The paragraph with this id, or `None` for an id of another document.
    pub fn paragraph(&self, id: ParagraphId) -> Option<Paragraph<'_>> {
        if id.document != self.id {
            return None;
        }
        Paragraph::at(self, id.index)
    }

    /// The table with this id, or `None` for an id of another document.
    pub fn table(&self, id: TableId) -> Option<Table<'_>> {
        if id.document != self.id {
            return None;
        }
        Table::at(self, id.index)
    }

    /// The text of the document body: each paragraph followed by a line feed, the cells
    /// of a table row separated by tabs, without deleted text and field codes. Text is
    /// given as written: white space at the ends of a text element is kept whether or
    /// not it says `xml:space="preserve"`. Text inside elements kept without being read
    /// (drawings and text boxes among them) is left out.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for section in &self.sections {
            text::blocks(self, &section.blocks, &mut out);
        }
        out
    }

    /// The styles of the document (`styles.xml`); none when it has no styles part.
    pub fn styles(&self) -> Styles<'_> {
        Styles::new(self)
    }

    /// What kind of package the document came from.
    pub fn kind(&self) -> DocumentKind {
        self.kind
    }

    /// The conformance class the document is written in.
    pub fn conformance(&self) -> Conformance {
        self.conformance
    }

    /// What was repaired or ignored while reading, as Word does when it opens the file.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub(crate) fn node(&self, index: u32) -> Option<&Node> {
        self.nodes.get(index as usize)
    }

    pub(crate) fn element(&self, span: Span) -> Element<'_> {
        span.of(&self.tokens, &self.strings)
    }

    fn paragraph_id(&self, index: u32) -> ParagraphId {
        ParagraphId {
            document: self.id,
            index,
        }
    }

    fn table_id(&self, index: u32) -> TableId {
        TableId {
            document: self.id,
            index,
        }
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("kind", &self.kind)
            .field("conformance", &self.conformance)
            .field("sections", &self.sections.len())
            .field("nodes", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

// Documents are read from shared references on purpose; a field that is not `Send` or
// `Sync` would silently take that away.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Document>();
};

/// A section as stored: its properties and its blocks.
#[derive(Debug, Default)]
pub(crate) struct SectionData {
    pub(crate) properties: SectionProperties,
    pub(crate) blocks: Vec<u32>,
}

/// One node of the document tree. Children are indices into [`Document::nodes`], so the
/// tree is dropped and walked without recursion.
#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) kind: Kind,
    /// Property elements (`w:pPr`, `w:rPr`, `w:tblPr`, `w:tcPr`, `w:sdtPr`, …) as read,
    /// until the properties are typed.
    pub(crate) properties: Vec<Span>,
    pub(crate) children: Vec<u32>,
}

#[derive(Debug)]
pub(crate) enum Kind {
    Paragraph,
    Table {
        columns: Vec<Option<Pt>>,
    },
    Row,
    Cell,
    ContentControl {
        level: ControlLevel,
    },
    Run {
        content: Vec<RunItem>,
    },
    Hyperlink(Box<HyperlinkData>),
    Revision(Box<RevisionData>),
    SimpleField {
        instruction: Box<str>,
    },
    /// A container the model sees through, such as `w:customXml` or `w:smartTag`: its
    /// children belong to its parent.
    Wrapper,
    Other(Span),
}

#[derive(Debug, Default)]
pub(crate) struct HyperlinkData {
    /// Interned: a target comes from the relationships part, and many links may name it.
    pub(crate) target: Option<xml::Sym>,
    pub(crate) anchor: Option<Box<str>>,
}

/// Where a content control sits, and so what it contains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControlLevel {
    Block,
    Inline,
    Row,
    Cell,
}

#[derive(Debug)]
pub(crate) struct RevisionData {
    pub(crate) kind: RevisionKind,
    pub(crate) author: Option<Box<str>>,
    pub(crate) date: Option<Box<str>>,
}

/// What a run contains, as stored.
#[derive(Debug)]
pub(crate) enum RunItem {
    Text(Box<str>),
    DeletedText(Box<str>),
    FieldCode(Box<str>),
    DeletedFieldCode(Box<str>),
    Tab,
    Break(Break),
    CarriageReturn,
    Symbol {
        font: Option<Box<str>>,
        char: char,
    },
    FieldChar {
        kind: FieldCharKind,
        /// The element kept whole, as it may hold form field data (`w:ffData`).
        #[allow(dead_code)] // reason: kept for the DOCX writer, which writes it back
        element: Span,
    },
    NoBreakHyphen,
    SoftHyphen,
    FootnoteReference(i64),
    EndnoteReference(i64),
    CommentReference(i64),
    LastRenderedPageBreak,
    Other(Span),
}
