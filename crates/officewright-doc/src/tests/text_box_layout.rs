//! Text boxes in the translation into the layout IR: their frame and place, their insets,
//! anchoring, fill and outline, and what they have that the IR does not hold.

use officewright_core::Color;
use officewright_layout::ReportEntry;
use officewright_layout::ir;

use super::layout::{paragraphs, translate};
use super::text_box::p;
use super::*;

/// The namespaces of drawings and VML that the test documents do not declare.
const NAMESPACES: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:w10="urn:schemas-microsoft-com:office:word""#;

/// The `a:graphic` of a shape (`wps:wsp`) holding `shape`.
fn graphic(shape: &str) -> String {
    format!(
        r#"<a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp>{shape}</wps:wsp></a:graphicData></a:graphic>"#
    )
}

/// A run with a drawing in the line, 100 by 50 pt, whose effects reach 1 pt to the left,
/// of a shape holding `shape`.
fn inline(shape: &str) -> String {
    format!(
        r#"<w:r><w:drawing {NAMESPACES}><wp:inline><wp:extent cx="1270000" cy="635000"/><wp:effectExtent l="12700" t="0" r="0" b="0"/><wp:docPr id="1" name="Text Box 1"/>{}</wp:inline></w:drawing></w:r>"#,
        graphic(shape)
    )
}

/// A run with a floating drawing, 100 by 50 pt, 10 pt from the page's left edge and 20 pt
/// below its paragraph, of a shape holding `shape`.
fn floating(shape: &str) -> String {
    format!(
        r#"<w:r><w:drawing {NAMESPACES}><wp:anchor simplePos="0" relativeHeight="5" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>127000</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>254000</wp:posOffset></wp:positionV><wp:extent cx="1270000" cy="635000"/><wp:wrapNone/><wp:docPr id="2" name="Text Box 2"/>{}</wp:anchor></w:drawing></w:r>"#,
        graphic(shape)
    )
}

/// The parts of a shape that is a text box: these shape properties, the content of these
/// blocks, and these attributes of `wps:bodyPr`.
fn shape(properties: &str, blocks: &str, body: &str) -> String {
    format!(
        "<wps:spPr>{properties}</wps:spPr><wps:txbx><w:txbxContent>{blocks}</w:txbxContent></wps:txbx><wps:bodyPr {body}/>"
    )
}

/// A run with a VML drawing of `shapes`.
fn pict(shapes: &str) -> String {
    format!("<w:r><w:pict {NAMESPACES}>{shapes}</w:pict></w:r>")
}

/// The text box an inline shows.
fn text_box(inline: &ir::Inline) -> &ir::TextBox {
    let content = match inline {
        ir::Inline::Object(object) => object.content(),
        ir::Inline::Anchored(object) => object.content(),
        other => panic!("not an object: {other:?}"),
    };
    match content {
        ir::ObjectContent::TextBox(text_box) => text_box,
        other => panic!("not a text box: {other:?}"),
    }
}

/// The text of each paragraph of blocks.
fn texts(blocks: &[ir::Block]) -> Vec<String> {
    blocks
        .iter()
        .map(|block| match block {
            ir::Block::Paragraph(paragraph) => paragraph
                .inlines()
                .iter()
                .filter_map(|inline| match inline {
                    ir::Inline::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect(),
            other => panic!("not a paragraph: {other:?}"),
        })
        .collect()
}

/// What a translation left out, each with its place.
fn listed(translation: &Translation) -> Vec<String> {
    translation
        .left_out()
        .iter()
        .map(ReportEntry::to_string)
        .collect()
}

fn insets(left: f64, top: f64, right: f64, bottom: f64) -> ir::Insets {
    ir::Insets::default()
        .with_left(Pt(left))
        .with_top(Pt(top))
        .with_right(Pt(right))
        .with_bottom(Pt(bottom))
}

fn line(style: ir::BorderStyle, width: f64, color: Color) -> ir::Border {
    ir::Border::default()
        .with_style(style)
        .with_width(Pt(width))
        .with_color(ir::ColorValue::Rgb(color))
}

#[test]
fn translates_text_boxes_with_their_frame_insets_anchoring_fill_and_outline() {
    let red = r#"<a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>"#;
    let blue_dashes = r#"<a:ln w="12700"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill><a:prstDash val="dash"/></a:ln>"#;
    let body = format!(
        "<w:p>{}{}{}{}</w:p>",
        inline(&shape(
            &format!(r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{red}{blue_dashes}"#),
            &p("Inline"),
            r#"lIns="12700" tIns="25400" rIns="38100" bIns="50800" anchor="ctr""#,
        )),
        floating(&shape(
            "<a:noFill/><a:ln><a:noFill/></a:ln>",
            &format!("{}{}", p("Floating"), p("Twice")),
            r#"anchor="b""#,
        )),
        pict(&format!(
            r##"<v:shape style="width:20pt;height:10pt" fillcolor="#00ff00" strokecolor="red" strokeweight="2pt"><v:textbox inset="1pt,2pt,3pt,4pt" style="v-text-anchor:bottom"><w:txbxContent>{}</w:txbxContent></v:textbox></v:shape>"##,
            p("VML")
        )),
        pict(&format!(
            r#"<v:rect style="position:absolute;margin-left:30pt;margin-top:40pt;width:50pt;height:60pt;z-index:7;mso-position-horizontal-relative:page;mso-position-vertical-relative:page"><v:textbox><w:txbxContent>{}</w:txbxContent></v:textbox></v:rect>"#,
            p("Floating VML")
        )),
    );
    let translation = translate(&open(&body));
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        listed(&translation)
    );
    let inlines = paragraphs(&translation)[0].inlines();
    assert_eq!(inlines.len(), 4);

    let ir::Inline::Object(object) = &inlines[0] else {
        panic!("a text box in the line");
    };
    assert_eq!(object.size(), ir::Size::new(Pt(100.0), Pt(50.0)));
    assert_eq!(object.effect_extent(), insets(1.0, 0.0, 0.0, 0.0));
    let boxed = text_box(&inlines[0]);
    assert_eq!(texts(boxed.blocks()), ["Inline"]);
    // The text starts inside the line: the insets count from its inner edge, half the
    // 1 pt line within the frame.
    assert_eq!(boxed.insets(), insets(1.5, 2.5, 3.5, 4.5));
    assert_eq!(boxed.anchor(), ir::TextBoxAnchor::Center);
    assert_eq!(boxed.fill(), Some(Color::rgb(255, 0, 0)));
    assert_eq!(
        boxed.outline(),
        Some(line(ir::BorderStyle::Dashed, 1.0, Color::rgb(0, 0, 255)))
    );

    let ir::Inline::Anchored(floating) = &inlines[1] else {
        panic!("a floating text box");
    };
    assert_eq!(floating.size(), ir::Size::new(Pt(100.0), Pt(50.0)));
    assert_eq!(
        (floating.horizontal(), floating.vertical()),
        (
            ir::HorizontalAnchor::default()
                .with_relative_to(ir::HorizontalReference::Page)
                .with_placement(ir::HorizontalPlacement::Offset(Pt(10.0))),
            ir::VerticalAnchor::default()
                .with_relative_to(ir::VerticalReference::Paragraph)
                .with_placement(ir::VerticalPlacement::Offset(Pt(20.0)))
        )
    );
    assert_eq!(floating.z_order(), 5);
    let boxed = text_box(&inlines[1]);
    assert_eq!(texts(boxed.blocks()), ["Floating", "Twice"]);
    // DrawingML's insets where the document writes none.
    assert_eq!(boxed.insets(), insets(7.2, 3.6, 7.2, 3.6));
    assert_eq!(boxed.anchor(), ir::TextBoxAnchor::Bottom);
    assert_eq!((boxed.fill(), boxed.outline()), (None, None));

    let ir::Inline::Object(object) = &inlines[2] else {
        panic!("a VML text box in the line");
    };
    assert_eq!(object.size(), ir::Size::new(Pt(20.0), Pt(10.0)));
    assert_eq!(object.effect_extent(), ir::Insets::default());
    let boxed = text_box(&inlines[2]);
    assert_eq!(texts(boxed.blocks()), ["VML"]);
    // Half the 2 pt line, as in DrawingML.
    assert_eq!(boxed.insets(), insets(2.0, 3.0, 4.0, 5.0));
    assert_eq!(boxed.anchor(), ir::TextBoxAnchor::Bottom);
    assert_eq!(boxed.fill(), Some(Color::rgb(0, 255, 0)));
    assert_eq!(
        boxed.outline(),
        Some(line(ir::BorderStyle::Single, 2.0, Color::rgb(255, 0, 0)))
    );

    let ir::Inline::Anchored(floating) = &inlines[3] else {
        panic!("a floating VML text box");
    };
    assert_eq!(floating.size(), ir::Size::new(Pt(50.0), Pt(60.0)));
    assert_eq!(
        (floating.horizontal(), floating.vertical()),
        (
            ir::HorizontalAnchor::default()
                .with_relative_to(ir::HorizontalReference::Page)
                .with_placement(ir::HorizontalPlacement::Offset(Pt(30.0))),
            ir::VerticalAnchor::default()
                .with_relative_to(ir::VerticalReference::Page)
                .with_placement(ir::VerticalPlacement::Offset(Pt(40.0)))
        )
    );
    assert_eq!(floating.z_order(), 7);
    let boxed = text_box(&inlines[3]);
    assert_eq!(texts(boxed.blocks()), ["Floating VML"]);
    assert_eq!(boxed.insets(), insets(7.575, 3.975, 7.575, 3.975));
    assert_eq!(boxed.anchor(), ir::TextBoxAnchor::Top);
    // VML's defaults: filled white, and a black line of 0.75 pt.
    assert_eq!(boxed.fill(), Some(Color::WHITE));
    assert_eq!(
        boxed.outline(),
        Some(line(ir::BorderStyle::Single, 0.75, Color::BLACK))
    );
}

#[test]
fn translates_the_dashes_and_compound_lines_of_a_text_box_s_outline() {
    // A line with these attributes of `a:ln`, and these dashes after its fill.
    let outline = |attributes: &str, dashes: &str| {
        inline(&shape(
            &format!(
                r#"<a:ln w="25400" {attributes}><a:solidFill><a:srgbClr val="000000"/></a:solidFill>{dashes}</a:ln>"#
            ),
            &p("x"),
            "",
        ))
    };
    let dash = |name: &str| format!(r#"<a:prstDash val="{name}"/>"#);
    let cases = [
        (outline("", ""), ir::BorderStyle::Single),
        (outline("", &dash("solid")), ir::BorderStyle::Single),
        (outline("", &dash("sysDot")), ir::BorderStyle::Dotted),
        (outline("", &dash("dot")), ir::BorderStyle::Dotted),
        (outline("", &dash("lgDash")), ir::BorderStyle::Dashed),
        (outline("", &dash("sysDash")), ir::BorderStyle::Dashed),
        (outline("", &dash("dashDot")), ir::BorderStyle::DotDash),
        (outline("", &dash("sysDashDot")), ir::BorderStyle::DotDash),
        (
            outline("", &dash("lgDashDotDot")),
            ir::BorderStyle::DotDotDash,
        ),
        (
            outline("", &dash("sysDashDotDot")),
            ir::BorderStyle::DotDotDash,
        ),
        (outline(r#"cmpd="sng""#, ""), ir::BorderStyle::Single),
        (outline(r#"cmpd="dbl""#, ""), ir::BorderStyle::Double),
        (
            outline(r#"cmpd="thickThin""#, ""),
            ir::BorderStyle::ThickThinSmallGap,
        ),
        (
            outline(r#"cmpd="thinThick""#, ""),
            ir::BorderStyle::ThinThickSmallGap,
        ),
        // DrawingML's triple line is thin, thick and thin.
        (
            outline(r#"cmpd="tri""#, ""),
            ir::BorderStyle::ThinThickThinSmallGap,
        ),
        (
            outline(r#"cmpd="dbl""#, &dash("solid")),
            ir::BorderStyle::Double,
        ),
    ];
    let body: String = cases.iter().map(|(run, _)| run.as_str()).collect();
    let translation = translate(&open(&format!("<w:p>{body}</w:p>")));
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        listed(&translation)
    );
    let outlines: Vec<ir::Border> = paragraphs(&translation)[0]
        .inlines()
        .iter()
        .map(|inline| text_box(inline).outline().unwrap())
        .collect();
    let expected: Vec<ir::Border> = cases
        .iter()
        .map(|(_, style)| line(*style, 2.0, Color::BLACK))
        .collect();
    assert_eq!(outlines, expected);
}

#[test]
fn lists_what_text_boxes_have_that_the_ir_does_not_hold_once_where_it_is_first_met() {
    let plain = |body: &str| inline(&shape("", &p("x"), body));
    let with = |properties: &str| inline(&shape(properties, &p("x"), ""));
    let first_of_chain = inline(
        r#"<wps:spPr/><wps:txbx id="1"><w:txbxContent><w:p><w:r><w:t>Chained</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/>"#,
    );
    let later_in_chain = inline(r#"<wps:spPr/><wps:linkedTxbx id="1" seq="1"/><wps:bodyPr/>"#);
    let runs = [
        // Nothing to list: a rectangle, upright, across, of one column.
        with(r#"<a:xfrm rot="21600000"/><a:prstGeom prst="rect"/><a:effectLst/>"#),
        first_of_chain,
        later_in_chain,
        with(r#"<a:xfrm rot="5400000"/>"#),
        plain(r#"vert="vert270""#),
        plain(r#"fromWordArt="1""#),
        plain(r#"numCol="2""#),
        with(r#"<a:prstGeom prst="roundRect"/>"#),
        with(r#"<a:effectLst><a:outerShdw blurRad="1"/></a:effectLst>"#),
        plain(r#"vertOverflow="clip""#),
        plain(r#"lIns="-12700""#),
    ];
    // Each feature once, however many boxes have it.
    let body = format!("<w:p>{}</w:p><w:p>{}</w:p>", runs.concat(), runs.concat());
    let translation = translate(&open(&body));
    let at = |index: usize| format!("sections[0].blocks[0].inlines[{index}].content: ");
    let expected = [
        (
            1,
            "a chain of linked text boxes (wps:linkedTxbx, or mso-next-textbox in VML) is drawn with all its text in its first box, here and wherever else the document has one",
        ),
        (
            3,
            "a turned text box (a:xfrm rot, or rotation in VML) is drawn upright, here and wherever else the document has one",
        ),
        (
            4,
            "vertical text in a text box (wps:bodyPr vert, or layout-flow in VML) is drawn across, here and wherever else the document has some",
        ),
        (
            5,
            "WordArt (fromWordArt, or a:prstTxWarp) is drawn as plain text in a box, here and wherever else the document has some",
        ),
        (
            6,
            "the columns of a text box (numCol) are drawn as one, here and wherever else the document has them",
        ),
        (
            7,
            "a text box shaped other than as a rectangle (a:prstGeom other than rect, a:custGeom, or a VML shape such as v:roundrect or a callout) is drawn as a rectangle, here and wherever else the document has one",
        ),
        (
            8,
            "a text box's shadow, reflection, glow, soft edges or 3-D effect (a:effectLst, a:scene3d, a:sp3d, or v:shadow) is not drawn, here and wherever else the document has one",
        ),
        (
            9,
            "text taller than a text box that cuts it off at its bottom (vertOverflow clip or ellipsis) runs on past the box, here and wherever else the document has some",
        ),
        (
            10,
            "a text box's negative inset (lIns, tIns, rIns, bIns, or inset in VML) is taken as 0, here and wherever else the document has one",
        ),
    ];
    let expected: Vec<String> = expected
        .iter()
        .map(|(index, message)| format!("{}{message}", at(*index)))
        .collect();
    assert_eq!(listed(&translation), expected);
    let inlines = paragraphs(&translation)[0].inlines();
    assert_eq!(inlines.len(), runs.len());
    // The chain's text is in its first box; a later box shows none of its own.
    assert_eq!(texts(text_box(&inlines[1]).blocks()), ["Chained"]);
    assert!(text_box(&inlines[2]).blocks().is_empty());
    assert_eq!(text_box(&inlines[10]).insets(), insets(0.0, 3.6, 7.2, 3.6));
}

#[test]
fn lists_the_shapes_and_shadows_of_vml_text_boxes_but_not_their_rectangles() {
    let vml = |shape: &str| {
        pict(&format!(
            r##"<v:shapetype id="_x0000_t202" o:spt="202" path="m,l,21600r21600,l21600,xe"/><v:shapetype id="_x0000_t62" o:spt="62"/>{shape}"##
        ))
    };
    let content = format!("<w:txbxContent>{}</w:txbxContent>", p("x"));
    let shape = |name: &str, attributes: &str, children: &str| {
        vml(&format!(
            r#"<v:{name} style="width:10pt;height:10pt" {attributes}>{children}<v:textbox>{content}</v:textbox></v:{name}>"#
        ))
    };
    let unlisted = [
        shape("rect", "", ""),
        shape("shape", r##"type="#_x0000_t202""##, ""),
        shape("shape", r#"o:spt="1""#, ""),
        // A shape type the document does not define, as Word's picture frame.
        shape("shape", r##"type="#_x0000_t75""##, ""),
        shape("shape", "", r#"<v:shadow on="f"/>"#),
    ];
    let translation = translate(&open(&format!("<w:p>{}</w:p>", unlisted.concat())));
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        listed(&translation)
    );
    assert_eq!(paragraphs(&translation)[0].inlines().len(), unlisted.len());

    let shaped = "a text box shaped other than as a rectangle (a:prstGeom other than rect, a:custGeom, or a VML shape such as v:roundrect or a callout) is drawn as a rectangle, here and wherever else the document has one";
    let shadow = "a text box's shadow, reflection, glow, soft edges or 3-D effect (a:effectLst, a:scene3d, a:sp3d, or v:shadow) is not drawn, here and wherever else the document has one";
    for (listed_shape, message) in [
        (shape("roundrect", "", ""), shaped),
        (shape("oval", "", ""), shaped),
        // A callout, by its shape type or its own.
        (shape("shape", r##"type="#_x0000_t62""##, ""), shaped),
        (
            shape("shape", r##"type="#_x0000_t202" o:spt="62""##, ""),
            shaped,
        ),
        (shape("rect", "", r#"<v:shadow on="t"/>"#), shadow),
        (shape("rect", "", r#"<v:shadow on=""/>"#), shadow),
    ] {
        let translation = translate(&open(&format!("<w:p>{listed_shape}</w:p>")));
        assert_eq!(
            listed(&translation),
            [format!(
                "sections[0].blocks[0].inlines[0].content: {message}"
            )],
            "{listed_shape}"
        );
    }
}

/// The shape properties of a text box, the fill and the outline it gets, and what is
/// listed for it.
type Case = (
    String,
    Option<Color>,
    Option<ir::Border>,
    &'static [&'static str],
);

#[test]
fn draws_a_text_box_s_fill_and_line_as_the_ir_can_and_lists_the_rest() {
    const GRADIENT: &str = "a text box's gradient or pattern fill (a:gradFill, a:pattFill, or a v:fill of that type) is drawn in its first colour, here and wherever else the document has one";
    const PICTURE: &str = "a text box's picture fill (a:blipFill, or a v:fill of type tile or frame) is left out, here and wherever else the document has one";
    const TRANSPARENT: &str = "a text box's partly transparent fill or line (a:alpha, or opacity in VML) is drawn opaque, here and wherever else the document has one";
    const BLACK: &str = "a text box's line in a gradient, a pattern or a colour the document does not define is drawn black, here and wherever else the document has one";
    const DASHES: &str = "a text box's line of dashes of the document's own lengths (a:custDash, or numbers in dashstyle) or of dashes on a double or triple line is drawn with plain dashes or solid, here and wherever else the document has one";
    let green = r#"<a:srgbClr val="00FF00"/>"#;
    let half_green = r#"<a:srgbClr val="00FF00"><a:alpha val="50000"/></a:srgbClr>"#;
    let clear_green = r#"<a:srgbClr val="00FF00"><a:alpha val="0"/></a:srgbClr>"#;
    let line = |attributes: &str, fill: &str, dashes: &str| {
        format!(r#"<a:ln w="12700"{attributes}>{fill}{dashes}</a:ln>"#)
    };
    let solid = |color: &str| format!("<a:solidFill>{color}</a:solidFill>");
    let gradient = format!(
        r#"<a:gradFill><a:gsLst><a:gs pos="0">{green}</a:gs><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs></a:gsLst></a:gradFill>"#
    );
    let pattern = format!(
        r#"<a:pattFill prst="pct5"><a:fgClr>{green}</a:fgClr><a:bgClr><a:srgbClr val="FFFFFF"/></a:bgClr></a:pattFill>"#
    );
    let custom = r#"<a:custDash><a:ds d="100000" sp="100000"/></a:custDash>"#;
    let dash = r#"<a:prstDash val="dash"/>"#;
    let green_fill = Some(Color::rgb(0, 255, 0));
    let thin = |style, color| {
        Some(
            ir::Border::default()
                .with_style(style)
                .with_width(Pt(1.0))
                .with_color(color),
        )
    };
    let rgb = |red, green, blue| ir::ColorValue::Rgb(Color::rgb(red, green, blue));
    let single = ir::BorderStyle::Single;
    let cases: [Case; 11] = [
        (gradient.clone(), green_fill, None, &[GRADIENT]),
        (pattern, green_fill, None, &[GRADIENT]),
        ("<a:blipFill/>".to_owned(), None, None, &[PICTURE]),
        // Fully transparent: no fill, and nothing to list.
        (solid(clear_green), None, None, &[]),
        (solid(half_green), green_fill, None, &[TRANSPARENT]),
        (
            line("", &gradient, ""),
            None,
            thin(single, ir::ColorValue::Auto),
            &[BLACK],
        ),
        (
            line("", &solid(green), custom),
            None,
            thin(ir::BorderStyle::Dashed, rgb(0, 255, 0)),
            &[DASHES],
        ),
        (
            line(r#" cmpd="dbl""#, &solid(green), dash),
            None,
            thin(ir::BorderStyle::Double, rgb(0, 255, 0)),
            &[DASHES],
        ),
        (line("", &solid(clear_green), ""), None, None, &[]),
        (
            line("", &solid(half_green), ""),
            None,
            thin(single, rgb(0, 255, 0)),
            &[TRANSPARENT],
        ),
        // Each listed once.
        (
            format!("{gradient}{}", line("", &solid(half_green), custom)),
            green_fill,
            thin(ir::BorderStyle::Dashed, rgb(0, 255, 0)),
            &[GRADIENT, TRANSPARENT, DASHES],
        ),
    ];
    for (properties, fill, outline, expected) in cases {
        let run = inline(&shape(&properties, &p("x"), ""));
        let translation = translate(&open(&format!("<w:p>{run}{run}</w:p>")));
        let expected: Vec<String> = expected
            .iter()
            .map(|message| format!("sections[0].blocks[0].inlines[0].content: {message}"))
            .collect();
        assert_eq!(listed(&translation), expected, "{properties}");
        for inline in paragraphs(&translation)[0].inlines() {
            let text_box = text_box(inline);
            assert_eq!(
                (text_box.fill(), text_box.outline()),
                (fill, outline),
                "{properties}"
            );
        }
    }
}

#[test]
fn lists_each_feature_of_a_text_box_the_ir_does_not_hold() {
    let cases = [
        (
            r#"<wps:spPr><a:prstGeom prst="ellipse"/></wps:spPr>"#,
            "",
            "a text box shaped other than as a rectangle (a:prstGeom other than rect, a:custGeom, or a VML shape such as v:roundrect or a callout) is drawn as a rectangle, here and wherever else the document has one",
        ),
        (
            r#"<wps:spPr><a:custGeom/></wps:spPr>"#,
            "",
            "a text box shaped other than as a rectangle (a:prstGeom other than rect, a:custGeom, or a VML shape such as v:roundrect or a callout) is drawn as a rectangle, here and wherever else the document has one",
        ),
        (
            r#"<wps:spPr><a:effectLst><a:glow rad="1"/></a:effectLst></wps:spPr>"#,
            "",
            "a text box's shadow, reflection, glow, soft edges or 3-D effect (a:effectLst, a:scene3d, a:sp3d, or v:shadow) is not drawn, here and wherever else the document has one",
        ),
        (
            "<wps:spPr/>",
            r#"vert="eaVert""#,
            "vertical text in a text box (wps:bodyPr vert, or layout-flow in VML) is drawn across, here and wherever else the document has some",
        ),
        (
            "<wps:spPr/>",
            r#"bIns="-1""#,
            "a text box's negative inset (lIns, tIns, rIns, bIns, or inset in VML) is taken as 0, here and wherever else the document has one",
        ),
        (
            "<wps:spPr/>",
            r#"vertOverflow="ellipsis""#,
            "text taller than a text box that cuts it off at its bottom (vertOverflow clip or ellipsis) runs on past the box, here and wherever else the document has some",
        ),
    ];
    for (properties, body, message) in cases {
        let run = inline(&format!(
            "{properties}<wps:txbx><w:txbxContent>{}</w:txbxContent></wps:txbx><wps:bodyPr {body}/>",
            p("x")
        ));
        let translation = translate(&open(&format!("<w:p>{run}</w:p>")));
        assert_eq!(
            listed(&translation),
            [format!(
                "sections[0].blocks[0].inlines[0].content: {message}"
            )],
            "{properties} {body}"
        );
    }
}

#[test]
fn places_what_a_text_box_leaves_out_inside_the_box_also_in_a_field_s_result() {
    let reference = r#"<w:p><w:r><w:footnoteReference w:id="1"/></w:r></w:p>"#;
    let boxed = inline(&shape(r#"<a:prstGeom prst="roundRect"/>"#, reference, ""));
    let field = |content: &str| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>QUOTE "x"</w:instrText></w:r>{content}<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Result</w:t></w:r>{boxed}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    // A box in an instruction shows nothing, and lists nothing: here in that of a field,
    // and in that of a field in the result of another.
    let nested = format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>QUOTE "x"</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Outer</w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>QUOTE "y"</w:instrText></w:r>{boxed}<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Inner</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    );
    let body = format!(
        "<w:p><w:r><w:t>Before</w:t></w:r>{boxed}</w:p><w:p><w:r><w:t>Text</w:t></w:r>{}</w:p><w:p>{nested}</w:p>",
        field(&boxed)
    );
    let translation = translate(&open(&body));
    assert_eq!(
        listed(&translation),
        [
            "sections[0].blocks[0].inlines[1].content: a text box shaped other than as a rectangle (a:prstGeom other than rect, a:custGeom, or a VML shape such as v:roundrect or a callout) is drawn as a rectangle, here and wherever else the document has one",
            "sections[0].blocks[0].inlines[1].content.blocks[0].inlines: a reference to a footnote inside a text box is left out",
            "sections[0].blocks[1].inlines[1].result[1].content.blocks[0].inlines: a reference to a footnote inside a text box is left out",
        ]
    );
    let paragraphs = paragraphs(&translation);
    assert!(text_box(&paragraphs[0].inlines()[1]).blocks().len() == 1);
    let ir::Inline::Field(field) = &paragraphs[1].inlines()[1] else {
        panic!("a field: {:?}", paragraphs[1].inlines());
    };
    assert_eq!(field.result().len(), 2);
    text_box(&field.result()[1]);
    let [ir::Inline::Field(field)] = paragraphs[2].inlines() else {
        panic!("a field: {:?}", paragraphs[2].inlines());
    };
    assert!(
        field
            .result()
            .iter()
            .all(|inline| matches!(inline, ir::Inline::Text { .. })),
        "{:?}",
        field.result()
    );
}

#[test]
fn leaves_out_vml_groups_hidden_boxes_and_boxes_without_a_size() {
    let content = format!("<w:txbxContent>{}</w:txbxContent>", p("x"));
    let vml_box = |style: &str| {
        format!(r#"<v:shape style="{style}"><v:textbox>{content}</v:textbox></v:shape>"#)
    };
    let cases = [
        (
            pict(&format!(
                r#"<v:group style="width:10pt;height:10pt" coordsize="100,100">{}</v:group>"#,
                vml_box("width:100;height:100")
            )),
            0,
            vec!["a group of VML shapes is left out, with the text boxes in it (w:pict)"],
        ),
        (
            pict(&format!(
                r#"<v:rect style="width:1pt;height:1pt"/>{}"#,
                vml_box("width:10pt;height:10pt")
            )),
            1,
            vec!["a VML drawing's shapes other than its first text box are left out (w:pict)"],
        ),
        (
            pict(&format!(
                r#"{}<v:group style="width:10pt;height:10pt">{}</v:group>"#,
                vml_box("width:10pt;height:10pt"),
                vml_box("width:100;height:100")
            )),
            1,
            vec!["a VML drawing's shapes other than its first text box are left out (w:pict)"],
        ),
        (
            pict(&format!(
                r#"<v:group style="width:10pt;height:10pt">{}</v:group>{}"#,
                vml_box("width:100;height:100"),
                vml_box("width:10pt;height:10pt")
            )),
            1,
            vec!["a VML drawing's shapes other than its first text box are left out (w:pict)"],
        ),
        // Word does not show a hidden shape.
        (
            pict(&vml_box("visibility:hidden;width:10pt;height:10pt")),
            0,
            vec![],
        ),
        (
            pict(&vml_box("width:10pt")),
            0,
            vec!["a text box without a size is left out"],
        ),
        (
            format!(
                r#"<w:r><w:drawing {NAMESPACES}><wp:inline><wp:docPr id="1" name="Text Box 1"/>{}</wp:inline></w:drawing></w:r>"#,
                graphic(&shape("", &p("x"), ""))
            ),
            0,
            vec!["a text box without a size is left out"],
        ),
        (
            pict(r#"<v:rect style="width:1pt;height:1pt"/>"#),
            0,
            vec![
                "a VML drawing that shows no picture or text box (a shape or group) is left out (w:pict)",
            ],
        ),
    ];
    for (run, objects, expected) in cases {
        let translation = translate(&open(&format!("<w:p>{run}</w:p>")));
        let messages: Vec<&str> = translation
            .left_out()
            .iter()
            .map(ReportEntry::message)
            .collect();
        assert_eq!(messages, expected, "{run}");
        assert_eq!(
            paragraphs(&translation)[0].inlines().len(),
            objects,
            "{run}"
        );
    }
}

#[test]
fn translates_no_text_box_inside_a_text_box() {
    // The document leaves out the content of a box in a box as it opens, so that the IR,
    // which refuses one, holds none.
    let inner = inline(&shape("", &p("Inner"), ""));
    let outer = inline(&shape(
        "",
        &format!("<w:p>{inner}<w:r><w:t>Outer</w:t></w:r></w:p>"),
        "",
    ));
    let translation = translate(&open(&format!("<w:p>{outer}</w:p>")));
    let boxed = text_box(&paragraphs(&translation)[0].inlines()[0]);
    assert_eq!(texts(boxed.blocks()), ["Outer"]);
    assert_eq!(
        listed(&translation),
        [
            "sections[0].blocks[0].inlines[0].content.blocks[0].inlines: a drawing that shows no picture (a shape, chart or group) is left out"
        ]
    );
}

#[test]
fn translates_tables_nested_to_the_limit_around_and_in_a_text_box_on_a_small_stack() {
    // A text box's blocks are translated where its paragraph is, as a note's are, and
    // their tables count their depth on from those around the box: the IR's limit on
    // nesting bounds the recursion through both.
    let half = ir::MAX_TABLE_DEPTH / 2;
    let nested = |inside: &str| {
        format!(
            "{}{inside}{}",
            "<w:tbl><w:tr><w:tc>".repeat(half),
            "</w:tc></w:tr></w:tbl><w:p/>".repeat(half)
        )
    };
    let boxed = inline(&shape("", &nested("<w:p/>"), ""));
    let body = nested(&format!("<w:p>{boxed}</w:p>"));
    let options = LoadOptions::new().with_limits(Limits::new().with_max_xml_depth(1 << 12));
    let document = options.open_bytes(docx(&body)).unwrap();
    let kept = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("100 nested tables around and in a text box on 1.5 MiB of stack".into())
            .stack_size(1_536 * 1024)
            .spawn_scoped(scope, || {
                let translation = document.to_layout_ir();
                assert!(translation.document().validate().is_ok());
                fn depth(mut blocks: &[ir::Block]) -> (usize, &[ir::Block]) {
                    let mut depth = 0;
                    while let Some(ir::Block::Table(table)) = blocks.first() {
                        depth += 1;
                        blocks = table.rows()[0].cells()[0].blocks();
                    }
                    (depth, blocks)
                }
                let (around, blocks) = depth(translation.document().sections()[0].blocks());
                let ir::Block::Paragraph(paragraph) = &blocks[0] else {
                    panic!("not a paragraph");
                };
                let (inside, _) = depth(text_box(&paragraph.inlines()[0]).blocks());
                (around, inside)
            })
            .unwrap()
            .join()
            .unwrap()
    });
    assert_eq!(kept, (half, half));
}
