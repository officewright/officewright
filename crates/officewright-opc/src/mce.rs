//! Markup Compatibility (ECMA-376 Part 3): what a reader does with elements and
//! attributes from namespaces it does not implement.
//!
//! A [`Processor`] follows the compatibility attributes as a part is read and tells the
//! consumer, for each element and attribute, whether it is [understood](Disposition),
//! declared ignorable, or neither:
//!
//! - **Understood**: an element from a namespace the consumer implements or from the
//!   Markup Compatibility namespace; an attribute from such a namespace, from `xml`, or
//!   without a prefix (it belongs to its element).
//! - **Ignore**: from a namespace declared in `mc:Ignorable`. The consumer does not
//!   interpret it, but keeps it to write it back unchanged.
//! - **Process content**: an ignorable element listed in `mc:ProcessContent`. Its
//!   children belong to its parent.
//! - **Not understood**: from a namespace that is neither (an element without a
//!   namespace included). Part 3 asks consumers to reject such content; officewright
//!   keeps it as unknown content instead, the way Office keeps content it does not know.
//!
//! In `mc:AlternateContent`, the consumer reads the first `mc:Choice` that
//! [`is_selectable`](Processor::is_selectable), or the `mc:Fallback` if none is, and
//! skips the others; [`Processor`] describes how to skip.
//!
//! ```
//! use officewright_opc::Limits;
//! use officewright_opc::mce::{Disposition, Processor};
//! use officewright_opc::xml::{Event, Input};
//!
//! let xml = r#"<w:document xmlns:w="urn:w" xmlns:w14="urn:w14"
//!     xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
//!     mc:Ignorable="w14">
//!   <w:p w14:paraId="1A2B"/>
//!   <mc:AlternateContent>
//!     <mc:Choice Requires="w14"><w14:shape/></mc:Choice>
//!     <mc:Fallback><w:picture/></mc:Fallback>
//!   </mc:AlternateContent>
//! </w:document>"#;
//! let input = Input::decode(xml.as_bytes())?;
//! let mut reader = input.reader(&Limits::new());
//! let mut mce = Processor::new(&mut reader);
//! let w = reader.intern("urn:w");
//! let mc = reader.intern(officewright_opc::mce::NAMESPACE);
//! mce.understand(w);
//! // Whether a branch of each open mc:AlternateContent has been taken.
//! let mut alternates = Vec::new();
//! let mut read = Vec::new();
//! while let Some(event) = reader.read_event()? {
//!     match event {
//!         Event::Start(tag) => {
//!             let disposition = mce.start(&reader, &tag)?;
//!             let name = tag.name();
//!             let take = if name.is(mc, "AlternateContent") {
//!                 alternates.push(false);
//!                 true
//!             } else if name.is(mc, "Choice") || name.is(mc, "Fallback") {
//!                 let taken = alternates.last_mut().expect("inside mc:AlternateContent");
//!                 let take = !*taken
//!                     && (name.is(mc, "Fallback") || mce.is_selectable(&reader, &tag)?);
//!                 *taken |= take;
//!                 take
//!             } else {
//!                 disposition == Disposition::Understood
//!             };
//!             if !take {
//!                 // Skipped content is kept, not interpreted: no start() inside it.
//!                 reader.skip_element()?;
//!                 mce.end();
//!                 continue;
//!             }
//!             if name.is(w, "p") {
//!                 let para_id = tag.attributes()[0].name();
//!                 assert_eq!(mce.attribute(para_id), Disposition::Ignore);
//!             }
//!             read.push(name.local().to_owned());
//!         }
//!         Event::End(tag) => {
//!             if tag.name().is(mc, "AlternateContent") {
//!                 alternates.pop();
//!             }
//!             mce.end();
//!         }
//!         _ => {}
//!     }
//! }
//! // w14 is not understood, so the fallback is read.
//! assert_eq!(read, ["document", "p", "AlternateContent", "Fallback", "picture"]);
//! # Ok::<(), officewright_opc::Error>(())
//! ```

use std::collections::{HashMap, HashSet};

use crate::xml::{Name, NsId, Reader, StartTag};
use crate::{Error, Limit, Result};

/// The Markup Compatibility namespace. ISO/IEC 29500 Strict uses it too.
pub const NAMESPACE: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

/// What a consumer does with an element or attribute; see the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Disposition {
    /// The consumer implements its namespace.
    Understood,
    /// Its namespace is ignorable: keep it, do not interpret it.
    Ignore,
    /// An ignorable element whose children are read as children of its parent.
    ProcessContent,
    /// Neither understood nor ignorable.
    NotUnderstood,
}

/// Follows the Markup Compatibility attributes of one part; see the [module docs](self).
///
/// The consumer calls [`start`](Self::start) for every start tag it reads into, and it
/// returns what to do with that element. For an element the consumer reads into
/// (understood, or ignorable with its content processed), it calls [`end`](Self::end) at
/// the end tag. For an element it skips (ignored, not understood, an `mc:Choice` that is
/// not selected, an `mc:Fallback` after a selected Choice), it calls
/// [`Reader::skip_element`] and then `end` once, and never `start` for anything inside:
/// skipped content is kept, not interpreted, so its `mc:MustUnderstand` does not apply.
///
/// Every lookup takes constant time and a name that an attribute repeats, or that an
/// outer element already declared, is kept once, so the work grows with the size of
/// the part, however its attributes repeat themselves. Each attribute may list at most
/// [`Limits::max_mce_names`](crate::Limits::max_mce_names) names.
#[derive(Debug)]
pub struct Processor {
    mc: NsId,
    understood: HashSet<NsId>,
    /// The ignorable namespaces in scope.
    ignorable: HashSet<NsId>,
    /// Namespaces all of whose elements have their content processed (`p:*`).
    process_all: HashSet<NsId>,
    /// Elements whose content is processed, by namespace.
    process_names: HashMap<NsId, HashSet<Box<str>>>,
    /// What the open elements added to the sets above, outermost first, so that `end`
    /// takes out exactly what its element put in.
    added: Vec<Added>,
    /// The length of `added` before each open element.
    frames: Vec<usize>,
}

#[derive(Debug)]
enum Added {
    Ignorable(NsId),
    ProcessAll(NsId),
    ProcessName(NsId, Box<str>),
}

impl Processor {
    /// A processor for the part `reader` reads, understanding no namespace yet.
    pub fn new(reader: &mut Reader<'_>) -> Self {
        Self {
            mc: reader.intern(NAMESPACE),
            understood: HashSet::new(),
            ignorable: HashSet::new(),
            process_all: HashSet::new(),
            process_names: HashMap::new(),
            added: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// Declares that the consumer implements a namespace.
    pub fn understand(&mut self, namespace: NsId) {
        self.understood.insert(namespace);
    }

    /// Takes in the compatibility attributes of a start tag the reader just returned and
    /// tells what to do with its element; the element's own `mc:Ignorable` and
    /// `mc:ProcessContent` count. See [`Processor`] for when to call
    /// [`end`](Self::end).
    ///
    /// # Errors
    ///
    /// [`Error::Xml`] for an attribute that names an undeclared prefix or is not a
    /// Markup Compatibility attribute, [`Error::LimitExceeded`] for an attribute that
    /// lists more than [`Limits::max_mce_names`](crate::Limits::max_mce_names) names,
    /// and [`Error::Unsupported`] when the element is to be read and its
    /// `mc:MustUnderstand` names a namespace the consumer does not understand.
    pub fn start(&mut self, reader: &Reader<'_>, tag: &StartTag<'_>) -> Result<Disposition> {
        self.frames.push(self.added.len());
        let mut must_understand = None;
        for attribute in tag.attributes() {
            if attribute.name().namespace() != Some(self.mc) {
                continue;
            }
            let names = names(reader, tag, attribute.value())?;
            match attribute.name().local() {
                // A prefix that is not declared cannot name any element or attribute, so
                // the lists of ignorable and preserved names skip it rather than reject
                // the part. LibreOffice's sw/qa/extras/layout/data/
                // floattable-center-shift-down.docx lists an undeclared w14.
                "Ignorable" => {
                    for namespace in names.filter_map(|prefix| declared(reader, prefix)) {
                        if self.ignorable.insert(namespace) {
                            self.added.push(Added::Ignorable(namespace));
                        }
                    }
                }
                "MustUnderstand" => {
                    let mut namespaces = Vec::new();
                    for prefix in names {
                        namespaces.push(resolve(reader, tag, "MustUnderstand", prefix)?);
                    }
                    must_understand = Some(namespaces);
                }
                "ProcessContent" => {
                    for name in names {
                        let (prefix, local) = qualified(reader, tag, "ProcessContent", name)?;
                        let Some(namespace) = declared(reader, prefix) else {
                            continue;
                        };
                        if local == "*" {
                            if self.process_all.insert(namespace) {
                                self.added.push(Added::ProcessAll(namespace));
                            }
                        } else if self
                            .process_names
                            .entry(namespace)
                            .or_default()
                            .insert(local.into())
                        {
                            self.added.push(Added::ProcessName(namespace, local.into()));
                        }
                    }
                }
                // Office keeps all content it does not understand, so these lists of
                // what to keep change nothing; their names are only checked.
                "PreserveElements" | "PreserveAttributes" => {
                    for name in names {
                        qualified(reader, tag, attribute.name().local(), name)?;
                    }
                }
                other => {
                    return Err(reader.error(
                        tag.offset(),
                        format!("unknown markup compatibility attribute {other}"),
                    ));
                }
            }
        }
        let disposition = self.element(tag.name());
        if matches!(
            disposition,
            Disposition::Understood | Disposition::ProcessContent
        ) {
            for namespace in must_understand.unwrap_or_default() {
                if !self.is_understood(namespace) {
                    return Err(Error::Unsupported(format!(
                        "the document requires namespace {} (mc:MustUnderstand), which \
                         officewright does not implement",
                        reader.namespace_uri(namespace)
                    )));
                }
            }
        }
        Ok(disposition)
    }

    /// Leaves the element [`start`](Self::start) was last called for.
    pub fn end(&mut self) {
        let Some(len) = self.frames.pop() else {
            return;
        };
        while self.added.len() > len {
            match self.added.pop() {
                Some(Added::Ignorable(namespace)) => {
                    self.ignorable.remove(&namespace);
                }
                Some(Added::ProcessAll(namespace)) => {
                    self.process_all.remove(&namespace);
                }
                Some(Added::ProcessName(namespace, local)) => {
                    if let Some(names) = self.process_names.get_mut(&namespace) {
                        names.remove(&local);
                    }
                }
                None => break,
            }
        }
    }

    /// What to do with an element, inside the scope of the start tags taken in so far
    /// (its own included).
    fn element(&self, name: Name<'_>) -> Disposition {
        let Some(namespace) = name.namespace() else {
            return Disposition::NotUnderstood;
        };
        if self.is_understood(namespace) {
            Disposition::Understood
        } else if !self.ignorable.contains(&namespace) {
            Disposition::NotUnderstood
        } else if self.process_all.contains(&namespace)
            || self
                .process_names
                .get(&namespace)
                .is_some_and(|names| names.contains(name.local()))
        {
            Disposition::ProcessContent
        } else {
            Disposition::Ignore
        }
    }

    /// What to do with an attribute of the element whose start tag was taken in last.
    /// Attributes without a prefix belong to their element and are understood.
    pub fn attribute(&self, name: Name<'_>) -> Disposition {
        match name.namespace() {
            None => Disposition::Understood,
            Some(namespace) if namespace == NsId::XML || self.is_understood(namespace) => {
                Disposition::Understood
            }
            Some(namespace) if self.ignorable.contains(&namespace) => Disposition::Ignore,
            Some(_) => Disposition::NotUnderstood,
        }
    }

    /// Whether every namespace an `mc:Choice` requires is understood.
    ///
    /// # Errors
    ///
    /// [`Error::Xml`] when `Requires` is missing, empty, or names an undeclared prefix,
    /// and [`Error::LimitExceeded`] when it lists more than
    /// [`Limits::max_mce_names`](crate::Limits::max_mce_names) prefixes.
    pub fn is_selectable(&self, reader: &Reader<'_>, choice: &StartTag<'_>) -> Result<bool> {
        let requires = choice
            .attribute(None, "Requires")
            .filter(|requires| !requires.trim_ascii().is_empty())
            .ok_or_else(|| reader.error(choice.offset(), "mc:Choice lacks Requires"))?;
        let mut selectable = true;
        for prefix in names(reader, choice, requires)? {
            selectable &= self.is_understood(resolve(reader, choice, "Requires", prefix)?);
        }
        Ok(selectable)
    }

    fn is_understood(&self, namespace: NsId) -> bool {
        namespace == self.mc || self.understood.contains(&namespace)
    }
}

fn resolve(reader: &Reader<'_>, tag: &StartTag<'_>, attribute: &str, prefix: &str) -> Result<NsId> {
    declared(reader, prefix).ok_or_else(|| {
        reader.error(
            tag.offset(),
            format!("{attribute} names the undeclared prefix {prefix}"),
        )
    })
}

/// The names of a Markup Compatibility attribute, at most
/// [`Limits::max_mce_names`](crate::Limits::max_mce_names) of them.
fn names<'v>(
    reader: &Reader<'_>,
    tag: &StartTag<'_>,
    value: &'v str,
) -> Result<std::str::SplitAsciiWhitespace<'v>> {
    if value
        .split_ascii_whitespace()
        .nth(reader.max_mce_names())
        .is_some()
    {
        return Err(reader.limit(Limit::MceNames, tag.offset()));
    }
    Ok(value.split_ascii_whitespace())
}

fn declared(reader: &Reader<'_>, prefix: &str) -> Option<NsId> {
    reader.resolve_prefix(prefix).filter(|_| !prefix.is_empty())
}

/// The prefix and local name of a `prefix:local` or `prefix:*` name from
/// `ProcessContent` and the `Preserve` lists.
fn qualified<'v>(
    reader: &Reader<'_>,
    tag: &StartTag<'_>,
    attribute: &str,
    name: &'v str,
) -> Result<(&'v str, &'v str)> {
    name.split_once(':')
        .filter(|(prefix, local)| !prefix.is_empty() && !local.is_empty())
        .ok_or_else(|| {
            reader.error(
                tag.offset(),
                format!("{attribute} lists {name}, which is not a qualified name"),
            )
        })
}

#[cfg(test)]
mod tests;
