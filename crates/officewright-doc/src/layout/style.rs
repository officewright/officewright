//! Run formatting in effect as the IR's run styles, one per distinct format.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hasher};

use officewright_core::Color;
use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::Translator;
use crate::model::{
    CharacterFormat, ColorValue, FontHint, Highlight, Shading, ShadingPattern, UnderlineStyle,
    VerticalPosition,
};

/// The longest font name passed on, in bytes, cut at the end of a character. No font has
/// a longer name (Word allows 31 characters, MS-OI29500 §17.8.3.10(a)), and the IR keeps
/// a copy of the names per run style and per symbol.
const MAX_FONT_NAME: usize = 256;

/// The longest language tag passed on, in bytes; a longer one is left out. BCP 47 tags
/// are far shorter, and the IR keeps a copy of the tags per run style.
const MAX_LANGUAGE: usize = 64;

/// How many run styles with the same key a new one is compared with before it gets a run
/// style of its own. The key hashes the whole IR style, so styles that share a key are
/// almost always equal, and the first comparison finds them.
const MAX_COMPARISONS: usize = 8;

/// The run styles added to the IR so far, by a hash of the whole IR style.
#[derive(Default)]
pub(super) struct RunStyles {
    by_key: HashMap<u64, Vec<ir::RunStyleId>>,
}

impl Translator<'_> {
    /// The run style of a format, added to the IR the first time it is met. The format
    /// becomes its IR style first, whose names and tags are cut, so hashing and comparing
    /// it costs the same however long the document's names are; formats that give the same
    /// IR style share it.
    pub(super) fn run_style(&mut self, format: &CharacterFormat) -> ir::RunStyleId {
        let style = self.translate_run_style(format);
        let key = key(&style);
        if let Some(ids) = self.run_styles.by_key.get(&key) {
            for &id in ids.iter().take(MAX_COMPARISONS) {
                let Some(known) = self.ir.run_style(id) else {
                    continue;
                };
                #[cfg(test)]
                super::work(|work| work.compared += names_length(known) + names_length(&style));
                if *known == style {
                    return id;
                }
            }
        }
        self.report_cuts(format);
        let id = self.ir.add_run_style(style);
        // Styles past the ones compared with are not kept.
        let ids = self.run_styles.by_key.entry(key).or_default();
        if ids.len() < MAX_COMPARISONS {
            ids.push(id);
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
fn cut_font_name(name: &str) -> String {
    let end = (0..=MAX_FONT_NAME.min(name.len()))
        .rev()
        .find(|&end| name.is_char_boundary(end))
        .unwrap_or(0);
    name.get(..end).unwrap_or_default().to_owned()
}

/// A language tag as the IR gets it: none past [`MAX_LANGUAGE`] bytes.
fn cut_language(tag: Option<&str>) -> Option<String> {
    tag.filter(|tag| tag.len() <= MAX_LANGUAGE)
        .map(str::to_owned)
}

/// A key of an IR run style: a hash of all of it, through its debug text, which costs a
/// bounded amount since its names and tags are cut. Equal styles have equal keys.
fn key(style: &ir::RunStyle) -> u64 {
    struct Hashing<'a>(&'a mut DefaultHasher);
    impl std::fmt::Write for Hashing<'_> {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0.write(text.as_bytes());
            Ok(())
        }
    }
    let mut hasher = DefaultHasher::new();
    // reason: writing into a hasher cannot fail.
    let _ = std::fmt::Write::write_fmt(&mut Hashing(&mut hasher), format_args!("{style:?}"));
    hasher.finish()
}

/// The bytes of the font names of a style, for tests of the cost of comparing styles.
#[cfg(test)]
fn names_length(style: &ir::RunStyle) -> usize {
    let fonts = style.fonts();
    fonts.ascii().len()
        + fonts.high_ansi().len()
        + fonts.east_asia().len()
        + fonts.complex_script().len()
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
