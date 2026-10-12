//! Formatting as written in a document: the properties a run, a paragraph or a style sets
//! at its own level. A property the element does not set is `None`; what is in effect
//! comes from resolving the style hierarchy, which this layer does not do.

use officewright_core::Pt;

/// Defines a public enum of the values of a simple type of WordprocessingML, with each
/// variant documented by the value it is written as, and the function that reads it.
macro_rules! xml_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $value:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum $name {
            $(
                #[doc = concat!("`", $value, "`")]
                $variant,
            )*
        }

        impl $name {
            /// The value written as `value`, if it is one.
            pub(crate) fn from_xml(value: &str) -> Option<Self> {
                match value {
                    $($value => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

xml_enum! {
    /// A font of the document's theme (ST_Theme), which a
    /// run names instead of a font family.
    ThemeFont {
        MajorEastAsia = "majorEastAsia",
        MajorBidi = "majorBidi",
        MajorAscii = "majorAscii",
        MajorHAnsi = "majorHAnsi",
        MinorEastAsia = "minorEastAsia",
        MinorBidi = "minorBidi",
        MinorAscii = "minorAscii",
        MinorHAnsi = "minorHAnsi",
    }
}

xml_enum! {
    /// Which of a run's fonts characters shared by several scripts use
    /// (ST_Hint).
    FontHint {
        Default = "default",
        EastAsia = "eastAsia",
        ComplexScript = "cs",
    }
}

xml_enum! {
    /// A colour of the document's theme (ST_ThemeColor).
    ThemeColor {
        Dark1 = "dark1",
        Light1 = "light1",
        Dark2 = "dark2",
        Light2 = "light2",
        Accent1 = "accent1",
        Accent2 = "accent2",
        Accent3 = "accent3",
        Accent4 = "accent4",
        Accent5 = "accent5",
        Accent6 = "accent6",
        Hyperlink = "hyperlink",
        FollowedHyperlink = "followedHyperlink",
        None = "none",
        Background1 = "background1",
        Text1 = "text1",
        Background2 = "background2",
        Text2 = "text2",
    }
}

xml_enum! {
    /// How text is underlined (ST_Underline).
    UnderlineStyle {
        Single = "single",
        Words = "words",
        Double = "double",
        Thick = "thick",
        Dotted = "dotted",
        DottedHeavy = "dottedHeavy",
        Dash = "dash",
        DashedHeavy = "dashedHeavy",
        DashLong = "dashLong",
        DashLongHeavy = "dashLongHeavy",
        DotDash = "dotDash",
        DashDotHeavy = "dashDotHeavy",
        DotDotDash = "dotDotDash",
        DashDotDotHeavy = "dashDotDotHeavy",
        Wave = "wave",
        WavyHeavy = "wavyHeavy",
        WavyDouble = "wavyDouble",
        None = "none",
    }
}

xml_enum! {
    /// Whether text is raised or lowered as a superscript or subscript
    /// (ST_VerticalAlignRun).
    VerticalPosition {
        Baseline = "baseline",
        Superscript = "superscript",
        Subscript = "subscript",
    }
}

xml_enum! {
    /// The colour text is highlighted with (ST_HighlightColor).
    Highlight {
        Black = "black",
        Blue = "blue",
        Cyan = "cyan",
        Green = "green",
        Magenta = "magenta",
        Red = "red",
        Yellow = "yellow",
        White = "white",
        DarkBlue = "darkBlue",
        DarkCyan = "darkCyan",
        DarkGreen = "darkGreen",
        DarkMagenta = "darkMagenta",
        DarkRed = "darkRed",
        DarkYellow = "darkYellow",
        DarkGray = "darkGray",
        LightGray = "lightGray",
        None = "none",
    }
}

xml_enum! {
    /// The pattern of a shading, laid in its colour over its fill (ST_Shd).
    ShadingPattern {
        Clear = "clear",
        Solid = "solid",
        HorizontalStripe = "horzStripe",
        VerticalStripe = "vertStripe",
        ReverseDiagonalStripe = "reverseDiagStripe",
        DiagonalStripe = "diagStripe",
        HorizontalCross = "horzCross",
        DiagonalCross = "diagCross",
        ThinHorizontalStripe = "thinHorzStripe",
        ThinVerticalStripe = "thinVertStripe",
        ThinReverseDiagonalStripe = "thinReverseDiagStripe",
        ThinDiagonalStripe = "thinDiagStripe",
        ThinHorizontalCross = "thinHorzCross",
        ThinDiagonalCross = "thinDiagCross",
        Percent5 = "pct5",
        Percent10 = "pct10",
        Percent12 = "pct12",
        Percent15 = "pct15",
        Percent20 = "pct20",
        Percent25 = "pct25",
        Percent30 = "pct30",
        Percent35 = "pct35",
        Percent37 = "pct37",
        Percent40 = "pct40",
        Percent45 = "pct45",
        Percent50 = "pct50",
        Percent55 = "pct55",
        Percent60 = "pct60",
        Percent62 = "pct62",
        Percent65 = "pct65",
        Percent70 = "pct70",
        Percent75 = "pct75",
        Percent80 = "pct80",
        Percent85 = "pct85",
        Percent87 = "pct87",
        Percent90 = "pct90",
        Percent95 = "pct95",
        Nil = "nil",
    }
}

/// A colour as written: a value, a theme colour, or both, the theme colour then taking
/// precedence (ECMA-376 Part 1, §17.3.2.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub(crate) value: Option<ColorValue>,
    pub(crate) theme: Option<ThemeColor>,
    pub(crate) tint: Option<u8>,
    pub(crate) shade: Option<u8>,
}

impl Color {
    /// The colour value (`w:val`, `w:color` or `w:fill`).
    pub fn value(&self) -> Option<ColorValue> {
        self.value
    }

    /// The theme colour.
    pub fn theme(&self) -> Option<ThemeColor> {
        self.theme
    }

    /// The tint applied to the theme colour, 0 to 255 (255: none).
    pub fn tint(&self) -> Option<u8> {
        self.tint
    }

    /// The shade applied to the theme colour, 0 to 255 (255: none).
    pub fn shade(&self) -> Option<u8> {
        self.shade
    }

    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A colour value (ST_HexColor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColorValue {
    /// Chosen by the application: for text, black, or white on a dark background.
    Auto,
    /// Red, green and blue.
    Rgb(u8, u8, u8),
}

/// An underline: its style and colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Underline {
    pub(crate) style: Option<UnderlineStyle>,
    pub(crate) color: Color,
}

impl Underline {
    /// The style; `None` when the element gives none.
    pub fn style(&self) -> Option<UnderlineStyle> {
        self.style
    }

    /// The colour, as written.
    pub fn color(&self) -> &Color {
        &self.color
    }
}

/// A shading: a pattern in a colour over a fill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shading {
    pub(crate) pattern: Option<ShadingPattern>,
    pub(crate) color: Color,
    pub(crate) fill: Color,
}

impl Shading {
    /// The pattern.
    pub fn pattern(&self) -> Option<ShadingPattern> {
        self.pattern
    }

    /// The colour of the pattern.
    pub fn color(&self) -> &Color {
        &self.color
    }

    /// The fill under the pattern.
    pub fn fill(&self) -> &Color {
        &self.fill
    }
}

/// The fonts a run names, by the script of the characters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunFonts {
    pub(crate) ascii: Option<String>,
    pub(crate) high_ansi: Option<String>,
    pub(crate) east_asia: Option<String>,
    pub(crate) complex_script: Option<String>,
    pub(crate) ascii_theme: Option<ThemeFont>,
    pub(crate) high_ansi_theme: Option<ThemeFont>,
    pub(crate) east_asia_theme: Option<ThemeFont>,
    pub(crate) complex_script_theme: Option<ThemeFont>,
    pub(crate) hint: Option<FontHint>,
}

impl RunFonts {
    /// The font for characters U+0000–U+007F (`w:ascii`).
    pub fn ascii(&self) -> Option<&str> {
        self.ascii.as_deref()
    }

    /// The font for other characters that are not East Asian or complex script
    /// (`w:hAnsi`).
    pub fn high_ansi(&self) -> Option<&str> {
        self.high_ansi.as_deref()
    }

    /// The font for East Asian characters (`w:eastAsia`).
    pub fn east_asia(&self) -> Option<&str> {
        self.east_asia.as_deref()
    }

    /// The font for complex script characters, such as Arabic or Hebrew (`w:cs`).
    pub fn complex_script(&self) -> Option<&str> {
        self.complex_script.as_deref()
    }

    /// The theme font for ASCII characters, which wins over [`ascii`](Self::ascii).
    pub fn ascii_theme(&self) -> Option<ThemeFont> {
        self.ascii_theme
    }

    /// The theme font for [`high_ansi`](Self::high_ansi) characters.
    pub fn high_ansi_theme(&self) -> Option<ThemeFont> {
        self.high_ansi_theme
    }

    /// The theme font for East Asian characters.
    pub fn east_asia_theme(&self) -> Option<ThemeFont> {
        self.east_asia_theme
    }

    /// The theme font for complex script characters.
    pub fn complex_script_theme(&self) -> Option<ThemeFont> {
        self.complex_script_theme
    }

    /// Which font characters that several scripts share use.
    pub fn hint(&self) -> Option<FontHint> {
        self.hint
    }
}

/// The languages of a run's text, as BCP 47 tags such as `en-US`, by script.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Language {
    pub(crate) latin: Option<String>,
    pub(crate) east_asia: Option<String>,
    pub(crate) bidi: Option<String>,
}

impl Language {
    /// The language of Latin text (`w:val`).
    pub fn latin(&self) -> Option<&str> {
        self.latin.as_deref()
    }

    /// The language of East Asian text.
    pub fn east_asia(&self) -> Option<&str> {
        self.east_asia.as_deref()
    }

    /// The language of complex script text.
    pub fn bidi(&self) -> Option<&str> {
        self.bidi.as_deref()
    }
}

/// The character properties written on a run or in a style (`w:rPr`, ECMA-376 Part 1,
/// §17.3.2): only what is set there. A property that is not set is `None`; what is in
/// effect comes from the styles and the document defaults.
///
/// Properties officewright does not type yet (text effects, emphasis marks, East Asian
/// layout, borders around runs, …) are kept with the document as written.
#[doc(alias = "CharacterFormat")]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunProperties {
    pub(crate) style: Option<String>,
    pub(crate) fonts: RunFonts,
    pub(crate) toggles: [Option<bool>; Toggle::COUNT],
    pub(crate) color: Option<Color>,
    pub(crate) underline: Option<Underline>,
    pub(crate) size: Option<Pt>,
    pub(crate) size_complex_script: Option<Pt>,
    pub(crate) vertical_position: Option<VerticalPosition>,
    pub(crate) position: Option<Pt>,
    pub(crate) spacing: Option<Pt>,
    pub(crate) scale: Option<u16>,
    pub(crate) kerning: Option<Pt>,
    pub(crate) highlight: Option<Highlight>,
    pub(crate) shading: Option<Shading>,
    pub(crate) language: Language,
}

/// The on/off properties of a run, by the element that sets them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Toggle {
    Bold,
    BoldComplexScript,
    Italic,
    ItalicComplexScript,
    Caps,
    SmallCaps,
    Strike,
    DoubleStrike,
    Outline,
    Shadow,
    Emboss,
    Imprint,
    NoProof,
    SnapToGrid,
    Vanish,
    WebHidden,
    SpecVanish,
    RightToLeft,
    ComplexScript,
    Math,
}

impl Toggle {
    pub(crate) const COUNT: usize = 20;

    pub(crate) const ALL: [(Self, &'static str); Self::COUNT] = [
        (Self::Bold, "b"),
        (Self::BoldComplexScript, "bCs"),
        (Self::Italic, "i"),
        (Self::ItalicComplexScript, "iCs"),
        (Self::Caps, "caps"),
        (Self::SmallCaps, "smallCaps"),
        (Self::Strike, "strike"),
        (Self::DoubleStrike, "dstrike"),
        (Self::Outline, "outline"),
        (Self::Shadow, "shadow"),
        (Self::Emboss, "emboss"),
        (Self::Imprint, "imprint"),
        (Self::NoProof, "noProof"),
        (Self::SnapToGrid, "snapToGrid"),
        (Self::Vanish, "vanish"),
        (Self::WebHidden, "webHidden"),
        (Self::SpecVanish, "specVanish"),
        (Self::RightToLeft, "rtl"),
        (Self::ComplexScript, "cs"),
        (Self::Math, "oMath"),
    ];
}

/// Getters of the on/off properties.
macro_rules! toggles {
    ($($(#[$meta:meta])* $name:ident: $toggle:ident,)*) => {
        $(
            $(#[$meta])*
            pub fn $name(&self) -> Option<bool> {
                self.toggles.get(Toggle::$toggle as usize).copied().flatten()
            }
        )*
    };
}

impl RunProperties {
    /// The character style (`w:rStyle`), by its id.
    pub fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }

    /// The fonts (`w:rFonts`).
    pub fn fonts(&self) -> &RunFonts {
        &self.fonts
    }

    toggles! {
        /// Bold (`w:b`).
        bold: Bold,
        /// Bold for complex script text (`w:bCs`).
        bold_complex_script: BoldComplexScript,
        /// Italic (`w:i`).
        italic: Italic,
        /// Italic for complex script text (`w:iCs`).
        italic_complex_script: ItalicComplexScript,
        /// All capitals (`w:caps`).
        caps: Caps,
        /// Small capitals (`w:smallCaps`).
        small_caps: SmallCaps,
        /// Struck through with one line (`w:strike`).
        strike: Strike,
        /// Struck through with two lines (`w:dstrike`).
        double_strike: DoubleStrike,
        /// Outlined (`w:outline`).
        outline: Outline,
        /// Shadowed (`w:shadow`).
        shadow: Shadow,
        /// Embossed (`w:emboss`).
        emboss: Emboss,
        /// Engraved (`w:imprint`).
        imprint: Imprint,
        /// Not checked for spelling or grammar (`w:noProof`).
        no_proof: NoProof,
        /// Laid out on the document grid when it has one (`w:snapToGrid`).
        snap_to_grid: SnapToGrid,
        /// Hidden (`w:vanish`).
        vanish: Vanish,
        /// Hidden in web layout (`w:webHidden`).
        web_hidden: WebHidden,
        /// The paragraph mark is hidden and the paragraph joins the next one in tables of
        /// contents (`w:specVanish`).
        spec_vanish: SpecVanish,
        /// Right-to-left text (`w:rtl`).
        right_to_left: RightToLeft,
        /// Laid out as complex script text, with its fonts and sizes (`w:cs`).
        complex_script: ComplexScript,
        /// Part of an equation (`w:oMath`).
        math: Math,
    }

    /// The text colour (`w:color`).
    pub fn color(&self) -> Option<&Color> {
        self.color.as_ref()
    }

    /// The underline (`w:u`).
    pub fn underline(&self) -> Option<&Underline> {
        self.underline.as_ref()
    }

    /// The font size (`w:sz`).
    pub fn size(&self) -> Option<Pt> {
        self.size
    }

    /// The font size for complex script text (`w:szCs`).
    pub fn size_complex_script(&self) -> Option<Pt> {
        self.size_complex_script
    }

    /// Superscript or subscript (`w:vertAlign`).
    pub fn vertical_position(&self) -> Option<VerticalPosition> {
        self.vertical_position
    }

    /// How far the text is raised (positive) or lowered (`w:position`).
    pub fn position(&self) -> Option<Pt> {
        self.position
    }

    /// Space added after each character, or taken away when negative (`w:spacing`).
    pub fn spacing(&self) -> Option<Pt> {
        self.spacing
    }

    /// The width of the characters in percent of their normal width (`w:w`).
    pub fn scale(&self) -> Option<u16> {
        self.scale
    }

    /// The smallest font size kerned (`w:kern`).
    pub fn kerning(&self) -> Option<Pt> {
        self.kerning
    }

    /// The highlight (`w:highlight`).
    pub fn highlight(&self) -> Option<Highlight> {
        self.highlight
    }

    /// The shading behind the text (`w:shd`).
    pub fn shading(&self) -> Option<&Shading> {
        self.shading.as_ref()
    }

    /// The languages of the text (`w:lang`).
    pub fn language(&self) -> &Language {
        &self.language
    }
}
