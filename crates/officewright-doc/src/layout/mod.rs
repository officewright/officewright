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
    /// flags, table cells copied where a horizontal merge should move them, and cells
    /// whose properties in effect were worked out.
    pub(crate) static WORK: std::cell::Cell<Work> = const { std::cell::Cell::new(Work {
        inlines: 0,
        compared: 0,
        built: 0,
        cut_steps: 0,
        flags: 0,
        cell_copies: 0,
        cell_reads: 0,
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
    /// Cells whose properties in effect the formatting worked out.
    pub(crate) cell_reads: usize,
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
    /// Not translated yet, and listed: the table properties of a table style's conditional
    /// formats and a row's table property exceptions, drawings that show no picture
    /// (shapes, charts, groups), VML drawings
    /// (`w:pict`) and other objects, equations, and indents and spacing given in
    /// character or line units, which keep their value in points.
    ///
    /// Also listed is what the IR has no place for yet: a row's own alignment and cell
    /// spacing where they differ from the table's, a row's hidden end mark, a cell's
    /// text fitting and diagonal borders, and the content of a cell that continues a
    /// horizontal merge. A cell that does not end with a paragraph, which Word refuses
    /// to open, gets an empty one, and that is listed too.
    ///
    /// And, once per document at the first place each is met, what the IR has no place
    /// for yet in sections, paragraphs, runs and pictures: page borders, line numbers,
    /// the document grid and vertical text; right-to-left paragraphs, text frames and
    /// drop caps; in a run that shows text and is not hidden, embossed, engraved,
    /// outlined and shadowed text, emphasis marks, and Word 2010's effects of text (glow,
    /// shadow, reflection, outline, fill and 3-D) unless their values change nothing;
    /// borders around text, in a run that is not hidden and shows text, a tab or a
    /// picture in the line; a picture's outline,
    /// its colour effects, its shadow, reflection, glow, soft edges and 3-D effects, and
    /// a shape it is cut to; and a paragraph mark, a table row or a table cell deleted as
    /// a tracked change, which the IR keeps, and a paragraph mark moved away. A deleted
    /// mark is listed only where a paragraph or a table follows, in the same cell or
    /// story or in the next section: the last paragraph of a cell or a story is joined to
    /// nothing. A row in which the last paragraph mark of every cell is deleted or moved
    /// away is listed as a deleted row, since that is how Word writes a row it moves. Other tracked
    /// changes are taken as accepted and comments are left out, as Word's PDF of the
    /// document without markup shows them, so they are not listed.
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
    /// What has been reported once for the document, a bit per [`Once`].
    reported: u32,
    /// The last paragraph translated, while its mark is deleted as a tracked change and
    /// that has not been listed yet: listed once a block follows it.
    deleted_mark: Option<PendingMark>,
    /// Whether the blocks translated last end with a paragraph whose mark is deleted as a
    /// tracked change: in a cell, Word marks so the last paragraph of every cell of a row
    /// it deletes or moves away.
    ends_deleted: bool,
}

/// A paragraph whose mark is deleted as a tracked change, until a block follows it. Its
/// place is built only when it is listed: most such paragraphs end a cell, and the path
/// to one in nested tables is long.
enum PendingMark {
    /// At this index among the blocks being translated.
    At(usize),
    /// The last paragraph of a section's body, at this place, carried into the next.
    Carried(Location),
}

/// The entry of a paragraph mark deleted as a tracked change.
const DELETED_MARK: &str = "a paragraph mark deleted or moved away as a tracked change (w:del or w:moveFrom) is kept, so the paragraph is not joined to the next as in Word's PDF of the document without markup, here and wherever else the document has one";

/// What the translation reports once per document, at the first place it meets it: a
/// feature that a document may use on every line, in every row or picture, or in every
/// section (a mail merge writes a section per letter), so that the report's 1,000
/// entries are not spent on one feature.
#[derive(Clone, Copy, Debug)]
enum Once {
    Emboss = 1,
    Imprint = 1 << 1,
    Outline = 1 << 2,
    Shadow = 1 << 3,
    EmphasisMark = 1 << 4,
    RunBorder = 1 << 5,
    RightToLeft = 1 << 6,
    Frame = 1 << 7,
    DropCap = 1 << 8,
    PageBorders = 1 << 9,
    LineNumbers = 1 << 10,
    DocumentGrid = 1 << 11,
    TextDirection = 1 << 12,
    DeletedMark = 1 << 13,
    DeletedRow = 1 << 14,
    PictureOutline = 1 << 15,
    PictureColor = 1 << 16,
    PictureEffects = 1 << 17,
    PictureShape = 1 << 18,
    DeletedCell = 1 << 19,
    Glow = 1 << 20,
    TextShadow = 1 << 21,
    Reflection = 1 << 22,
    TextOutline = 1 << 23,
    TextFill = 1 << 24,
    Text3d = 1 << 25,
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
            reported: 0,
            deleted_mark: None,
            ends_deleted: false,
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
        #[cfg(test)]
        work(|work| work.cell_reads += self.formats.work().cells);
        Translation {
            document: self.ir,
            left_out: self.left_out,
        }
    }

    /// The blocks of a story or a cell, through content controls, which the IR does not
    /// have.
    fn blocks(&mut self, blocks: impl Iterator<Item = Block<'a>> + 'a) -> Vec<ir::Block> {
        self.blocks_in(Some("blocks"), blocks, false)
    }

    /// The blocks of a section's body: as [`Translator::blocks`], but continuing the
    /// bodies before it, since Word joins a paragraph whose mark is deleted at the end of
    /// one to the next, removing the section break.
    fn body_blocks(&mut self, blocks: impl Iterator<Item = Block<'a>> + 'a) -> Vec<ir::Block> {
        self.blocks_in(Some("blocks"), blocks, true)
    }

    /// A paragraph as a block, and whether its mark is deleted as a tracked change;
    /// translated here, so that its temporaries take no room in the frame of
    /// [`Translator::blocks_in`], which nested tables repeat (#187).
    #[inline(never)]
    fn paragraph_block(&mut self, paragraph: crate::model::Paragraph<'a>) -> (ir::Block, bool) {
        let (paragraph, deleted) = paragraph::paragraph(self, paragraph);
        (paragraph.into(), deleted)
    }

    /// Blocks held in the field `field` of what is being translated, or, with none, in
    /// the list of blocks the current place names (a header, a note separator): the
    /// place of each is written as the layout's report writes it. A paragraph whose mark
    /// is deleted is listed once a paragraph or a table follows it in these blocks, or,
    /// when they `continues` the blocks before them (a section's body), in the next ones:
    /// the last paragraph of a cell or a story has nothing after it to be joined to.
    fn blocks_in(
        &mut self,
        field: Option<&'static str>,
        blocks: impl Iterator<Item = Block<'a>> + 'a,
        continues: bool,
    ) -> Vec<ir::Block> {
        // These blocks may be translated inside a paragraph (a note) or between bodies
        // (a header): what the blocks around them left pending waits for them to end.
        let outer = if continues {
            None
        } else {
            self.deleted_mark.take()
        };
        let mut out = Vec::new();
        let mut ends_deleted = false;
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
                Block::Paragraph(paragraph) => {
                    self.joined();
                    let (block, deleted) = self.paragraph_block(paragraph);
                    if deleted && !self.reported(Once::DeletedMark) {
                        self.deleted_mark = Some(PendingMark::At(out.len()));
                    }
                    ends_deleted = deleted;
                    out.push(block);
                }
                Block::ContentControl(control) => pending.push(Box::new(control.blocks())),
                Block::Table(table) => {
                    self.joined();
                    ends_deleted = false;
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
        if continues {
            if let Some(PendingMark::At(index)) = self.deleted_mark {
                let at = self.location_at(self.path.len(), field, index);
                self.deleted_mark = Some(PendingMark::Carried(at));
            }
        } else {
            self.deleted_mark = outer;
        }
        self.ends_deleted = ends_deleted;
        out
    }

    /// Lists the paragraph whose deleted mark was pending, now that a block follows it,
    /// at the place of the block in [`Translator::blocks_in`].
    fn joined(&mut self) {
        let at = match self.deleted_mark.take() {
            None => return,
            Some(PendingMark::Carried(at)) => at,
            // Its index in place of the index of the block that follows it.
            Some(PendingMark::At(index)) => {
                self.location_at(self.path.len().saturating_sub(1), None, index)
            }
        };
        self.report_once_at(Once::DeletedMark, at, DELETED_MARK);
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
        if self.room() {
            self.left_out
                .push(ReportEntry::new(kind, self.location(), message()));
        }
    }

    /// Whether the list has room for another entry, counting the entry as not listed
    /// when it has none. One place is kept for the entry that counts those not listed.
    fn room(&mut self) -> bool {
        let room = self.left_out.len() < MAX_LEFT_OUT - 1;
        if !room {
            self.omitted += 1;
        }
        room
    }

    /// Lists `message` the first time the document meets `once`, at that place.
    fn report_once(&mut self, once: Once, message: &'static str) {
        if !self.reported(once) {
            let at = self.location();
            self.report_once_at(once, at, message);
        }
    }

    /// Whether `once` has been listed.
    fn reported(&self, once: Once) -> bool {
        self.reported & once as u32 != 0
    }

    /// Lists `message` at the place `at`, unless `once` has been listed.
    fn report_once_at(&mut self, once: Once, at: Location, message: &'static str) {
        if !self.reported(once) {
            self.reported |= once as u32;
            if self.room() {
                self.left_out.push(ReportEntry::new(
                    ReportKind::FeatureUnsupported,
                    at,
                    String::from(message),
                ));
            }
        }
    }

    /// The current place in the IR.
    fn location(&self) -> Location {
        self.location_at(self.path.len(), None, None)
    }

    /// The place of the first `keep` steps of the current one, then the field `field`,
    /// then the item at `index`.
    fn location_at(
        &self,
        keep: usize,
        field: Option<&'static str>,
        index: impl Into<Option<usize>>,
    ) -> Location {
        let steps = self.path.get(..keep).unwrap_or(&self.path);
        let location = steps
            .iter()
            .fold(Location::document(), |location, step| match *step {
                Step::Field(name) => location.field(name),
                Step::Index(index) => location.index(index),
            });
        let location = match field {
            Some(name) => location.field(name),
            None => location,
        };
        match index.into() {
            Some(index) => location.index(index),
            None => location,
        }
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
