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
//! What the model does not represent yet (drawings, equations, bookmarks, and the
//! properties not typed yet) is kept as it was read; such content shows as `Other` with
//! an [`Opaque`] view.
//!
//! Opening never panics on a damaged file: it either returns an [`Error`], or opens the
//! document and lists in [`Document::diagnostics`] what it repaired, as Word does.
//! [`Limits`] bound the memory and time one file may take.
#![forbid(unsafe_code)]

mod diagnostic;
mod docx;
mod error;
mod model;
mod options;
mod units;

pub use diagnostic::{Diagnostic, DiagnosticKind};
pub use error::{Error, Limit, Result};
pub use model::{
    ArtBorder, Block, Border, BorderStyle, Break, BreakClear, BreakKind, CellBorders, CellMargins,
    CellMerge, ChapterSeparator, Color, ColorSchemeIndex, ColorSchemeMapping, ColorValue, Column,
    Columns, CompatibilityMode, CompatibilityOption, CompatibilitySetting, Conformance,
    ContentControl, Document, DocumentGrid, DocumentGridKind, DocumentKind, FieldCharKind,
    FontHint, HeaderFooterKind, HeaderFooterReference, HeightRule, Highlight, HorizontalAlignment,
    Hyperlink, Indentation, Inline, Justification, Language, LineNumberRestart, LineNumbering,
    LineSpacing, NotePosition, NoteProperties, NoteRestart, NumberFormat, NumberingReference,
    Opaque, Orientation, PageBorderDisplay, PageBorderOffset, PageBorderZOrder, PageBorders,
    PageMargins, PageNumbering, Paragraph, ParagraphBorders, ParagraphId, ParagraphProperties,
    PositionAnchor, Revision, RevisionKind, RowHeight, Run, RunContent, RunFonts, RunProperties,
    Section, SectionProperties, SectionStart, Settings, Shading, ShadingPattern, SimpleField,
    Spacing, Style, StyleKind, Styles, TabKind, TabLeader, TabStop, Table, TableBorders, TableCell,
    TableCellProperties, TableId, TableJustification, TableLayout, TableLook, TableOverlap,
    TablePosition, TableProperties, TableRow, TableRowProperties, TableWidth, TextDirection,
    ThemeColor, ThemeFont, Underline, UnderlineStyle, VerticalAlignment, VerticalJustification,
    VerticalPosition,
};
pub use options::{Limits, LoadOptions};

#[cfg(test)]
mod tests;
