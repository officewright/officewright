//! Properties read from kept elements: what a `w:rPr` of a run, a style or the document
//! defaults sets (ECMA-376 Part 1, §17.3.2).

use crate::model::Names;
use crate::model::properties::{
    Color, ColorValue, FontHint, Highlight, Language, RunFonts, RunProperties, Shading,
    ShadingPattern, ThemeColor, ThemeFont, Toggle, Underline, UnderlineStyle, VerticalPosition,
};
use crate::model::xml::Element;
use crate::units::{half_points_measure, twips_measure};

/// Called for each attribute whose value is not valid: the element's local name, the
/// attribute's, and the value. The property is read as not set.
pub(crate) type Report<'r> = &'r mut dyn FnMut(&str, &str, &str);

/// The properties a `w:rPr` sets.
pub(crate) fn run_properties(
    r_pr: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> RunProperties {
    let mut properties = RunProperties::default();
    let w = &names.w;
    for child in r_pr.children_through_fallback(names.mc) {
        let Some(local) = child.local_in(w) else {
            continue;
        };
        if let Some(&(toggle, _)) = Toggle::ALL.iter().find(|(_, name)| *name == local) {
            if let Some(slot) = properties.toggles.get_mut(toggle as usize) {
                *slot = on_off(child, names, report);
            }
            continue;
        }
        let mut length = |half_points: bool, signed: bool| {
            let value = child.attribute(w, "val")?;
            let length = if half_points {
                half_points_measure(value, signed)
            } else {
                twips_measure(value, signed)
            };
            if length.is_none() {
                report(local, "val", value);
            }
            length
        };
        match local {
            "rStyle" => properties.style = child.attribute(w, "val").map(Into::into),
            "rFonts" => properties.fonts = fonts(child, names, report),
            "color" => {
                let color = color(
                    child,
                    names,
                    ["val", "themeColor", "themeTint", "themeShade"],
                    report,
                );
                properties.color = (!color.is_empty()).then_some(color);
            }
            "u" => {
                properties.underline = Some(Underline {
                    style: value(child, names, "val", UnderlineStyle::from_xml, report),
                    color: color(
                        child,
                        names,
                        ["color", "themeColor", "themeTint", "themeShade"],
                        report,
                    ),
                });
            }
            "sz" => properties.size = length(true, false),
            "szCs" => properties.size_complex_script = length(true, false),
            "position" => properties.position = length(true, true),
            "kern" => properties.kerning = length(true, false),
            "spacing" => properties.spacing = length(false, true),
            "vertAlign" => {
                properties.vertical_position =
                    value(child, names, "val", VerticalPosition::from_xml, report);
            }
            "w" => properties.scale = value(child, names, "val", scale, report),
            "highlight" => {
                properties.highlight = value(child, names, "val", Highlight::from_xml, report)
            }
            "shd" => properties.shading = Some(shading(child, names, report)),
            "lang" => {
                properties.language = Language {
                    latin: child.attribute(w, "val").map(Into::into),
                    east_asia: child.attribute(w, "eastAsia").map(Into::into),
                    bidi: child.attribute(w, "bidi").map(Into::into),
                };
            }
            _ => {}
        }
    }
    properties
}

/// An on/off property (ST_OnOff): `w:val` absent, `true`, `on` or `1` sets it, `false`,
/// `off` or `0` clears it.
fn on_off(element: Element<'_>, names: &Names, report: Report<'_>) -> Option<bool> {
    match element.attribute(&names.w, "val") {
        None | Some("true" | "on" | "1") => Some(true),
        Some("false" | "off" | "0") => Some(false),
        Some(other) => {
            report(element.local_name(), "val", other);
            None
        }
    }
}

/// An attribute read with `parse`, reported when it does not parse.
fn value<T>(
    element: Element<'_>,
    names: &Names,
    attribute: &str,
    parse: impl Fn(&str) -> Option<T>,
    report: Report<'_>,
) -> Option<T> {
    let written = element.attribute(&names.w, attribute)?;
    let value = parse(written);
    if value.is_none() {
        report(element.local_name(), attribute, written);
    }
    value
}

fn fonts(element: Element<'_>, names: &Names, report: Report<'_>) -> RunFonts {
    let name = |attribute| element.attribute(&names.w, attribute).map(Into::into);
    let theme = |attribute, report: Report<'_>| {
        value(element, names, attribute, ThemeFont::from_xml, report)
    };
    RunFonts {
        ascii: name("ascii"),
        high_ansi: name("hAnsi"),
        east_asia: name("eastAsia"),
        complex_script: name("cs"),
        ascii_theme: theme("asciiTheme", report),
        high_ansi_theme: theme("hAnsiTheme", report),
        east_asia_theme: theme("eastAsiaTheme", report),
        complex_script_theme: theme("cstheme", report),
        hint: value(element, names, "hint", FontHint::from_xml, report),
    }
}

/// A colour from four attributes: the value, the theme colour, its tint and its shade.
fn color(element: Element<'_>, names: &Names, attributes: [&str; 4], report: Report<'_>) -> Color {
    let [val, theme, tint, shade] = attributes;
    Color {
        value: value(element, names, val, hex_color, report),
        theme: value(element, names, theme, ThemeColor::from_xml, report),
        tint: value(element, names, tint, hex_byte, report),
        shade: value(element, names, shade, hex_byte, report),
    }
}

fn shading(element: Element<'_>, names: &Names, report: Report<'_>) -> Shading {
    Shading {
        pattern: value(element, names, "val", ShadingPattern::from_xml, report),
        color: color(
            element,
            names,
            ["color", "themeColor", "themeTint", "themeShade"],
            report,
        ),
        fill: color(
            element,
            names,
            ["fill", "themeFill", "themeFillTint", "themeFillShade"],
            report,
        ),
    }
}

/// ST_HexColor: `auto`, or red, green and blue in hexadecimal.
fn hex_color(value: &str) -> Option<ColorValue> {
    if value == "auto" {
        return Some(ColorValue::Auto);
    }
    if value.len() != 6 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(value.get(at..at + 2)?, 16).ok();
    Some(ColorValue::Rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// ST_UcharHexNumber: one byte in two hexadecimal digits.
fn hex_byte(value: &str) -> Option<u8> {
    let digits = value.len() == 2 && value.bytes().all(|b| b.is_ascii_hexdigit());
    digits.then(|| u8::from_str_radix(value, 16).ok())?
}

/// ST_TextScale: a percentage from 0 to 600, written with or without `%`.
fn scale(value: &str) -> Option<u16> {
    let number: u16 = value.strip_suffix('%').unwrap_or(value).parse().ok()?;
    (number <= 600).then_some(number)
}
