//! Properties read from kept elements: what a `w:rPr` or `w:pPr` of a run, a paragraph,
//! a style or the document defaults sets (ECMA-376 Part 1, §17.3.1 and §17.3.2).

use crate::model::Names;
use crate::model::properties::{
    ArtBorder, Border, BorderStyle, Color, ColorValue, FontHint, Highlight, Indentation,
    Justification, Language, LineSpacing, NumberingReference, ParagraphBorders,
    ParagraphProperties, RunFonts, RunProperties, Shading, ShadingPattern, Spacing, TabKind,
    TabLeader, TabStop, ThemeColor, ThemeFont, Toggle, Underline, UnderlineStyle, VerticalPosition,
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
pub(super) fn on_off(element: Element<'_>, names: &Names, report: Report<'_>) -> Option<bool> {
    match element.attribute(&names.w, "val") {
        None => Some(true),
        Some(_) => on_off_attribute(element, names, "val", report),
    }
}

/// An on/off attribute (ST_OnOff), reported when its value is not one; `None` when the
/// attribute is absent or not valid.
pub(crate) fn on_off_attribute(
    element: Element<'_>,
    names: &Names,
    attribute: &str,
    report: Report<'_>,
) -> Option<bool> {
    value(
        element,
        names,
        attribute,
        |v| match v {
            "true" | "on" | "1" => Some(true),
            "false" | "off" | "0" => Some(false),
            _ => None,
        },
        report,
    )
}

/// An attribute read with `parse`, reported when it does not parse.
pub(super) fn value<T>(
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

pub(super) fn shading(element: Element<'_>, names: &Names, report: Report<'_>) -> Shading {
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

/// ST_TextScale: a percentage from 1 to 600, written with or without `%`. The schema
/// allows 0 as well, but Word shows such text at its normal width: LibreOffice's
/// `tdf106001.docx` has `w:w w:val="0"`, and its public bug report (tdf#106001) shows
/// it in Word 2010 at normal width. So 0 reads as not set, with a diagnostic.
fn scale(value: &str) -> Option<u16> {
    let number: u16 = value.strip_suffix('%').unwrap_or(value).parse().ok()?;
    (1..=600).contains(&number).then_some(number)
}

/// The properties a `w:pPr` sets, with those of its paragraph mark.
pub(crate) fn paragraph_properties(
    p_pr: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> ParagraphProperties {
    let mut properties = ParagraphProperties::default();
    let w = &names.w;
    for child in p_pr.children_through_fallback(names.mc) {
        let Some(local) = child.local_in(w) else {
            continue;
        };
        let flag = match local {
            "keepNext" => Some(&mut properties.keep_next),
            "keepLines" => Some(&mut properties.keep_lines),
            "pageBreakBefore" => Some(&mut properties.page_break_before),
            "widowControl" => Some(&mut properties.widow_control),
            "suppressLineNumbers" => Some(&mut properties.suppress_line_numbers),
            "suppressAutoHyphens" => Some(&mut properties.suppress_auto_hyphens),
            "contextualSpacing" => Some(&mut properties.contextual_spacing),
            "mirrorIndents" => Some(&mut properties.mirror_indents),
            "wordWrap" => Some(&mut properties.word_wrap),
            "bidi" => Some(&mut properties.right_to_left),
            "snapToGrid" => Some(&mut properties.snap_to_grid),
            _ => None,
        };
        if let Some(flag) = flag {
            *flag = on_off(child, names, report);
            continue;
        }
        match local {
            "pStyle" => properties.style = child.attribute(w, "val").map(Into::into),
            "numPr" => properties.numbering = Some(numbering(child, names, report)),
            "pBdr" => properties.borders = Some(borders(child, names, report)),
            "shd" => properties.shading = Some(shading(child, names, report)),
            "tabs" => properties.tabs = tabs(child, names, report),
            "spacing" => properties.spacing = Some(spacing(child, names, report)),
            "ind" => properties.indentation = Some(indentation(child, names, report)),
            "jc" => {
                properties.justification =
                    value(child, names, "val", Justification::from_xml, report);
            }
            "outlineLvl" => {
                properties.outline_level = value(child, names, "val", integer, report);
            }
            "rPr" => properties.mark = run_properties(child, names, report),
            _ => {}
        }
    }
    properties
}

fn numbering(element: Element<'_>, names: &Names, report: Report<'_>) -> NumberingReference {
    let mut reference = NumberingReference::default();
    for child in element.children_through_fallback(names.mc) {
        match child.local_in(&names.w) {
            Some("numId") => reference.instance = value(child, names, "val", integer, report),
            Some("ilvl") => reference.level = value(child, names, "val", integer, report),
            _ => {}
        }
    }
    reference
}

fn borders(element: Element<'_>, names: &Names, report: Report<'_>) -> ParagraphBorders {
    let mut borders = ParagraphBorders::default();
    for child in element.children_through_fallback(names.mc) {
        let slot = match child.local_in(&names.w) {
            Some("top") => &mut borders.top,
            Some("bottom") => &mut borders.bottom,
            Some("left" | "start") => &mut borders.start,
            Some("right" | "end") => &mut borders.end,
            Some("between") => &mut borders.between,
            Some("bar") => &mut borders.bar,
            _ => continue,
        };
        *slot = Some(border(child, names, report));
    }
    borders
}

/// One border (CT_Border): style, colour, width in eighths of a point, space in points.
pub(super) fn border(element: Element<'_>, names: &Names, report: Report<'_>) -> Border {
    // An art border is no line style, and its width is in points, not eighths
    // (§17.3.4).
    let art = element
        .attribute(&names.w, "val")
        .and_then(ArtBorder::from_xml);
    let points = |eighths: f64| {
        move |value: &str| -> Option<officewright_core::Pt> {
            let number: u32 = value.parse().ok()?;
            Some(officewright_core::Pt(f64::from(number) / eighths))
        }
    };
    let style = match art {
        Some(_) => None,
        None => value(element, names, "val", BorderStyle::from_xml, report),
    };
    let per_point = if art.is_some() { 1.0 } else { 8.0 };
    Border {
        style,
        art,
        color: color(
            element,
            names,
            ["color", "themeColor", "themeTint", "themeShade"],
            report,
        ),
        width: value(element, names, "sz", points(per_point), report),
        space: value(element, names, "space", points(1.0), report),
        shadow: on_off_attribute(element, names, "shadow", report),
        frame: on_off_attribute(element, names, "frame", report),
    }
}

fn tabs(element: Element<'_>, names: &Names, report: Report<'_>) -> Vec<TabStop> {
    let mut tabs = Vec::new();
    for child in element.children_through_fallback(names.mc) {
        if child.local_in(&names.w) != Some("tab") {
            continue;
        }
        // A tab stop without a valid position cannot be placed; it is left out.
        let Some(position) = value(child, names, "pos", |v| twips_measure(v, true), report) else {
            continue;
        };
        tabs.push(TabStop {
            kind: value(child, names, "val", TabKind::from_xml, report),
            leader: value(child, names, "leader", TabLeader::from_xml, report),
            position,
        });
    }
    tabs
}

fn spacing(element: Element<'_>, names: &Names, report: Report<'_>) -> Spacing {
    let twips = |attribute, report: Report<'_>| {
        value(
            element,
            names,
            attribute,
            |v| twips_measure(v, false),
            report,
        )
    };
    let line = value(element, names, "line", |v| twips_measure(v, true), report);
    #[derive(Clone, Copy)]
    enum Rule {
        Auto,
        Exact,
        AtLeast,
    }
    // Auto is the default rule. An invalid rule is reported with or without a w:line.
    let rule = match element.attribute(&names.w, "lineRule") {
        None => Some(Rule::Auto),
        Some(_) => value(
            element,
            names,
            "lineRule",
            |v| match v {
                "auto" => Some(Rule::Auto),
                "exact" => Some(Rule::Exact),
                "atLeast" => Some(Rule::AtLeast),
                _ => None,
            },
            report,
        ),
    };
    let line = match (line, rule) {
        // For auto, w:line counts 240 to a line.
        (Some(line), Some(Rule::Auto)) => Some(LineSpacing::Multiple(line.0 * 20.0 / 240.0)),
        (Some(line), Some(Rule::Exact)) => Some(LineSpacing::Exact(line)),
        (Some(line), Some(Rule::AtLeast)) => Some(LineSpacing::AtLeast(line)),
        _ => None,
    };
    Spacing {
        before: twips("before", report),
        after: twips("after", report),
        before_lines: value(element, names, "beforeLines", integer, report),
        after_lines: value(element, names, "afterLines", integer, report),
        before_auto: on_off_attribute(element, names, "beforeAutospacing", report),
        after_auto: on_off_attribute(element, names, "afterAutospacing", report),
        line,
    }
}

fn indentation(element: Element<'_>, names: &Names, report: Report<'_>) -> Indentation {
    let length = |attributes: &[&str], signed: bool, report: Report<'_>| {
        let attribute = attributes
            .iter()
            .copied()
            .find(|attribute| element.attribute(&names.w, attribute).is_some())?;
        value(
            element,
            names,
            attribute,
            |v| twips_measure(v, signed),
            report,
        )
    };
    let chars = |attributes: &[&str], report: Report<'_>| {
        let attribute = attributes
            .iter()
            .copied()
            .find(|attribute| element.attribute(&names.w, attribute).is_some())?;
        value(element, names, attribute, integer, report)
    };
    Indentation {
        start: length(&["start", "left"], true, report),
        end: length(&["end", "right"], true, report),
        first_line: length(&["firstLine"], false, report),
        hanging: length(&["hanging"], false, report),
        start_chars: chars(&["startChars", "leftChars"], report),
        end_chars: chars(&["endChars", "rightChars"], report),
        first_line_chars: chars(&["firstLineChars"], report),
        hanging_chars: chars(&["hangingChars"], report),
    }
}

pub(super) fn integer(value: &str) -> Option<i64> {
    value.parse().ok()
}
