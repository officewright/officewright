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

mod inline;
mod list;
pub(crate) mod macros;
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

pub use inline::{BreakClear, BreakKind, Field, FieldKind, Inline};
pub use list::{
    Level, LevelAlignment, LevelOverride, LevelTextPart, ListDefinition, ListInstance, ListItem,
    Restart, Suffix,
};
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
/// definitions and their instances, and its sections.
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
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Document {
    tag: DocumentTag,
    settings: DocumentSettings,
    run_styles: Vec<RunStyle>,
    lists: Vec<ListDefinition>,
    list_instances: Vec<ListInstance>,
    sections: Vec<Section>,
}

impl Document {
    /// An empty document with these settings.
    pub fn new(settings: DocumentSettings) -> Self {
        Self {
            tag: DocumentTag::next(),
            settings,
            run_styles: Vec::new(),
            lists: Vec::new(),
            list_instances: Vec::new(),
            sections: Vec::new(),
        }
    }

    /// The settings.
    pub fn settings(&self) -> &DocumentSettings {
        &self.settings
    }

    /// Adds a run style and returns its id, which only this document and its clones
    /// accept.
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
        self.entry(id.document, id.index, &self.run_styles)
    }

    /// Adds a list definition and returns its id, which only this document and its
    /// clones accept.
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
        self.entry(id.document, id.index, &self.lists)
    }

    /// Adds an instance of a list definition and returns its id, which only this
    /// document and its clones accept.
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
        self.entry(id.document, id.index, &self.list_instances)
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
    /// unsigned are not negative, every id is of this document, text holds no control
    /// characters, list levels are 0 to 8, tables have rows, rows have cells and cells
    /// have content, the document has a section. It does not change the document.
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
        if self.sections.is_empty() {
            return Err(checker.error("has no section"));
        }
        checker.visit("sections", &self.sections)
    }

    /// Brings the values that a document may hold, but Word does not accept, within
    /// Word's limits, such as a tab stop beyond 22 inches, and reports each change as
    /// [`ReportKind::ValueClamped`](crate::ReportKind::ValueClamped). A second call
    /// changes nothing. Values that [`validate`](Self::validate) rejects are left alone.
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
        clamper.visit("sections", &mut self.sections);
        clamper.finish()
    }

    fn entry<'a, T>(&self, document: DocumentTag, index: usize, table: &'a [T]) -> Option<&'a T> {
        if document != self.tag {
            return None;
        }
        table.get(index)
    }

    /// Checks that an id of `what` names an entry of this document, whose table has
    /// `count` entries.
    fn check_id(
        &self,
        document: DocumentTag,
        index: usize,
        count: usize,
        what: &str,
        checker: &Checker<'_>,
    ) -> Result<()> {
        if document != self.tag {
            return Err(checker.error(format!("the {what} id is of another document")));
        }
        if index >= count {
            return Err(checker.error(format!(
                "{what} {index} does not exist; the document has {count}"
            )));
        }
        Ok(())
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new(DocumentSettings::default())
    }
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
                    let count = document.$table.len();
                    document.check_id(self.document, self.index, count, $what, checker)
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
}
