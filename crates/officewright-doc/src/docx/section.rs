//! Section properties (`w:sectPr`, ECMA-376 Part 1, §17.6).

use super::{Names, shorten};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::model::xml::Element;
use crate::model::{Orientation, SectionProperties, SectionStart};
use crate::units::twips_measure;

/// Reads the properties of a `w:sectPr` element of `part`. A value that is not valid is
/// left out with a diagnostic, and the section gets the default for it.
pub(super) fn read(
    sect_pr: Element<'_>,
    names: &Names,
    part: &str,
    diagnostics: &mut Diagnostics,
) -> SectionProperties {
    let w = &names.w;
    let mut properties = SectionProperties::default();
    for child in sect_pr.children() {
        let mut length = |attribute: &str, signed: bool| {
            let value = child.attribute(w, attribute)?;
            let length = twips_measure(value, signed);
            if length.is_none() {
                invalid(diagnostics, part, child, attribute, value);
            }
            length
        };
        if child.is(w, "pgSz") {
            properties.page_width = length("w", false);
            properties.page_height = length("h", false);
            properties.orientation = match child.attribute(w, "orient") {
                None => None,
                Some("portrait") => Some(Orientation::Portrait),
                Some("landscape") => Some(Orientation::Landscape),
                Some(other) => {
                    invalid(diagnostics, part, child, "orient", other);
                    None
                }
            };
        } else if child.is(w, "pgMar") {
            let margins = &mut properties.margins;
            margins.top = length("top", true);
            margins.bottom = length("bottom", true);
            margins.left = length("left", false);
            margins.right = length("right", false);
            margins.header = length("header", false);
            margins.footer = length("footer", false);
            margins.gutter = length("gutter", false);
        } else if child.is(w, "type") {
            properties.start = match child.attribute(w, "val") {
                // Without the attribute the type is nextPage (§17.6.22).
                None | Some("nextPage") => Some(SectionStart::NextPage),
                Some("continuous") => Some(SectionStart::Continuous),
                Some("evenPage") => Some(SectionStart::EvenPage),
                Some("oddPage") => Some(SectionStart::OddPage),
                Some("nextColumn") => Some(SectionStart::NextColumn),
                Some(other) => {
                    invalid(diagnostics, part, child, "val", other);
                    None
                }
            };
        }
    }
    properties
}

/// Records that an attribute of a property element is not valid and is ignored.
pub(super) fn invalid(
    diagnostics: &mut Diagnostics,
    part: &str,
    element: Element<'_>,
    attribute: &str,
    value: &str,
) {
    let local = element.local_name();
    diagnostics.push(DiagnosticKind::InvalidValue, || {
        format!(
            "{part}: w:{local} w:{attribute}=\"{}\" is not valid; it is ignored",
            shorten(value)
        )
    });
}
