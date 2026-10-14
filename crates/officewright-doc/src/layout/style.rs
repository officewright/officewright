//! Run formatting in effect as the IR's run styles, one per distinct format.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use officewright_core::Color;
use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::Translator;
use crate::model::properties::Toggle;
use crate::model::{
    CharacterFormat, ColorValue, FontHint, Highlight, Shading, ShadingPattern, UnderlineStyle,
    VerticalPosition,
};

/// The longest font name passed on, in bytes, cut at the end of a character. Word allows
/// 31 characters (MS-OI29500 §17.8.3.10(a)), at most 124 bytes, so no font's name is
/// cut; the IR keeps a copy of the names per run style and per symbol.
const MAX_FONT_NAME: usize = 128;

/// The longest language tag passed on, in bytes; a longer one is left out. BCP 47 tags
/// are far shorter, and the IR keeps a copy of the tags per run style.
const MAX_LANGUAGE: usize = 64;

/// How many formats with the same hash a new one is compared with before it gets a run
/// style of its own. The hash covers all of a [`StyleKey`], so formats that share it are
/// almost always equal, and the first comparison finds them.
const MAX_COMPARISONS: usize = 8;

/// The run styles added to the IR so far, by a hash of their formats' [`StyleKey`].
#[derive(Default)]
pub(super) struct RunStyles {
    by_key: HashMap<u64, Vec<(CharacterFormat, ir::RunStyleId)>>,
}

/// What decides the IR run style of a format: its names and tags as the IR keeps them,
/// cut and borrowed from the format, and each of its other properties. Formats with equal
/// keys give equal run styles, and hashing or comparing a key costs a bounded amount,
/// however long the document's names are. Nothing is copied: a run's key is built for
/// every run, the IR style only for a new one.
#[derive(PartialEq, Eq, Hash)]
struct StyleKey<'f> {
    names: [&'f str; 4],
    hint: Option<FontHint>,
    sizes: [u64; 2],
    toggles: [bool; Toggle::COUNT],
    colors: [ColorValue; 2],
    underline: Option<UnderlineStyle>,
    vertical_position: VerticalPosition,
    offsets: [u64; 2],
    scale: u16,
    kerning: Option<u64>,
    highlight: Option<Highlight>,
    shading: Option<Shading>,
    languages: [Option<&'f str>; 3],
    /// Which names and tags were cut or left out: a format whose tag is left out is not
    /// one without a tag, so that what is left out is listed once per key.
    cuts: [bool; 7],
}

impl<'f> StyleKey<'f> {
    fn of(format: &'f CharacterFormat) -> Self {
        // Every field, so that one added to the format cannot be left out of the key.
        let CharacterFormat {
            // The IR keeps no style id; what the style gives is in the other fields.
            style: _,
            fonts,
            size,
            size_complex_script,
            toggles,
            color,
            underline,
            underline_color,
            vertical_position,
            position,
            spacing,
            scale,
            kerning,
            highlight,
            shading,
            language_latin,
            language_east_asia,
            language_bidi,
        } = format;
        let names = [
            fonts.ascii(),
            fonts.high_ansi(),
            fonts.east_asia(),
            fonts.complex_script(),
        ];
        let tags = [
            language_latin.as_deref(),
            language_east_asia.as_deref(),
            language_bidi.as_deref(),
        ];
        let mut cuts = [false; 7];
        for (cut, name) in cuts.iter_mut().zip(names) {
            *cut = name.len() > MAX_FONT_NAME;
        }
        for (cut, tag) in cuts.iter_mut().skip(4).zip(tags) {
            *cut = tag.is_some_and(|tag| tag.len() > MAX_LANGUAGE);
        }
        Self {
            names: names.map(cut_name),
            hint: fonts.hint(),
            sizes: [size.0.to_bits(), size_complex_script.0.to_bits()],
            toggles: *toggles,
            colors: [*color, *underline_color],
            underline: *underline,
            vertical_position: *vertical_position,
            offsets: [position.0.to_bits(), spacing.0.to_bits()],
            scale: *scale,
            kerning: kerning.map(|kerning| kerning.0.to_bits()),
            highlight: *highlight,
            shading: *shading,
            languages: tags.map(cut_tag),
            cuts,
        }
    }

    /// The bytes of its names, for tests of the cost of comparing keys.
    #[cfg(test)]
    fn names_length(&self) -> usize {
        self.names.iter().map(|name| name.len()).sum()
    }
}

impl Translator<'_> {
    /// The run style of a format, added to the IR the first time a format with its
    /// [`StyleKey`] is met: formats that give the same IR style share it.
    pub(super) fn run_style(&mut self, format: &CharacterFormat) -> ir::RunStyleId {
        let key = StyleKey::of(format);
        let hash = {
            let mut hasher = DefaultHasher::new();
            key.hash(&mut hasher);
            hasher.finish()
        };
        if let Some(known) = self.run_styles.by_key.get(&hash) {
            for (known, id) in known.iter().take(MAX_COMPARISONS) {
                let known = StyleKey::of(known);
                #[cfg(test)]
                super::work(|work| work.compared += known.names_length() + key.names_length());
                if known == key {
                    return *id;
                }
            }
        }
        #[cfg(test)]
        super::work(|work| work.built += 1);
        self.report_cuts(format);
        let style = self.translate_run_style(format);
        let id = self.ir.add_run_style(style);
        // Formats past the ones compared with are not kept.
        let known = self.run_styles.by_key.entry(hash).or_default();
        if known.len() < MAX_COMPARISONS {
            known.push((format.clone(), id));
        }
        id
    }

    /// Lists the names and tags of a format that its run style has cut or left out.
    fn report_cuts(&mut self, format: &CharacterFormat) {
        let families = format.fonts();
        for name in [
            families.ascii(),
            families.high_ansi(),
            families.east_asia(),
            families.complex_script(),
        ] {
            if name.len() > MAX_FONT_NAME {
                self.leave_out(ReportKind::FeatureUnsupported, || {
                    format!("a font name of more than {MAX_FONT_NAME} bytes is cut")
                });
            }
        }
        for tag in [
            format.language(),
            format.language_east_asia(),
            format.language_bidi(),
        ]
        .into_iter()
        .flatten()
        {
            if tag.len() > MAX_LANGUAGE {
                self.leave_out(ReportKind::FeatureUnsupported, || {
                    format!("a language tag of more than {MAX_LANGUAGE} bytes is left out")
                });
            }
        }
    }

    fn translate_run_style(&self, format: &CharacterFormat) -> ir::RunStyle {
        let families = format.fonts();
        let east_asia = cut_font_name(families.east_asia());
        // Some characters take the East Asian font only for Chinese ones, by the font
        // table's character set of that font (MS-OI29500 2.1.88 b). The name looked up is
        // the one cut, so the work stays bounded per run style.
        let charset = self
            .document
            .font(&east_asia)
            .and_then(|font| font.charset());
        let mut fonts = ir::RunFonts::default()
            .with_ascii(cut_font_name(families.ascii()))
            .with_high_ansi(cut_font_name(families.high_ansi()))
            .with_east_asia(east_asia)
            .with_complex_script(cut_font_name(families.complex_script()))
            .with_hint(match families.hint() {
                None | Some(FontHint::Default) => ir::FontHint::Default,
                Some(FontHint::EastAsia) => ir::FontHint::EastAsia,
                Some(FontHint::ComplexScript) => ir::FontHint::ComplexScript,
            });
        fonts = fonts.with_east_asia_charset(match charset {
            Some(0x88) => ir::EastAsiaCharset::ChineseBig5,
            Some(0x86) => ir::EastAsiaCharset::Gb2312,
            _ => ir::EastAsiaCharset::Other,
        });
        let language = ir::Language::default()
            .with_latin(cut_language(format.language()))
            .with_east_asia(cut_language(format.language_east_asia()))
            .with_bidi(cut_language(format.language_bidi()));
        ir::RunStyle::default()
            .with_fonts(fonts)
            .with_size(format.size())
            .with_size_complex_script(format.size_complex_script())
            .with_bold(format.bold())
            .with_italic(format.italic())
            .with_bold_complex_script(format.bold_complex_script())
            .with_italic_complex_script(format.italic_complex_script())
            .with_complex_script(format.complex_script())
            .with_rtl(format.right_to_left())
            .with_color(color(format.color()))
            .with_underline(format.underline().map(|style| {
                ir::Underline::default()
                    .with_kind(underline(style))
                    .with_color(color(format.underline_color()))
            }))
            .with_strike(format.strike())
            .with_double_strike(format.double_strike())
            .with_caps(format.caps())
            .with_small_caps(format.small_caps())
            .with_vertical(match format.vertical_position() {
                VerticalPosition::Baseline => ir::VerticalPosition::Baseline,
                VerticalPosition::Superscript => ir::VerticalPosition::Superscript,
                VerticalPosition::Subscript => ir::VerticalPosition::Subscript,
            })
            .with_position(format.position())
            .with_letter_spacing(format.spacing())
            .with_scale(format.scale())
            .with_kern_min(format.kerning())
            .with_highlight(format.highlight().and_then(highlight))
            .with_shading(format.shading().and_then(|shading| self.shading(shading)))
            .with_hidden(format.hidden())
            .with_language(language)
    }

    /// A font name as the IR gets it, cut as [`cut_font_name`] does; a cut is listed.
    pub(super) fn font_name(&mut self, name: &str) -> String {
        if name.len() > MAX_FONT_NAME {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                format!("a font name of more than {MAX_FONT_NAME} bytes is cut")
            });
        }
        cut_font_name(name)
    }

    /// A shading as written, with its theme colours resolved; none for `nil`, which is
    /// no shading at all.
    pub(super) fn shading(&self, shading: &Shading) -> Option<ir::Shading> {
        use ShadingPattern as P;
        use ir::ShadingPattern as I;
        let pattern = match shading.pattern().unwrap_or(P::Clear) {
            P::Nil => return None,
            P::Clear => I::Clear,
            P::Solid => I::Solid,
            P::HorizontalStripe => I::HorizontalStripe,
            P::VerticalStripe => I::VerticalStripe,
            P::ReverseDiagonalStripe => I::ReverseDiagonalStripe,
            P::DiagonalStripe => I::DiagonalStripe,
            P::HorizontalCross => I::HorizontalCross,
            P::DiagonalCross => I::DiagonalCross,
            P::ThinHorizontalStripe => I::ThinHorizontalStripe,
            P::ThinVerticalStripe => I::ThinVerticalStripe,
            P::ThinReverseDiagonalStripe => I::ThinReverseDiagonalStripe,
            P::ThinDiagonalStripe => I::ThinDiagonalStripe,
            P::ThinHorizontalCross => I::ThinHorizontalCross,
            P::ThinDiagonalCross => I::ThinDiagonalCross,
            P::Percent5 => I::Percent5,
            P::Percent10 => I::Percent10,
            P::Percent12 => I::Percent12,
            P::Percent15 => I::Percent15,
            P::Percent20 => I::Percent20,
            P::Percent25 => I::Percent25,
            P::Percent30 => I::Percent30,
            P::Percent35 => I::Percent35,
            P::Percent37 => I::Percent37,
            P::Percent40 => I::Percent40,
            P::Percent45 => I::Percent45,
            P::Percent50 => I::Percent50,
            P::Percent55 => I::Percent55,
            P::Percent60 => I::Percent60,
            P::Percent62 => I::Percent62,
            P::Percent65 => I::Percent65,
            P::Percent70 => I::Percent70,
            P::Percent75 => I::Percent75,
            P::Percent80 => I::Percent80,
            P::Percent85 => I::Percent85,
            P::Percent87 => I::Percent87,
            P::Percent90 => I::Percent90,
            P::Percent95 => I::Percent95,
        };
        let document = self.document;
        Some(
            ir::Shading::default()
                .with_pattern(pattern)
                .with_color(color(document.resolve_color(shading.color())))
                .with_fill(color(document.resolve_color(shading.fill()))),
        )
    }
}

/// A font name cut to at most [`MAX_FONT_NAME`] bytes, at the end of a character.
fn cut_name(name: &str) -> &str {
    let end = (0..=MAX_FONT_NAME.min(name.len()))
        .rev()
        .find(|&end| name.is_char_boundary(end))
        .unwrap_or(0);
    name.get(..end).unwrap_or_default()
}

/// A font name as the IR keeps it, cut as [`cut_name`] does.
fn cut_font_name(name: &str) -> String {
    cut_name(name).to_owned()
}

/// A language tag as the IR gets it: none past [`MAX_LANGUAGE`] bytes.
fn cut_tag(tag: Option<&str>) -> Option<&str> {
    tag.filter(|tag| tag.len() <= MAX_LANGUAGE)
}

/// A language tag as the IR keeps it, as [`cut_tag`] gives it.
fn cut_language(tag: Option<&str>) -> Option<String> {
    cut_tag(tag).map(str::to_owned)
}

pub(super) fn color(value: ColorValue) -> ir::ColorValue {
    match value {
        ColorValue::Auto => ir::ColorValue::Auto,
        ColorValue::Rgb(red, green, blue) => ir::ColorValue::Rgb(Color::rgb(red, green, blue)),
    }
}

fn underline(style: UnderlineStyle) -> ir::UnderlineKind {
    use UnderlineStyle as U;
    use ir::UnderlineKind as K;
    match style {
        // A format has no underline of style none; it has none at all.
        U::Single | U::None => K::Single,
        U::Words => K::Words,
        U::Double => K::Double,
        U::Thick => K::Thick,
        U::Dotted => K::Dotted,
        U::DottedHeavy => K::DottedHeavy,
        U::Dash => K::Dash,
        U::DashedHeavy => K::DashedHeavy,
        U::DashLong => K::DashLong,
        U::DashLongHeavy => K::DashLongHeavy,
        U::DotDash => K::DotDash,
        U::DashDotHeavy => K::DashDotHeavy,
        U::DotDotDash => K::DotDotDash,
        U::DashDotDotHeavy => K::DashDotDotHeavy,
        U::Wave => K::Wave,
        U::WavyHeavy => K::WavyHeavy,
        U::WavyDouble => K::WavyDouble,
    }
}

/// The colour of a highlight (ST_HighlightColor, ECMA-376 Part 1, §17.18.40).
fn highlight(highlight: Highlight) -> Option<Color> {
    use Highlight as H;
    let [red, green, blue] = match highlight {
        H::None => return None,
        H::Black => [0x00, 0x00, 0x00],
        H::Blue => [0x00, 0x00, 0xFF],
        H::Cyan => [0x00, 0xFF, 0xFF],
        H::Green => [0x00, 0xFF, 0x00],
        H::Magenta => [0xFF, 0x00, 0xFF],
        H::Red => [0xFF, 0x00, 0x00],
        H::Yellow => [0xFF, 0xFF, 0x00],
        H::White => [0xFF, 0xFF, 0xFF],
        H::DarkBlue => [0x00, 0x00, 0x80],
        H::DarkCyan => [0x00, 0x80, 0x80],
        H::DarkGreen => [0x00, 0x80, 0x00],
        H::DarkMagenta => [0x80, 0x00, 0x80],
        H::DarkRed => [0x80, 0x00, 0x00],
        H::DarkYellow => [0x80, 0x80, 0x00],
        H::DarkGray => [0x80, 0x80, 0x80],
        H::LightGray => [0xC0, 0xC0, 0xC0],
    };
    Some(Color::rgb(red, green, blue))
}
