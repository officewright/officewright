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
    /// Tables are nested deeper than the layout follows, which keeps the stack a
    /// document can take bounded.
    #[non_exhaustive]
    NestingTooDeep {
        /// Where the table that is too deep starts.
        location: Location,
        /// The deepest nesting the layout follows: a table in a table is at depth 2.
        limit: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocument { location, problem } => write!(f, "{location}: {problem}"),
            Self::NestingTooDeep { location, limit } => {
                write!(f, "{location}: tables are nested more than {limit} deep")
            }
        }
    }
}

impl error::Error for Error {}

impl From<Error> for officewright_core::Error {
    fn from(err: Error) -> Self {
        match err {
            // The IR is an argument the caller built, not input read from a file.
            invalid @ Error::InvalidDocument { .. } => Self::InvalidArgument(invalid.to_string()),
            deep @ Error::NestingTooDeep { .. } => Self::LimitExceeded(deep.to_string()),
        }
    }
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
