//! The layout IR: a document as the layout engine reads it, independent of the format it
//! came from.
//!
//! A producer, such as the DOCX reader, resolves everything the format leaves to
//! inheritance (styles, themes, headers a section takes from the previous one) and
//! converts every length to [`Pt`](officewright_core::Pt). The IR then says what the
//! document intends: that a paragraph is justified, that its text is 11 pt Calibri. The
//! engine works out where everything goes.
//!
//! Every struct is built with a constructor or `Default` and `with_*` setters, and read
//! with getters; enums are `#[non_exhaustive]`. The IR is exempt from semantic
//! versioning until officewright 1.0: it may change in any release.
//!
//! [`Document::validate`] rejects what no correct producer emits;
//! [`Document::clamp`] brings values a document may hold, but Word does not accept,
//! within Word's limits, and reports each change.

mod image;
mod inline;
mod list;
pub(crate) mod macros;
mod note;
mod paragraph;
mod rules;
mod section;
mod settings;
mod style;
mod table;
pub(crate) mod walk;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU64, Ordering};

pub use image::{
    AnchorHorizontalAlign, AnchorVerticalAlign, AnchoredObject, Crop, HorizontalAnchor,
    HorizontalPlacement, HorizontalReference, ImageData, ImageFormat, InlineObject, Insets,
    ObjectContent, Picture, Size, VerticalAnchor, VerticalPlacement, VerticalReference, Wrap,
};
pub use inline::{BreakClear, BreakKind, Field, FieldKind, Inline};
pub use list::{
    Level, LevelAlignment, LevelOverride, LevelTextPart, ListDefinition, ListInstance, ListItem,
    Restart, Suffix,
};
pub use note::{Note, NoteKind, NotePosition, NoteProps, NoteRestart, NoteSeparators};
pub use paragraph::{
    Alignment, Block, Border, BorderStyle, Indent, LineSpacing, Paragraph, ParagraphBorders,
    ParagraphProps, Shading, ShadingPattern, Spacing, StyleKey, TabKind, TabLeader, TabStop,
};
pub use section::{
    Column, Columns, HeaderFooterSet, Margins, NumberFormat, Orientation, PageNumbering, PageSetup,
    PageVerticalAlign, Section, SectionStart,
};
pub use settings::{CompatFlag, CompatMode, DocumentSettings, Hyphenation};
pub use style::{
    ColorValue, EastAsiaCharset, FontHint, Language, Ligatures, RunFonts, RunStyle, Underline,
    UnderlineKind, VerticalPosition,
};
pub use table::{
    BorderSide, Cell, CellBorders, CellMargins, CellVerticalAlign, PreferredWidth, Row, RowHeight,
    Table, TableAlignment, TableBorders, TableLayout, TableProps, TextDirection, VerticalMerge,
};
pub use walk::MAX_TABLE_DEPTH;

use crate::{LayoutReport, Result};
use walk::{Checker, Clamper};

/// A document in the layout IR: its settings, the run styles its text uses, its list
/// definitions and their instances, its images, its footnotes and endnotes, and its
/// sections.
///
/// ```
/// use officewright_core::{PageSize, Pt};
/// use officewright_layout::ir::{
///     Alignment, Document, Inline, PageSetup, Paragraph, ParagraphProps, RunFonts, RunStyle,
///     Section,
/// };
///
/// let mut document = Document::default();
/// let body = document.add_run_style(
///     RunStyle::default()
///         .with_fonts(RunFonts::new("Liberation Sans"))
///         .with_size(Pt(11.0)),
/// );
/// let paragraph = Paragraph::new(body)
///     .with_props(ParagraphProps::default().with_alignment(Alignment::Both))
///     .with_inlines([Inline::text("Hello, world", body)]);
/// document.add_section(Section::new(PageSetup::new(PageSize::A4)).with_blocks([paragraph.into()]));
/// document.validate()?;
/// assert!(document.clamp().is_empty());
/// # Ok::<(), officewright_layout::Error>(())
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct Document {
    /// What the ids of the entries this document adds carry.
    tag: DocumentTag,
    /// The documents this one is a clone of, the nearest last, each with how many entries
    /// each table had when it was cloned: their ids below those counts name the same
    /// entries here, and ids they made later name none.
    ancestors: Vec<(DocumentTag, TableCounts)>,
    settings: DocumentSettings,
    run_styles: Vec<RunStyle>,
    lists: Vec<ListDefinition>,
    list_instances: Vec<ListInstance>,
    images: Vec<ImageData>,
    notes: Vec<Note>,
    footnote_separators: NoteSeparators,
    endnote_separators: NoteSeparators,
    sections: Vec<Section>,
}

impl Document {
    /// An empty document with these settings.
    pub fn new(settings: DocumentSettings) -> Self {
        Self {
            tag: DocumentTag::next(),
            ancestors: Vec::new(),
            settings,
            run_styles: Vec::new(),
            lists: Vec::new(),
            list_instances: Vec::new(),
            images: Vec::new(),
            notes: Vec::new(),
            footnote_separators: NoteSeparators::default(),
            endnote_separators: NoteSeparators::default(),
            sections: Vec::new(),
        }
    }

    /// The settings.
    pub fn settings(&self) -> &DocumentSettings {
        &self.settings
    }

    /// Adds a run style and returns its id, which this document and the clones made of it
    /// afterwards accept.
    pub fn add_run_style(&mut self, style: RunStyle) -> RunStyleId {
        let index = self.run_styles.len();
        self.run_styles.push(style);
        RunStyleId {
            document: self.tag,
            index,
        }
    }

    /// The run style with this id; `None` for an id of another document.
    pub fn run_style(&self, id: RunStyleId) -> Option<&RunStyle> {
        self.entry(id.document, id.index, &self.run_styles, |counts| {
            counts.run_styles
        })
    }

    /// Adds a list definition and returns its id, which this document and the clones made
    /// of it afterwards accept.
    pub fn add_list(&mut self, list: ListDefinition) -> ListId {
        let index = self.lists.len();
        self.lists.push(list);
        ListId {
            document: self.tag,
            index,
        }
    }

    /// The list definition with this id; `None` for an id of another document.
    pub fn list(&self, id: ListId) -> Option<&ListDefinition> {
        self.entry(id.document, id.index, &self.lists, |counts| counts.lists)
    }

    /// Adds an instance of a list definition and returns its id, which this document and
    /// the clones made of it afterwards accept.
    pub fn add_list_instance(&mut self, instance: ListInstance) -> ListInstanceId {
        let index = self.list_instances.len();
        self.list_instances.push(instance);
        ListInstanceId {
            document: self.tag,
            index,
        }
    }

    /// The list instance with this id; `None` for an id of another document.
    pub fn list_instance(&self, id: ListInstanceId) -> Option<&ListInstance> {
        self.entry(id.document, id.index, &self.list_instances, |counts| {
            counts.list_instances
        })
    }

    /// Adds the data of an image and returns its id, which this document and the clones
    /// made of it afterwards accept.
    pub fn add_image(&mut self, image: ImageData) -> ImageId {
        let index = self.images.len();
        self.images.push(image);
        ImageId {
            document: self.tag,
            index,
        }
    }

    /// The image data with this id; `None` for an id of another document.
    pub fn image(&self, id: ImageId) -> Option<&ImageData> {
        self.entry(id.document, id.index, &self.images, |counts| counts.images)
    }

    /// Adds a footnote or an endnote and returns its id, which this document and the
    /// clones made of it afterwards accept.
    pub fn add_note(&mut self, note: Note) -> NoteId {
        let index = self.notes.len();
        self.notes.push(note);
        NoteId {
            document: self.tag,
            index,
        }
    }

    /// The note with this id; `None` for an id of another document.
    pub fn note(&self, id: NoteId) -> Option<&Note> {
        self.entry(id.document, id.index, &self.notes, |counts| counts.notes)
    }

    /// Sets the lines that separate the notes of a kind from the text.
    pub fn set_note_separators(&mut self, kind: NoteKind, separators: NoteSeparators) {
        match kind {
            NoteKind::Footnote => self.footnote_separators = separators,
            NoteKind::Endnote => self.endnote_separators = separators,
        }
    }

    /// The lines that separate the notes of a kind from the text.
    pub fn note_separators(&self, kind: NoteKind) -> &NoteSeparators {
        match kind {
            NoteKind::Footnote => &self.footnote_separators,
            NoteKind::Endnote => &self.endnote_separators,
        }
    }

    /// Adds a section after the last one.
    pub fn add_section(&mut self, section: Section) {
        self.sections.push(section);
    }

    /// The sections, in order.
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Checks that the document keeps the rules of the IR, which no correct producer
    /// breaks: every length is a finite number, lengths that WordprocessingML only has
    /// unsigned are not negative, every id is of this document, the text of runs holds no
    /// control characters (that of list labels may, see [`LevelTextPart::Literal`]),
    /// list levels are 0 to 8, tables have rows, rows have cells and cells
    /// have content, notes hold no note references, the document has a section. It does
    /// not change the document.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidDocument`](crate::Error::InvalidDocument) for the first broken
    /// rule, naming where it is; [`Error::NestingTooDeep`](crate::Error::NestingTooDeep)
    /// for tables nested deeper than [`MAX_TABLE_DEPTH`].
    ///
    /// ```
    /// use officewright_core::{PageSize, Pt};
    /// use officewright_layout::ir::{Document, PageSetup, RunStyle, Section};
    ///
    /// let mut document = Document::default();
    /// document.add_run_style(RunStyle::default().with_size(Pt(f64::NAN)));
    /// document.add_section(Section::new(PageSetup::new(PageSize::A4)));
    /// let err = document.validate().unwrap_err();
    /// assert_eq!(err.to_string(), "run_styles[0].size: NaN is not a finite length");
    /// ```
    pub fn validate(&self) -> Result<()> {
        let mut checker = Checker::new(self);
        checker.visit("settings", &self.settings)?;
        checker.visit("run_styles", &self.run_styles)?;
        checker.visit("lists", &self.lists)?;
        checker.visit("list_instances", &self.list_instances)?;
        checker.visit("images", &self.images)?;
        checker.set_in_note(true);
        checker.visit("notes", &self.notes)?;
        checker.visit("footnote_separators", &self.footnote_separators)?;
        checker.visit("endnote_separators", &self.endnote_separators)?;
        checker.set_in_note(false);
        if self.sections.is_empty() {
            return Err(checker.error("has no section"));
        }
        checker.visit("sections", &self.sections)
    }

    /// Brings the values that a document may hold, but Word does not accept, within
    /// Word's limits, such as a tab stop beyond 22 inches, and reports each change as
    /// [`ReportKind::ValueClamped`](crate::ReportKind::ValueClamped). A value that
    /// several items share, such as paragraph properties, is clamped and reported once,
    /// at the first item that holds it, and the others take the same result. A second
    /// call changes nothing. Values that [`validate`](Self::validate) rejects are left
    /// alone.
    ///
    /// ```
    /// use officewright_core::{PageSize, Pt};
    /// use officewright_layout::ir::{Document, Margins, PageSetup, Section};
    ///
    /// let mut document = Document::default();
    /// let page = PageSetup::new(PageSize::A4).with_margins(Margins::default().with_left(Pt(2000.0)));
    /// document.add_section(Section::new(page));
    /// let report = document.clamp();
    /// assert_eq!(report.len(), 2); // the margin, and the page that must be as wide as it
    /// assert_eq!(document.sections()[0].page().margins().left(), Pt(1584.0));
    /// assert!(document.clamp().is_empty());
    /// ```
    pub fn clamp(&mut self) -> LayoutReport {
        let mut clamper = Clamper::default();
        clamper.visit("settings", &mut self.settings);
        clamper.visit("run_styles", &mut self.run_styles);
        clamper.visit("lists", &mut self.lists);
        clamper.visit("list_instances", &mut self.list_instances);
        clamper.visit("images", &mut self.images);
        clamper.visit("notes", &mut self.notes);
        clamper.visit("footnote_separators", &mut self.footnote_separators);
        clamper.visit("endnote_separators", &mut self.endnote_separators);
        clamper.visit("sections", &mut self.sections);
        clamper.finish()
    }

    fn entry<'a, T>(
        &self,
        document: DocumentTag,
        index: usize,
        table: &'a [T],
        inherited: fn(&TableCounts) -> usize,
    ) -> Option<&'a T> {
        if !self.owns(document, index, inherited) {
            return None;
        }
        table.get(index)
    }

    /// Whether an id of `document` with `index` is one of this document's: one it made,
    /// or one a document it is a clone of made before it was cloned, below `inherited` of
    /// that document's counts then.
    fn owns(
        &self,
        document: DocumentTag,
        index: usize,
        inherited: fn(&TableCounts) -> usize,
    ) -> bool {
        document == self.tag
            || self
                .ancestors
                .iter()
                .any(|(tag, counts)| *tag == document && index < inherited(counts))
    }

    /// How many entries each table has.
    fn counts(&self) -> TableCounts {
        TableCounts {
            run_styles: self.run_styles.len(),
            lists: self.lists.len(),
            list_instances: self.list_instances.len(),
            images: self.images.len(),
            notes: self.notes.len(),
        }
    }

    /// Checks that an id of `what` names an entry of this document. An id it owns names
    /// one: its own ids are below the lengths its tables had when it made them, an
    /// ancestor's below the counts at the clone, and tables only grow.
    fn check_id(
        &self,
        document: DocumentTag,
        index: usize,
        inherited: fn(&TableCounts) -> usize,
        what: &str,
        checker: &Checker<'_>,
    ) -> Result<()> {
        if !self.owns(document, index, inherited) {
            return Err(checker.error(format!("the {what} id is of another document")));
        }
        Ok(())
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new(DocumentSettings::default())
    }
}

/// A clone holds the same entries under the same ids, but the entries it adds later get
/// ids of its own: an id the original makes after the clone is not the clone's, nor the
/// other way round, and [`Document::validate`] rejects it.
impl Clone for Document {
    fn clone(&self) -> Self {
        let mut ancestors = self.ancestors.clone();
        ancestors.push((self.tag, self.counts()));
        Self {
            tag: DocumentTag::next(),
            ancestors,
            settings: self.settings.clone(),
            run_styles: self.run_styles.clone(),
            lists: self.lists.clone(),
            list_instances: self.list_instances.clone(),
            images: self.images.clone(),
            notes: self.notes.clone(),
            footnote_separators: self.footnote_separators.clone(),
            endnote_separators: self.endnote_separators.clone(),
            sections: self.sections.clone(),
        }
    }
}

/// How many entries each table of a document has.
#[derive(Clone, Copy, Debug)]
struct TableCounts {
    run_styles: usize,
    lists: usize,
    list_instances: usize,
    images: usize,
    notes: usize,
}

/// Which document an id belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DocumentTag(u64);

impl DocumentTag {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Defines ids of the entries of a document's tables: each knows its document, so that
/// [`Document::validate`] rejects an id of another one.
macro_rules! document_ids {
    ($($(#[$meta:meta])* $name:ident in $table:ident, $what:literal;)*) => {
        $(
            $(#[$meta])*
            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
            pub struct $name {
                document: DocumentTag,
                index: usize,
            }

            impl walk::Check for $name {
                fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
                    let document = checker.document();
                    let inherited = (|counts: &TableCounts| counts.$table) as fn(&TableCounts) -> usize;
                    document.check_id(self.document, self.index, inherited, $what, checker)
                }
            }

            impl walk::Clamp for $name {
                fn clamp(&mut self, _: &mut Clamper) {}
            }
        )*
    };
}

document_ids! {
    /// A run style of a [`Document`], from [`Document::add_run_style`].
    RunStyleId in run_styles, "run style";
    /// A list definition of a [`Document`], from [`Document::add_list`].
    ListId in lists, "list definition";
    /// A list instance of a [`Document`], from [`Document::add_list_instance`].
    ListInstanceId in list_instances, "list instance";
    /// The data of an image of a [`Document`], from [`Document::add_image`].
    ImageId in images, "image";
    /// A footnote or an endnote of a [`Document`], from [`Document::add_note`].
    NoteId in notes, "note";
}

impl ImageId {
    /// Its place in the document's table of images, from 0. Only the display list's text
    /// dump uses it until the PDF painter (plan PR 8) does.
    #[cfg(feature = "fuzzing")]
    pub(crate) fn index(self) -> usize {
        self.index
    }
}
