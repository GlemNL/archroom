//! Library file operations that must never destroy data:
//! moving originals to the freedesktop trash, and the missing-file scan.
//!
//! There is no permanent delete anywhere in here. A trashed file can always
//! be restored from the file manager, and [`TrashPhotos`] restores it itself
//! on undo.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use std::sync::Arc;
use viberoom_catalog::Catalog;
use viberoom_catalog::Result as CatalogResult;
use viberoom_catalog::command::{Command, Removal, RemovePhotos};
use viberoom_catalog::repo;
use viberoom_core::events::EventBus;
use viberoom_core::events::{CatalogEvent, PhotoField};
use viberoom_core::ids::{FolderId, PhotoId};
use viberoom_jobs::{Job, JobContext, Priority};

use crate::error::{Error, Result};
use crate::sidecar::sidecar_path_for;

// ---------------------------------------------------------------------
// Trash backend
// ---------------------------------------------------------------------

/// Where a trashed file went, enough to bring it back.
#[derive(Debug, Clone)]
pub struct TrashToken {
    pub original: PathBuf,
    pub trashed_at: i64,
}

/// Seam over the trash so tests (and a future non-freedesktop platform) can
/// substitute their own. The only implementations *move* files; none deletes.
pub trait TrashBackend: Send {
    fn trash(&mut self, path: &Path) -> Result<TrashToken>;
    fn restore(&mut self, token: &TrashToken) -> Result<()>;
}

/// The freedesktop trash (`trash` crate).
#[derive(Debug, Default)]
pub struct SystemTrash;

impl TrashBackend for SystemTrash {
    fn trash(&mut self, path: &Path) -> Result<TrashToken> {
        trash::delete(path).map_err(|e| {
            Error::Other(format!(
                "could not move {} to the trash: {e}",
                path.display()
            ))
        })?;
        let trashed_at = trash::os_limited::list()
            .ok()
            .and_then(|items| {
                items
                    .into_iter()
                    .filter(|i| i.original_path() == path)
                    .map(|i| i.time_deleted)
                    .max()
            })
            .unwrap_or(0);
        Ok(TrashToken {
            original: path.to_path_buf(),
            trashed_at,
        })
    }

    fn restore(&mut self, token: &TrashToken) -> Result<()> {
        let items = trash::os_limited::list()
            .map_err(|e| Error::Other(format!("could not read the trash: {e}")))?;
        let item = items
            .into_iter()
            .filter(|i| i.original_path() == token.original)
            .max_by_key(|i| (i.time_deleted == token.trashed_at, i.time_deleted))
            .ok_or_else(|| {
                Error::Other(format!(
                    "{} is no longer in the trash",
                    token.original.display()
                ))
            })?;
        trash::os_limited::restore_all([item]).map_err(|e| {
            Error::Other(format!(
                "could not restore {}: {e}",
                token.original.display()
            ))
        })
    }
}

// ---------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------

/// What trashing a set of photos would do, for the confirmation dialog.
#[derive(Debug, Clone, Default)]
pub struct TrashPlan {
    pub photos: Vec<PhotoId>,
    /// Existing files (originals and their sidecars) that would move to the trash.
    pub files: Vec<PathBuf>,
    /// Originals that stay because another catalog photo (a virtual copy
    /// outside the selection) still uses them.
    pub kept_shared: usize,
    /// Originals that are already gone from disk (nothing to trash).
    pub already_missing: usize,
}

/// Works out which files can go to the trash with `ids`. A file is only
/// trashed when *every* live photo that uses it is in `ids`.
pub fn plan_trash(conn: &Connection, ids: &[PhotoId]) -> Result<TrashPlan> {
    let mut plan = TrashPlan {
        photos: ids.to_vec(),
        ..TrashPlan::default()
    };
    let mut seen = std::collections::HashSet::new();
    for &id in ids {
        let Some(snap) = repo::photo_xmp_snapshot(conn, id)? else {
            continue;
        };
        if !seen.insert(snap.file_id.get()) {
            continue;
        }
        let others: i64 = {
            let list = ids
                .iter()
                .map(|i| i.get().to_string())
                .collect::<Vec<_>>()
                .join(",");
            conn.query_row(
                &format!(
                    "SELECT count(*) FROM photos
                     WHERE file_id = ?1 AND removed_at IS NULL AND id NOT IN ({list})"
                ),
                params![snap.file_id.get()],
                |r| r.get(0),
            )
            .map_err(viberoom_catalog::Error::from)?
        };
        if others > 0 {
            plan.kept_shared += 1;
            continue;
        }
        if !snap.image_path.exists() {
            plan.already_missing += 1;
            continue;
        }
        plan.files.push(snap.image_path.clone());
        let mut sidecars = vec![viberoom_io::xmp::sidecar_path_with_full_extension(
            &snap.image_path,
        )];
        sidecars.push(sidecar_path_for(&snap));
        sidecars.dedup();
        for s in sidecars {
            if s.exists() && !plan.files.contains(&s) {
                plan.files.push(s);
            }
        }
    }
    Ok(plan)
}

// ---------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------

/// Moves the photos' originals (and sidecars) to the trash, then soft-deletes
/// the photos. Files first: if any move fails, the files already moved are
/// put back and the catalog is left exactly as it was. Undo restores the
/// files, then the photos.
pub struct TrashPhotos {
    inner: RemovePhotos,
    files: Vec<PathBuf>,
    backend: Box<dyn TrashBackend>,
    moved: Vec<TrashToken>,
}

impl std::fmt::Debug for TrashPhotos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrashPhotos")
            .field("files", &self.files)
            .finish_non_exhaustive()
    }
}

impl TrashPhotos {
    pub fn new(plan: TrashPlan, backend: Box<dyn TrashBackend>) -> Self {
        Self {
            inner: RemovePhotos::new(plan.photos, Removal::Trash),
            files: plan.files,
            backend,
            moved: Vec::new(),
        }
    }

    /// Files currently in the trash because of this command.
    pub fn trashed(&self) -> &[TrashToken] {
        &self.moved
    }
}

fn other(msg: String) -> viberoom_catalog::Error {
    viberoom_catalog::Error::Other(msg)
}

impl Command for TrashPhotos {
    fn label(&self) -> String {
        self.inner.label()
    }

    fn apply(&mut self, conn: &Connection) -> CatalogResult<CatalogEvent> {
        self.moved.clear();
        for path in &self.files {
            match self.backend.trash(path) {
                Ok(token) => self.moved.push(token),
                Err(e) => {
                    for token in std::mem::take(&mut self.moved).iter().rev() {
                        if let Err(re) = self.backend.restore(token) {
                            tracing::error!(error = %re, "rollback: could not put a file back");
                        }
                    }
                    return Err(other(e.to_string()));
                }
            }
        }
        match self.inner.apply(conn) {
            Ok(ev) => Ok(ev),
            Err(e) => {
                for token in std::mem::take(&mut self.moved).iter().rev() {
                    if let Err(re) = self.backend.restore(token) {
                        tracing::error!(error = %re, "rollback: could not put a file back");
                    }
                }
                Err(e)
            }
        }
    }

    fn revert(&mut self, conn: &Connection) -> CatalogResult<CatalogEvent> {
        // Best effort: a file that was emptied from the trash meanwhile just
        // shows as missing (Locate… can relink it); the photo itself, with
        // all its edits, always comes back.
        for token in std::mem::take(&mut self.moved).iter().rev() {
            if let Err(e) = self.backend.restore(token) {
                tracing::warn!(error = %e, "undo: could not restore a trashed file");
            }
        }
        self.inner.revert(conn)
    }
}

// ---------------------------------------------------------------------
// Missing files
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum MissingScope<'a> {
    All,
    Folder(FolderId),
    Photos(&'a [PhotoId]),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingReport {
    pub checked: usize,
    pub now_missing: Vec<PhotoId>,
    pub restored: Vec<PhotoId>,
}

impl MissingReport {
    pub fn changed(&self) -> bool {
        !self.now_missing.is_empty() || !self.restored.is_empty()
    }

    /// The event the UI needs to refresh badges, if anything changed.
    pub fn event(&self) -> Option<CatalogEvent> {
        self.changed().then(|| CatalogEvent::PhotosChanged {
            ids: self
                .now_missing
                .iter()
                .chain(&self.restored)
                .copied()
                .collect(),
            fields: vec![PhotoField::Missing],
        })
    }
}

/// `stat`s the originals in `scope` and sets `files.missing` to match what is
/// on disk. Only that one column is ever written: nothing is deleted or
/// moved, and a file that reappears (a drive remounted) clears its own flag.
/// `cancelled` is polled between files.
pub fn check_missing(
    conn: &Connection,
    scope: MissingScope<'_>,
    cancelled: &dyn Fn() -> bool,
) -> Result<MissingReport> {
    let base = "SELECT f.id, f.missing, fo.path, f.filename FROM files f
                JOIN folders fo ON fo.id = f.folder_id
                WHERE EXISTS (SELECT 1 FROM photos p WHERE p.file_id = f.id AND p.removed_at IS NULL)";
    let sql = match scope {
        MissingScope::All => base.to_string(),
        MissingScope::Folder(id) => format!("{base} AND f.folder_id = {}", id.get()),
        MissingScope::Photos(ids) => {
            let list = ids
                .iter()
                .map(|i| i.get().to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!("{base} AND f.id IN (SELECT file_id FROM photos WHERE id IN ({list}))")
        }
    };
    let files: Vec<(i64, bool, PathBuf)> = {
        let mut stmt = conn.prepare(&sql).map_err(viberoom_catalog::Error::from)?;
        let rows = stmt
            .query_map([], |r| {
                let folder: String = r.get(2)?;
                let name: String = r.get(3)?;
                Ok((
                    r.get(0)?,
                    r.get::<_, i64>(1)? != 0,
                    Path::new(&folder).join(name),
                ))
            })
            .map_err(viberoom_catalog::Error::from)?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(viberoom_catalog::Error::from)?
    };

    let mut report = MissingReport::default();
    let mut changes: Vec<(i64, bool)> = Vec::new();
    for (file_id, was_missing, path) in files {
        if cancelled() {
            break;
        }
        report.checked += 1;
        // `try_exists` distinguishes "not there" from "could not tell"
        // (permissions, a dropped network share): only a definite "not
        // there" flags a file, so an unreachable drive is never *cleared*
        // wrongly and a flaky stat never flags a good file.
        let now_missing = match path.try_exists() {
            Ok(exists) => !exists,
            Err(_) => continue,
        };
        if now_missing != was_missing {
            changes.push((file_id, now_missing));
        }
    }
    for (file_id, missing) in changes {
        conn.execute(
            "UPDATE files SET missing = ?2 WHERE id = ?1",
            params![file_id, missing as i64],
        )
        .map_err(viberoom_catalog::Error::from)?;
        let photos: Vec<PhotoId> = {
            let mut stmt = conn
                .prepare("SELECT id FROM photos WHERE file_id = ?1 AND removed_at IS NULL")
                .map_err(viberoom_catalog::Error::from)?;
            let rows = stmt
                .query_map(params![file_id], |r| Ok(PhotoId::new(r.get(0)?)))
                .map_err(viberoom_catalog::Error::from)?;
            rows.collect::<rusqlite::Result<_>>()
                .map_err(viberoom_catalog::Error::from)?
        };
        if missing {
            report.now_missing.extend(photos);
        } else {
            report.restored.extend(photos);
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------
// Locate… (relink)
// ---------------------------------------------------------------------

struct FileRow {
    file_id: i64,
    ext: String,
    size: Option<i64>,
    hash: Option<Vec<u8>>,
}

fn file_row(conn: &Connection, photo: PhotoId) -> Result<FileRow> {
    conn.query_row(
        "SELECT f.id, f.ext, f.size, f.quick_hash FROM photos p JOIN files f ON f.id = p.file_id
         WHERE p.id = ?1",
        params![photo.get()],
        |r| {
            Ok(FileRow {
                file_id: r.get(0)?,
                ext: r.get(1)?,
                size: r.get(2)?,
                hash: r.get(3)?,
            })
        },
    )
    .map_err(|e| viberoom_catalog::Error::from(e).into())
}

/// Reasons `candidate` might not be the photo's file. Empty means it looks
/// right (same type, size and first-64-KiB hash). The UI shows these and lets
/// the user relink anyway: a re-saved copy legitimately differs.
pub fn relink_problems(conn: &Connection, photo: PhotoId, candidate: &Path) -> Result<Vec<String>> {
    let FileRow {
        ext, size, hash, ..
    } = file_row(conn, photo)?;
    let mut problems = Vec::new();
    let meta = std::fs::metadata(candidate).map_err(|e| Error::io(candidate, e))?;
    if !meta.is_file() {
        problems.push("That is not a file.".into());
        return Ok(problems);
    }
    let cand_ext = candidate
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if cand_ext != ext.to_lowercase() {
        problems.push(format!(
            "The file type differs (.{cand_ext}, expected .{ext})."
        ));
    }
    if let Some(size) = size
        && size as u64 != meta.len()
    {
        problems.push("The file size differs from the original.".into());
    }
    if let Some(hash) = hash
        && crate::import::quick_hash_of(candidate)? != hash
    {
        problems.push("The file contents differ from the original.".into());
    }
    Ok(problems)
}

fn mtime_secs(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// Points the photo's `files` row at `candidate` and clears its missing flag.
/// Nothing else changes (edits, keywords, ratings stay), and no file is
/// touched. Fails, changing nothing, if `candidate` is already another
/// catalogued file. Returns every photo that uses the file (virtual copies too).
pub fn relink(conn: &Connection, photo: PhotoId, candidate: &Path) -> Result<Vec<PhotoId>> {
    let file_id = file_row(conn, photo)?.file_id;
    let (Some(parent), Some(name)) = (candidate.parent(), candidate.file_name()) else {
        return Err(Error::Other("that path has no folder or file name".into()));
    };
    let name = name.to_string_lossy();
    let meta = std::fs::metadata(candidate).map_err(|e| Error::io(candidate, e))?;
    let folder = repo::upsert_folder_path(conn, parent)?;
    if let Some(existing) = repo::find_file(conn, folder, &name)?
        && existing.get() != file_id
    {
        return Err(Error::Other(format!(
            "{} is already in the catalog as another photo",
            candidate.display()
        )));
    }
    conn.execute(
        "UPDATE files SET folder_id = ?2, filename = ?3, size = ?4, mtime = ?5, missing = 0
         WHERE id = ?1",
        params![
            file_id,
            folder.get(),
            name.as_ref(),
            meta.len() as i64,
            mtime_secs(&meta)
        ],
    )
    .map_err(viberoom_catalog::Error::from)?;
    photos_of_file(conn, file_id)
}

fn photos_of_file(conn: &Connection, file_id: i64) -> Result<Vec<PhotoId>> {
    let mut stmt = conn
        .prepare("SELECT id FROM photos WHERE file_id = ?1 AND removed_at IS NULL")
        .map_err(viberoom_catalog::Error::from)?;
    let rows = stmt
        .query_map(params![file_id], |r| Ok(PhotoId::new(r.get(0)?)))
        .map_err(viberoom_catalog::Error::from)?;
    Ok(rows
        .collect::<rusqlite::Result<_>>()
        .map_err(viberoom_catalog::Error::from)?)
}

/// "Also locate the other missing files in this folder": relinks every other
/// missing file whose name exists in `folder` with the same size (and hash,
/// when known). Returns the photos relinked.
pub fn relink_missing_in_folder(conn: &Connection, folder: &Path) -> Result<Vec<PhotoId>> {
    let missing: Vec<(PhotoId, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT p.id, f.filename FROM files f JOIN photos p ON p.file_id = f.id
                 WHERE f.missing != 0 AND p.removed_at IS NULL GROUP BY f.id",
            )
            .map_err(viberoom_catalog::Error::from)?;
        let rows = stmt
            .query_map([], |r| Ok((PhotoId::new(r.get(0)?), r.get(1)?)))
            .map_err(viberoom_catalog::Error::from)?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(viberoom_catalog::Error::from)?
    };
    let mut out = Vec::new();
    for (photo, name) in missing {
        let candidate = folder.join(&name);
        if candidate.is_file()
            && relink_problems(conn, photo, &candidate)?.is_empty()
            && let Ok(photos) = relink(conn, photo, &candidate)
        {
            out.extend(photos);
        }
    }
    Ok(out)
}

/// [`check_missing`] over the whole catalog or one folder as a lowest-priority
/// background job; publishes one `PhotosChanged { Missing }` if anything flipped.
/// Opens its own connection, like the other jobs.
#[derive(Debug)]
pub struct CheckMissingJob {
    catalog_path: PathBuf,
    folder: Option<FolderId>,
    events: Arc<EventBus<CatalogEvent>>,
}

impl CheckMissingJob {
    pub fn new(
        catalog_path: PathBuf,
        folder: Option<FolderId>,
        events: Arc<EventBus<CatalogEvent>>,
    ) -> Self {
        Self {
            catalog_path,
            folder,
            events,
        }
    }
}

impl Job for CheckMissingJob {
    fn label(&self) -> String {
        "Checking for missing files".into()
    }

    fn priority(&self) -> Priority {
        Priority::Background
    }

    fn run(self: Box<Self>, cx: &JobContext) {
        let catalog = match Catalog::create_or_open(&self.catalog_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "missing-file check: could not open catalog");
                return;
            }
        };
        let scope = self.folder.map_or(MissingScope::All, MissingScope::Folder);
        match check_missing(catalog.connection(), scope, &|| cx.is_cancelled()) {
            Ok(report) => {
                tracing::debug!(checked = report.checked, "missing-file check finished");
                if let Some(ev) = report.event() {
                    self.events.publish(ev);
                }
            }
            Err(e) => tracing::error!(error = %e, "missing-file check failed"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::{Arc as StdArc, Mutex};
    use viberoom_catalog::command::CreateVirtualCopies;

    /// Moves files into a temp "trash" directory instead of the real one.
    #[derive(Clone)]
    struct FakeTrash {
        dir: PathBuf,
        fail_on: StdArc<Mutex<Option<PathBuf>>>,
    }

    impl TrashBackend for FakeTrash {
        fn trash(&mut self, path: &Path) -> Result<TrashToken> {
            if self.fail_on.lock().unwrap().as_deref() == Some(path) {
                return Err(Error::Other("read-only filesystem".into()));
            }
            let n = std::fs::read_dir(&self.dir).unwrap().count();
            std::fs::rename(
                path,
                self.dir.join(format!(
                    "{n}-{}",
                    path.file_name().unwrap().to_string_lossy()
                )),
            )
            .unwrap();
            Ok(TrashToken {
                original: path.to_path_buf(),
                trashed_at: n as i64,
            })
        }

        fn restore(&mut self, token: &TrashToken) -> Result<()> {
            let name = format!(
                "{}-{}",
                token.trashed_at,
                token.original.file_name().unwrap().to_string_lossy()
            );
            let src = self.dir.join(name);
            if !src.exists() {
                return Err(Error::Other("not in the trash".into()));
            }
            std::fs::rename(src, &token.original).unwrap();
            Ok(())
        }
    }

    struct Fx {
        _dir: tempfile::TempDir,
        photos: PathBuf,
        trash: FakeTrash,
        catalog: Catalog,
    }

    fn fixture(names: &[&str]) -> (Fx, Vec<PhotoId>) {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        let trash_dir = dir.path().join("trash");
        std::fs::create_dir_all(&photos).unwrap();
        std::fs::create_dir_all(&trash_dir).unwrap();
        let catalog = Catalog::create_or_open(dir.path().join("t.arcat")).unwrap();
        let conn = catalog.connection();
        let folder = repo::upsert_folder_path(conn, &photos).unwrap();
        let ids = names
            .iter()
            .map(|n| {
                std::fs::write(photos.join(n), b"pixels").unwrap();
                let ext = Path::new(n).extension().unwrap().to_str().unwrap();
                let file = repo::insert_file(
                    conn,
                    &repo::NewFile {
                        folder_id: folder,
                        filename: n,
                        ext,
                        kind: "jpeg",
                        ..Default::default()
                    },
                )
                .unwrap();
                repo::insert_photo(
                    conn,
                    &repo::NewPhoto {
                        file_id: file,
                        import_id: None,
                    },
                )
                .unwrap()
            })
            .collect();
        let fx = Fx {
            photos,
            trash: FakeTrash {
                dir: trash_dir,
                fail_on: StdArc::new(Mutex::new(None)),
            },
            catalog,
            _dir: dir,
        };
        (fx, ids)
    }

    fn live(conn: &Connection) -> i64 {
        repo::count_all_photos(conn).unwrap()
    }

    #[test]
    fn trash_moves_the_file_and_sidecar_and_undo_puts_them_back() {
        let (fx, ids) = fixture(&["a.jpg", "b.jpg"]);
        let conn = fx.catalog.connection();
        std::fs::write(fx.photos.join("a.xmp"), b"<x/>").unwrap();

        let plan = plan_trash(conn, &ids[..1]).unwrap();
        assert_eq!(plan.files.len(), 2, "original + sidecar");
        let mut cmd = TrashPhotos::new(plan, Box::new(fx.trash.clone()));
        assert_eq!(cmd.label(), "Move 1 Photo to Trash");
        cmd.apply(conn).unwrap();
        assert!(!fx.photos.join("a.jpg").exists());
        assert!(!fx.photos.join("a.xmp").exists());
        assert!(fx.photos.join("b.jpg").exists(), "untouched");
        assert_eq!(live(conn), 1);

        cmd.revert(conn).unwrap();
        assert_eq!(std::fs::read(fx.photos.join("a.jpg")).unwrap(), b"pixels");
        assert!(fx.photos.join("a.xmp").exists());
        assert_eq!(live(conn), 2);

        // Redo trashes again.
        cmd.apply(conn).unwrap();
        assert!(!fx.photos.join("a.jpg").exists());
        assert_eq!(live(conn), 1);
    }

    #[test]
    fn a_failed_move_leaves_the_catalog_and_every_file_untouched() {
        let (fx, ids) = fixture(&["a.jpg", "b.jpg", "c.jpg"]);
        let conn = fx.catalog.connection();
        *fx.trash.fail_on.lock().unwrap() = Some(fx.photos.join("c.jpg"));
        let plan = plan_trash(conn, &ids).unwrap();
        let mut cmd = TrashPhotos::new(plan, Box::new(fx.trash.clone()));
        let err = cmd.apply(conn).unwrap_err().to_string();
        assert!(err.contains("read-only"), "{err}");
        for n in ["a.jpg", "b.jpg", "c.jpg"] {
            assert!(fx.photos.join(n).exists(), "{n} must be put back");
        }
        assert_eq!(live(conn), 3, "catalog unchanged");
    }

    #[test]
    fn a_file_shared_with_a_virtual_copy_is_not_trashed() {
        let (fx, ids) = fixture(&["a.jpg"]);
        let conn = fx.catalog.connection();
        CreateVirtualCopies::new(vec![ids[0]]).apply(conn).unwrap();

        let plan = plan_trash(conn, &ids).unwrap();
        assert!(plan.files.is_empty());
        assert_eq!(plan.kept_shared, 1);
        let mut cmd = TrashPhotos::new(plan, Box::new(fx.trash.clone()));
        cmd.apply(conn).unwrap();
        assert!(fx.photos.join("a.jpg").exists(), "the copy still needs it");
        assert_eq!(live(conn), 1);

        // Selecting both photos does trash the file.
        cmd.revert(conn).unwrap();
        let all: Vec<PhotoId> = repo::list_all_photos(conn, repo::PhotoSort::ImportOrder)
            .unwrap()
            .into_iter()
            .map(|p| p.photo_id)
            .collect();
        let plan = plan_trash(conn, &all).unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.kept_shared, 0);
    }

    #[test]
    fn undo_survives_a_file_that_left_the_trash() {
        let (fx, ids) = fixture(&["a.jpg"]);
        let conn = fx.catalog.connection();
        let mut cmd = TrashPhotos::new(plan_trash(conn, &ids).unwrap(), Box::new(fx.trash.clone()));
        cmd.apply(conn).unwrap();
        for e in std::fs::read_dir(&fx.trash.dir).unwrap() {
            std::fs::remove_file(e.unwrap().path()).unwrap(); // user emptied the trash
        }
        cmd.revert(conn).unwrap();
        assert_eq!(live(conn), 1, "the photo and its edits come back");
        let r = check_missing(conn, MissingScope::All, &|| false).unwrap();
        assert_eq!(
            r.now_missing, ids,
            "and it is flagged missing, ready to relink"
        );
    }

    #[test]
    fn a_missing_original_is_skipped_by_the_plan() {
        let (fx, ids) = fixture(&["a.jpg"]);
        std::fs::remove_file(fx.photos.join("a.jpg")).unwrap();
        let plan = plan_trash(fx.catalog.connection(), &ids).unwrap();
        assert!(plan.files.is_empty());
        assert_eq!(plan.already_missing, 1);
    }

    #[test]
    fn check_missing_flags_and_clears_without_touching_anything_else() {
        let (fx, ids) = fixture(&["a.jpg", "b.jpg"]);
        let conn = fx.catalog.connection();
        let flag = |id: PhotoId| {
            repo::list_all_photos(conn, repo::PhotoSort::ImportOrder)
                .unwrap()
                .into_iter()
                .find(|p| p.photo_id == id)
                .unwrap()
                .missing
        };
        let r = check_missing(conn, MissingScope::All, &|| false).unwrap();
        assert_eq!((r.checked, r.changed()), (2, false));
        assert!(r.event().is_none());

        std::fs::rename(fx.photos.join("a.jpg"), fx.photos.join("moved.jpg")).unwrap();
        let r = check_missing(conn, MissingScope::All, &|| false).unwrap();
        assert_eq!(r.now_missing, vec![ids[0]]);
        assert!(flag(ids[0]) && !flag(ids[1]));
        assert!(matches!(
            r.event(),
            Some(CatalogEvent::PhotosChanged { .. })
        ));
        assert!(
            fx.photos.join("moved.jpg").exists(),
            "files are never modified"
        );

        // Unchanged state reports no change.
        assert!(
            !check_missing(conn, MissingScope::All, &|| false)
                .unwrap()
                .changed()
        );

        // The drive comes back.
        std::fs::rename(fx.photos.join("moved.jpg"), fx.photos.join("a.jpg")).unwrap();
        let r = check_missing(conn, MissingScope::Photos(&ids[..1]), &|| false).unwrap();
        assert_eq!(r.restored, vec![ids[0]]);
        assert!(!flag(ids[0]));
    }

    #[test]
    fn check_missing_ignores_removed_photos_and_honours_cancel() {
        let (fx, ids) = fixture(&["a.jpg", "b.jpg"]);
        let conn = fx.catalog.connection();
        RemovePhotos::new(vec![ids[0]], Removal::Catalog)
            .apply(conn)
            .unwrap();
        std::fs::remove_file(fx.photos.join("a.jpg")).unwrap();
        let r = check_missing(conn, MissingScope::All, &|| false).unwrap();
        assert_eq!(r.checked, 1);
        assert!(!r.changed());

        std::fs::remove_file(fx.photos.join("b.jpg")).unwrap();
        let r = check_missing(conn, MissingScope::All, &|| true).unwrap();
        assert_eq!(r.checked, 0, "cancelled before the first file");
        assert!(!r.changed());
    }

    /// Touches the real freedesktop trash, so it is opt-in:
    /// `cargo test -p viberoom-services system_trash -- --ignored`.
    #[test]
    #[ignore = "uses the real trash"]
    fn system_trash_round_trips_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("viberoom-trash-test-9f3a.txt");
        std::fs::write(&f, b"hello").unwrap();
        let mut t = SystemTrash;
        let token = t.trash(&f).unwrap();
        assert!(!f.exists());
        t.restore(&token).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"hello");
    }

    #[test]
    fn relink_points_the_file_at_its_new_home_and_keeps_everything_else() {
        let (fx, ids) = fixture(&["a.jpg", "b.jpg"]);
        let conn = fx.catalog.connection();
        conn.execute(
            "UPDATE photos SET rating = 5 WHERE id = ?1",
            params![ids[0].get()],
        )
        .unwrap();
        let new_dir = fx.photos.join("moved");
        std::fs::create_dir_all(&new_dir).unwrap();
        for n in ["a.jpg", "b.jpg"] {
            std::fs::rename(fx.photos.join(n), new_dir.join(n)).unwrap();
        }
        assert_eq!(
            check_missing(conn, MissingScope::All, &|| false)
                .unwrap()
                .now_missing
                .len(),
            2
        );

        let cand = new_dir.join("a.jpg");
        // Size differs from what the catalog recorded only when it was recorded;
        // the fixture records none, so this looks fine.
        assert!(relink_problems(conn, ids[0], &cand).unwrap().is_empty());
        assert_eq!(relink(conn, ids[0], &cand).unwrap(), vec![ids[0]]);
        let a = repo::photo_file_info(conn, ids[0]).unwrap().unwrap();
        assert_eq!(a.path, cand);
        let row = repo::list_all_photos(conn, repo::PhotoSort::ImportOrder).unwrap();
        assert!(!row.iter().find(|p| p.photo_id == ids[0]).unwrap().missing);
        assert!(row.iter().find(|p| p.photo_id == ids[1]).unwrap().missing);
        assert_eq!(row.iter().find(|p| p.photo_id == ids[0]).unwrap().rating, 5);

        // "Also locate the others in this folder".
        assert_eq!(
            relink_missing_in_folder(conn, &new_dir).unwrap(),
            vec![ids[1]]
        );
        assert!(
            repo::list_all_photos(conn, repo::PhotoSort::ImportOrder)
                .unwrap()
                .iter()
                .all(|p| !p.missing)
        );
    }

    #[test]
    fn relink_warns_about_a_different_file_and_refuses_another_catalogued_one() {
        let (fx, ids) = fixture(&["a.jpg", "b.jpg"]);
        let conn = fx.catalog.connection();
        conn.execute(
            "UPDATE files SET size = 6, quick_hash = ?1 WHERE id = (SELECT file_id FROM photos WHERE id = ?2)",
            params![crate::import::quick_hash_of(&fx.photos.join("a.jpg")).unwrap(), ids[0].get()],
        )
        .unwrap();
        std::fs::write(fx.photos.join("other.jpg"), b"different bytes").unwrap();
        std::fs::write(fx.photos.join("other.png"), b"pixels").unwrap();
        let p = relink_problems(conn, ids[0], &fx.photos.join("other.jpg")).unwrap();
        assert_eq!(p.len(), 2, "{p:?}"); // size + contents
        let p = relink_problems(conn, ids[0], &fx.photos.join("other.png")).unwrap();
        assert_eq!(p.len(), 1, "{p:?}"); // only the type; same bytes
        assert!(
            relink_problems(conn, ids[0], &fx.photos.join("a.jpg"))
                .unwrap()
                .is_empty()
        );

        let err = relink(conn, ids[0], &fx.photos.join("b.jpg"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("already in the catalog"), "{err}");
    }
}
