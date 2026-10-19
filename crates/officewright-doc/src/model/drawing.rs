//! Drawings in runs (`w:drawing`, ECMA-376 Part 1, §20.4): their frame, in the line or
//! floating, and the picture they show, read from the kept XML when asked for.

use std::sync::Arc;

use officewright_core::Pt;

use super::properties::xml_enum;
use super::xml::{Element, Span};
use super::{Document, ImageData, Names};

/// EMUs (English Metric Units) to a point.
const EMU_PER_POINT: f64 = 12_700.0;

/// The range of ST_Coordinate, in EMUs, which every length of a drawing is within.
const COORDINATES: std::ops::RangeInclusive<f64> = -27_273_042_329_600.0..=27_273_042_316_900.0;

/// A drawing in a run (`w:drawing`): a frame in the line (`wp:inline`) or floating
/// (`wp:anchor`), and what it shows. Pictures are read; shapes, charts and other graphics
/// show as no picture, and [`to_xml`](Self::to_xml) gives them as written.
///
/// ```
/// # fn main() -> officewright_doc::Result<()> {
/// use officewright_core::Pt;
/// use officewright_doc::{Block, Document, RunContent};
///
/// let document = Document::open("tests/data/picture.docx")?;
/// let mut pictures = Vec::new();
/// for section in document.sections() {
///     for block in section.blocks() {
///         let Block::Paragraph(paragraph) = block else {
///             continue;
///         };
///         for item in paragraph.runs().flat_map(|run| run.content()) {
///             let RunContent::Drawing(drawing) = item else {
///                 continue;
///             };
///             // A picture shows its image; a shape, chart or group is there as XML.
///             let image = drawing.picture().and_then(|picture| picture.image());
///             pictures.push((
///                 drawing.width(),
///                 drawing.height(),
///                 image.map(|image| image.content_type().to_owned()),
///             ));
///         }
///     }
/// }
/// assert_eq!(
///     pictures,
///     [(Some(Pt(72.0)), Some(Pt(36.0)), Some("image/png".to_owned()))]
/// );
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Drawing<'a> {
    document: &'a Document,
    element: Span,
    image: Option<u32>,
}

impl<'a> Drawing<'a> {
    pub(crate) fn new(document: &'a Document, element: Span, image: Option<u32>) -> Self {
        Self {
            document,
            element,
            image,
        }
    }

    /// The drawing as written, `w:drawing` and everything in it, as XML with the
    /// namespace declarations it needs.
    pub fn to_xml(self) -> String {
        self.document.element(self.element).to_xml()
    }

    /// The frame: its `wp:inline` or `wp:anchor`.
    fn frame(self) -> Option<Element<'a>> {
        frame(self.document.element(self.element), &self.document.names)
    }

    /// The children of an element of the drawing, through `mc:AlternateContent`.
    fn children(self, element: Element<'a>) -> impl Iterator<Item = Element<'a>> + 'a {
        element.children_through_alternates(self.document.names.mc)
    }

    /// The first child of `element` of this name in `namespaces`.
    fn child(
        self,
        element: Element<'a>,
        namespaces: &[Option<super::xml::Sym>],
        name: &str,
    ) -> Option<Element<'a>> {
        self.children(element)
            .find(|child| child.is(namespaces, name))
    }

    /// Whether the drawing sits in the line of text (`wp:inline`), not floating.
    pub fn is_inline(self) -> bool {
        self.frame()
            .is_some_and(|frame| frame.is(&self.document.names.wp, "inline"))
    }

    /// The width of its frame (`wp:extent`'s `cx`).
    pub fn width(self) -> Option<Pt> {
        self.extent("cx")
    }

    /// The height of its frame (`wp:extent`'s `cy`).
    pub fn height(self) -> Option<Pt> {
        self.extent("cy")
    }

    fn extent(self, attribute: &str) -> Option<Pt> {
        let extent = self.child(self.frame()?, &self.document.names.wp, "extent")?;
        coordinate(extent.attribute(&[None], attribute)?).filter(|length| length.0 >= 0.0)
    }

    /// How far effects such as shadows reach beyond the frame on each side
    /// (`wp:effectExtent`); 0 on each side not written.
    pub fn effect_extent(self) -> Offsets {
        let element = self
            .frame()
            .and_then(|frame| self.child(frame, &self.document.names.wp, "effectExtent"));
        let side = |name| {
            element
                .and_then(|element| element.attribute(&[None], name))
                .and_then(coordinate)
                .unwrap_or_default()
        };
        Offsets {
            left: side("l"),
            top: side("t"),
            right: side("r"),
            bottom: side("b"),
        }
    }

    /// The picture it shows (`pic:pic` in its `a:graphicData`), if it shows one; a
    /// group, a shape or a chart shows none, even when a picture is among its parts.
    pub fn picture(self) -> Option<Picture<'a>> {
        let picture = picture(self.document.element(self.element), &self.document.names)?;
        Some(Picture {
            document: self.document,
            element: picture,
            image: self.image,
        })
    }

    /// Where a floating drawing is and how text wraps around it; none for a drawing in the
    /// line.
    pub fn anchor(self) -> Option<Anchor> {
        let names = &self.document.names;
        let frame = self.frame().filter(|frame| frame.is(&names.wp, "anchor"))?;
        let flag = |name| frame.attribute(&[None], name).and_then(boolean);
        // ST_WrapDistance is unsigned: a negative one is not one.
        let distance = |name| {
            frame
                .attribute(&[None], name)
                .and_then(coordinate)
                .filter(|length| length.0 >= 0.0)
                .unwrap_or_default()
        };
        let mut anchor = Anchor {
            horizontal_reference: None,
            horizontal_align: None,
            horizontal_offset: None,
            vertical_reference: None,
            vertical_align: None,
            vertical_offset: None,
            wrap: None,
            behind_text: flag("behindDoc").unwrap_or(false),
            layout_in_cell: flag("layoutInCell").unwrap_or(true),
            allow_overlap: flag("allowOverlap").unwrap_or(true),
            z_order: frame
                .attribute(&[None], "relativeHeight")
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0),
            distances: Offsets {
                left: distance("distL"),
                top: distance("distT"),
                right: distance("distR"),
                bottom: distance("distB"),
            },
        };
        for child in self.children(frame) {
            let Some(local) = child.local_in(&names.wp) else {
                continue;
            };
            let relative = child.attribute(&[None], "relativeFrom");
            let align = self
                .child(child, &names.wp, "align")
                .map(|align| align.text().trim().to_owned());
            let offset = self
                .child(child, &names.wp, "posOffset")
                .and_then(|offset| coordinate(&offset.text()));
            match local {
                "positionH" => {
                    anchor.horizontal_reference = relative.and_then(HorizontalReference::from_xml);
                    anchor.horizontal_align =
                        align.as_deref().and_then(AnchorHorizontalAlign::from_xml);
                    anchor.horizontal_offset = offset;
                }
                "positionV" => {
                    anchor.vertical_reference = relative.and_then(VerticalReference::from_xml);
                    anchor.vertical_align =
                        align.as_deref().and_then(AnchorVerticalAlign::from_xml);
                    anchor.vertical_offset = offset;
                }
                "wrapNone" => anchor.wrap = Some(WrapKind::None),
                "wrapSquare" => anchor.wrap = Some(WrapKind::Square),
                "wrapTight" => anchor.wrap = Some(WrapKind::Tight),
                "wrapThrough" => anchor.wrap = Some(WrapKind::Through),
                "wrapTopAndBottom" => anchor.wrap = Some(WrapKind::TopAndBottom),
                _ => {}
            }
        }
        // With simplePos on, the position is wp:simplePos, from the page's top left
        // corner, in place of wp:positionH and wp:positionV (§20.4.2.3).
        if flag("simplePos") == Some(true) {
            let simple = self.child(frame, &names.wp, "simplePos");
            let at = |name| {
                simple
                    .and_then(|simple| simple.attribute(&[None], name))
                    .and_then(coordinate)
            };
            if let (Some(x), Some(y)) = (at("x"), at("y")) {
                anchor.horizontal_reference = Some(HorizontalReference::Page);
                anchor.horizontal_align = None;
                anchor.horizontal_offset = Some(x);
                anchor.vertical_reference = Some(VerticalReference::Page);
                anchor.vertical_align = None;
                anchor.vertical_offset = Some(y);
            }
        }
        Some(anchor)
    }
}

/// A picture (`pic:pic`): an image, cropped, turned and mirrored.
#[derive(Clone, Copy)]
pub struct Picture<'a> {
    document: &'a Document,
    element: Element<'a>,
    image: Option<u32>,
}

impl std::fmt::Debug for Picture<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picture")
            .field("image", &self.image)
            .field("crop", &self.crop())
            .field("rotation", &self.rotation())
            .field("flip_horizontal", &self.flip_horizontal())
            .field("flip_vertical", &self.flip_vertical())
            .finish_non_exhaustive()
    }
}

impl<'a> Picture<'a> {
    /// The image it shows, from the package; none when the package does not have it or
    /// it is linked from outside the package.
    pub fn image(self) -> Option<Image<'a>> {
        let data = self
            .document
            .images
            .get(usize::try_from(self.image?).ok()?)?;
        Some(Image { data })
    }

    /// Where its image is among the document's images, so that pictures of one image
    /// share it.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn image_index(self) -> Option<u32> {
        self.image
    }

    /// The part of the image shown (`a:srcRect` of `pic:blipFill`); none for all of it.
    pub fn crop(self) -> Option<Crop> {
        let names = &self.document.names;
        let rect = self
            .child(&names.pic, "blipFill")
            .and_then(|fill| child(fill, names, &names.a, "srcRect"))?;
        let side = |name| {
            rect.attribute(&[None], name)
                .and_then(percentage)
                .unwrap_or(0.0)
        };
        Some(Crop {
            left: side("l"),
            top: side("t"),
            right: side("r"),
            bottom: side("b"),
        })
    }

    fn child(self, namespaces: &[Option<super::xml::Sym>], name: &str) -> Option<Element<'a>> {
        child(self.element, &self.document.names, namespaces, name)
    }

    /// Whether its outline (`a:ln` of `pic:spPr`) draws a line: one filled other than
    /// with `a:noFill`. The layout does not draw it yet.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_outline(self) -> bool {
        let names = &self.document.names;
        self.child(&names.pic, "spPr")
            .and_then(|shape| child(shape, names, &names.a, "ln"))
            .is_some_and(|line| {
                line.children_through_alternates(names.mc).any(|fill| {
                    matches!(
                        fill.local_in(&names.a),
                        Some("solidFill" | "gradFill" | "pattFill")
                    )
                })
            })
    }

    /// Whether its image is changed in colour or transparency: an effect of `a:blip`, such
    /// as `a:grayscl` or `a:lum`, but not its `a:extLst` and not an effect whose values
    /// change nothing. The layout does not apply them yet.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_color_effects(self) -> bool {
        let names = &self.document.names;
        self.child(&names.pic, "blipFill")
            .and_then(|fill| child(fill, names, &names.a, "blip"))
            .is_some_and(|blip| {
                blip.children_through_alternates(names.mc)
                    .any(|effect| changes_image(effect, names))
            })
    }

    /// Whether it has a shadow, a reflection, a glow, soft edges or a 3-D effect (an
    /// `a:effectLst` or `a:effectDag` that lists one, an `a:scene3d` that turns it, an
    /// `a:sp3d` that gives it a bevel, depth or contour, all of `pic:spPr`). The layout
    /// does not draw them yet.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_effects(self) -> bool {
        let names = &self.document.names;
        let Some(shape) = self.child(&names.pic, "spPr") else {
            return false;
        };
        let nonzero = |element: Element<'_>, name| {
            element
                .attribute(&[None], name)
                .is_some_and(|value| coordinate(value).is_some_and(|length| length.0 != 0.0))
        };
        shape
            .children_through_alternates(names.mc)
            .any(|part| match part.local_in(&names.a) {
                // Word writes an empty a:effectLst for a picture without effects.
                Some("effectLst") => part.children_through_alternates(names.mc).next().is_some(),
                Some("effectDag") => part.children_through_alternates(names.mc).next().is_some(),
                // A camera in front of the picture, not turned, shows it as it is. Office
                // writes one with a rotation of zeros into the effect styles of themes.
                Some("scene3d") => child(part, names, &names.a, "camera").is_some_and(|camera| {
                    camera.attribute(&[None], "prst") != Some("orthographicFront")
                        || child(camera, names, &names.a, "rot").is_some_and(|rotation| {
                            ["lat", "lon", "rev"].into_iter().any(|name| {
                                rotation
                                    .attribute(&[None], name)
                                    .is_some_and(|angle| angle.trim() != "0")
                            })
                        })
                }),
                Some("sp3d") => {
                    child(part, names, &names.a, "bevelT").is_some()
                        || child(part, names, &names.a, "bevelB").is_some()
                        || nonzero(part, "extrusionH")
                        || nonzero(part, "contourW")
                }
                _ => false,
            })
    }

    /// Whether it is cut to a shape other than its rectangle: an `a:prstGeom` of
    /// `pic:spPr` other than `rect`, such as an oval or a rounded rectangle, or an
    /// `a:custGeom`. The layout does not cut it yet.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn is_shaped(self) -> bool {
        let names = &self.document.names;
        self.child(&names.pic, "spPr").is_some_and(|shape| {
            shape
                .children_through_alternates(names.mc)
                .any(|part| match part.local_in(&names.a) {
                    Some("prstGeom") => part.attribute(&[None], "prst") != Some("rect"),
                    Some("custGeom") => true,
                    _ => false,
                })
        })
    }

    fn transform(self) -> Option<Element<'a>> {
        let names = &self.document.names;
        child(self.child(&names.pic, "spPr")?, names, &names.a, "xfrm")
    }

    /// The rotation clockwise, in degrees (`a:xfrm`'s `rot`, 60,000 to a degree).
    pub fn rotation(self) -> f64 {
        self.transform()
            .and_then(|xfrm| xfrm.attribute(&[None], "rot"))
            .and_then(|value| value.trim().parse::<i32>().ok())
            .map_or(0.0, |value| f64::from(value) / 60_000.0)
    }

    /// Whether it is mirrored left to right (`flipH`).
    pub fn flip_horizontal(self) -> bool {
        self.flip("flipH")
    }

    /// Whether it is mirrored top to bottom (`flipV`).
    pub fn flip_vertical(self) -> bool {
        self.flip("flipV")
    }

    fn flip(self, name: &str) -> bool {
        self.transform()
            .and_then(|xfrm| xfrm.attribute(&[None], name))
            .and_then(boolean)
            .unwrap_or(false)
    }
}

/// An image of the package, which every picture that shows it shares.
#[derive(Clone, Copy, Debug)]
pub struct Image<'a> {
    data: &'a ImageData,
}

impl<'a> Image<'a> {
    /// The content type of its part, such as `image/png`.
    pub fn content_type(self) -> &'a str {
        &self.data.content_type
    }

    /// The file as the package holds it.
    pub fn bytes(self) -> &'a [u8] {
        &self.data.bytes
    }

    /// The bytes, shared rather than copied.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn shared(self) -> Arc<[u8]> {
        Arc::clone(&self.data.bytes)
    }
}

/// Lengths on the four sides of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Offsets {
    left: Pt,
    top: Pt,
    right: Pt,
    bottom: Pt,
}

impl Offsets {
    /// At the left.
    pub fn left(&self) -> Pt {
        self.left
    }

    /// At the top.
    pub fn top(&self) -> Pt {
        self.top
    }

    /// At the right.
    pub fn right(&self) -> Pt {
        self.right
    }

    /// At the bottom.
    pub fn bottom(&self) -> Pt {
        self.bottom
    }
}

/// The part of an image a picture shows: how much is cut off on each side, as a share
/// of the image's size (0.1 is a tenth); negative values add room.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Crop {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Crop {
    /// At the left.
    pub fn left(&self) -> f64 {
        self.left
    }

    /// At the top.
    pub fn top(&self) -> f64 {
        self.top
    }

    /// At the right.
    pub fn right(&self) -> f64 {
        self.right
    }

    /// At the bottom.
    pub fn bottom(&self) -> f64 {
        self.bottom
    }
}

/// Where a floating drawing is (`wp:anchor`), and how text wraps around it. With the
/// anchor's `simplePos` on, the position is `wp:simplePos`, measured from the page's top
/// left corner, in place of `wp:positionH` and `wp:positionV` (ECMA-376 Part 1,
/// §20.4.2.3).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Anchor {
    horizontal_reference: Option<HorizontalReference>,
    horizontal_align: Option<AnchorHorizontalAlign>,
    horizontal_offset: Option<Pt>,
    vertical_reference: Option<VerticalReference>,
    vertical_align: Option<AnchorVerticalAlign>,
    vertical_offset: Option<Pt>,
    wrap: Option<WrapKind>,
    behind_text: bool,
    layout_in_cell: bool,
    allow_overlap: bool,
    z_order: u32,
    distances: Offsets,
}

impl Anchor {
    /// What the position across the page is measured from (`wp:positionH`'s
    /// `relativeFrom`).
    pub fn horizontal_reference(&self) -> Option<HorizontalReference> {
        self.horizontal_reference
    }

    /// Where it is across, by alignment (`wp:align`).
    pub fn horizontal_align(&self) -> Option<AnchorHorizontalAlign> {
        self.horizontal_align
    }

    /// Where it is across, by offset (`wp:posOffset`).
    pub fn horizontal_offset(&self) -> Option<Pt> {
        self.horizontal_offset
    }

    /// What the position down the page is measured from (`wp:positionV`'s
    /// `relativeFrom`).
    pub fn vertical_reference(&self) -> Option<VerticalReference> {
        self.vertical_reference
    }

    /// Where it is down, by alignment (`wp:align`).
    pub fn vertical_align(&self) -> Option<AnchorVerticalAlign> {
        self.vertical_align
    }

    /// Where it is down, by offset (`wp:posOffset`).
    pub fn vertical_offset(&self) -> Option<Pt> {
        self.vertical_offset
    }

    /// How text wraps around it.
    pub fn wrap(&self) -> Option<WrapKind> {
        self.wrap
    }

    /// Whether it is behind the text (`behindDoc`).
    pub fn behind_text(&self) -> bool {
        self.behind_text
    }

    /// Whether, anchored in a table cell, it is placed within the cell (`layoutInCell`,
    /// which the standard requires; taken as on when missing).
    pub fn layout_in_cell(&self) -> bool {
        self.layout_in_cell
    }

    /// Whether it may overlap other floating objects (`allowOverlap`, which the standard
    /// requires; taken as on when missing).
    pub fn allow_overlap(&self) -> bool {
        self.allow_overlap
    }

    /// Its place in the stack of floating objects, higher in front (`relativeHeight`).
    pub fn z_order(&self) -> u32 {
        self.z_order
    }

    /// The space kept free between it and wrapping text (`distL` and the others).
    pub fn distances(&self) -> Offsets {
        self.distances
    }
}

xml_enum! {
    /// What a floating drawing's position across the page is measured from (ST_RelFromH).
    HorizontalReference {
        Character = "character",
        Column = "column",
        InsideMargin = "insideMargin",
        LeftMargin = "leftMargin",
        Margin = "margin",
        OutsideMargin = "outsideMargin",
        Page = "page",
        RightMargin = "rightMargin",
    }
}

xml_enum! {
    /// What a floating drawing's position down the page is measured from (ST_RelFromV).
    VerticalReference {
        BottomMargin = "bottomMargin",
        InsideMargin = "insideMargin",
        Line = "line",
        Margin = "margin",
        OutsideMargin = "outsideMargin",
        Page = "page",
        Paragraph = "paragraph",
        TopMargin = "topMargin",
    }
}

xml_enum! {
    /// How a floating drawing is aligned across (ST_AlignH).
    AnchorHorizontalAlign {
        Left = "left",
        Right = "right",
        Center = "center",
        Inside = "inside",
        Outside = "outside",
    }
}

xml_enum! {
    /// How a floating drawing is aligned down (ST_AlignV).
    AnchorVerticalAlign {
        Top = "top",
        Bottom = "bottom",
        Center = "center",
        Inside = "inside",
        Outside = "outside",
    }
}

/// How text wraps around a floating drawing (the `wp:wrap*` element of `wp:anchor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WrapKind {
    /// No wrapping: the drawing floats over or behind the text (`wp:wrapNone`).
    None,
    /// Around its frame (`wp:wrapSquare`).
    Square,
    /// Around its outline (`wp:wrapTight`).
    Tight,
    /// Around its outline and through its holes (`wp:wrapThrough`).
    Through,
    /// Above and below, nothing beside it (`wp:wrapTopAndBottom`).
    TopAndBottom,
}

/// The first child of `element` of this name in `namespaces`, through
/// `mc:AlternateContent`.
fn child<'a>(
    element: Element<'a>,
    names: &Names,
    namespaces: &[Option<super::xml::Sym>],
    name: &str,
) -> Option<Element<'a>> {
    element
        .children_through_alternates(names.mc)
        .find(|child| child.is(namespaces, name))
}

/// The frame of a `w:drawing`: its `wp:inline` or `wp:anchor`.
fn frame<'a>(drawing: Element<'a>, names: &Names) -> Option<Element<'a>> {
    drawing
        .children_through_alternates(names.mc)
        .find(|child| child.is(&names.wp, "inline") || child.is(&names.wp, "anchor"))
}

/// The picture of a `w:drawing`: the `pic:pic` of its frame's `a:graphicData`.
pub(crate) fn picture<'a>(drawing: Element<'a>, names: &Names) -> Option<Element<'a>> {
    let graphic = child(frame(drawing, names)?, names, &names.a, "graphic")?;
    let data = child(graphic, names, &names.a, "graphicData")?;
    child(data, names, &names.pic, "pic")
}

/// The relationship of the image a picture shows: its `a:blip`'s `r:embed`.
pub(crate) fn embedded_image<'a>(picture: Element<'a>, names: &Names) -> Option<&'a str> {
    let fill = child(picture, names, &names.pic, "blipFill")?;
    child(fill, names, &names.a, "blip")?.attribute(&names.r, "embed")
}

/// A length of DrawingML (ST_Coordinate and the like), in points: a count of EMUs, or in
/// Strict a number with a unit (ST_UniversalMeasure: `mm`, `cm`, `in`, `pt`, `pc`,
/// `pi`). None for one beyond ST_Coordinate's range, such as a number of hundreds of
/// digits, which `f64` reads as infinity.
fn coordinate(value: &str) -> Option<Pt> {
    let value = value.trim();
    let emus = match value.parse::<i64>() {
        Ok(emus) => emus as f64,
        Err(_) => {
            let at = value.len().checked_sub(2)?;
            let (number, unit) = (value.get(..at)?, value.get(at..)?);
            let per_unit = match unit {
                "mm" => 72.0 / 25.4,
                "cm" => 72.0 / 2.54,
                "in" => 72.0,
                "pt" => 1.0,
                "pc" | "pi" => 12.0,
                _ => return None,
            };
            measure_number(number)? * per_unit * EMU_PER_POINT
        }
    };
    // Infinity and NaN are not in the range either.
    COORDINATES
        .contains(&emus)
        .then(|| Pt(emus / EMU_PER_POINT))
}

/// The number of ST_UniversalMeasure and ST_Percentage: `-?[0-9]+(\.[0-9]+)?`.
fn measure_number(value: &str) -> Option<f64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let all_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let valid = match digits.split_once('.') {
        Some((whole, fraction)) => all_digits(whole) && all_digits(fraction),
        None => all_digits(digits),
    };
    valid
        .then(|| value.parse().ok())
        .flatten()
        .filter(|number: &f64| number.is_finite())
}

/// A share of DrawingML (ST_Percentage): thousandths of a percent, or in Strict a number
/// of percent with `%`; 0.5 for a half.
fn percentage(value: &str) -> Option<f64> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        return measure_number(percent).map(|percent| percent / 100.0);
    }
    value
        .parse::<i32>()
        .ok()
        .map(|value| f64::from(value) / 100_000.0)
}

/// Whether an effect of `a:blip` changes the image: not its `a:extLst`, and not an
/// effect whose values change nothing: `a:lum` without brightness or contrast,
/// `a:alphaModFix` at 100 % and `a:hsl` without a change of hue, saturation or
/// luminance, as Word writes some of them (fdo43573-2-min.docx of LibreOffice's tests
/// has `<a:lum/>`).
fn changes_image(effect: Element<'_>, names: &Names) -> bool {
    let value = |name, default| {
        effect
            .attribute(&[None], name)
            .and_then(percentage)
            .unwrap_or(default)
    };
    match effect.local_in(&names.a) {
        None | Some("extLst") => false,
        Some("lum") => value("bright", 0.0) != 0.0 || value("contrast", 0.0) != 0.0,
        Some("alphaModFix") => value("amt", 1.0) != 1.0,
        // The hue is an angle, not a share; only whether it is 0 counts here.
        Some("hsl") => ["hue", "sat", "lum"]
            .into_iter()
            .any(|name| value(name, 0.0) != 0.0),
        Some(_) => true,
    }
}

/// A boolean attribute of DrawingML (`xsd:boolean`).
fn boolean(value: &str) -> Option<bool> {
    match value.trim() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}
