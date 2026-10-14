//! DOC, DOCX and RTF documents.
//!
//! The first reader is for DOCX: [`Document::open`] reads a WordprocessingML package
//! (`.docx`, `.docm`, `.dotx`, `.dotm`, Transitional or Strict) into a [`Document`] of
//! [`Section`]s, each with its [`SectionProperties`] and its [`Block`]s: paragraphs,
//! tables and content controls, and in paragraphs runs of text.
//!
//! ```
//! # fn main() -> officewright_doc::Result<()> {
//! use officewright_doc::{Block, Document};
//!
//! let document = Document::open("tests/data/hello.docx")?;
//! let section = document.sections().next().expect("a document has a section");
//! let size = section.properties().page_size().expect("the page size is given");
//! assert_eq!((size.width().0, size.height().0), (595.3, 841.9));
//! for block in section.blocks() {
//!     if let Block::Paragraph(paragraph) = block {
//!         assert_eq!(paragraph.text(), "Hello, world");
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! What the model does not represent yet (equations, bookmarks, and the properties not
//! typed yet) is kept as it was read; such content shows as `Other` with an [`Opaque`]
//! view. Drawings show as [`RunContent::Drawing`]: their frame and picture are read, and
//! [`Drawing::to_xml`] gives a shape, chart or group as written.
//!
//! Opening never panics on a damaged file: it either returns an [`Error`], or opens the
//! document and lists in [`Document::diagnostics`] what it repaired, as Word does.
//! [`Limits`] bound the memory and time one file may take.
#![forbid(unsafe_code)]

mod diagnostic;
mod docx;
mod error;
#[cfg(feature = "pdf")]
mod layout;
mod model;
mod options;
mod units;

pub use diagnostic::{Diagnostic, DiagnosticKind};
pub use error::{Error, Limit, Result};
#[cfg(feature = "pdf")]
pub use layout::Translation;
pub use model::{
    Anchor, AnchorHorizontalAlign, AnchorVerticalAlign, ArtBorder, Block, Border, BorderStyle,
    Break, BreakClear, BreakKind, CellBorders, CellMargins, CellMerge, ChapterSeparator,
    CharacterFormat, Color, ColorScheme, ColorSchemeIndex, ColorSchemeMapping, ColorValue, Column,
    Columns, CompatibilityMode, CompatibilityOption, CompatibilitySetting, Conformance,
    ContentControl, Crop, Document, DocumentGrid, DocumentGridKind, DocumentKind, Drawing,
    EmbeddedFontReference, EmbeddedFontStyle, FieldCharKind, FontCollection, FontDeclaration,
    FontFamilies, FontFamily, FontHint, FontPitch, FontScheme, FontSignature, Formats,
    HeaderFooter, HeaderFooterKind, HeaderFooterReference, HeightRule, Highlight,
    HorizontalAlignment, HorizontalReference, Hyperlink, Image, Indentation, Inline, Justification,
    Language, LegacyNumbering, LevelOverride, LevelSuffix, Ligatures, LineNumberRestart,
    LineNumbering, LineSpacing, ListDefinition, ListFormat, ListInstance, ListLevel,
    MultiLevelType, Note, NotePosition, NoteProperties, NoteRestart, NoteType, NumberFormat,
    Numbering, NumberingReference, Offsets, Opaque, Orientation, PageBorderDisplay,
    PageBorderOffset, PageBorderZOrder, PageBorders, PageMargins, PageNumbering, Paragraph,
    ParagraphBorders, ParagraphFormat, ParagraphId, ParagraphProperties, Picture, PositionAnchor,
    Revision, RevisionKind, RowHeight, Run, RunContent, RunFonts, RunProperties, SchemeColor,
    Section, SectionProperties, SectionStart, Settings, Shading, ShadingPattern, SimpleField,
    Spacing, Style, StyleKind, Styles, SystemColor, TabKind, TabLeader, TabStop, Table,
    TableBorders, TableCell, TableCellProperties, TableId, TableJustification, TableLayout,
    TableLook, TableOverlap, TablePosition, TableProperties, TableRow, TableRowProperties,
    TableWidth, TextDirection, Theme, ThemeColor, ThemeFont, Underline, UnderlineStyle,
    VerticalAlignment, VerticalJustification, VerticalPosition, VerticalReference, WrapKind,
};
/// What [`Document::save_pdf`] and the like report, and the options they take.
#[cfg(feature = "pdf")]
pub use officewright_layout::{LayoutReport, Location, RenderOptions, ReportEntry, ReportKind};
/// How the streams of a PDF are compressed, for [`RenderOptions::with_compression`].
#[cfg(feature = "pdf")]
pub use officewright_pdf::Compression;
pub use options::{Limits, LoadOptions};

#[cfg(test)]
mod tests;
