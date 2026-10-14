//! Run formatting in effect as the IR's run styles, one per distinct format.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use officewright_core::Color;
use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::Translator;
use crate::model::{
    CharacterFormat, ColorValue, FontHint, Highlight, Shading, ShadingPattern, UnderlineStyle,
    VerticalPosition,
};

/// The longest font name passed on, in characters. No font has a longer name, and the IR
/// keeps a copy of the names per run style.
const MAX_FONT_NAME: usize = 256;

/// How many formats with the same key a new one is compared with before it gets a run
/// style of its own: sharing styles saves memory, it does not change the layout, so the
/// work it takes stays bounded.
const MAX_COMPARISONS: usize = 8;

/// The run styles added to the IR so far, by a key of their format.
#[derive(Default)]
pub(super) struct RunStyles {
    by_key: HashMap<u64, Vec<(CharacterFormat, ir::RunStyleId)>>,
}

impl Translator<'_> {
    /// The run style of a format, added to the IR the first time it is met.
    pub(super) fn run_style(&mut self, format: &CharacterFormat) -> ir::RunStyleId {
        let key = key(format);
        if let Some(styles) = self.run_styles.by_key.get(&key) {
            let found = styles
                .iter()
                .take(MAX_COMPARISONS)
                .find(|(known, _)| known == format);
            if let Some((_, id)) = found {
                return *id;
            }
        }
        let style = self.translate_run_style(format);
        let id = self.ir.add_run_style(style);
        self.run_styles
            .by_key
            .entry(key)
            .or_default()
            .push((format.clone(), id));
        id
    }

    fn translate_run_style(&mut self, format: &CharacterFormat) -> ir::RunStyle {
        let families = format.fonts();
        let mut fonts = ir::RunFonts::default()
            .with_ascii(self.font_name(families.ascii()))
            .with_high_ansi(self.font_name(families.high_ansi()))
            .with_east_asia(self.font_name(families.east_asia()))
            .with_complex_script(self.font_name(families.complex_script()))
            .with_hint(match families.hint() {
                None | Some(FontHint::Default) => ir::FontHint::Default,
                Some(FontHint::EastAsia) => ir::FontHint::EastAsia,
                Some(FontHint::ComplexScript) => ir::FontHint::ComplexScript,
            });
        // Some characters take the East Asian font only for Chinese ones, by the font
        // table's character set of that font (MS-OI29500 2.1.88 b).
        let charset = self
            .document
            .font(families.east_asia())
            .and_then(|font| font.charset());
        fonts = fonts.with_east_asia_charset(match charset {
            Some(0x88) => ir::EastAsiaCharset::ChineseBig5,
            Some(0x86) => ir::EastAsiaCharset::Gb2312,
            _ => ir::EastAsiaCharset::Other,
        });
        let language = ir::Language::default()
            .with_latin(format.language().map(str::to_owned))
            .with_east_asia(format.language_east_asia().map(str::to_owned))
            .with_bidi(format.language_bidi().map(str::to_owned));
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

    /// A font name as the IR gets it: cut to [`MAX_FONT_NAME`] characters.
    fn font_name(&mut self, name: &str) -> String {
        match name.char_indices().nth(MAX_FONT_NAME) {
            Some((end, _)) => {
                self.leave_out(ReportKind::FeatureUnsupported, || {
                    format!("a font name of more than {MAX_FONT_NAME} characters is cut")
                });
                name.get(..end).unwrap_or_default().to_owned()
            }
            None => name.to_owned(),
        }
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

/// A key of a format: equal formats have equal keys. Font names count by their length
/// and their ends only, so that the key costs the same however long they are.
fn key(format: &CharacterFormat) -> u64 {
    let mut hasher = DefaultHasher::new();
    let families = format.fonts();
    for name in [
        families.ascii(),
        families.high_ansi(),
        families.east_asia(),
        families.complex_script(),
    ] {
        let bytes = name.as_bytes();
        bytes.len().hash(&mut hasher);
        bytes.get(..bytes.len().min(32)).hash(&mut hasher);
        bytes
            .get(bytes.len().saturating_sub(32)..)
            .hash(&mut hasher);
    }
    format.size().0.to_bits().hash(&mut hasher);
    format.size_complex_script().0.to_bits().hash(&mut hasher);
    [
        format.bold(),
        format.italic(),
        format.caps(),
        format.small_caps(),
        format.strike(),
        format.hidden(),
    ]
    .hash(&mut hasher);
    format.color().hash(&mut hasher);
    format.underline().hash(&mut hasher);
    format.vertical_position().hash(&mut hasher);
    format.highlight().hash(&mut hasher);
    hasher.finish()
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
