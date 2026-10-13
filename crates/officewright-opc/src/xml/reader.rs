use std::borrow::Cow;
use std::collections::HashMap;

use super::chars::{is_char, is_name_char, is_name_start, is_whitespace};
use super::{
    Attribute, EndTag, Event, Name, NamespaceDeclaration, NsId, ProcessingInstruction, QName,
    StartTag,
};
use crate::{Error, Limit, Limits, Result};

/// The namespace the `xml` prefix is bound to in every document.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// The namespace of `xmlns` attributes, which no prefix may be bound to.
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// A pull parser over one XML document, created by [`Input::reader`](super::Input::reader).
///
/// It checks well-formedness (XML 1.0) and namespaces (Namespaces in XML 1.0) as it
/// goes and resolves every element and attribute name to a [`NsId`]. Document type
/// declarations are rejected: ECMA-376 Part 2 does not allow them in a package, and
/// without them no entity other than the five predefined ones can be referenced.
///
/// After an error the reader reports no further events.
#[derive(Debug)]
pub struct Reader<'a> {
    text: &'a str,
    pos: usize,
    event_offset: usize,
    state: State,
    max_depth: usize,
    max_attributes: usize,
    max_mce_names: usize,
    uris: Vec<Box<str>>,
    ids: HashMap<Box<str>, NsId>,
    /// The namespaces each prefix is bound to, innermost last; `""` is the default
    /// namespace, `None` an undeclared default namespace.
    bindings: HashMap<&'a str, Vec<Option<NsId>>>,
    /// The prefixes declared by the open elements, outermost first.
    declared: Vec<&'a str>,
    open: Vec<Open<'a>>,
    /// The last start tag was self-closing, so its end tag comes next.
    pending_end: bool,
}

#[derive(Debug)]
struct Open<'a> {
    name: Name<'a>,
    offset: usize,
    declared_from: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Start,
    Prolog,
    Content,
    Epilog,
    Done,
}

impl<'a> Reader<'a> {
    pub(super) fn new(text: &'a str, limits: &Limits) -> Self {
        let mut reader = Self {
            text,
            pos: 0,
            event_offset: 0,
            state: State::Start,
            max_depth: limits.max_xml_depth(),
            max_attributes: limits.max_xml_attributes(),
            max_mce_names: limits.max_mce_names(),
            uris: Vec::new(),
            ids: HashMap::new(),
            bindings: HashMap::new(),
            declared: Vec::new(),
            open: Vec::new(),
            pending_end: false,
        };
        reader.intern(XML_NAMESPACE);
        reader
    }

    /// The next event, or `None` after the end of the document.
    ///
    /// A self-closing element produces a [`Event::Start`] and an [`Event::End`], like
    /// an element with a start and an end tag.
    ///
    /// # Errors
    ///
    /// [`Error::Xml`] where the document is not well-formed, [`Error::Unsupported`] for
    /// an XML declaration that names an encoding other than UTF-8, UTF-16, US-ASCII or
    /// ASCII (a part labelled ASCII is read as UTF-8, any UTF-8 included), and
    /// [`Error::LimitExceeded`] when an element nests deeper than
    /// [`Limits::max_xml_depth`] or has more attributes than
    /// [`Limits::max_xml_attributes`].
    pub fn read_event(&mut self) -> Result<Option<Event<'a>>> {
        let event = self.next_event();
        if event.is_err() {
            self.state = State::Done;
            self.pending_end = false;
        }
        event
    }

    /// Reads events up to the end tag of the innermost open element, so that the next
    /// event is the one after it. Right after a start tag that is the element of that
    /// tag; after an end tag, its parent. Outside the root element it does nothing.
    ///
    /// # Errors
    ///
    /// The errors of [`read_event`](Self::read_event).
    pub fn skip_element(&mut self) -> Result<()> {
        let depth = self.open.len();
        while self.open.len() >= depth && depth > 0 {
            if self.read_event()?.is_none() {
                break;
            }
        }
        Ok(())
    }

    /// The id of a namespace URI. Interning the namespaces a consumer looks for before
    /// reading lets it compare [`Name::namespace`] with them.
    pub fn intern(&mut self, uri: &str) -> NsId {
        if let Some(&id) = self.ids.get(uri) {
            return id;
        }
        let id = NsId(u32::try_from(self.uris.len()).unwrap_or(u32::MAX));
        self.uris.push(uri.into());
        self.ids.insert(uri.into(), id);
        id
    }

    /// The URI of a namespace id from this reader.
    pub fn namespace_uri(&self, namespace: NsId) -> &str {
        usize::try_from(namespace.0)
            .ok()
            .and_then(|index| self.uris.get(index))
            .map_or("", |uri| uri)
    }

    /// The namespace a prefix is bound to where the reader is: on the element whose
    /// start tag was read last, its own declarations included. `None` for an unbound
    /// prefix; the empty prefix asks for the default namespace.
    pub fn resolve_prefix(&self, prefix: &str) -> Option<NsId> {
        self.lookup(prefix).flatten()
    }

    /// How many elements are open.
    pub fn depth(&self) -> usize {
        self.open.len()
    }

    /// The byte offset where the last event starts.
    pub fn offset(&self) -> usize {
        self.event_offset
    }

    /// An [`Error::Xml`] at a byte offset of this document, for consumers that find a
    /// problem in what the reader returned.
    pub fn error(&self, offset: usize, message: impl Into<String>) -> Error {
        error_at(self.text, offset, message)
    }

    fn next_event(&mut self) -> Result<Option<Event<'a>>> {
        if self.pending_end {
            self.pending_end = false;
            return Ok(Some(Event::End(self.close()?)));
        }
        loop {
            self.event_offset = self.pos;
            match self.state {
                State::Start => {
                    self.state = State::Prolog;
                    if self.rest().starts_with("<?xml")
                        && self.byte(self.pos + 5).is_some_and(is_whitespace)
                    {
                        self.declaration()?;
                    }
                }
                State::Prolog | State::Epilog => {
                    self.skip_whitespace();
                    self.event_offset = self.pos;
                    let rest = self.rest();
                    if rest.is_empty() {
                        if self.state == State::Prolog {
                            return Err(self.error(self.pos, "no root element"));
                        }
                        self.state = State::Done;
                        return Ok(None);
                    }
                    if rest.starts_with("<!--") {
                        return self.comment().map(Some);
                    }
                    if rest.starts_with("<?") {
                        return self.processing_instruction().map(Some);
                    }
                    if rest.starts_with("<!DOCTYPE") {
                        return Err(self.error(
                            self.pos,
                            "document type declarations are not allowed in OOXML parts",
                        ));
                    }
                    if self.state == State::Prolog && rest.starts_with('<') {
                        self.state = State::Content;
                        return self.start_tag().map(Some);
                    }
                    let message = if self.state == State::Prolog {
                        "content before the root element"
                    } else {
                        "content after the root element"
                    };
                    return Err(self.error(self.pos, message));
                }
                State::Content => return self.content().map(Some),
                State::Done => return Ok(None),
            }
        }
    }

    fn content(&mut self) -> Result<Event<'a>> {
        let rest = self.rest();
        if rest.is_empty() {
            let name = self
                .open
                .last()
                .map_or("", |open| open.name.qname().as_str());
            return Err(self.error(
                self.pos,
                format!("unexpected end of input inside <{}>", short(name)),
            ));
        }
        if rest.starts_with("</") {
            self.end_tag()
        } else if rest.starts_with("<!--") {
            self.comment()
        } else if rest.starts_with("<![CDATA[") {
            self.cdata()
        } else if rest.starts_with("<?") {
            self.processing_instruction()
        } else if rest.starts_with("<!") {
            Err(self.error(self.pos, "unexpected markup declaration"))
        } else if rest.starts_with('<') {
            self.start_tag()
        } else {
            self.text()
        }
    }

    fn start_tag(&mut self) -> Result<Event<'a>> {
        let offset = self.pos;
        self.pos += 1;
        let qname = self.qname()?;
        let mut raw: Vec<(QName<'a>, Cow<'a, str>)> = Vec::new();
        let self_closing = loop {
            let spaced = self.skip_whitespace();
            match self.byte(self.pos) {
                Some(b'>') => {
                    self.pos += 1;
                    break false;
                }
                Some(b'/') if self.rest().starts_with("/>") => {
                    self.pos += 2;
                    break true;
                }
                None => {
                    return Err(self.error(
                        self.pos,
                        format!("unexpected end of input in <{}>", short(qname.as_str())),
                    ));
                }
                Some(_) if !spaced => {
                    return Err(self.error(
                        self.pos,
                        format!(
                            "expected whitespace, '>' or '/>' in <{}>",
                            short(qname.as_str())
                        ),
                    ));
                }
                Some(_) => {
                    let name = self.qname()?;
                    self.skip_whitespace();
                    self.expect(b'=', "expected '=' after an attribute name")?;
                    self.skip_whitespace();
                    let value = self.attribute_value()?;
                    if raw.len() == self.max_attributes {
                        return Err(self.limit(Limit::XmlAttributes, offset));
                    }
                    raw.push((name, value));
                }
            }
        };
        if self.open.len() >= self.max_depth {
            return Err(self.limit(Limit::XmlDepth, offset));
        }
        if let Some((name, _)) = duplicate_by(&raw, |(name, _)| name.as_str()) {
            return Err(self.error(
                offset,
                format!(
                    "duplicate attribute {} in <{}>",
                    short(name.as_str()),
                    short(qname.as_str())
                ),
            ));
        }

        let declared_from = self.declared.len();
        let mut namespace_declarations = Vec::new();
        let mut unresolved = Vec::with_capacity(raw.len());
        for (name, value) in raw {
            let prefix = match (name.prefix(), name.local()) {
                (None, "xmlns") => None,
                (Some("xmlns"), prefix) => Some(prefix),
                _ => {
                    unresolved.push((name, value));
                    continue;
                }
            };
            let namespace = self.declare(offset, prefix, &value)?;
            namespace_declarations.push(NamespaceDeclaration { prefix, namespace });
        }
        let name = match qname.prefix() {
            None => Name {
                qname,
                namespace: self.lookup("").flatten(),
            },
            Some(prefix) => Name {
                qname,
                namespace: Some(self.bound(offset, prefix, qname)?),
            },
        };
        let mut attributes = Vec::with_capacity(unresolved.len());
        for (attribute, value) in unresolved {
            let namespace = match attribute.prefix() {
                None => None,
                Some(prefix) => Some(self.bound(offset, prefix, qname)?),
            };
            attributes.push(Attribute {
                name: Name {
                    qname: attribute,
                    namespace,
                },
                value,
            });
        }
        // Two prefixes bound to one namespace can still name the same attribute.
        let expanded =
            |attribute: &Attribute<'a>| (attribute.name.namespace, attribute.name.local());
        if let Some(attribute) = duplicate_by(&attributes, expanded) {
            return Err(self.error(
                offset,
                format!(
                    "duplicate attribute {} in <{}>",
                    short(attribute.name.qname().as_str()),
                    short(qname.as_str())
                ),
            ));
        }
        self.open.push(Open {
            name,
            offset,
            declared_from,
        });
        self.pending_end = self_closing;
        Ok(Event::Start(StartTag {
            name,
            attributes,
            namespace_declarations,
            self_closing,
            offset,
        }))
    }

    fn end_tag(&mut self) -> Result<Event<'a>> {
        let offset = self.pos;
        self.pos += 2;
        let qname = self.qname()?;
        self.skip_whitespace();
        self.expect(b'>', "expected '>' at the end of an end tag")?;
        let open = self
            .open
            .last()
            .map_or("", |open| open.name.qname().as_str());
        if qname.as_str() != open {
            return Err(self.error(
                offset,
                format!(
                    "end tag </{}> does not match start tag <{}>",
                    short(qname.as_str()),
                    short(open)
                ),
            ));
        }
        let mut end = self.close()?;
        end.offset = offset;
        Ok(Event::End(end))
    }

    /// Closes the innermost element and drops the namespaces it declared.
    fn close(&mut self) -> Result<EndTag<'a>> {
        let Some(open) = self.open.pop() else {
            return Err(self.error(self.pos, "end tag without an open element"));
        };
        while self.declared.len() > open.declared_from {
            if let Some(prefix) = self.declared.pop() {
                if let Some(stack) = self.bindings.get_mut(prefix) {
                    stack.pop();
                }
            }
        }
        if self.open.is_empty() {
            self.state = State::Epilog;
        }
        Ok(EndTag {
            name: open.name,
            offset: open.offset,
        })
    }

    fn text(&mut self) -> Result<Event<'a>> {
        let start = self.pos;
        let rest = self.rest();
        let raw = rest
            .find('<')
            .map_or(rest, |end| self.slice(start, start + end));
        if let Some(at) = raw.find("]]>") {
            return Err(self.error(start + at, "']]>' in text"));
        }
        self.pos = start + raw.len();
        self.expand(raw, start, false).map(Event::Text)
    }

    fn cdata(&mut self) -> Result<Event<'a>> {
        let start = self.pos + "<![CDATA[".len();
        let Some(len) = self.text.get(start..).and_then(|rest| rest.find("]]>")) else {
            return Err(self.error(self.pos, "unterminated CDATA section"));
        };
        self.pos = start + len + 3;
        Ok(Event::Text(Cow::Borrowed(self.slice(start, start + len))))
    }

    fn comment(&mut self) -> Result<Event<'a>> {
        let start = self.pos + "<!--".len();
        let rest = self.text.get(start..).unwrap_or_default();
        let Some(len) = rest.find("--") else {
            return Err(self.error(self.pos, "unterminated comment"));
        };
        if self.byte(start + len + 2) != Some(b'>') {
            return Err(self.error(start + len, "'--' in a comment"));
        }
        self.pos = start + len + 3;
        Ok(Event::Comment(self.slice(start, start + len)))
    }

    fn processing_instruction(&mut self) -> Result<Event<'a>> {
        let offset = self.pos;
        self.pos += 2;
        let target_start = self.pos;
        self.ncname()?;
        let target = self.slice(target_start, self.pos);
        if target.eq_ignore_ascii_case("xml") {
            return Err(self.error(
                offset,
                "an xml declaration is allowed only at the start of the document",
            ));
        }
        if self.rest().starts_with("?>") {
            self.pos += 2;
            return Ok(Event::ProcessingInstruction(ProcessingInstruction {
                target,
                data: "",
            }));
        }
        if !self.skip_whitespace() {
            return Err(self.error(
                self.pos,
                "expected whitespace after a processing instruction target",
            ));
        }
        let start = self.pos;
        let Some(len) = self.rest().find("?>") else {
            return Err(self.error(offset, "unterminated processing instruction"));
        };
        self.pos = start + len + 2;
        Ok(Event::ProcessingInstruction(ProcessingInstruction {
            target,
            data: self.slice(start, start + len),
        }))
    }

    /// The XML declaration (production 23), at the very start of the document.
    fn declaration(&mut self) -> Result<()> {
        let offset = self.pos;
        self.pos += "<?xml".len();
        self.skip_whitespace();
        let version = self.pseudo_attribute("version")?;
        let digits = version.strip_prefix("1.").unwrap_or_default();
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(self.error(
                offset,
                format!("unsupported xml version {}", short(version)),
            ));
        }
        let mut spaced = self.skip_whitespace();
        if spaced && self.rest().starts_with("encoding") {
            let encoding = self.pseudo_attribute("encoding")?;
            let valid = encoding
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic())
                && encoding
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
            if !valid {
                return Err(
                    self.error(offset, format!("invalid encoding name {}", short(encoding)))
                );
            }
            // The bytes were decoded before reading, as UTF-8 or UTF-16, and the label
            // is accepted whichever of the two they turned out to be. ECMA-376 Part 2
            // allows only UTF-8 and UTF-16 labels. A part labelled US-ASCII or ASCII is
            // accepted too (#11): ASCII is a subset of UTF-8, and producers outside Office
            // write that label, some over text that is really UTF-8, which is then read
            // as they meant it.
            let known = ["utf-8", "utf-16", "us-ascii", "ascii"];
            if !known
                .iter()
                .any(|label| encoding.eq_ignore_ascii_case(label))
            {
                return Err(Error::Unsupported(format!(
                    "xml encoding {}; OOXML parts are UTF-8 or UTF-16",
                    short(encoding)
                )));
            }
            spaced = self.skip_whitespace();
        }
        if spaced && self.rest().starts_with("standalone") {
            let standalone = self.pseudo_attribute("standalone")?;
            if standalone != "yes" && standalone != "no" {
                return Err(self.error(
                    offset,
                    format!("standalone must be yes or no, not {}", short(standalone)),
                ));
            }
            self.skip_whitespace();
        }
        self.expect(b'?', "malformed xml declaration")?;
        self.expect(b'>', "malformed xml declaration")
    }

    fn pseudo_attribute(&mut self, name: &str) -> Result<&'a str> {
        if !self.rest().starts_with(name) {
            return Err(self.error(self.pos, format!("expected {name} in the xml declaration")));
        }
        self.pos += name.len();
        self.skip_whitespace();
        self.expect(b'=', "expected '=' in the xml declaration")?;
        self.skip_whitespace();
        let quote = match self.byte(self.pos) {
            Some(quote @ (b'"' | b'\'')) => char::from(quote),
            _ => return Err(self.error(self.pos, "expected a quoted value")),
        };
        let start = self.pos + 1;
        let Some(len) = self.text.get(start..).and_then(|rest| rest.find(quote)) else {
            return Err(self.error(self.pos, "unterminated value in the xml declaration"));
        };
        self.pos = start + len + 1;
        Ok(self.slice(start, start + len))
    }

    fn attribute_value(&mut self) -> Result<Cow<'a, str>> {
        let quote = match self.byte(self.pos) {
            Some(quote @ (b'"' | b'\'')) => char::from(quote),
            _ => return Err(self.error(self.pos, "expected a quoted attribute value")),
        };
        let start = self.pos + 1;
        let Some(len) = self.text.get(start..).and_then(|rest| rest.find(quote)) else {
            return Err(self.error(self.pos, "unterminated attribute value"));
        };
        let raw = self.slice(start, start + len);
        if let Some(at) = raw.find('<') {
            return Err(self.error(start + at, "'<' in an attribute value"));
        }
        self.pos = start + len + 1;
        self.expand(raw, start, true)
    }

    /// Replaces references in text or an attribute value; in attribute values also
    /// tabs and line feeds with spaces (XML 1.0, section 3.3.3).
    fn expand(&self, raw: &'a str, start: usize, attribute: bool) -> Result<Cow<'a, str>> {
        let special: &[char] = if attribute {
            &['&', '\t', '\n']
        } else {
            &['&']
        };
        if !raw.contains(special) {
            return Ok(Cow::Borrowed(raw));
        }
        let mut value = String::with_capacity(raw.len());
        let mut done = 0;
        while let Some(found) = raw.get(done..).and_then(|rest| rest.find(special)) {
            let at = done + found;
            value.push_str(raw.get(done..at).unwrap_or_default());
            if raw.as_bytes().get(at) == Some(&b'&') {
                let (c, len) = self.reference(start + at)?;
                value.push(c);
                done = at + len;
            } else {
                value.push(' ');
                done = at + 1;
            }
        }
        value.push_str(raw.get(done..).unwrap_or_default());
        Ok(Cow::Owned(value))
    }

    /// The character a reference at `at` stands for, and the reference's length.
    fn reference(&self, at: usize) -> Result<(char, usize)> {
        let body = self.text.get(at + 1..).unwrap_or_default();
        // Production 66 puts no bound on the digits of a character reference, so neither
        // does this: the scan reads each byte once, and the number is parsed with checked
        // arithmetic.
        let len = body
            .bytes()
            .take_while(|&b| b.is_ascii_alphanumeric() || b == b'#')
            .count();
        let name = body.get(..len).unwrap_or_default();
        if body.as_bytes().get(len) != Some(&b';') {
            return Err(self.error(at, "'&' does not start a reference"));
        }
        let c = match name {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "apos" => '\'',
            "quot" => '"',
            _ => {
                let code = if let Some(hex) = name.strip_prefix("#x") {
                    parse_number(hex, 16)
                } else if let Some(decimal) = name.strip_prefix('#') {
                    parse_number(decimal, 10)
                } else {
                    return Err(self.error(at, format!("undeclared entity &{};", short(name))));
                };
                code.and_then(char::from_u32)
                    .filter(|&c| is_char(c))
                    .ok_or_else(|| {
                        self.error(
                            at,
                            format!("&{}; is not a character XML allows", short(name)),
                        )
                    })?
            }
        };
        Ok((c, len + 2))
    }

    /// A qualified name (Namespaces in XML 1.0, production 7).
    fn qname(&mut self) -> Result<QName<'a>> {
        let start = self.pos;
        self.ncname()?;
        let first = self.pos;
        if self.byte(self.pos) != Some(b':') {
            let raw = self.slice(start, first);
            return Ok(QName {
                raw,
                prefix: None,
                local: raw,
            });
        }
        self.pos += 1;
        self.ncname()?;
        Ok(QName {
            raw: self.slice(start, self.pos),
            prefix: Some(self.slice(start, first)),
            local: self.slice(first + 1, self.pos),
        })
    }

    fn ncname(&mut self) -> Result<()> {
        let rest = self.rest();
        if !rest.chars().next().is_some_and(is_name_start) {
            return Err(self.error(self.pos, "expected a name"));
        }
        self.pos += rest
            .char_indices()
            .find(|&(_, c)| !is_name_char(c))
            .map_or(rest.len(), |(len, _)| len);
        Ok(())
    }

    /// Binds a prefix (or the default namespace, for `None`) declared on the element
    /// at `offset`, checking the constraints of Namespaces in XML 1.0, section 3.
    fn declare(
        &mut self,
        offset: usize,
        prefix: Option<&'a str>,
        uri: &str,
    ) -> Result<Option<NsId>> {
        let reserved = uri == XML_NAMESPACE || uri == XMLNS_NAMESPACE;
        match prefix {
            Some("xmlns") => return Err(self.error(offset, "the prefix xmlns cannot be declared")),
            Some("xml") if uri == XML_NAMESPACE => return Ok(Some(NsId::XML)),
            Some("xml") => {
                return Err(self.error(
                    offset,
                    format!("the prefix xml can only be bound to {XML_NAMESPACE}"),
                ));
            }
            Some(prefix) if uri.is_empty() => {
                return Err(self.error(
                    offset,
                    format!(
                        "the namespace prefix {} cannot be undeclared",
                        short(prefix)
                    ),
                ));
            }
            None if uri.is_empty() => {
                self.bind("", None);
                return Ok(None);
            }
            _ if reserved => {
                return Err(self.error(
                    offset,
                    format!("{} cannot be bound to a prefix", short(uri)),
                ));
            }
            _ => {}
        }
        let namespace = self.intern(uri);
        self.bind(prefix.unwrap_or_default(), Some(namespace));
        Ok(Some(namespace))
    }

    fn bind(&mut self, prefix: &'a str, namespace: Option<NsId>) {
        self.bindings.entry(prefix).or_default().push(namespace);
        self.declared.push(prefix);
    }

    /// `None` for an unbound prefix, `Some(None)` for no default namespace.
    fn lookup(&self, prefix: &str) -> Option<Option<NsId>> {
        if prefix == "xml" {
            return Some(Some(NsId::XML));
        }
        self.bindings
            .get(prefix)
            .and_then(|stack| stack.last().copied())
    }

    fn bound(&self, offset: usize, prefix: &str, element: QName<'_>) -> Result<NsId> {
        self.lookup(prefix).flatten().ok_or_else(|| {
            self.error(
                offset,
                format!(
                    "undeclared namespace prefix {} in <{}>",
                    short(prefix),
                    short(element.as_str())
                ),
            )
        })
    }

    /// [`Limits::max_mce_names`] of the limits this reader was made with.
    pub(crate) fn max_mce_names(&self) -> usize {
        self.max_mce_names
    }

    /// An [`Error::LimitExceeded`] for one of the limits on a part's XML, at a byte
    /// offset of this document.
    pub(crate) fn limit(&self, limit: Limit, offset: usize) -> Error {
        let (line, column) = location(self.text, offset);
        let maximum = match limit {
            Limit::XmlDepth => self.max_depth,
            Limit::MceNames => self.max_mce_names,
            _ => self.max_attributes,
        };
        Error::LimitExceeded {
            limit,
            maximum: maximum as u64,
            context: format!("line {line}, column {column}"),
        }
    }

    fn expect(&mut self, byte: u8, message: &str) -> Result<()> {
        if self.byte(self.pos) != Some(byte) {
            return Err(self.error(self.pos, message));
        }
        self.pos += 1;
        Ok(())
    }

    fn skip_whitespace(&mut self) -> bool {
        let len = self
            .rest()
            .bytes()
            .take_while(|&b| is_whitespace(b))
            .count();
        self.pos += len;
        len > 0
    }

    fn rest(&self) -> &'a str {
        self.text.get(self.pos..).unwrap_or_default()
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        self.text.get(start..end).unwrap_or_default()
    }

    fn byte(&self, at: usize) -> Option<u8> {
        self.text.as_bytes().get(at).copied()
    }
}

/// An [`Error::Xml`] at a byte offset of `text`, with line and column.
pub(super) fn error_at(text: &str, offset: usize, message: impl Into<String>) -> Error {
    let (line, column) = location(text, offset);
    Error::Xml {
        part: None,
        line,
        column,
        message: message.into(),
    }
}

/// At most 64 characters of a name or value from the document, with `…` when there are
/// more, so that an error message names it without copying all of it.
fn short(text: &str) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(64) {
        Some((end, _)) => format!("{}…", text.get(..end).unwrap_or_default()).into(),
        None => text.into(),
    }
}

fn location(text: &str, offset: usize) -> (u64, u64) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    (line as u64, column as u64)
}

fn parse_number(digits: &str, radix: u32) -> Option<u32> {
    if digits.is_empty() {
        return None;
    }
    digits.chars().try_fold(0u32, |value, c| {
        value.checked_mul(radix)?.checked_add(c.to_digit(radix)?)
    })
}

/// The first item whose key equals an earlier one's.
fn duplicate_by<T, K: Ord>(items: &[T], key: impl Fn(&T) -> K) -> Option<&T> {
    // Most elements have a handful of attributes; comparing pairs needs no allocation.
    if items.len() <= 16 {
        return items.iter().enumerate().find_map(|(index, item)| {
            items
                .get(..index)
                .unwrap_or_default()
                .iter()
                .any(|earlier| key(earlier) == key(item))
                .then_some(item)
        });
    }
    let mut sorted: Vec<&T> = items.iter().collect();
    sorted.sort_by_key(|item| key(item));
    sorted.windows(2).find_map(|pair| match pair {
        [a, b] if key(a) == key(b) => Some(*b),
        _ => None,
    })
}
