use super::*;
use crate::{Error, Limit, Limits, Result};

/// The events of a document in a compact form: `<{ns}local {ns}attr=value>`, `</{ns}local>`,
/// `'text'`, `<!--comment-->` and `<?target data?>`.
fn trace_bytes(bytes: &[u8], limits: &Limits) -> Result<Vec<String>> {
    let input = Input::decode(bytes)?;
    let mut reader = input.reader(limits);
    let mut events = Vec::new();
    while let Some(event) = reader.read_event()? {
        let name = |name: Name<'_>| match name.namespace() {
            Some(ns) => format!("{{{}}}{}", reader.namespace_uri(ns), name.local()),
            None => name.local().to_owned(),
        };
        events.push(match event {
            Event::Start(tag) => {
                let mut out = format!("<{}", name(tag.name()));
                for attribute in tag.attributes() {
                    out.push_str(&format!(
                        " {}={}",
                        name(attribute.name()),
                        attribute.value()
                    ));
                }
                out + ">"
            }
            Event::End(tag) => format!("</{}>", name(tag.name())),
            Event::Text(text) => format!("'{text}'"),
            Event::Comment(comment) => format!("<!--{comment}-->"),
            Event::ProcessingInstruction(pi) => format!("<?{} {}?>", pi.target(), pi.data()),
        });
    }
    Ok(events)
}

fn trace(xml: &str) -> Vec<String> {
    trace_bytes(xml.as_bytes(), &Limits::new()).unwrap()
}

/// The message and position of the error a document produces.
fn error(xml: &str) -> String {
    match trace_bytes(xml.as_bytes(), &Limits::new()) {
        Err(Error::Xml {
            line,
            column,
            message,
            ..
        }) => format!("{line}:{column}: {message}"),
        other => panic!("expected an xml error, got {other:?}"),
    }
}

#[test]
fn reads_elements_attributes_text_and_namespaces() {
    assert_eq!(
        trace(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="urn:w" xmlns="urn:default" a="1" w:b="2"><w:t>Hi</w:t><plain/></w:document>"#
        ),
        [
            "<{urn:w}document a=1 {urn:w}b=2>",
            "<{urn:w}t>",
            "'Hi'",
            "</{urn:w}t>",
            "<{urn:default}plain>",
            "</{urn:default}plain>",
            "</{urn:w}document>",
        ]
    );
}

#[test]
fn reads_comments_processing_instructions_and_cdata() {
    assert_eq!(
        trace("<!-- before --><?pi data?><a><![CDATA[<&>]]><?empty?></a><!--after-->"),
        [
            "<!-- before -->",
            "<?pi data?>",
            "<a>",
            "'<&>'",
            "<?empty ?>",
            "</a>",
            "<!--after-->",
        ]
    );
}

#[test]
fn replaces_references() {
    assert_eq!(
        trace(r#"<a v="&lt;&#65;&#x42;&quot;">&amp;&apos;&gt;&#x10FFFF;</a>"#),
        ["<a v=<AB\">", "'&'>\u{10FFFF}'", "</a>"]
    );
}

#[test]
fn normalizes_line_ends_and_attribute_whitespace() {
    // A tab or line feed in an attribute value becomes a space; a character reference
    // to one does not (XML 1.0, section 3.3.3).
    assert_eq!(
        trace("<a v=\"1\t2\r\n3\r4&#10;5\">x\r\ny\rz</a>"),
        ["<a v=1 2 3 4\n5>", "'x\ny\nz'", "</a>"]
    );
}

#[test]
fn decodes_utf8_and_utf16_with_and_without_byte_order_marks() {
    let expected = ["<a>", "'Ж'", "</a>"];
    let xml = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><a>Ж</a>";
    let utf16le: Vec<u8> = xml.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let utf16be: Vec<u8> = xml.encode_utf16().flat_map(u16::to_be_bytes).collect();
    for bytes in [
        [b"\xEF\xBB\xBF".as_slice(), "<a>Ж</a>".as_bytes()].concat(),
        [b"\xFF\xFE".as_slice(), &utf16le].concat(),
        [b"\xFE\xFF".as_slice(), &utf16be].concat(),
        utf16le,
        utf16be,
    ] {
        assert_eq!(trace_bytes(&bytes, &Limits::new()).unwrap(), expected);
    }
}

#[test]
fn rejects_bytes_that_are_not_utf8_or_utf16() {
    let message = |bytes: &[u8]| match Input::decode(bytes) {
        Err(Error::Xml {
            line,
            column,
            message,
            ..
        }) => format!("{line}:{column}: {message}"),
        other => panic!("expected an xml error, got {other:?}"),
    };
    assert_eq!(message(b"<a>\n\xC3</a>"), "2:1: invalid UTF-8");
    assert_eq!(message(b"\xFF\xFE<\x00\x00\xD8"), "1:2: invalid UTF-16");
    assert_eq!(
        message(b"\xFF\xFE<\x00a"),
        "1:2: UTF-16 text has an odd number of bytes"
    );
    assert_eq!(
        message(b"<a>\x01</a>"),
        "1:4: character U+0001 is not allowed in XML"
    );
    assert_eq!(
        message("<a>\u{FFFE}</a>".as_bytes()),
        "1:4: character U+FFFE is not allowed in XML"
    );
}

#[test]
fn checks_the_xml_declaration() {
    assert_eq!(trace("<?xml version='1.0'?><a/>"), ["<a>", "</a>"]);
    assert_eq!(trace("<?xml version=\"1.0\" ?><a/>"), ["<a>", "</a>"]);
    assert_eq!(
        error("<?xml version=\"2.0\"?><a/>"),
        "1:1: unsupported xml version 2.0"
    );
    assert_eq!(
        error("<?xml encoding=\"UTF-8\"?><a/>"),
        "1:7: expected version in the xml declaration"
    );
    assert_eq!(
        error("<?xml version=\"1.0\" standalone=\"maybe\"?><a/>"),
        "1:1: standalone must be yes or no, not maybe"
    );
    assert_eq!(
        error("<?xml version=\"1.0\"encoding=\"UTF-8\"?><a/>"),
        "1:20: malformed xml declaration"
    );
    assert_eq!(
        error(" <?xml version=\"1.0\"?><a/>"),
        "1:2: an xml declaration is allowed only at the start of the document"
    );
    assert!(matches!(
        Input::decode(b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>")
            .unwrap()
            .reader(&Limits::new())
            .read_event(),
        Err(Error::Unsupported(message)) if message == "xml encoding ISO-8859-1; OOXML parts are UTF-8 or UTF-16"
    ));
}

#[test]
fn rejects_document_type_declarations() {
    // External entities and entity expansion need a DTD, so none is read at all.
    assert_eq!(
        error(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE a [<!ENTITY e SYSTEM \"http://example.com/\">]><a>&e;</a>"
        ),
        "2:1: document type declarations are not allowed in OOXML parts"
    );
    assert_eq!(
        error("<a><!DOCTYPE a></a>"),
        "1:4: unexpected markup declaration"
    );
}

#[test]
fn rejects_malformed_references() {
    assert_eq!(error("<a>&e;</a>"), "1:4: undeclared entity &e;");
    assert_eq!(error("<a>a & b</a>"), "1:6: '&' does not start a reference");
    assert_eq!(
        error("<a v='&amp'/>"),
        "1:7: '&' does not start a reference"
    );
    assert_eq!(
        error("<a>&#0;</a>"),
        "1:4: &#0; is not a character XML allows"
    );
    assert_eq!(
        error("<a>&#xD800;</a>"),
        "1:4: &#xD800; is not a character XML allows"
    );
    assert_eq!(
        error("<a>&#99999999999;</a>"),
        "1:4: &#99999999999; is not a character XML allows"
    );
    assert_eq!(
        error("<a>&#;</a>"),
        "1:4: &#; is not a character XML allows"
    );
    assert_eq!(
        error(&format!("<a>&{};</a>", "x".repeat(70))),
        format!("1:4: undeclared entity &{}…;", "x".repeat(64))
    );
}

#[test]
fn rejects_documents_that_are_not_well_formed() {
    assert_eq!(error(""), "1:1: no root element");
    assert_eq!(error("text<a/>"), "1:1: content before the root element");
    assert_eq!(error("<a/><b/>"), "1:5: content after the root element");
    assert_eq!(error("<a/>text"), "1:5: content after the root element");
    assert_eq!(
        error("<a><b></a>"),
        "1:7: end tag </a> does not match start tag <b>"
    );
    assert_eq!(error("<a><b>"), "1:7: unexpected end of input inside <b>");
    assert_eq!(error("<a>]]></a>"), "1:4: ']]>' in text");
    assert_eq!(error("<a><!-- a -- b --></a>"), "1:11: '--' in a comment");
    assert_eq!(error("<a><!-- a ---></a>"), "1:11: '--' in a comment");
    assert_eq!(error("<a><!-- a </a>"), "1:4: unterminated comment");
    assert_eq!(
        error("<a><![CDATA[x</a>"),
        "1:4: unterminated CDATA section"
    );
    assert_eq!(
        error("<a><?pi x</a>"),
        "1:4: unterminated processing instruction"
    );
    assert_eq!(error("<a v='<'/>"), "1:7: '<' in an attribute value");
    assert_eq!(
        error("<a v='1'w='2'/>"),
        "1:9: expected whitespace, '>' or '/>' in <a>"
    );
    assert_eq!(error("<a v=1/>"), "1:6: expected a quoted attribute value");
    assert_eq!(error("<a v/>"), "1:5: expected '=' after an attribute name");
    assert_eq!(error("<a v='1/>"), "1:6: unterminated attribute value");
    assert_eq!(
        error("<a v='1' v='2'/>"),
        "1:1: duplicate attribute v in <a>"
    );
    assert_eq!(error("<1a/>"), "1:2: expected a name");
    assert_eq!(
        error("<a:b:c/>"),
        "1:5: expected whitespace, '>' or '/>' in <a:b>"
    );
    assert_eq!(error("<a></a >x"), "1:9: content after the root element");
    assert_eq!(
        error("<a>\n  <b>\n</a>"),
        "3:1: end tag </a> does not match start tag <b>"
    );
}

#[test]
fn checks_namespace_constraints() {
    assert_eq!(
        error("<p:a/>"),
        "1:1: undeclared namespace prefix p in <p:a>"
    );
    assert_eq!(
        error("<a p:v='1'/>"),
        "1:1: undeclared namespace prefix p in <a>"
    );
    assert_eq!(
        error("<a xmlns:p=''/>"),
        "1:1: the namespace prefix p cannot be undeclared"
    );
    assert_eq!(
        error("<a xmlns:xmlns='urn:x'/>"),
        "1:1: the prefix xmlns cannot be declared"
    );
    assert_eq!(
        error("<a xmlns:xml='urn:x'/>"),
        "1:1: the prefix xml can only be bound to http://www.w3.org/XML/1998/namespace"
    );
    assert_eq!(
        error("<a xmlns:p='http://www.w3.org/XML/1998/namespace'/>"),
        "1:1: http://www.w3.org/XML/1998/namespace cannot be bound to a prefix"
    );
    assert_eq!(
        error("<a xmlns='http://www.w3.org/2000/xmlns/'/>"),
        "1:1: http://www.w3.org/2000/xmlns/ cannot be bound to a prefix"
    );
    // Two prefixes for one namespace still name the same attribute.
    assert_eq!(
        error("<a xmlns:p='urn:x' xmlns:q='urn:x' p:v='1' q:v='2'/>"),
        "1:1: duplicate attribute q:v in <a>"
    );
}

#[test]
fn scopes_namespace_declarations_to_their_element() {
    assert_eq!(
        trace(
            "<a xmlns='urn:1' xmlns:p='urn:p'><b xmlns='' xmlns:p='urn:q' p:v='1'/>\
             <p:c v='2' xml:space='preserve'/></a>"
        ),
        [
            "<{urn:1}a>",
            "<b {urn:q}v=1>",
            "</b>",
            "<{urn:p}c v=2 {http://www.w3.org/XML/1998/namespace}space=preserve>",
            "</{urn:p}c>",
            "</{urn:1}a>",
        ]
    );
    assert_eq!(
        error("<a><b xmlns:p='urn:p'/><p:c/></a>"),
        "1:24: undeclared namespace prefix p in <p:c>"
    );
}

#[test]
fn reports_namespace_declarations_and_resolves_prefixes() {
    let input = Input::decode(
        b"<a xmlns='urn:d' xmlns:p='urn:p' xmlns:xml='http://www.w3.org/XML/1998/namespace'/>",
    )
    .unwrap();
    let mut reader = input.reader(&Limits::new());
    let d = reader.intern("urn:d");
    let p = reader.intern("urn:p");
    let Some(Event::Start(tag)) = reader.read_event().unwrap() else {
        panic!("expected a start tag");
    };
    let declarations: Vec<_> = tag
        .namespace_declarations()
        .iter()
        .map(|declaration| (declaration.prefix(), declaration.namespace()))
        .collect();
    assert_eq!(
        declarations,
        [
            (None, Some(d)),
            (Some("p"), Some(p)),
            (Some("xml"), Some(NsId::XML))
        ]
    );
    assert!(tag.is_self_closing());
    assert_eq!(reader.resolve_prefix("p"), Some(p));
    assert_eq!(reader.resolve_prefix(""), Some(d));
    assert_eq!(reader.resolve_prefix("xml"), Some(NsId::XML));
    assert_eq!(reader.resolve_prefix("q"), None);
    assert_eq!(reader.namespace_uri(NsId::XML), XML_NAMESPACE);
    assert!(matches!(reader.read_event(), Ok(Some(Event::End(_)))));
    assert_eq!(reader.resolve_prefix("p"), None);
    assert_eq!(reader.read_event().unwrap(), None);
}

#[test]
fn finds_attributes_by_namespace_and_name() {
    let input = Input::decode(b"<a xmlns:w='urn:w' w:val='1' val='2'/>").unwrap();
    let mut reader = input.reader(&Limits::new());
    let w = reader.intern("urn:w");
    let Some(Event::Start(tag)) = reader.read_event().unwrap() else {
        panic!("expected a start tag");
    };
    assert_eq!(tag.attribute(Some(w), "val"), Some("1"));
    assert_eq!(tag.attribute(None, "val"), Some("2"));
    assert_eq!(tag.attribute(None, "w"), None);
    assert!(!tag.name().is(w, "a"));
    assert_eq!(tag.name().namespace(), None);
}

#[test]
fn skips_elements() {
    let input = Input::decode(b"<a><b><c/>text<d></d></b><e/></a>").unwrap();
    let mut reader = input.reader(&Limits::new());
    assert!(matches!(reader.read_event(), Ok(Some(Event::Start(_)))));
    assert!(matches!(reader.read_event(), Ok(Some(Event::Start(_)))));
    reader.skip_element().unwrap();
    assert_eq!(reader.depth(), 1);
    let Ok(Some(Event::Start(tag))) = reader.read_event() else {
        panic!("expected <e>");
    };
    assert_eq!(tag.name().local(), "e");
    reader.skip_element().unwrap();
    assert!(matches!(reader.read_event(), Ok(Some(Event::End(_)))));
    assert_eq!(reader.read_event().unwrap(), None);
}

#[test]
fn enforces_the_depth_and_attribute_limits() {
    let limits = Limits::new()
        .with_max_xml_depth(2)
        .with_max_xml_attributes(2);
    assert!(trace_bytes(b"<a><b/></a>", &limits).is_ok());
    assert!(matches!(
        trace_bytes(b"<a><b>\n<c/></b></a>", &limits),
        Err(Error::LimitExceeded { limit: Limit::XmlDepth, maximum: 2, context }) if context == "line 2, column 1"
    ));
    assert!(trace_bytes(b"<a x='1' y='2'/>", &limits).is_ok());
    assert!(matches!(
        trace_bytes(b"<a x='1' y='2' xmlns='urn:z'/>", &limits),
        Err(Error::LimitExceeded {
            limit: Limit::XmlAttributes,
            maximum: 2,
            ..
        })
    ));
}

#[test]
fn finds_duplicates_among_many_attributes() {
    let attributes: String = (0..40).map(|i| format!(" a{i}='{i}'")).collect();
    assert!(trace_bytes(format!("<a{attributes}/>").as_bytes(), &Limits::new()).is_ok());
    assert_eq!(
        error(&format!("<a{attributes} a7='x'/>")),
        "1:1: duplicate attribute a7 in <a>"
    );
}

#[test]
fn stops_after_an_error() {
    let input = Input::decode(b"<a><b></c></a>").unwrap();
    let mut reader = input.reader(&Limits::new());
    while let Ok(Some(_)) = reader.read_event() {}
    assert_eq!(reader.read_event().unwrap(), None);
}

#[test]
fn reports_offsets_of_events() {
    let input = Input::decode(b"<a>\n<b/></a>").unwrap();
    let mut reader = input.reader(&Limits::new());
    let mut offsets = Vec::new();
    while let Some(event) = reader.read_event().unwrap() {
        offsets.push(reader.offset());
        if let Event::Start(tag) = &event {
            assert_eq!(tag.offset(), reader.offset());
        }
    }
    assert_eq!(offsets, [0, 3, 4, 4, 8]);
    assert_eq!(reader.error(5, "bad").to_string(), "line 2, column 2: bad");
}

#[test]
fn reads_character_references_with_any_number_of_leading_zeros() {
    // Production 66 puts no bound on the digits.
    assert_eq!(
        trace(&format!(
            "<a>&#x{}41;&#{}66;</a>",
            "0".repeat(40),
            "0".repeat(40)
        )),
        ["<a>", "'AB'", "</a>"]
    );
}

#[test]
fn rejects_duplicate_namespace_declarations() {
    assert_eq!(
        error("<a xmlns='urn:x' xmlns='urn:y'/>"),
        "1:1: duplicate attribute xmlns in <a>"
    );
    assert_eq!(
        error("<a xmlns:p='urn:x' xmlns:p='urn:y'><p:b/></a>"),
        "1:1: duplicate attribute xmlns:p in <a>"
    );
}

#[test]
fn reads_the_xml_prefix_and_prefixed_attributes_named_xmlns() {
    // The xml prefix is bound by definition (Namespaces in XML 1.0, section 3), and an
    // attribute p:xmlns is an attribute, not a namespace declaration, which is xmlns or
    // begins with xmlns: (same section).
    assert_eq!(
        trace("<xml:a/>"),
        [
            "<{http://www.w3.org/XML/1998/namespace}a>",
            "</{http://www.w3.org/XML/1998/namespace}a>"
        ]
    );
    assert_eq!(
        trace(r#"<a xmlns:p="urn:x" p:xmlns="1"/>"#),
        ["<a {urn:x}xmlns=1>", "</a>"]
    );
    assert_eq!(
        error(r#"<a xmlns:p="urn:x" xmlns:q="urn:x" p:xmlns="1" q:xmlns="2"/>"#),
        "1:1: duplicate attribute q:xmlns in <a>"
    );
    assert_eq!(
        error(r#"<root xmlns:xmlns="urn:a"/>"#),
        "1:1: the prefix xmlns cannot be declared"
    );
}

#[test]
fn checks_the_grammar_of_encoding_names() {
    assert_eq!(
        error(r#"<?xml version="1.0" encoding="8bit"?><a/>"#),
        "1:1: invalid encoding name 8bit"
    );
}

#[test]
fn keeps_error_messages_short_whatever_the_document_holds() {
    let name = "a".repeat(8 << 20);
    let message = error(&format!("<{name}"));
    assert_eq!(
        message,
        format!(
            "1:{}: unexpected end of input in <{}…>",
            (8 << 20) + 2,
            "a".repeat(64)
        )
    );
}

#[test]
fn reads_parts_labelled_us_ascii_as_utf_8() {
    // ASCII is a subset of UTF-8; a part whose label says ASCII but whose text is UTF-8
    // is read as UTF-8, as its producer meant.
    for label in ["US-ASCII", "us-ascii", "ASCII"] {
        assert_eq!(
            trace(&format!(
                "<?xml version=\"1.0\" encoding=\"{label}\"?><a>\u{e9}t\u{e9}</a>"
            )),
            ["<a>", "'\u{e9}t\u{e9}'", "</a>"],
            "{label}"
        );
    }
    // Bytes that are not UTF-8 stay malformed, whatever the label.
    assert!(matches!(
        trace_bytes(
            b"<?xml version=\"1.0\" encoding=\"US-ASCII\"?><a>\xE9</a>",
            &Limits::new()
        ),
        Err(Error::Xml { .. })
    ));
    assert!(matches!(
        trace_bytes(
            b"<?xml version=\"1.0\" encoding=\"ASCII-8\"?><a/>",
            &Limits::new()
        ),
        Err(Error::Unsupported(_))
    ));
}
