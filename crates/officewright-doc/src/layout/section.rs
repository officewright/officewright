//! The document's settings and its sections as the IR's settings and sections.

use std::collections::HashMap;

use officewright_core::{PageSize, Pt};
use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::{Step, Translator};
use crate::model::{
    HeaderFooterKind, NotePosition, NoteProperties, NoteRestart, NumberFormat, Orientation,
    Section, SectionStart, VerticalJustification,
};

/// The settings that apply to the whole document.
pub(super) fn settings(t: &mut Translator<'_>) -> ir::DocumentSettings {
    let settings = t.document.settings();
    let compat_mode = match settings.effective_compatibility_mode().value() {
        ..=11 => ir::CompatMode::Word2003,
        12 | 13 => ir::CompatMode::Word2007,
        14 => ir::CompatMode::Word2010,
        15.. => ir::CompatMode::Word2013,
    };
    // The legacy options and the compatibility settings, each once, in the order first
    // written; of one written twice, the last value counts, as the model's docs say. A
    // setting is on when set to true. The flags are passed when on.
    // Flags are kept and compared as `Flag`s, whose comparisons tests count.
    let mut values: Vec<(Flag, bool)> = Vec::new();
    let mut places: HashMap<Flag, usize> = HashMap::new();
    let mut set = |flag: ir::CompatFlag, on: bool| {
        let flag = Flag(flag);
        match places.get(&flag) {
            Some(&place) => {
                if let Some(value) = values.get_mut(place) {
                    value.1 = on;
                }
            }
            None => {
                places.insert(Flag(flag.0.clone()), values.len());
                values.push((flag, on));
            }
        }
    };
    for (option, on) in settings.compatibility_options() {
        set(ir::CompatFlag::from_name(option.to_xml()), on);
    }
    for setting in settings.compatibility_settings() {
        if setting.name() == "compatibilityMode" {
            continue;
        }
        let on = setting
            .value()
            .is_some_and(|value| matches!(value.trim(), "1" | "true" | "on"));
        set(ir::CompatFlag::from_name(setting.name()), on);
    }
    let flags: Vec<ir::CompatFlag> = values
        .into_iter()
        .filter(|(_, on)| *on)
        .map(|(flag, _)| flag.0)
        .collect();
    let decimal_separator = settings
        .decimal_symbol()
        .and_then(|symbol| symbol.chars().next())
        .filter(|c| !c.is_control())
        .unwrap_or('.');
    let hyphenation = ir::Hyphenation::default()
        .with_auto(settings.auto_hyphenation().unwrap_or(false))
        .with_zone(settings.hyphenation_zone().unwrap_or(Pt(18.0)))
        .with_consecutive_limit(
            settings
                .consecutive_hyphen_limit()
                .and_then(|limit| u32::try_from(limit.max(0)).ok())
                .unwrap_or(0),
        )
        .with_hyphenate_caps(!settings.do_not_hyphenate_caps().unwrap_or(false));
    ir::DocumentSettings::default()
        .with_compat_mode(compat_mode)
        .with_compat_flags(flags)
        .with_default_tab_stop(settings.default_tab_stop().unwrap_or(Pt(36.0)))
        .with_decimal_separator(decimal_separator)
        .with_even_and_odd_headers(settings.even_and_odd_headers().unwrap_or(false))
        .with_mirror_margins(settings.mirror_margins().unwrap_or(false))
        .with_gutter_at_top(settings.gutter_at_top().unwrap_or(false))
        .with_hyphenation(hyphenation)
}

/// A section with its blocks.
pub(super) fn section<'a>(t: &mut Translator<'a>, section: Section<'a>) -> ir::Section {
    let properties = section.properties();
    // Word's page when the document gives none: Letter, as Word writes into a new
    // document in the United States; margins the IR's defaults.
    let size = properties.page_size().unwrap_or(PageSize::LETTER);
    let written = properties.margins();
    let defaults = ir::Margins::default();
    let margins = ir::Margins::default()
        .with_top(written.top().unwrap_or(defaults.top()))
        .with_bottom(written.bottom().unwrap_or(defaults.bottom()))
        .with_left(written.left().unwrap_or(defaults.left()))
        .with_right(written.right().unwrap_or(defaults.right()))
        .with_header(written.header().unwrap_or(defaults.header()))
        .with_footer(written.footer().unwrap_or(defaults.footer()))
        .with_gutter(written.gutter().unwrap_or(defaults.gutter()));
    let page = ir::PageSetup::new(size)
        .with_orientation(match properties.orientation() {
            Some(Orientation::Landscape) => ir::Orientation::Landscape,
            _ => ir::Orientation::Portrait,
        })
        .with_margins(margins);
    let columns = properties
        .columns()
        .map_or_else(ir::Columns::default, |columns| {
            let count = columns
                .count()
                .and_then(|count| u16::try_from(count.max(1)).ok())
                .unwrap_or(1);
            let mut translated = ir::Columns::default()
                .with_count(count)
                .with_spacing(columns.space().unwrap_or(Pt(36.0)))
                .with_separator(columns.separator().unwrap_or(false));
            // Columns of their own widths, unless they are of equal width (the default
            // when the document lists none).
            if columns.equal_width() == Some(false) {
                translated = translated.with_widths(columns.columns().iter().map(|column| {
                    ir::Column::default()
                        .with_width(column.width().unwrap_or_default())
                        .with_spacing(column.space().unwrap_or_default())
                }));
            }
            translated
        });
    let start = match properties.start() {
        None | Some(SectionStart::NextPage) => ir::SectionStart::NextPage,
        Some(SectionStart::Continuous) => ir::SectionStart::Continuous,
        Some(SectionStart::EvenPage) => ir::SectionStart::EvenPage,
        Some(SectionStart::OddPage) => ir::SectionStart::OddPage,
        Some(SectionStart::NextColumn) => ir::SectionStart::NextColumn,
    };
    let page_numbering =
        properties
            .page_numbering()
            .map_or_else(ir::PageNumbering::default, |numbering| {
                ir::PageNumbering::default()
                    .with_format(
                        numbering
                            .format()
                            .map_or(ir::NumberFormat::Decimal, number_format),
                    )
                    .with_start(
                        numbering
                            .start()
                            .and_then(|start| u32::try_from(start.max(0)).ok()),
                    )
                    .with_chapter_style(
                        numbering
                            .chapter_style()
                            .and_then(|style| u8::try_from(style).ok()),
                    )
            });
    let vertical_align = match properties.vertical_justification() {
        None | Some(VerticalJustification::Top) => ir::PageVerticalAlign::Top,
        Some(VerticalJustification::Center) => ir::PageVerticalAlign::Center,
        Some(VerticalJustification::Both) => ir::PageVerticalAlign::Both,
        Some(VerticalJustification::Bottom) => ir::PageVerticalAlign::Bottom,
    };
    let settings = t.document.settings();
    // Word places footnotes as the section says, not the document (MS-OI29500 2.1.309
    // a), and endnotes as the document says, not the section (§17.11.22).
    let footnotes = notes(
        ir::NoteProps::footnotes(),
        [
            (settings.footnotes(), false),
            (properties.footnotes(), true),
        ],
    );
    let endnotes = notes(
        ir::NoteProps::endnotes(),
        [(settings.endnotes(), true), (properties.endnotes(), false)],
    );
    let headers = t.header_footer_set(section, true);
    let footers = t.header_footer_set(section, false);
    let blocks = t.blocks(section.blocks());
    ir::Section::new(page)
        .with_columns(columns)
        .with_start(start)
        .with_title_page(properties.title_page().unwrap_or(false))
        .with_page_numbering(page_numbering)
        .with_vertical_align(vertical_align)
        .with_footnotes(footnotes)
        .with_endnotes(endnotes)
        .with_headers(headers)
        .with_footers(footers)
        .with_blocks(blocks)
}

impl<'a> Translator<'a> {
    /// The headers or footers in effect for a section, each translated once and copied
    /// into every section that has it. The copies are bounded: past [`MAX_COPIED`]
    /// blocks and inlines, or the size of the document if that is more, a section gets
    /// no more of them, and that is listed.
    fn header_footer_set(&mut self, section: Section<'a>, header: bool) -> ir::HeaderFooterSet {
        let mut set = ir::HeaderFooterSet::default();
        let place = if header { "headers" } else { "footers" };
        for (kind, field) in [
            (HeaderFooterKind::Default, "primary"),
            (HeaderFooterKind::First, "first"),
            (HeaderFooterKind::Even, "even"),
        ] {
            let story = if header {
                section.header(kind)
            } else {
                section.footer(kind)
            };
            let Some(story) = story else {
                continue;
            };
            self.path.extend([Step::Field(place), Step::Field(field)]);
            if !self.header_footers.contains_key(&story.index()) {
                let blocks = self.blocks_in(None, story.blocks());
                let weight = weight(&blocks);
                self.header_footers.insert(story.index(), (blocks, weight));
            }
            let limit = MAX_COPIED.max(self.document.xml_size);
            let copy = match self.header_footers.get(&story.index()) {
                Some((blocks, weight)) if self.copied.saturating_add(*weight) <= limit => {
                    self.copied += weight;
                    Some(blocks.clone())
                }
                _ => None,
            };
            match copy {
                Some(blocks) => {
                    set = match kind {
                        HeaderFooterKind::Default => set.with_primary(blocks),
                        HeaderFooterKind::First => set.with_first(blocks),
                        HeaderFooterKind::Even => set.with_even(blocks),
                    };
                }
                None => self.leave_out(ReportKind::FeatureUnsupported, || {
                    format!(
                        "a {} is left out: the copies of headers and footers in sections \
                         have reached the size of the document",
                        if header { "header" } else { "footer" }
                    )
                }),
            }
            self.path.truncate(self.path.len().saturating_sub(2));
        }
        set
    }
}

/// How much memory the copies of headers and footers in sections take at most, in
/// bytes as [`weight`] counts them, however small the document: a header of a few lines
/// in every section of a document of thousands of sections is not damage. A document
/// whose XML is larger may copy as much as its XML. Smaller in tests, which reach it.
const MAX_COPIED: usize = if cfg!(test) { 1 << 20 } else { 64 << 20 };

/// How many bytes of memory a copy of some blocks takes: every allocation a clone makes.
/// That is each block, row, cell, inline and tab stop, and what they own: a table's grid,
/// a paragraph's tab stops, the text of inlines, a symbol's font name, a field's name, its
/// number format's name and its result, an anchored object. A part that is small in the
/// document can take much more in the IR: a table of thousands of grid columns, or empty
/// paragraphs whose style gives 64 tab stops. Tables nest no deeper than the IR allows,
/// and the results of fields hold no fields.
fn weight(blocks: &[ir::Block]) -> usize {
    blocks
        .iter()
        .map(|block| {
            size_of::<ir::Block>()
                + match block {
                    ir::Block::Paragraph(paragraph) => {
                        size_of_val(paragraph.props().tabs()) + inlines_weight(paragraph.inlines())
                    }
                    ir::Block::Table(table) => {
                        size_of_val(table.grid())
                            + table
                                .rows()
                                .iter()
                                .map(|row| {
                                    size_of::<ir::Row>()
                                        + row
                                            .cells()
                                            .iter()
                                            .map(|cell| {
                                                size_of::<ir::Cell>() + weight(cell.blocks())
                                            })
                                            .sum::<usize>()
                                })
                                .sum::<usize>()
                    }
                    _ => 0,
                }
        })
        .sum()
}

/// How many bytes of memory a copy of some inlines takes, as [`weight`] counts them.
fn inlines_weight(inlines: &[ir::Inline]) -> usize {
    inlines
        .iter()
        .map(|inline| {
            size_of::<ir::Inline>()
                + match inline {
                    ir::Inline::Text { text, .. } => text.len(),
                    ir::Inline::Symbol { font, .. } => font.len(),
                    ir::Inline::Field(field) => {
                        let name = match field.kind() {
                            ir::FieldKind::Other(name) => name.len(),
                            _ => 0,
                        };
                        let format = match field.format() {
                            Some(ir::NumberFormat::Other(name)) => name.len(),
                            _ => 0,
                        };
                        name + format + inlines_weight(field.result())
                    }
                    ir::Inline::Anchored(_) => size_of::<ir::AnchoredObject>(),
                    _ => 0,
                }
        })
        .sum()
}

/// Note properties: Word's defaults, then the document's settings, then the section's;
/// the position only from the levels marked to give it.
fn notes(defaults: ir::NoteProps, levels: [(Option<&NoteProperties>, bool); 2]) -> ir::NoteProps {
    let mut props = defaults;
    for (level, places) in levels {
        let Some(level) = level else {
            continue;
        };
        if let Some(position) = level.position().filter(|_| places) {
            props = props.with_position(match position {
                NotePosition::PageBottom => ir::NotePosition::PageBottom,
                NotePosition::BeneathText => ir::NotePosition::BeneathText,
                NotePosition::SectionEnd => ir::NotePosition::SectionEnd,
                NotePosition::DocumentEnd => ir::NotePosition::DocumentEnd,
            });
        }
        if let Some(format) = level.number_format() {
            props = props.with_format(number_format(format));
        }
        if let Some(start) = level.start() {
            props =
                props.with_start(u16::try_from(start.clamp(0, i64::from(u16::MAX))).unwrap_or(1));
        }
        if let Some(restart) = level.restart() {
            props = props.with_restart(match restart {
                NoteRestart::Continuous => ir::NoteRestart::Continuous,
                NoteRestart::EachSection => ir::NoteRestart::EachSection,
                NoteRestart::EachPage => ir::NoteRestart::EachPage,
            });
        }
    }
    props
}

/// A number format; those the IR has no variant for keep their name.
pub(super) fn number_format(format: NumberFormat) -> ir::NumberFormat {
    match format {
        NumberFormat::Decimal => ir::NumberFormat::Decimal,
        NumberFormat::DecimalZero => ir::NumberFormat::DecimalZero,
        NumberFormat::LowerLetter => ir::NumberFormat::LowerLetter,
        NumberFormat::UpperLetter => ir::NumberFormat::UpperLetter,
        NumberFormat::LowerRoman => ir::NumberFormat::LowerRoman,
        NumberFormat::UpperRoman => ir::NumberFormat::UpperRoman,
        NumberFormat::Ordinal => ir::NumberFormat::Ordinal,
        NumberFormat::Chicago => ir::NumberFormat::Chicago,
        NumberFormat::Bullet => ir::NumberFormat::Bullet,
        NumberFormat::None => ir::NumberFormat::None,
        other => ir::NumberFormat::Other(other.to_xml().to_owned()),
    }
}

/// A compatibility flag as the key of a map; in tests, each comparison of two is counted,
/// so that a test sees what a lookup costs.
struct Flag(ir::CompatFlag);

impl std::hash::Hash for Flag {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for Flag {
    fn eq(&self, other: &Self) -> bool {
        #[cfg(test)]
        super::work(|work| work.flags += 1);
        self.0 == other.0
    }
}

impl Eq for Flag {}
