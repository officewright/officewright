//! The formatting of paragraphs in effect: the document defaults, the paragraph style,
//! the list level and direct formatting (ECMA-376 Part 1, §17.7.2; Word applies the
//! paragraph style before the numbering, MS-OI29500 §17.7.2(a)).

use std::sync::Arc;

use officewright_core::Pt;

use super::{Formats, IGNORED_STYLES, Layer, overwrite, style_index};
use crate::model::Document;
use crate::model::format::{ListFormat, ParagraphFlag, ParagraphFormat};
use crate::model::numbering::ListLevel;
use crate::model::properties::{
    Border, Justification, LineSpacing, ParagraphBorders, ParagraphProperties, Shading, TabKind,
    TabStop,
};
use crate::model::styles::StyleKind;

/// How far a list definition's `w:numStyleLink` is followed: a numbering style names an
/// instance whose definition names the style back (ECMA-376 §17.9.21, §17.9.27).
const MAX_STYLE_LINKS: usize = 4;

impl Formats<'_> {
    /// Combines the levels of the hierarchy for a paragraph whose style has the index
    /// `style` and whose own properties are `direct`.
    pub(super) fn resolve_paragraph(
        &mut self,
        style: Option<usize>,
        direct: &ParagraphProperties,
    ) -> ParagraphFormat {
        let document = self.document;
        let defaults = self.paragraph_defaults();
        let styled = style.map(|index| self.paragraphs.level(document, index));
        let direct = ParagraphLevel::of(direct);
        let style_id = style
            .and_then(|index| document.styles.styles.get(index))
            .map(|style| style.id.clone());

        let list = self.list(&defaults, styled.as_deref(), &direct, style_id.as_deref());

        // What the levels below direct formatting give is the same for every paragraph
        // of one style and list level: it is worked out once, so that a paragraph costs
        // what its own properties hold, however many tab stops it inherits.
        // The list level is the key by its address: instances share the levels of their
        // definition, and so share the entry too.
        let key = (
            style,
            list.as_ref()
                .and_then(|list| list.level.as_ref())
                .map(|level| Arc::as_ptr(level) as usize),
        );
        let inherited = match self.inherited.get(&key) {
            Some(inherited) => Arc::clone(inherited),
            None => {
                let mut inherited = ParagraphLevel::clone(&defaults);
                if let Some(styled) = &styled {
                    inherited.overlay(styled);
                }
                if let Some(level) = list.as_ref().and_then(ListFormat::level) {
                    inherited.overlay(&ParagraphLevel::of_list_level(level));
                }
                // Nothing lies below: a cleared stop that removed nothing goes.
                inherited.tabs = inherited.tabs.without_clears();
                let inherited = Arc::new(inherited);
                self.inherited.insert(key, Arc::clone(&inherited));
                inherited
            }
        };
        let mut effect = ParagraphLevel::clone(&inherited);
        effect.overlay(&direct);
        effect.format(style_id, list)
    }

    /// What the document defaults give to paragraphs.
    fn paragraph_defaults(&mut self) -> Arc<ParagraphLevel> {
        let document = self.document;
        Arc::clone(self.paragraph_defaults.get_or_insert_with(|| {
            Arc::new(ParagraphLevel::of(
                &crate::model::Styles::new(document).default_paragraph_properties(),
            ))
        }))
    }

    /// The list a paragraph is an item of. The instance and the level are properties of
    /// their own, each from the last level that gives it, as Word keeps them. Without a
    /// level in direct formatting, the level is the one whose `w:pStyle` names the
    /// paragraph style (ECMA-376 §17.9.23), else the one the style writes, which Word does
    /// not ignore (MS-OI29500 §17.3.1.19(a)), else 0. Instance 0 is no list, even when a
    /// `w:num` has that id (ECMA-376 §17.9.18).
    fn list(
        &mut self,
        defaults: &ParagraphLevel,
        styled: Option<&ParagraphLevel>,
        direct: &ParagraphLevel,
        style_id: Option<&str>,
    ) -> Option<ListFormat> {
        let inherited = styled
            .and_then(|level| level.list_instance)
            .or(defaults.list_instance);
        let instance = direct.list_instance.or(inherited).filter(|&id| id != 0)?;
        let levels = self.list_levels(instance)?;
        let written_level = direct.list_level.or_else(|| {
            let by_style = style_id.and_then(|id| {
                levels
                    .levels
                    .iter()
                    .flatten()
                    .find(|level| level.paragraph_style() == Some(id))
                    .and_then(|level| i64::try_from(level.level()).ok())
            });
            by_style.or_else(|| {
                styled
                    .and_then(|level| level.list_level)
                    .or(defaults.list_level)
            })
        });
        let level_number = usize::try_from(written_level.unwrap_or(0))
            .ok()
            .filter(|&level| level <= 8)?;
        Some(ListFormat {
            instance,
            definition: levels.definition,
            level_number,
            level: levels.levels.get(level_number).cloned().flatten(),
            start: levels.starts.get(level_number).copied().unwrap_or(0),
        })
    }

    /// The levels of the list instance `instance` with its overrides applied, worked out
    /// once per instance.
    fn list_levels(&mut self, instance: i64) -> Option<Arc<ListLevels>> {
        if let Some(levels) = self.lists.get(&instance) {
            return levels.clone();
        }
        let levels = self.instance_levels(instance).map(Arc::new);
        self.lists.insert(instance, levels.clone());
        levels
    }

    /// The levels of the list instance `instance`; none when the document has no such
    /// instance. The instance shares the definition's parsed levels and has copies only
    /// of those it overrides, so that many instances of one definition cost little.
    fn instance_levels(&mut self, instance: i64) -> Option<ListLevels> {
        let document = self.document;
        let num = document.numbering().instance(instance)?;
        let shared = num
            .definition_id()
            .map(|id| self.definition_levels(id))
            .unwrap_or_default();
        let overrides = num.overrides();
        let mut levels = shared.levels.clone();
        let mut starts = [0; 9];
        for (number, (slot, start)) in levels.iter_mut().zip(&mut starts).enumerate() {
            let override_ = overrides
                .iter()
                .find(|over| usize::try_from(over.level()).ok() == Some(number));
            // An override's own level replaces the definition's, but Word ignores its
            // start and restart (MS-OI29500 §17.9.25(b), §17.9.10(b)).
            if let Some(replacement) = override_.and_then(|over| over.definition()) {
                let base = slot.as_deref();
                let mut replacement = replacement.clone();
                replacement.level = number;
                replacement.start = base.and_then(ListLevel::start);
                replacement.restart = base.and_then(ListLevel::restart);
                *slot = Some(Arc::new(replacement));
            }
            *start = override_
                .and_then(|over| over.start())
                .or_else(|| slot.as_deref().and_then(ListLevel::start))
                .unwrap_or(0);
        }
        Some(ListLevels {
            definition: shared.definition,
            levels,
            starts,
        })
    }

    /// The parsed levels of the definition an instance names by `w:abstractNumId`,
    /// through a numbering style's link, worked out once per definition.
    fn definition_levels(&mut self, id: i64) -> Arc<DefinitionLevels> {
        if let Some(levels) = self.definitions.get(&id) {
            return Arc::clone(levels);
        }
        let document = self.document;
        let definition = document
            .numbering()
            .definition(id)
            .and_then(|definition| definition_with_levels(document, definition));
        let mut levels: [Option<Arc<ListLevel>>; 9] = Default::default();
        for level in definition
            .map(|definition| definition.levels())
            .unwrap_or_default()
        {
            if let Some(slot) = levels.get_mut(level.level()) {
                *slot = Some(Arc::new(level));
            }
        }
        let levels = Arc::new(DefinitionLevels {
            definition: definition.map(|definition| definition.id()),
            levels,
        });
        self.definitions.insert(id, Arc::clone(&levels));
        levels
    }
}

/// The parsed levels of a list definition, 0 to 8.
#[derive(Debug, Default)]
pub(super) struct DefinitionLevels {
    /// The definition whose levels these are, after a numbering style's link; none when
    /// there is no such definition or the link leads nowhere.
    definition: Option<i64>,
    levels: [Option<Arc<ListLevel>>; 9],
}

/// The levels of a list instance, 0 to 8, with its overrides applied.
#[derive(Debug)]
pub(super) struct ListLevels {
    /// The definition whose levels apply.
    definition: Option<i64>,
    levels: [Option<Arc<ListLevel>>; 9],
    /// The number each level starts from.
    starts: [i64; 9],
}

/// The definition whose levels apply for the definition `start`: through
/// `w:numStyleLink`, the numbering style's instance's definition (ECMA-376 §17.9.21).
/// None when the link leads nowhere, loops, or names a style whose children Word
/// ignores.
fn definition_with_levels<'a>(
    document: &'a Document,
    start: crate::model::ListDefinition<'a>,
) -> Option<crate::model::ListDefinition<'a>> {
    let mut definition = start;
    for _ in 0..MAX_STYLE_LINKS {
        let Some(link) = definition.numbering_style_link() else {
            return Some(definition);
        };
        if IGNORED_STYLES.contains(&link) {
            return None;
        }
        let style = style_index(document, link, StyleKind::Numbering)
            .and_then(|index| document.styles.styles.get(index))
            .map(|style| crate::model::Style::from_data(document, style))?;
        let instance = style.paragraph_properties().numbering?.instance?;
        definition = document.numbering().instance(instance)?.definition()?;
    }
    None
}

/// What one level of the hierarchy gives to a paragraph, or what several give together.
#[derive(Clone, Debug, Default)]
pub(super) struct ParagraphLevel {
    flags: [Option<bool>; ParagraphFlag::COUNT],
    justification: Option<Justification>,
    start: Option<Pt>,
    end: Option<Pt>,
    /// Negative for a hanging indent.
    first_line: Option<Pt>,
    /// Character units, hundredths; 0 cancels those of lower levels (MS-OI29500
    /// §17.3.1.12(a)).
    start_chars: Option<i64>,
    end_chars: Option<i64>,
    first_line_chars: Option<i64>,
    before: Option<Pt>,
    after: Option<Pt>,
    /// Line units, hundredths; 0 cancels those of lower levels.
    before_lines: Option<i64>,
    after_lines: Option<i64>,
    before_auto: Option<bool>,
    after_auto: Option<bool>,
    line: Option<LineSpacing>,
    /// top, bottom, start, end, between, bar.
    borders: [Option<Border>; 6],
    shading: Option<Shading>,
    tabs: Tabs,
    outline_level: Option<i64>,
    list_instance: Option<i64>,
    list_level: Option<i64>,
}

impl ParagraphLevel {
    /// What one `w:pPr` gives.
    fn of(properties: &ParagraphProperties) -> Self {
        let flags = [
            properties.keep_next,
            properties.keep_lines,
            properties.page_break_before,
            properties.widow_control,
            properties.suppress_line_numbers,
            properties.suppress_auto_hyphens,
            properties.contextual_spacing,
            properties.mirror_indents,
            properties.word_wrap,
            properties.right_to_left,
            properties.snap_to_grid,
        ];
        let mut level = Self {
            flags,
            justification: properties.justification,
            shading: properties.shading,
            tabs: Tabs::of(&properties.tabs),
            outline_level: properties.outline_level,
            list_instance: properties
                .numbering
                .and_then(|numbering| numbering.instance),
            list_level: properties.numbering.and_then(|numbering| numbering.level),
            ..Self::default()
        };
        level.indentation(properties);
        if let Some(spacing) = &properties.spacing {
            level.before = spacing.before;
            level.after = spacing.after;
            level.before_lines = spacing.before_lines;
            level.after_lines = spacing.after_lines;
            level.before_auto = spacing.before_auto;
            level.after_auto = spacing.after_auto;
            level.line = spacing.line;
        }
        if let Some(borders) = &properties.borders {
            level.borders = [
                borders.top,
                borders.bottom,
                borders.start,
                borders.end,
                borders.between,
                borders.bar,
            ];
        }
        level
    }

    /// What a list level gives to its paragraphs: Word takes only the alignment, the
    /// indentation and the tab stops of its `w:pPr` (MS-OI29500 §17.9.22(b)).
    fn of_list_level(list: &ListLevel) -> Self {
        let properties = list.paragraph_properties();
        let mut level = Self {
            justification: properties.justification,
            tabs: Tabs::of(&properties.tabs),
            ..Self::default()
        };
        level.indentation(properties);
        level
    }

    /// The indentation of one `w:ind`: a hanging indent wins over a first line indent,
    /// and character units over points (ECMA-376 §17.3.1.12).
    fn indentation(&mut self, properties: &ParagraphProperties) {
        let Some(ind) = &properties.indentation else {
            return;
        };
        self.start = ind.start;
        self.end = ind.end;
        self.first_line = ind.hanging.map(|hanging| Pt(-hanging.0)).or(ind.first_line);
        self.start_chars = ind.start_chars;
        self.end_chars = ind.end_chars;
        self.first_line_chars = ind
            .hanging_chars
            .map(|hanging| -hanging)
            .or(ind.first_line_chars);
    }

    /// The format in effect once every level is laid over the others.
    fn format(self, style: Option<Box<str>>, list: Option<ListFormat>) -> ParagraphFormat {
        let mut flags = [false; ParagraphFlag::COUNT];
        for (index, slot) in flags.iter_mut().enumerate() {
            let default = [
                ParagraphFlag::WidowControl,
                ParagraphFlag::WordWrap,
                ParagraphFlag::SnapToGrid,
            ]
            .iter()
            .any(|flag| *flag as usize == index);
            *slot = self.flags.get(index).copied().flatten().unwrap_or(default);
        }
        // A character or line unit other than 0 wins over points, whichever level gives
        // the points (MS-OI29500 §17.3.1.12(b), §17.3.1.33(a)).
        let units = |value: Option<i64>| value.filter(|&value| value != 0);
        let [top, bottom, start, end, between, bar] = self.borders;
        let tabs = self.tabs.without_clears().stops;
        // Word gives the outline level of its built-in headings by their style ids.
        let heading = style
            .as_deref()
            .and_then(|id| id.strip_prefix("Heading"))
            .filter(|number| number.len() == 1)
            .and_then(|number| number.parse::<u8>().ok())
            .filter(|number| (1..=9).contains(number))
            .map(|number| i64::from(number - 1));
        let outline_level = heading
            .or(self.outline_level)
            .and_then(|level| u8::try_from(level).ok())
            .filter(|&level| level <= 8);
        ParagraphFormat {
            style,
            justification: self.justification.unwrap_or(Justification::Start),
            start_indent: self.start.unwrap_or(Pt(0.0)),
            end_indent: self.end.unwrap_or(Pt(0.0)),
            first_line_indent: self.first_line.unwrap_or(Pt(0.0)),
            start_indent_chars: units(self.start_chars),
            end_indent_chars: units(self.end_chars),
            first_line_indent_chars: units(self.first_line_chars),
            before_spacing: self.before.unwrap_or(Pt(0.0)),
            after_spacing: self.after.unwrap_or(Pt(0.0)),
            before_spacing_lines: units(self.before_lines),
            after_spacing_lines: units(self.after_lines),
            before_auto_spacing: self.before_auto.unwrap_or(false),
            after_auto_spacing: self.after_auto.unwrap_or(false),
            line_spacing: self.line.unwrap_or(LineSpacing::Multiple(1.0)),
            flags,
            borders: ParagraphBorders {
                top,
                bottom,
                start,
                end,
                between,
                bar,
            },
            shading: self.shading,
            tabs,
            outline_level,
            list,
        }
    }
}

impl Layer for ParagraphLevel {
    fn of_style(document: &Document, index: usize) -> Self {
        document
            .styles
            .styles
            .get(index)
            .map(|style| {
                Self::of(&crate::model::Style::from_data(document, style).paragraph_properties())
            })
            .unwrap_or_default()
    }

    fn overlay(&mut self, above: &Self) {
        for (slot, value) in self.flags.iter_mut().zip(above.flags) {
            overwrite(slot, value);
        }
        overwrite(&mut self.justification, above.justification);
        overwrite(&mut self.start, above.start);
        overwrite(&mut self.end, above.end);
        overwrite(&mut self.first_line, above.first_line);
        overwrite(&mut self.start_chars, above.start_chars);
        overwrite(&mut self.end_chars, above.end_chars);
        overwrite(&mut self.first_line_chars, above.first_line_chars);
        overwrite(&mut self.before, above.before);
        overwrite(&mut self.after, above.after);
        overwrite(&mut self.before_lines, above.before_lines);
        overwrite(&mut self.after_lines, above.after_lines);
        overwrite(&mut self.before_auto, above.before_auto);
        overwrite(&mut self.after_auto, above.after_auto);
        overwrite(&mut self.line, above.line);
        for (slot, border) in self.borders.iter_mut().zip(above.borders) {
            overwrite(slot, border);
        }
        overwrite(&mut self.shading, above.shading);
        self.tabs = self.tabs.overlay(&above.tabs);
        overwrite(&mut self.outline_level, above.outline_level);
        overwrite(&mut self.list_instance, above.list_instance);
        overwrite(&mut self.list_level, above.list_level);
    }
}

/// Tab stops set and cleared, by position, one per position. A cleared stop stays, to
/// remove the stop a lower level sets at its position, until nothing lies below.
#[derive(Clone, Debug, Default)]
struct Tabs {
    stops: Arc<[TabStop]>,
    /// Whether a stop is a cleared one.
    clears: bool,
}

impl Tabs {
    /// The stops of one `w:tabs`, the last of several at one position winning.
    fn of(tabs: &[TabStop]) -> Self {
        let mut sorted = tabs.to_vec();
        // A stable sort keeps stops at one position in the order written.
        sorted.sort_by(|a, b| a.position.0.total_cmp(&b.position.0));
        let mut unique: Vec<TabStop> = Vec::with_capacity(sorted.len());
        for tab in sorted {
            match unique.last_mut() {
                Some(last) if last.position == tab.position => *last = tab,
                _ => unique.push(tab),
            }
        }
        Self::new(unique)
    }

    fn new(stops: Vec<TabStop>) -> Self {
        let clears = stops.iter().any(is_cleared);
        Self {
            stops: stops.into(),
            clears,
        }
    }

    /// These stops with `above` laid over them: the stops add up, a stop at a position
    /// replaces the one below it, and a cleared one removes it (ECMA-376 §17.3.1.38).
    /// Both lists are by position, so one pass merges them; a level without stops costs
    /// nothing.
    fn overlay(&self, above: &Tabs) -> Tabs {
        if above.stops.is_empty() {
            return self.clone();
        }
        if self.stops.is_empty() {
            return above.clone();
        }
        let mut below = self.stops.iter().copied().peekable();
        let mut merged = Vec::with_capacity(self.stops.len() + above.stops.len());
        for tab in above.stops.iter() {
            while let Some(lower) = below.next_if(|lower| lower.position.0 < tab.position.0) {
                merged.push(lower);
            }
            below.next_if(|lower| lower.position.0 == tab.position.0);
            merged.push(*tab);
        }
        merged.extend(below);
        Tabs::new(merged)
    }

    /// The stops without the cleared ones, for when nothing lies below.
    fn without_clears(&self) -> Tabs {
        if !self.clears {
            return self.clone();
        }
        Tabs::new(
            self.stops
                .iter()
                .filter(|tab| !is_cleared(tab))
                .copied()
                .collect(),
        )
    }
}

fn is_cleared(tab: &TabStop) -> bool {
    tab.kind == Some(TabKind::Clear)
}
