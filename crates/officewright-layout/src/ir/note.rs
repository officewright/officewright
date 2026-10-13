//! Footnotes and endnotes: their content, how they are numbered and where they go.

use super::macros::{ir_struct, no_rules};
use super::paragraph::Block;
use super::section::NumberFormat;

ir_struct! {
    /// A footnote or an endnote: its content, laid out where its reference sends it.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Note {
        /// Whether it is a footnote or an endnote.
        copy kind: NoteKind => with_kind;
        /// Its content. It holds no note references.
        vec blocks: Block => with_blocks;
    }
}

impl Note {
    /// A note of this kind with this content.
    pub fn new(kind: NoteKind, blocks: impl IntoIterator<Item = Block>) -> Self {
        Self {
            kind,
            blocks: blocks.into_iter().collect(),
        }
    }
}

/// Whether a note is a footnote or an endnote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NoteKind {
    /// A footnote.
    Footnote,
    /// An endnote.
    Endnote,
}

ir_struct! {
    /// How the footnotes or the endnotes of a section are numbered and placed, after the
    /// producer has applied the document's settings and the section's own. Word takes
    /// the placement of footnotes from the section, not the document (MS-OI29500
    /// 2.1.309 a).
    #[derive(Clone, Debug, PartialEq)]
    pub struct NoteProps {
        /// Where the notes go.
        copy position: NotePosition => with_position;
        /// How their numbers are written. Word writes bullet numbering as decimal
        /// (MS-OI29500 2.1.306 b, 2.1.307 b), which the producer passes as decimal.
        ref format: NumberFormat => with_format;
        /// The number of the first note, from 0 to 65535 in Word (MS-OI29500 2.1.308 a).
        copy start: u16 => with_start;
        /// When the numbers start again.
        copy restart: NoteRestart => with_restart;
    }
}

impl NoteProps {
    /// Footnotes as a document has them when it says nothing: at the bottom of the
    /// page, numbered 1, 2, 3 through the document.
    pub fn footnotes() -> Self {
        Self {
            position: NotePosition::PageBottom,
            format: NumberFormat::Decimal,
            start: 1,
            restart: NoteRestart::Continuous,
        }
    }

    /// Endnotes as a document has them when it says nothing: at the end of the
    /// document, numbered i, ii, iii, Word's default (MS-OI29500 2.1.306 a).
    pub fn endnotes() -> Self {
        Self {
            position: NotePosition::DocumentEnd,
            format: NumberFormat::LowerRoman,
            start: 1,
            restart: NoteRestart::Continuous,
        }
    }
}

/// Where the notes of a section go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NotePosition {
    /// At the bottom of the page of their reference.
    PageBottom,
    /// Below the last line of text on the page of their reference.
    BeneathText,
    /// At the end of the section.
    SectionEnd,
    /// At the end of the document.
    DocumentEnd,
}

/// When the numbers of notes start again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NoteRestart {
    /// Never: they run on through the document.
    #[default]
    Continuous,
    /// With each section.
    EachSection,
    /// With each page.
    EachPage,
}

ir_struct! {
    /// The lines that separate the notes of a kind from the text above them, from the
    /// document's special notes. Empty content draws nothing; a producer gives a
    /// document without them Word's separator, a paragraph holding
    /// [`Inline::separator`](super::Inline::separator).
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct NoteSeparators {
        /// Above the notes on a page.
        vec separator: Block => with_separator;
        /// Above notes continued from the previous page.
        vec continuation_separator: Block => with_continuation_separator;
        /// Below notes that continue on the next page.
        vec continuation_notice: Block => with_continuation_notice;
    }
}

no_rules!(NoteKind, NotePosition, NoteRestart);
