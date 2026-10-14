//! The formatting of paragraphs in effect: the document defaults, the paragraph style,
//! the list level and direct formatting (ECMA-376 Part 1, §17.7.2; Word applies the
//! paragraph style before the numbering, MS-OI29500 §17.7.2(a)).

use std::collections::HashSet;
use std::sync::Arc;

use officewright_core::Pt;

use super::character::CharacterLevel;
use super::{Formats, IGNORED_STYLES, Layer, MAX_COMBINATIONS, overwrite, style_index};
use crate::model::Document;
use crate::model::format::{ListFormat, ParagraphFlag, ParagraphFormat};
use crate::model::numbering::ListLevel;
use crate::model::properties::{
    Border, Justification, LineSpacing, ParagraphBorders, ParagraphProperties, Shading, TabKind,
    TabStop,
};
use crate::model::styles::StyleKind;

/// How many tab stops a level, and a paragraph, keeps, those at the lowest positions:
/// at most this many set and this many cleared. Word's own format adds at most 64 stops
/// and removes at most 64 in one change ([MS-DOC] 2.9.179 PChgTabsAdd, 2.9.180
/// PChgTabsDel); without a bound, stops that every paragraph of a style inherits would
/// cost each paragraph their number in time and memory.
const MAX_TABS: usize = 64;

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
        // The defaults with the style and the styles it is based on laid over them,
        // worked out once per style.
        let base = match style {
            Some(index) => self.paragraphs.level(document, index, &defaults),
            None => defaults,
        };
        let direct = ParagraphLevel::of(direct);
        let style_id = style
            .and_then(|index| document.styles.styles.get(index))
            .map(|style| Arc::clone(&style.id));

        let (list, below_direct) = match self.list(&base, &direct, style) {
            Some((
                list,
                InstanceLevel {
                    layer: Some(layer), ..
                },
            )) => {
                let combined = self.combination(style, &base, &layer);
                (Some(list), combined)
            }
            Some((list, _)) => (Some(list), base),
            None => (None, base),
        };
        let mut effect = ParagraphLevel::clone(&below_direct);
        effect.overlay(&direct);
        effect.format(style_id, list)
    }

    /// What the list level of a paragraph whose style has the index `style` and whose
    /// own properties are `direct` gives to its label: none when the paragraph is not a
    /// list item, `Some(None)` when the level gives the label nothing.
    pub(super) fn label_level(
        &mut self,
        style: Option<usize>,
        direct: &ParagraphProperties,
    ) -> Option<Option<Arc<CharacterLevel>>> {
        let document = self.document;
        let defaults = self.paragraph_defaults();
        let base = match style {
            Some(index) => self.paragraphs.level(document, index, &defaults),
            None => defaults,
        };
        let direct = ParagraphLevel::of(direct);
        let (_, level) = self.list(&base, &direct, style)?;
        Some(level.label)
    }

    /// The definition the list instance `instance` uses, after a numbering style's
    /// link, as paragraphs resolve it; none when there is no such instance or definition.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn list_definition(&mut self, instance: i64) -> Option<i64> {
        self.instance(instance)?.definition
    }

    /// What the document defaults give to paragraphs.
    fn paragraph_defaults(&mut self) -> Arc<ParagraphLevel> {
        let document = self.document;
        Arc::clone(self.paragraph_defaults.get_or_insert_with(|| {
            let mut defaults = ParagraphLevel::of(
                &crate::model::Styles::new(document).default_paragraph_properties(),
            );
            defaults.settle();
            Arc::new(defaults)
        }))
    }

    /// `base`, the defaults with the paragraph style at `style` laid over them, with the
    /// layer of a list level laid over that. Kept for the next paragraph of that style
    /// and level, whichever instance of the definition it is in, as long as the cache
    /// has room: a combination costs a merge of tab stops.
    fn combination(
        &mut self,
        style: Option<usize>,
        base: &ParagraphLevel,
        layer: &SourcedLayer,
    ) -> Arc<ParagraphLevel> {
        let key = (style, layer.source);
        if let Some(combined) = self.combinations.get(&key) {
            return Arc::clone(combined);
        }
        #[cfg(test)]
        {
            self.work.combinations += 1;
        }
        let mut combined = base.clone();
        combined.overlay_list(&layer.layer);
        combined.settle();
        let combined = Arc::new(combined);
        if self.combinations.len() >= MAX_COMBINATIONS {
            self.combinations.clear();
        }
        self.combinations.insert(key, Arc::clone(&combined));
        combined
    }

    /// The list a paragraph is an item of, and what its level gives to the paragraph.
    /// The instance and the level are properties of their own, each from the last level
    /// that gives it, as Word keeps them. Without a level in direct formatting, the level
    /// is the one whose `w:pStyle` names the paragraph style (ECMA-376 §17.9.23), else the
    /// one the style writes, which Word does not ignore (MS-OI29500 §17.3.1.19(a)), else
    /// 0. Instance 0 is no list, even when a `w:num` has that id (ECMA-376 §17.9.18).
    fn list(
        &mut self,
        base: &ParagraphLevel,
        direct: &ParagraphLevel,
        style: Option<usize>,
    ) -> Option<(ListFormat, InstanceLevel)> {
        let instance = direct
            .list_instance
            .or(base.list_instance)
            .filter(|&id| id != 0)?;
        let found = self.instance(instance)?;
        let written_level = direct.list_level.or_else(|| {
            style
                .and_then(|style| found.shared.level_of_style(style))
                .or(base.list_level)
        });
        let level_number = usize::try_from(written_level.unwrap_or(0))
            .ok()
            .filter(|&level| level <= 8)?;
        let level = self.instance_level(instance, &found, level_number);
        let list = ListFormat {
            instance,
            definition: found.definition,
            level_number,
            start: level.start,
        };
        Some((list, level))
    }

    /// The list instance `instance` and the definition whose levels it uses, worked out
    /// once per instance; none when the document has no such instance.
    fn instance(&mut self, instance: i64) -> Option<Arc<InstanceDefinition>> {
        if let Some(found) = self.instances.get(&instance) {
            return found.clone();
        }
        let document = self.document;
        let found = document.numbering().instance(instance).map(|num| {
            let definition = num
                .definition_id()
                .and_then(|id| self.linked_definition(id));
            let shared = definition
                .map(|id| self.definition_levels(id))
                .unwrap_or_default();
            Arc::new(InstanceDefinition { definition, shared })
        });
        self.instances.insert(instance, found.clone());
        found
    }

    /// The definition whose levels apply for the definition `id` an instance names:
    /// through `w:numStyleLink`, the numbering style's instance's definition (ECMA-376
    /// §17.9.21), reading only the style's `w:numPr`. None when there is no such
    /// definition or the link leads nowhere, comes back to a definition on the way, or
    /// names a style whose children Word ignores.
    ///
    /// The outcome is kept for every definition on the way, so that each definition's
    /// link is read once; a definition's outcome is the outcome of the one it links to,
    /// whichever definition the walk started from.
    fn linked_definition(&mut self, id: i64) -> Option<i64> {
        let document = self.document;
        let numbering = document.numbering();
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        let mut current = Some(id);
        let linked = loop {
            let Some(at) = current else {
                break None;
            };
            if let Some(linked) = self.links.get(&at) {
                break *linked;
            }
            if !seen.insert(at) {
                break None;
            }
            path.push(at);
            #[cfg(test)]
            {
                self.work.links += 1;
            }
            let Some(definition) = numbering.definition(at) else {
                break None;
            };
            let Some(link) = definition.numbering_style_link() else {
                break Some(at);
            };
            if IGNORED_STYLES.contains(&link) {
                break None;
            }
            current = style_index(document, link, StyleKind::Numbering)
                .and_then(|index| self.numbering_style_instance(index))
                // Instance 0 is no list (ECMA-376 §17.9.18), even when a `w:num` has
                // that id.
                .filter(|&instance| instance != 0)
                .and_then(|instance| numbering.instance(instance))
                .and_then(|instance| instance.definition_id());
        };
        for at in path {
            self.links.insert(at, linked);
        }
        linked
    }

    /// The list instance a numbering style names in its `w:numPr`, read once per style:
    /// reading it walks the style's `w:pPr`, which can be large.
    fn numbering_style_instance(&mut self, index: usize) -> Option<i64> {
        if let Some(instance) = self.numbering_styles.get(&index) {
            return *instance;
        }
        #[cfg(test)]
        {
            self.work.numbering_styles += 1;
        }
        let document = self.document;
        let instance = document.styles.styles.get(index).and_then(|style| {
            let style = document.element(style.element);
            let p_pr = style
                .children_through_alternates(document.names.mc)
                .find(|child| child.is(&document.names.w, "pPr"))?;
            crate::docx::properties::numbering_of(p_pr, &document.names)?.instance
        });
        self.numbering_styles.insert(index, instance);
        instance
    }

    /// The start of each level of the definition `id` and the style it names, worked
    /// out once and shared by every instance that uses it. The levels are parsed and
    /// dropped: what a level gives to paragraphs and to their labels is kept only for
    /// the levels paragraphs use ([`level_gives`](Self::level_gives)).
    fn definition_levels(&mut self, id: i64) -> Arc<DefinitionLevels> {
        if let Some(levels) = self.definitions.get(&id) {
            return Arc::clone(levels);
        }
        #[cfg(test)]
        {
            self.work.definitions += 1;
        }
        let document = self.document;
        let mut levels: [Option<DefinitionLevel>; 9] = Default::default();
        let written = document
            .numbering()
            .definition(id)
            .map(|definition| definition.levels())
            .unwrap_or_default();
        for level in written {
            if let Some(slot) = levels.get_mut(level.level()) {
                *slot = Some(DefinitionLevel {
                    start: level.start(),
                    style: level
                        .paragraph_style()
                        .and_then(|id| style_index(document, id, StyleKind::Paragraph)),
                });
            }
        }
        let levels = Arc::new(DefinitionLevels { levels });
        self.definitions.insert(id, Arc::clone(&levels));
        levels
    }

    /// What level `number` of the instance `instance` gives to paragraphs and to their
    /// labels, with the instance's override applied, and the number it starts from, worked
    /// out once per instance and level that a paragraph uses. Only the instance's first
    /// override of that level is read, and none when the instance's definition does not
    /// exist: such an instance is a list without levels. Without a replacement level the
    /// entry shares the definition's.
    fn instance_level(
        &mut self,
        instance: i64,
        found: &InstanceDefinition,
        number: usize,
    ) -> InstanceLevel {
        if let Some(level) = self.instance_levels.get(&(instance, number)) {
            return level.clone();
        }
        if found.definition.is_none() {
            return InstanceLevel::default();
        }
        let document = self.document;
        let base = found.shared.levels.get(number).and_then(Option::as_ref);
        let override_ = document
            .numbering()
            .instance(instance)
            .and_then(|num| num.override_of(number));
        // An override's own level replaces the definition's, but Word ignores its start
        // and restart (MS-OI29500 §17.9.25(b), §17.9.10(b)).
        let (layer, label) = match override_.as_ref().and_then(|over| over.definition()) {
            Some(replacement) => {
                #[cfg(test)]
                {
                    self.work.replacements += 1;
                }
                let layer = ListLayer::of(replacement).map(|layer| SourcedLayer {
                    source: LayerSource::Override(instance, number),
                    layer,
                });
                (
                    layer,
                    CharacterLevel::of_label(replacement.run_properties()),
                )
            }
            None if base.is_some() => match found.definition {
                Some(definition) => {
                    let gives = self.level_gives(definition, number);
                    let layer = gives.layer.map(|layer| SourcedLayer {
                        source: LayerSource::Definition(definition, number),
                        layer,
                    });
                    (layer, gives.label)
                }
                None => (None, None),
            },
            None => (None, None),
        };
        let start = override_
            .as_ref()
            .and_then(|over| over.start())
            .or_else(|| base.and_then(|base| base.start))
            .unwrap_or(0);
        let level = InstanceLevel {
            layer,
            start,
            label,
        };
        self.instance_levels
            .insert((instance, number), level.clone());
        level
    }

    /// What level `number` of the definition `definition` gives to paragraphs and to
    /// their labels, worked out once per definition and level that a paragraph uses.
    fn level_gives(&mut self, definition: i64, number: usize) -> LevelGives {
        if let Some(gives) = self.level_layers.get(&(definition, number)) {
            return gives.clone();
        }
        #[cfg(test)]
        {
            self.work.level_layers += 1;
        }
        let level = self
            .document
            .numbering()
            .definition(definition)
            .and_then(|definition| definition.level(number));
        let gives = LevelGives {
            layer: level.as_ref().and_then(ListLayer::of),
            label: level
                .as_ref()
                .and_then(|level| CharacterLevel::of_label(level.run_properties())),
        };
        self.level_layers
            .insert((definition, number), gives.clone());
        gives
    }
}

/// The level that applies to a list item: the level `list` names of the definition it
/// names, or the replacement in the instance's first override of that level, which
/// keeps the definition's start and restart (MS-OI29500 §17.9.25(b), §17.9.10(b)).
pub(crate) fn list_level(document: &Document, list: &ListFormat) -> Option<ListLevel> {
    let numbering = document.numbering();
    let base = numbering
        .definition(list.definition?)
        .and_then(|definition| definition.level(list.level_number));
    let replacement = numbering
        .instance(list.instance)
        .and_then(|instance| instance.override_of(list.level_number))
        .and_then(|over| over.definition().cloned());
    match replacement {
        Some(mut replacement) => {
            replacement.level = list.level_number;
            replacement.start = base.as_ref().and_then(ListLevel::start);
            replacement.restart = base.as_ref().and_then(ListLevel::restart);
            Some(replacement)
        }
        None => base,
    }
}

/// What a level of a list instance gives to paragraphs and to their labels, with the
/// instance's override, and the number it starts from.
#[derive(Clone, Debug, Default)]
pub(super) struct InstanceLevel {
    layer: Option<SourcedLayer>,
    start: i64,
    label: Option<Arc<CharacterLevel>>,
}

/// What a level of a list definition gives to paragraphs and to their labels.
#[derive(Clone, Debug, Default)]
pub(super) struct LevelGives {
    layer: Option<Arc<ListLayer>>,
    label: Option<Arc<CharacterLevel>>,
}

/// What a list level gives to paragraphs, and where it comes from.
#[derive(Clone, Debug)]
pub(super) struct SourcedLayer {
    source: LayerSource,
    layer: Arc<ListLayer>,
}

/// Where what a list level gives to paragraphs comes from: a level of a definition,
/// which every instance of the definition without an override of that level shares, or
/// the replacement in an instance's override of a level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum LayerSource {
    /// The definition and the level.
    Definition(i64, usize),
    /// The instance and the level.
    Override(i64, usize),
}

/// What the resolver keeps of the levels of a list definition, 0 to 8.
#[derive(Debug, Default)]
pub(super) struct DefinitionLevels {
    levels: [Option<DefinitionLevel>; 9],
}

impl DefinitionLevels {
    /// The first level whose `w:pStyle` names the paragraph style at `style`. An
    /// override's level does not take a style over.
    fn level_of_style(&self, style: usize) -> Option<i64> {
        self.levels
            .iter()
            .position(|level| {
                level
                    .as_ref()
                    .is_some_and(|level| level.style == Some(style))
            })
            .and_then(|number| i64::try_from(number).ok())
    }
}

/// What the resolver keeps of a level of a list definition.
#[derive(Debug)]
struct DefinitionLevel {
    /// Its `w:start`.
    start: Option<i64>,
    /// The index of the paragraph style its `w:pStyle` names.
    style: Option<usize>,
}

/// The definition a list instance uses, after a numbering style's link, and its levels.
#[derive(Debug)]
pub(super) struct InstanceDefinition {
    /// None when there is no such definition or the link leads nowhere.
    definition: Option<i64>,
    shared: Arc<DefinitionLevels>,
}

/// What a list level gives to its paragraphs: Word takes only the alignment, the
/// indentation and the tab stops of its `w:pPr` (MS-OI29500 §17.9.22(b)).
#[derive(Debug)]
pub(super) struct ListLayer {
    justification: Option<Justification>,
    indentation: Indentation,
    tabs: Tabs,
}

impl ListLayer {
    /// What `level` gives; none when it gives nothing.
    fn of(level: &ListLevel) -> Option<Arc<Self>> {
        let properties = level.paragraph_properties();
        let layer = Self {
            justification: properties.justification,
            indentation: Indentation::of(properties),
            tabs: Tabs::of(&properties.tabs),
        };
        let gives = layer.justification.is_some()
            || !layer.indentation.is_empty()
            || !layer.tabs.stops.is_empty();
        gives.then(|| Arc::new(layer))
    }
}

/// The indentation one level gives.
#[derive(Clone, Copy, Debug, Default)]
struct Indentation {
    start: Option<Pt>,
    end: Option<Pt>,
    /// Negative for a hanging indent.
    first_line: Option<Pt>,
    /// Character units, hundredths; 0 cancels those of lower levels (MS-OI29500
    /// §17.3.1.12(a)).
    start_chars: Option<i64>,
    end_chars: Option<i64>,
    first_line_chars: Option<i64>,
}

impl Indentation {
    /// The indentation of one `w:ind`: a hanging indent wins over a first line indent,
    /// and character units over points (ECMA-376 §17.3.1.12).
    fn of(properties: &ParagraphProperties) -> Self {
        let Some(ind) = &properties.indentation else {
            return Self::default();
        };
        Self {
            start: ind.start,
            end: ind.end,
            first_line: ind.hanging.map(|hanging| Pt(-hanging.0)).or(ind.first_line),
            start_chars: ind.start_chars,
            end_chars: ind.end_chars,
            first_line_chars: ind
                .hanging_chars
                // A value the file gives as i64::MIN has no negative; it saturates.
                .map(i64::saturating_neg)
                .or(ind.first_line_chars),
        }
    }

    fn is_empty(&self) -> bool {
        self.start.is_none()
            && self.end.is_none()
            && self.first_line.is_none()
            && self.start_chars.is_none()
            && self.end_chars.is_none()
            && self.first_line_chars.is_none()
    }

    fn overlay(&mut self, above: &Self) {
        overwrite(&mut self.start, above.start);
        overwrite(&mut self.end, above.end);
        overwrite(&mut self.first_line, above.first_line);
        overwrite(&mut self.start_chars, above.start_chars);
        overwrite(&mut self.end_chars, above.end_chars);
        overwrite(&mut self.first_line_chars, above.first_line_chars);
    }
}

/// What one level of the hierarchy gives to a paragraph, or what several give together.
#[derive(Clone, Debug, Default)]
pub(super) struct ParagraphLevel {
    flags: [Option<bool>; ParagraphFlag::COUNT],
    justification: Option<Justification>,
    indentation: Indentation,
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
            indentation: Indentation::of(properties),
            shading: properties.shading,
            tabs: Tabs::of(&properties.tabs),
            outline_level: properties.outline_level,
            list_instance: properties
                .numbering
                .and_then(|numbering| numbering.instance),
            list_level: properties.numbering.and_then(|numbering| numbering.level),
            ..Self::default()
        };
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

    /// Lays what a list level gives over this level.
    fn overlay_list(&mut self, layer: &ListLayer) {
        overwrite(&mut self.justification, layer.justification);
        self.indentation.overlay(&layer.indentation);
        self.tabs = self.tabs.overlay(&layer.tabs);
    }

    /// The format in effect once every level is laid over the others.
    fn format(&self, style: Option<Arc<str>>, list: Option<ListFormat>) -> ParagraphFormat {
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
        let tabs = Arc::clone(&self.tabs.set);
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
            start_indent: self.indentation.start.unwrap_or(Pt(0.0)),
            end_indent: self.indentation.end.unwrap_or(Pt(0.0)),
            first_line_indent: self.indentation.first_line.unwrap_or(Pt(0.0)),
            start_indent_chars: units(self.indentation.start_chars),
            end_indent_chars: units(self.indentation.end_chars),
            first_line_indent_chars: units(self.indentation.first_line_chars),
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
        self.indentation.overlay(&above.indentation);
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

    /// Once nothing lies below, a cleared stop has nothing left to remove.
    fn settle(&mut self) {
        self.tabs = self.tabs.without_clears();
    }
}

/// Tab stops set and cleared, by position, one per position. A cleared stop stays, to
/// remove the stop a lower level sets at its position, until nothing lies below.
#[derive(Clone, Debug, Default)]
struct Tabs {
    stops: Arc<[TabStop]>,
    /// The set stops among `stops`: those in effect once nothing lies below. The same
    /// list when none is cleared.
    set: Arc<[TabStop]>,
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
        let mut bounded = Bounded::default();
        for tab in unique {
            bounded.push(tab);
        }
        Self::new(bounded.stops)
    }

    fn new(stops: Vec<TabStop>) -> Self {
        let stops: Arc<[TabStop]> = stops.into();
        let set = if stops.iter().any(is_cleared) {
            stops
                .iter()
                .filter(|tab| !is_cleared(tab))
                .copied()
                .collect()
        } else {
            Arc::clone(&stops)
        };
        Self { stops, set }
    }

    /// These stops with `above` laid over them: the stops add up, a stop at a position
    /// replaces the one below it, and a cleared one removes it (ECMA-376 §17.3.1.38).
    /// Both lists are by position, so one pass merges and bounds them; a level without
    /// stops costs nothing.
    fn overlay(&self, above: &Tabs) -> Tabs {
        if above.stops.is_empty() {
            return self.clone();
        }
        if self.stops.is_empty() {
            return above.clone();
        }
        let mut below = self.stops.iter().copied().peekable();
        let mut merged = Bounded::default();
        for tab in above.stops.iter() {
            while let Some(lower) = below.next_if(|lower| lower.position.0 < tab.position.0) {
                merged.push(lower);
            }
            below.next_if(|lower| lower.position.0 == tab.position.0);
            merged.push(*tab);
        }
        for lower in below {
            merged.push(lower);
        }
        Tabs::new(merged.stops)
    }

    /// The stops without the cleared ones, for when nothing lies below.
    fn without_clears(&self) -> Tabs {
        Tabs {
            stops: Arc::clone(&self.set),
            set: Arc::clone(&self.set),
        }
    }
}

/// Stops pushed by position, of which it keeps those at the lowest positions: at most
/// [`MAX_TABS`] set and as many cleared.
#[derive(Default)]
struct Bounded {
    stops: Vec<TabStop>,
    set: usize,
    cleared: usize,
}

impl Bounded {
    fn push(&mut self, tab: TabStop) {
        let count = if is_cleared(&tab) {
            &mut self.cleared
        } else {
            &mut self.set
        };
        if *count < MAX_TABS {
            *count += 1;
            self.stops.push(tab);
        }
    }
}

fn is_cleared(tab: &TabStop) -> bool {
    tab.kind == Some(TabKind::Clear)
}
