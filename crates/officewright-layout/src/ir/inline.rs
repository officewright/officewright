//! The text and objects inside a paragraph.

use super::image::{AnchoredObject, InlineObject};
use super::macros::{ir_struct, no_rules};
use super::section::NumberFormat;
use super::walk::{Check, Checker, Clamp, Clamper};
use super::{NoteId, RunStyleId};
use crate::Result;

/// A piece of a paragraph's content, in reading order.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Inline {
    /// Text in one run style. It holds no control characters: tabs and breaks are
    /// inlines of their own. A soft hyphen (U+00AD) in the text works as
    /// [`SoftHyphen`](Self::SoftHyphen) does.
    #[non_exhaustive]
    Text {
        /// The text.
        text: String,
        /// Its formatting.
        style: RunStyleId,
    },
    /// A tab character, which moves the text after it to the next tab stop.
    #[non_exhaustive]
    Tab {
        /// The formatting, which the tab's leader is drawn with.
        style: RunStyleId,
    },
    /// A break: the end of a line, page or column.
    #[non_exhaustive]
    Break {
        /// What it ends.
        kind: BreakKind,
        /// The formatting, which gives the height of a line that holds only the break.
        style: RunStyleId,
    },
    /// A field: a value Word computes, such as the page number.
    Field(Field),
    /// A symbol from a font, by its code in that font (`w:sym`).
    #[non_exhaustive]
    Symbol {
        /// The font, such as `Wingdings`.
        font: String,
        /// The character code in the font, as written: symbol fonts use U+F020 to
        /// U+F0FF.
        code: u16,
        /// The formatting.
        style: RunStyleId,
    },
    /// A place where a word may be hyphenated: invisible unless the line breaks there,
    /// when it shows as a hyphen.
    #[non_exhaustive]
    SoftHyphen {
        /// The formatting of the hyphen.
        style: RunStyleId,
    },
    /// A hyphen that the line does not break at.
    #[non_exhaustive]
    NoBreakHyphen {
        /// The formatting.
        style: RunStyleId,
    },
    /// A drawing object in the line, such as a picture, which sits on the baseline like a
    /// large character.
    Object(InlineObject),
    /// A floating object anchored here. It takes no room in the line.
    Anchored(Box<AnchoredObject>),
    /// The reference to a footnote or an endnote: its number in the text. Word ignores
    /// one in a comment or a text box (MS-OI29500 2.1.305 a); one in a header or footer is
    /// laid out as in the body until a Word reference render shows otherwise.
    #[non_exhaustive]
    NoteReference {
        /// The note.
        note: NoteId,
        /// Whether the text that follows is the mark, in place of the number
        /// (`w:customMarkFollows`).
        custom_mark: bool,
        /// The formatting of the number.
        style: RunStyleId,
    },
    /// The number of the note this content belongs to, at the start of a note
    /// (`w:footnoteRef`, `w:endnoteRef`). Outside a note it shows nothing.
    #[non_exhaustive]
    NoteNumber {
        /// The formatting.
        style: RunStyleId,
    },
    /// The short line above the notes on a page (`w:separator`).
    #[non_exhaustive]
    Separator {
        /// The formatting.
        style: RunStyleId,
    },
    /// The full-width line above notes continued from the previous page
    /// (`w:continuationSeparator`).
    #[non_exhaustive]
    ContinuationSeparator {
        /// The formatting.
        style: RunStyleId,
    },
}

impl Inline {
    /// Text in a run style.
    pub fn text(text: impl Into<String>, style: RunStyleId) -> Self {
        Self::Text {
            text: text.into(),
            style,
        }
    }

    /// A tab character.
    pub fn tab(style: RunStyleId) -> Self {
        Self::Tab { style }
    }

    /// A line break: the text after it starts a new line of the same paragraph, below
    /// the floating objects `clear` names.
    pub fn line_break(clear: BreakClear, style: RunStyleId) -> Self {
        Self::Break {
            kind: BreakKind::Line(clear),
            style,
        }
    }

    /// A page break.
    pub fn page_break(style: RunStyleId) -> Self {
        Self::Break {
            kind: BreakKind::Page,
            style,
        }
    }

    /// A column break.
    pub fn column_break(style: RunStyleId) -> Self {
        Self::Break {
            kind: BreakKind::Column,
            style,
        }
    }

    /// A symbol from a font.
    pub fn symbol(font: impl Into<String>, code: u16, style: RunStyleId) -> Self {
        Self::Symbol {
            font: font.into(),
            code,
            style,
        }
    }

    /// A soft hyphen.
    pub fn soft_hyphen(style: RunStyleId) -> Self {
        Self::SoftHyphen { style }
    }

    /// A non-breaking hyphen.
    pub fn no_break_hyphen(style: RunStyleId) -> Self {
        Self::NoBreakHyphen { style }
    }

    /// A floating object anchored here.
    pub fn anchored(object: AnchoredObject) -> Self {
        Self::Anchored(Box::new(object))
    }

    /// The reference to a note.
    pub fn note_reference(note: NoteId, custom_mark: bool, style: RunStyleId) -> Self {
        Self::NoteReference {
            note,
            custom_mark,
            style,
        }
    }

    /// The number of the note this content belongs to.
    pub fn note_number(style: RunStyleId) -> Self {
        Self::NoteNumber { style }
    }

    /// The line above the notes on a page.
    pub fn separator(style: RunStyleId) -> Self {
        Self::Separator { style }
    }

    /// The line above notes continued from the previous page.
    pub fn continuation_separator(style: RunStyleId) -> Self {
        Self::ContinuationSeparator { style }
    }
}

impl From<InlineObject> for Inline {
    fn from(object: InlineObject) -> Self {
        Self::Object(object)
    }
}

impl From<Field> for Inline {
    fn from(field: Field) -> Self {
        Self::Field(field)
    }
}

impl Check for Inline {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Text { text, style } => {
                if let Some((offset, ch)) = text.char_indices().find(|(_, ch)| ch.is_control()) {
                    return Err(checker.error(format!(
                        "text has the control character U+{:04X} at byte {offset}; tabs and \
                         breaks are inlines of their own",
                        u32::from(ch)
                    )));
                }
                checker.visit("style", style)
            }
            Self::Tab { style }
            | Self::Break { style, .. }
            | Self::Symbol { style, .. }
            | Self::SoftHyphen { style }
            | Self::NoBreakHyphen { style }
            | Self::NoteNumber { style }
            | Self::Separator { style }
            | Self::ContinuationSeparator { style } => checker.visit("style", style),
            Self::Object(object) => object.check(checker),
            Self::Anchored(object) => object.check(checker),
            Self::NoteReference { note, style, .. } => {
                // Notes are laid out where they are referenced, so a reference in a note
                // could make a note contain itself.
                if checker.in_note() {
                    return Err(checker.error("a note reference inside a note or its separators"));
                }
                checker.visit("note", note)?;
                checker.visit("style", style)
            }
            Self::Field(field) => {
                if checker.in_field_result() {
                    return Err(checker.error(
                        "a field inside the result of a field; the result of the outer field \
                         holds the inner one's result as plain inlines",
                    ));
                }
                checker.set_in_field_result(true);
                field.check(checker)?;
                checker.set_in_field_result(false);
                Ok(())
            }
        }
    }
}

// Fields are not walked into, so that a document that was not validated cannot make the
// walk recurse deeply; drawing objects hold no blocks, so walking them is bounded.
impl Clamp for Inline {
    fn clamp(&mut self, clamper: &mut Clamper) {
        match self {
            Self::Object(object) => object.clamp(clamper),
            Self::Anchored(object) => object.clamp(clamper),
            _ => {}
        }
    }
}

/// What a break ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BreakKind {
    /// The line: the text after it starts a new line of the same paragraph. Its
    /// [`BreakClear`] says whether that line also moves below floating objects.
    Line(BreakClear),
    /// The page.
    Page,
    /// The column; on a page of one column, the page.
    Column,
}

/// Below which floating objects the line after a line break starts (`w:clear`). Text
/// does not wrap around floating objects yet, so only [`None`](Self::None) is laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BreakClear {
    /// None: the next line starts right below.
    #[default]
    None,
    /// Those on the left.
    Left,
    /// Those on the right.
    Right,
    /// All of them.
    All,
}

ir_struct! {
    /// A field: the result the document stores for it, and for the fields the layout
    /// computes, what to compute.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Field {
        /// What the field is.
        ref kind: FieldKind => with_kind;
        /// The result the document stores, shown for the fields the layout does not
        /// compute. It holds no fields: the result of a field inside it is part of it as
        /// plain inlines.
        vec result: Inline => with_result;
        /// How a computed number is written, from the field's `\*` switch; `None` takes the
        /// section's page number format.
        ref format: Option<NumberFormat> => with_format;
        /// The formatting of a computed value.
        copy style: RunStyleId => with_style;
    }
}

impl Field {
    /// A field of this kind, with no stored result, written in this style.
    pub fn new(kind: FieldKind, style: RunStyleId) -> Self {
        Self {
            kind,
            result: Vec::new(),
            format: None,
            style,
        }
    }
}

/// What a field is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FieldKind {
    /// `PAGE`: the number of the page.
    Page,
    /// `NUMPAGES`: the number of pages in the document.
    NumPages,
    /// `SECTIONPAGES`: the number of pages in the section.
    SectionPages,
    /// Any other field, by its name, such as `DATE`. It shows its stored result.
    Other(String),
}

no_rules!(BreakKind, BreakClear, FieldKind);
