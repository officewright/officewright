//! The colours of shapes: DrawingML's (ECMA-376 Part 1, §20.1.2.3), a colour and its
//! transforms, and VML's (Part 4, §19.1.2, ST_ColorType), resolved to red, green and blue
//! through the theme and the document's colour mapping.

use super::drawing::{percentage, value};
use super::properties::ThemeColor;
use super::xml::Element;
use super::{Document, Names};
use crate::docx::properties::Report;

/// A colour in effect: red, green and blue, and how opaque it is, from 0 (clear) to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Paint {
    pub(crate) rgb: (u8, u8, u8),
    pub(crate) opacity: f64,
}

/// The colour of `parent` (`a:solidFill`, `a:fillRef` and the like): its first colour
/// element, with the transforms in it applied in their order. `placeholder` stands for
/// `a:schemeClr val="phClr"`, the colour a style reference gives. A colour of the theme
/// is resolved through `document`'s colour mapping and theme, and is none without it, as
/// when the values are checked while the document is read. None also when it has no
/// colour, or one that names nothing the document has; values not taken are reported and
/// read as not written.
pub(crate) fn drawing_color(
    parent: Element<'_>,
    names: &Names,
    document: Option<&Document>,
    placeholder: Option<Paint>,
    report: Report<'_>,
) -> Option<Paint> {
    color_of(parent, names, document, placeholder, usize::MAX, report).0
}

/// The colour of `parent` as [`drawing_color`] reads it, with at most `max_transforms` of
/// its transforms applied, and whether it has more, which are left out. Its transforms are
/// read, and reported when not taken, also when its colour names nothing.
pub(crate) fn color_of(
    parent: Element<'_>,
    names: &Names,
    document: Option<&Document>,
    placeholder: Option<Paint>,
    max_transforms: usize,
    report: Report<'_>,
) -> (Option<Paint>, bool) {
    match color_element(parent, names) {
        Some(element) => color_of_element(
            element,
            names,
            document,
            placeholder,
            max_transforms,
            report,
        ),
        None => (None, false),
    }
}

/// The colour element of `parent` (`a:srgbClr` and the others): its first.
pub(crate) fn color_element<'a>(parent: Element<'a>, names: &Names) -> Option<Element<'a>> {
    parent.children_through_alternates(names.mc).find(|child| {
        matches!(
            child.local_in(&names.a),
            Some("srgbClr" | "scrgbClr" | "hslClr" | "sysClr" | "schemeClr" | "prstClr")
        )
    })
}

/// A colour element as [`color_of`] reads the colour of its parent.
pub(crate) fn color_of_element(
    element: Element<'_>,
    names: &Names,
    document: Option<&Document>,
    placeholder: Option<Paint>,
    max_transforms: usize,
    report: Report<'_>,
) -> (Option<Paint>, bool) {
    let base = base(element, names, document, placeholder, report);
    let mut paint = base.unwrap_or(Paint {
        rgb: (0, 0, 0),
        opacity: 1.0,
    });
    let mut rgb = [paint.rgb.0, paint.rgb.1, paint.rgb.2].map(|c| f64::from(c) / 255.0);
    let (mut applied, mut cut) = (0usize, false);
    for transform in element.children_through_alternates(names.mc) {
        let Some(local) = transform.local_in(&names.a) else {
            continue;
        };
        if applied == max_transforms {
            cut = true;
            break;
        }
        applied += 1;
        if matches!(local, "comp" | "inv" | "gray") {
            rgb = match local {
                "inv" => rgb.map(|c| 1.0 - c),
                "gray" => {
                    let [r, g, b] = rgb;
                    [0.3 * r + 0.59 * g + 0.11 * b; 3]
                }
                _ => {
                    let (hue, saturation, luminance) = to_hsl(rgb);
                    from_hsl(((hue + 180.0) % 360.0, saturation, luminance))
                }
            };
            continue;
        }
        let read = match local {
            "hue" | "hueOff" => angle,
            _ => percentage,
        };
        let Some(amount) = value(transform, names, "val", read, report) else {
            continue;
        };
        match local {
            "alpha" => paint.opacity = amount,
            "alphaMod" => paint.opacity *= amount,
            "alphaOff" => paint.opacity += amount,
            "shade" | "tint" => {
                // In linear light, as Word's VML fallbacks of shapes have such colours
                // (LibreOffice's fdo74401.docx: 4F81BD shaded 50 % is 385D8A), also the
                // stops it gives a gradient of a colour of the scheme with several
                // transforms (fdo73247.docx: 936545, D49366, FDAF7B). A colour of the scheme
                // with one shade or tint has its old index there (`#243f60 [1604]`) and a
                // value in its luminance, as Word's older colour model has it; what Word
                // draws for those no render shows yet (#468).
                rgb = rgb.map(|c| {
                    let linear = to_linear(c);
                    from_linear(if local == "shade" {
                        linear * amount
                    } else {
                        linear * amount + (1.0 - amount)
                    })
                });
            }
            "hue" | "hueOff" | "hueMod" | "sat" | "satOff" | "satMod" | "lum" | "lumOff"
            | "lumMod" => {
                let (mut hue, mut saturation, mut luminance) = to_hsl(rgb);
                match local {
                    "hue" => hue = amount,
                    "hueOff" => hue += amount,
                    "hueMod" => hue *= amount,
                    "sat" => saturation = amount,
                    "satOff" => saturation += amount,
                    "satMod" => saturation *= amount,
                    "lum" => luminance = amount,
                    "lumOff" => luminance += amount,
                    _ => luminance *= amount,
                }
                rgb = from_hsl((
                    hue.rem_euclid(360.0),
                    saturation.clamp(0.0, 1.0),
                    luminance.clamp(0.0, 1.0),
                ));
            }
            // The transforms of single channels and of gamma are not applied.
            _ => {}
        }
    }
    if base.is_none() {
        return (None, cut);
    }
    let [r, g, b] = rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    paint.rgb = (r, g, b);
    paint.opacity = paint.opacity.clamp(0.0, 1.0);
    (Some(paint), cut)
}

/// A colour element without its transforms.
fn base(
    element: Element<'_>,
    names: &Names,
    document: Option<&Document>,
    placeholder: Option<Paint>,
    report: Report<'_>,
) -> Option<Paint> {
    let opaque = |rgb| Paint { rgb, opacity: 1.0 };
    match element.local_name() {
        "srgbClr" => value(element, names, "val", hex, report).map(opaque),
        "sysClr" => value(element, names, "lastClr", hex, report).map(opaque),
        "scrgbClr" => {
            let mut channel = |name| {
                value(element, names, name, percentage, report)
                    .map(|c| (from_linear(c.clamp(0.0, 1.0)) * 255.0).round() as u8)
            };
            Some(opaque((channel("r")?, channel("g")?, channel("b")?)))
        }
        "hslClr" => {
            let hue = value(element, names, "hue", angle, report)?;
            let saturation = value(element, names, "sat", percentage, report)?;
            let luminance = value(element, names, "lum", percentage, report)?;
            let [r, g, b] = from_hsl((
                hue.rem_euclid(360.0),
                saturation.clamp(0.0, 1.0),
                luminance.clamp(0.0, 1.0),
            ))
            .map(|c| (c * 255.0).round() as u8);
            Some(opaque((r, g, b)))
        }
        "prstClr" => value(element, names, "val", preset, report).map(opaque),
        _ => {
            let scheme = value(element, names, "val", scheme_color, report)?;
            match scheme {
                None => placeholder,
                Some(theme) => super::resolve::theme_color(document?, theme).map(opaque),
            }
        }
    }
}

/// A colour of the theme's scheme (ST_SchemeColorVal) as the theme colour of
/// WordprocessingML that the document's colour mapping maps the same way; none for
/// `phClr`, the colour a style reference gives.
fn scheme_color(value: &str) -> Option<Option<ThemeColor>> {
    Some(Some(match value {
        "phClr" => return Some(None),
        "bg1" => ThemeColor::Background1,
        "tx1" => ThemeColor::Text1,
        "bg2" => ThemeColor::Background2,
        "tx2" => ThemeColor::Text2,
        "dk1" => ThemeColor::Dark1,
        "lt1" => ThemeColor::Light1,
        "dk2" => ThemeColor::Dark2,
        "lt2" => ThemeColor::Light2,
        "accent1" => ThemeColor::Accent1,
        "accent2" => ThemeColor::Accent2,
        "accent3" => ThemeColor::Accent3,
        "accent4" => ThemeColor::Accent4,
        "accent5" => ThemeColor::Accent5,
        "accent6" => ThemeColor::Accent6,
        "hlink" => ThemeColor::Hyperlink,
        "folHlink" => ThemeColor::FollowedHyperlink,
        _ => return None,
    }))
}

/// Six hexadecimal digits (ST_HexColorRGB).
fn hex(value: &str) -> Option<(u8, u8, u8)> {
    if value.len() != 6 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(value.get(at..at + 2)?, 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

/// An angle of 1/60,000 degree (ST_PositiveFixedAngle, ST_FixedAngle), in degrees.
fn angle(value: &str) -> Option<f64> {
    value
        .parse::<i32>()
        .ok()
        .map(|value| f64::from(value) / 60_000.0)
}

/// A colour channel of sRGB in linear light (IEC 61966-2-1).
fn to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A colour channel in linear light in sRGB.
fn from_linear(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Red, green and blue from 0 to 1 as hue in degrees, saturation and luminance.
fn to_hsl([r, g, b]: [f64; 3]) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let luminance = (max + min) / 2.0;
    let delta = max - min;
    if delta <= 0.0 {
        return (0.0, 0.0, luminance);
    }
    let saturation = if luminance <= 0.5 {
        delta / (max + min)
    } else {
        delta / (2.0 - max - min)
    };
    let hue = if max == r {
        (g - b) / delta
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    ((hue * 60.0).rem_euclid(360.0), saturation, luminance)
}

/// Hue in degrees, saturation and luminance as red, green and blue from 0 to 1.
fn from_hsl((hue, saturation, luminance): (f64, f64, f64)) -> [f64; 3] {
    let chroma = (1.0 - (2.0 * luminance - 1.0).abs()) * saturation;
    let sector = hue / 60.0;
    let x = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = luminance - chroma / 2.0;
    [r + m, g + m, b + m]
}

/// A colour of VML as Office takes it (ST_ColorType, ECMA-376 Part 4, §19.1.2; MS-OI29500
/// 2.1.1893).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VmlColor {
    /// Red, green and blue.
    Rgb((u8, u8, u8)),
    /// No colour (`none`, (g)).
    None,
    /// A colour the document does not give: one of the system's (`Window`,
    /// `wincolor(5)`, (a), (f)), another colour of the shape (`lineOrFill`, (g)), or one
    /// of those changed (`fill darken(50)`, (h)).
    Elsewhere,
}

/// The system's colours Office takes by name (MS-OI29500 2.1.1893(a)), in lower case.
const SYSTEM_COLORS: [&str; 28] = [
    "buttonface",
    "windowtext",
    "menu",
    "highlight",
    "highlighttext",
    "captiontext",
    "activecaption",
    "buttonhighlight",
    "buttonshadow",
    "buttontext",
    "graytext",
    "inactivecaption",
    "inactivecaptiontext",
    "infobackground",
    "infotext",
    "menutext",
    "scrollbar",
    "window",
    "windowframe",
    "threedlightshadow",
    "threeddarkshadow",
    "activeborder",
    "inactiveborder",
    "background",
    "appworkspace",
    "threedface",
    "threedshadow",
    "threedhighlight",
];

/// The colours of a shape that a colour of VML may name (MS-OI29500 2.1.1893(g)), in
/// lower case; `none` is no colour.
const SHAPE_COLORS: [&str; 7] = [
    "fill",
    "line",
    "lineorfill",
    "fillthenline",
    "fillback",
    "lineback",
    "shadow",
];

/// The instructions that change a colour of VML (MS-OI29500 2.1.1893(h)), in lower case.
const RECOLORING: [&str; 9] = [
    "darken",
    "lighten",
    "add",
    "subtract",
    "reversesubtract",
    "blackwhite",
    "invert",
    "invert128",
    "grayscale",
];

/// A colour of VML: `#` and six or three hexadecimal digits (MS-OI29500 2.1.1893(d)),
/// `rgb(` and three numbers to 255 (b), a colour's name, a colour of the system (a, f) or
/// of the shape (g), or one of those changed by an instruction after it (h), applied to a
/// colour the document gives; each followed or not
/// by the index of a colour of the scheme in brackets, as Word writes it
/// (`#4f81bd [3204]`), which the value before it already resolves.
pub(crate) fn vml_color(value: &str) -> Option<VmlColor> {
    let value = value
        .split_once('[')
        .map_or(value, |(color, _)| color)
        .trim()
        .to_ascii_lowercase();
    // One instruction after the colour it changes (MS-OI29500 2.1.1893(l)), whose base is
    // read as a plain colour: a second instruction makes the value one Office does not take.
    if let Some((base, instruction)) = value.rsplit_once(' ') {
        let (name, amount) = match instruction.split_once('(') {
            Some((name, rest)) => (name.trim(), Some(rest.strip_suffix(')')?.trim())),
            None => (instruction.trim(), None),
        };
        if RECOLORING.contains(&name) {
            let amount = match amount {
                Some(amount) => Some(amount.parse::<u8>().ok()?),
                None => None,
            };
            return match plain_vml_color(base.trim())? {
                VmlColor::Rgb(rgb) => recolor(rgb, name, amount).map(VmlColor::Rgb),
                VmlColor::None => None,
                VmlColor::Elsewhere => Some(VmlColor::Elsewhere),
            };
        }
    }
    plain_vml_color(&value)
}

/// A colour of VML without an instruction, in lower case: hexadecimal, `rgb(`, `wincolor(`,
/// `none`, a colour of the system or of the shape, or a colour's name.
fn plain_vml_color(value: &str) -> Option<VmlColor> {
    if let Some(digits) = value.strip_prefix('#') {
        let rgb = match digits.len() {
            6 => hex(digits),
            3 => {
                let doubled: String = digits.chars().flat_map(|c| [c, c]).collect();
                hex(&doubled)
            }
            _ => None,
        };
        return rgb.map(VmlColor::Rgb);
    }
    if let Some(channels) = value
        .strip_prefix("rgb(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let mut channels = channels
            .split(',')
            .map(|channel| channel.trim().parse::<u8>().ok());
        let rgb = (channels.next()??, channels.next()??, channels.next()??);
        return channels.next().is_none().then_some(VmlColor::Rgb(rgb));
    }
    if let Some(index) = value
        .strip_prefix("wincolor(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return index.trim().parse::<u8>().ok().map(|_| VmlColor::Elsewhere);
    }
    if value == "none" {
        return Some(VmlColor::None);
    }
    if SYSTEM_COLORS.contains(&value) || SHAPE_COLORS.contains(&value) {
        return Some(VmlColor::Elsewhere);
    }
    PRESET_COLORS
        .iter()
        .find(|(preset, _)| preset.eq_ignore_ascii_case(value))
        .map(|&(_, rgb)| VmlColor::Rgb(rgb))
}

/// A colour changed by an instruction of VML, with its amount from 0 to 255 where it takes
/// one (MS-OI29500 2.1.1893(h)). Of the two readings of `invert128` the notes give, (h)'s,
/// which keeps each channel within 0 to 255.
fn recolor((r, g, b): (u8, u8, u8), instruction: &str, amount: Option<u8>) -> Option<(u8, u8, u8)> {
    let channels = [r, g, b].map(u32::from);
    let amount = amount.map(u32::from);
    let changed = match (instruction, amount) {
        ("darken", Some(a)) => channels.map(|c| c * a / 256),
        ("lighten", Some(a)) => channels.map(|c| 255 - (255 - c) * a / 256),
        ("add", Some(a)) => channels.map(|c| (c + a).min(255)),
        ("subtract", Some(a)) => channels.map(|c| c.saturating_sub(a)),
        ("reversesubtract", Some(a)) => channels.map(|c| a.saturating_sub(c)),
        ("blackwhite", Some(a)) => channels.map(|c| if c < a { 0 } else { 255 }),
        ("invert", None) => channels.map(|c| 255 - c),
        ("invert128", None) => channels.map(|c| if c < 128 { c + 128 } else { c - 128 }),
        ("grayscale", None) => {
            let [r, g, b] = channels;
            [(30 * r + 59 * g + 11 * b) / 100; 3]
        }
        _ => return None,
    };
    let [r, g, b] = changed.map(|c| u8::try_from(c).unwrap_or(u8::MAX));
    Some((r, g, b))
}

/// A preset colour of DrawingML (ST_PresetColorVal).
fn preset(value: &str) -> Option<(u8, u8, u8)> {
    PRESET_COLORS
        .iter()
        .find(|(name, _)| *name == value)
        .map(|&(_, rgb)| rgb)
}

/// The preset colours of DrawingML (ST_PresetColorVal, ECMA-376 Part 1, §20.1.10.48),
/// the colours of CSS by their names, with the shorter names `dk`, `lt` and `med` that
/// the standard also gives; VML takes the same names.
const PRESET_COLORS: [(&str, (u8, u8, u8)); 190] = [
    ("aliceBlue", (0xF0, 0xF8, 0xFF)),
    ("antiqueWhite", (0xFA, 0xEB, 0xD7)),
    ("aqua", (0x00, 0xFF, 0xFF)),
    ("aquamarine", (0x7F, 0xFF, 0xD4)),
    ("azure", (0xF0, 0xFF, 0xFF)),
    ("beige", (0xF5, 0xF5, 0xDC)),
    ("bisque", (0xFF, 0xE4, 0xC4)),
    ("black", (0x00, 0x00, 0x00)),
    ("blanchedAlmond", (0xFF, 0xEB, 0xCD)),
    ("blue", (0x00, 0x00, 0xFF)),
    ("blueViolet", (0x8A, 0x2B, 0xE2)),
    ("brown", (0xA5, 0x2A, 0x2A)),
    ("burlyWood", (0xDE, 0xB8, 0x87)),
    ("cadetBlue", (0x5F, 0x9E, 0xA0)),
    ("chartreuse", (0x7F, 0xFF, 0x00)),
    ("chocolate", (0xD2, 0x69, 0x1E)),
    ("coral", (0xFF, 0x7F, 0x50)),
    ("cornflowerBlue", (0x64, 0x95, 0xED)),
    ("cornsilk", (0xFF, 0xF8, 0xDC)),
    ("crimson", (0xDC, 0x14, 0x3C)),
    ("cyan", (0x00, 0xFF, 0xFF)),
    ("darkBlue", (0x00, 0x00, 0x8B)),
    ("darkCyan", (0x00, 0x8B, 0x8B)),
    ("darkGoldenrod", (0xB8, 0x86, 0x0B)),
    ("darkGray", (0xA9, 0xA9, 0xA9)),
    ("darkGrey", (0xA9, 0xA9, 0xA9)),
    ("darkGreen", (0x00, 0x64, 0x00)),
    ("darkKhaki", (0xBD, 0xB7, 0x6B)),
    ("darkMagenta", (0x8B, 0x00, 0x8B)),
    ("darkOliveGreen", (0x55, 0x6B, 0x2F)),
    ("darkOrange", (0xFF, 0x8C, 0x00)),
    ("darkOrchid", (0x99, 0x32, 0xCC)),
    ("darkRed", (0x8B, 0x00, 0x00)),
    ("darkSalmon", (0xE9, 0x96, 0x7A)),
    ("darkSeaGreen", (0x8F, 0xBC, 0x8F)),
    ("darkSlateBlue", (0x48, 0x3D, 0x8B)),
    ("darkSlateGray", (0x2F, 0x4F, 0x4F)),
    ("darkSlateGrey", (0x2F, 0x4F, 0x4F)),
    ("darkTurquoise", (0x00, 0xCE, 0xD1)),
    ("darkViolet", (0x94, 0x00, 0xD3)),
    ("deepPink", (0xFF, 0x14, 0x93)),
    ("deepSkyBlue", (0x00, 0xBF, 0xFF)),
    ("dimGray", (0x69, 0x69, 0x69)),
    ("dimGrey", (0x69, 0x69, 0x69)),
    ("dkBlue", (0x00, 0x00, 0x8B)),
    ("dkCyan", (0x00, 0x8B, 0x8B)),
    ("dkGoldenrod", (0xB8, 0x86, 0x0B)),
    ("dkGray", (0xA9, 0xA9, 0xA9)),
    ("dkGrey", (0xA9, 0xA9, 0xA9)),
    ("dkGreen", (0x00, 0x64, 0x00)),
    ("dkKhaki", (0xBD, 0xB7, 0x6B)),
    ("dkMagenta", (0x8B, 0x00, 0x8B)),
    ("dkOliveGreen", (0x55, 0x6B, 0x2F)),
    ("dkOrange", (0xFF, 0x8C, 0x00)),
    ("dkOrchid", (0x99, 0x32, 0xCC)),
    ("dkRed", (0x8B, 0x00, 0x00)),
    ("dkSalmon", (0xE9, 0x96, 0x7A)),
    ("dkSeaGreen", (0x8F, 0xBC, 0x8F)),
    ("dkSlateBlue", (0x48, 0x3D, 0x8B)),
    ("dkSlateGray", (0x2F, 0x4F, 0x4F)),
    ("dkSlateGrey", (0x2F, 0x4F, 0x4F)),
    ("dkTurquoise", (0x00, 0xCE, 0xD1)),
    ("dkViolet", (0x94, 0x00, 0xD3)),
    ("dodgerBlue", (0x1E, 0x90, 0xFF)),
    ("firebrick", (0xB2, 0x22, 0x22)),
    ("floralWhite", (0xFF, 0xFA, 0xF0)),
    ("forestGreen", (0x22, 0x8B, 0x22)),
    ("fuchsia", (0xFF, 0x00, 0xFF)),
    ("gainsboro", (0xDC, 0xDC, 0xDC)),
    ("ghostWhite", (0xF8, 0xF8, 0xFF)),
    ("gold", (0xFF, 0xD7, 0x00)),
    ("goldenrod", (0xDA, 0xA5, 0x20)),
    ("gray", (0x80, 0x80, 0x80)),
    ("grey", (0x80, 0x80, 0x80)),
    ("green", (0x00, 0x80, 0x00)),
    ("greenYellow", (0xAD, 0xFF, 0x2F)),
    ("honeydew", (0xF0, 0xFF, 0xF0)),
    ("hotPink", (0xFF, 0x69, 0xB4)),
    ("indianRed", (0xCD, 0x5C, 0x5C)),
    ("indigo", (0x4B, 0x00, 0x82)),
    ("ivory", (0xFF, 0xFF, 0xF0)),
    ("khaki", (0xF0, 0xE6, 0x8C)),
    ("lavender", (0xE6, 0xE6, 0xFA)),
    ("lavenderBlush", (0xFF, 0xF0, 0xF5)),
    ("lawnGreen", (0x7C, 0xFC, 0x00)),
    ("lemonChiffon", (0xFF, 0xFA, 0xCD)),
    ("lightBlue", (0xAD, 0xD8, 0xE6)),
    ("lightCoral", (0xF0, 0x80, 0x80)),
    ("lightCyan", (0xE0, 0xFF, 0xFF)),
    ("lightGoldenrodYellow", (0xFA, 0xFA, 0xD2)),
    ("lightGray", (0xD3, 0xD3, 0xD3)),
    ("lightGrey", (0xD3, 0xD3, 0xD3)),
    ("lightGreen", (0x90, 0xEE, 0x90)),
    ("lightPink", (0xFF, 0xB6, 0xC1)),
    ("lightSalmon", (0xFF, 0xA0, 0x7A)),
    ("lightSeaGreen", (0x20, 0xB2, 0xAA)),
    ("lightSkyBlue", (0x87, 0xCE, 0xFA)),
    ("lightSlateGray", (0x77, 0x88, 0x99)),
    ("lightSlateGrey", (0x77, 0x88, 0x99)),
    ("lightSteelBlue", (0xB0, 0xC4, 0xDE)),
    ("lightYellow", (0xFF, 0xFF, 0xE0)),
    ("lime", (0x00, 0xFF, 0x00)),
    ("limeGreen", (0x32, 0xCD, 0x32)),
    ("linen", (0xFA, 0xF0, 0xE6)),
    ("ltBlue", (0xAD, 0xD8, 0xE6)),
    ("ltCoral", (0xF0, 0x80, 0x80)),
    ("ltCyan", (0xE0, 0xFF, 0xFF)),
    ("ltGoldenrodYellow", (0xFA, 0xFA, 0xD2)),
    ("ltGray", (0xD3, 0xD3, 0xD3)),
    ("ltGrey", (0xD3, 0xD3, 0xD3)),
    ("ltGreen", (0x90, 0xEE, 0x90)),
    ("ltPink", (0xFF, 0xB6, 0xC1)),
    ("ltSalmon", (0xFF, 0xA0, 0x7A)),
    ("ltSeaGreen", (0x20, 0xB2, 0xAA)),
    ("ltSkyBlue", (0x87, 0xCE, 0xFA)),
    ("ltSlateGray", (0x77, 0x88, 0x99)),
    ("ltSlateGrey", (0x77, 0x88, 0x99)),
    ("ltSteelBlue", (0xB0, 0xC4, 0xDE)),
    ("ltYellow", (0xFF, 0xFF, 0xE0)),
    ("magenta", (0xFF, 0x00, 0xFF)),
    ("maroon", (0x80, 0x00, 0x00)),
    ("medAquamarine", (0x66, 0xCD, 0xAA)),
    ("medBlue", (0x00, 0x00, 0xCD)),
    ("medOrchid", (0xBA, 0x55, 0xD3)),
    ("medPurple", (0x93, 0x70, 0xDB)),
    ("medSeaGreen", (0x3C, 0xB3, 0x71)),
    ("medSlateBlue", (0x7B, 0x68, 0xEE)),
    ("medSpringGreen", (0x00, 0xFA, 0x9A)),
    ("medTurquoise", (0x48, 0xD1, 0xCC)),
    ("medVioletRed", (0xC7, 0x15, 0x85)),
    ("mediumAquamarine", (0x66, 0xCD, 0xAA)),
    ("mediumBlue", (0x00, 0x00, 0xCD)),
    ("mediumOrchid", (0xBA, 0x55, 0xD3)),
    ("mediumPurple", (0x93, 0x70, 0xDB)),
    ("mediumSeaGreen", (0x3C, 0xB3, 0x71)),
    ("mediumSlateBlue", (0x7B, 0x68, 0xEE)),
    ("mediumSpringGreen", (0x00, 0xFA, 0x9A)),
    ("mediumTurquoise", (0x48, 0xD1, 0xCC)),
    ("mediumVioletRed", (0xC7, 0x15, 0x85)),
    ("midnightBlue", (0x19, 0x19, 0x70)),
    ("mintCream", (0xF5, 0xFF, 0xFA)),
    ("mistyRose", (0xFF, 0xE4, 0xE1)),
    ("moccasin", (0xFF, 0xE4, 0xB5)),
    ("navajoWhite", (0xFF, 0xDE, 0xAD)),
    ("navy", (0x00, 0x00, 0x80)),
    ("oldLace", (0xFD, 0xF5, 0xE6)),
    ("olive", (0x80, 0x80, 0x00)),
    ("oliveDrab", (0x6B, 0x8E, 0x23)),
    ("orange", (0xFF, 0xA5, 0x00)),
    ("orangeRed", (0xFF, 0x45, 0x00)),
    ("orchid", (0xDA, 0x70, 0xD6)),
    ("paleGoldenrod", (0xEE, 0xE8, 0xAA)),
    ("paleGreen", (0x98, 0xFB, 0x98)),
    ("paleTurquoise", (0xAF, 0xEE, 0xEE)),
    ("paleVioletRed", (0xDB, 0x70, 0x93)),
    ("papayaWhip", (0xFF, 0xEF, 0xD5)),
    ("peachPuff", (0xFF, 0xDA, 0xB9)),
    ("peru", (0xCD, 0x85, 0x3F)),
    ("pink", (0xFF, 0xC0, 0xCB)),
    ("plum", (0xDD, 0xA0, 0xDD)),
    ("powderBlue", (0xB0, 0xE0, 0xE6)),
    ("purple", (0x80, 0x00, 0x80)),
    ("red", (0xFF, 0x00, 0x00)),
    ("rosyBrown", (0xBC, 0x8F, 0x8F)),
    ("royalBlue", (0x41, 0x69, 0xE1)),
    ("saddleBrown", (0x8B, 0x45, 0x13)),
    ("salmon", (0xFA, 0x80, 0x72)),
    ("sandyBrown", (0xF4, 0xA4, 0x60)),
    ("seaGreen", (0x2E, 0x8B, 0x57)),
    ("seaShell", (0xFF, 0xF5, 0xEE)),
    ("sienna", (0xA0, 0x52, 0x2D)),
    ("silver", (0xC0, 0xC0, 0xC0)),
    ("skyBlue", (0x87, 0xCE, 0xEB)),
    ("slateBlue", (0x6A, 0x5A, 0xCD)),
    ("slateGray", (0x70, 0x80, 0x90)),
    ("slateGrey", (0x70, 0x80, 0x90)),
    ("snow", (0xFF, 0xFA, 0xFA)),
    ("springGreen", (0x00, 0xFF, 0x7F)),
    ("steelBlue", (0x46, 0x82, 0xB4)),
    ("tan", (0xD2, 0xB4, 0x8C)),
    ("teal", (0x00, 0x80, 0x80)),
    ("thistle", (0xD8, 0xBF, 0xD8)),
    ("tomato", (0xFF, 0x63, 0x47)),
    ("turquoise", (0x40, 0xE0, 0xD0)),
    ("violet", (0xEE, 0x82, 0xEE)),
    ("wheat", (0xF5, 0xDE, 0xB3)),
    ("white", (0xFF, 0xFF, 0xFF)),
    ("whiteSmoke", (0xF5, 0xF5, 0xF5)),
    ("yellow", (0xFF, 0xFF, 0x00)),
    ("yellowGreen", (0x9A, 0xCD, 0x32)),
];
