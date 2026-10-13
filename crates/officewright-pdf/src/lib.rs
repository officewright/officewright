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
//! offset, rather than altered. Page content and embedded fonts are compressed
//! ([`Compression`]).
//!
//! Text that a layout engine has shaped is drawn as a [`GlyphRun`]: glyphs of an embedded
//! font at explicit positions, with the text they stand for, so ligatures, kerning and
//! marks come out as shaped and text extraction still returns the text. Pages also take
//! filled and stroked [`GraphicsPath`]s, with dashed [`Stroke`]s, and clipping paths.
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
mod glyphs;
mod helvetica;
mod path;
mod script;
mod stream;
mod text;

pub use document::{Document, Page};
pub use error::{Error, Result};
pub use font::{EmbeddingNote, Font, StandardFont};
pub use glyphs::{GlyphRun, PlacedGlyph};
pub use officewright_core::{Color, PageSize, Pt};
pub use officewright_font::GlyphId;
pub use path::{GraphicsPath, LineCap, LineJoin, Stroke};
pub use stream::Compression;
pub use text::text_to_pdf;
