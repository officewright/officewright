//! The building blocks every officewright OOXML reader shares: so far, reading the ZIP
//! archives OOXML packages are stored in.
//!
//! The format crates build on this crate; most users never need it directly.
//!
//! Everything here reads untrusted input. Sizes and counts are bounded by [`Limits`],
//! and malformed input produces an [`Error`], never a panic.
#![forbid(unsafe_code)]

mod error;
mod limits;
#[cfg(test)]
mod testing;
pub mod zip;

pub use error::{Error, Limit, Result};
pub use limits::Limits;
