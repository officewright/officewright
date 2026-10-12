//! Writes PDF files.
//!
//! A [`Document`] is built from pages that show text in a [`Font`]; [`text_to_pdf`] lays
//! out plain text on pages. For now the standard Helvetica font is the only one: every
//! PDF viewer provides it, so nothing is embedded, but it is used with WinAnsiEncoding
//! and can only show the Latin characters that encoding covers. Text with other
//! characters is rejected with [`Error::Unencodable`] rather than altered.
//!
//! ```
//! use officewright_pdf::{Document, Font, PageSize, Pt, StandardFont};
//!
//! let helvetica = Font::standard(StandardFont::Helvetica);
//! let mut doc = Document::new();
//! doc.add_page(PageSize::A4)?
//!     .add_text(Pt(72.0), Pt(770.0), &helvetica, Pt(12.0), "Hello, world")?;
//! let pdf = doc.to_bytes()?;
//! assert!(pdf.starts_with(b"%PDF-1.4"));
//! # Ok::<(), officewright_pdf::Error>(())
//! ```
#![forbid(unsafe_code)]

mod document;
mod encoding;
mod error;
mod font;
mod helvetica;
mod text;

pub use document::{Document, Page};
pub use error::{Error, Result};
pub use font::{Font, StandardFont};
pub use officewright_core::{PageSize, Pt};
pub use text::text_to_pdf;
