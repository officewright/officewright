//! Lists: the definitions of numbered and bulleted lists, their instances, and the list
//! items paragraphs are.

use super::macros::{ir_struct, no_rules};
use super::section::NumberFormat;
use super::walk::Checker;
use super::{ListId, ListInstanceId, RunStyleId};
use crate::Result;

/// The number of levels a list has: Word does not load a document with a level outside 0
/// to 8 (MS-OI29500 2.1.280 a).
pub(crate) const LIST_LEVELS: u8 = 9;

ir_struct! {
    /// A list definition: up to nine levels, whose counters every instance of the
    /// definition shares (`w:abstractNum`). Word counts per definition, not per instance:
    /// two instances of one definition continue one sequence unless an override restarts
    /// it.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct ListDefinition {
        /// The levels, from level 0, the outermost. A list item of a level the definition
        /// does not have gets no label.
        vec levels: Level => with_levels, check at_most_nine_levels;
    }
}

fn at_most_nine_levels(levels: &[Level], checker: &mut Checker<'_>) -> Result<()> {
    if levels.len() > usize::from(LIST_LEVELS) {
        return Err(checker.error(format!(
            "{} levels, but a list has at most {LIST_LEVELS}",
            levels.len()
        )));
    }
    Ok(())
}

ir_struct! {
    /// A level of a list: how its items are numbered and labelled. Its indents and tab
    /// stops are not here: the producer merges them into the paragraphs' properties.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Level {
        /// The number of the first item.
        copy start: u32 = 0 => with_start;
        /// How the number is written.
        ref format: NumberFormat = NumberFormat::Decimal => with_format;
        /// The template of the label, such as `%1.%2.` for `2.3.`. For a bullet level the
        /// producer gives the template as one literal, since Word ignores `%` in it
        /// (MS-OI29500 2.1.286 a).
        vec text: LevelTextPart = Vec::new() => with_text;
        /// Whether the numbers of every level in the label are written in decimal, as in
        /// legal numbering (`w:isLgl`).
        copy legal: bool = false => with_legal;
        /// When the counter starts again.
        copy restart: Restart = Restart::Default => with_restart;
        /// What follows the label.
        copy suffix: Suffix = Suffix::Tab => with_suffix;
        /// How the label aligns at the start of the paragraph's first line.
        copy alignment: LevelAlignment = LevelAlignment::Start => with_alignment;
    }
}

/// A piece of a list level's label template.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LevelTextPart {
    /// Text that is shown as it is.
    Literal(String),
    /// The number of a level, from 0: `%1` in WordprocessingML is level 0.
    Placeholder(u8),
}

/// When the counter of a list level starts again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Restart {
    /// After an item of any outer level (MS-OI29500 2.1.282 d).
    #[default]
    Default,
    /// Never (`w:lvlRestart w:val="0"`).
    Never,
    /// After an item of this level or any outer one (MS-OI29500 2.1.282 c).
    /// `w:lvlRestart w:val="n"` is `AfterLevel(n - 1)`.
    AfterLevel(u8),
}

/// What follows the label of a list item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Suffix {
    /// A tab.
    #[default]
    Tab,
    /// A space.
    Space,
    /// Nothing.
    Nothing,
}

/// How the label of a list item aligns at the start of the first line. Word knows only
/// these three (MS-OI29500 2.1.281 a).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LevelAlignment {
    /// The label starts there.
    #[default]
    Start,
    /// The label is centred on it.
    Center,
    /// The label ends there.
    End,
}

ir_struct! {
    /// An instance of a list definition, with the levels it overrides (`w:num`).
    #[derive(Clone, Debug, PartialEq)]
    pub struct ListInstance {
        /// The definition.
        copy definition: ListId => with_definition;
        /// The levels it overrides; at most one override per level.
        vec overrides: LevelOverride => with_overrides;
    }
    check one_override_per_level;
}

impl ListInstance {
    /// An instance of a definition that overrides nothing.
    pub fn new(definition: ListId) -> Self {
        Self {
            definition,
            overrides: Vec::new(),
        }
    }
}

fn one_override_per_level(instance: &ListInstance, checker: &mut Checker<'_>) -> Result<()> {
    let mut seen = [false; LIST_LEVELS as usize];
    for (position, level_override) in instance.overrides.iter().enumerate() {
        // The level is below LIST_LEVELS: the override's own check ran first.
        if let Some(seen) = seen.get_mut(usize::from(level_override.level)) {
            if *seen {
                return Err(checker.error(format!(
                    "override {position} is the second override of level {}",
                    level_override.level
                )));
            }
            *seen = true;
        }
    }
    Ok(())
}

ir_struct! {
    /// How an instance changes a level of its definition (`w:lvlOverride`).
    #[derive(Clone, Debug, PartialEq)]
    pub struct LevelOverride {
        /// The level, from 0.
        copy level: u8 => with_level, check level_index;
        /// The number the first item of this level in this instance gets.
        copy start: Option<u32> => with_start;
        /// A level that replaces the definition's for this instance. Word ignores the
        /// restart rule and the start of a replacing level (MS-OI29500 2.1.282 b,
        /// 2.1.292 b), so the producer leaves them at their defaults.
        ref replacement: Option<Level> => with_replacement;
    }
}

impl LevelOverride {
    /// An override of a level that changes nothing yet.
    pub fn new(level: u8) -> Self {
        Self {
            level,
            start: None,
            replacement: None,
        }
    }
}

ir_struct! {
    /// What makes a paragraph an item of a list.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct ListItem {
        /// The list instance.
        copy instance: ListInstanceId => with_instance;
        /// The level, from 0.
        copy level: u8 => with_level, check level_index;
        /// The formatting of the label: the paragraph mark's with the level's own on top.
        copy label_style: RunStyleId => with_label_style;
    }
}

impl ListItem {
    /// An item at this level of a list instance, with its label in this style.
    pub fn new(instance: ListInstanceId, level: u8, label_style: RunStyleId) -> Self {
        Self {
            instance,
            level,
            label_style,
        }
    }
}

/// A list level is from 0 to 8.
fn level_index(level: &u8, checker: &mut Checker<'_>) -> Result<()> {
    if *level >= LIST_LEVELS {
        return Err(checker.error(format!(
            "level {level} does not exist; levels are 0 to {}",
            LIST_LEVELS - 1
        )));
    }
    Ok(())
}

impl super::walk::Check for LevelTextPart {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Placeholder(level) => level_index(level, checker),
            Self::Literal(_) => Ok(()),
        }
    }
}

impl super::walk::Check for Restart {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::AfterLevel(level) => level_index(level, checker),
            Self::Default | Self::Never => Ok(()),
        }
    }
}

impl super::walk::Clamp for LevelTextPart {
    fn clamp(&mut self, _: &mut super::walk::Clamper) {}
}

impl super::walk::Clamp for Restart {
    fn clamp(&mut self, _: &mut super::walk::Clamper) {}
}

no_rules!(Suffix, LevelAlignment);
