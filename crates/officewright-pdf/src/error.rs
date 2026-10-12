use std::path::PathBuf;
use std::{error, fmt, io};

use officewright_core::Pt;

/// An error from writing a PDF.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The text has a character that WinAnsiEncoding cannot represent, so the standard
    /// Helvetica font cannot show it.
    Unencodable {
        /// The character.
        ch: char,
        /// Its byte offset in the text.
        offset: usize,
    },
    /// A font size is outside [`Font::MIN_SIZE`](crate::Font::MIN_SIZE) to
    /// [`Font::MAX_SIZE`](crate::Font::MAX_SIZE).
    InvalidFontSize(Pt),
    /// A text position is outside -32767 to 32767 pt.
    InvalidPosition {
        /// Horizontal position.
        x: Pt,
        /// Vertical position.
        y: Pt,
    },
    /// A page size is outside the 3 to 14400 pt range PDF allows (ISO 32000-1, Annex C).
    InvalidPageSize {
        /// Page width.
        width: Pt,
        /// Page height.
        height: Pt,
    },
    /// The page leaves no room for text inside the margins.
    NoRoomForText {
        /// Page width.
        width: Pt,
        /// Page height.
        height: Pt,
        /// Margin on each side.
        margin: Pt,
    },
    /// The document has no pages; a PDF needs at least one.
    NoPages,
    /// The PDF would be larger than a cross-reference table can address (10^10 bytes).
    TooLarge,
    /// Writing the PDF failed.
    Io(io::Error),
    /// Writing the PDF to a file failed.
    Write {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unencodable { ch, offset } => write!(
                f,
                "character U+{:04X} '{}' at byte {offset} is not in WinAnsiEncoding",
                u32::from(*ch),
                ch.escape_debug()
            ),
            Self::InvalidFontSize(size) => write!(
                f,
                "font size {} pt is outside the {} to {} pt range",
                size.0,
                crate::Font::MIN_SIZE.0,
                crate::Font::MAX_SIZE.0
            ),
            Self::InvalidPosition { x, y } => write!(
                f,
                "text position ({}, {}) pt is outside the -{max} to {max} pt range",
                x.0,
                y.0,
                max = crate::document::MAX_POSITION
            ),
            Self::InvalidPageSize { width, height } => write!(
                f,
                "page size {} x {} pt is outside the 3 to 14400 pt range PDF allows",
                width.0, height.0
            ),
            Self::NoRoomForText {
                width,
                height,
                margin,
            } => write!(
                f,
                "page size {} x {} pt leaves no room for text inside {} pt margins",
                width.0, height.0, margin.0
            ),
            Self::NoPages => f.write_str("document has no pages"),
            Self::TooLarge => {
                f.write_str("PDF exceeds the 10000000000 bytes a cross-reference table can address")
            }
            Self::Io(_) => f.write_str("cannot write the PDF"),
            Self::Write { path, .. } => write!(f, "cannot write {}", path.display()),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io(source) | Self::Write { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<Error> for officewright_core::Error {
    fn from(err: Error) -> Self {
        let message = err.to_string();
        match err {
            Error::Io(source) | Error::Write { source, .. } => Self::Io { message, source },
            Error::Unencodable { .. } | Error::TooLarge => Self::Unsupported(message),
            Error::InvalidFontSize(_)
            | Error::InvalidPosition { .. }
            | Error::InvalidPageSize { .. }
            | Error::NoRoomForText { .. }
            | Error::NoPages => Self::InvalidArgument(message),
        }
    }
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
