//! The formatting of runs of text.

use officewright_core::{Color, Pt};

use super::macros::{ir_struct, no_rules};
use super::paragraph::Shading;
use super::rules;

ir_struct! {
    /// The formatting of a run of text, with the styles already applied. Each default is
    /// what WordprocessingML means when nothing in the style hierarchy sets the property,
    /// where it says; it leaves the font size to the application, and the default here is
    /// 10 pt.
    #[derive(Clone, Debug, PartialEq)]
    pub struct RunStyle {
        /// The font families, by the kind of character.
        ref fonts: RunFonts = RunFonts::default() => with_fonts;
        /// The font size of text that is not complex script (`w:sz`).
        copy size: Pt = Pt(10.0) => with_size, check rules::non_negative,
            clamp rules::font_size;
        /// The font size of complex script text (`w:szCs`).
        copy size_complex_script: Pt = Pt(10.0) => with_size_complex_script,
            check rules::non_negative, clamp rules::font_size;
        /// Whether text that is not complex script is bold.
        copy bold: bool = false => with_bold;
        /// Whether text that is not complex script is italic.
        copy italic: bool = false => with_italic;
        /// Whether complex script text is bold.
        copy bold_complex_script: bool = false => with_bold_complex_script;
        /// Whether complex script text is italic.
        copy italic_complex_script: bool = false => with_italic_complex_script;
        /// Whether the run is complex script text (`w:cs`): every character then takes the
        /// complex script font and properties.
        copy complex_script: bool = false => with_complex_script;
        /// Whether the run is right-to-left text (`w:rtl`), which also takes the complex
        /// script font and properties.
        copy rtl: bool = false => with_rtl;
        /// The colour of the text. Automatic is chosen by the layout: black, or white on a
        /// dark shading.
        copy color: ColorValue = ColorValue::Auto => with_color;
        /// The underline.
        copy underline: Option<Underline> = None => with_underline;
        /// Whether the text is struck through with one line.
        copy strike: bool = false => with_strike;
        /// Whether the text is struck through with two lines.
        copy double_strike: bool = false => with_double_strike;
        /// Whether lower-case letters are shown as capitals.
        copy caps: bool = false => with_caps;
        /// Whether lower-case letters are shown as smaller capitals.
        copy small_caps: bool = false => with_small_caps;
        /// Whether the text is a superscript or subscript.
        copy vertical: VerticalPosition = VerticalPosition::Baseline => with_vertical;
        /// How far the text is raised, or lowered when negative, from the baseline
        /// (`w:position`).
        copy position: Pt = Pt(0.0) => with_position;
        /// The space added after each character, or taken away when negative
        /// (`w:spacing`).
        copy letter_spacing: Pt = Pt(0.0) => with_letter_spacing, clamp rules::letter_spacing;
        /// The width of the characters, in percent of their normal width (`w:w`).
        copy scale: u16 = 100 => with_scale, clamp rules::text_scale;
        /// The font size from which the text is kerned (`w:kern`); `None` for no kerning.
        copy kern_min: Option<Pt> = None => with_kern_min,
            check rules::non_negative_if_set, clamp rules::kerning;
        /// The ligatures the text uses.
        copy ligatures: Ligatures = Ligatures::default() => with_ligatures;
        /// The highlight colour behind the text.
        copy highlight: Option<Color> = None => with_highlight;
        /// The shading behind the text.
        copy shading: Option<Shading> = None => with_shading;
        /// Whether the text is hidden.
        copy hidden: bool = false => with_hidden;
        /// The languages of the text, by the kind of character.
        ref language: Language = Language::default() => with_language;
    }
}

ir_struct! {
    /// The font families of a run, by the kind of character (`w:rFonts`): which one a
    /// character takes is Word's rule, which the layout applies (MS-OI29500 2.1.88 b).
    #[derive(Clone, Debug, PartialEq)]
    pub struct RunFonts {
        /// For characters U+0000 to U+007F (`w:ascii`).
        str ascii: String = String::from(DEFAULT_FONT) => with_ascii;
        /// For other characters that are not East Asian or complex script (`w:hAnsi`).
        str high_ansi: String = String::from(DEFAULT_FONT) => with_high_ansi;
        /// For East Asian characters (`w:eastAsia`).
        str east_asia: String = String::from(DEFAULT_FONT) => with_east_asia;
        /// For complex script characters (`w:cs`).
        str complex_script: String = String::from(DEFAULT_FONT) => with_complex_script;
        /// Which font the characters that several kinds share take (`w:hint`).
        copy hint: FontHint = FontHint::Default => with_hint;
        /// The character set of the East Asian font, from the document's font table: some
        /// characters take the East Asian font only for Chinese ones.
        copy east_asia_charset: EastAsiaCharset = EastAsiaCharset::Other
            => with_east_asia_charset;
    }
}

/// The family Word uses for every kind of character when nothing names one (MS-OI29500
/// 2.1.88 c).
const DEFAULT_FONT: &str = "Times New Roman";

impl RunFonts {
    /// The same family for every kind of character.
    pub fn new(family: impl AsRef<str>) -> Self {
        let family = family.as_ref();
        Self::default()
            .with_ascii(family)
            .with_high_ansi(family)
            .with_east_asia(family)
            .with_complex_script(family)
    }
}

/// Which of a run's fonts the characters that several kinds share take (ST_Hint).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FontHint {
    /// By Word's rule alone.
    #[default]
    Default,
    /// The East Asian font.
    EastAsia,
    /// The complex script font.
    ComplexScript,
}

/// The character set of a run's East Asian font, as far as Word's choice of font for a
/// character depends on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EastAsiaCharset {
    /// Traditional Chinese (`w:charset` 88, Big5).
    ChineseBig5,
    /// Simplified Chinese (`w:charset` 86, GB 2312).
    Gb2312,
    /// Any other.
    #[default]
    Other,
}

/// A colour, or automatic: the layout chooses it from what is around, as Word does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ColorValue {
    /// Automatic (`auto`).
    #[default]
    Auto,
    /// This colour.
    Rgb(Color),
}

ir_struct! {
    /// An underline.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Underline {
        /// Its style.
        copy kind: UnderlineKind = UnderlineKind::Single => with_kind;
        /// Its colour. Automatic is the colour of the text.
        copy color: ColorValue = ColorValue::Auto => with_color;
    }
}

/// The style of an underline (ST_Underline).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnderlineKind {
    /// `single`: one line.
    #[default]
    Single,
    /// `words`: one line under the words, not under the spaces between them.
    Words,
    /// `double`: two lines.
    Double,
    /// `thick`: one thick line.
    Thick,
    /// `dotted`: dots.
    Dotted,
    /// `dottedHeavy`: thick dots.
    DottedHeavy,
    /// `dash`: dashes.
    Dash,
    /// `dashedHeavy`: thick dashes.
    DashedHeavy,
    /// `dashLong`: long dashes.
    DashLong,
    /// `dashLongHeavy`: thick long dashes.
    DashLongHeavy,
    /// `dotDash`: dots and dashes in turn.
    DotDash,
    /// `dashDotHeavy`: thick dots and dashes in turn.
    DashDotHeavy,
    /// `dotDotDash`: two dots, then a dash.
    DotDotDash,
    /// `dashDotDotHeavy`: two thick dots, then a thick dash.
    DashDotDotHeavy,
    /// `wave`: a wavy line.
    Wave,
    /// `wavyHeavy`: a thick wavy line.
    WavyHeavy,
    /// `wavyDouble`: two wavy lines.
    WavyDouble,
}

/// Whether text is a superscript or subscript (ST_VerticalAlignRun).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VerticalPosition {
    /// On the baseline.
    #[default]
    Baseline,
    /// Smaller and raised.
    Superscript,
    /// Smaller and lowered.
    Subscript,
}

ir_struct! {
    /// The kinds of ligatures text uses (`w14:ligatures` of [MS-DOCX]). None when
    /// a document does not say.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Ligatures {
        /// Standard ligatures, such as fi.
        copy standard: bool = false => with_standard;
        /// Contextual alternates.
        copy contextual: bool = false => with_contextual;
        /// Historical ligatures.
        copy historical: bool = false => with_historical;
        /// Discretionary ligatures.
        copy discretional: bool = false => with_discretional;
    }
}

ir_struct! {
    /// The languages of a run's text, as BCP 47 tags such as `en-US`, by the kind of
    /// character. They choose the language-specific forms of glyphs and of capitals.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Language {
        /// Of text that is neither East Asian nor complex script (`w:val`).
        opt_str latin: Option<String> => with_latin;
        /// Of East Asian text.
        opt_str east_asia: Option<String> => with_east_asia;
        /// Of complex script text.
        opt_str bidi: Option<String> => with_bidi;
    }
}

no_rules!(
    FontHint,
    EastAsiaCharset,
    ColorValue,
    UnderlineKind,
    VerticalPosition
);
