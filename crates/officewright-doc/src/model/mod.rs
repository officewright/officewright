//! The document model: what a document contains, independent of the file format it was
//! read from.

pub(crate) mod drawing;
pub(crate) mod fonts;
pub(crate) mod format;
pub(crate) mod numbering;
pub(crate) mod properties;
mod resolve;
pub(crate) mod section;
pub(crate) mod settings;
pub(crate) mod styles;
pub(crate) mod table_properties;
mod text;
pub(crate) mod theme;
mod views;
pub(crate) mod xml;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use officewright_core::Pt;

pub use drawing::{
    Anchor, AnchorHorizontalAlign, AnchorVerticalAlign, Crop, Drawing, HorizontalReference, Image,
    Offsets, Picture, VerticalReference, WrapKind,
};
pub use fonts::{
    EmbeddedFontReference, EmbeddedFontStyle, FontDeclaration, FontFamily, FontPitch, FontSignature,
};
pub use format::{CharacterFormat, FontFamilies, ListFormat, ParagraphFormat};
pub use numbering::{
    LegacyNumbering, LevelOverride, LevelSuffix, ListDefinition, ListInstance, ListLevel,
    MultiLevelType, Numbering,
};
pub use properties::{
    ArtBorder, Border, BorderStyle, Color, ColorValue, FontHint, Highlight, Indentation,
    Justification, Language, Ligatures, LineSpacing, NumberFormat, NumberingReference,
    ParagraphBorders, ParagraphProperties, RunFonts, RunProperties, Shading, ShadingPattern,
    Spacing, TabKind, TabLeader, TabStop, ThemeColor, ThemeFont, Underline, UnderlineStyle,
    VerticalPosition,
};
pub use resolve::Formats;
pub(crate) use resolve::assign_paragraph_styles;
pub use section::{
    ChapterSeparator, Column, Columns, DocumentGrid, DocumentGridKind, HeaderFooterKind,
    HeaderFooterReference, LineNumberRestart, LineNumbering, NotePosition, NoteProperties,
    NoteRestart, NoteType, Orientation, PageBorderDisplay, PageBorderOffset, PageBorderZOrder,
    PageBorders, PageMargins, PageNumbering, SectionProperties, SectionStart,
};
pub use settings::{
    ColorSchemeIndex, ColorSchemeMapping, CompatibilityMode, CompatibilityOption,
    CompatibilitySetting, Settings,
};
pub use styles::{ConditionalFormat, Style, StyleKind, Styles, TableStyleCondition};
pub use table_properties::{
    CellBorders, CellMargins, CellMerge, HeightRule, HorizontalAlignment, PositionAnchor,
    RowHeight, TableBorders, TableCellProperties, TableJustification, TableLayout, TableLook,
    TableOverlap, TablePosition, TableProperties, TableRowProperties, TableWidth, TextDirection,
    VerticalAlignment, VerticalJustification,
};
pub use theme::{ColorScheme, FontCollection, FontScheme, SchemeColor, SystemColor, Theme};
pub(crate) use views::kind_index;
pub use views::{
    Block, Break, BreakClear, BreakKind, ContentControl, FieldCharKind, HeaderFooter, Hyperlink,
    Inline, Note, Opaque, Paragraph, Revision, RevisionKind, Run, RunContent, Section, SimpleField,
    Table, TableCell, TableRow,
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
    pub(crate) settings: settings::Settings,
    pub(crate) numbering: numbering::NumberingTable,
    pub(crate) theme: Option<theme::Theme>,
    pub(crate) fonts: Vec<fonts::FontDeclaration>,
    /// Where the first declaration of each font name is, by the name in lower case.
    pub(crate) font_index: HashMap<String, u32>,
    pub(crate) sections: Vec<SectionData>,
    /// The elements of the main part's `w:document` other than `w:body`, such as
    /// `w:background` and unknown extensions, kept for writing, each with whether it comes
    /// after the body (#111).
    pub(crate) root_elements: Vec<(xml::Span, bool)>,
    /// The headers, footers, footnotes and endnotes.
    pub(crate) stories: Vec<StoryData>,
    /// The header or footer each relationship of the main part leads to, by its id.
    pub(crate) header_footers: HashMap<Box<str>, u32>,
    /// The footnotes and the endnotes by their ids; of two with one id, the first.
    pub(crate) footnotes: HashMap<i64, u32>,
    pub(crate) endnotes: HashMap<i64, u32>,
    /// The images pictures show.
    pub(crate) images: Vec<ImageData>,
    /// Each image part read so far, by its name: where its image is among `images`, or
    /// none when it could not be read, so that it is read and reported once.
    pub(crate) image_parts: HashMap<Box<str>, Option<u32>>,
    pub(crate) kind: DocumentKind,
    pub(crate) conformance: Conformance,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// The bytes of the XML parts read into the document's stories, a measure of its
    /// size.
    pub(crate) xml_size: usize,
}

/// Where the namespaces the model reads are in a document's strings: WordprocessingML,
/// Transitional and Strict (read alike), and Markup Compatibility.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Names {
    pub(crate) w: [Option<xml::Sym>; 2],
    /// The relationships namespace of attributes such as `r:id`.
    pub(crate) r: [Option<xml::Sym>; 2],
    /// The main namespace of DrawingML, of the theme.
    pub(crate) a: [Option<xml::Sym>; 2],
    /// DrawingML's positioning in WordprocessingML (`wp:inline`, `wp:anchor`).
    pub(crate) wp: [Option<xml::Sym>; 2],
    /// DrawingML pictures (`pic:pic`).
    pub(crate) pic: [Option<xml::Sym>; 2],
    pub(crate) mc: Option<xml::Sym>,
    /// Word 2010's extensions, such as `w14:ligatures`.
    pub(crate) w14: Option<xml::Sym>,
}

/// What a WordprocessingML package holds, from the content type of its main part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentKind {
    /// A document, `.docx`.
    Document,
    /// A document with macros, `.docm`. The macros (`vbaProject.bin` and the parts with
    /// it) are neither read nor run, and the [`Document`] does not keep them.
    MacroEnabledDocument,
    /// A template, `.dotx`.
    Template,
    /// A template with macros, `.dotm`; its macros are treated as a `.docm`'s.
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
            settings: settings::Settings::default(),
            numbering: numbering::NumberingTable::default(),
            theme: None,
            fonts: Vec::new(),
            font_index: HashMap::new(),
            sections: Vec::new(),
            root_elements: Vec::new(),
            stories: Vec::new(),
            header_footers: HashMap::new(),
            footnotes: HashMap::new(),
            endnotes: HashMap::new(),
            images: Vec::new(),
            image_parts: HashMap::new(),
            kind: DocumentKind::Document,
            conformance: Conformance::Transitional,
            diagnostics: Vec::new(),
            xml_size: 0,
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

    /// The footnote with this id (`w:footnoteReference` names it), the first of several.
    pub fn footnote(&self, id: i64) -> Option<Note<'_>> {
        Note::at(self, *self.footnotes.get(&id)?)
    }

    /// The endnote with this id (`w:endnoteReference` names it), the first of several.
    pub fn endnote(&self, id: i64) -> Option<Note<'_>> {
        Note::at(self, *self.endnotes.get(&id)?)
    }

    /// Every footnote and endnote, separators included, in the order of their parts:
    /// footnotes first.
    pub fn notes(&self) -> impl Iterator<Item = Note<'_>> {
        (0..self.stories.len()).filter_map(|index| Note::at(self, u32::try_from(index).ok()?))
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

    /// Resolves the formatting in effect of paragraphs, runs, paragraph marks and list
    /// labels: the document defaults, the styles, numbering and direct formatting
    /// combined as Word combines them, with theme fonts and colours resolved. Keep it for
    /// the whole document: it remembers what it has worked out about each style and
    /// list.
    ///
    /// ```
    /// # fn main() -> officewright_doc::Result<()> {
    /// use officewright_doc::{Block, Document};
    ///
    /// let document = Document::open("tests/data/theme.docx")?;
    /// let mut formats = document.formats();
    /// for section in document.sections() {
    ///     for block in section.blocks() {
    ///         if let Block::Paragraph(paragraph) = block {
    ///             let format = formats.paragraph(paragraph);
    ///             println!("indented {} pt", format.start_indent().0);
    ///             for run in paragraph.runs() {
    ///                 let format = formats.run(run);
    ///                 println!("{} {} pt", format.fonts().ascii(), format.size().0);
    ///             }
    ///         }
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn formats(&self) -> Formats<'_> {
        Formats::new(self)
    }

    /// The numbering of the document (`numbering.xml`): its list definitions and list
    /// instances; none when it has no numbering part.
    pub fn numbering(&self) -> Numbering<'_> {
        Numbering::new(self)
    }

    /// The theme of the document (its theme part), whose colours and fonts theme colours
    /// and theme fonts stand for; `None` when it has none.
    pub fn theme(&self) -> Option<&Theme> {
        self.theme.as_ref()
    }

    /// What the font table (`fontTable.xml`) says about the fonts the document uses, in
    /// document order.
    pub fn fonts(&self) -> &[FontDeclaration] {
        &self.fonts
    }

    /// The font table's declaration of a font, by the name runs give it, compared
    /// without regard to case as Word does (MS-OI29500 §17.8.3.10(b); LibreOffice's
    /// `tdf140137.docx` names `times new roman` against a declared `Times New Roman`):
    /// both names are folded by Unicode's lower-case mapping ([`char::to_lowercase`]), so
    /// Cyrillic and Greek names match in any case too.
    /// A table may declare one name several times, once per face of a family with its
    /// own PANOSE and signature (`fdo77727.docx` declares five Noto Sans); this gives
    /// the first, and [`fonts`](Self::fonts) all of them.
    pub fn font(&self, name: &str) -> Option<&FontDeclaration> {
        let index = self.font_index.get(&fold_case(name))?;
        self.fonts.get(usize::try_from(*index).ok()?)
    }

    /// The settings of the document (`settings.xml`); none set when it has no settings
    /// part.
    pub fn settings(&self) -> &Settings {
        &self.settings
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
    /// The stories of its headers and footers in effect, those it inherits from earlier
    /// sections included, by kind: default, first, even.
    pub(crate) headers: [Option<u32>; 3],
    pub(crate) footers: [Option<u32>; 3],
}

/// A story other than the body: its kind and its blocks.
#[derive(Debug)]
pub(crate) struct StoryData {
    pub(crate) kind: StoryKind,
    pub(crate) blocks: Vec<u32>,
}

/// What a story is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoryKind {
    Header,
    Footer,
    Note {
        endnote: bool,
        id: i64,
        note_type: NoteType,
    },
}

/// One node of the document tree. Children are indices into [`Document::nodes`], so the
/// tree is dropped and walked without recursion.
#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) kind: Kind,
    /// The node this one is a child of; [`Node::NO_PARENT`] for a block of a section.
    pub(crate) parent: u32,
    /// Property elements (`w:pPr`, `w:rPr`, `w:tblPr`, `w:tcPr`, `w:sdtPr`, …) as read,
    /// until the properties are typed.
    pub(crate) properties: Vec<Span>,
    pub(crate) children: Vec<u32>,
}

impl Node {
    /// The parent of a block that belongs to a section, not to another node.
    pub(crate) const NO_PARENT: u32 = u32::MAX;
}

#[derive(Debug)]
pub(crate) enum Kind {
    Paragraph {
        /// The index of the paragraph style the resolver uses for it, worked out once the
        /// styles are read: its `w:pStyle` when that names one of the paragraph styles the
        /// resolver uses; else none, and the paragraph has the default paragraph style.
        style: Option<u32>,
    },
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
    NoteNumber,
    Separator,
    ContinuationSeparator,
    /// A `w:drawing` kept whole, and the image its picture shows, read when the part
    /// that holds it was.
    Drawing {
        element: Span,
        image: Option<u32>,
    },
    LastRenderedPageBreak,
    Other(Span),
}

/// A font name in lower case, as [`Document::font`] compares names.
pub(crate) fn fold_case(name: &str) -> String {
    #[cfg(test)]
    FOLDED.set(FOLDED.get().saturating_add(name.len()));
    name.chars().flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
thread_local! {
    /// The bytes of font names folded to lower case on this thread, for tests of the cost
    /// of looking fonts up.
    pub(crate) static FOLDED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// An image a picture of the document shows: the bytes of its part and its content
/// type, read once however many pictures show it.
pub(crate) struct ImageData {
    pub(crate) content_type: Box<str>,
    pub(crate) bytes: std::sync::Arc<[u8]>,
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageData")
            .field("content_type", &self.content_type)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}
