//! The formatting of runs in effect: the document defaults, the paragraph style, the
//! character style and direct formatting, with theme fonts and theme colours resolved.

use std::sync::Arc;

use officewright_core::Pt;

use super::{Formats, Layer, overwrite, style_index};
use crate::model::Document;
use crate::model::format::{CharacterFormat, FontFamilies};
use crate::model::properties::{
    Color, ColorValue, FontHint, Highlight, Ligatures, RunProperties, Shading, ThemeColor,
    ThemeFont, Toggle, UnderlineStyle, VerticalPosition,
};
use crate::model::settings::{ColorSchemeIndex, ColorSchemeMapping};
use crate::model::styles::StyleKind;

/// Word's font when nothing in the hierarchy gives one (MS-OI29500 §17.3.2.26(c)).
pub(super) const DEFAULT_FONT: &str = "Times New Roman";

/// The font size when nothing in the hierarchy gives one, though the document defaults
/// give run properties. The standard leaves it to the application (§17.3.2.38); not
/// confirmed against Word yet. Without run properties in the document defaults Word
/// gives its own, 11 pt among them (see [`word_run_defaults`]).
const DEFAULT_SIZE: Pt = Pt(10.0);

/// The Latin typefaces of the theme Word uses when a document has no theme part, its
/// default Office theme: major and minor (the theme Word writes into the re-saved copy of
/// the corpus file `word-rules/E-defaults-empty.docx`, Word 16.0.17932). Its East Asian
/// and complex script typefaces are empty, so those slots fall back as with a theme.
const OFFICE_THEME_LATIN: [&str; 2] = ["Calibri Light", "Calibri"];

/// The toggle properties of ECMA-376 §17.7.3.
const TOGGLES: [Toggle; 12] = [
    Toggle::Bold,
    Toggle::BoldComplexScript,
    Toggle::Italic,
    Toggle::ItalicComplexScript,
    Toggle::Caps,
    Toggle::SmallCaps,
    Toggle::Strike,
    Toggle::Outline,
    Toggle::Shadow,
    Toggle::Emboss,
    Toggle::Imprint,
    Toggle::Vanish,
];

impl Formats<'_> {
    /// Combines the levels of the hierarchy for a run whose paragraph style has the index
    /// `paragraph_style` and whose own properties are `direct`, with `label`, what a list
    /// level gives to its label, laid over them. A run without a `w:rStyle` gets no
    /// character style: Word's default character style is DefaultParagraphFont, whose
    /// children it ignores.
    pub(super) fn resolve_run(
        &mut self,
        paragraph_style: Option<usize>,
        direct: &RunProperties,
        label: Option<&CharacterLevel>,
    ) -> CharacterFormat {
        let document = self.document;
        let defaults = self.run_defaults();
        let paragraph = paragraph_style.map(|index| {
            self.characters
                .level(document, index, &CharacterLevel::default())
        });
        let character_style = direct
            .style
            .as_deref()
            .and_then(|id| style_index(document, id, StyleKind::Character));
        let character = character_style.map(|index| {
            self.characters
                .level(document, index, &CharacterLevel::default())
        });
        let direct = CharacterLevel::of(direct);

        // Each level replaces what it gives.
        let mut effect = CharacterLevel::clone(&defaults);
        for level in paragraph.iter().chain(character.iter()) {
            effect.overlay(level);
        }
        effect.overlay(&direct);
        if let Some(label) = label {
            effect.overlay(label);
        }

        // On/off properties: toggle properties by ECMA-376 §17.7.3, the others by the
        // last level that gives them.
        let value = |level: Option<&CharacterLevel>, toggle: usize| {
            level.and_then(|level| level.toggles.get(toggle).copied().flatten())
        };
        let mut toggles = [false; Toggle::COUNT];
        for (index, slot) in toggles.iter_mut().enumerate() {
            let is = |toggle: Toggle| toggle as usize == index;
            let levels = (
                value(Some(&defaults), index),
                value(paragraph.as_deref(), index),
                value(character.as_deref(), index),
                value(Some(&direct), index),
            );
            *slot = if TOGGLES.into_iter().any(is) {
                toggle(levels.0, levels.1, levels.2, levels.3)
            } else {
                // Word ignores webHidden in a character style (MS-OI29500 §17.3.2.44(a)).
                let character = levels.2.filter(|_| !is(Toggle::WebHidden));
                levels
                    .3
                    .or(character)
                    .or(levels.1)
                    .or(levels.0)
                    .unwrap_or(false)
            };
            // The list level's value, like direct formatting, sets it, as [MS-DOC] applies
            // the level's properties to the number text ("Determining List Formatting of
            // a Paragraph", part 3, step 2); not confirmed against Word yet (#162).
            if let Some(value) = value(label, index) {
                *slot = value;
            }
        }
        // Word ignores specVanish unless the text is hidden too (MS-OI29500 §17.3.2.36(a)).
        if toggles.get(Toggle::Vanish as usize) != Some(&true) {
            if let Some(spec_vanish) = toggles.get_mut(Toggle::SpecVanish as usize) {
                *spec_vanish = false;
            }
        }

        CharacterFormat {
            style: character_style
                .and_then(|index| document.styles.styles.get(index))
                .map(|style| Arc::clone(&style.id)),
            fonts: self.families(&effect),
            size: effect.size.unwrap_or(DEFAULT_SIZE),
            size_complex_script: effect.size_complex_script.unwrap_or(DEFAULT_SIZE),
            toggles,
            color: effect
                .color
                .as_ref()
                .map_or(ColorValue::Auto, |color| resolve_color(document, color)),
            underline: effect
                .underline_style
                .filter(|style| *style != UnderlineStyle::None),
            underline_color: effect
                .underline_color
                .as_ref()
                .map_or(ColorValue::Auto, |color| resolve_color(document, color)),
            vertical_position: effect
                .vertical_position
                .unwrap_or(VerticalPosition::Baseline),
            position: effect.position.unwrap_or(Pt(0.0)),
            spacing: effect.spacing.unwrap_or(Pt(0.0)),
            scale: effect.scale.unwrap_or(100),
            // A kerning size of 0 is no kerning, as Word writes it.
            kerning: effect.kerning.filter(|size| size.0 > 0.0),
            highlight: effect
                .highlight
                .filter(|highlight| *highlight != Highlight::None),
            shading: effect.shading,
            language_latin: effect.languages[0].clone(),
            language_east_asia: effect.languages[1].clone(),
            language_bidi: effect.languages[2].clone(),
            ligatures: effect.ligatures.unwrap_or_default(),
            contextual_alternates: effect.contextual_alternates.unwrap_or(false),
        }
    }

    /// What the document defaults give to runs; Word's own when they give no run
    /// properties.
    fn run_defaults(&mut self) -> Arc<CharacterLevel> {
        let document = self.document;
        Arc::clone(self.character_defaults.get_or_insert_with(|| {
            let styles = crate::model::Styles::new(document);
            Arc::new(if styles.has_default_run_properties() {
                CharacterLevel::of(&styles.default_run_properties())
            } else {
                word_run_defaults()
            })
        }))
    }

    /// The font families of a level, theme fonts resolved through the document's theme,
    /// each theme font once for all runs. The names are shared with the level, not
    /// copied, however long they are.
    fn families(&mut self, level: &CharacterLevel) -> FontFamilies {
        let mut slots: [Option<Arc<str>>; 4] = Default::default();
        for (slot, font) in slots.iter_mut().zip(&level.fonts) {
            *slot = match font {
                Some(FontSlot::Name(name)) => Some(Arc::clone(name)),
                Some(FontSlot::Theme(theme, written)) => {
                    self.theme_font(*theme).or_else(|| written.clone())
                }
                None => None,
            };
        }
        let [ascii, high_ansi, east_asia, complex_script] =
            slots.map(|slot| slot.unwrap_or_else(|| Arc::clone(&self.default_font)));
        FontFamilies {
            ascii,
            high_ansi,
            east_asia,
            complex_script,
            hint: level.hint,
        }
    }

    /// The typeface a theme font stands for, looked up once per theme font.
    fn theme_font(&mut self, font: ThemeFont) -> Option<Arc<str>> {
        let document = self.document;
        self.theme_fonts
            .entry(font)
            .or_insert_with(|| theme_font(document, font).map(Into::into))
            .clone()
    }
}

/// The run properties Word gives when the document defaults give none (no `w:rPr` in
/// `w:docDefaults/w:rPrDefault`): those it writes into `w:rPrDefault` when it saves such
/// a document again (the re-saved copy of the corpus file
/// `word-rules/E-defaults-empty.docx`, Word 16.0.17932), whose render shows Calibri
/// 11 pt. They are the theme's minor fonts, 11 pt, kerning from 1 pt and standard and
/// contextual ligatures (`w14:ligatures`). Word also writes languages there, which may
/// be those of its user's editing settings and are left out.
fn word_run_defaults() -> CharacterLevel {
    let theme = |font| Some(FontSlot::Theme(font, None));
    CharacterLevel {
        fonts: [
            theme(ThemeFont::MinorHAnsi),
            theme(ThemeFont::MinorHAnsi),
            theme(ThemeFont::MinorEastAsia),
            theme(ThemeFont::MinorBidi),
        ],
        size: Some(Pt(11.0)),
        size_complex_script: Some(Pt(11.0)),
        kerning: Some(Pt(1.0)),
        ligatures: Some(Ligatures {
            standard: true,
            contextual: true,
            ..Ligatures::default()
        }),
        ..CharacterLevel::default()
    }
}

/// The value of a toggle property in effect, from the value each level of the hierarchy
/// gives: the document defaults, the paragraph style and the character style, each the
/// first value up its `w:basedOn` chain, and direct formatting.
///
/// Direct formatting sets the value (ECMA-376 §17.7.3). A paragraph style sets it rather
/// than combining with what is below, as Word does (MS-OI29500 §17.7.8(a)). A character
/// style flips the value when its own differs from the document defaults': with defaults
/// of false that is the standard's XOR of the levels that give true; with defaults of
/// true it is Word's rule that the value is true when an even number of levels give
/// false, a level without a value taking the defaults' (MS-OI29500 §17.7.3(a), (b)).
/// The reference renders of `word-rules/E-style-toggles.docx` and
/// `E-style-toggles-defaults.docx` are to confirm it.
fn toggle(
    defaults: Option<bool>,
    paragraph: Option<bool>,
    character: Option<bool>,
    direct: Option<bool>,
) -> bool {
    let default = defaults.unwrap_or(false);
    let mut value = paragraph.unwrap_or(default);
    if character.is_some_and(|character| character != default) {
        value = !value;
    }
    direct.unwrap_or(value)
}

/// A font as a level writes it: by name, or as a font of the theme, with the name
/// written next to it, which Word writes as the theme font's typeface and which stands in
/// when the theme gives the font no typeface. A document without a theme has Word's
/// default theme.
#[derive(Clone, Debug)]
enum FontSlot {
    Name(Arc<str>),
    Theme(ThemeFont, Option<Arc<str>>),
}

/// What one level of the hierarchy gives — the document defaults, a style with the
/// styles it is based on, or direct formatting — or what several levels give together.
#[derive(Clone, Debug, Default)]
pub(super) struct CharacterLevel {
    /// ascii, hAnsi, eastAsia, cs.
    fonts: [Option<FontSlot>; 4],
    hint: Option<FontHint>,
    toggles: [Option<bool>; Toggle::COUNT],
    color: Option<Color>,
    underline_style: Option<UnderlineStyle>,
    underline_color: Option<Color>,
    size: Option<Pt>,
    size_complex_script: Option<Pt>,
    vertical_position: Option<VerticalPosition>,
    position: Option<Pt>,
    spacing: Option<Pt>,
    scale: Option<u16>,
    kerning: Option<Pt>,
    highlight: Option<Highlight>,
    shading: Option<Shading>,
    languages: [Option<Arc<str>>; 3],
    ligatures: Option<Ligatures>,
    contextual_alternates: Option<bool>,
}

impl CharacterLevel {
    /// What a list level's `w:rPr` gives to its label. Word does not allow `w:rStyle`,
    /// `w:highlight`, `w:oMath` and `w:rPrChange` there (MS-OI29500 §17.9.24(a)); the
    /// note does not say what Word does with a file that has them, and dropping them is
    /// our choice (`w:rPrChange` is never read). None when it gives nothing.
    pub(super) fn of_label(properties: &RunProperties) -> Option<Arc<Self>> {
        let mut level = Self::of(properties);
        level.highlight = None;
        if let Some(math) = level.toggles.get_mut(Toggle::Math as usize) {
            *math = None;
        }
        level.gives().then(|| Arc::new(level))
    }

    /// Whether the level gives anything.
    fn gives(&self) -> bool {
        let Self {
            fonts,
            hint,
            toggles,
            color,
            underline_style,
            underline_color,
            size,
            size_complex_script,
            vertical_position,
            position,
            spacing,
            scale,
            kerning,
            highlight,
            shading,
            languages,
            ligatures,
            contextual_alternates,
        } = self;
        fonts.iter().any(Option::is_some)
            || hint.is_some()
            || toggles.iter().any(Option::is_some)
            || color.is_some()
            || underline_style.is_some()
            || underline_color.is_some()
            || size.is_some()
            || size_complex_script.is_some()
            || vertical_position.is_some()
            || position.is_some()
            || spacing.is_some()
            || scale.is_some()
            || kerning.is_some()
            || highlight.is_some()
            || shading.is_some()
            || languages.iter().any(Option::is_some)
            || ligatures.is_some()
            || contextual_alternates.is_some()
    }

    /// What one `w:rPr` gives.
    fn of(properties: &RunProperties) -> Self {
        let fonts = &properties.fonts;
        // In one w:rFonts the theme attribute wins over the font (§17.3.2.26).
        let slot = |name: &Option<String>, theme: Option<ThemeFont>| match theme {
            Some(theme) => Some(FontSlot::Theme(theme, name.as_deref().map(Into::into))),
            None => name.as_deref().map(|name| FontSlot::Name(name.into())),
        };
        let underline = properties.underline.as_ref();
        let language = &properties.language;
        // Word writes a size of 0 in some linked character styles (Word's own
        // DocumentMapChar and BalloonTextChar in documents of the corpus); it is not a
        // size, so the level gives none.
        let size = |size: Option<Pt>| size.filter(|size| size.0 > 0.0);
        Self {
            fonts: [
                slot(&fonts.ascii, fonts.ascii_theme),
                slot(&fonts.high_ansi, fonts.high_ansi_theme),
                slot(&fonts.east_asia, fonts.east_asia_theme),
                slot(&fonts.complex_script, fonts.complex_script_theme),
            ],
            hint: fonts.hint,
            toggles: properties.toggles,
            color: properties.color,
            // The style and the colour of an underline come from the hierarchy one by
            // one (MS-OI29500 §17.3.2.40(a), (c)).
            underline_style: underline.and_then(|underline| underline.style),
            underline_color: underline
                .map(|underline| underline.color)
                .filter(|color| color.value.is_some() || color.theme.is_some()),
            size: size(properties.size),
            size_complex_script: size(properties.size_complex_script),
            vertical_position: properties.vertical_position,
            position: properties.position,
            spacing: properties.spacing,
            scale: properties.scale,
            kerning: properties.kerning,
            highlight: properties.highlight,
            shading: properties.shading,
            languages: [&language.latin, &language.east_asia, &language.bidi]
                .map(|language| language.as_deref().map(Into::into)),
            ligatures: properties.ligatures,
            contextual_alternates: properties.contextual_alternates,
        }
    }
}

impl Layer for CharacterLevel {
    fn of_style(document: &Document, index: usize) -> Self {
        document
            .styles
            .styles
            .get(index)
            .map(|style| {
                Self::of(&crate::model::Style::from_data(document, style).run_properties())
            })
            .unwrap_or_default()
    }

    /// Lays `above` over this level: what it gives replaces what this one gives. A font
    /// or a theme font written later replaces either (MS-OI29500 §17.3.2.26(e)).
    fn overlay(&mut self, above: &Self) {
        for (slot, font) in self.fonts.iter_mut().zip(&above.fonts) {
            overwrite(slot, font.clone());
        }
        overwrite(&mut self.hint, above.hint);
        for (slot, value) in self.toggles.iter_mut().zip(above.toggles) {
            overwrite(slot, value);
        }
        overwrite(&mut self.color, above.color);
        overwrite(&mut self.underline_style, above.underline_style);
        overwrite(&mut self.underline_color, above.underline_color);
        overwrite(&mut self.size, above.size);
        overwrite(&mut self.size_complex_script, above.size_complex_script);
        overwrite(&mut self.vertical_position, above.vertical_position);
        overwrite(&mut self.position, above.position);
        overwrite(&mut self.spacing, above.spacing);
        overwrite(&mut self.scale, above.scale);
        overwrite(&mut self.kerning, above.kerning);
        overwrite(&mut self.highlight, above.highlight);
        overwrite(&mut self.shading, above.shading);
        for (slot, language) in self.languages.iter_mut().zip(&above.languages) {
            overwrite(slot, language.clone());
        }
        overwrite(&mut self.ligatures, above.ligatures);
        overwrite(&mut self.contextual_alternates, above.contextual_alternates);
    }
}

/// The typeface a theme font stands for (ECMA-376 §17.15.1.88): with `w:themeFontLang`,
/// the theme's font for the script of the language it gives for the slot (`w:val` for
/// ascii and hAnsi, `w:eastAsia`, `w:bidi`); else, or when the theme has no font for
/// that script, the theme's latin, ea or cs font. A document without a theme part has
/// Word's default theme ([`OFFICE_THEME_LATIN`]).
fn theme_font(document: &Document, font: ThemeFont) -> Option<String> {
    let Some(theme) = document.theme.as_ref() else {
        let [major, minor] = OFFICE_THEME_LATIN;
        return match font {
            ThemeFont::MajorAscii | ThemeFont::MajorHAnsi => Some(major.into()),
            ThemeFont::MinorAscii | ThemeFont::MinorHAnsi => Some(minor.into()),
            _ => None,
        };
    };
    let scheme = theme.fonts()?;
    let (collection, slot) = match font {
        ThemeFont::MajorAscii | ThemeFont::MajorHAnsi => (scheme.major(), Slot::Latin),
        ThemeFont::MajorEastAsia => (scheme.major(), Slot::EastAsia),
        ThemeFont::MajorBidi => (scheme.major(), Slot::ComplexScript),
        ThemeFont::MinorAscii | ThemeFont::MinorHAnsi => (scheme.minor(), Slot::Latin),
        ThemeFont::MinorEastAsia => (scheme.minor(), Slot::EastAsia),
        ThemeFont::MinorBidi => (scheme.minor(), Slot::ComplexScript),
    };
    let languages = document.settings.theme_font_languages();
    let (language, by_slot) = match slot {
        Slot::Latin => (languages.and_then(|l| l.latin()), collection.latin()),
        Slot::EastAsia => (
            languages.and_then(|l| l.east_asia()),
            collection.east_asian(),
        ),
        Slot::ComplexScript => (
            languages.and_then(|l| l.bidi()),
            collection.complex_script(),
        ),
    };
    let not_empty = |typeface: &&str| !typeface.is_empty();
    let by_language = language
        .and_then(script_of)
        .and_then(|script| collection.script(script))
        .filter(not_empty);
    by_language.or(by_slot.filter(not_empty)).map(Into::into)
}

#[derive(Clone, Copy)]
enum Slot {
    Latin,
    EastAsia,
    ComplexScript,
}

/// The scripts themes give fonts for, by their ISO 15924 codes (Office's own `Uigh` and
/// `Viet` among them), as a script subtag of a language tag names them.
const THEME_SCRIPTS: [&str; 34] = [
    "Arab", "Armn", "Beng", "Cans", "Cher", "Cyrl", "Ethi", "Geor", "Grek", "Gujr", "Guru", "Hang",
    "Hans", "Hant", "Hebr", "Jpan", "Khmr", "Knda", "Laoo", "Latn", "Mlym", "Mong", "Orya", "Sinh",
    "Syrc", "Taml", "Telu", "Thaa", "Thai", "Tibt", "Uigh", "Viet", "Yiii", "Deva",
];

/// The ISO 15924 code of the script of a language tag, as themes name scripts for their
/// fonts (`Jpan`, `Arab`…): the tag's script subtag when it has one (`zh-Hant`,
/// `mn-Mong-CN`), else the usual script of the language.
fn script_of(language: &str) -> Option<&'static str> {
    let mut parts = language.split(['-', '_']);
    let primary = parts.next()?.to_ascii_lowercase();
    let mut region = String::new();
    for subtag in parts {
        if let Some(script) = THEME_SCRIPTS
            .iter()
            .find(|script| script.eq_ignore_ascii_case(subtag))
        {
            return Some(script);
        }
        if region.is_empty() && subtag.len() == 2 {
            region = subtag.to_ascii_uppercase();
        }
    }
    Some(match primary.as_str() {
        "ja" => "Jpan",
        "ko" => "Hang",
        "zh" if matches!(region.as_str(), "TW" | "HK" | "MO") => "Hant",
        "zh" => "Hans",
        "ar" | "fa" | "ur" | "ps" | "sd" => "Arab",
        "he" | "yi" => "Hebr",
        "th" => "Thai",
        "am" | "ti" => "Ethi",
        "bn" | "as" => "Beng",
        "gu" => "Gujr",
        "km" => "Khmr",
        "kn" => "Knda",
        "pa" => "Guru",
        "iu" => "Cans",
        "chr" => "Cher",
        "ii" => "Yiii",
        "bo" => "Tibt",
        "dv" => "Thaa",
        "hi" | "mr" | "ne" | "sa" | "kok" => "Deva",
        "te" => "Telu",
        "ta" => "Taml",
        "syr" => "Syrc",
        "or" => "Orya",
        "ml" => "Mlym",
        "lo" => "Laoo",
        "si" => "Sinh",
        // Mongolian is written in the Mongolian script in China, in Cyrillic elsewhere.
        "mn" if region == "CN" => "Mong",
        "ru" | "uk" | "be" | "bg" | "mk" | "kk" | "ky" | "tg" | "mn" => "Cyrl",
        "el" => "Grek",
        "vi" => "Viet",
        "ug" => "Uigh",
        "ka" => "Geor",
        "hy" => "Armn",
        _ => return None,
    })
}

/// A colour in effect: a theme colour through the colour mapping and the theme, with its
/// tint or shade; else the value written, or automatic.
pub(super) fn resolve_color(document: &Document, color: &Color) -> ColorValue {
    let themed = color
        .theme
        .filter(|theme| *theme != ThemeColor::None)
        .and_then(|theme| theme_color(document, theme))
        .map(|(red, green, blue)| {
            // Word applies the tint when both are written (MS-OI29500 §17.3.2.6(b)).
            let (red, green, blue) = match (color.tint, color.shade) {
                (Some(tint), _) => adjust_luminance((red, green, blue), Adjust::Tint(tint)),
                (None, Some(shade)) => adjust_luminance((red, green, blue), Adjust::Shade(shade)),
                (None, None) => (red, green, blue),
            };
            ColorValue::Rgb(red, green, blue)
        });
    themed.or(color.value).unwrap_or(ColorValue::Auto)
}

/// The RGB value of a theme colour, through the document's colour mapping.
fn theme_color(document: &Document, theme: ThemeColor) -> Option<(u8, u8, u8)> {
    use ColorSchemeIndex as Index;
    let mapping = document.settings.color_scheme_mapping();
    let map = |pick: fn(&ColorSchemeMapping) -> Option<Index>| mapping.and_then(pick);
    // The colour the mapping gives, and the one a missing mapping stands for: text 1 and
    // 2 stand for dark 1 and 2 (MS-OI29500 §17.15.1.20(a), (b)).
    let (mapped, default) = match theme {
        ThemeColor::Dark1 => (None, Index::Dark1),
        ThemeColor::Light1 => (None, Index::Light1),
        ThemeColor::Dark2 => (None, Index::Dark2),
        ThemeColor::Light2 => (None, Index::Light2),
        ThemeColor::Text1 => (map(ColorSchemeMapping::text1), Index::Dark1),
        ThemeColor::Background1 => (map(ColorSchemeMapping::background1), Index::Light1),
        ThemeColor::Text2 => (map(ColorSchemeMapping::text2), Index::Dark2),
        ThemeColor::Background2 => (map(ColorSchemeMapping::background2), Index::Light2),
        ThemeColor::Accent1 => (map(|m| m.accent(1)), Index::Accent1),
        ThemeColor::Accent2 => (map(|m| m.accent(2)), Index::Accent2),
        ThemeColor::Accent3 => (map(|m| m.accent(3)), Index::Accent3),
        ThemeColor::Accent4 => (map(|m| m.accent(4)), Index::Accent4),
        ThemeColor::Accent5 => (map(|m| m.accent(5)), Index::Accent5),
        ThemeColor::Accent6 => (map(|m| m.accent(6)), Index::Accent6),
        ThemeColor::Hyperlink => (map(ColorSchemeMapping::hyperlink), Index::Hyperlink),
        ThemeColor::FollowedHyperlink => (
            map(ColorSchemeMapping::followed_hyperlink),
            Index::FollowedHyperlink,
        ),
        ThemeColor::None => return None,
    };
    let index = mapped.unwrap_or(default);
    document.theme.as_ref()?.colors()?.color(index)?.rgb()
}

#[derive(Clone, Copy)]
enum Adjust {
    Tint(u8),
    Shade(u8),
}

/// A tint or shade applied to the luminance of the colour in HSL, as Word does
/// (MS-OI29500 §17.3.2.6(a), (c)): a shade s multiplies it by s / 255, a tint t moves it
/// towards white, to L · t / 255 + 1 − t / 255. The channels are truncated: that gives
/// the implementer notes' examples, 4F81BD with tint 99 as 95B3D7 exactly and C0504D with
/// shade BF as 943734 for Word's 943634; how Word rounds is not documented.
fn adjust_luminance((red, green, blue): (u8, u8, u8), adjust: Adjust) -> (u8, u8, u8) {
    let (hue, saturation, luminance) = to_hsl(red, green, blue);
    let luminance = match adjust {
        Adjust::Shade(shade) => luminance * f64::from(shade) / 255.0,
        Adjust::Tint(tint) => {
            let tint = f64::from(tint) / 255.0;
            luminance * tint + (1.0 - tint)
        }
    };
    from_hsl(hue, saturation, luminance.clamp(0.0, 1.0))
}

fn to_hsl(red: u8, green: u8, blue: u8) -> (f64, f64, f64) {
    let [r, g, b] = [red, green, blue].map(|c| f64::from(c) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let luminance = (max + min) / 2.0;
    let delta = max - min;
    if delta == 0.0 {
        return (0.0, 0.0, luminance);
    }
    let saturation = if luminance > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } / 6.0;
    (hue, saturation, luminance)
}

fn from_hsl(hue: f64, saturation: f64, luminance: f64) -> (u8, u8, u8) {
    let chroma = (1.0 - (2.0 * luminance - 1.0).abs()) * saturation;
    let sector = hue * 6.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector {
        s if s < 1.0 => (chroma, x, 0.0),
        s if s < 2.0 => (x, chroma, 0.0),
        s if s < 3.0 => (0.0, chroma, x),
        s if s < 4.0 => (0.0, x, chroma),
        s if s < 5.0 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = luminance - chroma / 2.0;
    let channel = |c: f64| {
        // The small addition keeps a channel that comes back as 191.999… from rounding
        // errors at 191, so that a colour without tint or shade keeps its value.
        // reason: the value is clamped to 0..=255 before the conversion.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let byte = ((c + m) * 255.0 + 1e-9).floor().clamp(0.0, 255.0) as u8;
        byte
    };
    (channel(r), channel(g), channel(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_script_of_a_language_tag() {
        let cases = [
            ("ja-JP", Some("Jpan")),
            ("zh-CN", Some("Hans")),
            ("zh-TW", Some("Hant")),
            ("zh-Hant", Some("Hant")),
            ("zh-Hans-HK", Some("Hans")),
            ("mn-MN", Some("Cyrl")),
            ("mn-Mong-CN", Some("Mong")),
            ("sr-Latn-RS", Some("Latn")),
            ("ar_SA", Some("Arab")),
            ("en-US", None),
            ("", None),
        ];
        for (language, script) in cases {
            assert_eq!(script_of(language), script, "{language}");
        }
    }

    #[test]
    fn a_full_tint_or_shade_keeps_the_colour() {
        for red in (0..=255).step_by(5) {
            for green in (0..=255).step_by(5) {
                for blue in (0..=255).step_by(5) {
                    let color = (red, green, blue);
                    assert_eq!(adjust_luminance(color, Adjust::Tint(255)), color);
                    assert_eq!(adjust_luminance(color, Adjust::Shade(255)), color);
                }
            }
        }
        assert_eq!(
            adjust_luminance((1, 2, 3), Adjust::Tint(0)),
            (255, 255, 255)
        );
        assert_eq!(adjust_luminance((250, 2, 3), Adjust::Shade(0)), (0, 0, 0));
    }
}
