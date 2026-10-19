//! List definitions and instances as the IR's lists, added when a paragraph first uses
//! them. The layout counts the items and builds the label text from them; the level's
//! indents and tab stops are already in the paragraph's formatting.

use std::collections::HashMap;

use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::Translator;
use super::section::number_format;
use crate::model::{Justification, LevelSuffix, ListLevel, NumberFormat};

/// The IR's lists added so far, by the ids of the document's.
#[derive(Default)]
pub(super) struct Lists {
    /// By `w:abstractNumId`.
    definitions: HashMap<i64, ir::ListId>,
    /// By `w:numId`; none for an instance whose definition does not exist.
    instances: HashMap<i64, Option<ir::ListInstanceId>>,
    /// Whether the label of a level of an instance draws something, by `w:numId` and the
    /// level: worked out once each.
    draws: HashMap<(i64, u8), bool>,
}

impl Translator<'_> {
    /// The IR instance of the document's list instance `id`, with its definition, added
    /// the first time a paragraph uses it; none when it uses no definition that exists.
    pub(super) fn list_instance(&mut self, id: i64) -> Option<ir::ListInstanceId> {
        if let Some(&found) = self.lists.instances.get(&id) {
            return found;
        }
        let numbering = self.document.numbering();
        let found = numbering.instance(id).and_then(|instance| {
            // The definition an instance uses, after a numbering style's link, as the
            // paragraph formatting resolves it.
            let definition = self.formats.list_definition(id)?;
            let definition = self.list_definition(definition)?;
            let overrides = instance.overrides().into_iter().filter_map(|over| {
                let level = u8::try_from(over.level())
                    .ok()
                    .filter(|level| *level <= 8)?;
                Some(
                    ir::LevelOverride::new(level)
                        .with_start(over.start().map(start))
                        // Word ignores a replacing level's start and restart rule
                        // (MS-OI29500 2.1.282 b, 2.1.292 b): the IR wants its defaults.
                        .with_replacement(over.definition().map(|replacement| {
                            self.level(replacement)
                                .with_start(0)
                                .with_restart(ir::Restart::Default)
                        })),
                )
            });
            let overrides: Vec<_> = overrides.collect();
            Some(
                self.ir
                    .add_list_instance(ir::ListInstance::new(definition).with_overrides(overrides)),
            )
        });
        self.lists.instances.insert(id, found);
        found
    }

    /// Whether the label of an item at `level` of the document's list instance `id`
    /// draws something, as the layout writes it: whether the first [`MAX_LABEL_CHARS`]
    /// counted of its level's text hold a character it draws, or a placeholder of a level
    /// that writes its number, and no placeholder of a deeper level, which makes Word
    /// ignore the whole text (MS-OI29500 2.1.283 c). Worked out once for each instance
    /// and level, from the IR's levels, so that the cost does not grow with the items or
    /// the length of a text.
    pub(super) fn label_draws(&mut self, id: i64, level: u8) -> bool {
        if let Some(&draws) = self.lists.draws.get(&(id, level)) {
            return draws;
        }
        #[cfg(test)]
        super::work(|work| work.label_checks += 1);
        let draws = self
            .list_instance(id)
            .is_some_and(|instance| draws(&self.ir, instance, level));
        self.lists.draws.insert((id, level), draws);
        draws
    }

    /// The IR definition of the document's definition `id`, added the first time.
    fn list_definition(&mut self, id: i64) -> Option<ir::ListId> {
        if let Some(&found) = self.lists.definitions.get(&id) {
            return Some(found);
        }
        let definition = self.document.numbering().definition(id)?;
        // Levels the definition leaves out are empty levels.
        let mut levels = vec![ir::Level::default(); 9];
        let mut last = None;
        for level in definition.levels() {
            if let Some(slot) = levels.get_mut(level.level()) {
                *slot = self.level(&level);
                last = Some(level.level());
            }
        }
        levels.truncate(last.map_or(0, |last| last + 1));
        let found = self
            .ir
            .add_list(ir::ListDefinition::default().with_levels(levels));
        self.lists.definitions.insert(id, found);
        Some(found)
    }

    /// A level of a definition or an override, field by field.
    fn level(&mut self, level: &ListLevel) -> ir::Level {
        let format = level.number_format().unwrap_or(NumberFormat::Decimal);
        let template = level.text().unwrap_or_default();
        let text = if format == NumberFormat::Bullet {
            // A bullet's text is shown as it is written (MS-OI29500 §17.9.17(a)).
            vec![ir::LevelTextPart::Literal(template.to_owned())]
        } else {
            label_text(template)
        };
        if level.picture_bullet().is_some() {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "a picture bullet is left out; the level's text stands in".into()
            });
        }
        let restart = match level.restart() {
            None => ir::Restart::Default,
            Some(0) => ir::Restart::Never,
            // Word takes 0 to 7 (MS-OI29500 §17.9.10(a)).
            Some(restart @ 1..=7) => {
                u8::try_from(restart - 1).map_or(ir::Restart::Default, ir::Restart::AfterLevel)
            }
            Some(_) => ir::Restart::Default,
        };
        ir::Level::default()
            .with_start(level.start().map_or(0, start))
            .with_format(number_format(format))
            .with_text(text)
            .with_legal(level.legal().unwrap_or(false))
            .with_restart(restart)
            .with_suffix(match level.suffix() {
                None | Some(LevelSuffix::Tab) => ir::Suffix::Tab,
                Some(LevelSuffix::Space) => ir::Suffix::Space,
                Some(LevelSuffix::Nothing) => ir::Suffix::Nothing,
            })
            .with_alignment(match level.justification() {
                Some(Justification::Center) => ir::LevelAlignment::Center,
                Some(Justification::End | Justification::Right) => ir::LevelAlignment::End,
                // Word takes only start, center and end (MS-OI29500 §17.9.7(a)).
                _ => ir::LevelAlignment::Start,
            })
    }
}

/// The layout's bound on what it reads of a level's text, counted as it counts: a
/// character one, a placeholder two, a piece of text with no characters one.
const MAX_LABEL_CHARS: usize = 256;

/// Whether the label of an item at `index` of `instance` draws something; see
/// [`Translator::label_draws`].
fn draws(ir: &ir::Document, instance: ir::ListInstanceId, index: u8) -> bool {
    let Some(instance) = ir.list_instance(instance) else {
        return false;
    };
    let levels = ir
        .list(instance.definition())
        .map(ir::ListDefinition::levels)
        .unwrap_or_default();
    let level_at = |at: u8| {
        instance
            .overrides()
            .iter()
            .find(|over| over.level() == at)
            .and_then(|over| over.replacement().as_ref())
            .or_else(|| levels.get(usize::from(at)))
    };
    let Some(level) = level_at(index) else {
        return false;
    };
    let mut left = MAX_LABEL_CHARS;
    let mut draws = false;
    'text: for part in level.text() {
        match part {
            ir::LevelTextPart::Literal(text) if text.is_empty() => {
                if left == 0 {
                    break;
                }
                left -= 1;
            }
            ir::LevelTextPart::Literal(text) => {
                for ch in text.chars() {
                    if left == 0 {
                        break 'text;
                    }
                    left -= 1;
                    // The layout draws U+007F as a bullet, and no other control character
                    // or line or paragraph separator.
                    draws |= ch == '\u{7F}'
                        || !(ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'));
                }
            }
            ir::LevelTextPart::Placeholder(of) => {
                if left < 2 {
                    break;
                }
                left -= 2;
                if *of > index {
                    return false;
                }
                // A missing level is written in decimal; a legal level writes every
                // number in decimal but that of a level without numbers.
                let format = level_at(*of).map_or(&ir::NumberFormat::Decimal, ir::Level::format);
                let silent = matches!(format, ir::NumberFormat::None)
                    || (!level.legal() && matches!(format, ir::NumberFormat::Bullet));
                draws |= !silent;
            }
            // A part of a later version of the IR draws nothing this one knows of.
            _ => {}
        }
    }
    draws
}

/// A start number; Word does not count below 0.
fn start(start: i64) -> u32 {
    u32::try_from(start.max(0)).unwrap_or(u32::MAX)
}

/// The parts of a level's text: `%1` to `%9` are the numbers of levels 0 to 8, the rest
/// is shown as it is.
fn label_text(template: &str) -> Vec<ir::LevelTextPart> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        let level = chars
            .peek()
            .and_then(|next| next.to_digit(10))
            .filter(|digit| (1..=9).contains(digit));
        match (c, level) {
            ('%', Some(digit)) => {
                chars.next();
                if !literal.is_empty() {
                    parts.push(ir::LevelTextPart::Literal(std::mem::take(&mut literal)));
                }
                if let Ok(level) = u8::try_from(digit - 1) {
                    parts.push(ir::LevelTextPart::Placeholder(level));
                }
            }
            (c, _) => literal.push(c),
        }
    }
    if !literal.is_empty() {
        parts.push(ir::LevelTextPart::Literal(literal));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_level_text_into_numbers_and_literals() {
        use ir::LevelTextPart::{Literal, Placeholder};
        let literal = |text: &str| Literal(text.to_owned());
        let cases = [
            ("%1.", vec![Placeholder(0), literal(".")]),
            ("(%2)", vec![literal("("), Placeholder(1), literal(")")]),
            (
                "%1.%2.%3.",
                vec![
                    Placeholder(0),
                    literal("."),
                    Placeholder(1),
                    literal("."),
                    Placeholder(2),
                    literal("."),
                ],
            ),
            ("Article %1", vec![literal("Article "), Placeholder(0)]),
            // Not a level number: %0, a % at the end, and %% before a number.
            ("%0%", vec![literal("%0%")]),
            ("%%1", vec![literal("%"), Placeholder(0)]),
            ("%9%10", vec![Placeholder(8), Placeholder(0), literal("0")]),
            ("", vec![]),
        ];
        for (template, parts) in cases {
            assert_eq!(label_text(template), parts, "{template}");
        }
    }
}
