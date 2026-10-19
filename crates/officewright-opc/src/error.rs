use std::{error, fmt};

/// An error from reading a ZIP archive, an XML part or an OOXML package.
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
    /// An XML part is not well-formed, or breaks a rule of Markup Compatibility.
    #[non_exhaustive]
    Xml {
        /// The part, when known.
        part: Option<String>,
        /// Line, starting at 1.
        line: u64,
        /// Column in characters, starting at 1.
        column: u64,
        /// What is wrong.
        message: String,
    },
    /// The package breaks a rule of the Open Packaging Conventions.
    #[non_exhaustive]
    Package {
        /// What is wrong, naming the part.
        message: String,
    },
    /// The input exceeds one of the configured [`Limits`](crate::Limits).
    #[non_exhaustive]
    LimitExceeded {
        /// Which limit.
        limit: Limit,
        /// The configured maximum.
        maximum: u64,
        /// Where: an entry, a part or a position in a part.
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
    /// [`Limits::max_total_size`](crate::Limits::max_total_size).
    TotalSize,
    /// [`Limits::max_part_name_length`](crate::Limits::max_part_name_length).
    PartNameLength,
    /// [`Limits::max_relationships`](crate::Limits::max_relationships).
    Relationships,
    /// [`Limits::max_content_types`](crate::Limits::max_content_types).
    ContentTypes,
    /// [`Limits::max_xml_depth`](crate::Limits::max_xml_depth).
    XmlDepth,
    /// [`Limits::max_xml_attributes`](crate::Limits::max_xml_attributes).
    XmlAttributes,
    /// [`Limits::max_mce_names`](crate::Limits::max_mce_names).
    MceNames,
    /// [`Limits::max_xml_namespaces`](crate::Limits::max_xml_namespaces).
    XmlNamespaces,
}

impl Error {
    /// Names the part an XML error or a size limit refers to, when the error does not
    /// name one yet.
    #[must_use]
    pub fn in_part(self, part: &str) -> Self {
        match self {
            Self::Xml {
                part: None,
                line,
                column,
                message,
            } => Self::Xml {
                part: Some(part.to_owned()),
                line,
                column,
                message,
            },
            Self::LimitExceeded {
                limit,
                maximum,
                context,
            } if matches!(
                limit,
                Limit::XmlDepth | Limit::XmlAttributes | Limit::XmlNamespaces | Limit::MceNames
            ) =>
            {
                Self::LimitExceeded {
                    limit,
                    maximum,
                    context: format!("{part}, {context}"),
                }
            }
            other => other,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zip { offset, message } => write!(f, "zip archive: {message} at byte {offset}"),
            Self::Entry { name, message } => write!(f, "zip entry {name}: {message}"),
            Self::Xml {
                part,
                line,
                column,
                message,
            } => {
                if let Some(part) = part {
                    write!(f, "{part}, ")?;
                }
                write!(f, "line {line}, column {column}: {message}")
            }
            Self::Package { message } | Self::Unsupported(message) => f.write_str(message),
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
            Self::TotalSize => "total uncompressed size of the parts read in bytes",
            Self::PartNameLength => "length of a part name in bytes",
            Self::Relationships => "number of relationships in the package",
            Self::ContentTypes => "number of Default and Override elements",
            Self::XmlDepth => "xml element nesting depth",
            Self::XmlAttributes => "number of attributes on an xml element",
            Self::MceNames => "number of names in a markup compatibility attribute",
            Self::XmlNamespaces => "number of namespaces an xml part declares",
        })
    }
}

impl error::Error for Error {}

impl From<Error> for officewright_core::Error {
    fn from(err: Error) -> Self {
        let message = err.to_string();
        match err {
            Error::Unsupported(_) => Self::Unsupported(message),
            Error::Zip { .. } | Error::Entry { .. } | Error::Xml { .. } | Error::Package { .. } => {
                Self::Malformed(message)
            }
            Error::LimitExceeded { .. } => Self::LimitExceeded(message),
        }
    }
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_limit_has_its_counterpart_in_officewright_doc() {
        // officewright-doc gives each limit of this crate its own (`impl
        // From<officewright_opc::Error>` in its error.rs), and can only report a limit it
        // does not name as a damaged document (#52). This match has no wildcard, so a new
        // limit stops this test compiling until it is listed here, after its counterpart
        // was added there.
        let listed = |limit: Limit| match limit {
            Limit::Entries
            | Limit::PartSize
            | Limit::TotalSize
            | Limit::PartNameLength
            | Limit::Relationships
            | Limit::ContentTypes
            | Limit::XmlDepth
            | Limit::XmlAttributes
            | Limit::MceNames
            | Limit::XmlNamespaces => true,
        };
        assert!(listed(Limit::Entries));
    }

    #[test]
    fn in_part_names_the_part_once() {
        let err = Error::Xml {
            part: None,
            line: 2,
            column: 7,
            message: "unexpected end of input".into(),
        };
        let err = err.in_part("/word/document.xml").in_part("/other.xml");
        assert_eq!(
            err.to_string(),
            "/word/document.xml, line 2, column 7: unexpected end of input"
        );
    }

    #[test]
    fn in_part_prefixes_xml_limits_only() {
        let depth = Error::LimitExceeded {
            limit: Limit::XmlDepth,
            maximum: 256,
            context: "line 9, column 1".into(),
        };
        assert_eq!(
            depth.in_part("/a.xml").to_string(),
            "/a.xml, line 9, column 1: xml element nesting depth exceeds the limit of 256"
        );
        let size = Error::LimitExceeded {
            limit: Limit::PartSize,
            maximum: 10,
            context: "zip entry a.xml".into(),
        };
        assert_eq!(
            size.in_part("/b.xml").to_string(),
            "zip entry a.xml: uncompressed size in bytes exceeds the limit of 10"
        );
    }

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
            officewright_core::Error::Malformed(ref m) if m == "zip archive: bad signature at byte 4"
        ));
        let xml: officewright_core::Error = Error::Xml {
            part: Some("/a.xml".into()),
            line: 1,
            column: 2,
            message: "expected a name".into(),
        }
        .into();
        assert!(matches!(
            xml,
            officewright_core::Error::Malformed(ref m) if m == "/a.xml, line 1, column 2: expected a name"
        ));
        let limit: officewright_core::Error = Error::LimitExceeded {
            limit: Limit::PartSize,
            maximum: 10,
            context: "zip entry a.xml".into(),
        }
        .into();
        assert!(matches!(
            limit,
            officewright_core::Error::LimitExceeded(ref m)
                if m == "zip entry a.xml: uncompressed size in bytes exceeds the limit of 10"
        ));
        let package: officewright_core::Error = Error::Package {
            message: "two parts are named /a.xml, ignoring case".into(),
        }
        .into();
        assert!(matches!(
            package,
            officewright_core::Error::Malformed(ref m) if m == "two parts are named /a.xml, ignoring case"
        ));
    }
}
