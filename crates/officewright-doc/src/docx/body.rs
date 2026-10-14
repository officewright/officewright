//! The main document part (`w:document`, ECMA-376 Part 1, §17.2), its headers and
//! footers (`w:hdr`, `w:ftr`, §17.10) and its notes (`w:footnotes`, `w:endnotes`, §17.11):
//! the body and the other stories, read into the model.
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

use super::capture::{Budget, Capture};
use super::styles::{Properties, check_properties};
use super::{R, R_STRICT, W, W_STRICT, section, shorten};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::model::Names;
use crate::model::drawing;
use crate::model::xml::{Span, Sym};
use crate::model::{
    Break, BreakClear, BreakKind, Conformance, ControlLevel, Document, FieldCharKind,
    HyperlinkData, ImageData, Kind, Node, NoteType, RevisionData, RevisionKind, RunItem,
    SectionData, SectionProperties, StoryData, StoryKind,
};
use crate::units::twips_measure;
use crate::{Error, Limits, Result};

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
    /// The story's own blocks: the body's, or a header's, footer's or note's.
    Body,
    Node(u32),
}

/// What the root element of a part holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Story {
    /// `w:document`: the body, split into sections.
    Document,
    /// `w:hdr`: a header.
    Header,
    /// `w:ftr`: a footer.
    Footer,
    /// `w:footnotes`: footnotes, each a story of its own.
    Footnotes,
    /// `w:endnotes`: endnotes, each a story of its own.
    Endnotes,
}

impl Story {
    /// The local name of the root element.
    fn root(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Header => "hdr",
            Self::Footer => "ftr",
            Self::Footnotes => "footnotes",
            Self::Endnotes => "endnotes",
        }
    }
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

/// A block-level content control or custom XML element open in the body.
#[derive(Clone, Copy, Debug)]
struct OpenBlock {
    node: u32,
    /// The frame that closes it: the content control's, or the custom XML element's.
    frame: usize,
    /// The frame its blocks are read in, once open (`w:sdtContent`).
    content: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
enum TextKind {
    Text,
    Deleted,
    FieldCode,
    DeletedFieldCode,
}

struct Reading<'d, 'p, 't> {
    c: Capture<'d, 'p, 't>,
    part: &'p Part<'p>,
    story: Story,
    /// The note being read: its frame, id and type.
    note: Option<(usize, i64, NoteType)>,
    names: Names,
    /// The reader's ids of the namespaces read into the model.
    w: [NsId; 2],
    r: [NsId; 2],
    mc: NsId,
    body: Vec<u32>,
    /// Where sections end: after this many blocks of the body, with these properties.
    breaks: Vec<(usize, SectionProperties)>,
    last_section: Option<SectionProperties>,
    stack: Vec<Frame>,
    /// For each frame, the index of the frame that says where its children go: its own,
    /// or for a transparent frame that of the frame it hands its children to.
    defining: Vec<usize>,
    /// How many tables are open around the element read now.
    tables_open: usize,
    /// A paragraph whose properties end a section, with the section's properties: the
    /// section ends when the paragraph does.
    pending_break: Option<(u32, SectionProperties)>,
    /// The block-level content controls and custom XML elements open in the body, outermost
    /// first: what a section break inside them continues in the next section.
    open_blocks: Vec<OpenBlock>,
    /// The copies made for the next section that are still open, with the parent each was
    /// added to: a copy is dropped again if nothing more goes into it.
    continued: HashMap<u32, Parent>,
    /// The external target of each hyperlink relationship read so far, interned once
    /// however many links name it.
    link_targets: HashMap<Box<str>, Option<Sym>>,
    /// The image each relationship a picture named so far leads to.
    images: HashMap<Box<str>, Option<u32>>,
}

pub(super) fn read(
    document: &mut Document,
    xml: &[u8],
    part: &Part<'_>,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
    budget: &mut Budget,
    story: Story,
) -> Result<()> {
    document.xml_size = document.xml_size.saturating_add(xml.len());
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
    let names = document.names;
    let mut reading = Reading {
        c: Capture::new(document, part_name, input.as_str(), diagnostics, budget),
        part,
        story,
        note: None,
        names,
        w,
        r,
        mc,
        body: Vec::new(),
        breaks: Vec::new(),
        last_section: None,
        stack: Vec::new(),
        defining: Vec::new(),
        tables_open: 0,
        pending_break: None,
        open_blocks: Vec::new(),
        continued: HashMap::new(),
        link_targets: HashMap::new(),
        images: HashMap::new(),
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
        while let Some(event) = reader.read_event().map_err(|err| opc(err, self.c.part))? {
            match event {
                Event::Start(tag) => {
                    self.c.at = tag.offset();
                    let disposition = mce
                        .start(reader, &tag)
                        .map_err(|err| opc(err, self.c.part))?;
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
                        self.c
                            .diagnose(offset, DiagnosticKind::UnexpectedContent, || {
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
            // A header or footer part is read by its root, whichever kind of relationship
            // reached it first: each reference's kind is checked against it later.
            let other = match self.story {
                Story::Header => Some(Story::Footer),
                Story::Footer => Some(Story::Header),
                _ => None,
            };
            if let Some(other) = other.filter(|other| {
                name.is(self.w[0], other.root()) || name.is(self.w[1], other.root())
            }) {
                self.story = other;
            }
            let root = self.story.root();
            let conformance = if name.is(self.w[0], root) {
                Conformance::Transitional
            } else if name.is(self.w[1], root) {
                Conformance::Strict
            } else {
                return Err(self.malformed(
                    reader,
                    &tag,
                    format!(
                        "the root element is <{}>, not w:{root}",
                        shorten(name.qname().as_str())
                    ),
                ));
            };
            match self.story {
                Story::Document => {
                    self.c.document.conformance = conformance;
                    self.push(Frame::Root);
                }
                // A header's or footer's blocks are the children of its root.
                Story::Header | Story::Footer => self.push(Frame::Container {
                    parent: Parent::Body,
                    level: Level::Block,
                }),
                Story::Footnotes | Story::Endnotes => self.push(Frame::Root),
            }
            return Ok(());
        }
        if let Some(Frame::Alternate { taken }) = self.stack.last_mut() {
            let choice = name.is(self.mc, "Choice");
            let take = !*taken
                && (name.is(self.mc, "Fallback")
                    || (choice
                        && mce
                            .is_selectable(reader, &tag)
                            .map_err(|err| Error::from(err.in_part(self.c.part)))?));
            if take {
                *taken = true;
                self.push(Frame::Transparent);
            } else {
                return self.c.skip(reader, mce);
            }
            return Ok(());
        }
        if name.is(self.mc, "AlternateContent") {
            self.push(Frame::Alternate { taken: false });
            return Ok(());
        }
        match disposition {
            Disposition::ProcessContent => {
                self.push(Frame::Transparent);
                return Ok(());
            }
            Disposition::Understood if self.is_w(&tag) => {}
            _ => return self.keep(reader, mce, tag),
        }
        let local = name.local();
        let frame = self.context();
        match frame {
            Context::Root => match (self.story, local) {
                (Story::Document, "body") => {
                    self.push(Frame::Container {
                        parent: Parent::Body,
                        level: Level::Block,
                    });
                    Ok(())
                }
                (Story::Footnotes, "footnote") | (Story::Endnotes, "endnote") => {
                    self.open_note(reader, mce, tag)
                }
                // w:background and the like: kept for writing in a later version.
                _ => self.c.capture(reader, mce, tag).map(drop),
            },
            Context::Text => {
                let offset = tag.offset();
                self.c
                    .diagnose(offset, DiagnosticKind::UnexpectedContent, || {
                        format!(
                            "element <{}> inside text is ignored",
                            shorten(name.qname().as_str())
                        )
                    });
                self.c.skip(reader, mce)
            }
            Context::ContentControl { node, level } => match local {
                "sdtPr" | "sdtEndPr" => self.property(reader, mce, tag, node),
                "sdtContent" => {
                    self.push(Frame::Container {
                        parent: Parent::Node(node),
                        level,
                    });
                    let frame = self.stack.len() - 1;
                    if let Some(open) = self.open_blocks.last_mut() {
                        if open.node == node && open.content.is_none() {
                            open.content = Some(frame);
                        }
                    }
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
            Parent::Node(node) => self.c.document.node(node).map(|node| &node.kind),
            Parent::Body => None,
        };
        // Properties of the parent element come first in it.
        let property = matches!(
            (parent_kind, local),
            (Some(Kind::Paragraph { .. }), "pPr")
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
            (Level::Block, "p") => {
                self.open(parent, Kind::Paragraph { style: None }, Level::Inline)
            }
            // Block-level w:ins, w:del, w:moveFrom and w:moveTo (around paragraphs or
            // rows) and a w:tbl written inside a paragraph are kept without being read.
            (Level::Block, "tbl") => self.open(
                parent,
                Kind::Table {
                    columns: Vec::new(),
                },
                Level::Row,
            ),
            (Level::Block, "sectPr") if parent == Parent::Body && self.story == Story::Document => {
                let place = self.c.place(tag.offset());
                let span = self.c.capture(reader, mce, tag)?;
                let element = span.of(&self.c.document.tokens, &self.c.document.strings);
                let properties = section::read(element, &self.names, &place, self.c.diagnostics);
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
                self.push(Frame::ContentControl { node, level });
                self.note_open_block(node, level, None);
                Ok(())
            }
            (Level::Block | Level::Row | Level::Cell, "customXml")
            | (Level::Inline, "customXml" | "smartTag" | "dir" | "bdo") => {
                self.open(parent, Kind::Wrapper, level)?;
                if let Some(Frame::Container {
                    parent: Parent::Node(node),
                    ..
                }) = self.stack.last()
                {
                    let (node, frame) = (*node, self.stack.len() - 1);
                    self.note_open_block(node, level, Some(frame));
                }
                Ok(())
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
                    self.c
                        .diagnose(tag.offset(), DiagnosticKind::InvalidValue, || {
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
            "drawing" => return self.drawing(reader, mce, tag, run),
            "footnoteRef" | "endnoteRef" => RunItem::NoteNumber,
            "separator" => RunItem::Separator,
            "continuationSeparator" => RunItem::ContinuationSeparator,
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
                let element = self.c.capture(reader, mce, tag)?;
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
        self.c.skip(reader, mce)
    }

    fn end(&mut self) -> Result<()> {
        let popped = self.stack.pop();
        self.defining.pop();
        if self
            .open_blocks
            .last()
            .is_some_and(|open| open.frame == self.stack.len())
        {
            self.open_blocks.pop();
        }
        match popped {
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
                }) = self.c.document.nodes.get_mut(run as usize)
                {
                    content.push(item);
                }
            }
            Some(Frame::Container {
                parent: Parent::Node(node),
                level,
            }) => {
                let kind = self.c.document.node(node).map(|node| &node.kind);
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
        if self
            .note
            .is_some_and(|(frame, ..)| frame == self.stack.len())
        {
            self.close_note();
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
        for i in 0..self.open_blocks.len() {
            let Some(&OpenBlock {
                node: original,
                frame,
                content,
            }) = self.open_blocks.get(i)
            else {
                break;
            };
            let Some(source) = self.c.document.node(original) else {
                continue;
            };
            let kind = match source.kind {
                Kind::ContentControl { level } => Kind::ContentControl { level },
                _ => Kind::Wrapper,
            };
            let properties = source.properties.clone();
            let copy = self.add(parent, kind)?;
            if let Some(node) = self.c.document.nodes.get_mut(copy as usize) {
                node.properties = properties;
            }
            // The paragraph that ends the section is inside the original, so it is not
            // empty and stays; it only stops being the open copy.
            self.continued.remove(&original);
            self.continued.insert(copy, parent);
            for index in std::iter::once(frame).chain(content) {
                match self.stack.get_mut(index) {
                    Some(Frame::ContentControl { node, .. }) => *node = copy,
                    Some(Frame::Container { parent, .. }) => *parent = Parent::Node(copy),
                    _ => {}
                }
            }
            if let Some(open) = self.open_blocks.get_mut(i) {
                open.node = copy;
            }
            parent = Parent::Node(copy);
        }
        Ok(())
    }

    /// Notes a block-level content control or custom XML element of the body as open,
    /// with the frame that closes it and, for a custom XML element, the frame its
    /// children are read in.
    fn note_open_block(&mut self, node: u32, level: Level, content: Option<usize>) {
        if level == Level::Block && self.tables_open == 0 {
            self.open_blocks.push(OpenBlock {
                node,
                frame: self.stack.len() - 1,
                content,
            });
        }
    }

    /// Takes a continued content control or wrapper out again when nothing went into it.
    fn drop_if_empty(&mut self, node: u32) {
        let Some(parent) = self.continued.remove(&node) else {
            return;
        };
        if self
            .c
            .document
            .node(node)
            .is_some_and(|node| !node.children.is_empty())
        {
            return;
        }
        let siblings = match parent {
            Parent::Body => &mut self.body,
            Parent::Node(parent) => match self.c.document.nodes.get_mut(parent as usize) {
                Some(parent) => &mut parent.children,
                None => return,
            },
        };
        if siblings.last() == Some(&node) {
            siblings.pop();
        }
    }

    /// Begins a note (`w:footnote`, `w:endnote`): its blocks are read like a body's. One
    /// without a valid id is kept without being read, as nothing can refer to it.
    fn open_note(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
    ) -> Result<()> {
        let id = self
            .w_attribute(&tag, "id")
            .and_then(|id| id.parse::<i64>().ok());
        let Some(id) = id else {
            let value = self.w_attribute(&tag, "id").unwrap_or_default().to_owned();
            self.invalid(&tag, "id", &value);
            return self.c.capture(reader, mce, tag).map(drop);
        };
        let note_type = match self.w_attribute(&tag, "type") {
            None => NoteType::Normal,
            Some(value) => NoteType::from_xml(value).unwrap_or_else(|| {
                let value = value.to_owned();
                self.invalid(&tag, "type", &value);
                NoteType::Normal
            }),
        };
        self.push(Frame::Container {
            parent: Parent::Body,
            level: Level::Block,
        });
        self.note = Some((self.stack.len() - 1, id, note_type));
        Ok(())
    }

    /// Ends the note being read: its blocks become a story of the document. Of two notes
    /// with one id, the first is the one references lead to.
    fn close_note(&mut self) {
        let Some((_, id, note_type)) = self.note.take() else {
            return;
        };
        let endnote = self.story == Story::Endnotes;
        let blocks = std::mem::take(&mut self.body);
        let document = &mut *self.c.document;
        let index = document.stories.len() as u32;
        document.stories.push(StoryData {
            kind: StoryKind::Note {
                endnote,
                id,
                note_type,
            },
            blocks,
        });
        let notes = if endnote {
            &mut document.endnotes
        } else {
            &mut document.footnotes
        };
        notes.entry(id).or_insert(index);
    }

    /// Ends reading: splits the body into sections, or keeps a header's or footer's blocks
    /// as a story.
    fn finish(mut self) {
        match self.story {
            Story::Document => {}
            Story::Header | Story::Footer => {
                let kind = if self.story == Story::Header {
                    StoryKind::Header
                } else {
                    StoryKind::Footer
                };
                let blocks = std::mem::take(&mut self.body);
                self.c.document.stories.push(StoryData { kind, blocks });
                return;
            }
            Story::Footnotes | Story::Endnotes => return,
        }
        let mut sections = Vec::with_capacity(self.breaks.len() + 1);
        let mut start = 0;
        for (end, properties) in std::mem::take(&mut self.breaks) {
            let blocks = self.body.get(start..end).unwrap_or_default().to_vec();
            sections.push(SectionData {
                properties,
                blocks,
                ..SectionData::default()
            });
            start = end;
        }
        sections.push(SectionData {
            properties: self.last_section.take().unwrap_or_default(),
            blocks: self.body.get(start..).unwrap_or_default().to_vec(),
            ..SectionData::default()
        });
        self.c.document.sections = sections;
    }

    /// The innermost frame that says where children go; transparent frames and the
    /// branches of `mc:AlternateContent` hand their children to it. Found without
    /// walking the stack, however many transparent frames are open.
    fn context(&self) -> Context {
        let frame = self
            .defining
            .last()
            .and_then(|&index| self.stack.get(index));
        match frame {
            Some(Frame::Container { parent, level }) => Context::Container {
                parent: *parent,
                level: *level,
            },
            Some(Frame::ContentControl { node, level }) => Context::ContentControl {
                node: *node,
                level: *level,
            },
            Some(Frame::Text { .. }) => Context::Text,
            Some(Frame::Root | Frame::Alternate { .. } | Frame::Transparent) | None => {
                Context::Root
            }
        }
    }

    fn push(&mut self, frame: Frame) {
        let defining = match frame {
            // The root element is pushed first, so a transparent frame always has one
            // below it; the fallback only keeps this total.
            Frame::Alternate { .. } | Frame::Transparent => {
                self.defining.last().copied().unwrap_or(self.stack.len())
            }
            _ => self.stack.len(),
        };
        self.stack.push(frame);
        self.defining.push(defining);
    }

    /// Adds a node to `parent` and reads its children at `level`.
    fn open(&mut self, parent: Parent, kind: Kind, level: Level) -> Result<()> {
        if matches!(kind, Kind::Table { .. }) {
            self.tables_open += 1;
        }
        let node = self.add(parent, kind)?;
        self.push(Frame::Container {
            parent: Parent::Node(node),
            level,
        });
        Ok(())
    }

    fn add(&mut self, parent: Parent, kind: Kind) -> Result<u32> {
        self.c.spend(1)?;
        let index = self.c.document.nodes.len() as u32;
        let parent_index = match parent {
            Parent::Body => Node::NO_PARENT,
            Parent::Node(parent) => parent,
        };
        self.c.document.nodes.push(Node {
            kind,
            parent: parent_index,
            properties: Vec::new(),
            children: Vec::new(),
        });
        match parent {
            Parent::Body => self.body.push(index),
            Parent::Node(parent) => {
                if let Some(parent) = self.c.document.nodes.get_mut(parent as usize) {
                    parent.children.push(index);
                }
            }
        }
        Ok(index)
    }

    fn push_item(&mut self, run: u32, item: RunItem) -> Result<()> {
        self.c.spend(1)?;
        if let Some(Node {
            kind: Kind::Run { content },
            ..
        }) = self.c.document.nodes.get_mut(run as usize)
        {
            content.push(item);
        }
        Ok(())
    }

    fn push_frame(&mut self, frame: Frame) -> Result<()> {
        self.c.spend(1)?;
        self.push(frame);
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
        let span = self.c.capture(reader, mce, tag)?;
        let kind = self.c.document.node(node).map(|node| &node.kind);
        let (paragraph, table) = (
            matches!(kind, Some(Kind::Paragraph { .. })),
            matches!(kind, Some(Kind::Table { .. })),
        );
        if paragraph {
            self.section_break(node, span, offset);
        }
        if table && self.c.document.element(span).is(&self.names.w, "tblGrid") {
            let columns = self.grid(span, offset);
            if let Some(Node {
                kind: Kind::Table { columns: table },
                ..
            }) = self.c.document.nodes.get_mut(node as usize)
            {
                *table = columns;
            }
        }
        let local = self.c.document.element(span).local_name();
        let which = match (self.c.document.node(node).map(|node| &node.kind), local) {
            (Some(Kind::Run { .. }), "rPr") => Some(Properties::Run),
            (Some(Kind::Paragraph { .. }), "pPr") => Some(Properties::Paragraph),
            (Some(Kind::Table { .. }), "tblPr") | (Some(Kind::Row), "tblPrEx") => {
                Some(Properties::Table)
            }
            (Some(Kind::Row), "trPr") => Some(Properties::Row),
            (Some(Kind::Cell), "tcPr") => Some(Properties::Cell),
            _ => None,
        };
        if let Some(which) = which {
            let names = self.names;
            check_properties(&mut self.c, &names, span, &[], which, offset);
        }
        if let Some(node) = self.c.document.nodes.get_mut(node as usize) {
            node.properties.push(span);
        }
        Ok(())
    }

    /// A `w:sectPr` in the properties of a paragraph ends a section after the paragraph
    /// (see [`break_section`](Self::break_section)). Word never writes one in a table cell,
    /// where its section break command splits the table instead; one found there ends no
    /// section until a rule document shows what Word makes of it.
    fn section_break(&mut self, paragraph: u32, p_pr: Span, offset: usize) {
        // Only the body has sections.
        if self.tables_open > 0 || self.story != Story::Document {
            return;
        }
        let place = self.c.place(offset);
        let p_pr = p_pr.of(&self.c.document.tokens, &self.c.document.strings);
        let Some(sect_pr) = p_pr.children().find(|c| c.is(&self.names.w, "sectPr")) else {
            return;
        };
        let properties = section::read(sect_pr, &self.names, &place, self.c.diagnostics);
        self.pending_break = Some((paragraph, properties));
    }

    /// The widths of the columns of a `w:tblGrid`.
    fn grid(&mut self, tbl_grid: Span, offset: usize) -> Vec<Option<officewright_core::Pt>> {
        let w = self.names.w;
        let place = self.c.place(offset);
        let mut columns = Vec::new();
        for col in tbl_grid
            .of(&self.c.document.tokens, &self.c.document.strings)
            .children()
        {
            if !col.is(&w, "gridCol") {
                continue;
            }
            let width = col.attribute(&w, "w");
            let length = width.and_then(|width| twips_measure(width, false));
            if let (Some(width), None) = (width, length) {
                section::invalid(self.c.diagnostics, &place, col, "w", width);
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
            self.c
                .diagnose(tag.offset(), DiagnosticKind::InvalidValue, || {
                    format!(
                        "hyperlink r:id=\"{}\" names no relationship; the link is ignored",
                        shorten(id)
                    )
                });
            return None;
        };
        let target = (relationship.target_mode() == officewright_opc::TargetMode::External)
            .then(|| self.c.document.strings.intern(relationship.target()));
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
        let span = self.c.capture(reader, mce, tag)?;
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

    /// A drawing (`w:drawing`), kept whole, with the image its picture shows read from
    /// the package: the part the picture's `r:embed` names among the relationships of the
    /// part being read, read once however many pictures show it. An image the package
    /// does not have is reported, and the picture shows none; one outside the package
    /// (`r:link`, or an external relationship) is never loaded.
    fn drawing(
        &mut self,
        reader: &mut Reader<'_>,
        mce: &mut Processor,
        tag: StartTag<'_>,
        run: u32,
    ) -> Result<()> {
        let offset = tag.offset();
        let element = self.c.capture(reader, mce, tag)?;
        let document = &*self.c.document;
        let embed = drawing::picture(document.element(element), &document.names)
            .and_then(|picture| drawing::embedded_image(picture, &document.names))
            .map(str::to_owned);
        let image = match embed {
            Some(id) => self.image(&id, offset)?,
            None => None,
        };
        self.push_item(run, RunItem::Drawing { element, image })
    }

    /// The image the relationship `id` of the part leads to, read the first time a
    /// picture of the package shows it. One the part or the package does not have, or
    /// that cannot be read, is reported once per relationship of the part, and the
    /// pictures show none; only a limit of the package stops the reading.
    fn image(&mut self, id: &str, offset: usize) -> Result<Option<u32>> {
        if let Some(&image) = self.images.get(id) {
            return Ok(image);
        }
        let image = self.read_image(id, offset)?;
        self.images.insert(id.into(), image);
        Ok(image)
    }

    fn read_image(&mut self, id: &str, offset: usize) -> Result<Option<u32>> {
        let Some(relationship) = self.part.relationships().get(id) else {
            let id = id.to_owned();
            self.c
                .diagnose(offset, DiagnosticKind::UnexpectedContent, || {
                    format!(
                        "a picture names the relationship {}, which the part does not have",
                        shorten(&id)
                    )
                });
            return Ok(None);
        };
        // None for an external target, which is never loaded.
        let Some(name) = relationship.target_part() else {
            return Ok(None);
        };
        if let Some(&index) = self.c.document.image_parts.get(name.as_str()) {
            return Ok(Some(index));
        }
        let Some(part) = self.part.package().part(&name) else {
            self.c
                .diagnose(offset, DiagnosticKind::UnexpectedContent, || {
                    format!(
                        "a picture shows the image {}, which the package does not have",
                        shorten(name.as_str())
                    )
                });
            return Ok(None);
        };
        let bytes = match part.read() {
            Ok(bytes) => bytes,
            Err(err @ officewright_opc::Error::LimitExceeded { .. }) => return Err(err.into()),
            Err(err) => {
                self.c
                    .diagnose(offset, DiagnosticKind::UnexpectedContent, || {
                        format!("a picture's image cannot be read: {err}")
                    });
                return Ok(None);
            }
        };
        let document = &mut *self.c.document;
        let index = document.images.len() as u32;
        document.images.push(ImageData {
            content_type: part.content_type().unwrap_or_default().into(),
            bytes: bytes.into(),
        });
        document.image_parts.insert(name.as_str().into(), index);
        Ok(Some(index))
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
        self.c
            .diagnose(tag.offset(), DiagnosticKind::InvalidValue, || {
                format!(
                    "w:{element} w:{attribute}=\"{}\" is not valid; it is ignored",
                    shorten(value)
                )
            });
    }

    fn malformed(&self, reader: &Reader<'_>, tag: &StartTag<'_>, message: String) -> Error {
        Error::from(reader.error(tag.offset(), message).in_part(self.c.part))
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
