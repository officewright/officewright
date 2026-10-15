use std::{error, fmt};

use crate::report::Location;

/// An error from laying out a document.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The layout IR breaks one of its rules: a length that is not a finite number, an id
    /// from another document, text with a control character. A correct producer never
    /// builds such a document, so this is a bug in the producer, not damage in a file.
    #[non_exhaustive]
    InvalidDocument {
        /// Where in the document.
        location: Location,
        /// What is wrong.
        problem: String,
    },
    /// Tables are nested deeper than the layout follows, which keeps the stack its walks
    /// over a document take bounded.
    #[non_exhaustive]
    NestingTooDeep {
        /// Where the table that is too deep starts.
        location: Location,
        /// The deepest nesting the layout follows: a table in a table is at depth 2.
        limit: usize,
    },
    /// The document takes more pages than the layout was allowed to make.
    #[non_exhaustive]
    TooManyPages {
        /// The most pages allowed.
        limit: usize,
    },
    /// `officewright-pdf` refused what the painter gave it, which `source` says. The
    /// painter keeps every page within what a PDF holds, so this is a defect of the
    /// layout, worth reporting with the document.
    #[cfg(feature = "pdf")]
    #[non_exhaustive]
    Pdf {
        /// What went wrong.
        source: officewright_pdf::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocument { location, problem } => write!(f, "{location}: {problem}"),
            Self::NestingTooDeep { location, limit } => {
                write!(f, "{location}: tables are nested more than {limit} deep")
            }
            Self::TooManyPages { limit } => {
                write!(f, "the document takes more pages than the limit of {limit}")
            }
            #[cfg(feature = "pdf")]
            Self::Pdf { .. } => write!(f, "the PDF could not be made"),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            #[cfg(feature = "pdf")]
            Self::Pdf { source } => Some(source),
            _ => None,
        }
    }
}

impl From<Error> for officewright_core::Error {
    fn from(err: Error) -> Self {
        match err {
            // The IR is an argument the caller built, not input read from a file.
            invalid @ Error::InvalidDocument { .. } => Self::InvalidArgument(invalid.to_string()),
            deep @ Error::NestingTooDeep { .. } => Self::LimitExceeded(deep.to_string()),
            long @ Error::TooManyPages { .. } => Self::LimitExceeded(long.to_string()),
            #[cfg(feature = "pdf")]
            Error::Pdf { source } => source.into(),
        }
    }
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
