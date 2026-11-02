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
    let inner = format!("<w:p><w:r>{}</w:r></w:p>", drawing(&txbx(&p("Inner"))));
    let body = format!(
        "<w:p><w:r>{}</w:r></w:p>{}",
        drawing(&txbx(&format!("{}{inner}", p("Outer")))),
        p("Body"),
    );
    let footnotes = format!(
        r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1"><w:p><w:r>{}</w:r></w:p></w:footnote></w:footnotes>"#,
        pict(&vml_box("width:10pt;height:10pt", "", &p("In a note"))),
    );
    let document = with_parts(&body, &[("footnotes", "footnotes.xml", footnotes)]);
    let texts_of_body: Vec<String> = paragraphs(&document).iter().map(|p| p.text()).collect();
    assert_eq!(texts_of_body, ["", "Body"]);
    let outer = drawings(&document)[0].text_box().unwrap();
    assert_eq!(texts(outer), ["Outer", ""]);
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
