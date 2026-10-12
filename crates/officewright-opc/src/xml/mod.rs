//! Reading the XML in OOXML parts.
//!
//! [`Input::decode`] turns the bytes of a part into checked text, and its
//! [`reader`](Input::reader) returns a [`Reader`] that produces [`Event`]s with every
//! name resolved to a namespace.
//!
//! The reader implements the XML that ECMA-376 Part 2 allows in a package: XML 1.0 with
//! namespaces, in UTF-8 or UTF-16, without document type declarations. Without them, no
//! entity can be declared, so there is no entity expansion and nothing is ever loaded
//! from outside the document.
//!
//! ```
//! use officewright_opc::Limits;
//! use officewright_opc::xml::{Event, Input};
//!
//! let input = Input::decode(br#"<w:p xmlns:w="urn:example"><w:t>a &amp; b</w:t></w:p>"#)?;
//! let mut reader = input.reader(&Limits::new());
//! let w = reader.intern("urn:example");
//! let mut text = String::new();
//! while let Some(event) = reader.read_event()? {
//!     match event {
//!         Event::Start(tag) => assert_eq!(tag.name().namespace(), Some(w)),
//!         Event::Text(t) => text.push_str(&t),
//!         _ => {}
//!     }
//! }
//! assert_eq!(text, "a & b");
//! # Ok::<(), officewright_opc::Error>(())
//! ```

use std::borrow::Cow;

mod chars;
mod input;
mod reader;
#[cfg(test)]
mod tests;

pub use input::Input;
pub use reader::{Reader, XML_NAMESPACE};

/// A namespace within one [`Reader`]: two names are in the same namespace when their
/// ids are equal. Ids come from [`Reader::intern`] or from the names the reader returns,
/// and mean nothing to another reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NsId(u32);

impl NsId {
    /// [`XML_NAMESPACE`], the namespace of the `xml` prefix (as in `xml:space`).
    pub const XML: Self = Self(0);
}

/// A name as written: an optional prefix and a local part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QName<'a> {
    raw: &'a str,
    prefix: Option<&'a str>,
    local: &'a str,
}

impl<'a> QName<'a> {
    /// The whole name, such as `w:p`.
    pub fn as_str(&self) -> &'a str {
        self.raw
    }

    /// The prefix, such as `w`.
    pub fn prefix(&self) -> Option<&'a str> {
        self.prefix
    }

    /// The local part, such as `p`.
    pub fn local(&self) -> &'a str {
        self.local
    }
}

/// An element or attribute name with its namespace resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name<'a> {
    qname: QName<'a>,
    namespace: Option<NsId>,
}

impl<'a> Name<'a> {
    /// The name as written.
    pub fn qname(&self) -> QName<'a> {
        self.qname
    }

    /// The namespace; `None` for an attribute without a prefix, or an element outside
    /// any default namespace.
    pub fn namespace(&self) -> Option<NsId> {
        self.namespace
    }

    /// The local part.
    pub fn local(&self) -> &'a str {
        self.qname.local
    }

    /// Whether this is the name `local` in `namespace`.
    pub fn is(&self, namespace: NsId, local: &str) -> bool {
        self.namespace == Some(namespace) && self.qname.local == local
    }
}

/// An attribute with its value, references replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute<'a> {
    name: Name<'a>,
    value: Cow<'a, str>,
}

impl<'a> Attribute<'a> {
    /// The attribute name.
    pub fn name(&self) -> Name<'a> {
        self.name
    }

    /// The value, with references replaced and tabs and line feeds turned into spaces.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The value, without copying it when it needed no replacements.
    pub fn into_value(self) -> Cow<'a, str> {
        self.value
    }
}

/// An `xmlns` or `xmlns:prefix` attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamespaceDeclaration<'a> {
    prefix: Option<&'a str>,
    namespace: Option<NsId>,
}

impl<'a> NamespaceDeclaration<'a> {
    /// The declared prefix; `None` for the default namespace.
    pub fn prefix(&self) -> Option<&'a str> {
        self.prefix
    }

    /// The namespace; `None` for `xmlns=""`, which leaves elements without a default
    /// namespace.
    pub fn namespace(&self) -> Option<NsId> {
        self.namespace
    }
}

/// A start tag, or a self-closing tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartTag<'a> {
    name: Name<'a>,
    attributes: Vec<Attribute<'a>>,
    namespace_declarations: Vec<NamespaceDeclaration<'a>>,
    self_closing: bool,
    offset: usize,
}

impl<'a> StartTag<'a> {
    /// The element name.
    pub fn name(&self) -> Name<'a> {
        self.name
    }

    /// The attributes in document order, without namespace declarations.
    pub fn attributes(&self) -> &[Attribute<'a>] {
        &self.attributes
    }

    /// The value of the attribute `local` in `namespace` (`None` for attributes without
    /// a prefix).
    pub fn attribute(&self, namespace: Option<NsId>, local: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| {
                attribute.name.namespace == namespace && attribute.name.local() == local
            })
            .map(Attribute::value)
    }

    /// Takes the attributes, without copying their values.
    pub fn into_attributes(self) -> Vec<Attribute<'a>> {
        self.attributes
    }

    /// The `xmlns` attributes in document order.
    pub fn namespace_declarations(&self) -> &[NamespaceDeclaration<'a>] {
        &self.namespace_declarations
    }

    /// Whether the tag is self-closing (`<a/>`). The reader returns an [`Event::End`]
    /// for it all the same.
    pub fn is_self_closing(&self) -> bool {
        self.self_closing
    }

    /// The byte offset of the tag in the decoded text.
    pub fn offset(&self) -> usize {
        self.offset
    }
}

/// An end tag, or the end of a self-closing tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndTag<'a> {
    name: Name<'a>,
    offset: usize,
}

impl<'a> EndTag<'a> {
    /// The element name.
    pub fn name(&self) -> Name<'a> {
        self.name
    }

    /// The byte offset of the end tag, or of the self-closing tag, in the decoded text.
    pub fn offset(&self) -> usize {
        self.offset
    }
}

/// A processing instruction, `<?target data?>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessingInstruction<'a> {
    target: &'a str,
    data: &'a str,
}

impl<'a> ProcessingInstruction<'a> {
    /// The target.
    pub fn target(&self) -> &'a str {
        self.target
    }

    /// Everything after the target and the whitespace that follows it.
    pub fn data(&self) -> &'a str {
        self.data
    }
}

/// What a [`Reader`] returns.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event<'a> {
    /// A start tag.
    Start(StartTag<'a>),
    /// An end tag.
    End(EndTag<'a>),
    /// Character data inside the root element, references replaced; also the content
    /// of a CDATA section. Whitespace between elements is returned as text too.
    Text(Cow<'a, str>),
    /// A comment, without `<!--` and `-->`.
    Comment(&'a str),
    /// A processing instruction.
    ProcessingInstruction(ProcessingInstruction<'a>),
}
