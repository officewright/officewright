//! The theme part (`a:theme`, ECMA-376 Part 1, §20.1.6): the colour and font schemes
//! are read, and every element is kept for writing.

use officewright_opc::mce::{Disposition, Processor};
use officewright_opc::xml::{Event, Input};

use super::capture::{Budget, Capture};
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
                let understood = disposition == Disposition::Understood
                    && tag.name().namespace().is_some_and(|ns| a.contains(&ns));
                let local = tag.name().local();
                // Every element is kept with the document's tokens for writing.
                let span = c.capture(&mut reader, &mut mce, tag)?;
                if understood && local == "themeElements" {
                    c.check_in(["a:", ""], span, offset, |element, report| {
                        theme_elements(&mut theme, element, &names, report);
                    });
                }
            }
            Event::End(_) => mce.end(),
            _ => {}
        }
    }
    c.document.theme = Some(theme);
    Ok(())
}

fn theme_elements(theme: &mut Theme, element: Element<'_>, names: &Names, report: Report<'_>) {
    for child in element.children_through_fallback(names.mc) {
        match child.local_in(&names.a) {
            Some("clrScheme") => theme.colors = Some(color_scheme(child, names, report)),
            Some("fontScheme") => {
                let mut scheme = FontScheme {
                    name: child.attribute(&[None], "name").map(Into::into),
                    ..FontScheme::default()
                };
                for collection in child.children_through_fallback(names.mc) {
                    match collection.local_in(&names.a) {
                        Some("majorFont") => scheme.major = fonts(collection, names),
                        Some("minorFont") => scheme.minor = fonts(collection, names),
                        _ => {}
                    }
                }
                theme.fonts = Some(scheme);
            }
            _ => {}
        }
    }
}

fn color_scheme(element: Element<'_>, names: &Names, report: Report<'_>) -> ColorScheme {
    let mut scheme = ColorScheme {
        name: element.attribute(&[None], "name").map(Into::into),
        ..ColorScheme::default()
    };
    for child in element.children_through_fallback(names.mc) {
        let index = match child.local_in(&names.a) {
            Some("dk1") => ColorSchemeIndex::Dark1,
            Some("lt1") => ColorSchemeIndex::Light1,
            Some("dk2") => ColorSchemeIndex::Dark2,
            Some("lt2") => ColorSchemeIndex::Light2,
            Some("accent1") => ColorSchemeIndex::Accent1,
            Some("accent2") => ColorSchemeIndex::Accent2,
            Some("accent3") => ColorSchemeIndex::Accent3,
            Some("accent4") => ColorSchemeIndex::Accent4,
            Some("accent5") => ColorSchemeIndex::Accent5,
            Some("accent6") => ColorSchemeIndex::Accent6,
            Some("hlink") => ColorSchemeIndex::Hyperlink,
            Some("folHlink") => ColorSchemeIndex::FollowedHyperlink,
            _ => continue,
        };
        let position = position(index);
        let color = child
            .children_through_fallback(names.mc)
            .into_iter()
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

fn fonts(element: Element<'_>, names: &Names) -> FontCollection {
    let mut collection = FontCollection::default();
    for child in element.children_through_fallback(names.mc) {
        let typeface = || child.attribute(&[None], "typeface").map(Into::into);
        match child.local_in(&names.a) {
            Some("latin") => collection.latin = typeface(),
            Some("ea") => collection.east_asian = typeface(),
            Some("cs") => collection.complex_script = typeface(),
            Some("font") => {
                if let (Some(script), Some(typeface)) =
                    (child.attribute(&[None], "script"), typeface())
                {
                    collection.scripts.push((script.into(), typeface));
                }
            }
            _ => {}
        }
    }
    collection
}
