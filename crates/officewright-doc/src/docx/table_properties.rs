//! The properties of tables, rows and cells (`w:tblPr`, `w:tblPrEx`, `w:trPr`, `w:tcPr`;
//! ECMA-376 Part 1, §17.4).

use super::properties::{Report, border, integer, on_off, on_off_attribute, shading, value};
use crate::model::Names;
use crate::model::table_properties::{
    CellBorders, CellMargins, HeightRule, HorizontalAlignment, Merge, RowHeight, TableAnchor,
    TableBorders, TableCellProperties, TableJustification, TableLayout, TableLook, TableOverlap,
    TablePosition, TableProperties, TableRowProperties, TableWidth, TextDirection,
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
    for child in tbl_pr.children_through_fallback(names.mc) {
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
    for child in tr_pr.children_through_fallback(names.mc) {
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
    for child in tc_pr.children_through_fallback(names.mc) {
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

/// A table measurement (CT_TblWidth, §17.4.87).
fn width(element: Element<'_>, names: &Names, report: Report<'_>) -> Option<TableWidth> {
    let w = &names.w;
    // Word reads a w with a percent sign as whole percent, whatever the type
    // (MS-OI29500 §17.4.87(b)).
    if let Some(percent) = element
        .attribute(w, "w")
        .and_then(|written| written.trim().strip_suffix('%'))
    {
        return value(
            element,
            names,
            "w",
            |_| {
                percent
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite())
                    .map(TableWidth::Percent)
            },
            report,
        );
    }
    match element.attribute(w, "type") {
        Some("auto") => Some(TableWidth::Auto),
        Some("nil") => Some(TableWidth::Nil),
        // Without a percent sign a percentage is in fiftieths of a percent
        // (MS-OI29500 §17.4.87(a)); Word reads it as a short (§17.4.87(c)).
        Some("pct") => value(
            element,
            names,
            "w",
            |v| {
                let fiftieths: i32 = v.trim().parse().ok()?;
                Some(TableWidth::Percent(f64::from(fiftieths) / 50.0))
            },
            report,
        ),
        // dxa is the type assumed when none is written (§17.4.87). The schema allows a
        // negative width; Word expects one only for tblInd (MS-OI29500 §17.4.87(d)), and
        // what it does with others is left to the resolution step.
        None | Some("dxa") => value(
            element,
            names,
            "w",
            |v| twips_measure(v, true).map(TableWidth::Length),
            report,
        ),
        Some(other) => {
            report(element.local_name(), "type", other);
            None
        }
    }
}

fn justification(
    element: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> Option<TableJustification> {
    value(element, names, "val", TableJustification::from_xml, report)
}

/// `w:vMerge` or `w:hMerge`: without `w:val` the cell continues the merge (§17.4.85).
fn merge(element: Element<'_>, names: &Names, report: Report<'_>) -> Option<Merge> {
    match element.attribute(&names.w, "val") {
        None => Some(Merge::Continue),
        Some(_) => value(element, names, "val", Merge::from_xml, report),
    }
}

/// The borders of a `w:tblBorders`, or the side borders of a `w:tcBorders`.
fn borders(element: Element<'_>, names: &Names, report: Report<'_>) -> TableBorders {
    let mut borders = TableBorders::default();
    for child in element.children_through_fallback(names.mc) {
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
    for child in element.children_through_fallback(names.mc) {
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
    for child in element.children_through_fallback(names.mc) {
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
/// is written (MS-OI29500 §17.4.55(c)). The bits are those of Word's 2003 schema for the
/// first four; the two banding bits are those Word writes next to `w:noHBand` and
/// `w:noVBand` (the 875 `w:tblLook` of corpus-ext that have both agree).
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
        horizontal_anchor: value(element, names, "horzAnchor", TableAnchor::from_xml, report),
        vertical_anchor: value(element, names, "vertAnchor", TableAnchor::from_xml, report),
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
