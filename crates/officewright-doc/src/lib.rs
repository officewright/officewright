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
//! properties of paragraphs and runs) is kept as it was read; such content shows as
//! `Other` with an [`Opaque`] view.
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
    Block, Break, BreakClear, BreakKind, Color, ColorValue, Conformance, ContentControl, Document,
    DocumentKind, FieldCharKind, FontHint, Highlight, Hyperlink, Inline, Language, Opaque,
    Orientation, PageMargins, Paragraph, ParagraphId, Revision, RevisionKind, Run, RunContent,
    RunFonts, RunProperties, Section, SectionProperties, SectionStart, Shading, ShadingPattern,
    SimpleField, Style, StyleKind, Styles, Table, TableCell, TableId, TableRow, ThemeColor,
    ThemeFont, Underline, UnderlineStyle, VerticalPosition,
};
pub use options::{Limits, LoadOptions};

#[cfg(test)]
mod tests;
