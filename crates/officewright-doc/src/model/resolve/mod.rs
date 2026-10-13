//! Resolving the formatting in effect (ECMA-376 Part 1, §17.7.2): the document defaults,
//! the paragraph's style, numbering, the run's character style and direct formatting,
//! combined as Word combines them.

mod character;
mod paragraph;

pub(crate) use paragraph::list_level;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::format::{CharacterFormat, ParagraphFormat};
use super::styles::StyleKind;
use super::views::{Paragraph, Run};
use super::{Document, Kind, Node};

use character::CharacterLevel;
use paragraph::ParagraphLevel;

/// Styles whose child elements Word ignores, `w:basedOn`, `w:pPr` and `w:rPr` among
/// them (MS-OI29500 §17.7.4.17(a)).
const IGNORED_STYLES: [&str; 3] = ["NoList", "DefaultParagraphFont", "TableNormal"];

/// Resolves the formatting in effect of paragraphs, runs and paragraph marks, keeping
/// what it has worked out about styles for the next ones: use one for a whole
/// document. It borrows the document; threads that format at the same time each use
/// their own.
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
    /// The index of the default paragraph style, once looked up.
    default_paragraph_style: Option<Option<usize>>,
    /// The last paragraph whose style was looked up, and the style: the runs of a
    /// paragraph come one after another, and reading the style walks its `w:pPr`.
    last_paragraph: Option<(&'a Node, Option<usize>)>,
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
    /// What each level of a definition that paragraphs use gives to them.
    level_layers: HashMap<(i64, usize), Option<Arc<paragraph::ListLayer>>>,
    /// Each list instance's levels that paragraphs use, with the instance's overrides,
    /// and the numbers they start from.
    instance_levels: HashMap<(i64, usize), paragraph::InstanceLevel>,
    /// The defaults and a paragraph style with a list level laid over them, by the
    /// style and where the level comes from; at most [`MAX_COMBINATIONS`] at a time.
    combinations: HashMap<(Option<usize>, paragraph::LayerSource), Arc<ParagraphLevel>>,
    /// How much work the caches saved, for tests.
    #[cfg(test)]
    work: Work,
}

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
    /// Levels of definitions parsed for what they give to paragraphs.
    pub(crate) level_layers: usize,
    /// Levels of instances replaced by an override's level.
    pub(crate) replacements: usize,
    /// A style's level and a list level laid over each other.
    pub(crate) combinations: usize,
    /// Paragraphs whose `w:pPr` was read for their style.
    pub(crate) paragraph_styles: usize,
}

impl<'a> Formats<'a> {
    pub(super) fn new(document: &'a Document) -> Self {
        Self {
            document,
            characters: StyleLevels::default(),
            paragraphs: StyleLevels::default(),
            character_defaults: None,
            paragraph_defaults: None,
            default_paragraph_style: None,
            last_paragraph: None,
            instances: HashMap::new(),
            links: HashMap::new(),
            numbering_styles: HashMap::new(),
            definitions: HashMap::new(),
            level_layers: HashMap::new(),
            instance_levels: HashMap::new(),
            combinations: HashMap::new(),
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
        self.resolve_paragraph(style, &paragraph.properties())
    }

    /// The formatting of a run in effect.
    pub fn run(&mut self, run: Run<'a>) -> CharacterFormat {
        if !std::ptr::eq(run.document(), self.document) {
            return Formats::new(run.document()).run(run);
        }
        let direct = run.properties();
        let paragraph = self.paragraph_style(paragraph_of(self.document, run.node()));
        self.resolve_run(paragraph, &direct)
    }

    /// The formatting of a paragraph's mark in effect, which decides, among other
    /// things, the height of an empty paragraph.
    pub fn paragraph_mark(&mut self, paragraph: Paragraph<'a>) -> CharacterFormat {
        if !std::ptr::eq(paragraph.document(), self.document) {
            return Formats::new(paragraph.document()).paragraph_mark(paragraph);
        }
        let direct = paragraph.properties().mark;
        let style = self.paragraph_style(Some(paragraph.node()));
        self.resolve_run(style, &direct)
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

    /// How many combinations of a style and a list level are kept, for tests of the
    /// bound.
    #[cfg(test)]
    pub(crate) fn kept_combinations(&self) -> usize {
        self.combinations.len()
    }

    /// The index of the paragraph style of the paragraph `node`: its `w:pStyle` when it
    /// names a paragraph style, else the default paragraph style (the first, as
    /// [`Styles::default_style`](super::Styles::default_style) gives it). The `w:pStyle`
    /// is read as [`Paragraph::properties`] reads it: through `mc:AlternateContent`, the
    /// last of several winning.
    fn paragraph_style(&mut self, node: Option<&'a Node>) -> Option<usize> {
        if let (Some(node), Some((last, style))) = (node, self.last_paragraph) {
            if std::ptr::eq(node, last) {
                return style;
            }
        }
        #[cfg(test)]
        if node.is_some() {
            self.work.paragraph_styles += 1;
        }
        let style = self.read_paragraph_style(node);
        if let Some(node) = node {
            self.last_paragraph = Some((node, style));
        }
        style
    }

    /// [`paragraph_style`](Self::paragraph_style), read from the paragraph's `w:pPr`.
    fn read_paragraph_style(&mut self, node: Option<&'a Node>) -> Option<usize> {
        let document = self.document;
        let written = node.and_then(|node| {
            node.properties
                .iter()
                .map(|&span| document.element(span))
                .find(|element| element.is(&document.names.w, "pPr"))
                .and_then(|p_pr| {
                    p_pr.children_through_fallback(document.names.mc)
                        .into_iter()
                        .rfind(|child| child.is(&document.names.w, "pStyle"))
                        .and_then(|p_style| p_style.attribute(&document.names.w, "val"))
                })
        });
        let written = written.and_then(|id| style_index(document, id, StyleKind::Paragraph));
        written.or_else(|| {
            *self.default_paragraph_style.get_or_insert_with(|| {
                document
                    .styles
                    .styles
                    .iter()
                    .take(MAX_STYLES)
                    .position(|style| style.kind == StyleKind::Paragraph && style.is_default)
            })
        })
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
    /// How many times a style's own properties were read.
    #[cfg(test)]
    reads: usize,
}

impl<L> Default for StyleLevels<L> {
    fn default() -> Self {
        Self {
            levels: HashMap::new(),
            #[cfg(test)]
            reads: 0,
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
            level.settle();
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
            level.settle();
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
/// Stshif.cstd), some of which may be empty. MS-OI29500 §17.7.4.17(c) says that Word does not open a
/// document with more than 4,079, yet Word 2010 saved LibreOffice's test document
/// `fdo76101.docx` with 4,091. The bound keeps what the resolver keeps per style small.
const MAX_STYLES: usize = 4093;

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
        if matches!(current.kind, Kind::Paragraph) {
            return Some(current);
        }
        current = document.node(current.parent)?;
    }
    None
}

/// Sets `slot` to `value` when there is one.
fn overwrite<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}
