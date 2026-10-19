//! Formatting in effect: what Word shows once the style hierarchy, the document defaults,
//! numbering and the theme are resolved (ECMA-376 Part 1, §17.7.2), as opposed to the
//! properties written at one level, which [`RunProperties`](super::RunProperties) and
//! [`ParagraphProperties`](super::ParagraphProperties) give.

use officewright_core::Pt;

use std::sync::Arc;

use super::Document;
use super::numbering::ListLevel;
use super::properties::{
    ColorValue, FontHint, Frame, Highlight, Justification, Ligatures, LineSpacing,
    ParagraphBorders, Shading, TabStop, TextEffects, Toggle, UnderlineStyle, VerticalPosition,
};

/// The font families of a run in effect, by the kind of character, with theme fonts
/// resolved to the theme's typefaces. Which of them a character takes is Word's rule
/// (MS-OI29500 §17.3.2.26(b)), which the layout applies.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FontFamilies {
    // Shared with the levels of the hierarchy that give them, and between runs: a name
    // can be as long as the part it is written in.
    pub(crate) ascii: Arc<str>,
    pub(crate) high_ansi: Arc<str>,
    pub(crate) east_asia: Arc<str>,
    pub(crate) complex_script: Arc<str>,
    pub(crate) hint: Option<FontHint>,
}

impl FontFamilies {
    /// For characters U+0000 to U+007F.
    pub fn ascii(&self) -> &str {
        &self.ascii
    }

    /// For other characters that are neither East Asian nor complex script.
    pub fn high_ansi(&self) -> &str {
        &self.high_ansi
    }

    /// For East Asian characters.
    pub fn east_asia(&self) -> &str {
        &self.east_asia
    }

    /// For complex script characters.
    pub fn complex_script(&self) -> &str {
        &self.complex_script
    }

    /// Which font the characters several kinds share take (`w:hint`).
    pub fn hint(&self) -> Option<FontHint> {
        self.hint
    }
}

/// The formatting of a run in effect, as Word shows it: the document defaults, the
/// paragraph's style, the run's character style and its direct formatting combined.
///
/// Tables do not contribute yet: inside a table the formatting a table style gives is
/// left out.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct CharacterFormat {
    pub(crate) style: Option<Arc<str>>,
    pub(crate) fonts: FontFamilies,
    pub(crate) size: Pt,
    pub(crate) size_complex_script: Pt,
    pub(crate) toggles: [bool; Toggle::COUNT],
    pub(crate) color: ColorValue,
    pub(crate) underline: Option<UnderlineStyle>,
    pub(crate) underline_color: ColorValue,
    pub(crate) vertical_position: VerticalPosition,
    pub(crate) position: Pt,
    pub(crate) spacing: Pt,
    pub(crate) scale: u16,
    pub(crate) kerning: Option<Pt>,
    pub(crate) highlight: Option<Highlight>,
    pub(crate) shading: Option<Shading>,
    pub(crate) language_latin: Option<Arc<str>>,
    pub(crate) language_east_asia: Option<Arc<str>>,
    pub(crate) language_bidi: Option<Arc<str>>,
    pub(crate) ligatures: Ligatures,
    pub(crate) contextual_alternates: bool,
    /// Whether the text has an emphasis mark (`w:em`), which the layout does not draw.
    pub(crate) emphasis_mark: bool,
    /// Whether the text has a border around it (`w:bdr`), which the layout does not draw.
    pub(crate) border: bool,
    /// Word 2010's effects of the text, which the layout does not draw.
    pub(crate) text_effects: TextEffects,
}

macro_rules! flags {
    ($($(#[$meta:meta])* $name:ident: $toggle:ident,)*) => {
        $(
            $(#[$meta])*
            pub fn $name(&self) -> bool {
                self.toggles.get(Toggle::$toggle as usize).copied().unwrap_or(false)
            }
        )*
    };
}

impl CharacterFormat {
    /// The character style applied (`w:rStyle`), by its id.
    pub fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }

    /// The font families.
    pub fn fonts(&self) -> &FontFamilies {
        &self.fonts
    }

    /// The font size of text that is not complex script.
    pub fn size(&self) -> Pt {
        self.size
    }

    /// The font size of complex script text.
    pub fn size_complex_script(&self) -> Pt {
        self.size_complex_script
    }

    flags! {
        /// Bold, for text that is not complex script.
        bold: Bold,
        /// Bold, for complex script text.
        bold_complex_script: BoldComplexScript,
        /// Italic, for text that is not complex script.
        italic: Italic,
        /// Italic, for complex script text.
        italic_complex_script: ItalicComplexScript,
        /// All capitals.
        caps: Caps,
        /// Small capitals.
        small_caps: SmallCaps,
        /// Struck through with one line.
        strike: Strike,
        /// Struck through with two lines.
        double_strike: DoubleStrike,
        /// Outlined.
        outline: Outline,
        /// Shadowed.
        shadow: Shadow,
        /// Embossed.
        emboss: Emboss,
        /// Engraved.
        imprint: Imprint,
        /// Not checked for spelling or grammar.
        no_proof: NoProof,
        /// Snaps to the document grid.
        snap_to_grid: SnapToGrid,
        /// Hidden.
        hidden: Vanish,
        /// Hidden in web layout.
        web_hidden: WebHidden,
        /// The paragraph mark is hidden and the paragraph joins the next one in tables of
        /// contents.
        spec_vanish: SpecVanish,
        /// Right-to-left text, which takes the complex script font and properties.
        right_to_left: RightToLeft,
        /// Complex script text: every character takes the complex script font and
        /// properties.
        complex_script: ComplexScript,
        /// Office Math.
        math: Math,
    }

    /// The colour of the text, theme colours resolved to their values.
    /// [`ColorValue::Auto`] leaves the colour to the layout: black, or white on a dark
    /// shading.
    pub fn color(&self) -> ColorValue {
        self.color
    }

    /// The underline; `None` for no underline.
    pub fn underline(&self) -> Option<UnderlineStyle> {
        self.underline
    }

    /// The colour of the underline, theme colours resolved; automatic when nothing in
    /// the hierarchy gives one (MS-OI29500 §17.3.2.40(a)).
    pub fn underline_color(&self) -> ColorValue {
        self.underline_color
    }

    /// Superscript, subscript or the baseline.
    pub fn vertical_position(&self) -> VerticalPosition {
        self.vertical_position
    }

    /// How far the text is raised, or lowered when negative.
    pub fn position(&self) -> Pt {
        self.position
    }

    /// The space added after each character, or taken away when negative.
    pub fn spacing(&self) -> Pt {
        self.spacing
    }

    /// The width of the characters in percent of their normal width.
    pub fn scale(&self) -> u16 {
        self.scale
    }

    /// The font size from which the text is kerned; `None` for no kerning.
    pub fn kerning(&self) -> Option<Pt> {
        self.kerning
    }

    /// The highlight behind the text.
    pub fn highlight(&self) -> Option<Highlight> {
        self.highlight
    }

    /// The shading behind the text, as written at the level that gives it: theme colours
    /// in it are not resolved yet.
    pub fn shading(&self) -> Option<&Shading> {
        self.shading.as_ref()
    }

    /// The language of Latin text.
    pub fn language(&self) -> Option<&str> {
        self.language_latin.as_deref()
    }

    /// The language of East Asian text.
    pub fn language_east_asia(&self) -> Option<&str> {
        self.language_east_asia.as_deref()
    }

    /// The language of complex script text.
    pub fn language_bidi(&self) -> Option<&str> {
        self.language_bidi.as_deref()
    }

    /// The kinds of ligatures the text asks for; none when nothing in the hierarchy says.
    /// When the document defaults give no run properties at all, Word's own defaults
    /// apply, which give standard and contextual ligatures; document defaults that give
    /// run properties but no ligatures give none. Word draws them only when the
    /// document's compatibility setting `enableOpenTypeFeatures` is on ([MS-DOCX] 2.3.3).
    pub fn ligatures(&self) -> Ligatures {
        self.ligatures
    }

    /// Whether the text asks for the font's contextual alternates; off when nothing in the
    /// hierarchy says. Word uses them only when the document's compatibility setting
    /// `enableOpenTypeFeatures` is on ([MS-DOCX] 2.3.3).
    pub fn contextual_alternates(&self) -> bool {
        self.contextual_alternates
    }
}

/// The on/off properties of a paragraph in effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParagraphFlag {
    KeepNext,
    KeepLines,
    PageBreakBefore,
    WidowControl,
    SuppressLineNumbers,
    SuppressAutoHyphens,
    ContextualSpacing,
    MirrorIndents,
    WordWrap,
    RightToLeft,
    SnapToGrid,
}

impl ParagraphFlag {
    pub(crate) const COUNT: usize = 11;
}

/// The formatting of a paragraph in effect, as Word lays it out: the document defaults,
/// the paragraph's style, its list level and its direct formatting combined.
///
/// Tables do not contribute yet: inside a table the formatting a table style gives is
/// left out.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ParagraphFormat {
    pub(crate) style: Option<Arc<str>>,
    pub(crate) justification: Justification,
    pub(crate) start_indent: Pt,
    pub(crate) end_indent: Pt,
    pub(crate) first_line_indent: Pt,
    pub(crate) start_indent_chars: Option<i64>,
    pub(crate) end_indent_chars: Option<i64>,
    pub(crate) first_line_indent_chars: Option<i64>,
    pub(crate) before_spacing: Pt,
    pub(crate) after_spacing: Pt,
    pub(crate) before_spacing_lines: Option<i64>,
    pub(crate) after_spacing_lines: Option<i64>,
    pub(crate) before_auto_spacing: bool,
    pub(crate) after_auto_spacing: bool,
    pub(crate) line_spacing: LineSpacing,
    pub(crate) flags: [bool; ParagraphFlag::COUNT],
    pub(crate) borders: ParagraphBorders,
    pub(crate) shading: Option<Shading>,
    pub(crate) tabs: Arc<[TabStop]>,
    pub(crate) outline_level: Option<u8>,
    pub(crate) list: Option<ListFormat>,
    /// Whether the paragraph is a text frame or a drop cap (`w:framePr`), which the layout
    /// does not place.
    pub(crate) frame: Option<Frame>,
    /// Whether the paragraph's mark is deleted as a tracked change, written on the
    /// paragraph itself.
    pub(crate) mark_deleted: bool,
}

macro_rules! paragraph_flags {
    ($($(#[$meta:meta])* $name:ident: $flag:ident,)*) => {
        $(
            $(#[$meta])*
            pub fn $name(&self) -> bool {
                self.flags.get(ParagraphFlag::$flag as usize).copied().unwrap_or(false)
            }
        )*
    };
}

impl ParagraphFormat {
    /// The paragraph style applied, by its id: the one `w:pStyle` names, or the default
    /// paragraph style.
    pub fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }

    /// The alignment of the lines; [`Justification::Start`] when nothing gives one
    /// (MS-OI29500 §17.3.1.13(a)).
    pub fn justification(&self) -> Justification {
        self.justification
    }

    /// The indent from the start edge of the text area (left in a left-to-right
    /// paragraph). [`start_indent_chars`](Self::start_indent_chars) replaces it when
    /// it is given.
    pub fn start_indent(&self) -> Pt {
        self.start_indent
    }

    /// The indent from the end edge of the text area.
    pub fn end_indent(&self) -> Pt {
        self.end_indent
    }

    /// The indent of the first line from the start indent; negative for a hanging
    /// indent.
    pub fn first_line_indent(&self) -> Pt {
        self.first_line_indent
    }

    /// The start indent in hundredths of a character, when given in character units:
    /// it then replaces [`start_indent`](Self::start_indent), as Word does even when the
    /// indent in points comes from a later level (MS-OI29500 §17.3.1.12(b)).
    pub fn start_indent_chars(&self) -> Option<i64> {
        self.start_indent_chars
    }

    /// The end indent in hundredths of a character, when given in character units.
    pub fn end_indent_chars(&self) -> Option<i64> {
        self.end_indent_chars
    }

    /// The first line indent in hundredths of a character, negative for a hanging
    /// indent, when given in character units.
    pub fn first_line_indent_chars(&self) -> Option<i64> {
        self.first_line_indent_chars
    }

    /// The space above the paragraph.
    pub fn before_spacing(&self) -> Pt {
        self.before_spacing
    }

    /// The space below the paragraph.
    pub fn after_spacing(&self) -> Pt {
        self.after_spacing
    }

    /// The space above in hundredths of a line, when given in line units: it then
    /// replaces [`before_spacing`](Self::before_spacing) (MS-OI29500 §17.3.1.33(a)).
    pub fn before_spacing_lines(&self) -> Option<i64> {
        self.before_spacing_lines
    }

    /// The space below in hundredths of a line, when given in line units.
    pub fn after_spacing_lines(&self) -> Option<i64> {
        self.after_spacing_lines
    }

    /// The space above is chosen by the application, as HTML spaces paragraphs
    /// (`w:beforeAutospacing`).
    pub fn before_auto_spacing(&self) -> bool {
        self.before_auto_spacing
    }

    /// The space below is chosen by the application (`w:afterAutospacing`).
    pub fn after_auto_spacing(&self) -> bool {
        self.after_auto_spacing
    }

    /// The height of the lines; single when nothing gives one.
    pub fn line_spacing(&self) -> LineSpacing {
        self.line_spacing
    }

    paragraph_flags! {
        /// Kept on the page of the next paragraph.
        keep_next: KeepNext,
        /// All lines kept on one page.
        keep_lines: KeepLines,
        /// Starts a new page.
        page_break_before: PageBreakBefore,
        /// A single first or last line is not left alone on a page; on when nothing
        /// says otherwise (ECMA-376 §17.3.1.44).
        widow_control: WidowControl,
        /// Left out of line numbering.
        suppress_line_numbers: SuppressLineNumbers,
        /// Not hyphenated automatically.
        suppress_auto_hyphens: SuppressAutoHyphens,
        /// The space above and below is left out between paragraphs of the same style.
        contextual_spacing: ContextualSpacing,
        /// The start and end indents swap on even pages.
        mirror_indents: MirrorIndents,
        /// Latin words wrap whole; off lets a word break at any character. On when
        /// nothing says otherwise (ECMA-376 §17.3.1.45).
        word_wrap: WordWrap,
        /// A right-to-left paragraph.
        right_to_left: RightToLeft,
        /// The lines snap to the document grid when it has one; on when nothing says
        /// otherwise (ECMA-376 §17.3.1.32).
        snap_to_grid: SnapToGrid,
    }

    /// The borders, each side from the level that gives it.
    pub fn borders(&self) -> &ParagraphBorders {
        &self.borders
    }

    /// The shading behind the paragraph, as written at the level that gives it: theme
    /// colours in it are not resolved yet.
    pub fn shading(&self) -> Option<&Shading> {
        self.shading.as_ref()
    }

    /// The custom tab stops by position, those of every level added up, a stop of kind
    /// [`TabKind::Clear`](super::TabKind::Clear) removing the one a lower level set at
    /// its position (ECMA-376 §17.3.1.38). A hanging indent makes a stop too, which the
    /// layout adds.
    ///
    /// At most 64, those at the lowest positions; each level gives at most 64 set and 64
    /// cleared: Word's own format adds at most 64 stops and removes at most 64 in one
    /// change ([MS-DOC] 2.9.179, 2.9.180).
    pub fn tabs(&self) -> &[TabStop] {
        &self.tabs
    }

    /// The outline level, 0 for the top, for tables of contents; `None` for body text.
    /// Word takes it from the style id for Heading1 to Heading9 (MS-OI29500
    /// §17.3.1.20(a)); a style based on one of them has the `w:outlineLvl` it inherits
    /// like any other property, not the level of its base's id.
    pub fn outline_level(&self) -> Option<u8> {
        self.outline_level
    }

    /// The list the paragraph is an item of, with its level.
    pub fn list(&self) -> Option<&ListFormat> {
        self.list.as_ref()
    }
}

/// The list a paragraph is an item of: the list instance, the definition whose levels
/// apply and the level, with the instance's overrides applied.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ListFormat {
    pub(crate) instance: i64,
    pub(crate) definition: Option<i64>,
    pub(crate) level_number: usize,
    pub(crate) start: i64,
}

impl ListFormat {
    /// The list instance (`w:numId`).
    pub fn instance(&self) -> i64 {
        self.instance
    }

    /// The definition whose levels apply (`w:abstractNumId`), through a numbering
    /// style's link (`w:numStyleLink`) when the instance's definition has one.
    pub fn definition(&self) -> Option<i64> {
        self.definition
    }

    /// The level, 0 to 8.
    pub fn level_number(&self) -> usize {
        self.level_number
    }

    /// The level's definition, an override's when the instance has one; `None` when
    /// the definition has no such level. `document` is the document of the paragraph:
    /// the level is parsed from it on each call, since a document can name many.
    pub fn level(&self, document: &Document) -> Option<ListLevel> {
        super::resolve::list_level(document, self)
    }

    /// The number the level starts from: the instance's override (`w:startOverride`),
    /// else the level's `w:start`, else 0 (ECMA-376 §17.9.25).
    pub fn start(&self) -> i64 {
        self.start
    }
}
