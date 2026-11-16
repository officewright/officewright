//! Links and the named destinations they go to: link annotations (ISO 32000-1, 12.5.6.5)
//! and the `/Dests` name tree of the catalog (12.3.2.3).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{BuildHasher, RandomState};
use std::ops::Range;
use std::sync::Arc;

use officewright_core::Pt;

use crate::document::{MAX_POSITION, check_position, number, push_string};
use crate::glyphs::MAX_STRING_BYTES;
use crate::path::check_range;
use crate::{Error, Result};

/// The smallest side of a link: smaller ones would round to 0 in the four decimals the
/// file stores.
const MIN_LINK_SIDE: f64 = 0.01;

/// The most links a page may have: the most elements its `/Annots` array may have in PDF
/// 1.4 (PDF Reference 1.4, Appendix C).
pub(crate) const MAX_LINKS: usize = 8191;

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
    /// link (12.3.2.3). The file stores the name as
    /// [`add_destination`](crate::Page::add_destination) says.
    Destination(String),
}

/// A link of a page, checked and in the form the file stores.
#[derive(Clone, Debug)]
pub(crate) struct Link {
    /// The lower-left and upper-right corners.
    rect: [f64; 4],
    /// Shared by the links of one target that [`LinkTargets`] gave out, so that a link over
    /// many lines keeps its target once (#581).
    target: Arc<Target>,
}

/// Where a link goes, checked and in the form the file stores: the action that every
/// link to it shares, written once.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Target {
    /// The URI, percent-encoded.
    Uri(Vec<u8>),
    /// The destination's name as its key in the name tree.
    Destination(Vec<u8>),
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

/// The targets of links checked so far, each kept once in the form the file stores: the
/// links of one target share it. Only a cache, so that a link that spans many lines does
/// not keep its URI once for each (#581): the file writes each target once whether or not
/// the links share it.
#[derive(Clone, Debug, Default)]
pub(crate) struct LinkTargets {
    /// The targets by the hash of what they were checked from.
    targets: HashMap<u64, Vec<Arc<Target>>>,
    hasher: RandomState,
    /// The target asked for last, as it was asked for, which the boxes of one link ask
    /// for one after another: found by comparing it, without hashing or encoding it again.
    last: Option<(LinkTarget, Arc<Target>)>,
}

impl LinkTargets {
    /// `target` checked, the same as for every link to it before.
    fn get(&mut self, target: &LinkTarget) -> Result<Arc<Target>> {
        if let Some((_, checked)) = self.last.as_ref().filter(|(last, _)| last == target) {
            return Ok(Arc::clone(checked));
        }
        let hash = self.hasher.hash_one(target);
        let known = self
            .targets
            .get(&hash)
            .and_then(|known| known.iter().find(|known| known.is(target)))
            .cloned();
        let checked = match known {
            Some(checked) => checked,
            None => {
                let checked = Arc::new(Target::check(target)?);
                self.targets
                    .entry(hash)
                    .or_default()
                    .push(Arc::clone(&checked));
                checked
            }
        };
        self.last = Some((target.clone(), Arc::clone(&checked)));
        Ok(checked)
    }
}

impl Link {
    /// Checks the rectangle of a link with its lower-left corner at (`x`, `y`) and the
    /// given width and height: its corners.
    pub(crate) fn rect(x: Pt, y: Pt, width: Pt, height: Pt) -> Result<[f64; 4]> {
        check_range("link width", width.0, MIN_LINK_SIDE, MAX_POSITION)?;
        check_range("link height", height.0, MIN_LINK_SIDE, MAX_POSITION)?;
        check_position(x, y)?;
        let (right, top) = (Pt(x.0 + width.0), Pt(y.0 + height.0));
        check_position(right, top)?;
        Ok([x.0, y.0, right.0, top.0])
    }

    /// A link of the corners `rect` ([`Self::rect`]) to `target`, checked by `targets`.
    pub(crate) fn new(
        rect: [f64; 4],
        target: &LinkTarget,
        targets: &mut LinkTargets,
    ) -> Result<Self> {
        Ok(Self {
            rect,
            target: targets.get(target)?,
        })
    }

    /// Where the link goes.
    pub(crate) fn target(&self) -> &Target {
        &self.target
    }

    /// The key of the destination the link goes to; `None` for a URI.
    fn destination(&self) -> Option<&[u8]> {
        match self.target.as_ref() {
            Target::Destination(key) => Some(key),
            Target::Uri(_) => None,
        }
    }

    /// The annotation dictionary, whose action is object `action` ([`Target::action`]).
    pub(crate) fn dictionary(&self, action: usize) -> Vec<u8> {
        let [left, bottom, right, top] = self.rect.map(number);
        format!(
            "<< /Type /Annot /Subtype /Link /Rect [{left} {bottom} {right} {top}] /Border [0 0 0] \
             /A {action} 0 R >>"
        )
        .into_bytes()
    }
}

impl Target {
    /// `target` checked, in the form the file stores.
    fn check(target: &LinkTarget) -> Result<Self> {
        match target {
            LinkTarget::Uri(uri) => {
                let encoded = uri_key(uri);
                check_length("URI length in bytes", encoded.len())?;
                Ok(Self::Uri(encoded))
            }
            LinkTarget::Destination(name) => Ok(Self::Destination(name_key(name)?)),
        }
    }

    /// Whether it is `target` checked, compared in the form the file stores.
    fn is(&self, target: &LinkTarget) -> bool {
        match (self, target) {
            (Self::Uri(encoded), LinkTarget::Uri(uri)) => encodes_to(uri, keeps_uri, encoded),
            (Self::Destination(key), LinkTarget::Destination(name)) => {
                encodes_to(name, keeps_name, key)
            }
            _ => false,
        }
    }

    /// The action of every link to it (ISO 32000-1, 12.6.4.7 and 12.6.4.2): an indirect
    /// object, which the links share (12.5.6.5), so that a link over many lines writes
    /// its target once.
    pub(crate) fn action(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Uri(uri) => {
                out.extend_from_slice(b"<< /S /URI /URI ");
                push_string(&mut out, uri);
            }
            Self::Destination(key) => {
                out.extend_from_slice(b"<< /S /GoTo /D ");
                push_string(&mut out, key);
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
            if let [(first, first_page), (second, second_page)] = pair {
                if first.key == second.key {
                    return Err(Error::DuplicateDestination {
                        name: second.name.clone(),
                        first: *first_page,
                        second: *second_page,
                    });
                }
            }
        }
        for (page, (_, links)) in pages.into_iter().enumerate() {
            for key in links.iter().filter_map(Link::destination) {
                if entries
                    .binary_search_by(|(known, _)| known.key.as_slice().cmp(key))
                    .is_err()
                {
                    return Err(Error::UnknownDestination {
                        name: key_name(key),
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
                                literal(&destination.key),
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
                            .map_or_else(String::new, |(destination, _)| literal(&destination.key))
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
fn uri_key(uri: &str) -> Vec<u8> {
    percent_encode(uri, keeps_uri)
}

/// Whether a URI keeps `byte` as it is.
fn keeps_uri(byte: u8) -> bool {
    (b'!'..=b'~').contains(&byte)
}

/// Whether a destination's key keeps `byte` as it is.
fn keeps_name(byte: u8) -> bool {
    byte != b'%' && (b' '..=b'~').contains(&byte)
}

/// Whether `text` percent-encoded as [`percent_encode`] does with `keep` is `encoded`,
/// compared without encoding it.
fn encodes_to(text: &str, keep: impl Fn(u8) -> bool, encoded: &[u8]) -> bool {
    let mut rest = encoded;
    for &byte in text.as_bytes() {
        let matched = if keep(byte) {
            rest.split_first().filter(|(first, _)| **first == byte)
        } else {
            let escape = [b'%', hex_digit(byte >> 4), hex_digit(byte & 0x0F)];
            rest.split_at_checked(3)
                .filter(|(head, _)| *head == escape)
                .map(|(_, tail)| (&0, tail))
        };
        match matched {
            Some((_, tail)) => rest = tail,
            None => return false,
        }
    }
    rest.is_empty()
}

/// The upper-case hexadecimal digit of `nibble` (0 to 15), as `%XX` writes it.
fn hex_digit(nibble: u8) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        _ => b'A' + nibble - 10,
    }
}

/// The name a destination's key stands for: its `%XX` decoded.
fn key_name(key: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(key.len());
    let mut rest = key;
    while let Some((&byte, tail)) = rest.split_first() {
        let decoded = (byte == b'%')
            .then(|| tail.get(..2))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match decoded {
            Some(value) => {
                bytes.push(value);
                rest = tail.get(2..).unwrap_or_default();
            }
            None => {
                bytes.push(byte);
                rest = tail;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `text` with each byte of its UTF-8 form that `keep` refuses written as `%XX`.
fn percent_encode(text: &str, keep: impl Fn(u8) -> bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if keep(byte) {
            out.push(byte);
        } else {
            out.extend_from_slice(format!("%{byte:02X}").as_bytes());
        }
    }
    out
}

/// `name` as a key of the name tree: printable ASCII as it is, but for `%`, and every
/// other byte of its UTF-8 form as `%XX`. The keys are sorted by their bytes (7.9.6), and
/// readers that compare them as text, as qpdf does, put printable ASCII in the same order.
/// `%` itself is encoded, so that no two names share a key.
fn name_key(name: &str) -> Result<Vec<u8>> {
    let key = percent_encode(name, keeps_name);
    check_length("destination name length in bytes", key.len())?;
    Ok(key)
}

/// Fails unless a string of `length` bytes is not empty and fits in a PDF string.
fn check_length(what: &'static str, length: usize) -> Result<()> {
    check_range(what, length as f64, 1.0, MAX_STRING_BYTES as f64)
}

/// `key` as a literal string.
fn literal(key: &[u8]) -> String {
    let mut out = Vec::with_capacity(key.len() + 2);
    push_string(&mut out, key);
    // push_string writes printable ASCII only.
    String::from_utf8_lossy(&out).into_owned()
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
            uri_key("https://example.com/a b?q=%41&x=~!"),
            b"https://example.com/a%20b?q=%41&x=~!"
        );
        assert_eq!(uri_key("\t\u{7f}é"), b"%09%7F%C3%A9");
        assert_eq!(
            uri_key("https://пр.укр/ж"),
            b"https://%D0%BF%D1%80.%D1%83%D0%BA%D1%80/%D0%B6"
        );
        assert_eq!(uri_key("\u{1F600}"), b"%F0%9F%98%80");
    }

    #[test]
    fn names_are_keys_of_printable_ascii() {
        assert_eq!(name_key("Appendix A").unwrap(), b"Appendix A");
        assert_eq!(name_key(" (~)").unwrap(), b" (~)");
        assert_eq!(name_key("a\tb\u{7f}").unwrap(), b"a%09b%7F");
        assert_eq!(name_key("Ж").unwrap(), b"%D0%96");
        assert_eq!(name_key("\u{1F600}").unwrap(), b"%F0%9F%98%80");
        // `%` is encoded too, so a name that looks encoded keeps a key of its own.
        assert_eq!(name_key("100%").unwrap(), b"100%25");
        assert_ne!(name_key("%D0%96").unwrap(), name_key("Ж").unwrap());
    }

    #[test]
    fn compares_a_target_without_encoding_it_and_names_a_key_back() {
        let texts = [
            "https://example.com/a b/(Ж)",
            "100%",
            "%41",
            "Глава",
            "a)b",
            "\u{1F600}",
            "~ !",
        ];
        for text in texts {
            assert!(encodes_to(text, keeps_uri, &uri_key(text)), "{text}");
            let key = name_key(text).unwrap();
            assert!(encodes_to(text, keeps_name, &key), "{text}");
            assert_eq!(key_name(&key), text);
            // Another text, a longer one and a shorter one are not the same.
            for other in texts.iter().filter(|other| **other != text) {
                assert!(
                    !encodes_to(other, keeps_uri, &uri_key(text)),
                    "{other} {text}"
                );
            }
            assert!(!encodes_to(&format!("{text}x"), keeps_uri, &uri_key(text)));
            let shorter = &uri_key(text)[..uri_key(text).len() - 1];
            assert!(!encodes_to(text, keeps_uri, shorter), "{text}");
        }
        // A key that is not one keeps what it cannot decode.
        assert_eq!(key_name(b"%4"), "%4");
        assert_eq!(key_name(b"%zz%41"), "%zzA");
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
        // Three bytes for each byte of UTF-8 that is not printable ASCII, and for `%`.
        assert_eq!(name_key(&"ж".repeat(5461)).unwrap().len(), 32766);
        refused(
            name_key(&"ж".repeat(5462)),
            "destination name length in bytes",
            32772.0,
        );
        refused(
            name_key(&"%".repeat(10923)),
            "destination name length in bytes",
            32769.0,
        );
    }
}
