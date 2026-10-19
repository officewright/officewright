//! Properties read from kept elements: what a `w:rPr` or `w:pPr` of a run, a paragraph,
//! a style or the document defaults sets (ECMA-376 Part 1, §17.3.1 and §17.3.2).

use crate::model::Names;
use crate::model::properties::{
    ArtBorder, Border, BorderStyle, Color, ColorValue, FontHint, Frame, Highlight, Indentation,
    Justification, Language, Ligatures, LineSpacing, NumberingReference, ParagraphBorders,
    ParagraphProperties, RunFonts, RunProperties, Shading, ShadingPattern, Spacing, TabKind,
    TabLeader, TabStop, TextFill, ThemeColor, ThemeFont, Toggle, Underline, UnderlineStyle,
    VerticalPosition,
};
use crate::model::xml::Element;
use crate::units::{half_points_measure, twips_measure};

/// Called for each attribute whose value is not valid: the element's local name, the
/// attribute's, and the value. The property is read as not set.
pub(crate) type Report<'r> = &'r mut dyn FnMut(&str, &str, &str);

/// An integer attribute of a text effect, or `default` when it is not written or not an
/// integer (they are ST_PositiveCoordinate, ST_FixedPercentage and the like, [MS-DOCX]
/// 2.6.3).
fn effect_value(effect: Element<'_>, names: &Names, name: &str, default: i64) -> i64 {
    effect
        .attribute(&[names.w14], name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// The first child of a text effect of this name.
fn effect_child<'a>(effect: Element<'a>, names: &Names, name: &str) -> Option<Element<'a>> {
    effect
        .children_through_alternates(names.mc)
        .find(|child| child.local_in(&[names.w14]) == Some(name))
}

/// Whether a `w14:glow` shows: one of radius 0 does not.
fn glows(glow: Element<'_>, names: &Names) -> bool {
    effect_value(glow, names, "rad", 0) > 0
}

/// Whether a `w14:shadow` shows. One scaled to nothing does not, nor one right under the
/// text: no blur, no distance, its full size, no skew. The defaults are those of
/// [MS-DOCX] 2.6.3.30 (CT_Shadow), all 0: a shadow that writes no `w14:sx` and `w14:sy`
/// is scaled to nothing, as is the one Word writes into list levels with them 0.
fn casts_shadow(shadow: Element<'_>, names: &Names) -> bool {
    let value = |name| effect_value(shadow, names, name, 0);
    let (sx, sy) = (value("sx"), value("sy"));
    let under = value("blurRad") == 0
        && value("dist") == 0
        && (sx, sy) == (100_000, 100_000)
        && value("kx") == 0
        && value("ky") == 0;
    sx != 0 && sy != 0 && !under
}

/// Whether a `w14:reflection` shows: one whose start and end are both fully transparent
/// does not, nor one of size 0 or scaled to nothing. The defaults are those of [MS-DOCX] 2.6.3.24
/// (CT_Reflection), all 0, so the reflection Word writes into list levels, all zeros,
/// shows nothing either.
fn reflects(reflection: Element<'_>, names: &Names) -> bool {
    let value = |name| effect_value(reflection, names, name, 0);
    // The reflection fades along a ramp that ends at endPos, the reflection's size in
    // Word's dialog; at its default of 0 nothing of it is drawn ([MS-DOCX] 2.6.3.24).
    (value("stA") != 0 || value("endA") != 0)
        && value("endPos") != 0
        && value("sx") != 0
        && value("sy") != 0
}

/// Whether a `w14:textOutline` draws a line: one filled other than with `w14:noFill`.
fn outlines(outline: Element<'_>, names: &Names) -> bool {
    outline
        .children_through_alternates(names.mc)
        .any(|child| matches!(child.local_in(&[names.w14]), Some("solidFill" | "gradFill")))
}

/// What a `w14:textFill` fills text with. Without a fill it is solid black ([MS-DOCX]
/// 2.6.3.5, CT_FillTextEffect), and so is a `w14:solidFill` without a colour (2.6.3.31,
/// CT_SolidColorFillProperties).
fn text_fill(fill: Element<'_>, names: &Names) -> TextFill {
    const BLACK: TextFill = TextFill::Solid(0, 0, 0);
    let kind = fill.children_through_alternates(names.mc).find(|child| {
        matches!(
            child.local_in(&[names.w14]),
            Some("noFill" | "solidFill" | "gradFill")
        )
    });
    let Some(kind) = kind else {
        return BLACK;
    };
    if kind.local_in(&[names.w14]) != Some("solidFill") {
        return TextFill::Other;
    }
    let colour = kind
        .children_through_alternates(names.mc)
        .find(|child| matches!(child.local_in(&[names.w14]), Some("srgbClr" | "schemeClr")));
    let Some(colour) = colour else {
        return BLACK;
    };
    // One colour as written, with nothing that changes it (w14:alpha, w14:lumMod, …).
    if colour
        .children_through_alternates(names.mc)
        .next()
        .is_some()
    {
        return TextFill::Other;
    }
    let value = colour.attribute(&[names.w14], "val");
    let solid = if colour.local_in(&[names.w14]) == Some("srgbClr") {
        value.and_then(hex_color).and_then(|colour| match colour {
            ColorValue::Rgb(red, green, blue) => Some(TextFill::Solid(red, green, blue)),
            ColorValue::Auto => None,
        })
    } else {
        value.and_then(scheme_color).map(TextFill::Scheme)
    };
    solid.unwrap_or(TextFill::Other)
}

/// The theme colour a colour of the theme's scheme names (the `w14:val` of a
/// `w14:schemeClr`, [MS-DOCX] 2.6.3.27, CT_SchemeColor, of type ST_SchemeColorVal,
/// 2.6.4.16); none for the placeholder colour, which has none.
fn scheme_color(value: &str) -> Option<ThemeColor> {
    Some(match value {
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
    })
}

/// Whether a `w14:scene3d`'s camera turns the text: one in front of it shows it as it is.
/// The w14 camera has no rotation of its own ([MS-DOCX] 2.6.3.2, CT_Camera); the rotation
/// of zeros Word writes into list levels is the light rig's.
fn turns(scene: Element<'_>, names: &Names) -> bool {
    effect_child(scene, names, "camera")
        .is_some_and(|camera| camera.attribute(&[names.w14], "prst") != Some("orthographicFront"))
}

/// Whether a `w14:props3d` shows on text seen from the front: a bevel at the top, of a
/// width and a height ([MS-DOCX] 2.6.3.1, CT_Bevel, whose defaults are 0), or a contour.
/// Depth and a bevel at the bottom face away from a camera in front, which is our
/// reading, no render shows it yet; with a camera that turns the text, `w14:scene3d` is
/// listed anyway.
fn has_depth(props: Element<'_>, names: &Names) -> bool {
    effect_child(props, names, "bevelT").is_some_and(|bevel| {
        effect_value(bevel, names, "w", 0) > 0 && effect_value(bevel, names, "h", 0) > 0
    }) || effect_value(props, names, "contourW", 0) > 0
}

/// Whether a `w14:stylisticSets` turns a set on: a `w14:styleSet` whose `w14:val` is not
/// false ([MS-DOCX] CT_StyleSet: one that is false is ignored, and it is on when not
/// written).
fn stylistic_set_on(sets: Element<'_>, names: &Names) -> bool {
    sets.children_through_alternates(names.mc)
        .filter(|set| set.local_in(&[names.w14]) == Some("styleSet"))
        .any(|set| !matches!(set.attribute(&[names.w14], "val"), Some("false" | "0")))
}

/// The properties a `w:rPr` sets.
pub(crate) fn run_properties(
    r_pr: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> RunProperties {
    let mut properties = RunProperties::default();
    let w = &names.w;
    for child in r_pr.children_through_alternates(names.mc) {
        // Word 2010's extensions ([MS-DOCX] 2.6.1.17 ligatures, 2.6.1.2 cntxtAlts). A
        // value that is not valid is not read; not reporting it is our choice, since the
        // diagnostics name attributes of w:.
        if let Some(local) = child.local_in(&[names.w14]) {
            let value = child.attribute(&[names.w14], "val");
            match local {
                "ligatures" => properties.ligatures = value.and_then(Ligatures::from_xml),
                // Text effects ([MS-DOCX] 2.6.1): only whether each changes what is drawn,
                // so that the layout's report can say that it is not drawn (#339).
                "glow" => properties.text_effects.glow = Some(glows(child, names)),
                "shadow" => properties.text_effects.shadow = Some(casts_shadow(child, names)),
                "reflection" => {
                    properties.text_effects.reflection = Some(reflects(child, names));
                }
                "textOutline" => properties.text_effects.outline = Some(outlines(child, names)),
                "textFill" => properties.text_effects.fill = Some(text_fill(child, names)),
                "scene3d" => properties.text_effects.turned = Some(turns(child, names)),
                "props3d" => properties.text_effects.depth = Some(has_depth(child, names)),
                // Only whether each asks for other than the font's default, so that the
                // layout's report can say that it is not applied (#361).
                "numForm" => {
                    properties.font_choices.number_form = value.map(|value| value != "default");
                }
                "numSpacing" => {
                    properties.font_choices.number_spacing = value.map(|value| value != "default");
                }
                "stylisticSets" => {
                    properties.font_choices.stylistic_sets = Some(stylistic_set_on(child, names));
                }
                "cntxtAlts" => {
                    // w14's ST_OnOff has no on and off ([MS-DOCX] 2.6.4.9).
                    properties.contextual_alternates = match value {
                        None => Some(true),
                        Some("true" | "1") => Some(true),
                        Some("false" | "0") => Some(false),
                        Some(_) => None,
                    };
                }
                _ => {}
            }
            continue;
        }
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
            // Only whether there is one, so that the layout's report can say that it is
            // not drawn (#303).
            "em" => properties.emphasis_mark = child.attribute(w, "val").map(|v| v != "none"),
            // Only in a paragraph mark's properties.
            "del" | "moveFrom" => properties.deleted = Some(true),
            "bdr" => {
                properties.border = child
                    .attribute(w, "val")
                    .map(|v| !matches!(v, "none" | "nil"));
            }
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
    for child in p_pr.children_through_alternates(names.mc) {
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
            // Only what kind of frame, so that the layout's report can say that it is
            // not placed (#303).
            "framePr" => {
                properties.frame = Some(match child.attribute(w, "dropCap") {
                    Some("drop" | "margin") => Frame::DropCap,
                    _ => Frame::Frame,
                });
            }
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

/// The list reference of a `w:pPr` (its last `w:numPr`, as
/// [`paragraph_properties`] reads it), without reading its other properties.
pub(crate) fn numbering_of(p_pr: Element<'_>, names: &Names) -> Option<NumberingReference> {
    p_pr.children_through_alternates(names.mc)
        .filter(|child| child.is(&names.w, "numPr"))
        .last()
        .map(|num_pr| numbering(num_pr, names, &mut |_, _, _| {}))
}

fn numbering(element: Element<'_>, names: &Names, report: Report<'_>) -> NumberingReference {
    let mut reference = NumberingReference::default();
    for child in element.children_through_alternates(names.mc) {
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
    for child in element.children_through_alternates(names.mc) {
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
    for child in element.children_through_alternates(names.mc) {
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

/// An `xsd:integer` (ST_DecimalNumber and its kin) as Office accepts it: no whitespace
/// around it. The schema collapses whitespace, but Office accepts only values whose
/// whitespace is already normalized (MS-OI29500 2.1.1(a)), so Word does not read `" 2 "`
/// as 2, and neither does the reader: it reports the value as not valid.
pub(super) fn integer(value: &str) -> Option<i64> {
    value.parse().ok()
}
