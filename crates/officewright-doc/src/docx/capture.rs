//! Reading the elements of a part into a document: tokens of elements kept as written,
//! the node limit all parts share, and the positions and diagnostics of a part.

use std::collections::{HashMap, HashSet};

use officewright_opc::mce::{Disposition, Processor};
use officewright_opc::xml::{Event, NsId, Reader, StartTag};

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::model::Document;
use crate::model::xml::{Declaration, Span, Sym, Token, XmlAttribute, XmlName};
use crate::{Error, Limit, Result};

/// The namespace of `xsi:type`, whose value is a qualified name.
const XSI_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// The prefixes the value of an attribute names: those of the Markup Compatibility
/// attributes (ECMA-376 Part 3, §7 and §10.2) and of `xsi:type`; none for any other.
/// `None` stands for the default namespace, which an unprefixed `xsi:type` names (XML
/// Schema resolves a QName without a prefix in it).
fn value_prefixes<'v>(
    reader: &Reader<'_>,
    tag: &StartTag<'_>,
    attribute: &'v officewright_opc::xml::Attribute<'_>,
) -> impl Iterator<Item = Option<&'v str>> + 'v {
    let name = attribute.name();
    let uri = name
        .namespace()
        .map_or("", |namespace| reader.namespace_uri(namespace));
    let tag_uri = tag
        .name()
        .namespace()
        .map_or("", |namespace| reader.namespace_uri(namespace));
    // Lists of prefixes, lists of qualified names, and one qualified name.
    let (prefixes, names, qualified) = match (uri, name.local()) {
        (officewright_opc::mce::NAMESPACE, "Ignorable" | "MustUnderstand") => (true, false, false),
        (
            officewright_opc::mce::NAMESPACE,
            "ProcessContent" | "PreserveElements" | "PreserveAttributes",
        ) => (false, true, false),
        ("", "Requires")
            if tag_uri == officewright_opc::mce::NAMESPACE && tag.name().local() == "Choice" =>
        {
            (true, false, false)
        }
        (XSI_NAMESPACE, "type") => (false, false, true),
        _ => (false, false, false),
    };
    let value: &'v str = attribute.value();
    let tokens = (prefixes || names).then(|| value.split_ascii_whitespace());
    let one = qualified.then(|| value.trim_matches([' ', '\t', '\n', '\r']));
    tokens
        .into_iter()
        .flatten()
        .chain(one)
        .filter_map(move |token| {
            if prefixes {
                Some(Some(token))
            } else {
                match token.split_once(':') {
                    Some((prefix, _)) => Some(Some(prefix)),
                    // A list of qualified names needs prefixes (§10.2); one QName may
                    // have none.
                    None => qualified.then_some(None),
                }
            }
        })
        .filter(|prefix| {
            prefix.is_none_or(|prefix| !prefix.is_empty() && prefix != "xml" && prefix != "xmlns")
        })
}

/// What is left of [`Limits::max_nodes`](crate::Limits::max_nodes) for the parts still
/// to read.
#[derive(Debug)]
pub(crate) struct Budget {
    left: usize,
    max: usize,
}

impl Budget {
    pub(crate) fn new(max: usize) -> Self {
        Self { left: max, max }
    }
}

/// What Markup Compatibility makes of an element [`Capture::capture`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// An `mc:AlternateContent` that is read: its branches are started in `mce`, and
    /// the one it takes is read.
    Alternate,
    /// Read: started in `mce`, and so is every element in it.
    Read,
    /// Started in `mce`, and kept as written inside: an ignorable element, one `mce`
    /// does not understand, a branch not taken, or an element the caller keeps.
    Started,
    /// Kept as written, never started in `mce`.
    Kept,
}

/// The state of reading one part into a document.
pub(super) struct Capture<'d, 'p, 't> {
    pub(super) document: &'d mut Document,
    /// The part's name, which positions and messages name.
    pub(super) part: &'p str,
    pub(super) diagnostics: &'d mut Diagnostics,
    budget: &'d mut Budget,
    uris: HashMap<NsId, Sym>,
    /// The reader's Markup Compatibility namespace, once a capture has asked for it.
    mc: Option<NsId>,
    locator: Locator<'t>,
    /// The offset of the event read last, where a limit is exceeded.
    pub(super) at: usize,
}

impl<'d, 'p, 't> Capture<'d, 'p, 't> {
    pub(super) fn new(
        document: &'d mut Document,
        part: &'p str,
        text: &'t str,
        diagnostics: &'d mut Diagnostics,
        budget: &'d mut Budget,
    ) -> Self {
        Self {
            document,
            part,
            diagnostics,
            budget,
            uris: HashMap::new(),
            mc: None,
            locator: Locator::new(text),
            at: 0,
        }
    }

    /// Reads an element and everything in it into a fragment. The caller started the
    /// element in `mce`; `read` is what that gave when the reader reads the element,
    /// and `None` when it keeps the element as written.
    ///
    /// Everything in an element read goes through Markup Compatibility as the elements
    /// the body walks do (#156, #296): each element in it is started in `mce`, so an
    /// `mc:MustUnderstand` that names a namespace `mce` does not understand refuses the
    /// part; of each `mc:AlternateContent`, the branch `mce` takes is marked, and a
    /// choice whose `Requires` is not valid refuses the part; and an ignorable element
    /// whose content `mc:ProcessContent` names is marked too, so that its children are
    /// read in place of it (see `Token::Start`). What an ignorable element, an element
    /// `mce` does not understand or a branch not taken holds is kept as written.
    ///
    /// In an element kept as written, nothing is started in `mce`. The branch of each
    /// `mc:AlternateContent` that `mce` would take is still marked, while the prefixes
    /// its `Requires` names can be resolved; a choice whose `Requires` is missing or
    /// empty, names an undeclared prefix, or lists more than
    /// [`Limits::max_mce_names`](officewright_opc::Limits::max_mce_names) names is not
    /// taken there: what is inside is not read, so it is not an error.
    ///
    /// When the element read is an `mc:AlternateContent`, or an ignorable element under
    /// `mc:ProcessContent`, the reader of a part reads its children in its place (see
    /// [`through_alternates`](Self::through_alternates)).
    pub(super) fn capture(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
        read: Option<Disposition>,
    ) -> Result<Span> {
        self.spend(1)?;
        let start = self.document.tokens.len();
        let mc = *self
            .mc
            .get_or_insert_with(|| reader.intern(officewright_opc::mce::NAMESPACE));
        // The depth of each open mc:AlternateContent, and whether a branch of it has
        // been taken.
        let mut alternates: Vec<(usize, bool)> = Vec::new();
        // What Markup Compatibility makes of each open element. The caller started the
        // element itself; it is ended after the loop, not at its end tag.
        let alternate = tag.name().is(mc, "AlternateContent");
        if alternate {
            alternates.push((1, false));
        }
        let mut roles = vec![match read {
            Some(Disposition::Understood | Disposition::ProcessContent) if alternate => {
                Role::Alternate
            }
            Some(Disposition::Understood | Disposition::ProcessContent) => Role::Read,
            _ => Role::Started,
        }];
        let part = self.part;
        let opc = |err: officewright_opc::Error| Error::from(err.in_part(part));
        self.push_start(reader, &tag, read == Some(Disposition::ProcessContent))?;
        let mut depth = 1usize;
        while depth > 0 {
            let event = reader
                .read_event()
                .map_err(|err| Error::from(err.in_part(self.part)))?;
            // Where the event just read starts, so that an error names it (#34).
            self.at = reader.offset();
            self.spend(1)?;
            match event {
                Some(Event::Start(tag)) => {
                    let name = tag.name();
                    let parent = roles.last().copied().unwrap_or(Role::Kept);
                    let processed = matches!(parent, Role::Alternate | Role::Read);
                    let disposition = if processed {
                        Some(mce.start(reader, &tag).map_err(opc)?)
                    } else {
                        None
                    };
                    let mut taken = false;
                    if let Some((at, branch_taken)) = alternates.last_mut() {
                        if *at == depth && !*branch_taken {
                            let selectable = || match mce.is_selectable(reader, &tag) {
                                Ok(selectable) => Ok(selectable),
                                Err(err) if processed => Err(opc(err)),
                                Err(_) => Ok(false),
                            };
                            taken =
                                name.is(mc, "Fallback") || (name.is(mc, "Choice") && selectable()?);
                            *branch_taken = taken;
                        }
                    }
                    let content = disposition == Some(Disposition::ProcessContent);
                    roles.push(match parent {
                        Role::Alternate if taken => Role::Read,
                        Role::Alternate => Role::Started,
                        Role::Read if name.is(mc, "AlternateContent") => Role::Alternate,
                        Role::Read => match disposition {
                            Some(Disposition::Understood | Disposition::ProcessContent) => {
                                Role::Read
                            }
                            _ => Role::Started,
                        },
                        Role::Started | Role::Kept => Role::Kept,
                    });
                    depth += 1;
                    if name.is(mc, "AlternateContent") {
                        alternates.push((depth, false));
                    }
                    self.push_start(reader, &tag, taken || (parent == Role::Read && content))?;
                }
                Some(Event::End(_)) => {
                    self.document.tokens.push(Token::End);
                    let role = roles.pop();
                    if !roles.is_empty() && role.is_some_and(|role| role != Role::Kept) {
                        mce.end();
                    }
                    if alternates.last().is_some_and(|(at, _)| *at == depth) {
                        alternates.pop();
                    }
                    depth -= 1;
                }
                Some(Event::Text(text)) => self.document.tokens.push(Token::Text(text.into())),
                Some(Event::Comment(text)) => {
                    self.document.tokens.push(Token::Comment(text.into()))
                }
                Some(Event::ProcessingInstruction(pi)) => {
                    self.document.tokens.push(Token::ProcessingInstruction {
                        target: pi.target().into(),
                        data: pi.data().into(),
                    });
                }
                Some(_) => {}
                None => break,
            }
        }
        mce.end();
        Ok(Span::new(start, self.document.tokens.len()))
    }

    /// Pushes a start tag as a token, its names interned in the document, and its
    /// namespace declarations, if it has any, as a [`Token::Declarations`] after it. Its attributes count
    /// against the node limit, as each costs memory of its own, and so do its namespace
    /// declarations.
    ///
    /// The declarations written on the tag are kept, and so is the binding of each prefix
    /// the value of a Markup Compatibility attribute or of `xsi:type` names that the tag
    /// does not declare itself, as it is in scope where the tag is read: such a prefix
    /// may be declared outside the element kept, and the value means nothing without it.
    /// A prefix is looked at once per tag, however often its values name it.
    pub(super) fn push_start(
        &mut self,
        reader: &Reader<'_>,
        tag: &StartTag<'_>,
        in_place: bool,
    ) -> Result<()> {
        let written = tag.namespace_declarations();
        self.spend(tag.attributes().len() + written.len())?;
        let name = self.name(reader, tag.name());
        let mut declarations = Vec::with_capacity(written.len());
        // The prefixes the tag declares, and those its values named so far.
        let mut seen: HashSet<Option<&str>> = HashSet::with_capacity(written.len());
        for declaration in written {
            seen.insert(declaration.prefix());
            declarations.push(Declaration {
                prefix: declaration
                    .prefix()
                    .map(|prefix| self.document.strings.intern(prefix)),
                uri: declaration
                    .namespace()
                    .map(|namespace| self.uri(reader, namespace)),
                written: true,
            });
        }
        for attribute in tag.attributes() {
            for prefix in value_prefixes(reader, tag, attribute) {
                if !seen.insert(prefix) {
                    continue;
                }
                if let Some(namespace) = reader.resolve_prefix(prefix.unwrap_or_default()) {
                    self.spend(1)?;
                    declarations.push(Declaration {
                        prefix: prefix.map(|prefix| self.document.strings.intern(prefix)),
                        uri: Some(self.uri(reader, namespace)),
                        written: false,
                    });
                }
            }
        }
        let attributes = tag
            .attributes()
            .iter()
            .map(|attribute| XmlAttribute {
                name: self.name(reader, attribute.name()),
                value: attribute.value().into(),
            })
            .collect();
        self.document.tokens.push(Token::Start {
            name,
            attributes,
            in_place,
        });
        if !declarations.is_empty() {
            self.document
                .tokens
                .push(Token::Declarations(declarations.into_boxed_slice()));
        }
        Ok(())
    }

    fn name(&mut self, reader: &Reader<'_>, name: officewright_opc::xml::Name<'_>) -> XmlName {
        XmlName {
            namespace: name.namespace().map(|ns| self.uri(reader, ns)),
            qname: self.document.strings.intern(name.qname().as_str()),
        }
    }

    /// A namespace of the reader as a namespace of the document, which outlives it.
    fn uri(&mut self, reader: &Reader<'_>, namespace: NsId) -> Sym {
        if let Some(&uri) = self.uris.get(&namespace) {
            return uri;
        }
        let uri = self
            .document
            .strings
            .intern(reader.namespace_uri(namespace));
        self.uris.insert(namespace, uri);
        uri
    }

    /// The elements to read in place of the element of `span`, a child of a part's root:
    /// the element itself or, for an `mc:AlternateContent`, the children of the branch
    /// it takes, and for an ignorable element under `mc:ProcessContent`, its children.
    pub(super) fn through_alternates(&self, span: Span) -> Vec<Span> {
        let document = &self.document;
        span.through_alternates(&document.tokens, &document.strings, document.names.mc)
            .collect()
    }

    /// Skips an element the model reads nothing more from.
    pub(super) fn skip(&mut self, reader: &mut Reader<'_>, mce: &mut Processor) -> Result<()> {
        reader
            .skip_element()
            .map_err(|err| Error::from(err.in_part(self.part)))?;
        mce.end();
        Ok(())
    }

    /// Charges `nodes` against [`Limits::max_nodes`](crate::Limits::max_nodes), which
    /// all parts of a document share.
    pub(super) fn spend(&mut self, nodes: usize) -> Result<()> {
        match self.budget.left.checked_sub(nodes) {
            Some(left) => {
                self.budget.left = left;
                Ok(())
            }
            None => Err(Error::LimitExceeded {
                limit: Limit::Nodes,
                maximum: self.budget.max as u64,
                context: self.place(self.at),
            }),
        }
    }

    /// Records a diagnostic at a position in the part, which it names as errors do.
    pub(super) fn diagnose(
        &mut self,
        offset: usize,
        kind: DiagnosticKind,
        message: impl FnOnce() -> String,
    ) {
        if self.diagnostics.is_full() {
            // Counted, not kept: the position is not worth finding.
            self.diagnostics.push(kind, String::new);
            return;
        }
        let place = self.place(offset);
        self.diagnostics
            .push(kind, || format!("{place}: {}", message()));
    }

    /// Runs `read` over the element of `span` and reports the values it finds not valid
    /// as diagnostics at `offset`. Values are collected only while diagnostics have room
    /// for them; the rest are counted, so damage that is not reported costs no memory.
    /// They are reported together when `read` returns, so within one element they come
    /// before the [`notes`](Self::notes) on its structure, whatever the order of both in
    /// the document.
    pub(super) fn check(
        &mut self,
        span: Span,
        offset: usize,
        read: impl FnOnce(crate::model::xml::Element<'_>, super::properties::Report<'_>),
    ) {
        self.check_in(["w:", "w:"], span, offset, read);
    }

    /// As [`check`](Self::check), for a vocabulary whose elements and attributes take
    /// these prefixes in messages, such as `["a:", ""]` for DrawingML's unqualified
    /// attributes.
    pub(super) fn check_in(
        &mut self,
        [element_prefix, attribute_prefix]: [&str; 2],
        span: Span,
        offset: usize,
        read: impl FnOnce(crate::model::xml::Element<'_>, super::properties::Report<'_>),
    ) {
        let room = self.diagnostics.room();
        let mut omitted = 0u64;
        let mut invalid: Vec<(String, String, String)> = Vec::new();
        read(
            self.document.element(span),
            &mut |element, attribute, value| {
                if invalid.len() < room {
                    invalid.push((element.into(), attribute.into(), value.into()));
                } else {
                    omitted += 1;
                }
            },
        );
        self.diagnostics.omit(omitted);
        for (element, attribute, value) in invalid {
            self.diagnose(offset, DiagnosticKind::InvalidValue, || {
                format!(
                    "{element_prefix}{element} {attribute_prefix}{attribute}=\"{}\" is not valid; it is ignored",
                    super::shorten(&value)
                )
            });
        }
    }

    /// A collector of notes on structure (repeated or incomplete elements) for one
    /// element, to give to [`diagnose_notes`](Self::diagnose_notes) after reading it: they
    /// come after the values [`check`](Self::check) reports for the element.
    pub(super) fn notes(&self) -> Notes {
        Notes {
            room: self.diagnostics.room(),
            messages: Vec::new(),
            omitted: 0,
        }
    }

    /// Reports collected notes as diagnostics at `offset`.
    pub(super) fn diagnose_notes(&mut self, offset: usize, notes: Notes) {
        self.diagnostics.omit(notes.omitted);
        for message in notes.messages {
            self.diagnose(offset, DiagnosticKind::UnexpectedContent, || message);
        }
    }

    /// The part, line and column of an offset, as errors name them.
    pub(super) fn place(&mut self, offset: usize) -> String {
        let (line, column) = self.locator.locate(offset);
        format!("{}, line {line}, column {column}", self.part)
    }
}

/// Lines and columns of offsets in a part. Each position is found by reading on from the
/// one asked for before, so that positions asked for in document order, as a reader asks
/// for them, cost one pass over the part together, not one each.
struct Locator<'t> {
    text: &'t str,
    offset: usize,
    line: u64,
    column: u64,
}

impl<'t> Locator<'t> {
    fn new(text: &'t str) -> Self {
        Self {
            text,
            offset: 0,
            line: 1,
            column: 1,
        }
    }

    /// The line and the column, in characters, of a byte offset, both counted from 1.
    fn locate(&mut self, offset: usize) -> (u64, u64) {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        if offset < self.offset {
            *self = Self::new(self.text);
        }
        for c in self
            .text
            .get(self.offset..offset)
            .unwrap_or_default()
            .chars()
        {
            if c == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
        self.offset = offset;
        (self.line, self.column)
    }
}

/// Notes on the structure of an element, such as a child written twice. Messages are
/// built only while diagnostics have room for them; the rest are counted.
pub(super) struct Notes {
    room: usize,
    messages: Vec<String>,
    omitted: u64,
}

impl Notes {
    /// Adds a note.
    pub(super) fn add(&mut self, message: impl FnOnce() -> String) {
        if self.messages.len() < self.room {
            #[cfg(test)]
            NOTES_BUILT.set(NOTES_BUILT.get() + 1);
            self.messages.push(message());
        } else {
            self.omitted += 1;
        }
    }
}

#[cfg(test)]
thread_local! {
    /// The messages of notes built on this thread, for tests of the bound on them.
    pub(crate) static NOTES_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
