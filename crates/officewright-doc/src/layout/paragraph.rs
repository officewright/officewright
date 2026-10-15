//! Paragraphs and their formatting in effect as IR paragraphs.

use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::style::color;
use super::{Once, Translator};
use crate::model::properties::Frame;
use crate::model::{
    Border, BorderStyle, Justification, LineSpacing, Paragraph, ParagraphFormat, TabKind, TabLeader,
};

/// A paragraph: its formatting, its mark, its list and its content.
pub(super) fn paragraph<'a>(t: &mut Translator<'a>, paragraph: Paragraph<'a>) -> ir::Paragraph {
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
        let label = t.run_style(&label);
        let level = u8::try_from(list.level_number()).ok()?;
        Some(ir::ListItem::new(instance, level, label))
    });
    let props = props(t, &format).with_style_key(t.style_key(paragraph));
    t.path.push(super::Step::Field("inlines"));
    let inlines = t.inlines(paragraph, mark);
    t.path.pop();
    ir::Paragraph::new(mark)
        .with_props(props)
        .with_inlines(inlines)
        .with_list(list)
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
