//! OOXML packages: parts, content types and relationships (ECMA-376 Part 2).

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::xml::{Event, Input, Reader, StartTag};
use crate::zip::{Archive, Entry};
use crate::{Error, Limit, Limits, Result};

mod content_types;
mod part_name;
mod relationships;
#[cfg(test)]
mod tests;

use content_types::ContentTypes;
pub use part_name::PartName;
pub use relationships::{Relationship, Relationships, TargetMode};

/// The first bytes of an OLE compound file: a binary Office document, or an OOXML
/// document encrypted with a password ([MS-OFFCRYPTO]).
const OLE_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// An OOXML package: a ZIP archive of parts, each with a content type, linked by
/// relationships.
///
/// Opening a package reads the ZIP central directory, `[Content_Types].xml` and every
/// relationships part. Other parts are decompressed only when [read](Part::read), so a
/// document whose images are never needed costs only their compressed size.
///
/// ```
/// use officewright_opc::{Limits, Package};
///
/// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/hello.docx"));
/// let package = Package::from_bytes(bytes.as_slice(), &Limits::new())?;
/// let main = package
///     .relationships()
///     .of_type("http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument")
///     .find_map(|relationship| package.part(relationship.target_part()?))
///     .expect("the test document has a main part");
/// assert_eq!(main.name().as_str(), "/word/document.xml");
/// assert_eq!(
///     main.content_type(),
///     Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml")
/// );
/// assert!(main.read()?.starts_with(b"<?xml"));
/// # Ok::<(), officewright_opc::Error>(())
/// ```
pub struct Package {
    archive: Archive,
    parts: Vec<PartEntry>,
    /// Index into `parts` by [`PartName::key`].
    index: HashMap<String, usize>,
    /// By the [`PartName::key`] of the source part; `""` for the package.
    relationships: HashMap<String, Relationships>,
    no_relationships: Relationships,
    max_total_size: u64,
    read_size: AtomicU64,
}

#[derive(Debug)]
struct PartEntry {
    name: PartName,
    entry: usize,
    content_type: Option<String>,
}

/// A part of a [`Package`].
#[derive(Clone, Copy)]
pub struct Part<'p> {
    package: &'p Package,
    entry: &'p PartEntry,
}

impl Package {
    /// Opens the package in `data`.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] for an OLE compound file (an encrypted OOXML document or
    /// a binary Office document); [`Error::Package`] when `[Content_Types].xml` is
    /// missing, a part name is invalid or two parts have the same name; and the errors
    /// of [`Archive::new`], and of reading `[Content_Types].xml` and the relationships
    /// parts.
    pub fn from_bytes(data: impl Into<Vec<u8>>, limits: &Limits) -> Result<Self> {
        let data = data.into();
        if data.starts_with(&OLE_SIGNATURE) {
            return Err(Error::Unsupported(
                "the input is an OLE compound file, not a zip package: an encrypted OOXML \
                 document or a binary Office document"
                    .into(),
            ));
        }
        let archive = Archive::new(data, limits)?;
        let mut content_types = None;
        let mut parts = Vec::new();
        let mut index = HashMap::new();
        for (entry, item) in archive.entries().enumerate() {
            // Some writers separate folders with '\'; Office opens such packages (POI's
            // test-data/spreadsheet/49609.xlsx).
            let name = item.name().replace('\\', "/");
            if name.ends_with('/') {
                // A folder entry, which some zip tools write; it is not a part.
                continue;
            }
            if name.eq_ignore_ascii_case("[Content_Types].xml") {
                if content_types.replace(item).is_some() {
                    return Err(Error::Package(
                        "the package has two [Content_Types].xml entries".into(),
                    ));
                }
                continue;
            }
            if is_piece(&name) {
                return Err(Error::Unsupported(format!(
                    "interleaved parts such as {name} are not supported"
                )));
            }
            let name = PartName::new(&format!("/{name}"))?;
            if index.insert(name.key().to_owned(), parts.len()).is_some() {
                return Err(Error::Package(format!(
                    "two parts are named {name}, ignoring case"
                )));
            }
            parts.push(PartEntry {
                name,
                entry,
                content_type: None,
            });
        }
        let content_types = content_types.ok_or_else(|| {
            Error::Package(
                "[Content_Types].xml is missing: the input is not an OOXML package".into(),
            )
        })?;

        let mut read_size = 0;
        let mut read = |entry: Entry<'_>| {
            read_size = reserve(read_size, &entry, limits.max_total_size())?;
            entry.read()
        };
        let types = ContentTypes::parse(&read(content_types)?, limits)?;
        let mut relationships = HashMap::new();
        for part in &mut parts {
            part.content_type = types.get(&part.name).map(str::to_owned);
            let Some(source) = part.name.relationships_source() else {
                continue;
            };
            let Some(item) = archive.entry(part.entry) else {
                continue;
            };
            let (key, folder) = source
                .as_ref()
                .map_or(("", ""), |source| (source.key(), source.folder()));
            let parsed = Relationships::parse(&read(item)?, limits, part.name.as_str(), folder)?;
            relationships.insert(key.to_owned(), parsed);
        }
        Ok(Self {
            archive,
            parts,
            index,
            relationships,
            no_relationships: Relationships::default(),
            max_total_size: limits.max_total_size(),
            read_size: AtomicU64::new(read_size),
        })
    }

    /// The parts in the order of the ZIP archive, relationships parts included.
    pub fn parts(&self) -> impl ExactSizeIterator<Item = Part<'_>> {
        self.parts.iter().map(|entry| Part {
            package: self,
            entry,
        })
    }

    /// The part with this name.
    pub fn part(&self, name: &PartName) -> Option<Part<'_>> {
        let entry = self.parts.get(*self.index.get(name.key())?)?;
        Some(Part {
            package: self,
            entry,
        })
    }

    /// The package relationships, from `/_rels/.rels`; empty when it is missing.
    pub fn relationships(&self) -> &Relationships {
        self.relationships.get("").unwrap_or(&self.no_relationships)
    }
}

impl fmt::Debug for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Package")
            .field("parts", &self.parts)
            .finish_non_exhaustive()
    }
}

impl<'p> Part<'p> {
    /// The part name.
    pub fn name(&self) -> &'p PartName {
        &self.entry.name
    }

    /// The content type from `[Content_Types].xml`; `None` if it gives none.
    pub fn content_type(&self) -> Option<&'p str> {
        self.entry.content_type.as_deref()
    }

    /// The uncompressed size the archive declares, in bytes.
    pub fn size(&self) -> u64 {
        self.archive_entry().map_or(0, |entry| entry.size())
    }

    /// The relationships whose source is this part; empty if it has none.
    pub fn relationships(&self) -> &'p Relationships {
        self.package
            .relationships
            .get(self.entry.name.key())
            .unwrap_or(&self.package.no_relationships)
    }

    /// Decompresses the part.
    ///
    /// # Errors
    ///
    /// [`Error::LimitExceeded`] once the parts read from the package exceed
    /// [`Limits::max_total_size`], and the errors of [`Entry::read`].
    pub fn read(&self) -> Result<Vec<u8>> {
        let entry = self
            .archive_entry()
            .ok_or_else(|| Error::Package(format!("part {} has no zip entry", self.entry.name)))?;
        let max = self.package.max_total_size;
        let mut used = self.package.read_size.load(Ordering::Relaxed);
        loop {
            let total = reserve(used, &entry, max)?;
            match self.package.read_size.compare_exchange_weak(
                used,
                total,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => used = actual,
            }
        }
        entry.read()
    }

    fn archive_entry(&self) -> Option<Entry<'p>> {
        self.package.archive.entry(self.entry.entry)
    }
}

impl fmt::Debug for Part<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Part")
            .field("name", &self.entry.name)
            .field("content_type", &self.entry.content_type)
            .finish_non_exhaustive()
    }
}

/// Adds an entry's size to the bytes read so far, within [`Limits::max_total_size`].
fn reserve(used: u64, entry: &Entry<'_>, max: u64) -> Result<u64> {
    used.checked_add(entry.size())
        .filter(|&total| total <= max)
        .ok_or_else(|| total_size_error(entry, max))
}

fn total_size_error(entry: &Entry<'_>, max: u64) -> Error {
    Error::LimitExceeded {
        limit: Limit::TotalSize,
        maximum: max,
        context: format!("zip entry {}", entry.name()),
    }
}

/// Whether a ZIP item is a piece of an interleaved part, `.../[0].piece` or
/// `.../[5].last.piece` (ECMA-376 Part 2, Annex B).
fn is_piece(name: &str) -> bool {
    let file = name
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some(rest) = file.strip_prefix('[') else {
        return false;
    };
    let Some((number, suffix)) = rest.split_once(']') else {
        return false;
    };
    !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
        && (suffix == ".piece" || suffix == ".last.piece")
}

/// Reads the flat XML of `[Content_Types].xml` and relationships parts: a root element
/// in `namespace` whose children are empty elements, each passed to `child`. Whitespace,
/// comments and processing instructions are allowed between them, nothing else.
fn read_flat_xml(
    bytes: &[u8],
    limits: &Limits,
    part: &str,
    namespace: &str,
    root: &str,
    mut child: impl FnMut(&Reader<'_>, StartTag<'_>) -> Result<()>,
) -> Result<()> {
    let input = Input::decode(bytes).map_err(|err| err.in_part(part))?;
    let mut reader = input.reader(limits);
    let namespace = reader.intern(namespace);
    read_children(&mut reader, namespace, root, &mut child).map_err(|err| err.in_part(part))
}

fn read_children(
    reader: &mut Reader<'_>,
    namespace: crate::xml::NsId,
    root: &str,
    child: &mut impl FnMut(&Reader<'_>, StartTag<'_>) -> Result<()>,
) -> Result<()> {
    let mut depth_of_root = false;
    while let Some(event) = reader.read_event()? {
        match event {
            Event::Start(tag) if !depth_of_root => {
                if !tag.name().is(namespace, root) {
                    return Err(reader.error(
                        tag.offset(),
                        format!(
                            "the root element is <{}>, not {root} in {}",
                            tag.name().qname().as_str(),
                            reader.namespace_uri(namespace)
                        ),
                    ));
                }
                depth_of_root = true;
            }
            Event::Start(tag) => {
                if tag.name().namespace() != Some(namespace) {
                    return Err(reader.error(
                        tag.offset(),
                        format!("unexpected element <{}>", tag.name().qname().as_str()),
                    ));
                }
                let offset = tag.offset();
                let name = tag.name().qname().as_str();
                child(reader, tag)?;
                loop {
                    match reader.read_event()? {
                        Some(Event::End(_)) => break,
                        Some(Event::Text(text)) if is_whitespace(&text) => {}
                        Some(Event::Comment(_) | Event::ProcessingInstruction(_)) => {}
                        _ => {
                            return Err(reader.error(offset, format!("<{name}> must be empty")));
                        }
                    }
                }
            }
            Event::Text(text) if !is_whitespace(&text) => {
                return Err(reader.error(reader.offset(), "unexpected text"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_whitespace(text: &str) -> bool {
    text.bytes()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}
