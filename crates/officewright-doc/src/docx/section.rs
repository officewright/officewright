//! Section properties (`w:sectPr`, ECMA-376 Part 1, §17.6).

use super::properties::{Report, border, integer, on_off, value};
use super::shorten;
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::model::properties::NumberFormat;
use crate::model::section::{
    ChapterSeparator, Column, Columns, DocumentGrid, DocumentGridKind, HeaderFooterKind,
    HeaderFooterReference, LineNumberRestart, LineNumbering, NotePosition, NoteProperties,
    NoteRestart, PageBorderDisplay, PageBorderOffset, PageBorderZOrder, PageBorders, PageNumbering,
};
use crate::model::table_properties::{TextDirection, VerticalJustification};
use crate::model::xml::Element;
use crate::model::{Names, Orientation, SectionProperties, SectionStart};
use crate::units::twips_measure;

/// Reads the properties of a `w:sectPr` element at `place` (the part, line and column of
/// the element that holds it). A value that is not valid is left out with a diagnostic,
/// and the section gets the default for it.
pub(super) fn read(
    sect_pr: Element<'_>,
    names: &Names,
    place: &str,
    diagnostics: &mut Diagnostics,
) -> SectionProperties {
    let w = &names.w;
    let mut properties = SectionProperties::default();
    let mut unreferenced = 0u32;
    let mut seen: Vec<&str> = Vec::new();
    // Headers and footers, by kind: default, even, first.
    let mut seen_references = [[false; 3]; 2];
    for child in sect_pr.children_through_alternates(names.mc) {
        let Some(local) = child.local_in(w) else {
            continue;
        };
        // Each of these is written once at most (EG_SectPrContents); as for a second
        // w:docDefaults, the first one counts.
        if SECTION_CONTENTS.contains(&local) {
            if seen.contains(&local) {
                diagnostics.push(DiagnosticKind::UnexpectedContent, || {
                    format!("{place}: a second w:{local} is ignored")
                });
                continue;
            }
            seen.push(local);
        }
        if matches!(local, "headerReference" | "footerReference") {
            // Two of one kind are not conformant (§17.10.5); both are kept, and which
            // one Word uses is not measured.
            let kind = child
                .attribute(w, "type")
                .and_then(HeaderFooterKind::from_xml);
            let index = match kind {
                Some(HeaderFooterKind::Default) => Some(0),
                Some(HeaderFooterKind::Even) => Some(1),
                Some(HeaderFooterKind::First) => Some(2),
                None => None,
            };
            let row = seen_references.get_mut(usize::from(local == "footerReference"));
            if let Some(seen) = index.and_then(|index| row?.get_mut(index)) {
                if *seen {
                    diagnostics.push(DiagnosticKind::UnexpectedContent, || {
                        let kind = child.attribute(w, "type").unwrap_or_default();
                        format!("{place}: a second w:{local} of type {kind} is kept")
                    });
                }
                *seen = true;
            }
        }
        let mut report = |element: &str, attribute: &str, value: &str| {
            diagnostics.push(DiagnosticKind::InvalidValue, || {
                format!(
                    "{place}: w:{element} w:{attribute}=\"{}\" is not valid; it is ignored",
                    shorten(value)
                )
            });
        };
        let report: Report<'_> = &mut report;
        let length = |attribute, signed: bool, report: Report<'_>| {
            value(
                child,
                names,
                attribute,
                |v| twips_measure(v, signed),
                report,
            )
        };
        match local {
            "headerReference" | "footerReference" => {
                let Some(id) = child.attribute(&names.r, "id") else {
                    unreferenced += 1;
                    continue;
                };
                let reference = HeaderFooterReference {
                    kind: value(child, names, "type", HeaderFooterKind::from_xml, report),
                    relationship_id: id.into(),
                };
                if local == "headerReference" {
                    properties.headers.push(reference);
                } else {
                    properties.footers.push(reference);
                }
            }
            "footnotePr" => properties.footnotes = Some(notes(child, names, false, report)),
            "endnotePr" => properties.endnotes = Some(notes(child, names, true, report)),
            "type" => {
                properties.start = match child.attribute(w, "val") {
                    // Without the attribute the type is nextPage (§17.6.22); Word
                    // requires the attribute (MS-OI29500 §17.6.22(a)).
                    None => Some(SectionStart::NextPage),
                    Some(_) => value(child, names, "val", section_start, report),
                };
            }
            "pgSz" => {
                properties.page_width = length("w", false, report);
                properties.page_height = length("h", false, report);
                properties.orientation = value(child, names, "orient", orientation, report);
                properties.paper_code = value(child, names, "code", integer, report);
            }
            "pgMar" => {
                let margins = &mut properties.margins;
                margins.top = length("top", true, report);
                margins.bottom = length("bottom", true, report);
                margins.left = length("left", false, report);
                margins.right = length("right", false, report);
                margins.header = length("header", false, report);
                margins.footer = length("footer", false, report);
                margins.gutter = length("gutter", false, report);
            }
            "pgBorders" => properties.page_borders = Some(page_borders(child, names, report)),
            "lnNumType" => {
                properties.line_numbering = Some(LineNumbering {
                    count_by: value(child, names, "countBy", integer, report),
                    start: value(child, names, "start", integer, report),
                    distance: length("distance", false, report),
                    restart: value(child, names, "restart", LineNumberRestart::from_xml, report),
                });
            }
            "pgNumType" => {
                properties.page_numbering = Some(PageNumbering {
                    format: value(child, names, "fmt", NumberFormat::from_xml, report),
                    start: value(child, names, "start", integer, report),
                    chapter_style: value(child, names, "chapStyle", integer, report),
                    chapter_separator: value(
                        child,
                        names,
                        "chapSep",
                        ChapterSeparator::from_xml,
                        report,
                    ),
                });
            }
            "cols" => properties.columns = Some(columns(child, names, report)),
            "formProt" => properties.form_protection = on_off(child, names, report),
            "vAlign" => {
                properties.vertical_justification =
                    value(child, names, "val", VerticalJustification::from_xml, report);
            }
            "noEndnote" => properties.no_endnote = on_off(child, names, report),
            "titlePg" => properties.title_page = on_off(child, names, report),
            "textDirection" => {
                properties.text_direction =
                    value(child, names, "val", TextDirection::from_xml, report);
            }
            "bidi" => properties.right_to_left = on_off(child, names, report),
            "rtlGutter" => properties.right_to_left_gutter = on_off(child, names, report),
            "docGrid" => {
                properties.grid = Some(DocumentGrid {
                    kind: value(child, names, "type", DocumentGridKind::from_xml, report),
                    line_pitch: value(
                        child,
                        names,
                        "linePitch",
                        |v| integer(v).map(twentieths),
                        report,
                    ),
                    character_space: value(child, names, "charSpace", integer, report),
                });
            }
            _ => {}
        }
    }
    if unreferenced > 0 {
        diagnostics.push(DiagnosticKind::UnexpectedContent, || {
            format!("{place}: {unreferenced} header or footer references without r:id are ignored")
        });
    }
    properties
}

/// The children of `w:sectPr` that may be written once only.
const SECTION_CONTENTS: [&str; 19] = [
    "footnotePr",
    "endnotePr",
    "type",
    "pgSz",
    "pgMar",
    "paperSrc",
    "pgBorders",
    "lnNumType",
    "pgNumType",
    "cols",
    "formProt",
    "vAlign",
    "noEndnote",
    "titlePg",
    "textDirection",
    "bidi",
    "rtlGutter",
    "docGrid",
    "printerSettings",
];

fn section_start(value: &str) -> Option<SectionStart> {
    match value {
        "nextPage" => Some(SectionStart::NextPage),
        "continuous" => Some(SectionStart::Continuous),
        "evenPage" => Some(SectionStart::EvenPage),
        "oddPage" => Some(SectionStart::OddPage),
        "nextColumn" => Some(SectionStart::NextColumn),
        _ => None,
    }
}

fn orientation(value: &str) -> Option<Orientation> {
    match value {
        "portrait" => Some(Orientation::Portrait),
        "landscape" => Some(Orientation::Landscape),
        _ => None,
    }
}

/// A whole number of twentieths of a point. Lengths in documents are far below 2^53,
/// where an `f64` stops holding every integer.
fn twentieths(twips: i64) -> officewright_core::Pt {
    officewright_core::Pt(twips as f64 / 20.0)
}

/// `w:footnotePr` or, with `endnotes`, `w:endnotePr`, whose positions are only the
/// section's or the document's end.
pub(super) fn notes(
    element: Element<'_>,
    names: &Names,
    endnotes: bool,
    report: Report<'_>,
) -> NoteProperties {
    let mut properties = NoteProperties::default();
    for child in element.children_through_alternates(names.mc) {
        match child.local_in(&names.w) {
            Some("pos") => {
                properties.position = value(
                    child,
                    names,
                    "val",
                    |v| {
                        NotePosition::from_xml(v).filter(|position| {
                            !endnotes
                                || matches!(
                                    position,
                                    NotePosition::SectionEnd | NotePosition::DocumentEnd
                                )
                        })
                    },
                    report,
                );
            }
            Some("numFmt") => {
                properties.number_format =
                    value(child, names, "val", NumberFormat::from_xml, report);
            }
            Some("numStart") => properties.start = value(child, names, "val", integer, report),
            Some("numRestart") => {
                properties.restart = value(child, names, "val", NoteRestart::from_xml, report);
            }
            _ => {}
        }
    }
    properties
}

fn page_borders(element: Element<'_>, names: &Names, report: Report<'_>) -> PageBorders {
    let mut borders = PageBorders {
        z_order: value(element, names, "zOrder", PageBorderZOrder::from_xml, report),
        display: value(
            element,
            names,
            "display",
            PageBorderDisplay::from_xml,
            report,
        ),
        offset_from: value(
            element,
            names,
            "offsetFrom",
            PageBorderOffset::from_xml,
            report,
        ),
        ..PageBorders::default()
    };
    for child in element.children_through_alternates(names.mc) {
        let slot = match child.local_in(&names.w) {
            Some("top") => &mut borders.top,
            Some("left") => &mut borders.left,
            Some("bottom") => &mut borders.bottom,
            Some("right") => &mut borders.right,
            _ => continue,
        };
        *slot = Some(border(child, names, report));
    }
    borders
}

fn columns(element: Element<'_>, names: &Names, report: Report<'_>) -> Columns {
    let length = |element, attribute, report: Report<'_>| {
        value(
            element,
            names,
            attribute,
            |v| twips_measure(v, false),
            report,
        )
    };
    let mut columns = Columns {
        count: value(element, names, "num", integer, report),
        space: length(element, "space", report),
        equal_width: super::properties::on_off_attribute(element, names, "equalWidth", report),
        separator: super::properties::on_off_attribute(element, names, "sep", report),
        columns: Vec::new(),
    };
    for child in element.children_through_alternates(names.mc) {
        if child.local_in(&names.w) == Some("col") {
            columns.columns.push(Column {
                width: length(child, "w", report),
                space: length(child, "space", report),
            });
        }
    }
    columns
}

/// Records that an attribute of a property element is not valid and is ignored.
pub(super) fn invalid(
    diagnostics: &mut Diagnostics,
    place: &str,
    element: Element<'_>,
    attribute: &str,
    value: &str,
) {
    let local = element.local_name();
    diagnostics.push(DiagnosticKind::InvalidValue, || {
        format!(
            "{place}: w:{local} w:{attribute}=\"{}\" is not valid; it is ignored",
            shorten(value)
        )
    });
}
