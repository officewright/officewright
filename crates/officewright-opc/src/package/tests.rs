use super::*;
use crate::testing::{CONTENT_TYPES, DOCUMENT, PACKAGE_RELATIONSHIPS, TestEntry, hello_docx, zip};

const OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";

fn open(entries: &[TestEntry]) -> Result<Package> {
    Package::from_bytes(zip(entries), &Limits::new())
}

fn package_message(result: Result<Package>) -> String {
    match result {
        Err(Error::Package(message)) => message,
        other => panic!("expected a package error, got {other:?}"),
    }
}

fn xml_message(result: Result<Package>) -> String {
    match result {
        Err(err @ Error::Xml { .. }) => err.to_string(),
        other => panic!("expected an xml error, got {other:?}"),
    }
}

fn name(name: &str) -> PartName {
    PartName::new(name).unwrap()
}

fn types(body: &str) -> TestEntry {
    types_named("[Content_Types].xml", body)
}

fn types_named(entry: &str, body: &str) -> TestEntry {
    TestEntry::stored(
        entry,
        format!(
            "<Types xmlns='http://schemas.openxmlformats.org/package/2006/content-types'>{body}</Types>"
        )
        .as_bytes(),
    )
}

fn rels(path: &str, body: &str) -> TestEntry {
    TestEntry::stored(
        path,
        format!(
            "<Relationships xmlns='http://schemas.openxmlformats.org/package/2006/relationships'>{body}</Relationships>"
        )
        .as_bytes(),
    )
}

#[test]
fn opens_a_package_and_finds_its_main_part() {
    let package = Package::from_bytes(hello_docx(), &Limits::new()).unwrap();
    let names: Vec<_> = package.parts().map(|part| part.name().as_str()).collect();
    assert_eq!(names, ["/_rels/.rels", "/word/document.xml"]);
    let relationship = package.relationships().get("rId1").unwrap();
    assert_eq!(relationship.relationship_type(), OFFICE_DOCUMENT);
    assert_eq!(relationship.target(), "word/document.xml");
    assert_eq!(relationship.target_mode(), TargetMode::Internal);
    let main = package.part(relationship.target_part().unwrap()).unwrap();
    assert_eq!(
        main.content_type(),
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml")
    );
    assert_eq!(main.size(), DOCUMENT.len() as u64);
    assert_eq!(main.read().unwrap(), DOCUMENT.as_bytes());
    assert!(main.relationships().is_empty());
    let rels = package.part(&name("/_rels/.rels")).unwrap();
    assert_eq!(
        rels.content_type(),
        Some("application/vnd.openxmlformats-package.relationships+xml")
    );
}

#[test]
fn accepts_backslashes_folder_entries_and_any_case_of_the_content_types_name() {
    // As in POI's test-data/spreadsheet/49609.xlsx, which Office opens.
    let package = open(&[
        TestEntry::stored("[content_types].xml", CONTENT_TYPES.as_bytes()),
        TestEntry::stored("_rels/", b""),
        TestEntry::stored("_rels\\.rels", PACKAGE_RELATIONSHIPS.as_bytes()),
        TestEntry::deflated("word\\document.xml", DOCUMENT.as_bytes()),
    ])
    .unwrap();
    let main = package.part(&name("/word/document.xml")).unwrap();
    assert_eq!(main.read().unwrap(), DOCUMENT.as_bytes());
    assert_eq!(package.parts().len(), 2);
}

#[test]
fn rejects_inputs_that_are_not_packages() {
    assert_eq!(
        package_message(open(&[TestEntry::stored("mimetype", b"application/x")])),
        "[Content_Types].xml is missing: the input is not an OOXML package"
    );
    let mut ole = OLE_SIGNATURE.to_vec();
    ole.resize(512, 0);
    assert!(matches!(
        Package::from_bytes(ole, &Limits::new()),
        Err(Error::Unsupported(message)) if message.starts_with("the input is an OLE compound file")
    ));
    assert!(matches!(
        open(&[types(""), TestEntry::stored("word/document.xml/[0].piece", b"")]),
        Err(Error::Unsupported(message)) if message.starts_with("interleaved parts")
    ));
}

#[test]
fn rejects_parts_with_the_same_name_ignoring_case() {
    assert_eq!(
        package_message(open(&[
            types(""),
            TestEntry::stored("word/a.xml", b""),
            TestEntry::stored("Word/A.XML", b""),
        ])),
        "two parts are named /Word/A.XML, ignoring case"
    );
    assert_eq!(
        package_message(open(&[
            types(""),
            TestEntry::stored("a%20b.xml", b""),
            TestEntry::stored("a b.xml", b""),
        ])),
        "two parts are named /a b.xml, ignoring case"
    );
    assert_eq!(
        package_message(open(&[types(""), types_named("[content_types].xml", "")])),
        "the package has two [Content_Types].xml entries"
    );
}

#[test]
fn rejects_unsafe_part_names() {
    for (entry, why) in [
        ("word/../secret.xml", "a segment ends with '.'"),
        ("word/./a.xml", "a segment ends with '.'"),
        ("word//a.xml", "it has an empty segment"),
        ("/etc/passwd", "it has an empty segment"),
        ("word/a.", "a segment ends with '.'"),
        ("word/a%2Fb.xml", "it has a '\\' or an encoded '/'"),
        ("word/a%5cb.xml", "it has a '\\' or an encoded '/'"),
        ("word/a%G1.xml", "it has a malformed percent-encoding"),
        ("word/a%FF.xml", "it has a malformed percent-encoding"),
        ("word/a\u{1}.xml", "it has a control character"),
    ] {
        assert_eq!(
            package_message(open(&[types(""), TestEntry::stored(entry, b"")])),
            format!(
                "invalid part name {:?}: {why}",
                format!("/{}", entry.replace('\\', "/"))
            ),
            "{entry}"
        );
    }
}

#[test]
fn compares_part_names_as_the_conventions_require() {
    let a = name("/Word/Media/Image%201.PNG");
    assert_eq!(a, name("/word/media/image 1.png"));
    assert_ne!(a, name("/word/media/image1.png"));
    assert_eq!(a.as_str(), "/Word/Media/Image%201.PNG");
    assert_eq!(a.to_string(), "/Word/Media/Image%201.PNG");
    assert_eq!(a.extension(), Some("PNG"));
    assert_eq!(a.folder(), "/Word/Media");
    assert_eq!(name("/a").extension(), None);
    assert_eq!(name("/a.xml").folder(), "");
    // Office writes names like these; they are kept as they are.
    assert!(PartName::new("/[trash]/0000.dat").is_ok());
    assert!(PartName::new("/word/Ж.xml").is_ok());
    assert!(PartName::new("word/a.xml").is_err());
    assert!(PartName::new("/word/a.xml/").is_err());
}

#[test]
fn finds_the_source_of_relationships_parts() {
    assert_eq!(name("/_rels/.rels").relationships_source(), Some(None));
    assert_eq!(
        name("/word/_rels/document.xml.rels").relationships_source(),
        Some(Some(name("/word/document.xml")))
    );
    assert_eq!(
        name("/_rels/a.xml.RELS").relationships_source(),
        Some(Some(name("/a.xml")))
    );
    assert_eq!(name("/word/_rels/.rels").relationships_source(), None);
    assert_eq!(name("/word/document.xml").relationships_source(), None);
    assert_eq!(name("/word/rels/a.xml.rels").relationships_source(), None);
}

#[test]
fn resolves_relationship_targets() {
    let resolve = |folder: &str, target: &str| {
        part_name::resolve_target(folder, target).map(|name| name.as_str().to_owned())
    };
    assert_eq!(
        resolve("/word", "media/a.png").as_deref(),
        Some("/word/media/a.png")
    );
    assert_eq!(
        resolve("/word", "../customXml/item1.xml").as_deref(),
        Some("/customXml/item1.xml")
    );
    assert_eq!(
        resolve("/word", "./styles.xml").as_deref(),
        Some("/word/styles.xml")
    );
    assert_eq!(
        resolve("/word", "/docProps/core.xml").as_deref(),
        Some("/docProps/core.xml")
    );
    assert_eq!(
        resolve("", "word/document.xml").as_deref(),
        Some("/word/document.xml")
    );
    for target in [
        "",
        "../../etc/passwd",
        "../../../a.xml",
        "http://example.com/a.xml",
        "file:///C:/a.xml",
        "a.xml#part",
        "a.xml?x",
        "a//b.xml",
        "media/",
    ] {
        assert_eq!(resolve("/word", target), None, "{target}");
    }
}

#[test]
fn reads_relationships_of_parts() {
    let package = open(&[
        types(""),
        TestEntry::stored("word/document.xml", b"<a/>"),
        rels(
            "word/_rels/document.xml.rels",
            "<Relationship Id='rId1' Type='urn:styles' Target='styles.xml'/>\
             <Relationship Id='rId2' Type='urn:link' Target='https://example.com/' TargetMode='External'/>\
             <Relationship Id='rId3' Type='urn:styles' Target='../../outside.xml'/>",
        ),
    ])
    .unwrap();
    assert!(package.relationships().is_empty());
    let document = package.part(&name("/word/document.xml")).unwrap();
    assert_eq!(document.content_type(), None);
    let relationships = document.relationships();
    assert_eq!(relationships.len(), 3);
    let styles: Vec<_> = relationships
        .of_type("urn:styles")
        .map(|relationship| relationship.target_part().map(PartName::as_str))
        .collect();
    assert_eq!(styles, [Some("/word/styles.xml"), None]);
    let link = relationships.get("rId2").unwrap();
    assert_eq!(link.target_mode(), TargetMode::External);
    assert_eq!(link.target(), "https://example.com/");
    assert_eq!(link.target_part(), None);
    let ids: Vec<_> = relationships.iter().map(Relationship::id).collect();
    assert_eq!(ids, ["rId1", "rId2", "rId3"]);
}

#[test]
fn rejects_malformed_relationships() {
    for (body, message) in [
        (
            "<Relationship Id='a' Type='t' Target='x'/><Relationship Id='a' Type='t' Target='y'/>",
            "a second relationship with Id a",
        ),
        (
            "<Relationship Id='a' Type='t' Target='x' TargetMode='Elsewhere'/>",
            "invalid TargetMode \"Elsewhere\"",
        ),
        (
            "<Relationship Id='a' Target='x'/>",
            "<Relationship> lacks the Type attribute",
        ),
        (
            "<Relationship Id='a' Type='t' Target='x' Extra='1'/>",
            "unexpected attribute Extra on <Relationship>",
        ),
        ("<Other/>", "unexpected element <Other>"),
        (
            "<Relationship Id='a' Type='t' Target='x'>text</Relationship>",
            "<Relationship> must be empty",
        ),
        ("text", "unexpected text"),
    ] {
        let error = xml_message(open(&[types(""), rels("_rels/.rels", body)]));
        assert!(
            error.starts_with("/_rels/.rels, line 1, column ") && error.ends_with(message),
            "{error}"
        );
    }
    assert_eq!(
        xml_message(open(&[
            types(""),
            TestEntry::stored("_rels/.rels", b"<Types/>")
        ])),
        "/_rels/.rels, line 1, column 1: the root element is <Types>, not Relationships in \
         http://schemas.openxmlformats.org/package/2006/relationships"
    );
}

#[test]
fn reads_content_types_by_override_and_by_extension() {
    let package = open(&[
        types(
            "<Default Extension='XML' ContentType='application/xml'/>\
             <Default Extension='png' ContentType='image/png'/>\
             <Override PartName='/word/document.xml' ContentType='application/x-main+xml; charset=utf-8'/>\
             <Override PartName='/word/missing.xml' ContentType='application/x-missing'/>",
        ),
        TestEntry::stored("word/document.xml", b""),
        TestEntry::stored("word/styles.xml", b""),
        TestEntry::stored("word/media/a.PNG", b""),
        TestEntry::stored("word/media/b.emf", b""),
    ])
    .unwrap();
    let content_type = |part: &str| package.part(&name(part)).unwrap().content_type();
    assert_eq!(
        content_type("/word/document.xml"),
        Some("application/x-main+xml; charset=utf-8")
    );
    assert_eq!(content_type("/word/styles.xml"), Some("application/xml"));
    assert_eq!(content_type("/word/media/a.PNG"), Some("image/png"));
    assert_eq!(content_type("/word/media/b.emf"), None);
}

#[test]
fn rejects_malformed_content_types() {
    for (body, message) in [
        (
            "<Default Extension='xml' ContentType='application/xml'/><Default Extension='XML' ContentType='text/xml'/>",
            "a second <Default> for the same extension",
        ),
        (
            "<Override PartName='/a.xml' ContentType='a/b'/><Override PartName='/A.xml' ContentType='a/b'/>",
            "a second <Override> for the same part",
        ),
        (
            "<Default Extension='.xml' ContentType='application/xml'/>",
            "invalid extension \".xml\"",
        ),
        (
            "<Default Extension='xml' ContentType='application'/>",
            "invalid content type \"application\"",
        ),
        (
            "<Default Extension='xml'/>",
            "<Default> lacks the ContentType attribute",
        ),
        (
            "<Override PartName='a.xml' ContentType='a/b'/>",
            "invalid part name \"a.xml\": it does not start with '/'",
        ),
        (
            "<Default Extension='xml' ContentType='a/b' Other='1'/>",
            "unexpected attributes on <Default>",
        ),
        ("<Unknown/>", "unexpected element <Unknown>"),
    ] {
        let error = xml_message(open(&[types(body)]));
        assert!(
            error.starts_with("[Content_Types].xml, line 1, column ") && error.ends_with(message),
            "{error}"
        );
    }
}

#[test]
fn counts_every_part_read_against_the_total_size_limit() {
    let bytes = hello_docx();
    let opening = (CONTENT_TYPES.len() + PACKAGE_RELATIONSHIPS.len()) as u64;
    let limits = Limits::new().with_max_total_size(opening + DOCUMENT.len() as u64);
    let package = Package::from_bytes(bytes.as_slice(), &limits).unwrap();
    let main = package.part(&name("/word/document.xml")).unwrap();
    assert!(main.read().is_ok());
    assert!(matches!(
        main.read(),
        Err(Error::LimitExceeded {
            limit: Limit::TotalSize,
            ..
        })
    ));
    let limits = Limits::new().with_max_total_size(opening - 1);
    assert!(matches!(
        Package::from_bytes(bytes, &limits),
        Err(Error::LimitExceeded {
            limit: Limit::TotalSize,
            ..
        })
    ));
}

#[test]
fn recognizes_pieces_of_interleaved_parts() {
    assert!(is_piece("word/document.xml/[0].piece"));
    assert!(is_piece("a/[12].LAST.piece"));
    assert!(!is_piece("[trash]/0000.dat"));
    assert!(!is_piece("a/[].piece"));
    assert!(!is_piece("a/[x].piece"));
}

#[test]
fn keeps_relationships_without_an_id_or_a_target() {
    let package = open(&[
        types(""),
        rels(
            "_rels/.rels",
            "<Relationship Type='urn:a' Target='a.xml'/><Relationship Id='' Type='urn:b' Target='b.xml'/>\
             <Relationship Type='urn:c' TargetMode='External'/>",
        ),
    ])
    .unwrap();
    let relationships = package.relationships();
    assert_eq!(relationships.len(), 3);
    let c = relationships.of_type("urn:c").next().unwrap();
    assert_eq!((c.target(), c.target_part()), ("", None));
    assert!(relationships.get("").is_none());
    let a = relationships.of_type("urn:a").next().unwrap();
    assert_eq!(a.id(), "");
    assert_eq!(a.target_part().map(PartName::as_str), Some("/a.xml"));
}
