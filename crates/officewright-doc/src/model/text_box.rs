//! Text boxes: shapes whose content (`w:txbxContent`, ECMA-376 Part 1, §17.17.1.1) is a
//! story of its own, in a drawing (`wps:wsp` with `wps:txbx`, [MS-ODRAWXML] 2.3.3) or in
//! VML (a shape with a `v:textbox`, Part 4, §19.1.2.22). The content is read into the
//! model with the document; the properties of the box around it are read from the kept
//! XML when asked for.

use officewright_core::Pt;

use super::color::{Paint, VmlColor, drawing_color, vml_color};
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
    /// The `w:drawing` or the `w:pict` it is in.
    holder: Element<'a>,
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
        Self::new(document, drawing, shape, None)
    }

    /// The text boxes of a VML drawing (`w:pict`) whose content was read, those of the
    /// shapes of its groups included, in document order.
    pub(crate) fn in_vml(
        document: &'a Document,
        pict: Element<'a>,
    ) -> impl Iterator<Item = Self> + 'a {
        vml_shapes(pict, &document.names)
            .into_iter()
            .filter_map(move |(shape, text_box)| Self::new(document, pict, shape, Some(text_box)))
    }

    /// The text box of a shape; none when it has content that was not read, as a text
    /// box in another has.
    fn new(
        document: &'a Document,
        holder: Element<'a>,
        shape: Element<'a>,
        vml: Option<Element<'a>>,
    ) -> Option<Self> {
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
            holder,
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

    /// Whether the box asks for text taller than it to be cut at its bottom (`wps:bodyPr`'s
    /// `vertOverflow` `clip` or `ellipsis`), rather than run on past it as `overflow`, the
    /// default, has it.
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

    /// What the box is filled with: its shape's `a:solidFill` or other fill, else the fill
    /// of the theme's format scheme its style names (`a:fillRef`); VML's `v:fill`, or its
    /// shape's `filled` and `fillcolor`, white where not written. None for no fill.
    pub fn fill(self) -> Option<ShapeFill> {
        let document = Some(self.document);
        let names = &self.document.names;
        match self.vml {
            Some(_) => vml_fill(self.shape, self.holder, names, &mut |_, _, _| {}),
            None => drawing_fill(self.shape, names, document, &mut |_, _, _| {}),
        }
    }

    /// The line around the box: its shape's `a:ln`, each of its values else that of the
    /// line of the theme's format scheme its style names (`a:lnRef`); VML's `v:stroke`, or
    /// its shape's `stroked`, `strokecolor` and `strokeweight`, a black line of 0.75 pt
    /// where not written. None for no line.
    pub fn outline(self) -> Option<ShapeOutline> {
        let document = Some(self.document);
        let names = &self.document.names;
        match self.vml {
            Some(_) => vml_outline(self.shape, self.holder, names, &mut |_, _, _| {}),
            None => drawing_outline(self.shape, names, document, &mut |_, _, _| {}),
        }
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

/// What a shape is filled with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeFill {
    kind: FillKind,
    color: Option<(u8, u8, u8)>,
    opacity: f64,
}

impl ShapeFill {
    /// Whether it is one colour, a gradient, a picture or a pattern.
    pub fn kind(&self) -> FillKind {
        self.kind
    }

    /// Its colour, as red, green and blue: for a gradient, that of its first stop; for a
    /// pattern, that of its foreground; none for a picture, and for a colour that names
    /// nothing the document has.
    pub fn color(&self) -> Option<(u8, u8, u8)> {
        self.color
    }

    /// How opaque its colour is, from 0 (clear) to 1 (`a:alpha`, or `v:fill`'s
    /// `opacity`).
    pub fn opacity(&self) -> f64 {
        self.opacity
    }
}

/// What kind of fill a shape has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FillKind {
    /// One colour (`a:solidFill`, VML's `solid`).
    Solid,
    /// Colours that run into each other (`a:gradFill`, VML's `gradient` and
    /// `gradientRadial`).
    Gradient,
    /// A picture (`a:blipFill`, VML's `tile` and `frame`).
    Picture,
    /// A pattern of two colours (`a:pattFill`, VML's `pattern`).
    Pattern,
}

/// The line around a shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeOutline {
    width: Pt,
    color: Option<(u8, u8, u8)>,
    opacity: f64,
    dash: LineDash,
    compound: CompoundLine,
}

impl ShapeOutline {
    /// Its width (`a:ln`'s `w`, or VML's `strokeweight`); 0 for the thinnest line.
    pub fn width(&self) -> Pt {
        self.width
    }

    /// Its colour, as red, green and blue; none for a gradient or a pattern, and for a
    /// colour that names nothing the document has.
    pub fn color(&self) -> Option<(u8, u8, u8)> {
        self.color
    }

    /// How opaque its colour is, from 0 (clear) to 1.
    pub fn opacity(&self) -> f64 {
        self.opacity
    }

    /// Its dashes (`a:prstDash`, or VML's `dashstyle`).
    pub fn dash(&self) -> LineDash {
        self.dash
    }

    /// Whether it is one line or several side by side (`a:ln`'s `cmpd`, or VML's
    /// `linestyle`).
    pub fn compound(&self) -> CompoundLine {
        self.compound
    }
}

/// The dashes of a line (ST_PresetLineDashVal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LineDash {
    /// `solid`: no dashes.
    Solid,
    /// `dot`: dots.
    Dot,
    /// `dash`: dashes.
    Dash,
    /// `lgDash`: long dashes.
    LongDash,
    /// `dashDot`: a dash, then a dot.
    DashDot,
    /// `lgDashDot`: a long dash, then a dot.
    LongDashDot,
    /// `lgDashDotDot`: a long dash, then two dots.
    LongDashDotDot,
    /// `sysDash`: short dashes, as wide as the line.
    SystemDash,
    /// `sysDot`: short dots, as wide as the line.
    SystemDot,
    /// `sysDashDot`: a short dash, then a short dot.
    SystemDashDot,
    /// `sysDashDotDot`: a short dash, then two short dots.
    SystemDashDotDot,
    /// Dashes of the lengths the document gives (`a:custDash`, or numbers in VML's
    /// `dashstyle`).
    Custom,
}

impl LineDash {
    fn from_xml(value: &str) -> Option<Self> {
        Some(match value {
            "solid" => Self::Solid,
            "dot" => Self::Dot,
            "dash" => Self::Dash,
            "lgDash" => Self::LongDash,
            "dashDot" => Self::DashDot,
            "lgDashDot" => Self::LongDashDot,
            "lgDashDotDot" => Self::LongDashDotDot,
            "sysDash" => Self::SystemDash,
            "sysDot" => Self::SystemDot,
            "sysDashDot" => Self::SystemDashDot,
            "sysDashDotDot" => Self::SystemDashDotDot,
            _ => return None,
        })
    }

    /// VML's `dashstyle` (ECMA-376 Part 4, §19.1.2.21), whose names stand for those of
    /// DrawingML: `short` for `sys`, `long` for `lg`. Numbers give dashes of their own.
    fn from_vml(value: &str) -> Option<Self> {
        Some(match value {
            "solid" => Self::Solid,
            "shortdash" => Self::SystemDash,
            "shortdot" => Self::SystemDot,
            "shortdashdot" => Self::SystemDashDot,
            "shortdashdotdot" => Self::SystemDashDotDot,
            "dot" => Self::Dot,
            "dash" => Self::Dash,
            "longdash" => Self::LongDash,
            "dashdot" => Self::DashDot,
            "longdashdot" => Self::LongDashDot,
            "longdashdotdot" => Self::LongDashDotDot,
            custom
                if !custom.trim().is_empty()
                    && custom
                        .split_ascii_whitespace()
                        .all(|length| length.parse::<f64>().is_ok_and(f64::is_finite)) =>
            {
                Self::Custom
            }
            _ => return None,
        })
    }
}

xml_enum! {
    /// Whether a line is one line or several side by side (ST_CompoundLine).
    CompoundLine {
        Single = "sng",
        Double = "dbl",
        /// A thick line, then a thin one.
        ThickThin = "thickThin",
        /// A thin line, then a thick one.
        ThinThick = "thinThick",
        Triple = "tri",
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
        match vml {
            Some(_) => {
                vml_fill(shape, holder, names, report);
                vml_outline(shape, holder, names, report);
            }
            None => {
                drawing_fill(shape, names, None, report);
                drawing_outline(shape, names, None, report);
            }
        }
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
/// (`v:group`) included, in document order: found in one pass over the drawing, however
/// deep its groups nest.
fn vml_shapes<'a>(pict: Element<'a>, names: &Names) -> Vec<(Element<'a>, Element<'a>)> {
    let group = |namespace, local: &str| local == "group" && names.v.contains(&namespace);
    pict.descendants_through_alternates(names.mc, group)
        .into_iter()
        .filter(|shape| shape.local_in(&names.v).is_some())
        .filter_map(|shape| {
            let text_box = shape
                .children_through_alternates(names.mc)
                .find(|part| part.is(&names.v, "textbox"))?;
            Some((shape, text_box))
        })
        .collect()
}

/// The content of a text box (`w:txbxContent`), if it has one.
fn content<'a>(shape: Element<'a>, vml: Option<Element<'a>>, names: &Names) -> Option<Element<'a>> {
    let holder = match vml {
        Some(text_box) => text_box,
        None => child(shape, names, &[names.wps], "txbx")?,
    };
    child(
        holder,
        names,
        &[names.w[0], names.w[1], names.wne],
        "txbxContent",
    )
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

/// The fills of DrawingML (EG_FillProperties), and those a line takes (EG_LineFillProperties).
const FILLS: [&str; 6] = [
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
];
const LINE_FILLS: [&str; 4] = ["noFill", "solidFill", "gradFill", "pattFill"];

/// The first child of `parent` that is one of these fills.
fn fill_element<'a>(parent: Element<'a>, names: &Names, fills: &[&str]) -> Option<Element<'a>> {
    parent.children_through_alternates(names.mc).find(|child| {
        child
            .local_in(&names.a)
            .is_some_and(|local| fills.contains(&local))
    })
}

/// What a shape's style (`wps:style`) takes from the theme's format scheme by its reference
/// `name` (`a:fillRef`, `a:lnRef`, ST_StyleMatrixColumnIndex): the entry of the list
/// `lists.0` it names from 1 on, or of `lists.1` from 1001 on, and the colour of the
/// reference, which the entry's placeholder colour stands for. None for 0, no fill or
/// line, and without `document`, as when the values are checked while it is read.
fn style_reference<'a>(
    shape: Element<'a>,
    names: &Names,
    document: Option<&'a Document>,
    name: &str,
    lists: (&str, Option<&str>),
    report: Report<'_>,
) -> Option<(Element<'a>, Option<Paint>)> {
    let style = child(shape, names, &[names.wps], "style")?;
    let reference = child(style, names, &names.a, name)?;
    let index = value(reference, names, "idx", |v| v.parse::<u32>().ok(), report)?;
    let placeholder = drawing_color(reference, names, document, None, report);
    let (list, at) = match (index, lists.1) {
        (0, _) => return None,
        (1001.., Some(background)) => (background, index - 1001),
        _ => (lists.0, index - 1),
    };
    let document = document?;
    let scheme = document.element(document.theme.as_ref()?.format_scheme?);
    let entry = child(scheme, names, &names.a, list)?
        .children_through_alternates(names.mc)
        .filter(|entry| entry.local_in(&names.a).is_some())
        .nth(usize::try_from(at).ok()?)?;
    Some((entry, placeholder))
}

fn drawing_fill(
    shape: Element<'_>,
    names: &Names,
    document: Option<&Document>,
    report: Report<'_>,
) -> Option<ShapeFill> {
    let own = child(shape, names, &[names.wps], "spPr")
        .and_then(|properties| fill_element(properties, names, &FILLS));
    let (fill, placeholder) = match own {
        Some(fill) => (fill, None),
        None => style_reference(
            shape,
            names,
            document,
            "fillRef",
            ("fillStyleLst", Some("bgFillStyleLst")),
            report,
        )?,
    };
    let (kind, colored) = match fill.local_name() {
        "solidFill" => (FillKind::Solid, Some(fill)),
        "gradFill" => (
            FillKind::Gradient,
            child(fill, names, &names.a, "gsLst")
                .and_then(|stops| child(stops, names, &names.a, "gs")),
        ),
        "pattFill" => (FillKind::Pattern, child(fill, names, &names.a, "fgClr")),
        "blipFill" => (FillKind::Picture, None),
        // No fill, and a group's fill in a shape of no group read.
        _ => return None,
    };
    let paint =
        colored.and_then(|element| drawing_color(element, names, document, placeholder, report));
    Some(ShapeFill {
        kind,
        color: paint.map(|paint| paint.rgb),
        opacity: paint.map_or(1.0, |paint| paint.opacity),
    })
}

fn drawing_outline(
    shape: Element<'_>,
    names: &Names,
    document: Option<&Document>,
    report: Report<'_>,
) -> Option<ShapeOutline> {
    let own = child(shape, names, &[names.wps], "spPr")
        .and_then(|properties| child(properties, names, &names.a, "ln"));
    let style = style_reference(
        shape,
        names,
        document,
        "lnRef",
        ("lnStyleLst", None),
        report,
    );
    let (mut width, mut compound, mut dash, mut fill) = (None, None, None, None);
    // Each value from the shape's own line, else from its style's.
    for (line, placeholder) in [own.map(|line| (line, None)), style].into_iter().flatten() {
        if width.is_none() {
            width = value(line, names, "w", line_width, report);
        }
        if compound.is_none() {
            compound = value(line, names, "cmpd", CompoundLine::from_xml, report);
        }
        if dash.is_none() {
            dash = match child(line, names, &names.a, "prstDash") {
                Some(preset) => value(preset, names, "val", LineDash::from_xml, report),
                None => child(line, names, &names.a, "custDash").map(|_| LineDash::Custom),
            };
        }
        if fill.is_none() {
            fill = fill_element(line, names, &LINE_FILLS).map(|fill| (fill, placeholder));
        }
    }
    let (fill, placeholder) = fill?;
    let paint = match fill.local_name() {
        "noFill" => return None,
        "solidFill" => drawing_color(fill, names, document, placeholder, report),
        _ => None,
    };
    Some(ShapeOutline {
        width: width.unwrap_or(Pt(0.0)),
        color: paint.map(|paint| paint.rgb),
        opacity: paint.map_or(1.0, |paint| paint.opacity),
        dash: dash.unwrap_or(LineDash::Solid),
        compound: compound.unwrap_or(CompoundLine::Single),
    })
}

/// The width of a line of DrawingML (ST_LineWidth): EMUs from 0 to 20,116,800.
fn line_width(value: &str) -> Option<Pt> {
    let emus = value
        .parse::<u32>()
        .ok()
        .filter(|emus| *emus <= 20_116_800)?;
    Some(Pt(f64::from(emus) / 12_700.0))
}

/// The width of a VML line: a length, or a number of EMUs without a unit, from 0 to
/// 20,116,800 EMUs, the bounds Office gives it in EMUs (MS-OI29500 2.1.1810(f)).
fn stroke_weight(value: &str) -> Option<Pt> {
    let width = match value.parse::<u32>() {
        Ok(emus) => Pt(f64::from(emus) / 12_700.0),
        Err(_) => vml::size(value)?,
    };
    (width.0 <= 20_116_800.0 / 12_700.0).then_some(width)
}

/// A VML shape's first child of this local name.
fn vml_child<'a>(shape: Element<'a>, names: &Names, local: &str) -> Option<Element<'a>> {
    shape
        .children_through_alternates(names.mc)
        .find(|child| child.is(&names.v, local))
}

/// The shape type (`v:shapetype`) a VML shape's `type` names in its drawing, whose values
/// stand for those the shape does not write.
fn shape_type<'a>(shape: Element<'a>, pict: Element<'a>, names: &Names) -> Option<Element<'a>> {
    let id = shape.attribute(&[None], "type")?;
    let id = id.strip_prefix('#').unwrap_or(id);
    pict.children_through_alternates(names.mc)
        .filter(|child| child.is(&names.v, "shapetype"))
        .find(|child| child.attribute(&[None], "id") == Some(id))
}

/// A value of a VML shape: from its child (`v:fill`, `v:stroke`) as `child_name`, else the
/// shape's `name`, else its shape type's `name`. The child's and the shape's are read
/// whether or not they are used, so that each not taken is reported.
fn vml_value<T>(
    (part, child_name): (Option<Element<'_>>, &str),
    shape: Element<'_>,
    pict: Element<'_>,
    names: &Names,
    name: &str,
    read: impl Fn(&str) -> Option<T>,
    report: Report<'_>,
) -> Option<T> {
    let from_part = part.and_then(|part| value(part, names, child_name, &read, report));
    let from_shape = value(shape, names, name, &read, report);
    from_part.or(from_shape).or_else(|| {
        shape_type(shape, pict, names)
            .and_then(|shape_type| value(shape_type, names, name, &read, report))
    })
}

fn vml_fill(
    shape: Element<'_>,
    pict: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> Option<ShapeFill> {
    let fill = vml_child(shape, names, "fill");
    let on = vml_value(
        (fill, "on"),
        shape,
        pict,
        names,
        "filled",
        vml::true_false,
        report,
    );
    let color = vml_value(
        (fill, "color"),
        shape,
        pict,
        names,
        "fillcolor",
        vml_color,
        report,
    );
    let opacity = fill.and_then(|fill| value(fill, names, "opacity", vml::fraction, report));
    let kind = fill.and_then(|fill| {
        value(
            fill,
            names,
            "type",
            |kind| match kind {
                "solid" => Some(FillKind::Solid),
                "gradient" | "gradientRadial" => Some(FillKind::Gradient),
                "tile" | "frame" => Some(FillKind::Picture),
                "pattern" => Some(FillKind::Pattern),
                _ => None,
            },
            report,
        )
    });
    let color = color.unwrap_or(VmlColor::Rgb((0xFF, 0xFF, 0xFF)));
    if !on.unwrap_or(true) || color == VmlColor::None {
        return None;
    }
    let kind = kind.unwrap_or(FillKind::Solid);
    let rgb = match color {
        VmlColor::Rgb(rgb) => Some(rgb),
        VmlColor::None | VmlColor::Elsewhere => None,
    };
    Some(ShapeFill {
        kind,
        color: rgb.filter(|_| kind != FillKind::Picture),
        opacity: opacity.unwrap_or(1.0).clamp(0.0, 1.0),
    })
}

fn vml_outline(
    shape: Element<'_>,
    pict: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> Option<ShapeOutline> {
    let stroke = vml_child(shape, names, "stroke");
    let on = vml_value(
        (stroke, "on"),
        shape,
        pict,
        names,
        "stroked",
        vml::true_false,
        report,
    );
    let color = vml_value(
        (stroke, "color"),
        shape,
        pict,
        names,
        "strokecolor",
        vml_color,
        report,
    );
    let width = vml_value(
        (stroke, "weight"),
        shape,
        pict,
        names,
        "strokeweight",
        stroke_weight,
        report,
    );
    let opacity = stroke.and_then(|stroke| value(stroke, names, "opacity", vml::fraction, report));
    let dash =
        stroke.and_then(|stroke| value(stroke, names, "dashstyle", LineDash::from_vml, report));
    let compound = stroke.and_then(|stroke| {
        value(
            stroke,
            names,
            "linestyle",
            |style| match style {
                "single" => Some(CompoundLine::Single),
                "thinThin" => Some(CompoundLine::Double),
                "thinThick" => Some(CompoundLine::ThinThick),
                "thickThin" => Some(CompoundLine::ThickThin),
                "thickBetweenThin" => Some(CompoundLine::Triple),
                _ => None,
            },
            report,
        )
    });
    let color = color.unwrap_or(VmlColor::Rgb((0, 0, 0)));
    if !on.unwrap_or(true) || color == VmlColor::None {
        return None;
    }
    Some(ShapeOutline {
        // Office's default, where the standard's is 1 pt (MS-OI29500 2.1.1810(h)).
        width: width.unwrap_or(Pt(0.75)),
        color: match color {
            VmlColor::Rgb(rgb) => Some(rgb),
            VmlColor::None | VmlColor::Elsewhere => None,
        },
        opacity: opacity.unwrap_or(1.0).clamp(0.0, 1.0),
        dash: dash.unwrap_or(LineDash::Solid),
        compound: compound.unwrap_or(CompoundLine::Single),
    })
}
