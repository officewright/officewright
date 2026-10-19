//! Resolving the formatting in effect (ECMA-376 Part 1, §17.7.2): the document defaults,
//! in a cell what the table's style gives, numbering, the paragraph's style, the run's
//! character style and direct formatting, combined as Word combines them.

mod character;
mod paragraph;
mod table;

pub(crate) use paragraph::list_level;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::format::{CharacterFormat, ParagraphFormat};
use super::properties::ThemeFont;
use super::styles::StyleKind;
use super::views::{Paragraph, Run};
use super::{Document, Kind, Node};

use character::CharacterLevel;
use paragraph::ParagraphLevel;

/// Styles whose child elements Word ignores, `w:basedOn`, `w:pPr` and `w:rPr` among
/// them (MS-OI29500 §17.7.4.17(a)).
const IGNORED_STYLES: [&str; 3] = ["NoList", "DefaultParagraphFont", "TableNormal"];

/// Resolves the formatting in effect of paragraphs, runs, paragraph marks and list
/// labels, keeping what it has worked out about styles for the next ones: use one for a
/// whole document. It borrows the document; threads that format at the same time each
/// use their own.
#[derive(Debug)]
pub struct Formats<'a> {
    document: &'a Document,
    /// What each style gives to runs together with the styles it is based on.
    characters: StyleLevels<CharacterLevel>,
    /// What each paragraph style gives to paragraphs together with the styles it is
    /// based on.
    paragraphs: StyleLevels<ParagraphLevel>,
    character_defaults: Option<Arc<CharacterLevel>>,
    paragraph_defaults: Option<Arc<ParagraphLevel>>,
    /// The typeface each theme font stands for, once looked up.
    theme_fonts: HashMap<ThemeFont, Option<Arc<str>>>,
    /// Word's font when nothing gives one, shared by every run that has it.
    default_font: Arc<str>,
    /// The index of the default paragraph style, once looked up.
    default_paragraph_style: Option<Option<usize>>,
    /// The definition each list instance named so far uses.
    instances: HashMap<i64, Option<Arc<paragraph::InstanceDefinition>>>,
    /// The definition each definition an instance names leads to through
    /// `w:numStyleLink`.
    links: HashMap<i64, Option<i64>>,
    /// The list instance each numbering style named by a link so far names.
    numbering_styles: HashMap<usize, Option<i64>>,
    /// The parsed levels of each list definition used so far, which its instances
    /// share.
    definitions: HashMap<i64, Arc<paragraph::DefinitionLevels>>,
    /// What each level of a definition that paragraphs use gives to them and to their
    /// labels.
    level_layers: HashMap<(i64, usize), paragraph::LevelGives>,
    /// Each list instance's levels that paragraphs use, with the instance's overrides,
    /// and the numbers they start from.
    instance_levels: HashMap<(i64, usize), paragraph::InstanceLevel>,
    /// What each table style gives with the styles it is based on.
    table_styles: StyleLevels<table::TableStyleLevel>,
    /// What is worked out once per table, by the table's node.
    tables: HashMap<u32, Arc<table::TableContext>>,
    /// The table each row and cell of a table worked out so far belongs to.
    table_of: HashMap<u32, u32>,
    /// The index of the default table style, once looked up.
    default_table_style: Option<Option<usize>>,
    /// The defaults and a paragraph style with a list level laid over them, by the
    /// style, where the level comes from and the table layer below the style, if any; at
    /// most [`MAX_COMBINATIONS`] at a time.
    combinations:
        HashMap<(Option<usize>, paragraph::LayerSource, Option<u64>), Arc<ParagraphLevel>>,
    /// What table styles give the paragraphs and runs of cells, by the table style and
    /// the set of conditions of the cells.
    table_texts: HashMap<(usize, u16), Arc<table::TableText>>,
    /// The id the last table text got.
    next_table_text: u64,
    /// Over each table text, by its id: the paragraph defaults with it laid over them,
    /// and the default paragraph style's level once its justification gave way.
    cell_paragraphs: HashMap<u64, CellParagraphs>,
    /// What each paragraph style gives with the styles it is based on, before anything
    /// lies below it: its cleared tab stops kept, to clear those of the table text a
    /// cell's paragraph lies on. Worked out once per style, whatever the number of table
    /// texts.
    paragraph_chains: StyleLevels<ParagraphLevel>,
    /// A paragraph style's chain laid over a table text, by the table text's id and the
    /// style; at most [`MAX_CELL_LEVELS`] at a time.
    cell_levels: HashMap<(u64, usize), Arc<ParagraphLevel>>,
    /// Whether a size of 11 or 12 pt and a left justification that the default paragraph
    /// style gives give way to a table style's ([MS-DOCX] 2.3.x
    /// `overrideTableStyleFontSizeAndJustification` off), once worked out.
    table_style_wins: Option<bool>,
    /// How much work the caches saved, for tests.
    #[cfg(test)]
    work: Work,
}

/// What lies under the paragraphs of cells of one table text: the paragraph defaults with
/// it laid over them, and the default paragraph style's level once its justification gave
/// way to the table style's.
type CellParagraphs = (Arc<ParagraphLevel>, Option<Arc<ParagraphLevel>>);

/// How many paragraph style chains laid over table texts are kept at a time: each is one
/// level, so the bound keeps them small whatever the number of table texts and styles.
const MAX_CELL_LEVELS: usize = 4096;

/// How many times each kind of work was done, for tests that the caches are used.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Work {
    /// Definitions whose link (`w:numStyleLink`) was read.
    pub(crate) links: usize,
    /// Numbering styles whose list reference was read.
    pub(crate) numbering_styles: usize,
    /// Definitions whose levels were parsed.
    pub(crate) definitions: usize,
    /// Levels of definitions parsed for what they give to paragraphs and labels.
    pub(crate) level_layers: usize,
    /// Levels of instances replaced by an override's level.
    pub(crate) replacements: usize,
    /// A style's level and a list level laid over each other.
    pub(crate) combinations: usize,
    /// Tables whose rows and cells were worked out.
    pub(crate) tables: usize,
    /// Cells whose properties in effect were worked out.
    pub(crate) cells: usize,
    /// What a table style gives paragraphs and runs worked out for a set of conditions.
    pub(crate) table_texts: usize,
    /// What a paragraph style writes itself read for the style chains in cells.
    pub(crate) cell_style_reads: usize,
    /// Paragraph style chains laid over a table text.
    pub(crate) cell_levels: usize,
}

impl<'a> Formats<'a> {
    pub(super) fn new(document: &'a Document) -> Self {
        Self {
            document,
            characters: StyleLevels::default(),
            paragraphs: StyleLevels::default(),
            character_defaults: None,
            paragraph_defaults: None,
            theme_fonts: HashMap::new(),
            default_font: Arc::from(character::DEFAULT_FONT),
            default_paragraph_style: None,
            instances: HashMap::new(),
            links: HashMap::new(),
            numbering_styles: HashMap::new(),
            definitions: HashMap::new(),
            level_layers: HashMap::new(),
            instance_levels: HashMap::new(),
            combinations: HashMap::new(),
            table_texts: HashMap::new(),
            next_table_text: 0,
            cell_paragraphs: HashMap::new(),
            paragraph_chains: StyleLevels::unsettled(),
            cell_levels: HashMap::new(),
            table_style_wins: None,
            table_styles: StyleLevels::default(),
            tables: HashMap::new(),
            table_of: HashMap::new(),
            default_table_style: None,
            #[cfg(test)]
            work: Work::default(),
        }
    }

    /// The formatting of a paragraph in effect.
    pub fn paragraph(&mut self, paragraph: Paragraph<'a>) -> ParagraphFormat {
        if !std::ptr::eq(paragraph.document(), self.document) {
            // A paragraph of another document takes that document's styles.
            return Formats::new(paragraph.document()).paragraph(paragraph);
        }
        let style = self.paragraph_style(Some(paragraph.node()));
        let table = self.table_text(paragraph.node());
        self.resolve_paragraph(style, &paragraph.properties(), table.as_deref())
    }

    /// The formatting of a run in effect.
    pub fn run(&mut self, run: Run<'a>) -> CharacterFormat {
        if !std::ptr::eq(run.document(), self.document) {
            return Formats::new(run.document()).run(run);
        }
        let direct = run.properties();
        let paragraph = paragraph_of(self.document, run.node());
        let table = paragraph.and_then(|paragraph| self.table_text(paragraph));
        let style = self.paragraph_style(paragraph);
        self.resolve_run(style, &direct, table.as_deref(), None)
    }

    /// The formatting of a paragraph's mark in effect, which decides, among other
    /// things, the height of an empty paragraph.
    pub fn paragraph_mark(&mut self, paragraph: Paragraph<'a>) -> CharacterFormat {
        if !std::ptr::eq(paragraph.document(), self.document) {
            return Formats::new(paragraph.document()).paragraph_mark(paragraph);
        }
        let direct = paragraph.properties().mark;
        let style = self.paragraph_style(Some(paragraph.node()));
        let table = self.table_text(paragraph.node());
        self.resolve_run(style, &direct, table.as_deref(), None)
    }

    /// The formatting in effect of the mark of a paragraph that has the default
    /// paragraph style and no properties of its own, such as the empty paragraph the
    /// translation into the layout IR adds to a cell that does not end with one.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn default_mark(&mut self) -> CharacterFormat {
        let style = self.paragraph_style(None);
        self.resolve_run(
            style,
            &super::properties::RunProperties::default(),
            None,
            None,
        )
    }

    /// The formatting of a list item's label, its number or bullet, in effect: the
    /// paragraph mark's, with the run properties of the list level laid over it, whose
    /// on/off values set the value as direct formatting does. Word does not allow the
    /// level's `w:rStyle`, `w:highlight`, `w:oMath` and `w:rPrChange` there (MS-OI29500
    /// §17.9.24(a)); a file that has them is read, and they are dropped. None when the
    /// paragraph is not a list item.
    ///
    /// Not confirmed against Word yet: [MS-DOC] ("Determining List Formatting of a
    /// Paragraph", part 3) lays the level over the mark this way, and the reference
    /// renders of the rule documents asked for in issue #162 are to confirm it. The
    /// level's linked paragraph style, which [MS-DOC] applies too, does not reach the
    /// label.
    ///
    /// What a level gives to labels is read once per level, however many paragraphs
    /// use it.
    pub fn list_label(&mut self, paragraph: Paragraph<'a>) -> Option<CharacterFormat> {
        if !std::ptr::eq(paragraph.document(), self.document) {
            return Formats::new(paragraph.document()).list_label(paragraph);
        }
        let properties = paragraph.properties();
        let style = self.paragraph_style(Some(paragraph.node()));
        let label = self.label_level(style, &properties)?;
        let table = self.table_text(paragraph.node());
        Some(self.resolve_run(style, &properties.mark, table.as_deref(), label.as_deref()))
    }

    /// The place among the styles of a paragraph's style: its own, else the default
    /// paragraph style; none for a paragraph of another document. For the translation
    /// into the layout IR, which keys contextual spacing by it.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn paragraph_style_index(&mut self, paragraph: Paragraph<'a>) -> Option<usize> {
        if !std::ptr::eq(paragraph.document(), self.document) {
            return None;
        }
        self.paragraph_style(Some(paragraph.node()))
    }

    /// How many times a style's own properties were read, for tests of the work done.
    #[cfg(test)]
    pub(crate) fn style_reads(&self) -> usize {
        self.characters.reads + self.paragraphs.reads
    }

    /// How much other work was done, for tests that the caches are used.
    #[cfg(test)]
    pub(crate) fn work(&self) -> &Work {
        &self.work
    }

    /// How many table texts, paragraph defaults laid under styles in cells, style chains
    /// and those chains laid over table texts are kept, for tests of the bounds.
    #[cfg(test)]
    pub(crate) fn kept_table_texts(&self) -> (usize, usize, usize, usize) {
        (
            self.table_texts.len(),
            self.cell_paragraphs.len(),
            self.paragraph_chains.levels.len(),
            self.cell_levels.len(),
        )
    }

    /// How many combinations of a style and a list level are kept, for tests of the
    /// bound.
    #[cfg(test)]
    pub(crate) fn kept_combinations(&self) -> usize {
        self.combinations.len()
    }

    /// The index of the paragraph style of the paragraph `node`: the one worked out when
    /// the document was read (see [`assign_paragraph_styles`]), else the default
    /// paragraph style (the last, as
    /// [`Styles::default_style`](super::Styles::default_style) gives it).
    fn paragraph_style(&mut self, node: Option<&'a Node>) -> Option<usize> {
        let written = node.and_then(|node| match node.kind {
            Kind::Paragraph { style } => style,
            _ => None,
        });
        let document = self.document;
        written
            .and_then(|index| usize::try_from(index).ok())
            .or_else(|| {
                *self
                    .default_paragraph_style
                    .get_or_insert_with(|| default_paragraph_style(document))
            })
    }
}

/// The default paragraph style: the last paragraph style marked default.
fn default_paragraph_style(document: &Document) -> Option<usize> {
    document
        .styles
        .styles
        .iter()
        .take(MAX_STYLES)
        .rposition(|style| style.kind == StyleKind::Paragraph && style.is_default)
}

/// Whether the style at `index` is the default paragraph style, whose values equal to
/// the document defaults' Word does not keep as its own: it drops them from the copy it
/// saves, and draws the cells of word-rules/E-table-style-size-*.docx as if the style did
/// not give them, in the table style's size and justification whatever the setting
/// `overrideTableStyleFontSizeAndJustification` says (Normal's `w:sz` equal to the
/// defaults' and its `w:jc left`, #297). Outside tables nothing changes: the defaults give
/// the same value.
fn drops_values_equal_to_defaults(document: &Document, index: usize) -> bool {
    document
        .styles
        .styles
        .get(index)
        .is_some_and(|style| style.kind == StyleKind::Paragraph && style.is_default)
        && default_paragraph_style(document) == Some(index)
}

/// Works out once, after the styles are read, the paragraph style of every paragraph
/// whose `w:pStyle` names one the resolver uses, so that a run's formatting never reads
/// its paragraph's `w:pPr`; a paragraph's own formatting and its mark's read only the
/// properties of that paragraph. The `w:pStyle` is read as
/// [`Paragraph::properties`] reads it: through `mc:AlternateContent`, the last of several
/// winning. Each paragraph's own `w:pPr` is walked once, so the work grows with the size
/// of the document.
pub(crate) fn assign_paragraph_styles(document: &mut Document) {
    for index in 0..document.nodes.len() {
        let Some(node) = document.nodes.get(index) else {
            continue;
        };
        if !matches!(node.kind, Kind::Paragraph { .. }) {
            continue;
        }
        let names = &document.names;
        let style = node
            .properties
            .iter()
            .map(|&span| document.element(span))
            .find(|element| element.is(&names.w, "pPr"))
            .and_then(|p_pr| {
                p_pr.children_through_alternates(names.mc)
                    .filter(|child| child.is(&names.w, "pStyle"))
                    .last()
            })
            .and_then(|p_style| p_style.attribute(&names.w, "val"))
            .and_then(|id| style_index(document, id, StyleKind::Paragraph))
            .and_then(|index| u32::try_from(index).ok());
        if let Some(Node {
            kind: Kind::Paragraph { style: slot },
            ..
        }) = document.nodes.get_mut(index)
        {
            *slot = style;
        }
    }
}

/// What one level of the style hierarchy gives, for runs or for paragraphs.
trait Layer: Clone + Default {
    /// What the style at `index` writes itself; nothing for the styles whose children
    /// Word ignores.
    fn of_style(document: &Document, index: usize) -> Self;

    /// Lays `above` over this level: what it gives replaces what this one gives.
    fn overlay(&mut self, above: &Self);

    /// Drops what only a level below could use, once this level lies over the root.
    fn settle(&mut self) {}
}

/// What each style gives together with the styles it is based on, by the style's index
/// in the document's styles: one entry per style, whatever the length of the chains.
#[derive(Debug)]
struct StyleLevels<L> {
    levels: HashMap<usize, Arc<L>>,
    /// Whether each level is settled, as it is once it lies over the root; not for chains
    /// that are laid over another level later.
    settles: bool,
    /// How many times a style's own properties were read.
    #[cfg(test)]
    reads: usize,
}

impl<L> Default for StyleLevels<L> {
    fn default() -> Self {
        Self {
            levels: HashMap::new(),
            settles: true,
            #[cfg(test)]
            reads: 0,
        }
    }
}

impl<L> StyleLevels<L> {
    /// Levels that are not settled, for chains laid over another level later.
    fn unsettled() -> Self {
        Self {
            settles: false,
            ..Self::default()
        }
    }
}

impl<L: Layer> StyleLevels<L> {
    /// What the style at `index` gives together with the styles it is based on, laid
    /// over `root`, the style's own properties winning (ECMA-376 §17.7.4.3; for toggle
    /// properties the first value found up the chain, §17.7.3). The root is the same for
    /// every call on one cache.
    ///
    /// Each style is worked out once, from the style it is based on. A style on a cycle
    /// of `w:basedOn`, which Word does not allow (MS-OI29500 §17.7.4.3(a)), is taken
    /// with its own properties only, so that the result does not depend on which style
    /// of the cycle is asked for first.
    fn level(&mut self, document: &Document, index: usize, root: &L) -> Arc<L> {
        if let Some(level) = self.levels.get(&index) {
            return Arc::clone(level);
        }
        // The styles up the chain that are not worked out yet, the asked one first, up
        // to one that is, the root, or the first style met twice.
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        let mut current = Some(index);
        let mut base = None;
        let mut cycle_from = None;
        while let Some(style) = current {
            if let Some(level) = self.levels.get(&style) {
                base = Some(Arc::clone(level));
                break;
            }
            if !seen.insert(style) {
                cycle_from = path.iter().position(|&other| other == style);
                break;
            }
            path.push(style);
            current = parent(document, style);
        }
        // The styles of the cycle, if any, are at the end of `path`.
        let cycle: &[usize] = cycle_from
            .and_then(|from| path.get(from..))
            .unwrap_or_default();
        for &style in cycle {
            let mut level = root.clone();
            level.overlay(&self.own(document, style));
            if self.settles {
                level.settle();
            }
            self.levels.insert(style, Arc::new(level));
        }
        let tail = path
            .get(..cycle_from.unwrap_or(path.len()))
            .unwrap_or_default();
        let mut below = base.or_else(|| {
            let first = cycle.first()?;
            self.levels.get(first).map(Arc::clone)
        });
        for &style in tail.iter().rev() {
            let mut level = below.as_deref().unwrap_or(root).clone();
            level.overlay(&self.own(document, style));
            if self.settles {
                level.settle();
            }
            let level = Arc::new(level);
            self.levels.insert(style, Arc::clone(&level));
            below = Some(level);
        }
        self.levels
            .get(&index)
            .map_or_else(Arc::default, Arc::clone)
    }

    /// What the style at `index` writes itself.
    fn own(&mut self, document: &Document, index: usize) -> L {
        #[cfg(test)]
        {
            self.reads += 1;
        }
        let ignored = document
            .styles
            .styles
            .get(index)
            .is_none_or(|style| IGNORED_STYLES.contains(&&*style.id));
        if ignored {
            L::default()
        } else {
            L::of_style(document, index)
        }
    }
}

/// How many styles the resolver uses, the first ones of the styles part: as many as
/// Word's binary format has entries for, fewer than 0x0FFE ([MS-DOC] 2.9.274
/// Stshif.cstd), some of which may be empty. MS-OI29500 §17.7.4.17(c) says that Word
/// does not open a document with more than 4,079, yet Word 2010 saved LibreOffice's test
/// document `fdo76101.docx` with 4,091. The bound keeps what the resolver keeps per style
/// small.
const MAX_STYLES: usize = 4093;

impl Formats<'_> {
    /// Forgets the table texts and what is laid over them, when there are too many.
    fn forget_table_texts(&mut self) {
        self.table_texts.clear();
        self.cell_paragraphs.clear();
        self.cell_levels.clear();
    }

    /// Whether, in a cell, the size of 11 or 12 pt and the left justification that the
    /// default paragraph style gives give way to the table style's: when the setting
    /// `overrideTableStyleFontSizeAndJustification` ([MS-DOCX] 2.3.x) is off. Not written,
    /// it is off below compatibility mode 15, as word-rules/E-table-style-size-12pt-14.docx
    /// shows (Word draws the cells in the table style's 14 pt over Normal's 12 pt), and on
    /// from mode 15: Word writes it on when it saves such a document
    /// (E-table-style-size-12pt-none.docx and the copy Word saved of it), and draws Normal's
    /// 12 pt.
    fn table_style_wins(&mut self) -> bool {
        if let Some(wins) = self.table_style_wins {
            return wins;
        }
        let settings = &self.document.settings;
        let written = settings
            .compatibility_settings()
            .iter()
            .rev()
            .find(|setting| setting.name() == "overrideTableStyleFontSizeAndJustification")
            .and_then(|setting| setting.value())
            .map(|value| matches!(value.trim(), "1" | "true" | "on"));
        let overrides = written.unwrap_or_else(|| {
            settings.effective_compatibility_mode() >= super::CompatibilityMode::WORD_2013
        });
        self.table_style_wins = Some(!overrides);
        !overrides
    }
}

/// How many combinations of a paragraph style and a list level the resolver keeps at a
/// time: each is a full set of paragraph properties, and the paragraphs of a document
/// mostly use a few.
const MAX_COMBINATIONS: usize = 1024;

/// The index of the style with this id when it is of `kind` and among the styles the
/// resolver uses.
fn style_index(document: &Document, id: &str, kind: StyleKind) -> Option<usize> {
    let index = *document.styles.by_id.get(id)?;
    let style = document.styles.styles.get(index)?;
    (style.kind == kind && index < MAX_STYLES).then_some(index)
}

/// The style the style at `index` is based on: none when its `w:basedOn` names no style
/// of its kind (ECMA-376 §17.7.4.3) or when Word ignores the style's children.
fn parent(document: &Document, index: usize) -> Option<usize> {
    let style = document.styles.styles.get(index)?;
    if IGNORED_STYLES.contains(&&*style.id) {
        return None;
    }
    style_index(document, style.based_on.as_deref()?, style.kind)
}

/// The paragraph a node is in, through the containers between them.
fn paragraph_of<'a>(document: &'a Document, node: &'a Node) -> Option<&'a Node> {
    let mut current = node;
    for _ in 0..document.nodes.len() {
        if matches!(current.kind, Kind::Paragraph { .. }) {
            return Some(current);
        }
        current = document.node(current.parent)?;
    }
    None
}

impl Document {
    /// A colour as written, in effect: a theme colour through the colour mapping and
    /// the theme, with its tint or shade; else the value written, or automatic.
    // reason: only the translation into the layout IR asks for it outside the resolver.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn resolve_color(&self, color: &super::properties::Color) -> super::ColorValue {
        character::resolve_color(self, color)
    }
}

/// Sets `slot` to `value` when there is one.
fn overwrite<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}
