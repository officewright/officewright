//! Hyperlinks, `HYPERLINK` fields and bookmarks as the IR's link marks.

use std::borrow::Cow;
use std::sync::Arc;

use officewright_layout::ir;

use super::{MAX_LINK_BYTES, Once, Translator};
use crate::model::{BookmarkStart, HyperlinkField};

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

    /// Where a hyperlink (`w:hyperlink`, ECMA-376 Part 1, §17.16.22) with this target from
    /// its relationship and this anchor goes. See [`shared_target`](Self::shared_target).
    pub(super) fn link_target(
        &mut self,
        target: Option<&'a str>,
        anchor: Option<&'a str>,
    ) -> Option<Arc<ir::LinkTarget>> {
        self.shared_target(target, anchor)
    }

    /// Where a `HYPERLINK` field (§17.16.5.25) goes: as a hyperlink with its first
    /// argument for a target and its `\l` for an anchor, a hypothesis for a field with
    /// both until a Word render.
    pub(super) fn field_link_target(
        &mut self,
        link: &HyperlinkField,
    ) -> Option<Arc<ir::LinkTarget>> {
        self.shared_target(
            link.target().map(str::to_owned),
            link.bookmark().map(str::to_owned),
        )
    }

    /// Where a link with this target and this anchor goes: the target, with `#` and the
    /// anchor after it where it has both, as Word makes it (MS-OI29500 §17.16.22(b)),
    /// where the standard ignores the anchor; else the bookmark the anchor names. None for
    /// a link with neither, or past the bytes of targets the IR may hold. `w:docLocation`
    /// is not part of it: what Word does with it is not known.
    ///
    /// Every link to one address gets the same target, made once.
    fn shared_target<K>(
        &mut self,
        target: Option<K>,
        anchor: Option<K>,
    ) -> Option<Arc<ir::LinkTarget>>
    where
        K: AsRef<str> + Into<Cow<'a, str>>,
    {
        let target = target.filter(|target| !target.as_ref().is_empty());
        let anchor = anchor.filter(|anchor| !anchor.as_ref().is_empty());
        let (written, place) = (
            target.as_ref().map_or("", AsRef::as_ref),
            anchor.as_ref().map_or("", AsRef::as_ref),
        );
        if let Some(shared) = self
            .link_targets
            .get(written)
            .and_then(|by_anchor| by_anchor.get(place))
        {
            return Some(Arc::clone(shared));
        }
        let made = match (written, place) {
            ("", "") => return None,
            ("", anchor) => ir::LinkTarget::Bookmark(anchor.to_owned()),
            (target, "") => ir::LinkTarget::Uri(target.to_owned()),
            (target, anchor) => ir::LinkTarget::Uri(format!("{target}#{anchor}")),
        };
        let length = match &made {
            ir::LinkTarget::Uri(text) | ir::LinkTarget::Bookmark(text) => text.len(),
            _ => 0,
        };
        if self.link_bytes.saturating_add(length) > MAX_LINK_BYTES {
            self.report_once(
                Once::LinkBytes,
                "links past 32 MiB of web addresses and other targets are left out, here and wherever else the document has one",
            );
            return None;
        }
        self.link_bytes += length;
        let shared = Arc::new(made);
        self.link_targets
            .entry(target.map_or(Cow::Borrowed(""), Into::into))
            .or_default()
            .insert(
                anchor.map_or(Cow::Borrowed(""), Into::into),
                Arc::clone(&shared),
            );
        Some(shared)
    }
}

/// The IR's mark for the start of a link to `target`, which it shares.
pub(super) fn link_start(target: &Arc<ir::LinkTarget>) -> ir::Inline {
    ir::Inline::link_start(Arc::clone(target))
}
