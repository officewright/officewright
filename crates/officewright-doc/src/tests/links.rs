//! Hyperlinks, `HYPERLINK` fields and bookmarks in the translation into the layout IR
//! (#570).

use officewright_layout::ir;

use super::layout::{contents, describe, paragraphs, translate, work_of};
use super::*;

const HYPERLINK: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

/// A document whose relationships give `rId1`, `rId2`… the external targets `targets`.
fn with_links(body: &str, targets: &[&str]) -> Document {
    let rels: String = targets
        .iter()
        .enumerate()
        .map(|(n, target)| {
            format!(
                r#"<Relationship Id="rId{}" Type="{HYPERLINK}" Target="{target}" TargetMode="External"/>"#,
                n + 1
            )
        })
        .collect();
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#
    );
    Document::from_bytes(package(
        DOCUMENT,
        &document_xml(body),
        &[("word/_rels/document.xml.rels", &rels)],
    ))
    .unwrap()
}

/// The inlines of every paragraph of the first section, those of tables' cells in their
/// order among them, briefly.
fn all_contents(translation: &Translation) -> Vec<Vec<String>> {
    fn walk(blocks: &[ir::Block], out: &mut Vec<Vec<String>>) {
        for block in blocks {
            match block {
                ir::Block::Paragraph(paragraph) => {
                    out.push(paragraph.inlines().iter().map(describe).collect());
                }
                ir::Block::Table(table) => {
                    for row in table.rows() {
                        for cell in row.cells() {
                            walk(cell.blocks(), out);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(translation.document().sections()[0].blocks(), &mut out);
    out
}

/// The messages of the entries the translation listed.
fn messages(translation: &Translation) -> Vec<&str> {
    translation
        .left_out()
        .iter()
        .map(|entry| entry.message())
        .collect()
}

#[test]
fn makes_a_link_of_each_hyperlink_to_its_target_its_target_and_anchor_or_its_anchor() {
    let document = with_links(
        r#"<w:p><w:hyperlink r:id="rId1"><w:r><w:t>web</w:t></w:r></w:hyperlink><w:hyperlink r:id="rId1" w:anchor="Part"><w:r><w:t>part</w:t></w:r></w:hyperlink><w:hyperlink w:anchor="Terms"><w:r><w:t>terms</w:t></w:r></w:hyperlink><w:hyperlink><w:r><w:t>none</w:t></w:r></w:hyperlink><w:hyperlink w:anchor=""><w:r><w:t>empty</w:t></w:r></w:hyperlink><w:hyperlink r:id="rId1"><w:hyperlink w:anchor="Terms"><w:r><w:t>inner</w:t></w:r></w:hyperlink></w:hyperlink></w:p><w:p><w:bookmarkStart w:id="0" w:name="Terms"/><w:r><w:t>Terms</w:t></w:r></w:p>"#,
        &["https://example.com/a"],
    );
    let translation = translate(&document);
    // Word appends `#` and the anchor to a target (MS-OI29500 §17.16.22(b)). A hyperlink
    // with neither gets no link; one inside another, a link inside the other's.
    assert_eq!(
        contents(&translation),
        [
            vec![
                r#"LinkStart { target: Uri("https://example.com/a") }"#,
                "text web",
                "LinkEnd",
                r#"LinkStart { target: Uri("https://example.com/a#Part") }"#,
                "text part",
                "LinkEnd",
                r#"LinkStart { target: Bookmark("Terms") }"#,
                "text terms",
                "LinkEnd",
                "text none",
                "text empty",
                r#"LinkStart { target: Uri("https://example.com/a") }"#,
                r#"LinkStart { target: Bookmark("Terms") }"#,
                "text inner",
                "LinkEnd",
                "LinkEnd",
            ],
            vec![r#"Bookmark { name: "Terms" }"#, "text Terms"],
        ]
    );
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
}

#[test]
fn makes_a_link_of_each_hyperlink_field_around_its_result() {
    let field = |instruction: &str, result: &str| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{instruction}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{result}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    let body = format!(
        r#"<w:p>{}{}{}</w:p><w:p><w:fldSimple w:instr=" HYPERLINK \l &quot;Terms&quot; "><w:r><w:t>simple</w:t></w:r></w:fldSimple></w:p><w:p><w:bookmarkStart w:id="0" w:name="Terms"/></w:p>"#,
        field(r#" HYPERLINK "https://example.com/b" "#, "web"),
        field(
            r#" hyperlink "https://example.com/b" \L "Sec" \o "Tip" "#,
            "both"
        ),
        field(" PAGE ", "1"),
    );
    let document = open(&body);
    let translation = translate(&document);
    let paragraphs = contents(&translation);
    // The link starts with the result and ends with it, inside the field; the field's
    // name and switches are read in any case.
    assert!(
        paragraphs[0][0]
            .contains(r#"[LinkStart { target: Uri("https://example.com/b") }, text web, LinkEnd]"#),
        "{:?}",
        paragraphs[0]
    );
    assert!(
        paragraphs[0][1].contains(
            r#"[LinkStart { target: Uri("https://example.com/b#Sec") }, text both, LinkEnd]"#
        ),
        "{:?}",
        paragraphs[0]
    );
    assert!(
        !paragraphs[0][2].contains("LinkStart"),
        "{:?}",
        paragraphs[0]
    );
    assert!(
        paragraphs[1][0]
            .contains(r#"[LinkStart { target: Bookmark("Terms") }, text simple, LinkEnd]"#),
        "{:?}",
        paragraphs[1]
    );
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
}

#[test]
fn shows_a_hyperlink_field_too_long_to_read_without_its_link_and_says_so_once() {
    // The instruction is read up to 33 KiB: room for the longest target a PDF holds,
    // 32,767 bytes, with the field's name, quotes and switches.
    let field = |length: usize| {
        let instruction = format!(
            r#" HYPERLINK "https://example.com/{}" \o "tip" "#,
            "a".repeat(length - 20)
        );
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{instruction}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>long</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    let document = open(&format!(
        "<w:p>{}{}{}</w:p>",
        field(33 << 10),
        field(33 << 10),
        field(32_767)
    ));
    let translation = translate(&document);
    let fields = &contents(&translation)[0];
    assert!(!fields[0].contains("LinkStart"), "{:.200}", fields[0]);
    assert!(!fields[1].contains("LinkStart"), "{:.200}", fields[1]);
    // A target of 32,767 bytes is read whole.
    assert!(fields[2].contains("LinkStart"), "{:.200}", fields[2]);
    assert_eq!(
        messages(&translation),
        [
            "a HYPERLINK field whose instruction is longer than 33 KiB is shown without its link, here and wherever else the document has one"
        ]
    );
}

#[test]
fn marks_the_bookmark_word_keeps_of_each_name_where_its_text_starts() {
    let document = open(
        r#"<w:p><w:bookmarkStart w:id="1" w:name="Dup"/><w:r><w:t>first</w:t></w:r></w:p><w:bookmarkStart w:id="2" w:name="Block"/><w:p><w:bookmarkStart w:id="3" w:name="Dup"/><w:r><w:t>second</w:t></w:r><w:bookmarkStart w:id="4" w:name=""/><w:bookmarkStart w:id="5"/></w:p><w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:bookmarkStart w:id="6" w:name="Row"/><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:bookmarkStart w:id="7" w:name="Cell"/><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let translation = translate(&document);
    // Word keeps the last bookmark of a name (MS-OI29500 §17.13.6.2(c)), and none
    // without a name. One among blocks, between rows or between cells starts at the next
    // paragraph.
    assert_eq!(
        all_contents(&translation),
        [
            vec!["text first"],
            vec![
                r#"Bookmark { name: "Block" }"#,
                r#"Bookmark { name: "Dup" }"#,
                "text second",
            ],
            vec![r#"Bookmark { name: "Row" }"#, "text a"],
            vec![r#"Bookmark { name: "Cell" }"#, "text b"],
            vec![],
        ]
    );
}

#[test]
fn makes_the_target_of_an_address_once_for_all_its_links() {
    // A target of 1,000,000 bytes: 33 distinct addresses fit into the 32 MiB the
    // translation makes, the 34th does not. Links to one address count once.
    let target = format!("https://example.com/{}", "a".repeat(1_000_000 - 20));
    let links = |anchor: &dyn Fn(usize) -> String| -> String {
        (0..40)
            .map(|n| {
                format!(
                    r#"<w:hyperlink r:id="rId1" w:anchor="{}"><w:r><w:t>{n}</w:t></w:r></w:hyperlink>"#,
                    anchor(n)
                )
            })
            .collect()
    };
    let linked = |translation: &Translation| {
        paragraphs(translation)[0]
            .inlines()
            .iter()
            .filter(|inline| matches!(inline, ir::Inline::LinkStart { .. }))
            .count()
    };
    let same = with_links(
        &format!("<w:p>{}</w:p>", links(&|_| "A".to_owned())),
        &[&target],
    );
    // Each link costs its anchor, not the target it shares (#627): the target is made once.
    let (translation, made) = work_of(|work| work.link_target_bytes, || translate(&same));
    assert_eq!(made, target.len() + 2);
    let (_, looked_up) = work_of(|work| work.link_key_bytes, || translate(&same));
    assert_eq!(looked_up, 40);
    assert_eq!(linked(&translation), 40);
    // The IR's marks share the one target.
    let targets = |translation: &Translation| -> Vec<std::sync::Arc<ir::LinkTarget>> {
        paragraphs(translation)[0]
            .inlines()
            .iter()
            .filter_map(|inline| match inline {
                ir::Inline::LinkStart { target, .. } => Some(std::sync::Arc::clone(target)),
                _ => None,
            })
            .collect()
    };
    let shared = targets(&translation);
    assert!(
        shared
            .iter()
            .all(|target| std::sync::Arc::ptr_eq(target, &shared[0]))
    );
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
    let distinct = with_links(
        &format!("<w:p>{}</w:p>", links(&|n| format!("A{n}"))),
        &[&target],
    );
    // A link to an address past the budget makes nothing.
    let (translation, made) = work_of(|work| work.link_target_bytes, || translate(&distinct));
    assert!(made < 33 * (target.len() + 4), "{made}");
    assert_eq!(linked(&translation), 33);
    let apart = targets(&translation);
    assert!(!std::sync::Arc::ptr_eq(&apart[0], &apart[1]));
    assert_eq!(
        messages(&translation),
        [
            "links past 32 MiB of web addresses and other targets are left out, here and wherever else the document has one"
        ]
    );
}

#[test]
fn ends_the_last_paragraph_of_a_story_cell_or_body_with_the_bookmarks_after_it() {
    // A bookmark after a story's, a cell's or the body's last block stays there, at the end
    // of the last paragraph, and does not start the next paragraph translated, which may
    // be another story's or another cell's. One left between a section's body and the
    // next is not taken by that section's header, translated before its body.
    // Each section has a header of its own, so that the second is first translated with
    // a bookmark of the body pending.
    let header = |n: u32| {
        format!(
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:r><w:t>header</w:t></w:r></w:p><w:bookmarkStart w:id="1{n}" w:name="InHeader{n}"/></w:hdr>"#
        )
    };
    let reference = |n: u32| format!(r#"<w:headerReference w:type="default" r:id="rIdH{n}"/>"#);
    let body = format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p><w:bookmarkStart w:id="2" w:name="EndOfCell"/></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t>first</w:t></w:r></w:p><w:bookmarkStart w:id="3" w:name="BeforeBreak"/><w:p><w:pPr><w:sectPr>{}</w:sectPr></w:pPr></w:p><w:bookmarkStart w:id="4" w:name="Between"/><w:p><w:r><w:t>second</w:t></w:r></w:p><w:bookmarkStart w:id="5" w:name="EndOfBody"/><w:sectPr>{}</w:sectPr>"#,
        reference(1),
        reference(2)
    );
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdH1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/><Relationship Id="rIdH2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header2.xml"/></Relationships>"#;
    let document = Document::from_bytes(package(
        DOCUMENT,
        &document_xml(&body),
        &[
            ("word/_rels/document.xml.rels", rels),
            ("word/header1.xml", &header(1)),
            ("word/header2.xml", &header(2)),
        ],
    ))
    .unwrap();
    let translation = translate(&document);
    let sections = translation.document().sections();
    let text = |blocks: &[ir::Block]| -> Vec<Vec<String>> {
        blocks
            .iter()
            .filter_map(|block| match block {
                ir::Block::Paragraph(paragraph) => {
                    Some(paragraph.inlines().iter().map(describe).collect())
                }
                ir::Block::Table(table) => Some(
                    table.rows()[0]
                        .cells()
                        .iter()
                        .flat_map(|cell| cell.blocks())
                        .filter_map(|block| match block {
                            ir::Block::Paragraph(paragraph) => Some(
                                paragraph
                                    .inlines()
                                    .iter()
                                    .map(describe)
                                    .collect::<Vec<_>>()
                                    .join(" | "),
                            ),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        text(sections[0].blocks()),
        [
            vec![
                r#"text a | Bookmark { name: "EndOfCell" }"#.to_owned(),
                "text b".to_owned()
            ],
            vec!["text first".to_owned()],
            vec![r#"Bookmark { name: "BeforeBreak" }"#.to_owned()],
        ]
    );
    assert_eq!(
        text(sections[1].blocks()),
        [vec![
            r#"Bookmark { name: "Between" }"#.to_owned(),
            "text second".to_owned(),
            r#"Bookmark { name: "EndOfBody" }"#.to_owned(),
        ]]
    );
    // The header of each section ends with its own bookmark, and takes none of the
    // body's.
    for (n, section) in sections.iter().enumerate() {
        assert_eq!(
            text(section.headers().primary()),
            [vec![
                "text header".to_owned(),
                format!(r#"Bookmark {{ name: "InHeader{}" }}"#, n + 1)
            ]]
        );
    }
}

#[test]
fn reads_the_result_of_a_field_inside_a_hyperlink_field_s_instruction() {
    // Word reads a field nested in an instruction as its result: the mail-merge form
    // `{ HYPERLINK { MERGEFIELD Address } }`, and a target in part from a field.
    let nested = |before: &str, after: &str, result: &str| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{before}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Address </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{result}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve">{after}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>link</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    let document = open(&format!(
        "<w:p>{}{}</w:p>",
        nested(" HYPERLINK ", " ", "https://example.com/c"),
        nested(r#" HYPERLINK "mailto:"#, r#"" "#, "office@example.com"),
    ));
    let translation = translate(&document);
    let fields = &contents(&translation)[0];
    assert!(
        fields[0].contains(r#"LinkStart { target: Uri("https://example.com/c") }"#),
        "{}",
        fields[0]
    );
    assert!(
        fields[1].contains(r#"LinkStart { target: Uri("mailto:office@example.com") }"#),
        "{}",
        fields[1]
    );
}

#[test]
fn moves_the_marks_in_the_result_of_a_field_the_layout_computes_after_it() {
    // A hyperlink that ends inside a PAGE field's result ends after the field, whose
    // result the layout replaces; so does a bookmark there.
    let document = open(
        r#"<w:p><w:hyperlink w:anchor="Top"><w:r><w:t>page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:bookmarkStart w:id="1" w:name="Top"/><w:r><w:t>1</w:t></w:r></w:hyperlink><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t> after</w:t></w:r></w:p>"#,
    );
    let translation = translate(&document);
    let inlines = &contents(&translation)[0];
    assert_eq!(inlines[0], r#"LinkStart { target: Bookmark("Top") }"#);
    assert_eq!(inlines[1], "text page ");
    assert!(inlines[2].starts_with("field Page"), "{}", inlines[2]);
    assert!(!inlines[2].contains("Link"), "{}", inlines[2]);
    assert_eq!(
        &inlines[3..],
        [r#"Bookmark { name: "Top" }"#, "LinkEnd", "text  after"]
    );
}

#[test]
fn counts_the_names_of_bookmarks_in_headers_against_the_copies_of_headers() {
    // A header that holds little else than a long bookmark name, in many sections: its
    // copies are bounded like those of any other content (MAX_COPIED is 1 MiB in tests).
    let name = "n".repeat(300_000);
    let header = format!(
        r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:bookmarkStart w:id="1" w:name="{name}"/></w:p></w:hdr>"#
    );
    let reference = r#"<w:headerReference w:type="default" r:id="rIdH"/>"#;
    let body: String = (0..12)
        .map(|_| format!(r#"<w:p><w:pPr><w:sectPr>{reference}</w:sectPr></w:pPr></w:p>"#))
        .collect::<String>()
        + &format!("<w:sectPr>{reference}</w:sectPr>");
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdH" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/></Relationships>"#;
    let document = Document::from_bytes(package(
        DOCUMENT,
        &document_xml(&body),
        &[
            ("word/_rels/document.xml.rels", rels),
            ("word/header1.xml", &header),
        ],
    ))
    .unwrap();
    let translation = translate(&document);
    let with_header = translation
        .document()
        .sections()
        .iter()
        .filter(|section| !section.headers().primary().is_empty())
        .count();
    assert!(with_header <= 4, "{with_header}");
    assert!(
        messages(&translation)
            .iter()
            .any(|message| message.contains("the copies of headers and footers")),
        "{:?}",
        messages(&translation)
    );
}

#[test]
fn shares_one_target_between_a_hyperlink_and_a_hyperlink_field_to_one_address() {
    let document = with_links(
        r#"<w:p><w:hyperlink r:id="rId1"><w:r><w:t>link</w:t></w:r></w:hyperlink><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK "https://example.com/a" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>field</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        &["https://example.com/a"],
    );
    let translation = translate(&document);
    let inlines = paragraphs(&translation)[0].inlines();
    let Some(ir::Inline::LinkStart { target: link, .. }) = inlines.first() else {
        panic!("{inlines:?}");
    };
    let Some(ir::Inline::Field(field)) = inlines.get(3) else {
        panic!("{inlines:?}");
    };
    let Some(ir::Inline::LinkStart {
        target: in_field, ..
    }) = field.result().first()
    else {
        panic!("{field:?}");
    };
    assert!(std::sync::Arc::ptr_eq(link, in_field));
}
