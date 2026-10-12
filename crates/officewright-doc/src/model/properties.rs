//! Formatting as written in a document: the properties a run, a paragraph or a style sets
//! at its own level. A property the element does not set is `None`; what is in effect
//! comes from resolving the style hierarchy, which this layer does not do.
//!
//! Values are checked against their type in the schema: one that does not fit it is read
//! as not set, with a diagnostic. Ranges that only the prose of ECMA-376 gives (list
//! levels 0 to 8, say) are not checked here; a value outside them is kept as written,
//! for the resolution step to treat as Word does.

use officewright_core::Pt;

/// Defines a public enum of the values of a simple type of WordprocessingML, with each
/// variant documented by the value it is written as, and the function that reads it.
macro_rules! xml_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($(#[$variant_meta:meta])* $variant:ident = $value:literal,)* }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum $name {
            $(
                #[doc = concat!("`", $value, "`")]
                $(#[$variant_meta])*
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
        /// No theme colour.
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
        /// Under words only, not under the spaces between them.
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
        /// No underline.
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
        /// No highlight.
        None = "none",
    }
}

xml_enum! {
    /// The pattern of a shading, laid in its colour over its fill (ST_Shd).
    ShadingPattern {
        /// No pattern: the fill alone.
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
        /// No shading at all, not even a fill.
        Nil = "nil",
    }
}

/// A colour as written: a value, a theme colour, or both, the theme colour then taking
/// precedence (ECMA-376 Part 1, §17.3.2.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Color {
    pub(crate) value: Option<ColorValue>,
    pub(crate) theme: Option<ThemeColor>,
    pub(crate) tint: Option<u8>,
    pub(crate) shade: Option<u8>,
}

impl Color {
    /// The colour value (`w:val`, `w:color` or `w:fill`). Word writes theme colours
    /// without one (LibreOffice's `tdf149200.docx`); that is not damage.
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
#[non_exhaustive]
pub struct Underline {
    pub(crate) style: Option<UnderlineStyle>,
    pub(crate) color: Color,
}

impl Underline {
    /// The style; `None` when the element gives none. Word writes underlines with only
    /// a colour (POI's `57312.docx`): the style then comes from the styles below, the
    /// colour from here.
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
#[non_exhaustive]
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
#[non_exhaustive]
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
#[non_exhaustive]
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
#[non_exhaustive]
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

    /// The font size (`w:sz`). Word writes `0` in some linked character styles; it is
    /// not a size to lay text out with.
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

    /// The width of the characters in percent of their normal width, 1 to 600 (`w:w`).
    pub fn scale(&self) -> Option<u16> {
        self.scale
    }

    /// The smallest font size kerned (`w:kern`); `0`, which Word writes often, means no
    /// kerning.
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

xml_enum! {
    /// How the lines of a paragraph are aligned (ST_Jc). Transitional documents write
    /// `left` and `right` where Strict ones write `start` and `end`; both are kept as
    /// written.
    Justification {
        Start = "start",
        Center = "center",
        End = "end",
        Both = "both",
        MediumKashida = "mediumKashida",
        Distribute = "distribute",
        NumberTab = "numTab",
        HighKashida = "highKashida",
        LowKashida = "lowKashida",
        ThaiDistribute = "thaiDistribute",
        Left = "left",
        Right = "right",
    }
}

xml_enum! {
    /// What a tab stop does to the text after it (ST_TabJc).
    TabKind {
        Clear = "clear",
        Start = "start",
        Center = "center",
        End = "end",
        Decimal = "decimal",
        Bar = "bar",
        Number = "num",
        Left = "left",
        Right = "right",
    }
}

xml_enum! {
    /// What fills the space before a tab stop (ST_TabTlc).
    TabLeader {
        None = "none",
        Dot = "dot",
        Hyphen = "hyphen",
        Underscore = "underscore",
        Heavy = "heavy",
        MiddleDot = "middleDot",
    }
}

xml_enum! {
    /// The line style of a border (ST_Border). The art borders of pages are not read
    /// yet: a border that names one reads as having no style.
    BorderStyle {
        Nil = "nil",
        None = "none",
        Single = "single",
        Thick = "thick",
        Double = "double",
        Dotted = "dotted",
        Dashed = "dashed",
        DotDash = "dotDash",
        DotDotDash = "dotDotDash",
        Triple = "triple",
        ThinThickSmallGap = "thinThickSmallGap",
        ThickThinSmallGap = "thickThinSmallGap",
        ThinThickThinSmallGap = "thinThickThinSmallGap",
        ThinThickMediumGap = "thinThickMediumGap",
        ThickThinMediumGap = "thickThinMediumGap",
        ThinThickThinMediumGap = "thinThickThinMediumGap",
        ThinThickLargeGap = "thinThickLargeGap",
        ThickThinLargeGap = "thickThinLargeGap",
        ThinThickThinLargeGap = "thinThickThinLargeGap",
        Wave = "wave",
        DoubleWave = "doubleWave",
        DashSmallGap = "dashSmallGap",
        DashDotStroked = "dashDotStroked",
        ThreeDEmboss = "threeDEmboss",
        ThreeDEngrave = "threeDEngrave",
        Outset = "outset",
        Inset = "inset",
    }
}

/// One border: its line style, colour, width and distance from the text.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Border {
    pub(crate) style: Option<BorderStyle>,
    pub(crate) color: Color,
    pub(crate) width: Option<Pt>,
    pub(crate) space: Option<Pt>,
    pub(crate) shadow: Option<bool>,
    pub(crate) frame: Option<bool>,
}

impl Border {
    /// The line style (`w:val`).
    pub fn style(&self) -> Option<BorderStyle> {
        self.style
    }

    /// The colour.
    pub fn color(&self) -> &Color {
        &self.color
    }

    /// The width of the line (`w:sz`, eighths of a point).
    pub fn width(&self) -> Option<Pt> {
        self.width
    }

    /// The distance between the border and the text (`w:space`, points).
    pub fn space(&self) -> Option<Pt> {
        self.space
    }

    /// Whether the border has a shadow (`w:shadow`).
    pub fn shadow(&self) -> Option<bool> {
        self.shadow
    }

    /// Whether the border looks raised or engraved (`w:frame`).
    pub fn frame(&self) -> Option<bool> {
        self.frame
    }
}

/// The borders of a paragraph (`w:pBdr`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParagraphBorders {
    pub(crate) top: Option<Border>,
    pub(crate) bottom: Option<Border>,
    pub(crate) start: Option<Border>,
    pub(crate) end: Option<Border>,
    pub(crate) between: Option<Border>,
    pub(crate) bar: Option<Border>,
}

impl ParagraphBorders {
    /// Above the paragraph.
    pub fn top(&self) -> Option<&Border> {
        self.top.as_ref()
    }

    /// Below the paragraph.
    pub fn bottom(&self) -> Option<&Border> {
        self.bottom.as_ref()
    }

    /// At the start of the lines (`w:left` in Transitional documents, `w:start` in
    /// Strict ones).
    pub fn start(&self) -> Option<&Border> {
        self.start.as_ref()
    }

    /// At the end of the lines (`w:right` or `w:end`).
    pub fn end(&self) -> Option<&Border> {
        self.end.as_ref()
    }

    /// Between this paragraph and the next one with the same borders.
    pub fn between(&self) -> Option<&Border> {
        self.between.as_ref()
    }

    /// A vertical bar beside the paragraph (`w:bar`).
    pub fn bar(&self) -> Option<&Border> {
        self.bar.as_ref()
    }
}

/// A tab stop (`w:tab` in `w:tabs`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabStop {
    pub(crate) kind: Option<TabKind>,
    pub(crate) leader: Option<TabLeader>,
    pub(crate) position: Pt,
}

impl TabStop {
    /// What the stop does; [`TabKind::Clear`] removes a stop a style set. `None` when
    /// `w:val` is absent or not valid.
    pub fn kind(&self) -> Option<TabKind> {
        self.kind
    }

    /// What fills the space before the stop.
    pub fn leader(&self) -> Option<TabLeader> {
        self.leader
    }

    /// Where the stop is, from the start edge of the text.
    pub fn position(&self) -> Pt {
        self.position
    }
}

/// The spacing between lines.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
///
/// The value is kept as written, sign included: `w:line` may be negative by the schema,
/// and some documents have it so (LibreOffice's `tdf125469_singleSpacing.docx`). What
/// Word makes of a negative value is to be measured.
pub enum LineSpacing {
    /// A multiple of the line height the fonts ask for: 1.0 is single.
    Multiple(f64),
    /// Exactly this line height.
    Exact(Pt),
    /// At least this line height.
    AtLeast(Pt),
}

/// The space around and between the lines of a paragraph (`w:spacing`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spacing {
    pub(crate) before: Option<Pt>,
    pub(crate) after: Option<Pt>,
    pub(crate) before_lines: Option<i64>,
    pub(crate) after_lines: Option<i64>,
    pub(crate) before_auto: Option<bool>,
    pub(crate) after_auto: Option<bool>,
    pub(crate) line: Option<LineSpacing>,
}

impl Spacing {
    /// Space above the paragraph (`w:before`).
    pub fn before(&self) -> Option<Pt> {
        self.before
    }

    /// Space below the paragraph (`w:after`).
    pub fn after(&self) -> Option<Pt> {
        self.after
    }

    /// Space above, in hundredths of a line, which wins over [`before`](Self::before)
    /// (`w:beforeLines`).
    pub fn before_lines(&self) -> Option<i64> {
        self.before_lines
    }

    /// Space below, in hundredths of a line (`w:afterLines`).
    pub fn after_lines(&self) -> Option<i64> {
        self.after_lines
    }

    /// Whether the application chooses the space above, as for HTML paragraphs
    /// (`w:beforeAutospacing`).
    pub fn before_auto(&self) -> Option<bool> {
        self.before_auto
    }

    /// Whether the application chooses the space below (`w:afterAutospacing`).
    pub fn after_auto(&self) -> Option<bool> {
        self.after_auto
    }

    /// The spacing between lines (`w:line` with `w:lineRule`).
    pub fn line(&self) -> Option<LineSpacing> {
        self.line
    }
}

/// The indentation of a paragraph (`w:ind`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Indentation {
    pub(crate) start: Option<Pt>,
    pub(crate) end: Option<Pt>,
    pub(crate) first_line: Option<Pt>,
    pub(crate) hanging: Option<Pt>,
    pub(crate) start_chars: Option<i64>,
    pub(crate) end_chars: Option<i64>,
    pub(crate) first_line_chars: Option<i64>,
    pub(crate) hanging_chars: Option<i64>,
}

impl Indentation {
    /// From the start edge of the text area (`w:left` in Transitional documents,
    /// `w:start` in Strict ones).
    pub fn start(&self) -> Option<Pt> {
        self.start
    }

    /// From the end edge (`w:right` or `w:end`).
    pub fn end(&self) -> Option<Pt> {
        self.end
    }

    /// How much further in the first line starts (`w:firstLine`).
    pub fn first_line(&self) -> Option<Pt> {
        self.first_line
    }

    /// How much less far in the first line starts (`w:hanging`).
    pub fn hanging(&self) -> Option<Pt> {
        self.hanging
    }

    /// The start indentation in hundredths of a character, which wins over
    /// [`start`](Self::start) (`w:leftChars` or `w:startChars`).
    pub fn start_chars(&self) -> Option<i64> {
        self.start_chars
    }

    /// The end indentation in hundredths of a character.
    pub fn end_chars(&self) -> Option<i64> {
        self.end_chars
    }

    /// The first line indentation in hundredths of a character.
    pub fn first_line_chars(&self) -> Option<i64> {
        self.first_line_chars
    }

    /// The hanging indentation in hundredths of a character.
    pub fn hanging_chars(&self) -> Option<i64> {
        self.hanging_chars
    }
}

/// The list a paragraph is an item of (`w:numPr`): a numbering instance and a level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NumberingReference {
    pub(crate) instance: Option<i64>,
    pub(crate) level: Option<i64>,
}

impl NumberingReference {
    /// The numbering instance (`w:numId`); 0 takes the paragraph out of a list its
    /// style puts it in.
    pub fn instance(&self) -> Option<i64> {
        self.instance
    }

    /// The level (`w:ilvl`): 0 to 8 by ECMA-376 §17.9.3, other values kept as written.
    pub fn level(&self) -> Option<i64> {
        self.level
    }
}

/// The paragraph properties written on a paragraph or in a style (`w:pPr`, ECMA-376
/// Part 1, §17.3.1): only what is set there.
#[doc(alias = "ParagraphFormat")]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParagraphProperties {
    pub(crate) style: Option<String>,
    pub(crate) keep_next: Option<bool>,
    pub(crate) keep_lines: Option<bool>,
    pub(crate) page_break_before: Option<bool>,
    pub(crate) widow_control: Option<bool>,
    pub(crate) suppress_line_numbers: Option<bool>,
    pub(crate) suppress_auto_hyphens: Option<bool>,
    pub(crate) contextual_spacing: Option<bool>,
    pub(crate) mirror_indents: Option<bool>,
    pub(crate) word_wrap: Option<bool>,
    pub(crate) right_to_left: Option<bool>,
    pub(crate) snap_to_grid: Option<bool>,
    pub(crate) numbering: Option<NumberingReference>,
    pub(crate) borders: Option<ParagraphBorders>,
    pub(crate) shading: Option<Shading>,
    pub(crate) tabs: Vec<TabStop>,
    pub(crate) spacing: Option<Spacing>,
    pub(crate) indentation: Option<Indentation>,
    pub(crate) justification: Option<Justification>,
    pub(crate) outline_level: Option<i64>,
    pub(crate) mark: RunProperties,
}

impl ParagraphProperties {
    /// The paragraph style (`w:pStyle`), by its id.
    pub fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }

    /// Kept on the page of the next paragraph (`w:keepNext`).
    pub fn keep_next(&self) -> Option<bool> {
        self.keep_next
    }

    /// All lines kept on one page (`w:keepLines`).
    pub fn keep_lines(&self) -> Option<bool> {
        self.keep_lines
    }

    /// Starts a new page (`w:pageBreakBefore`).
    pub fn page_break_before(&self) -> Option<bool> {
        self.page_break_before
    }

    /// No single first or last line alone at the bottom or top of a page
    /// (`w:widowControl`).
    pub fn widow_control(&self) -> Option<bool> {
        self.widow_control
    }

    /// Left out of line numbering (`w:suppressLineNumbers`).
    pub fn suppress_line_numbers(&self) -> Option<bool> {
        self.suppress_line_numbers
    }

    /// Not hyphenated automatically (`w:suppressAutoHyphens`).
    pub fn suppress_auto_hyphens(&self) -> Option<bool> {
        self.suppress_auto_hyphens
    }

    /// Space before and after left out between paragraphs of the same style
    /// (`w:contextualSpacing`).
    pub fn contextual_spacing(&self) -> Option<bool> {
        self.contextual_spacing
    }

    /// Start and end indentation swapped on even pages (`w:mirrorIndents`).
    pub fn mirror_indents(&self) -> Option<bool> {
        self.mirror_indents
    }

    /// Whether Latin words are kept whole at the ends of lines; off lets lines break
    /// inside them, as East Asian text does (`w:wordWrap`).
    pub fn word_wrap(&self) -> Option<bool> {
        self.word_wrap
    }

    /// A right-to-left paragraph (`w:bidi`).
    pub fn right_to_left(&self) -> Option<bool> {
        self.right_to_left
    }

    /// Laid out on the document grid when it has one (`w:snapToGrid`).
    pub fn snap_to_grid(&self) -> Option<bool> {
        self.snap_to_grid
    }

    /// The list the paragraph is an item of (`w:numPr`).
    pub fn numbering(&self) -> Option<&NumberingReference> {
        self.numbering.as_ref()
    }

    /// The borders (`w:pBdr`).
    pub fn borders(&self) -> Option<&ParagraphBorders> {
        self.borders.as_ref()
    }

    /// The shading behind the paragraph (`w:shd`).
    pub fn shading(&self) -> Option<&Shading> {
        self.shading.as_ref()
    }

    /// The tab stops set or cleared here (`w:tabs`), in document order. A `w:tab`
    /// without a valid `w:pos` cannot be placed and is left out.
    pub fn tabs(&self) -> &[TabStop] {
        &self.tabs
    }

    /// The spacing (`w:spacing`).
    pub fn spacing(&self) -> Option<&Spacing> {
        self.spacing.as_ref()
    }

    /// The indentation (`w:ind`).
    pub fn indentation(&self) -> Option<&Indentation> {
        self.indentation.as_ref()
    }

    /// The alignment of the lines (`w:jc`).
    pub fn justification(&self) -> Option<Justification> {
        self.justification
    }

    /// The outline level (`w:outlineLvl`): 0 to 8, or 9 for body text, by ECMA-376
    /// §17.3.1.20; other values kept as written.
    pub fn outline_level(&self) -> Option<i64> {
        self.outline_level
    }

    /// The run properties of the paragraph mark (the `w:rPr` in `w:pPr`, ECMA-376
    /// §17.3.1.29). How far they shape the paragraph in Word, such as the height of an
    /// empty one or its list label, is to be measured.
    pub fn mark_run_properties(&self) -> &RunProperties {
        &self.mark
    }
}
