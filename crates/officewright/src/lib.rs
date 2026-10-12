//! Read, write and convert office documents without Microsoft Office or LibreOffice.
//!
//! Each format lives in its own crate. This crate re-exports the ones enabled by the
//! Cargo features `doc`, `pdf`, `xls` and `slides`, all on by default.
#![forbid(unsafe_code)]

/// DOC, DOCX and RTF documents.
#[cfg(feature = "doc")]
pub use officewright_doc as doc;

/// PDF reading and writing.
#[cfg(feature = "pdf")]
pub use officewright_pdf as pdf;

/// Font loading, font files for embedding, and finding the fonts a document asks for.
#[cfg(feature = "pdf")]
pub use officewright_font as font;

/// PPT and PPTX presentations.
#[cfg(feature = "slides")]
pub use officewright_slides as slides;

/// XLS and XLSX workbooks.
#[cfg(feature = "xls")]
pub use officewright_xls as xls;
