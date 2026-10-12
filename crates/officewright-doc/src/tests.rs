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
