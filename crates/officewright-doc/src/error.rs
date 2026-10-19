use std::{error, fmt, io};

/// An error from opening a document, or from rendering it into PDF.
///
/// Damage that Word repairs by itself is not an error: the document opens and
/// [`Document::diagnostics`](crate::Document::diagnostics) says what was repaired.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Reading the file failed.
    #[non_exhaustive]
    Io {
        /// What was being read.
        message: String,
        /// The underlying error.
        source: io::Error,
    },
    /// The input is not a document, or its structure is broken: a damaged ZIP archive,
    /// XML that is not well-formed, a package without a main document part.
    #[non_exhaustive]
    Malformed {
        /// What is wrong, naming the part and position when known.
        message: String,
    },
    /// The input exceeds one of the configured [`Limits`](crate::Limits).
    #[non_exhaustive]
    LimitExceeded {
        /// Which limit.
        limit: Limit,
        /// The configured maximum.
        maximum: u64,
        /// Where: a part, or a position in a part.
        context: String,
    },
    /// The input is valid but uses something officewright cannot read yet, such as an
    /// encrypted document or a binary Word document.
    Unsupported(String),
    /// Laying the document out or making its PDF failed, such as when it takes more
    /// pages than the [`RenderOptions`](crate::RenderOptions) allow. The layout engine's
    /// error is passed through: its message and its cause are this error's.
    #[cfg(feature = "pdf")]
    #[non_exhaustive]
    Layout {
        /// The layout engine's error.
        source: officewright_layout::Error,
    },
    /// A font the [`RenderOptions`](crate::RenderOptions) give cannot be read, or a
    /// directory of fonts cannot be listed. The font library's error is passed through,
    /// as for `Layout`.
    #[cfg(feature = "pdf")]
    #[non_exhaustive]
    Font {
        /// The font library's error.
        source: officewright_font::Error,
    },
    /// The PDF cannot be made, such as when a font cannot be subset for it, or cannot be
    /// written, to the file or the writer. The PDF writer's error is passed through, as
    /// for `Layout`.
    #[cfg(feature = "pdf")]
    #[non_exhaustive]
    Pdf {
        /// The PDF writer's error.
        source: officewright_pdf::Error,
    },
}

/// One of the bounds set by [`Limits`](crate::Limits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Limit {
    /// [`Limits::max_file_size`](crate::Limits::max_file_size).
    FileSize,
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
    /// [`Limits::max_nodes`](crate::Limits::max_nodes).
    Nodes,
    /// [`Limits::max_xml_namespaces`](crate::Limits::max_xml_namespaces).
    XmlNamespaces,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { message, .. } | Self::Malformed { message } | Self::Unsupported(message) => {
                f.write_str(message)
            }
            Self::LimitExceeded {
                limit,
                maximum,
                context,
            } => write!(f, "{context}: {limit} exceeds the limit of {maximum}"),
            #[cfg(feature = "pdf")]
            Self::Layout { source } => write!(f, "{source}"),
            #[cfg(feature = "pdf")]
            Self::Font { source } => write!(f, "{source}"),
            #[cfg(feature = "pdf")]
            Self::Pdf { source } => write!(f, "{source}"),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            // The errors of rendering are passed through: their message is this error's,
            // and their cause its cause, so that no message is said twice.
            #[cfg(feature = "pdf")]
            Self::Layout { source } => source.source(),
            #[cfg(feature = "pdf")]
            Self::Font { source } => source.source(),
            #[cfg(feature = "pdf")]
            Self::Pdf { source } => source.source(),
            _ => None,
        }
    }
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FileSize => "size of the file in bytes",
            Self::Entries => "number of zip entries",
            Self::PartSize => "unpacked size of a part in bytes",
            Self::TotalSize => "unpacked size of all parts read in bytes",
            Self::PartNameLength => "length of a part name in bytes",
            Self::Relationships => "number of relationships in the package",
            Self::ContentTypes => "number of Default and Override elements",
            Self::XmlDepth => "xml element nesting depth",
            Self::XmlAttributes => "number of attributes on one xml element",
            Self::MceNames => "number of names in one markup compatibility attribute",
            Self::Nodes => "number of elements in the document",
            Self::XmlNamespaces => "number of namespaces one xml part declares",
        })
    }
}

impl From<officewright_opc::Error> for Error {
    fn from(err: officewright_opc::Error) -> Self {
        use officewright_opc::{Error as Opc, Limit as L};
        match err {
            Opc::LimitExceeded {
                limit,
                maximum,
                context,
                ..
            } => {
                let limit = match limit {
                    L::Entries => Limit::Entries,
                    L::PartSize => Limit::PartSize,
                    L::TotalSize => Limit::TotalSize,
                    L::PartNameLength => Limit::PartNameLength,
                    L::Relationships => Limit::Relationships,
                    L::ContentTypes => Limit::ContentTypes,
                    L::XmlDepth => Limit::XmlDepth,
                    L::XmlAttributes => Limit::XmlAttributes,
                    L::MceNames => Limit::MceNames,
                    L::XmlNamespaces => Limit::XmlNamespaces,
                    // A limit added to officewright-opc but not to this enum yet, which
                    // the test lists_every_limit_so_that_a_new_one_stops_the_build of
                    // officewright-opc keeps from happening (#52); its message would still
                    // say which.
                    _ => {
                        return Self::Malformed {
                            message: format!("{context}: a limit of {maximum} is exceeded"),
                        };
                    }
                };
                Self::LimitExceeded {
                    limit,
                    maximum,
                    context,
                }
            }
            Opc::Unsupported(message) => Self::Unsupported(message),
            other => Self::Malformed {
                message: other.to_string(),
            },
        }
    }
}

impl From<Error> for officewright_core::Error {
    fn from(err: Error) -> Self {
        match err {
            Error::Io { message, source } => Self::Io { message, source },
            Error::Unsupported(message) => Self::Unsupported(message),
            Error::Malformed { message } => Self::Malformed(message),
            limit @ Error::LimitExceeded { .. } => Self::LimitExceeded(limit.to_string()),
            #[cfg(feature = "pdf")]
            Error::Layout { source } => source.into(),
            #[cfg(feature = "pdf")]
            Error::Font { source } => source.into(),
            #[cfg(feature = "pdf")]
            Error::Pdf { source } => source.into(),
        }
    }
}

#[cfg(feature = "pdf")]
impl From<officewright_layout::Error> for Error {
    fn from(source: officewright_layout::Error) -> Self {
        Self::Layout { source }
    }
}

#[cfg(feature = "pdf")]
impl From<officewright_font::Error> for Error {
    fn from(source: officewright_font::Error) -> Self {
        Self::Font { source }
    }
}

#[cfg(feature = "pdf")]
impl From<officewright_pdf::Error> for Error {
    fn from(source: officewright_pdf::Error) -> Self {
        Self::Pdf { source }
    }
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
