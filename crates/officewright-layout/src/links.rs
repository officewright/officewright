//! Links and bookmarks of laid-out paragraphs: the rectangle a link takes on each line it
//! spans, and the place each bookmark marks, for the painter to make link annotations and
//! named destinations of.

use std::ops::Range;
use std::sync::Arc;

use officewright_core::Pt;

use crate::display::{DisplayItem, Point};
use crate::ir::LinkTarget;
use crate::itemize::{Itemized, Mark};
use crate::lines::{Line, Piece, PieceKind};

/// The smallest width or height of a link that a PDF takes: a smaller one could not be
/// clicked, and is left out.
pub(crate) const MIN_LINK: f64 = 0.01;

/// Adds to `out` the links and bookmarks of `lines` of a paragraph laid out in `all`,
/// itemized as `itemized`, the first of `lines` with its top at `origin.y`, in a column
/// whose start edge is at `origin.x`. A line's top is below the room a floating object it
/// goes below takes (`Line::above`).
///
/// A link takes one rectangle on each line it spans, and on each part of a line beside
/// floating objects (`Line::parts`): across, from where the first of its pieces there
/// starts to where the last ends, the hyphen drawn after the last included, but for the
/// spaces that hang at the end; down, the line's box, from its top to its bottom. A
/// bookmark marks the line's start edge and its top on the line that holds what follows
/// it, or on the paragraph's last line when nothing does. Both are hypotheses until a Word
/// export of a document with links shows its annotations.
pub(crate) fn paint(
    itemized: &Itemized,
    all: &[Line],
    lines: Range<usize>,
    origin: Point,
    out: &mut Vec<DisplayItem>,
) {
    if itemized.links.is_empty() && itemized.bookmarks.is_empty() {
        return;
    }
    let last_line = all.len().checked_sub(1);
    // The bookmarks that the lines before these mark.
    let mut marked = all
        .get(..lines.start)
        .unwrap_or(&[])
        .iter()
        .rev()
        .find_map(|line| last_placed(&line.pieces))
        .map_or(0, |piece| marked_by(itemized, piece));
    let mut top = origin.y.0;
    for (index, line) in all.iter().enumerate().take(lines.end).skip(lines.start) {
        // The line's own box starts below the room a floating object it goes below takes.
        top += line.above.0;
        let start = Point {
            x: Pt(origin.x.0 + line.start.0),
            y: Pt(top),
        };
        let upto = if Some(index) == last_line {
            itemized.bookmarks.len()
        } else {
            last_placed(&line.pieces).map_or(marked, |piece| marked_by(itemized, piece))
        };
        for (_, name) in itemized.bookmarks.get(marked..upto).unwrap_or(&[]) {
            out.push(DisplayItem::Destination {
                at: start,
                name: name.clone(),
            });
        }
        marked = marked.max(upto);
        for pieces in line.part_pieces() {
            part_links(itemized, (pieces, line.height), (origin.x.0, top), out);
        }
        top += line.height.0;
    }
}

/// The links of `pieces`, those of a line `height` high or of a part of it, whose top is at
/// `y`, in a column whose start edge is at `x`.
fn part_links(
    itemized: &Itemized,
    (pieces, height): (&[Piece], Pt),
    (x, y): (f64, f64),
    out: &mut Vec<DisplayItem>,
) {
    let first = pieces.iter().find(|piece| place(piece).is_some());
    let (Some(first), Some(last)) = (first, last_placed(pieces)) else {
        return;
    };
    let links = &itemized.links;
    // The links that end after the line's first piece and start before its last end.
    let from = links.partition_point(|link| !before(first, link.end));
    let to = links.partition_point(|link| after(last, link.start));
    let spans = links.get(from..to).unwrap_or(&[]);
    let (Some(first_link), Some(last_link)) = (spans.first(), spans.last()) else {
        return;
    };
    // How far each link reaches on the line, from the column's start edge.
    let mut reach: Vec<Option<(f64, f64)>> = vec![None; spans.len()];
    let mut extend = |at: usize, start: f64, end: f64| {
        if let Some(slot) = reach.get_mut(at) {
            let (low, high) = slot.unwrap_or((start, end));
            *slot = Some((low.min(start), high.max(end)));
        }
    };
    // The link that the piece or glyph at hand may be in: the pieces are in reading
    // order, and so are the links.
    let mut at = 0;
    // The link of the glyph or piece before, which the hyphen drawn after it belongs to.
    let mut previous = None;
    for piece in pieces.iter().filter(|piece| !piece.hanging) {
        let end = piece.x.0 + piece.width.0;
        match (&piece.kind, place(piece)) {
            // Text outside every link is not taken apart into glyphs.
            (PieceKind::Text { .. }, Some(_))
                if !after(piece, first_link.start) || !before(piece, last_link.end) =>
            {
                previous = None;
            }
            (PieceKind::Text { .. }, Some(_)) => {
                let mut pen = piece.x.0;
                for glyph in piece.glyphs(itemized) {
                    let start = pen;
                    pen += glyph.advance.0;
                    while spans
                        .get(at)
                        .is_some_and(|span| span.end.text <= glyph.cluster)
                    {
                        at += 1;
                    }
                    previous = spans
                        .get(at)
                        .is_some_and(|span| span.start.text <= glyph.cluster)
                        .then_some(at);
                    if let Some(link) = previous {
                        extend(link, start, pen);
                    }
                }
            }
            (_, Some(item)) => {
                while spans.get(at).is_some_and(|span| span.end.item <= item) {
                    at += 1;
                }
                previous = spans
                    .get(at)
                    .is_some_and(|span| span.start.item <= item)
                    .then_some(at);
                // A floating object or a note's mark takes no room: a link of nothing else
                // takes none on the line, and a floating object's frame takes it.
                if let Some(link) = previous.filter(|_| piece.width.0 > 0.0) {
                    extend(link, piece.x.0, end);
                }
            }
            (_, None) => {
                if let Some(link) = previous {
                    extend(link, piece.x.0, end);
                }
            }
        }
    }
    for (span, reach) in spans.iter().zip(reach) {
        if let Some((start, end)) = reach {
            out.push(DisplayItem::Link {
                at: Point {
                    x: Pt(x + start),
                    y: Pt(y),
                },
                width: Pt(end - start),
                height,
                target: span.target.clone(),
            });
        }
    }
}

/// The target of the link that item `item` of `itemized`, an item that is not text, is in.
pub(crate) fn target_of(itemized: &Itemized, item: usize) -> Option<Arc<LinkTarget>> {
    let links = &itemized.links;
    let at = links.partition_point(|link| link.end.item <= item);
    links
        .get(at)
        .filter(|link| link.start.item <= item)
        .map(|link| link.target.clone())
}

/// The item of a piece that has a place of its own among the paragraph's pieces; `None`
/// for the hyphen drawn where a line ends at a soft hyphen, which stands for no text and
/// belongs to the link of what it follows.
fn place(piece: &Piece) -> Option<usize> {
    match &piece.kind {
        PieceKind::Text { item, .. }
        | PieceKind::Tab { item, .. }
        | PieceKind::Break { item }
        | PieceKind::Object { item, .. } => Some(*item),
        PieceKind::Hyphen { .. } => None,
    }
}

/// The last of `pieces` that has a place of its own.
fn last_placed(pieces: &[Piece]) -> Option<&Piece> {
    pieces.iter().rev().find(|piece| place(piece).is_some())
}

/// How many of the paragraph's bookmarks come before `piece` or within it.
fn marked_by(itemized: &Itemized, piece: &Piece) -> usize {
    itemized
        .bookmarks
        .partition_point(|(mark, _)| after(piece, *mark))
}

/// Whether some of `piece` comes after `mark`.
fn after(piece: &Piece, mark: Mark) -> bool {
    match &piece.kind {
        PieceKind::Text { text, .. } => text.end > mark.text,
        _ => place(piece).is_some_and(|item| item >= mark.item),
    }
}

/// Whether some of `piece` comes before `mark`.
fn before(piece: &Piece, mark: Mark) -> bool {
    match &piece.kind {
        PieceKind::Text { text, .. } => text.start < mark.text,
        _ => place(piece).is_some_and(|item| item < mark.item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::Fonts;
    use crate::ir::{
        BreakClear, Document, Field, FieldKind, ImageData, ImageFormat, Indent, Inline,
        InlineObject, LinkTarget, ObjectContent, Paragraph, ParagraphProps, Picture, RunFonts,
        RunStyle, RunStyleId, Size,
    };
    use crate::paragraph::{ParagraphLayout, lay_out};
    use crate::report::Location;
    use crate::shape::Shaper;
    use crate::testing::{book, minimal, with_hyphen, with_soft_hyphen};

    /// A link or a destination, in points: a link by its box and target, a destination by
    /// its point and name.
    #[derive(Debug, PartialEq)]
    enum Mark {
        Link(f64, f64, f64, f64, &'static str),
        Destination(f64, f64, &'static str),
    }

    fn uri(text: &str) -> LinkTarget {
        LinkTarget::Uri(text.to_owned())
    }

    /// A paragraph of the inlines `make` gives for the test font at 10 pt (`A` 6 pt wide,
    /// a space 2.5 pt, lines 11.5 pt high), laid out `width` points wide with `props`.
    fn laid(
        width: f64,
        props: ParagraphProps,
        make: impl Fn(RunStyleId, &mut Document) -> Vec<Inline>,
    ) -> (Document, ParagraphLayout) {
        laid_in(minimal(), width, props, make)
    }

    /// [`laid`] in `font`.
    fn laid_in(
        font: Vec<u8>,
        width: f64,
        props: ParagraphProps,
        make: impl Fn(RunStyleId, &mut Document) -> Vec<Inline>,
    ) -> (Document, ParagraphLayout) {
        let book = book(vec![font]);
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let inlines = make(style, &mut document);
        let paragraph = Paragraph::new(style)
            .with_props(props)
            .with_inlines(inlines);
        let mut fonts = Fonts::new(&book);
        let layout = lay_out(
            &document,
            &paragraph,
            Pt(width),
            &mut fonts,
            &mut Shaper::new(),
            &Location::default(),
        );
        (document, layout)
    }

    /// The links and destinations that `lines` of `layout` draw, the first of them with
    /// its top at 20 pt, in a column whose start edge is at 50 pt.
    fn marks(layout: &ParagraphLayout, lines: Range<usize>) -> Vec<Mark> {
        let mut out = Vec::new();
        let origin = Point {
            x: Pt(50.0),
            y: Pt(20.0),
        };
        paint(&layout.itemized, &layout.lines, lines, origin, &mut out);
        let name = |text: &str| -> &'static str { Box::leak(text.to_owned().into_boxed_str()) };
        out.iter()
            .map(|item| match item {
                DisplayItem::Link {
                    at,
                    width,
                    height,
                    target,
                } => {
                    let target = match &**target {
                        LinkTarget::Uri(uri) => name(uri),
                        LinkTarget::Bookmark(bookmark) => name(&format!("#{bookmark}")),
                    };
                    Mark::Link(at.x.0, at.y.0, width.0, height.0, target)
                }
                DisplayItem::Destination { at, name: bookmark } => {
                    Mark::Destination(at.x.0, at.y.0, name(bookmark))
                }
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// What all the lines of `layout` draw, as [`marks`].
    fn all_marks(layout: &ParagraphLayout) -> Vec<Mark> {
        marks(layout, 0..layout.lines.len())
    }

    #[test]
    fn a_link_takes_the_box_of_its_glyphs_on_its_line() {
        // The link does not cut the text into runs: its glyphs are found in the one run.
        let (_, layout) = laid(100.0, ParagraphProps::default(), |style, _| {
            vec![
                Inline::text("AA", style),
                Inline::link_start(uri("https://example.com/")),
                Inline::text("AAA", style),
                Inline::link_end(),
                Inline::text("A", style),
            ]
        });
        assert_eq!(layout.itemized.items.len(), 1);
        assert_eq!(
            all_marks(&layout),
            [Mark::Link(62.0, 20.0, 18.0, 11.5, "https://example.com/")]
        );
    }

    #[test]
    fn a_link_over_lines_takes_a_box_on_each_but_for_the_spaces_that_hang() {
        // 40 pt wide: "AAA AAA " on the first line, its last space hanging, "AAA" on the
        // second.
        let (_, layout) = laid(40.0, ParagraphProps::default(), |style, _| {
            vec![
                Inline::text("AAA ", style),
                Inline::link_start(uri("u")),
                Inline::text("AAA AAA", style),
                Inline::link_end(),
            ]
        });
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(
            all_marks(&layout),
            [
                Mark::Link(70.5, 20.0, 18.0, 11.5, "u"),
                Mark::Link(50.0, 31.5, 18.0, 11.5, "u"),
            ]
        );
    }

    #[test]
    fn a_link_ends_at_its_end_the_next_link_or_the_paragraph_s_end() {
        let (_, layout) = laid(100.0, ParagraphProps::default(), |style, _| {
            vec![
                // An end outside a link does nothing, and a link that holds nothing is
                // left out.
                Inline::link_end(),
                Inline::link_start(uri("empty")),
                Inline::link_end(),
                Inline::text("A", style),
                // A tab in a link is part of it: from 12 pt to the default stop at 36.
                Inline::link_start(uri("first")),
                Inline::text("A", style),
                Inline::tab(style),
                Inline::link_start(uri("second")),
                Inline::text("A", style),
            ]
        });
        assert_eq!(
            all_marks(&layout),
            [
                Mark::Link(56.0, 20.0, 30.0, 11.5, "first"),
                Mark::Link(86.0, 20.0, 6.0, 11.5, "second"),
            ]
        );
    }

    #[test]
    fn a_link_holds_its_pictures_and_the_shown_result_of_a_field() {
        let (_, layout) = laid(100.0, ParagraphProps::default(), |style, document| {
            let image = document.add_image(ImageData::new(ImageFormat::Png, Vec::new()));
            let picture = || {
                Inline::Object(InlineObject::new(
                    ObjectContent::Picture(Picture::new(image)),
                    Size::new(Pt(8.0), Pt(5.0)),
                ))
            };
            let field = Field::new(FieldKind::Other("HYPERLINK".into()), style).with_result([
                Inline::link_start(LinkTarget::Bookmark("Terms".into())),
                Inline::text("AA", style),
                Inline::link_end(),
            ]);
            vec![
                Inline::link_start(uri("logo")),
                picture(),
                Inline::link_end(),
                // Right after the link's end, outside it: from 8 pt to the stop at 36.
                Inline::tab(style),
                Inline::text("A", style),
                Inline::Field(field),
                Inline::tab(style),
                // A link of a picture alone that ends the line, right after a tab.
                Inline::link_start(uri("last")),
                picture(),
            ]
        });
        assert_eq!(
            all_marks(&layout),
            [
                Mark::Link(50.0, 20.0, 8.0, 11.5, "logo"),
                Mark::Link(92.0, 20.0, 12.0, 11.5, "#Terms"),
                Mark::Link(122.0, 20.0, 8.0, 11.5, "last"),
            ]
        );
    }

    #[test]
    fn a_link_cut_at_a_soft_hyphen_holds_the_hyphen() {
        // In this font `Á` and the hyphen are 6 pt: 25 pt wide, "ÁÁÁ" and its hyphen on
        // the first line, "ÁÁÁ" on the second.
        let font = with_soft_hyphen(with_hyphen(minimal()));
        let (_, layout) = laid_in(font, 25.0, ParagraphProps::default(), |style, _| {
            vec![
                Inline::link_start(uri("word")),
                Inline::text("ÁÁÁ", style),
                Inline::soft_hyphen(style),
                Inline::text("ÁÁÁ", style),
            ]
        });
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(
            all_marks(&layout),
            [
                Mark::Link(50.0, 20.0, 24.0, 11.5, "word"),
                Mark::Link(50.0, 31.5, 18.0, 11.5, "word"),
            ]
        );
    }

    #[test]
    fn a_bookmark_marks_the_start_and_top_of_the_line_of_what_follows_it() {
        // 50 pt wide from an indent of 10: "AAA AAA " on the first line, "AAA" on the
        // second.
        let props = ParagraphProps::default().with_indent(Indent::default().with_start(Pt(10.0)));
        let (_, layout) = laid(50.0, props, |style, _| {
            vec![
                Inline::bookmark("start"),
                Inline::text("AAA ", style),
                Inline::bookmark("second word"),
                Inline::text("AAA ", style),
                // After the space that hangs at the end of the first line.
                Inline::bookmark("third word"),
                Inline::text("AAA", style),
                // After everything: on the last line.
                Inline::bookmark("end"),
            ]
        });
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(
            all_marks(&layout),
            [
                Mark::Destination(60.0, 20.0, "start"),
                Mark::Destination(60.0, 20.0, "second word"),
                Mark::Destination(60.0, 31.5, "third word"),
                Mark::Destination(60.0, 31.5, "end"),
            ]
        );
    }

    #[test]
    fn each_part_of_a_paragraph_marks_its_own_bookmarks_and_links_once() {
        // Painted line by line, as on pages of their own, each part has its top at 20 pt.
        let (_, layout) = laid(40.0, ParagraphProps::default(), |style, _| {
            vec![
                Inline::text("AAA ", style),
                Inline::link_start(uri("u")),
                Inline::bookmark("first"),
                Inline::text("AAA ", style),
                Inline::bookmark("second"),
                Inline::text("AAA AAA", style),
                Inline::bookmark("end"),
            ]
        });
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(
            marks(&layout, 0..1),
            [
                Mark::Destination(50.0, 20.0, "first"),
                Mark::Link(70.5, 20.0, 18.0, 11.5, "u"),
            ]
        );
        assert_eq!(
            marks(&layout, 1..2),
            [
                Mark::Destination(50.0, 20.0, "second"),
                Mark::Destination(50.0, 20.0, "end"),
                Mark::Link(50.0, 20.0, 38.5, 11.5, "u"),
            ]
        );
    }

    #[test]
    fn a_hyphen_belongs_to_the_link_of_what_it_follows_only() {
        // 25 pt wide in a font whose `Á` and hyphen are 6 pt: a link of `Á` alone, then
        // "ÁÁ", a soft hyphen and "ÁÁÁ" in a style of their own, outside the link. The first
        // line ends at the soft hyphen, whose hyphen follows the unlinked "ÁÁ".
        let font = with_soft_hyphen(with_hyphen(minimal()));
        let (_, layout) = laid_in(font, 25.0, ParagraphProps::default(), |style, document| {
            let other = document.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Officewright Test"))
                    .with_size(Pt(10.0))
                    .with_kern_min(Some(Pt(100.0))),
            );
            vec![
                Inline::link_start(uri("word")),
                Inline::text("Á", style),
                Inline::link_end(),
                Inline::text("ÁÁ", other),
                Inline::soft_hyphen(other),
                Inline::text("ÁÁÁ", other),
            ]
        });
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(layout.itemized.items.len(), 2);
        assert_eq!(
            all_marks(&layout),
            [Mark::Link(50.0, 20.0, 6.0, 11.5, "word")]
        );
    }

    #[test]
    fn a_bookmark_after_a_soft_hyphen_where_the_line_ends_marks_the_next_line() {
        // The hyphen drawn at the end of the first line stands for no text: the bookmark
        // after the soft hyphen marks the second line, which holds what follows it.
        let font = with_soft_hyphen(with_hyphen(minimal()));
        let (_, layout) = laid_in(font, 25.0, ParagraphProps::default(), |style, _| {
            vec![
                Inline::text("ÁÁÁ", style),
                Inline::soft_hyphen(style),
                Inline::bookmark("after"),
                Inline::text("ÁÁÁ", style),
            ]
        });
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(all_marks(&layout), [Mark::Destination(50.0, 31.5, "after")]);
    }

    #[test]
    fn a_floating_object_takes_no_room_of_its_link_on_the_line() {
        use crate::ir::{AnchoredObject, HorizontalAnchor, VerticalAnchor};
        // A link holding only a floating picture draws no box on the line: the picture's
        // frame takes the link where the page draws it.
        let (_, layout) = laid(100.0, ParagraphProps::default(), |style, document| {
            let image = document.add_image(ImageData::new(ImageFormat::Png, Vec::new()));
            let picture = || {
                Inline::anchored(AnchoredObject::new(
                    ObjectContent::Picture(Picture::new(image)),
                    Size::new(Pt(80.0), Pt(50.0)),
                    HorizontalAnchor::default(),
                    VerticalAnchor::default(),
                ))
            };
            vec![
                picture(),
                Inline::text("A", style),
                Inline::link_start(uri("logo")),
                picture(),
                Inline::link_end(),
                picture(),
                Inline::text("A", style),
                Inline::tab(style),
            ]
        });
        assert_eq!(all_marks(&layout), []);
        // The pictures are items 0, before the link, 2, in it, and 3, right after its end,
        // between the runs of text; the tab after the link is item 5.
        assert_eq!(target_of(&layout.itemized, 0), None);
        assert_eq!(
            target_of(&layout.itemized, 2).as_deref(),
            Some(&uri("logo"))
        );
        assert_eq!(target_of(&layout.itemized, 3), None);
        assert_eq!(target_of(&layout.itemized, 5), None);
    }

    #[test]
    fn a_bookmark_right_before_a_break_marks_the_line_the_break_ends() {
        // "A" and a break on the first line; the bookmark, then a break alone on the
        // second; "A" on the third.
        let (_, layout) = laid(100.0, ParagraphProps::default(), |style, _| {
            vec![
                Inline::text("A", style),
                Inline::line_break(BreakClear::None, style),
                Inline::bookmark("empty line"),
                Inline::line_break(BreakClear::None, style),
                Inline::text("A", style),
            ]
        });
        assert_eq!(layout.lines.len(), 3);
        assert_eq!(
            all_marks(&layout),
            [Mark::Destination(50.0, 31.5, "empty line")]
        );
    }

    #[test]
    fn a_paragraph_without_text_marks_its_bookmark_on_its_line() {
        let (_, layout) = laid(100.0, ParagraphProps::default(), |_, _| {
            vec![Inline::bookmark("empty")]
        });
        assert_eq!(all_marks(&layout), [Mark::Destination(50.0, 20.0, "empty")]);
    }
}
