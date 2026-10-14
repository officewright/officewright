//! What a layout run did differently from Word: every substitution, omission and clamp,
//! so that users know why the output differs.

use std::fmt;

/// The entries a layout run produced, in the order it produced them.
///
/// ```
/// use officewright_core::{PageSize, Pt};
/// use officewright_layout::ReportKind;
/// use officewright_layout::ir::{Document, PageSetup, Paragraph, RunStyle, Section};
///
/// let mut document = Document::default();
/// // Word's character scale goes up to 600 %.
/// let style = document.add_run_style(RunStyle::default().with_scale(900));
/// document.add_section(
///     Section::new(PageSetup::new(PageSize::A4)).with_blocks([Paragraph::new(style).into()]),
/// );
/// let report = document.clamp();
/// assert_eq!(report.len(), 1);
/// let entry = &report.entries()[0];
/// assert_eq!(entry.kind(), ReportKind::ValueClamped);
/// assert_eq!(entry.location().to_string(), "run_styles[0].scale");
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutReport {
    entries: Vec<ReportEntry>,
}

impl LayoutReport {
    /// The entries.
    pub fn entries(&self) -> &[ReportEntry] {
        &self.entries
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, ReportEntry> {
        self.entries.iter()
    }

    pub(crate) fn push(&mut self, kind: ReportKind, location: Location, message: String) {
        self.entries.push(ReportEntry {
            kind,
            location,
            message,
        });
    }
}

impl Extend<ReportEntry> for LayoutReport {
    fn extend<T: IntoIterator<Item = ReportEntry>>(&mut self, entries: T) {
        self.entries.extend(entries);
    }
}

impl FromIterator<ReportEntry> for LayoutReport {
    fn from_iter<T: IntoIterator<Item = ReportEntry>>(entries: T) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }
}

impl IntoIterator for LayoutReport {
    type Item = ReportEntry;
    type IntoIter = std::vec::IntoIter<ReportEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a LayoutReport {
    type Item = &'a ReportEntry;
    type IntoIter = std::slice::Iter<'a, ReportEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// One thing the layout did differently from Word, and where.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ReportEntry {
    kind: ReportKind,
    location: Location,
    message: String,
}

impl ReportEntry {
    /// An entry of `kind` at `location`, saying `message`: for a producer of the layout IR
    /// that reports what it leaves out, so that its entries and the layout's make one
    /// report. Messages follow the crate's: lowercase, without a full stop at the end.
    ///
    /// ```
    /// use officewright_layout::{LayoutReport, Location, ReportEntry, ReportKind};
    ///
    /// let place = Location::document().field("sections").index(0).field("blocks").index(3);
    /// let entry = ReportEntry::new(ReportKind::FeatureUnsupported, place, "a chart is left out");
    /// let report: LayoutReport = [entry].into_iter().collect();
    /// assert_eq!(report.entries()[0].location().to_string(), "sections[0].blocks[3]");
    /// ```
    pub fn new(kind: ReportKind, location: Location, message: impl Into<String>) -> Self {
        Self {
            kind,
            location,
            message: message.into(),
        }
    }

    /// What kind of difference it is.
    pub fn kind(&self) -> ReportKind {
        self.kind
    }

    /// Where in the document.
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// What happened, in words.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ReportEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

/// The kinds of [`ReportEntry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReportKind {
    /// A font is not available and another one is used in its place.
    FontSubstituted,
    /// A font is embedded with what its license or its format restricts: for viewing
    /// and printing only, for editing but not installing, whole rather than as a subset,
    /// or only the default instance of a variable font; or it cannot be embedded, and its
    /// text is not drawn.
    FontEmbedding,
    /// No available font has a glyph for a character; the font's missing-glyph shape
    /// is drawn.
    GlyphMissing,
    /// Something the document uses is not laid out yet, or not the way Word does: text
    /// wrapping around a floating object, an image format, a floating table.
    FeatureUnsupported,
    /// A field whose value is not computed shows the result stored in the document.
    FieldCached,
    /// A legacy compatibility option is set that the layout does not follow.
    CompatIgnored,
    /// A value is beyond what Word accepts, or what a PDF holds, and was brought within
    /// its limits.
    ValueClamped,
    /// Content does not fit its area, such as a word wider than its column.
    Overflow,
}

/// A place in the layout IR, written as the path of fields and list positions from the
/// document to the value, such as `sections[0].blocks[3].props.tabs[1].position`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Location {
    steps: Vec<Step>,
}

/// One step of a [`Location`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Step {
    /// A field of a struct.
    Field(&'static str),
    /// A position in a list, from 0.
    Index(usize),
}

impl Location {
    /// The document as a whole, which a producer extends with [`field`](Self::field) and
    /// [`index`](Self::index) to name a place in it.
    pub fn document() -> Self {
        Self::default()
    }

    /// Field `name` of the value at this place.
    pub fn field(mut self, name: &'static str) -> Self {
        self.steps.push(Step::Field(name));
        self
    }

    /// Position `index`, from 0, of the list at this place.
    pub fn index(mut self, index: usize) -> Self {
        self.steps.push(Step::Index(index));
        self
    }

    pub(crate) fn push(&mut self, step: Step) {
        self.steps.push(step);
    }

    pub(crate) fn pop(&mut self) {
        self.steps.pop();
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.steps.is_empty() {
            return f.write_str("document");
        }
        for (position, step) in self.steps.iter().enumerate() {
            match step {
                Step::Field(name) if position == 0 => f.write_str(name)?,
                Step::Field(name) => write!(f, ".{name}")?,
                Step::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}
