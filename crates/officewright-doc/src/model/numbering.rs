//! The numbering of a document (`numbering.xml`, ECMA-376 Part 1, §17.9), as written:
//! list definitions (`w:abstractNum`) and the list instances that paragraphs name
//! (`w:num`).

use std::collections::HashMap;

use officewright_core::Pt;

use super::Document;
use super::properties::{
    Justification, NumberFormat, ParagraphProperties, RunProperties, xml_enum,
};
use super::xml::{Element, Span};

/// The numbering part as stored: each definition and instance kept whole, with the ids
/// they are looked up by.
#[derive(Debug, Default)]
pub(crate) struct NumberingTable {
    pub(crate) definitions: Vec<DefinitionData>,
    /// Index into `definitions` by `w:abstractNumId`; the first with an id wins.
    pub(crate) by_definition: HashMap<i64, usize>,
    pub(crate) instances: Vec<InstanceData>,
    /// Index into `instances` by `w:numId`; the first with an id wins.
    pub(crate) by_instance: HashMap<i64, usize>,
}

#[derive(Debug)]
pub(crate) struct DefinitionData {
    pub(crate) id: i64,
    /// The `w:abstractNum` element.
    pub(crate) element: Span,
}

#[derive(Debug)]
pub(crate) struct InstanceData {
    pub(crate) id: i64,
    pub(crate) definition: Option<i64>,
    /// The `w:num` element.
    pub(crate) element: Span,
}

xml_enum! {
    /// What kind of list a definition is (ST_MultiLevelType); it changes only how Word
    /// presents the list for editing.
    MultiLevelType {
        SingleLevel = "singleLevel",
        Multilevel = "multilevel",
        /// Levels that each have their own template code.
        HybridMultilevel = "hybridMultilevel",
    }
}

xml_enum! {
    /// What follows a list label (ST_LevelSuffix).
    LevelSuffix {
        Tab = "tab",
        Space = "space",
        Nothing = "nothing",
    }
}

/// The numbering of a document: its list definitions and list instances.
///
/// A paragraph that is a list item names an instance and a level
/// ([`NumberingReference`](super::NumberingReference)); the instance leads to its
/// definition, and the definition to the level's number format and label template.
///
/// ```
/// # fn main() -> officewright_doc::Result<()> {
/// use officewright_doc::{Block, Document, NumberFormat};
///
/// let document = Document::open("tests/data/list.docx")?;
/// let numbering = document.numbering();
/// for block in document.sections().flat_map(|section| section.blocks()) {
///     let Block::Paragraph(paragraph) = block else {
///         continue;
///     };
///     let properties = paragraph.properties();
///     let Some(reference) = properties.numbering() else {
///         continue;
///     };
///     let instance = numbering
///         .instance(reference.instance().expect("the item names a list"))
///         .expect("the document has the list");
///     let level = reference.level().and_then(|level| usize::try_from(level).ok());
///     let level = instance
///         .definition()
///         .and_then(|definition| definition.level(level.unwrap_or(0)))
///         .expect("the list defines the level");
///     assert_eq!(level.number_format(), Some(NumberFormat::Decimal));
///     assert_eq!(level.text(), Some("%1."));
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Numbering<'a> {
    document: &'a Document,
}

impl<'a> Numbering<'a> {
    pub(super) fn new(document: &'a Document) -> Self {
        Self { document }
    }

    /// Every list definition (`w:abstractNum`), in the order of `numbering.xml`.
    pub fn definitions(self) -> impl ExactSizeIterator<Item = ListDefinition<'a>> + 'a {
        let document = self.document;
        document
            .numbering
            .definitions
            .iter()
            .map(move |data| ListDefinition { document, data })
    }

    /// The list definition with this id (`w:abstractNumId`).
    pub fn definition(self, id: i64) -> Option<ListDefinition<'a>> {
        let table = &self.document.numbering;
        let data = table.definitions.get(*table.by_definition.get(&id)?)?;
        Some(ListDefinition {
            document: self.document,
            data,
        })
    }

    /// Every list instance (`w:num`), in the order of `numbering.xml`.
    pub fn instances(self) -> impl ExactSizeIterator<Item = ListInstance<'a>> + 'a {
        let document = self.document;
        document
            .numbering
            .instances
            .iter()
            .map(move |data| ListInstance { document, data })
    }

    /// The list instance with this id (`w:numId`), as a paragraph's
    /// [`NumberingReference::instance`](super::NumberingReference::instance) names it.
    pub fn instance(self, id: i64) -> Option<ListInstance<'a>> {
        let table = &self.document.numbering;
        let data = table.instances.get(*table.by_instance.get(&id)?)?;
        Some(ListInstance {
            document: self.document,
            data,
        })
    }
}

/// A list definition (`w:abstractNum`): the levels of a list, from 0 to 8.
///
/// Word counts list items per definition: instances that share a definition continue
/// one sequence, and an override's start begins it again, as the reference render of
/// the corpus file `word-rules/E-list-shared-abstract.docx` shows.
#[derive(Clone, Copy, Debug)]
pub struct ListDefinition<'a> {
    document: &'a Document,
    data: &'a DefinitionData,
}

impl<'a> ListDefinition<'a> {
    /// The id instances name it by (`w:abstractNumId`).
    pub fn id(self) -> i64 {
        self.data.id
    }

    /// The number that identifies the definition across documents (`w:nsid`).
    pub fn identifier(self) -> Option<u32> {
        self.child("nsid")
            .and_then(|nsid| nsid.attribute(&self.document.names.w, "val"))
            .and_then(long_hex)
    }

    /// A code an application may use to place the definition in its user interface
    /// (`w:tmpl`).
    pub fn template(self) -> Option<u32> {
        self.child("tmpl")
            .and_then(|tmpl| tmpl.attribute(&self.document.names.w, "val"))
            .and_then(long_hex)
    }

    /// What kind of list it is (`w:multiLevelType`).
    pub fn multi_level_type(self) -> Option<MultiLevelType> {
        self.child("multiLevelType")
            .and_then(|kind| kind.attribute(&self.document.names.w, "val"))
            .and_then(MultiLevelType::from_xml)
    }

    /// The name shown to users (`w:name`).
    pub fn name(self) -> Option<&'a str> {
        self.string("name")
    }

    /// The id of the numbering style this definition defines (`w:styleLink`).
    pub fn style_link(self) -> Option<&'a str> {
        self.string("styleLink")
    }

    /// The id of the numbering style whose definition this one stands for
    /// (`w:numStyleLink`): its own levels are then not used.
    pub fn numbering_style_link(self) -> Option<&'a str> {
        self.string("numStyleLink")
    }

    /// The levels as written (`w:lvl`), in document order. A level whose `w:ilvl` is
    /// not 0 to 8 is left out, since Word does not open such a file (MS-OI29500
    /// §17.9.6(a)), and so is a second level with one `w:ilvl`.
    pub fn levels(self) -> Vec<ListLevel> {
        let document = self.document;
        let mut seen = [false; 9];
        let mut levels = Vec::new();
        for child in self.children() {
            if !child.is(&document.names.w, "lvl") {
                continue;
            }
            let Some(index) = level_index(child, document) else {
                continue;
            };
            if let Some(seen) = seen.get_mut(index) {
                if !*seen {
                    *seen = true;
                    levels.extend(crate::docx::numbering::level(
                        child,
                        &document.names,
                        &mut |_, _, _| {},
                    ));
                }
            }
        }
        levels
    }

    /// The level with this number, 0 to 8.
    pub fn level(self, number: usize) -> Option<ListLevel> {
        let document = self.document;
        self.children()
            .into_iter()
            .filter(|child| child.is(&document.names.w, "lvl"))
            .find(|&child| level_index(child, document) == Some(number))
            .and_then(|lvl| crate::docx::numbering::level(lvl, &document.names, &mut |_, _, _| {}))
    }

    fn children(self) -> Vec<Element<'a>> {
        let document = self.document;
        document
            .element(self.data.element)
            .children_through_fallback(document.names.mc)
    }

    fn child(self, local: &str) -> Option<Element<'a>> {
        let w = &self.document.names.w;
        self.children().into_iter().find(|child| child.is(w, local))
    }

    fn string(self, local: &str) -> Option<&'a str> {
        self.child(local)?.attribute(&self.document.names.w, "val")
    }
}

/// A list instance (`w:num`): a definition, with overrides of some of its levels, that
/// paragraphs name to be items of the list.
#[derive(Clone, Copy, Debug)]
pub struct ListInstance<'a> {
    document: &'a Document,
    data: &'a InstanceData,
}

impl<'a> ListInstance<'a> {
    /// The id paragraphs name it by (`w:numId`).
    pub fn id(self) -> i64 {
        self.data.id
    }

    /// The id of its definition (`w:abstractNumId`).
    pub fn definition_id(self) -> Option<i64> {
        self.data.definition
    }

    /// Its definition, when the document has one with that id.
    pub fn definition(self) -> Option<ListDefinition<'a>> {
        Numbering::new(self.document).definition(self.data.definition?)
    }

    /// The first override of level `number`, the only one parsed.
    pub(crate) fn override_of(self, number: usize) -> Option<LevelOverride> {
        let document = self.document;
        let number = i64::try_from(number).ok()?;
        document
            .element(self.data.element)
            .children_through_fallback(document.names.mc)
            .into_iter()
            .filter(|child| child.is(&document.names.w, "lvlOverride"))
            .find(|element| {
                crate::docx::numbering::override_level(*element, &document.names) == Some(number)
            })
            .and_then(|element| {
                crate::docx::numbering::level_override(element, &document.names, &mut |_, _, _| {})
            })
    }

    /// The overrides of levels of the definition (`w:lvlOverride`), in document order.
    /// One whose `w:ilvl` is not a number is left out, and so is a second override of
    /// a level 0 to 8, as a definition's second level of one number is.
    pub fn overrides(self) -> Vec<LevelOverride> {
        let document = self.document;
        let mut seen = [false; 9];
        document
            .element(self.data.element)
            .children_through_fallback(document.names.mc)
            .into_iter()
            .filter(|child| child.is(&document.names.w, "lvlOverride"))
            .filter_map(|element| {
                crate::docx::numbering::level_override(element, &document.names, &mut |_, _, _| {})
            })
            .filter(|level_override| {
                let index = usize::try_from(level_override.level).ok();
                match index.and_then(|index| seen.get_mut(index)) {
                    Some(seen) if *seen => false,
                    Some(seen) => {
                        *seen = true;
                        true
                    }
                    None => true,
                }
            })
            .collect()
    }
}

/// An override of one level of a list definition for one list instance
/// (`w:lvlOverride`).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LevelOverride {
    pub(crate) level: i64,
    pub(crate) start: Option<i64>,
    pub(crate) definition: Option<ListLevel>,
}

impl LevelOverride {
    /// The level overridden (`w:ilvl`), as written.
    pub fn level(&self) -> i64 {
        self.level
    }

    /// The number the level starts at again in this instance (`w:startOverride`).
    pub fn start(&self) -> Option<i64> {
        self.start
    }

    /// A level that replaces the definition's for this instance (`w:lvl`), the first
    /// one written; its own `w:ilvl` is as written and may name another level. Word
    /// ignores its `w:start` and `w:lvlRestart` (MS-OI29500 §17.9.10(b), §17.9.25(b)).
    pub fn definition(&self) -> Option<&ListLevel> {
        self.definition.as_ref()
    }
}

/// Numbering of an earlier word processor that did not support all of today's
/// numbering properties (`w:legacy`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct LegacyNumbering {
    pub(crate) legacy: Option<bool>,
    pub(crate) space: Option<Pt>,
    pub(crate) indent: Option<Pt>,
}

impl LegacyNumbering {
    /// The level uses the legacy rules (`w:legacy`).
    pub fn legacy(&self) -> Option<bool> {
        self.legacy
    }

    /// The space after the label (`w:legacySpace`).
    pub fn space(&self) -> Option<Pt> {
        self.space
    }

    /// The indentation of the label (`w:legacyIndent`).
    pub fn indent(&self) -> Option<Pt> {
        self.indent
    }
}

/// One level of a list definition, as written (`w:lvl`).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ListLevel {
    pub(crate) level: usize,
    pub(crate) start: Option<i64>,
    pub(crate) number_format: Option<NumberFormat>,
    pub(crate) restart: Option<i64>,
    pub(crate) paragraph_style: Option<Box<str>>,
    pub(crate) legal: Option<bool>,
    pub(crate) suffix: Option<LevelSuffix>,
    pub(crate) text: Option<Box<str>>,
    pub(crate) picture_bullet: Option<i64>,
    pub(crate) legacy: Option<LegacyNumbering>,
    pub(crate) justification: Option<Justification>,
    pub(crate) paragraph_properties: ParagraphProperties,
    pub(crate) run_properties: RunProperties,
    pub(crate) tentative: Option<bool>,
    pub(crate) template_code: Option<u32>,
}

impl ListLevel {
    /// The level, 0 to 8 (`w:ilvl`).
    pub fn level(&self) -> usize {
        self.level
    }

    /// The first number (`w:start`); 0 when it is not written (§17.9.25).
    pub fn start(&self) -> Option<i64> {
        self.start
    }

    /// How the number is written (`w:numFmt`). Word writes a custom format in the choice
    /// of an `mc:AlternateContent` and a standard one in its fallback (POI's
    /// `Numbering.docx`); the fallback's is the one read here.
    pub fn number_format(&self) -> Option<NumberFormat> {
        self.number_format
    }

    /// When the level starts again (`w:lvlRestart`), as written: 0 never, a level
    /// counted from 1 otherwise. Word restarts when that level or any higher one is
    /// used, and without the element when any higher level is used; it allows 0 to 7
    /// (MS-OI29500 §17.9.10(a), (c), (d)).
    pub fn restart(&self) -> Option<i64> {
        self.restart
    }

    /// The paragraph style linked to the level (`w:pStyle`).
    pub fn paragraph_style(&self) -> Option<&str> {
        self.paragraph_style.as_deref()
    }

    /// The numbers of all levels in the label are written as decimal numbers
    /// (`w:isLgl`). In Word this includes the level's own number, except where its
    /// format is none (MS-OI29500 §17.9.4).
    pub fn legal(&self) -> Option<bool> {
        self.legal
    }

    /// What follows the label (`w:suff`).
    pub fn suffix(&self) -> Option<LevelSuffix> {
        self.suffix
    }

    /// The template of the label (`w:lvlText`), where `%1` to `%9` stand for the number
    /// of levels 1 to 9: its `w:val`, or empty when there is none and `w:null` is on.
    /// Word requires `w:null` on when `w:val` is not written, ignores the whole template
    /// when it names a level deeper than this one, and uses it literally for bullets
    /// (MS-OI29500 §17.9.11(c), (d), §17.9.17(a)).
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// The picture used as the bullet (`w:lvlPicBulletId`), an id of the numbering
    /// part's picture bullets.
    pub fn picture_bullet(&self) -> Option<i64> {
        self.picture_bullet
    }

    /// Legacy numbering (`w:legacy`).
    pub fn legacy(&self) -> Option<&LegacyNumbering> {
        self.legacy.as_ref()
    }

    /// The alignment of the label (`w:lvlJc`); Word supports start, center and end
    /// only (MS-OI29500 §17.9.7(a)).
    pub fn justification(&self) -> Option<Justification> {
        self.justification
    }

    /// The paragraph properties the level gives its paragraphs (its `w:pPr`); of these
    /// Word allows the alignment, indentation and tab stops only (MS-OI29500
    /// §17.9.22(b)), though it writes others too.
    pub fn paragraph_properties(&self) -> &ParagraphProperties {
        &self.paragraph_properties
    }

    /// The run properties of the label (its `w:rPr`).
    pub fn run_properties(&self) -> &RunProperties {
        &self.run_properties
    }

    /// The level is tentative (`w:tentative`): the standard keeps such levels out of the
    /// user interface, but Word shows them in its list of multilevel lists (MS-OI29500
    /// §17.9.6(b)).
    pub fn tentative(&self) -> Option<bool> {
        self.tentative
    }

    /// A code Word uses to tell its built-in levels apart (`w:tplc`).
    pub fn template_code(&self) -> Option<u32> {
        self.template_code
    }
}

/// The level of a `w:lvl`; the reader of the level refuses one past 8.
fn level_index(lvl: Element<'_>, document: &Document) -> Option<usize> {
    lvl.attribute(&document.names.w, "ilvl")?.parse().ok()
}

/// ST_LongHexNumber: four bytes, eight hexadecimal digits.
pub(crate) fn long_hex(value: &str) -> Option<u32> {
    (value.len() == 8 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(value, 16).ok())
        .flatten()
}
