//! Run formatting in effect as the IR's run styles, one per distinct format.

use std::collections::HashMap;

use officewright_core::Color;
use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::Translator;
use crate::model::{
    CharacterFormat, ColorValue, FontFamilies, FontHint, Highlight, Ligatures, Shading,
    ShadingPattern, UnderlineStyle, VerticalPosition,
};

/// The longest font name passed on, in bytes, cut at the end of a character. Word lets
/// one type 31 characters (MS-OI29500 §17.8.3.10(a)), but keeps longer names it reads:
/// LibreOffice's `redline-number-portion.docx`, saved by Word, names a CSS font list of
/// 191 bytes. The IR keeps a copy of the names per run style and per symbol (#175).
const MAX_FONT_NAME: usize = 256;

/// The longest language tag passed on, in bytes; a longer one is left out. BCP 47 tags
/// are far shorter, and the IR keeps a copy of the tags per run style.
const MAX_LANGUAGE: usize = 64;

/// The run styles added to the IR so far, by the [`OwnedKey`] of their formats, and the
/// strings their keys name.
#[derive(Default)]
pub(super) struct RunStyles {
    by_key: HashMap<OwnedKey, ir::RunStyleId>,
    /// The cut font names and language tags of the keys so far, each once, by its id.
    strings: HashMap<Box<str>, u32>,
}

/// What decides the IR run style of a format, as kept for each style: its names and tags
/// by their ids in [`RunStyles::strings`] ([`NO_TAG`] for none), and its other values.
#[derive(PartialEq, Eq, Hash)]
struct OwnedKey {
    strings: [u32; 7],
    values: Values,
}

/// The id of a tag a format does not give.
const NO_TAG: u32 = u32::MAX;

/// What decides the IR run style of a format besides its names and tags: every value the
/// translation reads, and which names and tags were cut or left out (so that a format
/// whose tag is left out is not one without a tag, and what is left out is listed once).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Values {
    hint: Option<FontHint>,
    sizes: [u64; 2],
    /// The on/off properties the IR keeps, one bit each.
    toggles: u16,
    colors: [ColorValue; 2],
    underline: Option<UnderlineStyle>,
    vertical_position: VerticalPosition,
    offsets: [u64; 2],
    scale: u16,
    kerning: Option<u64>,
    highlight: Option<Highlight>,
    shading: Option<Shading>,
    ligatures: Ligatures,
    contextual_alternates: bool,
    cuts: u8,
}

/// What decides the IR run style of a format: its names and tags as the IR keeps them,
/// cut and borrowed from the format, and its [`Values`]. Formats with equal keys give equal
/// run styles. A key is built for every run without copying anything, and costs a bounded
/// amount however long the document's names are; the IR style is built only for a key
/// not met before.
struct StyleKey<'f> {
    /// The four names, then the three tags.
    strings: [Option<&'f str>; 7],
    values: Values,
}

impl<'f> StyleKey<'f> {
    /// The key of a format; its ligatures and contextual alternates count only with
    /// `open_type_features`, as the IR style takes them.
    fn of(format: &'f CharacterFormat, open_type_features: bool) -> Self {
        // Every field, so that one added to the format cannot be left out of the key.
        let CharacterFormat {
            // The IR keeps no style id; what the style gives is in the other fields.
            style: _,
            fonts,
            size,
            size_complex_script,
            // Only those the IR keeps, below: one it does not keep would split styles
            // that are the same.
            toggles: _,
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
            ligatures,
            contextual_alternates,
            // Not drawn, so not in the IR; reported (#303).
            emphasis_mark: _,
            border: _,
        } = format;
        let FontFamilies {
            ascii,
            high_ansi,
            east_asia,
            complex_script,
            hint,
        } = fonts;
        let names: [&str; 4] = [ascii, high_ansi, east_asia, complex_script];
        let tags = [
            language_latin.as_deref(),
            language_east_asia.as_deref(),
            language_bidi.as_deref(),
        ];
        let mut cuts = 0u8;
        for (at, name) in names.iter().enumerate() {
            if name.len() > MAX_FONT_NAME {
                cuts |= 1 << at;
            }
        }
        for (at, tag) in tags.iter().enumerate() {
            if tag.is_some_and(|tag| tag.len() > MAX_LANGUAGE) {
                cuts |= 1 << (4 + at);
            }
        }
        let mut toggles = 0u16;
        for (at, on) in [
            format.bold(),
            format.italic(),
            format.bold_complex_script(),
            format.italic_complex_script(),
            format.complex_script(),
            format.right_to_left(),
            format.strike(),
            format.double_strike(),
            format.caps(),
            format.small_caps(),
            format.hidden(),
        ]
        .into_iter()
        .enumerate()
        {
            if on {
                toggles |= 1 << at;
            }
        }
        let [a, b, c, d] = names.map(cut_name);
        let [e, f, g] = tags.map(cut_tag);
        Self {
            strings: [Some(a), Some(b), Some(c), Some(d), e, f, g],
            values: Values {
                // The IR takes no hint and the hint default alike.
                hint: hint.filter(|hint| *hint != FontHint::Default),
                sizes: [size.0.to_bits(), size_complex_script.0.to_bits()],
                toggles,
                // The underline's colour only counts with an underline.
                colors: [
                    *color,
                    if underline.is_some() {
                        *underline_color
                    } else {
                        ColorValue::Auto
                    },
                ],
                underline: *underline,
                vertical_position: *vertical_position,
                offsets: [position.0.to_bits(), spacing.0.to_bits()],
                scale: *scale,
                kerning: kerning.map(|kerning| kerning.0.to_bits()),
                // The resolver gives the highlight none as no highlight already.
                highlight: *highlight,
                // A shading of pattern nil is no shading at all.
                shading: shading.filter(|shading| shading.pattern() != Some(ShadingPattern::Nil)),
                ligatures: if open_type_features {
                    *ligatures
                } else {
                    Ligatures::default()
                },
                contextual_alternates: open_type_features && *contextual_alternates,
                cuts,
            },
        }
    }
}

impl Translator<'_> {
    /// The run style of a format, added to the IR the first time a format with its
    /// [`StyleKey`] is met. Formats whose keys are equal share it; the key holds only what
    /// the IR keeps.
    pub(super) fn run_style(&mut self, format: &CharacterFormat) -> ir::RunStyleId {
        let key = StyleKey::of(format, self.open_type_features);
        // The ids of its strings: one not met yet means a key not met yet.
        let strings = &self.run_styles.strings;
        let mut ids = [NO_TAG; 7];
        let mut known = true;
        for (id, string) in ids.iter_mut().zip(key.strings) {
            if let Some(string) = string {
                #[cfg(test)]
                super::work(|work| work.compared += string.len());
                match strings.get(string) {
                    Some(&found) => *id = found,
                    None => known = false,
                }
            }
        }
        if known {
            let owned = OwnedKey {
                strings: ids,
                values: key.values,
            };
            if let Some(&id) = self.run_styles.by_key.get(&owned) {
                return id;
            }
        }
        for (id, string) in ids.iter_mut().zip(key.strings) {
            if let Some(string) = string {
                let next = u32::try_from(self.run_styles.strings.len()).unwrap_or(NO_TAG - 1);
                *id = *self.run_styles.strings.entry(string.into()).or_insert(next);
            }
        }
        self.report_cuts(format);
        let style = self.translate_run_style(format);
        let id = self.ir.add_run_style(style);
        self.run_styles.by_key.insert(
            OwnedKey {
                strings: ids,
                values: key.values,
            },
            id,
        );
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
        #[cfg(test)]
        super::work(|work| work.built += 1);
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
            .with_latin(cut_language(format.language()).map(Into::into))
            .with_east_asia(cut_language(format.language_east_asia()).map(Into::into))
            .with_bidi(cut_language(format.language_bidi()).map(Into::into));
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
            // Not gated by enableOpenTypeFeatures, though its note ([MS-DOCX] 2.3.3) names
            // kerning with a link to w:kern (§17.3.2.19): Word 2007 kerned without the
            // setting. Word's renders asked in #184 decide.
            .with_kern_min(format.kerning())
            .with_highlight(format.highlight().and_then(highlight))
            .with_shading(format.shading().and_then(|shading| self.shading(shading)))
            .with_hidden(format.hidden())
            .with_language(language)
            // Word uses them only when the document turns OpenType features on.
            .with_ligatures({
                let ligatures = if self.open_type_features {
                    format.ligatures()
                } else {
                    Ligatures::default()
                };
                ir::Ligatures::default()
                    .with_standard(ligatures.standard())
                    .with_contextual(ligatures.contextual())
                    .with_historical(ligatures.historical())
                    .with_discretional(ligatures.discretional())
            })
            .with_contextual_alternates(self.open_type_features && format.contextual_alternates())
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
        .find(|&end| {
            #[cfg(test)]
            super::work(|work| work.cut_steps += 1);
            name.is_char_boundary(end)
        })
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
