//! The theme part (`a:theme`, ECMA-376 Part 1, §20.1.6): the colour and font schemes
//! are read, and every element is kept for writing.
//!
//! Attributes are kept as written and typed values are checked; a colour, a font or a
//! script font without its required attribute reads as not set. An element the schema
//! allows once is ignored after the first, with a diagnostic, as in `w:sectPr`.
//! Diagnostics name the position of `a:themeElements`. An `mc:AlternateContent`
//! directly under `a:theme` is kept whole, and the `a:themeElements` in the branch it
//! takes is read.

use std::collections::HashSet;

use officewright_opc::mce::Processor;
use officewright_opc::xml::{Event, Input};

use super::capture::{Budget, Capture, Notes};
use super::properties::Report;
use super::{A, A_STRICT, R, R_STRICT, shorten};
use crate::diagnostic::Diagnostics;
use crate::model::settings::ColorSchemeIndex;
use crate::model::theme::{
    ColorScheme, FontCollection, FontScheme, SchemeColor, SystemColor, Theme, position,
};
use crate::model::xml::Element;
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
    let a = [reader.intern(A), reader.intern(A_STRICT)];
    for namespace in a.iter().chain(&[reader.intern(R), reader.intern(R_STRICT)]) {
        mce.understand(*namespace);
    }
    let names = document.names;
    let mut theme = Theme::default();
    let mut c = Capture::new(document, part, input.as_str(), diagnostics, budget);
    let mut in_root = false;
    let mut elements_read = false;
    while let Some(event) = reader.read_event().map_err(opc)? {
        match event {
            Event::Start(tag) => {
                c.at = tag.offset();
                let disposition = mce.start(&reader, &tag).map_err(opc)?;
                if !in_root {
                    if !a.iter().any(|&namespace| tag.name().is(namespace, "theme")) {
                        let message = format!(
                            "the root element is <{}>, not a:theme",
                            shorten(tag.name().qname().as_str())
                        );
                        return Err(opc(reader.error(tag.offset(), message)));
                    }
                    theme.name = tag.attribute(None, "name").map(Into::into);
                    in_root = true;
                    continue;
                }
                let offset = tag.offset();
                // Every element is kept with the document's tokens for writing. An
                // mc:AlternateContent is kept whole, and the children of the branch it
                // takes are read in its place; so are the children of an ignorable element
                // whose content mc:ProcessContent names.
                let span = c.capture(&mut reader, &mut mce, tag, Some(disposition))?;
                for span in c.through_alternates(span) {
                    if c.document.element(span).local_in(&names.a) != Some("themeElements") {
                        continue;
                    }
                    let mut notes = c.notes();
                    if elements_read {
                        notes.add(|| "a second a:themeElements is ignored".into());
                    } else {
                        elements_read = true;
                        c.check_in(["a:", ""], span, offset, |element, report| {
                            theme_elements(&mut theme, element, &names, report, &mut notes);
                        });
                    }
                    c.diagnose_notes(offset, notes);
                }
            }
            Event::End(_) => mce.end(),
            _ => {}
        }
    }
    c.document.theme = Some(theme);
    Ok(())
}

/// The children of `element` in DrawingML, with a note for each that repeats one of
/// `once` already seen, which is left out.
fn once<'d>(
    element: Element<'d>,
    names: &Names,
    once: &[&str],
    notes: &mut Notes,
) -> Vec<(&'d str, Element<'d>)> {
    let mut seen: Vec<&str> = Vec::new();
    let mut children = Vec::new();
    for child in element.children_through_alternates(names.mc) {
        let Some(local) = child.local_in(&names.a) else {
            continue;
        };
        if let Some(&name) = once.iter().find(|name| **name == local) {
            if seen.contains(&name) {
                notes.add(|| format!("a second a:{local} is ignored"));
                continue;
            }
            seen.push(name);
        }
        children.push((local, child));
    }
    children
}

fn theme_elements(
    theme: &mut Theme,
    element: Element<'_>,
    names: &Names,
    report: Report<'_>,
    notes: &mut Notes,
) {
    for (local, child) in once(element, names, &["clrScheme", "fontScheme"], notes) {
        match local {
            "clrScheme" => theme.colors = Some(color_scheme(child, names, report, notes)),
            "fontScheme" => {
                let mut scheme = FontScheme {
                    name: child.attribute(&[None], "name").map(Into::into),
                    ..FontScheme::default()
                };
                for (local, collection) in once(child, names, &["majorFont", "minorFont"], notes) {
                    match local {
                        "majorFont" => scheme.major = fonts(collection, names, notes),
                        "minorFont" => scheme.minor = fonts(collection, names, notes),
                        _ => {}
                    }
                }
                theme.fonts = Some(scheme);
            }
            _ => {}
        }
    }
}

/// The twelve colours of a scheme, each allowed once.
const COLORS: [&str; 12] = [
    "dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6",
    "hlink", "folHlink",
];

fn color_scheme(
    element: Element<'_>,
    names: &Names,
    report: Report<'_>,
    notes: &mut Notes,
) -> ColorScheme {
    let mut scheme = ColorScheme {
        name: element.attribute(&[None], "name").map(Into::into),
        ..ColorScheme::default()
    };
    for (local, child) in once(element, names, &COLORS, notes) {
        let index = match local {
            "dk1" => ColorSchemeIndex::Dark1,
            "lt1" => ColorSchemeIndex::Light1,
            "dk2" => ColorSchemeIndex::Dark2,
            "lt2" => ColorSchemeIndex::Light2,
            "accent1" => ColorSchemeIndex::Accent1,
            "accent2" => ColorSchemeIndex::Accent2,
            "accent3" => ColorSchemeIndex::Accent3,
            "accent4" => ColorSchemeIndex::Accent4,
            "accent5" => ColorSchemeIndex::Accent5,
            "accent6" => ColorSchemeIndex::Accent6,
            "hlink" => ColorSchemeIndex::Hyperlink,
            "folHlink" => ColorSchemeIndex::FollowedHyperlink,
            _ => continue,
        };
        let position = position(index);
        let color = child
            .children_through_alternates(names.mc)
            .find_map(|model| scheme_color(model, names, report));
        if let Some(slot) = scheme.colors.get_mut(position) {
            *slot = color;
        }
    }
    scheme
}

/// A colour of the models Word writes in schemes, RGB and system colours; `None` for
/// the others, which are not read yet, and for values that are not valid.
fn scheme_color(model: Element<'_>, names: &Names, report: Report<'_>) -> Option<SchemeColor> {
    let local = model.local_in(&names.a)?;
    let attribute = |name: &str, report: Report<'_>, parse: &dyn Fn(&str) -> Option<()>| {
        let value = model.attribute(&[None], name)?;
        if parse(value).is_none() {
            report(local, name, value);
        }
        Some(value)
    };
    match local {
        "srgbClr" => {
            let value = attribute("val", report, &|v| rgb(v).map(drop))?;
            let (red, green, blue) = rgb(value)?;
            Some(SchemeColor::Rgb(red, green, blue))
        }
        "sysClr" => {
            let value = attribute("val", report, &|v| SystemColor::from_xml(v).map(drop))?;
            let last = attribute("lastClr", report, &|v| rgb(v).map(drop)).and_then(rgb);
            Some(SchemeColor::System {
                color: SystemColor::from_xml(value)?,
                last,
            })
        }
        _ => None,
    }
}

/// ST_HexColorRGB: three bytes, six hexadecimal digits.
fn rgb(value: &str) -> Option<(u8, u8, u8)> {
    if value.len() != 6 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(value.get(at..at + 2)?, 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

fn fonts(element: Element<'_>, names: &Names, notes: &mut Notes) -> FontCollection {
    let mut collection = FontCollection::default();
    let mut scripts: HashSet<&str> = HashSet::new();
    for (local, child) in once(element, names, &["latin", "ea", "cs"], notes) {
        let typeface = || child.attribute(&[None], "typeface").map(Into::into);
        match local {
            "latin" => collection.latin = typeface(),
            "ea" => collection.east_asian = typeface(),
            "cs" => collection.complex_script = typeface(),
            "font" => {
                let (Some(script), Some(typeface)) =
                    (child.attribute(&[None], "script"), typeface())
                else {
                    continue;
                };
                // The first font for a script counts, as for the other children.
                if !scripts.insert(script) {
                    notes.add(|| {
                        format!(
                            "a second a:font for the script {} is ignored",
                            shorten(script)
                        )
                    });
                    continue;
                }
                collection.scripts.push((script.into(), typeface));
            }
            _ => {}
        }
    }
    collection
}
