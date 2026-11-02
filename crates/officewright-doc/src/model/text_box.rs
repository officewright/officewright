//! Text boxes: shapes whose content (`w:txbxContent`, ECMA-376 Part 1, §17.17.1.1) is a
//! story of its own, in a drawing (`wps:wsp` with `wps:txbx`, [MS-ODRAWXML] 2.3.3) or in
//! VML (a shape with a `v:textbox`, Part 4, §19.1.2.22). The content is read into the
//! model with the document; the properties of the box around it are read from the kept
//! XML when asked for.

use officewright_core::Pt;

use super::drawing::{Offsets, boolean, child, coordinate, value};
use super::properties::xml_enum;
use super::views::{Block, blocks};
use super::vml::{self, Style};
use super::xml::Element;
use super::{Document, Names};
use crate::docx::properties::Report;

/// DrawingML's insets of a text box when not written, in EMUs: 0.1 in across, 0.05 in down
/// (`wps:bodyPr`, Part 1, §21.1.2.1.1). VML's are the same (`v:textbox`'s `inset`).
const INSETS: [f64; 4] = [91_440.0, 45_720.0, 91_440.0, 45_720.0];

/// The properties of a `v:textbox`'s `style` that Word reads and the readers take
/// (MS-OI29500, Part 4, §19.1.2.22(h)).
const TEXT_BOX_STYLE: [&str; 5] = [
    "layout-flow",
    "mso-layout-flow-alt",
    "mso-fit-shape-to-text",
    "mso-next-textbox",
    "v-text-anchor",
];

/// A text box: a shape whose content is a story of its own, in a drawing (`wps:wsp` with
/// `wps:txbx`) or in VML (a shape with a `v:textbox`). Its paragraphs and tables are
/// read with the document; the box's insets, the anchoring of its text and what it does
/// not lay out plainly (a turn, vertical text, WordArt, columns, a link to another box)
/// are read from its shape when asked for.
///
/// ```
/// # fn main() -> officewright_doc::Result<()> {
/// use officewright_core::Pt;
/// use officewright_doc::{Block, Document, RunContent, TextBoxAnchor};
///
/// let document = Document::open("tests/data/text-box.docx")?;
/// let mut boxes = Vec::new();
/// for section in document.sections() {
///     for block in section.blocks() {
///         let Block::Paragraph(paragraph) = block else {
///             continue;
///         };
///         for item in paragraph.runs().flat_map(|run| run.content()) {
///             let RunContent::Drawing(drawing) = item else {
///                 continue;
///             };
///             let Some(text_box) = drawing.text_box() else {
///                 continue;
///             };
///             let text: Vec<String> = text_box
///                 .blocks()
///                 .filter_map(|block| match block {
///                     Block::Paragraph(paragraph) => Some(paragraph.text()),
///                     _ => None,
///                 })
///                 .collect();
///             boxes.push((text, text_box.insets().left(), text_box.vertical_anchor()));
///         }
///     }
/// }
/// assert_eq!(
///     boxes,
///     [(vec!["In the box".to_owned()], Pt(7.2), TextBoxAnchor::Center)]
/// );
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct TextBox<'a> {
    document: &'a Document,
    /// The `wps:wsp`, or the VML shape.
    shape: Element<'a>,
    /// For a VML shape, its `v:textbox`.
    vml: Option<Element<'a>>,
    blocks: &'a [u32],
}

impl<'a> TextBox<'a> {
    /// The text box of a drawing (`w:drawing`), if its shape has one whose content was
    /// read.
    pub(crate) fn in_drawing(document: &'a Document, drawing: Element<'a>) -> Option<Self> {
        let shape = drawing_shape(drawing, &document.names)?;
        Self::new(document, shape, None)
    }

    /// The text boxes of a VML drawing (`w:pict`) whose content was read, those of the
    /// shapes of its groups included, in document order.
    pub(crate) fn in_vml(
        document: &'a Document,
        pict: Element<'a>,
    ) -> impl Iterator<Item = Self> + 'a {
        vml_shapes(pict, &document.names)
            .into_iter()
            .filter_map(move |(shape, text_box)| Self::new(document, shape, Some(text_box)))
    }

    /// The text box of a shape; none when it has content that was not read, as a text
    /// box in another has.
    fn new(document: &'a Document, shape: Element<'a>, vml: Option<Element<'a>>) -> Option<Self> {
        let blocks = match content(shape, vml, &document.names) {
            Some(content) => {
                let stories = &document.text_boxes;
                let at = stories
                    .binary_search_by_key(&content.index(), |story| story.content)
                    .ok()?;
                stories.get(at)?.blocks.as_slice()
            }
            // A box later in a chain of linked boxes has no content of its own.
            None => &[],
        };
        Some(Self {
            document,
            shape,
            vml,
            blocks,
        })
    }

    /// Its paragraphs, tables and other blocks; none for a box later in a chain of linked
    /// boxes, whose text the first box of the chain holds.
    pub fn blocks(self) -> impl Iterator<Item = Block<'a>> + 'a {
        blocks(self.document, self.blocks)
    }

    /// The space between its edges and its text (`wps:bodyPr`'s `lIns`, `tIns`, `rIns`
    /// and `bIns`, or `v:textbox`'s `inset`); 7.2 pt across and 3.6 pt down where not
    /// written.
    pub fn insets(self) -> Offsets {
        insets(
            self.shape,
            self.vml,
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Where its text is from top to bottom (`wps:bodyPr`'s `anchor`, or `v-text-anchor`
    /// in the `style` of the `v:textbox` or of its shape); at the top where not written.
    pub fn vertical_anchor(self) -> TextBoxAnchor {
        vertical_anchor(
            self.shape,
            self.vml,
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Whether the box grows to the height of its text (`a:spAutoFit`, or
    /// `mso-fit-shape-to-text` in the `style` of the `v:textbox`).
    pub fn grows_to_fit(self) -> bool {
        grows_to_fit(
            self.shape,
            self.vml,
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Whether text taller than the box is cut at its bottom (`wps:bodyPr`'s
    /// `vertOverflow` `clip` or `ellipsis`), rather than running on past it.
    pub fn clips_overflow(self) -> bool {
        clips_overflow(
            self.shape,
            self.vml,
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// How far the box is turned clockwise, in degrees (`a:xfrm`'s `rot`, or `rotation`
    /// in the `style` of its VML shape); 0 where not written.
    pub fn rotation(self) -> f64 {
        rotation(
            self.shape,
            self.vml,
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Which way its text runs (`wps:bodyPr`'s `vert`, or `layout-flow` and
    /// `mso-layout-flow-alt` in the `style` of the `v:textbox`).
    pub fn text_direction(self) -> TextBoxDirection {
        text_direction(
            self.shape,
            self.vml,
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Whether it is WordArt: its text drawn as a shape (`wps:bodyPr`'s `fromWordArt`,
    /// or a warp of its text, `a:prstTxWarp`, other than `textNoShape`).
    pub fn is_word_art(self) -> bool {
        is_word_art(
            self.shape,
            self.vml,
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// How many columns its text is in (`wps:bodyPr`'s `numCol`); 1 where not written.
    pub fn columns(self) -> u32 {
        columns(
            self.shape,
            self.vml,
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Whether it is in a chain of linked boxes, whose text runs on from one box to the
    /// next: the first (`wps:txbx`'s `id`, or `mso-next-textbox` in the `style` of the
    /// `v:textbox` naming another shape) or a later one (`wps:linkedTxbx`).
    pub fn is_linked(self) -> bool {
        is_linked(self.shape, self.vml, &self.document.names)
    }
}

xml_enum! {
    /// Where the text of a text box is from top to bottom (ST_TextAnchoringType).
    TextBoxAnchor {
        Top = "t",
        Center = "ctr",
        Bottom = "b",
        /// Its lines spread over the height of the box.
        Justified = "just",
        /// Its lines spread over the height of the box, the first and last at its edges.
        Distributed = "dist",
    }
}

xml_enum! {
    /// Which way the text of a text box runs (ST_TextVerticalType).
    TextBoxDirection {
        /// Across, as in the body.
        Horizontal = "horz",
        /// Down, turned a quarter clockwise.
        Vertical = "vert",
        /// Up, turned a quarter anticlockwise.
        Vertical270 = "vert270",
        /// Down, each character upright, as WordArt sets it.
        WordArtVertical = "wordArtVert",
        /// Down, East Asian characters upright and others turned, lines from right to left.
        EastAsianVertical = "eaVert",
        /// Down, turned a quarter clockwise, lines from left to right.
        MongolianVertical = "mongolianVert",
        /// Down, each character upright, lines from right to left.
        WordArtVerticalRightToLeft = "wordArtVertRtl",
    }
}

/// Reports the values of the text boxes of a drawing (`w:drawing`) or a VML drawing
/// (`w:pict`) that the readers do not take, as the document opens; the readers read them
/// again when asked for, as not written.
pub(crate) fn check(holder: Element<'_>, names: &Names, report: Report<'_>) {
    let drawing = drawing_shape(holder, names).map(|shape| (shape, None));
    let shapes = vml_shapes(holder, names)
        .into_iter()
        .map(|(shape, text_box)| (shape, Some(text_box)));
    for (shape, vml) in drawing.into_iter().chain(shapes) {
        insets(shape, vml, names, report);
        vertical_anchor(shape, vml, names, report);
        grows_to_fit(shape, vml, names, report);
        clips_overflow(shape, vml, names, report);
        rotation(shape, vml, names, report);
        text_direction(shape, vml, names, report);
        is_word_art(shape, vml, names, report);
        columns(shape, vml, names, report);
    }
}

/// The content (`w:txbxContent`) of each text box of a drawing or a VML drawing.
pub(crate) fn contents<'a>(
    holder: Element<'a>,
    names: &'a Names,
) -> impl Iterator<Item = Element<'a>> + 'a {
    let drawing = drawing_shape(holder, names).map(|shape| (shape, None));
    let shapes = vml_shapes(holder, names)
        .into_iter()
        .map(|(shape, text_box)| (shape, Some(text_box)));
    drawing
        .into_iter()
        .chain(shapes)
        .filter_map(|(shape, vml)| content(shape, vml, names))
}

/// The shape of a drawing (`w:drawing`) if it is a text box: a `wps:wsp` with a
/// `wps:txbx`, or a `wps:linkedTxbx` when later in a chain of linked boxes.
fn drawing_shape<'a>(drawing: Element<'a>, names: &Names) -> Option<Element<'a>> {
    let shape = super::drawing::shape(drawing, names)?;
    let wps = [names.wps];
    (child(shape, names, &wps, "txbx").is_some()
        || child(shape, names, &wps, "linkedTxbx").is_some())
    .then_some(shape)
}

/// The shapes of a VML drawing that are text boxes, with their `v:textbox` (Office takes
/// at most one, MS-OI29500, Part 4, §19.1.2.22(j): the first), those of its groups
/// (`v:group`) included, in document order.
fn vml_shapes<'a>(pict: Element<'a>, names: &Names) -> Vec<(Element<'a>, Element<'a>)> {
    let mut shapes = Vec::new();
    // The groups still to look into, each with the shapes of it not looked at yet, so
    // that groups in groups are walked without recursion.
    let mut open = vec![pict.children_through_alternates(names.mc)];
    while let Some(children) = open.last_mut() {
        let Some(child) = children.next() else {
            open.pop();
            continue;
        };
        if child.is(&names.v, "group") {
            open.push(child.children_through_alternates(names.mc));
        } else if child.local_in(&names.v).is_some() {
            let text_box = child
                .children_through_alternates(names.mc)
                .find(|part| part.is(&names.v, "textbox"));
            if let Some(text_box) = text_box {
                shapes.push((child, text_box));
            }
        }
    }
    shapes
}

/// The content of a text box (`w:txbxContent`), if it has one.
fn content<'a>(shape: Element<'a>, vml: Option<Element<'a>>, names: &Names) -> Option<Element<'a>> {
    let holder = match vml {
        Some(text_box) => text_box,
        None => child(shape, names, &[names.wps], "txbx")?,
    };
    child(holder, names, &names.w, "txbxContent")
}

/// A shape's `wps:bodyPr`.
fn body<'a>(shape: Element<'a>, names: &Names) -> Option<Element<'a>> {
    child(shape, names, &[names.wps], "bodyPr")
}

/// The declarations of a `v:textbox`'s `style` that the readers take.
fn text_box_style<'a>(text_box: Element<'a>) -> Style<'a, 5> {
    Style::new(
        "textbox",
        text_box.attribute(&[None], "style").unwrap_or_default(),
        &TEXT_BOX_STYLE,
    )
}

fn insets(
    shape: Element<'_>,
    vml: Option<Element<'_>>,
    names: &Names,
    report: Report<'_>,
) -> Offsets {
    let emus = |emus: f64| Pt(emus / 12_700.0);
    let [left, top, right, bottom] = INSETS.map(emus);
    let mut insets = Offsets {
        left,
        top,
        right,
        bottom,
    };
    match vml {
        Some(text_box) => {
            let Some(written) = text_box.attribute(&[None], "inset") else {
                return insets;
            };
            let sides = [
                &mut insets.left,
                &mut insets.top,
                &mut insets.right,
                &mut insets.bottom,
            ];
            // An empty or missing side keeps its default.
            for (side, part) in sides.into_iter().zip(written.split(',')) {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                match vml::length(part) {
                    Some(length) => *side = length,
                    None => {
                        let part = crate::docx::shorten(part);
                        report("v:textbox", "inset", &part);
                    }
                }
            }
        }
        None => {
            if let Some(body) = body(shape, names) {
                let sides = [
                    ("lIns", &mut insets.left),
                    ("tIns", &mut insets.top),
                    ("rIns", &mut insets.right),
                    ("bIns", &mut insets.bottom),
                ];
                for (name, side) in sides {
                    if let Some(length) = value(body, names, name, coordinate, report) {
                        *side = length;
                    }
                }
            }
        }
    }
    insets
}

fn vertical_anchor(
    shape: Element<'_>,
    vml: Option<Element<'_>>,
    names: &Names,
    report: Report<'_>,
) -> TextBoxAnchor {
    let anchor = match vml {
        // Word reads it on the v:textbox (MS-OI29500, Part 4, §19.1.2.22(h)) and writes it
        // on the shape.
        Some(text_box) => text_box_style(text_box)
            .read("v-text-anchor", vml_anchor, report)
            .or_else(|| vml::shape_style(shape).read("v-text-anchor", vml_anchor, report)),
        None => body(shape, names)
            .and_then(|body| value(body, names, "anchor", TextBoxAnchor::from_xml, report)),
    };
    anchor.unwrap_or(TextBoxAnchor::Top)
}

/// The anchoring of VML's `v-text-anchor`: its part from top to bottom. Its centring
/// across (`top-center`) and its anchoring at a baseline (`top-baseline`) are not read.
fn vml_anchor(value: &str) -> Option<TextBoxAnchor> {
    Some(match value {
        "top" | "top-center" | "top-baseline" | "top-center-baseline" => TextBoxAnchor::Top,
        "middle" | "middle-center" => TextBoxAnchor::Center,
        "bottom" | "bottom-center" | "bottom-baseline" | "bottom-center-baseline" => {
            TextBoxAnchor::Bottom
        }
        _ => return None,
    })
}

fn grows_to_fit(
    shape: Element<'_>,
    vml: Option<Element<'_>>,
    names: &Names,
    report: Report<'_>,
) -> bool {
    match vml {
        Some(text_box) => text_box_style(text_box)
            .read("mso-fit-shape-to-text", vml::true_false, report)
            .unwrap_or(false),
        None => body(shape, names)
            .is_some_and(|body| child(body, names, &names.a, "spAutoFit").is_some()),
    }
}

fn clips_overflow(
    shape: Element<'_>,
    vml: Option<Element<'_>>,
    names: &Names,
    report: Report<'_>,
) -> bool {
    if vml.is_some() {
        return false;
    }
    body(shape, names)
        .and_then(|body| {
            value(
                body,
                names,
                "vertOverflow",
                |value| match value {
                    "overflow" => Some(false),
                    "ellipsis" | "clip" => Some(true),
                    _ => None,
                },
                report,
            )
        })
        .unwrap_or(false)
}

fn rotation(
    shape: Element<'_>,
    vml: Option<Element<'_>>,
    names: &Names,
    report: Report<'_>,
) -> f64 {
    if vml.is_some() {
        return vml::shape_style(shape)
            .read("rotation", vml::angle, report)
            .unwrap_or(0.0);
    }
    child(shape, names, &[names.wps], "spPr")
        .and_then(|properties| child(properties, names, &names.a, "xfrm"))
        .and_then(|xfrm| {
            value(
                xfrm,
                names,
                "rot",
                |value| value.parse::<i32>().ok(),
                report,
            )
        })
        .map_or(0.0, |value| f64::from(value) / 60_000.0)
}

fn text_direction(
    shape: Element<'_>,
    vml: Option<Element<'_>>,
    names: &Names,
    report: Report<'_>,
) -> TextBoxDirection {
    let Some(text_box) = vml else {
        return body(shape, names)
            .and_then(|body| value(body, names, "vert", TextBoxDirection::from_xml, report))
            .unwrap_or(TextBoxDirection::Horizontal);
    };
    // Word writes `vert270` as `vertical` read bottom to top, `vert` and `mongolianVert`
    // as `vertical` (the latter top to bottom), and `eaVert` as `vertical-ideographic`,
    // as the VML fallbacks of the corpora show.
    let style = text_box_style(text_box);
    let upward = style.read(
        "mso-layout-flow-alt",
        |value| match value {
            "bottom-to-top" => Some(true),
            "top-to-bottom" => Some(false),
            _ => None,
        },
        report,
    );
    style
        .read(
            "layout-flow",
            |value| match value {
                "horizontal" | "horizontal-ideographic" => Some(TextBoxDirection::Horizontal),
                "vertical" if upward == Some(true) => Some(TextBoxDirection::Vertical270),
                "vertical" => Some(TextBoxDirection::Vertical),
                "vertical-ideographic" => Some(TextBoxDirection::EastAsianVertical),
                _ => None,
            },
            report,
        )
        .unwrap_or(TextBoxDirection::Horizontal)
}

fn is_word_art(
    shape: Element<'_>,
    vml: Option<Element<'_>>,
    names: &Names,
    report: Report<'_>,
) -> bool {
    if vml.is_some() {
        return false;
    }
    let Some(body) = body(shape, names) else {
        return false;
    };
    let from_word_art = value(body, names, "fromWordArt", boolean, report).unwrap_or(false);
    let warped = child(body, names, &names.a, "prstTxWarp")
        .and_then(|warp| warp.attribute(&[None], "prst"))
        .is_some_and(|preset| preset != "textNoShape");
    from_word_art || warped
}

fn columns(shape: Element<'_>, vml: Option<Element<'_>>, names: &Names, report: Report<'_>) -> u32 {
    if vml.is_some() {
        return 1;
    }
    body(shape, names)
        .and_then(|body| {
            value(
                body,
                names,
                "numCol",
                // ST_TextColumnCount.
                |value| {
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|count| (1..=16).contains(count))
                },
                report,
            )
        })
        .unwrap_or(1)
}

fn is_linked(shape: Element<'_>, vml: Option<Element<'_>>, names: &Names) -> bool {
    match vml {
        // A box that names itself as the next is in no chain, as Word writes some
        // (business/report-hhs-usability-test-report.docx).
        Some(text_box) => text_box_style(text_box)
            .read("mso-next-textbox", Some, &mut |_, _, _| {})
            .map(|next| next.strip_prefix('#').unwrap_or(next))
            .is_some_and(|next| {
                !next.is_empty()
                    && shape.attribute(&[None], "id") != Some(next)
                    && shape.attribute(&names.o, "spid") != Some(next)
            }),
        None => {
            let wps = [names.wps];
            child(shape, names, &wps, "linkedTxbx").is_some()
                || child(shape, names, &wps, "txbx")
                    .is_some_and(|text_box| text_box.attribute(&[None], "id").is_some())
        }
    }
}
