//! Opens OOXML packages and reads every XML part they reach through relationships, as a
//! document reader would (following Markup Compatibility, skipping what it does not
//! read), for checking officewright-opc by hand and over a corpus.
//!
//! ```text
//! cargo run -p officewright-opc --example package_report -- [--verbose] FILE...
//! ```
//!
//! Prints one line per file that fails (every file with `--verbose`) and a summary: how
//! many files opened and had all their XML parts read, and the errors of the others by
//! kind.

use std::collections::{BTreeMap, HashSet};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use officewright_opc::mce::{Disposition, Processor};
use officewright_opc::xml::{Event, Input};
use officewright_opc::{Error, Limits, Package, PartName, Relationships};

fn main() -> ExitCode {
    let mut verbose = false;
    let mut files = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--verbose" => verbose = true,
            _ => files.push(arg),
        }
    }
    if files.is_empty() {
        eprintln!("usage: package_report [--verbose] FILE...");
        return ExitCode::from(2);
    }
    let limits = Limits::new();
    let mut ok = 0;
    let mut parts = 0;
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    let mut slowest = (Duration::ZERO, String::new());
    for file in &files {
        let start = Instant::now();
        let result = std::fs::read(file)
            .map_err(|err| ("io", err.to_string()))
            .and_then(|bytes| check(bytes, &limits).map_err(|err| (kind(&err), err.to_string())));
        let elapsed = start.elapsed();
        if elapsed > slowest.0 {
            slowest = (elapsed, file.clone());
        }
        match result {
            Ok(count) => {
                ok += 1;
                parts += count;
                if verbose {
                    println!("ok    {file}: {count} xml parts in {elapsed:.1?}");
                }
            }
            Err((kind, message)) => {
                *kinds.entry(kind).or_default() += 1;
                println!("error {file}: {message}");
            }
        }
    }
    println!(
        "\n{ok} of {} files opened with all {parts} xml parts read",
        files.len()
    );
    for (kind, count) in &kinds {
        println!("{count:>6} {kind}");
    }
    println!("slowest: {:.1?} {}", slowest.0, slowest.1);
    ExitCode::SUCCESS
}

/// Opens a package and reads every XML part it reaches through relationships, following
/// Markup Compatibility; returns the number of XML parts.
fn check(bytes: Vec<u8>, limits: &Limits) -> Result<usize, Error> {
    let package = Package::from_bytes(bytes, limits)?;
    let mut seen = HashSet::new();
    let mut queue = targets(package.relationships());
    let mut count = 0;
    while let Some(name) = queue.pop() {
        let Some(part) = package.part(&name) else {
            continue;
        };
        if !seen.insert(name) {
            continue;
        }
        queue.extend(targets(part.relationships()));
        let is_xml = part.content_type().is_some_and(|kind| {
            kind.ends_with("+xml") || kind == "application/xml" || kind == "text/xml"
        });
        if is_xml {
            let data = part.read()?;
            read_xml(&data, limits).map_err(|err| err.in_part(part.name().as_str()))?;
            count += 1;
        }
    }
    Ok(count)
}

fn targets(relationships: &Relationships) -> Vec<PartName> {
    relationships
        .iter()
        .filter_map(|relationship| relationship.target_part())
        .collect()
}

/// The namespaces of ECMA-376, Transitional and Strict, that a document reader
/// implements. Office's extensions (w14, w15 and the like) are left to Markup
/// Compatibility, which documents declare them ignorable for.
const UNDERSTOOD: &[&str] = &[
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
    "http://schemas.openxmlformats.org/presentationml/2006/main",
    "http://schemas.openxmlformats.org/drawingml/2006/main",
    "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing",
    "http://schemas.openxmlformats.org/drawingml/2006/picture",
    "http://schemas.openxmlformats.org/drawingml/2006/chart",
    "http://schemas.openxmlformats.org/drawingml/2006/diagram",
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://schemas.openxmlformats.org/officeDocument/2006/math",
    "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties",
    "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties",
    "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes",
    "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
    "http://purl.org/dc/elements/1.1/",
    "http://purl.org/dc/terms/",
    "urn:schemas-microsoft-com:vml",
    "urn:schemas-microsoft-com:office:office",
    "urn:schemas-microsoft-com:office:word",
    "urn:schemas-microsoft-com:office:excel",
    "urn:schemas-microsoft-com:office:powerpoint",
    "http://purl.oclc.org/ooxml/wordprocessingml/main",
    "http://purl.oclc.org/ooxml/spreadsheetml/main",
    "http://purl.oclc.org/ooxml/presentationml/main",
    "http://purl.oclc.org/ooxml/drawingml/main",
    "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing",
    "http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing",
    "http://purl.oclc.org/ooxml/drawingml/picture",
    "http://purl.oclc.org/ooxml/drawingml/chart",
    "http://purl.oclc.org/ooxml/drawingml/diagram",
    "http://purl.oclc.org/ooxml/officeDocument/relationships",
    "http://purl.oclc.org/ooxml/officeDocument/math",
];

/// Reads a part the way a document reader does: it reads into elements of the
/// namespaces above, takes one branch of each `mc:AlternateContent`, and skips the rest,
/// as [`Processor`] describes.
fn read_xml(data: &[u8], limits: &Limits) -> Result<(), Error> {
    let input = Input::decode(data)?;
    let mut reader = input.reader(limits);
    let mut mce = Processor::new(&mut reader);
    for uri in UNDERSTOOD {
        let namespace = reader.intern(uri);
        mce.understand(namespace);
    }
    let mc = reader.intern(officewright_opc::mce::NAMESPACE);
    // Whether a branch of each open mc:AlternateContent has been taken.
    let mut alternates: Vec<bool> = Vec::new();
    while let Some(event) = reader.read_event()? {
        match event {
            Event::Start(tag) => {
                let disposition = mce.start(&reader, &tag)?;
                let name = tag.name();
                let read = if name.is(mc, "AlternateContent") {
                    alternates.push(false);
                    true
                } else if name.is(mc, "Choice") || name.is(mc, "Fallback") {
                    let selectable = name.is(mc, "Fallback") || mce.is_selectable(&reader, &tag)?;
                    match alternates.last_mut() {
                        Some(taken) if !*taken && selectable => {
                            *taken = true;
                            true
                        }
                        _ => false,
                    }
                } else {
                    matches!(
                        disposition,
                        Disposition::Understood | Disposition::ProcessContent
                    )
                };
                if !read {
                    reader.skip_element()?;
                    mce.end();
                }
            }
            Event::End(tag) => {
                if tag.name().is(mc, "AlternateContent") {
                    alternates.pop();
                }
                mce.end();
            }
            _ => {}
        }
    }
    Ok(())
}

fn kind(err: &Error) -> &'static str {
    match err {
        Error::Zip { .. } => "zip",
        Error::Entry { .. } => "zip entry",
        Error::Xml { .. } => "xml",
        Error::Package { .. } => "package",
        Error::LimitExceeded { .. } => "limit exceeded",
        Error::Unsupported(_) => "unsupported",
        _ => "other",
    }
}
