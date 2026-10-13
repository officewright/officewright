//! Formatting in effect: what Word shows once the style hierarchy, the document defaults
//! and the theme are resolved (ECMA-376 Part 1, §17.7.2), as opposed to the properties
//! written at one level, which [`RunProperties`](super::RunProperties) gives.

use officewright_core::Pt;

use super::properties::{
    ColorValue, FontHint, Highlight, Shading, Toggle, UnderlineStyle, VerticalPosition,
};

/// The font families of a run in effect, by the kind of character, with theme fonts
/// resolved to the theme's typefaces. Which of them a character takes is Word's rule
/// (MS-OI29500 §17.3.2.26(b)), which the layout applies.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FontFamilies {
    pub(crate) ascii: Box<str>,
    pub(crate) high_ansi: Box<str>,
    pub(crate) east_asia: Box<str>,
    pub(crate) complex_script: Box<str>,
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
    pub(crate) style: Option<Box<str>>,
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
    pub(crate) language_latin: Option<Box<str>>,
    pub(crate) language_east_asia: Option<Box<str>>,
    pub(crate) language_bidi: Option<Box<str>>,
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

    /// The shading behind the text, as written at the level that gives it.
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
}
