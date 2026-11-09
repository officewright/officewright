//! Text boxes: their content read as stories of their own, and the properties of the box.

use super::drawing::reported;
use super::format::paragraphs;
use super::*;

pub(super) const SHAPES: &str = r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office""#;

/// A drawing, in the line, whose shape (`wps:wsp`) holds `shape`.
pub(super) fn drawing(shape: &str) -> String {
    format!(
        r#"<w:drawing {SHAPES}><wp:inline><wp:extent cx="1270000" cy="635000"/><wp:docPr id="1" name="Text Box 1"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/>{shape}</wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing>"#
    )
}

/// A text box's content (`w:txbxContent`) of these blocks, in DrawingML.
pub(super) fn txbx(blocks: &str) -> String {
    format!("<wps:txbx><w:txbxContent>{blocks}</w:txbxContent></wps:txbx>")
}

/// A VML drawing of these shapes.
pub(super) fn pict(shapes: &str) -> String {
    format!("<w:pict {SHAPES}>{shapes}</w:pict>")
}

/// A VML text box shape, with this `style` of its shape and these attributes of its
/// `v:textbox`, holding these blocks.
fn vml_box(style: &str, text_box: &str, blocks: &str) -> String {
    format!(
        r#"<v:shape style="{style}"><v:textbox {text_box}><w:txbxContent>{blocks}</w:txbxContent></v:textbox></v:shape>"#
    )
}

pub(super) fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

/// The text of each block of a text box; a table is "table".
fn texts(text_box: TextBox<'_>) -> Vec<String> {
    text_box
        .blocks()
        .map(|block| match block {
            Block::Paragraph(paragraph) => paragraph.text(),
            Block::Table(_) => "table".to_owned(),
            other => panic!("not a paragraph or table: {other:?}"),
        })
        .collect()
}

fn contents<'a>(paragraph: Paragraph<'a>) -> Vec<RunContent<'a>> {
    paragraph.runs().flat_map(|run| run.content()).collect()
}

fn drawings(document: &Document) -> Vec<Drawing<'_>> {
    paragraphs(document)
        .into_iter()
        .flat_map(contents)
        .filter_map(|item| match item {
            RunContent::Drawing(drawing) => Some(drawing),
            _ => None,
        })
        .collect()
}

fn vml_text_boxes(document: &Document) -> Vec<TextBox<'_>> {
    paragraphs(document)
        .into_iter()
        .flat_map(contents)
        .filter_map(|item| match item {
            RunContent::VmlDrawing(drawing) => Some(drawing),
            _ => None,
        })
        .flat_map(|drawing| drawing.text_boxes())
        .collect()
}

#[test]
fn reads_a_text_box_s_content_as_a_story_of_its_own() {
    // As Word writes a text box: in DrawingML, with a VML fallback, of which only the
    // first branch is read.
    let content = format!(
        "{}<w:tbl><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>{}",
        p("One"),
        p("Cell"),
        p("Two")
    );
    let body = format!(
        r#"<w:p><w:r><w:t>Before</w:t></w:r><w:r><mc:AlternateContent><mc:Choice Requires="wps" {SHAPES}>{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent></w:r><w:r><w:t>After</w:t></w:r></w:p>{}"#,
        drawing(&format!(r#"{}<wps:bodyPr anchor="b"/>"#, txbx(&content))),
        pict(&vml_box("width:100pt;height:50pt", "", &content)),
        p("Next"),
    );
    let document = open(&body);
    let body_paragraphs = paragraphs(&document);
    let texts_of_body: Vec<String> = body_paragraphs.iter().map(|p| p.text()).collect();
    assert_eq!(texts_of_body, ["BeforeAfter", "Next"]);
    let items = contents(body_paragraphs[0]);
    assert_eq!(items.len(), 3, "{items:?}");
    let RunContent::Drawing(drawing) = items[1] else {
        panic!("not a drawing: {:?}", items[1]);
    };
    let text_box = drawing.text_box().unwrap();
    assert_eq!(texts(text_box), ["One", "table", "Two"]);
    // The shape is read on past its text box's content.
    assert_eq!(text_box.vertical_anchor(), TextBoxAnchor::Bottom);
    // The content is in the model, not in the shape kept as written.
    let xml = drawing.to_xml();
    assert!(xml.contains("txbxContent") && !xml.contains("One"), "{xml}");
    // A paragraph of a text box has an id like any other.
    let Some(Block::Paragraph(first)) = text_box.blocks().next() else {
        panic!("no paragraph");
    };
    assert_eq!(document.paragraph(first.id()).unwrap().text(), "One");
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reads_vml_text_boxes_and_those_of_their_groups() {
    let group = format!(
        r#"<v:group style="width:100pt;height:100pt" coordsize="100,100">{}{}</v:group>"#,
        vml_box("width:50;height:50", "", &p("B")),
        vml_box("width:50;height:50", "", &p("C")),
    );
    let body = format!(
        "<w:p><w:r>{}</w:r><w:r>{}</w:r></w:p>",
        pict(&vml_box("width:10pt;height:10pt", "", &p("A"))),
        pict(&format!(
            "{group}{}<v:rect style=\"width:10pt;height:10pt\"/>",
            vml_box("width:10pt;height:10pt", "", &p("D"))
        )),
    );
    let document = open(&body);
    let boxes: Vec<Vec<String>> = vml_text_boxes(&document).into_iter().map(texts).collect();
    assert_eq!(boxes, [["A"], ["B"], ["C"], ["D"]]);
    assert_eq!(paragraphs(&document).len(), 1);
}

#[test]
fn reads_text_boxes_in_headers_and_table_cells() {
    // A header's story is found after the text box it holds, which is a story too.
    let header = format!(
        r#"<w:hdr xmlns:w="{W}">{}<w:p><w:r>{}</w:r></w:p>{}</w:hdr>"#,
        p("Top"),
        drawing(&txbx(&p("In the header"))),
        p("Bottom"),
    );
    let body = format!(
        r#"<w:tbl><w:tr><w:tc><w:p><w:r>{}</w:r></w:p>{}</w:tc></w:tr></w:tbl>{}<w:sectPr><w:headerReference w:type="default" r:id="rId0"/></w:sectPr>"#,
        drawing(&txbx(&p("In a cell"))),
        p("Cell"),
        p("Body"),
    );
    let document = with_parts(&body, &[("header", "header1.xml", header)]);
    let section = document.sections().next().unwrap();
    let header = section.header(HeaderFooterKind::Default).unwrap();
    let header_paragraphs: Vec<Paragraph<'_>> = header
        .blocks()
        .map(|block| match block {
            Block::Paragraph(paragraph) => paragraph,
            other => panic!("not a paragraph: {other:?}"),
        })
        .collect();
    let header_texts: Vec<String> = header_paragraphs.iter().map(|p| p.text()).collect();
    assert_eq!(header_texts, ["Top", "", "Bottom"]);
    let RunContent::Drawing(drawing) = contents(header_paragraphs[1])[0] else {
        panic!("no drawing in the header");
    };
    assert_eq!(texts(drawing.text_box().unwrap()), ["In the header"]);
    let Some(Block::Table(table)) = section.blocks().next() else {
        panic!("no table");
    };
    let cell = table.rows().next().unwrap().cells().next().unwrap();
    let cell_paragraphs: Vec<Paragraph<'_>> = cell
        .blocks()
        .map(|block| match block {
            Block::Paragraph(paragraph) => paragraph,
            other => panic!("not a paragraph: {other:?}"),
        })
        .collect();
    assert_eq!(cell_paragraphs.len(), 2);
    assert_eq!(cell_paragraphs[1].text(), "Cell");
    let RunContent::Drawing(drawing) = contents(cell_paragraphs[0])[0] else {
        panic!("no drawing in the cell");
    };
    assert_eq!(texts(drawing.text_box().unwrap()), ["In a cell"]);
    assert_eq!(section.blocks().count(), 2);
}

#[test]
fn leaves_out_a_text_box_in_a_text_box_or_a_note_and_reports_it() {
    // MS-OI29500, Part 4, §14.9.1.1(a): Word allows no text box content there.
    // An embedded object, whose VML is not read for text boxes anywhere, is not reported
    // in a text box either; and a box of no content of its own is left out in a note.
    let inner = format!(
        r#"<w:p><w:r>{}</w:r></w:p><w:p><w:r><w:object {SHAPES}>{}</w:object></w:r></w:p>"#,
        drawing(&txbx(&p("Inner"))),
        vml_box("width:1pt;height:1pt", "", &p("Object"))
    );
    let body = format!(
        "<w:p><w:r>{}</w:r></w:p>{}",
        drawing(&txbx(&format!("{}{inner}", p("Outer")))),
        p("Body"),
    );
    let footnotes = format!(
        r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1"><w:p><w:r>{}</w:r><w:r>{}</w:r><w:r>{}</w:r></w:p></w:footnote></w:footnotes>"#,
        pict(&vml_box("width:10pt;height:10pt", "", &p("In a note"))),
        drawing(r#"<wps:linkedTxbx id="1" seq="1"/><wps:bodyPr/>"#),
        pict(r#"<v:shape style="width:10pt;height:10pt"><v:textbox/></v:shape>"#),
    );
    let document = with_parts(&body, &[("footnotes", "footnotes.xml", footnotes)]);
    let texts_of_body: Vec<String> = paragraphs(&document).iter().map(|p| p.text()).collect();
    assert_eq!(texts_of_body, ["", "Body"]);
    let outer = drawings(&document)[0].text_box().unwrap();
    assert_eq!(texts(outer), ["Outer", "", ""]);
    let Some(Block::Paragraph(holder)) = outer.blocks().nth(1) else {
        panic!("no paragraph");
    };
    let RunContent::Drawing(inner) = contents(holder)[0] else {
        panic!("no inner drawing");
    };
    assert!(inner.text_box().is_none());
    assert!(inner.to_xml().contains("Inner"));
    let note = document.footnote(1).unwrap();
    let Some(Block::Paragraph(paragraph)) = note.blocks().next() else {
        panic!("no paragraph in the note");
    };
    let RunContent::VmlDrawing(in_note) = contents(paragraph)[0] else {
        panic!("no drawing in the note");
    };
    assert_eq!(in_note.text_boxes().count(), 0);
    assert!(in_note.to_xml().contains("In a note"));
    let items = contents(paragraph);
    let RunContent::Drawing(linked) = items[1] else {
        panic!("no linked box in the note");
    };
    assert!(linked.text_box().is_none());
    let RunContent::VmlDrawing(empty) = items[2] else {
        panic!("no empty box in the note");
    };
    assert_eq!(empty.text_boxes().count(), 0);
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.message())
        .collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        messages[0].starts_with("/word/document.xml, line ")
            && messages[0].ends_with(
                ": a text box in another text box is left out, as Word allows none there"
            ),
        "{messages:?}"
    );
    assert!(
        messages[1].starts_with("/word/footnotes.xml, line ")
            && messages[1]
                .ends_with(": a text box in a note is left out, as Word allows none there"),
        "{messages:?}"
    );
}

#[test]
fn a_section_break_in_a_text_box_ends_no_section() {
    let content = format!(
        r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="5000" w:h="5000"/></w:sectPr></w:pPr></w:p><w:sectPr><w:pgSz w:w="6000" w:h="6000"/></w:sectPr>{}"#,
        p("Last")
    );
    let body = format!(
        r#"<w:p><w:r>{}</w:r></w:p>{}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr>"#,
        drawing(&txbx(&content)),
        p("Body"),
    );
    let document = open(&body);
    assert_eq!(document.sections().count(), 1);
    let section = document.sections().next().unwrap();
    assert_eq!(
        section.properties().page_size().map(|size| size.width()),
        Some(Pt(612.0))
    );
    assert_eq!(section.blocks().count(), 2);
    let text_box = drawings(&document)[0].text_box().unwrap();
    // The w:sectPr of the text box is kept there, as an element the model does not read.
    assert_eq!(text_box.blocks().count(), 3);
}

#[test]
fn reads_linked_text_boxes() {
    let body = format!(
        "<w:p><w:r>{}</w:r><w:r>{}</w:r><w:r>{}</w:r><w:r>{}</w:r><w:r>{}</w:r></w:p>",
        drawing(
            r#"<wps:txbx id="1"><w:txbxContent><w:p><w:r><w:t>Chain</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/>"#
        ),
        drawing(r#"<wps:linkedTxbx id="1" seq="1"/><wps:bodyPr/>"#),
        drawing(&format!("{}<wps:bodyPr/>", txbx(&p("Alone")))),
        pict(&vml_box(
            "width:10pt;height:10pt",
            r#"style="mso-next-textbox:#_x0000_s1026""#,
            &p("Chained"),
        )),
        pict(r#"<v:shape style="width:10pt;height:10pt"><v:textbox/></v:shape>"#),
    );
    let body = format!(
        r##"{body}<w:p><w:r>{}</w:r><w:r>{}</w:r><w:r>{}</w:r></w:p>"##,
        // A link to no box links to nothing, and so does one to the box itself.
        pict(&vml_box(
            "width:10pt;height:10pt",
            r#"style="mso-next-textbox: ""#,
            &p("Unlinked"),
        )),
        pict(&format!(
            r##"<v:shape id="_x0000_s2050" style="width:10pt;height:10pt"><v:textbox style="mso-next-textbox:#_x0000_s2050"><w:txbxContent>{}</w:txbxContent></v:textbox></v:shape>"##,
            p("Itself")
        )),
        pict(&format!(
            r##"<v:shape id="Text Box 2" o:spid="_x0000_s1026" style="width:10pt;height:10pt"><v:textbox style="mso-next-textbox:#_x0000_s1026"><w:txbxContent>{}</w:txbxContent></v:textbox></v:shape>"##,
            p("Itself too")
        )),
    );
    let document = open(&body);
    let boxes: Vec<TextBox<'_>> = drawings(&document)
        .into_iter()
        .map(|drawing| drawing.text_box().unwrap())
        .collect();
    assert_eq!(texts(boxes[0]), ["Chain"]);
    assert!(boxes[0].is_linked());
    assert_eq!(texts(boxes[1]), Vec::<String>::new());
    assert!(boxes[1].is_linked());
    assert!(!boxes[2].is_linked());
    let vml = vml_text_boxes(&document);
    assert_eq!(texts(vml[0]), ["Chained"]);
    assert!(vml[0].is_linked());
    // A later box of a VML chain has no content of its own.
    assert_eq!(texts(vml[1]), Vec::<String>::new());
    assert!(!vml[1].is_linked());
    assert_eq!(texts(vml[2]), ["Unlinked"]);
    assert!(!vml[2].is_linked());
    assert_eq!(vml.len(), 5);
    assert!(!vml[3].is_linked() && !vml[4].is_linked());
}

#[test]
fn reading_a_text_box_leaves_markup_compatibility_as_it_was() {
    // The run declares what Markup Compatibility makes of an element after the text box:
    // its content is read in its place, as the run's text.
    let body = format!(
        r#"<w:p><w:r xmlns:x="urn:x" mc:Ignorable="x" mc:ProcessContent="x:wrapper">{}<x:wrapper><w:t>After</w:t></x:wrapper></w:r></w:p>"#,
        drawing(&format!("{}<wps:bodyPr/>", txbx(&p("In the box"))))
    );
    let document = open(&body);
    assert_eq!(paragraphs(&document)[0].text(), "After");
    assert_eq!(
        texts(drawings(&document)[0].text_box().unwrap()),
        ["In the box"]
    );
}

#[test]
fn keeps_the_text_boxes_of_an_embedded_object_with_it() {
    // An embedded object is shown by its preview picture; what else its VML holds is not
    // read.
    let body = format!(
        r#"<w:p><w:r><w:object {SHAPES} w:dxaOrig="200" w:dyaOrig="200">{}</w:object></w:r></w:p>"#,
        vml_box("width:10pt;height:10pt", "", &p("In the object"))
    );
    let document = open(&body);
    let object = paragraphs(&document)[0]
        .runs()
        .flat_map(|run| run.content())
        .find_map(|item| match item {
            RunContent::Object(object) => Some(object),
            _ => None,
        })
        .unwrap();
    assert!(object.to_xml().contains("In the object"));
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reads_the_properties_of_drawing_text_boxes() {
    let body = format!(
        "<w:p><w:r>{}</w:r><w:r>{}</w:r><w:r>{}</w:r><w:r>{}</w:r></w:p>",
        drawing(&format!(
            r#"<wps:spPr><a:xfrm rot="5400000"><a:off x="0" y="0"/><a:ext cx="1" cy="1"/></a:xfrm></wps:spPr>{}<wps:bodyPr lIns="0" tIns="12700" rIns="25400" anchor="ctr" vertOverflow="clip" vert="vert270" numCol="2" fromWordArt="1"><a:prstTxWarp prst="textNoShape"><a:avLst/></a:prstTxWarp><a:spAutoFit/></wps:bodyPr>"#,
            txbx(&p("Set"))
        )),
        drawing(&format!("{}<wps:bodyPr/>", txbx(&p("Defaults")))),
        drawing(&format!(
            r#"{}<wps:bodyPr anchor="just" vertOverflow="ellipsis" vert="eaVert"><a:prstTxWarp prst="textArchUp"/><a:noAutofit/></wps:bodyPr>"#,
            txbx(&p("Warped"))
        )),
        drawing(&format!(
            r#"{}<wps:bodyPr vertOverflow="overflow" fromWordArt="0"><a:prstTxWarp prst="textNoShape"/></wps:bodyPr>"#,
            txbx(&p("Plain"))
        )),
    );
    let document = open(&body);
    let boxes: Vec<TextBox<'_>> = drawings(&document)
        .into_iter()
        .map(|drawing| drawing.text_box().unwrap())
        .collect();
    let set = boxes[0];
    let insets = set.insets();
    assert_eq!(
        (insets.left(), insets.top(), insets.right(), insets.bottom()),
        (Pt(0.0), Pt(1.0), Pt(2.0), Pt(3.6))
    );
    assert_eq!(set.vertical_anchor(), TextBoxAnchor::Center);
    assert!(set.grows_to_fit());
    assert!(set.clips_overflow());
    assert_eq!(set.rotation(), 90.0);
    assert_eq!(set.text_direction(), TextBoxDirection::Vertical270);
    assert!(set.is_word_art());
    assert_eq!(set.columns(), 2);
    let defaults = boxes[1];
    let insets = defaults.insets();
    assert_eq!(
        (insets.left(), insets.top(), insets.right(), insets.bottom()),
        (Pt(7.2), Pt(3.6), Pt(7.2), Pt(3.6))
    );
    assert_eq!(defaults.vertical_anchor(), TextBoxAnchor::Top);
    assert!(!defaults.grows_to_fit());
    assert!(!defaults.clips_overflow());
    assert_eq!(defaults.rotation(), 0.0);
    assert_eq!(defaults.text_direction(), TextBoxDirection::Horizontal);
    assert!(!defaults.is_word_art());
    assert_eq!(defaults.columns(), 1);
    let warped = boxes[2];
    assert_eq!(warped.vertical_anchor(), TextBoxAnchor::Justified);
    assert!(warped.clips_overflow());
    assert_eq!(warped.text_direction(), TextBoxDirection::EastAsianVertical);
    assert!(warped.is_word_art());
    assert!(!warped.grows_to_fit());
    let plain = boxes[3];
    assert!(!plain.clips_overflow());
    assert!(!plain.is_word_art());
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reads_the_properties_of_vml_text_boxes() {
    let body = format!(
        "<w:p><w:r>{}</w:r></w:p>",
        pict(&format!(
            "{}{}{}{}{}",
            vml_box(
                "width:10pt;height:10pt;rotation:45;v-text-anchor:middle",
                r#"inset=",5pt,,0" style="layout-flow:vertical;mso-layout-flow-alt:bottom-to-top;mso-fit-shape-to-text:t""#,
                &p("Set"),
            ),
            // The anchor of the v:textbox comes before its shape's.
            vml_box(
                "width:10pt;height:10pt;v-text-anchor:middle",
                r#"inset="1mm,0in,0,.5pt" style="v-text-anchor:bottom-center;layout-flow:vertical-ideographic""#,
                &p("Both"),
            ),
            vml_box(
                "width:10pt;height:10pt",
                r#"style="layout-flow:vertical;mso-layout-flow-alt:top-to-bottom;mso-fit-shape-to-text:f""#,
                &p("Down"),
            ),
            vml_box("width:10pt;height:10pt", "", &p("Defaults")),
            vml_box(
                "width:10pt;height:10pt",
                r#"style="layout-flow:horizontal-ideographic""#,
                &p("Across"),
            ),
        )),
    );
    let document = open(&body);
    let boxes = vml_text_boxes(&document);
    assert_eq!(boxes.len(), 5);
    let set = boxes[0];
    let insets = set.insets();
    assert_eq!(
        (insets.left(), insets.top(), insets.right(), insets.bottom()),
        (Pt(7.2), Pt(5.0), Pt(7.2), Pt(0.0))
    );
    assert_eq!(set.vertical_anchor(), TextBoxAnchor::Center);
    assert!(set.grows_to_fit());
    assert_eq!(set.rotation(), 45.0);
    assert_eq!(set.text_direction(), TextBoxDirection::Vertical270);
    assert!(!set.clips_overflow());
    assert!(!set.is_word_art());
    assert_eq!(set.columns(), 1);
    let both = boxes[1];
    let insets = both.insets();
    assert_eq!(
        insets.left().0.mul_add(1e6, 0.0).round(),
        (72.0 / 25.4 * 1e6_f64).round()
    );
    assert_eq!(
        (insets.top(), insets.right(), insets.bottom()),
        (Pt(0.0), Pt(0.0), Pt(0.5))
    );
    assert_eq!(both.vertical_anchor(), TextBoxAnchor::Bottom);
    assert_eq!(both.text_direction(), TextBoxDirection::EastAsianVertical);
    let down = boxes[2];
    assert_eq!(down.text_direction(), TextBoxDirection::Vertical);
    assert!(!down.grows_to_fit());
    let defaults = boxes[3];
    let insets = defaults.insets();
    assert_eq!(
        (insets.left(), insets.top(), insets.right(), insets.bottom()),
        (Pt(7.2), Pt(3.6), Pt(7.2), Pt(3.6))
    );
    assert_eq!(defaults.vertical_anchor(), TextBoxAnchor::Top);
    assert_eq!(defaults.text_direction(), TextBoxDirection::Horizontal);
    assert_eq!(defaults.rotation(), 0.0);
    assert_eq!(boxes[4].text_direction(), TextBoxDirection::Horizontal);
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reports_the_values_of_text_boxes_the_readers_do_not_take() {
    let body = format!(
        "<w:p><w:r>{}</w:r><w:r>{}</w:r></w:p>",
        drawing(&format!(
            r#"<wps:spPr><a:xfrm rot="1.5"/></wps:spPr>{}<wps:bodyPr lIns="x" anchor="middle" vertOverflow="cut" vert="sideways" numCol="17" fromWordArt="maybe"/>"#,
            txbx(&p("Bad"))
        )),
        pict(&vml_box(
            "width:10pt;height:10pt;rotation:x;v-text-anchor:center",
            r#"inset="1q,2pt" style="layout-flow:diagonal;mso-layout-flow-alt:sideways;mso-fit-shape-to-text:yes""#,
            &p("Bad too"),
        )),
    );
    let document = open(&body);
    assert_eq!(
        reported(&document),
        [
            r#"wps:bodyPr lIns="x" is not valid; it is ignored"#,
            r#"wps:bodyPr anchor="middle" is not valid; it is ignored"#,
            r#"wps:bodyPr vertOverflow="cut" is not valid; it is ignored"#,
            r#"a:xfrm rot="1.5" is not valid; it is ignored"#,
            r#"wps:bodyPr vert="sideways" is not valid; it is ignored"#,
            r#"wps:bodyPr fromWordArt="maybe" is not valid; it is ignored"#,
            r#"wps:bodyPr numCol="17" is not valid; it is ignored"#,
            r#"v:textbox inset="1q" is not valid; it is ignored"#,
            r#"v:shape style="v-text-anchor:center" is not valid; it is ignored"#,
            r#"v:textbox style="mso-fit-shape-to-text:yes" is not valid; it is ignored"#,
            r#"v:shape style="rotation:x" is not valid; it is ignored"#,
            r#"v:textbox style="mso-layout-flow-alt:sideways" is not valid; it is ignored"#,
            r#"v:textbox style="layout-flow:diagonal" is not valid; it is ignored"#,
        ]
    );
    // Each is read as not written.
    let text_box = drawings(&document)[0].text_box().unwrap();
    assert_eq!(text_box.insets().left(), Pt(7.2));
    assert_eq!(text_box.vertical_anchor(), TextBoxAnchor::Top);
    assert!(!text_box.clips_overflow());
    assert_eq!(text_box.rotation(), 0.0);
    assert_eq!(text_box.text_direction(), TextBoxDirection::Horizontal);
    assert!(!text_box.is_word_art());
    assert_eq!(text_box.columns(), 1);
    let vml = vml_text_boxes(&document)[0];
    let insets = vml.insets();
    assert_eq!((insets.left(), insets.top()), (Pt(7.2), Pt(2.0)));
    assert_eq!(vml.vertical_anchor(), TextBoxAnchor::Top);
    assert_eq!(vml.rotation(), 0.0);
    assert_eq!(vml.text_direction(), TextBoxDirection::Horizontal);
    assert!(!vml.grows_to_fit());
}

#[test]
fn keeps_a_text_box_in_a_shape_not_read_with_the_shape() {
    // Word 2010's groups (wpg) are not read: a text box in one stays in the drawing as
    // written.
    let body = format!(
        r#"<w:p><w:r><w:drawing {SHAPES} xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"><wp:inline><wp:extent cx="1" cy="1"/><wp:docPr id="1" name="Group 1"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"><wpg:wgp><wps:wsp>{}<wps:bodyPr/></wps:wsp></wpg:wgp></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
        txbx(&p("Grouped"))
    );
    let document = open(&body);
    let drawing = drawings(&document)[0];
    assert!(drawing.text_box().is_none());
    assert!(drawing.to_xml().contains("Grouped"));
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn finds_the_text_boxes_of_deeply_nested_groups_in_linear_time() {
    // From the review of #459: the children of each group were walked again for every
    // group around it, as the document opened and on every call: 32,000 nested groups
    // took 35 s to open.
    let n = 20_000;
    let body = |n: usize| {
        format!(
            "<w:p><w:r>{}</w:r></w:p>",
            pict(&format!(
                "{}{}{}",
                r#"<v:group style="width:10pt;height:10pt">"#.repeat(n),
                vml_box("width:1;height:1", "", &p("Deep")),
                "</v:group>".repeat(n)
            ))
        )
    };
    let document = super::open_in_linear_time(body, n);
    crate::model::xml::STEPS.set(0);
    let boxes = vml_text_boxes(&document);
    let steps = crate::model::xml::STEPS.get();
    assert_eq!(boxes.len(), 1);
    assert_eq!(texts(boxes[0]), ["Deep"]);
    // Each group is a start and an end tag.
    let tokens = 2 * n;
    assert!(steps <= 2 * tokens, "{steps} steps for {tokens} tokens");
}

#[test]
fn reads_the_text_boxes_of_strict_documents() {
    // From the review of #459: a Strict document writes a text box's content in Word
    // 2006's namespace ([MS-ODRAWXML] 2.13.3.2; MS-OI29500, Part 4, §14.9.1.1(b)), and it
    // read as empty.
    let content = r#"<wne:txbxContent><w:p><w:r><w:t>Strict</w:t></w:r></w:p></wne:txbxContent>"#;
    let xml = format!(
        r#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main" xmlns:wp="http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing" xmlns:a="http://purl.oclc.org/ooxml/drawingml/main" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wne="http://schemas.microsoft.com/office/word/2006/wordml" xmlns:v="urn:schemas-microsoft-com:vml" conformance="strict"><w:body><w:p><w:r><w:drawing><wp:inline><wp:extent cx="12700" cy="12700"/><wp:docPr id="1" name="Text Box 1"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:txbx>{content}</wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></w:r><w:r><w:pict><v:shape style="width:1pt;height:1pt"><v:textbox>{content}</v:textbox></v:shape></w:pict></w:r></w:p></w:body></w:document>"#
    );
    let document = Document::from_bytes(super::package(super::DOCUMENT, &xml, &[])).unwrap();
    let text_box = drawings(&document)[0].text_box().unwrap();
    assert_eq!(texts(text_box), ["Strict"]);
    assert!(!drawings(&document)[0].to_xml().contains("Strict"));
    let vml = vml_text_boxes(&document);
    assert_eq!(vml.len(), 1);
    assert_eq!(texts(vml[0]), ["Strict"]);
    assert_eq!(paragraphs(&document)[0].text(), "");
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reports_text_box_content_that_no_text_box_shows() {
    // From the review of #459: Office takes the first v:textbox of a shape (MS-OI29500,
    // Part 4, §19.1.2.22(j)); the content of a second, and one that is no text box's, was
    // left out of the model's views and of the shape kept as written, unreported.
    let stray = format!(
        r#"<w:drawing {SHAPES}><wp:inline><wp:extent cx="1" cy="1"/><wp:docPr id="1" name="Odd"/><a:graphic><a:graphicData uri="urn:odd"><w:txbxContent>{}</w:txbxContent></a:graphicData></a:graphic></wp:inline></w:drawing>"#,
        p("Stray")
    );
    let body = format!(
        r#"<w:p><w:r>{}</w:r><w:r>{}</w:r></w:p>"#,
        // Only a shape of VML has a text box, not an element of Office's extensions.
        pict(&format!(
            r#"<v:shape style="width:1pt;height:1pt"><v:textbox><w:txbxContent>{}</w:txbxContent></v:textbox><v:textbox><w:txbxContent>{}</w:txbxContent></v:textbox></v:shape><o:lock><v:textbox><w:txbxContent>{}</w:txbxContent></v:textbox></o:lock>"#,
            p("First"),
            p("Second"),
            p("Locked")
        )),
        stray,
    );
    let document = open(&body);
    let boxes = vml_text_boxes(&document);
    assert_eq!(boxes.len(), 1);
    assert_eq!(texts(boxes[0]), ["First"]);
    assert_eq!(
        reported(&document),
        [
            "text box content (w:txbxContent) that no text box of its shape shows is left out",
            "text box content (w:txbxContent) that no text box of its shape shows is left out",
            "text box content (w:txbxContent) that no text box of its shape shows is left out",
        ]
    );
}

#[test]
fn reading_a_text_box_ends_markup_compatibility_s_scope_of_its_content_first() {
    // From the review of #459: what the shape declares around its text box no longer holds
    // after it. Here wps:txbx makes x:wrap ignorable with its content processed; after
    // it, x:wrap is kept as written, and so is the wps:bodyPr in it.
    let body = format!(
        r#"<w:p><w:r>{}</w:r></w:p>"#,
        drawing(&format!(
            r#"<wps:txbx xmlns:x="urn:x" mc:Ignorable="x" mc:ProcessContent="x:wrap"><w:txbxContent>{}</w:txbxContent></wps:txbx><x:wrap xmlns:x="urn:x"><wps:bodyPr anchor="b"/></x:wrap>"#,
            p("Box")
        ))
    );
    let document = open(&body);
    let text_box = drawings(&document)[0].text_box().unwrap();
    assert_eq!(texts(text_box), ["Box"]);
    assert_eq!(text_box.vertical_anchor(), TextBoxAnchor::Top);
}

#[test]
fn finds_the_text_boxes_of_vml_drawings_through_markup_compatibility() {
    // From the re-check of #459: the walk over a drawing's shapes reads mc:AlternateContent,
    // the branch taken and an element read in place as the model's other walks do.
    let alternate = |taken: &str, not_taken: &str| {
        format!(
            r#"<mc:AlternateContent><mc:Choice Requires="v">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
            vml_box("width:1pt;height:1pt", "", &p(taken)),
            vml_box("width:1pt;height:1pt", "", &p(not_taken)),
        )
    };
    let in_place = |text: &str| {
        format!(
            "<x:wrap>{}</x:wrap>",
            vml_box("width:1pt;height:1pt", "", &p(text))
        )
    };
    let body = format!(
        r#"<w:p><w:r><w:pict {SHAPES} xmlns:x="urn:x" mc:Ignorable="x" mc:ProcessContent="x:wrap">{}{}<v:group style="width:1pt;height:1pt">{}{}</v:group></w:pict></w:r></w:p>"#,
        alternate("Choice", "Fallback"),
        in_place("Wrapped"),
        alternate("Grouped choice", "Grouped fallback"),
        in_place("Grouped and wrapped"),
    );
    let document = open(&body);
    let boxes: Vec<Vec<String>> = vml_text_boxes(&document).into_iter().map(texts).collect();
    assert_eq!(
        boxes,
        [
            ["Choice"],
            ["Wrapped"],
            ["Grouped choice"],
            ["Grouped and wrapped"]
        ]
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

/// A theme whose accent 1 is 4F81BD, with a format scheme of the fills and lines styles
/// name: the first fill and the first background fill are the style's colour, the second
/// fill a gradient; the lines are 6,350, 12,700 and 19,050 EMU in the style's colour, the
/// third dashed and double; the three effect styles Office writes come before the background
/// fill.
fn shape_theme() -> (&'static str, &'static str, String) {
    (
        "theme",
        "theme/theme1.xml",
        r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="T"><a:themeElements><a:clrScheme name="C"><a:dk1><a:sysClr val="windowText" lastClr="101010"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2><a:accent3><a:srgbClr val="9BBB59"/></a:accent3><a:accent4><a:srgbClr val="8064A2"/></a:accent4><a:accent5><a:srgbClr val="4BACC6"/></a:accent5><a:accent6><a:srgbClr val="F79646"/></a:accent6><a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink></a:clrScheme><a:fontScheme name="F"><a:majorFont><a:latin typeface="Cambria"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="S"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:gradFill><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"><a:tint val="50000"/></a:schemeClr></a:gs></a:gsLst></a:gradFill><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="19050" cmpd="dbl"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="dash"/></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"><a:alpha val="50000"/></a:schemeClr></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#.to_owned(),
    )
}

/// Settings that map background 1 to dark 1 and text 1 to light 1, as a dark page does.
fn dark_mapping() -> (&'static str, &'static str, String) {
    (
        "settings",
        "settings.xml",
        format!(
            r#"<w:settings xmlns:w="{W}"><w:clrSchemeMapping w:bg1="dark1" w:t1="light1" w:bg2="light2" w:t2="dark2" w:accent1="accent1" w:accent2="accent2" w:accent3="accent3" w:accent4="accent4" w:accent5="accent5" w:accent6="accent6" w:hyperlink="hyperlink" w:followedHyperlink="followedHyperlink"/></w:settings>"#
        ),
    )
}

/// A DrawingML text box whose `wps:spPr` holds `properties`, with this `wps:style`.
fn styled_box(properties: &str, style: &str) -> String {
    format!(
        "<w:r>{}</w:r>",
        drawing(&format!(
            r#"<wps:spPr>{properties}</wps:spPr>{}<wps:style>{style}</wps:style><wps:bodyPr/>"#,
            txbx(&p("Box"))
        ))
    )
}

/// A fill's kind, colour and opacity.
type FillValues = (FillKind, Option<(u8, u8, u8)>, f64);

/// A line's width, colour, dashes and lines.
type OutlineValues = (Pt, Option<(u8, u8, u8)>, LineDash, CompoundLine);

fn fill_of(text_box: TextBox<'_>) -> Option<FillValues> {
    text_box
        .fill()
        .map(|fill| (fill.kind(), fill.color(), fill.opacity()))
}

fn outline_of(text_box: TextBox<'_>) -> Option<OutlineValues> {
    text_box
        .outline()
        .map(|line| (line.width(), line.color(), line.dash(), line.compound()))
}

#[test]
fn reads_the_fill_of_drawing_text_boxes_from_the_shape_or_its_style() {
    let boxes = [
        // Its own fills, which come before its style's.
        styled_box(
            r#"<a:solidFill><a:srgbClr val="FF8000"><a:alpha val="40000"/></a:srgbClr></a:solidFill>"#,
            r#"<a:fillRef idx="1"><a:schemeClr val="accent2"/></a:fillRef>"#,
        ),
        styled_box(
            "<a:noFill/>",
            r#"<a:fillRef idx="1"><a:schemeClr val="accent2"/></a:fillRef>"#,
        ),
        styled_box(
            r#"<a:gradFill><a:gsLst><a:gs pos="0"><a:prstClr val="red"/></a:gs><a:gs pos="100000"><a:prstClr val="blue"/></a:gs></a:gsLst></a:gradFill>"#,
            "",
        ),
        styled_box(
            r#"<a:pattFill prst="pct5"><a:fgClr><a:srgbClr val="112233"/></a:fgClr><a:bgClr><a:srgbClr val="FFFFFF"/></a:bgClr></a:pattFill>"#,
            "",
        ),
        styled_box(r#"<a:blipFill><a:blip/></a:blipFill>"#, ""),
        styled_box("<a:grpFill/>", ""),
        // The style's fills: the first and second fills of the theme, its first background
        // fill, and none.
        styled_box(
            "",
            r#"<a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef>"#,
        ),
        styled_box(
            "",
            r#"<a:fillRef idx="2"><a:schemeClr val="accent1"/></a:fillRef>"#,
        ),
        styled_box(
            "",
            r#"<a:fillRef idx="1001"><a:schemeClr val="accent2"/></a:fillRef>"#,
        ),
        styled_box(
            "",
            r#"<a:fillRef idx="0"><a:schemeClr val="accent1"/></a:fillRef>"#,
        ),
        // An entry the theme does not have, and no style at all.
        styled_box(
            "",
            r#"<a:fillRef idx="9"><a:schemeClr val="accent1"/></a:fillRef>"#,
        ),
        format!(
            "<w:r>{}</w:r>",
            drawing(&format!("{}<wps:bodyPr/>", txbx(&p("Bare"))))
        ),
    ];
    let body = format!("<w:p>{}</w:p>", boxes.concat());
    let document = with_parts(&body, &[shape_theme(), dark_mapping()]);
    let fills: Vec<_> = drawings(&document)
        .into_iter()
        .map(|drawing| fill_of(drawing.text_box().unwrap()))
        .collect();
    assert_eq!(
        fills,
        [
            Some((FillKind::Solid, Some((0xFF, 0x80, 0x00)), 0.4)),
            None,
            Some((FillKind::Gradient, Some((0xFF, 0x00, 0x00)), 1.0)),
            Some((FillKind::Pattern, Some((0x11, 0x22, 0x33)), 1.0)),
            Some((FillKind::Picture, None, 1.0)),
            None,
            Some((FillKind::Solid, Some((0x4F, 0x81, 0xBD)), 1.0)),
            // 4F81BD tinted 50 % in linear light.
            Some((FillKind::Gradient, Some((0xC2, 0xCD, 0xE1)), 1.0)),
            Some((FillKind::Solid, Some((0xC0, 0x50, 0x4D)), 0.5)),
            None,
            None,
            None,
        ]
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reads_the_outline_of_drawing_text_boxes_value_by_value() {
    let boxes = [
        // Its own line, whole.
        styled_box(
            r#"<a:ln w="25400" cmpd="thickThin"><a:solidFill><a:srgbClr val="4F81BD"><a:shade val="50000"/></a:srgbClr></a:solidFill><a:prstDash val="lgDashDot"/></a:ln>"#,
            r#"<a:lnRef idx="3"><a:schemeClr val="accent2"/></a:lnRef>"#,
        ),
        // Its own width over the style's line, whose colour is the reference's, shaded.
        styled_box(
            r#"<a:ln w="38100"/>"#,
            r#"<a:lnRef idx="3"><a:schemeClr val="accent1"><a:shade val="50000"/></a:schemeClr></a:lnRef>"#,
        ),
        // The style's line alone.
        styled_box(
            "",
            r#"<a:lnRef idx="2"><a:schemeClr val="accent1"/></a:lnRef>"#,
        ),
        // No line: its own, the style's index 0, and none at all.
        styled_box(
            "<a:ln><a:noFill/></a:ln>",
            r#"<a:lnRef idx="2"><a:schemeClr val="accent1"/></a:lnRef>"#,
        ),
        styled_box(
            "",
            r#"<a:lnRef idx="0"><a:schemeClr val="accent1"/></a:lnRef>"#,
        ),
        styled_box(r#"<a:ln w="9525"/>"#, ""),
        // Dashes of its own lengths, a gradient and colours of every kind.
        styled_box(
            r#"<a:ln><a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="000000"/></a:gs></a:gsLst></a:gradFill><a:custDash><a:ds d="100000" sp="100000"/></a:custDash></a:ln>"#,
            "",
        ),
        styled_box(
            r#"<a:ln><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></a:ln>"#,
            "",
        ),
        styled_box(
            r#"<a:ln><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:ln>"#,
            "",
        ),
        styled_box(
            r#"<a:ln><a:solidFill><a:sysClr val="windowText" lastClr="ABCDEF"/></a:solidFill></a:ln>"#,
            "",
        ),
        styled_box(
            r#"<a:ln><a:solidFill><a:hslClr hue="7200000" sat="100000" lum="50000"/></a:solidFill></a:ln>"#,
            "",
        ),
        styled_box(
            r#"<a:ln><a:solidFill><a:scrgbClr r="21404" g="100000" b="0"/></a:solidFill></a:ln>"#,
            "",
        ),
        styled_box(
            r#"<a:ln><a:solidFill><a:srgbClr val="FFFFFF"><a:lumMod val="75000"/></a:srgbClr></a:solidFill></a:ln>"#,
            "",
        ),
        styled_box(
            r#"<a:ln><a:solidFill><a:schemeClr val="accent1"><a:lumMod val="60000"/><a:lumOff val="40000"/></a:schemeClr></a:solidFill></a:ln>"#,
            "",
        ),
    ];
    let body = format!("<w:p>{}</w:p>", boxes.concat());
    let document = with_parts(&body, &[shape_theme(), dark_mapping()]);
    let lines: Vec<_> = drawings(&document)
        .into_iter()
        .map(|drawing| outline_of(drawing.text_box().unwrap()))
        .collect();
    let solid =
        |width: f64, rgb| Some((Pt(width), Some(rgb), LineDash::Solid, CompoundLine::Single));
    assert_eq!(
        lines,
        [
            // An RGB colour is shaded in linear light.
            Some((
                Pt(2.0),
                Some((0x38, 0x5D, 0x8A)),
                LineDash::LongDashDot,
                CompoundLine::ThickThin
            )),
            // A colour of the scheme too: 385D8A, the outline of Office 2010's default shape.
            Some((
                Pt(3.0),
                Some((0x38, 0x5D, 0x8A)),
                LineDash::Dash,
                CompoundLine::Double
            )),
            solid(1.0, (0x4F, 0x81, 0xBD)),
            None,
            None,
            None,
            Some((Pt(0.0), None, LineDash::Custom, CompoundLine::Single)),
            // Background 1 and text 1 through the document's colour mapping.
            solid(0.0, (0x10, 0x10, 0x10)),
            solid(0.0, (0xFF, 0xFF, 0xFF)),
            solid(0.0, (0xAB, 0xCD, 0xEF)),
            solid(0.0, (0x00, 0xFF, 0x00)),
            solid(0.0, (0x7F, 0xFF, 0x00)),
            solid(0.0, (0xBF, 0xBF, 0xBF)),
            // 4F81BD lightened by 40 %, as Word writes it for its "lighter 40 %" (95B3D7).
            solid(0.0, (0x95, 0xB3, 0xD7)),
        ]
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reads_the_fill_and_outline_of_vml_text_boxes() {
    let shape = |attributes: &str, children: &str| {
        format!(
            r#"<v:shape style="width:10pt;height:10pt" {attributes}>{children}<v:textbox><w:txbxContent>{}</w:txbxContent></v:textbox></v:shape>"#,
            p("Box")
        )
    };
    let body = format!(
        "<w:p><w:r>{}</w:r></w:p>",
        pict(&[
            // VML's defaults: white, with a black line of 0.75 pt.
            shape("", ""),
            shape(r##"fillcolor="#ffc [3201]" strokecolor="navy" strokeweight="2pt""##, ""),
            shape(r#"filled="f" stroked="false""#, ""),
            // The child's values come before the shape's.
            shape(
                r##"fillcolor="red" strokecolor="red""##,
                r##"<v:fill on="t" color="#00FF00" opacity="32768f" type="gradient"/><v:stroke color="#0000ff" weight="1.5pt" dashstyle="longdashdot" linestyle="thickBetweenThin" opacity=".25"/>"##,
            ),
            shape("", r#"<v:fill on="false" type="frame"/><v:stroke on="f"/>"#),
            shape(
                "",
                r#"<v:fill type="tile"/><v:stroke dashstyle="4 2 1 2" linestyle="thinThin"/>"#,
            ),
            // The shape type's values stand for those the shape does not write.
            r##"<v:shapetype id="_x0000_t202" filled="f" strokecolor="#123456"/>"##.to_owned(),
            shape(r##"type="#_x0000_t202""##, ""),
            shape(r##"type="#_x0000_t202" filled="t""##, ""),
            // The forms Office takes besides RGB (MS-OI29500 2.1.1893): no colour, a colour
            // of the system or of the shape, one changed, and rgb(); a width in EMUs.
            shape(r#"fillcolor="none" strokecolor="none""#, ""),
            shape(
                r#"fillcolor="Window" strokecolor="rgb(95, 39, 38)""#,
                r#"<v:stroke weight="25400"/>"#,
            ),
            shape(
                r#"fillcolor="fill darken(50)" strokecolor="lineOrFill""#,
                r#"<v:stroke weight="20116800"/>"#,
            ),
            shape(r#"fillcolor="wincolor(5)" strokecolor="fill invert""#, ""),
        ]
        .concat()),
    );
    let document = open(&body);
    let boxes = vml_text_boxes(&document);
    let fills: Vec<_> = boxes.iter().map(|text_box| fill_of(*text_box)).collect();
    assert_eq!(
        fills,
        [
            Some((FillKind::Solid, Some((0xFF, 0xFF, 0xFF)), 1.0)),
            Some((FillKind::Solid, Some((0xFF, 0xFF, 0xCC)), 1.0)),
            None,
            Some((FillKind::Gradient, Some((0x00, 0xFF, 0x00)), 0.5)),
            None,
            Some((FillKind::Picture, None, 1.0)),
            None,
            Some((FillKind::Solid, Some((0xFF, 0xFF, 0xFF)), 1.0)),
            None,
            Some((FillKind::Solid, None, 1.0)),
            Some((FillKind::Solid, None, 1.0)),
            Some((FillKind::Solid, None, 1.0)),
        ]
    );
    let lines: Vec<_> = boxes
        .iter()
        .map(|text_box| {
            text_box.outline().map(|line| {
                (
                    line.width(),
                    line.color(),
                    line.dash(),
                    line.compound(),
                    line.opacity(),
                )
            })
        })
        .collect();
    let black = Some((0, 0, 0));
    assert_eq!(
        lines,
        [
            Some((Pt(0.75), black, LineDash::Solid, CompoundLine::Single, 1.0)),
            Some((
                Pt(2.0),
                Some((0, 0, 0x80)),
                LineDash::Solid,
                CompoundLine::Single,
                1.0
            )),
            None,
            Some((
                Pt(1.5),
                Some((0, 0, 0xFF)),
                LineDash::LongDashDot,
                CompoundLine::Triple,
                0.25
            )),
            None,
            Some((Pt(0.75), black, LineDash::Custom, CompoundLine::Double, 1.0)),
            Some((
                Pt(0.75),
                Some((0x12, 0x34, 0x56)),
                LineDash::Solid,
                CompoundLine::Single,
                1.0
            )),
            Some((
                Pt(0.75),
                Some((0x12, 0x34, 0x56)),
                LineDash::Solid,
                CompoundLine::Single,
                1.0
            )),
            None,
            Some((
                Pt(2.0),
                Some((95, 39, 38)),
                LineDash::Solid,
                CompoundLine::Single,
                1.0
            )),
            Some((Pt(1584.0), None, LineDash::Solid, CompoundLine::Single, 1.0)),
            Some((Pt(0.75), None, LineDash::Solid, CompoundLine::Single, 1.0)),
        ]
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reports_the_colours_and_lines_of_text_boxes_the_readers_do_not_take() {
    let body = format!(
        "<w:p>{}<w:r>{}</w:r></w:p>",
        styled_box(
            r#"<a:solidFill><a:srgbClr val="12345G"/></a:solidFill><a:ln w="20116801" cmpd="quad"><a:solidFill><a:schemeClr val="accent9"/></a:solidFill><a:prstDash val="wavy"/></a:ln>"#,
            r#"<a:lnRef idx="x"><a:prstClr val="blurple"/></a:lnRef>"#,
        ),
        pict(&format!(
            r#"<v:shape style="width:10pt;height:10pt" fillcolor="blurple" strokecolor="fill frobnicate(3)" strokeweight="2em"><v:fill opacity="lots" type="hatch" color="rgb(1,2)"/><v:stroke dashstyle="dotty" linestyle="fat" weight="20116801"/><v:textbox><w:txbxContent>{}</w:txbxContent></v:textbox></v:shape>"#,
            p("Box")
        )),
    );
    let document = with_parts(&body, &[shape_theme()]);
    let reported = reported(&document);
    for message in [
        r#"a:srgbClr val="12345G" is not valid; it is ignored"#,
        r#"a:ln w="20116801" is not valid; it is ignored"#,
        r#"a:ln cmpd="quad" is not valid; it is ignored"#,
        r#"a:prstDash val="wavy" is not valid; it is ignored"#,
        r#"a:schemeClr val="accent9" is not valid; it is ignored"#,
        r#"a:lnRef idx="x" is not valid; it is ignored"#,
        r#"v:shape fillcolor="blurple" is not valid; it is ignored"#,
        r#"v:shape strokeweight="2em" is not valid; it is ignored"#,
        r#"v:shape strokecolor="fill frobnicate(3)" is not valid; it is ignored"#,
        r#"v:fill color="rgb(1,2)" is not valid; it is ignored"#,
        r#"v:stroke weight="20116801" is not valid; it is ignored"#,
        r#"v:fill opacity="lots" is not valid; it is ignored"#,
        r#"v:fill type="hatch" is not valid; it is ignored"#,
        r#"v:stroke dashstyle="dotty" is not valid; it is ignored"#,
        r#"v:stroke linestyle="fat" is not valid; it is ignored"#,
    ] {
        assert!(
            reported.iter().any(|line| line == message),
            "{message}: {reported:?}"
        );
    }
    // Each is read as not written: a fill and a line of no colour.
    let text_box = drawings(&document)[0].text_box().unwrap();
    assert_eq!(fill_of(text_box), Some((FillKind::Solid, None, 1.0)));
    assert_eq!(
        outline_of(text_box),
        Some((Pt(0.0), None, LineDash::Solid, CompoundLine::Single))
    );
    let vml = vml_text_boxes(&document)[0];
    assert_eq!(
        fill_of(vml),
        Some((FillKind::Solid, Some((0xFF, 0xFF, 0xFF)), 1.0))
    );
    assert_eq!(
        outline_of(vml),
        Some((
            Pt(0.75),
            Some((0, 0, 0)),
            LineDash::Solid,
            CompoundLine::Single
        ))
    );
}

#[test]
fn applies_the_transforms_of_a_colour_in_their_order() {
    let line = |color: &str| {
        styled_box(
            &format!("<a:ln><a:solidFill>{color}</a:solidFill></a:ln>"),
            "",
        )
    };
    let red = |transforms: &str| {
        line(&format!(
            r#"<a:srgbClr val="FF0000">{transforms}</a:srgbClr>"#
        ))
    };
    let boxes = [
        red("<a:comp/>"),
        red("<a:inv/>"),
        red("<a:gray/>"),
        red(r#"<a:hueOff val="7200000"/>"#),
        line(r#"<a:srgbClr val="00FF00"><a:hueMod val="50000"/></a:srgbClr>"#),
        red(r#"<a:hue val="14400000"/>"#),
        red(r#"<a:sat val="0"/>"#),
        red(r#"<a:satMod val="50000"/>"#),
        red(r#"<a:satOff val="-50000"/>"#),
        red(r#"<a:lum val="25000"/>"#),
        // In linear light, as a:shade.
        line(r#"<a:srgbClr val="000000"><a:tint val="50000"/></a:srgbClr>"#),
        // Each in its turn: shaded, then inverted, is not inverted, then shaded.
        line(r#"<a:srgbClr val="FFFFFF"><a:shade val="50000"/><a:inv/></a:srgbClr>"#),
        red(r#"<a:alpha val="50000"/><a:alphaMod val="50000"/><a:alphaOff val="10000"/>"#),
        // An offset of hue adds to the colour's own hue; the complement of a grey is that
        // grey, its inverse another; an amount keeps its thousandths of a per cent.
        line(r#"<a:srgbClr val="00FF00"><a:hueOff val="7200000"/></a:srgbClr>"#),
        line(r#"<a:srgbClr val="404040"><a:comp/></a:srgbClr>"#),
        red(r#"<a:alpha val="12345"/>"#),
    ];
    let body = format!("<w:p>{}</w:p>", boxes.concat());
    let document = open(&body);
    let lines: Vec<_> = drawings(&document)
        .into_iter()
        .map(|drawing| {
            let line = drawing.text_box().unwrap().outline().unwrap();
            (line.color(), line.opacity())
        })
        .collect();
    let opaque = |rgb| (Some(rgb), 1.0);
    assert_eq!(
        lines,
        [
            opaque((0x00, 0xFF, 0xFF)),
            opaque((0x00, 0xFF, 0xFF)),
            opaque((0x4D, 0x4D, 0x4D)),
            opaque((0x00, 0xFF, 0x00)),
            opaque((0xFF, 0xFF, 0x00)),
            opaque((0x00, 0x00, 0xFF)),
            opaque((0x80, 0x80, 0x80)),
            opaque((0xBF, 0x40, 0x40)),
            opaque((0xBF, 0x40, 0x40)),
            opaque((0x80, 0x00, 0x00)),
            opaque((0xBC, 0xBC, 0xBC)),
            opaque((0x43, 0x43, 0x43)),
            (Some((0xFF, 0x00, 0x00)), 0.35),
            opaque((0x00, 0x00, 0xFF)),
            opaque((0x40, 0x40, 0x40)),
            (Some((0xFF, 0x00, 0x00)), 0.12345),
        ]
    );
}

/// A theme as [`shape_theme`] gives, with these entries in place of its fills.
fn theme_with_fills(fills: &str) -> (&'static str, &'static str, String) {
    let (kind, target, xml) = shape_theme();
    let start = xml.find("<a:fillStyleLst>").unwrap() + "<a:fillStyleLst>".len();
    let end = xml.find("</a:fillStyleLst>").unwrap();
    (
        kind,
        target,
        format!("{}{fills}{}", &xml[..start], &xml[end..]),
    )
}

#[test]
fn finds_shape_types_and_style_entries_in_time_in_proportion_to_the_document() {
    // From the review of #471: each VML shape searched its drawing for its shape type, and
    // each box walked the theme's list to its entry and applied its colour's transforms
    // again: 30,000 shapes took over 30 s to open, and 8,000 boxes 20 s to 42 s.
    let n = 20_000;
    let body = |n: usize| {
        format!(
            "<w:p><w:r>{}</w:r></w:p>",
            pict(&format!(
                r#"{}<v:shapetype id="t" fillcolor="red"/>"#,
                r##"<v:shape type="#t" style="width:1pt;height:1pt"><v:textbox/></v:shape>"##
                    .repeat(n)
            ))
        )
    };
    let document = super::open_in_linear_time(body, n);
    let boxes = vml_text_boxes(&document);
    assert_eq!(boxes.len(), n);
    crate::model::xml::STEPS.set(0);
    let red = boxes
        .iter()
        .filter(|text_box| text_box.fill().and_then(|fill| fill.color()) == Some((0xFF, 0, 0)))
        .count();
    let steps = crate::model::xml::STEPS.get();
    assert_eq!(red, n);
    assert!(steps <= 20 * n, "{steps} steps for {n} shapes");
    // Boxes whose style names the last of many background fills (from 1001 on; the fills
    // themselves reach to 999), whose colour has many transforms.
    let (entries, transforms, count) = (20_000, 10_000, 100);
    let last = format!(
        r#"<a:solidFill><a:schemeClr val="phClr">{}</a:schemeClr></a:solidFill>"#,
        r#"<a:alpha val="100000"/>"#.repeat(transforms)
    );
    let fills = format!(
        "{}{last}",
        r#"<a:solidFill><a:srgbClr val="00FF00"/></a:solidFill>"#.repeat(entries - 1)
    );
    let styled = styled_box(
        "",
        &format!(
            r#"<a:fillRef idx="{}"><a:schemeClr val="accent1"/></a:fillRef>"#,
            1000 + entries
        ),
    );
    let body = format!("<w:p>{}</w:p>", styled.repeat(count));
    let (kind, target, theme) = shape_theme();
    let start = theme.find("<a:bgFillStyleLst>").unwrap() + "<a:bgFillStyleLst>".len();
    let end = theme.find("</a:bgFillStyleLst>").unwrap();
    let theme = format!("{}{fills}{}", &theme[..start], &theme[end..]);
    let document = with_parts(&body, &[(kind, target, theme)]);
    let boxes = drawings(&document);
    crate::model::xml::STEPS.set(0);
    let fills: Vec<_> = boxes
        .iter()
        .map(|drawing| fill_of(drawing.text_box().unwrap()))
        .collect();
    let steps = crate::model::xml::STEPS.get();
    assert!(
        fills
            .iter()
            .all(|fill| *fill == Some((FillKind::Solid, Some((0x4F, 0x81, 0xBD)), 1.0))),
        "{:?}",
        fills.first()
    );
    assert!(steps <= 1_000 * count, "{steps} steps for {count} boxes");
}

#[test]
fn applies_at_most_64_transforms_to_a_colour_of_the_theme_s_format_scheme_and_reports_more() {
    // 70 steps of 1 % of luminance from black: the first 64 make 64 %, A3A3A3.
    let fill = format!(
        r#"<a:solidFill><a:srgbClr val="000000">{}</a:srgbClr></a:solidFill>"#,
        r#"<a:lumOff val="1000"/>"#.repeat(70)
    );
    let body = format!(
        "<w:p>{}</w:p>",
        styled_box(
            "",
            r#"<a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef>"#
        )
    );
    let document = with_parts(&body, &[theme_with_fills(&fill)]);
    let text_box = drawings(&document)[0].text_box().unwrap();
    assert_eq!(
        fill_of(text_box),
        Some((FillKind::Solid, Some((0xA3, 0xA3, 0xA3)), 1.0))
    );
    let messages: Vec<&str> = document.diagnostics().iter().map(|d| d.message()).collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("/word/theme/theme1.xml")
            && messages[0].ends_with(
                "a colour of a:fmtScheme has more than 64 transforms; those after are left out"
            ),
        "{messages:?}"
    );
    // A shape's own colour keeps all of them: 70 %, B3B3B3.
    let body = format!("<w:p>{}</w:p>", styled_box(&fill, ""));
    let document = open(&body);
    let text_box = drawings(&document)[0].text_box().unwrap();
    assert_eq!(
        fill_of(text_box),
        Some((FillKind::Solid, Some((0xB3, 0xB3, 0xB3)), 1.0))
    );
}

#[test]
fn reads_the_values_the_review_of_471_found_refused_or_unreported() {
    let (kind, target, theme) = shape_theme();
    let theme = theme.replace(
        r#"<a:solidFill><a:srgbClr val="00FF00"/></a:solidFill>"#,
        r#"<a:solidFill><a:srgbClr val="zz"/></a:solidFill>"#,
    );
    let body = format!(
        "<w:p>{}{}{}{}<w:r>{}</w:r></w:p>",
        // Index 1000 of the fills is no fill (§20.1.4.2.10).
        styled_box(
            "",
            r#"<a:fillRef idx="1000"><a:schemeClr val="accent1"/></a:fillRef>"#
        ),
        // A colour above full opacity is opaque; a width at its bound is taken.
        styled_box(
            r#"<a:solidFill><a:srgbClr val="FF0000"><a:alpha val="150000"/></a:srgbClr></a:solidFill><a:ln w="20116800"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln>"#,
            "",
        ),
        // A transform of a colour of the scheme is checked as the document opens, before
        // the theme is read.
        styled_box(
            r#"<a:solidFill><a:schemeClr val="accent1"><a:lumMod val="much"/></a:schemeClr></a:solidFill>"#,
            "",
        ),
        styled_box(
            "",
            r#"<a:fillRef idx="3"><a:schemeClr val="accent1"/></a:fillRef>"#
        ),
        pict(&format!(
            r##"<v:shapetype id="t" filled="maybe"/>{}{}"##,
            // Word's names of dashes, in its own case, and recoloured colours.
            r##"<v:shape type="#t" style="width:1pt;height:1pt" fillcolor="#808080 darken(128)" strokecolor="rgb(95, 39, 38) lighten(128)"><v:stroke dashstyle="longDashDot"/><v:textbox/></v:shape>"##,
            r##"<v:shape type="#t" style="width:1pt;height:1pt" fillcolor="blurple darken(50)"><v:fill type="gradient" colors="0 #936545;.5 #d49366;1 #fdaf7b" opacity="2"/><v:textbox/></v:shape><v:shape type="#t" style="width:1pt;height:1pt"><v:textbox/></v:shape>"##,
        )),
    );
    let document = with_parts(&body, &[(kind, target, theme)]);
    let boxes = drawings(&document);
    assert_eq!(fill_of(boxes[0].text_box().unwrap()), None);
    let opaque = boxes[1].text_box().unwrap();
    assert_eq!(
        fill_of(opaque),
        Some((FillKind::Solid, Some((0xFF, 0, 0)), 1.0))
    );
    assert_eq!(opaque.outline().unwrap().width(), Pt(1584.0));
    assert_eq!(
        fill_of(boxes[2].text_box().unwrap()),
        Some((FillKind::Solid, Some((0x4F, 0x81, 0xBD)), 1.0))
    );
    let vml = vml_text_boxes(&document);
    let first = vml[0];
    assert_eq!(
        fill_of(first),
        Some((FillKind::Solid, Some((0x40, 0x40, 0x40)), 1.0))
    );
    let line = first.outline().unwrap();
    // 255 - (255 - c) · 128 / 256 for each channel of 5F2726.
    assert_eq!(
        (line.color(), line.dash()),
        (Some((0xAF, 0x93, 0x93)), LineDash::LongDashDot)
    );
    // A gradient gives its first stop; an opacity above 1 is opaque.
    assert_eq!(
        fill_of(vml[1]),
        Some((FillKind::Gradient, Some((0x93, 0x65, 0x45)), 1.0))
    );
    let reported: Vec<&str> = document.diagnostics().iter().map(|d| d.message()).collect();
    let ends = |end: &str| reported.iter().filter(|line| line.ends_with(end)).count();
    assert_eq!(
        ends(r#"a:lumMod val="much" is not valid; it is ignored"#),
        1,
        "{reported:?}"
    );
    assert_eq!(
        ends(r#"v:fill opacity="2" is not valid; it is ignored"#),
        0,
        "{reported:?}"
    );
    // A shape type's value is reported once, where it is defined, not for each of its
    // three shapes.
    assert_eq!(
        ends(r#"v:shapetype filled="maybe" is not valid; it is ignored"#),
        1,
        "{reported:?}"
    );
    assert_eq!(
        ends(r#"v:shape fillcolor="blurple darken(50)" is not valid; it is ignored"#),
        1,
        "{reported:?}"
    );
    // The theme's format scheme is checked as the theme is read.
    assert!(
        reported
            .iter()
            .any(|line| line.starts_with("/word/theme/theme1.xml")
                && line.ends_with(r#"a:srgbClr val="zz" is not valid; it is ignored"#)),
        "{reported:?}"
    );
    assert_eq!(reported.len(), 4, "{reported:?}");
}

#[test]
fn takes_index_1000_as_no_fill_and_darkens_by_a_256th() {
    // From the mutations of the review of #471: index 1000 is no fill even in a theme of a
    // thousand fills (§20.1.4.2.10), and VML's darken scales by its amount / 256
    // (MS-OI29500 2.1.1893(h)): white darkened by 255 is FEFEFE.
    let fills = r#"<a:solidFill><a:srgbClr val="00FF00"/></a:solidFill>"#.repeat(1_000);
    let body = format!(
        "<w:p>{}{}<w:r>{}</w:r></w:p>",
        styled_box(
            "",
            r#"<a:fillRef idx="1000"><a:schemeClr val="accent1"/></a:fillRef>"#
        ),
        styled_box(
            "",
            r#"<a:fillRef idx="999"><a:schemeClr val="accent1"/></a:fillRef>"#
        ),
        pict(&vml_box("width:1pt;height:1pt", "", &p("Dark"),).replace(
            "<v:shape ",
            r##"<v:shape fillcolor="#ffffff darken(255)" "##
        )),
    );
    let document = with_parts(&body, &[theme_with_fills(&fills)]);
    let boxes = drawings(&document);
    assert_eq!(fill_of(boxes[0].text_box().unwrap()), None);
    assert_eq!(
        fill_of(boxes[1].text_box().unwrap()),
        Some((FillKind::Solid, Some((0, 0xFF, 0)), 1.0))
    );
    assert_eq!(
        fill_of(vml_text_boxes(&document)[0]),
        Some((FillKind::Solid, Some((0xFE, 0xFE, 0xFE)), 1.0))
    );
}

/// `xml` with `from` replaced by `to` where it occurs, which it must.
fn replaced(xml: &str, from: &str, to: &str) -> String {
    assert!(xml.contains(from), "{from}");
    xml.replacen(from, to, 1)
}

#[test]
fn resolves_the_colours_of_theme_styles_without_reading_the_theme_again() {
    // From the re-check of #471: a colour of the theme's format scheme with many children
    // of another namespace, or content inside a transform, cost every box whose style
    // names it the size of the colour (8,000 boxes took 56 s); also for a line and a
    // background fill.
    let (junk, count) = (20_000, 50);
    let foreign = "<x:j/>".repeat(junk);
    let fills = format!(
        r#"<a:solidFill><a:schemeClr val="phClr"><a:lumMod val="50000"/>{foreign}</a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:lumMod val="50000">{}</a:lumMod></a:schemeClr></a:solidFill>"#,
        "<a:j/>".repeat(junk)
    );
    let (kind, target, theme) = theme_with_fills(&fills);
    let theme = replaced(
        &theme,
        r#" name="T">"#,
        r#" xmlns:x="urn:example:junk" name="T">"#,
    );
    let theme = replaced(
        &theme,
        r#"<a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/>"#,
        &format!(
            r#"<a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"><a:shade val="50000"/>{foreign}</a:schemeClr>"#
        ),
    );
    let theme = replaced(
        &theme,
        r#"<a:alpha val="50000"/></a:schemeClr></a:solidFill></a:bgFillStyleLst>"#,
        &format!(
            r#"<a:tint val="50000"/>{foreign}</a:schemeClr></a:solidFill></a:bgFillStyleLst>"#
        ),
    );
    let style = |references: &str| styled_box("", references);
    let body = format!(
        "<w:p>{}{}{}{}{}</w:p>",
        // The same colours as the shapes' own.
        styled_box(
            r#"<a:solidFill><a:schemeClr val="accent1"><a:lumMod val="50000"/></a:schemeClr></a:solidFill><a:ln w="6350"><a:solidFill><a:schemeClr val="accent1"><a:shade val="50000"/></a:schemeClr></a:solidFill></a:ln>"#,
            ""
        ),
        styled_box(
            r#"<a:solidFill><a:schemeClr val="accent1"><a:tint val="50000"/></a:schemeClr></a:solidFill>"#,
            ""
        ),
        style(
            r#"<a:lnRef idx="1"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef>"#
        )
        .repeat(count),
        style(r#"<a:fillRef idx="2"><a:schemeClr val="accent1"/></a:fillRef>"#).repeat(count),
        style(r#"<a:fillRef idx="1001"><a:schemeClr val="accent1"/></a:fillRef>"#).repeat(count),
    );
    let document = with_parts(&body, &[(kind, target, theme)]);
    let drawings = drawings(&document);
    let own = drawings[0].text_box().unwrap();
    let (darker, shaded) = (fill_of(own), outline_of(own).map(|line| line.1));
    let lighter = fill_of(drawings[1].text_box().unwrap());
    assert_eq!(shaded, Some(Some((0x38, 0x5D, 0x8A))));
    assert_eq!(
        lighter,
        Some((FillKind::Solid, Some((0xC2, 0xCD, 0xE1)), 1.0))
    );
    crate::model::xml::STEPS.set(0);
    let styled: Vec<_> = drawings[2..]
        .iter()
        .map(|drawing| {
            let text_box = drawing.text_box().unwrap();
            (fill_of(text_box), outline_of(text_box).map(|line| line.1))
        })
        .collect();
    let steps = crate::model::xml::STEPS.get();
    let expected: Vec<_> = [(darker, shaded), (darker, None), (lighter, None)]
        .into_iter()
        .flat_map(|values| std::iter::repeat_n(values, count))
        .collect();
    assert_eq!(styled, expected);
    assert!(
        steps <= 1_000 * 3 * count,
        "{steps} steps for {} boxes",
        3 * count
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn reports_a_vml_colour_of_several_recolouring_instructions_without_recursion() {
    // From the re-check of #471: each instruction was read by a call of its own, with a copy
    // of the rest of the value, so that 10,000 of them (a value of 110 KB) overflowed the
    // stack. One instruction follows the colour it changes (MS-OI29500 2.1.1893(l)).
    let many = format!("#ffffff{}", " darken(255)".repeat(10_000));
    let value = many.clone();
    let read = std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || crate::model::color::vml_color(&value))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(read, None);
    let shape = |color: &str| {
        format!(
            r#"<v:shape style="width:1pt;height:1pt" fillcolor="{color}"><v:textbox/></v:shape>"#
        )
    };
    let body = format!(
        "<w:p><w:r>{}</w:r></w:p>",
        pict(
            &[
                shape(&many),
                shape("#808080 darken(128) darken(128)"),
                shape("none darken(50)"),
                shape("#808080 darken(128)"),
            ]
            .concat()
        )
    );
    let document = open(&body);
    let fills: Vec<_> = vml_text_boxes(&document).into_iter().map(fill_of).collect();
    // A value not taken is read as not written: white.
    let white = Some((FillKind::Solid, Some((0xFF, 0xFF, 0xFF)), 1.0));
    assert_eq!(
        fills,
        [
            white,
            white,
            white,
            Some((FillKind::Solid, Some((0x40, 0x40, 0x40)), 1.0))
        ]
    );
    let reported = reported(&document);
    assert_eq!(reported.len(), 3, "{reported:?}");
    assert!(
        reported[0].starts_with(r##"v:shape fillcolor="#ffffff darken(255) darken(255)"##)
            && reported[0].ends_with("is not valid; it is ignored"),
        "{reported:?}"
    );
    assert_eq!(
        reported[1..],
        [
            r##"v:shape fillcolor="#808080 darken(128) darken(128)" is not valid; it is ignored"##,
            r#"v:shape fillcolor="none darken(50)" is not valid; it is ignored"#,
        ]
    );
}

#[test]
fn names_the_background_fills_from_1001_after_the_effect_styles() {
    // From the re-check of #471: the effect styles, which come between the lines and the
    // background fills, were taken for background fills.
    let (kind, target, theme) = shape_theme();
    let background = format!(
        "{}<a:noFill/>",
        ["FF0000", "00FF00", "0000FF"]
            .map(|rgb| format!(r#"<a:solidFill><a:srgbClr val="{rgb}"/></a:solidFill>"#))
            .concat()
    );
    let theme = replaced(
        &theme,
        r#"<a:solidFill><a:schemeClr val="phClr"><a:alpha val="50000"/></a:schemeClr></a:solidFill></a:bgFillStyleLst>"#,
        &format!("{background}</a:bgFillStyleLst>"),
    );
    let body = format!(
        "<w:p>{}</w:p>",
        (1001..=1005)
            .map(|index| styled_box(
                "",
                &format!(r#"<a:fillRef idx="{index}"><a:schemeClr val="accent1"/></a:fillRef>"#)
            ))
            .collect::<String>()
    );
    let document = with_parts(&body, &[(kind, target, theme)]);
    let fills: Vec<_> = drawings(&document)
        .iter()
        .map(|drawing| fill_of(drawing.text_box().unwrap()))
        .collect();
    let solid = |rgb| Some((FillKind::Solid, Some(rgb), 1.0));
    assert_eq!(
        fills,
        [
            solid((0xFF, 0, 0)),
            solid((0, 0xFF, 0)),
            solid((0, 0, 0xFF)),
            None,
            None
        ]
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn bounds_the_transforms_of_theme_lines_and_reports_the_colours_cut_once() {
    // 70 steps of 1 % of luminance from black: the first 64 make A3A3A3.
    let fill = format!(
        r#"<a:solidFill><a:srgbClr val="000000">{}</a:srgbClr></a:solidFill>"#,
        r#"<a:lumOff val="1000"/>"#.repeat(70)
    );
    let (kind, target, theme) = theme_with_fills(&fill.repeat(2));
    let theme = replaced(
        &theme,
        r#"<a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>"#,
        &format!(r#"<a:ln w="6350">{fill}</a:ln>"#),
    );
    let body = format!(
        "<w:p>{}</w:p>",
        styled_box(
            "",
            r#"<a:lnRef idx="1"><a:schemeClr val="accent1"/></a:lnRef>"#
        )
    );
    let document = with_parts(&body, &[(kind, target, theme)]);
    let line = drawings(&document)[0]
        .text_box()
        .unwrap()
        .outline()
        .unwrap();
    assert_eq!(line.color(), Some((0xA3, 0xA3, 0xA3)));
    let messages: Vec<&str> = document.diagnostics().iter().map(|d| d.message()).collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("/word/theme/theme1.xml")
            && messages[0].ends_with(
                "3 colours of a:fmtScheme have more than 64 transforms; those after are left out"
            ),
        "{messages:?}"
    );
}

#[test]
fn takes_the_first_shape_type_of_an_id_and_reports_those_of_embedded_objects() {
    // Word defines a shape type once, in the first drawing that names it; a later
    // definition of the same id does not change it.
    let drawing = |rgb: &str| {
        format!(
            "<w:r>{}</w:r>",
            pict(&format!(
                r##"<v:shapetype id="t" fillcolor="#{rgb}"/><v:shape type="#t" style="width:1pt;height:1pt"><v:textbox/></v:shape>"##
            ))
        )
    };
    let body = format!(
        r##"<w:p>{}{}<w:r><w:object {SHAPES} w:dxaOrig="200" w:dyaOrig="200"><v:shapetype id="o" filled="maybe"/><v:shape type="#o" style="width:10pt;height:10pt"/></w:object></w:r></w:p>"##,
        drawing("ff0000"),
        drawing("0000ff")
    );
    let document = open(&body);
    let fills: Vec<_> = vml_text_boxes(&document).into_iter().map(fill_of).collect();
    let red = Some((FillKind::Solid, Some((0xFF, 0, 0)), 1.0));
    assert_eq!(fills, [red, red]);
    // The values of an embedded object's shape types are reported too.
    assert_eq!(
        reported(&document),
        [r#"v:shapetype filled="maybe" is not valid; it is ignored"#]
    );
}

#[test]
fn reads_vml_dash_lengths_of_a_preset_s_pattern_as_that_preset() {
    // Word writes sysDot and sysDash in VML as lengths (`1 1`, `3 1`).
    let shapes: String = [
        "1 3",
        "4 3",
        "8 3",
        "4 3 1 3",
        "8 3 1 3",
        "8 3 1 3 1 3",
        "3 1",
        "1 1",
        "3 1 1 1",
        "3 1 1 1 1 1",
        "2 2",
        "1 x",
        "",
    ]
        .map(|dashes| {
            format!(
                r#"<v:shape style="width:1pt;height:1pt"><v:stroke dashstyle="{dashes}"/><v:textbox/></v:shape>"#
            )
        })
        .concat();
    let document = open(&format!("<w:p><w:r>{}</w:r></w:p>", pict(&shapes)));
    let dashes: Vec<_> = vml_text_boxes(&document)
        .into_iter()
        .map(|text_box| text_box.outline().unwrap().dash())
        .collect();
    assert_eq!(
        dashes,
        [
            LineDash::Dot,
            LineDash::Dash,
            LineDash::LongDash,
            LineDash::DashDot,
            LineDash::LongDashDot,
            LineDash::LongDashDotDot,
            LineDash::SystemDash,
            LineDash::SystemDot,
            LineDash::SystemDashDot,
            LineDash::SystemDashDotDot,
            LineDash::Custom,
            LineDash::Solid,
            LineDash::Solid
        ]
    );
    assert_eq!(
        reported(&document),
        [
            r#"v:stroke dashstyle="1 x" is not valid; it is ignored"#,
            r#"v:stroke dashstyle="" is not valid; it is ignored"#
        ]
    );
}

#[test]
fn takes_a_gradient_s_first_stop_only_with_its_position() {
    let body = format!(
        "<w:p><w:r>{}</w:r></w:p>",
        pict(
            r##"<v:shape style="width:1pt;height:1pt" fillcolor="#0000ff"><v:fill type="gradient" colors="x #ff0000;1 #00ff00"/><v:textbox/></v:shape>"##
        )
    );
    let document = open(&body);
    assert_eq!(
        fill_of(vml_text_boxes(&document)[0]),
        Some((FillKind::Gradient, Some((0, 0, 0xFF)), 1.0))
    );
    assert_eq!(
        reported(&document),
        [r##"v:fill colors="x #ff0000;1 #00ff00" is not valid; it is ignored"##]
    );
}

#[test]
fn reads_a_shape_type_s_values_once_however_many_shapes_name_it() {
    // From the re-check of #471: each shape read its shape type's values again, so that a
    // type whose colours were 2 MB long, named by 50,000 shapes, took 31 s to read.
    let (n, long) = (2_000, 200_000);
    let color = format!("#ff0000{}", " ".repeat(long));
    let body = format!(
        "<w:p><w:r>{}</w:r></w:p>",
        pict(&format!(
            r##"<v:shapetype id="t" fillcolor="{color}" strokecolor="{color}"/>{}"##,
            r##"<v:shape type="#t" style="width:1pt;height:1pt"><v:textbox/></v:shape>"##.repeat(n)
        ))
    );
    let document = open(&body);
    let boxes = vml_text_boxes(&document);
    assert_eq!(boxes.len(), n);
    crate::model::drawing::VALUE_BYTES.set(0);
    let red = boxes
        .iter()
        .filter(|text_box| {
            fill_of(**text_box).and_then(|fill| fill.1) == Some((0xFF, 0, 0))
                && text_box.outline().and_then(|line| line.color()) == Some((0xFF, 0, 0))
        })
        .count();
    let read = crate::model::drawing::VALUE_BYTES.get();
    assert_eq!(red, n);
    // Read again for each shape, the type's values would be 400 KB a shape.
    assert!(
        read <= 100 * n,
        "{read} bytes of values read for {n} shapes"
    );
}

#[test]
fn reports_the_values_of_transforms_not_applied() {
    // The transforms of single channels are not applied, but a damaged value of theirs is
    // reported, in a shape and in the theme.
    let (kind, target, theme) = theme_with_fills(
        r#"<a:solidFill><a:srgbClr val="00FF00"><a:redMod val="much"/></a:srgbClr></a:solidFill>"#,
    );
    let body = format!(
        "<w:p>{}{}</w:p>",
        styled_box(
            r#"<a:solidFill><a:srgbClr val="0000FF"><a:red val="zz"/><a:greenMod val="50000"/><a:gamma/></a:srgbClr></a:solidFill>"#,
            ""
        ),
        styled_box(
            "",
            r#"<a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef>"#
        )
    );
    let document = with_parts(&body, &[(kind, target, theme)]);
    let boxes = drawings(&document);
    assert_eq!(
        fill_of(boxes[0].text_box().unwrap()),
        Some((FillKind::Solid, Some((0, 0, 0xFF)), 1.0))
    );
    assert_eq!(
        fill_of(boxes[1].text_box().unwrap()),
        Some((FillKind::Solid, Some((0, 0xFF, 0)), 1.0))
    );
    let messages: Vec<&str> = document.diagnostics().iter().map(|d| d.message()).collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        messages.iter().any(|m| m.starts_with("/word/document.xml")
            && m.ends_with(r#"a:red val="zz" is not valid; it is ignored"#)),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("/word/theme/theme1.xml")
                && m.ends_with(r#"redMod val="much" is not valid; it is ignored"#)),
        "{messages:?}"
    );
}
