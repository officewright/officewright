use std::path::PathBuf;
use std::{error, fmt, io};

use crate::face::OutlineFormat;
use crate::font::GlyphId;

/// An error from reading, subsetting or finding a font.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The data is neither a TrueType or OpenType font nor a font collection, or its
    /// header is damaged.
    #[non_exhaustive]
    NotAFont {
        /// What the font parser found.
        source: Cause,
    },
    /// The file has no face with this index.
    #[non_exhaustive]
    FaceIndexOutOfRange {
        /// The requested face.
        index: u32,
        /// The number of faces in the file.
        count: u32,
    },
    /// The table directory of a face is damaged.
    #[non_exhaustive]
    MalformedFace {
        /// The face.
        face: u32,
        /// What the font parser found.
        source: Cause,
    },
    /// A table that every font needs is missing.
    #[non_exhaustive]
    MissingTable {
        /// The face.
        face: u32,
        /// The table tag, such as `"hmtx"`.
        table: &'static str,
    },
    /// A table is damaged.
    #[non_exhaustive]
    MalformedTable {
        /// The face.
        face: u32,
        /// The table tag, such as `"hmtx"`.
        table: &'static str,
        /// What the font parser found.
        source: Cause,
    },
    /// A table is within the format but past a limit the library sets on the work of
    /// reading it: `name` records that point at more than 1 MiB of strings to keep when
    /// subsetting, `CFF ` Font DICTs that point at more than 1 MiB of Private DICTs or
    /// have more than 262,144 local subroutines in all, or an FDSelect that declares more
    /// than 2,048 ranges, which no real font comes near; or glyphs to keep whose `CFF `
    /// charstrings take more than 16,777,216 operands, operators and hint mask bytes to
    /// decompile, which only the largest CJK fonts reach, kept whole, or more than is left
    /// of the [`SubsetBudget`](crate::SubsetBudget) the subset is made within.
    #[non_exhaustive]
    TableOverLimit {
        /// The face.
        face: u32,
        /// The table tag, such as `"name"`.
        table: &'static str,
        /// What is past the limit.
        source: Cause,
    },
    /// `unitsPerEm` in the `head` table is outside the 16 to 16384 range that the
    /// OpenType specification allows.
    #[non_exhaustive]
    InvalidUnitsPerEm {
        /// The face.
        face: u32,
        /// The value in the font.
        units_per_em: u16,
    },
    /// A glyph ID is not smaller than the number of glyphs in the face.
    #[non_exhaustive]
    GlyphOutOfRange {
        /// The face.
        face: u32,
        /// The glyph.
        glyph: GlyphId,
        /// The number of glyphs in the face.
        glyph_count: u16,
    },
    /// The outlines of the face cannot be subset: it has none, or they are CFF2.
    #[non_exhaustive]
    UnsupportedOutlines {
        /// The face.
        face: u32,
        /// The outline format, `None` if the face has no outlines.
        format: Option<OutlineFormat>,
    },
    /// The subsetter rejected the face.
    #[non_exhaustive]
    Subset {
        /// The face.
        face: u32,
        /// What the subsetter reported.
        source: Cause,
    },
    /// The face uses something the subsetter does not implement yet, such as accented
    /// glyphs that CFF outlines compose with the deprecated four-argument `endchar`.
    #[non_exhaustive]
    SubsetUnsupported {
        /// The face.
        face: u32,
        /// What the subsetter reported, or what the library found that the subsetter does
        /// not take.
        source: Cause,
    },
    /// The subsetter panicked on the data of the face. The panic was caught; the font
    /// is malformed in a way the subsetter does not handle yet.
    #[non_exhaustive]
    SubsetterPanicked {
        /// The face.
        face: u32,
        /// The panic message.
        message: String,
    },
    /// Reading a font file or directory failed.
    #[non_exhaustive]
    Read {
        /// The file or directory.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAFont { .. } => {
                f.write_str("data is not a TrueType or OpenType font or font collection")
            }
            Self::FaceIndexOutOfRange { index, count } => {
                let faces = if *count == 1 { "face" } else { "faces" };
                write!(
                    f,
                    "font face {index} does not exist, the file has {count} {faces}"
                )
            }
            Self::MalformedFace { face, .. } => {
                write!(f, "font face {face}: the table directory is damaged")
            }
            Self::MissingTable { face, table } => {
                write!(f, "font face {face}: required table '{table}' is missing")
            }
            Self::MalformedTable { face, table, .. } => {
                write!(f, "font face {face}: table '{table}' is damaged")
            }
            Self::TableOverLimit { face, table, .. } => {
                write!(
                    f,
                    "font face {face}: table '{table}' is past a limit of the library"
                )
            }
            Self::InvalidUnitsPerEm { face, units_per_em } => write!(
                f,
                "font face {face}: unitsPerEm {units_per_em} is outside the 16 to 16384 \
                 range OpenType allows"
            ),
            Self::GlyphOutOfRange {
                face,
                glyph,
                glyph_count,
            } => write!(
                f,
                "font face {face}: glyph {glyph} does not exist, the face has {glyph_count} \
                 glyphs"
            ),
            Self::UnsupportedOutlines { face, format } => match format {
                Some(format) => write!(
                    f,
                    "font face {face}: {format} outlines cannot be subset yet"
                ),
                None => write!(f, "font face {face} has no glyph outlines to subset"),
            },
            Self::Subset { face, .. } => write!(f, "cannot subset font face {face}"),
            Self::SubsetUnsupported { face, .. } => write!(
                f,
                "cannot subset font face {face}: it uses a feature the subsetter does not \
                 support yet"
            ),
            Self::SubsetterPanicked { face, message } => write!(
                f,
                "cannot subset font face {face}: the subsetter crashed on its data \
                 ({message})"
            ),
            Self::Read { path, .. } => write!(f, "cannot read {}", path.display()),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::NotAFont { source }
            | Self::MalformedFace { source, .. }
            | Self::MalformedTable { source, .. }
            | Self::TableOverLimit { source, .. }
            | Self::Subset { source, .. }
            | Self::SubsetUnsupported { source, .. } => Some(source),
            Self::Read { source, .. } => Some(source),
            Self::FaceIndexOutOfRange { .. }
            | Self::MissingTable { .. }
            | Self::InvalidUnitsPerEm { .. }
            | Self::GlyphOutOfRange { .. }
            | Self::UnsupportedOutlines { .. }
            | Self::SubsetterPanicked { .. } => None,
        }
    }
}

impl From<Error> for officewright_core::Error {
    /// A file that cannot be read is an I/O error. Data that is not a font, or a font
    /// that cannot be read or subset, is malformed; a face index or glyph the font does
    /// not have is an invalid argument; outlines or features that cannot be subset yet
    /// are unsupported; a table past a limit of the library is a limit exceeded. The
    /// message keeps the reason the font parser or the subsetter gave.
    fn from(err: Error) -> Self {
        let mut message = err.to_string();
        if let Some(cause) = error::Error::source(&err) {
            if !matches!(err, Error::Read { .. }) {
                message.push_str(": ");
                message.push_str(&cause.to_string());
            }
        }
        match err {
            Error::Read { source, .. } => Self::Io { message, source },
            Error::UnsupportedOutlines { .. } | Error::SubsetUnsupported { .. } => {
                Self::Unsupported(message)
            }
            Error::NotAFont { .. }
            | Error::MalformedFace { .. }
            | Error::MissingTable { .. }
            | Error::MalformedTable { .. }
            | Error::InvalidUnitsPerEm { .. }
            | Error::Subset { .. }
            | Error::SubsetterPanicked { .. } => Self::Malformed(message),
            Error::FaceIndexOutOfRange { .. } | Error::GlyphOutOfRange { .. } => {
                Self::InvalidArgument(message)
            }
            Error::TableOverLimit { .. } => Self::LimitExceeded(message),
        }
    }
}

/// The reason the font parser or the subsetter gave for an [`Error`].
///
/// It is kept as text so that the types of those crates do not become part of this
/// crate's API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cause(String);

impl Cause {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl error::Error for Cause {}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn messages_name_the_face_and_table() {
        let err = Error::MalformedTable {
            face: 2,
            table: "hmtx",
            source: Cause::new("An offset was out of bounds"),
        };
        assert_eq!(err.to_string(), "font face 2: table 'hmtx' is damaged");
        assert_eq!(
            error::Error::source(&err).map(ToString::to_string),
            Some("An offset was out of bounds".into())
        );
    }

    #[test]
    fn converts_into_the_core_error_with_its_cause() {
        let damaged = Error::MalformedTable {
            face: 0,
            table: "cmap",
            source: Cause::new("the table declares 2 records but holds 0"),
        };
        match officewright_core::Error::from(damaged) {
            officewright_core::Error::Malformed(message) => assert_eq!(
                message,
                "font face 0: table 'cmap' is damaged: the table declares 2 records but holds 0"
            ),
            other => panic!("{other:?}"),
        }
        // A table past a limit of the library is a limit, not damage.
        let over = Error::TableOverLimit {
            face: 0,
            table: "name",
            source: Cause::new("too many strings"),
        };
        assert!(matches!(
            officewright_core::Error::from(over),
            officewright_core::Error::LimitExceeded(message)
                if message == "font face 0: table 'name' is past a limit of the library: too many strings"
        ));
        // A face the file does not have is the caller's mistake.
        assert!(matches!(
            officewright_core::Error::from(Error::FaceIndexOutOfRange { index: 3, count: 1 }),
            officewright_core::Error::InvalidArgument(message)
                if message == "font face 3 does not exist, the file has 1 face"
        ));
        let cff2 = Error::UnsupportedOutlines {
            face: 1,
            format: Some(OutlineFormat::Cff2),
        };
        assert!(matches!(
            officewright_core::Error::from(cff2),
            officewright_core::Error::Unsupported(message)
                if message == "font face 1: CFF2 outlines cannot be subset yet"
        ));
        let read = Error::Read {
            path: PathBuf::from("fonts/a.ttf"),
            source: io::Error::other("disk gone"),
        };
        match officewright_core::Error::from(read) {
            officewright_core::Error::Io { message, source } => {
                assert_eq!(
                    message,
                    format!("cannot read {}", Path::new("fonts/a.ttf").display())
                );
                assert_eq!(source.to_string(), "disk gone");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn face_count_is_pluralized() {
        let one = Error::FaceIndexOutOfRange { index: 1, count: 1 };
        assert_eq!(
            one.to_string(),
            "font face 1 does not exist, the file has 1 face"
        );
        let two = Error::FaceIndexOutOfRange { index: 5, count: 2 };
        assert_eq!(
            two.to_string(),
            "font face 5 does not exist, the file has 2 faces"
        );
    }
}
