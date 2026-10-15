//! Reads TrueType and OpenType fonts, subsets them for embedding and finds the font a
//! document asks for.
//!
//! - [`FontFace`] is one face of a font file (`.ttf`, `.otf`) or font collection (`.ttc`,
//!   `.otc`) with TrueType or CFF outlines: metrics, character map, names and embedding
//!   permissions.
//! - [`FontFace::subset`] keeps only the glyphs a document uses, for embedding in a PDF.
//! - [`FontBook`] indexes the fonts a document embeds, fonts the caller adds and the
//!   fonts installed on the system. [`FontResolver`] finds the face for each family a
//!   document asks for, the way Word names fonts; when the family is not installed it
//!   takes a metric-compatible substitute (Carlito for Calibri, Liberation for Arial,
//!   Times New Roman and Courier New) or a fallback, preferring one of similar design
//!   (Caladea for Cambria), and reports every substitution so users know why the
//!   output differs.
//!
//! All metrics are in font design units; [`FontFace::units_per_em`] of them make one em.
#![forbid(unsafe_code)]

mod book;
mod cff;
mod coverage;
mod error;
mod face;
mod font;
mod index;
mod name_offsets;
mod resolve;
mod subset;
mod system;
#[cfg(test)]
mod testing;

pub use book::{FaceId, FaceInfo, FontBook, FontOrigin, SkippedFont};
pub use error::{Cause, Error, Result};
pub use face::{EmbeddingLicense, EmbeddingPermissions, FontNames, OutlineFormat};
pub use font::{
    BoundingBox, CharMap, DecorationMetrics, FontFace, GlyphId, LineMetrics, VerticalMetrics,
    WinMetrics,
};
pub use resolve::{FontRequest, FontResolver, GenericFamily, Resolution, ResolutionKind};
pub use subset::{Subset, SubsetBudget};
pub use system::system_font_dirs;
