//! Images, inline and anchored: the image data a document holds once, the pictures that
//! show it, and the position of floating objects.

use std::sync::Arc;

use officewright_core::{Color, Pt};

use super::macros::{ir_struct, no_rules};
use super::rules;
use super::walk::{Check, Checker, Clamp, Clamper};
use super::{Block, Border, ImageId};
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
    /// A picture: an image, cropped, turned and mirrored, filling the frame of the
    /// object that shows it ([`InlineObject`] or [`AnchoredObject`]).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Picture {
        /// The image.
        copy image: ImageId => with_image;
        /// The part of the image that is shown (`a:srcRect`, whose values are thousandths of a
        /// percent); `None` for all of it.
        copy crop: Option<Crop> => with_crop;
        /// The rotation clockwise, in degrees (`a:xfrm`'s `rot` counts 60000 to a degree,
        /// clockwise as in Word, MS-OI29500 2.1.1283 a). A negative value turns
        /// counter-clockwise; nothing brings it within a turn.
        copy rotation: f64 => with_rotation;
        /// Whether the picture is mirrored left to right.
        copy flip_horizontal: bool => with_flip_horizontal;
        /// Whether the picture is mirrored top to bottom.
        copy flip_vertical: bool => with_flip_vertical;
    }
}

impl Picture {
    /// The whole image, not turned or mirrored.
    pub fn new(image: ImageId) -> Self {
        Self {
            image,
            crop: None,
            rotation: 0.0,
            flip_horizontal: false,
            flip_vertical: false,
        }
    }
}

ir_struct! {
    /// A text box (`wps:txbx` in DrawingML, `v:textbox` in VML): text laid out in the frame
    /// of the object that shows it ([`InlineObject`] or [`AnchoredObject`]), inside its
    /// insets, on the box's fill and inside its outline. Its paragraphs are laid out as a
    /// small story of their own: their list labels count from the box's start. Tables in
    /// a box, and floating objects in its paragraphs, are left out and reported; so is text
    /// that is turned, runs vertically or flows on into a linked box, which the producer
    /// reports.
    ///
    /// A text box holds no text box: `Document::validate` refuses one, as Word holds none.
    ///
    /// ```
    /// use officewright_core::{Color, Pt};
    /// use officewright_layout::ir::{
    ///     AnchoredObject, Border, Document, HorizontalAnchor, HorizontalReference, Inline,
    ///     Insets, ObjectContent, PageSetup, Paragraph, RunStyle, Section, Size, TextBox,
    ///     TextBoxAnchor, VerticalAnchor, VerticalPlacement, VerticalReference,
    /// };
    ///
    /// let mut document = Document::default();
    /// let style = document.add_run_style(RunStyle::default());
    /// let address = Paragraph::new(style).with_inlines([Inline::text("Main Street 1", style)]);
    /// // An address in a box with a thin black outline, its text centred down the box.
    /// let text_box = TextBox::new()
    ///     .with_blocks([address.into()])
    ///     .with_insets(Insets::default().with_left(Pt(7.2)).with_right(Pt(7.2)))
    ///     .with_anchor(TextBoxAnchor::Center)
    ///     .with_fill(Some(Color::WHITE))
    ///     .with_outline(Some(Border::default().with_width(Pt(0.5))));
    /// let object = AnchoredObject::new(
    ///     ObjectContent::text_box(text_box),
    ///     Size::new(Pt(180.0), Pt(60.0)),
    ///     HorizontalAnchor::default().with_relative_to(HorizontalReference::Margin),
    ///     VerticalAnchor::default()
    ///         .with_relative_to(VerticalReference::Margin)
    ///         .with_placement(VerticalPlacement::Offset(Pt(36.0))),
    /// );
    /// let anchor = Paragraph::new(style).with_inlines([Inline::anchored(object)]);
    /// document.add_section(Section::new(PageSetup::default()).with_blocks([anchor.into()]));
    /// document.validate()?;
    /// # Ok::<(), officewright_layout::Error>(())
    /// ```
    #[derive(Clone, Debug, PartialEq)]
    pub struct TextBox {
        /// What the box holds, in reading order. Only its paragraphs are drawn yet.
        vec blocks: Block = Vec::new() => with_blocks;
        /// The room between the frame and the text on each side (`wps:bodyPr`'s `lIns`,
        /// `tIns`, `rIns` and `bIns`, `v:textbox`'s `inset`). The default is DrawingML's
        /// for attributes the document leaves out: 7.2 pt at the left and right, 3.6 pt at
        /// the top and bottom (ECMA-376 Part 1, §20.1.2.1.1).
        copy insets: Insets = Insets {
            left: Pt(7.2),
            top: Pt(3.6),
            right: Pt(7.2),
            bottom: Pt(3.6),
        } => with_insets, check insets_non_negative;
        /// Where the text is down the box when it is shorter than the box
        /// (`wps:bodyPr`'s `anchor`, `v:textbox`'s `v-text-anchor` in its style).
        copy anchor: TextBoxAnchor = TextBoxAnchor::Top => with_anchor;
        /// The colour the box is filled with, behind its text; `None` for none.
        copy fill: Option<Color> = None => with_fill;
        /// The line drawn along the frame, centred on it; `None` for none. Its `space` and
        /// `shadow` are not used.
        copy outline: Option<Border> = None => with_outline;
    }
}

impl TextBox {
    /// An empty box, with DrawingML's default insets, its text at the top, neither
    /// filled nor outlined.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Where the text of a text box is down the box (ST_TextAnchoringType, without `just` and
/// `dist`, which the producer passes as `Center` and `Top`). Text taller than the box
/// starts at its top and runs on past its bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextBoxAnchor {
    /// At the top.
    #[default]
    Top,
    /// Centred.
    Center,
    /// At the bottom.
    Bottom,
}

/// What a drawing object shows, in a line of text or floating.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ObjectContent {
    /// A picture.
    Picture(Picture),
    /// A text box, boxed so that it makes no inline larger ([`ObjectContent::text_box`]).
    TextBox(Box<TextBox>),
}

impl ObjectContent {
    /// A text box.
    pub fn text_box(text_box: TextBox) -> Self {
        Self::TextBox(Box::new(text_box))
    }
}

impl Check for ObjectContent {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Picture(picture) => picture.check(checker),
            Self::TextBox(text_box) => {
                checker.enter_text_box()?;
                text_box.check(checker)?;
                checker.leave_text_box();
                Ok(())
            }
        }
    }
}

impl Clamp for ObjectContent {
    fn clamp(&mut self, clamper: &mut Clamper) {
        match self {
            Self::Picture(picture) => picture.clamp(clamper),
            Self::TextBox(text_box) => {
                if clamper.enter_text_box() {
                    text_box.clamp(clamper);
                    clamper.leave_text_box();
                }
            }
        }
    }
}

ir_struct! {
    /// A drawing object in a line of text (`wp:inline`): a frame that sits on the
    /// baseline like a large character, with what it shows.
    #[derive(Clone, Debug, PartialEq)]
    pub struct InlineObject {
        /// What the object shows.
        ref content: ObjectContent => with_content;
        /// The size of its frame (`wp:extent`).
        copy size: Size => with_size;
        /// How far effects such as shadows reach beyond the frame on each side
        /// (`wp:effectExtent`); the object takes this much more room in the line. Word
        /// adds `l`, `r` and `t` at the left, right and top, where the standard says
        /// bottom (MS-OI29500 2.1.1352 a).
        copy effect_extent: Insets => with_effect_extent;
    }
}

impl InlineObject {
    /// An object showing this content in a frame of this size, without effects.
    pub fn new(content: ObjectContent, size: Size) -> Self {
        Self {
            content,
            size,
            effect_extent: Insets::default(),
        }
    }
}

ir_struct! {
    /// A width and a height. Lengths beyond what a file can hold (32-bit counts of
    /// EMUs, about 169,000 pt) are left to the layout, like any other size far beyond a
    /// page.
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
    /// reported. A position Word writes as `simplePos` the producer passes as offsets from
    /// the page on both axes.
    ///
    /// ```
    /// use officewright_core::Pt;
    /// use officewright_layout::ir::{
    ///     AnchorHorizontalAlign, AnchoredObject, Document, HorizontalAnchor,
    ///     HorizontalPlacement, HorizontalReference, ImageData, ImageFormat, Inline,
    ///     ObjectContent, PageSetup, Paragraph, Picture, RunStyle, Section, Size,
    ///     VerticalAnchor, VerticalPlacement, VerticalReference,
    /// };
    ///
    /// let mut document = Document::default();
    /// let style = document.add_run_style(RunStyle::default());
    /// let logo = document.add_image(ImageData::new(ImageFormat::Png, b"\x89PNG\r\n\x1a\n".to_vec()));
    /// // A logo at the right edge of the page, a quarter inch below the top edge.
    /// let object = AnchoredObject::new(
    ///     ObjectContent::Picture(Picture::new(logo)),
    ///     Size::new(Pt(120.0), Pt(36.0)),
    ///     HorizontalAnchor::default()
    ///         .with_relative_to(HorizontalReference::Page)
    ///         .with_placement(HorizontalPlacement::Align(AnchorHorizontalAlign::Right)),
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
        ref content: ObjectContent => with_content;
        /// The size of its frame (`wp:extent`).
        copy size: Size => with_size;
        /// How far effects reach beyond the frame on each side (`wp:effectExtent`, read as
        /// in [`InlineObject::effect_extent`]).
        copy effect_extent: Insets => with_effect_extent;
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
        /// The space kept free between it and wrapping text (`distT` and the others, 0 when
        /// the file leaves one out).
        copy distances: Insets => with_distances, check insets_non_negative;
    }
}

impl AnchoredObject {
    /// An object with this content, frame size and position, without effects, in front
    /// of the text, without wrapping, positioned within a table cell when anchored in
    /// one, allowed to overlap other objects, at the bottom of the stack and with no
    /// distances. WordprocessingML requires the wrapping, `behindDoc`, `layoutInCell`,
    /// `allowOverlap` and `relativeHeight`, so the producer passes the document's.
    pub fn new(
        content: ObjectContent,
        size: Size,
        horizontal: HorizontalAnchor,
        vertical: VerticalAnchor,
    ) -> Self {
        Self {
            content,
            size,
            effect_extent: Insets::default(),
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
    Align(AnchorHorizontalAlign),
}

/// Where an anchored object is down its reference.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum VerticalPlacement {
    /// This far from the reference's top edge, downwards.
    Offset(Pt),
    /// Aligned within the reference.
    Align(AnchorVerticalAlign),
}

/// How an anchored object aligns across its reference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnchorHorizontalAlign {
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
pub enum AnchorVerticalAlign {
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
    AnchorHorizontalAlign,
    AnchorVerticalAlign,
    TextBoxAnchor,
    Wrap
);
