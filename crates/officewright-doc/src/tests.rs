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
fn keeps_tables_and_inline_containers_as_written() {
    let document = open(
        r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:hyperlink w:anchor="a"><w:r><w:t>link</w:t></w:r></w:hyperlink><w:r><w:t>text</w:t></w:r></w:p>"#,
    );
    assert_eq!(outline(&document), ["section 0", "other tbl", "p \"text\""]);
    let Some(Block::Paragraph(paragraph)) = document.sections().next().unwrap().blocks().nth(1)
    else {
        panic!("no paragraph");
    };
    let inlines: Vec<String> = paragraph
        .inlines()
        .map(|inline| match inline {
            Inline::Run(run) => format!("run {:?}", run.text()),
            Inline::Other(other) => format!("other {}", other.local_name()),
        })
        .collect();
    assert_eq!(inlines, ["other hyperlink", "run \"text\""]);
}

#[test]
fn an_id_finds_its_node_only_in_its_own_document() {
    let first = open("<w:p/>");
    let second = open("<w:p/>");
    let Some(Block::Paragraph(paragraph)) = first.sections().next().unwrap().blocks().next() else {
        panic!("no paragraph");
    };
    assert!(first.paragraph(paragraph.id()).is_some());
    assert!(second.paragraph(paragraph.id()).is_none());
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
