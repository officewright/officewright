//! Hyperlinks, `HYPERLINK` fields and bookmarks as the IR's link marks.

use std::sync::Arc;

use officewright_layout::ir;

use super::{MAX_LINK_BYTES, Once, Translator};
use crate::model::xml::Sym;
use crate::model::{BookmarkStart, Hyperlink, HyperlinkField};

impl<'a> Translator<'a> {
    /// The name of a bookmark start that the IR gets: the start Word keeps of its name,
    /// the last of the document (MS-OI29500 §17.13.6.2(c)); none for any other, or one
    /// without a name.
    pub(super) fn kept_bookmark(&self, start: BookmarkStart<'a>) -> Option<&'a str> {
        let name = start.name().filter(|name| !name.is_empty())?;
        (self.last_bookmarks.get(name) == Some(&start.span())).then_some(name)
    }

    /// Notes a bookmark among blocks, between rows or between cells, which starts at the
    /// next paragraph: where Word puts what it marks.
    pub(super) fn bookmark_before(&mut self, start: BookmarkStart<'a>) {
        if let Some(name) = self.kept_bookmark(start) {
            self.pending_bookmarks.push(name.to_owned());
        }
    }

    /// Where a hyperlink (`w:hyperlink`, ECMA-376 Part 1, §17.16.22) goes. See
    /// [`shared_target`](Self::shared_target).
    pub(super) fn link_target(&mut self, link: Hyperlink<'a>) -> Option<Arc<ir::LinkTarget>> {
        let anchor = link.anchor().unwrap_or_default();
        // Keyed by the interned target, so that a link to a known address costs as much as
        // its own anchor, however long the target that many links share.
        let key = LinkKey::Hyperlink(link.target_symbol(), anchor);
        if let Some(known) = self.link_keys.get(&key) {
            return known.clone();
        }
        let target = self.shared_target(link.target().unwrap_or_default(), anchor);
        self.link_keys.insert(key, target.clone());
        target
    }

    /// Where a `HYPERLINK` field (§17.16.5.25) goes: as a hyperlink with its first
    /// argument for a target and its `\l` for an anchor, a hypothesis for a field with
    /// both until a Word render.
    pub(super) fn field_link_target(
        &mut self,
        link: &HyperlinkField,
    ) -> Option<Arc<ir::LinkTarget>> {
        let (target, anchor) = (
            link.target().unwrap_or_default(),
            link.bookmark().unwrap_or_default(),
        );
        let key = LinkKey::Field(target.into(), anchor.into());
        if let Some(known) = self.link_keys.get(&key) {
            return known.clone();
        }
        let shared = self.shared_target(target, anchor);
        self.link_keys.insert(key, shared.clone());
        shared
    }

    /// The target of a link with this target and this anchor (empty for none): the
    /// target, with `#` and the anchor after it where it has both, as Word makes it
    /// (MS-OI29500 §17.16.22(b)), where the standard ignores the anchor; else the bookmark
    /// the anchor names. None for a link with neither, or past the bytes of targets the IR
    /// may hold. `w:docLocation` is not part of it: what Word does with it is not known.
    ///
    /// Every link to one address gets the same target, made once. The budget is checked
    /// before the target is made, so that an address past it costs nothing; such an
    /// address is left out even where links of another kind (a hyperlink and a field) made
    /// the same target before.
    fn shared_target(&mut self, target: &str, anchor: &str) -> Option<Arc<ir::LinkTarget>> {
        let length = match (target, anchor) {
            ("", "") => return None,
            ("", anchor) => anchor.len(),
            (target, "") => target.len(),
            (target, anchor) => target.len() + 1 + anchor.len(),
        };
        if self.link_bytes.saturating_add(length) > MAX_LINK_BYTES {
            self.report_once(
                Once::LinkBytes,
                "links past 32 MiB of web addresses and other targets are left out, here and wherever else the document has one",
            );
            return None;
        }
        let made = made(target, anchor);
        if let Some(shared) = self.link_targets.get(&made) {
            return Some(Arc::clone(shared));
        }
        self.link_bytes += length;
        let shared = Arc::new(made);
        self.link_targets.insert(Arc::clone(&shared));
        Some(shared)
    }
}

/// The target of a link with this target and this anchor, not both empty.
fn made(target: &str, anchor: &str) -> ir::LinkTarget {
    let made = match (target, anchor) {
        ("", anchor) => ir::LinkTarget::Bookmark(anchor.to_owned()),
        (target, "") => ir::LinkTarget::Uri(target.to_owned()),
        (target, anchor) => ir::LinkTarget::Uri(format!("{target}#{anchor}")),
    };
    #[cfg(test)]
    if let ir::LinkTarget::Uri(text) | ir::LinkTarget::Bookmark(text) = &made {
        super::work(|work| work.link_target_bytes += text.len());
    }
    made
}

/// What makes a link's address, as the translation keys the targets it made: a
/// hyperlink's interned target and its anchor, or a `HYPERLINK` field's target and
/// anchor, copied from its instruction (empty for none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LinkKey<'a> {
    Hyperlink(Option<Sym>, &'a str),
    Field(Box<str>, Box<str>),
}

impl std::hash::Hash for LinkKey<'_> {
    /// As derived would, counting in tests the bytes of text it hashes.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        let texts: [&str; 2] = match self {
            Self::Hyperlink(symbol, anchor) => {
                symbol.hash(state);
                ["", anchor]
            }
            Self::Field(target, anchor) => [target, anchor],
        };
        for text in texts {
            #[cfg(test)]
            super::work(|work| work.link_key_bytes += text.len());
            text.hash(state);
        }
    }
}

/// The IR's mark for the start of a link to `target`, which it shares.
pub(super) fn link_start(target: &Arc<ir::LinkTarget>) -> ir::Inline {
    ir::Inline::link_start(Arc::clone(target))
}
