//! Plain text of the document: what [`Document::text`] and the `text` methods of the
//! views give.

use super::{Document, Kind, RunItem};

/// Appends the text of blocks: each paragraph followed by a line feed.
pub(super) fn blocks(document: &Document, children: &[u32], out: &mut String) {
    for &index in children {
        if let Some(node) = document.node(index) {
            if let Kind::Paragraph = node.kind {
                inlines(document, &node.children, out);
                out.push('\n');
            }
        }
    }
}

/// Appends the text of a paragraph's content.
pub(super) fn inlines(document: &Document, children: &[u32], out: &mut String) {
    for &index in children {
        if let Some(Kind::Run { content }) = document.node(index).map(|node| &node.kind) {
            run(content, out);
        }
    }
}

/// Appends the text of a run's content.
pub(super) fn run(content: &[RunItem], out: &mut String) {
    for item in content {
        match item {
            RunItem::Text(text) => out.push_str(text),
            RunItem::Tab => out.push('\t'),
            RunItem::Break(_) | RunItem::CarriageReturn => out.push('\n'),
            RunItem::NoBreakHyphen => out.push('\u{2011}'),
            _ => {}
        }
    }
}
