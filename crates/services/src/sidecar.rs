//! XMP sidecar sync (plan §5.3, M2): the bridge between the catalog — the
//! source of truth (plan principle 2) — and the `.xmp` files other apps
//! (Lightroom, darktable, digiKam) read. `io::xmp` does the byte-level
//! read-modify-write and preserves foreign fields; this module decides
//! *what* to write (catalog rows), *when* (`Ctrl+S`, the auto-write
//! preference, import) and *which file* (`IMG_0001.xmp`, or
//! `IMG_0001.CR3.xmp` for a RAW+JPEG pair, plan D5).

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use viberoom_catalog::Catalog;
use viberoom_catalog::repo;
use viberoom_core::ids::PhotoId;
use viberoom_io::xmp::{self, SidecarData};
use viberoom_jobs::{Job, JobContext, Priority};

use crate::error::Result;

/// The sidecar path for one photo's snapshot: the Adobe-style
/// `IMG_0001.xmp` next to the image, or the full-extension fallback when
/// another catalogued file in the same folder shares the basename (a
/// RAW+JPEG pair) and the plain name would collide (plan D5).
pub(crate) fn sidecar_path_for(snap: &repo::PhotoXmpSnapshot) -> PathBuf {
    if snap.same_stem_sibling {
        xmp::sidecar_path_with_full_extension(&snap.image_path)
    } else {
        xmp::sidecar_path(&snap.image_path)
    }
}

/// Writes one photo's catalog metadata to its sidecar (plan §5.3's
/// mapping) and records the new `files.sidecar_mtime`. Photos that no
/// longer exist are silently skipped — callers batch whole selections and
/// shouldn't care about one stale id.
pub fn save_one(conn: &Connection, photo_id: PhotoId) -> Result<()> {
    let Some(snap) = repo::photo_xmp_snapshot(conn, photo_id)? else {
        return Ok(());
    };

    let path = sidecar_path_for(&snap);
    let data = SidecarData {
        rating: Some(snap.rating),
        flag: Some(snap.flag),
        color_label: snap.color_label.clone(),
        keywords: snap.keywords.clone(),
        title: snap.title.clone(),
        caption: snap.caption.clone(),
        creator: snap.creator.clone(),
        copyright: snap.copyright.clone(),
    };
    xmp::write_sidecar(&path, &data)?;

    let mtime = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    Ok(repo::set_sidecar_mtime(conn, snap.file_id, mtime)?)
}

/// Saves sidecars for every photo in `photo_ids`, skipping (with a
/// warning) the ones that fail — one unreadable file must not block the
/// rest of a selection. Returns how many were written.
pub fn save_metadata_to_xmp(conn: &Connection, photo_ids: &[PhotoId]) -> Result<usize> {
    let mut written = 0;
    for &photo_id in photo_ids {
        match save_one(conn, photo_id) {
            Ok(()) => written += 1,
            Err(e) => tracing::warn!(photo = photo_id.get(), error = %e, "xmp save failed"),
        }
    }
    Ok(written)
}

/// Import time (plan §5.3): if a sidecar already sits next to the image —
/// written by Lightroom, darktable or digiKam — read it and apply its
/// metadata to the freshly imported photo. Returns whether a sidecar was
/// found. Sidecar keywords come back as flat `dc:subject` names, so they
/// attach as one-segment paths.
pub fn apply_sidecar_on_import(
    conn: &Connection,
    photo_id: PhotoId,
    file_id: viberoom_core::ids::FileId,
    image_path: &Path,
) -> Result<bool> {
    let candidates = [
        xmp::sidecar_path(image_path),
        xmp::sidecar_path_with_full_extension(image_path),
    ];
    let Some(sidecar) = candidates.iter().find(|p| p.exists()) else {
        return Ok(false);
    };

    let data = xmp::read_sidecar(sidecar)?;
    repo::apply_sidecar_metadata(
        conn,
        photo_id,
        repo::SidecarMetadata {
            rating: data.rating.unwrap_or(0),
            flag: data.flag.unwrap_or(0),
            color_label: data.color_label.as_deref(),
            title: data.title.as_deref(),
            caption: data.caption.as_deref(),
            creator: data.creator.as_deref(),
            copyright: data.copyright.as_deref(),
        },
    )?;
    for name in &data.keywords {
        let segments = repo::split_keyword_path(name);
        if segments.is_empty() {
            continue;
        }
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        let keyword_id = repo::upsert_keyword_path(conn, &refs)?;
        repo::add_photo_keyword(conn, photo_id, keyword_id)?;
    }

    let mtime = std::fs::metadata(sidecar)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    repo::set_sidecar_mtime(conn, file_id, mtime)?;
    Ok(true)
}

/// Runs [`save_metadata_to_xmp`] as an `viberoom-jobs` background job
/// (plan §4.6: XMP writes never block the UI thread). `Ctrl+S` and the
/// auto-write preference both submit this; it opens its own connection to
/// the catalog file, like the import job does.
#[derive(Debug)]
pub struct SaveXmpJob {
    catalog_path: PathBuf,
    photo_ids: Vec<PhotoId>,
}

impl SaveXmpJob {
    pub fn new(catalog_path: PathBuf, photo_ids: Vec<PhotoId>) -> Self {
        Self {
            catalog_path,
            photo_ids,
        }
    }
}

impl Job for SaveXmpJob {
    fn label(&self) -> String {
        format!(
            "Saving metadata to XMP ({} photo{})",
            self.photo_ids.len(),
            if self.photo_ids.len() == 1 { "" } else { "s" }
        )
    }

    fn priority(&self) -> Priority {
        Priority::UserBatch
    }

    fn run(self: Box<Self>, _cx: &JobContext) {
        let catalog = match Catalog::create_or_open(&self.catalog_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "xmp save job: could not open catalog");
                return;
            }
        };
        match save_metadata_to_xmp(catalog.connection(), &self.photo_ids) {
            Ok(n) => tracing::debug!(written = n, "xmp save job finished"),
            Err(e) => tracing::error!(error = %e, "xmp save job failed"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use viberoom_catalog::repo;
    use viberoom_core::ids::FileId;

    /// A photo whose file lives in `dir`, so sidecars land next to it.
    /// The image itself doesn't need to exist — sidecars are written from
    /// catalog rows, not by touching the original (plan principle 1).
    fn seed_photo_in(catalog: &Catalog, dir: &Path, filename: &str) -> (PhotoId, FileId) {
        let conn = catalog.connection();
        let folder = repo::upsert_folder_path(conn, dir).unwrap();
        let file_id = repo::insert_file(
            conn,
            &repo::NewFile {
                folder_id: folder,
                filename,
                ext: "jpg",
                kind: "jpeg",
                ..Default::default()
            },
        )
        .unwrap();
        let photo_id = repo::insert_photo(
            conn,
            &repo::NewPhoto {
                file_id,
                import_id: None,
            },
        )
        .unwrap();
        (photo_id, file_id)
    }

    fn open_catalog(dir: &Path) -> Catalog {
        Catalog::create_or_open(dir.join("T.arcat")).unwrap()
    }

    #[test]
    fn save_one_writes_the_sidecar_and_records_its_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = open_catalog(dir.path());
        let (photo, file_id) = seed_photo_in(&catalog, dir.path(), "img.jpg");
        let conn = catalog.connection();
        conn.execute(
            "UPDATE photos SET rating = 4, title = 'Title', creator = 'Clem' WHERE id = ?1",
            rusqlite::params![photo.get()],
        )
        .unwrap();
        let kw = repo::upsert_keyword(conn, None, "Sunset").unwrap();
        repo::add_photo_keyword(conn, photo, kw).unwrap();

        save_one(conn, photo).unwrap();

        let sidecar = dir.path().join("img.xmp");
        let data = xmp::read_sidecar(&sidecar).unwrap();
        assert_eq!(data.rating, Some(4));
        assert_eq!(data.title.as_deref(), Some("Title"));
        assert_eq!(data.creator.as_deref(), Some("Clem"));
        assert_eq!(data.keywords, vec!["Sunset"]);

        let mtime: Option<i64> = conn
            .query_row(
                "SELECT sidecar_mtime FROM files WHERE id = ?1",
                rusqlite::params![file_id.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            mtime.is_some(),
            "the write must be recorded for folder sync"
        );
    }

    #[test]
    fn save_one_uses_the_full_extension_name_for_a_raw_jpeg_pair() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = open_catalog(dir.path());
        // Same basename, one raw one jpeg — plan D5's collision case.
        let (raw_photo, _raw_file) = seed_photo_in(&catalog, dir.path(), "img.CR3");
        seed_photo_in(&catalog, dir.path(), "img.jpg");

        save_one(catalog.connection(), raw_photo).unwrap();

        assert!(dir.path().join("img.CR3.xmp").exists());
        assert!(!dir.path().join("img.xmp").exists());
    }

    #[test]
    fn apply_sidecar_on_import_brings_metadata_and_keywords_into_the_catalog() {
        let dir = tempfile::tempdir().unwrap();

        // A sidecar some other app (or a previous Viberoom session) left
        // behind, checked first so the reader can't echo our own writes.
        xmp::write_sidecar(
            &dir.path().join("img.xmp"),
            &SidecarData {
                rating: Some(5),
                flag: Some(1),
                color_label: Some("Red".into()),
                keywords: vec!["Paris".into(), "Places > France".into()],
                title: Some("Tour Eiffel".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let catalog = open_catalog(dir.path());
        let (photo, file) = seed_photo_in(&catalog, dir.path(), "img.jpg");
        let conn = catalog.connection();

        assert!(apply_sidecar_on_import(conn, photo, file, &dir.path().join("img.jpg")).unwrap());

        let iptc = repo::photo_iptc(conn, photo).unwrap();
        assert_eq!(iptc.title.as_deref(), Some("Tour Eiffel"));
        let rating: i32 = conn
            .query_row(
                "SELECT rating FROM photos WHERE id = ?1",
                rusqlite::params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rating, 5);
        let mut paths = repo::keyword_paths_for_photo(conn, photo).unwrap();
        paths.sort();
        assert_eq!(paths, vec!["Paris", "Places > France"]);
    }

    #[test]
    fn apply_sidecar_on_import_is_a_noop_without_a_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = open_catalog(dir.path());
        let (photo, file) = seed_photo_in(&catalog, dir.path(), "img.jpg");

        assert!(
            !apply_sidecar_on_import(
                catalog.connection(),
                photo,
                file,
                &dir.path().join("img.jpg"),
            )
            .unwrap()
        );
    }
}
