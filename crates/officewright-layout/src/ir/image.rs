//! Images, inline and anchored: the image data a document holds once, the pictures that
//! show it, and the position of floating objects.

use std::sync::Arc;

use officewright_core::Pt;

use super::ImageId;
use super::macros::{ir_struct, no_rules};
use super::rules;
use super::walk::{Check, Checker, Clamp, Clamper};
use crate::Result;

ir_struct! {
    /// The bytes of an image, which every picture that shows it shares.
    #[derive(Clone, Debug, PartialEq)]
    pub struct ImageData {
        /// The format, as the producer found it.
        ref format: ImageFormat => with_format;
        /// The file as the document holds it.
        ref bytes: Arc<[u8]> => with_bytes;
    }
}

impl ImageData {
    /// Image data of this format.
    pub fn new(format: ImageFormat, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            format,
            bytes: bytes.into(),
        }
    }
}

/// The format of an image.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPEG.
    Jpeg,
    /// Any other, by a short name such as `emf` or `svg`. Shown as a placeholder of the
    /// same size, and reported.
    Other(String),
}

ir_struct! {
    /// A picture: an image shown at a size, in a line of text or in an anchored object.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Picture {
        /// The image.
        copy image: ImageId => with_image;
        /// The size the image is shown at (`wp:extent`).
        copy size: Size => with_size;
        /// The part of the image that is shown (`a:srcRect`, whose values are thousandths of a
        /// percent); `None` for all of it.
        copy crop: Option<Crop> => with_crop;
        /// How far effects such as shadows reach beyond the picture on each side
        /// (`wp:effectExtent`); a picture in a line takes this much more room. Word adds
        /// `l`, `r` and `t` at the left, right and top, where the standard says bottom
        /// (MS-OI29500 2.1.1352 a).
        copy effect_extent: Insets => with_effect_extent;
        /// The rotation clockwise, in degrees (`a:xfrm`'s `rot` counts 60000 to a degree).
        copy rotation: f64 => with_rotation;
        /// Whether the picture is mirrored left to right.
        copy flip_horizontal: bool => with_flip_horizontal;
        /// Whether the picture is mirrored top to bottom.
        copy flip_vertical: bool => with_flip_vertical;
    }
}

impl Picture {
    /// The whole image at this size, not rotated, without effects.
    pub fn new(image: ImageId, size: Size) -> Self {
        Self {
            image,
            size,
            crop: None,
            effect_extent: Insets::default(),
            rotation: 0.0,
            flip_horizontal: false,
            flip_vertical: false,
        }
    }
}

ir_struct! {
    /// A width and a height.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Size {
        /// The width.
        copy width: Pt = Pt(0.0) => with_width, check rules::non_negative;
        /// The height.
        copy height: Pt = Pt(0.0) => with_height, check rules::non_negative;
    }
}

impl Size {
    /// This width and height.
    pub fn new(width: Pt, height: Pt) -> Self {
        Self { width, height }
    }
}

ir_struct! {
    /// Lengths at the four sides of a box, left and right as on the page.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Insets {
        /// At the left.
        copy left: Pt = Pt(0.0) => with_left;
        /// At the top.
        copy top: Pt = Pt(0.0) => with_top;
        /// At the right.
        copy right: Pt = Pt(0.0) => with_right;
        /// At the bottom.
        copy bottom: Pt = Pt(0.0) => with_bottom;
    }
}

ir_struct! {
    /// The part of an image a picture shows, as shares of the image's width and height
    /// cut off at each side: 0.1 cuts off a tenth. A negative share adds empty room.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Crop {
        /// At the left.
        copy left: f64 = 0.0 => with_left;
        /// At the top.
        copy top: f64 = 0.0 => with_top;
        /// At the right.
        copy right: f64 = 0.0 => with_right;
        /// At the bottom.
        copy bottom: f64 = 0.0 => with_bottom;
    }
}

ir_struct! {
    /// A floating object (`wp:anchor`), anchored to the paragraph it appears in. Text
    /// does not wrap around it yet: it is drawn at its position, and any wrapping is
    /// reported.
    ///
    /// ```
    /// use officewright_core::Pt;
    /// use officewright_layout::ir::{
    ///     AnchoredContent, AnchoredObject, Document, HorizontalAlign, HorizontalAnchor,
    ///     HorizontalPlacement, HorizontalReference, ImageData, ImageFormat, Inline, PageSetup,
    ///     Paragraph, Picture, RunStyle, Section, Size, VerticalAnchor, VerticalPlacement,
    ///     VerticalReference,
    /// };
    ///
    /// let mut document = Document::default();
    /// let style = document.add_run_style(RunStyle::default());
    /// let logo = document.add_image(ImageData::new(ImageFormat::Png, b"\x89PNG\r\n\x1a\n".to_vec()));
    /// // A logo at the right edge of the page, a quarter inch below the top edge.
    /// let object = AnchoredObject::new(
    ///     AnchoredContent::Picture(Picture::new(logo, Size::new(Pt(120.0), Pt(36.0)))),
    ///     HorizontalAnchor::default()
    ///         .with_relative_to(HorizontalReference::Page)
    ///         .with_placement(HorizontalPlacement::Align(HorizontalAlign::Right)),
    ///     VerticalAnchor::default()
    ///         .with_relative_to(VerticalReference::Page)
    ///         .with_placement(VerticalPlacement::Offset(Pt(18.0))),
    /// );
    /// let header = Paragraph::new(style).with_inlines([Inline::anchored(object)]);
    /// document.add_section(Section::new(PageSetup::default()).with_blocks([header.into()]));
    /// document.validate()?;
    /// # Ok::<(), officewright_layout::Error>(())
    /// ```
    #[derive(Clone, Debug, PartialEq)]
    pub struct AnchoredObject {
        /// What the object shows.
        ref content: AnchoredContent => with_content;
        /// Where it is across the page.
        copy horizontal: HorizontalAnchor => with_horizontal;
        /// Where it is down the page.
        copy vertical: VerticalAnchor => with_vertical;
        /// How text wraps around it.
        copy wrap: Wrap => with_wrap;
        /// Whether it is drawn behind the text instead of in front.
        copy behind_text: bool => with_behind_text;
        /// Whether, anchored in a table cell, it is positioned within the cell.
        copy layout_in_cell: bool => with_layout_in_cell;
        /// Whether it may overlap other floating objects.
        copy allow_overlap: bool => with_allow_overlap;
        /// Its place in the stack of floating objects: higher is in front
        /// (`relativeHeight`).
        copy z_order: u32 => with_z_order;
        /// The space kept free between it and wrapping text (`distT` and the others).
        copy distances: Insets => with_distances, check insets_non_negative;
    }
}

impl AnchoredObject {
    /// An object with this content and position, in front of the text, without
    /// wrapping, positioned within a table cell when anchored in one, allowed to overlap
    /// other objects, at the bottom of the stack and with no distances. WordprocessingML
    /// requires each of these attributes, so the producer passes the document's.
    pub fn new(
        content: AnchoredContent,
        horizontal: HorizontalAnchor,
        vertical: VerticalAnchor,
    ) -> Self {
        Self {
            content,
            horizontal,
            vertical,
            wrap: Wrap::None,
            behind_text: false,
            layout_in_cell: true,
            allow_overlap: true,
            z_order: 0,
            distances: Insets::default(),
        }
    }
}

fn insets_non_negative(insets: &Insets, checker: &mut Checker<'_>) -> Result<()> {
    for (side, value) in [
        ("left", insets.left),
        ("top", insets.top),
        ("right", insets.right),
        ("bottom", insets.bottom),
    ] {
        checker.enter(side);
        rules::non_negative(&value, checker)?;
        checker.leave();
    }
    Ok(())
}

/// What an anchored object shows.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum AnchoredContent {
    /// A picture.
    Picture(Picture),
}

impl Check for AnchoredContent {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Picture(picture) => picture.check(checker),
        }
    }
}

impl Clamp for AnchoredContent {
    fn clamp(&mut self, clamper: &mut Clamper) {
        match self {
            Self::Picture(picture) => picture.clamp(clamper),
        }
    }
}

ir_struct! {
    /// The position of an anchored object across the page (`wp:positionH`). The element is
    /// required, so its defaults here are only a starting point: 0 pt from the column.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct HorizontalAnchor {
        /// What the position is measured from.
        copy relative_to: HorizontalReference = HorizontalReference::Column => with_relative_to;
        /// Where the object is against it.
        copy placement: HorizontalPlacement = HorizontalPlacement::Offset(Pt(0.0))
            => with_placement;
    }
}

ir_struct! {
    /// The position of an anchored object down the page (`wp:positionV`). The element is
    /// required, so its defaults here are only a starting point: 0 pt from the paragraph.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct VerticalAnchor {
        /// What the position is measured from.
        copy relative_to: VerticalReference = VerticalReference::Paragraph => with_relative_to;
        /// Where the object is against it.
        copy placement: VerticalPlacement = VerticalPlacement::Offset(Pt(0.0))
            => with_placement;
    }
}

/// What an anchored object's position across the page is measured from (ST_RelFromH).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HorizontalReference {
    /// The anchor's place in its line.
    Character,
    /// The column the anchor is in.
    #[default]
    Column,
    /// The margin on the inner side: left on odd pages, right on even ones.
    InsideMargin,
    /// The left margin.
    LeftMargin,
    /// The area between the left and right margins.
    Margin,
    /// The margin on the outer side.
    OutsideMargin,
    /// The page.
    Page,
    /// The right margin.
    RightMargin,
}

/// What an anchored object's position down the page is measured from (ST_RelFromV).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VerticalReference {
    /// The bottom margin.
    BottomMargin,
    /// The margin on the inner side of the page.
    InsideMargin,
    /// The line the anchor is in.
    Line,
    /// The area between the top and bottom margins.
    Margin,
    /// The margin on the outer side of the page.
    OutsideMargin,
    /// The page.
    Page,
    /// The paragraph the anchor is in. MS-OI29500 2.1.1355 a says Word does not allow
    /// it, but Word writes it.
    #[default]
    Paragraph,
    /// The top margin.
    TopMargin,
}

/// Where an anchored object is across its reference.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum HorizontalPlacement {
    /// This far from the reference's left edge, rightwards.
    Offset(Pt),
    /// Aligned within the reference.
    Align(HorizontalAlign),
}

/// Where an anchored object is down its reference.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum VerticalPlacement {
    /// This far from the reference's top edge, downwards.
    Offset(Pt),
    /// Aligned within the reference.
    Align(VerticalAlign),
}

/// How an anchored object aligns across its reference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HorizontalAlign {
    /// At the left.
    #[default]
    Left,
    /// Centred.
    Center,
    /// At the right.
    Right,
    /// On the inner side: left on odd pages, right on even ones.
    Inside,
    /// On the outer side.
    Outside,
}

/// How an anchored object aligns down its reference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VerticalAlign {
    /// At the top.
    #[default]
    Top,
    /// Centred.
    Center,
    /// At the bottom.
    Bottom,
    /// On the inner side.
    Inside,
    /// On the outer side.
    Outside,
}

/// How text wraps around an anchored object. Only [`None`](Self::None) is laid out yet:
/// with the others the text runs as if the object were not there, and that is reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Wrap {
    /// It does not: the object is in front of the text or behind it.
    #[default]
    None,
    /// Text is above and below, not beside it.
    TopAndBottom,
    /// Text flows around its bounding box.
    Square,
    /// Text flows around its outline.
    Tight,
    /// Text flows around its outline and into open parts of it.
    Through,
}

impl Check for HorizontalPlacement {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Offset(offset) => offset.check(checker),
            Self::Align(_) => Ok(()),
        }
    }
}

impl Check for VerticalPlacement {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Offset(offset) => offset.check(checker),
            Self::Align(_) => Ok(()),
        }
    }
}

impl Clamp for HorizontalPlacement {
    fn clamp(&mut self, _: &mut Clamper) {}
}

impl Clamp for VerticalPlacement {
    fn clamp(&mut self, _: &mut Clamper) {}
}

no_rules!(
    ImageFormat,
    Arc<[u8]>,
    HorizontalReference,
    VerticalReference,
    HorizontalAlign,
    VerticalAlign,
    Wrap
);
