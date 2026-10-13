//! Reading the elements of a part into a document: tokens of elements kept as written,
//! the node limit all parts share, and the positions and diagnostics of a part.

use std::collections::HashMap;

use officewright_opc::mce::Processor;
use officewright_opc::xml::{Event, NsId, Reader, StartTag};

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::model::Document;
use crate::model::xml::{Span, Sym, Token, XmlAttribute, XmlName};
use crate::{Error, Limit, Result};

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

    /// Reads an element and everything in it into a fragment. Only the element itself
    /// went through Markup Compatibility: what is inside is kept as written, except
    /// that the branch of each `mc:AlternateContent` that `mce` would take is marked,
    /// while the prefixes its `Requires` names can still be resolved (see
    /// `Token::Start`). A choice whose `Requires` is missing or names an undeclared
    /// prefix is not taken: what is inside is not read yet, so it is not an error.
    pub(super) fn capture(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
    ) -> Result<Span> {
        self.spend(1)?;
        let start = self.document.tokens.len();
        let mc = *self
            .mc
            .get_or_insert_with(|| reader.intern(officewright_opc::mce::NAMESPACE));
        // The depth of each open mc:AlternateContent, and whether a branch of it has
        // been taken.
        let mut alternates: Vec<(usize, bool)> = Vec::new();
        if tag.name().is(mc, "AlternateContent") {
            alternates.push((1, false));
        }
        let token = self.start_token(reader, &tag, false)?;
        self.document.tokens.push(token);
        let mut depth = 1usize;
        while depth > 0 {
            self.spend(1)?;
            self.at = reader.offset();
            let event = reader
                .read_event()
                .map_err(|err| Error::from(err.in_part(self.part)))?;
            match event {
                Some(Event::Start(tag)) => {
                    let name = tag.name();
                    let mut taken = false;
                    if let Some((at, branch_taken)) = alternates.last_mut() {
                        if *at == depth && !*branch_taken {
                            taken = name.is(mc, "Fallback")
                                || (name.is(mc, "Choice")
                                    && mce.is_selectable(reader, &tag).unwrap_or(false));
                            *branch_taken = taken;
                        }
                    }
                    depth += 1;
                    if name.is(mc, "AlternateContent") {
                        alternates.push((depth, false));
                    }
                    let token = self.start_token(reader, &tag, taken)?;
                    self.document.tokens.push(token);
                }
                Some(Event::End(_)) => {
                    self.document.tokens.push(Token::End);
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

    /// A start tag as a token, its names interned in the document. Its attributes count
    /// against the node limit, as each costs memory of its own.
    pub(super) fn start_token(
        &mut self,
        reader: &Reader<'_>,
        tag: &StartTag<'_>,
        taken: bool,
    ) -> Result<Token> {
        self.spend(tag.attributes().len())?;
        let name = self.name(reader, tag.name());
        let attributes = tag
            .attributes()
            .iter()
            .map(|attribute| XmlAttribute {
                name: self.name(reader, attribute.name()),
                value: attribute.value().into(),
            })
            .collect();
        Ok(Token::Start {
            name,
            attributes,
            taken,
        })
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
    /// it takes.
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
    /// element, to give to [`diagnose_notes`](Self::diagnose_notes) after reading it.
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
            self.messages.push(message());
        } else {
            self.omitted += 1;
        }
    }
}
