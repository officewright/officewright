//! Plain text of the document: what [`Document::text`] and the `text` methods of the
//! views give.
//!
//! The tree is walked with an explicit stack: tables nest in tables as deep as the XML
//! allows, and that must not decide how deep the call stack grows.

use super::views::through_containers;
use super::{Document, Kind, RevisionKind, RunItem};

enum Task {
    Node(u32),
    Push(char),
    /// A table cell starts here.
    CellStart,
    /// Drops a line feed the cell that started last wrote at its end: the last paragraph
    /// of a table cell ends the cell, not a line.
    CellEnd,
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
    // Where the text of each open cell starts.
    let mut cells: Vec<usize> = Vec::new();
    while let Some(task) = tasks.pop() {
        let index = match task {
            Task::Node(index) => index,
            Task::Push(c) => {
                out.push(c);
                continue;
            }
            Task::CellStart => {
                cells.push(out.len());
                continue;
            }
            Task::CellEnd => {
                let start = cells.pop().unwrap_or(out.len());
                if out.len() > start && out.ends_with('\n') {
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
            Kind::Paragraph { .. } => {
                tasks.push(Task::Push('\n'));
                push_children(&mut tasks);
            }
            Kind::Row => {
                tasks.push(Task::Push('\n'));
                let cells: Vec<u32> = through_containers(document, &node.children)
                    .filter(|&i| matches!(document.node(i).map(|n| &n.kind), Some(Kind::Cell)))
                    .collect();
                for (n, &cell) in cells.iter().enumerate().rev() {
                    tasks.push(Task::CellEnd);
                    tasks.push(Task::Node(cell));
                    tasks.push(Task::CellStart);
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
            | Kind::ContentControl { .. }
            | Kind::Hyperlink(_)
            | Kind::Revision(_)
            | Kind::SimpleField { .. }
            | Kind::Wrapper => push_children(&mut tasks),
        }
    }
}
