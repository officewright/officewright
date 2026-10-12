//! Opens OOXML packages and reads every XML part they reach through relationships, as a
//! document reader would, for checking officewright-opc by hand and over a corpus.
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

use officewright_opc::mce::Processor;
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

fn read_xml(data: &[u8], limits: &Limits) -> Result<(), Error> {
    let input = Input::decode(data)?;
    let mut reader = input.reader(limits);
    let mut mce = Processor::new(&mut reader);
    while let Some(event) = reader.read_event()? {
        match event {
            Event::Start(tag) => mce.start(&reader, &tag)?,
            Event::End(_) => mce.end(),
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
