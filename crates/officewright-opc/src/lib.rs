//! The building blocks every officewright OOXML reader shares: so far, reading the ZIP
//! archives OOXML packages are stored in and the XML of their parts, and following
//! Markup Compatibility (ECMA-376 Part 3).
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
#[cfg(test)]
mod testing;
pub mod xml;
pub mod zip;

pub use error::{Error, Limit, Result};
pub use limits::Limits;
