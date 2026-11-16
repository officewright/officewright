//! Bookmarks wherever they stand, and the attributes of hyperlinks.

use super::*;

/// What the blocks of the first section hold, bookmarks among them, depth first: blocks,
/// the content of tables and rows, and the inline content of paragraphs.
fn walk(document: &Document) -> Vec<String> {
    fn start(start: BookmarkStart<'_>) -> String {
        format!(
            "start {:?} {:?} {:?} {:?}",
            start.id(),
            start.name(),
            start.first_column(),
            start.last_column()
        )
    }
    fn end(end: BookmarkEnd<'_>) -> String {
        format!("end {:?}", end.id())
    }
    fn blocks<'a>(of: impl Iterator<Item = Block<'a>>, out: &mut Vec<String>) {
        for block in of {
            match block {
                Block::Paragraph(paragraph) => {
                    out.push("p".into());
                    for inline in paragraph.inlines() {
                        match inline {
                            Inline::BookmarkStart(b) => out.push(format!("  {}", start(b))),
                            Inline::BookmarkEnd(b) => out.push(format!("  {}", end(b))),
                            Inline::Run(run) => out.push(format!("  run {:?}", run.text())),
                            Inline::ContentControl(_) => out.push("  sdt".into()),
                            Inline::Hyperlink(_) => out.push("  link".into()),
                            _ => out.push("  other inline".into()),
                        }
                    }
                }
                Block::Table(table) => {
                    out.push("table".into());
                    for content in table.content() {
                        match content {
                            TableContent::Row(row) => {
                                out.push("row".into());
                                for content in row.content() {
                                    match content {
                                        RowContent::Cell(cell) => {
                                            out.push("cell".into());
                                            blocks(cell.blocks(), out);
                                        }
                                        RowContent::BookmarkStart(b) => out.push(start(b)),
                                        RowContent::BookmarkEnd(b) => out.push(end(b)),
                                        RowContent::Other(o) => {
                                            out.push(format!("other {}", o.local_name()))
                                        }
                                    }
                                }
                            }
                            TableContent::BookmarkStart(b) => out.push(start(b)),
                            TableContent::BookmarkEnd(b) => out.push(end(b)),
                            TableContent::Other(o) => out.push(format!("other {}", o.local_name())),
                        }
                    }
                }
                Block::ContentControl(control) => blocks(control.blocks(), out),
                Block::BookmarkStart(b) => out.push(start(b)),
                Block::BookmarkEnd(b) => out.push(end(b)),
                Block::Other(o) => out.push(format!("other {}", o.local_name())),
            }
        }
    }
    let mut out = Vec::new();
    blocks(document.sections().next().unwrap().blocks(), &mut out);
    out
}

#[test]
fn reads_bookmarks_in_paragraphs_among_blocks_between_rows_and_between_cells() {
    let document = open(
        r#"<w:bookmarkStart w:id="1" w:name="Before"/><w:p><w:bookmarkStart w:id="2" w:name="_Toc1"/><w:r><w:t>a</w:t></w:r><w:bookmarkEnd w:id="2"/></w:p><w:bookmarkEnd w:id="1"/><w:tbl><w:bookmarkStart w:id="3" w:name="Cols" w:colFirst="0" w:colLast="1"/><w:tr><w:bookmarkStart w:id="4" w:name="Cell"/><w:tc><w:p/></w:tc><w:bookmarkEnd w:id="4"/></w:tr><w:bookmarkEnd w:id="3"/></w:tbl><w:sdt><w:sdtContent><w:bookmarkStart w:id="5" w:name="InControl"/><w:p/></w:sdtContent></w:sdt><w:p><w:sdt><w:sdtContent><w:bookmarkStart w:id="6" w:name="Inline"/></w:sdtContent></w:sdt><w:hyperlink w:anchor="x"><w:bookmarkEnd w:id="6"/></w:hyperlink></w:p>"#,
    );
    assert_eq!(
        walk(&document),
        [
            "start Some(1) Some(\"Before\") None None",
            "p",
            "  start Some(2) Some(\"_Toc1\") None None",
            "  run \"a\"",
            "  end Some(2)",
            "end Some(1)",
            "table",
            "start Some(3) Some(\"Cols\") Some(0) Some(1)",
            "row",
            "start Some(4) Some(\"Cell\") None None",
            "cell",
            "p",
            "end Some(4)",
            "end Some(3)",
            "start Some(5) Some(\"InControl\") None None",
            "p",
            "p",
            "  sdt",
            "  link",
        ]
    );
    // Inside the inline control and the hyperlink, where they are read too.
    let Some(Inline::ContentControl(control)) = Some(inline(&document, 5, 0)) else {
        panic!("a control");
    };
    assert!(matches!(
        control.inlines().next(),
        Some(Inline::BookmarkStart(b)) if b.name() == Some("Inline")
    ));
    let Some(Inline::Hyperlink(link)) = Some(inline(&document, 5, 1)) else {
        panic!("a hyperlink");
    };
    assert!(matches!(
        link.inlines().next(),
        Some(Inline::BookmarkEnd(b)) if b.id() == Some(6)
    ));
    // Bookmarks show nothing: the text is as without them. The table's rows and cells are
    // what they were.
    assert_eq!(document.text(), "a\n\n\n\n");
    let Some(Block::Table(table)) = document.sections().next().unwrap().blocks().nth(3) else {
        panic!("the table");
    };
    assert_eq!(table.rows().count(), 1);
    assert_eq!(table.rows().next().unwrap().cells().count(), 1);
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

/// Inline `n` of block `block` of the first section, a paragraph.
fn inline(document: &Document, block: usize, n: usize) -> Inline<'_> {
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().nth(block)
    else {
        panic!("no paragraph at {block}");
    };
    paragraph.inlines().nth(n).unwrap()
}

#[test]
fn writes_a_bookmark_back_as_read() {
    let document = open(
        r#"<w:p><w:bookmarkStart w:id="7" w:name="Here" w:colFirst="2" w:displacedByCustomXml="next"/><w:bookmarkEnd w:id="7"/></w:p>"#,
    );
    let (Inline::BookmarkStart(start), Inline::BookmarkEnd(end)) =
        (inline(&document, 0, 0), inline(&document, 0, 1))
    else {
        panic!("a bookmark");
    };
    let written = start.to_xml();
    assert!(written.starts_with("<w:bookmarkStart "), "{written}");
    for attribute in [
        r#"w:id="7""#,
        r#"w:name="Here""#,
        r#"w:colFirst="2""#,
        r#"w:displacedByCustomXml="next""#,
    ] {
        assert!(written.contains(attribute), "{written}");
    }
    assert!(end.to_xml().contains(r#"w:id="7""#));
    // Read again, the written start is the same bookmark.
    let again = open(&format!("<w:p>{written}</w:p>"));
    let Inline::BookmarkStart(again) = inline(&again, 0, 0) else {
        panic!("a bookmark");
    };
    assert_eq!(
        (
            again.id(),
            again.name(),
            again.first_column(),
            again.last_column()
        ),
        (Some(7), Some("Here"), Some(2), None)
    );
}

#[test]
fn reports_bookmarks_without_an_id_or_a_name_and_values_that_are_no_integers() {
    let document = open(
        r#"<w:p><w:bookmarkStart w:name="NoId"/><w:bookmarkStart w:id=" 3"/><w:bookmarkStart w:id="4" w:name="Cols" w:colFirst="x" w:colLast="1.5"/><w:bookmarkEnd/><w:bookmarkEnd w:id="nine"/></w:p>"#,
    );
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect();
    let expected = [
        "w:bookmarkStart has no w:id",
        r#"w:bookmarkStart w:id=" 3" is not valid; it is ignored"#,
        "w:bookmarkStart has no w:name",
        r#"w:bookmarkStart w:colFirst="x" is not valid; it is ignored"#,
        r#"w:bookmarkStart w:colLast="1.5" is not valid; it is ignored"#,
        "w:bookmarkEnd has no w:id",
        r#"w:bookmarkEnd w:id="nine" is not valid; it is ignored"#,
    ];
    assert_eq!(messages.len(), expected.len(), "{messages:?}");
    for (message, expected) in messages.iter().zip(expected) {
        assert!(message.ends_with(expected), "{message}");
    }
    // The views give no value for what is not one, and the rest as written.
    type Start<'a> = (Option<i64>, Option<&'a str>, Option<i64>, Option<i64>);
    let starts: Vec<Start<'_>> = (0..3)
        .map(|n| match inline(&document, 0, n) {
            Inline::BookmarkStart(b) => (b.id(), b.name(), b.first_column(), b.last_column()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        starts,
        [
            (None, Some("NoId"), None, None),
            (None, None, None, None),
            (Some(4), Some("Cols"), None, None)
        ]
    );
    assert!(matches!(inline(&document, 0, 4), Inline::BookmarkEnd(b) if b.id().is_none()));
}

#[test]
fn reads_every_attribute_of_a_hyperlink() {
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/a" TargetMode="External"/></Relationships>"#;
    let body = r#"<w:p><w:hyperlink r:id="rId1" w:anchor="Part" w:docLocation="sheet 2" w:tgtFrame="_blank" w:tooltip="Open it" w:history="1"><w:r><w:t>a</w:t></w:r></w:hyperlink><w:hyperlink w:anchor="Top" w:history="off"><w:r><w:t>b</w:t></w:r></w:hyperlink><w:hyperlink w:history="maybe"><w:r><w:t>c</w:t></w:r></w:hyperlink></w:p>"#;
    let document = Document::from_bytes(package(
        DOCUMENT,
        &document_xml(body),
        &[("word/_rels/document.xml.rels", rels)],
    ))
    .unwrap();
    type Link<'a> = (
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        Option<bool>,
    );
    let links: Vec<Link<'_>> = (0..3)
        .map(|n| match inline(&document, 0, n) {
            Inline::Hyperlink(l) => (
                l.target(),
                l.anchor(),
                l.doc_location(),
                l.target_frame(),
                l.tooltip(),
                l.history(),
            ),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        links,
        [
            (
                Some("https://example.com/a"),
                Some("Part"),
                Some("sheet 2"),
                Some("_blank"),
                Some("Open it"),
                Some(true)
            ),
            (None, Some("Top"), None, None, None, Some(false)),
            (None, None, None, None, None, None),
        ]
    );
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].ends_with(r#"w:hyperlink w:history="maybe" is not valid; it is ignored"#));
}
