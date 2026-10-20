//! Laying out a paragraph for a width: its lines, and the space above and below it.

// reason: the layout itself uses this once pagination (plan PR 8) lays out pages; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use officewright_core::Pt;

use crate::fonts::Fonts;
use crate::ir::{
    Block, CompatFlag, Document, Inline, ObjectContent, Paragraph, ParagraphProps, Size, TextBox,
};
use crate::itemize::{FieldValues, Item, Itemized, itemize_with, shape_runs};
use crate::lines::{Breaker, Input, Line, LineEnd, inline_at};
use crate::lists::{Counters, Label};
use crate::pages::report_list_formats;
use crate::report::{Location, ReportKind};
use crate::shape::Shaper;

/// The space automatic spacing (`w:beforeAutospacing`, `w:afterAutospacing`) puts
/// above or below a paragraph, whatever its font size and spacing: Word's renders of
/// `E-autospacing.docx` (11 and 20 pt text, with and without explicit spacing).
const AUTO_SPACING: Pt = Pt(14.0);

/// A paragraph laid out for a width.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParagraphLayout {
    /// The width of the column it is laid out for.
    pub(crate) width: Pt,
    /// Its text and pieces, as the lines refer to them.
    pub(crate) itemized: Itemized,
    pub(crate) lines: Vec<Line>,
    /// The text boxes its objects show, laid out, in the order of their items.
    pub(crate) boxes: Vec<BoxLayout>,
}

impl ParagraphLayout {
    /// The height of the lines.
    pub(crate) fn height(&self) -> Pt {
        Pt(self.lines.iter().map(|line| line.height.0).sum())
    }

    /// The text box that item `item` shows, laid out.
    pub(crate) fn text_box(&self, item: usize) -> Option<&BoxLayout> {
        self.boxes.iter().find(|laid| laid.item == item)
    }
}

/// The text of a text box laid out in the box's frame, inside its insets.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BoxLayout {
    /// The item of the object that shows the box.
    pub(crate) item: usize,
    /// Its paragraphs, each by its place among the box's blocks, with its layout and its
    /// top from the top of the text.
    pub(crate) paragraphs: Vec<(usize, ParagraphLayout, f64)>,
    /// The height of the text, from the space before its first paragraph to the space
    /// after its last.
    pub(crate) height: f64,
}

/// Lays out `paragraph` of `document` in a column `width` wide, without a list label;
/// `location` is its place, for the report. Its fields show the results the document
/// stores. For the unit tests.
#[cfg(test)]
pub(crate) fn lay_out(
    document: &Document,
    paragraph: &Paragraph,
    width: Pt,
    fonts: &mut Fonts<'_>,
    shaper: &mut Shaper,
    location: &Location,
) -> ParagraphLayout {
    lay_out_with(
        document,
        (paragraph, None),
        width,
        (fonts, shaper),
        location,
        None,
    )
}

/// Lays out `paragraph` of `document` in a column `width` wide, starting with `label` when
/// it is a list item, and the text boxes its objects show; `location` is its place, for
/// the report. Its fields show the values `fields` gives for those the layout computes,
/// else the results the document stores.
pub(crate) fn lay_out_with(
    document: &Document,
    paragraph: (&Paragraph, Option<&Label>),
    width: Pt,
    tools: (&mut Fonts<'_>, &mut Shaper),
    location: &Location,
    fields: Option<&FieldValues>,
) -> ParagraphLayout {
    lay_out_paragraph(document, paragraph, width, tools, (location, fields), true)
}

/// [`lay_out_with`], laying out the text boxes of the paragraph's objects when `boxes`
/// holds, as it does but in a text box: no box is laid out in a box, which
/// `Document::validate` refuses, so that a document that was not validated cannot make the
/// layout recurse without bound.
fn lay_out_paragraph(
    document: &Document,
    (paragraph, label): (&Paragraph, Option<&Label>),
    width: Pt,
    (fonts, shaper): (&mut Fonts<'_>, &mut Shaper),
    (location, fields): (&Location, Option<&FieldValues>),
    boxes: bool,
) -> ParagraphLayout {
    let itemized = itemize_with(
        document,
        (label, paragraph.inlines()),
        fonts,
        location,
        fields,
    );
    let shaped = shape_runs(document, &itemized, fonts, shaper, location);
    let input = Input {
        document,
        paragraph,
        width,
        location,
    };
    let lines = Breaker::new(input, &itemized, &shaped, fonts, shaper).lines();
    // Word hyphenates only where a line wraps, in a paragraph that does not suppress it, in a
    // document that asks for it (Part 1 §17.3.1.34, §17.15.1.10), so the entry names the
    // first paragraph with such a line: a document whose paragraphs all suppress it, or
    // whose lines all end otherwise, gets none.
    if document.settings().hyphenation().auto()
        && !paragraph.props().suppress_auto_hyphens()
        && lines.iter().any(|line| line.end == LineEnd::Wrap)
    {
        fonts.report_once(
            ReportKind::FeatureUnsupported,
            location,
            "automatic hyphenation is not done: words are not hyphenated at the ends of lines"
                .to_owned(),
        );
    }
    let boxes = if boxes {
        lay_out_boxes(
            document,
            paragraph,
            &itemized,
            (fonts, shaper),
            (location, fields),
        )
    } else {
        Vec::new()
    };
    ParagraphLayout {
        width,
        itemized,
        lines,
        boxes,
    }
}

/// The text boxes that the objects of `paragraph`, itemized as `itemized`, show, laid out.
fn lay_out_boxes(
    document: &Document,
    paragraph: &Paragraph,
    itemized: &Itemized,
    (fonts, shaper): (&mut Fonts<'_>, &mut Shaper),
    (location, fields): (&Location, Option<&FieldValues>),
) -> Vec<BoxLayout> {
    let mut boxes = Vec::new();
    for (item, entry) in itemized.items.iter().enumerate() {
        let (Item::Object(path) | Item::Anchored(path)) = entry else {
            continue;
        };
        if let Some((text_box, size)) = inline_at(paragraph.inlines(), path).and_then(text_box) {
            let laid = lay_out_box(
                document,
                (item, text_box, size),
                (fonts, shaper),
                (location, fields),
            );
            boxes.push(laid);
        }
    }
    boxes
}

/// The text box `inline` shows, if it is an object that shows one, with the size of the
/// object's frame.
pub(crate) fn text_box(inline: &Inline) -> Option<(&TextBox, Size)> {
    let (content, size) = match inline {
        Inline::Object(object) => (object.content(), object.size()),
        Inline::Anchored(object) => (object.content(), object.size()),
        _ => return None,
    };
    match content {
        ObjectContent::TextBox(text_box) => Some((text_box, size)),
        _ => None,
    }
}

/// `text_box` laid out in a frame of `size`, inside its insets, as a story of its own: its
/// paragraphs stacked with their spacing as in a header (a hypothesis, until Word renders
/// `E-textbox-*.docx`), their list labels counted from the box's start. Tables in the box,
/// and floating objects in its paragraphs, are left out and reported. `item` is that of the
/// object that shows it.
fn lay_out_box(
    document: &Document,
    (item, text_box, size): (usize, &TextBox, Size),
    (fonts, shaper): (&mut Fonts<'_>, &mut Shaper),
    (location, fields): (&Location, Option<&FieldValues>),
) -> BoxLayout {
    let insets = text_box.insets();
    let width = Pt((size.width().0 - insets.left().0 - insets.right().0).max(0.0));
    let mut counters = Counters::default();
    let mut paragraphs = Vec::new();
    let mut height = 0.0;
    let mut above: Option<&Paragraph> = None;
    for (index, block) in text_box.blocks().iter().enumerate() {
        let Block::Paragraph(inner) = block else {
            fonts.report_once(
                ReportKind::FeatureUnsupported,
                location,
                "tables in text boxes are not drawn yet".to_owned(),
            );
            continue;
        };
        height += match above {
            Some(above) => space_between(document, above.props(), inner.props()).0,
            None => space_before(document, inner.props()).0,
        };
        let label = inner
            .list()
            .and_then(|item| counters.label(document, &item, location));
        let layout = lay_out_paragraph(
            document,
            (inner, label.as_ref()),
            width,
            (fonts, shaper),
            (location, fields),
            false,
        );
        if layout
            .itemized
            .items
            .iter()
            .any(|item| matches!(item, Item::Anchored(_)))
        {
            fonts.report_once(
                ReportKind::FeatureUnsupported,
                location,
                "floating objects in text boxes are not drawn".to_owned(),
            );
        }
        let top = height;
        height += layout.height().0;
        paragraphs.push((index, layout, top));
        above = Some(inner);
    }
    if let Some(last) = above {
        height += space_after(document, last.props()).0;
    }
    report_list_formats(&counters, fonts);
    BoxLayout {
        item,
        paragraphs,
        height,
    }
}

/// The space between a paragraph with `above` and the one with `below` that follows it,
/// in `document`: the larger of the space below the first and the space above the
/// second, as Word's renders of `cv-en-academic-open.docx` and `cv-en-classic-open.docx`
/// show (12 pt below and 12 pt above make 12 pt; 6 and 14 make 14; 4 and 6 make 6).
///
/// Contextual spacing leaves out a paragraph's own space next to a paragraph of the
/// same style (ECMA-376 Part 1, §17.3.1.9). Automatic spacing is 14 pt
/// (`E-autospacing.docx`), and the legacy option `doNotUseHTMLParagraphAutoSpacing`
/// turns it off, so that the explicit values apply (`E-autospacing-nohtml.docx`).
pub(crate) fn space_between(
    document: &Document,
    above: &ParagraphProps,
    below: &ParagraphProps,
) -> Pt {
    let after = own_space_after(document, above, Some(below));
    let before = own_space_before(document, below, Some(above));
    Pt(after.0.max(before.0))
}

/// The space a paragraph with `props` keeps below it before the paragraph `below` it:
/// none when contextual spacing leaves it out next to a paragraph of the same style.
pub(crate) fn own_space_after(
    document: &Document,
    props: &ParagraphProps,
    below: Option<&ParagraphProps>,
) -> Pt {
    if props.contextual_spacing()
        && below.is_some_and(|below| below.style_key() == props.style_key())
    {
        Pt(0.0)
    } else {
        space_after(document, props)
    }
}

/// The space a paragraph with `props` keeps above it after the paragraph `above` it:
/// none when contextual spacing leaves it out next to a paragraph of the same style.
pub(crate) fn own_space_before(
    document: &Document,
    props: &ParagraphProps,
    above: Option<&ParagraphProps>,
) -> Pt {
    if props.contextual_spacing()
        && above.is_some_and(|above| above.style_key() == props.style_key())
    {
        Pt(0.0)
    } else {
        space_before(document, props)
    }
}

/// The space above a paragraph with `props`, with no paragraph above it to share it.
pub(crate) fn space_before(document: &Document, props: &ParagraphProps) -> Pt {
    let spacing = props.spacing();
    if auto_spacing(document) && spacing.before_auto() {
        AUTO_SPACING
    } else {
        spacing.before()
    }
}

/// The space below a paragraph with `props`, with no paragraph below it to share it.
pub(crate) fn space_after(document: &Document, props: &ParagraphProps) -> Pt {
    let spacing = props.spacing();
    if auto_spacing(document) && spacing.after_auto() {
        AUTO_SPACING
    } else {
        spacing.after()
    }
}

/// Whether automatic spacing is on in `document`.
fn auto_spacing(document: &Document) -> bool {
    !document
        .settings()
        .has_compat_flag(&CompatFlag::DoNotUseHTMLParagraphAutoSpacing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{DocumentSettings, Spacing, StyleKey};

    fn spaced(before: f64, after: f64, before_auto: bool, after_auto: bool) -> ParagraphProps {
        ParagraphProps::default().with_spacing(
            Spacing::default()
                .with_before(Pt(before))
                .with_after(Pt(after))
                .with_before_auto(before_auto)
                .with_after_auto(after_auto),
        )
    }

    /// The space between paragraphs `a` and `b`.
    fn between(document: &Document, a: &ParagraphProps, b: &ParagraphProps) -> f64 {
        space_between(document, a, b).0
    }

    #[test]
    fn the_space_between_two_paragraphs_is_the_larger_of_theirs() {
        // Word's renders of the CVs: 12 pt below and 12 pt above are 12.08 pt apart
        // (cv-en-academic-open), 6 and 14 are 14.05, 4 and 6 are 6.09, 2 and 6 are 5.89
        // (cv-en-classic-open).
        let document = Document::default();
        for (after, before, gap) in [
            (12.0, 12.0, 12.0),
            (6.0, 14.0, 14.0),
            (4.0, 6.0, 6.0),
            (2.0, 6.0, 6.0),
        ] {
            let a = spaced(0.0, after, false, false);
            let b = spaced(before, 0.0, false, false);
            assert_eq!(between(&document, &a, &b), gap);
        }
        // With nothing to share them, a paragraph's own spaces.
        let own = spaced(6.0, 10.0, false, false);
        assert_eq!(space_before(&document, &own), Pt(6.0));
        assert_eq!(space_after(&document, &own), Pt(10.0));
    }

    #[test]
    fn automatic_spacing_is_14_points_and_one_between_two() {
        // E-autospacing: shaded 20 pt lines 33.96 pt apart around an automatic
        // paragraph, 14 pt between two automatic ones, explicit values ignored.
        let document = Document::default();
        let plain = spaced(0.0, 0.0, false, false);
        let auto = spaced(0.0, 0.0, true, true);
        assert_eq!(between(&document, &plain, &auto), 14.0);
        assert_eq!(between(&document, &auto, &plain), 14.0);
        assert_eq!(between(&document, &auto, &auto), 14.0);
        let explicit = spaced(24.0, 24.0, true, true);
        assert_eq!(between(&document, &plain, &explicit), 14.0);
        assert_eq!(between(&document, &explicit, &plain), 14.0);
        let before_only = spaced(0.0, 0.0, true, false);
        assert_eq!(between(&document, &before_only, &plain), 0.0);
        assert_eq!(space_before(&document, &auto), Pt(14.0));
    }

    #[test]
    fn the_legacy_option_turns_automatic_spacing_off() {
        // E-autospacing-nohtml: the explicit values apply, 0 when there are none.
        let settings = DocumentSettings::default()
            .with_compat_flags([CompatFlag::DoNotUseHTMLParagraphAutoSpacing]);
        let document = Document::new(settings);
        let plain = spaced(0.0, 0.0, false, false);
        let auto = spaced(0.0, 0.0, true, true);
        assert_eq!(between(&document, &plain, &auto), 0.0);
        assert_eq!(between(&document, &auto, &auto), 0.0);
        let explicit = spaced(24.0, 24.0, true, true);
        assert_eq!(between(&document, &plain, &explicit), 24.0);
        assert_eq!(between(&document, &explicit, &plain), 24.0);
    }

    #[test]
    fn contextual_spacing_leaves_out_the_space_next_to_the_same_style() {
        let document = Document::default();
        let style = |key, contextual| {
            spaced(6.0, 10.0, false, false)
                .with_style_key(StyleKey::new(key))
                .with_contextual_spacing(contextual)
        };
        // Both sides ask for it: no space at all.
        assert_eq!(between(&document, &style(1, true), &style(1, true)), 0.0);
        // Only the second: its space above goes, the first's below stays.
        assert_eq!(between(&document, &style(1, false), &style(1, true)), 10.0);
        // Only the first: its space below goes.
        assert_eq!(between(&document, &style(1, true), &style(1, false)), 6.0);
        // Different styles: both stay, and the larger counts, whichever side it is on.
        assert_eq!(between(&document, &style(1, true), &style(2, true)), 10.0);
        let before_larger =
            |key| style(key, true).with_spacing(Spacing::default().with_before(Pt(12.0)));
        assert_eq!(between(&document, &style(1, true), &before_larger(2)), 12.0);
        assert_eq!(between(&document, &style(1, true), &before_larger(1)), 0.0);
    }
}
