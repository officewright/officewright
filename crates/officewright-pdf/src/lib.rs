//! Writes PDF files.
//!
//! A [`Document`] is built from pages that show text in a [`Font`]; [`text_to_pdf`] lays
//! out plain text on pages.
//!
//! - The standard Helvetica font is provided by every PDF viewer, so nothing is
//!   embedded, but it is used with WinAnsiEncoding and shows only the Latin characters
//!   that encoding covers.
//! - A TrueType or OpenType font ([`Font::from_file`]) is embedded, reduced to the
//!   glyphs the document uses, with a ToUnicode map so that its text can be copied and
//!   searched. It shows Latin, Greek, Cyrillic and the other scripts that need no
//!   shaping, one glyph per character.
//!
//! Text a font cannot show is rejected with an error that names the character and its
//! offset, rather than altered.
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
mod embed;
mod encoding;
mod error;
mod font;
mod helvetica;
mod script;
mod text;

pub use document::{Document, Page};
pub use error::{Error, Result};
pub use font::{EmbeddingNote, Font, StandardFont};
pub use officewright_core::{PageSize, Pt};
pub use text::text_to_pdf;
