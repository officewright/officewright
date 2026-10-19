//! The styles of a document (`styles.xml`, ECMA-376 Part 1, §17.7), as written.

use std::collections::HashMap;

use super::Document;
use super::properties::{ParagraphProperties, RunProperties, xml_enum};
use super::table_properties::{TableCellProperties, TableProperties, TableRowProperties};
use super::xml::{Element, Span};

/// The styles of a document as stored: each with its typed attributes and its element
/// kept whole, the document defaults and the latent styles.
#[derive(Debug, Default)]
pub(crate) struct StyleTable {
    pub(crate) styles: Vec<StyleData>,
    /// Index into `styles` by style id; the first style with an id wins.
    pub(crate) by_id: HashMap<Box<str>, usize>,
    /// `w:docDefaults`.
    pub(crate) defaults: Option<Span>,
    /// `w:latentStyles`, kept for writing; it describes styles Word offers in its user
    /// interface, not formatting.
    pub(crate) latent: Option<Span>,
}

#[derive(Debug)]
pub(crate) struct StyleData {
    pub(crate) kind: StyleKind,
    /// Shared, so that a format in effect names its style without copying the id.
    pub(crate) id: std::sync::Arc<str>,
    pub(crate) name: Option<Box<str>>,
    pub(crate) based_on: Option<Box<str>>,
    pub(crate) next: Option<Box<str>>,
    pub(crate) link: Option<Box<str>>,
    pub(crate) is_default: bool,
    pub(crate) is_custom: bool,
    /// The `w:style` element.
    pub(crate) element: Span,
}

/// What a style formats (`w:type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StyleKind {
    /// Paragraphs, and the runs in them.
    Paragraph,
    /// Runs.
    Character,
    /// Tables.
    Table,
    /// Lists: a numbering definition given a style's name.
    Numbering,
}

/// The styles of a document.
#[derive(Clone, Copy, Debug)]
pub struct Styles<'a> {
    document: &'a Document,
}

impl<'a> Styles<'a> {
    pub(super) fn new(document: &'a Document) -> Self {
        Self { document }
    }

    /// Every style, in the order of `styles.xml`.
    pub fn iter(self) -> impl ExactSizeIterator<Item = Style<'a>> + 'a {
        let document = self.document;
        document
            .styles
            .styles
            .iter()
            .map(move |data| Style { document, data })
    }

    /// The style with this id (`w:styleId`, compared exactly).
    pub fn get(self, id: &str) -> Option<Style<'a>> {
        let index = *self.document.styles.by_id.get(id)?;
        let data = self.document.styles.styles.get(index)?;
        Some(Style {
            document: self.document,
            data,
        })
    }

    /// The default style of a kind: what paragraphs, runs or tables that name no style
    /// of their own get. Of several of a kind marked `w:default`, the last, as the
    /// standard says ([MS-WPE376] Part 4 §2.7.3.17 quotes it) and Word's render of
    /// word-rules/E-default-paragraph-style-after-normal.docx shows for paragraph styles,
    /// even when Normal is one of them (#308).
    pub fn default_style(self, kind: StyleKind) -> Option<Style<'a>> {
        self.iter()
            .filter(|style| style.kind() == kind && style.is_default())
            .last()
    }

    /// The run properties of the document defaults (`w:docDefaults/w:rPrDefault`), the
    /// bottom of every run's formatting when they give any. Without a `w:rPr` there,
    /// Word gives runs defaults of its own, which [`Formats`](crate::Formats) applies.
    pub fn default_run_properties(self) -> RunProperties {
        let document = self.document;
        self.defaults(["rPrDefault", "rPr"])
            .map_or_else(RunProperties::default, |r_pr| {
                crate::docx::properties::run_properties(r_pr, &document.names, &mut |_, _, _| {})
            })
    }

    /// Whether the document defaults give run properties (a `w:rPr` in
    /// `w:docDefaults/w:rPrDefault`); Word gives its own when they do not.
    pub(crate) fn has_default_run_properties(self) -> bool {
        self.defaults(["rPrDefault", "rPr"]).is_some()
    }

    /// The paragraph properties of the document defaults
    /// (`w:docDefaults/w:pPrDefault`), the bottom of every paragraph's formatting.
    pub fn default_paragraph_properties(self) -> ParagraphProperties {
        let document = self.document;
        self.defaults(["pPrDefault", "pPr"])
            .map_or_else(ParagraphProperties::default, |p_pr| {
                crate::docx::properties::paragraph_properties(
                    p_pr,
                    &document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The element at `path` in `w:docDefaults`.
    fn defaults(self, path: [&str; 2]) -> Option<super::xml::Element<'a>> {
        let document = self.document;
        let mut element = document.element(document.styles.defaults?);
        for local in path {
            element = element
                .children_through_alternates(document.names.mc)
                .find(|child| child.is(&document.names.w, local))?;
        }
        Some(element)
    }
}

/// A style of a document (`w:style`).
#[derive(Clone, Copy, Debug)]
pub struct Style<'a> {
    document: &'a Document,
    data: &'a StyleData,
}

impl<'a> Style<'a> {
    pub(super) fn from_data(document: &'a Document, data: &'a StyleData) -> Self {
        Self { document, data }
    }

    /// The style id, by which paragraphs, runs and other styles name it (`w:styleId`).
    pub fn id(self) -> &'a str {
        &self.data.id
    }

    /// What the style formats.
    pub fn kind(self) -> StyleKind {
        self.data.kind
    }

    /// The name shown to users (`w:name`).
    pub fn name(self) -> Option<&'a str> {
        self.data.name.as_deref()
    }

    /// The id of the style this one inherits from (`w:basedOn`).
    pub fn based_on(self) -> Option<&'a str> {
        self.data.based_on.as_deref()
    }

    /// The id of the style of a paragraph started after one in this style (`w:next`).
    pub fn next(self) -> Option<&'a str> {
        self.data.next.as_deref()
    }

    /// The id of the paragraph or character style paired with this one (`w:link`).
    pub fn link(self) -> Option<&'a str> {
        self.data.link.as_deref()
    }

    /// Whether this is the default style of its kind (`w:default`).
    pub fn is_default(self) -> bool {
        self.data.is_default
    }

    /// Whether the style was made by a user rather than built in (`w:customStyle`).
    pub fn is_custom(self) -> bool {
        self.data.is_custom
    }

    /// The paragraph properties the style sets itself (its `w:pPr`), without those of
    /// the styles it is based on.
    pub fn paragraph_properties(self) -> ParagraphProperties {
        let document = self.document;
        self.child("pPr")
            .map_or_else(ParagraphProperties::default, |p_pr| {
                crate::docx::properties::paragraph_properties(
                    p_pr,
                    &document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The table properties a table style sets itself (its `w:tblPr`), without those of
    /// the styles it is based on or of its conditional formats.
    pub fn table_properties(self) -> TableProperties {
        self.child("tblPr")
            .map_or_else(TableProperties::default, |tbl_pr| {
                crate::docx::table_properties::table_properties(
                    tbl_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The row properties a table style sets itself (its `w:trPr`).
    pub fn row_properties(self) -> TableRowProperties {
        self.child("trPr")
            .map_or_else(TableRowProperties::default, |tr_pr| {
                crate::docx::table_properties::row_properties(
                    tr_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The cell properties a table style sets itself (its `w:tcPr`).
    pub fn cell_properties(self) -> TableCellProperties {
        self.child("tcPr")
            .map_or_else(TableCellProperties::default, |tc_pr| {
                crate::docx::table_properties::cell_properties(
                    tc_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The conditional formats of a table style (`w:tblStylePr`), in document order:
    /// what the style gives the parts of a table, such as its first row or every other
    /// band of rows, which the table's `w:tblLook` turns on. One of a type that is not
    /// one is left out.
    ///
    /// ```
    /// # fn main() -> officewright_doc::Result<()> {
    /// use officewright_doc::{Document, TableStyleCondition};
    ///
    /// let document = Document::open("tests/data/table-style.docx")?;
    /// let style = document.styles().get("Grid").expect("the style Grid");
    /// let formats: Vec<_> = style.conditional_formats().collect();
    /// assert_eq!(formats.len(), 1);
    /// assert_eq!(formats[0].condition(), TableStyleCondition::FirstRow);
    /// assert!(formats[0].cell_properties().shading().is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub fn conditional_formats(self) -> impl Iterator<Item = ConditionalFormat<'a>> + 'a {
        let document = self.document;
        document
            .element(self.data.element)
            .children_through_alternates(document.names.mc)
            .filter(|child| child.is(&document.names.w, "tblStylePr"))
            .filter_map(move |element| {
                let condition = element
                    .attribute(&document.names.w, "type")
                    .and_then(TableStyleCondition::from_xml)?;
                Some(ConditionalFormat {
                    document,
                    condition,
                    element,
                })
            })
    }

    /// The child of the `w:style` element with this local name.
    fn child(self, local: &str) -> Option<super::xml::Element<'a>> {
        let document = self.document;
        document
            .element(self.data.element)
            .children_through_alternates(document.names.mc)
            .find(|child| child.is(&document.names.w, local))
    }

    /// The run properties the style sets itself (its `w:rPr`), without those of the
    /// styles it is based on.
    pub fn run_properties(self) -> RunProperties {
        let document = self.document;
        self.child("rPr")
            .map_or_else(RunProperties::default, |r_pr| {
                crate::docx::properties::run_properties(r_pr, &document.names, &mut |_, _, _| {})
            })
    }
}

xml_enum! {
    /// The part of a table a conditional format of a table style applies to
    /// (ST_TblStyleOverrideType).
    TableStyleCondition {
        /// The whole table.
        WholeTable = "wholeTable",
        /// The first row.
        FirstRow = "firstRow",
        /// The last row.
        LastRow = "lastRow",
        /// The first column.
        FirstColumn = "firstCol",
        /// The last column.
        LastColumn = "lastCol",
        /// The odd bands of columns.
        OddColumnBand = "band1Vert",
        /// The even bands of columns.
        EvenColumnBand = "band2Vert",
        /// The odd bands of rows.
        OddRowBand = "band1Horz",
        /// The even bands of rows.
        EvenRowBand = "band2Horz",
        /// The top right cell.
        TopRightCell = "neCell",
        /// The top left cell.
        TopLeftCell = "nwCell",
        /// The bottom right cell.
        BottomRightCell = "seCell",
        /// The bottom left cell.
        BottomLeftCell = "swCell",
    }
}

/// A conditional format of a table style (`w:tblStylePr`): the properties it gives the
/// part of a table its condition names, as written.
#[derive(Clone, Copy, Debug)]
pub struct ConditionalFormat<'a> {
    document: &'a Document,
    condition: TableStyleCondition,
    element: Element<'a>,
}

impl<'a> ConditionalFormat<'a> {
    /// The part of the table it applies to.
    pub fn condition(self) -> TableStyleCondition {
        self.condition
    }

    /// The child of the `w:tblStylePr` element with this local name.
    fn child(self, local: &str) -> Option<Element<'a>> {
        let names = &self.document.names;
        self.element
            .children_through_alternates(names.mc)
            .find(|child| child.is(&names.w, local))
    }

    /// The paragraph properties it gives (its `w:pPr`).
    pub fn paragraph_properties(self) -> ParagraphProperties {
        self.child("pPr")
            .map_or_else(ParagraphProperties::default, |p_pr| {
                crate::docx::properties::paragraph_properties(
                    p_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The run properties it gives (its `w:rPr`).
    pub fn run_properties(self) -> RunProperties {
        self.child("rPr")
            .map_or_else(RunProperties::default, |r_pr| {
                crate::docx::properties::run_properties(
                    r_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The table properties it gives (its `w:tblPr`).
    pub fn table_properties(self) -> TableProperties {
        self.child("tblPr")
            .map_or_else(TableProperties::default, |tbl_pr| {
                crate::docx::table_properties::table_properties(
                    tbl_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The row properties it gives (its `w:trPr`).
    pub fn row_properties(self) -> TableRowProperties {
        self.child("trPr")
            .map_or_else(TableRowProperties::default, |tr_pr| {
                crate::docx::table_properties::row_properties(
                    tr_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            })
    }

    /// The cell properties it gives (its `w:tcPr`).
    pub fn cell_properties(self) -> TableCellProperties {
        self.child("tcPr")
            .map_or_else(TableCellProperties::default, |tc_pr| {
                crate::docx::table_properties::cell_properties(
                    tc_pr,
                    &self.document.names,
                    &mut |_, _, _| {},
                )
            })
    }
}
