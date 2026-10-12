//! Markup Compatibility (ECMA-376 Part 3): what a reader does with elements and
//! attributes from namespaces it does not implement.
//!
//! A [`Processor`] follows the compatibility attributes as a part is read and tells the
//! consumer, for each element and attribute, whether it is [understood](Disposition),
//! declared ignorable, or neither:
//!
//! - **Understood**: from a namespace the consumer implements, or the
//!   Markup Compatibility, `xml` and no namespace.
//! - **Ignore**: from a namespace declared in `mc:Ignorable`. The consumer does not
//!   interpret it, but keeps it to write it back unchanged.
//! - **Process content**: an ignorable element listed in `mc:ProcessContent`. Its
//!   children belong to its parent.
//! - **Not understood**: from a namespace that is neither. Part 3 asks consumers to
//!   reject such content; officewright keeps it as unknown content instead, the way
//!   Office keeps content it does not know.
//!
//! In `mc:AlternateContent`, the consumer reads the first `mc:Choice` that
//! [`is_selectable`](Processor::is_selectable), or the `mc:Fallback` if none is.
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
//! mce.understand(w);
//! let mut choices = Vec::new();
//! while let Some(event) = reader.read_event()? {
//!     match event {
//!         Event::Start(tag) => {
//!             mce.start(&reader, &tag)?;
//!             if tag.name().is(w, "p") {
//!                 let para_id = tag.attributes()[0].name();
//!                 assert_eq!(mce.attribute(para_id), Disposition::Ignore);
//!             }
//!             if tag.name().local() == "Choice" {
//!                 choices.push(mce.is_selectable(&reader, &tag)?);
//!             }
//!         }
//!         Event::End(_) => mce.end(),
//!         _ => {}
//!     }
//! }
//! // w14 is not understood, so the fallback is read.
//! assert_eq!(choices, [false]);
//! # Ok::<(), officewright_opc::Error>(())
//! ```

use crate::xml::{Name, NsId, Reader, StartTag};
use crate::{Error, Result};

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
#[derive(Debug)]
pub struct Processor {
    mc: NsId,
    understood: Vec<NsId>,
    /// Ignorable namespaces declared by the open elements, outermost first.
    ignorable: Vec<NsId>,
    /// Elements whose content is processed: a namespace and a local name, or `None` for
    /// every element of the namespace.
    process_content: Vec<(NsId, Option<Box<str>>)>,
    /// The lengths of `ignorable` and `process_content` before each open element.
    frames: Vec<(usize, usize)>,
}

impl Processor {
    /// A processor for the part `reader` reads, understanding no namespace yet.
    pub fn new(reader: &mut Reader<'_>) -> Self {
        Self {
            mc: reader.intern(NAMESPACE),
            understood: Vec::new(),
            ignorable: Vec::new(),
            process_content: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// Declares that the consumer implements a namespace.
    pub fn understand(&mut self, namespace: NsId) {
        if !self.understood.contains(&namespace) {
            self.understood.push(namespace);
        }
    }

    /// Takes in the compatibility attributes of a start tag the reader just returned.
    /// Call [`end`](Self::end) at its end tag.
    ///
    /// # Errors
    ///
    /// [`Error::Xml`] for an attribute that names an undeclared prefix or is not a
    /// Markup Compatibility attribute, and [`Error::Unsupported`] when
    /// `mc:MustUnderstand` names a namespace the consumer does not understand.
    pub fn start(&mut self, reader: &Reader<'_>, tag: &StartTag<'_>) -> Result<()> {
        self.frames
            .push((self.ignorable.len(), self.process_content.len()));
        for attribute in tag.attributes() {
            if attribute.name().namespace() != Some(self.mc) {
                continue;
            }
            let value = attribute.value();
            match attribute.name().local() {
                // A prefix that is not declared cannot name any element or attribute, so
                // the lists of ignorable and preserved names skip it rather than reject
                // the part. LibreOffice's sw/qa/extras/layout/data/
                // floattable-center-shift-down.docx lists an undeclared w14.
                "Ignorable" => {
                    let declared = value
                        .split_ascii_whitespace()
                        .filter_map(|prefix| declared(reader, prefix));
                    self.ignorable.extend(declared);
                }
                "MustUnderstand" => {
                    for prefix in value.split_ascii_whitespace() {
                        let namespace = resolve(reader, tag, "MustUnderstand", prefix)?;
                        if !self.is_understood(namespace) {
                            return Err(Error::Unsupported(format!(
                                "the document requires namespace {} (mc:MustUnderstand), \
                                 which officewright does not implement",
                                reader.namespace_uri(namespace)
                            )));
                        }
                    }
                }
                "ProcessContent" => {
                    for name in value.split_ascii_whitespace() {
                        let (prefix, local) = qualified(reader, tag, "ProcessContent", name)?;
                        if let Some(namespace) = declared(reader, prefix) {
                            let local = (local != "*").then(|| local.into());
                            self.process_content.push((namespace, local));
                        }
                    }
                }
                // Office keeps all content it does not understand, so these lists of
                // what to keep change nothing; their names are only checked.
                "PreserveElements" | "PreserveAttributes" => {
                    for name in value.split_ascii_whitespace() {
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
        Ok(())
    }

    /// Leaves the element [`start`](Self::start) was last called for.
    pub fn end(&mut self) {
        if let Some((ignorable, process_content)) = self.frames.pop() {
            self.ignorable.truncate(ignorable);
            self.process_content.truncate(process_content);
        }
    }

    /// What to do with an element, inside the scope of the start tags taken in so far
    /// (its own included).
    pub fn element(&self, name: Name<'_>) -> Disposition {
        let Some(namespace) = name.namespace() else {
            return Disposition::NotUnderstood;
        };
        if self.is_understood(namespace) {
            Disposition::Understood
        } else if !self.ignorable.contains(&namespace) {
            Disposition::NotUnderstood
        } else if self.process_content.iter().any(|(ns, local)| {
            *ns == namespace && local.as_deref().is_none_or(|local| local == name.local())
        }) {
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
    /// [`Error::Xml`] when `Requires` is missing, empty, or names an undeclared prefix.
    pub fn is_selectable(&self, reader: &Reader<'_>, choice: &StartTag<'_>) -> Result<bool> {
        let requires = choice
            .attribute(None, "Requires")
            .filter(|requires| !requires.trim_ascii().is_empty())
            .ok_or_else(|| reader.error(choice.offset(), "mc:Choice lacks Requires"))?;
        let mut selectable = true;
        for prefix in requires.split_ascii_whitespace() {
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
