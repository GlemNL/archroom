//! Library selection state (plan §4.5, §7.4): the active photo, the
//! selected set, and which source (All Photographs, a folder, Previous
//! Import…) the Grid is currently showing. Shared by the Grid, filmstrip
//! and Loupe so they always agree on what's selected.

use std::collections::BTreeSet;

use viberoom_core::ids::{CollectionId, FolderId, ImportId, PhotoId};

/// What the Grid/filmstrip are currently listing (plan §7.2's Catalog and
/// Folders panels). Collections join once M2 builds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibrarySource {
    #[default]
    AllPhotographs,
    Folder(FolderId),
    Import(ImportId),
    Collection(CollectionId),
}

#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub source: LibrarySource,
    pub active: Option<PhotoId>,
    /// The photos the Grid currently lists, in order — set by Library so
    /// Develop can step to the next/previous photo.
    pub visible: Vec<PhotoId>,
    selected: BTreeSet<PhotoId>,
}

impl Selection {
    pub fn is_selected(&self, id: PhotoId) -> bool {
        self.selected.contains(&id)
    }

    pub fn selected(&self) -> impl Iterator<Item = PhotoId> + '_ {
        self.selected.iter().copied()
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// A plain click: replaces the selection with just `id`.
    pub fn select_single(&mut self, id: PhotoId) {
        self.selected.clear();
        self.selected.insert(id);
        self.active = Some(id);
    }

    /// A ctrl/cmd-click: toggles `id`'s membership without touching the
    /// rest of the selection.
    pub fn toggle(&mut self, id: PhotoId) {
        if !self.selected.remove(&id) {
            self.selected.insert(id);
            self.active = Some(id);
        } else if self.active == Some(id) {
            self.active = self.selected.iter().next_back().copied();
        }
    }

    /// A shift-click: selects the contiguous run between the current
    /// active photo and `to` within `ordered` (the Grid's current, sorted
    /// list of ids). Falls back to [`select_single`] if there's no active
    /// photo to range from.
    pub fn select_range(&mut self, ordered: &[PhotoId], to: PhotoId) {
        let Some(active) = self.active else {
            self.select_single(to);
            return;
        };
        let (Some(from_idx), Some(to_idx)) = (
            ordered.iter().position(|&id| id == active),
            ordered.iter().position(|&id| id == to),
        ) else {
            self.select_single(to);
            return;
        };
        let (lo, hi) = (from_idx.min(to_idx), from_idx.max(to_idx));
        self.selected.extend(ordered[lo..=hi].iter().copied());
        self.active = Some(to);
    }

    pub fn select_all(&mut self, ordered: &[PhotoId]) {
        self.selected = ordered.iter().copied().collect();
        if self.active.is_none() {
            self.active = ordered.first().copied();
        }
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.active = None;
    }

    /// Called when the Grid's source or contents change (a new import, a
    /// folder switch): drops any selected ids that are no longer listed.
    pub fn retain(&mut self, still_present: &BTreeSet<PhotoId>) {
        self.selected.retain(|id| still_present.contains(id));
        if self.active.is_some_and(|id| !still_present.contains(&id)) {
            self.active = self.selected.iter().next().copied();
        }
    }

    /// Moves the active photo by `delta` positions within `ordered` and
    /// replaces the selection with just that photo — Loupe's previous/next
    /// (plan §7.3) and the Grid's `Shift+`rating/flag auto-advance (§7.4)
    /// share this.
    pub fn advance(&mut self, ordered: &[PhotoId], delta: isize) {
        if ordered.is_empty() {
            return;
        }
        let Some(active) = self.active else {
            self.select_single(ordered[0]);
            return;
        };
        let Some(idx) = ordered.iter().position(|&id| id == active) else {
            return;
        };
        let new_idx = (idx as isize + delta).clamp(0, ordered.len() as isize - 1) as usize;
        self.select_single(ordered[new_idx]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn ids(n: i64) -> Vec<PhotoId> {
        (1..=n).map(PhotoId::new).collect()
    }

    #[test]
    fn select_single_replaces_the_selection() {
        let mut sel = Selection::default();
        sel.select_single(PhotoId::new(1));
        sel.select_single(PhotoId::new(2));
        assert_eq!(sel.selected().collect::<Vec<_>>(), vec![PhotoId::new(2)]);
        assert_eq!(sel.active, Some(PhotoId::new(2)));
    }

    #[test]
    fn toggle_adds_and_removes() {
        let mut sel = Selection::default();
        sel.toggle(PhotoId::new(1));
        sel.toggle(PhotoId::new(2));
        assert_eq!(sel.selected_count(), 2);
        sel.toggle(PhotoId::new(1));
        assert_eq!(sel.selected().collect::<Vec<_>>(), vec![PhotoId::new(2)]);
    }

    #[test]
    fn select_range_selects_the_contiguous_span() {
        let ordered = ids(5);
        let mut sel = Selection::default();
        sel.select_single(ordered[1]); // id 2
        sel.select_range(&ordered, ordered[3]); // id 4

        let mut got = sel.selected().collect::<Vec<_>>();
        got.sort();
        assert_eq!(got, vec![PhotoId::new(2), PhotoId::new(3), PhotoId::new(4)]);
        assert_eq!(sel.active, Some(PhotoId::new(4)));
    }

    #[test]
    fn select_range_with_no_active_falls_back_to_single() {
        let ordered = ids(3);
        let mut sel = Selection::default();
        sel.select_range(&ordered, ordered[1]);
        assert_eq!(sel.selected().collect::<Vec<_>>(), vec![PhotoId::new(2)]);
    }

    #[test]
    fn advance_clamps_at_the_ends() {
        let ordered = ids(3);
        let mut sel = Selection::default();
        sel.select_single(ordered[0]);
        sel.advance(&ordered, -1);
        assert_eq!(
            sel.active,
            Some(ordered[0]),
            "must not go below the first photo"
        );

        sel.advance(&ordered, 1);
        sel.advance(&ordered, 1);
        sel.advance(&ordered, 1);
        assert_eq!(
            sel.active,
            Some(ordered[2]),
            "must not go past the last photo"
        );
    }

    #[test]
    fn retain_drops_ids_no_longer_present() {
        let mut sel = Selection::default();
        sel.select_all(&ids(3));
        let still: BTreeSet<PhotoId> = [PhotoId::new(2)].into_iter().collect();
        sel.retain(&still);
        assert_eq!(sel.selected().collect::<Vec<_>>(), vec![PhotoId::new(2)]);
        assert_eq!(sel.active, Some(PhotoId::new(2)));
    }
}
