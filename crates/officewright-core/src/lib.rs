//! Units, geometry and error types shared by the officewright crates.
#![forbid(unsafe_code)]

use std::{error, fmt, io};

/// A length in PostScript points (1/72 inch), the unit of all geometry in officewright.
///
/// Twips, EMU and half-points are converted to `Pt` when a document is read and back
/// when it is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Pt(pub f64);

/// The width and height of a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    width: Pt,
    height: Pt,
}

impl PageSize {
    /// ISO 216 A4, 210 × 297 mm.
    pub const A4: Self = Self::new(Pt(210.0 * 72.0 / 25.4), Pt(297.0 * 72.0 / 25.4));

    /// US Letter, 8.5 × 11 in.
    pub const LETTER: Self = Self::new(Pt(612.0), Pt(792.0));

    /// Creates a page size. Each format crate checks the range it can write.
    pub const fn new(width: Pt, height: Pt) -> Self {
        Self { width, height }
    }

    /// The page width.
    pub const fn width(self) -> Pt {
        self.width
    }

    /// The page height.
    pub const fn height(self) -> Pt {
        self.height
    }
}

/// The error type every officewright format crate converts its errors into.
///
/// The variants are categories that bindings can map to their own error types; the
/// message names the offending object.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Reading or writing failed.
    Io {
        /// What was being read or written.
        message: String,
        /// The underlying error.
        source: io::Error,
    },
    /// An argument is out of range or inconsistent with the document.
    InvalidArgument(String),
    /// The input is valid but uses something officewright cannot handle yet.
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { message, .. }
            | Self::InvalidArgument(message)
            | Self::Unsupported(message) => f.write_str(message),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidArgument(_) | Self::Unsupported(_) => None,
        }
    }
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_is_210_by_297_mm() {
        let mm = |pt: Pt| pt.0 * 25.4 / 72.0;
        assert!((mm(PageSize::A4.width()) - 210.0).abs() < 1e-9);
        assert!((mm(PageSize::A4.height()) - 297.0).abs() < 1e-9);
    }

    #[test]
    fn io_error_keeps_its_source() {
        let err = Error::Io {
            message: "cannot write out.pdf".into(),
            source: io::Error::other("disk full"),
        };
        assert_eq!(err.to_string(), "cannot write out.pdf");
        assert_eq!(
            error::Error::source(&err).map(ToString::to_string),
            Some("disk full".into())
        );
    }
}
