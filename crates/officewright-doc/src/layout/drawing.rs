//! Drawings as IR objects: a picture in the line or floating, with its image added to
//! the IR once however many pictures show it. Drawings that show no picture (shapes,
//! charts, groups) are listed.

use officewright_core::Pt;
use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::Translator;
use crate::model::{
    Anchor, AnchorHorizontalAlign, AnchorVerticalAlign, Drawing, HorizontalReference, Image,
    Offsets, VerticalReference, WrapKind,
};

/// The PNG file signature.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The start of a JPEG file: a start of image marker and the next marker's 0xFF.
const JPEG: &[u8] = b"\xFF\xD8\xFF";

impl<'a> Translator<'a> {
    /// A drawing as an IR object in the line or anchored there, or none when it shows
    /// no picture or has no size; both are listed. A picture whose image the package does
    /// not have keeps its frame, empty, and is listed.
    pub(super) fn drawing(&mut self, drawing: Drawing<'a>) -> Option<ir::Inline> {
        let Some(picture) = drawing.picture() else {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "a drawing that shows no picture (a shape, chart or group) is left out".into()
            });
            return None;
        };
        let (Some(width), Some(height)) = (drawing.width(), drawing.height()) else {
            self.leave_out(ReportKind::FeatureUnsupported, || {
                "a picture without a size is left out".into()
            });
            return None;
        };
        let image = match picture.image() {
            Some(image) => self.image(picture.image_index(), image),
            None => {
                self.leave_out(ReportKind::FeatureUnsupported, || {
                    "a picture's image is not in the package: its frame is left empty".into()
                });
                self.missing_image()
            }
        };
        let crop = picture.crop().map(|crop| {
            ir::Crop::default()
                .with_left(crop.left())
                .with_top(crop.top())
                .with_right(crop.right())
                .with_bottom(crop.bottom())
        });
        let content = ir::ObjectContent::Picture(
            ir::Picture::new(image)
                .with_crop(crop)
                .with_rotation(picture.rotation())
                .with_flip_horizontal(picture.flip_horizontal())
                .with_flip_vertical(picture.flip_vertical()),
        );
        let size = ir::Size::new(width, height);
        let effect_extent = insets(drawing.effect_extent());
        let Some(anchor) = drawing.anchor() else {
            return Some(
                ir::InlineObject::new(content, size)
                    .with_effect_extent(effect_extent)
                    .into(),
            );
        };
        Some(ir::Inline::anchored(
            ir::AnchoredObject::new(content, size, horizontal(&anchor), vertical(&anchor))
                .with_effect_extent(effect_extent)
                .with_wrap(wrap(anchor.wrap()))
                .with_behind_text(anchor.behind_text())
                .with_layout_in_cell(anchor.layout_in_cell())
                .with_allow_overlap(anchor.allow_overlap())
                .with_z_order(anchor.z_order())
                .with_distances(insets(anchor.distances())),
        ))
    }

    /// The IR image of an image of the package, added the first time a picture shows it.
    fn image(&mut self, index: Option<u32>, image: Image<'a>) -> ir::ImageId {
        if let Some(id) = index.and_then(|index| self.images.get(&index)) {
            return *id;
        }
        let format = format(image.content_type(), image.bytes());
        let id = self
            .ir
            .add_image(ir::ImageData::new(format, image.shared()));
        if let Some(index) = index {
            self.images.insert(index, id);
        }
        id
    }

    /// One empty image for every picture whose image the package does not have, which
    /// the layout shows as a placeholder of the picture's size.
    fn missing_image(&mut self) -> ir::ImageId {
        if let Some(id) = self.missing_image {
            return id;
        }
        let id = self.ir.add_image(ir::ImageData::new(
            ir::ImageFormat::Other("missing".into()),
            Vec::new(),
        ));
        self.missing_image = Some(id);
        id
    }
}

/// The format of an image, by its signature whatever the package calls it, else by the
/// short name of its content type (`emf` for `image/x-emf`, `svg` for `image/svg+xml`).
fn format(content_type: &str, bytes: &[u8]) -> ir::ImageFormat {
    if bytes.starts_with(PNG) {
        return ir::ImageFormat::Png;
    }
    if bytes.starts_with(JPEG) {
        return ir::ImageFormat::Jpeg;
    }
    let name = signature(bytes).map_or_else(|| subtype(content_type), str::to_owned);
    ir::ImageFormat::Other(if name.is_empty() {
        "unknown".into()
    } else {
        name
    })
}

/// The short name of a format the layout does not draw, by its signature: GIF, BMP,
/// TIFF, EMF ([MS-EMF] 2.3.4.2, the header record with " EMF" at byte 40) and placeable
/// WMF ([MS-WMF] 2.3.2.3).
fn signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some("tiff")
    } else if bytes.starts_with(&[1, 0, 0, 0]) && bytes.get(40..44) == Some(b" EMF") {
        Some("emf")
    } else if bytes.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A]) {
        Some("wmf")
    } else {
        None
    }
}

/// The subtype of a content type, without `x-` and `+xml`, in lower case.
fn subtype(content_type: &str) -> String {
    let subtype = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let subtype = subtype.strip_prefix("x-").unwrap_or(&subtype);
    subtype.strip_suffix("+xml").unwrap_or(subtype).to_owned()
}

fn insets(offsets: Offsets) -> ir::Insets {
    ir::Insets::default()
        .with_left(offsets.left())
        .with_top(offsets.top())
        .with_right(offsets.right())
        .with_bottom(offsets.bottom())
}

/// Where a floating picture is across: by alignment if the file gives one, else by
/// offset; from the column when the file does not say from what.
fn horizontal(anchor: &Anchor) -> ir::HorizontalAnchor {
    let relative_to = match anchor.horizontal_reference() {
        Some(HorizontalReference::Character) => ir::HorizontalReference::Character,
        Some(HorizontalReference::InsideMargin) => ir::HorizontalReference::InsideMargin,
        Some(HorizontalReference::LeftMargin) => ir::HorizontalReference::LeftMargin,
        Some(HorizontalReference::Margin) => ir::HorizontalReference::Margin,
        Some(HorizontalReference::OutsideMargin) => ir::HorizontalReference::OutsideMargin,
        Some(HorizontalReference::Page) => ir::HorizontalReference::Page,
        Some(HorizontalReference::RightMargin) => ir::HorizontalReference::RightMargin,
        Some(HorizontalReference::Column) | None => ir::HorizontalReference::Column,
    };
    let placement = match anchor.horizontal_align() {
        Some(align) => ir::HorizontalPlacement::Align(match align {
            AnchorHorizontalAlign::Left => ir::AnchorHorizontalAlign::Left,
            AnchorHorizontalAlign::Right => ir::AnchorHorizontalAlign::Right,
            AnchorHorizontalAlign::Center => ir::AnchorHorizontalAlign::Center,
            AnchorHorizontalAlign::Inside => ir::AnchorHorizontalAlign::Inside,
            AnchorHorizontalAlign::Outside => ir::AnchorHorizontalAlign::Outside,
        }),
        None => ir::HorizontalPlacement::Offset(anchor.horizontal_offset().unwrap_or(Pt(0.0))),
    };
    ir::HorizontalAnchor::default()
        .with_relative_to(relative_to)
        .with_placement(placement)
}

/// Where a floating picture is down, as [`horizontal`] says; from the paragraph when the
/// file does not say from what.
fn vertical(anchor: &Anchor) -> ir::VerticalAnchor {
    let relative_to = match anchor.vertical_reference() {
        Some(VerticalReference::BottomMargin) => ir::VerticalReference::BottomMargin,
        Some(VerticalReference::InsideMargin) => ir::VerticalReference::InsideMargin,
        Some(VerticalReference::Line) => ir::VerticalReference::Line,
        Some(VerticalReference::Margin) => ir::VerticalReference::Margin,
        Some(VerticalReference::OutsideMargin) => ir::VerticalReference::OutsideMargin,
        Some(VerticalReference::Page) => ir::VerticalReference::Page,
        Some(VerticalReference::TopMargin) => ir::VerticalReference::TopMargin,
        Some(VerticalReference::Paragraph) | None => ir::VerticalReference::Paragraph,
    };
    let placement = match anchor.vertical_align() {
        Some(align) => ir::VerticalPlacement::Align(match align {
            AnchorVerticalAlign::Top => ir::AnchorVerticalAlign::Top,
            AnchorVerticalAlign::Bottom => ir::AnchorVerticalAlign::Bottom,
            AnchorVerticalAlign::Center => ir::AnchorVerticalAlign::Center,
            AnchorVerticalAlign::Inside => ir::AnchorVerticalAlign::Inside,
            AnchorVerticalAlign::Outside => ir::AnchorVerticalAlign::Outside,
        }),
        None => ir::VerticalPlacement::Offset(anchor.vertical_offset().unwrap_or(Pt(0.0))),
    };
    ir::VerticalAnchor::default()
        .with_relative_to(relative_to)
        .with_placement(placement)
}

/// How text wraps; not at all when the file does not say.
fn wrap(wrap: Option<WrapKind>) -> ir::Wrap {
    match wrap {
        Some(WrapKind::Square) => ir::Wrap::Square,
        Some(WrapKind::Tight) => ir::Wrap::Tight,
        Some(WrapKind::Through) => ir::Wrap::Through,
        Some(WrapKind::TopAndBottom) => ir::Wrap::TopAndBottom,
        Some(WrapKind::None) | None => ir::Wrap::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_images_by_their_signature_whatever_the_package_calls_them() {
        let other = |name: &str| ir::ImageFormat::Other(name.into());
        let octets = "application/octet-stream";
        // An EMF's header record says " EMF" at byte 40 ([MS-EMF] 2.3.4.2).
        let mut emf = vec![1, 0, 0, 0];
        emf.resize(40, 0);
        emf.extend_from_slice(b" EMF");
        let cases: [(&[u8], ir::ImageFormat); 8] = [
            (b"\x89PNG\r\n\x1a\n", ir::ImageFormat::Png),
            (b"\xFF\xD8\xFF\xE0", ir::ImageFormat::Jpeg),
            (b"GIF89a", other("gif")),
            (b"BM\0\0", other("bmp")),
            (b"II*\0", other("tiff")),
            (b"MM\0*", other("tiff")),
            (&emf, other("emf")),
            (&[0xD7, 0xCD, 0xC6, 0x9A], other("wmf")),
        ];
        for (bytes, expected) in cases {
            assert_eq!(format(octets, bytes), expected, "{bytes:?}");
        }
        // Without a signature, the content type: an EMF whose header lacks " EMF" is
        // named by it.
        assert_eq!(format("image/x-emf", &[1, 0, 0, 0]), other("emf"));
        assert_eq!(format("image/svg+xml", b"<svg"), other("svg"));
        assert_eq!(format("", b"?"), other("unknown"));
    }
}
