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
    Break, BreakClear, BreakKind, Conformance, ControlLevel, Document, FieldCharKind,
    HyperlinkData, Kind, Node, RevisionData, RevisionKind, RunItem, SectionData, SectionProperties,
};
use crate::units::twips_measure;
use crate::{Error, Limit, Limits, Result};

/// Where the children of an element go, and what they may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    /// Paragraphs, tables and block-level content controls.
    Block,
    /// Runs, hyperlinks and other content of a paragraph.
    Inline,
    /// The content of a run.
    Run,
    /// The rows of a table.
    Row,
    /// The cells of a row.
    Cell,
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
    /// A content control (`w:sdt`): its properties, then its content at `level`.
    ContentControl { node: u32, level: Level },
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

struct Reading<'d, 'p, 't> {
    document: &'d mut Document,
    part: &'p Part<'p>,
    part_name: &'p str,
    names: Names,
    /// The reader's ids of the namespaces read into the model.
    w: [NsId; 2],
    r: [NsId; 2],
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
    locator: Locator<'t>,
    /// The offset of the event read last, where a limit is exceeded.
    at: usize,
    /// How many tables are open around the element read now.
    tables_open: usize,
    /// A paragraph whose properties end a section, with the section's properties: the
    /// section ends when the paragraph does.
    pending_break: Option<(u32, SectionProperties)>,
    /// Content controls and wrappers continued in the next section, by the parent they
    /// were added to, dropped again if nothing more goes into them.
    continued: Vec<(u32, Parent)>,
    /// The external target of each hyperlink relationship read so far, interned once
    /// however many links name it.
    link_targets: HashMap<Box<str>, Option<Sym>>,
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
        part,
        part_name,
        names,
        w,
        r,
        mc,
        uris: HashMap::new(),
        body: Vec::new(),
        breaks: Vec::new(),
        last_section: None,
        diagnostics: Diagnostics::default(),
        nodes_left: limits.max_nodes(),
        max_nodes: limits.max_nodes(),
        stack: Vec::new(),
        locator: Locator::new(input.as_str()),
        at: 0,
        tables_open: 0,
        pending_break: None,
        continued: Vec::new(),
        link_targets: HashMap::new(),
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

impl Reading<'_, '_, '_> {
    fn run(&mut self, reader: &mut Reader<'_>, mce: &mut Processor) -> Result<()> {
        let opc = |err: officewright_opc::Error, part: &str| Error::from(err.in_part(part));
        while let Some(event) = reader
            .read_event()
            .map_err(|err| opc(err, self.part_name))?
        {
            match event {
                Event::Start(tag) => {
                    self.at = tag.offset();
                    let disposition = mce
                        .start(reader, &tag)
                        .map_err(|err| opc(err, self.part_name))?;
                    self.start(reader, mce, tag, disposition)?;
                }
                Event::End(_) => {
                    self.end()?;
                    mce.end();
                }
                Event::Text(text) => match self.stack.last_mut() {
                    Some(Frame::Text { text: buffer, .. }) => buffer.push_str(&text),
                    _ if text.trim().is_empty() => {}
                    _ => {
                        let offset = reader.offset();
                        self.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
                            format!(
                                "text {:?} outside a text element is ignored",
                                shorten(text.trim())
                            )
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
                self.diagnose(offset, DiagnosticKind::UnexpectedContent, || {
                    format!(
                        "element <{}> inside text is ignored",
                        shorten(name.qname().as_str())
                    )
                });
                self.skip(reader, mce)
            }
            Context::ContentControl { node, level } => match local {
                "sdtPr" | "sdtEndPr" => self.property(reader, mce, tag, node),
                "sdtContent" => {
                    self.stack.push(Frame::Container {
                        parent: Parent::Node(node),
                        level,
                    });
                    Ok(())
                }
                _ => self.keep(reader, mce, tag),
            },
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
            (Some(Kind::Paragraph), "pPr")
                | (Some(Kind::Run { .. }), "rPr")
                | (Some(Kind::Table { .. }), "tblPr" | "tblGrid")
                | (Some(Kind::Row), "trPr" | "tblPrEx")
                | (Some(Kind::Cell), "tcPr")
                | (Some(Kind::Wrapper), "customXmlPr" | "smartTagPr")
        );
        if property {
            if let Parent::Node(node) = parent {
                return self.property(reader, mce, tag, node);
            }
        }
        match (level, local) {
            (Level::Block, "p") => self.open(parent, Kind::Paragraph, Level::Inline),
            // Block-level w:ins, w:del, w:moveFrom and w:moveTo (around paragraphs or
            // rows) and a w:tbl written inside a paragraph are kept without being read.
            (Level::Block, "tbl") => self.open(
                parent,
                Kind::Table {
                    columns: Vec::new(),
                },
                Level::Row,
            ),
            (Level::Block, "sectPr") if parent == Parent::Body => {
                let place = self.place(tag.offset());
                let span = self.capture(reader, mce, tag)?;
                let element = span.of(&self.document.tokens, &self.document.strings);
                let properties = section::read(element, &self.names, &place, &mut self.diagnostics);
                self.last_section = Some(properties);
                Ok(())
            }
            (Level::Row, "tr") => self.open(parent, Kind::Row, Level::Cell),
            (Level::Cell, "tc") => self.open(parent, Kind::Cell, Level::Block),
            (Level::Block | Level::Inline | Level::Row | Level::Cell, "sdt") => {
                let control = match level {
                    Level::Block => ControlLevel::Block,
                    Level::Inline => ControlLevel::Inline,
                    Level::Row => ControlLevel::Row,
                    _ => ControlLevel::Cell,
                };
                let node = self.add(parent, Kind::ContentControl { level: control })?;
                self.stack.push(Frame::ContentControl { node, level });
                Ok(())
            }
            (Level::Block | Level::Row | Level::Cell, "customXml")
            | (Level::Inline, "customXml" | "smartTag" | "dir" | "bdo") => {
                self.open(parent, Kind::Wrapper, level)
            }
            (Level::Inline, "r") => self.open(
                parent,
                Kind::Run {
                    content: Vec::new(),
                },
                Level::Run,
            ),
            (Level::Inline, "hyperlink") => {
                let data = self.hyperlink(&tag);
                self.open(parent, Kind::Hyperlink(Box::new(data)), Level::Inline)
            }
            (Level::Inline, "ins" | "del" | "moveFrom" | "moveTo") => {
                let kind = match local {
                    "ins" => RevisionKind::Insertion,
                    "del" => RevisionKind::Deletion,
                    "moveFrom" => RevisionKind::MoveFrom,
                    _ => RevisionKind::MoveTo,
                };
                let data = RevisionData {
                    kind,
                    author: self.w_attribute(&tag, "author").map(Into::into),
                    date: self.w_attribute(&tag, "date").map(Into::into),
                };
                self.open(parent, Kind::Revision(Box::new(data)), Level::Inline)
            }
            (Level::Inline, "fldSimple") => {
                if self.w_attribute(&tag, "instr").is_none() {
                    self.diagnose(tag.offset(), DiagnosticKind::InvalidValue, || {
                        "w:fldSimple has no w:instr; its instruction is empty".into()
                    });
                }
                let instruction = self.w_attribute(&tag, "instr").unwrap_or_default().into();
                self.open(parent, Kind::SimpleField { instruction }, Level::Inline)
            }
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
                        self.invalid(&tag, "type", other);
                        BreakKind::Line
                    }
                };
                let clear = match self.w_attribute(&tag, "clear") {
                    None | Some("none") => BreakClear::None,
                    Some("left") => BreakClear::Left,
                    Some("right") => BreakClear::Right,
                    Some("all") => BreakClear::All,
                    Some(other) => {
                        self.invalid(&tag, "clear", other);
                        BreakClear::None
                    }
                };
                RunItem::Break(Break::new(kind, clear))
            }
            "fldChar" => {
                let kind = match self.w_attribute(&tag, "fldCharType") {
                    Some("begin") => FieldCharKind::Begin,
                    Some("separate") => FieldCharKind::Separate,
                    Some("end") => FieldCharKind::End,
                    other => {
                        self.invalid(&tag, "fldCharType", other.unwrap_or_default());
                        return self.keep(reader, mce, tag);
                    }
                };
                // The element is kept whole: it may hold form field data (w:ffData).
                let element = self.capture(reader, mce, tag)?;
                return self.push_item(run, RunItem::FieldChar { kind, element });
            }
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
                    self.invalid(&tag, "char", &value);
                    return self.keep(reader, mce, tag);
                };
                let font = self.w_attribute(&tag, "font").map(Into::into);
                RunItem::Symbol { font, char }
            }
            local @ ("footnoteReference" | "endnoteReference" | "commentReference") => {
                let Some(id) = self.w_attribute(&tag, "id").and_then(|id| id.parse().ok()) else {
                    let value = self.w_attribute(&tag, "id").unwrap_or_default().to_owned();
                    self.invalid(&tag, "id", &value);
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

    fn end(&mut self) -> Result<()> {
        match self.stack.pop() {
            Some(Frame::Text { run, kind, text }) => {
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
            Some(Frame::Container {
                parent: Parent::Node(node),
                level,
            }) => {
                let kind = self.document.node(node).map(|node| &node.kind);
                let (table, wrapper) = (
                    matches!(kind, Some(Kind::Table { .. })),
                    matches!(kind, Some(Kind::Wrapper)),
                );
                if level == Level::Row && table {
                    self.tables_open = self.tables_open.saturating_sub(1);
                }
                if matches!(&self.pending_break, Some((paragraph, _)) if *paragraph == node) {
                    if let Some((_, properties)) = self.pending_break.take() {
                        self.break_section(properties)?;
                    }
                }
                if wrapper {
                    self.drop_if_empty(node);
                }
            }
            Some(Frame::ContentControl { node, .. }) => self.drop_if_empty(node),
            _ => {}
        }
        Ok(())
    }

    /// Ends a section after the paragraph just read. Word ties a section to the
    /// paragraph that holds its `w:sectPr` (§17.6.17), even inside a content control,
    /// which marks a range of the document and has no layout of its own; such a control
    /// (or custom XML element) is continued by a copy with the same properties in the
    /// next section, so that a section stays a list of blocks.
    /// `tdf170602_exportContext.docx` of LibreOffice's tests has a cover page in a
    /// content control whose last paragraph ends the first section.
    fn break_section(&mut self, properties: SectionProperties) -> Result<()> {
        self.breaks.push((self.body.len(), properties));
        let mut parent = Parent::Body;
        let mut copies: Vec<(u32, u32)> = Vec::new();
        for i in 0..self.stack.len() {
            let original = match self.stack.get(i) {
                Some(Frame::ContentControl {
                    node,
                    level: Level::Block,
                }) => *node,
                Some(Frame::Container {
                    parent: Parent::Node(node),
                    level: Level::Block,
                }) => {
                    if let Some(&(_, copy)) = copies.iter().find(|(original, _)| original == node) {
                        if let Some(Frame::Container { parent, .. }) = self.stack.get_mut(i) {
                            *parent = Parent::Node(copy);
                        }
                        continue;
                    }
                    *node
                }
                _ => continue,
            };
            let Some(source) = self.document.node(original) else {
                continue;
            };
            let kind = match source.kind {
                Kind::ContentControl { level } => Kind::ContentControl { level },
                Kind::Wrapper => Kind::Wrapper,
                _ => continue,
            };
            let properties = source.properties.clone();
            let copy = self.add(parent, kind)?;
            if let Some(node) = self.document.nodes.get_mut(copy as usize) {
                node.properties = properties;
            }
            self.continued.push((copy, parent));
            copies.push((original, copy));
            match self.stack.get_mut(i) {
                Some(Frame::ContentControl { node, .. }) => *node = copy,
                Some(Frame::Container { parent, .. }) => *parent = Parent::Node(copy),
                _ => {}
            }
            parent = Parent::Node(copy);
        }
        Ok(())
    }

    /// Takes a continued content control or wrapper out again when nothing went into it.
    fn drop_if_empty(&mut self, node: u32) {
        let Some(at) = self.continued.iter().position(|&(copy, _)| copy == node) else {
            return;
        };
        let (_, parent) = self.continued.swap_remove(at);
        if self
            .document
            .node(node)
            .is_some_and(|node| !node.children.is_empty())
        {
            return;
        }
        let siblings = match parent {
            Parent::Body => &mut self.body,
            Parent::Node(parent) => match self.document.nodes.get_mut(parent as usize) {
                Some(parent) => &mut parent.children,
                None => return,
            },
        };
        if siblings.last() == Some(&node) {
            siblings.pop();
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
                Frame::ContentControl { node, level } => Context::ContentControl {
                    node: *node,
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
        if matches!(kind, Kind::Table { .. }) {
            self.tables_open += 1;
        }
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
        let offset = tag.offset();
        let span = self.capture(reader, mce, tag)?;
        let kind = self.document.node(node).map(|node| &node.kind);
        let (paragraph, table) = (
            matches!(kind, Some(Kind::Paragraph)),
            matches!(kind, Some(Kind::Table { .. })),
        );
        if paragraph {
            self.section_break(node, span, offset);
        }
        if table && self.document.element(span).is(&self.names.w, "tblGrid") {
            let columns = self.grid(span, offset);
            if let Some(Node {
                kind: Kind::Table { columns: table },
                ..
            }) = self.document.nodes.get_mut(node as usize)
            {
                *table = columns;
            }
        }
        if let Some(node) = self.document.nodes.get_mut(node as usize) {
            node.properties.push(span);
        }
        Ok(())
    }

    /// A `w:sectPr` in the properties of a paragraph ends a section after the paragraph
    /// (see [`break_section`](Self::break_section)). Word never writes one in a table cell,
    /// where its section break command splits the table instead; one found there ends no
    /// section until a rule document shows what Word makes of it.
    fn section_break(&mut self, paragraph: u32, p_pr: Span, offset: usize) {
        if self.tables_open > 0 {
            return;
        }
        let place = self.place(offset);
        let p_pr = p_pr.of(&self.document.tokens, &self.document.strings);
        let Some(sect_pr) = p_pr.children().find(|c| c.is(&self.names.w, "sectPr")) else {
            return;
        };
        let properties = section::read(sect_pr, &self.names, &place, &mut self.diagnostics);
        self.pending_break = Some((paragraph, properties));
    }

    /// The widths of the columns of a `w:tblGrid`.
    fn grid(&mut self, tbl_grid: Span, offset: usize) -> Vec<Option<officewright_core::Pt>> {
        let w = self.names.w;
        let place = self.place(offset);
        let mut columns = Vec::new();
        for col in tbl_grid
            .of(&self.document.tokens, &self.document.strings)
            .children()
        {
            if !col.is(&w, "gridCol") {
                continue;
            }
            let width = col.attribute(&w, "w");
            let length = width.and_then(|width| twips_measure(width, false));
            if let (Some(width), None) = (width, length) {
                section::invalid(&mut self.diagnostics, &place, col, "w", width);
            }
            columns.push(length);
        }
        columns
    }

    /// A hyperlink's anchor and external target. The `w:tooltip`, `w:tgtFrame` and
    /// `w:docLocation` attributes are not read yet.
    fn hyperlink(&mut self, tag: &StartTag<'_>) -> HyperlinkData {
        let id = self.r.iter().find_map(|&r| tag.attribute(Some(r), "id"));
        HyperlinkData {
            target: id.and_then(|id| self.link_target(tag, id)),
            anchor: self.w_attribute(tag, "anchor").map(Into::into),
        }
    }

    /// The external target of a relationship of the part. A target is copied once,
    /// however many links name it: it comes from another part, so its length is not
    /// bounded by the links.
    fn link_target(&mut self, tag: &StartTag<'_>, id: &str) -> Option<Sym> {
        if let Some(&target) = self.link_targets.get(id) {
            return target;
        }
        let Some(relationship) = self.part.relationships().get(id) else {
            self.diagnose(tag.offset(), DiagnosticKind::InvalidValue, || {
                format!(
                    "hyperlink r:id=\"{}\" names no relationship; the link is ignored",
                    shorten(id)
                )
            });
            return None;
        };
        let target = (relationship.target_mode() == officewright_opc::TargetMode::External)
            .then(|| self.document.strings.intern(relationship.target()));
        self.link_targets.insert(id.into(), target);
        target
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
            Context::ContentControl { node, .. } => {
                self.add(Parent::Node(node), Kind::Other(span)).map(drop)
            }
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
            self.at = reader.offset();
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
                context: self.place(self.at),
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

    fn invalid(&mut self, tag: &StartTag<'_>, attribute: &str, value: &str) {
        let element = tag.name().local();
        self.diagnose(tag.offset(), DiagnosticKind::InvalidValue, || {
            format!(
                "w:{element} w:{attribute}=\"{}\" is not valid; it is ignored",
                shorten(value)
            )
        });
    }

    /// Records a diagnostic at a position in the part, which it names as errors do.
    fn diagnose(&mut self, offset: usize, kind: DiagnosticKind, message: impl FnOnce() -> String) {
        if self.diagnostics.is_full() {
            // Counted, not kept: the position is not worth finding.
            self.diagnostics.push(kind, String::new);
            return;
        }
        let place = self.place(offset);
        self.diagnostics
            .push(kind, || format!("{place}: {}", message()));
    }

    /// The part, line and column of an offset, as errors name them.
    fn place(&mut self, offset: usize) -> String {
        let (line, column) = self.locator.locate(offset);
        format!("{}, line {line}, column {column}", self.part_name)
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
    ContentControl { node: u32, level: Level },
    Text,
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
