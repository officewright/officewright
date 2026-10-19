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

/// The most characters a label is drawn with, and the most of its template that is read,
/// counted as `w:lvlText` holds it (see [`read_template`]). A level's template is written
/// once but read and drawn for every item of the level, so a long one would take time and
/// room in proportion to the items times its length; real labels ("1.1.1.", "Article
/// 12.") are far shorter. The rest of a longer template is as if it were not there: for
/// the label's text, for the levels it shows and for the rule that ignores a template
/// showing a deeper level. What Word does with a longer template is a hypothesis until a
/// Word render (#320).
pub(crate) const MAX_LABEL_CHARS: usize = 256;

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
    /// The first paragraph whose label, or the template it is written from, is cut at
    /// [`MAX_LABEL_CHARS`].
    pub(crate) long_label: Option<Location>,
}

/// The values of the levels of one list definition.
#[derive(Debug, Default)]
struct Sequence {
    /// Each level's value, once it has had a paragraph; `None` again when a level above
    /// restarts it, so that it is shown, and counts on, from its start.
    values: [Option<u32>; LEVELS],
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
        // What the labels show comes from each level as the instance has it, replaced or
        // not; how a level counts, its start and when it starts again, from the
        // definition, since Word ignores those of a replacing level (MS-OI29500
        // 2.1.282 b, 2.1.292 b).
        let levels: Vec<Option<&Level>> = (0..LEVELS)
            .map(|level| effective_level(document, item, level))
            .collect();
        let definition = document.list(instance.definition());
        let counting: Vec<Option<&Level>> = (0..LEVELS)
            .map(|level| {
                definition
                    .and_then(|list| list.levels().get(level))
                    .or_else(|| levels.get(level).copied().flatten())
            })
            .collect();
        let level = levels.get(index).copied().flatten()?;
        let (template, long_template) = read_template(level.text());
        let start_of = |level: usize| {
            counting
                .get(level)
                .copied()
                .flatten()
                .map_or(0, Level::start)
        };
        let first_of_instance = self.started.insert((item.instance(), item.level()));
        let start_override = instance
            .overrides()
            .iter()
            .find(|change| change.level() == item.level())
            .and_then(|change| change.start())
            .filter(|_| first_of_instance);
        let sequence = self.lists.entry(instance.definition()).or_default();
        // An instance's start override sets the value of its first paragraph at the level
        // outright; else the level counts on, or starts at its start when it has had no
        // paragraph since a level above restarted it.
        let value = match start_override {
            Some(start) => start,
            None => match sequence.values.get(index).copied().flatten() {
                Some(previous) => previous.saturating_add(1),
                None => start_of(index),
            },
        };
        if let Some(slot) = sequence.values.get_mut(index) {
            *slot = Some(value);
        }
        // A level above that has had no paragraph since it started, and that the label
        // shows, shows its start, and Word counts on from there: after "1.1." with no "1."
        // before it, the next item of the level above is "2." (E-list-level-inherit,
        // cases 01 to 05). A label that shows only its own level leaves the levels above
        // as they are; that is a hypothesis, which no reference shows since #292.
        for piece in &template {
            if let Piece::Level(above) = piece {
                let above = usize::from(*above);
                if let (true, Some(slot)) = (above < index, sequence.values.get_mut(above)) {
                    if slot.is_none() {
                        *slot = Some(start_of(above));
                    }
                }
            }
        }
        // The levels below this one start again at their next paragraph, as their rule
        // says: after any level above them (the default, OI 2.1.282 d), after a given
        // level or any above it (`w:lvlRestart`, OI 2.1.282 c), or never.
        for (deeper, rule) in counting.iter().enumerate().skip(index + 1) {
            let restarts = match rule.map(|level| level.restart()) {
                Some(Restart::Default) | None => true,
                Some(Restart::AfterLevel(after)) => index <= usize::from(after),
                Some(Restart::Never) => false,
            };
            if let (true, Some(slot)) = (restarts, sequence.values.get_mut(deeper)) {
                *slot = None;
            }
        }
        let mut unsupported = false;
        let mut cut = long_template;
        let shown = Shown {
            levels: &levels,
            starts: &counting,
            values: &sequence.values,
        };
        let text = label_text(
            &template,
            level.legal(),
            index,
            &shown,
            (&mut unsupported, &mut cut),
        );
        if unsupported && self.unsupported_format.is_none() {
            self.unsupported_format = Some(location.clone());
        }
        if cut && self.long_label.is_none() {
            self.long_label = Some(location.clone());
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

/// What a label shows of each level of its list: the level as the instance has it, the
/// level whose start counts, and the level's value.
struct Shown<'a, 'd> {
    levels: &'a [Option<&'d Level>],
    starts: &'a [Option<&'d Level>],
    values: &'a [Option<u32>; LEVELS],
}

/// A piece of a template as [`read_template`] reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Piece {
    /// A character of its text, as the template holds it.
    Char(char),
    /// A placeholder: the number of this level.
    Level(u8),
}

/// The pieces of `template` the layout reads, up to [`MAX_LABEL_CHARS`] counted as
/// `w:lvlText` holds them: a character of text one, a placeholder two (`%1`), and a piece
/// of text with no characters one, so that a template of many empty pieces is bounded too;
/// and whether the template goes on past them.
fn read_template(template: &[LevelTextPart]) -> (Vec<Piece>, bool) {
    let mut pieces = Vec::new();
    let mut left = MAX_LABEL_CHARS;
    for part in template {
        match part {
            LevelTextPart::Literal(text) if text.is_empty() => {
                if left == 0 {
                    return (pieces, true);
                }
                left -= 1;
            }
            LevelTextPart::Literal(text) => {
                for ch in text.chars() {
                    if left == 0 {
                        return (pieces, true);
                    }
                    left -= 1;
                    pieces.push(Piece::Char(ch));
                }
            }
            LevelTextPart::Placeholder(of) => {
                if left < 2 {
                    return (pieces, true);
                }
                left -= 2;
                pieces.push(Piece::Level(*of));
            }
        }
    }
    (pieces, false)
}

/// The text of the label of a paragraph at `index` of a level with `template` (as
/// [`read_template`] reads it), written in decimal when `legal`: each placeholder the
/// value of its level in that level's format, a level with no value its start. A
/// placeholder of a level below the paragraph's makes Word ignore the whole template
/// (MS-OI29500 2.1.283 c). A legal level writes every number in decimal but that of a
/// level without numbers (OI 2.1.278 a, b). The text ends at [`MAX_LABEL_CHARS`], and
/// `cut` says when it does.
fn label_text(
    template: &[Piece],
    legal: bool,
    index: usize,
    shown: &Shown<'_, '_>,
    (unsupported, cut): (&mut bool, &mut bool),
) -> String {
    let mut text = String::new();
    let mut count = 0;
    for piece in template {
        match *piece {
            Piece::Char(ch) => {
                *cut |= push_within(&mut text, &mut count, drawn(ch).into_iter());
            }
            Piece::Level(of) => {
                let of = usize::from(of);
                if of > index {
                    return String::new();
                }
                let of_level = shown.levels.get(of).copied().flatten();
                let value = shown
                    .values
                    .get(of)
                    .copied()
                    .flatten()
                    .or_else(|| shown.starts.get(of).copied().flatten().map(Level::start))
                    .unwrap_or(0);
                let format = of_level.map_or(&NumberFormat::Decimal, Level::format);
                let format = if legal && *format != NumberFormat::None {
                    &NumberFormat::Decimal
                } else {
                    format
                };
                let (number, decimal) = number_text(value, format);
                *unsupported |= decimal;
                *cut |= push_within(&mut text, &mut count, number.chars());
            }
        }
    }
    text
}

/// Adds `chars` to `text`, of which `count` characters are written, up to
/// [`MAX_LABEL_CHARS`]; whether some are left out.
fn push_within(text: &mut String, count: &mut usize, chars: impl Iterator<Item = char>) -> bool {
    for ch in chars {
        if *count == MAX_LABEL_CHARS {
            return true;
        }
        text.push(ch);
        *count += 1;
    }
    false
}

/// What a character of a label's template draws. Word draws U+007F as a bullet: its
/// render of `report-usds-tech-challenge-playbook.docx` shows "•" for bullets of U+007F
/// in Arial, which has no glyph for that character. The other control characters, which
/// the text of runs never holds, and the line and paragraph separators draw nothing, so
/// that none of them breaks the label's line or moves to a tab stop; that is a
/// hypothesis until a Word render.
fn drawn(ch: char) -> Option<char> {
    match ch {
        '\u{7F}' => Some('\u{2022}'),
        '\u{2028}' | '\u{2029}' => None,
        ch if ch.is_control() => None,
        ch => Some(ch),
    }
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
    fn control_characters_of_a_template_are_drawn_as_word_draws_them() {
        // U+007F is Word's bullet (report-usds-tech-challenge-playbook.docx); the others
        // draw nothing.
        let mut document = Document::default();
        let levels = [
            level(&[literal("\u{7F}")], NumberFormat::Bullet),
            level(
                &[
                    literal("\t("),
                    LevelTextPart::Placeholder(1),
                    literal(")\n\r\u{0}\u{85}"),
                ],
                NumberFormat::LowerLetter,
            )
            .with_start(1),
        ];
        let list = document.add_list(ListDefinition::default().with_levels(levels));
        let one = document.add_list_instance(ListInstance::new(list));
        assert_eq!(
            labels(&mut document, &[(one, 0), (one, 1)]),
            ["\u{2022}", "(a)"]
        );
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
        // "a)" at level 2, the first item at level 1 starts at its start. A hypothesis: no
        // reference shows this order since #292 put case 04 at level 0.
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
        assert_eq!(counters.unsupported_format, Some(location.clone()));
        // A later paragraph in that format leaves the report naming the first.
        let mut later = Location::default();
        later.push(crate::report::Step::Index(7));
        let label = counters
            .label(&document, &ListItem::new(one, 0, style), &later)
            .unwrap();
        assert_eq!(label.text, "6");
        assert_eq!(counters.unsupported_format, Some(location));
    }

    #[test]
    fn a_replacing_level_counts_as_the_definition_s_level_does() {
        // Word ignores the start and the restart rule of a level an override replaces
        // (MS-OI29500 2.1.282 b, 2.1.292 b): E-list-label-format, case 04.
        let mut document = Document::default();
        let definition = [
            level(
                &[LevelTextPart::Placeholder(0), literal(".")],
                NumberFormat::Decimal,
            )
            .with_start(3),
            level(
                &[LevelTextPart::Placeholder(1), literal(")")],
                NumberFormat::LowerLetter,
            )
            .with_start(1)
            .with_restart(Restart::Never),
        ];
        let list = document.add_list(ListDefinition::default().with_levels(definition));
        // The replacements show other templates, and leave start and restart at their
        // defaults, as the producer writes them.
        let replace = |of: u8, template: &str| {
            LevelOverride::new(of).with_replacement(Some(level(
                &[LevelTextPart::Placeholder(of), literal(template)],
                NumberFormat::Decimal,
            )))
        };
        let replaced = document.add_list_instance(
            ListInstance::new(list).with_overrides([replace(0, ":"), replace(1, "]")]),
        );
        assert_eq!(
            labels(
                &mut document,
                &[(replaced, 0), (replaced, 1), (replaced, 0), (replaced, 1)]
            ),
            ["3:", "1]", "4:", "2]"]
        );
    }

    #[test]
    fn a_level_restarted_above_shows_its_start_where_a_deeper_label_shows_it() {
        // Levels 0, 1, 1, 1, 0, 2, 1: after "2.", level 1 starts again, so "2.1.1." shows
        // its start, and the next item of level 1 counts on from there.
        let mut document = Document::default();
        let list = document.add_list(outline());
        let one = document.add_list_instance(ListInstance::new(list));
        let items = [
            (one, 0),
            (one, 1),
            (one, 1),
            (one, 1),
            (one, 0),
            (one, 2),
            (one, 1),
        ];
        assert_eq!(
            labels(&mut document, &items),
            ["1.", "1.1.", "1.2.", "1.3.", "2.", "2.1.1.", "2.2."]
        );
    }

    #[test]
    fn a_label_is_cut_at_its_bound_and_the_rest_of_its_template_is_not_written() {
        let mut document = Document::default();
        let long = "ab ".repeat(10_000);
        let many = vec![LevelTextPart::Placeholder(1); 30_000];
        let levels = [
            level(
                &[literal(&long), LevelTextPart::Placeholder(0)],
                NumberFormat::Decimal,
            ),
            level(&many, NumberFormat::UpperLetter).with_start(700),
            level(&[literal("1.\u{2028}x\u{2029}")], NumberFormat::Bullet),
        ];
        let list = document.add_list(ListDefinition::default().with_levels(levels));
        let one = document.add_list_instance(ListInstance::new(list));
        let style = document.add_run_style(RunStyle::default());
        let mut counters = Counters::default();
        let mut location = Location::default();
        location.push(crate::report::Step::Index(2));
        let mut label = |level| {
            counters
                .label(&document, &ListItem::new(one, level, style), &location)
                .unwrap()
                .text
        };
        let first = label(0);
        assert_eq!(first.chars().count(), MAX_LABEL_CHARS);
        assert!(first.starts_with("ab ab "));
        // Placeholders of 27 letters each ("XXX…" for 700): cut after the first
        // characters.
        let second = label(1);
        assert_eq!(second, "X".repeat(MAX_LABEL_CHARS));
        // Line and paragraph separators draw nothing.
        assert_eq!(label(2), "1.x");
        assert_eq!(counters.long_label, Some(location));
    }

    /// The label at level `index` of a list whose level `index` has `template` in
    /// `format`, the other levels nothing, with whether it is reported as cut.
    fn only(index: u8, template: Vec<LevelTextPart>, format: NumberFormat) -> (String, bool) {
        let mut document = Document::default();
        let levels = (0..=index).map(|at| {
            if at == index {
                level(&template, format.clone()).with_start(1)
            } else {
                Level::default().with_start(1)
            }
        });
        let list = document.add_list(ListDefinition::default().with_levels(levels));
        let one = document.add_list_instance(ListInstance::new(list));
        let style = document.add_run_style(RunStyle::default());
        let mut counters = Counters::default();
        let text = counters
            .label(
                &document,
                &ListItem::new(one, index, style),
                &Location::default(),
            )
            .unwrap()
            .text;
        (text, counters.long_label.is_some())
    }

    #[test]
    fn a_template_is_read_up_to_its_bound_whatever_its_pieces_write() {
        let x = || literal("x");
        let many = |part: LevelTextPart, count| std::iter::repeat_n(part, count);
        // Pieces that write nothing count too: numbers of a level without numbers,
        // control characters, empty text. The "x" after them is past the bound.
        let none: Vec<_> = many(LevelTextPart::Placeholder(0), 1_000)
            .chain([x()])
            .collect();
        assert_eq!(only(0, none, NumberFormat::None), (String::new(), true));
        let controls = vec![
            literal(&"\u{85}".repeat(1_000)),
            LevelTextPart::Placeholder(0),
        ];
        assert_eq!(
            only(0, controls, NumberFormat::Decimal),
            (String::new(), true)
        );
        let empty: Vec<_> = many(literal(""), 1_000).chain([x()]).collect();
        assert_eq!(only(0, empty, NumberFormat::Decimal), (String::new(), true));
        // A placeholder counts two, as "%1" does: 127 of them and "xx" are read whole,
        // 128 of them and "x" are not.
        let fits: Vec<_> = many(LevelTextPart::Placeholder(0), 127)
            .chain([literal("xx")])
            .collect();
        assert_eq!(only(0, fits, NumberFormat::None), ("xx".to_owned(), false));
        let past: Vec<_> = many(LevelTextPart::Placeholder(0), 128)
            .chain([x()])
            .collect();
        assert_eq!(only(0, past, NumberFormat::None), (String::new(), true));
        // A placeholder of a deeper level makes Word ignore the template where it is
        // read (MS-OI29500 2.1.283 c), and past the bound it is not.
        let deeper = |before| vec![literal(&"x".repeat(before)), LevelTextPart::Placeholder(1)];
        assert_eq!(
            only(0, deeper(200), NumberFormat::Decimal),
            (String::new(), false)
        );
        assert_eq!(
            only(0, deeper(300), NumberFormat::Decimal),
            ("x".repeat(MAX_LABEL_CHARS), true)
        );
        // The levels a template shows past the bound are not started either: at level 1,
        // a template whose "%1" comes after 300 characters leaves level 0 as it was.
        let mut document = Document::default();
        let shows_above = [
            literal(&"\u{85}".repeat(300)),
            LevelTextPart::Placeholder(0),
        ];
        let list = document.add_list(ListDefinition::default().with_levels([
            level(&[LevelTextPart::Placeholder(0)], NumberFormat::Decimal).with_start(1),
            level(&shows_above, NumberFormat::Decimal).with_start(1),
        ]));
        let one = document.add_list_instance(ListInstance::new(list));
        let style = document.add_run_style(RunStyle::default());
        let mut counters = Counters::default();
        let mut label = |level| {
            counters
                .label(
                    &document,
                    &ListItem::new(one, level, style),
                    &Location::default(),
                )
                .unwrap()
                .text
        };
        assert_eq!(label(1), "");
        assert_eq!(label(0), "1");
    }

    #[test]
    fn the_first_cut_label_is_the_one_reported() {
        let mut document = Document::default();
        let long = level(&[literal(&"x".repeat(300))], NumberFormat::Decimal);
        let list = document.add_list(ListDefinition::default().with_levels([long]));
        let one = document.add_list_instance(ListInstance::new(list));
        let style = document.add_run_style(RunStyle::default());
        let mut counters = Counters::default();
        let at = |index| {
            let mut location = Location::default();
            location.push(crate::report::Step::Index(index));
            location
        };
        for index in [4, 9] {
            counters.label(&document, &ListItem::new(one, 0, style), &at(index));
        }
        assert_eq!(counters.long_label, Some(at(4)));
    }
}
