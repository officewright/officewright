//! The font table part (`w:fonts`, ECMA-376 Part 1, §17.8): each `w:font` is read, and
//! every element is kept for writing. Attributes are kept as written; typed values are
//! checked, and a child written twice is ignored after the first, with a diagnostic, as
//! in `w:sectPr`. An `mc:AlternateContent` directly under `w:fonts` is kept whole, and
//! the fonts in the branch it takes are read.

use officewright_opc::mce::Processor;
use officewright_opc::xml::{Event, Input};

use super::capture::{Budget, Capture, Notes};
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
                mce.start(&reader, &tag).map_err(opc)?;
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
                // Every element is kept with the document's tokens for writing. An
                // mc:AlternateContent is kept whole, and the children of the branch it
                // takes are read in its place.
                let span = c.capture(&mut reader, &mut mce, tag)?;
                for span in c.through_alternates(span) {
                    if c.document.element(span).local_in(&names.w) != Some("font") {
                        continue;
                    }
                    let mut read = None;
                    let mut notes = c.notes();
                    c.check(span, offset, |element, report| {
                        read = font(element, &names, report, &mut notes);
                    });
                    c.diagnose_notes(offset, notes);
                    let Some(font) = read else {
                        c.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
                            "a w:font without a w:name is ignored".into()
                        });
                        continue;
                    };
                    fonts.push(font);
                }
            }
            Event::End(_) => mce.end(),
            _ => {}
        }
    }
    // An index of the names in lower case, built once: the first declaration of a name
    // counts.
    let mut index = std::collections::HashMap::new();
    for (n, font) in fonts.iter().enumerate() {
        if let Ok(n) = u32::try_from(n) {
            index
                .entry(crate::model::fold_case(font.name()))
                .or_insert(n);
        }
    }
    c.document.fonts = fonts;
    c.document.font_index = index;
    Ok(())
}

/// The children of a `w:font`, each of which the schema allows once.
const ONCE: [&str; 11] = [
    "altName",
    "panose1",
    "charset",
    "family",
    "notTrueType",
    "pitch",
    "sig",
    "embedRegular",
    "embedBold",
    "embedItalic",
    "embedBoldItalic",
];

/// A `w:font`; `None` without a `w:name`.
fn font(
    element: Element<'_>,
    names: &Names,
    report: Report<'_>,
    notes: &mut Notes,
) -> Option<FontDeclaration> {
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
    let mut seen: Vec<&str> = Vec::new();
    for child in element.children_through_alternates(names.mc) {
        let Some(local) = child.local_in(w) else {
            continue;
        };
        // CT_Font writes each of its children once.
        if ONCE.contains(&local) {
            if seen.contains(&local) {
                notes.add(|| format!("a second w:{local} in a w:font is ignored"));
                continue;
            }
            seen.push(local);
        }
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
                        (v.len() == 2 && v.bytes().all(|b| b.is_ascii_hexdigit()))
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
                // CT_FontSig requires all six; without one, or with one not valid, the
                // signature is not read, since zeros would say the font covers nothing.
                const BITS: [&str; 6] = ["usb0", "usb1", "usb2", "usb3", "csb0", "csb1"];
                if BITS.iter().any(|bits| child.attribute(w, bits).is_none()) {
                    notes.add(|| "a w:sig without all six of its attributes is ignored".into());
                }
                let mut read = [None; 6];
                for (slot, bits) in read.iter_mut().zip(BITS) {
                    *slot = value(child, names, bits, long_hex, report);
                }
                if let [Some(u0), Some(u1), Some(u2), Some(u3), Some(c0), Some(c1)] = read {
                    font.signature = Some(FontSignature {
                        unicode: [u0, u1, u2, u3],
                        code_pages: [c0, c1],
                    });
                }
            }
            "notTrueType" => font.not_true_type = on_off(child, names, report),
            "embedRegular" | "embedBold" | "embedItalic" | "embedBoldItalic" => {
                let (Some(style), Some(id)) = (
                    EmbeddedFontStyle::from_xml(local),
                    child.attribute(&names.r, "id"),
                ) else {
                    notes.add(|| format!("a w:{local} without r:id is ignored"));
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
