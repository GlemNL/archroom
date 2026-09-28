//! Keeps the Grid/Loupe/filmstrip's photo list in sync with the catalog:
//! reloads on a source change (plan §7.2's Catalog/Folders panels) or
//! whenever a `CatalogEvent` says the set of photos changed (plan §4.5).

use std::collections::BTreeSet;

use archroom_core::events::CatalogEvent;
use archroom_services::repo::{self, FolderRow, PhotoSort, PhotoSummary};
use archroom_shell::{AppCx, LibrarySource};
use crossbeam_channel::Receiver;

#[derive(Debug, Default)]
pub struct LibraryData {
    events_rx: Option<Receiver<CatalogEvent>>,
    last_source: Option<LibrarySource>,
    pub photos: Vec<PhotoSummary>,
    pub folders: Vec<FolderRow>,
    pub latest_import: Option<archroom_core::ids::ImportId>,
    pub total_photo_count: i64,
    pub sort: PhotoSort,
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

        let mut needs_refresh = self.last_source != Some(cx.selection.source);
        if let Some(rx) = &self.events_rx {
            while let Ok(ev) = rx.try_recv() {
                needs_refresh |= matches!(
                    ev,
                    CatalogEvent::PhotosAdded { .. }
                        | CatalogEvent::PhotosRemoved { .. }
                        | CatalogEvent::PhotosChanged { .. }
                        | CatalogEvent::ImportFinished { .. }
                );
            }
        }

        if needs_refresh {
            self.refresh(cx);
            self.last_source = Some(cx.selection.source);
        }
    }

    pub fn refresh(&mut self, cx: &mut AppCx) {
        let Some(catalog) = &cx.catalog else {
            self.photos.clear();
            self.folders.clear();
            return;
        };
        let conn = catalog.connection();

        let photos = match cx.selection.source {
            LibrarySource::AllPhotographs => repo::list_all_photos(conn, self.sort),
            LibrarySource::Folder(id) => repo::list_photos_for_folder(conn, id, self.sort),
            LibrarySource::Import(id) => repo::list_photos_for_import(conn, id, self.sort),
        };
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

        self.latest_import = repo::latest_import_id(conn).unwrap_or_default();
        self.total_photo_count = repo::count_all_photos(conn).unwrap_or_default();

        let still_present: BTreeSet<_> = self.photos.iter().map(|p| p.photo_id).collect();
        cx.selection.retain(&still_present);
    }
}
