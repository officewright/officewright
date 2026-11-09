//! Links and the named destinations they go to: link annotations (ISO 32000-1, 12.5.6.5)
//! and the `/Dests` name tree of the catalog (12.3.2.3).

use std::fmt::Write as _;
use std::ops::Range;

use officewright_core::Pt;

use crate::document::{MAX_POSITION, check_position, number, push_string};
use crate::glyphs::MAX_STRING_BYTES;
use crate::path::check_range;
use crate::{Error, Result};

/// The smallest side of a link: smaller ones would round to 0 in the four decimals the
/// file stores.
const MIN_LINK_SIDE: f64 = 0.01;

/// The most names a leaf of the `/Dests` name tree holds, and the most kids a node above
/// the leaves has, so that a reader finds a name without reading them all.
const TREE_NODE: usize = 64;

/// Where a link goes: [`Page::add_link`](crate::Page::add_link).
///
/// One of the drawing primitives the layout engine paints with: public, but exempt from
/// semantic versioning until officewright 1.0 (feature `unstable-drawing`).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LinkTarget {
    /// Opens a URI (ISO 32000-1, 12.6.4.7). A URI is ASCII, so each byte of the UTF-8
    /// form of this text outside `!` to `~` is written percent-encoded as `%XX`: spaces,
    /// control characters and every character outside ASCII, as RFC 3987, 3.1 maps an IRI
    /// to a URI. Nothing else is changed.
    Uri(String),
    /// Goes to the named destination of this name, which a page of the same document adds
    /// with [`Page::add_destination`](crate::Page::add_destination), before or after the
    /// link (12.3.2.3).
    Destination(String),
}

/// A link of a page, checked and in the form the file stores.
#[derive(Clone, Debug)]
pub(crate) struct Link {
    /// The lower-left and upper-right corners.
    rect: [f64; 4],
    target: Target,
}

#[derive(Clone, Debug)]
enum Target {
    /// The URI, percent-encoded.
    Uri(Vec<u8>),
    /// The destination's name, and its key in the name tree.
    Destination { name: String, key: Vec<u8> },
}

/// A named destination of a page, checked and in the form the file stores.
#[derive(Clone, Debug)]
pub(crate) struct Destination {
    name: String,
    /// The name as the name tree's key.
    key: Vec<u8>,
    x: f64,
    y: f64,
}

impl Link {
    /// Checks a link of the rectangle with its lower-left corner at (`x`, `y`) and the
    /// given width and height, to `target`.
    pub(crate) fn new(x: Pt, y: Pt, width: Pt, height: Pt, target: &LinkTarget) -> Result<Self> {
        check_range("link width", width.0, MIN_LINK_SIDE, MAX_POSITION)?;
        check_range("link height", height.0, MIN_LINK_SIDE, MAX_POSITION)?;
        check_position(x, y)?;
        let (right, top) = (Pt(x.0 + width.0), Pt(y.0 + height.0));
        check_position(right, top)?;
        let target = match target {
            LinkTarget::Uri(uri) => {
                let encoded = percent_encode(uri);
                check_length("URI length in bytes", encoded.len())?;
                Target::Uri(encoded)
            }
            LinkTarget::Destination(name) => Target::Destination {
                name: name.clone(),
                key: name_key(name)?,
            },
        };
        Ok(Self {
            rect: [x.0, y.0, right.0, top.0],
            target,
        })
    }

    /// The name of the destination the link goes to, and its key; `None` for a URI.
    fn destination(&self) -> Option<(&str, &[u8])> {
        match &self.target {
            Target::Destination { name, key } => Some((name, key)),
            Target::Uri(_) => None,
        }
    }

    /// The annotation dictionary.
    pub(crate) fn dictionary(&self) -> Vec<u8> {
        let [left, bottom, right, top] = self.rect.map(number);
        let mut out = format!(
            "<< /Type /Annot /Subtype /Link /Rect [{left} {bottom} {right} {top}] /Border [0 0 0] "
        )
        .into_bytes();
        match &self.target {
            Target::Uri(uri) => {
                out.extend_from_slice(b"/A << /S /URI /URI ");
                push_string(&mut out, uri);
                out.extend_from_slice(b" >>");
            }
            Target::Destination { key, .. } => {
                out.extend_from_slice(format!("/Dest {}", hex_string(key)).as_bytes());
            }
        }
        out.extend_from_slice(b" >>");
        out
    }
}

impl Destination {
    /// Checks a destination named `name` at the point (`x`, `y`).
    pub(crate) fn new(name: &str, x: Pt, y: Pt) -> Result<Self> {
        check_position(x, y)?;
        Ok(Self {
            name: name.to_owned(),
            key: name_key(name)?,
            x: x.0,
            y: y.0,
        })
    }
}

/// The named destinations of a document, sorted by their keys, with the pages they are
/// on.
pub(crate) struct Destinations<'a> {
    entries: Vec<(&'a Destination, usize)>,
    nodes: Vec<Node>,
}

/// A node of the name tree.
struct Node {
    /// The names of a leaf, or the nodes that are a node's kids.
    kind: NodeKind,
    /// The first and last name under the node.
    limits: (usize, usize),
}

enum NodeKind {
    Leaf(Range<usize>),
    Kids(Range<usize>),
}

impl<'a> Destinations<'a> {
    /// The destinations of `pages`, given as the destinations and links of each page.
    ///
    /// # Errors
    ///
    /// [`Error::DuplicateDestination`] if two destinations have one name, and
    /// [`Error::UnknownDestination`] if a link goes to a name that no destination has.
    pub(crate) fn collect<I>(pages: I) -> Result<Self>
    where
        I: IntoIterator<Item = (&'a [Destination], &'a [Link])> + Clone,
    {
        let mut entries: Vec<(&Destination, usize)> = pages
            .clone()
            .into_iter()
            .enumerate()
            .flat_map(|(page, (destinations, _))| destinations.iter().map(move |d| (d, page)))
            .collect();
        // A stable sort keeps the destinations of one name in the order of their pages.
        entries.sort_by(|a, b| a.0.key.cmp(&b.0.key));
        for pair in entries.windows(2) {
            if let [(first, first_page), (second, second_page)] = pair
                && first.key == second.key
            {
                return Err(Error::DuplicateDestination {
                    name: second.name.clone(),
                    first: *first_page,
                    second: *second_page,
                });
            }
        }
        for (page, (_, links)) in pages.into_iter().enumerate() {
            for (name, key) in links.iter().filter_map(Link::destination) {
                if entries
                    .binary_search_by(|(known, _)| known.key.as_slice().cmp(key))
                    .is_err()
                {
                    return Err(Error::UnknownDestination {
                        name: name.to_owned(),
                        page,
                    });
                }
            }
        }
        let nodes = tree(entries.len(), TREE_NODE, TREE_NODE);
        Ok(Self { entries, nodes })
    }

    /// The number of objects the name tree takes: none without destinations.
    pub(crate) fn object_count(&self) -> usize {
        self.nodes.len()
    }

    /// The name tree's objects, the root first, numbered from `first`, with the page
    /// object of each page given by `page_object`.
    pub(crate) fn objects(
        &self,
        first: usize,
        page_object: impl Fn(usize) -> usize,
    ) -> Vec<Vec<u8>> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                let mut out = String::from("<< ");
                match &node.kind {
                    NodeKind::Leaf(names) => {
                        out.push_str("/Names [");
                        for (destination, page) in self.entries.get(names.clone()).unwrap_or(&[]) {
                            let _ = write!(
                                out,
                                "\n{} [{} 0 R /XYZ {} {} null]",
                                hex_string(&destination.key),
                                page_object(*page),
                                number(destination.x),
                                number(destination.y)
                            );
                        }
                        out.push_str("\n]");
                    }
                    NodeKind::Kids(kids) => {
                        let kids: Vec<String> = kids
                            .clone()
                            .map(|kid| format!("{} 0 R", first + kid))
                            .collect();
                        let _ = write!(out, "/Kids [{}]", kids.join(" "));
                    }
                }
                // The root has no limits (7.9.6).
                if i > 0 {
                    let key = |entry: usize| {
                        self.entries
                            .get(entry)
                            .map_or_else(String::new, |(destination, _)| {
                                hex_string(&destination.key)
                            })
                    };
                    let _ = write!(
                        out,
                        " /Limits [{} {}]",
                        key(node.limits.0),
                        key(node.limits.1)
                    );
                }
                out.push_str(" >>");
                out.into_bytes()
            })
            .collect()
    }
}

/// The nodes of a name tree of `count` sorted names, the root first: leaves of at most
/// `leaf` names, and above them nodes of at most `fanout` kids, as few levels as fit.
/// No nodes for no names.
fn tree(count: usize, leaf: usize, fanout: usize) -> Vec<Node> {
    if count == 0 {
        return Vec::new();
    }
    let (leaf, fanout) = (leaf.max(1), fanout.max(2));
    // Built from the leaves up: each level's kids are nodes of the level below.
    let mut levels: Vec<Vec<Node>> = vec![
        (0..count)
            .step_by(leaf)
            .map(|start| {
                let end = (start + leaf).min(count);
                Node {
                    kind: NodeKind::Leaf(start..end),
                    limits: (start, end - 1),
                }
            })
            .collect(),
    ];
    while let Some(below) = levels.last().filter(|level| level.len() > 1) {
        let level = (0..below.len())
            .step_by(fanout)
            .map(|start| {
                let end = (start + fanout).min(below.len());
                let first = below.get(start).map_or(0, |node| node.limits.0);
                let last = below.get(end - 1).map_or(0, |node| node.limits.1);
                Node {
                    kind: NodeKind::Kids(start..end),
                    limits: (first, last),
                }
            })
            .collect();
        levels.push(level);
    }
    // Numbered from the root down, so the kids of a level's nodes are in the level that
    // follows it.
    let mut nodes = Vec::new();
    for level in levels.into_iter().rev() {
        let below = nodes.len() + level.len();
        nodes.extend(level.into_iter().map(|node| Node {
            kind: match node.kind {
                NodeKind::Kids(kids) => NodeKind::Kids(below + kids.start..below + kids.end),
                leaf @ NodeKind::Leaf(_) => leaf,
            },
            limits: node.limits,
        }));
    }
    nodes
}

/// `uri` as a URI: each byte of its UTF-8 form outside `!` to `~` as `%XX`.
fn percent_encode(uri: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(uri.len());
    for &byte in uri.as_bytes() {
        if (b'!'..=b'~').contains(&byte) {
            out.push(byte);
        } else {
            out.extend_from_slice(format!("%{byte:02X}").as_bytes());
        }
    }
    out
}

/// `name` as a text string (7.9.2.2): as it is when every character is printable ASCII,
/// else in UTF-16BE with a byte order mark.
fn name_key(name: &str) -> Result<Vec<u8>> {
    let key = if name.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        name.as_bytes().to_vec()
    } else {
        let mut key = vec![0xFE, 0xFF];
        key.extend(name.encode_utf16().flat_map(u16::to_be_bytes));
        key
    };
    check_length("destination name length in bytes", key.len())?;
    Ok(key)
}

/// Fails unless a string of `length` bytes is not empty and fits in a PDF string.
fn check_length(what: &'static str, length: usize) -> Result<()> {
    check_range(what, length as f64, 1.0, MAX_STRING_BYTES as f64)
}

/// `bytes` as a hexadecimal string.
fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + 2 * bytes.len());
    out.push('<');
    for byte in bytes {
        let _ = write!(out, "{byte:02X}");
    }
    out.push('>');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tree of `count` names as text: each node's kids or names and its limits, the
    /// root first.
    fn shape(count: usize, leaf: usize, fanout: usize) -> Vec<String> {
        tree(count, leaf, fanout)
            .iter()
            .map(|node| {
                let kind = match &node.kind {
                    NodeKind::Leaf(names) => format!("names {names:?}"),
                    NodeKind::Kids(kids) => format!("kids {kids:?}"),
                };
                format!("{kind} limits {:?}", node.limits)
            })
            .collect()
    }

    #[test]
    fn makes_one_leaf_of_names_that_fit_in_one() {
        assert!(tree(0, 3, 3).is_empty());
        assert_eq!(shape(1, 3, 3), ["names 0..1 limits (0, 0)"]);
        assert_eq!(shape(3, 3, 3), ["names 0..3 limits (0, 2)"]);
    }

    #[test]
    fn puts_nodes_above_leaves_until_one_is_left() {
        assert_eq!(
            shape(4, 3, 3),
            [
                "kids 1..3 limits (0, 3)",
                "names 0..3 limits (0, 2)",
                "names 3..4 limits (3, 3)",
            ]
        );
        assert_eq!(
            shape(10, 3, 2),
            [
                "kids 1..3 limits (0, 9)",
                "kids 3..5 limits (0, 5)",
                "kids 5..7 limits (6, 9)",
                "names 0..3 limits (0, 2)",
                "names 3..6 limits (3, 5)",
                "names 6..9 limits (6, 8)",
                "names 9..10 limits (9, 9)",
            ]
        );
    }

    /// The names under `node`, in order, and the depth of each leaf.
    fn walk(
        nodes: &[Node],
        node: usize,
        depth: usize,
        names: &mut Vec<usize>,
        depths: &mut Vec<usize>,
    ) {
        match &nodes[node].kind {
            NodeKind::Leaf(range) => {
                names.extend(range.clone());
                depths.push(depth);
            }
            NodeKind::Kids(kids) => {
                for kid in kids.clone() {
                    // A kid comes after its node, so the walk ends.
                    assert!(kid > node);
                    walk(nodes, kid, depth + 1, names, depths);
                }
            }
        }
    }

    #[test]
    fn every_name_is_under_the_root_once_and_in_order() {
        for (leaf, fanout) in [(1, 2), (2, 2), (3, 3), (64, 64)] {
            for count in 1..300 {
                let nodes = tree(count, leaf, fanout);
                let (mut names, mut depths) = (Vec::new(), Vec::new());
                walk(&nodes, 0, 0, &mut names, &mut depths);
                assert_eq!(
                    names,
                    (0..count).collect::<Vec<_>>(),
                    "{count} {leaf} {fanout}"
                );
                // Balanced: every leaf is as deep as the others.
                assert!(depths.iter().all(|&depth| depth == depths[0]));
                for node in &nodes {
                    let (size, limit) = match &node.kind {
                        NodeKind::Leaf(range) => (range.len(), leaf),
                        NodeKind::Kids(range) => (range.len(), fanout),
                    };
                    assert!((1..=limit).contains(&size), "{count} {leaf} {fanout}");
                }
                // The limits are those of the first and last name under each node.
                for (i, node) in nodes.iter().enumerate() {
                    let mut under = Vec::new();
                    walk(&nodes, i, 0, &mut under, &mut Vec::new());
                    assert_eq!(node.limits, (under[0], under[under.len() - 1]));
                }
            }
        }
    }

    #[test]
    fn percent_encodes_what_a_uri_cannot_hold() {
        assert_eq!(
            percent_encode("https://example.com/a b?q=%41&x=~!"),
            b"https://example.com/a%20b?q=%41&x=~!"
        );
        assert_eq!(percent_encode("\t\u{7f}é"), b"%09%7F%C3%A9");
        assert_eq!(
            percent_encode("https://пр.укр/ж"),
            b"https://%D0%BF%D1%80.%D1%83%D0%BA%D1%80/%D0%B6"
        );
        assert_eq!(percent_encode("\u{1F600}"), b"%F0%9F%98%80");
    }

    #[test]
    fn names_are_ascii_or_utf16_text_strings() {
        assert_eq!(name_key("Appendix A").unwrap(), b"Appendix A");
        assert_eq!(name_key(" ~").unwrap(), b" ~");
        // A tab is not printable, so the name is UTF-16, which no printable ASCII name
        // can be mistaken for.
        assert_eq!(name_key("a\tb").unwrap(), b"\xFE\xFF\0a\0\t\0b");
        assert_eq!(name_key("Ж").unwrap(), b"\xFE\xFF\x04\x16");
        assert_eq!(name_key("\u{1F600}").unwrap(), b"\xFE\xFF\xD8\x3D\xDE\x00");
    }

    #[test]
    fn strings_fit_in_a_pdf_string() {
        let refused = |result: Result<Vec<u8>>, what: &str, length: f64| {
            assert!(
                matches!(&result, Err(Error::OutOfRange { what: w, value, .. }) if *w == what && *value == length),
                "{result:?}"
            );
        };
        refused(name_key(""), "destination name length in bytes", 0.0);
        assert_eq!(name_key(&"a".repeat(32767)).unwrap().len(), 32767);
        refused(
            name_key(&"a".repeat(32768)),
            "destination name length in bytes",
            32768.0,
        );
        // Two bytes of byte order mark and two for each UTF-16 code unit.
        assert_eq!(name_key(&"ж".repeat(16382)).unwrap().len(), 32766);
        refused(
            name_key(&"ж".repeat(16383)),
            "destination name length in bytes",
            32768.0,
        );
    }
}
