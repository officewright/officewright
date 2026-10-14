//! The document as the layout engine reads it: the translation of the model into the
//! layout IR of `officewright-layout`, on the way to PDF.
//!
//! The IR wants everything resolved: the formatting in effect of every paragraph, run,
//! paragraph mark and list label ([`Formats`]), lengths in points, colours as values.
//! What the IR cannot hold yet, or what the model does not read yet, is left out and
//! listed in [`Translation::left_out`], so that users know why the output differs from
//! Word's.

mod drawing;
mod inline;
mod list;
mod notes;
mod paragraph;
mod render;
mod section;
mod style;
mod table;

use std::collections::HashMap;

use officewright_layout::ir;
use officewright_layout::{LayoutReport, Location, ReportEntry, ReportKind};

use crate::model::{Block, Document, Formats};

#[cfg(test)]
thread_local! {
    /// Work the translation did on this thread, for tests of its cost: inlines added to
    /// paragraphs and field results, bytes of names and tags looked up to share run
    /// styles, run styles built, places tried to cut names, comparisons of compatibility
    /// flags, and table cells copied where a horizontal merge should move them.
    pub(crate) static WORK: std::cell::Cell<Work> = const { std::cell::Cell::new(Work {
        inlines: 0,
        compared: 0,
        built: 0,
        cut_steps: 0,
        flags: 0,
        cell_copies: 0,
    }) };
}

/// The kinds of work [`WORK`] counts.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Work {
    pub(crate) inlines: usize,
    pub(crate) compared: usize,
    pub(crate) built: usize,
    pub(crate) cut_steps: usize,
    pub(crate) flags: usize,
    /// Table cells widened by a horizontal merge whose blocks were not where the cell
    /// was built: copies, where the cell should have been moved.
    pub(crate) cell_copies: usize,
}

/// Counts work for [`WORK`].
#[cfg(test)]
fn work(count: impl FnOnce(&mut Work)) {
    let mut work = WORK.get();
    count(&mut work);
    WORK.set(work);
}

/// The most entries a translation lists, the last of which counts the others when there
/// are more.
const MAX_LEFT_OUT: usize = 1000;

/// Whether Word uses OpenType features in a document: ligatures and contextual
/// alternates, which it uses only when the compatibility setting
/// `enableOpenTypeFeatures` is on, off by default ([MS-DOCX] 2.3.3). Of the setting
/// written twice, the last counts, as for the other compatibility settings. The note
/// names kerning too, which the translation does not gate yet (see `translate_run_style`).
fn open_type_features(document: &Document) -> bool {
    document
        .settings()
        .compatibility_settings()
        .iter()
        .rev()
        .find(|setting| setting.name() == "enableOpenTypeFeatures")
        .and_then(|setting| setting.value())
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "on"))
}

impl Document {
    /// The document in the layout IR of `officewright-layout`, with what the IR does not
    /// hold listed in [`Translation::left_out`]. The IR, and so this function, is exempt
    /// from semver until 1.0.
    ///
    /// Not translated yet, and listed: table styles and a row's table property
    /// exceptions, drawings that show no picture (shapes, charts, groups), VML drawings
    /// (`w:pict`) and other objects, equations, and indents and spacing given in
    /// character or line units, which keep their value in points.
    ///
    /// Also listed is what the IR has no place for yet: a row's own alignment and cell
    /// spacing where they differ from the table's, a row's hidden end mark, a cell's
    /// text fitting and diagonal borders, and the content of a cell that continues a
    /// horizontal merge. A cell that does not end with a paragraph, which Word refuses
    /// to open, gets an empty one, and that is listed too.
    ///
    /// ```
    /// # fn main() -> officewright_doc::Result<()> {
    /// use officewright_doc::Document;
    ///
    /// let document = Document::open("tests/data/hello.docx")?;
    /// let translation = document.to_layout_ir();
    /// // The IR is valid for every document the reader opens.
    /// assert!(translation.document().validate().is_ok());
    /// assert_eq!(translation.document().sections().len(), 1);
    /// for entry in translation.left_out() {
    ///     println!("{entry}");
    /// }
    /// # Ok(())
    /// # }
    /// ```
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
    left_out: Vec<ReportEntry>,
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
    pub fn left_out(&self) -> &[ReportEntry] {
        &self.left_out
    }

    /// The document in the IR and what the translation left out, as a report the layout
    /// adds its own entries to.
    pub fn into_parts(self) -> (ir::Document, LayoutReport) {
        (self.document, self.left_out.into_iter().collect())
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
    /// Whether the document's compatibility setting `enableOpenTypeFeatures` is on.
    open_type_features: bool,
    formats: Formats<'a>,
    ir: ir::Document,
    run_styles: style::RunStyles,
    lists: list::Lists,
    /// A key per paragraph style, by its place among the styles, for contextual spacing.
    style_keys: HashMap<usize, ir::StyleKey>,
    left_out: Vec<ReportEntry>,
    /// Entries not listed once the list is full.
    omitted: usize,
    path: Vec<Step>,
    /// How many tables the current block is in.
    table_depth: usize,
    /// The translated blocks of each header and footer used so far, by story, and their
    /// weight, the bytes a copy of them takes.
    header_footers: HashMap<u32, (Vec<ir::Block>, usize)>,
    /// The weight of the headers and footers copied into sections so far.
    copied: usize,
    /// The IR note of each note referred to so far: whether it is an endnote, and its id.
    notes: HashMap<(bool, i64), ir::NoteId>,
    /// Whether the blocks being translated are a note's, which holds no note references.
    in_note: bool,
    /// How many notes the IR has.
    notes_added: usize,
    /// The IR image of each image of the package shown so far, by its place among the
    /// document's images.
    images: HashMap<u32, ir::ImageId>,
    /// The empty image of pictures whose image the package does not have.
    missing_image: Option<ir::ImageId>,
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
            table_depth: 0,
            open_type_features: open_type_features(document),
            header_footers: HashMap::new(),
            copied: 0,
            notes: HashMap::new(),
            in_note: false,
            notes_added: 0,
            images: HashMap::new(),
            missing_image: None,
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
        self.note_separators();
        if self.omitted > 0 {
            let omitted = self.omitted;
            self.left_out.push(ReportEntry::new(
                ReportKind::FeatureUnsupported,
                Location::document(),
                format!("{omitted} more entries are not listed"),
            ));
        }
        Translation {
            document: self.ir,
            left_out: self.left_out,
        }
    }

    /// The blocks of a story, through content controls, which the IR does not have.
    fn blocks(&mut self, blocks: impl Iterator<Item = Block<'a>> + 'a) -> Vec<ir::Block> {
        self.blocks_in(Some("blocks"), blocks)
    }

    /// A paragraph as a block; translated here, so that its temporaries take no room in
    /// the frame of [`Translator::blocks_in`], which nested tables repeat (#187).
    #[inline(never)]
    fn paragraph_block(&mut self, paragraph: crate::model::Paragraph<'a>) -> ir::Block {
        paragraph::paragraph(self, paragraph).into()
    }

    /// Blocks held in the field `field` of what is being translated, or, with none, in
    /// the list of blocks the current place names (a header, a note separator): the
    /// place of each is written as the layout's report writes it.
    fn blocks_in(
        &mut self,
        field: Option<&'static str>,
        blocks: impl Iterator<Item = Block<'a>> + 'a,
    ) -> Vec<ir::Block> {
        let mut out = Vec::new();
        let steps = if field.is_some() { 2 } else { 1 };
        // Content controls nest: walked with a stack of their blocks, never recursively.
        let mut pending: Vec<Box<dyn Iterator<Item = Block<'a>> + 'a>> = vec![Box::new(blocks)];
        while let Some(top) = pending.last_mut() {
            let Some(block) = top.next() else {
                pending.pop();
                continue;
            };
            if let Some(field) = field {
                self.path.push(Step::Field(field));
            }
            self.path.push(Step::Index(out.len()));
            match block {
                Block::Paragraph(paragraph) => out.push(self.paragraph_block(paragraph)),
                Block::ContentControl(control) => pending.push(Box::new(control.blocks())),
                Block::Table(table) => {
                    if let Some(table) = self.table(table) {
                        out.push(table.into());
                    }
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
            self.path.truncate(self.path.len().saturating_sub(steps));
        }
        out
    }

    /// The key of a paragraph's style, for contextual spacing: one per style, by its
    /// place among the styles, so that a paragraph without `w:pStyle` has the key of
    /// the default paragraph style it takes, and no style id is hashed per paragraph.
    /// Without a paragraph style at all, the key 0, which no style has.
    fn style_key(&mut self, paragraph: crate::model::Paragraph<'a>) -> ir::StyleKey {
        let Some(style) = self.formats.paragraph_style_index(paragraph) else {
            return ir::StyleKey::default();
        };
        if let Some(&key) = self.style_keys.get(&style) {
            return key;
        }
        let number = u32::try_from(self.style_keys.len() + 1).unwrap_or(u32::MAX);
        let key = ir::StyleKey::new(number);
        self.style_keys.insert(style, key);
        key
    }

    /// Lists something left out at the current place.
    fn leave_out(&mut self, kind: ReportKind, message: impl FnOnce() -> String) {
        // One place is kept for the entry that counts those not listed.
        if self.left_out.len() >= MAX_LEFT_OUT - 1 {
            self.omitted += 1;
            return;
        }
        self.left_out
            .push(ReportEntry::new(kind, self.location(), message()));
    }

    /// The current place in the IR.
    fn location(&self) -> Location {
        self.path
            .iter()
            .fold(Location::document(), |location, step| match *step {
                Step::Field(name) => location.field(name),
                Step::Index(index) => location.index(index),
            })
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
