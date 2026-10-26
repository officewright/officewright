//! VML drawings in runs (`w:pict`, ECMA-376 Part 4, §14.3.2.2 and §19): the picture one
//! shows, in the line or floating, read from the kept XML when asked for.
//!
//! VML places a shape with the CSS-like declarations of its `style` attribute, which
//! ECMA-376 leaves partly open; Word's reading of them is in MS-OI29500's notes on
//! Part 4, §19.1.2.19 (shape, (e): the positioning properties) and §19.1.2.1 (arc, the
//! notes every shape shares), cited below by their letters.

use officewright_core::Pt;

use super::drawing::{
    Anchor, AnchorHorizontalAlign, AnchorVerticalAlign, Crop, HorizontalReference, Image, Offsets,
    VerticalReference, WrapKind,
};
use super::drawing::{attribute, value};
use super::xml::{Element, Span};
use super::{Document, Names};
use crate::docx::properties::Report;

/// The largest length a VML picture may have, in points: that of ST_Coordinate, which
/// bounds every length of a DrawingML picture, so that both reach the layout alike.
const MAX_LENGTH: f64 = 27_273_042_316_900.0 / 12_700.0;

/// A VML drawing in a run (`w:pict`). A picture (a `v:shape` whose `v:imagedata` names
/// its image) is read, in the line or floating; other shapes, text boxes, WordArt and
/// groups show as no picture, and [`to_xml`](Self::to_xml) gives them as written.
///
/// ```
/// # fn main() -> officewright_doc::Result<()> {
/// use officewright_core::Pt;
/// use officewright_doc::{Block, Document, RunContent};
///
/// let document = Document::open("tests/data/vml-picture.docx")?;
/// let mut pictures = Vec::new();
/// for section in document.sections() {
///     for block in section.blocks() {
///         let Block::Paragraph(paragraph) = block else {
///             continue;
///         };
///         for item in paragraph.runs().flat_map(|run| run.content()) {
///             let RunContent::VmlDrawing(drawing) = item else {
///                 continue;
///             };
///             // A picture shows its image; a shape, text box or group is there as XML.
///             let Some(picture) = drawing.picture() else {
///                 continue;
///             };
///             let image = picture.image();
///             pictures.push((
///                 picture.width(),
///                 picture.height(),
///                 picture.anchor().is_some(),
///                 image.map(|image| image.content_type().to_owned()),
///             ));
///         }
///     }
/// }
/// assert_eq!(
///     pictures,
///     [(Some(Pt(72.0)), Some(Pt(36.0)), false, Some("image/png".to_owned()))]
/// );
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct VmlDrawing<'a> {
    document: &'a Document,
    element: Span,
    image: Option<u32>,
}

impl<'a> VmlDrawing<'a> {
    pub(crate) fn new(document: &'a Document, element: Span, image: Option<u32>) -> Self {
        Self {
            document,
            element,
            image,
        }
    }

    /// The drawing as written, `w:pict` and everything in it, as XML with the namespace
    /// declarations it needs.
    pub fn to_xml(self) -> String {
        self.document.element(self.element).to_xml()
    }

    /// The picture it shows: its first `v:shape` whose `v:imagedata` names an image by
    /// `r:id`, if it has one. A group shows none, even when a picture is among its parts.
    pub fn picture(self) -> Option<VmlPicture<'a>> {
        let names = &self.document.names;
        let pict = self.document.element(self.element);
        let shape = picture(pict, names)?;
        Some(VmlPicture {
            document: self.document,
            pict,
            shape,
            image: self.image,
        })
    }

    /// Whether it has VML shapes besides its picture, which the layout leaves out: a line,
    /// a rectangle, a text box, a group or a second picture (MS-OI29500, Part 4,
    /// §14.3.2.2(a), lists what Word allows in a `w:pict`). A shape type
    /// (`v:shapetype`) only defines shapes, and an embedded object or a control
    /// (`o:OLEObject`, `w:control`) is shown by its picture.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_other_shapes(self) -> bool {
        let names = &self.document.names;
        self.document
            .element(self.element)
            .children_through_alternates(names.mc)
            .filter(|child| {
                child
                    .local_in(&names.v)
                    .is_some_and(|local| local != "shapetype")
            })
            .nth(1)
            .is_some()
    }
}

/// A VML picture (a `v:shape` whose `v:imagedata` names its image): an image, cropped,
/// turned and mirrored, in the line or floating.
#[derive(Clone, Copy)]
pub struct VmlPicture<'a> {
    document: &'a Document,
    /// The `w:pict` that holds it.
    pict: Element<'a>,
    shape: Element<'a>,
    image: Option<u32>,
}

impl std::fmt::Debug for VmlPicture<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmlPicture")
            .field("image", &self.image)
            .field("width", &self.width())
            .field("height", &self.height())
            .field("crop", &self.crop())
            .field("rotation", &self.rotation())
            .field("anchor", &self.anchor())
            .finish_non_exhaustive()
    }
}

impl<'a> VmlPicture<'a> {
    /// The image it shows, from the package; none when the package does not have it.
    /// Word takes the image of a VML picture in a document from `v:imagedata`'s `r:id`
    /// only (MS-OI29500, Part 4, §19.1.2.11(p, q)), and one outside the package is
    /// never loaded.
    pub fn image(self) -> Option<Image<'a>> {
        let data = self
            .document
            .images
            .get(usize::try_from(self.image?).ok()?)?;
        Some(Image::new(data))
    }

    /// The index of its image among the document's images.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn image_index(self) -> Option<u32> {
        self.image
    }

    /// The declarations of its `style`.
    fn style(self) -> Style<'a> {
        style(self.shape)
    }

    /// Its width (`width` of its `style`); none when the style gives none, or one that
    /// is negative or not a length Word takes.
    pub fn width(self) -> Option<Pt> {
        self.style().read("width", size, &mut |_, _, _| {})
    }

    /// Its height (`height` of its `style`), as [`width`](Self::width) says.
    pub fn height(self) -> Option<Pt> {
        self.style().read("height", size, &mut |_, _, _| {})
    }

    /// Whether it is hidden (`visibility:hidden` in its `style`), which Word does not
    /// show.
    pub fn is_hidden(self) -> bool {
        self.style().get("visibility") == Some("hidden")
    }

    /// The part of the image shown (`v:imagedata`'s `cropleft`, `croptop`, `cropright`
    /// and `cropbottom`); none for all of it.
    pub fn crop(self) -> Option<Crop> {
        crop(self.shape, &self.document.names, &mut |_, _, _| {})
    }

    /// The rotation clockwise, in degrees (`rotation` of its `style`).
    pub fn rotation(self) -> f64 {
        self.style()
            .read("rotation", angle, &mut |_, _, _| {})
            .unwrap_or(0.0)
    }

    /// Whether it is mirrored left to right (`flip` of its `style` holds `x`).
    pub fn flip_horizontal(self) -> bool {
        flip(self.style(), &mut |_, _, _| {}).0
    }

    /// Whether it is mirrored top to bottom (`flip` of its `style` holds `y`).
    pub fn flip_vertical(self) -> bool {
        flip(self.style(), &mut |_, _, _| {}).1
    }

    /// Whether it draws an outline, which the layout does not draw yet: `v:stroke`'s `on`,
    /// else the shape's `stroked`, else that of the `v:shapetype` its `type` names, else
    /// VML's default, on. Word writes a shape type once in a document, in the first
    /// `w:pict` that uses it: one named but not in this `w:pict` is taken as Word's
    /// picture frame (`o:spt="75"`), which is not stroked.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_outline(self) -> bool {
        has_outline(
            self.shape,
            self.pict,
            &self.document.names,
            &mut |_, _, _| {},
        )
    }

    /// Whether its `v:imagedata` changes the image's colours, which the layout does not
    /// do yet: a `gain`, `blacklevel` or `gamma` other than none, `grayscale` or
    /// `bilevel` on, or a `chromakey`, `embosscolor` or `recolortarget` (the washed-out
    /// look of a picture watermark is a gain and a black level).
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_color_adjustments(self) -> bool {
        has_color_adjustments(self.shape, &self.document.names, &mut |_, _, _| {})
    }

    /// Whether it casts a shadow (`v:shadow` with `on` true, which a shadow needs to show),
    /// which the layout does not draw yet. Office takes a blank `on` for true and an
    /// absent one for false (MS-OI29500, Part 4, §19.1.2.18).
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_shadow(self) -> bool {
        has_shadow(self.shape, &self.document.names, &mut |_, _, _| {})
    }

    /// Whether it has a border, which the layout does not draw yet: a `w10:bordertop`,
    /// `w10:borderleft`, `w10:borderbottom` or `w10:borderright` (ECMA-376 Part 4,
    /// §19.3.2.2 to §19.3.2.5) whose `type` is not `none`. One without a `type` counts:
    /// no source says whether Word draws it, and a border left out without an entry in
    /// the layout's report would be the worse error.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_border(self) -> bool {
        let names = &self.document.names;
        self.shape
            .children_through_alternates(names.mc)
            .any(|child| {
                ["bordertop", "borderleft", "borderbottom", "borderright"]
                    .into_iter()
                    .any(|name| child.is(&names.w10, name))
                    && child.attribute(&[None], "type") != Some("none")
            })
    }

    /// Whether it holds text (`v:textbox`), which the layout does not draw yet.
    // reason: only the translation into the layout IR asks for it.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn has_text(self) -> bool {
        let names = &self.document.names;
        self.shape
            .children_through_alternates(names.mc)
            .any(|child| child.is(&names.v, "textbox"))
    }

    /// Where a floating picture (`position:absolute` in its `style`) is and how text
    /// wraps around it; none for a picture in the line.
    ///
    /// The position across is `mso-position-horizontal` as an alignment within
    /// `mso-position-horizontal-relative`, else `margin-left` and `left` added up as an
    /// offset from it, as Word combines them (MS-OI29500, Part 4, §19.1.2.1(hh, dddd));
    /// down, the same with `mso-position-vertical`, `mso-position-vertical-relative`,
    /// `margin-top` and `top`. Where the style names no base, `w10:wrap`'s `anchorx` and
    /// `anchory` do, as Word 2007 wrote it, `text` when not written (§19.3.2.6(a)); Office
    /// takes `margin`, `page` and `text` there, not `char` or `line` (§19.3.3.3(a),
    /// §19.3.3.4(a)). A
    /// negative `z-index` puts it behind the text, and the value orders it among the
    /// floating objects of its side. Text wraps as its `w10:wrap`
    /// says, `mso-wrap-distance-left` and `-right` from it, 9 pt when not written, as
    /// Word takes them (§19.1.2.1(o)), and `-top` and `-bottom`, 0 when not written.
    pub fn anchor(self) -> Option<Anchor> {
        anchor(self.shape, &self.document.names, &mut |_, _, _| {})
    }
}

/// Reports the values of a VML drawing's picture (`w:pict`) that its readers do not take,
/// as the document opens; the readers read them again when asked for, as not written.
/// Every value of the picture that a reader reads is read here once, also one the
/// picture does not use, such as the offset of an aligned position; those that place a
/// floating picture, only when it floats.
pub(crate) fn check(pict: Element<'_>, names: &Names, report: Report<'_>) {
    let Some(shape) = picture(pict, names) else {
        return;
    };
    let style = style(shape);
    style.read("width", size, report);
    style.read("height", size, report);
    crop(shape, names, report);
    style.read("rotation", angle, report);
    flip(style, report);
    has_outline(shape, pict, names, report);
    has_color_adjustments(shape, names, report);
    has_shadow(shape, names, report);
    anchor(shape, names, report);
}

/// The declarations of a shape's `style`.
fn style(shape: Element<'_>) -> Style<'_> {
    Style(shape.attribute(&[None], "style").unwrap_or_default())
}

/// The part of its image a picture shows (`v:imagedata`'s `cropleft`, `croptop`,
/// `cropright` and `cropbottom`); none for all of it.
fn crop(shape: Element<'_>, names: &Names, report: Report<'_>) -> Option<Crop> {
    let data = image_data(shape, names)?;
    let mut side = |name| value(data, names, name, fraction, report);
    let sides = [
        side("cropleft"),
        side("croptop"),
        side("cropright"),
        side("cropbottom"),
    ];
    if sides.iter().all(Option::is_none) {
        return None;
    }
    let [left, top, right, bottom] = sides.map(|side| side.unwrap_or(0.0));
    Some(Crop {
        left,
        top,
        right,
        bottom,
    })
}

/// The axes `flip` mirrors: Word takes `x`, `y`, `x y` and `y x` (MS-OI29500, Part 4,
/// §19.1.2.1(c)), and nothing for any other value.
fn flip(style: Style<'_>, report: Report<'_>) -> (bool, bool) {
    style
        .read(
            "flip",
            |value| match value {
                "x" => Some((true, false)),
                "y" => Some((false, true)),
                "x y" | "y x" => Some((true, true)),
                _ => None,
            },
            report,
        )
        .unwrap_or((false, false))
}

/// Whether a picture draws an outline ([`VmlPicture::has_outline`]). Each value is read,
/// so that each one not taken is reported.
fn has_outline(shape: Element<'_>, pict: Element<'_>, names: &Names, report: Report<'_>) -> bool {
    let stroke = shape
        .children_through_alternates(names.mc)
        .find(|child| child.is(&names.v, "stroke"))
        .and_then(|stroke| value(stroke, names, "on", true_false, report));
    let stroked = value(shape, names, "stroked", true_false, report);
    let kind = shape.attribute(&[None], "type");
    let id = kind.map(|kind| kind.strip_prefix('#').unwrap_or(kind));
    let type_stroked = id
        .and_then(|id| {
            pict.children_through_alternates(names.mc)
                .filter(|child| child.is(&names.v, "shapetype"))
                .find(|child| child.attribute(&[None], "id") == Some(id))
        })
        .map(|shape_type| value(shape_type, names, "stroked", true_false, report).unwrap_or(true));
    stroke
        .or(stroked)
        .unwrap_or_else(|| kind.is_none() || type_stroked.unwrap_or(false))
}

/// Whether a picture's `v:imagedata` changes the image's colours
/// ([`VmlPicture::has_color_adjustments`]). Each value is read, so that each one not
/// taken is reported.
fn has_color_adjustments(shape: Element<'_>, names: &Names, report: Report<'_>) -> bool {
    let Some(data) = image_data(shape, names) else {
        return false;
    };
    let mut differs = |name, none: f64| {
        value(data, names, name, fraction, report).is_some_and(|value| value != none)
    };
    let adjusted = differs("gain", 1.0) | differs("blacklevel", 0.0) | differs("gamma", 1.0);
    let mut on = |name| value(data, names, name, true_false, report) == Some(true);
    let drawn = on("grayscale") | on("bilevel");
    adjusted
        || drawn
        || ["chromakey", "embosscolor", "recolortarget"]
            .into_iter()
            .any(|name| data.attribute(&[None], name).is_some())
}

/// Whether a picture casts a shadow ([`VmlPicture::has_shadow`]). Each `v:shadow` is
/// read, so that each value not taken is reported.
fn has_shadow(shape: Element<'_>, names: &Names, report: Report<'_>) -> bool {
    shape
        .children_through_alternates(names.mc)
        .filter(|child| child.is(&names.v, "shadow"))
        .fold(false, |shadow, element| {
            // A blank on is true; an absent one is false.
            let on = value(
                element,
                names,
                "on",
                |on| {
                    if on.is_empty() {
                        Some(true)
                    } else {
                        true_false(on)
                    }
                },
                report,
            );
            on == Some(true) || shadow
        })
}

/// Where a floating picture is and how text wraps around it ([`VmlPicture::anchor`]).
fn anchor(shape: Element<'_>, names: &Names, report: Report<'_>) -> Option<Anchor> {
    let style = style(shape);
    if style.get("position") != Some("absolute") {
        return None;
    }
    let horizontal_align = style
        .read(
            "mso-position-horizontal",
            |value| alignment(value, AnchorHorizontalAlign::from_vml),
            report,
        )
        .flatten();
    let vertical_align = style
        .read(
            "mso-position-vertical",
            |value| alignment(value, AnchorVerticalAlign::from_vml),
            report,
        )
        .flatten();
    let mut offset = |margin, edge| {
        let sum = style.read(margin, length, report).unwrap_or_default().0
            + style.read(edge, length, report).unwrap_or_default().0;
        Pt(sum.clamp(-MAX_LENGTH, MAX_LENGTH))
    };
    let horizontal_offset = offset("margin-left", "left");
    let vertical_offset = offset("margin-top", "top");
    let z_index = style
        .read("z-index", |value| value.parse::<i32>().ok(), report)
        .unwrap_or(0);
    let mut flag =
        |name| attribute(shape, names, (&names.o, "o:"), name, true_false, report).unwrap_or(true);
    let layout_in_cell = flag("allowincell");
    let allow_overlap = flag("allowoverlap");
    let mut distance = |name, default| style.read(name, size, report).unwrap_or(Pt(default));
    let distances = Offsets {
        left: distance("mso-wrap-distance-left", 9.0),
        top: distance("mso-wrap-distance-top", 0.0),
        right: distance("mso-wrap-distance-right", 9.0),
        bottom: distance("mso-wrap-distance-bottom", 0.0),
    };
    let wrap_element = shape
        .children_through_alternates(names.mc)
        .find(|child| child.is(&names.w10, "wrap"));
    let wrap = wrap_element.and_then(|wrap| value(wrap, names, "type", WrapKind::from_vml, report));
    let anchor_x = wrap_element.and_then(|wrap| {
        value(
            wrap,
            names,
            "anchorx",
            HorizontalReference::from_wrap,
            report,
        )
    });
    let anchor_y = wrap_element
        .and_then(|wrap| value(wrap, names, "anchory", VerticalReference::from_wrap, report));
    let horizontal_reference = style
        .read(
            "mso-position-horizontal-relative",
            HorizontalReference::from_vml,
            report,
        )
        .or(anchor_x);
    let vertical_reference = style
        .read(
            "mso-position-vertical-relative",
            VerticalReference::from_vml,
            report,
        )
        .or(anchor_y);
    // Word ignores the offsets when the picture is aligned (§19.1.2.1(t, dddd)).
    Some(Anchor {
        horizontal_reference,
        horizontal_align,
        horizontal_offset: horizontal_align.is_none().then_some(horizontal_offset),
        vertical_reference,
        vertical_align,
        vertical_offset: vertical_align.is_none().then_some(vertical_offset),
        wrap,
        behind_text: z_index < 0,
        layout_in_cell,
        allow_overlap,
        z_order: z_order(z_index),
        distances,
    })
}

/// An alignment of `mso-position-horizontal` or `mso-position-vertical`: none for
/// `absolute`, which places the picture by its offset (MS-OI29500, Part 4,
/// §19.1.2.19(e)).
fn alignment<A>(value: &str, from_vml: fn(&str) -> Option<A>) -> Option<Option<A>> {
    match value {
        "absolute" => Some(None),
        _ => from_vml(value).map(Some),
    }
}

/// How far below a picture's `relativeHeight` Word writes the `z-index` of its VML when the
/// picture is behind the text.
const BEHIND_TEXT_Z_SHIFT: i64 = 503_316_480;

/// The order of a picture among the floating objects of its side of the text, on the
/// scale of DrawingML's `relativeHeight`, from its `z-index`. MS-OI29500, Part 4,
/// §19.1.2.1(ee), says only that Office keeps the order and sign of `z-index`; where Word
/// writes a floating object twice, a DrawingML shape with a VML fallback, the `z-index`
/// of one in front of the text is its `relativeHeight` and that of one behind the text
/// its `relativeHeight` less [`BEHIND_TEXT_Z_SHIFT`] (every such pair of the committed
/// corpus, 15 in front and 18 behind, and all but 3 of the 483 pairs of corpus-ext, which
/// LibreOffice wrote). So the order is taken back the same way, and VML pictures and
/// DrawingML objects stack together; an index below that shift is at the back.
fn z_order(z_index: i32) -> u32 {
    let height = if z_index >= 0 {
        i64::from(z_index)
    } else {
        i64::from(z_index) + BEHIND_TEXT_Z_SHIFT
    };
    u32::try_from(height.max(0)).unwrap_or(0)
}

/// The picture of a `w:pict`: its first `v:shape` whose `v:imagedata` names an image by
/// `r:id`, the only way Word takes one in a document (MS-OI29500, Part 4,
/// §19.1.2.11(q)). A shape whose `v:imagedata` names none, as a text box Word writes may
/// have, shows no picture.
pub(crate) fn picture<'a>(pict: Element<'a>, names: &Names) -> Option<Element<'a>> {
    pict.children_through_alternates(names.mc)
        .filter(|child| child.is(&names.v, "shape"))
        .find(|shape| embedded_image(*shape, names).is_some())
}

/// The relationship of the image a VML picture shows: its `v:imagedata`'s `r:id`.
pub(crate) fn embedded_image<'a>(shape: Element<'a>, names: &Names) -> Option<&'a str> {
    image_data(shape, names)?.attribute(&names.r, "id")
}

/// The `v:imagedata` of a shape; Word takes at most one (MS-OI29500, Part 4,
/// §19.1.2.11(a)), the first.
fn image_data<'a>(shape: Element<'a>, names: &Names) -> Option<Element<'a>> {
    shape
        .children_through_alternates(names.mc)
        .find(|child| child.is(&names.v, "imagedata"))
}

/// The declarations of a VML `style` attribute: `name:value` pairs separated by `;`.
#[derive(Clone, Copy)]
struct Style<'a>(&'a str);

impl<'a> Style<'a> {
    /// The value of a property, without the whitespace around it. Word takes the last
    /// of a property written more than once (MS-OI29500, Part 4, §19.1.2.1(ggg)); CSS
    /// names are not case-sensitive.
    fn get(self, name: &str) -> Option<&'a str> {
        self.0
            .split(';')
            .filter_map(|declaration| declaration.split_once(':'))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
            .next_back()
    }

    /// A property's value as `read` takes it; one it does not take is reported, as the
    /// declaration of `v:shape`'s `style` that gives it, and read as not written.
    fn read<T>(
        self,
        name: &str,
        read: impl FnOnce(&'a str) -> Option<T>,
        report: Report<'_>,
    ) -> Option<T> {
        let value = self.get(name)?;
        let taken = read(value);
        if taken.is_none() {
            report("v:shape", "style", &format!("{name}:{value}"));
        }
        taken
    }
}

/// A length that is not negative, as Word requires of sizes and wrap distances
/// (MS-OI29500, Part 4, §19.1.2.1(mm)).
fn size(value: &str) -> Option<Pt> {
    length(value).filter(|length| length.0 >= 0.0)
}

/// A length of a VML style, in points: a number and one of the units Word takes, `pt`,
/// `in`, `cm`, `mm`, `pc` or `px` (MS-OI29500, Part 4, §19.1.2.1(n, r, s)), or 0 without
/// one, as CSS allows. Word counts a pixel as a relative unit (§19.1.2.1(ddd)): it is
/// taken as 0.75 pt, a pixel at 96 dpi. None for a percentage, `em`, `auto`, any other
/// value, and one beyond [`MAX_LENGTH`].
fn length(value: &str) -> Option<Pt> {
    let at = value.len().saturating_sub(2);
    let (number, unit) = match (value.get(..at), value.get(at..)) {
        (Some(number), Some(unit)) if unit.bytes().all(|b| b.is_ascii_alphabetic()) => {
            (number, unit)
        }
        _ => (value, ""),
    };
    let per_unit = match unit.to_ascii_lowercase().as_str() {
        "pt" => 1.0,
        "in" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        "pc" => 12.0,
        "px" => 0.75,
        "" => 0.0,
        _ => return None,
    };
    let number = css_number(number)?;
    if unit.is_empty() && number != 0.0 {
        return None;
    }
    let points = number * per_unit;
    (points.abs() <= MAX_LENGTH).then_some(Pt(points))
}

/// A number of CSS: an optional sign, then digits with an optional fraction, or a
/// fraction alone (Word writes `.1pt`).
fn css_number(value: &str) -> Option<f64> {
    let digits = value
        .strip_prefix('-')
        .or_else(|| value.strip_prefix('+'))
        .unwrap_or(value);
    let all_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    let valid = match digits.split_once('.') {
        Some((whole, fraction)) => {
            all_digits(whole) && all_digits(fraction) && !(whole.is_empty() && fraction.is_empty())
        }
        None => !digits.is_empty() && all_digits(digits),
    };
    valid
        .then(|| value.parse().ok())
        .flatten()
        .filter(|number: &f64| number.is_finite())
}

/// A share of VML (ST_Fraction, MS-OI29500 §3.9.1, which Office takes for
/// `v:imagedata`'s crop attributes, Part 4, §19.1.2.11(h, i, m, n)): a decimal in
/// [-32768, 32767], or a count of 1/65,536 with `f`.
fn fraction(value: &str) -> Option<f64> {
    if let Some(count) = value.strip_suffix('f') {
        return fixed(count);
    }
    css_number(value).filter(|number| (-32_768.0..=32_767.0).contains(number))
}

/// An angle in degrees: a decimal, or a count of 1/65,536 degree with `fd`
/// (ST_Fraction's third form).
fn angle(value: &str) -> Option<f64> {
    match value.strip_suffix("fd") {
        Some(count) => fixed(count),
        None => css_number(value),
    }
}

/// A count of 1/65,536, ST_Fraction's integer forms: within [-2147483648, 2147418112].
fn fixed(count: &str) -> Option<f64> {
    let count = count.parse::<i32>().ok()?;
    (count <= 2_147_418_112).then(|| f64::from(count) / 65_536.0)
}

/// A boolean of VML (ST_TrueFalse): `t`, `true`, `f` or `false`.
fn true_false(value: &str) -> Option<bool> {
    match value {
        "t" | "true" => Some(true),
        "f" | "false" => Some(false),
        _ => None,
    }
}

impl HorizontalReference {
    /// The value of `mso-position-horizontal-relative` (MS-OI29500, Part 4,
    /// §19.1.2.19(e) and §19.1.2.1(j)); the notes write the margins both with and
    /// without `-area`.
    fn from_vml(value: &str) -> Option<Self> {
        Some(match value {
            "margin" => Self::Margin,
            "page" => Self::Page,
            "text" => Self::Column,
            "char" => Self::Character,
            "left-margin-area" | "left-margin" => Self::LeftMargin,
            "right-margin-area" | "right-margin" => Self::RightMargin,
            "inner-margin-area" => Self::InsideMargin,
            "outer-margin-area" => Self::OutsideMargin,
            _ => return None,
        })
    }

    /// The value of `w10:wrap`'s `anchorx` (ST_HorizontalAnchor): Office takes `margin`,
    /// `page` and `text`, not `char` (MS-OI29500, Part 4, §19.3.3.3(a)).
    fn from_wrap(value: &str) -> Option<Self> {
        Some(match value {
            "margin" => Self::Margin,
            "page" => Self::Page,
            "text" => Self::Column,
            _ => return None,
        })
    }
}

impl VerticalReference {
    /// The value of `mso-position-vertical-relative` (MS-OI29500, Part 4,
    /// §19.1.2.19(e) and §19.1.2.1(m)), as [`HorizontalReference::from_vml`] says.
    fn from_vml(value: &str) -> Option<Self> {
        Some(match value {
            "margin" => Self::Margin,
            "page" => Self::Page,
            "text" => Self::Paragraph,
            "line" => Self::Line,
            "top-margin-area" | "top-margin" => Self::TopMargin,
            "bottom-margin-area" | "bottom-margin" => Self::BottomMargin,
            "inner-margin-area" => Self::InsideMargin,
            "outer-margin-area" => Self::OutsideMargin,
            _ => return None,
        })
    }

    /// The value of `w10:wrap`'s `anchory` (ST_VerticalAnchor): Office takes `margin`,
    /// `page` and `text`, not `line` (MS-OI29500, Part 4, §19.3.3.4(a)).
    fn from_wrap(value: &str) -> Option<Self> {
        Some(match value {
            "margin" => Self::Margin,
            "page" => Self::Page,
            "text" => Self::Paragraph,
            _ => return None,
        })
    }
}

impl AnchorHorizontalAlign {
    /// A value of `mso-position-horizontal` other than `absolute` ([`alignment`]).
    fn from_vml(value: &str) -> Option<Self> {
        Some(match value {
            "left" => Self::Left,
            "center" => Self::Center,
            "right" => Self::Right,
            "inside" => Self::Inside,
            "outside" => Self::Outside,
            _ => return None,
        })
    }
}

impl AnchorVerticalAlign {
    /// A value of `mso-position-vertical` other than `absolute` ([`alignment`]).
    fn from_vml(value: &str) -> Option<Self> {
        Some(match value {
            "top" => Self::Top,
            "center" => Self::Center,
            "bottom" => Self::Bottom,
            "inside" => Self::Inside,
            "outside" => Self::Outside,
            _ => return None,
        })
    }
}

impl WrapKind {
    /// The `type` of `w10:wrap` (ECMA-376 Part 4, §19.3.2.6), of ST_WrapType (§19.3.3).
    fn from_vml(value: &str) -> Option<Self> {
        Some(match value {
            "none" => Self::None,
            "square" => Self::Square,
            "tight" => Self::Tight,
            "through" => Self::Through,
            "topAndBottom" => Self::TopAndBottom,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lengths_in_the_units_word_takes() {
        assert_eq!(length("12.5pt"), Some(Pt(12.5)));
        assert_eq!(length(".1pt"), Some(Pt(0.1)));
        assert_eq!(length("-6pt"), Some(Pt(-6.0)));
        assert_eq!(length("1in"), Some(Pt(72.0)));
        assert_eq!(length("2.54cm"), Some(Pt(72.0)));
        assert_eq!(length("25.4mm"), Some(Pt(72.0)));
        assert_eq!(length("1pc"), Some(Pt(12.0)));
        assert_eq!(length("4px"), Some(Pt(3.0)));
        assert_eq!(length("0"), Some(Pt(0.0)));
        for refused in [
            "", "pt", "12", "12%", "2em", "auto", "1e3pt", "1.pt.", "--1pt", "12 pt", "1ptx",
        ] {
            assert_eq!(length(refused), None, "{refused:?}");
        }
        let endless = format!("{}pt", "9".repeat(400));
        assert_eq!(length(&endless), None);
        assert_eq!(length("2147483647pt"), Some(Pt(2_147_483_647.0)));
        assert_eq!(length("2147483648pt"), None);
    }

    #[test]
    fn reads_the_last_declaration_of_a_property() {
        let style = Style(" width : 10pt;HEIGHT:5pt;width:20pt; ;junk;rotation");
        assert_eq!(style.get("width").and_then(length), Some(Pt(20.0)));
        assert_eq!(style.get("height").and_then(length), Some(Pt(5.0)));
        assert_eq!(style.get("rotation"), None);
        assert_eq!(Style("width:-1pt").get("width").and_then(size), None);
    }

    #[test]
    fn reads_fractions_and_angles_of_st_fraction() {
        assert_eq!(fraction("32768f"), Some(0.5));
        assert_eq!(fraction("-6554f").map(|f| (f * 1e4).round()), Some(-1000.0));
        assert_eq!(fraction("0.25"), Some(0.25));
        assert_eq!(fraction("32767"), Some(32_767.0));
        assert_eq!(fraction("32768"), None);
        assert_eq!(fraction("2147418113f"), None);
        assert_eq!(fraction("12%"), None);
        assert_eq!(angle("-90"), Some(-90.0));
        assert_eq!(angle("5898240fd"), Some(90.0));
        assert_eq!(angle("90deg"), None);
    }

    #[test]
    fn orders_pictures_by_z_index_on_each_side_of_the_text() {
        // In front of the text, the relativeHeight Word writes is the z-index.
        assert_eq!(z_order(0), 0);
        assert_eq!(z_order(251_659_264), 251_659_264);
        assert_eq!(z_order(i32::MAX), i32::MAX.unsigned_abs());
        // Behind the text, it is the z-index plus 503,316,480: form-cfpb-credit-form-c3-es
        // writes relativeHeight 487587840 and z-index -15728640 for one picture.
        assert_eq!(z_order(-15_728_640), 487_587_840);
        assert_eq!(z_order(-251_658_240), 251_658_240);
        assert_eq!(z_order(-1), 503_316_479);
        assert!(z_order(-15_778_816) < z_order(-15_728_640));
        // An index below the shift is at the back.
        assert_eq!(z_order(-503_316_480), 0);
        assert_eq!(z_order(-503_316_481), 0);
        assert_eq!(z_order(i32::MIN), 0);
    }
}
