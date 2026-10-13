//! The font table part (`w:fonts`, ECMA-376 Part 1, §17.8): each `w:font` is read, and
//! every element is kept for writing.

use officewright_opc::mce::{Disposition, Processor};
use officewright_opc::xml::{Event, Input};

use super::capture::{Budget, Capture};
use super::properties::{Report, on_off, on_off_attribute, value};
use super::{R, R_STRICT, W, W_STRICT, shorten};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::model::fonts::{
    EmbeddedFontReference, EmbeddedFontStyle, FontDeclaration, FontFamily, FontPitch, FontSignature,
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
    let w = [reader.intern(W), reader.intern(W_STRICT)];
    for namespace in w.iter().chain(&[reader.intern(R), reader.intern(R_STRICT)]) {
        mce.understand(*namespace);
    }
    let names = document.names;
    let mut fonts: Vec<FontDeclaration> = Vec::new();
    let mut c = Capture::new(document, part, input.as_str(), diagnostics, budget);
    let mut in_root = false;
    while let Some(event) = reader.read_event().map_err(opc)? {
        match event {
            Event::Start(tag) => {
                c.at = tag.offset();
                let disposition = mce.start(&reader, &tag).map_err(opc)?;
                if !in_root {
                    if !w.iter().any(|&namespace| tag.name().is(namespace, "fonts")) {
                        let message = format!(
                            "the root element is <{}>, not w:fonts",
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
                // Every element is kept with the document's tokens for writing.
                let span = c.capture(&mut reader, &mut mce, tag)?;
                if !(understood && local == "font") {
                    continue;
                }
                let mut read = None;
                c.check(span, offset, |element, report| {
                    read = font(element, &names, report);
                });
                let Some(font) = read else {
                    c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
                        "a w:font without a w:name is ignored".into()
                    });
                    continue;
                };
                fonts.push(font);
            }
            Event::End(_) => mce.end(),
            _ => {}
        }
    }
    c.document.fonts = fonts;
    Ok(())
}

/// A `w:font`; `None` without a `w:name`.
fn font(element: Element<'_>, names: &Names, report: Report<'_>) -> Option<FontDeclaration> {
    let w = &names.w;
    let mut font = FontDeclaration {
        name: element.attribute(w, "name")?.into(),
        alternative_name: None,
        panose: None,
        charset: None,
        character_set: None,
        family: None,
        pitch: None,
        signature: None,
        not_true_type: None,
        embedded: Vec::new(),
    };
    for child in element.children_through_fallback(names.mc) {
        let Some(local) = child.local_in(w) else {
            continue;
        };
        match local {
            "altName" => font.alternative_name = child.attribute(w, "val").map(Into::into),
            "panose1" => font.panose = value(child, names, "val", panose, report),
            "charset" => {
                // ST_UcharHexNumber: one byte, two hexadecimal digits.
                font.charset = value(
                    child,
                    names,
                    "val",
                    |v| {
                        (v.len() == 2)
                            .then(|| u8::from_str_radix(v, 16).ok())
                            .flatten()
                    },
                    report,
                );
                font.character_set = child.attribute(w, "characterSet").map(Into::into);
            }
            "family" => font.family = value(child, names, "val", FontFamily::from_xml, report),
            "pitch" => font.pitch = value(child, names, "val", FontPitch::from_xml, report),
            "sig" => {
                let bits = |attribute, report: Report<'_>| {
                    value(child, names, attribute, long_hex, report).unwrap_or_default()
                };
                font.signature = Some(FontSignature {
                    unicode: [
                        bits("usb0", report),
                        bits("usb1", report),
                        bits("usb2", report),
                        bits("usb3", report),
                    ],
                    code_pages: [bits("csb0", report), bits("csb1", report)],
                });
            }
            "notTrueType" => font.not_true_type = on_off(child, names, report),
            "embedRegular" | "embedBold" | "embedItalic" | "embedBoldItalic" => {
                let (Some(style), Some(id)) = (
                    EmbeddedFontStyle::from_xml(local),
                    child.attribute(&names.r, "id"),
                ) else {
                    continue;
                };
                font.embedded.push(EmbeddedFontReference {
                    style,
                    relationship_id: id.into(),
                    key: child.attribute(w, "fontKey").map(Into::into),
                    subsetted: on_off_attribute(child, names, "subsetted", report),
                });
            }
            _ => {}
        }
    }
    Some(font)
}

/// ST_Panose: ten bytes, twenty hexadecimal digits.
fn panose(value: &str) -> Option<[u8; 10]> {
    if value.len() != 20 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; 10];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(value.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
}

/// ST_LongHexNumber: four bytes, eight hexadecimal digits.
fn long_hex(value: &str) -> Option<u32> {
    (value.len() == 8 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(value, 16).ok())
        .flatten()
}
