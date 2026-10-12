//! The building blocks every officewright OOXML reader shares: ZIP archives, XML,
//! the Open Packaging Conventions (ECMA-376 Part 2) and Markup Compatibility (ECMA-376
//! Part 3).
//!
//! The format crates build on this crate; most users never need it directly.
//!
//! Everything here reads untrusted input. Sizes, nesting and counts are bounded by
//! [`Limits`], and malformed input produces an [`Error`], never a panic. Nothing outside
//! the input is ever read: XML document type declarations, and with them external
//! entities, are rejected.
#![forbid(unsafe_code)]

mod error;
mod limits;
pub mod mce;
mod package;
#[cfg(test)]
mod testing;
pub mod xml;
pub mod zip;

pub use error::{Error, Limit, Result};
pub use limits::Limits;
pub use package::{Package, Part, PartName, Relationship, Relationships, TargetMode};
