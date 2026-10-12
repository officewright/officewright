//! The main document part (`w:document`, ECMA-376 Part 1, §17.2): the body, read into
//! the model.
//!
//! The part is read with an explicit stack of frames rather than recursion, so that
//! tables nested as deep as the XML allows never decide how deep the call stack grows.
//! Every element goes through the Markup Compatibility processor: elements of
//! WordprocessingML are read into the model, one branch of each `mc:AlternateContent`
//! is taken, and everything else is kept in its place without being understood.

use std::collections::HashMap;

use officewright_opc::Part;
use officewright_opc::mce::{Disposition, Processor};
use officewright_opc::xml::{Event, Input, NsId, Reader, StartTag};

use super::{Names, R, R_STRICT, W, W_STRICT, section, shorten};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::model::xml::{Span, Sym, Token, XmlAttribute, XmlName};
use crate::model::{
    Break, BreakClear, BreakKind, Conformance, Document, FieldCharKind, Kind, Node, RunItem,
    SectionData, SectionProperties,
};
use crate::{Error, Limit, Limits, Result};

/// Where the children of an element go, and what they may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    /// Paragraphs.
    Block,
    /// Runs and other content of a paragraph.
    Inline,
    /// The content of a run.
    Run,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parent {
    Body,
    Node(u32),
}

#[derive(Debug)]
enum Frame {
    /// The root element, `w:document`.
    Root,
    /// An element whose children are read at `level` into `parent`.
    Container { parent: Parent, level: Level },
    /// An `mc:AlternateContent`, and whether one of its branches was taken.
    Alternate { taken: bool },
    /// An element whose children belong to its parent: the branch of an
    /// `mc:AlternateContent` that was taken, or an element to process the content of.
    Transparent,
    /// A text element of a run (`w:t`, `w:instrText`, …).
    Text {
        run: u32,
        kind: TextKind,
        text: String,
    },
}

#[derive(Clone, Copy, Debug)]
enum TextKind {
    Text,
    Deleted,
    FieldCode,
    DeletedFieldCode,
}

struct Reading<'d, 'p> {
    document: &'d mut Document,
    part_name: &'p str,
    names: Names,
    /// The reader's ids of the namespaces read into the model.
    w: [NsId; 2],
    mc: NsId,
    uris: HashMap<NsId, Sym>,
    body: Vec<u32>,
    /// Where sections end: after this many blocks of the body, with these properties.
    breaks: Vec<(usize, SectionProperties)>,
    last_section: Option<SectionProperties>,
    diagnostics: Diagnostics,
    nodes_left: usize,
    max_nodes: usize,
    stack: Vec<Frame>,
}

pub(super) fn read(
    document: &mut Document,
    xml: &[u8],
    part: &Part<'_>,
    limits: &Limits,
) -> Result<()> {
    let part_name = part.name().as_str();
    let input = Input::decode(xml).map_err(|err| Error::from(err.in_part(part_name)))?;
    let mut reader = input.reader(limits.package());
    let mut mce = Processor::new(&mut reader);
    let w = [reader.intern(W), reader.intern(W_STRICT)];
    let r = [reader.intern(R), reader.intern(R_STRICT)];
    for namespace in w.iter().chain(&r) {
        mce.understand(*namespace);
    }
    let mc = reader.intern(officewright_opc::mce::NAMESPACE);
    let names = Names::new(&mut document.strings);
    let mut reading = Reading {
        document,
        part_name,
        names,
        w,
        mc,
        uris: HashMap::new(),
        body: Vec::new(),
        breaks: Vec::new(),
        last_section: None,
        diagnostics: Diagnostics::default(),
        nodes_left: limits.max_nodes(),
        max_nodes: limits.max_nodes(),
        stack: Vec::new(),
    };
    reading
        .run(&mut reader, &mut mce)
        .map_err(|err| match err {
            Error::Malformed { message } if !message.starts_with(part_name) => Error::Malformed {
                message: format!("{part_name}, {message}"),
            },
            other => other,
        })?;
    reading.finish();
    Ok(())
}

impl Reading<'_, '_> {
    fn run(&mut self, reader: &mut Reader<'_>, mce: &mut Processor) -> Result<()> {
        let opc = |err: officewright_opc::Error, part: &str| Error::from(err.in_part(part));
        while let Some(event) = reader
            .read_event()
            .map_err(|err| opc(err, self.part_name))?
        {
            match event {
                Event::Start(tag) => {
                    let disposition = mce
                        .start(reader, &tag)
                        .map_err(|err| opc(err, self.part_name))?;
                    self.start(reader, mce, tag, disposition)?;
                }
                Event::End(_) => {
                    self.end();
                    mce.end();
                }
                Event::Text(text) => match self.stack.last_mut() {
                    Some(Frame::Text { text: buffer, .. }) => buffer.push_str(&text),
                    _ if text.trim().is_empty() => {}
                    _ => {
                        let offset = reader.offset();
                        self.diagnose(reader, offset, DiagnosticKind::UnexpectedContent, || {
                            format!("text {:?} outside any run is ignored", shorten(text.trim()))
                        });
                    }
                },
                Event::Comment(_) | Event::ProcessingInstruction(_) => {}
                _ => {}
            }
        }
        Ok(())
    }

    fn start(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
        disposition: Disposition,
    ) -> Result<()> {
        let name = tag.name();
        if self.stack.is_empty() {
            let conformance = if name.is(self.w[0], "document") {
                Conformance::Transitional
            } else if name.is(self.w[1], "document") {
                Conformance::Strict
            } else {
                return Err(self.malformed(
                    reader,
                    &tag,
                    format!(
                        "the root element is <{}>, not w:document",
                        shorten(name.qname().as_str())
                    ),
                ));
            };
            self.document.conformance = conformance;
            self.stack.push(Frame::Root);
            return Ok(());
        }
        if let Some(Frame::Alternate { taken }) = self.stack.last_mut() {
            let choice = name.is(self.mc, "Choice");
            let take = !*taken
                && (name.is(self.mc, "Fallback")
                    || (choice
                        && mce
                            .is_selectable(reader, &tag)
                            .map_err(|err| Error::from(err.in_part(self.part_name)))?));
            if take {
                *taken = true;
                self.stack.push(Frame::Transparent);
            } else {
                return self.skip(reader, mce);
            }
            return Ok(());
        }
        if name.is(self.mc, "AlternateContent") {
            self.stack.push(Frame::Alternate { taken: false });
            return Ok(());
        }
        match disposition {
            Disposition::ProcessContent => {
                self.stack.push(Frame::Transparent);
                return Ok(());
            }
            Disposition::Understood if self.is_w(&tag) => {}
            _ => return self.keep(reader, mce, tag),
        }
        let local = name.local();
        let frame = self.context();
        match frame {
            Context::Root => {
                if local == "body" {
                    self.stack.push(Frame::Container {
                        parent: Parent::Body,
                        level: Level::Block,
                    });
                    Ok(())
                } else {
                    // w:background and the like: kept for writing in a later version.
                    self.capture(reader, mce, tag).map(drop)
                }
            }
            Context::Text => {
                let offset = tag.offset();
                self.diagnose(reader, offset, DiagnosticKind::UnexpectedContent, || {
                    format!(
                        "element <{}> inside text is ignored",
                        shorten(name.qname().as_str())
                    )
                });
                self.skip(reader, mce)
            }
            Context::Container { parent, level } => self.child(reader, mce, tag, parent, level),
        }
    }

    /// An element of WordprocessingML inside a container.
    fn child(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
        parent: Parent,
        level: Level,
    ) -> Result<()> {
        let local = tag.name().local();
        let parent_kind = match parent {
            Parent::Node(node) => self.document.node(node).map(|node| &node.kind),
            Parent::Body => None,
        };
        // Properties of the parent element come first in it.
        let property = matches!(
            (parent_kind, local),
            (Some(Kind::Paragraph), "pPr") | (Some(Kind::Run { .. }), "rPr")
        );
        if property {
            if let Parent::Node(node) = parent {
                return self.property(reader, mce, tag, node);
            }
        }
        match (level, local) {
            (Level::Block, "p") => self.open(parent, Kind::Paragraph, Level::Inline),
            (Level::Block, "sectPr") if parent == Parent::Body => {
                let span = self.capture(reader, mce, tag)?;
                let element = span.of(&self.document.tokens, &self.document.strings);
                let properties =
                    section::read(element, &self.names, self.part_name, &mut self.diagnostics);
                self.last_section = Some(properties);
                Ok(())
            }
            (Level::Inline, "r") => self.open(
                parent,
                Kind::Run {
                    content: Vec::new(),
                },
                Level::Run,
            ),
            (Level::Run, _) => match parent {
                Parent::Node(run) => self.run_content(reader, mce, tag, run),
                Parent::Body => self.keep(reader, mce, tag),
            },
            _ => self.keep(reader, mce, tag),
        }
    }

    /// An element of WordprocessingML inside a run.
    fn run_content(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
        run: u32,
    ) -> Result<()> {
        let text = |kind| Frame::Text {
            run,
            kind,
            text: String::new(),
        };
        let item = match tag.name().local() {
            "t" => return self.push_frame(text(TextKind::Text)),
            "delText" => return self.push_frame(text(TextKind::Deleted)),
            "instrText" => return self.push_frame(text(TextKind::FieldCode)),
            "delInstrText" => return self.push_frame(text(TextKind::DeletedFieldCode)),
            "tab" => RunItem::Tab,
            "cr" => RunItem::CarriageReturn,
            "noBreakHyphen" => RunItem::NoBreakHyphen,
            "softHyphen" => RunItem::SoftHyphen,
            "lastRenderedPageBreak" => RunItem::LastRenderedPageBreak,
            "br" => {
                let kind = match self.w_attribute(&tag, "type") {
                    None | Some("textWrapping") => BreakKind::Line,
                    Some("page") => BreakKind::Page,
                    Some("column") => BreakKind::Column,
                    Some(other) => {
                        self.invalid(reader, &tag, "type", other);
                        BreakKind::Line
                    }
                };
                let clear = match self.w_attribute(&tag, "clear") {
                    None | Some("none") => BreakClear::None,
                    Some("left") => BreakClear::Left,
                    Some("right") => BreakClear::Right,
                    Some("all") => BreakClear::All,
                    Some(other) => {
                        self.invalid(reader, &tag, "clear", other);
                        BreakClear::None
                    }
                };
                RunItem::Break(Break::new(kind, clear))
            }
            "fldChar" => match self.w_attribute(&tag, "fldCharType") {
                Some("begin") => RunItem::FieldChar(FieldCharKind::Begin),
                Some("separate") => RunItem::FieldChar(FieldCharKind::Separate),
                Some("end") => RunItem::FieldChar(FieldCharKind::End),
                other => {
                    self.invalid(reader, &tag, "fldCharType", other.unwrap_or_default());
                    return self.keep(reader, mce, tag);
                }
            },
            "sym" => {
                let char = self
                    .w_attribute(&tag, "char")
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .and_then(char::from_u32);
                let Some(char) = char else {
                    let value = self
                        .w_attribute(&tag, "char")
                        .unwrap_or_default()
                        .to_owned();
                    self.invalid(reader, &tag, "char", &value);
                    return self.keep(reader, mce, tag);
                };
                let font = self.w_attribute(&tag, "font").map(Into::into);
                RunItem::Symbol { font, char }
            }
            local @ ("footnoteReference" | "endnoteReference" | "commentReference") => {
                let Some(id) = self.w_attribute(&tag, "id").and_then(|id| id.parse().ok()) else {
                    let value = self.w_attribute(&tag, "id").unwrap_or_default().to_owned();
                    self.invalid(reader, &tag, "id", &value);
                    return self.keep(reader, mce, tag);
                };
                match local {
                    "footnoteReference" => RunItem::FootnoteReference(id),
                    "endnoteReference" => RunItem::EndnoteReference(id),
                    _ => RunItem::CommentReference(id),
                }
            }
            _ => return self.keep(reader, mce, tag),
        };
        self.push_item(run, item)?;
        // The element is read; whatever it contains is not part of the model.
        self.skip(reader, mce)
    }

    fn end(&mut self) {
        if let Some(Frame::Text { run, kind, text }) = self.stack.pop() {
            let text = text.into_boxed_str();
            let item = match kind {
                TextKind::Text => RunItem::Text(text),
                TextKind::Deleted => RunItem::DeletedText(text),
                TextKind::FieldCode => RunItem::FieldCode(text),
                TextKind::DeletedFieldCode => RunItem::DeletedFieldCode(text),
            };
            // The text was counted against the node limit when its frame opened.
            if let Some(Node {
                kind: Kind::Run { content },
                ..
            }) = self.document.nodes.get_mut(run as usize)
            {
                content.push(item);
            }
        }
    }

    /// Ends reading: splits the body into sections.
    fn finish(mut self) {
        let mut sections = Vec::with_capacity(self.breaks.len() + 1);
        let mut start = 0;
        for (end, properties) in std::mem::take(&mut self.breaks) {
            let blocks = self.body.get(start..end).unwrap_or_default().to_vec();
            sections.push(SectionData { properties, blocks });
            start = end;
        }
        sections.push(SectionData {
            properties: self.last_section.take().unwrap_or_default(),
            blocks: self.body.get(start..).unwrap_or_default().to_vec(),
        });
        self.document.sections = sections;
        self.document.diagnostics = self.diagnostics.finish();
    }

    /// The innermost frame that says where children go; transparent frames and the
    /// branches of `mc:AlternateContent` hand their children to it.
    fn context(&self) -> Context {
        for frame in self.stack.iter().rev() {
            return match frame {
                Frame::Root => Context::Root,
                Frame::Container { parent, level } => Context::Container {
                    parent: *parent,
                    level: *level,
                },
                Frame::Text { .. } => Context::Text,
                Frame::Alternate { .. } | Frame::Transparent => continue,
            };
        }
        Context::Root
    }

    /// Adds a node to `parent` and reads its children at `level`.
    fn open(&mut self, parent: Parent, kind: Kind, level: Level) -> Result<()> {
        let node = self.add(parent, kind)?;
        self.stack.push(Frame::Container {
            parent: Parent::Node(node),
            level,
        });
        Ok(())
    }

    fn add(&mut self, parent: Parent, kind: Kind) -> Result<u32> {
        self.spend(1)?;
        let index = self.document.nodes.len() as u32;
        self.document.nodes.push(Node {
            kind,
            properties: Vec::new(),
            children: Vec::new(),
        });
        match parent {
            Parent::Body => self.body.push(index),
            Parent::Node(parent) => {
                if let Some(parent) = self.document.nodes.get_mut(parent as usize) {
                    parent.children.push(index);
                }
            }
        }
        Ok(index)
    }

    fn push_item(&mut self, run: u32, item: RunItem) -> Result<()> {
        self.spend(1)?;
        if let Some(Node {
            kind: Kind::Run { content },
            ..
        }) = self.document.nodes.get_mut(run as usize)
        {
            content.push(item);
        }
        Ok(())
    }

    fn push_frame(&mut self, frame: Frame) -> Result<()> {
        self.spend(1)?;
        self.stack.push(frame);
        Ok(())
    }

    /// Reads a property element of `node` and keeps it with the node.
    fn property(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
        node: u32,
    ) -> Result<()> {
        let span = self.capture(reader, mce, tag)?;
        if let Some(Kind::Paragraph) = self.document.node(node).map(|node| &node.kind) {
            self.section_break(span);
        }
        if let Some(node) = self.document.nodes.get_mut(node as usize) {
            node.properties.push(span);
        }
        Ok(())
    }

    /// A `w:sectPr` in the properties of a paragraph of the body ends a section after
    /// that paragraph. (Tables are kept without being read, so a paragraph in a table
    /// cell, where Word does not use `w:sectPr`, never gets here.)
    fn section_break(&mut self, p_pr: Span) {
        let p_pr = p_pr.of(&self.document.tokens, &self.document.strings);
        let Some(sect_pr) = p_pr.children().find(|c| c.is(&self.names.w, "sectPr")) else {
            return;
        };
        let properties = section::read(sect_pr, &self.names, self.part_name, &mut self.diagnostics);
        self.breaks.push((self.body.len(), properties));
    }

    /// Keeps an element without understanding it, where it is.
    fn keep(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
    ) -> Result<()> {
        let span = self.capture(reader, mce, tag)?;
        match self.context() {
            Context::Container {
                parent: Parent::Node(run),
                level: Level::Run,
            } => self.push_item(run, RunItem::Other(span)),
            Context::Container { parent, .. } => self.add(parent, Kind::Other(span)).map(drop),
            Context::Root | Context::Text => Ok(()),
        }
    }

    /// Reads an element and everything in it into a fragment. Only the element itself
    /// went through Markup Compatibility: what is inside is kept as written.
    fn capture(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
    ) -> Result<Span> {
        self.spend(1)?;
        let start = self.document.tokens.len();
        let token = self.start_token(reader, &tag)?;
        self.document.tokens.push(token);
        let mut depth = 1usize;
        while depth > 0 {
            self.spend(1)?;
            let event = reader
                .read_event()
                .map_err(|err| Error::from(err.in_part(self.part_name)))?;
            match event {
                Some(Event::Start(tag)) => {
                    let token = self.start_token(reader, &tag)?;
                    self.document.tokens.push(token);
                    depth += 1;
                }
                Some(Event::End(_)) => {
                    self.document.tokens.push(Token::End);
                    depth -= 1;
                }
                Some(Event::Text(text)) => self.document.tokens.push(Token::Text(text.into())),
                Some(_) => {}
                None => break,
            }
        }
        mce.end();
        Ok(Span::new(start, self.document.tokens.len()))
    }

    /// A start tag as a token, its names interned in the document. Its attributes count
    /// against the node limit, as each costs memory of its own.
    fn start_token(&mut self, reader: &Reader<'_>, tag: &StartTag<'_>) -> Result<Token> {
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
        Ok(Token::Start { name, attributes })
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

    /// Skips an element the model reads nothing more from.
    fn skip(&mut self, reader: &mut Reader<'_>, mce: &mut Processor) -> Result<()> {
        reader
            .skip_element()
            .map_err(|err| Error::from(err.in_part(self.part_name)))?;
        mce.end();
        Ok(())
    }

    fn spend(&mut self, nodes: usize) -> Result<()> {
        match self.nodes_left.checked_sub(nodes) {
            Some(left) => {
                self.nodes_left = left;
                Ok(())
            }
            None => Err(Error::LimitExceeded {
                limit: Limit::Nodes,
                maximum: self.max_nodes as u64,
                context: self.part_name.to_owned(),
            }),
        }
    }

    fn is_w(&self, tag: &StartTag<'_>) -> bool {
        tag.name()
            .namespace()
            .is_some_and(|ns| self.w.contains(&ns))
    }

    fn w_attribute<'t>(&self, tag: &'t StartTag<'_>, local: &str) -> Option<&'t str> {
        self.w.iter().find_map(|&w| tag.attribute(Some(w), local))
    }

    fn invalid(&mut self, reader: &Reader<'_>, tag: &StartTag<'_>, attribute: &str, value: &str) {
        let element = tag.name().local();
        self.diagnose(reader, tag.offset(), DiagnosticKind::InvalidValue, || {
            format!(
                "w:{element} w:{attribute}=\"{}\" is not valid; it is ignored",
                shorten(value)
            )
        });
    }

    /// Records a diagnostic at a position in the part, which it names as errors do.
    fn diagnose(
        &mut self,
        reader: &Reader<'_>,
        offset: usize,
        kind: DiagnosticKind,
        message: impl FnOnce() -> String,
    ) {
        let part = self.part_name;
        self.diagnostics.push(kind, || {
            reader.error(offset, message()).in_part(part).to_string()
        });
    }

    fn malformed(&self, reader: &Reader<'_>, tag: &StartTag<'_>, message: String) -> Error {
        Error::from(reader.error(tag.offset(), message).in_part(self.part_name))
    }
}

/// Where the children of the innermost element go.
#[derive(Clone, Copy, Debug)]
enum Context {
    Root,
    Container { parent: Parent, level: Level },
    Text,
}
