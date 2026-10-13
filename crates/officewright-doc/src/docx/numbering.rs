//! The numbering part (`w:numbering`, ECMA-376 Part 1, §17.9): list definitions and
//! list instances, each kept whole with the ids they are looked up by.

use std::collections::hash_map::Entry;

use officewright_opc::mce::{Disposition, Processor};
use officewright_opc::xml::{Event, Input};

use super::capture::{Budget, Capture};
use super::properties::{
    Report, integer, on_off, on_off_attribute, paragraph_properties, run_properties, value,
};
use super::{R, R_STRICT, W, W_STRICT, shorten};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::model::numbering::{
    DefinitionData, InstanceData, LegacyNumbering, LevelOverride, LevelSuffix, ListLevel,
    MultiLevelType, long_hex,
};
use crate::model::properties::{Justification, NumberFormat};
use crate::model::xml::{Element, Span};
use crate::model::{Document, Names};
use crate::units::twips_measure;
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
                    if !w
                        .iter()
                        .any(|&namespace| tag.name().is(namespace, "numbering"))
                    {
                        let message = format!(
                            "the root element is <{}>, not w:numbering",
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
                // Every element is kept with the document's tokens for writing; picture
                // bullets are not read yet. An mc:AlternateContent here is kept whole
                // too, so a definition in it is not read.
                let span = c.capture(&mut reader, &mut mce, tag)?;
                match local {
                    "abstractNum" if understood => definition(&mut c, &names, span, offset),
                    "num" if understood => instance(&mut c, &names, span, offset),
                    _ => {}
                }
            }
            Event::End(_) => mce.end(),
            _ => {}
        }
    }
    Ok(())
}

/// Adds a `w:abstractNum` to the document's list definitions.
fn definition(c: &mut Capture<'_, '_, '_>, names: &Names, span: Span, offset: usize) {
    let element = c.document.element(span);
    let id = element
        .attribute(&names.w, "abstractNumId")
        .and_then(integer);
    // Levels written twice: the first counts, as ListDefinition::levels reads them.
    let mut seen = [false; 9];
    let mut repeated: Vec<usize> = Vec::new();
    for child in element.children_through_fallback(names.mc) {
        if !child.is(&names.w, "lvl") {
            continue;
        }
        let index = child
            .attribute(&names.w, "ilvl")
            .and_then(|ilvl| ilvl.parse::<usize>().ok());
        if let Some(seen) = index.and_then(|index| seen.get_mut(index)) {
            if *seen {
                repeated.extend(index);
            }
            *seen = true;
        }
    }
    for level in repeated {
        c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
            format!("a second w:lvl with w:ilvl {level} is ignored")
        });
    }
    c.check(span, offset, |element, report| {
        // Read only for their diagnostics.
        let _ = value(element, names, "abstractNumId", integer, report);
        for child in element.children_through_fallback(names.mc) {
            match child.local_in(&names.w) {
                Some("nsid" | "tmpl") => {
                    let _ = value(child, names, "val", long_hex, report);
                }
                Some("multiLevelType") => {
                    let _ = value(child, names, "val", MultiLevelType::from_xml, report);
                }
                Some("lvl") => {
                    let _ = level(child, names, report);
                }
                _ => {}
            }
        }
    });
    let Some(id) = id else {
        c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
            "a w:abstractNum without a valid w:abstractNumId is ignored".into()
        });
        return;
    };
    let table = &mut c.document.numbering;
    let index = table.definitions.len();
    table.definitions.push(DefinitionData { id, element: span });
    if let Entry::Vacant(slot) = table.by_definition.entry(id) {
        slot.insert(index);
    } else {
        c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
            format!(
                "a second w:abstractNum with id {id} is ignored where definitions are looked up by id"
            )
        });
    }
}

/// Adds a `w:num` to the document's list instances.
fn instance(c: &mut Capture<'_, '_, '_>, names: &Names, span: Span, offset: usize) {
    let element = c.document.element(span);
    let w = &names.w;
    let id = element.attribute(w, "numId").and_then(integer);
    let definition = element
        .children_through_fallback(names.mc)
        .into_iter()
        .find(|child| child.is(w, "abstractNumId"))
        .and_then(|reference| reference.attribute(w, "val"))
        .and_then(integer);
    c.check(span, offset, |element, report| {
        // Read only for their diagnostics.
        let _ = value(element, names, "numId", integer, report);
        for child in element.children_through_fallback(names.mc) {
            match child.local_in(&names.w) {
                Some("abstractNumId") => {
                    let _ = value(child, names, "val", integer, report);
                }
                Some("lvlOverride") => {
                    let _ = level_override(child, names, report);
                }
                _ => {}
            }
        }
    });
    let Some(id) = id else {
        c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
            "a w:num without a valid w:numId is ignored".into()
        });
        return;
    };
    let table = &mut c.document.numbering;
    let index = table.instances.len();
    table.instances.push(InstanceData {
        id,
        definition,
        element: span,
    });
    if let Entry::Vacant(slot) = table.by_instance.entry(id) {
        slot.insert(index);
    } else {
        c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
            format!("a second w:num with id {id} is ignored where instances are looked up by id")
        });
    }
}

/// A `w:lvl`; `None` when its `w:ilvl` is not 0 to 8, which Word does not open
/// (MS-OI29500 §17.9.6(a)).
pub(crate) fn level(lvl: Element<'_>, names: &Names, report: Report<'_>) -> Option<ListLevel> {
    let w = &names.w;
    let index = value(
        lvl,
        names,
        "ilvl",
        |v| v.parse::<usize>().ok().filter(|level| *level <= 8),
        report,
    )?;
    let mut level = ListLevel {
        level: index,
        tentative: on_off_attribute(lvl, names, "tentative", report),
        template_code: value(lvl, names, "tplc", long_hex, report),
        ..ListLevel::default()
    };
    for child in lvl.children_through_fallback(names.mc) {
        let Some(local) = child.local_in(w) else {
            continue;
        };
        match local {
            "start" => level.start = value(child, names, "val", integer, report),
            "numFmt" => {
                level.number_format = value(child, names, "val", NumberFormat::from_xml, report);
            }
            "lvlRestart" => level.restart = value(child, names, "val", integer, report),
            "pStyle" => level.paragraph_style = child.attribute(w, "val").map(Into::into),
            "isLgl" => level.legal = on_off(child, names, report),
            "suff" => level.suffix = value(child, names, "val", LevelSuffix::from_xml, report),
            "lvlText" => {
                level.text = match child.attribute(w, "val") {
                    Some(text) => Some(text.into()),
                    None => on_off_attribute(child, names, "null", report)
                        .filter(|null| *null)
                        .map(|_| "".into()),
                };
            }
            "lvlPicBulletId" => {
                level.picture_bullet = value(child, names, "val", integer, report);
            }
            "legacy" => {
                level.legacy = Some(LegacyNumbering {
                    legacy: on_off_attribute(child, names, "legacy", report),
                    space: value(
                        child,
                        names,
                        "legacySpace",
                        |v| twips_measure(v, false),
                        report,
                    ),
                    indent: value(
                        child,
                        names,
                        "legacyIndent",
                        |v| twips_measure(v, true),
                        report,
                    ),
                });
            }
            "lvlJc" => {
                level.justification = value(child, names, "val", Justification::from_xml, report);
            }
            "pPr" => level.paragraph_properties = paragraph_properties(child, names, report),
            "rPr" => level.run_properties = run_properties(child, names, report),
            _ => {}
        }
    }
    Some(level)
}

/// A `w:lvlOverride`; `None` when its `w:ilvl` is not a number.
pub(crate) fn level_override(
    element: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> Option<LevelOverride> {
    let overridden = value(element, names, "ilvl", integer, report)?;
    let mut level_override = LevelOverride {
        level: overridden,
        start: None,
        definition: None,
    };
    for child in element.children_through_fallback(names.mc) {
        match child.local_in(&names.w) {
            Some("startOverride") => {
                level_override.start = value(child, names, "val", integer, report);
            }
            Some("lvl") => level_override.definition = level(child, names, report),
            _ => {}
        }
    }
    Some(level_override)
}
