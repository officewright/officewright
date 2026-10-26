//! VML drawings: pictures in the line and floating, and their images.

use super::drawing::{images_package, inline_picture, with_images};
use super::*;

/// The namespaces a `w:pict` of the tests declares.
const VML_NAMESPACES: &str = r#"xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:w10="urn:schemas-microsoft-com:office:word""#;

/// A run with a VML drawing of `content`.
pub(super) fn pict(content: &str) -> String {
    format!(r#"<w:r><w:pict {VML_NAMESPACES}>{content}</w:pict></w:r>"#)
}

/// A run with a VML picture of the image of relationship `id`, with this `style`, these
/// attributes of `v:shape` and of `v:imagedata`, and these children of `v:shape` after
/// its `v:imagedata`. Like Word's, it has a `v:shapetype` first.
pub(super) fn vml_picture(
    id: &str,
    style: &str,
    shape: &str,
    data: &str,
    children: &str,
) -> String {
    pict(&format!(
        r##"<v:shapetype id="_x0000_t75" coordsize="21600,21600" o:spt="75" filled="f" stroked="f"><v:path o:connecttype="rect"/></v:shapetype><v:shape id="_x0000_i1025" type="#_x0000_t75" style="{style}" {shape}><v:imagedata r:id="{id}" o:title="" {data}/>{children}</v:shape>"##
    ))
}

/// A run with a VML picture in the line, 144 by 72 pt, turned and mirrored, cropped by
/// both forms of ST_Fraction.
pub(super) fn inline_vml_picture(id: &str) -> String {
    vml_picture(
        id,
        "width:144pt;height:72pt;rotation:-90;flip:x",
        "",
        r#"cropleft="6554f" cropbottom="-.05""#,
        "",
    )
}

/// A run with a floating VML picture, 100 by 40 pt: on the right of the page (its
/// offsets are ignored), 10 pt above the top margin, behind the text, with square
/// wrapping, not laid out in a cell.
pub(super) fn floating_vml_picture(id: &str) -> String {
    vml_picture(
        id,
        "position:absolute;margin-left:10pt;left:5pt;margin-top:-12pt;top:2pt;width:100pt;height:40pt;z-index:-251658240;mso-position-horizontal:right;mso-position-horizontal-relative:page;mso-position-vertical:absolute;mso-position-vertical-relative:top-margin-area;mso-wrap-distance-left:0;mso-wrap-distance-bottom:3pt",
        r#"o:allowincell="f""#,
        "",
        r#"<w10:wrap type="square" anchorx="page"/>"#,
    )
}

/// A run with a floating VML picture that writes nothing it may leave out.
pub(super) fn bare_floating_vml_picture(id: &str) -> String {
    vml_picture(id, "position:absolute;width:1pt;height:2pt", "", "", "")
}

/// The VML drawings of the body's runs, in order.
fn vml_drawings(document: &Document) -> Vec<VmlDrawing<'_>> {
    super::format::paragraphs(document)
        .into_iter()
        .flat_map(|paragraph| paragraph.runs())
        .flat_map(|run| run.content())
        .filter_map(|item| match item {
            RunContent::VmlDrawing(drawing) => Some(drawing),
            _ => None,
        })
        .collect()
}

#[test]
fn reads_vml_pictures_in_the_line_and_floating() {
    let body = format!(
        "<w:p>{}{}{}</w:p>",
        inline_vml_picture("rIdPng"),
        floating_vml_picture("rIdJpeg"),
        bare_floating_vml_picture("rIdPng"),
    );
    let document = with_images(&body);
    let drawings = vml_drawings(&document);
    assert_eq!(drawings.len(), 3);

    let inline = drawings[0].picture().unwrap();
    assert_eq!(
        (inline.width(), inline.height()),
        (Some(Pt(144.0)), Some(Pt(72.0)))
    );
    assert_eq!(inline.anchor(), None);
    assert!(!inline.is_hidden());
    let crop = inline.crop().unwrap();
    assert_eq!(
        (crop.left(), crop.top(), crop.right(), crop.bottom()),
        (6554.0 / 65536.0, 0.0, 0.0, -0.05)
    );
    assert_eq!(inline.rotation(), -90.0);
    assert!(inline.flip_horizontal() && !inline.flip_vertical());
    let image = inline.image().unwrap();
    assert_eq!(
        (image.content_type(), image.bytes()),
        ("image/png", PNG_BYTES)
    );

    let floating = drawings[1].picture().unwrap();
    assert_eq!(
        (floating.width(), floating.height()),
        (Some(Pt(100.0)), Some(Pt(40.0)))
    );
    assert_eq!(floating.crop(), None);
    assert_eq!(floating.rotation(), 0.0);
    let anchor = floating.anchor().unwrap();
    assert_eq!(
        anchor.horizontal_reference(),
        Some(HorizontalReference::Page)
    );
    assert_eq!(
        anchor.horizontal_align(),
        Some(AnchorHorizontalAlign::Right)
    );
    // Aligned, so the offsets across are ignored; down, margin-top and top add up.
    assert_eq!(anchor.horizontal_offset(), None);
    assert_eq!(
        anchor.vertical_reference(),
        Some(VerticalReference::TopMargin)
    );
    assert_eq!(anchor.vertical_align(), None);
    assert_eq!(anchor.vertical_offset(), Some(Pt(-10.0)));
    assert_eq!(anchor.wrap(), Some(WrapKind::Square));
    assert!(anchor.behind_text());
    assert!(!anchor.layout_in_cell() && anchor.allow_overlap());
    // The relativeHeight Word writes for a picture behind the text whose VML has this
    // z-index (z-index plus 503,316,480).
    assert_eq!(anchor.z_order(), 251_658_240);
    let distances = anchor.distances();
    assert_eq!(
        (
            distances.left(),
            distances.top(),
            distances.right(),
            distances.bottom()
        ),
        (Pt(0.0), Pt(0.0), Pt(9.0), Pt(3.0))
    );
    assert_eq!(
        floating.image().unwrap().content_type(),
        "application/octet-stream"
    );

    // What a floating picture leaves out: from the column and the paragraph by 0, no
    // wrapping, in the cell, overlapping, in front of the text, 9 pt beside it.
    let bare = drawings[2].picture().unwrap().anchor().unwrap();
    assert_eq!(
        (
            bare.horizontal_reference(),
            bare.horizontal_align(),
            bare.horizontal_offset()
        ),
        (None, None, Some(Pt(0.0)))
    );
    assert_eq!(
        (
            bare.vertical_reference(),
            bare.vertical_align(),
            bare.vertical_offset()
        ),
        (None, None, Some(Pt(0.0)))
    );
    assert_eq!(bare.wrap(), None);
    assert!(bare.layout_in_cell() && bare.allow_overlap() && !bare.behind_text());
    assert_eq!(bare.z_order(), 0);
    let distances = bare.distances();
    assert_eq!(
        (
            distances.left(),
            distances.top(),
            distances.right(),
            distances.bottom()
        ),
        (Pt(9.0), Pt(0.0), Pt(9.0), Pt(0.0))
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reads_every_reference_and_alignment_of_a_floating_vml_picture() {
    let across = [
        ("margin", HorizontalReference::Margin),
        ("page", HorizontalReference::Page),
        ("text", HorizontalReference::Column),
        ("char", HorizontalReference::Character),
        ("left-margin-area", HorizontalReference::LeftMargin),
        ("left-margin", HorizontalReference::LeftMargin),
        ("right-margin-area", HorizontalReference::RightMargin),
        ("right-margin", HorizontalReference::RightMargin),
        ("inner-margin-area", HorizontalReference::InsideMargin),
        ("outer-margin-area", HorizontalReference::OutsideMargin),
    ];
    let down = [
        ("margin", VerticalReference::Margin),
        ("page", VerticalReference::Page),
        ("text", VerticalReference::Paragraph),
        ("line", VerticalReference::Line),
        ("top-margin-area", VerticalReference::TopMargin),
        ("top-margin", VerticalReference::TopMargin),
        ("bottom-margin-area", VerticalReference::BottomMargin),
        ("bottom-margin", VerticalReference::BottomMargin),
        ("inner-margin-area", VerticalReference::InsideMargin),
        ("outer-margin-area", VerticalReference::OutsideMargin),
    ];
    let aligned_across = [
        ("left", AnchorHorizontalAlign::Left),
        ("center", AnchorHorizontalAlign::Center),
        ("right", AnchorHorizontalAlign::Right),
        ("inside", AnchorHorizontalAlign::Inside),
        ("outside", AnchorHorizontalAlign::Outside),
    ];
    let aligned_down = [
        ("top", AnchorVerticalAlign::Top),
        ("center", AnchorVerticalAlign::Center),
        ("bottom", AnchorVerticalAlign::Bottom),
        ("inside", AnchorVerticalAlign::Inside),
        ("outside", AnchorVerticalAlign::Outside),
    ];
    let wraps = [
        ("none", WrapKind::None),
        ("square", WrapKind::Square),
        ("tight", WrapKind::Tight),
        ("through", WrapKind::Through),
        ("topAndBottom", WrapKind::TopAndBottom),
    ];
    let anchor = |style: &str, children: &str| {
        let document = with_images(&format!(
            "<w:p>{}</w:p>",
            vml_picture(
                "rIdPng",
                &format!("position:absolute;width:1pt;height:1pt;{style}"),
                "",
                "",
                children
            )
        ));
        vml_drawings(&document)[0]
            .picture()
            .unwrap()
            .anchor()
            .unwrap()
    };
    for (value, reference) in across {
        let style = format!("mso-position-horizontal-relative:{value}");
        assert_eq!(anchor(&style, "").horizontal_reference(), Some(reference));
    }
    for (value, reference) in down {
        let style = format!("mso-position-vertical-relative:{value}");
        assert_eq!(anchor(&style, "").vertical_reference(), Some(reference));
    }
    for (value, align) in aligned_across {
        let style = format!("mso-position-horizontal:{value};margin-left:5pt");
        let found = anchor(&style, "");
        assert_eq!(
            (found.horizontal_align(), found.horizontal_offset()),
            (Some(align), None)
        );
    }
    for (value, align) in aligned_down {
        let style = format!("mso-position-vertical:{value};margin-top:5pt");
        let found = anchor(&style, "");
        assert_eq!(
            (found.vertical_align(), found.vertical_offset()),
            (Some(align), None)
        );
    }
    for (value, kind) in wraps {
        let wrap = format!(r#"<w10:wrap type="{value}"/>"#);
        assert_eq!(anchor("", &wrap).wrap(), Some(kind));
    }
    // Values Word does not write: no reference, no alignment, offsets as written.
    let odd = anchor(
        "mso-position-horizontal-relative:column;mso-position-vertical-relative:paragraph;mso-position-horizontal:absolute;mso-position-vertical:middle;margin-left:1in;margin-top:2pt",
        r#"<w10:wrap type="wrapSquare"/>"#,
    );
    assert_eq!(
        (
            odd.horizontal_reference(),
            odd.horizontal_align(),
            odd.horizontal_offset()
        ),
        (None, None, Some(Pt(72.0)))
    );
    assert_eq!(
        (
            odd.vertical_reference(),
            odd.vertical_align(),
            odd.vertical_offset()
        ),
        (None, None, Some(Pt(2.0)))
    );
    assert_eq!(odd.wrap(), None);
    // Overlap and cells by ST_TrueFalse; a value it does not have leaves the default.
    let flags = |value: &str| {
        let document = with_images(&format!(
            "<w:p>{}</w:p>",
            vml_picture(
                "rIdPng",
                "position:absolute;width:1pt;height:1pt",
                &format!(r#"o:allowincell="{value}" o:allowoverlap="{value}""#),
                "",
                ""
            )
        ));
        let anchor = vml_drawings(&document)[0]
            .picture()
            .unwrap()
            .anchor()
            .unwrap();
        (anchor.layout_in_cell(), anchor.allow_overlap())
    };
    assert_eq!(flags("f"), (false, false));
    assert_eq!(flags("false"), (false, false));
    assert_eq!(flags("t"), (true, true));
    assert_eq!(flags("0"), (true, true));
}

#[test]
fn reads_a_vml_picture_s_sizes_turns_and_flips_only_as_word_writes_them() {
    let picture = |style: &str, data: &str| {
        let document = with_images(&format!(
            "<w:p>{}</w:p>",
            vml_picture("rIdPng", style, "", data, "")
        ));
        let picture = vml_drawings(&document)[0].picture().unwrap();
        (
            picture.width(),
            picture.height(),
            picture.rotation(),
            (picture.flip_horizontal(), picture.flip_vertical()),
            picture.crop().map(|crop| crop.right()),
            picture.is_hidden(),
        )
    };
    assert_eq!(
        picture("width:2in;height:.1pt;rotation:5898240fd;flip:x y", ""),
        (
            Some(Pt(144.0)),
            Some(Pt(0.1)),
            90.0,
            (true, true),
            None,
            false
        )
    );
    assert_eq!(
        picture("flip:y x;rotation:45;width:8px", r#"cropright="0.5""#).3,
        (true, true)
    );
    assert_eq!(
        picture("width:8px;height:1cm;flip:y", r#"cropright="0.5""#),
        (
            Some(Pt(6.0)),
            Some(Pt(72.0 / 2.54)),
            0.0,
            (false, true),
            Some(0.5),
            false
        )
    );
    // A negative size, a percentage, em, auto and a bare number are no sizes; xy is no
    // flip (Word writes "x y"), and a crop that is no ST_Fraction crops nothing.
    assert_eq!(
        picture(
            "width:-1pt;height:50%;rotation:90deg;flip:xy;visibility:hidden",
            r#"cropright="10%""#
        ),
        (None, None, 0.0, (false, false), None, true)
    );
    assert_eq!(picture("width:2em;height:auto", "").0, None);
    assert_eq!(picture("width:12;height:0", "").0, None);
    assert_eq!(picture("width:12;height:0", "").1, Some(Pt(0.0)));
}

#[test]
fn shows_no_picture_for_vml_shapes_without_an_image_and_reads_missing_images() {
    let body = format!(
        "<w:p>{}{}{}{}{}{}{}</w:p>",
        // A text box, as Word writes one.
        pict(
            r#"<v:shape id="Text Box 2" style="position:absolute;width:80pt;height:14pt;z-index:251660288"><v:textbox><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape>"#
        ),
        // A text box with a v:imagedata that names no image, as the footer of
        // LibreOffice's tdf131594.docx has (from the review of #401): Word shows no
        // picture there.
        pict(
            r##"<v:shape type="#_x0000_t202" style="position:absolute;width:144pt;height:144pt;z-index:251659264" filled="f" stroked="f"><v:imagedata o:title=""/><v:textbox><w:txbxContent><w:p/></w:txbxContent></v:textbox></v:shape>"##
        ),
        // Word takes r:id only in a document: o:relid is SpreadsheetML's.
        pict(r#"<v:shape style="width:1pt;height:1pt"><v:imagedata o:relid="rIdPng"/></v:shape>"#),
        pict(r#"<v:line from="0,0" to="10pt,0"/>"#),
        // A picture in a group is the group's, not the drawing's.
        pict(
            r#"<v:group style="width:10pt;height:10pt"><v:shape style="width:10pt;height:10pt"><v:imagedata r:id="rIdPng"/></v:shape></v:group>"#
        ),
        vml_picture("rIdGone", "width:1pt;height:1pt", "", "", ""),
        vml_picture("rIdNone", "width:1pt;height:1pt", "", "", ""),
    );
    let document = with_images(&body);
    let drawings = vml_drawings(&document);
    assert_eq!(drawings.len(), 7);
    assert!(
        drawings[..5]
            .iter()
            .all(|drawing| drawing.picture().is_none())
    );
    assert!(drawings[0].to_xml().contains("<w:t>Boxed</w:t>"));
    // A picture whose image the package lacks, or whose relationship the part lacks,
    // keeps its frame.
    for drawing in &drawings[5..] {
        let picture = drawing.picture().unwrap();
        assert!(picture.image().is_none());
        assert_eq!(picture.width(), Some(Pt(1.0)));
    }
    let messages: Vec<String> = document
        .diagnostics()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[0].ends_with(
        "a picture shows the image /word/media/gone.png, which the package does not have"
    ));
    assert!(
        messages[1]
            .ends_with("a picture names the relationship rIdNone, which the part does not have")
    );
}

#[test]
fn reads_an_image_once_for_vml_and_drawingml_pictures_alike() {
    let body = format!(
        "<w:p>{}{}{}</w:p>",
        inline_vml_picture("rIdPng"),
        inline_picture("rIdPng"),
        inline_vml_picture("rIdPng"),
    );
    let document = with_images(&body);
    // The header's picture shows the same part.
    assert_eq!(document.images.len(), 1);
    let drawing = vml_drawings(&document)[0];
    let xml = drawing.to_xml();
    assert!(xml.starts_with("<w:pict"), "{xml}");
    assert!(xml.contains(r#"<v:imagedata r:id="rIdPng""#), "{xml}");
    assert!(
        xml.contains("xmlns:v=\"urn:schemas-microsoft-com:vml\""),
        "{xml}"
    );
}

#[test]
fn reads_vml_drawings_through_markup_compatibility() {
    // From the review of #401: what lies below w:pict goes through Markup Compatibility,
    // VML's namespaces understood as DrawingML's are (#312).
    let picture = inline_vml_picture("rIdPng");
    let floating = floating_vml_picture("rIdPng");
    let locked = picture.replacen(
        "<v:path ",
        r#"<o:lock v:ext="edit" aspectratio="t"/><v:path "#,
        1,
    );
    for (picture, from, to) in [
        (
            &picture,
            "<v:shapetype ",
            r#"<v:shapetype mc:MustUnderstand="w14" "#,
        ),
        (
            &picture,
            "<v:shape ",
            r#"<v:shape mc:MustUnderstand="w14" "#,
        ),
        (
            &picture,
            "<v:imagedata ",
            r#"<v:imagedata mc:MustUnderstand="w14" "#,
        ),
        (
            &floating,
            "<w10:wrap ",
            r#"<w10:wrap mc:MustUnderstand="w14" "#,
        ),
        (&locked, "<o:lock ", r#"<o:lock mc:MustUnderstand="w14" "#),
    ] {
        let body = format!("<w:p>{}</w:p>", picture.replacen(from, to, 1));
        match Document::from_bytes(images_package(&body)) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("mc:MustUnderstand"), "{to}: {message}")
            }
            other => panic!("{to} gave {other:?}"),
        }
    }
    // A choice that requires VML is taken, one that requires a namespace not understood
    // is not.
    let alternate = |requires: &str| {
        pict(&format!(
            r#"<mc:AlternateContent><mc:Choice Requires="{requires}"><v:shape style="width:1pt;height:1pt"><v:imagedata r:id="rIdPng"/></v:shape></mc:Choice><mc:Fallback><v:shape style="width:2pt;height:2pt"><v:imagedata r:id="rIdJpeg"/></v:shape></mc:Fallback></mc:AlternateContent>"#
        ))
    };
    let shown = |requires: &str| {
        let document = with_images(&format!("<w:p>{}</w:p>", alternate(requires)));
        let picture = vml_drawings(&document)[0].picture().unwrap();
        (
            picture.width(),
            picture.image().map(|image| image.content_type().to_owned()),
        )
    };
    assert_eq!(shown("v"), (Some(Pt(1.0)), Some("image/png".to_owned())));
    assert_eq!(
        shown("w14"),
        (Some(Pt(2.0)), Some("application/octet-stream".to_owned()))
    );
}

#[test]
fn takes_a_floating_vml_picture_s_bases_from_its_wrap_when_its_style_names_none() {
    // From the review of #401: Word 2007 wrote a picture's bases in w10:wrap's anchorx and
    // anchory only, as LibreOffice's n764005.docx (a page-size picture in a header) has;
    // read from the column and the paragraph instead, that picture moved 72 pt right and
    // 36 pt down.
    let references = |style: &str, wrap: &str| {
        let document = with_images(&format!(
            "<w:p>{}</w:p>",
            vml_picture(
                "rIdPng",
                &format!("position:absolute;width:1pt;height:1pt;{style}"),
                "",
                "",
                wrap
            )
        ));
        let anchor = vml_drawings(&document)[0]
            .picture()
            .unwrap()
            .anchor()
            .unwrap();
        (anchor.horizontal_reference(), anchor.vertical_reference())
    };
    assert_eq!(
        references("", r#"<w10:wrap anchorx="page" anchory="page"/>"#),
        (
            Some(HorizontalReference::Page),
            Some(VerticalReference::Page)
        )
    );
    assert_eq!(
        references(
            "",
            r#"<w10:wrap type="none" anchorx="margin" anchory="margin"/>"#
        ),
        (
            Some(HorizontalReference::Margin),
            Some(VerticalReference::Margin)
        )
    );
    assert_eq!(
        references("", r#"<w10:wrap anchorx="char" anchory="line"/>"#),
        (
            Some(HorizontalReference::Character),
            Some(VerticalReference::Line)
        )
    );
    assert_eq!(
        references("", r#"<w10:wrap anchorx="text" anchory="text"/>"#),
        (
            Some(HorizontalReference::Column),
            Some(VerticalReference::Paragraph)
        )
    );
    // The style's bases come first; with neither, the column and the paragraph, Office's
    // default text, are taken by the layout.
    assert_eq!(
        references(
            "mso-position-horizontal-relative:margin;mso-position-vertical-relative:line",
            r#"<w10:wrap anchorx="page" anchory="page"/>"#
        ),
        (
            Some(HorizontalReference::Margin),
            Some(VerticalReference::Line)
        )
    );
    assert_eq!(references("", "<w10:wrap/>"), (None, None));
    assert_eq!(references("", ""), (None, None));
}
