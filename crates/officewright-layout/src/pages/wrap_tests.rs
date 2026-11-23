//! Text wrapped around floating objects on pages (`Pager::wrap`, `crate::wrap`): pages of
//! 300 by 200 pt with margins of 20, so a body from 20 to 280 across and from 20 to 180
//! down, and exact lines of 20 pt of the test font at 10 pt, whose `A` is 6 pt and space
//! 2.5 pt: a word `AAAA` is 24 pt, `k` words are `26.5 k - 2.5`, so 9 fit the column's
//! 260, 5 fit 150 and 3 fit 100. A line's baseline is 16 pt below its top.

use officewright_core::{PageSize, Pt};

use super::{Page, Paginated, paginate};
use crate::display::DisplayItem;
use crate::fonts::Fonts;
use crate::ir::{
    AnchorHorizontalAlign, AnchoredObject, Block, Cell, Document, HeaderFooterSet,
    HorizontalAnchor, HorizontalPlacement, HorizontalReference, ImageData, ImageFormat, Inline,
    Insets, LineSpacing, Margins, ObjectContent, PageSetup, Paragraph, ParagraphProps, Picture,
    Row, RunFonts, RunStyle, RunStyleId, Section, Size, Spacing, Table, VerticalAnchor,
    VerticalPlacement, VerticalReference, Wrap, WrapSide,
};
use crate::report::LayoutReport;
use crate::shape::Shaper;
use crate::testing::{book, minimal};

fn setup() -> PageSetup {
    PageSetup::new(PageSize::new(Pt(300.0), Pt(200.0))).with_margins(
        Margins::default()
            .with_top(Pt(20.0))
            .with_bottom(Pt(20.0))
            .with_left(Pt(20.0))
            .with_right(Pt(20.0)),
    )
}

fn exact() -> ParagraphProps {
    ParagraphProps::default().with_spacing(
        Spacing::default()
            .with_line(LineSpacing::Exact(Pt(20.0)))
            .with_before(Pt(0.0))
            .with_after(Pt(0.0)),
    )
}

/// `count` words `AAAA`, a space between each two.
fn words(count: usize, style: RunStyleId) -> Inline {
    Inline::text(vec!["AAAA"; count].join(" "), style)
}

/// A paragraph of exact lines that starts with `objects`, then `count` words.
fn paragraph(style: RunStyleId, objects: Vec<Inline>, count: usize) -> Block {
    let mut inlines = objects;
    inlines.push(words(count, style));
    Paragraph::new(style)
        .with_props(exact())
        .with_inlines(inlines)
        .into()
}

/// How text wraps around a picture: its wrapping, side and distances.
#[derive(Clone, Copy)]
struct Wrapping {
    wrap: Wrap,
    side: WrapSide,
    /// Left, top, right, bottom.
    distances: [f64; 4],
}

impl Wrapping {
    fn square(distances: [f64; 4]) -> Self {
        Self {
            wrap: Wrap::Square,
            side: WrapSide::BothSides,
            distances,
        }
    }
}

/// A picture of `size` at `across` from the margin (an offset, or right or centred) and
/// `down` from its paragraph's top, wrapped as `wrapping` says.
fn picture(
    document: &mut Document,
    size: (f64, f64),
    across: HorizontalPlacement,
    down: (VerticalReference, f64),
    wrapping: Wrapping,
) -> Inline {
    let image = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
    let [left, top, right, bottom] = wrapping.distances;
    Inline::anchored(
        AnchoredObject::new(
            ObjectContent::Picture(Picture::new(image)),
            Size::new(Pt(size.0), Pt(size.1)),
            HorizontalAnchor::default()
                .with_relative_to(HorizontalReference::Margin)
                .with_placement(across),
            VerticalAnchor::default()
                .with_relative_to(down.0)
                .with_placement(VerticalPlacement::Offset(Pt(down.1))),
        )
        .with_wrap(wrapping.wrap)
        .with_wrap_side(wrapping.side)
        .with_distances(
            Insets::default()
                .with_left(Pt(left))
                .with_top(Pt(top))
                .with_right(Pt(right))
                .with_bottom(Pt(bottom)),
        ),
    )
}

fn at(x: f64) -> HorizontalPlacement {
    HorizontalPlacement::Offset(Pt(x))
}

fn from_paragraph(y: f64) -> (VerticalReference, f64) {
    (VerticalReference::Paragraph, y)
}

/// A document of the test font at 10 pt whose one section of `setup()` holds the blocks
/// `make` gives.
fn document(make: impl FnOnce(RunStyleId, &mut Document) -> Vec<Block>) -> Document {
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Officewright Test"))
            .with_size(Pt(10.0)),
    );
    let blocks = make(style, &mut document);
    document.add_section(Section::new(setup()).with_blocks(blocks));
    document
}

fn lay(document: &Document) -> (Paginated, LayoutReport) {
    let book = book(vec![minimal()]);
    let mut fonts = Fonts::new(&book);
    let mut shaper = Shaper::new();
    let laid = paginate(document, &mut fonts, &mut shaper, 100).expect("pages");
    (laid, fonts.into_report())
}

fn messages(report: &LayoutReport) -> Vec<String> {
    report
        .entries()
        .iter()
        .map(|entry| entry.message().to_owned())
        .collect()
}

/// The lines of the body of `page` by their glyphs: where each starts across, and its
/// baseline, top to bottom.
fn lines(page: &Page) -> Vec<(f64, f64)> {
    let mut lines: Vec<(f64, f64)> = Vec::new();
    for item in page.items.iter().skip(page.frame_items) {
        let DisplayItem::Glyphs(run) = item else {
            continue;
        };
        let (x, y) = (run.origin.x.0, run.origin.y.0);
        match lines.iter_mut().find(|line| (line.1 - y).abs() < 1e-9) {
            Some(line) => line.0 = line.0.min(x),
            None => lines.push((x, y)),
        }
    }
    lines.sort_by(|a, b| a.1.total_cmp(&b.1));
    lines
}

/// The words on each line of the body of `page`, top to bottom.
fn words_per_line(page: &Page) -> Vec<usize> {
    let mut lines: Vec<(f64, usize)> = Vec::new();
    for item in page.items.iter().skip(page.frame_items) {
        let DisplayItem::Glyphs(run) = item else {
            continue;
        };
        let text: String = run.clusters.iter().map(|c| c.text.as_str()).collect();
        let count = text.split_whitespace().count();
        let y = run.origin.y.0;
        match lines.iter_mut().find(|line| (line.0 - y).abs() < 1e-9) {
            Some(line) => line.1 += count,
            None => lines.push((y, count)),
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines.into_iter().map(|(_, count)| count).collect()
}

#[test]
fn lines_beside_an_object_wrapped_square_start_its_distance_past_it() {
    // E-wrap-square case 01: a picture at the margin, at its paragraph's top, 9 pt from the
    // text beside it; here 100 by 50 at 20, 10 pt: the lines whose boxes meet it, from 20,
    // 40 and 60 (whose top is above its bottom at 70), start at 130 and take 5 words; the
    // line from 80 starts at the margin and takes 9.
    let document = document(|style, document| {
        let wrapping = Wrapping::square([10.0, 0.0, 10.0, 0.0]);
        let object = picture(
            document,
            (100.0, 50.0),
            at(0.0),
            from_paragraph(0.0),
            wrapping,
        );
        vec![paragraph(style, vec![object], 20)]
    });
    let (laid, report) = lay(&document);
    let page = &laid.pages[0];
    assert_eq!(
        lines(page),
        [(130.0, 36.0), (130.0, 56.0), (130.0, 76.0), (20.0, 96.0)]
    );
    assert_eq!(words_per_line(page), [5, 5, 5, 5]);
    assert_eq!(page.exclusions.len(), 1);
    assert!(messages(&report).is_empty(), "{:?}", messages(&report));

    // E-wrap-square case 02: at the right margin the lines end its distance before it, at
    // 170, so they take 5 words too, starting at the margin.
    let document = self::document(|style, document| {
        let wrapping = Wrapping::square([10.0, 0.0, 10.0, 0.0]);
        let right = HorizontalPlacement::Align(AnchorHorizontalAlign::Right);
        let object = picture(
            document,
            (100.0, 50.0),
            right,
            from_paragraph(0.0),
            wrapping,
        );
        vec![paragraph(style, vec![object], 20)]
    });
    let (laid, _) = lay(&document);
    let page = &laid.pages[0];
    assert_eq!(
        lines(page),
        [(20.0, 36.0), (20.0, 56.0), (20.0, 76.0), (20.0, 96.0)]
    );
    assert_eq!(words_per_line(page), [5, 5, 5, 5]);
}

#[test]
fn text_goes_on_the_side_of_an_object_that_its_wrap_side_allows() {
    // A picture 40 wide 100 pt from the margin, from 120 to 160, with no distances: 100 pt
    // on its left (3 words), 120 on its right (4 words). E-wrap-square cases 04 (largest),
    // 05 (left) and 06 (right); on both sides, text goes on the larger side only, and that
    // is reported.
    let case = |side: WrapSide| {
        let document = document(|style, document| {
            let wrapping = Wrapping {
                side,
                ..Wrapping::square([0.0; 4])
            };
            let object = picture(
                document,
                (40.0, 50.0),
                at(100.0),
                from_paragraph(0.0),
                wrapping,
            );
            vec![paragraph(style, vec![object], 12)]
        });
        let (laid, report) = lay(&document);
        let page = &laid.pages[0];
        (lines(page), words_per_line(page), messages(&report))
    };
    let left = [(20.0, 36.0), (20.0, 56.0), (20.0, 76.0), (20.0, 96.0)];
    let right = [(160.0, 36.0), (160.0, 56.0), (160.0, 76.0)];
    let none: Vec<String> = Vec::new();
    assert_eq!(
        case(WrapSide::Left),
        (left.to_vec(), vec![3, 3, 3, 3], none.clone())
    );
    assert_eq!(
        case(WrapSide::Right),
        (right.to_vec(), vec![4, 4, 4], none.clone())
    );
    assert_eq!(
        case(WrapSide::Largest),
        (right.to_vec(), vec![4, 4, 4], none)
    );
    assert_eq!(
        case(WrapSide::BothSides),
        (
            right.to_vec(),
            vec![4, 4, 4],
            vec![
                "text with room on both sides of a floating object goes on the side with more \
                 room only"
                    .to_owned()
            ]
        )
    );
}

#[test]
fn a_line_that_meets_an_object_wrapped_top_and_bottom_goes_below_it() {
    // E-wrap-top-bottom cases 02 and 04: a picture 30 high, 25 below the paragraph's top
    // (20), with 5 pt above and below: its band is from 40 to 80. The first line, from 20
    // to 40, stays above it; the second goes below it, at 80, though there is room beside
    // it; the paragraph after starts below the last line, at 120.
    let document = document(|style, document| {
        let wrapping = Wrapping {
            wrap: Wrap::TopAndBottom,
            side: WrapSide::BothSides,
            distances: [9.0, 5.0, 9.0, 5.0],
        };
        let object = picture(
            document,
            (60.0, 30.0),
            at(0.0),
            from_paragraph(25.0),
            wrapping,
        );
        vec![
            paragraph(style, vec![object], 20),
            paragraph(style, Vec::new(), 1),
        ]
    });
    let (laid, report) = lay(&document);
    let page = &laid.pages[0];
    assert_eq!(
        lines(page),
        [(20.0, 36.0), (20.0, 96.0), (20.0, 116.0), (20.0, 136.0)]
    );
    assert_eq!(words_per_line(page), [9, 9, 2, 1]);
    let fragments: Vec<(f64, f64)> = page
        .fragments
        .iter()
        .map(|fragment| (fragment.top.0, fragment.height.0))
        .collect();
    assert_eq!(fragments, [(20.0, 100.0), (120.0, 20.0)]);
    assert!(messages(&report).is_empty(), "{:?}", messages(&report));
}

#[test]
fn a_line_with_too_little_room_beside_an_object_goes_below_it() {
    // A picture 240 wide centred, from 30 to 270, leaves 10 pt on each side, too little
    // for text (`wrap::LEAST_ROOM`, a hypothesis): the first line goes below it, at 50.
    let document = document(|style, document| {
        let center = HorizontalPlacement::Align(AnchorHorizontalAlign::Center);
        let wrapping = Wrapping::square([0.0; 4]);
        let object = picture(
            document,
            (240.0, 30.0),
            center,
            from_paragraph(0.0),
            wrapping,
        );
        vec![paragraph(style, vec![object], 10)]
    });
    let (laid, _) = lay(&document);
    assert_eq!(lines(&laid.pages[0]), [(20.0, 66.0), (20.0, 86.0)]);
}

#[test]
fn the_paragraphs_after_a_one_line_anchor_paragraph_wrap_too() {
    // E-wrap-square case 10: the anchor paragraph holds one line, and the lines of the
    // paragraph after it that meet the picture go beside it.
    let document = document(|style, document| {
        let wrapping = Wrapping::square([10.0, 0.0, 10.0, 0.0]);
        let object = picture(
            document,
            (100.0, 50.0),
            at(0.0),
            from_paragraph(0.0),
            wrapping,
        );
        vec![
            paragraph(style, vec![object], 1),
            paragraph(style, Vec::new(), 20),
        ]
    });
    let (laid, _) = lay(&document);
    let page = &laid.pages[0];
    assert_eq!(
        lines(page),
        [
            (130.0, 36.0),
            (130.0, 56.0),
            (130.0, 76.0),
            (20.0, 96.0),
            (20.0, 116.0)
        ]
    );
    assert_eq!(words_per_line(page), [1, 5, 5, 9, 1]);
}

#[test]
fn a_chain_that_moves_takes_the_wrapping_of_its_objects_along() {
    // Six lines fill page 1 down to 140; a paragraph of one line kept with the next anchors
    // a picture wrapped square; the next paragraph, three lines kept together, does not fit
    // under it, so both move to page 2. Going back drops the picture and what it keeps text
    // out of from page 1, and on page 2 the lines of both wrap around it.
    let document = document(|style, document| {
        let mut blocks: Vec<Block> = (0..6).map(|_| paragraph(style, Vec::new(), 1)).collect();
        let wrapping = Wrapping::square([10.0, 0.0, 10.0, 0.0]);
        let object = picture(
            document,
            (100.0, 50.0),
            at(0.0),
            from_paragraph(0.0),
            wrapping,
        );
        blocks.push(
            Paragraph::new(style)
                .with_props(exact().with_keep_next(true))
                .with_inlines([object, words(1, style)])
                .into(),
        );
        blocks.push(
            Paragraph::new(style)
                .with_props(exact().with_keep_lines(true))
                .with_inlines([words(15, style)])
                .into(),
        );
        blocks
    });
    let (laid, _) = lay(&document);
    let pages = &laid.pages;
    assert_eq!(pages.len(), 2);
    let pictures = |page: &Page| {
        page.items
            .iter()
            .filter(|item| matches!(item, DisplayItem::Image(_)))
            .count()
    };
    assert_eq!((pictures(&pages[0]), pictures(&pages[1])), (0, 1));
    assert!(pages[0].exclusions.is_empty());
    assert_eq!(pages[1].exclusions.len(), 1);
    assert_eq!(
        lines(&pages[1]),
        [(130.0, 36.0), (130.0, 56.0), (130.0, 76.0), (20.0, 96.0)]
    );
    assert_eq!(words_per_line(&pages[1]), [1, 5, 5, 5]);
}

#[test]
fn lines_that_go_on_to_the_next_page_are_laid_out_again_for_it() {
    // Four lines fill page 1 down to 100; the next paragraph anchors a picture taller than
    // the rest of the page at its top: its four lines there start at 130 and take 5 words
    // each, and its fifth, 2 words, does not fit. Widow control moves the fourth line on
    // with it, and on page 2, where nothing is beside them, the 7 words left take one line
    // from the margin.
    let document = document(|style, document| {
        let mut blocks: Vec<Block> = (0..4).map(|_| paragraph(style, Vec::new(), 1)).collect();
        let wrapping = Wrapping::square([10.0, 0.0, 10.0, 0.0]);
        let object = picture(
            document,
            (100.0, 200.0),
            at(0.0),
            from_paragraph(0.0),
            wrapping,
        );
        blocks.push(paragraph(style, vec![object], 22));
        blocks
    });
    let (laid, _) = lay(&document);
    let pages = &laid.pages;
    assert_eq!(pages.len(), 2);
    assert_eq!(
        lines(&pages[0]),
        [
            (20.0, 36.0),
            (20.0, 56.0),
            (20.0, 76.0),
            (20.0, 96.0),
            (130.0, 116.0),
            (130.0, 136.0),
            (130.0, 156.0)
        ]
    );
    assert_eq!(words_per_line(&pages[0]), [1, 1, 1, 1, 5, 5, 5]);
    assert_eq!(lines(&pages[1]), [(20.0, 36.0)]);
    assert_eq!(words_per_line(&pages[1]), [7]);
    assert!(pages[1].exclusions.is_empty());
    let last = pages[1]
        .fragments
        .first()
        .expect("the paragraph's last line");
    assert_eq!((last.paragraph, last.lines.clone()), (4, 3..4));
}

#[test]
fn objects_no_line_meets_leave_the_layout_as_without_wrapping() {
    // A document laid out with pictures that text would wrap around, where none of its
    // lines meets one, is laid out as with pictures that it does not wrap around: one
    // below the text, one in the page's right margin beside the lines, whose band they
    // meet but whose room it leaves as it is.
    let pages = |wrap: Wrap| {
        let document = document(|style, document| {
            let wrapping = Wrapping {
                wrap,
                ..Wrapping::square([10.0, 0.0, 10.0, 0.0])
            };
            let below = picture(
                document,
                (100.0, 20.0),
                at(0.0),
                from_paragraph(100.0),
                wrapping,
            );
            let margin = picture(
                document,
                (10.0, 60.0),
                at(275.0),
                from_paragraph(0.0),
                wrapping,
            );
            vec![
                paragraph(style, vec![below, margin], 20),
                paragraph(style, Vec::new(), 3),
            ]
        });
        lay(&document).0.pages
    };
    // What a page draws, its pictures by place and size: each document numbers its
    // images apart.
    let drawn = |page: &Page| -> Vec<String> {
        page.items
            .iter()
            .map(|item| match item {
                DisplayItem::Image(image) => {
                    format!("{:?} {:?} {:?}", image.at, image.width, image.height)
                }
                other => format!("{other:?}"),
            })
            .collect()
    };
    let (square, none) = (pages(Wrap::Square), pages(Wrap::None));
    assert_eq!(square[0].exclusions.len(), 2);
    assert_eq!(square.len(), none.len());
    for (square, none) in square.iter().zip(&none) {
        assert_eq!(drawn(square), drawn(none));
        assert_eq!(square.fragments, none.fragments);
    }
}

#[test]
fn what_text_does_not_wrap_around_yet_is_reported() {
    // A picture wrapped square in the header; in the body, a paragraph of one line, then
    // one that anchors a picture wrapped tight, whose 30 pt above reach up to that line, and
    // a table beside the picture.
    let document = {
        let mut document = Document::default();
        let style = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Officewright Test"))
                .with_size(Pt(10.0)),
        );
        let wrapping = Wrapping::square([0.0; 4]);
        let logo = picture(
            &mut document,
            (30.0, 10.0),
            at(0.0),
            from_paragraph(0.0),
            wrapping,
        );
        let tight = Wrapping {
            wrap: Wrap::Tight,
            ..Wrapping::square([0.0, 30.0, 0.0, 0.0])
        };
        let object = picture(
            &mut document,
            (100.0, 100.0),
            at(0.0),
            from_paragraph(0.0),
            tight,
        );
        let table =
            Table::default()
                .with_grid([Pt(100.0)])
                .with_rows(vec![Row::default().with_cells(vec![
                    Cell::default().with_blocks([paragraph(style, Vec::new(), 1)]),
                ])]);
        let header = Paragraph::new(style).with_inlines([logo, words(1, style)]);
        document.add_section(
            Section::new(setup())
                .with_headers(HeaderFooterSet::default().with_primary([header.into()]))
                .with_blocks(vec![
                    paragraph(style, Vec::new(), 1),
                    paragraph(style, vec![object], 1),
                    table.into(),
                ]),
        );
        document
    };
    let (_, report) = lay(&document);
    let messages = messages(&report);
    for reported in [
        "text does not wrap around the floating objects of headers, footers and notes yet: it \
         runs over or under them",
        "text wrapped tight or through a floating object goes around its frame, not its outline",
        "text above the paragraph that anchors a floating object does not wrap around it yet: \
         it runs over or under it",
        "tables do not wrap around floating objects yet: a table that reaches one runs over or \
         under it",
    ] {
        assert!(messages.iter().any(|m| m == reported), "{messages:?}");
    }
}

#[test]
fn laying_paragraphs_out_again_around_objects_is_bounded() {
    use super::{MAX_FRAME_DRAWN, MAX_FRAME_LAID, paginate_wrapping};
    use crate::tables::MAX_TABLE_REDRAWN;

    // E-wrap-square case 01 as above; with less budget than laying its paragraph out again
    // costs, the paragraph's lines stay as first laid out, over the picture, and that is
    // reported; with the cost of one, they go beside it.
    let document = document(|style, document| {
        let wrapping = Wrapping::square([10.0, 0.0, 10.0, 0.0]);
        let object = picture(
            document,
            (100.0, 50.0),
            at(0.0),
            from_paragraph(0.0),
            wrapping,
        );
        vec![paragraph(style, vec![object], 20)]
    });
    let cost = match document.sections().first().and_then(|s| s.blocks().first()) {
        Some(Block::Paragraph(paragraph)) => super::paragraph_cost(&document, paragraph, true),
        _ => panic!("a paragraph"),
    };
    let wrapped = |budget: usize| {
        let book = book(vec![minimal()]);
        let mut fonts = Fonts::new(&book);
        let mut shaper = Shaper::new();
        let budgets = ((MAX_FRAME_DRAWN, MAX_FRAME_LAID, MAX_TABLE_REDRAWN), budget);
        let laid = paginate_wrapping(&document, (&mut fonts, &mut shaper), 100, budgets, Vec::new)
            .expect("pages");
        let page = laid.sink.first().expect("a page");
        (lines(page), messages(&fonts.into_report()))
    };
    let (over, report) = wrapped(cost - 1);
    assert_eq!(over, [(20.0, 36.0), (20.0, 56.0), (20.0, 76.0)]);
    assert_eq!(
        report,
        [format!(
            "text stops wrapping around floating objects past {} characters and pieces of \
             paragraphs laid out again around them: the lines after stay as first laid out",
            super::MAX_WRAP_LAID
        )]
    );
    let (beside, report) = wrapped(cost);
    assert_eq!(beside.first(), Some(&(130.0, 36.0)));
    assert!(report.is_empty(), "{report:?}");
}

#[test]
fn text_wraps_around_the_first_objects_of_a_page_only() {
    use crate::wrap::MAX_EXCLUSIONS;

    // More pictures wrapped square than text goes around, in the page's left margin,
    // where they leave the lines their room: only the first are kept, and that is
    // reported.
    let document = document(|style, document| {
        let objects = (0..=MAX_EXCLUSIONS)
            .map(|_| {
                let wrapping = Wrapping::square([0.0; 4]);
                picture(
                    document,
                    (5.0, 30.0),
                    at(-15.0),
                    from_paragraph(0.0),
                    wrapping,
                )
            })
            .collect();
        vec![paragraph(style, objects, 20)]
    });
    let (laid, report) = lay(&document);
    let page = &laid.pages[0];
    assert_eq!(page.exclusions.len(), MAX_EXCLUSIONS);
    assert_eq!(lines(page), [(20.0, 36.0), (20.0, 56.0), (20.0, 76.0)]);
    assert_eq!(
        messages(&report),
        [format!(
            "text wraps around the first {MAX_EXCLUSIONS} floating objects of a page only: it \
             runs over or under the others"
        )]
    );
}

#[test]
fn links_and_bookmarks_on_a_line_below_an_object_are_where_its_text_is() {
    use crate::ir::LinkTarget;

    // The picture of the test above, wrapped top and bottom with its band from 40 to 80:
    // the first line, 9 words, stays above it; the second, which starts with a bookmark
    // and two linked words, goes below it, at 80. The link's box is that line's own, from
    // 80 to 100, around its baseline at 96, and the bookmark marks its start edge and top.
    let document = document(|style, document| {
        let wrapping = Wrapping {
            wrap: Wrap::TopAndBottom,
            side: WrapSide::BothSides,
            distances: [9.0, 5.0, 9.0, 5.0],
        };
        let object = picture(
            document,
            (60.0, 30.0),
            at(0.0),
            from_paragraph(25.0),
            wrapping,
        );
        vec![
            Paragraph::new(style)
                .with_props(exact())
                .with_inlines([
                    object,
                    Inline::text(["AAAA"; 9].join(" ") + " ", style),
                    Inline::bookmark("Below"),
                    Inline::link_start(LinkTarget::Uri("https://example.com/".into())),
                    words(2, style),
                    Inline::link_end(),
                    Inline::text(" AAAA", style),
                ])
                .into(),
        ]
    });
    let (laid, _) = lay(&document);
    let page = &laid.pages[0];
    assert_eq!(lines(page), [(20.0, 36.0), (20.0, 96.0)]);
    let marks: Vec<String> = page
        .items
        .iter()
        .filter_map(|item| match item {
            DisplayItem::Link {
                at, width, height, ..
            } => Some(format!(
                "link {} {} {} {}",
                at.x.0, at.y.0, width.0, height.0
            )),
            DisplayItem::Destination { at, name } => {
                Some(format!("bookmark {name} {} {}", at.x.0, at.y.0))
            }
            _ => None,
        })
        .collect();
    assert_eq!(marks, ["bookmark Below 20 80", "link 20 80 50.5 20"]);
}

#[test]
fn objects_whose_effects_take_more_than_their_frames_keep_the_room_beside_them_small() {
    use crate::wrap::MAX_EXCLUSIONS;

    // As many pictures as a page wraps around, each 10 pt wide at 100 pt from the margin,
    // from 120 to 130, 50 high, with effects of -20 pt at the left and right, which take
    // more than its frame: each keeps text out of an empty band at its middle, 125. The
    // lines beside them go on the larger side, from 125 to 280, 5 words each, and the
    // room beside them stays two parts, not one for each way of taking a side of each.
    let document = document(|style, document| {
        let objects = (0..MAX_EXCLUSIONS)
            .map(|_| {
                let image = document.add_image(ImageData::new(ImageFormat::Png, &b"\x89PNG"[..]));
                Inline::anchored(
                    AnchoredObject::new(
                        ObjectContent::Picture(Picture::new(image)),
                        Size::new(Pt(10.0), Pt(50.0)),
                        HorizontalAnchor::default()
                            .with_relative_to(HorizontalReference::Margin)
                            .with_placement(at(100.0)),
                        VerticalAnchor::default()
                            .with_relative_to(VerticalReference::Paragraph)
                            .with_placement(VerticalPlacement::Offset(Pt(0.0))),
                    )
                    .with_wrap(Wrap::Square)
                    .with_effect_extent(
                        Insets::default().with_left(Pt(-20.0)).with_right(Pt(-20.0)),
                    ),
                )
            })
            .collect();
        vec![paragraph(style, objects, 200)]
    });
    let (laid, _) = lay(&document);
    let page = &laid.pages[0];
    assert_eq!(page.exclusions.len(), MAX_EXCLUSIONS);
    assert!(
        page.exclusions
            .iter()
            .all(|exclusion| (exclusion.left, exclusion.right) == (125.0, 125.0))
    );
    assert_eq!(
        lines(page).get(..4),
        Some(&[(125.0, 36.0), (125.0, 56.0), (125.0, 76.0), (20.0, 96.0)][..])
    );
    assert_eq!(words_per_line(page).get(..4), Some(&[5, 5, 5, 9][..]));
}

#[test]
fn lines_go_around_the_objects_that_their_page_draws_only() {
    // Twelve lines of 9 words, the last of which anchors a picture: page 1 takes 8 lines,
    // and the picture goes on to page 2 with its line, where it is drawn. Page 1's lines go
    // around nothing; page 2's first three go around the picture, at the paragraph's top
    // there, 5 words each, and the 21 words left take three lines from the margin.
    let document = document(|style, document| {
        let wrapping = Wrapping::square([10.0, 0.0, 10.0, 0.0]);
        let object = picture(
            document,
            (100.0, 50.0),
            at(0.0),
            from_paragraph(0.0),
            wrapping,
        );
        vec![
            Paragraph::new(style)
                .with_props(exact())
                .with_inlines([words(108, style), Inline::text(" ", style), object])
                .into(),
        ]
    });
    let (laid, report) = lay(&document);
    let pages = &laid.pages;
    assert_eq!(pages.len(), 2);
    let pictures = |page: &Page| -> Vec<(f64, f64)> {
        page.items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Image(image) => Some((image.at.x.0, image.at.y.0)),
                _ => None,
            })
            .collect()
    };
    assert!(pictures(&pages[0]).is_empty());
    assert!(pages[0].exclusions.is_empty());
    assert_eq!(words_per_line(&pages[0]), [9; 8]);
    assert!(lines(&pages[0]).iter().all(|&(x, _)| x == 20.0));
    assert_eq!(pictures(&pages[1]), [(20.0, 20.0)]);
    assert_eq!(
        lines(&pages[1]),
        [
            (130.0, 36.0),
            (130.0, 56.0),
            (130.0, 76.0),
            (20.0, 96.0),
            (20.0, 116.0),
            (20.0, 136.0)
        ]
    );
    assert_eq!(words_per_line(&pages[1]), [5, 5, 5, 9, 9, 3]);
    assert!(messages(&report).is_empty(), "{:?}", messages(&report));
}

#[test]
fn a_line_whose_object_takes_the_whole_body_goes_below_it_on_the_page_that_draws_it() {
    // A paragraph of one line, then one of 120 words that anchors, after its 20th word, in
    // its third line, a picture as large as the body, wrapped top and bottom, placed from
    // the margin. Page 1 takes the second paragraph's first two lines, from the margin, as
    // the picture goes on with its line: its line starts page 2, which draws the picture,
    // and goes below it, past the body, which is reported. The rest goes on from page 3 on,
    // from the margin, around nothing.
    let document = document(|style, document| {
        let wrapping = Wrapping {
            wrap: Wrap::TopAndBottom,
            side: WrapSide::BothSides,
            distances: [0.0; 4],
        };
        let object = picture(
            document,
            (260.0, 160.0),
            at(0.0),
            (VerticalReference::Margin, 0.0),
            wrapping,
        );
        vec![
            paragraph(style, Vec::new(), 1),
            Paragraph::new(style)
                .with_props(exact())
                .with_inlines([
                    words(20, style),
                    object,
                    Inline::text(" ", style),
                    words(100, style),
                ])
                .into(),
        ]
    });
    let (laid, report) = lay(&document);
    let pages = &laid.pages;
    let drawn: Vec<usize> = pages
        .iter()
        .map(|page| {
            page.items
                .iter()
                .filter(|item| matches!(item, DisplayItem::Image(_)))
                .count()
        })
        .collect();
    assert_eq!(drawn, [0, 1, 0, 0]);
    assert_eq!(lines(&pages[0]), [(20.0, 36.0), (20.0, 56.0), (20.0, 76.0)]);
    assert_eq!(words_per_line(&pages[0]), [1, 9, 9]);
    assert_eq!(lines(&pages[1]), [(20.0, 196.0)]);
    assert_eq!(words_per_line(&pages[2]), [9; 8]);
    assert_eq!(words_per_line(&pages[3]), [9, 9, 3]);
    assert!(pages[2].exclusions.is_empty() && pages[3].exclusions.is_empty());
    let messages = messages(&report);
    assert!(
        messages
            .iter()
            .any(|m| m == "a line is taller than the body of its page"),
        "{messages:?}"
    );
}

#[test]
fn an_object_that_going_around_it_would_take_to_the_next_page_is_reported() {
    // Seven lines fill page 1 down to 160; the last line there is the first of a paragraph
    // without widow control that anchors a picture after its sixth word. Going around the
    // picture, the line takes 5 words, and the picture goes on to page 2 with the sixth;
    // without it, the line takes 9 words, the picture among them. It stays there, drawn
    // at the paragraph's top over the line, and that is reported.
    let document = document(|style, document| {
        let mut blocks: Vec<Block> = (0..7).map(|_| paragraph(style, Vec::new(), 1)).collect();
        let wrapping = Wrapping::square([10.0, 0.0, 10.0, 0.0]);
        let object = picture(
            document,
            (100.0, 50.0),
            at(0.0),
            from_paragraph(0.0),
            wrapping,
        );
        blocks.push(
            Paragraph::new(style)
                .with_props(exact().with_widow_control(false))
                .with_inlines([
                    words(6, style),
                    object,
                    Inline::text(" ".to_owned() + &["AAAA"; 14].join(" "), style),
                ])
                .into(),
        );
        blocks
    });
    let (laid, report) = lay(&document);
    let page = &laid.pages[0];
    assert_eq!(lines(page).last(), Some(&(20.0, 176.0)));
    assert_eq!(words_per_line(page).last(), Some(&9));
    let pictures: Vec<(f64, f64)> = page
        .items
        .iter()
        .filter_map(|item| match item {
            DisplayItem::Image(image) => Some((image.at.x.0, image.at.y.0)),
            _ => None,
        })
        .collect();
    assert_eq!(pictures, [(20.0, 160.0)]);
    let messages = messages(&report);
    let reported = "a line does not go around a floating object it holds where going around it \
                    would take the object to the next page: it runs over or under it";
    assert!(messages.iter().any(|m| m == reported), "{messages:?}");
}
