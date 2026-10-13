//! Resolving the formatting in effect (ECMA-376 Part 1, §17.7.2): the document defaults,
//! the paragraph's style, the run's character style and direct formatting, combined as
//! Word combines them, with theme fonts and theme colours resolved.

use std::collections::HashMap;
use std::sync::Arc;

use officewright_core::Pt;

use super::format::{CharacterFormat, FontFamilies};
use super::properties::{
    Color, ColorValue, FontHint, Highlight, RunProperties, Shading, ThemeColor, ThemeFont, Toggle,
    UnderlineStyle, VerticalPosition,
};
use super::settings::{ColorSchemeIndex, ColorSchemeMapping};
use super::styles::{StyleData, StyleKind};
use super::views::{Paragraph, Run};
use super::{Document, Kind, Node};

/// Word's font when nothing in the hierarchy gives one (MS-OI29500 §17.3.2.26(c)).
const DEFAULT_FONT: &str = "Times New Roman";

/// The font size when nothing in the hierarchy gives one. The standard leaves it to the
/// application (§17.3.2.38); 10 pt is to be confirmed by the reference render of the
/// corpus file `word-rules/E-defaults-empty.docx`.
const DEFAULT_SIZE: Pt = Pt(10.0);

/// How far a `w:basedOn` chain is followed; Word does not allow cycles
/// (MS-OI29500 §17.7.4.3(a)), and a cycle or a longer chain stops here.
const MAX_CHAIN: usize = 64;

/// Styles whose children Word ignores (MS-OI29500 §17.7.4.17(a)).
const IGNORED_STYLES: [&str; 3] = ["NoList", "DefaultParagraphFont", "TableNormal"];

/// The properties ECMA-376 §17.7.3 makes toggle properties.
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

/// Resolves the formatting in effect of runs and paragraph marks, keeping what it has
/// worked out about styles for the next ones: use one for the runs of a whole
/// document. It borrows the document; threads that format runs at the same time each
/// use their own.
#[derive(Debug)]
pub struct Formats<'a> {
    document: &'a Document,
    /// The run properties of each style's `w:basedOn` chain, the root first, by the
    /// style's index in the document's styles.
    chains: HashMap<usize, Arc<Vec<RunProperties>>>,
    defaults: Option<Arc<RunProperties>>,
    /// The index of the default paragraph style, once looked up.
    default_paragraph_style: Option<Option<usize>>,
}

impl<'a> Formats<'a> {
    pub(super) fn new(document: &'a Document) -> Self {
        Self {
            document,
            chains: HashMap::new(),
            defaults: None,
            default_paragraph_style: None,
        }
    }

    /// The formatting of a run in effect.
    pub fn run(&mut self, run: Run<'a>) -> CharacterFormat {
        if !std::ptr::eq(run.document(), self.document) {
            // A run of another document takes that document's styles.
            return Formats::new(run.document()).run(run);
        }
        let direct = run.properties();
        let paragraph = self.paragraph_style(paragraph_of(self.document, run.node()));
        self.resolve(paragraph, &direct)
    }

    /// The formatting of a paragraph's mark in effect, which decides, among other
    /// things, the height of an empty paragraph.
    pub fn paragraph_mark(&mut self, paragraph: Paragraph<'a>) -> CharacterFormat {
        if !std::ptr::eq(paragraph.document(), self.document) {
            return Formats::new(paragraph.document()).paragraph_mark(paragraph);
        }
        let direct = paragraph.properties().mark;
        let style = self.paragraph_style(Some(paragraph.node()));
        self.resolve(style, &direct)
    }

    /// The index of the paragraph style of the paragraph `node`: its `w:pStyle` when it
    /// names a paragraph style, else the default paragraph style (the first, as
    /// [`Styles::default_style`](super::Styles::default_style) gives it).
    fn paragraph_style(&mut self, node: Option<&'a Node>) -> Option<usize> {
        let document = self.document;
        let written = node.and_then(|node| {
            node.properties
                .iter()
                .map(|&span| document.element(span))
                .find(|element| element.is(&document.names.w, "pPr"))
                .and_then(|p_pr| {
                    p_pr.children()
                        .find(|child| child.is(&document.names.w, "pStyle"))
                        .and_then(|p_style| p_style.attribute(&document.names.w, "val"))
                })
        });
        let written = written.and_then(|id| style_index(document, id, StyleKind::Paragraph));
        written.or_else(|| {
            *self.default_paragraph_style.get_or_insert_with(|| {
                document
                    .styles
                    .styles
                    .iter()
                    .position(|style| style.kind == StyleKind::Paragraph && style.is_default)
            })
        })
    }

    /// Combines the levels of the hierarchy for a run whose paragraph style has the index
    /// `paragraph_style` and whose own properties are `direct`.
    fn resolve(
        &mut self,
        paragraph_style: Option<usize>,
        direct: &RunProperties,
    ) -> CharacterFormat {
        let document = self.document;
        let defaults = self.defaults();
        let paragraph = paragraph_style
            .map(|index| self.chain(index))
            .unwrap_or_default();
        let character_style = direct
            .style
            .as_deref()
            .and_then(|id| style_index(document, id, StyleKind::Character));
        let character = character_style
            .map(|index| self.chain(index))
            .unwrap_or_default();

        let mut layered = Layered::default();
        layered.apply(&defaults);
        for level in paragraph.iter().chain(character.iter()) {
            layered.apply(level);
        }
        layered.apply(direct);

        let mut toggles = [false; Toggle::COUNT];
        for (index, slot) in toggles.iter_mut().enumerate() {
            let value = |props: &RunProperties| props.toggles.get(index).copied().flatten();
            let is = |toggle: Toggle| toggle as usize == index;
            *slot = if TOGGLES.into_iter().any(is) {
                toggle(
                    value(&defaults),
                    paragraph.iter().map(value),
                    character.iter().map(value),
                    value(direct),
                )
            } else {
                // Not a toggle property: the last level that sets it counts. Word ignores
                // webHidden in a character style (MS-OI29500 §17.3.2.44(a)).
                let character = character.iter().filter(|_| !is(Toggle::WebHidden));
                std::iter::once(&*defaults)
                    .chain(paragraph.iter())
                    .chain(character)
                    .chain(std::iter::once(direct))
                    .rev()
                    .find_map(value)
                    .unwrap_or(false)
            };
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
                .map(|style| style.id.clone()),
            fonts: layered.fonts(document),
            size: layered.size.unwrap_or(DEFAULT_SIZE),
            size_complex_script: layered.size_complex_script.unwrap_or(DEFAULT_SIZE),
            toggles,
            color: layered
                .color
                .as_ref()
                .map_or(ColorValue::Auto, |color| resolve_color(document, color)),
            underline: layered
                .underline_style
                .filter(|style| *style != UnderlineStyle::None),
            underline_color: layered
                .underline_color
                .as_ref()
                .map_or(ColorValue::Auto, |color| resolve_color(document, color)),
            vertical_position: layered
                .vertical_position
                .unwrap_or(VerticalPosition::Baseline),
            position: layered.position.unwrap_or(Pt(0.0)),
            spacing: layered.spacing.unwrap_or(Pt(0.0)),
            scale: layered.scale.unwrap_or(100),
            // A kerning size of 0 is no kerning, as Word writes it.
            kerning: layered.kerning.filter(|size| size.0 > 0.0),
            highlight: layered
                .highlight
                .filter(|highlight| *highlight != Highlight::None),
            shading: layered.shading,
            language_latin: layered.languages[0].clone(),
            language_east_asia: layered.languages[1].clone(),
            language_bidi: layered.languages[2].clone(),
        }
    }

    /// The run properties of the document defaults.
    fn defaults(&mut self) -> Arc<RunProperties> {
        let document = self.document;
        Arc::clone(
            self.defaults.get_or_insert_with(|| {
                Arc::new(super::Styles::new(document).default_run_properties())
            }),
        )
    }

    /// The run properties of the `w:basedOn` chain of the style at `index`, the root
    /// first. A `w:basedOn` naming a style of another kind is ignored (ECMA-376 Part 1,
    /// §17.7.4.3).
    fn chain(&mut self, index: usize) -> Arc<Vec<RunProperties>> {
        if let Some(chain) = self.chains.get(&index) {
            return Arc::clone(chain);
        }
        let document = self.document;
        let mut styles: Vec<&StyleData> = Vec::new();
        let mut current = document.styles.styles.get(index);
        while let Some(style) = current {
            if styles.len() >= MAX_CHAIN || styles.iter().any(|seen| std::ptr::eq(*seen, style)) {
                break;
            }
            styles.push(style);
            current = style
                .based_on
                .as_deref()
                .and_then(|id| style_index(document, id, style.kind))
                .and_then(|index| document.styles.styles.get(index));
        }
        let chain: Vec<RunProperties> = styles
            .iter()
            .rev()
            .filter(|style| !IGNORED_STYLES.contains(&&*style.id))
            .map(|style| super::Style::from_data(document, style).run_properties())
            .collect();
        let chain = Arc::new(chain);
        self.chains.insert(index, Arc::clone(&chain));
        chain
    }
}

/// The index of the style with this id when it is of `kind`.
fn style_index(document: &Document, id: &str, kind: StyleKind) -> Option<usize> {
    let index = *document.styles.by_id.get(id)?;
    let style = document.styles.styles.get(index)?;
    (style.kind == kind).then_some(index)
}

/// The paragraph a node is in, through the containers between them.
fn paragraph_of<'a>(document: &'a Document, node: &'a Node) -> Option<&'a Node> {
    let mut current = node;
    for _ in 0..document.nodes.len() {
        if matches!(current.kind, Kind::Paragraph) {
            return Some(current);
        }
        current = document.node(current.parent)?;
    }
    None
}

/// The value of a toggle property in effect.
///
/// The document defaults give the starting value. A paragraph style sets the value
/// rather than toggling it, as Word does (MS-OI29500 §17.7.8(a)). In a character style an
/// on value toggles the value so far and an off value leaves it, as ECMA-376 §17.7.3
/// says; direct formatting sets the value. How Word combines a document default of on
/// with the styles (MS-OI29500 §17.7.3(a), (b)) is to be confirmed by the reference
/// renders of `word-rules/E-style-toggles.docx` and `E-style-toggles-defaults.docx`.
fn toggle(
    defaults: Option<bool>,
    paragraph: impl Iterator<Item = Option<bool>>,
    character: impl Iterator<Item = Option<bool>>,
    direct: Option<bool>,
) -> bool {
    let mut value = defaults.unwrap_or(false);
    for level in paragraph.flatten() {
        value = level;
    }
    for level in character {
        if level == Some(true) {
            value = !value;
        }
    }
    direct.unwrap_or(value)
}

/// A font as a level writes it: by name, or as a font of the theme, with the name
/// written next to it, which Word writes as the theme font's typeface and which stands in
/// when the document has no theme.
#[derive(Clone, Debug)]
enum FontSlot {
    Name(Box<str>),
    Theme(ThemeFont, Option<Box<str>>),
}

/// What the levels set so far, the last one winning each property.
#[derive(Debug, Default)]
struct Layered {
    /// ascii, hAnsi, eastAsia, cs.
    fonts: [Option<FontSlot>; 4],
    hint: Option<FontHint>,
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
    languages: [Option<Box<str>>; 3],
}

impl Layered {
    fn apply(&mut self, properties: &RunProperties) {
        let fonts = &properties.fonts;
        // In one w:rFonts the theme attribute wins over the font; across levels, the last
        // level that writes either wins (MS-OI29500 §17.3.2.26(e)).
        let slots = [
            (&fonts.ascii, fonts.ascii_theme),
            (&fonts.high_ansi, fonts.high_ansi_theme),
            (&fonts.east_asia, fonts.east_asia_theme),
            (&fonts.complex_script, fonts.complex_script_theme),
        ];
        for (slot, (name, theme)) in self.fonts.iter_mut().zip(slots) {
            if let Some(theme) = theme {
                let written = name.as_deref().map(Into::into);
                *slot = Some(FontSlot::Theme(theme, written));
            } else if let Some(name) = name {
                *slot = Some(FontSlot::Name(name.as_str().into()));
            }
        }
        overwrite(&mut self.hint, fonts.hint);
        if properties.color.is_some() {
            self.color = properties.color;
        }
        if let Some(underline) = &properties.underline {
            // The style and the colour of an underline come from the hierarchy one by
            // one (MS-OI29500 §17.3.2.40(a), (c)).
            overwrite(&mut self.underline_style, underline.style);
            if underline.color.value.is_some() || underline.color.theme.is_some() {
                self.underline_color = Some(underline.color);
            }
        }
        overwrite(&mut self.size, properties.size);
        overwrite(
            &mut self.size_complex_script,
            properties.size_complex_script,
        );
        overwrite(&mut self.vertical_position, properties.vertical_position);
        overwrite(&mut self.position, properties.position);
        overwrite(&mut self.spacing, properties.spacing);
        overwrite(&mut self.scale, properties.scale);
        overwrite(&mut self.kerning, properties.kerning);
        overwrite(&mut self.highlight, properties.highlight);
        if properties.shading.is_some() {
            self.shading = properties.shading;
        }
        let language = &properties.language;
        for (slot, value) in
            self.languages
                .iter_mut()
                .zip([&language.latin, &language.east_asia, &language.bidi])
        {
            if let Some(value) = value {
                *slot = Some(value.as_str().into());
            }
        }
    }

    /// The font families, theme fonts resolved through the document's theme.
    fn fonts(&self, document: &Document) -> FontFamilies {
        let [ascii, high_ansi, east_asia, complex_script] =
            self.fonts.clone().map(|slot| match slot {
                Some(FontSlot::Name(name)) => name,
                Some(FontSlot::Theme(theme, written)) => theme_font(document, theme)
                    .map(Into::into)
                    .or(written)
                    .unwrap_or_else(|| DEFAULT_FONT.into()),
                None => DEFAULT_FONT.into(),
            });
        FontFamilies {
            ascii,
            high_ansi,
            east_asia,
            complex_script,
            hint: self.hint,
        }
    }
}

fn overwrite<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

/// The typeface a theme font stands for. For East Asian and complex-script slots left
/// empty in the theme, the font the theme gives for the script of the document's theme
/// font language (`w:themeFontLang`).
fn theme_font(document: &Document, font: ThemeFont) -> Option<String> {
    let scheme = document.theme.as_ref()?.fonts()?;
    let (collection, kind) = match font {
        ThemeFont::MajorAscii | ThemeFont::MajorHAnsi => (scheme.major(), Slot::Latin),
        ThemeFont::MajorEastAsia => (scheme.major(), Slot::EastAsia),
        ThemeFont::MajorBidi => (scheme.major(), Slot::ComplexScript),
        ThemeFont::MinorAscii | ThemeFont::MinorHAnsi => (scheme.minor(), Slot::Latin),
        ThemeFont::MinorEastAsia => (scheme.minor(), Slot::EastAsia),
        ThemeFont::MinorBidi => (scheme.minor(), Slot::ComplexScript),
    };
    let languages = document.settings.theme_font_languages();
    let (written, language) = match kind {
        Slot::Latin => (collection.latin(), None),
        Slot::EastAsia => (
            collection.east_asian(),
            languages.and_then(|languages| languages.east_asia()),
        ),
        Slot::ComplexScript => (
            collection.complex_script(),
            languages.and_then(|languages| languages.bidi()),
        ),
    };
    written
        .filter(|typeface| !typeface.is_empty())
        .or_else(|| collection.script(script_of(language?)?))
        .map(Into::into)
}

#[derive(Clone, Copy)]
enum Slot {
    Latin,
    EastAsia,
    ComplexScript,
}

/// The ISO 15924 code of the usual script of a language, as themes name scripts for
/// their fonts (`Jpan`, `Arab`…), for the languages whose scripts themes list.
fn script_of(language: &str) -> Option<&'static str> {
    let mut parts = language.split(['-', '_']);
    let primary = parts.next()?.to_ascii_lowercase();
    let region = parts.next().unwrap_or("").to_ascii_uppercase();
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
        "mn" => "Mong",
        "vi" => "Viet",
        "ug" => "Uigh",
        "ka" => "Geor",
        "hy" => "Armn",
        _ => return None,
    })
}

/// A colour in effect: a theme colour through the colour mapping and the theme, with its
/// tint or shade; else the value written, or automatic.
fn resolve_color(document: &Document, color: &Color) -> ColorValue {
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
