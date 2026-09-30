//! Keeps the Grid/Loupe/filmstrip's photo list in sync with the catalog:
//! reloads on a source change (plan §7.2's Catalog/Folders panels) or
//! whenever a `CatalogEvent` says the set of photos changed (plan §4.5).

use std::collections::BTreeSet;

use crossbeam_channel::Receiver;
use viberoom_core::events::CatalogEvent;
use viberoom_services::collections::{self, CollectionRow, PhotoSource};
use viberoom_services::repo::{self, FolderRow, PhotoSort, PhotoSummary};
use viberoom_shell::{AppCx, LibrarySource};

use crate::filter_bar::FilterState;

#[derive(Debug, Default)]
pub struct LibraryData {
    events_rx: Option<Receiver<CatalogEvent>>,
    last_source: Option<LibrarySource>,
    pub photos: Vec<PhotoSummary>,
    pub folders: Vec<FolderRow>,
    pub collections: Vec<CollectionRow>,
    pub filter: FilterState,
    last_filter: Option<FilterState>,
    pub latest_import: Option<viberoom_core::ids::ImportId>,
    pub total_photo_count: i64,
    pub sort: PhotoSort,
    /// Bumped every time a drained `CatalogEvent` was a `PhotosChanged`:
    /// the right panel's caches (plan §7.6) re-read the catalog only when
    /// this or the selection moves, never per frame.
    pub metadata_dirty: u64,
}

impl LibraryData {
    /// Call once per frame, before reading `photos`/`folders`. Subscribes
    /// to catalog events on first use (deferred because `AppCx` — and so
    /// the event bus — doesn't exist until the module is constructed by
    /// `app`, not at `LibraryModule::new()` time).
    pub fn sync(&mut self, cx: &mut AppCx) {
        if self.events_rx.is_none() {
            self.events_rx = Some(cx.events.subscribe());
        }

        let mut needs_refresh = self.last_source != Some(cx.selection.source)
            || self.last_filter.as_ref() != Some(&self.filter);
        if let Some(rx) = &self.events_rx {
            while let Ok(ev) = rx.try_recv() {
                // `PhotosChanged` also bumps `metadata_dirty`: a photo's
                // rating/keywords/IPTC may have moved under the right
                // panel's feet (plan §7.6's cached reads).
                let metadata_changed = matches!(ev, CatalogEvent::PhotosChanged { .. });
                self.metadata_dirty += u64::from(metadata_changed);
                needs_refresh |= matches!(
                    ev,
                    CatalogEvent::PhotosAdded { .. }
                        | CatalogEvent::PhotosRemoved { .. }
                        | CatalogEvent::PhotosChanged { .. }
                        | CatalogEvent::ImportFinished { .. }
                        | CatalogEvent::CollectionsChanged { .. }
                );
            }
        }

        if needs_refresh {
            self.refresh(cx);
            self.last_source = Some(cx.selection.source);
            self.last_filter = Some(self.filter.clone());
        }
    }

    pub fn refresh(&mut self, cx: &mut AppCx) {
        let Some(catalog) = &cx.catalog else {
            self.photos.clear();
            self.folders.clear();
            self.collections.clear();
            return;
        };
        let conn = catalog.connection();

        let source = match cx.selection.source {
            LibrarySource::AllPhotographs => PhotoSource::All,
            LibrarySource::Folder(id) => PhotoSource::Folder(id),
            LibrarySource::Import(id) => PhotoSource::Import(id),
            LibrarySource::Collection(id) => PhotoSource::Collection(id),
        };
        let photos = collections::list_photos(conn, &source, &self.filter.rules(), true, self.sort);
        match photos {
            Ok(photos) => self.photos = photos,
            Err(e) => {
                tracing::error!(error = %e, "failed to list photos");
                self.photos.clear();
            }
        }

        match repo::list_folders(conn) {
            Ok(folders) => self.folders = folders,
            Err(e) => tracing::error!(error = %e, "failed to list folders"),
        }

        match collections::list_collections(conn) {
            Ok(rows) => self.collections = rows,
            Err(e) => tracing::error!(error = %e, "failed to list collections"),
        }

        self.latest_import = repo::latest_import_id(conn).unwrap_or_default();
        self.total_photo_count = repo::count_all_photos(conn).unwrap_or_default();

        let still_present: BTreeSet<_> = self.photos.iter().map(|p| p.photo_id).collect();
        cx.selection.visible = self.photos.iter().map(|p| p.photo_id).collect();
        cx.selection.retain(&still_present);
    }

    pub fn ordered_ids(&self) -> Vec<viberoom_core::ids::PhotoId> {
        self.photos.iter().map(|p| p.photo_id).collect()
    }
}
