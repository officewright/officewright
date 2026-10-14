//! The document as the layout engine reads it: the translation of the model into the
//! layout IR of `officewright-layout`, on the way to PDF.
//!
//! The IR wants everything resolved: the formatting in effect of every paragraph, run,
//! paragraph mark and list label ([`Formats`]), lengths in points, colours as values.
//! What the IR cannot hold yet, or what the model does not read yet, is left out and
//! listed in [`Translation::left_out`], so that users know why the output differs from
//! Word's.

mod inline;
mod list;
mod paragraph;
mod section;
mod style;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use officewright_layout::ReportKind;
use officewright_layout::ir;

use crate::model::{Block, Document, Formats};

/// The most entries a translation lists; the others are counted in one last entry.
const MAX_LEFT_OUT: usize = 1000;

impl Document {
    /// The document in the layout IR of `officewright-layout`, with what the IR does not
    /// hold listed in [`Translation::left_out`]. The IR, and so this function, is exempt
    /// from semver until 1.0.
    ///
    /// Not translated yet, and listed: tables, headers and footers, footnotes and
    /// endnotes, drawings, pictures and other objects, equations, and indents and spacing
    /// given in character or line units, which keep their value in points.
    #[cfg_attr(not(feature = "unstable-ir"), doc(hidden))]
    pub fn to_layout_ir(&self) -> Translation {
        Translator::new(self).translate()
    }
}

/// A document translated into the layout IR, and what the translation left out.
#[cfg_attr(not(feature = "unstable-ir"), doc(hidden))]
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Translation {
    document: ir::Document,
    left_out: Vec<LeftOut>,
}

impl Translation {
    /// The document in the IR.
    pub fn document(&self) -> &ir::Document {
        &self.document
    }

    /// The document in the IR, taken out of the translation.
    pub fn into_document(self) -> ir::Document {
        self.document
    }

    /// What the IR does not hold, or holds otherwise than Word lays it out, in document
    /// order; at most 1,000 entries, the last of which then counts the others.
    pub fn left_out(&self) -> &[LeftOut] {
        &self.left_out
    }
}

/// Something of the document the translation left out or changed.
#[cfg_attr(not(feature = "unstable-ir"), doc(hidden))]
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LeftOut {
    kind: ReportKind,
    location: String,
    message: String,
}

impl LeftOut {
    /// What kind of difference it is.
    pub fn kind(&self) -> ReportKind {
        self.kind
    }

    /// Where in the IR, as the layout's report writes places:
    /// `sections[0].blocks[3]`, or `document` for the document as a whole.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// What was left out, in words.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LeftOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

/// One step of the place in the IR the translation is at.
#[derive(Clone, Copy, Debug)]
enum Step {
    Field(&'static str),
    Index(usize),
}

/// The state of one translation.
struct Translator<'a> {
    document: &'a Document,
    formats: Formats<'a>,
    ir: ir::Document,
    run_styles: style::RunStyles,
    lists: list::Lists,
    /// A key per paragraph style, for contextual spacing; a paragraph without a style has
    /// 0.
    style_keys: HashMap<Arc<str>, ir::StyleKey>,
    left_out: Vec<LeftOut>,
    /// Entries not listed once the list is full.
    omitted: usize,
    path: Vec<Step>,
}

impl<'a> Translator<'a> {
    fn new(document: &'a Document) -> Self {
        Self {
            document,
            formats: document.formats(),
            ir: ir::Document::default(),
            run_styles: style::RunStyles::default(),
            lists: list::Lists::default(),
            style_keys: HashMap::new(),
            left_out: Vec::new(),
            omitted: 0,
            path: Vec::new(),
        }
    }

    fn translate(mut self) -> Translation {
        let settings = section::settings(&mut self);
        self.ir = ir::Document::new(settings);
        for (index, model) in self.document.sections().enumerate() {
            self.path
                .extend([Step::Field("sections"), Step::Index(index)]);
            let section = section::section(&mut self, model);
            self.ir.add_section(section);
            self.path.truncate(self.path.len().saturating_sub(2));
        }
        if self.omitted > 0 {
            let omitted = self.omitted;
            self.left_out.push(LeftOut {
                kind: ReportKind::FeatureUnsupported,
                location: "document".to_owned(),
                message: format!("{omitted} more entries are not listed"),
            });
        }
        Translation {
            document: self.ir,
            left_out: self.left_out,
        }
    }

    /// The blocks of a story, through content controls, which the IR does not have.
    fn blocks(&mut self, blocks: impl Iterator<Item = Block<'a>> + 'a) -> Vec<ir::Block> {
        let mut out = Vec::new();
        // Content controls nest: walked with a stack of their blocks, never recursively.
        let mut pending: Vec<Box<dyn Iterator<Item = Block<'a>> + 'a>> = vec![Box::new(blocks)];
        while let Some(top) = pending.last_mut() {
            let Some(block) = top.next() else {
                pending.pop();
                continue;
            };
            self.path
                .extend([Step::Field("blocks"), Step::Index(out.len())]);
            match block {
                Block::Paragraph(paragraph) => {
                    out.push(paragraph::paragraph(self, paragraph).into());
                }
                Block::ContentControl(control) => pending.push(Box::new(control.blocks())),
                Block::Table(_) => {
                    self.leave_out(ReportKind::FeatureUnsupported, || {
                        "a table is left out: tables are not translated yet".into()
                    });
                }
                Block::Other(other) => {
                    if !is_marker(other.local_name()) {
                        let name = other.local_name().to_owned();
                        self.leave_out(ReportKind::FeatureUnsupported, || {
                            format!("a block-level w:{name} is left out")
                        });
                    }
                }
            }
            self.path.truncate(self.path.len().saturating_sub(2));
        }
        out
    }

    /// The key of a paragraph style, for contextual spacing.
    fn style_key(&mut self, style: Option<&str>) -> ir::StyleKey {
        let Some(style) = style else {
            return ir::StyleKey::default();
        };
        if let Some(&key) = self.style_keys.get(style) {
            return key;
        }
        let number = u32::try_from(self.style_keys.len() + 1).unwrap_or(u32::MAX);
        let key = ir::StyleKey::new(number);
        self.style_keys.insert(style.into(), key);
        key
    }

    /// Lists something left out at the current place.
    fn leave_out(&mut self, kind: ReportKind, message: impl FnOnce() -> String) {
        if self.left_out.len() >= MAX_LEFT_OUT {
            self.omitted += 1;
            return;
        }
        self.left_out.push(LeftOut {
            kind,
            location: self.location(),
            message: message(),
        });
    }

    /// The current place, written as the layout's report writes places.
    fn location(&self) -> String {
        if self.path.is_empty() {
            return "document".to_owned();
        }
        let mut out = String::new();
        for (position, step) in self.path.iter().enumerate() {
            match step {
                Step::Field(name) if position == 0 => out.push_str(name),
                Step::Field(name) => {
                    out.push('.');
                    out.push_str(name);
                }
                Step::Index(index) => out.push_str(&format!("[{index}]")),
            }
        }
        out
    }
}

/// Whether an element kept without being understood only marks a place, such as a
/// bookmark, and so needs no layout.
fn is_marker(local_name: &str) -> bool {
    matches!(
        local_name,
        "bookmarkStart"
            | "bookmarkEnd"
            | "commentRangeStart"
            | "commentRangeEnd"
            | "moveFromRangeStart"
            | "moveFromRangeEnd"
            | "moveToRangeStart"
            | "moveToRangeEnd"
            | "permStart"
            | "permEnd"
            | "proofErr"
            | "customXmlInsRangeStart"
            | "customXmlInsRangeEnd"
            | "customXmlDelRangeStart"
            | "customXmlDelRangeEnd"
            | "customXmlMoveFromRangeStart"
            | "customXmlMoveFromRangeEnd"
            | "customXmlMoveToRangeStart"
            | "customXmlMoveToRangeEnd"
    )
}
