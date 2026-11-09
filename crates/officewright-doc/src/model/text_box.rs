//! Text boxes: shapes whose content (`w:txbxContent`, ECMA-376 Part 1, §17.17.1.1) is a
//! story of its own, in a drawing (`wps:wsp` with `wps:txbx`, [MS-ODRAWXML] 2.3.3) or in
//! VML (a shape with a `v:textbox`, Part 4, §19.1.2.22). The content is read into the
//! model with the document; the properties of the box around it are read from the kept
//! XML when asked for.

use officewright_core::Pt;

use super::color::{
    Paint, VmlColor, color_element, color_of_element, drawing_color, read_color, resolve, vml_color,
};
use super::drawing::{self, Anchor, Offsets, attribute, boolean, child, coordinate, value};
use super::properties::xml_enum;
use super::theme::{StyleFill, StyleLine};
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
    /// Where the shape is: in a drawing, its frame; in VML, its `v:textbox`.
    place: Place<'a>,
    blocks: &'a [u32],
}

/// Where the shape of a text box is.
#[derive(Clone, Copy, Debug)]
enum Place<'a> {
    /// In a drawing (`w:drawing`), with its frame (`wp:inline` or `wp:anchor`).
    Drawing(Option<Element<'a>>),
    /// In VML, with its `v:textbox`, and whether it is in a group (`v:group`).
    Vml(Element<'a>, bool),
}

impl<'a> TextBox<'a> {
    /// The text box of a drawing (`w:drawing`), if its shape has one whose content was
    /// read.
    pub(crate) fn in_drawing(document: &'a Document, drawing: Element<'a>) -> Option<Self> {
        let names = &document.names;
        let shape = drawing_shape(drawing, names)?;
        Self::new(
            document,
            shape,
            Place::Drawing(drawing::frame(drawing, names)),
        )
    }

    /// The text boxes of a VML drawing (`w:pict`) whose content was read, those of the
    /// shapes of its groups included, in document order.
    pub(crate) fn in_vml(
        document: &'a Document,
        pict: Element<'a>,
    ) -> impl Iterator<Item = Self> + 'a {
        vml_shapes(pict, &document.names).into_iter().filter_map(
            move |(shape, text_box, in_group)| {
                Self::new(document, shape, Place::Vml(text_box, in_group))
            },
        )
    }

    /// The text box of a shape; none when it has content that was not read, as a text
    /// box in another has.
    fn new(document: &'a Document, shape: Element<'a>, place: Place<'a>) -> Option<Self> {
        let vml = match place {
            Place::Drawing(_) => None,
            Place::Vml(text_box, _) => Some(text_box),
        };
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
            place,
            blocks,
        })
    }

    /// For a VML shape, its `v:textbox`.
    fn vml(self) -> Option<Element<'a>> {
        match self.place {
            Place::Drawing(_) => None,
            Place::Vml(text_box, _) => Some(text_box),
        }
    }

    /// The width of its frame: that of its drawing (`wp:extent`'s `cx`), or the `width` in
    /// the `style` of its VML shape; none when not written or not a length Word takes, and
    /// for a VML shape in a group (`v:group`), which gives it in the group's own units.
    pub fn width(self) -> Option<Pt> {
        self.side("cx", "width")
    }

    /// The height of its frame (`wp:extent`'s `cy`, or `height`), as
    /// [`width`](Self::width) says.
    pub fn height(self) -> Option<Pt> {
        self.side("cy", "height")
    }

    fn side(self, extent: &str, style: &str) -> Option<Pt> {
        let names = &self.document.names;
        match self.place {
            Place::Drawing(frame) => drawing::extent(frame?, names, extent, &mut |_, _, _| {}),
            Place::Vml(_, false) => {
                vml::shape_style(self.shape).read(style, vml::size, &mut |_, _, _| {})
            }
            Place::Vml(_, true) => None,
        }
    }

    /// Where a floating box is and how text wraps around it: its drawing's `wp:anchor`, or
    /// the `style` of its VML shape when it says `position:absolute`, read as
    /// [`VmlPicture::anchor`](super::VmlPicture::anchor) reads a picture's. None for a box
    /// in the line, and for a VML shape in a group, which the group places.
    pub fn anchor(self) -> Option<Anchor> {
        let names = &self.document.names;
        match self.place {
            Place::Drawing(frame) => {
                let frame = frame.filter(|frame| frame.is(&names.wp, "anchor"))?;
                Some(drawing::anchor(frame, names, &mut |_, _, _| {}))
            }
            Place::Vml(_, false) => vml::anchor(self.shape, names, &mut |_, _, _| {}),
            Place::Vml(_, true) => None,
        }
    }

    /// Whether it is a VML shape in a group (`v:group`), which places and sizes the shapes
    /// in it in units of its own: such a box has no [`width`](Self::width),
    /// [`height`](Self::height) or [`anchor`](Self::anchor) of its own. A box in a group
    /// of a drawing (`wpg:wgp`) is not read as a text box.
    pub fn is_in_group(self) -> bool {
        matches!(self.place, Place::Vml(_, true))
    }

    /// Whether it is hidden (`visibility:hidden` in the `style` of its VML shape), which
    /// Word does not show.
    pub fn is_hidden(self) -> bool {
        matches!(self.place, Place::Vml(..)) && vml::is_hidden(self.shape)
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
            self.vml(),
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Where its text is from top to bottom (`wps:bodyPr`'s `anchor`, or `v-text-anchor`
    /// in the `style` of the `v:textbox` or of its shape); at the top where not written.
    pub fn vertical_anchor(self) -> TextBoxAnchor {
        vertical_anchor(
            self.shape,
            self.vml(),
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Whether the box grows to the height of its text (`a:spAutoFit`, or
    /// `mso-fit-shape-to-text` in the `style` of the `v:textbox`).
    pub fn grows_to_fit(self) -> bool {
        grows_to_fit(
            self.shape,
            self.vml(),
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
            self.vml(),
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// How far the box is turned clockwise, in degrees (`a:xfrm`'s `rot`, or `rotation`
    /// in the `style` of its VML shape); 0 where not written.
    pub fn rotation(self) -> f64 {
        rotation(
            self.shape,
            self.vml(),
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Which way its text runs (`wps:bodyPr`'s `vert`, or `layout-flow` and
    /// `mso-layout-flow-alt` in the `style` of the `v:textbox`).
    pub fn text_direction(self) -> TextBoxDirection {
        text_direction(
            self.shape,
            self.vml(),
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Whether it is WordArt: its text drawn as a shape (`wps:bodyPr`'s `fromWordArt`,
    /// or a warp of its text, `a:prstTxWarp`, other than `textNoShape`).
    pub fn is_word_art(self) -> bool {
        is_word_art(
            self.shape,
            self.vml(),
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// How many columns its text is in (`wps:bodyPr`'s `numCol`); 1 where not written.
    pub fn columns(self) -> u32 {
        columns(
            self.shape,
            self.vml(),
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
        match self.vml() {
            Some(_) => vml_fill(self.shape, self.shape_type(), names, &mut |_, _, _| {}),
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
        match self.vml() {
            Some(_) => vml_outline(self.shape, self.shape_type(), names, &mut |_, _, _| {}),
            None => drawing_outline(self.shape, names, document, &mut |_, _, _| {}),
        }
    }

    /// The values of the shape type its VML shape names (`type`), as the document
    /// registers shape types while it is read: the first of each id.
    fn shape_type(self) -> Option<ShapeTypeValues> {
        let id = self.shape.attribute(&[None], "type")?;
        let id = id.strip_prefix('#').unwrap_or(id);
        self.document.vml_shape_types.get(id).copied()
    }

    /// Whether it is in a chain of linked boxes, whose text runs on from one box to the
    /// next: the first (`wps:txbx`'s `id`, or `mso-next-textbox` in the `style` of the
    /// `v:textbox` naming another shape) or a later one (`wps:linkedTxbx`).
    pub fn is_linked(self) -> bool {
        is_linked(self.shape, self.vml(), &self.document.names)
    }

    /// Whether it is drawn other than as a rectangle: in a drawing, a shape of an
    /// `a:prstGeom` other than `rect`, such as a rounded rectangle or a callout, or of an
    /// `a:custGeom`; in VML, a shape other than `v:rect`, such as `v:roundrect` or
    /// `v:oval`, or a `v:shape` whose shape type (its `o:spt`, else that of the
    /// `v:shapetype` its `type` names) is known and neither a rectangle (1) nor a text box
    /// (202). The layout draws a box as a rectangle.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn is_shaped(self) -> bool {
        let names = &self.document.names;
        match self.vml() {
            Some(_) => vml_shaped(self.shape, self.shape_type(), names, &mut |_, _, _| {}),
            None => drawing_shaped(self.shape, names),
        }
    }

    /// Whether it has a shadow, a reflection, a glow, soft edges or a 3-D effect: in its
    /// `wps:spPr`, as a picture has them in its `pic:spPr`, or, in VML, a `v:shadow` that is
    /// on. The effects of the theme's format scheme its style names (`a:effectRef`) are
    /// not read. The layout does not draw them.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_effects(self) -> bool {
        let names = &self.document.names;
        match self.vml() {
            Some(_) => vml::has_shadow(self.shape, names, &mut |_, _, _| {}),
            None => drawing_effects(self.shape, names, &mut |_, _, _| {}),
        }
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
    alignment: LineAlignment,
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

    /// Where it is drawn: centred on the shape's edge, or inside it (`a:ln`'s `algn`;
    /// a VML line is centred).
    pub fn alignment(&self) -> LineAlignment {
        self.alignment
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
    /// `dashstyle` that are no preset's pattern).
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
    /// DrawingML: `short` for `sys`, `long` for `lg`. Numbers give dashes of their own. Word
    /// writes the names in either case (`longDashDot` in LibreOffice's
    /// LineStyle_DashType.docx, which Word 16 saved).
    fn from_vml(value: &str) -> Option<Self> {
        Some(match value.to_ascii_lowercase().as_str() {
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
            custom => {
                let lengths = || {
                    custom
                        .split_ascii_whitespace()
                        .map(|length| length.parse::<f64>().ok().filter(|l| l.is_finite()))
                };
                if custom.trim().is_empty() || lengths().any(|length| length.is_none()) {
                    return None;
                }
                // Lengths that are a preset's pattern name that preset, as Word writes
                // `sysDot` and `sysDash` (`1 1`, `3 1`).
                Self::PATTERNS
                    .iter()
                    .find(|(pattern, _)| lengths().flatten().eq(pattern.iter().copied()))
                    .map_or(Self::Custom, |&(_, dash)| dash)
            }
        })
    }

    /// The lengths of each preset's dashes and gaps, in widths of the line (ECMA-376 Part 1,
    /// §20.1.10.48).
    const PATTERNS: [(&[f64], Self); 10] = [
        (&[1.0, 3.0], Self::Dot),
        (&[4.0, 3.0], Self::Dash),
        (&[8.0, 3.0], Self::LongDash),
        (&[4.0, 3.0, 1.0, 3.0], Self::DashDot),
        (&[8.0, 3.0, 1.0, 3.0], Self::LongDashDot),
        (&[8.0, 3.0, 1.0, 3.0, 1.0, 3.0], Self::LongDashDotDot),
        (&[3.0, 1.0], Self::SystemDash),
        (&[1.0, 1.0], Self::SystemDot),
        (&[3.0, 1.0, 1.0, 1.0], Self::SystemDashDot),
        (&[3.0, 1.0, 1.0, 1.0, 1.0, 1.0], Self::SystemDashDotDot),
    ];
}

xml_enum! {
    /// Where a line is drawn on the edge of its shape (ST_PenAlignment).
    LineAlignment {
        /// Centred on the edge.
        Center = "ctr",
        /// Inside the edge.
        Inside = "in",
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
    let drawing = drawing_shape(holder, names).map(|shape| (shape, None, false));
    let shapes = vml_shapes(holder, names)
        .into_iter()
        .map(|(shape, text_box, in_group)| (shape, Some(text_box), in_group));
    // A VML drawing's picture has its frame and shadow read with it (`vml::check`), and a
    // drawing's frame is read with the drawing.
    let picture = vml::picture(holder, names).map(Element::index);
    for (shape, vml, in_group) in drawing.into_iter().chain(shapes) {
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
                vml_fill(shape, None, names, report);
                vml_outline(shape, None, names, report);
                vml_shaped(shape, None, names, report);
                if picture != Some(shape.index()) {
                    vml::has_shadow(shape, names, report);
                    if !in_group {
                        let style = vml::shape_style(shape);
                        style.read("width", vml::size, report);
                        style.read("height", vml::size, report);
                        vml::anchor(shape, names, report);
                    }
                }
            }
            None => {
                drawing_fill(shape, names, None, report);
                drawing_outline(shape, names, None, report);
                drawing_effects(shape, names, report);
            }
        }
    }
}

/// Whether the shape of a text box in a drawing is drawn other than as a rectangle
/// ([`TextBox::is_shaped`]).
fn drawing_shaped(shape: Element<'_>, names: &Names) -> bool {
    child(shape, names, &[names.wps], "spPr").is_some_and(|properties| {
        properties
            .children_through_alternates(names.mc)
            .any(|part| match part.local_in(&names.a) {
                Some("prstGeom") => part.attribute(&[None], "prst") != Some("rect"),
                Some("custGeom") => true,
                _ => false,
            })
    })
}

/// Whether the shape of a text box in a drawing has an effect the layout does not draw
/// ([`TextBox::has_effects`]).
fn drawing_effects(shape: Element<'_>, names: &Names, report: Report<'_>) -> bool {
    child(shape, names, &[names.wps], "spPr")
        .is_some_and(|properties| drawing::shape_effects(properties, names, report))
}

/// The shape types of VML (`o:spt`) drawn as rectangles: a rectangle and a text box.
const RECTANGLES: [f64; 2] = [1.0, 202.0];

/// Whether a VML shape is drawn other than as a rectangle ([`TextBox::is_shaped`]), with
/// `shape_type`, the values of the shape type it names.
fn vml_shaped(
    shape: Element<'_>,
    shape_type: Option<ShapeTypeValues>,
    names: &Names,
    report: Report<'_>,
) -> bool {
    match shape.local_name() {
        "rect" => false,
        "shape" => attribute(shape, names, (&names.o, "o:"), "spt", shape_kind, report)
            .or(shape_type.and_then(|values| values.kind))
            .is_some_and(|kind| !RECTANGLES.contains(&kind)),
        _ => true,
    }
}

/// A VML shape type (`o:spt`, an xsd:float).
fn shape_kind(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|kind| kind.is_finite())
}

/// The content (`w:txbxContent`) of each text box of a drawing or a VML drawing.
pub(crate) fn contents<'a>(
    holder: Element<'a>,
    names: &'a Names,
) -> impl Iterator<Item = Element<'a>> + 'a {
    let drawing = drawing_shape(holder, names).map(|shape| (shape, None));
    let shapes = vml_shapes(holder, names)
        .into_iter()
        .map(|(shape, text_box, _)| (shape, Some(text_box)));
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
/// at most one, MS-OI29500, Part 4, §19.1.2.22(j): the first) and whether each is in a
/// group (`v:group`), those of its groups included, in document order: found in one pass
/// over the drawing, however deep its groups nest.
fn vml_shapes<'a>(pict: Element<'a>, names: &Names) -> Vec<(Element<'a>, Element<'a>, bool)> {
    let group = |namespace, local: &str| local == "group" && names.v.contains(&namespace);
    pict.descendants_through_alternates(names.mc, group)
        .into_iter()
        .filter(|(shape, _)| shape.local_in(&names.v).is_some())
        .filter_map(|(shape, in_group)| {
            let text_box = shape
                .children_through_alternates(names.mc)
                .find(|part| part.is(&names.v, "textbox"))?;
            Some((shape, text_box, in_group))
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

/// How many transforms a colour of the theme's format scheme has applied at most. Every
/// box whose style names it applies them again, so that a theme of many transforms would
/// cost time in proportion to it for each box; Office writes a few. Those after are left
/// out and reported as the theme is read.
pub(crate) const MAX_SCHEME_TRANSFORMS: usize = 64;

/// The index of a shape's style's reference (`wps:style`'s `a:fillRef` or `a:lnRef`,
/// ST_StyleMatrixColumnIndex), and its colour, which the placeholder colour of the entry it
/// names stands for.
fn reference(
    shape: Element<'_>,
    names: &Names,
    document: Option<&Document>,
    name: &str,
    report: Report<'_>,
) -> Option<(u32, Option<Paint>)> {
    let style = child(shape, names, &[names.wps], "style")?;
    let reference = child(style, names, &names.a, name)?;
    let index = value(reference, names, "idx", |v| v.parse::<u32>().ok(), report)?;
    let placeholder = drawing_color(reference, names, document, None, report);
    Some((index, placeholder))
}

/// The fill of the theme's format scheme that a shape's style names (`a:fillRef`): of
/// `a:fillStyleLst` from 1, of `a:bgFillStyleLst` from 1001; none for 0 and 1000, no fill
/// (§20.1.4.2.10), and without `document`, as when the values are checked while it is
/// read.
fn style_fill<'d>(
    shape: Element<'_>,
    names: &Names,
    document: Option<&'d Document>,
    report: Report<'_>,
) -> Option<(&'d StyleFill, Option<Paint>)> {
    let (index, placeholder) = reference(shape, names, document, "fillRef", report)?;
    let scheme = &document?.theme.as_ref()?.format_scheme;
    let at = |list: &'d [StyleFill], first: u32| {
        list.get(usize::try_from(index.checked_sub(first)?).ok()?)
    };
    let entry = match index {
        0 | 1000 => return None,
        1001.. => at(&scheme.background_fills, 1001),
        _ => at(&scheme.fills, 1),
    }?;
    Some((entry, placeholder))
}

/// The line of the theme's format scheme that a shape's style names (`a:lnRef`): of
/// `a:lnStyleLst` from 1; none for 0, and without `document`.
fn style_line<'d>(
    shape: Element<'_>,
    names: &Names,
    document: Option<&'d Document>,
    report: Report<'_>,
) -> Option<(&'d StyleLine, Option<Paint>)> {
    let (index, placeholder) = reference(shape, names, document, "lnRef", report)?;
    let scheme = &document?.theme.as_ref()?.format_scheme;
    let entry = scheme
        .lines
        .get(usize::try_from(index.checked_sub(1)?).ok()?)?;
    Some((entry, placeholder))
}

/// The kind of a fill element (`a:solidFill` and the others) and its colour element: that of
/// an `a:solidFill`, of a gradient's first stop, or of a pattern's foreground. None for no
/// fill, and for a group's fill in a shape of no group read.
fn fill_parts<'a>(fill: Element<'a>, names: &Names) -> Option<(FillKind, Option<Element<'a>>)> {
    let (kind, colored) = match fill.local_name() {
        "solidFill" => (FillKind::Solid, Some(fill)),
        "gradFill" => (
            FillKind::Gradient,
            child(fill, names, &names.a, "gsLst")
                .and_then(|stops| child(stops, names, &names.a, "gs")),
        ),
        "pattFill" => (FillKind::Pattern, child(fill, names, &names.a, "fgClr")),
        "blipFill" => (FillKind::Picture, None),
        _ => return None,
    };
    Some((
        kind,
        colored.and_then(|parent| color_element(parent, names)),
    ))
}

/// A fill of this kind and colour.
fn shape_fill(kind: FillKind, paint: Option<Paint>) -> ShapeFill {
    ShapeFill {
        kind,
        color: paint.map(|paint| paint.rgb),
        opacity: paint.map_or(1.0, |paint| paint.opacity),
    }
}

/// An entry of a theme's fills (`a:fillStyleLst`, `a:bgFillStyleLst`) as the theme is
/// read: its values not taken are reported, and whether its colour has more transforms
/// than [`MAX_SCHEME_TRANSFORMS`] is given.
pub(crate) fn style_fill_entry(
    entry: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> (StyleFill, bool) {
    let Some((kind, color)) = fill_parts(entry, names) else {
        return (
            StyleFill {
                kind: None,
                color: None,
            },
            false,
        );
    };
    let (color, over) = match color {
        Some(color) => read_color(color, names, MAX_SCHEME_TRANSFORMS, report),
        None => (None, false),
    };
    let fill = StyleFill {
        kind: Some(kind),
        color,
    };
    (fill, over)
}

/// An entry of a theme's lines (`a:lnStyleLst`, an `a:ln`) as the theme is read, as
/// [`style_fill_entry`] reads a fill.
pub(crate) fn style_line_entry(
    entry: Element<'_>,
    names: &Names,
    report: Report<'_>,
) -> (StyleLine, bool) {
    let width = value(entry, names, "w", line_width_emus, report);
    let compound = value(entry, names, "cmpd", CompoundLine::from_xml, report);
    let dash = line_dash(entry, names, report);
    let alignment = value(entry, names, "algn", LineAlignment::from_xml, report);
    let (fill, over) = match fill_element(entry, names, &LINE_FILLS) {
        Some(fill) => {
            let (fill, over) = style_fill_entry(fill, names, report);
            (Some(fill), over)
        }
        None => (None, false),
    };
    let line = StyleLine {
        width,
        compound,
        dash,
        alignment,
        fill,
    };
    (line, over)
}

fn drawing_fill(
    shape: Element<'_>,
    names: &Names,
    document: Option<&Document>,
    report: Report<'_>,
) -> Option<ShapeFill> {
    let own = child(shape, names, &[names.wps], "spPr")
        .and_then(|properties| fill_element(properties, names, &FILLS));
    match own {
        Some(fill) => {
            let (kind, color) = fill_parts(fill, names)?;
            let paint =
                color.and_then(|color| color_of_element(color, names, document, None, report));
            Some(shape_fill(kind, paint))
        }
        None => {
            let (entry, placeholder) = style_fill(shape, names, document, report)?;
            let paint = entry
                .color
                .as_ref()
                .and_then(|color| resolve(color, document, placeholder));
            Some(shape_fill(entry.kind?, paint))
        }
    }
}

fn drawing_outline(
    shape: Element<'_>,
    names: &Names,
    document: Option<&Document>,
    report: Report<'_>,
) -> Option<ShapeOutline> {
    let own = child(shape, names, &[names.wps], "spPr")
        .and_then(|properties| child(properties, names, &names.a, "ln"));
    let style = style_line(shape, names, document, report);
    // Each value from the shape's own line, else from its style's.
    let own_width = own.and_then(|line| value(line, names, "w", line_width_emus, report));
    let own_compound =
        own.and_then(|line| value(line, names, "cmpd", CompoundLine::from_xml, report));
    let own_dash = own.and_then(|line| line_dash(line, names, report));
    let own_alignment =
        own.and_then(|line| value(line, names, "algn", LineAlignment::from_xml, report));
    let width = own_width.or(style.and_then(|(line, _)| line.width));
    let compound = own_compound.or(style.and_then(|(line, _)| line.compound));
    let dash = own_dash.or(style.and_then(|(line, _)| line.dash));
    let alignment = own_alignment.or(style.and_then(|(line, _)| line.alignment));
    let paint = match own.and_then(|line| fill_element(line, names, &LINE_FILLS)) {
        Some(fill) => match fill.local_name() {
            "noFill" => return None,
            "solidFill" => drawing_color(fill, names, document, None, report),
            _ => None,
        },
        None => {
            let (line, placeholder) = style?;
            let fill = line.fill.as_ref()?;
            match fill.kind? {
                FillKind::Solid => fill
                    .color
                    .as_ref()
                    .and_then(|color| resolve(color, document, placeholder)),
                _ => None,
            }
        }
    };
    Some(ShapeOutline {
        width: width.map_or(Pt(0.0), |emus| Pt(f64::from(emus) / 12_700.0)),
        color: paint.map(|paint| paint.rgb),
        opacity: paint.map_or(1.0, |paint| paint.opacity),
        dash: dash.unwrap_or(LineDash::Solid),
        compound: compound.unwrap_or(CompoundLine::Single),
        alignment: alignment.unwrap_or(LineAlignment::Center),
    })
}

/// The dashes of a line: its `a:prstDash`, or dashes of its own (`a:custDash`).
fn line_dash(line: Element<'_>, names: &Names, report: Report<'_>) -> Option<LineDash> {
    match child(line, names, &names.a, "prstDash") {
        Some(preset) => value(preset, names, "val", LineDash::from_xml, report),
        None => child(line, names, &names.a, "custDash").map(|_| LineDash::Custom),
    }
}

/// The width of a line of DrawingML (ST_LineWidth), in EMUs from 0 to 20,116,800.
fn line_width_emus(value: &str) -> Option<u32> {
    value.parse::<u32>().ok().filter(|emus| *emus <= 20_116_800)
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

/// The colour of the first stop of a VML gradient's `colors`: stops separated by `;`, each
/// a position and a colour.
fn first_stop(colors: &str) -> Option<VmlColor> {
    let stop = colors.split(';').next()?.trim();
    let (position, color) = stop.split_once(char::is_whitespace)?;
    vml::fraction(position.trim())?;
    vml_color(color.trim())
}

/// A VML shape's first child of this local name.
fn vml_child<'a>(shape: Element<'a>, names: &Names, local: &str) -> Option<Element<'a>> {
    shape
        .children_through_alternates(names.mc)
        .find(|child| child.is(&names.v, local))
}

/// Reports the values of the shape types (`v:shapetype`) of a VML drawing or embedded
/// object that their shapes take and the readers do not: where each is defined, once
/// however many shapes name it.
pub(crate) fn check_shape_types(holder: Element<'_>, names: &Names, report: Report<'_>) {
    for shape_type in holder
        .children_through_alternates(names.mc)
        .filter(|child| child.is(&names.v, "shapetype"))
    {
        check_shape_type(shape_type, names, report);
    }
}

/// The values of a shape type that its shapes take.
fn check_shape_type(shape_type: Element<'_>, names: &Names, report: Report<'_>) {
    attribute(
        shape_type,
        names,
        (&names.o, "o:"),
        "spt",
        shape_kind,
        report,
    );
    value(shape_type, names, "filled", vml::true_false, report);
    value(shape_type, names, "fillcolor", vml_color, report);
    value(shape_type, names, "stroked", vml::true_false, report);
    value(shape_type, names, "strokecolor", vml_color, report);
    value(shape_type, names, "strokeweight", stroke_weight, report);
}

/// The values a VML shape type (`v:shapetype`) gives the shapes that name it and write
/// none of their own.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ShapeTypeValues {
    /// The shape type (`o:spt`).
    kind: Option<f64>,
    filled: Option<bool>,
    fill_color: Option<VmlColor>,
    stroked: Option<bool>,
    stroke_color: Option<VmlColor>,
    stroke_weight: Option<Pt>,
}

/// The values of a shape type, read once, as the document registers it, so that a shape
/// naming it does not read them again; they are reported where it is defined
/// ([`check_shape_type`]).
pub(crate) fn shape_type_values(shape_type: Element<'_>, names: &Names) -> ShapeTypeValues {
    let quiet = &mut |_: &str, _: &str, _: &str| {};
    ShapeTypeValues {
        kind: attribute(
            shape_type,
            names,
            (&names.o, "o:"),
            "spt",
            shape_kind,
            quiet,
        ),
        filled: value(shape_type, names, "filled", vml::true_false, quiet),
        fill_color: value(shape_type, names, "fillcolor", vml_color, quiet),
        stroked: value(shape_type, names, "stroked", vml::true_false, quiet),
        stroke_color: value(shape_type, names, "strokecolor", vml_color, quiet),
        stroke_weight: value(shape_type, names, "strokeweight", stroke_weight, quiet),
    }
}

/// A value of a VML shape: from its child (`v:fill`, `v:stroke`) as `child_name`, else the
/// shape's `name`, else `inherited`, its shape type's. The child's and the shape's are read
/// whether or not they are used, so that each not taken is reported.
fn vml_value<T>(
    (part, child_name): (Option<Element<'_>>, &str),
    shape: Element<'_>,
    inherited: Option<T>,
    names: &Names,
    name: &str,
    read: impl Fn(&str) -> Option<T>,
    report: Report<'_>,
) -> Option<T> {
    let from_part = part.and_then(|part| value(part, names, child_name, &read, report));
    let from_shape = value(shape, names, name, &read, report);
    from_part.or(from_shape).or(inherited)
}

fn vml_fill(
    shape: Element<'_>,
    shape_type: Option<ShapeTypeValues>,
    names: &Names,
    report: Report<'_>,
) -> Option<ShapeFill> {
    let fill = vml_child(shape, names, "fill");
    let on = vml_value(
        (fill, "on"),
        shape,
        shape_type.and_then(|values| values.filled),
        names,
        "filled",
        vml::true_false,
        report,
    );
    let color = vml_value(
        (fill, "color"),
        shape,
        shape_type.and_then(|values| values.fill_color),
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
    // A gradient's first stop, where it lists its stops (`colors`, `0 #936545;.5 #d49366`).
    let first_stop = fill
        .filter(|_| kind == FillKind::Gradient)
        .and_then(|fill| value(fill, names, "colors", first_stop, report));
    let rgb = match first_stop.unwrap_or(color) {
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
    shape_type: Option<ShapeTypeValues>,
    names: &Names,
    report: Report<'_>,
) -> Option<ShapeOutline> {
    let stroke = vml_child(shape, names, "stroke");
    let on = vml_value(
        (stroke, "on"),
        shape,
        shape_type.and_then(|values| values.stroked),
        names,
        "stroked",
        vml::true_false,
        report,
    );
    let color = vml_value(
        (stroke, "color"),
        shape,
        shape_type.and_then(|values| values.stroke_color),
        names,
        "strokecolor",
        vml_color,
        report,
    );
    let width = vml_value(
        (stroke, "weight"),
        shape,
        shape_type.and_then(|values| values.stroke_weight),
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
        alignment: LineAlignment::Center,
    })
}
