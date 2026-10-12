use officewright_core::{PageSize, Pt};

/// The properties of a section as written in the document (`w:sectPr`): page size,
/// orientation, margins and how the section starts. A value the document does not give
/// is `None`.
#[doc(alias = "PageSetup")]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SectionProperties {
    pub(crate) page_width: Option<Pt>,
    pub(crate) page_height: Option<Pt>,
    pub(crate) orientation: Option<Orientation>,
    pub(crate) margins: PageMargins,
    pub(crate) start: Option<SectionStart>,
}

impl SectionProperties {
    /// The page size, when the document gives both width and height.
    pub fn page_size(&self) -> Option<PageSize> {
        Some(PageSize::new(self.page_width?, self.page_height?))
    }

    /// The page width as written.
    pub fn page_width(&self) -> Option<Pt> {
        self.page_width
    }

    /// The page height as written.
    pub fn page_height(&self) -> Option<Pt> {
        self.page_height
    }

    /// The orientation as written (`w:orient`).
    pub fn orientation(&self) -> Option<Orientation> {
        self.orientation
    }

    /// The page margins as written.
    pub fn margins(&self) -> &PageMargins {
        &self.margins
    }

    /// How the section starts relative to the one before (`w:type`).
    pub fn start(&self) -> Option<SectionStart> {
        self.start
    }
}

/// The page margins of a section (`w:pgMar`), as written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageMargins {
    pub(crate) top: Option<Pt>,
    pub(crate) bottom: Option<Pt>,
    pub(crate) left: Option<Pt>,
    pub(crate) right: Option<Pt>,
    pub(crate) header: Option<Pt>,
    pub(crate) footer: Option<Pt>,
    pub(crate) gutter: Option<Pt>,
}

impl PageMargins {
    /// From the top edge of the page to the text. A negative value means the text
    /// starts there even when the header is taller.
    pub fn top(&self) -> Option<Pt> {
        self.top
    }

    /// From the bottom edge of the page to the text. A negative value means the text
    /// ends there even when the footer is taller.
    pub fn bottom(&self) -> Option<Pt> {
        self.bottom
    }

    /// From the left edge of the page to the text.
    pub fn left(&self) -> Option<Pt> {
        self.left
    }

    /// From the right edge of the page to the text.
    pub fn right(&self) -> Option<Pt> {
        self.right
    }

    /// From the top edge of the page to the header.
    pub fn header(&self) -> Option<Pt> {
        self.header
    }

    /// From the bottom edge of the page to the footer.
    pub fn footer(&self) -> Option<Pt> {
        self.footer
    }

    /// Extra space for binding, added to the left margin (or the top one, depending on
    /// the document settings).
    pub fn gutter(&self) -> Option<Pt> {
        self.gutter
    }
}

/// The orientation of the pages of a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Orientation {
    /// Taller than wide.
    Portrait,
    /// Wider than tall.
    Landscape,
}

/// How a section starts relative to the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SectionStart {
    /// On a new page.
    NextPage,
    /// On the same page.
    Continuous,
    /// On the next even page.
    EvenPage,
    /// On the next odd page.
    OddPage,
    /// In the next column.
    NextColumn,
}
