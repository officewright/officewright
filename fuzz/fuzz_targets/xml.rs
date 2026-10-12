//! Reads the input as an XML part and checks it against roxmltree: both must accept and
//! reject the same documents and read the same elements, attributes, text, comments and
//! processing instructions from them.
#![no_main]

use libfuzzer_sys::fuzz_target;
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
            assert_eq!(ours, nodes);
        }
        (Ok(ours), Err(err)) => {
            panic!("roxmltree rejects ({err}) what officewright reads as {ours:?}")
        }
        (Err(err), Ok(_)) if rejected_on_purpose(input.as_str(), &err) => {}
        (Err(err), Ok(_)) => panic!("officewright rejects ({err}) what roxmltree accepts"),
        (Err(_), Err(_)) => {}
    }
});

/// Whether officewright rejects the text where roxmltree accepts it, and rightly so:
/// either a document type declaration, which OOXML parts may not have and roxmltree
/// accepts when it is empty, or a rule of XML 1.0 that roxmltree does not check.
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
    if message == "document type declarations are not allowed in OOXML parts" {
        return true;
    }
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
    // The Unique Att Spec constraint covers namespace declarations too; roxmltree reads
    // two `xmlns` attributes on one element.
    if message.starts_with("duplicate attribute xmlns") {
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
    let line = text
        .split('\n')
        .nth(usize::try_from(line).ok()?.checked_sub(1)?)?;
    line.chars()
        .nth(usize::try_from(column).ok()?.checked_sub(1)?)
}

fn options<'a>() -> ParsingOptions<'a> {
    ParsingOptions {
        allow_dtd: false,
        ..ParsingOptions::default()
    }
}

fn read(input: &Input<'_>, limits: &Limits) -> Result<Vec<Node>, Error> {
    let mut reader = input.reader(limits);
    let mut nodes = Vec::new();
    while let Some(event) = reader.read_event()? {
        match event {
            Event::Start(tag) => {
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
            Event::End(_) => nodes.push(Node::End),
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
