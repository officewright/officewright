//! List numbering: the counters of list definitions, run in document order over one
//! story, and the text of each item's label.
//!
//! Word counts per list definition (`w:abstractNum`), not per instance (`w:num`): two
//! instances of one definition continue one sequence unless an override restarts it.

use std::collections::{HashMap, HashSet};

use crate::ir::{
    Document, Level, LevelAlignment, LevelTextPart, ListId, ListInstanceId, ListItem, NumberFormat,
    Restart, RunStyleId, Suffix,
};
use crate::numbers::number_text;
use crate::report::Location;

/// The levels a list has: Word takes `w:ilvl` from 0 to 8 (MS-OI29500 2.1.280 a).
const LEVELS: usize = 9;

/// The label of a list item, and what follows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Label {
    /// The text, as the level's template and the counters make it; empty for a level
    /// whose template Word ignores.
    pub(crate) text: String,
    /// Its formatting.
    pub(crate) style: RunStyleId,
    /// What separates it from the paragraph's text.
    pub(crate) suffix: Suffix,
    /// How it stands at the start of the first line.
    pub(crate) alignment: LevelAlignment,
}

/// The counters of every list definition of one story (the body, a header, a footer),
/// which the items of that story advance in document order.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    lists: HashMap<ListId, Sequence>,
    /// The levels of each instance that have had a paragraph: a start override applies to
    /// the first only.
    started: HashSet<(ListInstanceId, u8)>,
    /// The first paragraph whose label has a number written in decimal because its
    /// format is one the layout does not write.
    pub(crate) unsupported_format: Option<Location>,
}

/// The values of the levels of one list definition.
#[derive(Debug, Default)]
struct Sequence {
    /// Each level's value, once it has had a paragraph.
    values: [Option<u32>; LEVELS],
    /// Whether each level starts again at its next paragraph.
    restart: [bool; LEVELS],
}

impl Counters {
    /// The label of `item`, the next paragraph of its list in document order, which it
    /// advances; `None` when its list or level does not exist. `location` is the
    /// paragraph's place, for the report.
    pub(crate) fn label(
        &mut self,
        document: &Document,
        item: &ListItem,
        location: &Location,
    ) -> Option<Label> {
        let index = usize::from(item.level());
        if index >= LEVELS {
            return None;
        }
        let instance = document.list_instance(item.instance())?;
        let levels: Vec<Option<&Level>> = (0..LEVELS)
            .map(|level| effective_level(document, item, level))
            .collect();
        let level = levels.get(index).copied().flatten()?;
        let first_of_instance = self.started.insert((item.instance(), item.level()));
        let start_override = instance
            .overrides()
            .iter()
            .find(|change| change.level() == item.level())
            .and_then(|change| change.start())
            .filter(|_| first_of_instance);
        let sequence = self.lists.entry(instance.definition()).or_default();
        // An instance's start override sets the value of its first paragraph at the level
        // outright; else the level starts again after a level above it, or counts on.
        let value = match start_override {
            Some(start) => start,
            None => match sequence.values.get(index).copied().flatten() {
                Some(previous) if !sequence.restart.get(index).copied().unwrap_or(true) => {
                    previous.saturating_add(1)
                }
                _ => level.start(),
            },
        };
        if let Some(slot) = sequence.values.get_mut(index) {
            *slot = Some(value);
        }
        // A level above that has had no paragraph and that the label shows, shows its
        // start, and Word counts on from there: after "1.1." with no "1." before it, the next
        // item of the level above is "2."; a label that shows only its own level leaves the
        // levels above as they are (E-list-level-inherit, cases 01 to 05 and 04 to 08).
        for part in level.text() {
            if let LevelTextPart::Placeholder(above) = part {
                let above = usize::from(*above);
                if let (true, Some(slot)) = (above < index, sequence.values.get_mut(above)) {
                    if slot.is_none() {
                        *slot = Some(levels.get(above).copied().flatten().map_or(0, Level::start));
                    }
                }
            }
        }
        if let Some(slot) = sequence.restart.get_mut(index) {
            *slot = false;
        }
        // The levels below this one start again at their next paragraph, as their rule
        // says: after any level above them (the default, OI 2.1.282 d), after a given
        // level or any above it (`w:lvlRestart`, OI 2.1.282 c), or never.
        for (deeper, rule) in levels.iter().enumerate().skip(index + 1) {
            let restarts = match rule.map(|level| level.restart()) {
                Some(Restart::Default) | None => true,
                Some(Restart::AfterLevel(after)) => index <= usize::from(after),
                Some(Restart::Never) => false,
            };
            if let (true, Some(slot)) = (restarts, sequence.restart.get_mut(deeper)) {
                *slot = true;
            }
        }
        let mut unsupported = false;
        let text = label_text(level, index, &levels, &sequence.values, &mut unsupported);
        if unsupported && self.unsupported_format.is_none() {
            self.unsupported_format = Some(location.clone());
        }
        Some(Label {
            text,
            style: item.label_style(),
            suffix: level.suffix(),
            alignment: level.alignment(),
        })
    }
}

/// Level `level` of the list of `item`: its instance's replacement, else its
/// definition's.
fn effective_level<'d>(document: &'d Document, item: &ListItem, level: usize) -> Option<&'d Level> {
    let instance = document.list_instance(item.instance())?;
    let replacement = instance
        .overrides()
        .iter()
        .find(|change| usize::from(change.level()) == level)
        .and_then(|change| change.replacement().as_ref());
    replacement.or_else(|| document.list(instance.definition())?.levels().get(level))
}

/// The text of the label of a paragraph at `index` of `level`: its template, each
/// placeholder the value of its level in that level's format. A placeholder of a level
/// below the paragraph's makes Word ignore the whole template (MS-OI29500 2.1.283 c). A
/// legal level writes every number in decimal but that of a level without numbers
/// (OI 2.1.278 a, b).
fn label_text(
    level: &Level,
    index: usize,
    levels: &[Option<&Level>],
    values: &[Option<u32>; LEVELS],
    unsupported: &mut bool,
) -> String {
    let mut text = String::new();
    for part in level.text() {
        match part {
            LevelTextPart::Literal(literal) => text.push_str(literal),
            LevelTextPart::Placeholder(of) => {
                let of = usize::from(*of);
                if of > index {
                    return String::new();
                }
                let of_level = levels.get(of).copied().flatten();
                let value = values
                    .get(of)
                    .copied()
                    .flatten()
                    .or_else(|| of_level.map(Level::start))
                    .unwrap_or(0);
                let format = of_level.map_or(&NumberFormat::Decimal, Level::format);
                let format = if level.legal() && *format != NumberFormat::None {
                    &NumberFormat::Decimal
                } else {
                    format
                };
                let (number, decimal) = number_text(value, format);
                *unsupported |= decimal;
                text.push_str(&number);
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{LevelOverride, ListDefinition, ListInstance, RunStyle};

    /// A level of the template `template` in `format`.
    fn level(template: &[LevelTextPart], format: NumberFormat) -> Level {
        Level::default()
            .with_text(template.to_vec())
            .with_format(format)
    }

    fn literal(text: &str) -> LevelTextPart {
        LevelTextPart::Literal(text.into())
    }

    /// The decimal outline "1.", "1.1.", "1.1.1." … of nine levels.
    fn outline() -> ListDefinition {
        ListDefinition::default().with_levels((0..9).map(|depth| {
            let mut parts = Vec::new();
            for of in 0..=depth {
                parts.push(LevelTextPart::Placeholder(of));
                parts.push(literal("."));
            }
            level(&parts, NumberFormat::Decimal).with_start(1)
        }))
    }

    /// The labels of items at `levels` of `instances`, in that order, the text of those
    /// without a label `-`.
    fn labels(document: &mut Document, items: &[(ListInstanceId, u8)]) -> Vec<String> {
        let style = document.add_run_style(RunStyle::default());
        let document = &*document;
        let mut counters = Counters::default();
        items
            .iter()
            .map(|&(instance, level)| {
                counters
                    .label(
                        document,
                        &ListItem::new(instance, level, style),
                        &Location::default(),
                    )
                    .map_or_else(|| "-".to_owned(), |label| label.text)
            })
            .collect()
    }

    #[test]
    fn levels_count_on_and_start_again_after_a_level_above_them() {
        let mut document = Document::default();
        let list = document.add_list(outline());
        let one = document.add_list_instance(ListInstance::new(list));
        let items = [
            (one, 0),
            (one, 1),
            (one, 1),
            (one, 2),
            (one, 0),
            (one, 1),
            (one, 2),
        ];
        assert_eq!(
            labels(&mut document, &items),
            ["1.", "1.1.", "1.2.", "1.2.1.", "2.", "2.1.", "2.1.1."]
        );
    }

    #[test]
    fn a_level_above_shown_at_its_start_counts_on_from_there() {
        // Word's render of E-list-level-inherit: "1.1.", "1.2.", "1.2.1.", then "2.".
        let mut document = Document::default();
        let list = document.add_list(outline());
        let one = document.add_list_instance(ListInstance::new(list));
        let items = [(one, 1), (one, 1), (one, 2), (one, 0), (one, 0)];
        assert_eq!(
            labels(&mut document, &items),
            ["1.1.", "1.2.", "1.2.1.", "2.", "3."]
        );
        // A label that shows only its own level leaves the levels above unused: after
        // "a)" at level 2, the first item at level 1 starts at its start (cases 04, 08).
        let mut document = Document::default();
        let own = |of: u8, format: NumberFormat| {
            level(&[LevelTextPart::Placeholder(of), literal(")")], format).with_start(1)
        };
        let levels = [
            own(0, NumberFormat::Decimal),
            own(1, NumberFormat::LowerLetter),
            own(2, NumberFormat::LowerLetter),
        ];
        let list = document.add_list(ListDefinition::default().with_levels(levels));
        let two = document.add_list_instance(ListInstance::new(list));
        assert_eq!(
            labels(&mut document, &[(two, 2), (two, 1), (two, 0)]),
            ["a)", "a)", "1)"]
        );
    }

    #[test]
    fn instances_of_one_definition_continue_one_sequence() {
        let mut document = Document::default();
        let list = document.add_list(outline());
        let first = document.add_list_instance(ListInstance::new(list));
        let second = document.add_list_instance(ListInstance::new(list));
        // A start override restarts the instance's first paragraph at its level only.
        let restarted = document.add_list_instance(
            ListInstance::new(list).with_overrides([LevelOverride::new(0).with_start(Some(7))]),
        );
        let items = [
            (first, 0),
            (first, 0),
            (second, 0),
            (restarted, 0),
            (restarted, 0),
            (first, 0),
        ];
        assert_eq!(
            labels(&mut document, &items),
            ["1.", "2.", "3.", "7.", "8.", "9."]
        );
    }

    #[test]
    fn restart_rules_follow_the_level() {
        let mut document = Document::default();
        let levels = vec![
            level(&[LevelTextPart::Placeholder(0)], NumberFormat::Decimal).with_start(1),
            level(&[LevelTextPart::Placeholder(1)], NumberFormat::LowerLetter)
                .with_start(1)
                .with_restart(Restart::Never),
            level(&[LevelTextPart::Placeholder(2)], NumberFormat::LowerRoman)
                .with_start(1)
                .with_restart(Restart::AfterLevel(0)),
        ];
        let list = document.add_list(ListDefinition::default().with_levels(levels));
        let one = document.add_list_instance(ListInstance::new(list));
        let items = [
            (one, 1),
            (one, 2),
            (one, 0),
            (one, 1),
            (one, 2),
            (one, 1),
            (one, 2),
        ];
        // Level 1 never starts again; level 2 starts again only after level 0, not after 1.
        assert_eq!(
            labels(&mut document, &items),
            ["a", "i", "1", "b", "i", "c", "ii"]
        );
    }

    #[test]
    fn templates_write_each_level_in_its_format() {
        let mut document = Document::default();
        let levels = vec![
            level(&[LevelTextPart::Placeholder(0)], NumberFormat::UpperRoman).with_start(4),
            level(
                &[
                    literal("("),
                    LevelTextPart::Placeholder(0),
                    literal("-"),
                    LevelTextPart::Placeholder(1),
                    literal(")"),
                ],
                NumberFormat::UpperLetter,
            )
            .with_start(1),
            // A legal level writes the levels above it in decimal.
            level(
                &[
                    LevelTextPart::Placeholder(0),
                    literal("."),
                    LevelTextPart::Placeholder(2),
                ],
                NumberFormat::LowerRoman,
            )
            .with_start(2)
            .with_legal(true),
            // A placeholder of a level below makes Word ignore the template.
            level(
                &[literal("x"), LevelTextPart::Placeholder(4)],
                NumberFormat::Decimal,
            ),
            Level::default()
                .with_text([literal("\u{2022}")])
                .with_format(NumberFormat::Bullet),
        ];
        let list = document.add_list(ListDefinition::default().with_levels(levels));
        let one = document.add_list_instance(ListInstance::new(list));
        let items = [(one, 1), (one, 2), (one, 3), (one, 4), (one, 5), (one, 9)];
        // Level 0 has had no paragraph: it shows its start.
        assert_eq!(
            labels(&mut document, &items),
            ["(IV-A)", "4.2", "", "\u{2022}", "-", "-"]
        );
    }

    #[test]
    fn a_format_the_layout_does_not_write_is_decimal_and_counted() {
        let mut document = Document::default();
        let levels = [level(
            &[LevelTextPart::Placeholder(0)],
            NumberFormat::Other("hebrew1".into()),
        )
        .with_start(5)];
        let list = document.add_list(ListDefinition::default().with_levels(levels));
        let one = document.add_list_instance(ListInstance::new(list));
        let style = document.add_run_style(RunStyle::default());
        let mut counters = Counters::default();
        let mut location = Location::default();
        location.push(crate::report::Step::Index(3));
        let label = counters
            .label(&document, &ListItem::new(one, 0, style), &location)
            .unwrap();
        assert_eq!(label.text, "5");
        assert_eq!(counters.unsupported_format, Some(location));
    }
}
