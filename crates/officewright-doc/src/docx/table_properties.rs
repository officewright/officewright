//! The properties of tables, rows and cells (`w:tblPr`, `w:tblPrEx`, `w:trPr`, `w:tcPr`;
//! ECMA-376 Part 1, §17.4).

use super::properties::{Report, border, integer, on_off, on_off_attribute, shading, value};
use crate::model::Names;
use crate::model::table_properties::{
    CellBorders, CellMargins, CellMerge, HeightRule, HorizontalAlignment, PositionAnchor,
    RowHeight, TableBorders, TableCellProperties, TableJustification, TableLayout, TableLook,
    TableOverlap, TablePosition, TableProperties, TableRowProperties, TableWidth, TextDirection,
    VerticalAlignment, VerticalJustification,
};
use crate::model::xml::Element;
use crate::units::twips_measure;

/// The properties a `w:tblPr` or a `w:tblPrEx` sets.
pub(crate) fn table_properties(
    tbl_pr: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> TableProperties {
    let mut properties = TableProperties::default();
    for child in tbl_pr.children_through_alternates(names.mc) {
        let Some(local) = child.local_in(&names.w) else {
            continue;
        };
        let val = |report: Report<'_>| value(child, names, "val", integer, report);
        let string = || child.attribute(&names.w, "val").map(Into::into);
        match local {
            "tblStyle" => properties.style = string(),
            "tblpPr" => properties.position = Some(position(child, names, report)),
            "tblOverlap" => {
                properties.overlap = value(child, names, "val", TableOverlap::from_xml, report);
            }
            "bidiVisual" => properties.right_to_left = on_off(child, names, report),
            "tblStyleRowBandSize" => properties.row_band_size = val(report),
            "tblStyleColBandSize" => properties.column_band_size = val(report),
            "tblW" => properties.width = width(child, names, report),
            "jc" => properties.justification = justification(child, names, report),
            "tblCellSpacing" => properties.cell_spacing = width(child, names, report),
            "tblInd" => properties.indentation = width(child, names, report),
            "tblBorders" => properties.borders = Some(borders(child, names, report)),
            "shd" => properties.shading = Some(shading(child, names, report)),
            "tblLayout" => {
                properties.layout = value(child, names, "type", TableLayout::from_xml, report);
            }
            "tblCellMar" => properties.cell_margins = Some(margins(child, names, report)),
            "tblLook" => properties.look = Some(look(child, names, report)),
            "tblCaption" => properties.caption = string(),
            "tblDescription" => properties.description = string(),
            _ => {}
        }
    }
    properties
}

/// The properties a `w:trPr` sets. Its children may repeat; the last of each wins.
pub(crate) fn row_properties(
    tr_pr: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> TableRowProperties {
    let mut properties = TableRowProperties::default();
    for child in tr_pr.children_through_alternates(names.mc) {
        let Some(local) = child.local_in(&names.w) else {
            continue;
        };
        match local {
            "gridBefore" => properties.grid_before = value(child, names, "val", integer, report),
            "gridAfter" => properties.grid_after = value(child, names, "val", integer, report),
            "wBefore" => properties.width_before = width(child, names, report),
            "wAfter" => properties.width_after = width(child, names, report),
            "cantSplit" => properties.cant_split = on_off(child, names, report),
            "trHeight" => {
                properties.height = Some(RowHeight {
                    height: value(child, names, "val", |v| twips_measure(v, false), report),
                    rule: value(child, names, "hRule", HeightRule::from_xml, report),
                });
            }
            "tblHeader" => properties.header = on_off(child, names, report),
            "tblCellSpacing" => properties.cell_spacing = width(child, names, report),
            "jc" => properties.justification = justification(child, names, report),
            "hidden" => properties.hidden = on_off(child, names, report),
            "del" => properties.deleted = Some(true),
            _ => {}
        }
    }
    properties
}

/// The properties a `w:tcPr` sets.
pub(crate) fn cell_properties(
    tc_pr: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> TableCellProperties {
    let mut properties = TableCellProperties::default();
    for child in tc_pr.children_through_alternates(names.mc) {
        let Some(local) = child.local_in(&names.w) else {
            continue;
        };
        match local {
            "tcW" => properties.width = width(child, names, report),
            "gridSpan" => properties.grid_span = value(child, names, "val", integer, report),
            "hMerge" => properties.horizontal_merge = merge(child, names, report),
            "vMerge" => properties.vertical_merge = merge(child, names, report),
            "tcBorders" => properties.borders = Some(cell_borders(child, names, report)),
            "shd" => properties.shading = Some(shading(child, names, report)),
            "noWrap" => properties.no_wrap = on_off(child, names, report),
            "tcMar" => properties.margins = Some(margins(child, names, report)),
            "textDirection" => {
                properties.text_direction =
                    value(child, names, "val", TextDirection::from_xml, report);
            }
            "tcFitText" => properties.fit_text = on_off(child, names, report),
            "vAlign" => {
                properties.vertical_justification =
                    value(child, names, "val", VerticalJustification::from_xml, report);
            }
            "hideMark" => properties.hide_mark = on_off(child, names, report),
            _ => {}
        }
    }
    properties
}

/// A table measurement (CT_TblWidth, §17.4.87). Both attributes are checked against
/// their schema types even where Word's precedence ignores one of them.
fn width(element: Element<'_>, names: &Names, report: Report<'_>) -> Option<TableWidth> {
    #[derive(Clone, Copy)]
    enum Kind {
        Auto,
        Nil,
        Pct,
        Dxa,
    }
    let local = element.local_name();
    let value = value(element, names, "w", width_value, report);
    // dxa is the type assumed when none is written (§17.4.87).
    let kind = match element.attribute(&names.w, "type") {
        None | Some("dxa") => Some(Kind::Dxa),
        Some("auto") => Some(Kind::Auto),
        Some("nil") => Some(Kind::Nil),
        Some("pct") => Some(Kind::Pct),
        Some(other) => {
            report(local, "type", other);
            None
        }
    };
    // Word reads a w with a percent sign as whole percent, whatever the type
    // (MS-OI29500 §17.4.87(b)).
    if let Some(WidthValue::Percent(percent)) = value {
        return Some(TableWidth::Percent(percent));
    }
    match (kind?, value) {
        (Kind::Auto, _) => Some(TableWidth::Auto),
        (Kind::Nil, _) => Some(TableWidth::Nil),
        // Without a percent sign a percentage is in fiftieths of a percent
        // (MS-OI29500 §17.4.87(a)).
        (Kind::Pct, Some(WidthValue::Number(fiftieths))) => {
            Some(TableWidth::Percent(fiftieths / 50.0))
        }
        // A length is no percentage.
        (Kind::Pct, Some(WidthValue::Length(_))) => {
            report(
                local,
                "w",
                element.attribute(&names.w, "w").unwrap_or_default(),
            );
            None
        }
        // The schema allows a negative width; Word expects one only for tblInd
        // (MS-OI29500 §17.4.87(d)), and what it does with others is left to the
        // resolution step.
        (Kind::Dxa, Some(WidthValue::Number(twips))) => {
            Some(TableWidth::Length(officewright_core::Pt(twips / 20.0)))
        }
        (Kind::Dxa, Some(WidthValue::Length(length))) => Some(TableWidth::Length(length)),
        (_, Some(WidthValue::Percent(_)) | None) => None,
    }
}

/// What the `w:w` of a table measurement holds (ST_MeasurementOrPercent).
#[derive(Clone, Copy)]
enum WidthValue {
    /// ST_UnqualifiedPercentage, an `xsd:decimal`: twips or fiftieths of a percent by
    /// the type.
    Number(f64),
    /// ST_Percentage: a number with a percent sign.
    Percent(f64),
    /// ST_UniversalMeasure: a number with a unit.
    Length(officewright_core::Pt),
}

/// A value of `w:w`, or none when it is not one or does not fit in 32 bits of its unit:
/// twips, or fiftieths of a percent. Lengths elsewhere in a document fit in 32 bits of
/// twips too (`units::measure`); a value beyond, such as a number of hundreds of digits,
/// which `f64` reads as infinity, is damage.
fn width_value(value: &str) -> Option<WidthValue> {
    // Infinity fails the comparison too.
    let fits = |number: f64, per_unit: f64| (number * per_unit).abs() <= f64::from(i32::MAX);
    if let Some(number) = value.strip_suffix('%') {
        return pattern_number(number)
            .filter(|percent| fits(*percent, 50.0))
            .map(WidthValue::Percent);
    }
    if let Some(number) = decimal(value) {
        return fits(number, 1.0).then_some(WidthValue::Number(number));
    }
    let unit_start = value.len().checked_sub(2)?;
    let (number, unit) = (value.get(..unit_start)?, value.get(unit_start..)?);
    let per_unit = match unit {
        "pt" => 1.0,
        "in" => 72.0,
        "mm" => 72.0 / 25.4,
        "cm" => 72.0 / 2.54,
        "pc" | "pi" => 12.0,
        _ => return None,
    };
    pattern_number(number)
        .map(|number| number * per_unit)
        .filter(|points| fits(*points, 20.0))
        .map(|points| WidthValue::Length(officewright_core::Pt(points)))
}

/// The number of ST_Percentage and ST_UniversalMeasure: `-?[0-9]+(\.[0-9]+)?`.
fn pattern_number(value: &str) -> Option<f64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let all_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let valid = match digits.split_once('.') {
        Some((whole, fraction)) => all_digits(whole) && all_digits(fraction),
        None => all_digits(digits),
    };
    valid.then(|| value.parse().ok()).flatten()
}

/// An `xsd:decimal`: an optional sign, then digits with an optional point, at least one
/// digit in all; spaces around it are collapsed away.
fn decimal(value: &str) -> Option<f64> {
    let value = value.trim_matches([' ', '\t', '\n', '\r']);
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let only_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    (only_digits(whole) && only_digits(fraction) && !(whole.is_empty() && fraction.is_empty()))
        .then(|| value.parse().ok())
        .flatten()
}

fn justification(
    element: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> Option<TableJustification> {
    value(element, names, "val", TableJustification::from_xml, report)
}

/// `w:vMerge` or `w:hMerge`: without `w:val` the cell continues the merge (§17.4.84).
fn merge(element: Element<'_>, names: &Names, report: Report<'_>) -> Option<CellMerge> {
    match element.attribute(&names.w, "val") {
        None => Some(CellMerge::Continue),
        Some(_) => value(element, names, "val", CellMerge::from_xml, report),
    }
}

/// The borders of a `w:tblBorders`, or the side borders of a `w:tcBorders`.
fn borders(element: Element<'_>, names: &Names, report: Report<'_>) -> TableBorders {
    let mut borders = TableBorders::default();
    for child in element.children_through_alternates(names.mc) {
        let slot = match child.local_in(&names.w) {
            Some("top") => &mut borders.top,
            Some("bottom") => &mut borders.bottom,
            Some("left" | "start") => &mut borders.start,
            Some("right" | "end") => &mut borders.end,
            Some("insideH") => &mut borders.inside_horizontal,
            Some("insideV") => &mut borders.inside_vertical,
            _ => continue,
        };
        *slot = Some(border(child, names, report));
    }
    borders
}

fn cell_borders(element: Element<'_>, names: &Names, report: Report<'_>) -> CellBorders {
    let mut borders = CellBorders {
        sides: self::borders(element, names, report),
        ..CellBorders::default()
    };
    for child in element.children_through_alternates(names.mc) {
        let slot = match child.local_in(&names.w) {
            Some("tl2br") => &mut borders.top_left_to_bottom_right,
            Some("tr2bl") => &mut borders.top_right_to_bottom_left,
            _ => continue,
        };
        *slot = Some(border(child, names, report));
    }
    borders
}

fn margins(element: Element<'_>, names: &Names, report: Report<'_>) -> CellMargins {
    let mut margins = CellMargins::default();
    for child in element.children_through_alternates(names.mc) {
        let slot = match child.local_in(&names.w) {
            Some("top") => &mut margins.top,
            Some("bottom") => &mut margins.bottom,
            Some("left" | "start") => &mut margins.start,
            Some("right" | "end") => &mut margins.end,
            _ => continue,
        };
        *slot = width(child, names, report);
    }
    margins
}

/// `w:tblLook`. Word reads the bit mask in `w:val` only when none of the six attributes
/// is written (MS-OI29500 §17.4.55(c)); the mask is checked either way. Its bits are
/// those of ECMA-376 1st edition, Part 4, §2.4.52, the clause ISO/IEC 29500-4 §14.4.12
/// keeps; where Word writes both, the 875 `w:tblLook` of corpus-ext agree.
fn look(element: Element<'_>, names: &Names, report: Report<'_>) -> TableLook {
    const ATTRIBUTES: [&str; 6] = [
        "firstRow",
        "lastRow",
        "firstColumn",
        "lastColumn",
        "noHBand",
        "noVBand",
    ];
    let w = &names.w;
    // ST_ShortHexNumber: two bytes, four hexadecimal digits.
    let mask = value(
        element,
        names,
        "val",
        |v| {
            (v.len() == 4 && v.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| u16::from_str_radix(v, 16).ok())
                .flatten()
        },
        report,
    );
    if ATTRIBUTES
        .iter()
        .any(|attribute| element.attribute(w, attribute).is_some())
    {
        let mut flag = |attribute| on_off_attribute(element, names, attribute, report);
        return TableLook {
            first_row: flag("firstRow"),
            last_row: flag("lastRow"),
            first_column: flag("firstColumn"),
            last_column: flag("lastColumn"),
            no_horizontal_bands: flag("noHBand"),
            no_vertical_bands: flag("noVBand"),
        };
    }
    let Some(mask) = mask else {
        return TableLook::default();
    };
    let bit = |bit: u16| Some(mask & bit != 0);
    TableLook {
        first_row: bit(0x0020),
        last_row: bit(0x0040),
        first_column: bit(0x0080),
        last_column: bit(0x0100),
        no_horizontal_bands: bit(0x0200),
        no_vertical_bands: bit(0x0400),
    }
}

fn position(element: Element<'_>, names: &Names, report: Report<'_>) -> TablePosition {
    let length = |attribute, signed: bool, report: Report<'_>| {
        value(
            element,
            names,
            attribute,
            |v| twips_measure(v, signed),
            report,
        )
    };
    TablePosition {
        left_from_text: length("leftFromText", false, report),
        right_from_text: length("rightFromText", false, report),
        top_from_text: length("topFromText", false, report),
        bottom_from_text: length("bottomFromText", false, report),
        horizontal_anchor: value(
            element,
            names,
            "horzAnchor",
            PositionAnchor::from_xml,
            report,
        ),
        vertical_anchor: value(
            element,
            names,
            "vertAnchor",
            PositionAnchor::from_xml,
            report,
        ),
        horizontal_alignment: value(
            element,
            names,
            "tblpXSpec",
            HorizontalAlignment::from_xml,
            report,
        ),
        x: length("tblpX", true, report),
        vertical_alignment: value(
            element,
            names,
            "tblpYSpec",
            VerticalAlignment::from_xml,
            report,
        ),
        y: length("tblpY", true, report),
    }
}
