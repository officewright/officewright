//! Hyperlinks, `HYPERLINK` fields and bookmarks in the translation into the layout IR
//! (#570).

use officewright_layout::ir;

use super::layout::{contents, describe, paragraphs, translate};
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
    // The instruction is read up to 32 KiB, the longest target a PDF holds.
    let field = |length: usize| {
        let instruction = format!(
            r#" HYPERLINK "https://example.com/{}" "#,
            "a".repeat(length)
        );
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{instruction}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>long</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    let document = open(&format!(
        "<w:p>{}{}{}</w:p>",
        field(32 << 10),
        field(32 << 10),
        field(30_000)
    ));
    let translation = translate(&document);
    let fields = &contents(&translation)[0];
    assert!(!fields[0].contains("LinkStart"), "{:.200}", fields[0]);
    assert!(!fields[1].contains("LinkStart"), "{:.200}", fields[1]);
    // One of 30,000 bytes is read whole.
    assert!(fields[2].contains("LinkStart"), "{:.200}", fields[2]);
    assert_eq!(
        messages(&translation),
        [
            "a HYPERLINK field whose instruction is longer than 32 KiB is shown without its link, here and wherever else the document has one"
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
    let translation = translate(&same);
    assert_eq!(linked(&translation), 40);
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
    let distinct = with_links(
        &format!("<w:p>{}</w:p>", links(&|n| format!("A{n}"))),
        &[&target],
    );
    let translation = translate(&distinct);
    assert_eq!(linked(&translation), 33);
    assert_eq!(
        messages(&translation),
        [
            "links past 32 MiB of web addresses and other targets are left out, here and wherever else the document has one"
        ]
    );
}
