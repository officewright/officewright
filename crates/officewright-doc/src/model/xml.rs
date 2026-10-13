//! XML kept without being understood: elements of extensions and of features the model
//! does not represent yet, and property elements until they are typed.
//!
//! The elements of a document are stored as one flat list of tokens in document order,
//! and each kept element is a span of it, so that keeping, dropping and writing them
//! never recurses, however deep they nest. Names and namespaces are interned: an element
//! costs a few machine words, not an allocation per name.

use std::collections::HashMap;
use std::fmt::Write;

/// A string of a document's [`Strings`]: a namespace URI or a qualified name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Sym(u32);

/// The namespace URIs and qualified names of one document, each kept once.
#[derive(Debug, Default)]
pub(crate) struct Strings {
    list: Vec<Box<str>>,
    index: HashMap<Box<str>, Sym>,
}

impl Strings {
    pub(crate) fn intern(&mut self, text: &str) -> Sym {
        if let Some(&sym) = self.index.get(text) {
            return sym;
        }
        let sym = Sym(self.list.len() as u32);
        self.list.push(text.into());
        self.index.insert(text.into(), sym);
        sym
    }

    pub(crate) fn get(&self, sym: Sym) -> &str {
        self.list.get(sym.0 as usize).map_or("", |text| text)
    }
}

/// A name as written, with its prefix, and its namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct XmlName {
    pub(crate) namespace: Option<Sym>,
    /// `prefix:local` or `local`.
    pub(crate) qname: Sym,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XmlAttribute {
    pub(crate) name: XmlName,
    pub(crate) value: Box<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Start {
        name: XmlName,
        attributes: Box<[XmlAttribute]>,
    },
    End,
    Text(Box<str>),
    Comment(Box<str>),
    ProcessingInstruction {
        target: Box<str>,
        data: Box<str>,
    },
}

/// The tokens of one element and everything in it, in a document's token list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    start: u32,
    end: u32,
}

impl Span {
    pub(crate) fn new(start: usize, end: usize) -> Self {
        Self {
            start: start as u32,
            end: end as u32,
        }
    }

    pub(crate) fn of<'d>(self, tokens: &'d [Token], strings: &'d Strings) -> Element<'d> {
        Element {
            tokens: tokens
                .get(self.start as usize..self.end as usize)
                .unwrap_or_default(),
            strings,
        }
    }
}

/// An element: its start tag, what it contains and its end.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Element<'d> {
    tokens: &'d [Token],
    strings: &'d Strings,
}

impl<'d> Element<'d> {
    fn start(self) -> Option<(&'d XmlName, &'d [XmlAttribute])> {
        match self.tokens.first() {
            Some(Token::Start { name, attributes }) => Some((name, attributes)),
            _ => None,
        }
    }

    /// The namespace URI, empty for none.
    pub(crate) fn namespace(self) -> &'d str {
        self.start()
            .and_then(|(name, _)| name.namespace)
            .map_or("", |uri| self.strings.get(uri))
    }

    pub(crate) fn local_name(self) -> &'d str {
        self.start()
            .map_or("", |(name, _)| local(self.strings.get(name.qname)))
    }

    pub(crate) fn is(self, namespaces: &[Option<Sym>], local_name: &str) -> bool {
        self.start().is_some_and(|(name, _)| {
            namespaces.contains(&name.namespace)
                && local(self.strings.get(name.qname)) == local_name
        })
    }

    /// The value of an attribute in one of `namespaces`.
    pub(crate) fn attribute(
        &self,
        namespaces: &[Option<Sym>],
        local_name: &str,
    ) -> Option<&'d str> {
        let (_, attributes) = self.start()?;
        attributes
            .iter()
            .find(|a| {
                namespaces.contains(&a.name.namespace)
                    && local(self.strings.get(a.name.qname)) == local_name
            })
            .map(|a| &*a.value)
    }

    /// The local name, if the element is in one of `namespaces`.
    pub(crate) fn local_in(self, namespaces: &[Option<Sym>]) -> Option<&'d str> {
        let (name, _) = self.start()?;
        namespaces
            .contains(&name.namespace)
            .then(|| local(self.strings.get(name.qname)))
    }

    /// The child elements, with each `mc:AlternateContent` replaced by the children of
    /// its `mc:Fallback`, the branch a consumer takes when no `mc:Choice` is selectable
    /// (ECMA-376 Part 3, §10.2). A choice that requires only namespaces officewright
    /// implements would be selectable, but the prefixes `Requires` names are not kept
    /// with the tokens, so property elements are always read through the fallback.
    pub(crate) fn children_through_fallback(self, mc: Option<Sym>) -> Vec<Element<'d>> {
        let mut out = Vec::new();
        let mut pending: Vec<Element<'d>> = self.children().collect();
        pending.reverse();
        while let Some(child) = pending.pop() {
            if mc.is_some() && child.is(&[mc], "AlternateContent") {
                let fallback = child.children().find(|branch| branch.is(&[mc], "Fallback"));
                if let Some(fallback) = fallback {
                    let mut inner: Vec<Element<'d>> = fallback.children().collect();
                    inner.reverse();
                    pending.extend(inner);
                }
            } else {
                out.push(child);
            }
        }
        out
    }

    /// The child elements, each with everything in it.
    pub(crate) fn children(self) -> impl Iterator<Item = Element<'d>> + 'd {
        let (tokens, strings) = (self.tokens, self.strings);
        let mut depth = 0usize;
        let mut start = None;
        tokens
            .iter()
            .enumerate()
            .filter_map(move |(i, token)| match token {
                Token::Start { .. } => {
                    depth += 1;
                    if depth == 2 {
                        start = Some(i);
                    }
                    None
                }
                Token::End => {
                    depth = depth.saturating_sub(1);
                    if depth == 1 {
                        let tokens = tokens.get(start.take()?..=i)?;
                        return Some(Element { tokens, strings });
                    }
                    None
                }
                Token::Text(_) | Token::Comment(_) | Token::ProcessingInstruction { .. } => None,
            })
    }

    /// The element and everything in it as XML that stands on its own: each element
    /// declares the prefixes it and its attributes use where they are not yet bound to
    /// the same namespace.
    pub(crate) fn to_xml(self) -> String {
        let strings = self.strings;
        let mut out = String::new();
        // The bindings of each prefix in scope, innermost last, and the prefixes each
        // open element bound.
        let mut bindings: HashMap<Option<&str>, Vec<Option<Sym>>> = HashMap::new();
        let mut open: Vec<(&str, Vec<Option<&str>>)> = Vec::new();
        for token in self.tokens {
            match token {
                Token::Start { name, attributes } => {
                    let qname = strings.get(name.qname);
                    out.push('<');
                    out.push_str(qname);
                    let mut added = Vec::new();
                    let names = std::iter::once((name, true))
                        .chain(attributes.iter().map(|attribute| (&attribute.name, false)));
                    for (name, is_element) in names {
                        let prefix = prefix(strings.get(name.qname));
                        // An unprefixed attribute is in no namespace and needs nothing;
                        // xml is bound by definition.
                        if (prefix.is_none() && !is_element) || prefix == Some("xml") {
                            continue;
                        }
                        let bound = bindings
                            .get(&prefix)
                            .and_then(|stack| stack.last().copied());
                        // The default namespace starts out as no namespace.
                        let bound = bound.or(prefix.is_none().then_some(None));
                        if bound == Some(name.namespace) {
                            continue;
                        }
                        match prefix {
                            Some(prefix) => {
                                let _ = write!(out, " xmlns:{prefix}=\"");
                            }
                            None => out.push_str(" xmlns=\""),
                        }
                        escape(
                            &mut out,
                            name.namespace.map_or("", |uri| strings.get(uri)),
                            true,
                        );
                        out.push('"');
                        bindings.entry(prefix).or_default().push(name.namespace);
                        added.push(prefix);
                    }
                    for attribute in attributes.iter() {
                        out.push(' ');
                        out.push_str(strings.get(attribute.name.qname));
                        out.push_str("=\"");
                        escape(&mut out, &attribute.value, true);
                        out.push('"');
                    }
                    out.push('>');
                    open.push((qname, added));
                }
                Token::End => {
                    if let Some((qname, added)) = open.pop() {
                        out.push_str("</");
                        out.push_str(qname);
                        out.push('>');
                        for prefix in added {
                            if let Some(stack) = bindings.get_mut(&prefix) {
                                stack.pop();
                            }
                        }
                    }
                }
                Token::Text(text) => escape(&mut out, text, false),
                Token::Comment(text) => {
                    out.push_str("<!--");
                    out.push_str(text);
                    out.push_str("-->");
                }
                Token::ProcessingInstruction { target, data } => {
                    out.push_str("<?");
                    out.push_str(target);
                    if !data.is_empty() {
                        out.push(' ');
                        out.push_str(data);
                    }
                    out.push_str("?>");
                }
            }
        }
        out
    }
}

fn prefix(qname: &str) -> Option<&str> {
    qname.split_once(':').map(|(prefix, _)| prefix)
}

fn local(qname: &str) -> &str {
    qname.split_once(':').map_or(qname, |(_, local)| local)
}

/// Escapes text for element content, or with `attribute` for an attribute value, where
/// white space other than a space would otherwise be normalised away.
fn escape(out: &mut String, text: &str, attribute: bool) {
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\r' => out.push_str("&#13;"),
            '"' if attribute => out.push_str("&quot;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\n' if attribute => out.push_str("&#10;"),
            c => out.push(c),
        }
    }
}
