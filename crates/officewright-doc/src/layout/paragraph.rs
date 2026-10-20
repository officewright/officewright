//! Paragraphs and their formatting in effect as IR paragraphs.

use std::collections::HashMap;
use std::sync::Arc;

use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::style::color;
use super::{Once, Translator};
use crate::model::properties::Frame;
use crate::model::{
    Border, BorderStyle, Justification, LineSpacing, Paragraph, ParagraphFormat, TabKind, TabLeader,
};

/// A paragraph: its formatting, its mark, its list and its content; and whether its mark
/// is deleted or moved away as a tracked change, which the caller lists once a block
/// follows it, as Word joins the paragraph to that block.
pub(super) fn paragraph<'a>(
    t: &mut Translator<'a>,
    paragraph: Paragraph<'a>,
) -> (ir::Paragraph, bool) {
    let format = t.formats.paragraph(paragraph);
    // The IR has no right-to-left paragraphs and no frames yet, so the PDF lays these out
    // as ordinary paragraphs, left to right (#303).
    if format.right_to_left() {
        t.report_once(
            Once::RightToLeft,
            "a right-to-left paragraph (w:bidi) is laid out left to right, here and wherever else the document has one",
        );
    }
    match format.frame {
        Some(Frame::Frame) => t.report_once(
            Once::Frame,
            "a text frame (w:framePr) is laid out as an ordinary paragraph, here and wherever else the document has one",
        ),
        Some(Frame::DropCap) => t.report_once(
            Once::DropCap,
            "a drop cap (w:framePr) is laid out as an ordinary paragraph, here and wherever else the document has one",
        ),
        None => {}
    }
    let mark = t.formats.paragraph_mark(paragraph);
    let mark = t.run_style(&mark);
    let list = format.list().and_then(|list| {
        let instance = t.list_instance(list.instance())?;
        let label = t.formats.list_label(paragraph)?;
        // The label is drawn with its run style as runs are, so what its format asks for
        // and the PDF leaves out is listed as a run's is, when it shows: it is not hidden
        // and its text draws something.
        if !label.hidden() {
            let shows = u8::try_from(list.level_number())
                .is_ok_and(|level| t.label_draws(list.instance(), level));
            t.report_run_features(&label, shows);
        }
        let label = t.run_style(&label);
        let level = u8::try_from(list.level_number()).ok()?;
        Some(ir::ListItem::new(instance, level, label))
    });
    let props = props(t, &format).with_style_key(t.style_key(paragraph));
    let props = t.shared_props(props);
    t.path.push(super::Step::Field("inlines"));
    let inlines = t.inlines(paragraph, mark);
    t.path.pop();
    let paragraph = ir::Paragraph::new(mark)
        .with_props(props)
        .with_inlines(inlines)
        .with_list(list);
    (paragraph, format.mark_deleted)
}

/// How many paragraph properties are kept for one fingerprint: others with that
/// fingerprint get properties of their own, so the comparisons stay bounded.
pub(super) const SHARED_PER_FINGERPRINT: usize = 4;

impl Translator<'_> {
    /// `props`, or equal properties given to an earlier paragraph: 500,000 paragraphs of
    /// one style with 64 tab stops hold the stops once (#275).
    fn shared_props(&mut self, props: ir::ParagraphProps) -> Arc<ir::ParagraphProps> {
        let key = fingerprint(&props);
        self.paragraph_props.share(key, props)
    }
}

/// Paragraph properties given to the IR so far, by their fingerprint, at most
/// [`SHARED_PER_FINGERPRINT`] for each: a paragraph whose properties equal kept ones
/// shares them, found with at most that many comparisons. The first properties of a
/// fingerprint are kept in the map itself and only further ones in a list, so that
/// 500,000 paragraphs that all differ cost no allocation each besides the map's.
#[derive(Default)]
pub(super) struct SharedProps {
    first: HashMap<u64, Arc<ir::ParagraphProps>>,
    more: HashMap<u64, Vec<Arc<ir::ParagraphProps>>>,
}

impl SharedProps {
    /// `props`, or kept properties of the fingerprint `key` that are equal to them; only
    /// equality decides, the fingerprint only finds the candidates.
    fn share(&mut self, key: u64, props: ir::ParagraphProps) -> Arc<ir::ParagraphProps> {
        let Some(first) = self.first.get(&key) else {
            let shared = Arc::new(props);
            #[cfg(test)]
            super::work(|work| work.paragraph_props += 1);
            self.first.insert(key, Arc::clone(&shared));
            return shared;
        };
        let more = self.more.get(&key).map_or(&[][..], Vec::as_slice);
        for candidate in std::iter::once(first).chain(more) {
            #[cfg(test)]
            super::work(|work| work.props_compared += 1);
            if **candidate == props {
                return Arc::clone(candidate);
            }
        }
        #[cfg(test)]
        super::work(|work| work.paragraph_props += 1);
        let shared = Arc::new(props);
        let more = self.more.entry(key).or_default();
        if more.len() + 1 < SHARED_PER_FINGERPRINT {
            more.push(Arc::clone(&shared));
        }
        shared
    }
}

/// A hash of every field that equality of paragraph properties compares, so that only
/// properties that are equal share a fingerprint, but for a collision of the hash: four
/// paragraphs that differ only in a field left out would take every place of their
/// fingerprint and keep the paragraphs after them from sharing (review of #390).
fn fingerprint(props: &ir::ParagraphProps) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let indent = props.indent();
    let spacing = props.spacing();
    for value in [
        indent.start(),
        indent.end(),
        indent.first_line(),
        spacing.before(),
        spacing.after(),
    ] {
        value.0.to_bits().hash(&mut hasher);
    }
    props.alignment().hash(&mut hasher);
    props.style_key().hash(&mut hasher);
    props.outline_level().hash(&mut hasher);
    [
        spacing.before_auto(),
        spacing.after_auto(),
        props.contextual_spacing(),
        props.keep_next(),
        props.keep_lines(),
        props.widow_control(),
        props.page_break_before(),
        props.word_wrap(),
        props.suppress_auto_hyphens(),
    ]
    .hash(&mut hasher);
    match spacing.line() {
        ir::LineSpacing::Multiple(factor) => (0u8, factor.to_bits()).hash(&mut hasher),
        ir::LineSpacing::Exact(height) => (1u8, height.0.to_bits()).hash(&mut hasher),
        ir::LineSpacing::AtLeast(height) => (2u8, height.0.to_bits()).hash(&mut hasher),
        // A rule of a later IR: its kind alone.
        other => std::mem::discriminant(&other).hash(&mut hasher),
    }
    props.tabs().len().hash(&mut hasher);
    for tab in props.tabs() {
        (tab.position().0.to_bits(), tab.kind(), tab.leader()).hash(&mut hasher);
    }
    let borders = props.borders();
    for border in [
        borders.top(),
        borders.bottom(),
        borders.start(),
        borders.end(),
        borders.between(),
        borders.bar(),
    ] {
        border
            .map(|border| {
                (
                    border.style(),
                    border.width().0.to_bits(),
                    border.space().0.to_bits(),
                    border.color(),
                    border.shadow(),
                )
            })
            .hash(&mut hasher);
    }
    props
        .shading()
        .map(|shading| (shading.pattern(), shading.color(), shading.fill()))
        .hash(&mut hasher);
    hasher.finish()
}

fn props(t: &mut Translator<'_>, format: &ParagraphFormat) -> ir::ParagraphProps {
    let units = [
        format.start_indent_chars(),
        format.end_indent_chars(),
        format.first_line_indent_chars(),
        format.before_spacing_lines(),
        format.after_spacing_lines(),
    ];
    if units.iter().flatten().any(|&units| units != 0) {
        t.leave_out(ReportKind::FeatureUnsupported, || {
            "indents or spacing in character or line units keep their value in points".into()
        });
    }
    let alignment = match format.justification() {
        Justification::Start | Justification::Left => ir::Alignment::Start,
        Justification::Center => ir::Alignment::Center,
        Justification::End | Justification::Right => ir::Alignment::End,
        Justification::Both => ir::Alignment::Both,
        Justification::Distribute => ir::Alignment::Distribute,
        other => {
            t.leave_out(ReportKind::FeatureUnsupported, || {
                format!("the alignment {} is laid out as justified", other.to_xml())
            });
            ir::Alignment::Both
        }
    };
    let indent = ir::Indent::default()
        .with_start(format.start_indent())
        .with_end(format.end_indent())
        .with_first_line(format.first_line_indent());
    let line = match format.line_spacing() {
        LineSpacing::Multiple(factor) => ir::LineSpacing::Multiple(factor),
        LineSpacing::Exact(height) => ir::LineSpacing::Exact(height),
        LineSpacing::AtLeast(height) => ir::LineSpacing::AtLeast(height),
    };
    let spacing = ir::Spacing::default()
        .with_before(format.before_spacing())
        .with_after(format.after_spacing())
        .with_before_auto(format.before_auto_spacing())
        .with_after_auto(format.after_auto_spacing())
        .with_line(line);
    let tabs: Vec<ir::TabStop> = format.tabs().iter().filter_map(tab).collect();
    let written = format.borders();
    let borders = ir::ParagraphBorders::default()
        .with_top(written.top().and_then(|b| border(t, b)))
        .with_bottom(written.bottom().and_then(|b| border(t, b)))
        .with_start(written.start().and_then(|b| border(t, b)))
        .with_end(written.end().and_then(|b| border(t, b)))
        .with_between(written.between().and_then(|b| border(t, b)))
        .with_bar(written.bar().and_then(|b| border(t, b)));
    let shading = format.shading().and_then(|shading| t.shading(shading));
    ir::ParagraphProps::default()
        .with_alignment(alignment)
        .with_indent(indent)
        .with_spacing(spacing)
        .with_contextual_spacing(format.contextual_spacing())
        .with_keep_next(format.keep_next())
        .with_keep_lines(format.keep_lines())
        .with_widow_control(format.widow_control())
        .with_page_break_before(format.page_break_before())
        .with_tabs(tabs)
        .with_borders(borders)
        .with_shading(shading)
        .with_word_wrap(format.word_wrap())
        .with_suppress_auto_hyphens(format.suppress_auto_hyphens())
        .with_outline_level(format.outline_level())
}

/// A tab stop; a stop that clears one of a lower level is not in a format.
fn tab(stop: &crate::model::TabStop) -> Option<ir::TabStop> {
    let kind = match stop.kind()? {
        TabKind::Start | TabKind::Left | TabKind::Number => ir::TabKind::Start,
        TabKind::Center => ir::TabKind::Center,
        TabKind::End | TabKind::Right => ir::TabKind::End,
        TabKind::Decimal => ir::TabKind::Decimal,
        TabKind::Bar => ir::TabKind::Bar,
        TabKind::Clear => return None,
    };
    let leader = match stop.leader().unwrap_or(TabLeader::None) {
        TabLeader::None => ir::TabLeader::None,
        TabLeader::Dot => ir::TabLeader::Dot,
        TabLeader::Hyphen => ir::TabLeader::Hyphen,
        TabLeader::Underscore => ir::TabLeader::Underscore,
        TabLeader::Heavy => ir::TabLeader::Heavy,
        TabLeader::MiddleDot => ir::TabLeader::MiddleDot,
    };
    Some(
        ir::TabStop::default()
            .with_position(stop.position())
            .with_kind(kind)
            .with_leader(leader),
    )
}

/// A border; none for a border of style `none` or `nil`.
pub(super) fn border(t: &mut Translator<'_>, border: &Border) -> Option<ir::Border> {
    use BorderStyle as B;
    use ir::BorderStyle as I;
    let style = match border.style()? {
        B::None | B::Nil => return None,
        B::Single => I::Single,
        B::Thick => I::Thick,
        B::Double => I::Double,
        B::Dotted => I::Dotted,
        B::Dashed => I::Dashed,
        B::DotDash => I::DotDash,
        B::DotDotDash => I::DotDotDash,
        B::Triple => I::Triple,
        B::ThinThickSmallGap => I::ThinThickSmallGap,
        B::ThickThinSmallGap => I::ThickThinSmallGap,
        B::ThinThickThinSmallGap => I::ThinThickThinSmallGap,
        B::ThinThickMediumGap => I::ThinThickMediumGap,
        B::ThickThinMediumGap => I::ThickThinMediumGap,
        B::ThinThickThinMediumGap => I::ThinThickThinMediumGap,
        B::ThinThickLargeGap => I::ThinThickLargeGap,
        B::ThickThinLargeGap => I::ThickThinLargeGap,
        B::ThinThickThinLargeGap => I::ThinThickThinLargeGap,
        B::Wave => I::Wave,
        B::DoubleWave => I::DoubleWave,
        B::DashSmallGap => I::DashSmallGap,
        B::DashDotStroked => I::DashDotStroked,
        B::ThreeDEmboss => I::ThreeDEmboss,
        B::ThreeDEngrave => I::ThreeDEngrave,
        B::Outset => I::Outset,
        B::Inset => I::Inset,
    };
    let document = t.document;
    Some(
        ir::Border::default()
            .with_style(style)
            .with_width(border.width().unwrap_or_default())
            .with_space(border.space().unwrap_or_default())
            .with_color(color(document.resolve_color(border.color())))
            .with_shadow(border.shadow().unwrap_or(false)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_only_equal_properties_and_compares_a_bounded_number() {
        // From the review of #390: one fingerprint for properties that differ, as a
        // collision gives; equality decides, and the comparisons stay bounded.
        let mut shared = SharedProps::default();
        let props = |before: f64| {
            ir::ParagraphProps::default()
                .with_spacing(ir::Spacing::default().with_before(officewright_core::Pt(before)))
        };
        let first = shared.share(7, props(1.0));
        let other = shared.share(7, props(2.0));
        assert!(!Arc::ptr_eq(&first, &other));
        assert_eq!(other.spacing().before(), officewright_core::Pt(2.0));
        assert!(Arc::ptr_eq(&first, &shared.share(7, props(1.0))));
        // 1,000 more that differ, under the one fingerprint: each is compared with at
        // most four kept ones.
        super::super::WORK.set(super::super::Work::default());
        for n in 0..1000 {
            let _ = shared.share(7, props(f64::from(n) + 10.0));
        }
        let compared = super::super::WORK.get().props_compared;
        assert!(compared <= 1000 * SHARED_PER_FINGERPRINT, "{compared}");
    }
}
