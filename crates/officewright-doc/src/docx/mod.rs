//! Reading WordprocessingML packages (`.docx`, `.docm`, `.dotx`, `.dotm`).

mod body;
mod capture;
pub(crate) mod properties;
mod section;
mod settings;
mod styles;
pub(crate) mod table_properties;

use officewright_opc::{Package, Part};

use crate::diagnostic::Diagnostics;
use crate::model::Names;
use crate::model::xml::Strings;
use crate::model::{Document, DocumentKind};
use crate::{Error, Limits, Result};
use capture::Budget;

/// The main namespace of WordprocessingML, Transitional.
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
/// The main namespace of WordprocessingML, Strict.
const W_STRICT: &str = "http://purl.oclc.org/ooxml/wordprocessingml/main";
/// The relationships namespace of attributes such as `r:id`, Transitional and Strict.
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const R_STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";

/// The relationship type of the main part, Transitional and Strict.
const OFFICE_DOCUMENT: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
    "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument",
];

impl Names {
    fn new(strings: &mut Strings) -> Self {
        Self {
            w: [Some(strings.intern(W)), Some(strings.intern(W_STRICT))],
            r: [Some(strings.intern(R)), Some(strings.intern(R_STRICT))],
            mc: Some(strings.intern(officewright_opc::mce::NAMESPACE)),
        }
    }
}

/// Opens a WordprocessingML package.
pub(crate) fn read(data: Vec<u8>, limits: &Limits) -> Result<Document> {
    let package = Package::from_bytes(data, limits.package())?;
    let main = main_part(&package)?;
    let kind = kind(&main)?;
    let xml = main.read()?;
    let mut document = Document::new();
    document.kind = kind;
    document.names = Names::new(&mut document.strings);
    let mut diagnostics = Diagnostics::default();
    let mut budget = Budget::new(limits.max_nodes());
    body::read(
        &mut document,
        &xml,
        &main,
        limits,
        &mut diagnostics,
        &mut budget,
    )?;
    if let Some(part) = related(&package, &main, &STYLES) {
        let name = part.name().as_str();
        let xml = part.read()?;
        styles::read(
            &mut document,
            &xml,
            name,
            limits,
            &mut diagnostics,
            &mut budget,
        )?;
    }
    if let Some(part) = related(&package, &main, &SETTINGS) {
        let name = part.name().as_str();
        let xml = part.read()?;
        settings::read(
            &mut document,
            &xml,
            name,
            limits,
            &mut diagnostics,
            &mut budget,
        )?;
    }
    document.diagnostics = diagnostics.finish();
    Ok(document)
}

/// The relationship type of the settings part, Transitional and Strict.
const SETTINGS: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings",
    "http://purl.oclc.org/ooxml/officeDocument/relationships/settings",
];

/// The relationship type of the styles part, Transitional and Strict.
const STYLES: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles",
    "http://purl.oclc.org/ooxml/officeDocument/relationships/styles",
];

/// The first part that `source` relates to with one of `types` and that exists.
fn related<'p>(package: &'p Package, source: &Part<'p>, types: &[&str]) -> Option<Part<'p>> {
    source
        .relationships()
        .iter()
        .filter(|relationship| types.contains(&relationship.relationship_type()))
        .find_map(|relationship| package.part(&relationship.target_part()?))
}

/// The main document part, which the package's officeDocument relationship names.
fn main_part(package: &Package) -> Result<Part<'_>> {
    let mut named = None;
    for relationship in package.relationships() {
        if !OFFICE_DOCUMENT.contains(&relationship.relationship_type()) {
            continue;
        }
        let Some(name) = relationship.target_part() else {
            continue;
        };
        if let Some(part) = package.part(&name) {
            return Ok(part);
        }
        named.get_or_insert(name);
    }
    Err(Error::Malformed {
        message: match named {
            Some(name) => format!("the main document part {name} is missing"),
            None => "the package has no main document part (no officeDocument relationship)".into(),
        },
    })
}

/// What the content type of the main part says the package is.
fn kind(main: &Part<'_>) -> Result<DocumentKind> {
    let Some(content_type) = main.content_type() else {
        return Err(Error::Malformed {
            message: format!("the main document part {} has no content type", main.name()),
        });
    };
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    let kinds = [
        (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
            DocumentKind::Document,
        ),
        (
            "application/vnd.ms-word.document.macroEnabled.main+xml",
            DocumentKind::MacroEnabledDocument,
        ),
        (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
            DocumentKind::Template,
        ),
        (
            "application/vnd.ms-word.template.macroEnabledTemplate.main+xml",
            DocumentKind::MacroEnabledTemplate,
        ),
    ];
    kinds
        .iter()
        .find(|(name, _)| essence.eq_ignore_ascii_case(name))
        .map(|&(_, kind)| kind)
        .ok_or_else(|| {
            Error::Unsupported(format!(
                "the main part {} is {}, not a Word document",
                main.name(),
                shorten(content_type)
            ))
        })
}

/// At most 64 characters of a value from the document, for a message.
pub(crate) fn shorten(value: &str) -> String {
    match value.char_indices().nth(64) {
        Some((end, _)) => format!("{}…", value.get(..end).unwrap_or_default()),
        None => value.to_owned(),
    }
}
