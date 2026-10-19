//! Drawings: pictures in the line and floating, and their images.

use super::*;

const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const PICTURE_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

/// The `a:graphic` of a picture of the image of relationship `id`, with these extra
/// children of `pic:blipFill` and of `pic:spPr`.
pub(super) fn graphic(id: &str, fill: &str, shape: &str) -> String {
    format!(
        r#"<a:graphic xmlns:a="{A}"><a:graphicData uri="{PICTURE_URI}"><pic:pic xmlns:pic="{PIC}"><pic:nvPicPr><pic:cNvPr id="0" name="p"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{id}"/>{fill}<a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr>{shape}<a:prstGeom prst="rect"/></pic:spPr></pic:pic></a:graphicData></a:graphic>"#
    )
}

/// A run with a picture in the line, 144 by 72 pt, of the image of relationship `id`.
pub(super) fn inline_picture(id: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline xmlns:wp="{WP}" distT="0" distB="0" distL="0" distR="0"><wp:extent cx="1828800" cy="914400"/><wp:effectExtent l="0" t="25400" r="25400" b="0"/><wp:docPr id="1" name="Picture 1"/>{}</wp:inline></w:drawing></w:r>"#,
        graphic(
            id,
            r#"<a:srcRect l="10000" b="-5000"/>"#,
            r#"<a:xfrm rot="-5400000" flipH="1"><a:off x="0" y="0"/><a:ext cx="1828800" cy="914400"/></a:xfrm>"#
        )
    )
}

/// A run with a floating picture, 100 by 40 pt, of the image of relationship `id`: on
/// the right of the page, 10 pt above the top margin (the fallback of a position Word
/// 2010 writes in percent), behind the text, with square wrapping.
pub(super) fn floating_picture(id: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:anchor xmlns:wp="{WP}" xmlns:wp14="http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing" distT="0" distB="0" distL="114300" distR="-5" simplePos="0" relativeHeight="251659264" behindDoc="1" locked="0" layoutInCell="0" allowOverlap="false"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:align>right</wp:align></wp:positionH><mc:AlternateContent><mc:Choice Requires="wp14"><wp:positionV relativeFrom="margin"><wp14:pctPosVOffset>50000</wp14:pctPosVOffset></wp:positionV></mc:Choice><mc:Fallback><wp:positionV relativeFrom="topMargin"><wp:posOffset>-127000</wp:posOffset></wp:positionV></mc:Fallback></mc:AlternateContent><wp:extent cx="1270000" cy="508000"/><wp:effectExtent l="1in" t="0" r="0" b="0"/><wp:wrapSquare wrapText="bothSides"/><wp:docPr id="2" name="Logo"/>{}</wp:anchor></w:drawing></w:r>"#,
        graphic(id, "", "")
    )
}

/// A run with a floating picture of the image of relationship `id` that writes nothing
/// it may leave out: no attributes on `wp:anchor`, positions without `relativeFrom`, no
/// wrapping.
pub(super) fn bare_floating_picture(id: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:anchor xmlns:wp="{WP}"><wp:simplePos x="0" y="0"/><wp:positionH><wp:posOffset>12700</wp:posOffset></wp:positionH><wp:positionV><wp:posOffset>25400</wp:posOffset></wp:positionV><wp:extent cx="12700" cy="12700"/><wp:docPr id="4" name="Bare"/>{}</wp:anchor></w:drawing></w:r>"#,
        graphic(id, "", "")
    )
}

/// A run with a picture in the line whose width is negative.
pub(super) fn picture_of_negative_width(id: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline xmlns:wp="{WP}"><wp:extent cx="-12700" cy="12700"/><wp:docPr id="5" name="Negative"/>{}</wp:inline></w:drawing></w:r>"#,
        graphic(id, "", "")
    )
}

/// A run with a group in the line: a picture among its parts is not the drawing's.
pub(super) fn group(id: &str) -> String {
    let wpg = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
    format!(
        r#"<w:r><w:drawing><wp:inline xmlns:wp="{WP}"><wp:extent cx="12700" cy="12700"/><wp:docPr id="3" name="Group"/><a:graphic xmlns:a="{A}"><a:graphicData uri="{wpg}"><wpg:wgp xmlns:wpg="{wpg}"><pic:pic xmlns:pic="{PIC}"><pic:blipFill><a:blip r:embed="{id}"/></pic:blipFill></pic:pic></wpg:wgp></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

/// The relationships of the main part to the images of the tests: a PNG, a JPEG the
/// package calls `application/octet-stream`, an EMF, a GIF without a content type, an
/// image the package does not have, a linked image, and a header that shows the PNG
/// too.
pub(super) fn image_relationships() -> String {
    let image = |id: &str, target: &str| {
        format!(r#"<Relationship Id="{id}" Type="{IMAGE}" Target="{target}"/>"#)
    };
    [
        image("rIdPng", "media/image1.png"),
        image("rIdJpeg", "media/photo.bin"),
        image("rIdEmf", "media/image3.emf"),
        image("rIdGif", "media/animation"),
        image("rIdGone", "media/gone.png"),
        image("rIdGifAgain", "media/animation"),
        image("rIdWeb", "http://example.com/b.png"),
        r#"<Relationship Id="rIdLink" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="http://example.com/a.png" TargetMode="External"/>"#.to_owned(),
        r#"<Relationship Id="rIdHeader" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>"#.to_owned(),
    ]
    .concat()
}

/// A document with the images of [`image_relationships`] and `body`, whose last section
/// has the header, which shows the PNG through a relationship of its own.
pub(super) fn with_images(body: &str) -> Document {
    Document::from_bytes(images_package(body)).unwrap()
}

/// The package [`with_images`] opens.
fn images_package(body: &str) -> Vec<u8> {
    let header = format!(
        r#"<w:hdr xmlns:w="{W}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:p>{}</w:p></w:hdr>"#,
        inline_picture("rId1")
    );
    let header_rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{IMAGE}" Target="media/image1.png"/></Relationships>"#
    );
    let body = format!(
        r#"{body}<w:sectPr><w:headerReference w:type="default" r:id="rIdHeader"/></w:sectPr>"#
    );
    files_package(
        &body,
        &image_relationships(),
        &[
            ("word/media/image1.png", PNG_BYTES),
            ("word/media/photo.bin", JPEG_BYTES),
            ("word/media/image3.emf", b"\x01\0\0\0emf"),
            ("word/media/animation", b"GIF89a\x01\0\x01\0"),
            ("word/header1.xml", header.as_bytes()),
            ("word/_rels/header1.xml.rels", header_rels.as_bytes()),
        ],
        r#"<Default Extension="png" ContentType="image/png"/><Default Extension="bin" ContentType="application/octet-stream"/><Default Extension="emf" ContentType="image/x-emf"/>"#,
    )
}

/// The drawings of the body's runs, in order.
fn drawings(document: &Document) -> Vec<Drawing<'_>> {
    super::format::paragraphs(document)
        .into_iter()
        .flat_map(|paragraph| paragraph.runs())
        .flat_map(|run| run.content())
        .filter_map(|item| match item {
            RunContent::Drawing(drawing) => Some(drawing),
            _ => None,
        })
        .collect()
}

#[test]
fn reads_pictures_in_the_line_and_floating() {
    let body = format!(
        "<w:p>{}{}{}</w:p>",
        inline_picture("rIdPng"),
        floating_picture("rIdPng"),
        group("rIdJpeg")
    );
    let document = with_images(&body);
    let drawings = drawings(&document);
    assert_eq!(drawings.len(), 3);

    let inline = drawings[0];
    assert!(inline.is_inline());
    assert_eq!(
        (inline.width(), inline.height()),
        (Some(Pt(144.0)), Some(Pt(72.0)))
    );
    let extent = inline.effect_extent();
    assert_eq!(
        (extent.left(), extent.top(), extent.right(), extent.bottom()),
        (Pt(0.0), Pt(2.0), Pt(2.0), Pt(0.0))
    );
    assert_eq!(inline.anchor(), None);
    let picture = inline.picture().unwrap();
    let crop = picture.crop().unwrap();
    assert_eq!(
        (crop.left(), crop.top(), crop.right(), crop.bottom()),
        (0.1, 0.0, 0.0, -0.05)
    );
    assert_eq!(picture.rotation(), -90.0);
    assert!(picture.flip_horizontal());
    assert!(!picture.flip_vertical());
    let image = picture.image().unwrap();
    assert_eq!(
        (image.content_type(), image.bytes()),
        ("image/png", PNG_BYTES)
    );

    let floating = drawings[1];
    assert!(!floating.is_inline());
    assert_eq!(
        (floating.width(), floating.height()),
        (Some(Pt(100.0)), Some(Pt(40.0)))
    );
    // A universal measure, as Strict writes ST_Coordinate.
    assert_eq!(floating.effect_extent().left(), Pt(72.0));
    let anchor = floating.anchor().unwrap();
    assert_eq!(
        anchor.horizontal_reference(),
        Some(HorizontalReference::Page)
    );
    assert_eq!(
        anchor.horizontal_align(),
        Some(AnchorHorizontalAlign::Right)
    );
    assert_eq!(anchor.horizontal_offset(), None);
    // The fallback, as the reader does not implement wp14.
    assert_eq!(
        anchor.vertical_reference(),
        Some(VerticalReference::TopMargin)
    );
    assert_eq!(anchor.vertical_align(), None);
    assert_eq!(anchor.vertical_offset(), Some(Pt(-10.0)));
    assert_eq!(anchor.wrap(), Some(WrapKind::Square));
    assert!(anchor.behind_text());
    assert!(!anchor.layout_in_cell());
    assert!(!anchor.allow_overlap());
    assert_eq!(anchor.z_order(), 251_659_264);
    let distances = anchor.distances();
    // distR is unsigned: -5 is not a distance.
    assert_eq!(
        (
            distances.left(),
            distances.top(),
            distances.right(),
            distances.bottom()
        ),
        (Pt(9.0), Pt(0.0), Pt(0.0), Pt(0.0))
    );
    assert!(floating.picture().unwrap().crop().is_none());

    // A group shows no picture of its own, and the image of the picture among its parts
    // is not read.
    assert!(drawings[2].picture().is_none());
    assert!(drawings[2].is_inline());

    // The PNG is read once for both pictures and the header's.
    assert_eq!(document.images.len(), 1);
    let header = document
        .sections()
        .next()
        .unwrap()
        .header(HeaderFooterKind::Default)
        .unwrap();
    let Block::Paragraph(paragraph) = header.blocks().next().unwrap() else {
        panic!("a paragraph");
    };
    let shown = paragraph
        .runs()
        .flat_map(|run| run.content())
        .find_map(|item| match item {
            RunContent::Drawing(drawing) => drawing.picture()?.image(),
            _ => None,
        })
        .unwrap();
    assert!(std::ptr::eq(shown.bytes(), image.bytes()));
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reads_pictures_whose_image_is_missing_or_linked() {
    let body = format!(
        "<w:p>{}{}{}{}</w:p>",
        inline_picture("rIdNone"),
        inline_picture("rIdGone"),
        inline_picture("rIdLink"),
        inline_picture("rIdJpeg")
    );
    let document = with_images(&body);
    let drawings = drawings(&document);
    let images: Vec<Option<&str>> = drawings
        .iter()
        .map(|drawing| Some(drawing.picture()?.image()?.content_type()))
        .collect();
    // The image the package calls octet-stream is still read; the layout knows it by
    // its signature.
    assert_eq!(images, [None, None, None, Some("application/octet-stream")]);
    let messages: Vec<String> = document
        .diagnostics()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        messages[0]
            .ends_with("a picture names the relationship rIdNone, which the part does not have")
    );
    assert!(messages[1].ends_with(
        "a picture shows the image /word/media/gone.png, which the package does not have"
    ));
}

#[test]
fn reads_what_a_floating_picture_leaves_out_and_a_negative_size() {
    let body = format!(
        "<w:p>{}{}</w:p>",
        bare_floating_picture("rIdPng"),
        picture_of_negative_width("rIdPng")
    );
    let document = with_images(&body);
    let drawings = drawings(&document);
    let anchor = drawings[0].anchor().unwrap();
    assert_eq!(
        (anchor.horizontal_reference(), anchor.horizontal_offset()),
        (None, Some(Pt(1.0)))
    );
    assert_eq!(
        (anchor.vertical_reference(), anchor.vertical_offset()),
        (None, Some(Pt(2.0)))
    );
    assert_eq!(anchor.wrap(), None);
    // ECMA-376 Part 1, §20.4.2.3 requires layoutInCell and allowOverlap; missing, they
    // are taken as on, our choice, and the others as off.
    assert!(anchor.layout_in_cell() && anchor.allow_overlap());
    assert!(!anchor.behind_text());
    assert_eq!(anchor.z_order(), 0);
    assert_eq!(anchor.distances(), Offsets::default());
    // ST_PositiveCoordinate: a negative width is no width.
    assert_eq!(
        (drawings[1].width(), drawings[1].height()),
        (None, Some(Pt(1.0)))
    );
}

#[test]
fn reports_an_image_that_cannot_be_read_once_and_reads_the_rest() {
    // Two pictures of one damaged image: the package's check of the entry fails, which
    // is reported once; the document still opens.
    let body = format!(
        "<w:p>{}{}{}{}</w:p>",
        inline_picture("rIdGif"),
        inline_picture("rIdGif"),
        inline_picture("rIdGifAgain"),
        inline_picture("rIdPng")
    );
    let mut package = images_package(&body);
    let at = package
        .windows(6)
        .position(|window| window == b"GIF89a")
        .unwrap();
    package[at + 6] ^= 0xFF;
    let document = Document::from_bytes(package).unwrap();
    let images: Vec<bool> = drawings(&document)
        .iter()
        .map(|drawing| drawing.picture().unwrap().image().is_some())
        .collect();
    // Through a second relationship too, the damaged part is not read again.
    assert_eq!(images, [false, false, false, true]);
    let messages: Vec<String> = document
        .diagnostics()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("a picture's image cannot be read: ")
            && messages[0].contains("word/media/animation"),
        "{messages:?}"
    );
}

#[test]
fn stops_at_a_limit_of_the_package_while_reading_images() {
    // The only picture is the header's, so its image is the last part read: below the
    // least total size of the parts read that opens the document, the limit stops the
    // reading rather than being reported as an image that cannot be read.
    let package = images_package("<w:p/>");
    let opened = |total| {
        LoadOptions::new()
            .with_limits(Limits::new().with_max_total_size(total))
            .open_bytes(package.clone())
    };
    let (mut low, mut high) = (0, 1 << 20);
    while low + 1 < high {
        let middle = (low + high) / 2;
        if opened(middle).is_ok() {
            high = middle;
        } else {
            low = middle;
        }
    }
    let err = opened(high - 1).unwrap_err();
    assert!(matches!(err, Error::LimitExceeded { .. }), "{err}");
    let document = opened(high).unwrap();
    let header = document
        .sections()
        .next()
        .unwrap()
        .header(HeaderFooterKind::Default)
        .unwrap();
    let Block::Paragraph(paragraph) = header.blocks().next().unwrap() else {
        panic!("a paragraph");
    };
    let image = paragraph
        .runs()
        .flat_map(|run| run.content())
        .find_map(|item| match item {
            RunContent::Drawing(drawing) => drawing.picture()?.image(),
            _ => None,
        });
    assert_eq!(image.map(Image::bytes), Some(PNG_BYTES));
}

/// A run with a floating picture of the image of relationship `id`, whose `wp:anchor` has
/// these attributes and these children before its `wp:docPr`.
pub(super) fn anchored(id: &str, attributes: &str, children: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:anchor xmlns:wp="{WP}" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1" {attributes}>{children}<wp:docPr id="1" name="p"/>{}</wp:anchor></w:drawing></w:r>"#,
        graphic(id, "", "")
    )
}

/// A length of 308 digits and a unit, which is infinite as a number of points.
pub(super) fn endless() -> String {
    format!("{}in", "1".repeat(308))
}

#[test]
fn reads_lengths_in_every_unit_and_within_a_coordinate_s_range() {
    let endless = endless();
    let children = format!(
        r#"<wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>{endless}</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:posOffset>6pi</wp:posOffset></wp:positionV><wp:extent cx="{endless}" cy="72pt"/><wp:effectExtent l="{endless}" t="2.54cm" r="25.4mm" b="6pc"/><wp:wrapNone/>"#
    );
    let attributes = format!(r#"simplePos="0" distT="{endless}" distB="1in" distL="0" distR="0""#);
    let document = with_images(&format!(
        "<w:p>{}</w:p>",
        anchored("rIdPng", &attributes, &children)
    ));
    let drawing = drawings(&document)[0];
    let near = |length: Pt, points: f64| (length.0 - points).abs() < 1e-9;
    // Beyond ST_Coordinate's range a length is none, as one not written.
    assert_eq!(drawing.width(), None);
    assert!(near(drawing.height().unwrap(), 72.0));
    let effect = drawing.effect_extent();
    assert_eq!(effect.left(), Pt(0.0));
    for side in [effect.top(), effect.right(), effect.bottom()] {
        assert!(near(side, 72.0), "{side:?}");
    }
    let anchor = drawing.anchor().unwrap();
    assert_eq!(anchor.horizontal_offset(), None);
    assert!(near(anchor.vertical_offset().unwrap(), 72.0));
    assert_eq!(anchor.distances().top(), Pt(0.0));
    assert!(near(anchor.distances().bottom(), 72.0));
}

#[test]
fn places_a_picture_by_its_simple_position_when_the_anchor_says_so() {
    let children = |x: &str| {
        format!(
            r#"<wp:simplePos x="{x}" y="1in"/><wp:positionH relativeFrom="column"><wp:align> center </wp:align></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>12700</wp:posOffset></wp:positionV><wp:extent cx="12700" cy="12700"/><wp:wrapNone/>"#
        )
    };
    let document = with_images(&format!(
        "<w:p>{}{}{}</w:p>",
        anchored("rIdPng", r#"simplePos="1""#, &children("10cm")),
        anchored("rIdPng", r#"simplePos="0""#, &children("10cm")),
        anchored("rIdPng", r#"simplePos="1""#, &children("wide")),
    ));
    let anchors: Vec<Anchor> = drawings(&document)
        .iter()
        .map(|drawing| drawing.anchor().unwrap())
        .collect();
    // On: wp:simplePos, from the page's top left corner (ECMA-376 Part 1, §20.4.2.3).
    let on = anchors[0];
    assert_eq!(on.horizontal_reference(), Some(HorizontalReference::Page));
    assert_eq!(on.horizontal_align(), None);
    assert!((on.horizontal_offset().unwrap().0 - 100.0 * 72.0 / 25.4).abs() < 1e-9);
    assert_eq!(on.vertical_reference(), Some(VerticalReference::Page));
    assert_eq!(on.vertical_offset(), Some(Pt(72.0)));
    // Off, or a simple position that is not one: wp:positionH and wp:positionV, whose
    // alignment is read without the spaces around it.
    for anchor in [anchors[1], anchors[2]] {
        assert_eq!(
            anchor.horizontal_reference(),
            Some(HorizontalReference::Column)
        );
        assert_eq!(
            anchor.horizontal_align(),
            Some(AnchorHorizontalAlign::Center)
        );
        assert_eq!(
            anchor.vertical_reference(),
            Some(VerticalReference::Paragraph)
        );
        assert_eq!(anchor.vertical_offset(), Some(Pt(1.0)));
    }
}

#[test]
fn reads_a_strict_picture() {
    let xml = r#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main" xmlns:r="http://purl.oclc.org/ooxml/officeDocument/relationships" conformance="strict"><w:body><w:p><w:r><w:drawing><wp:inline xmlns:wp="http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing"><wp:extent cx="2in" cy="1in"/><wp:docPr id="1" name="p"/><a:graphic xmlns:a="http://purl.oclc.org/ooxml/drawingml/main"><a:graphicData uri="http://purl.oclc.org/ooxml/drawingml/picture"><pic:pic xmlns:pic="http://purl.oclc.org/ooxml/drawingml/picture"><pic:blipFill><a:blip r:embed="rIdNone"/><a:srcRect l="10%" r="-5.5%"/></pic:blipFill><pic:spPr/></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p></w:body></w:document>"#;
    let document = Document::from_bytes(package(DOCUMENT, xml, &[])).unwrap();
    let drawing = drawings(&document)[0];
    assert!(drawing.is_inline());
    assert_eq!(
        (drawing.width(), drawing.height()),
        (Some(Pt(144.0)), Some(Pt(72.0)))
    );
    let crop = drawing.picture().unwrap().crop().unwrap();
    assert!((crop.left() - 0.1).abs() < 1e-12);
    assert!((crop.right() + 0.055).abs() < 1e-12);
}

#[test]
fn reports_an_image_target_that_is_no_part() {
    // An internal relationship whose target is no part name is reported; an external
    // one is never loaded and not reported.
    let document = with_images(&format!(
        "<w:p>{}{}</w:p>",
        inline_picture("rIdWeb"),
        inline_picture("rIdLink")
    ));
    let messages: Vec<String> = document
        .diagnostics()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0]
            .ends_with("a picture's image http://example.com/b.png is not a part of the package"),
        "{messages:?}"
    );
}

#[test]
fn shows_a_drawing_as_written_and_a_picture_briefly() {
    let document = with_images(&format!("<w:p>{}</w:p>", group("rIdPng")));
    let drawing = drawings(&document)[0];
    let xml = drawing.to_xml();
    assert!(xml.starts_with("<w:drawing"), "{xml}");
    assert!(xml.contains("wpg:wgp"), "{xml}");
    let document = with_images(&format!("<w:p>{}</w:p>", inline_picture("rIdPng")));
    let picture = drawings(&document)[0].picture().unwrap();
    let debug = format!("{picture:?}");
    assert!(
        debug.starts_with("Picture {") && debug.len() < 300,
        "{debug}"
    );
}

#[test]
fn reads_lengths_and_shares_only_of_the_schema_s_pattern_and_range() {
    // ST_Coordinate's range in EMUs is -27273042329600 to 27273042316900: the edges are
    // read, one past them is not, nor 400,000,000 inches.
    let width = |cx: &str| {
        let document = with_images(&format!(
            "<w:p>{}</w:p>",
            anchored(
                "rIdPng",
                r#"simplePos="0""#,
                &format!(
                    r#"<wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>{cx}</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="12700" cy="12700"/><wp:wrapNone/>"#
                )
            )
        ));
        drawings(&document)[0].anchor().unwrap().horizontal_offset()
    };
    assert_eq!(
        width("27273042316900"),
        Some(Pt(27_273_042_316_900.0 / 12_700.0))
    );
    assert_eq!(
        width("-27273042329600"),
        Some(Pt(-27_273_042_329_600.0 / 12_700.0))
    );
    for value in [
        "27273042316901",
        "-27273042329601",
        "400000000in",
        // Not of the pattern -?[0-9]+(\.[0-9]+)? before the unit.
        "1e2in",
        "+1in",
        ".5in",
        "1.in",
        "infin",
    ] {
        assert_eq!(width(value), None, "{value}");
    }
    // A crop in percent of the same pattern; others are not read.
    let crop = |l: &str| {
        let fill = format!(r#"<a:srcRect l="{l}"/>"#);
        let picture = format!(
            r#"<w:r><w:drawing><wp:inline xmlns:wp="{WP}"><wp:extent cx="12700" cy="12700"/><wp:docPr id="1" name="p"/>{}</wp:inline></w:drawing></w:r>"#,
            graphic("rIdPng", &fill, "")
        );
        let document = with_images(&format!("<w:p>{picture}</w:p>"));
        drawings(&document)[0]
            .picture()
            .unwrap()
            .crop()
            .unwrap()
            .left()
    };
    assert!((crop("12.5%") - 0.125).abs() < 1e-12);
    for value in ["1e3%", "inf%", "+5%", ".5%"] {
        assert_eq!(crop(value), 0.0, "{value}");
    }
    // A simple position without y is not one: the other position counts.
    let document = with_images(&format!(
        "<w:p>{}</w:p>",
        anchored(
            "rIdPng",
            r#"simplePos="1""#,
            r#"<wp:simplePos x="12700"/><wp:positionH relativeFrom="column"><wp:posOffset>25400</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>38100</wp:posOffset></wp:positionV><wp:extent cx="12700" cy="12700"/><wp:wrapNone/>"#
        )
    ));
    let anchor = drawings(&document)[0].anchor().unwrap();
    assert_eq!(
        (anchor.horizontal_reference(), anchor.horizontal_offset()),
        (Some(HorizontalReference::Column), Some(Pt(2.0)))
    );
    assert_eq!(
        (anchor.vertical_reference(), anchor.vertical_offset()),
        (Some(VerticalReference::Paragraph), Some(Pt(3.0)))
    );
}

#[test]
fn reads_drawings_through_markup_compatibility() {
    // Issue #312: what lies below w:drawing goes through Markup Compatibility as the
    // rest of the body does, since the namespaces the picture reader reads are
    // understood.
    let picture = inline_picture("rIdPng");
    for (from, to) in [
        ("<a:blip ", r#"<a:blip mc:MustUnderstand="w14" "#),
        ("<wp:inline ", r#"<wp:inline mc:MustUnderstand="w14" "#),
        ("<pic:pic ", r#"<pic:pic mc:MustUnderstand="w14" "#),
    ] {
        let body = format!("<w:p>{}</w:p>", picture.replace(from, to));
        match Document::from_bytes(images_package(&body)) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("mc:MustUnderstand"), "{to}: {message}")
            }
            other => panic!("{to} gave {other:?}"),
        }
    }
    // An ignorable wrapper around pic:pic whose content mc:ProcessContent names is read
    // through; one that nothing names is left out, with the picture in it.
    let wrapped = picture
        .replace("<pic:pic ", "<w14:x><pic:pic ")
        .replace("</pic:pic>", "</pic:pic></w14:x>");
    let document = with_images(&format!(
        r#"<w:p mc:ProcessContent="w14:x">{wrapped}</w:p>"#
    ));
    let image = drawings(&document)[0]
        .picture()
        .and_then(|picture| picture.image());
    assert_eq!(image.map(|image| image.bytes()), Some(PNG_BYTES));
    let document = with_images(&format!("<w:p>{wrapped}</w:p>"));
    assert!(drawings(&document)[0].picture().is_none());

    // The same in Strict, on each element of each of the three namespaces.
    let strict = r#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main" xmlns:r="http://purl.oclc.org/ooxml/officeDocument/relationships" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" mc:Ignorable="w14" conformance="strict"><w:body><w:p><w:r><w:drawing><wp:inline xmlns:wp="http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing"WP><wp:extent cx="2in" cy="1in"/><wp:docPr id="1" name="p"/><a:graphic xmlns:a="http://purl.oclc.org/ooxml/drawingml/main"><a:graphicData uri="http://purl.oclc.org/ooxml/drawingml/picture"><pic:pic xmlns:pic="http://purl.oclc.org/ooxml/drawingml/picture"PIC><pic:blipFill><a:blip r:embed="rIdNone"A/></pic:blipFill><pic:spPr/></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p></w:body></w:document>"#;
    let must = r#" mc:MustUnderstand="w14""#;
    for at in ["WP", "PIC", "A"] {
        let mut xml = strict.replace(&format!("\"{at}>"), &format!("\"{must}>"));
        xml = xml.replace(&format!("\"{at}/>"), &format!("\"{must}/>"));
        for other in ["WP", "PIC", "A"] {
            xml = xml
                .replace(&format!("\"{other}>"), "\">")
                .replace(&format!("\"{other}/>"), "\"/>");
        }
        match Document::from_bytes(package(DOCUMENT, &xml, &[])) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("mc:MustUnderstand"), "{at}: {message}")
            }
            other => panic!("{at} gave {other:?}"),
        }
    }
}

#[test]
fn reads_drawings_through_markup_compatibility_everywhere_they_are() {
    // From the review of #316: in a header as in the body, on a floating picture as on
    // one in the line, and for what Markup Compatibility refuses besides an
    // mc:MustUnderstand.
    let must = r#"mc:MustUnderstand="w14" "#;
    let refused = |document: Result<Document>, case: &str| match document {
        Err(Error::Unsupported(message)) => {
            assert!(message.contains("mc:MustUnderstand"), "{case}: {message}")
        }
        other => panic!("{case} gave {other:?}"),
    };
    // A picture in a header whose a:blip must be understood.
    let header = format!(
        r#"<w:hdr xmlns:w="{W}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" mc:Ignorable="w14"><w:p>{}</w:p></w:hdr>"#,
        inline_picture("rId1").replace("<a:blip ", &format!("<a:blip {must}"))
    );
    let header_rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{IMAGE}" Target="media/image1.png"/></Relationships>"#
    );
    let package = files_package(
        r#"<w:p/><w:sectPr><w:headerReference w:type="default" r:id="rIdHeader"/></w:sectPr>"#,
        &image_relationships(),
        &[
            ("word/media/image1.png", PNG_BYTES),
            ("word/header1.xml", header.as_bytes()),
            ("word/_rels/header1.xml.rels", header_rels.as_bytes()),
        ],
        r#"<Default Extension="png" ContentType="image/png"/>"#,
    );
    refused(Document::from_bytes(package), "a header");
    // A floating picture whose wp:anchor must be understood.
    let anchored = floating_picture("rIdPng").replace("<wp:anchor ", &format!("<wp:anchor {must}"));
    refused(
        Document::from_bytes(images_package(&format!("<w:p>{anchored}</w:p>"))),
        "wp:anchor",
    );
    // A choice that requires wp is taken in a drawing, not its fallback.
    let position =
        |offset: i64| format!("<wp:positionV><wp:posOffset>{offset}</wp:posOffset></wp:positionV>");
    let chosen = bare_floating_picture("rIdPng").replace(
        &position(25400),
        &format!(
            r#"<mc:AlternateContent><mc:Choice Requires="wp">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
            position(12700),
            position(25400)
        ),
    );
    let document = with_images(&format!("<w:p>{chosen}</w:p>"));
    let anchor = drawings(&document)[0].anchor().unwrap();
    assert_eq!(anchor.vertical_offset(), Some(Pt(1.0)));
    // Below a:blip, a Requires that names an undeclared prefix and an attribute of the mc
    // namespace that Markup Compatibility does not define refuse the document.
    for (case, blip) in [
        (
            "Requires",
            r#"<a:blip r:embed="rIdPng"><mc:AlternateContent><mc:Choice Requires="zz"><a:extLst/></mc:Choice></mc:AlternateContent></a:blip>"#,
        ),
        ("mc:Foo", r#"<a:blip r:embed="rIdPng" mc:Foo="1"/>"#),
    ] {
        let picture = inline_picture("rIdPng").replace(r#"<a:blip r:embed="rIdPng"/>"#, blip);
        assert!(
            matches!(
                Document::from_bytes(images_package(&format!("<w:p>{picture}</w:p>"))),
                Err(Error::Malformed { .. })
            ),
            "{case}"
        );
    }
}

#[test]
fn lists_a_picture_s_outline_and_colour_effects_once() {
    // Issue #303: the IR's picture has no outline and no colour effects yet, so the
    // report says so, once per document; an outline of no fill and a blip with only
    // its extensions are nothing to list.
    let entries = |body: &str| -> Vec<String> {
        with_images(body)
            .to_layout_ir()
            .left_out()
            .iter()
            .map(|entry| entry.message().to_owned())
            .filter(|message| message.starts_with("a picture's"))
            .collect()
    };
    let picture = |line: &str, blip: &str| {
        inline_picture("rIdPng")
            .replace(
                r#"<a:blip r:embed="rIdPng"/>"#,
                &format!(r#"<a:blip r:embed="rIdPng">{blip}</a:blip>"#),
            )
            .replace(
                r#"<a:prstGeom prst="rect"/>"#,
                &format!(r#"<a:prstGeom prst="rect"/>{line}"#),
            )
    };
    let red = r#"<a:ln w="76200"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln>"#;
    let body = format!(
        "<w:p>{}{}</w:p>",
        picture(red, "<a:grayscl/>"),
        picture(red, r#"<a:lum bright="20000"/>"#)
    );
    assert_eq!(
        entries(&body),
        [
            "a picture's outline (a:ln) is not drawn, here and wherever else the document has one",
            "a picture's colour or transparency effects (a:blip, such as a:grayscl or a:lum) are not applied, here and wherever else the document has them",
        ]
    );
    let plain = picture("<a:ln><a:noFill/></a:ln>", "<a:extLst/>");
    assert!(entries(&format!("<w:p>{plain}</w:p>")).is_empty());
}

#[test]
fn lists_a_picture_s_effects_and_shape_once_but_no_value_that_changes_nothing() {
    // From the re-check of #326: the shadows, reflections, glows, soft edges, 3-D effects
    // and shapes Word's picture styles write have no place in the IR's picture yet. An
    // empty a:effectLst, a camera in front of the picture, 3-D without depth and colour
    // effects whose values change nothing (LibreOffice's fdo43573-2-min.docx has
    // <a:lum/>) are nothing to list.
    let entries = |geometry: &str, effects: &str, blip: &str| -> Vec<String> {
        let picture = inline_picture("rIdPng")
            .replace(
                r#"<a:blip r:embed="rIdPng"/>"#,
                &format!(r#"<a:blip r:embed="rIdPng">{blip}</a:blip>"#),
            )
            .replace(
                r#"<a:prstGeom prst="rect"/>"#,
                &format!("{geometry}{effects}"),
            );
        with_images(&format!("<w:p>{picture}</w:p>"))
            .to_layout_ir()
            .left_out()
            .iter()
            .map(|entry| entry.message().to_owned())
            .filter(|message| message.starts_with("a picture"))
            .collect()
    };
    let rect = r#"<a:prstGeom prst="rect"/>"#;
    let front = r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d>"#;
    let effects = "a picture's shadow, reflection, glow, soft edges or 3-D effect (a:effectLst, a:scene3d, a:sp3d) is not drawn, here and wherever else the document has one";
    for listed in [
        r#"<a:effectLst><a:outerShdw blurRad="50800" dist="38100" dir="2700000"><a:srgbClr val="000000"/></a:outerShdw></a:effectLst>"#,
        r#"<a:effectLst><a:softEdge rad="112500"/></a:effectLst>"#,
        r#"<a:effectDag><a:glow rad="63500"><a:srgbClr val="FF0000"/></a:glow></a:effectDag>"#,
        r#"<a:scene3d><a:camera prst="isometricRightUp"/><a:lightRig rig="threePt" dir="t"/></a:scene3d>"#,
        r#"<a:scene3d><a:camera prst="orthographicFront"><a:rot lat="0" lon="1200000" rev="0"/></a:camera><a:lightRig rig="threePt" dir="t"/></a:scene3d>"#,
        r#"<a:scene3d><a:camera prst="orthographicFront"><a:rot lat="300000" lon="0" rev="0"/></a:camera><a:lightRig rig="threePt" dir="t"/></a:scene3d>"#,
        r#"<a:scene3d><a:camera prst="orthographicFront"><a:rot lat="0" lon="0" rev="60000"/></a:camera><a:lightRig rig="threePt" dir="t"/></a:scene3d>"#,
        &format!("{front}<a:sp3d><a:bevelT/></a:sp3d>"),
        r#"<a:sp3d><a:bevelB w="12700" h="12700"/></a:sp3d>"#,
        r#"<a:sp3d extrusionH="76200"/>"#,
        r#"<a:sp3d contourW="12700"/>"#,
    ] {
        assert_eq!(entries(rect, listed, ""), [effects], "{listed}");
    }
    let shape = "a picture cut to a shape (a:prstGeom other than rect, or a:custGeom) is drawn whole as a rectangle, here and wherever else the document has one";
    for geometry in [
        r#"<a:prstGeom prst="ellipse"/>"#,
        r#"<a:custGeom><a:pathLst/></a:custGeom>"#,
    ] {
        assert_eq!(entries(geometry, "", ""), [shape], "{geometry}");
    }
    let colour = "a picture's colour or transparency effects (a:blip, such as a:grayscl or a:lum) are not applied, here and wherever else the document has them";
    for blip in [
        "<a:grayscl/>",
        r#"<a:duotone><a:srgbClr val="000000"/><a:srgbClr val="FFFFFF"/></a:duotone>"#,
        r#"<a:alphaModFix amt="99000"/>"#,
        r#"<a:alphaModFix amt="50%"/>"#,
        r#"<a:lum bright="20000"/>"#,
        r#"<a:lum contrast="-20000"/>"#,
        r#"<a:hsl hue="0" sat="50000" lum="0"/>"#,
        r#"<a:hsl lum="10000"/>"#,
        r#"<a:hsl hue="5400000"/>"#,
    ] {
        assert_eq!(entries(rect, "", blip), [colour], "{blip}");
    }
    // Office writes the camera with a rotation of zeros into the effect styles of themes.
    let nothing = entries(
        rect,
        r#"<a:effectLst/><a:effectDag/><a:scene3d><a:camera prst="orthographicFront"><a:rot lat="0" lon="0" rev="0"/></a:camera><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d extrusionH="0" contourW="0" prstMaterial="plastic"/>"#,
        r#"<a:lum/><a:lum bright="0" contrast="0"/><a:alphaModFix/><a:alphaModFix amt="100000"/><a:alphaModFix amt="100%"/><a:hsl hue="0" sat="0" lum="0"/>"#,
    );
    assert!(nothing.is_empty(), "{nothing:?}");
    assert!(entries(rect, front, "").is_empty());
}

#[test]
fn lists_a_border_around_a_picture_in_the_line_only() {
    // From the re-check of #326 (POI's bug59058.docx has a border around each of its
    // pictures, alone in their runs): a border around a run surrounds a picture in the
    // line, but not one that floats.
    let entries = |picture: String| -> Vec<String> {
        let run = picture.replacen(
            "<w:r>",
            r#"<w:r><w:rPr><w:bdr w:val="single" w:sz="6" w:color="CCCCCC"/></w:rPr>"#,
            1,
        );
        with_images(&format!("<w:p>{run}</w:p>"))
            .to_layout_ir()
            .left_out()
            .iter()
            .map(|entry| entry.message().to_owned())
            .filter(|message| message.starts_with("borders"))
            .collect()
    };
    assert_eq!(
        entries(inline_picture("rIdPng")),
        ["borders around text (w:bdr) are not drawn, here and wherever else the document has them"]
    );
    assert!(entries(floating_picture("rIdPng")).is_empty());
}
