//! The styles part (`w:styles`, ECMA-376 Part 1, §17.7): the document defaults, the
//! latent styles and the styles, each kept whole with the attributes the model types.

use officewright_opc::mce::{Disposition, Processor};
use officewright_opc::xml::{Event, Input, NsId};

use super::capture::{Budget, Capture};
use super::properties::{on_off_attribute, paragraph_properties, run_properties};
use super::table_properties::{cell_properties, row_properties, table_properties};
use super::{R, R_STRICT, W, W_STRICT, shorten};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::model::styles::{StyleData, StyleKind};
use crate::model::xml::{Element, Span};
use crate::model::{Document, Names};
use crate::{Error, Limits, Result};

pub(super) fn read(
    document: &mut Document,
    xml: &[u8],
    part: &str,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
    budget: &mut Budget,
) -> Result<()> {
    let opc = |err: officewright_opc::Error| Error::from(err.in_part(part));
    let input = Input::decode(xml).map_err(opc)?;
    let mut reader = input.reader(limits.package());
    let mut mce = Processor::new(&mut reader);
    let w = [reader.intern(W), reader.intern(W_STRICT)];
    for namespace in w.iter().chain(&[reader.intern(R), reader.intern(R_STRICT)]) {
        mce.understand(*namespace);
    }
    let names = document.names;
    let mut c = Capture::new(document, part, input.as_str(), diagnostics, budget);
    let mut in_root = false;
    while let Some(event) = reader.read_event().map_err(opc)? {
        match event {
            Event::Start(tag) => {
                c.at = tag.offset();
                let disposition = mce.start(&reader, &tag).map_err(opc)?;
                if !in_root {
                    if !is_w(&tag.name(), &w, "styles") {
                        let message = format!(
                            "the root element is <{}>, not w:styles",
                            shorten(tag.name().qname().as_str())
                        );
                        return Err(opc(reader.error(tag.offset(), message)));
                    }
                    in_root = true;
                    continue;
                }
                let offset = tag.offset();
                let understood = disposition == Disposition::Understood
                    && tag.name().namespace().is_some_and(|ns| w.contains(&ns));
                let local = tag.name().local();
                let span = c.capture(&mut reader, &mut mce, tag)?;
                match local {
                    "docDefaults" | "latentStyles" if understood => {
                        let defaults = local == "docDefaults";
                        let slot = if defaults {
                            &mut c.document.styles.defaults
                        } else {
                            &mut c.document.styles.latent
                        };
                        if slot.is_some() {
                            c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
                                format!("a second w:{local} is ignored")
                            });
                        } else {
                            *slot = Some(span);
                            if defaults {
                                let run = ["rPrDefault", "rPr"];
                                check_properties(
                                    &mut c,
                                    &names,
                                    span,
                                    &run,
                                    Properties::Run,
                                    offset,
                                );
                                let paragraph = ["pPrDefault", "pPr"];
                                check_properties(
                                    &mut c,
                                    &names,
                                    span,
                                    &paragraph,
                                    Properties::Paragraph,
                                    offset,
                                );
                            }
                        }
                    }
                    "style" if understood => style(&mut c, &names, span, offset),
                    // Kept with the document's tokens for writing; nothing refers to it. An
                    // mc:AlternateContent here is kept whole too, so a style in it is not
                    // read; no document of the corpus has one.
                    _ => {}
                }
            }
            Event::End(_) => mce.end(),
            _ => {}
        }
    }
    Ok(())
}

fn is_w(name: &officewright_opc::xml::Name<'_>, w: &[NsId; 2], local: &str) -> bool {
    w.iter().any(|&namespace| name.is(namespace, local))
}

/// Adds a `w:style` to the document's styles.
fn style(c: &mut Capture<'_, '_, '_>, names: &Names, span: Span, offset: usize) {
    let w = &names.w;
    let element = c.document.element(span);
    let kind = match element.attribute(w, "type") {
        // A style without w:type is a paragraph style, the attribute's default.
        None | Some("paragraph") => Ok(StyleKind::Paragraph),
        Some("character") => Ok(StyleKind::Character),
        Some("table") => Ok(StyleKind::Table),
        Some("numbering") => Ok(StyleKind::Numbering),
        Some(other) => Err(other.to_owned()),
    };
    let mut invalid: Vec<(String, String)> = Vec::new();
    let mut on = |attribute| {
        on_off_attribute(element, names, attribute, &mut |_, attribute, value| {
            invalid.push((attribute.into(), value.into()));
        })
        .unwrap_or(false)
    };
    let child = |local| {
        element
            .children()
            .find(|child| child.is(w, local))
            .and_then(|child| child.attribute(w, "val"))
            .map(Into::into)
    };
    let id = element
        .attribute(w, "styleId")
        .filter(|id| !id.is_empty())
        .map(Box::<str>::from);
    let data = id.as_ref().map(|id| StyleData {
        kind: *kind.as_ref().unwrap_or(&StyleKind::Paragraph),
        id: id.clone(),
        name: child("name"),
        based_on: child("basedOn"),
        next: child("next"),
        link: child("link"),
        is_default: on("default"),
        is_custom: on("customStyle"),
        element: span,
    });
    for (attribute, value) in invalid {
        c.diagnose(offset, DiagnosticKind::InvalidValue, || {
            format!(
                "w:style w:{attribute}=\"{}\" is not valid; it is read as off",
                shorten(&value)
            )
        });
    }
    if let Err(other) = kind {
        c.diagnose(offset, DiagnosticKind::InvalidValue, || {
            format!(
                "w:style w:type=\"{}\" is not valid; the style is read as a paragraph style",
                shorten(&other)
            )
        });
    }
    let Some(data) = data else {
        c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
            "a w:style without a w:styleId is ignored".into()
        });
        return;
    };
    let styles = &mut c.document.styles;
    let index = styles.styles.len();
    if styles.by_id.contains_key(&data.id) {
        let id = data.id.clone();
        c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
            format!(
                "a second style with id {} is ignored where styles are looked up by id",
                shorten(&id)
            )
        });
    } else {
        styles.by_id.insert(data.id.clone(), index);
    }
    c.document.styles.styles.push(data);
    for (local, which) in [
        ("pPr", Properties::Paragraph),
        ("rPr", Properties::Run),
        ("tblPr", Properties::Table),
        ("trPr", Properties::Row),
        ("tcPr", Properties::Cell),
    ] {
        check_properties(c, names, span, &[local], which, offset);
    }
}

/// Which properties an element holds.
#[derive(Clone, Copy, Debug)]
pub(super) enum Properties {
    Run,
    Paragraph,
    /// `w:tblPr` or `w:tblPrEx`.
    Table,
    Row,
    Cell,
}

/// Reports the values that are not valid in the property element found by following
/// `path` down from `span`, as diagnostics at `offset`.
pub(super) fn check_properties(
    c: &mut Capture<'_, '_, '_>,
    names: &Names,
    span: Span,
    path: &[&str],
    which: Properties,
    offset: usize,
) {
    // Values are collected only while diagnostics have room for them; the rest are
    // counted, so damage that is not reported costs no memory.
    let room = c.diagnostics.room();
    let mut omitted = 0u64;
    let mut invalid: Vec<(String, String, String)> = Vec::new();
    {
        let mut element: Option<Element<'_>> = Some(c.document.element(span));
        for local in path {
            element = element.and_then(|parent| {
                parent
                    .children_through_fallback(names.mc)
                    .into_iter()
                    .find(|child| child.is(&names.w, local))
            });
        }
        let mut report = |element: &str, attribute: &str, value: &str| {
            if invalid.len() < room {
                invalid.push((element.into(), attribute.into(), value.into()));
            } else {
                omitted += 1;
            }
        };
        if let Some(element) = element {
            match which {
                Properties::Run => drop(run_properties(element, names, &mut report)),
                Properties::Paragraph => drop(paragraph_properties(element, names, &mut report)),
                Properties::Table => drop(table_properties(element, names, &mut report)),
                Properties::Row => drop(row_properties(element, names, &mut report)),
                Properties::Cell => drop(cell_properties(element, names, &mut report)),
            }
        }
    }
    c.diagnostics.omit(omitted);
    for (element, attribute, value) in invalid {
        c.diagnose(offset, DiagnosticKind::InvalidValue, || {
            format!(
                "w:{element} w:{attribute}=\"{}\" is not valid; it is ignored",
                shorten(&value)
            )
        });
    }
}
