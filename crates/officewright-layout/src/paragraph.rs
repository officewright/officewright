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
        let index = self
            .boxes
            .binary_search_by_key(&item, |laid| laid.item)
            .ok()?;
        self.boxes.get(index)
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
    lay_out_paragraph(
        document,
        paragraph,
        width,
        tools,
        (location, fields),
        (true, false),
    )
}

/// [`lay_out_with`] for a paragraph in a table cell, whose lines of text take the height
/// of its paragraph mark and breaks too (`Breaker::set_height`).
pub(crate) fn lay_out_in_cell(
    document: &Document,
    paragraph: (&Paragraph, Option<&Label>),
    width: Pt,
    tools: (&mut Fonts<'_>, &mut Shaper),
    location: &Location,
    fields: Option<&FieldValues>,
) -> ParagraphLayout {
    lay_out_paragraph(
        document,
        paragraph,
        width,
        tools,
        (location, fields),
        (true, true),
    )
}

/// [`lay_out_with`], laying out the text boxes of the paragraph's objects when `boxes`
/// holds, as it does but in a text box: no box is laid out in a box, which
/// `Document::validate` refuses, so that a document that was not validated cannot make the
/// layout recurse without bound. `cell` says the paragraph is in a table cell.
fn lay_out_paragraph(
    document: &Document,
    (paragraph, label): (&Paragraph, Option<&Label>),
    width: Pt,
    (fonts, shaper): (&mut Fonts<'_>, &mut Shaper),
    (location, fields): (&Location, Option<&FieldValues>),
    (boxes, cell): (bool, bool),
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
        cell,
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
    if text_box
        .blocks()
        .iter()
        .any(|block| matches!(block, Block::Table(_)))
    {
        fonts.report_once(
            ReportKind::FeatureUnsupported,
            location,
            "tables in text boxes are not drawn yet".to_owned(),
        );
    }
    for (index, block) in text_box.blocks().iter().enumerate() {
        let Block::Paragraph(inner) = block else {
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
            (false, false),
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

/// The width the paragraphs of a table cell are measured in for their widest line: wider
/// than any table (a PDF places nothing farther than 32,767 pt from a page's corner), so
/// that only their breaks end lines.
const MEASURE_WIDTH: Pt = Pt(1.0e6);

/// The narrowest width `paragraph` of `document` fits in with no word broken between its
/// characters, and the width it takes with no line wrapped, its indents included, starting
/// with `label` when it is a list item: what a table that fits its columns to their
/// content measures each cell's paragraphs by (design 8.2). `location` is its place, for
/// the report; its fields show the values `fields` gives, as when it is laid out.
pub(crate) fn content_widths(
    document: &Document,
    (paragraph, label): (&Paragraph, Option<&Label>),
    (fonts, shaper): (&mut Fonts<'_>, &mut Shaper),
    location: &Location,
    fields: Option<&FieldValues>,
) -> (Pt, Pt) {
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
        width: MEASURE_WIDTH,
        location,
        // Widths only: no line's height is read.
        cell: true,
    };
    let mut breaker = Breaker::new(input, &itemized, &shaped, fonts, shaper);
    let min = breaker.min_width();
    let max = breaker.max_width();
    (min, Pt(max.0.max(min.0)))
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
    fn no_paragraph_gives_harfrust_a_run_of_a_face_refused_before() {
        use crate::ir::{RunFonts, RunStyle};
        use crate::shape::HARFRUST_CALLS;
        // Issue #405: once a run of a face is refused, the layout's later paragraphs in
        // it go to HarfRust neither to be shaped nor to be cut where a cluster starts: a
        // run longer than the itemizer cuts runs at (4,096 bytes) is not shaped either.
        let font = crate::testing::with_multiplied_a(crate::testing::minimal(), 64);
        let book = crate::testing::book(vec![font]);
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let location = Location::default();
        let mut lay_out_text = |text: &str| {
            let paragraph = Paragraph::new(style).with_inlines([Inline::text(text, style)]);
            let before = HARFRUST_CALLS.with(std::cell::Cell::get);
            lay_out(
                &document,
                &paragraph,
                Pt(400.0),
                &mut fonts,
                &mut shaper,
                &location,
            );
            HARFRUST_CALLS.with(std::cell::Cell::get) - before
        };
        let long = " ".repeat(5000);
        assert!(lay_out_text("A") > 0);
        assert_eq!(lay_out_text(&long), 0);
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

    #[test]
    fn a_large_mark_makes_the_last_line_taller_only_in_a_table_cell() {
        use crate::fonts::Fonts;
        use crate::ir::{
            AnchoredObject, BreakClear, HorizontalAnchor, ImageData, ImageFormat, Inline,
            InlineObject, ObjectContent, Picture, RunFonts, RunStyle, Size, VerticalAnchor,
        };
        use crate::testing::{book, minimal};
        // The test font: a line of 1.15 em. 10 pt text with a 20 pt mark and a 20 pt
        // break: out of a cell, each line of text is as tall as its text; in a cell, the
        // break's line and the last line take the 20 pt of the break and of the mark, as
        // the rows of Word's letterheads do (generated/invoice-en-standard).
        let book = book(vec![minimal()]);
        let mut document = Document::default();
        let style = |size: f64| {
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(size))
        };
        let (small, big) = (
            document.add_run_style(style(10.0)),
            document.add_run_style(style(20.0)),
        );
        let text = Paragraph::new(big).with_inlines([
            Inline::text("A", small),
            Inline::line_break(BreakClear::None, big),
            Inline::text("A", small),
        ]);
        // A line of an inline picture 30 pt high is as tall as the picture out of a cell.
        let image = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
        let picture = Paragraph::new(big).with_inlines([Inline::Object(InlineObject::new(
            ObjectContent::Picture(Picture::new(image)),
            Size::new(Pt(40.0), Pt(30.0)),
        ))]);
        // A line holding only the anchor of a floating picture, which takes no room in it,
        // has the mark's height.
        let anchored = Paragraph::new(big).with_inlines([Inline::anchored(AnchoredObject::new(
            ObjectContent::Picture(Picture::new(image)),
            Size::new(Pt(40.0), Pt(30.0)),
            HorizontalAnchor::default(),
            VerticalAnchor::default(),
        ))]);
        let heights = |paragraph: &Paragraph, cell: bool| -> Vec<f64> {
            let mut fonts = Fonts::new(&book);
            let mut shaper = Shaper::new();
            let tools = (&mut fonts, &mut shaper);
            let location = Location::default();
            let layout = if cell {
                lay_out_in_cell(
                    &document,
                    (paragraph, None),
                    Pt(200.0),
                    tools,
                    &location,
                    None,
                )
            } else {
                lay_out_with(
                    &document,
                    (paragraph, None),
                    Pt(200.0),
                    tools,
                    &location,
                    None,
                )
            };
            layout.lines.iter().map(|line| line.height.0).collect()
        };
        let line = |size: f64| size * 1.15;
        let close = |a: &[f64], b: &[f64]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
        };
        assert!(
            close(&heights(&text, false), &[line(10.0), line(10.0)]),
            "{:?}",
            heights(&text, false)
        );
        assert!(
            close(&heights(&text, true), &[line(20.0), line(20.0)]),
            "{:?}",
            heights(&text, true)
        );
        assert!(
            close(&heights(&picture, false), &[30.0]),
            "{:?}",
            heights(&picture, false)
        );
        let in_cell = heights(&picture, true);
        assert!(
            !in_cell.is_empty() && in_cell.iter().all(|height| *height > 30.0),
            "{in_cell:?}"
        );
        assert!(
            close(&heights(&anchored, false), &[line(20.0)]),
            "{:?}",
            heights(&anchored, false)
        );
    }

    #[test]
    fn a_paragraph_is_measured_by_its_widest_word_and_its_widest_line() {
        use crate::fonts::Fonts;
        use crate::ir::{BreakClear, Indent, Inline, RunFonts, RunStyle};
        use crate::testing::{book, minimal};
        // The test font at 10 pt: `A` 6 pt, a space 2.5 pt.
        let book = book(vec![minimal()]);
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let measure = |inlines: Vec<Inline>, indent: Indent| {
            let paragraph = Paragraph::new(style)
                .with_props(ParagraphProps::default().with_indent(indent))
                .with_inlines(inlines);
            let mut fonts = Fonts::new(&book);
            let mut shaper = Shaper::new();
            let (min, max) = content_widths(
                &document,
                (&paragraph, None),
                (&mut fonts, &mut shaper),
                &Location::default(),
                None,
            );
            (min.0, max.0)
        };
        let text = |text: &str| vec![Inline::text(text, style)];
        // The widest word, and the whole line; spaces at the end of either take no room.
        assert_eq!(measure(text("A AAA AA  "), Indent::default()), (18.0, 41.0));
        // A line break ends a line, and the widest line counts.
        let broken = vec![
            Inline::text("AAAA", style),
            Inline::line_break(BreakClear::None, style),
            Inline::text("A A", style),
        ];
        assert_eq!(measure(broken, Indent::default()), (24.0, 24.0));
        // Indents count, the first line's for the first word and line.
        let indent = Indent::default()
            .with_start(Pt(10.0))
            .with_end(Pt(5.0))
            .with_first_line(Pt(20.0));
        assert_eq!(measure(text("AA AAAA"), indent), (47.0, 73.5));
        // A first line that hangs out far: its line alone is narrower than the second
        // word on a line of its own, and the widest line is never narrower than that.
        let hanging = Indent::default()
            .with_start(Pt(30.0))
            .with_first_line(Pt(-30.0));
        assert_eq!(measure(text("A AAAA"), hanging), (54.0, 54.0));
        // Review of #415: spaces at the start (5 pt) stay on the first line, and the word
        // after them goes to the next line if it does not fit there; its minimum is the
        // narrower of the two, the narrowest width at which the line breaker keeps it
        // whole, and half a point less breaks it. A first line that hangs 30 pt out keeps
        // the word: 5 + 24; one indented 20 pt sends it on: 24.
        let lines_at = |indent: Indent, width: f64| {
            let paragraph = Paragraph::new(style)
                .with_props(ParagraphProps::default().with_indent(indent))
                .with_inlines(text("  AAAA"));
            let mut fonts = Fonts::new(&book);
            let mut shaper = Shaper::new();
            lay_out(
                &document,
                &paragraph,
                Pt(width),
                &mut fonts,
                &mut shaper,
                &Location::default(),
            )
            .lines
            .len()
        };
        let indented = Indent::default().with_first_line(Pt(20.0));
        for (indent, min, lines) in [(hanging, 29.0, 1), (indented, 24.0, 2)] {
            assert_eq!(measure(text("  AAAA"), indent).0, min, "{indent:?}");
            assert_eq!(lines_at(indent, min), lines, "{indent:?} at {min}");
            assert!(
                lines_at(indent, min - 0.5) > lines,
                "{indent:?} below {min}"
            );
        }
        // A paragraph that starts with a line break puts its word on the second line, from
        // the start indent: 30 + 24 with the hanging first line.
        let after_break = || {
            vec![
                Inline::line_break(BreakClear::None, style),
                Inline::text("AAAA", style),
            ]
        };
        assert_eq!(measure(after_break(), hanging).0, 54.0);
        // The widest line counts its own indent: the first line indented 20 pt, the second
        // not.
        let two = vec![
            Inline::text("AA", style),
            Inline::line_break(BreakClear::None, style),
            Inline::text("AAAA", style),
        ];
        assert_eq!(measure(two, indented).1, 32.0);
        // Nothing at all takes no room.
        assert_eq!(measure(Vec::new(), Indent::default()), (0.0, 0.0));
    }

    #[test]
    fn a_word_that_ends_at_a_soft_hyphen_is_measured_with_its_hyphen() {
        use crate::fonts::Fonts;
        use crate::ir::{Inline, RunFonts, RunStyle};
        use crate::testing::{book, minimal, with_hyphen, with_soft_hyphen};
        // Review of #415: a column at its minimum holds the hyphen too. In this font the
        // hyphen is 6 pt, as is `Á`: `ÁÁÁ` and its hyphen make 24 pt.
        let book = book(vec![with_soft_hyphen(with_hyphen(minimal()))]);
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let paragraph = Paragraph::new(style).with_inlines([
            Inline::text("ÁÁÁ", style),
            Inline::soft_hyphen(style),
            Inline::text("ÁÁ", style),
        ]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let (min, max) = content_widths(
            &document,
            (&paragraph, None),
            (&mut fonts, &mut shaper),
            &Location::default(),
            None,
        );
        assert_eq!((min.0, max.0), (24.0, 30.0));
    }
}
