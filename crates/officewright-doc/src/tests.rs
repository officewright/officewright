use officewright_core::Pt;

use crate::*;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const DOCUMENT: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";

/// A package with a main part of `content_type` holding `body`, and `extra` parts.
fn package(content_type: &str, document_xml: &str, extra: &[(&str, &str)]) -> Vec<u8> {
    let mut entries = vec![
        (
            "[Content_Types].xml".to_owned(),
            format!(
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="{content_type}"/></Types>"#
            ),
        ),
        (
            "_rels/.rels".to_owned(),
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_owned(),
        ),
        ("word/document.xml".to_owned(), document_xml.to_owned()),
    ];
    for (name, content) in extra {
        entries.push(((*name).to_owned(), (*content).to_owned()));
    }
    zip(&entries)
}

/// A document whose body is `body`, with the w, r and mc prefixes declared.
fn docx(body: &str) -> Vec<u8> {
    package(DOCUMENT, &document_xml(body), &[])
}

fn document_xml(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" mc:Ignorable="w14"><w:body>{body}</w:body></w:document>"#
    )
}

fn open(body: &str) -> Document {
    Document::from_bytes(docx(body)).unwrap()
}

/// A ZIP archive with the entries stored.
fn zip(entries: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let (name, data) = (name.as_bytes(), data.as_bytes());
        let offset = out.len() as u32;
        let crc = crc32(data);
        out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        for header in [&mut out, &mut central] {
            header.extend_from_slice(&crc.to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&[0, 0]);
        }
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
        out.extend_from_slice(name);
        out.extend_from_slice(data);
    }
    let directory = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&directory.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// One line per block, with the kinds of what is inside.
fn outline(document: &Document) -> Vec<String> {
    let mut out = Vec::new();
    for (n, section) in document.sections().enumerate() {
        out.push(format!("section {n}"));
        for block in section.blocks() {
            out.push(match block {
                Block::Paragraph(p) => format!("p {:?}", p.text()),
                Block::Table(t) => format!("table {} rows", t.rows().count()),
                Block::ContentControl(c) => format!("sdt {} blocks", c.blocks().count()),
                Block::Other(o) => format!("other {}", o.local_name()),
            });
        }
    }
    out
}

#[test]
fn reads_paragraphs_runs_and_their_content() {
    let document = open(
        r#"<w:p><w:r><w:t xml:space="preserve">Hello, </w:t></w:r><w:r><w:t>world</w:t><w:tab/><w:t>x</w:t><w:br/><w:t>y</w:t><w:br w:type="page"/><w:cr/><w:noBreakHyphen/><w:softHyphen/><w:sym w:font="Wingdings" w:char="F0FC"/><w:lastRenderedPageBreak/></w:r></w:p><w:p/>"#,
    );
    assert_eq!(document.text(), "Hello, world\tx\ny\n\n\u{2011}\n\n");
    let paragraph = match document.sections().next().unwrap().blocks().next().unwrap() {
        Block::Paragraph(p) => p,
        other => panic!("{other:?}"),
    };
    let runs: Vec<Run<'_>> = paragraph.runs().collect();
    assert_eq!(runs.len(), 2);
    let content: Vec<String> = runs[1]
        .content()
        .map(|item| match item {
            RunContent::Text(text) => format!("text {text}"),
            RunContent::Break(b) => format!("break {:?} {:?}", b.kind(), b.clear()),
            RunContent::Symbol { font, char, .. } => format!("symbol {font:?} {:X}", char as u32),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        content,
        [
            "text world",
            "Tab",
            "text x",
            "break Line None",
            "text y",
            "break Page None",
            "CarriageReturn",
            "NoBreakHyphen",
            "SoftHyphen",
            "symbol Some(\"Wingdings\") F0FC",
            "LastRenderedPageBreak",
        ]
    );
}

#[test]
fn splits_the_body_into_sections() {
    let document = open(
        r#"<w:p><w:r><w:t>one</w:t></w:r></w:p><w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:type w:val="continuous"/></w:sectPr></w:pPr><w:r><w:t>end of one</w:t></w:r></w:p><w:p><w:r><w:t>two</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/><w:pgMar w:top="-1440" w:right="1440" w:bottom="1440" w:left="2.54cm" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>"#,
    );
    assert_eq!(
        outline(&document),
        [
            "section 0",
            "p \"one\"",
            "p \"end of one\"",
            "section 1",
            "p \"two\""
        ]
    );
    let sections: Vec<Section<'_>> = document.sections().collect();
    let first = sections[0].properties();
    assert_eq!(first.page_size(), Some(officewright_core::PageSize::LETTER));
    assert_eq!(first.start(), Some(SectionStart::Continuous));
    assert_eq!(first.orientation(), None);
    let last = sections[1].properties();
    assert_eq!(last.page_width(), Some(Pt(841.9)));
    assert_eq!(last.orientation(), Some(Orientation::Landscape));
    assert_eq!(last.start(), None);
    let margins = last.margins();
    assert_eq!(margins.top(), Some(Pt(-72.0)));
    assert!((margins.left().unwrap().0 - 72.0).abs() < 1e-9);
    assert_eq!(margins.gutter(), Some(Pt(0.0)));
}

#[test]
fn a_document_without_a_body_has_one_empty_section() {
    let document = Document::from_bytes(package(
        DOCUMENT,
        &format!(r#"<w:document xmlns:w="{W}"/>"#),
        &[],
    ))
    .unwrap();
    assert_eq!(outline(&document), ["section 0"]);
    assert_eq!(
        document.sections().next().unwrap().properties(),
        &SectionProperties::default()
    );
}

#[test]
fn a_section_break_in_a_table_cell_ends_no_section() {
    // Word uses w:sectPr only in paragraphs of the body.
    let document = open(
        r#"<w:tbl><w:tr><w:tc><w:p><w:pPr><w:sectPr/></w:pPr></w:p></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    assert_eq!(document.sections().len(), 1);
}

#[test]
fn reads_tables_through_content_controls() {
    let document = open(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="bad"/></w:tblGrid><w:tr><w:tc><w:tcPr/><w:p><w:r><w:t>a</w:t></w:r></w:p><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc><w:sdt><w:sdtPr/><w:sdtContent><w:tc><w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/></w:tc></w:sdtContent></w:sdt></w:tr><w:customXml w:element="row"><w:tr><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:customXml></w:tbl>"#,
    );
    let Some(Block::Table(table)) = document.sections().next().unwrap().blocks().next() else {
        panic!("no table");
    };
    assert_eq!(
        table.grid_columns().collect::<Vec<_>>(),
        [Some(Pt(100.0)), None]
    );
    let rows: Vec<TableRow<'_>> = table.rows().collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].cells().count(), 2);
    assert_eq!(document.text(), "a\nb\tinner\n\nc\n");
    assert_eq!(
        document.diagnostics().len(),
        1,
        "{:?}",
        document.diagnostics()
    );
    assert_eq!(
        document.diagnostics()[0].kind(),
        DiagnosticKind::InvalidValue
    );
}

#[test]
fn reads_inline_containers() {
    let document = Document::from_bytes(package(
        DOCUMENT,
        &document_xml(
            r#"<w:p><w:hyperlink r:id="rId7" w:anchor="top"><w:r><w:t>link</w:t></w:r></w:hyperlink><w:ins w:author="A" w:date="2024-01-02T03:04:05Z"><w:r><w:t> new</w:t></w:r></w:ins><w:del w:author="B"><w:r><w:delText> old</w:delText></w:r></w:del><w:moveFrom><w:r><w:t> moved</w:t></w:r></w:moveFrom><w:fldSimple w:instr=" PAGE "><w:r><w:t>3</w:t></w:r></w:fldSimple><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> DATE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>today</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:sdt><w:sdtContent><w:r><w:t>!</w:t></w:r></w:sdtContent></w:sdt><w:smartTag w:element="x"><w:r><w:t>?</w:t></w:r></w:smartTag></w:p>"#,
        ),
        &[(
            "word/_rels/document.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/></Relationships>"#,
        )],
    ))
    .unwrap();
    assert_eq!(document.text(), "link new3today!?\n");
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().next()
    else {
        panic!("no paragraph");
    };
    let kinds: Vec<String> = paragraph
        .inlines()
        .map(|inline| match inline {
            Inline::Run(r) => format!("run {:?}", r.text()),
            Inline::Hyperlink(h) => format!("link {:?} {:?}", h.target(), h.anchor()),
            Inline::Revision(r) => format!("{:?} {:?} {:?}", r.kind(), r.author(), r.date()),
            Inline::SimpleField(f) => format!("field {:?}", f.instruction()),
            Inline::ContentControl(c) => format!("sdt {}", c.inlines().count()),
            Inline::Other(o) => format!("other {}", o.local_name()),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "link Some(\"https://example.com/\") Some(\"top\")",
            "Insertion Some(\"A\") Some(\"2024-01-02T03:04:05Z\")",
            "Deletion Some(\"B\") None",
            "MoveFrom None None",
            "field \" PAGE \"",
            "run \"\"",
            "run \"\"",
            "run \"\"",
            "run \"today\"",
            "run \"\"",
            "sdt 1",
            // The smart tag is seen through.
            "run \"?\"",
        ]
    );
    assert_eq!(paragraph.runs().count(), 12);
}

#[test]
fn follows_markup_compatibility() {
    let document = open(
        r#"<w:p w14:paraId="1A2B"><w:r><mc:AlternateContent><mc:Choice Requires="wp"><w:t>choice</w:t></mc:Choice><mc:Fallback><w:t>fallback</w:t></mc:Fallback></mc:AlternateContent></w:r><mc:AlternateContent><mc:Choice Requires="w14"><w:r><w:t>w14</w:t></w:r></mc:Choice><mc:Fallback><w:r><w:t>plain</w:t></w:r></mc:Fallback></mc:AlternateContent><w14:extension><w:r><w:t>hidden</w:t></w:r></w14:extension></w:p>"#,
    );
    // wp is not understood here (drawings are kept as they are), w14 is ignorable: the
    // fallbacks are read, and the w14 element is kept in its place.
    assert_eq!(document.text(), "fallbackplain\n");
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().next()
    else {
        panic!("no paragraph");
    };
    let other = paragraph
        .inlines()
        .find_map(|inline| match inline {
            Inline::Other(o) => Some(o),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        other.namespace(),
        "http://schemas.microsoft.com/office/word/2010/wordml"
    );
    assert_eq!(
        other.to_xml(),
        format!(
            r#"<w14:extension xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"><w:r xmlns:w="{W}"><w:t>hidden</w:t></w:r></w14:extension>"#
        )
    );
}

#[test]
fn refuses_content_that_must_be_understood() {
    // On an element that is read; on one kept without being understood it is not
    // checked, as ECMA-376 Part 3 asks only of what a consumer processes.
    let xml = document_xml(r#"<w:p xmlns:x="urn:x" mc:MustUnderstand="x"/>"#);
    let err = Document::from_bytes(package(DOCUMENT, &xml, &[])).unwrap_err();
    assert!(matches!(err, Error::Unsupported(_)), "{err}");
    let kept = document_xml(r#"<w:p><w14:x xmlns:x="urn:x" mc:MustUnderstand="x"/></w:p>"#);
    assert!(Document::from_bytes(package(DOCUMENT, &kept, &[])).is_ok());
}

#[test]
fn reads_strict_documents() {
    let xml = r#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main" conformance="strict"><w:body><w:p><w:r><w:t>strict</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="8.5in" w:h="11in"/></w:sectPr></w:body></w:document>"#;
    let document = Document::from_bytes(package(DOCUMENT, xml, &[])).unwrap();
    assert_eq!(document.conformance(), Conformance::Strict);
    assert_eq!(document.text(), "strict\n");
    assert_eq!(
        document.sections().next().unwrap().properties().page_size(),
        Some(officewright_core::PageSize::LETTER)
    );
}

#[test]
fn tells_the_kinds_of_word_packages_apart() {
    for (content_type, kind) in [
        (DOCUMENT, DocumentKind::Document),
        (
            "application/vnd.ms-word.document.macroEnabled.main+xml",
            DocumentKind::MacroEnabledDocument,
        ),
        (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
            DocumentKind::Template,
        ),
        (
            "application/vnd.ms-word.template.macroEnabledTemplate.main+xml",
            DocumentKind::MacroEnabledTemplate,
        ),
    ] {
        let document = Document::from_bytes(package(content_type, &document_xml(""), &[])).unwrap();
        assert_eq!(document.kind(), kind);
    }
    let sheet = package(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
        "<workbook/>",
        &[],
    );
    assert!(matches!(
        Document::from_bytes(sheet),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn refuses_what_is_not_a_word_document() {
    let message = |result: Result<Document>| match result {
        Err(Error::Malformed { message }) => message,
        other => panic!("expected a malformed error, got {other:?}"),
    };
    assert_eq!(
        message(Document::from_bytes(b"not a zip".to_vec())),
        "zip archive: the input is too short to be a zip archive at byte 0"
    );
    let no_main = zip(&[(
        "[Content_Types].xml".into(),
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#.into(),
    )]);
    assert_eq!(
        message(Document::from_bytes(no_main)),
        "the package has no main document part (no officeDocument relationship)"
    );
    let wrong_root = package(DOCUMENT, r#"<x:document xmlns:x="urn:x"/>"#, &[]);
    assert_eq!(
        message(Document::from_bytes(wrong_root)),
        "/word/document.xml, line 1, column 1: the root element is <x:document>, not w:document"
    );
    let broken = package(DOCUMENT, &document_xml("<w:p>"), &[]);
    assert!(message(Document::from_bytes(broken)).starts_with("/word/document.xml, line 1"));
    let ole = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0];
    assert!(matches!(
        Document::from_bytes(ole.to_vec()),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn an_id_finds_its_node_only_in_its_own_document() {
    let first = open("<w:p/><w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>");
    let second = open("<w:p/><w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>");
    let mut blocks = first.sections().next().unwrap().blocks();
    let (Some(Block::Paragraph(paragraph)), Some(Block::Table(table))) =
        (blocks.next(), blocks.next())
    else {
        panic!("unexpected blocks");
    };
    assert!(first.paragraph(paragraph.id()).is_some());
    assert!(first.table(table.id()).is_some());
    assert!(second.paragraph(paragraph.id()).is_none());
    assert!(second.table(table.id()).is_none());
}

#[test]
fn keeps_unknown_content_and_reports_stray_text() {
    let document = open(
        r#"<w:bookmarkStart w:id="0" w:name="a"/><w:p>stray<w:r><w:t>x</w:t><w:drawing/></w:r></w:p>"#,
    );
    assert_eq!(
        outline(&document),
        ["section 0", "other bookmarkStart", "p \"x\""]
    );
    assert_eq!(document.diagnostics().len(), 1);
    assert_eq!(
        document.diagnostics()[0].kind(),
        DiagnosticKind::UnexpectedContent
    );
}

#[test]
fn keeps_at_most_a_thousand_diagnostics() {
    let body = r#"<w:p><w:r><w:br w:type="bad"/></w:r></w:p>"#.repeat(1500);
    let document = open(&body);
    let diagnostics = document.diagnostics();
    assert_eq!(diagnostics.len(), 1001);
    assert_eq!(diagnostics[1000].kind(), DiagnosticKind::Omitted);
    assert_eq!(
        diagnostics[1000].message(),
        "500 more diagnostics are not kept"
    );
    let first = diagnostics[0].message();
    assert!(
        first.starts_with("/word/document.xml, line 1, column "),
        "{first}"
    );
    assert!(
        first.ends_with(r#": w:br w:type="bad" is not valid; it is ignored"#),
        "{first}"
    );
}

#[test]
fn bounds_the_number_of_nodes() {
    let body = "<w:p/>".repeat(100);
    let options = LoadOptions::new().with_limits(Limits::new().with_max_nodes(99));
    let err = options.open_bytes(docx(&body)).unwrap_err();
    assert!(
        matches!(
            err,
            Error::LimitExceeded {
                limit: Limit::Nodes,
                maximum: 99,
                ..
            }
        ),
        "{err}"
    );
    let options = LoadOptions::new().with_limits(Limits::new().with_max_nodes(100));
    assert!(options.open_bytes(docx(&body)).is_ok());
}

#[test]
fn bounds_the_size_of_the_file() {
    let data = docx("<w:p/>");
    let limits = Limits::new().with_max_file_size(data.len() as u64 - 1);
    let options = LoadOptions::new().with_limits(limits);
    for result in [
        options.open_bytes(data.clone()),
        options.open_reader(&data[..]),
    ] {
        assert!(
            matches!(
                result,
                Err(Error::LimitExceeded {
                    limit: Limit::FileSize,
                    ..
                })
            ),
            "{result:?}"
        );
    }
    let limits = Limits::new().with_max_file_size(data.len() as u64);
    assert!(
        LoadOptions::new()
            .with_limits(limits)
            .open_reader(&data[..])
            .is_ok()
    );
}

#[test]
fn walks_deeply_nested_tables_without_recursion() {
    // 3,000 tables in tables: 9,000 elements deep, far beyond what the call stack of a
    // recursive walk would allow in a test thread.
    let depth = 3000;
    let body = format!(
        "{}<w:p><w:r><w:t>deep</w:t></w:r></w:p>{}",
        "<w:tbl><w:tr><w:tc>".repeat(depth),
        "</w:tc></w:tr></w:tbl>".repeat(depth)
    );
    let options = LoadOptions::new().with_limits(Limits::new().with_max_xml_depth(20_000));
    let document = options.open_bytes(docx(&body)).unwrap();
    assert_eq!(document.text(), "deep\n");
    drop(document);
}

#[test]
fn opens_files_and_converts_errors_into_core_errors() {
    let document = Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/hello.docx"
    ))
    .unwrap();
    assert_eq!(document.text(), "Hello, world\n");
    let missing = Document::open("no such file.docx").unwrap_err();
    assert!(matches!(missing, Error::Io { .. }));
    assert_eq!(missing.to_string(), "cannot read no such file.docx");
    assert!(matches!(
        officewright_core::Error::from(missing),
        officewright_core::Error::Io { .. }
    ));
    let malformed = Document::from_bytes(Vec::new()).unwrap_err();
    assert!(matches!(
        officewright_core::Error::from(malformed),
        officewright_core::Error::InvalidArgument(_)
    ));
}

#[test]
fn keeps_deeply_nested_elements_without_recursion() {
    // 20,000 nested elements, far beyond what the call stack of a recursive reader,
    // writer or drop would allow in a test thread.
    let depth = 20_000;
    let body = format!(
        "<w:p>{}{}<w:r><w:t>after</w:t></w:r></w:p>",
        "<w14:a>".repeat(depth),
        "</w14:a>".repeat(depth)
    );
    let options = LoadOptions::new().with_limits(Limits::new().with_max_xml_depth(30_000));
    let document = options.open_bytes(docx(&body)).unwrap();
    assert_eq!(document.text(), "after\n");
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().next()
    else {
        panic!("no paragraph");
    };
    let Some(Inline::Other(other)) = paragraph.inlines().next() else {
        panic!("not kept");
    };
    let xml = other.to_xml();
    assert!(xml.starts_with(
        r#"<w14:a xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"><w14:a><w14:a>"#
    ));
    let declaration = r#" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml""#;
    assert_eq!(
        xml.len(),
        "<w14:a></w14:a>".len() * depth + declaration.len()
    );
    drop(document);
}

#[test]
fn reads_complex_fields_deleted_text_and_references() {
    let document = open(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Check1"/><w:checkBox><w:default w:val="1"/></w:checkBox></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMCHECKBOX </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>result</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:delText>deleted</w:delText><w:delInstrText>DATE</w:delInstrText><w:br w:type="textWrapping" w:clear="all"/><w:footnoteReference w:id="2"/><w:endnoteReference w:id="-1"/><w:commentReference w:id="0"/><w:fldChar w:fldCharType="bad"/><w:footnoteReference w:id="x"/></w:r></w:p>"#,
    );
    // Field codes and deleted text are not text; the field's result and the break are.
    assert_eq!(document.text(), "result\n\n");
    let items: Vec<String> = document
        .sections()
        .flat_map(|s| s.blocks())
        .filter_map(|block| match block {
            Block::Paragraph(p) => Some(p),
            _ => None,
        })
        .flat_map(|p| p.runs())
        .flat_map(|r| r.content())
        .map(|item| match item {
            RunContent::Other(o) => format!("Other({})", o.local_name()),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        items,
        [
            "FieldChar(Begin)",
            "FieldCode(\" FORMCHECKBOX \")",
            "FieldChar(Separate)",
            "Text(\"result\")",
            "FieldChar(End)",
            "DeletedText(\"deleted\")",
            "DeletedFieldCode(\"DATE\")",
            "Break(Break { kind: Line, clear: All })",
            "FootnoteReference(2)",
            "EndnoteReference(-1)",
            "CommentReference(0)",
            "Other(fldChar)",
            "Other(footnoteReference)",
        ]
    );
    // The form field data in the first w:fldChar is kept, not dropped.
    let names: Vec<&str> = document
        .tokens
        .iter()
        .filter_map(|token| match token {
            model::xml::Token::Start { name, .. } => Some(document.strings.get(name.qname)),
            _ => None,
        })
        .collect();
    assert!(
        names.starts_with(&["w:fldChar", "w:ffData", "w:name", "w:checkBox", "w:default"]),
        "{names:?}"
    );
    assert_eq!(
        document.diagnostics().len(),
        2,
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn finds_the_positions_of_diagnostics_in_one_pass() {
    // 8 MiB of text, then 1,000 values to report: finding each position from the start
    // of the part would read 8 GiB.
    let body = format!(
        "<w:p><w:r><w:t>{}</w:t></w:r></w:p><w:p><w:r>{}</w:r></w:p>",
        "x".repeat(8 << 20),
        r#"<w:br w:type="bad"/>"#.repeat(1000)
    );
    let start = std::time::Instant::now();
    let document = open(&body);
    let elapsed = start.elapsed();
    assert_eq!(document.diagnostics().len(), 1000);
    assert!(
        document.diagnostics()[999]
            .message()
            .starts_with("/word/document.xml, line 1, column "),
        "{}",
        document.diagnostics()[999]
    );
    assert!(elapsed < std::time::Duration::from_secs(20), "{elapsed:?}");
}

#[test]
fn places_section_diagnostics_and_the_node_limit() {
    let document = open("<w:p/>\n<w:sectPr><w:pgSz w:w=\"wide\"/></w:sectPr>");
    assert_eq!(
        document.diagnostics()[0].message(),
        "/word/document.xml, line 2, column 1: w:pgSz w:w=\"wide\" is not valid; it is ignored"
    );
    let options = LoadOptions::new().with_limits(Limits::new().with_max_nodes(3));
    match options.open_bytes(docx("<w:p/><w:p/><w:p/><w:p/>")) {
        Err(Error::LimitExceeded { context, .. }) => {
            assert!(
                context.starts_with("/word/document.xml, line 1, column "),
                "{context}"
            );
        }
        other => panic!("expected the node limit, got {other:?}"),
    }
    assert_eq!(
        Limits::new().with_max_nodes(usize::MAX).max_nodes(),
        u32::MAX as usize
    );
}

#[test]
fn keeps_comments_and_processing_instructions_in_kept_elements() {
    let document = open("<w:p><w14:x><!-- note --><?pi data?></w14:x></w:p>");
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().next()
    else {
        panic!("no paragraph");
    };
    let Some(Inline::Other(other)) = paragraph.inlines().next() else {
        panic!("not kept");
    };
    assert_eq!(
        other.to_xml(),
        r#"<w14:x xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"><!-- note --><?pi data?></w14:x>"#
    );
}

#[test]
fn ends_a_section_after_its_paragraph_inside_content_controls() {
    // As in LibreOffice's tdf170602_exportContext.docx: a cover page in a content
    // control whose last paragraph but one ends the first section.
    let document = open(
        r#"<w:sdt><w:sdtPr><w:docPartObj/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>cover</w:t></w:r></w:p><w:p><w:pPr><w:sectPr><w:type w:val="nextPage"/></w:sectPr></w:pPr><w:r><w:t>end of cover</w:t></w:r></w:p><w:p><w:r><w:t>Contents</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>body</w:t></w:r></w:p>"#,
    );
    assert_eq!(
        outline(&document),
        [
            "section 0",
            "sdt 2 blocks",
            "section 1",
            "sdt 1 blocks",
            "p \"body\""
        ]
    );
    assert_eq!(document.text(), "cover\nend of cover\nContents\nbody\n");
    // When the paragraph ends the control, nothing is left over for the next section;
    // nested controls and custom XML are continued alike.
    let document = open(
        r#"<w:sdt><w:sdtContent><w:customXml w:element="x"><w:customXmlPr/><w:sdt><w:sdtContent><w:p><w:pPr><w:sectPr/></w:pPr></w:p><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:sdtContent></w:sdt></w:customXml><w:p><w:pPr><w:sectPr/></w:pPr></w:p></w:sdtContent></w:sdt><w:p/>"#,
    );
    assert_eq!(
        outline(&document),
        [
            "section 0",
            "sdt 1 blocks",
            "section 1",
            "sdt 2 blocks",
            "section 2",
            "p \"\""
        ]
    );
    let sections: Vec<Section<'_>> = document.sections().collect();
    let Some(Block::ContentControl(second)) = sections[1].blocks().next() else {
        panic!("no content control");
    };
    let Some(Block::ContentControl(inner)) = second.blocks().next() else {
        panic!("no inner control");
    };
    assert_eq!(inner.blocks().count(), 1);
}

#[test]
fn counts_tables_around_section_breaks() {
    let document = open(
        r#"<w:tbl><w:tr><w:tc><w:tbl><w:tr><w:tc><w:p><w:pPr><w:sectPr/></w:pPr></w:p></w:tc></w:tr></w:tbl><w:p><w:pPr><w:sectPr/></w:pPr></w:p></w:tc></w:tr></w:tbl><w:p><w:pPr><w:sectPr/></w:pPr></w:p><w:p/>"#,
    );
    assert_eq!(
        outline(&document),
        ["section 0", "table 1 rows", "p \"\"", "section 1", "p \"\""]
    );
}

#[test]
fn reads_the_properties_of_custom_xml_and_smart_tags_as_properties() {
    let document = open(
        r#"<w:customXml w:element="x"><w:customXmlPr><w:attr w:name="a" w:val="b"/></w:customXmlPr><w:p><w:smartTag w:element="y"><w:smartTagPr/><w:r><w:t>tag</w:t></w:r></w:smartTag></w:p></w:customXml>"#,
    );
    assert_eq!(outline(&document), ["section 0", "p \"tag\""]);
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().next()
    else {
        panic!("no paragraph");
    };
    assert_eq!(paragraph.inlines().count(), 1);
}

#[test]
fn ends_a_row_with_a_line_feed_when_a_cell_has_no_paragraph() {
    let document = open(
        r#"<w:p><w:r><w:t>a</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:bookmarkStart w:id="0" w:name="b"/><w:bookmarkEnd w:id="0"/></w:tc><w:tc><w:p><w:r><w:t>y</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
    );
    assert_eq!(document.text(), "a\nx\n\ty\n");
}

#[test]
fn gives_content_controls_only_the_content_of_their_level() {
    let document = open(
        r#"<w:sdt><w:sdtContent><w:bookmarkStart w:id="0" w:name="a"/><w:p/><w:bookmarkEnd w:id="0"/></w:sdtContent></w:sdt><w:p><w:sdt><w:sdtContent><w:bookmarkStart w:id="1" w:name="b"/><w:r><w:t>x</w:t></w:r><w:bookmarkEnd w:id="1"/></w:sdtContent></w:sdt></w:p>"#,
    );
    let mut blocks = document.sections().next().unwrap().blocks();
    let (Some(Block::ContentControl(block)), Some(Block::Paragraph(paragraph))) =
        (blocks.next(), blocks.next())
    else {
        panic!("unexpected blocks");
    };
    assert_eq!((block.blocks().count(), block.inlines().count()), (3, 0));
    let Some(Inline::ContentControl(inline)) = paragraph.inlines().next() else {
        panic!("no inline control");
    };
    assert_eq!((inline.blocks().count(), inline.inlines().count()), (0, 3));
}

#[test]
fn reads_hyperlink_targets_from_external_relationships_only() {
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/a" TargetMode="External"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
    let body = r#"<w:p><w:hyperlink r:id="rId1"><w:r><w:t>a</w:t></w:r></w:hyperlink><w:hyperlink r:id="rId1"><w:r><w:t>b</w:t></w:r></w:hyperlink><w:hyperlink r:id="rId2"><w:r><w:t>c</w:t></w:r></w:hyperlink><w:hyperlink r:id="rId9"><w:r><w:t>d</w:t></w:r></w:hyperlink><w:fldSimple><w:r><w:t>e</w:t></w:r></w:fldSimple></w:p>"#;
    let document = Document::from_bytes(package(
        DOCUMENT,
        &document_xml(body),
        &[("word/_rels/document.xml.rels", rels)],
    ))
    .unwrap();
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().next()
    else {
        panic!("no paragraph");
    };
    let targets: Vec<Option<&str>> = paragraph
        .inlines()
        .filter_map(|inline| match inline {
            Inline::Hyperlink(link) => Some(link.target()),
            _ => None,
        })
        .collect();
    assert_eq!(
        targets,
        [
            Some("https://example.com/a"),
            Some("https://example.com/a"),
            None,
            None
        ]
    );
    // Links naming one relationship share one copy of its target.
    assert!(std::ptr::eq(targets[0].unwrap(), targets[1].unwrap()));
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        messages[0]
            .ends_with(r#"hyperlink r:id="rId9" names no relationship; the link is ignored"#)
    );
    assert!(messages[1].ends_with("w:fldSimple has no w:instr; its instruction is empty"));
}

/// Opens `body` with a raised XML depth limit and fails if it takes more than 20 s, far
/// above the second or so it takes when the work is linear.
fn open_in_linear_time(body: &str) -> Document {
    let options = LoadOptions::new().with_limits(Limits::new().with_max_xml_depth(1 << 20));
    let start = std::time::Instant::now();
    let document = options.open_bytes(docx(body)).unwrap();
    let elapsed = start.elapsed();
    assert!(elapsed < std::time::Duration::from_secs(20), "{elapsed:?}");
    document
}

#[test]
fn closes_content_controls_after_section_breaks_in_linear_time() {
    // A reviewer of #32 found each content control that closed searched every copy made
    // for a section break: 500,000 breaks and 500,000 controls took 45 s.
    let n = 100_000;
    let body = format!(
        "<w:sdt><w:sdtContent>{}</w:sdtContent></w:sdt><w:p>{}</w:p>",
        "<w:p><w:pPr><w:sectPr/></w:pPr></w:p>".repeat(n),
        "<w:sdt><w:sdtContent><w:r><w:t>x</w:t></w:r></w:sdtContent></w:sdt>".repeat(n)
    );
    let document = open_in_linear_time(&body);
    assert_eq!(document.sections().len(), n + 1);
}

#[test]
fn breaks_sections_under_nested_content_controls_in_linear_time() {
    // ...and each break looked through every frame open around it and every copy made
    // for it so far: 100 breaks under 20,000 nested controls took 15 s.
    let depth = 10_000;
    let body = format!(
        "{}{}{}",
        "<w:sdt><w:sdtContent>".repeat(depth),
        "<w:p><w:pPr><w:sectPr/></w:pPr></w:p>".repeat(100),
        "</w:sdtContent></w:sdt>".repeat(depth)
    );
    let document = open_in_linear_time(&body);
    assert_eq!(document.sections().len(), 101);
}

#[test]
fn reads_under_nested_alternate_content_in_linear_time() {
    // Each element looked past every mc:AlternateContent open around it for where its
    // children go: 100,000 paragraphs under 100,000 of them took about a minute in a
    // debug build, and take about a second now.
    let n = 100_000;
    let body = format!(
        "{}{}{}",
        "<mc:AlternateContent><mc:Fallback>".repeat(n),
        "<w:p><w:pPr><w:sectPr/></w:pPr></w:p>".repeat(n),
        "</mc:Fallback></mc:AlternateContent>".repeat(n)
    );
    let document = open_in_linear_time(&body);
    assert_eq!(document.sections().len(), n + 1);
}

/// A document with a styles part, whose relationship the main part gives.
fn with_styles(body: &str, styles_xml: &str) -> Document {
    Document::from_bytes(package(
        DOCUMENT,
        &document_xml(body),
        &[
            (
                "word/_rels/document.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
            ),
            ("word/styles.xml", styles_xml),
        ],
    ))
    .unwrap()
}

fn first_run(document: &Document) -> Run<'_> {
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().next()
    else {
        panic!("no paragraph");
    };
    paragraph.runs().next().unwrap()
}

#[test]
fn reads_the_properties_written_on_a_run() {
    let document = open(
        r#"<w:p><w:r><w:rPr><w:rStyle w:val="Strong"/><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="MS Mincho" w:cs="Arial" w:asciiTheme="minorHAnsi" w:cstheme="minorBidi" w:hint="eastAsia"/><w:b/><w:bCs w:val="0"/><w:i w:val="true"/><w:iCs w:val="off"/><w:caps w:val="on"/><w:smallCaps w:val="false"/><w:strike/><w:dstrike w:val="1"/><w:vanish/><w:rtl w:val="0"/><w:color w:val="1F3864" w:themeColor="accent1" w:themeShade="BF"/><w:spacing w:val="-20"/><w:w w:val="150"/><w:kern w:val="28"/><w:position w:val="-6"/><w:sz w:val="22"/><w:szCs w:val="21"/><w:highlight w:val="yellow"/><w:u w:val="dotDash" w:color="FF0000"/><w:shd w:val="pct25" w:color="auto" w:fill="D9E2F3" w:themeFill="accent1" w:themeFillTint="33"/><w:vertAlign w:val="superscript"/><w:lang w:val="en-US" w:eastAsia="ja-JP" w:bidi="ar-SA"/><w14:ligatures w14:val="standard"/></w:rPr><w:t>x</w:t></w:r></w:p>"#,
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
    let run = first_run(&document);
    let properties = run.properties();
    assert_eq!(properties.style(), Some("Strong"));
    let fonts = properties.fonts();
    assert_eq!(
        (
            fonts.ascii(),
            fonts.high_ansi(),
            fonts.east_asia(),
            fonts.complex_script()
        ),
        (
            Some("Calibri"),
            Some("Calibri"),
            Some("MS Mincho"),
            Some("Arial")
        )
    );
    assert_eq!(fonts.ascii_theme(), Some(ThemeFont::MinorHAnsi));
    assert_eq!(fonts.complex_script_theme(), Some(ThemeFont::MinorBidi));
    assert_eq!(fonts.east_asia_theme(), None);
    assert_eq!(fonts.hint(), Some(FontHint::EastAsia));
    assert_eq!(properties.bold(), Some(true));
    assert_eq!(properties.bold_complex_script(), Some(false));
    assert_eq!(properties.italic(), Some(true));
    assert_eq!(properties.italic_complex_script(), Some(false));
    assert_eq!(properties.caps(), Some(true));
    assert_eq!(properties.small_caps(), Some(false));
    assert_eq!(properties.strike(), Some(true));
    assert_eq!(properties.double_strike(), Some(true));
    assert_eq!(properties.vanish(), Some(true));
    assert_eq!(properties.right_to_left(), Some(false));
    assert_eq!(properties.outline(), None);
    let color = properties.color().unwrap();
    assert_eq!(color.value(), Some(ColorValue::Rgb(0x1F, 0x38, 0x64)));
    assert_eq!(color.theme(), Some(ThemeColor::Accent1));
    assert_eq!((color.tint(), color.shade()), (None, Some(0xBF)));
    assert_eq!(properties.spacing(), Some(Pt(-1.0)));
    assert_eq!(properties.scale(), Some(150));
    assert_eq!(properties.kerning(), Some(Pt(14.0)));
    assert_eq!(properties.position(), Some(Pt(-3.0)));
    assert_eq!(properties.size(), Some(Pt(11.0)));
    assert_eq!(properties.size_complex_script(), Some(Pt(10.5)));
    assert_eq!(properties.highlight(), Some(Highlight::Yellow));
    let underline = properties.underline().unwrap();
    assert_eq!(underline.style(), Some(UnderlineStyle::DotDash));
    assert_eq!(underline.color().value(), Some(ColorValue::Rgb(0xFF, 0, 0)));
    let shading = properties.shading().unwrap();
    assert_eq!(shading.pattern(), Some(ShadingPattern::Percent25));
    assert_eq!(shading.color().value(), Some(ColorValue::Auto));
    assert_eq!(
        shading.fill().value(),
        Some(ColorValue::Rgb(0xD9, 0xE2, 0xF3))
    );
    assert_eq!(shading.fill().theme(), Some(ThemeColor::Accent1));
    assert_eq!(shading.fill().tint(), Some(0x33));
    assert_eq!(
        properties.vertical_position(),
        Some(VerticalPosition::Superscript)
    );
    let language = properties.language();
    assert_eq!(
        (language.latin(), language.east_asia(), language.bidi()),
        (Some("en-US"), Some("ja-JP"), Some("ar-SA"))
    );
    // A run without properties sets nothing.
    let plain = open("<w:p><w:r><w:t>x</w:t></w:r></w:p>");
    assert_eq!(first_run(&plain).properties(), RunProperties::default());
}

#[test]
fn reads_invalid_property_values_as_not_set() {
    let document = open(
        r#"<w:p><w:r><w:rPr><w:b w:val="yes"/><w:sz w:val="big"/><w:color w:val="red" w:themeTint="1"/><w:u w:val="squiggly"/><w:w w:val="601"/><w:rFonts w:asciiTheme="majorLatin"/><w:i/></w:rPr><w:t>x</w:t></w:r></w:p>"#,
    );
    let properties = first_run(&document).properties();
    assert_eq!(properties.bold(), None);
    assert_eq!(properties.size(), None);
    assert_eq!(properties.color(), None);
    assert_eq!(properties.underline().unwrap().style(), None);
    assert_eq!(properties.scale(), None);
    assert_eq!(properties.fonts().ascii_theme(), None);
    assert_eq!(properties.italic(), Some(true));
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect();
    assert_eq!(messages.len(), 7, "{messages:?}");
    assert!(
        messages
            .iter()
            .all(|m| m.starts_with("/word/document.xml, line 1, column "))
    );
    assert!(messages[0].ends_with(r#": w:b w:val="yes" is not valid; it is ignored"#));
    assert!(
        messages[6]
            .ends_with(r#": w:rFonts w:asciiTheme="majorLatin" is not valid; it is ignored"#)
    );
}

#[test]
fn reads_run_properties_through_the_fallback_of_alternate_content() {
    let document = open(
        r#"<w:p><w:r><w:rPr><mc:AlternateContent><mc:Choice Requires="w14"><w14:glow w14:rad="1"/></mc:Choice><mc:Fallback><w:b/></mc:Fallback></mc:AlternateContent><w:i/></w:rPr><w:t>x</w:t></w:r></w:p>"#,
    );
    let properties = first_run(&document).properties();
    assert_eq!(
        (properties.bold(), properties.italic()),
        (Some(true), Some(true))
    );
}

#[test]
fn reads_styles_and_the_document_defaults() {
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:asciiTheme="minorHAnsi"/><w:sz w:val="22"/><w:lang w:val="en-GB"/></w:rPr></w:rPrDefault><w:pPrDefault/></w:docDefaults><w:latentStyles w:defQFormat="0"/><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:link w:val="Heading1Char"/><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="character" w:customStyle="1" w:styleId="Heading1Char"><w:name w:val="Heading 1 Char"/><w:rPr><w:b/></w:rPr></w:style><w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"/><w:style w:type="table" w:default="1" w:styleId="TableNormal"/><w:style w:type="numbering" w:styleId="NoList"/></w:styles>"#
    );
    let document = with_styles(r#"<w:p><w:r><w:t>x</w:t></w:r></w:p>"#, &styles);
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
    let styles = document.styles();
    assert_eq!(styles.iter().len(), 6);
    let kinds: Vec<(&str, StyleKind)> = styles.iter().map(|s| (s.id(), s.kind())).collect();
    assert_eq!(
        kinds,
        [
            ("Normal", StyleKind::Paragraph),
            ("Heading1", StyleKind::Paragraph),
            ("Heading1Char", StyleKind::Character),
            ("DefaultParagraphFont", StyleKind::Character),
            ("TableNormal", StyleKind::Table),
            ("NoList", StyleKind::Numbering),
        ]
    );
    let heading = styles.get("Heading1").unwrap();
    assert_eq!(heading.name(), Some("heading 1"));
    assert_eq!(heading.based_on(), Some("Normal"));
    assert_eq!(heading.next(), Some("Normal"));
    assert_eq!(heading.link(), Some("Heading1Char"));
    assert!(!heading.is_default() && !heading.is_custom());
    assert_eq!(heading.run_properties().bold(), Some(true));
    assert_eq!(heading.run_properties().size(), Some(Pt(16.0)));
    assert!(styles.get("Heading1Char").unwrap().is_custom());
    assert!(styles.get("heading1").is_none());
    assert_eq!(
        styles.default_style(StyleKind::Paragraph).map(Style::id),
        Some("Normal")
    );
    assert_eq!(
        styles.default_style(StyleKind::Character).map(Style::id),
        Some("DefaultParagraphFont")
    );
    let defaults = styles.default_run_properties();
    assert_eq!(defaults.fonts().ascii_theme(), Some(ThemeFont::MinorHAnsi));
    assert_eq!(defaults.size(), Some(Pt(11.0)));
    assert_eq!(defaults.language().latin(), Some("en-GB"));
    // A document without a styles part has no styles.
    let plain = open("<w:p/>");
    assert_eq!(plain.styles().iter().len(), 0);
    assert_eq!(
        plain.styles().default_run_properties(),
        RunProperties::default()
    );
}

#[test]
fn repairs_damaged_style_lists() {
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:style w:styleId="A"><w:name w:val="first"/></w:style><w:style w:styleId="A"><w:name w:val="second"/></w:style><w:style w:type="paragraph" w:styleId=""><w:name w:val="no id"/></w:style><w:style w:type="list" w:styleId="B"><w:rPr><w:sz w:val="x"/></w:rPr></w:style></w:styles>"#
    );
    let document = with_styles("<w:p/>", &styles);
    let styles = document.styles();
    // The first style with an id is the one found by it; one without an id is dropped.
    assert_eq!(styles.get("A").unwrap().name(), Some("first"));
    assert_eq!(styles.iter().len(), 3);
    assert_eq!(styles.get("B").unwrap().kind(), StyleKind::Paragraph);
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect();
    assert_eq!(messages.len(), 4, "{messages:?}");
    assert!(messages[0].starts_with("/word/styles.xml, line 1, column "));
    assert!(
        messages[0]
            .ends_with("a second style with id A is ignored where styles are looked up by id")
    );
    assert!(messages[1].ends_with("a w:style without a w:styleId is ignored"));
    assert!(messages[2].ends_with(
        r#"w:style w:type="list" is not valid; the style is read as a paragraph style"#
    ));
    assert!(messages[3].ends_with(r#"w:sz w:val="x" is not valid; it is ignored"#));
    // A styles part whose root is not w:styles is damage.
    let err = Document::from_bytes(package(
        DOCUMENT,
        &document_xml("<w:p/>"),
        &[
            (
                "word/_rels/document.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
            ),
            ("word/styles.xml", "<x/>"),
        ],
    ))
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "/word/styles.xml, line 1, column 1: the root element is <x>, not w:styles"
    );
}

#[test]
fn shares_the_node_limit_between_parts() {
    // From the review of #38: 100 paragraphs cost 100 nodes, 100 one-attribute styles
    // 300, and both count against one limit.
    let body = "<w:p/>".repeat(100);
    let mut styles = format!(r#"<w:styles xmlns:w="{W}">"#);
    for n in 0..100 {
        styles.push_str(&format!(r#"<w:style w:styleId="s{n}"/>"#));
    }
    styles.push_str("</w:styles>");
    let data = package(
        DOCUMENT,
        &document_xml(&body),
        &[
            (
                "word/_rels/document.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
            ),
            ("word/styles.xml", &styles),
        ],
    );
    let options = LoadOptions::new().with_limits(Limits::new().with_max_nodes(350));
    match options.open_bytes(data.clone()).unwrap_err() {
        Error::LimitExceeded {
            limit: Limit::Nodes,
            context,
            ..
        } => {
            assert!(context.starts_with("/word/styles.xml"), "{context}");
        }
        other => panic!("{other}"),
    }
    let options = LoadOptions::new().with_limits(Limits::new().with_max_nodes(400));
    assert!(options.open_bytes(data).is_ok());
}

#[test]
fn reads_a_zero_text_scale_as_not_set() {
    // As in LibreOffice's tdf106001.docx, which Word shows at normal width.
    let document = open(r#"<w:p><w:r><w:rPr><w:w w:val="0"/></w:rPr><w:t>x</w:t></w:r></w:p>"#);
    assert_eq!(first_run(&document).properties().scale(), None);
    assert_eq!(document.diagnostics().len(), 1);
}

#[test]
fn reports_damage_to_style_flags_and_defaults() {
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="20"/></w:rPr></w:rPrDefault></w:docDefaults><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="40"/></w:rPr></w:rPrDefault></w:docDefaults><w:latentStyles/><w:latentStyles/><w:style w:styleId="A" w:default="yes" w:customStyle="x"/></w:styles>"#
    );
    let document = with_styles("<w:p/>", &styles);
    // The first of two document defaults is the one read, as for styles.
    assert_eq!(
        document.styles().default_run_properties().size(),
        Some(Pt(10.0))
    );
    let style = document.styles().get("A").unwrap();
    assert!(!style.is_default() && !style.is_custom());
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect();
    assert_eq!(messages.len(), 4, "{messages:?}");
    assert!(messages[0].ends_with("a second w:docDefaults is ignored"));
    assert!(messages[1].ends_with("a second w:latentStyles is ignored"));
    assert!(messages[2].ends_with(r#"w:style w:default="yes" is not valid; it is read as off"#));
    assert!(messages[3].ends_with(r#"w:style w:customStyle="x" is not valid; it is read as off"#));
}

#[test]
fn counts_invalid_values_past_the_diagnostic_limit() {
    let r_pr = r#"<w:b w:val="x"/>"#.repeat(1500);
    let document = open(&format!(
        "<w:p><w:r><w:rPr>{r_pr}</w:rPr><w:t>x</w:t></w:r></w:p>"
    ));
    let diagnostics = document.diagnostics();
    assert_eq!(diagnostics.len(), 1001);
    assert_eq!(
        diagnostics[1000].message(),
        "500 more diagnostics are not kept"
    );
}

fn first_paragraph(document: &Document) -> Paragraph<'_> {
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().next()
    else {
        panic!("no paragraph");
    };
    paragraph
}

#[test]
fn reads_the_properties_written_on_a_paragraph() {
    let document = open(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/><w:keepNext/><w:keepLines w:val="1"/><w:pageBreakBefore w:val="0"/><w:widowControl w:val="off"/><w:numPr><w:ilvl w:val="1"/><w:numId w:val="3"/></w:numPr><w:suppressLineNumbers/><w:pBdr><w:top w:val="single" w:sz="4" w:space="1" w:color="auto"/><w:left w:val="double" w:sz="12" w:space="4" w:color="FF0000" w:shadow="1"/><w:between w:val="dotted" w:sz="2" w:space="0" w:color="auto"/><w:bar w:val="thick" w:sz="24" w:space="0" w:color="auto"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/><w:tabs><w:tab w:val="left" w:pos="720"/><w:tab w:val="center" w:leader="dot" w:pos="4320"/><w:tab w:val="right" w:leader="middleDot" w:pos="9360"/><w:tab w:val="clear" w:pos="1440"/><w:tab w:val="decimal" w:pos="-360"/></w:tabs><w:suppressAutoHyphens/><w:spacing w:before="240" w:beforeLines="100" w:beforeAutospacing="1" w:after="120" w:line="276" w:lineRule="auto"/><w:ind w:left="720" w:right="-360" w:hanging="360" w:leftChars="200"/><w:contextualSpacing/><w:mirrorIndents/><w:wordWrap w:val="0"/><w:bidi/><w:snapToGrid w:val="0"/><w:jc w:val="both"/><w:outlineLvl w:val="0"/><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
    let properties = first_paragraph(&document).properties();
    assert_eq!(properties.style(), Some("Heading1"));
    assert_eq!(properties.keep_next(), Some(true));
    assert_eq!(properties.keep_lines(), Some(true));
    assert_eq!(properties.page_break_before(), Some(false));
    assert_eq!(properties.widow_control(), Some(false));
    assert_eq!(properties.suppress_line_numbers(), Some(true));
    assert_eq!(properties.suppress_auto_hyphens(), Some(true));
    assert_eq!(properties.contextual_spacing(), Some(true));
    assert_eq!(properties.mirror_indents(), Some(true));
    assert_eq!(properties.word_wrap(), Some(false));
    assert_eq!(properties.right_to_left(), Some(true));
    assert_eq!(properties.snap_to_grid(), Some(false));
    let numbering = properties.numbering().unwrap();
    assert_eq!(
        (numbering.instance(), numbering.level()),
        (Some(3), Some(1))
    );
    let borders = properties.borders().unwrap();
    let top = borders.top().unwrap();
    assert_eq!(top.style(), Some(BorderStyle::Single));
    assert_eq!((top.width(), top.space()), (Some(Pt(0.5)), Some(Pt(1.0))));
    assert_eq!(top.color().value(), Some(ColorValue::Auto));
    let start = borders.start().unwrap();
    assert_eq!(start.style(), Some(BorderStyle::Double));
    assert_eq!(start.width(), Some(Pt(1.5)));
    assert_eq!(start.shadow(), Some(true));
    assert_eq!(borders.bottom(), None);
    assert_eq!(borders.end(), None);
    assert_eq!(
        borders.between().unwrap().style(),
        Some(BorderStyle::Dotted)
    );
    assert_eq!(borders.bar().unwrap().width(), Some(Pt(3.0)));
    assert_eq!(
        properties.shading().unwrap().fill().value(),
        Some(ColorValue::Rgb(0xF2, 0xF2, 0xF2))
    );
    let tabs: Vec<(Option<TabKind>, Option<TabLeader>, f64)> = properties
        .tabs()
        .iter()
        .map(|tab| (tab.kind(), tab.leader(), tab.position().0))
        .collect();
    assert_eq!(
        tabs,
        [
            (Some(TabKind::Left), None, 36.0),
            (Some(TabKind::Center), Some(TabLeader::Dot), 216.0),
            (Some(TabKind::Right), Some(TabLeader::MiddleDot), 468.0),
            (Some(TabKind::Clear), None, 72.0),
            (Some(TabKind::Decimal), None, -18.0),
        ]
    );
    let spacing = properties.spacing().unwrap();
    assert_eq!(
        (spacing.before(), spacing.after()),
        (Some(Pt(12.0)), Some(Pt(6.0)))
    );
    assert_eq!(
        (spacing.before_lines(), spacing.after_lines()),
        (Some(100), None)
    );
    assert_eq!(
        (spacing.before_auto(), spacing.after_auto()),
        (Some(true), None)
    );
    assert_eq!(spacing.line(), Some(LineSpacing::Multiple(1.15)));
    let indentation = properties.indentation().unwrap();
    assert_eq!(indentation.start(), Some(Pt(36.0)));
    assert_eq!(indentation.end(), Some(Pt(-18.0)));
    assert_eq!(indentation.hanging(), Some(Pt(18.0)));
    assert_eq!(indentation.first_line(), None);
    assert_eq!(indentation.start_chars(), Some(200));
    assert_eq!(properties.justification(), Some(Justification::Both));
    assert_eq!(properties.outline_level(), Some(0));
    let mark = properties.mark_run_properties();
    assert_eq!((mark.bold(), mark.size()), (Some(true), Some(Pt(14.0))));
    // A paragraph without properties sets nothing.
    let plain = open("<w:p/>");
    assert_eq!(
        first_paragraph(&plain).properties(),
        ParagraphProperties::default()
    );
}

#[test]
fn reads_line_spacing_rules_and_strict_indentation() {
    let line = |xml: &str| {
        let document = open(&format!("<w:p><w:pPr>{xml}</w:pPr></w:p>"));
        first_paragraph(&document)
            .properties()
            .spacing()
            .and_then(Spacing::line)
    };
    assert_eq!(
        line(r#"<w:spacing w:line="240"/>"#),
        Some(LineSpacing::Multiple(1.0))
    );
    assert_eq!(
        line(r#"<w:spacing w:line="300" w:lineRule="exact"/>"#),
        Some(LineSpacing::Exact(Pt(15.0)))
    );
    assert_eq!(
        line(r#"<w:spacing w:line="12pt" w:lineRule="atLeast"/>"#),
        Some(LineSpacing::AtLeast(Pt(12.0)))
    );
    assert_eq!(line(r#"<w:spacing w:before="0"/>"#), None);
    let xml = r#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main" conformance="strict"><w:body><w:p><w:pPr><w:ind w:start="1in" w:end="0.5in" w:firstLine="0.25in"/><w:jc w:val="start"/></w:pPr></w:p></w:body></w:document>"#;
    let document = Document::from_bytes(package(DOCUMENT, xml, &[])).unwrap();
    let properties = first_paragraph(&document).properties();
    let indentation = properties.indentation().unwrap();
    assert_eq!(
        (
            indentation.start(),
            indentation.end(),
            indentation.first_line()
        ),
        (Some(Pt(72.0)), Some(Pt(36.0)), Some(Pt(18.0)))
    );
    assert_eq!(properties.justification(), Some(Justification::Start));
}

#[test]
fn reads_invalid_paragraph_values_as_not_set() {
    let document = open(
        r#"<w:p><w:pPr><w:jc w:val="middle"/><w:spacing w:before="much" w:line="240" w:lineRule="often"/><w:ind w:hanging="-5"/><w:outlineLvl w:val="twelve"/><w:tabs><w:tab w:val="left" w:pos="far"/><w:tab w:val="sideways" w:pos="10"/></w:tabs><w:pBdr><w:top w:val="zigzag" w:sz="-1"/></w:pBdr><w:keepNext w:val="maybe"/></w:pPr></w:p>"#,
    );
    let properties = first_paragraph(&document).properties();
    assert_eq!(properties.justification(), None);
    let spacing = properties.spacing().unwrap();
    assert_eq!((spacing.before(), spacing.line()), (None, None));
    assert_eq!(properties.indentation().unwrap().hanging(), None);
    assert_eq!(properties.outline_level(), None);
    // The stop without a position is left out; the one without a valid kind is kept.
    assert_eq!(properties.tabs().len(), 1);
    assert_eq!(properties.tabs()[0].kind(), None);
    let top = properties.borders().unwrap().top().unwrap();
    assert_eq!((top.style(), top.width()), (None, None));
    assert_eq!(properties.keep_next(), None);
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect();
    assert_eq!(messages.len(), 10, "{messages:?}");
    assert!(messages[0].ends_with(r#": w:jc w:val="middle" is not valid; it is ignored"#));
}

#[test]
fn reads_the_paragraph_properties_of_styles_and_defaults() {
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:styleId="Heading1"><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="0"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:sz w:val="32"/></w:rPr></w:style></w:styles>"#
    );
    let document = with_styles("<w:p/>", &styles);
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
    let styles = document.styles();
    let defaults = styles.default_paragraph_properties();
    let spacing = defaults.spacing().unwrap();
    assert_eq!(spacing.after(), Some(Pt(8.0)));
    assert_eq!(spacing.line(), Some(LineSpacing::Multiple(259.0 / 240.0)));
    let heading = styles.get("Heading1").unwrap().paragraph_properties();
    assert_eq!(heading.keep_next(), Some(true));
    assert_eq!(heading.outline_level(), Some(0));
    assert_eq!(heading.spacing().unwrap().before(), Some(Pt(12.0)));
}

#[test]
fn reports_invalid_paragraph_values_of_styles_and_defaults() {
    // From the review of #39: the paragraph properties of the document defaults and of
    // a style are checked at load, like those of the body.
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:pPrDefault><w:pPr><w:spacing w:line="240" w:lineRule="often"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:styleId="X"><w:pPr><w:jc w:val="middle"/></w:pPr></w:style></w:styles>"#
    );
    let document = with_styles("<w:p/>", &styles);
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[0].starts_with("/word/styles.xml, line 1, column "));
    assert!(messages[0].ends_with(r#"w:spacing w:lineRule="often" is not valid; it is ignored"#));
    assert!(messages[1].starts_with("/word/styles.xml, line 1, column "));
    assert!(messages[1].ends_with(r#"w:jc w:val="middle" is not valid; it is ignored"#));
}

#[test]
fn keeps_values_outside_the_ranges_of_the_prose_as_written() {
    // Negative line spacing, as in LibreOffice's tdf125469_singleSpacing.docx, and list
    // and outline levels past those ECMA-376 names, as in tdf61309.docx.
    let document = open(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="12"/><w:numId w:val="1"/></w:numPr><w:spacing w:line="-240"/><w:outlineLvl w:val="25"/></w:pPr></w:p><w:p><w:pPr><w:spacing w:line="-240" w:lineRule="exact"/></w:pPr></w:p><w:p><w:pPr><w:spacing w:lineRule="often"/></w:pPr></w:p>"#,
    );
    let paragraphs: Vec<ParagraphProperties> = document
        .sections()
        .next()
        .unwrap()
        .blocks()
        .map(|block| match block {
            Block::Paragraph(paragraph) => paragraph.properties(),
            _ => panic!("not a paragraph"),
        })
        .collect();
    let first = &paragraphs[0];
    assert_eq!(first.numbering().unwrap().level(), Some(12));
    assert_eq!(first.outline_level(), Some(25));
    assert_eq!(
        first.spacing().unwrap().line(),
        Some(LineSpacing::Multiple(-1.0))
    );
    assert_eq!(
        paragraphs[1].spacing().unwrap().line(),
        Some(LineSpacing::Exact(Pt(-12.0)))
    );
    // A rule that is not valid is reported even without a w:line.
    assert_eq!(paragraphs[2].spacing().unwrap().line(), None);
    let messages: Vec<&str> = document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].ends_with(r#"w:spacing w:lineRule="often" is not valid; it is ignored"#));
}

fn first_table(document: &Document) -> Table<'_> {
    document
        .sections()
        .next()
        .unwrap()
        .blocks()
        .find_map(|block| match block {
            Block::Table(table) => Some(table),
            _ => None,
        })
        .expect("no table")
}

fn messages(document: &Document) -> Vec<&str> {
    document
        .diagnostics()
        .iter()
        .map(Diagnostic::message)
        .collect()
}

#[test]
fn reads_the_properties_written_on_a_table() {
    let document = open(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblpPr w:leftFromText="180" w:rightFromText="180" w:topFromText="0" w:bottomFromText="144" w:vertAnchor="page" w:horzAnchor="margin" w:tblpXSpec="center" w:tblpY="-720"/><w:tblOverlap w:val="never"/><w:bidiVisual/><w:tblStyleRowBandSize w:val="2"/><w:tblStyleColBandSize w:val="1"/><w:tblW w:w="5000" w:type="pct"/><w:jc w:val="center"/><w:tblCellSpacing w:w="20" w:type="dxa"/><w:tblInd w:w="-108" w:type="dxa"/><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="double" w:sz="8" w:space="0" w:color="00FF00"/><w:insideH w:val="dashed" w:sz="2" w:space="0" w:color="auto"/><w:insideV w:val="nil"/></w:tblBorders><w:shd w:val="clear" w:color="auto" w:fill="EEEEEE"/><w:tblLayout w:type="fixed"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar><w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/><w:tblCaption w:val="Prices"/><w:tblDescription w:val="Prices by year"/></w:tblPr><w:tblGrid><w:gridCol w:w="4680"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#,
    );
    assert!(messages(&document).is_empty(), "{:?}", messages(&document));
    let properties = first_table(&document).properties();
    assert_eq!(properties.style(), Some("TableGrid"));
    let position = properties.position().unwrap();
    assert_eq!(
        (
            position.left_from_text(),
            position.top_from_text(),
            position.bottom_from_text()
        ),
        (Some(Pt(9.0)), Some(Pt(0.0)), Some(Pt(7.2)))
    );
    assert_eq!(position.vertical_anchor(), Some(PositionAnchor::Page));
    assert_eq!(position.horizontal_anchor(), Some(PositionAnchor::Margin));
    assert_eq!(
        position.horizontal_alignment(),
        Some(HorizontalAlignment::Center)
    );
    assert_eq!((position.x(), position.y()), (None, Some(Pt(-36.0))));
    assert_eq!(position.vertical_alignment(), None);
    assert_eq!(properties.overlap(), Some(TableOverlap::Never));
    assert_eq!(properties.right_to_left(), Some(true));
    assert_eq!(
        (properties.row_band_size(), properties.column_band_size()),
        (Some(2), Some(1))
    );
    assert_eq!(properties.width(), Some(TableWidth::Percent(100.0)));
    assert_eq!(properties.justification(), Some(TableJustification::Center));
    assert_eq!(properties.cell_spacing(), Some(TableWidth::Length(Pt(1.0))));
    assert_eq!(properties.indentation(), Some(TableWidth::Length(Pt(-5.4))));
    let borders = properties.borders().unwrap();
    assert_eq!(borders.top().unwrap().width(), Some(Pt(0.5)));
    assert_eq!(borders.start().unwrap().style(), Some(BorderStyle::Double));
    assert_eq!(
        borders.inside_horizontal().unwrap().style(),
        Some(BorderStyle::Dashed)
    );
    assert_eq!(
        borders.inside_vertical().unwrap().style(),
        Some(BorderStyle::Nil)
    );
    assert_eq!((borders.bottom(), borders.end()), (None, None));
    assert_eq!(
        properties.shading().unwrap().fill().value(),
        Some(ColorValue::Rgb(0xEE, 0xEE, 0xEE))
    );
    assert_eq!(properties.layout(), Some(TableLayout::Fixed));
    let margins = properties.cell_margins().unwrap();
    assert_eq!(margins.top(), Some(TableWidth::Length(Pt(0.0))));
    assert_eq!(margins.start(), Some(TableWidth::Length(Pt(5.4))));
    assert_eq!(margins.end(), Some(TableWidth::Length(Pt(5.4))));
    assert_eq!(margins.bottom(), None);
    let look = properties.look().unwrap();
    assert_eq!(
        (
            look.first_row(),
            look.last_row(),
            look.first_column(),
            look.last_column()
        ),
        (Some(true), Some(false), Some(true), Some(false))
    );
    assert_eq!(
        (look.no_horizontal_bands(), look.no_vertical_bands()),
        (Some(false), Some(true))
    );
    assert_eq!(properties.caption(), Some("Prices"));
    assert_eq!(properties.description(), Some("Prices by year"));
    // A table without w:tblPr sets nothing.
    let plain = open("<w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>");
    assert_eq!(first_table(&plain).properties(), TableProperties::default());
}

#[test]
fn reads_the_properties_of_rows_and_cells() {
    let document = open(
        r#"<w:tbl><w:tr><w:tblPrEx><w:tblW w:w="2500" w:type="pct"/><w:tblLook w:val="0600"/></w:tblPrEx><w:trPr><w:gridBefore w:val="1"/><w:gridAfter w:val="2"/><w:wBefore w:w="426" w:type="dxa"/><w:wAfter w:w="0" w:type="auto"/><w:cantSplit/><w:trHeight w:val="567" w:hRule="exact"/><w:tblHeader/><w:tblCellSpacing w:w="0" w:type="nil"/><w:jc w:val="end"/><w:hidden w:val="0"/></w:trPr><w:tc><w:tcPr><w:tcW w:w="1440" w:type="dxa"/><w:gridSpan w:val="2"/><w:vMerge w:val="restart"/><w:tcBorders><w:start w:val="single" w:sz="12" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:tl2br w:val="single" w:sz="4" w:space="0" w:color="FF0000"/></w:tcBorders><w:shd w:val="pct25" w:color="auto" w:fill="auto"/><w:noWrap/><w:tcMar><w:start w:w="0" w:type="dxa"/><w:bottom w:w="57" w:type="dxa"/></w:tcMar><w:textDirection w:val="btLr"/><w:tcFitText/><w:vAlign w:val="center"/><w:hideMark/></w:tcPr><w:p/></w:tc></w:tr><w:tr><w:trPr><w:trHeight w:val="300"/></w:trPr><w:tc><w:tcPr><w:vMerge/><w:hMerge w:val="restart"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>"#,
    );
    assert!(messages(&document).is_empty(), "{:?}", messages(&document));
    let rows: Vec<TableRow> = first_table(&document).rows().collect();
    let exceptions = rows[0].table_property_exceptions();
    assert_eq!(exceptions.width(), Some(TableWidth::Percent(50.0)));
    let look = exceptions.look().unwrap();
    assert_eq!(
        (
            look.first_row(),
            look.no_horizontal_bands(),
            look.no_vertical_bands()
        ),
        (Some(false), Some(true), Some(true))
    );
    let row = rows[0].properties();
    assert_eq!((row.grid_before(), row.grid_after()), (Some(1), Some(2)));
    assert_eq!(row.width_before(), Some(TableWidth::Length(Pt(21.3))));
    assert_eq!(row.width_after(), Some(TableWidth::Auto));
    assert_eq!(row.cant_split(), Some(true));
    let height = row.height().unwrap();
    assert_eq!(
        (height.height(), height.rule()),
        (Some(Pt(28.35)), Some(HeightRule::Exact))
    );
    assert_eq!(row.header(), Some(true));
    assert_eq!(row.cell_spacing(), Some(TableWidth::Nil));
    assert_eq!(row.justification(), Some(TableJustification::End));
    assert_eq!(row.hidden(), Some(false));
    // Without w:hRule the rule is not set; Word reads it as at least.
    let second = rows[1].properties();
    assert_eq!(second.height().unwrap().rule(), None);
    assert_eq!(
        rows[1].table_property_exceptions(),
        TableProperties::default()
    );

    let cell = rows[0].cells().next().unwrap().properties();
    assert_eq!(cell.width(), Some(TableWidth::Length(Pt(72.0))));
    assert_eq!(cell.grid_span(), Some(2));
    assert_eq!(cell.vertical_merge(), Some(CellMerge::Restart));
    assert_eq!(cell.horizontal_merge(), None);
    let borders = cell.borders().unwrap();
    assert_eq!(borders.sides().start().unwrap().width(), Some(Pt(1.5)));
    assert!(borders.sides().inside_horizontal().is_some());
    assert_eq!(
        borders.top_left_to_bottom_right().unwrap().color().value(),
        Some(ColorValue::Rgb(0xFF, 0, 0))
    );
    assert_eq!(borders.top_right_to_bottom_left(), None);
    assert_eq!(
        cell.shading().unwrap().pattern(),
        Some(ShadingPattern::Percent25)
    );
    assert_eq!(cell.no_wrap(), Some(true));
    let margins = cell.margins().unwrap();
    assert_eq!(margins.start(), Some(TableWidth::Length(Pt(0.0))));
    assert_eq!(margins.bottom(), Some(TableWidth::Length(Pt(2.85))));
    assert_eq!(
        cell.text_direction(),
        Some(TextDirection::BottomToTopLeftToRight)
    );
    assert_eq!(cell.fit_text(), Some(true));
    assert_eq!(
        cell.vertical_justification(),
        Some(VerticalJustification::Center)
    );
    assert_eq!(cell.hide_mark(), Some(true));
    // w:vMerge without w:val continues the merge above.
    let merged = rows[1].cells().next().unwrap().properties();
    assert_eq!(merged.vertical_merge(), Some(CellMerge::Continue));
    assert_eq!(merged.horizontal_merge(), Some(CellMerge::Restart));
}

#[test]
fn reads_table_widths_as_word_does() {
    let width = |attributes: &str| {
        let document = open(&format!(
            "<w:tbl><w:tblPr><w:tblW {attributes}/></w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"
        ));
        let width = first_table(&document).properties().width();
        (width, messages(&document).len())
    };
    // pct without a percent sign is in fiftieths of a percent (MS-OI29500 §17.4.87(a)),
    // an xsd:decimal.
    assert_eq!(
        width(r#"w:w="4625" w:type="pct""#),
        (Some(TableWidth::Percent(92.5)), 0)
    );
    assert_eq!(
        width(r#"w:w="4625.5" w:type="pct""#),
        (Some(TableWidth::Percent(92.51)), 0)
    );
    assert_eq!(
        width(r#"w:w="+2500" w:type="pct""#),
        (Some(TableWidth::Percent(50.0)), 0)
    );
    // A percent sign makes a percentage whatever the type (§17.4.87(b)); the type is
    // still checked.
    assert_eq!(
        width(r#"w:w="50%" w:type="pct""#),
        (Some(TableWidth::Percent(50.0)), 0)
    );
    assert_eq!(
        width(r#"w:w="9%" w:type="dxa""#),
        (Some(TableWidth::Percent(9.0)), 0)
    );
    assert_eq!(width(r#"w:w="9%""#), (Some(TableWidth::Percent(9.0)), 0));
    assert_eq!(
        width(r#"w:w="50%" w:type="bogus""#),
        (Some(TableWidth::Percent(50.0)), 1)
    );
    // dxa is the type assumed when none is written (§17.4.87).
    assert_eq!(width(r#"w:w="70""#), (Some(TableWidth::Length(Pt(3.5))), 0));
    assert_eq!(
        width(r#"w:w="108.0" w:type="dxa""#),
        (Some(TableWidth::Length(Pt(5.4))), 0)
    );
    assert_eq!(
        width(r#"w:w="1in" w:type="dxa""#),
        (Some(TableWidth::Length(Pt(72.0))), 0)
    );
    assert_eq!(
        width(r#"w:w="81.85pt""#),
        (Some(TableWidth::Length(Pt(81.85))), 0)
    );
    assert_eq!(
        width(r#"w:w="-1.5pc""#),
        (Some(TableWidth::Length(Pt(-18.0))), 0)
    );
    assert_eq!(
        width(r#"w:w="0" w:type="auto""#),
        (Some(TableWidth::Auto), 0)
    );
    assert_eq!(width(r#"w:type="nil""#), (Some(TableWidth::Nil), 0));
    // A w Word ignores under auto is still checked.
    assert_eq!(
        width(r#"w:w="wide" w:type="auto""#),
        (Some(TableWidth::Auto), 1)
    );
    // Not valid by the schema: read as not set, with a diagnostic.
    for attributes in [
        r#"w:w="1e2%""#,
        r#"w:w="+50%""#,
        r#"w:w=".5%""#,
        r#"w:w="5.%""#,
        r#"w:w="x%""#,
        r#"w:w="5cm" w:type="pct""#,
        r#"w:w="wide" w:type="dxa""#,
        r#"w:w="1e3""#,
        r#"w:w="5 cm""#,
        r#"w:w="100" w:type="twips""#,
    ] {
        assert_eq!(width(attributes), (None, 1), "{attributes}");
    }
    // dxa without a width gives none.
    assert_eq!(width(r#"w:type="dxa""#), (None, 0));
}

#[test]
fn reads_the_look_of_a_table_as_word_does() {
    let look = |attributes: &str| {
        let document = open(&format!(
            "<w:tbl><w:tblPr><w:tblLook {attributes}/></w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"
        ));
        let look = *first_table(&document).properties().look().unwrap();
        (
            [
                look.first_row(),
                look.last_row(),
                look.first_column(),
                look.last_column(),
                look.no_horizontal_bands(),
                look.no_vertical_bands(),
            ],
            messages(&document).len(),
        )
    };
    let (yes, no) = (Some(true), Some(false));
    // The bit mask alone is read (lower case too, as Word writes it at times).
    assert_eq!(look(r#"w:val="04A0""#), ([yes, no, yes, no, no, yes], 0));
    assert_eq!(look(r#"w:val="01e0""#), ([yes, yes, yes, yes, no, no], 0));
    assert_eq!(look(r#"w:val="0000""#), ([no; 6], 0));
    // With any of the attributes the mask is not read (MS-OI29500 §17.4.55(c)), but it
    // is checked.
    assert_eq!(
        look(r#"w:val="04A0" w:firstRow="0""#),
        ([no, None, None, None, None, None], 0)
    );
    assert_eq!(
        look(r#"w:val="4A0" w:firstRow="1""#),
        ([yes, None, None, None, None, None], 1)
    );
    // Not four hexadecimal digits: nothing is set.
    assert_eq!(look(r#"w:val="4A0""#), ([None; 6], 1));
    assert_eq!(look(r#"w:val="04G0""#), ([None; 6], 1));
    assert_eq!(look(r#"w:firstRow="yes""#), ([None; 6], 1));
}

#[test]
fn reads_invalid_table_values_as_not_set() {
    let document = open(
        r#"<w:tbl><w:tblPr><w:jc w:val="both"/><w:tblLayout w:type="loose"/><w:tblOverlap w:val="maybe"/><w:tblpPr w:vertAnchor="paragraph" w:tblpX="far"/><w:tblStyleRowBandSize w:val="two"/></w:tblPr><w:tr><w:tblPrEx><w:jc w:val="middle"/></w:tblPrEx><w:trPr><w:trHeight w:val="-5" w:hRule="exactly"/><w:gridBefore w:val="x"/></w:trPr><w:tc><w:tcPr><w:vMerge w:val="start"/><w:vAlign w:val="middle"/><w:textDirection w:val="up"/><w:noWrap w:val="no"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>"#,
    );
    let table = first_table(&document);
    let properties = table.properties();
    assert_eq!(properties.justification(), None);
    assert_eq!(properties.layout(), None);
    assert_eq!(properties.overlap(), None);
    let position = properties.position().unwrap();
    assert_eq!((position.vertical_anchor(), position.x()), (None, None));
    assert_eq!(properties.row_band_size(), None);
    let row = table.rows().next().unwrap();
    assert_eq!(row.table_property_exceptions().justification(), None);
    let height = *row.properties().height().unwrap();
    assert_eq!((height.height(), height.rule()), (None, None));
    let cell = row.cells().next().unwrap().properties();
    assert_eq!(cell.vertical_merge(), None);
    assert_eq!(cell.vertical_justification(), None);
    assert_eq!(cell.text_direction(), None);
    assert_eq!(cell.no_wrap(), None);
    let messages = messages(&document);
    assert_eq!(messages.len(), 14, "{messages:?}");
    assert!(messages[0].ends_with(r#": w:jc w:val="both" is not valid; it is ignored"#));
    assert!(messages[7].ends_with(r#": w:trHeight w:val="-5" is not valid; it is ignored"#));
    assert!(messages[13].ends_with(r#": w:noWrap w:val="no" is not valid; it is ignored"#));
}

#[test]
fn reads_the_table_properties_of_styles() {
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:style w:type="table" w:styleId="Grid"><w:tblPr><w:tblBorders><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders><w:tblCellMar><w:left w:w="108" w:type="dxa"/></w:tblCellMar><w:jc w:val="left"/><w:tblLayout w:type="x"/></w:tblPr><w:trPr><w:cantSplit/><w:jc w:val="right"/><w:hidden w:val="maybe"/></w:trPr><w:tcPr><w:vAlign w:val="bottom"/><w:shd w:val="nope"/></w:tcPr></w:style></w:styles>"#
    );
    let document = with_styles("<w:p/>", &styles);
    let style = document.styles().get("Grid").unwrap();
    let table = style.table_properties();
    assert!(table.borders().unwrap().inside_vertical().is_some());
    assert_eq!(
        table.cell_margins().unwrap().start(),
        Some(TableWidth::Length(Pt(5.4)))
    );
    assert_eq!(table.justification(), Some(TableJustification::Left));
    assert_eq!(style.row_properties().cant_split(), Some(true));
    assert_eq!(
        style.row_properties().justification(),
        Some(TableJustification::Right)
    );
    let cell = style.cell_properties();
    assert_eq!(
        cell.vertical_justification(),
        Some(VerticalJustification::Bottom)
    );
    assert_eq!(cell.shading().unwrap().pattern(), None);
    // The values that are not valid are reported where the style is.
    let messages = messages(&document);
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert!(messages[0].starts_with("/word/styles.xml, line 1, column "));
    assert!(messages[0].ends_with(r#"w:tblLayout w:type="x" is not valid; it is ignored"#));
    assert!(messages[1].ends_with(r#"w:hidden w:val="maybe" is not valid; it is ignored"#));
    assert!(messages[2].ends_with(r#"w:shd w:val="nope" is not valid; it is ignored"#));
    // A style without them sets none.
    let plain = format!(r#"<w:styles xmlns:w="{W}"><w:style w:styleId="P"/></w:styles>"#);
    let plain = with_styles("<w:p/>", &plain);
    let style = plain.styles().get("P").unwrap();
    assert_eq!(style.table_properties(), TableProperties::default());
    assert_eq!(style.row_properties(), TableRowProperties::default());
    assert_eq!(style.cell_properties(), TableCellProperties::default());
}

#[test]
fn reads_strict_table_properties() {
    let xml = r#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main" conformance="strict"><w:body><w:tbl><w:tblPr><w:tblW w:w="50%" w:type="pct"/><w:tblInd w:w="0.5in" w:type="dxa"/><w:tblCellMar><w:start w:w="0.1in" w:type="dxa"/><w:end w:w="0.1in" w:type="dxa"/></w:tblCellMar><w:jc w:val="start"/></w:tblPr><w:tr><w:tc><w:tcPr><w:tcBorders><w:end w:val="single" w:sz="4"/></w:tcBorders></w:tcPr><w:p/></w:tc></w:tr></w:tbl></w:body></w:document>"#;
    let document = Document::from_bytes(package(DOCUMENT, xml, &[])).unwrap();
    assert!(messages(&document).is_empty(), "{:?}", messages(&document));
    let table = first_table(&document);
    let properties = table.properties();
    assert_eq!(properties.width(), Some(TableWidth::Percent(50.0)));
    assert_eq!(properties.indentation(), Some(TableWidth::Length(Pt(36.0))));
    let margins = properties.cell_margins().unwrap();
    assert_eq!(
        (margins.start(), margins.end()),
        (
            Some(TableWidth::Length(Pt(7.2))),
            Some(TableWidth::Length(Pt(7.2)))
        )
    );
    assert_eq!(properties.justification(), Some(TableJustification::Start));
    let cell = table.rows().next().unwrap().cells().next().unwrap();
    assert!(cell.properties().borders().unwrap().sides().end().is_some());
}

#[test]
fn reads_strict_text_directions_as_their_transitional_equivalents() {
    // The pairs ECMA-376 makes equivalent, as MS-OI29500 §17.18.93 quotes them.
    for (written, direction) in [
        ("lrTb", TextDirection::LeftToRightTopToBottom),
        ("tb", TextDirection::LeftToRightTopToBottom),
        ("tbRl", TextDirection::TopToBottomRightToLeft),
        ("rl", TextDirection::TopToBottomRightToLeft),
        ("btLr", TextDirection::BottomToTopLeftToRight),
        ("lr", TextDirection::BottomToTopLeftToRight),
        ("lrTbV", TextDirection::LeftToRightTopToBottomRotated),
        ("tbV", TextDirection::LeftToRightTopToBottomRotated),
        ("tbRlV", TextDirection::TopToBottomRightToLeftRotated),
        ("rlV", TextDirection::TopToBottomRightToLeftRotated),
        ("tbLrV", TextDirection::TopToBottomLeftToRightRotated),
        ("lrV", TextDirection::TopToBottomLeftToRightRotated),
    ] {
        let document = open(&format!(
            r#"<w:tbl><w:tr><w:tc><w:tcPr><w:textDirection w:val="{written}"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>"#
        ));
        let cell = first_table(&document)
            .rows()
            .next()
            .unwrap()
            .cells()
            .next()
            .unwrap()
            .properties();
        assert_eq!(cell.text_direction(), Some(direction), "{written}");
    }
}

#[test]
fn reads_the_full_properties_of_a_section() {
    let document = open(
        r#"<w:p/><w:sectPr><w:headerReference w:type="default" r:id="rId8"/><w:headerReference w:type="first" r:id="rId9"/><w:footerReference w:type="even" r:id="rId10"/><w:footnotePr><w:pos w:val="beneathText"/><w:numFmt w:val="lowerRoman"/><w:numStart w:val="1"/><w:numRestart w:val="eachPage"/></w:footnotePr><w:endnotePr><w:pos w:val="sectEnd"/><w:numFmt w:val="upperLetter"/></w:endnotePr><w:type w:val="continuous"/><w:pgSz w:w="11906" w:h="16838" w:orient="portrait" w:code="9"/><w:pgMar w:top="1134" w:right="850" w:bottom="1134" w:left="1701" w:header="708" w:footer="708" w:gutter="0"/><w:pgBorders w:zOrder="back" w:display="firstPage" w:offsetFrom="page"><w:top w:val="single" w:sz="4" w:space="24" w:color="auto"/><w:left w:val="apples" w:sz="20" w:space="24"/></w:pgBorders><w:lnNumType w:countBy="5" w:start="0" w:distance="360" w:restart="newSection"/><w:pgNumType w:fmt="upperRoman" w:start="3" w:chapStyle="1" w:chapSep="emDash"/><w:cols w:num="2" w:space="708" w:equalWidth="0" w:sep="1"><w:col w:w="3000" w:space="720"/><w:col w:w="5000"/></w:cols><w:formProt w:val="0"/><w:vAlign w:val="center"/><w:noEndnote/><w:titlePg/><w:textDirection w:val="tbRl"/><w:bidi/><w:rtlGutter w:val="0"/><w:docGrid w:type="lines" w:linePitch="360" w:charSpace="4096"/></w:sectPr>"#,
    );
    assert!(messages(&document).is_empty(), "{:?}", messages(&document));
    let section = document.sections().next().unwrap();
    let properties = section.properties();
    let headers: Vec<(Option<HeaderFooterKind>, &str)> = properties
        .headers()
        .iter()
        .map(|reference| (reference.kind(), reference.relationship_id()))
        .collect();
    assert_eq!(
        headers,
        [
            (Some(HeaderFooterKind::Default), "rId8"),
            (Some(HeaderFooterKind::First), "rId9")
        ]
    );
    let footer = &properties.footers()[0];
    assert_eq!(
        (footer.kind(), footer.relationship_id()),
        (Some(HeaderFooterKind::Even), "rId10")
    );
    let footnotes = properties.footnotes().unwrap();
    assert_eq!(footnotes.position(), Some(NotePosition::BeneathText));
    assert_eq!(footnotes.number_format(), Some(NumberFormat::LowerRoman));
    assert_eq!(
        (footnotes.start(), footnotes.restart()),
        (Some(1), Some(NoteRestart::EachPage))
    );
    let endnotes = properties.endnotes().unwrap();
    assert_eq!(endnotes.position(), Some(NotePosition::SectionEnd));
    assert_eq!(endnotes.number_format(), Some(NumberFormat::UpperLetter));
    assert_eq!(properties.start(), Some(SectionStart::Continuous));
    assert_eq!(properties.paper_code(), Some(9));
    assert_eq!(properties.margins().left(), Some(Pt(85.05)));
    let borders = properties.page_borders().unwrap();
    assert_eq!(borders.z_order(), Some(PageBorderZOrder::Back));
    assert_eq!(borders.display(), Some(PageBorderDisplay::FirstPage));
    assert_eq!(borders.offset_from(), Some(PageBorderOffset::Page));
    let top = borders.top().unwrap();
    assert_eq!(
        (top.style(), top.art(), top.width()),
        (Some(BorderStyle::Single), None, Some(Pt(0.5)))
    );
    // An art border has no line style, and its width is in points.
    let left = borders.left().unwrap();
    assert_eq!(left.style(), None);
    assert_eq!(left.art().map(ArtBorder::name), Some("apples"));
    assert_eq!(left.width(), Some(Pt(20.0)));
    assert_eq!((borders.bottom(), borders.right()), (None, None));
    let lines = properties.line_numbering().unwrap();
    assert_eq!((lines.count_by(), lines.start()), (Some(5), Some(0)));
    assert_eq!(lines.distance(), Some(Pt(18.0)));
    assert_eq!(lines.restart(), Some(LineNumberRestart::NewSection));
    let pages = properties.page_numbering().unwrap();
    assert_eq!(pages.format(), Some(NumberFormat::UpperRoman));
    assert_eq!((pages.start(), pages.chapter_style()), (Some(3), Some(1)));
    assert_eq!(pages.chapter_separator(), Some(ChapterSeparator::EmDash));
    let columns = properties.columns().unwrap();
    assert_eq!(
        (columns.count(), columns.space()),
        (Some(2), Some(Pt(35.4)))
    );
    assert_eq!(
        (columns.equal_width(), columns.separator()),
        (Some(false), Some(true))
    );
    let widths: Vec<(Option<Pt>, Option<Pt>)> = columns
        .columns()
        .iter()
        .map(|column| (column.width(), column.space()))
        .collect();
    assert_eq!(
        widths,
        [(Some(Pt(150.0)), Some(Pt(36.0))), (Some(Pt(250.0)), None)]
    );
    assert_eq!(properties.form_protection(), Some(false));
    assert_eq!(
        properties.vertical_justification(),
        Some(VerticalJustification::Center)
    );
    assert_eq!(properties.no_endnote(), Some(true));
    assert_eq!(properties.title_page(), Some(true));
    assert_eq!(
        properties.text_direction(),
        Some(TextDirection::TopToBottomRightToLeft)
    );
    assert_eq!(properties.right_to_left(), Some(true));
    assert_eq!(properties.right_to_left_gutter(), Some(false));
    let grid = properties.grid().unwrap();
    assert_eq!(grid.kind(), Some(DocumentGridKind::Lines));
    assert_eq!(
        (grid.line_pitch(), grid.character_space()),
        (Some(Pt(18.0)), Some(4096))
    );
}

#[test]
fn reads_invalid_section_values_as_not_set() {
    let document = open(
        r#"<w:p/><w:sectPr><w:headerReference w:type="odd" r:id="rId1"/><w:footerReference w:type="default"/><w:headerReference w:type="first"/><w:endnotePr><w:pos w:val="pageBottom"/></w:endnotePr><w:footnotePr><w:numRestart w:val="never"/></w:footnotePr><w:type/><w:pgSz w:w="12240" w:h="15840" w:code="letter"/><w:pgBorders w:zOrder="middle"><w:top w:val="lemons" w:sz="4"/></w:pgBorders><w:pgNumType w:fmt="roman"/><w:cols w:num="two" w:sep="maybe"><w:col w:w="-5"/></w:cols><w:titlePg w:val="yes"/><w:docGrid w:type="squares" w:linePitch="18pt"/></w:sectPr>"#,
    );
    let properties = document.sections().next().unwrap().properties();
    // A reference without a valid kind keeps its relationship; one without r:id is left
    // out.
    assert_eq!(properties.headers().len(), 1);
    assert_eq!(properties.headers()[0].kind(), None);
    assert!(properties.footers().is_empty());
    // Endnotes go to the end of the section or the document only.
    assert_eq!(properties.endnotes().unwrap().position(), None);
    assert_eq!(properties.footnotes().unwrap().restart(), None);
    // A w:type without w:val is a break to the next page (§17.6.22).
    assert_eq!(properties.start(), Some(SectionStart::NextPage));
    assert_eq!(properties.paper_code(), None);
    assert_eq!(properties.page_size().unwrap().width(), Pt(612.0));
    let borders = properties.page_borders().unwrap();
    assert_eq!(borders.z_order(), None);
    let top = borders.top().unwrap();
    assert_eq!((top.style(), top.art()), (None, None));
    assert_eq!(properties.page_numbering().unwrap().format(), None);
    let columns = properties.columns().unwrap();
    assert_eq!((columns.count(), columns.separator()), (None, None));
    assert_eq!(columns.columns()[0].width(), None);
    assert_eq!(properties.title_page(), None);
    let grid = properties.grid().unwrap();
    assert_eq!((grid.kind(), grid.line_pitch()), (None, None));
    let messages = messages(&document);
    assert_eq!(messages.len(), 14, "{messages:?}");
    assert!(
        messages[0].ends_with(r#": w:headerReference w:type="odd" is not valid; it is ignored"#)
    );
    assert!(messages[1].ends_with(r#": w:pos w:val="pageBottom" is not valid; it is ignored"#));
    assert!(messages[13].ends_with(": 2 header or footer references without r:id are ignored"));
}

#[test]
fn reads_art_borders_without_a_diagnostic() {
    let document = open(
        r#"<w:p><w:pPr><w:pBdr><w:bottom w:val="zigZag" w:sz="12" w:space="1"/></w:pBdr></w:pPr></w:p>"#,
    );
    assert!(messages(&document).is_empty(), "{:?}", messages(&document));
    let properties = first_paragraph(&document).properties();
    let bottom = properties.borders().unwrap().bottom().unwrap();
    assert_eq!(bottom.art().map(ArtBorder::name), Some("zigZag"));
    assert_eq!((bottom.style(), bottom.width()), (None, Some(Pt(12.0))));
}

#[test]
fn reads_strict_header_references() {
    let xml = r#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main" xmlns:r="http://purl.oclc.org/ooxml/officeDocument/relationships" conformance="strict"><w:body><w:p/><w:sectPr><w:headerReference w:type="default" r:id="rId4"/><w:cols w:space="0.5in"/></w:sectPr></w:body></w:document>"#;
    let document = Document::from_bytes(package(DOCUMENT, xml, &[])).unwrap();
    assert!(messages(&document).is_empty(), "{:?}", messages(&document));
    let properties = document.sections().next().unwrap().properties();
    assert_eq!(properties.headers()[0].relationship_id(), "rId4");
    assert_eq!(properties.columns().unwrap().space(), Some(Pt(36.0)));
}

#[test]
fn reads_section_properties_through_the_fallback() {
    // From the review of #43.
    let document = open(
        r#"<w:p/><w:sectPr><mc:AlternateContent><mc:Choice Requires="w14"><w:pgSz w:w="1" w:h="1"/><w:titlePg/></mc:Choice><mc:Fallback><w:pgSz w:w="12240" w:h="15840"/><w:headerReference w:type="default" r:id="rId8"/></mc:Fallback></mc:AlternateContent><w:cols w:num="3"/></w:sectPr>"#,
    );
    assert!(messages(&document).is_empty(), "{:?}", messages(&document));
    let properties = document.sections().next().unwrap().properties();
    assert_eq!(properties.page_size().unwrap().width(), Pt(612.0));
    assert_eq!(properties.title_page(), None);
    assert_eq!(properties.headers().len(), 1);
    assert_eq!(properties.columns().unwrap().count(), Some(3));
}

#[test]
fn reads_a_custom_border_as_not_valid() {
    // Word treats a file with ST_Border's custom as corrupt (MS-OI29500 §17.18.2(d)).
    let document = open(
        r#"<w:p/><w:sectPr><w:pgBorders><w:top w:val="custom" w:sz="4" r:id="rId9"/><w:left w:val="zigZagStitch" w:sz="8"/></w:pgBorders></w:sectPr>"#,
    );
    let borders = *document
        .sections()
        .next()
        .unwrap()
        .properties()
        .page_borders()
        .unwrap();
    let top = borders.top().unwrap();
    assert_eq!((top.style(), top.art()), (None, None));
    let left = borders.left().unwrap();
    assert_eq!(left.art().map(ArtBorder::name), Some("zigZagStitch"));
    assert_eq!(
        format!("{:?}", left.art().unwrap()),
        r#"ArtBorder("zigZagStitch")"#
    );
    let messages = messages(&document);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].ends_with(r#": w:top w:val="custom" is not valid; it is ignored"#));
}

#[test]
fn reports_repeated_section_properties() {
    let document = open(
        r#"<w:p/><w:sectPr><w:headerReference w:type="default" r:id="rId1"/><w:headerReference w:type="default" r:id="rId2"/><w:footerReference w:type="default" r:id="rId3"/><w:pgSz w:w="12240" w:h="15840"/><w:pgSz w:w="11906" w:h="16838"/><w:type w:val="continuous"/><w:type w:val="oddPage"/></w:sectPr>"#,
    );
    let properties = document.sections().next().unwrap().properties();
    // The first of each counts; both references of one kind are kept.
    assert_eq!(properties.page_size().unwrap().width(), Pt(612.0));
    assert_eq!(properties.start(), Some(SectionStart::Continuous));
    assert_eq!(properties.headers().len(), 2);
    assert_eq!(properties.footers().len(), 1);
    let messages = messages(&document);
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert!(messages[0].ends_with(": a second w:headerReference of type default is kept"));
    assert!(messages[1].ends_with(": a second w:pgSz is ignored"));
    assert!(messages[2].ends_with(": a second w:type is ignored"));
}
