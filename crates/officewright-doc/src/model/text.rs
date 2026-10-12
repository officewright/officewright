//! Plain text of the document: what [`Document::text`] and the `text` methods of the
//! views give.
//!
//! The tree is walked with an explicit stack: tables nest in tables as deep as the XML
//! allows, and that must not decide how deep the call stack grows.

use super::{Document, Kind, RevisionKind, RunItem};

enum Task {
    Node(u32),
    Push(char),
    /// Drops a line feed at the end of the text: the last paragraph of a table cell
    /// ends the cell, not a line.
    TrimLineFeed,
    /// Entering (`true`) or leaving deleted content.
    Deleted(bool),
}

/// Appends the text of blocks: each paragraph followed by a line feed, the cells of a
/// table row separated by tabs and the row followed by a line feed.
pub(super) fn blocks(document: &Document, children: &[u32], out: &mut String) {
    walk(document, children, out);
}

/// Appends the text of a paragraph's content.
pub(super) fn inlines(document: &Document, children: &[u32], out: &mut String) {
    walk(document, children, out);
}

/// Appends the text of a run's content outside deleted content.
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

fn walk(document: &Document, children: &[u32], out: &mut String) {
    let mut tasks: Vec<Task> = children.iter().rev().map(|&i| Task::Node(i)).collect();
    let mut deleted = 0usize;
    while let Some(task) = tasks.pop() {
        let index = match task {
            Task::Node(index) => index,
            Task::Push(c) => {
                out.push(c);
                continue;
            }
            Task::TrimLineFeed => {
                if out.ends_with('\n') {
                    out.pop();
                }
                continue;
            }
            Task::Deleted(entering) => {
                if entering {
                    deleted += 1;
                } else {
                    deleted = deleted.saturating_sub(1);
                }
                continue;
            }
        };
        let Some(node) = document.node(index) else {
            continue;
        };
        let push_children = |tasks: &mut Vec<Task>| {
            tasks.extend(node.children.iter().rev().map(|&i| Task::Node(i)));
        };
        match &node.kind {
            Kind::Paragraph => {
                tasks.push(Task::Push('\n'));
                push_children(&mut tasks);
            }
            Kind::Row => {
                tasks.push(Task::Push('\n'));
                let cells = cells(document, &node.children);
                for (n, &cell) in cells.iter().enumerate().rev() {
                    tasks.push(Task::TrimLineFeed);
                    tasks.push(Task::Node(cell));
                    if n > 0 {
                        tasks.push(Task::Push('\t'));
                    }
                }
            }
            Kind::Revision(data)
                if matches!(data.kind, RevisionKind::Deletion | RevisionKind::MoveFrom) =>
            {
                tasks.push(Task::Deleted(false));
                push_children(&mut tasks);
                tasks.push(Task::Deleted(true));
            }
            Kind::Run { content } => {
                if deleted == 0 {
                    run(content, out);
                }
            }
            Kind::Other(_) => {}
            Kind::Table { .. }
            | Kind::Cell
            | Kind::ContentControl
            | Kind::Hyperlink(_)
            | Kind::Revision(_)
            | Kind::SimpleField { .. }
            | Kind::Wrapper => push_children(&mut tasks),
        }
    }
}

/// The cells of a row, seen through the content controls and wrappers around them.
fn cells(document: &Document, children: &[u32]) -> Vec<u32> {
    let mut cells = Vec::new();
    let mut stack = vec![children.iter()];
    while let Some(iter) = stack.last_mut() {
        let Some(&index) = iter.next() else {
            stack.pop();
            continue;
        };
        match document.node(index).map(|node| (&node.kind, node)) {
            Some((Kind::Cell, _)) => cells.push(index),
            Some((Kind::ContentControl | Kind::Wrapper, node)) => {
                stack.push(node.children.iter());
            }
            _ => {}
        }
    }
    cells
}
