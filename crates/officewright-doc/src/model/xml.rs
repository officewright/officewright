//! XML kept without being understood: elements of extensions and of features the model
//! does not represent yet, and property elements until they are typed.
//!
//! The elements of a document are stored as one flat list of tokens in document order,
//! and each kept element is a span of it, so that keeping, dropping and writing them
//! never recurses, however deep they nest. Names and namespaces are interned: an element
//! costs a few machine words, not an allocation per name.

use std::collections::{HashMap, HashSet};
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
    /// The bytes the strings take, each kept twice: in the list and as a key.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        2 * self.list.iter().map(|text| text.len()).sum::<usize>()
    }

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

/// A namespace declaration of an element: written on it, or the binding in scope where
/// it was read of a prefix that one of its values names (see [`Token::Declarations`]).
/// Both are interned, so that a declaration costs a few bytes however long its URI is,
/// and however many elements name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Declaration {
    /// The prefix declared; `None` for the default namespace.
    pub(crate) prefix: Option<Sym>,
    /// The namespace; `None` for `xmlns=""`.
    pub(crate) uri: Option<Sym>,
    /// Written on the element, rather than added for a value.
    pub(crate) written: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Start {
        name: XmlName,
        attributes: Box<[XmlAttribute]>,
        /// Whether the element is the branch of its `mc:AlternateContent` that the reader
        /// of its part takes: the first `mc:Choice` all of whose `Requires` namespaces
        /// the reader implements, else the `mc:Fallback` (ECMA-376 Part 3, §10.2).
        /// Decided as the element is read, while the prefixes `Requires` names are still
        /// in scope; false for every other element.
        taken: bool,
    },
    /// Right after the start tag of an element that has any: the namespace declarations
    /// written on it, and the binding of each prefix that the value of a Markup
    /// Compatibility attribute or of `xsi:type` names and the element does not declare
    /// itself, as it was in scope where the element was read. Such a prefix may be
    /// declared outside the element kept, and the value means nothing without it. A
    /// token of its own, so that the many elements without any cost nothing more.
    Declarations(Box<[Declaration]>),
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

    /// The elements a reader reads in place of the element of this span: the element
    /// itself or, for an `mc:AlternateContent`, the children of the branch it takes,
    /// each read through its own alternates as
    /// [`Element::children_through_alternates`] reads them.
    pub(crate) fn through_alternates<'d>(
        self,
        tokens: &'d [Token],
        strings: &'d Strings,
        mc: Option<Sym>,
    ) -> impl Iterator<Item = Span> + 'd {
        let element = self.of(tokens, strings);
        let alternate = mc.is_some() && element.is(&[mc], "AlternateContent");
        let (whole, inside) = if alternate {
            (None, Some(Alternates::new(element, mc, true)))
        } else {
            (Some(0..element.tokens.len()), None)
        };
        let start = self.start as usize;
        whole
            .into_iter()
            .chain(inside.into_iter().flatten())
            .map(move |range| Span::new(start + range.start, start + range.end))
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
            Some(Token::Start {
                name, attributes, ..
            }) => Some((name, attributes)),
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
    /// the branch the reader of the part takes (ECMA-376 Part 3, §10.2): the first
    /// `mc:Choice` that requires only namespaces it implements, else the `mc:Fallback`,
    /// else nothing. The branch was chosen as the part was read (see `Token::Start`).
    /// One pass over the element's tokens, however deep alternates nest in each other.
    pub(crate) fn children_through_alternates(
        self,
        mc: Option<Sym>,
    ) -> impl Iterator<Item = Element<'d>> + 'd {
        let (tokens, strings) = (self.tokens, self.strings);
        Alternates::new(self, mc, false).filter_map(move |range| {
            Some(Element {
                tokens: tokens.get(range)?,
                strings,
            })
        })
    }

    /// The text directly in the element, not in its children.
    pub(crate) fn text(self) -> String {
        let mut text = String::new();
        let mut depth = 0usize;
        for token in self.tokens.iter().skip(1) {
            match token {
                Token::Start { .. } => depth += 1,
                Token::End => depth = depth.saturating_sub(1),
                Token::Text(part) if depth == 0 => text.push_str(part),
                Token::Text(_)
                | Token::Comment(_)
                | Token::ProcessingInstruction { .. }
                | Token::Declarations(_) => {}
            }
        }
        text
    }

    /// The element and everything in it as XML that stands on its own: each element
    /// keeps the namespace declarations written on it, and the prefixes its name, its
    /// attributes and its values need are declared where they are not yet bound to the
    /// same namespace. A prefix that no declaration inside the element binds is bound
    /// outside it, the same everywhere in it, so it is declared once, on the element
    /// itself: the XML grows with the element, not with how many of its elements need
    /// a long URI.
    pub(crate) fn to_xml(self) -> String {
        let strings = self.strings;
        let uri = |namespace: Option<Sym>| namespace.map(|uri| strings.get(uri));
        let mut out = String::new();
        let outside = self.outside_bindings();
        // The URI each prefix is bound to in scope, innermost last, and the prefixes each
        // open element bound.
        let mut bindings: HashMap<Option<&str>, Vec<Option<&str>>> = HashMap::new();
        let mut open: Vec<(&str, Vec<Option<&str>>)> = Vec::new();
        let declare = |out: &mut String, prefix: Option<&str>, namespace: Option<&str>| {
            match prefix {
                Some(prefix) => {
                    let _ = write!(out, " xmlns:{prefix}=\"");
                }
                None => out.push_str(" xmlns=\""),
            }
            escape(out, namespace.unwrap_or_default(), true);
            out.push('"');
        };
        for (i, token) in self.tokens.iter().enumerate() {
            match token {
                Token::Start {
                    name, attributes, ..
                } => {
                    let declarations = self.declarations_at(i + 1);
                    let qname = strings.get(name.qname);
                    out.push('<');
                    out.push_str(qname);
                    let mut added = Vec::new();
                    for declaration in declarations.iter().filter(|d| d.written) {
                        let prefix = declaration.prefix.map(|prefix| strings.get(prefix));
                        let namespace = uri(declaration.uri);
                        declare(&mut out, prefix, namespace);
                        bindings.entry(prefix).or_default().push(namespace);
                        added.push(prefix);
                    }
                    if open.is_empty() {
                        for &(prefix, namespace) in &outside {
                            declare(&mut out, prefix, namespace);
                            bindings.entry(prefix).or_default().push(namespace);
                            added.push(prefix);
                        }
                    }
                    for (prefix, namespace) in needs(strings, name, attributes, declarations) {
                        let bound = bindings
                            .get(&prefix)
                            .and_then(|stack| stack.last().copied());
                        // The default namespace starts out as no namespace.
                        let bound = bound.or(prefix.is_none().then_some(None));
                        if bound == Some(namespace) {
                            continue;
                        }
                        declare(&mut out, prefix, namespace);
                        bindings.entry(prefix).or_default().push(namespace);
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
                // Written with the start tag before them.
                Token::Declarations(_) => {}
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

/// The prefixes a start tag needs bound, with the namespace each needs: that of its
/// name, those of its prefixed attributes, and those its values name (the declarations
/// added for them). An unprefixed attribute is in no namespace and needs nothing; `xml`
/// is bound by definition.
fn needs<'s>(
    strings: &'s Strings,
    name: &'s XmlName,
    attributes: &'s [XmlAttribute],
    declarations: &'s [Declaration],
) -> impl Iterator<Item = (Option<&'s str>, Option<&'s str>)> + 's {
    let uri = |namespace: Option<Sym>| namespace.map(|uri| strings.get(uri));
    let names = std::iter::once((name, true))
        .chain(attributes.iter().map(|attribute| (&attribute.name, false)))
        .filter_map(move |(name, is_element)| {
            let prefix = prefix(strings.get(name.qname));
            ((prefix.is_some() || is_element) && prefix != Some("xml"))
                .then(|| (prefix, uri(name.namespace)))
        });
    let values = declarations
        .iter()
        .filter(|declaration| !declaration.written)
        .map(move |declaration| {
            (
                declaration.prefix.map(|prefix| strings.get(prefix)),
                uri(declaration.uri),
            )
        });
    names.chain(values)
}

impl<'d> Element<'d> {
    /// The declarations of the element whose start tag is just before `index`: those of
    /// the token at `index`, if it is [`Token::Declarations`].
    fn declarations_at(self, index: usize) -> &'d [Declaration] {
        match self.tokens.get(index) {
            Some(Token::Declarations(declarations)) => declarations,
            _ => &[],
        }
    }

    /// The bindings the element needs from outside it, each once, in the order first
    /// needed: the prefixes its names and values need where no declaration written
    /// inside it binds them. The default namespace is left out when it is no namespace,
    /// as it is where [`to_xml`](Self::to_xml) starts.
    fn outside_bindings(self) -> Vec<(Option<&'d str>, Option<&'d str>)> {
        let strings = self.strings;
        let mut outside: Vec<(Option<&str>, Option<&str>)> = Vec::new();
        let mut seen: HashSet<Option<&str>> = HashSet::new();
        // How many declarations written inside the element bind each prefix in scope, and
        // the prefixes each open element declared.
        let mut inside: HashMap<Option<&str>, usize> = HashMap::new();
        let mut open: Vec<Vec<Option<&str>>> = Vec::new();
        for (i, token) in self.tokens.iter().enumerate() {
            match token {
                Token::Start {
                    name, attributes, ..
                } => {
                    let declarations = self.declarations_at(i + 1);
                    let mut declared = Vec::new();
                    for declaration in declarations.iter().filter(|d| d.written) {
                        let prefix = declaration.prefix.map(|prefix| strings.get(prefix));
                        *inside.entry(prefix).or_default() += 1;
                        declared.push(prefix);
                    }
                    for (prefix, namespace) in needs(strings, name, attributes, declarations) {
                        let bound_inside = inside.get(&prefix).is_some_and(|&count| count > 0);
                        if bound_inside || (prefix.is_none() && namespace.is_none()) {
                            continue;
                        }
                        if seen.insert(prefix) {
                            outside.push((prefix, namespace));
                        }
                    }
                    open.push(declared);
                }
                Token::End => {
                    for prefix in open.pop().unwrap_or_default() {
                        if let Some(count) = inside.get_mut(&prefix) {
                            *count = count.saturating_sub(1);
                        }
                    }
                }
                Token::Text(_)
                | Token::Comment(_)
                | Token::ProcessingInstruction { .. }
                | Token::Declarations(_) => {}
            }
        }
        outside
    }
}

/// The elements read in place of the children of an element, as ranges of its tokens;
/// see [`Element::children_through_alternates`]. Each token is looked at once, and the
/// start tag of an element it returns or passes over twice: such an element is passed
/// over by finding its end tag, an alternate or a taken branch is entered.
struct Alternates<'d> {
    tokens: &'d [Token],
    strings: &'d Strings,
    mc: Option<Sym>,
    /// The token to look at next.
    next: usize,
    /// The element's end tag, where the walk stops.
    end: usize,
    /// For each `mc:AlternateContent` and each taken branch open at `next`, outermost
    /// first: whether it is an `mc:AlternateContent`, whose children are branches.
    open: Vec<bool>,
}

impl<'d> Alternates<'d> {
    /// The walk over the children of `element`, or with `inside` over the children of
    /// the branch `element`, an `mc:AlternateContent`, takes.
    fn new(element: Element<'d>, mc: Option<Sym>, inside: bool) -> Self {
        Self {
            tokens: element.tokens,
            strings: element.strings,
            mc,
            next: 1,
            end: element.tokens.len().saturating_sub(1),
            open: if inside { vec![true] } else { Vec::new() },
        }
    }

    /// The index of the end tag of the element whose start tag is at `start`.
    fn end_of(&self, start: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (i, token) in self.tokens.iter().enumerate().skip(start) {
            #[cfg(test)]
            STEPS.set(STEPS.get() + 1);
            match token {
                Token::Start { .. } => depth += 1,
                Token::End => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i);
                    }
                }
                Token::Text(_)
                | Token::Comment(_)
                | Token::ProcessingInstruction { .. }
                | Token::Declarations(_) => {}
            }
        }
        None
    }
}

impl Iterator for Alternates<'_> {
    type Item = std::ops::Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next < self.end {
            let i = self.next;
            #[cfg(test)]
            STEPS.set(STEPS.get() + 1);
            match self.tokens.get(i)? {
                Token::Start { name, taken, .. } => {
                    if self.open.last() == Some(&true) {
                        // A branch, or anything else an mc:AlternateContent holds.
                        if *taken {
                            self.open.push(false);
                            self.next = i + 1;
                        } else {
                            self.next = self.end_of(i)? + 1;
                        }
                    } else if self.mc.is_some()
                        && name.namespace == self.mc
                        && local(self.strings.get(name.qname)) == "AlternateContent"
                    {
                        self.open.push(true);
                        self.next = i + 1;
                    } else {
                        let end = self.end_of(i)?;
                        self.next = end + 1;
                        return Some(i..end + 1);
                    }
                }
                Token::End => {
                    self.open.pop();
                    self.next = i + 1;
                }
                Token::Text(_)
                | Token::Comment(_)
                | Token::ProcessingInstruction { .. }
                | Token::Declarations(_) => {
                    self.next = i + 1;
                }
            }
        }
        None
    }
}

#[cfg(test)]
thread_local! {
    /// The tokens [`Alternates`] has looked at on this thread, for tests of its cost.
    pub(crate) static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
