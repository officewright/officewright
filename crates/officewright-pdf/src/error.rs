use std::path::PathBuf;
use std::{error, fmt, io};

use officewright_core::Pt;

use crate::script::{self, Refusal};

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
    /// The text has a character that an embedded font cannot be drawn with yet: text is
    /// drawn one glyph per character, left to right, without shaping, so control
    /// characters, bidirectional formatting characters, unassigned and private-use code
    /// points, conjoining Hangul jamo and the scripts that need shaping or right-to-left
    /// layout are refused. See [`Page::add_text`](crate::Page::add_text).
    UnsupportedCharacter {
        /// The character.
        ch: char,
        /// Its byte offset in the text.
        offset: usize,
    },
    /// The embedded font has no glyph for a character of the text.
    MissingGlyph {
        /// The character.
        ch: char,
        /// Its byte offset in the text.
        offset: usize,
        /// The font's name.
        font: String,
    },
    /// A font size is outside [`Font::MIN_SIZE`](crate::Font::MIN_SIZE) to
    /// [`Font::MAX_SIZE`](crate::Font::MAX_SIZE).
    InvalidFontSize(Pt),
    /// A position is outside -32767 to 32767 pt: where text or a glyph is drawn, a glyph's
    /// offset in its run, or a point of a path.
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
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// A [`GlyphRun`](crate::GlyphRun) is in a standard font. Glyphs are drawn by their
    /// IDs in an embedded font; a standard font is addressed through WinAnsiEncoding.
    NotEmbedded {
        /// The font's name.
        font: String,
    },
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// A glyph ID is not less than the number of glyphs of the font.
    InvalidGlyph {
        /// The glyph ID.
        glyph: u16,
        /// The font's name.
        font: String,
    },
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// A cluster of a [`GlyphRun`](crate::GlyphRun) has no glyphs, so nothing would show
    /// its text.
    EmptyCluster {
        /// The cluster's index in its run, from 0.
        cluster: usize,
    },
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// The text of a cluster of a [`GlyphRun`](crate::GlyphRun) has more than 16382 UTF-16
    /// code units, too many for its replacement text to fit in a PDF string.
    ClusterTooLong {
        /// The cluster's index in its run, from 0.
        cluster: usize,
        /// The number of UTF-16 code units of its text.
        units: usize,
    },
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// A number for drawing is outside the range it must be in.
    OutOfRange {
        /// What the number is, such as "line width".
        what: &'static str,
        /// The number.
        value: f64,
        /// The smallest value allowed.
        min: f64,
        /// The largest value allowed.
        max: f64,
    },
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// A dash pattern has only lengths of 0, which draws nothing (ISO 32000-1, 8.4.3.6);
    /// a solid line has an empty pattern.
    InvalidDash,
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// [`Page::pop_clip`](crate::Page::pop_clip) found no clipping path to remove.
    NoClip,
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// An image is damaged: truncated, with a wrong checksum, missing or invalid headers,
    /// or compressed data that does not decompress to the size its header gives.
    MalformedImage {
        /// The image file, `None` for an image in memory.
        path: Option<PathBuf>,
        /// What is wrong with it.
        reason: String,
    },
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// An image is not a JPEG or PNG file, or uses a feature that cannot go into a PDF.
    UnsupportedImage {
        /// The image file, `None` for an image in memory.
        path: Option<PathBuf>,
        /// What cannot be used.
        reason: String,
    },
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// A PNG image's pixel data would take more than 256 MiB.
    ImageTooLarge {
        /// The image file, `None` for an image in memory.
        path: Option<PathBuf>,
    },
    #[cfg_attr(not(feature = "unstable-drawing"), doc(hidden))]
    /// More clipping paths would be in effect at once than every PDF reader can nest:
    /// [`Page::MAX_CLIPS`](crate::Page::MAX_CLIPS).
    TooManyClips,
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
    /// Reading a font file failed.
    Read {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A font cannot be used: it is not a TrueType or OpenType font, has no such face,
    /// is damaged, or has outlines that cannot be embedded.
    Font {
        /// The font file, `None` for a font in memory.
        path: Option<PathBuf>,
        /// What is wrong with it.
        source: officewright_font::Error,
    },
    /// The license of a font forbids embedding it (`fsType` in its `OS/2` table).
    EmbeddingNotAllowed {
        /// The font's name.
        font: String,
        /// The license allows embedding only the font's bitmaps, but the PDF writer
        /// embeds outlines; otherwise the license forbids embedding altogether.
        bitmap_only: bool,
    },
    /// No font could be found for a family: the font book has neither the family nor a
    /// substitute or fallback for it.
    FontNotFound {
        /// The requested family.
        family: String,
    },
    /// A font could not be reduced to the glyphs the document uses: it is damaged in a
    /// way that only subsetting those glyphs finds, or uses a feature the subsetter does
    /// not support yet.
    Embed {
        /// The font's name.
        font: String,
        /// What the subsetter found.
        source: officewright_font::Error,
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
            Self::UnsupportedCharacter { ch, offset } => {
                write!(
                    f,
                    "character U+{:04X} '{}' at byte {offset} ",
                    u32::from(*ch),
                    ch.escape_debug()
                )?;
                match script::check(*ch) {
                    Err(Refusal::Control) => f.write_str("is a control character"),
                    Err(Refusal::Bidi) => f.write_str(
                        "is a bidirectional formatting character, and right-to-left layout is \
                         not supported yet",
                    ),
                    Err(Refusal::Jamo) => f.write_str(
                        "is a conjoining Hangul jamo; only precomposed Hangul syllables can be \
                         drawn without shaping",
                    ),
                    Err(Refusal::Script(script)) => write!(
                        f,
                        "is in the {} script, which needs shaping or right-to-left layout; that \
                         is not supported yet",
                        script.full_name()
                    ),
                    Err(Refusal::Unknown) => f.write_str("is unassigned or for private use"),
                    Ok(()) => f.write_str("cannot be drawn"),
                }
            }
            Self::MissingGlyph { ch, offset, font } => write!(
                f,
                "font {font} has no glyph for character U+{:04X} '{}' at byte {offset}",
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
                "position ({}, {}) pt is outside the -{max} to {max} pt range",
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
            Self::NotEmbedded { font } => write!(
                f,
                "glyphs can be drawn only in an embedded font, and {font} is a standard font"
            ),
            Self::InvalidGlyph { glyph, font } => write!(f, "font {font} has no glyph {glyph}"),
            Self::EmptyCluster { cluster } => {
                write!(f, "cluster {cluster} of the glyph run has no glyphs")
            }
            Self::ClusterTooLong { cluster, units } => write!(
                f,
                "cluster {cluster} of the glyph run has {units} UTF-16 code units of text, more \
                 than the {} its replacement text can hold",
                crate::glyphs::MAX_CLUSTER_UNITS
            ),
            Self::OutOfRange {
                what,
                value,
                min,
                max,
            } => write!(f, "{what} {value} is outside the {min} to {max} range"),
            Self::InvalidDash => {
                f.write_str("dash pattern has only lengths of 0; a solid line has an empty pattern")
            }
            Self::NoClip => f.write_str("there is no clipping path to remove"),
            Self::MalformedImage { path, reason } => {
                write!(
                    f,
                    "image {} is damaged: {reason}",
                    image_name(path.as_deref())
                )
            }
            Self::UnsupportedImage { path, reason } => write!(
                f,
                "image {} cannot be used: {reason}",
                image_name(path.as_deref())
            ),
            Self::ImageTooLarge { path } => write!(
                f,
                "image {} is too large: its pixels would take more than {} bytes",
                image_name(path.as_deref()),
                crate::image::MAX_PNG_DATA
            ),
            Self::TooManyClips => write!(
                f,
                "more than {} clipping paths would be in effect at once",
                crate::Page::MAX_CLIPS
            ),
            Self::TooLarge => {
                f.write_str("PDF exceeds the 10000000000 bytes a cross-reference table can address")
            }
            Self::Io(_) => f.write_str("cannot write the PDF"),
            Self::Write { path, .. } => write!(f, "cannot write {}", path.display()),
            Self::Read { path, .. } => write!(f, "cannot read {}", path.display()),
            Self::Font {
                path: Some(path), ..
            } => write!(f, "cannot use the font in {}", path.display()),
            Self::Font { path: None, .. } => f.write_str("cannot use the font data"),
            Self::EmbeddingNotAllowed {
                font,
                bitmap_only: true,
            } => write!(
                f,
                "the license of font {font} allows embedding only its bitmaps, not its outlines"
            ),
            Self::EmbeddingNotAllowed { font, .. } => {
                write!(f, "the license of font {font} does not allow embedding it")
            }
            Self::FontNotFound { family } => write!(f, "no font found for family \"{family}\""),
            Self::Embed { font, .. } => write!(f, "cannot embed font {font}"),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io(source) | Self::Write { source, .. } | Self::Read { source, .. } => {
                Some(source)
            }
            Self::Font { source, .. } | Self::Embed { source, .. } => Some(source),
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
            Error::Io(source) | Error::Write { source, .. } | Error::Read { source, .. } => {
                Self::Io { message, source }
            }
            // The font error decides the kind; the message keeps every reason, since only
            // an I/O error carries its source.
            Error::Font { source, .. } | Error::Embed { source, .. } => {
                let message = format!("{message}: {}", chain(&source));
                match Self::from(source) {
                    Self::Io { source, .. } => Self::Io { message, source },
                    Self::Unsupported(_) => Self::Unsupported(message),
                    Self::Malformed(_) => Self::Malformed(message),
                    Self::LimitExceeded(_) => Self::LimitExceeded(message),
                    Self::InvalidArgument(_) => Self::InvalidArgument(message),
                    // A kind of error this version does not know yet.
                    _ => Self::Unsupported(message),
                }
            }
            Error::MalformedImage { .. } => Self::Malformed(message),
            Error::ImageTooLarge { .. } => Self::LimitExceeded(message),
            Error::Unencodable { .. }
            | Error::UnsupportedImage { .. }
            | Error::UnsupportedCharacter { .. }
            | Error::MissingGlyph { .. }
            | Error::EmbeddingNotAllowed { .. }
            | Error::FontNotFound { .. }
            | Error::TooLarge => Self::Unsupported(message),
            Error::InvalidFontSize(_)
            | Error::InvalidPosition { .. }
            | Error::InvalidPageSize { .. }
            | Error::NoRoomForText { .. }
            | Error::NoPages
            | Error::NotEmbedded { .. }
            | Error::InvalidGlyph { .. }
            | Error::EmptyCluster { .. }
            | Error::ClusterTooLong { .. }
            | Error::OutOfRange { .. }
            | Error::InvalidDash
            | Error::NoClip
            | Error::TooManyClips => Self::InvalidArgument(message),
        }
    }
}

/// How an image error names the image: by its file, or as data in memory.
fn image_name(path: Option<&std::path::Path>) -> String {
    match path {
        Some(path) => path.display().to_string(),
        None => "data".to_owned(),
    }
}

/// An error's message followed by the messages of its causes.
fn chain(err: &dyn error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message = format!("{message}: {cause}");
        source = cause.source();
    }
    message
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
