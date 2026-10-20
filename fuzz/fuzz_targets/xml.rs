//! Reads the input as an XML part, following Markup Compatibility, and checks it against
//! roxmltree: both must accept and reject the same documents and read the same elements,
//! attributes, text, comments and processing instructions from them.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_opc::mce::Processor;
use officewright_opc::xml::{Event, Input, NsId};
use officewright_opc::{Error, Limits};
use roxmltree::{Document, NodeType, ParsingOptions};

/// What both parsers read, in document order. Adjacent text is merged, because roxmltree
/// merges text and CDATA sections into one node.
#[derive(Debug, PartialEq)]
enum Node {
    Start {
        namespace: String,
        local: String,
        /// Namespace, local name and value, sorted.
        attributes: Vec<(String, String, String)>,
    },
    End,
    Text(String),
    Comment(String),
    Pi(String, Option<String>),
}

fuzz_target!(|data: &[u8]| {
    check_decoding(data);
    let limits = Limits::new();
    let input = match Input::decode(data) {
        Ok(input) => input,
        Err(_) => {
            // Text officewright cannot decode must not parse as XML either.
            let comparable = std::str::from_utf8(data)
                .ok()
                .filter(|text| !text.starts_with('\u{FEFF}'));
            if let Some(text) = comparable {
                assert!(
                    Document::parse_with_options(text, options()).is_err(),
                    "roxmltree accepts text officewright cannot decode"
                );
            }
            return;
        }
    };
    // roxmltree drops a U+FEFF at the start of its input. After the byte order mark,
    // which Input::decode removes, it is a character, and XML allows none before the
    // root element.
    if input.as_str().starts_with('\u{FEFF}') {
        return;
    }
    let ours = read(&input, &limits);
    // Limits and the encodings officewright does not read are no reason to compare.
    if matches!(
        ours,
        Err(Error::LimitExceeded { .. } | Error::Unsupported(_))
    ) {
        return;
    }
    let theirs = Document::parse_with_options(input.as_str(), options());
    match (ours, theirs) {
        (Ok(ours), Ok(theirs)) => {
            let mut nodes = Vec::new();
            walk(theirs.root(), &mut nodes);
            // roxmltree reads an XML declaration whose first whitespace is a tab or a line
            // feed (production 24 allows any S) as a processing instruction named xml,
            // which officewright never returns.
            if matches!(nodes.first(), Some(Node::Pi(target, _)) if target == "xml") {
                nodes.remove(0);
            }
            // roxmltree takes an attribute p:xmlns for a declaration of the default
            // namespace; Namespaces in XML 1.0, section 3, makes only xmlns and xmlns:p
            // declarations, so officewright reads it as an attribute.
            let prefixed_xmlns = ours.iter().any(|node| match node {
                Node::Start { attributes, .. } => attributes
                    .iter()
                    .any(|(namespace, local, _)| !namespace.is_empty() && local == "xmlns"),
                _ => false,
            });
            if !prefixed_xmlns {
                assert_eq!(ours, nodes);
            }
        }
        // Namespaces in XML 1.0, section 3: the prefix xml is bound by definition, so
        // `<xml:a/>` is namespace-well-formed; roxmltree reports an unknown prefix.
        (Ok(_), Err(roxmltree::Error::UnknownNamespace(prefix, _))) if prefix == "xml" => {}
        (Ok(ours), Err(err)) => {
            panic!("roxmltree rejects ({err}) what officewright reads as {ours:?}")
        }
        (Err(err), Ok(_)) if rejected_on_purpose(input.as_str(), &err) => {}
        (Err(err), Ok(_)) => panic!("officewright rejects ({err}) what roxmltree accepts"),
        (Err(_), Err(_)) => {}
    }
});

/// Whether officewright rejects the text where roxmltree accepts it, and rightly so: a
/// rule of XML 1.0 or of Namespaces in XML 1.0 that roxmltree does not check. (Both
/// reject every document type declaration.)
fn rejected_on_purpose(text: &str, err: &Error) -> bool {
    let Error::Xml {
        line,
        column,
        message,
        ..
    } = err
    else {
        return false;
    };
    // Production 16: a processing instruction target is followed by whitespace or `?>`;
    // roxmltree reads `<?version="1.0"?>`.
    if message == "expected whitespace after a processing instruction target" {
        return true;
    }
    // Production 17: no processing instruction target but the XML declaration's is `xml`
    // in any case; roxmltree reads `<?xMl version="1.0"?>`.
    if message == "an xml declaration is allowed only at the start of the document" {
        return true;
    }
    // Namespaces in XML 1.0, section 7: names are QNames and processing instruction
    // targets have no colon; roxmltree reads `<:a>`, `<a :b="1">` and `<?:a?>`.
    if message == "expected a name" && char_at(text, *line, *column) == Some(':') {
        return true;
    }
    // The Unique Att Spec constraint covers namespace declarations too, and Namespaces
    // in XML 1.0, section 6.3, attributes with the same expanded name; roxmltree reads
    // two xmlns, two xmlns:p, and p:xmlns next to q:xmlns for one namespace.
    let duplicate = message
        .strip_prefix("duplicate attribute ")
        .and_then(|rest| rest.split_once(" in <"))
        .map(|(name, _)| name);
    if duplicate.is_some_and(|name| {
        name == "xmlns" || name.starts_with("xmlns:") || name.ends_with(":xmlns")
    }) {
        return true;
    }
    // Namespaces in XML 1.0, section 5, Prefix Declared: the prefix of an attribute p:xmlns
    // must be declared like any other; roxmltree takes such an attribute for a declaration
    // of the default namespace and never looks its prefix up. The error points at the
    // start tag that holds it.
    let undeclared = message
        .strip_prefix("undeclared namespace prefix ")
        .and_then(|rest| rest.split_once(" in <"))
        .map(|(prefix, _)| prefix);
    if undeclared.is_some_and(|prefix| {
        attribute_names(text, *line, *column)
            .into_iter()
            .any(|name| {
                name.strip_suffix(":xmlns")
                    .is_some_and(|own| short(own) == prefix)
            })
    }) {
        return true;
    }
    // Namespaces in XML 1.0, section 3: the prefix xmlns must not be declared;
    // roxmltree reads `xmlns:xmlns="urn:a"`.
    if message == "the prefix xmlns cannot be declared" {
        return true;
    }
    // The Legal Character constraint: a character reference names a Char; roxmltree reads
    // `&#1114112;`, past the last Unicode character.
    if message.ends_with("is not a character XML allows") {
        return true;
    }
    // Namespaces in XML 1.0, section 3, No Prefix Undeclaring; roxmltree reads
    // `xmlns:r=""`, which only XML 1.1 allows.
    if message.ends_with("cannot be undeclared") {
        return true;
    }
    // Productions 23 to 32: roxmltree does not check the grammar of the XML
    // declaration, such as `version="0.0"`.
    if !text.starts_with("<?xml") {
        return false;
    }
    let declaration = text.find("?>").map_or(text, |end| &text[..end + 2]);
    let end_line = declaration.matches('\n').count() as u64 + 1;
    let end_column = declaration
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count() as u64;
    (*line, *column) <= (end_line, end_column)
}

fn char_at(text: &str, line: u64, column: u64) -> Option<char> {
    text.get(offset(text, line, column)?..)?.chars().next()
}

/// The byte offset of a line and column as officewright counts them: lines end at `\n`,
/// columns count characters, both from 1.
fn offset(text: &str, line: u64, column: u64) -> Option<usize> {
    let line_index = usize::try_from(line).ok()?.checked_sub(1)?;
    let start: usize = text
        .split('\n')
        .take(line_index)
        .map(|line| line.len() + 1)
        .sum();
    let line = text.get(start..)?.split('\n').next()?;
    let (within, _) = line
        .char_indices()
        .nth(usize::try_from(column).ok()?.checked_sub(1)?)?;
    Some(start + within)
}

/// The names of the attributes of the start tag at a line and column of the text. Only
/// called on text roxmltree read, so the tag is well formed.
fn attribute_names(text: &str, line: u64, column: u64) -> Vec<&str> {
    let mut names = Vec::new();
    let Some(mut rest) = offset(text, line, column)
        .and_then(|start| text.get(start..))
        .and_then(|tag| tag.strip_prefix('<'))
    else {
        return names;
    };
    let space = |c: char| matches!(c, ' ' | '\t' | '\r' | '\n');
    rest = rest.trim_start_matches(|c: char| !space(c) && c != '/' && c != '>');
    loop {
        rest = rest.trim_start_matches(space);
        if rest.starts_with(['/', '>']) {
            return names;
        }
        let Some((name, value)) = rest.split_once('=') else {
            return names;
        };
        names.push(name.trim_end_matches(space));
        let value = value.trim_start_matches(space);
        let Some(quote) = value.chars().next() else {
            return names;
        };
        let Some((_, after)) = value[quote.len_utf8()..].split_once(quote) else {
            return names;
        };
        rest = after;
    }
}

/// What officewright's messages keep of a name: at most 64 characters, then `…`.
fn short(text: &str) -> String {
    match text.char_indices().nth(64) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

fn options<'a>() -> ParsingOptions<'a> {
    ParsingOptions {
        allow_dtd: false,
        ..ParsingOptions::default()
    }
}

fn read(input: &Input<'_>, limits: &Limits) -> Result<Vec<Node>, Error> {
    let mut reader = input.reader(limits);
    let mut mce = Processor::new(&mut reader);
    let understood = reader.intern("urn:understood");
    mce.understand(understood);
    let mut nodes = Vec::new();
    while let Some(event) = reader.read_event()? {
        match event {
            Event::Start(tag) => {
                // Markup Compatibility errors are not XML errors, which is all roxmltree
                // checks, so they only must not panic.
                if mce.start(&reader, &tag).is_ok() {
                    for attribute in tag.attributes() {
                        let _ = mce.attribute(attribute.name());
                    }
                    if tag.name().local() == "Choice" {
                        let _ = mce.is_selectable(&reader, &tag);
                    }
                }
                let namespace =
                    |ns: Option<NsId>| ns.map_or("", |ns| reader.namespace_uri(ns)).to_owned();
                let mut attributes: Vec<_> = tag
                    .attributes()
                    .iter()
                    .map(|attribute| {
                        (
                            namespace(attribute.name().namespace()),
                            attribute.name().local().to_owned(),
                            attribute.value().to_owned(),
                        )
                    })
                    .collect();
                attributes.sort();
                nodes.push(Node::Start {
                    namespace: namespace(tag.name().namespace()),
                    local: tag.name().local().to_owned(),
                    attributes,
                });
            }
            Event::End(_) => {
                mce.end();
                nodes.push(Node::End);
            }
            Event::Text(text) => push_text(&mut nodes, &text),
            Event::Comment(comment) => nodes.push(Node::Comment(comment.to_owned())),
            Event::ProcessingInstruction(pi) => nodes.push(Node::Pi(
                pi.target().to_owned(),
                Some(pi.data())
                    .filter(|data| !data.is_empty())
                    .map(str::to_owned),
            )),
            _ => {}
        }
    }
    Ok(nodes)
}

/// Walks roxmltree's tree; only called when officewright read the document, so its depth
/// is within officewright's limit.
fn walk(node: roxmltree::Node<'_, '_>, nodes: &mut Vec<Node>) {
    for child in node.children() {
        match child.node_type() {
            NodeType::Element => {
                let mut attributes: Vec<_> = child
                    .attributes()
                    .map(|attribute| {
                        (
                            attribute.namespace().unwrap_or_default().to_owned(),
                            attribute.name().to_owned(),
                            attribute.value().to_owned(),
                        )
                    })
                    .collect();
                attributes.sort();
                nodes.push(Node::Start {
                    namespace: child.tag_name().namespace().unwrap_or_default().to_owned(),
                    local: child.tag_name().name().to_owned(),
                    attributes,
                });
                walk(child, nodes);
                nodes.push(Node::End);
            }
            NodeType::Text => push_text(nodes, child.text().unwrap_or_default()),
            NodeType::Comment => {
                nodes.push(Node::Comment(child.text().unwrap_or_default().to_owned()))
            }
            NodeType::PI => {
                if let Some(pi) = child.pi() {
                    nodes.push(Node::Pi(
                        pi.target.to_owned(),
                        pi.value
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned),
                    ));
                }
            }
            NodeType::Root => {}
        }
    }
}

fn push_text(nodes: &mut Vec<Node>, text: &str) {
    match nodes.last_mut() {
        Some(Node::Text(previous)) => previous.push_str(text),
        _ => nodes.push(Node::Text(text.to_owned())),
    }
}

/// The decoding has no oracle in roxmltree, which reads the text `Input::decode` gives
/// (#259). Every input is decoded here as well, with the standard library and XML 1.0's
/// rules: as UTF-16 where `Input::decode` should take it so (a byte order mark, or `<?`
/// in either order of bytes), else as UTF-8 after an optional byte order mark; then
/// section 2.11 (CR LF and a lone CR become LF) and section 2.2 (the characters XML
/// allows). `Input::decode` must give the same text, or fail exactly when this decoding
/// fails, so a decoder that takes too much as UTF-16 fails too (review of #387).
fn check_decoding(data: &[u8]) {
    let utf16 = |units: &[u8], little: bool| {
        units
            .len()
            .is_multiple_of(2)
            .then(|| {
                let units: Vec<u16> = units
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&pair| {
                        if little {
                            u16::from_le_bytes(pair)
                        } else {
                            u16::from_be_bytes(pair)
                        }
                    })
                    .collect();
                String::from_utf16(&units).ok()
            })
            .flatten()
    };
    let decoded = match data {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0x3C, 0x00, 0x3F, 0x00, ..] => utf16(data, true),
        [0x00, 0x3C, 0x00, 0x3F, ..] => utf16(data, false),
        _ => {
            let text = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
            std::str::from_utf8(text).ok().map(str::to_owned)
        }
    };
    let expected = decoded
        .map(|text| text.replace("\r\n", "\n").replace('\r', "\n"))
        .filter(|text| text.chars().all(is_xml_char));
    match (Input::decode(data), expected) {
        (Ok(input), Some(expected)) => assert_eq!(input.as_str(), expected),
        (Err(_), None) => {}
        (Ok(input), None) => panic!(
            "officewright decodes {:?}, the standard library does not",
            input.as_str()
        ),
        (Err(err), Some(expected)) => panic!("officewright refuses {expected:?}: {err}"),
    }
}

/// XML 1.0, section 2.2, production 2.
fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}
