use std::{error, fmt};

/// An error from reading a ZIP archive.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The input is not a ZIP archive, or its structure is broken.
    #[non_exhaustive]
    Zip {
        /// Byte offset of the problem in the archive.
        offset: u64,
        /// What is wrong.
        message: String,
    },
    /// A ZIP entry cannot be decompressed or fails its integrity check.
    #[non_exhaustive]
    Entry {
        /// The entry name.
        name: String,
        /// What is wrong.
        message: String,
    },
    /// The input exceeds one of the configured [`Limits`](crate::Limits).
    #[non_exhaustive]
    LimitExceeded {
        /// Which limit.
        limit: Limit,
        /// The configured maximum.
        maximum: u64,
        /// Where: the archive or an entry.
        context: String,
    },
    /// The input is valid but uses something this crate does not read.
    Unsupported(String),
}

/// One of the bounds set by [`Limits`](crate::Limits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Limit {
    /// [`Limits::max_entries`](crate::Limits::max_entries).
    Entries,
    /// [`Limits::max_part_size`](crate::Limits::max_part_size).
    PartSize,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zip { offset, message } => write!(f, "zip archive: {message} at byte {offset}"),
            Self::Entry { name, message } => write!(f, "zip entry {name}: {message}"),
            Self::Unsupported(message) => f.write_str(message),
            Self::LimitExceeded {
                limit,
                maximum,
                context,
            } => write!(f, "{context}: {limit} exceeds the limit of {maximum}"),
        }
    }
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Entries => "number of zip entries",
            Self::PartSize => "uncompressed size in bytes",
        })
    }
}

impl error::Error for Error {}

impl From<Error> for officewright_core::Error {
    fn from(err: Error) -> Self {
        let message = err.to_string();
        match err {
            Error::Unsupported(_) => Self::Unsupported(message),
            // officewright_core::Error has no category for malformed input yet; the
            // bytes passed in are the argument that is wrong.
            Error::Zip { .. } | Error::Entry { .. } | Error::LimitExceeded { .. } => {
                Self::InvalidArgument(message)
            }
        }
    }
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_into_core_categories() {
        let unsupported: officewright_core::Error =
            Error::Unsupported("encrypted zip entry a.xml".into()).into();
        assert!(matches!(
            unsupported,
            officewright_core::Error::Unsupported(ref m) if m == "encrypted zip entry a.xml"
        ));
        let malformed: officewright_core::Error = Error::Zip {
            offset: 4,
            message: "bad signature".into(),
        }
        .into();
        assert!(matches!(
            malformed,
            officewright_core::Error::InvalidArgument(ref m) if m == "zip archive: bad signature at byte 4"
        ));
        let limit: officewright_core::Error = Error::LimitExceeded {
            limit: Limit::PartSize,
            maximum: 10,
            context: "zip entry a.xml".into(),
        }
        .into();
        assert!(matches!(
            limit,
            officewright_core::Error::InvalidArgument(ref m)
                if m == "zip entry a.xml: uncompressed size in bytes exceeds the limit of 10"
        ));
    }
}
