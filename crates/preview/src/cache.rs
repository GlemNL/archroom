//! The `previews.db` index plus the JPEG files it points at (plan §5.4,
//! §4.7, decision D6): stored next to the catalog in `Viberoom Previews/`
//! so the catalog stays portable, but as a separate SQLite file from the
//! catalog itself, since preview *files* are disposable and shouldn't share
//! a WAL/transaction log with catalog data that isn't.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use viberoom_core::ids::PhotoId;

use crate::error::{Error, Result};

/// No develop settings exist before M3, so every Phase-A preview is keyed
/// to this sentinel. Real hashing (plan §5.2's `params_hash`, an xxh3 of
/// the canonical develop-settings JSON) starts once there's something to
/// hash; existing rows just become stale and regenerate.
pub const DEFAULT_PARAMS_HASH: i64 = 0;

pub const LEVEL_L1: &str = "l1";
pub const LEVEL_L2: &str = "l2";

pub struct PreviewCache {
    conn: Connection,
    dir: PathBuf,
}

impl std::fmt::Debug for PreviewCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewCache")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl PreviewCache {
    /// `catalog_path` is the `.arcat` file; the cache lives in
    /// `Viberoom Previews/` next to it.
    pub fn open_for_catalog(catalog_path: &Path) -> Result<Self> {
        let base = catalog_path.parent().unwrap_or_else(|| Path::new("."));
        Self::open_at(&base.join("Viberoom Previews"))
    }

    pub fn open_at(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let conn = Connection::open(dir.join("previews.db"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS previews (
                photo_id    INTEGER NOT NULL,
                level       TEXT NOT NULL,
                params_hash INTEGER NOT NULL,
                filename    TEXT NOT NULL,
                width       INTEGER NOT NULL,
                height      INTEGER NOT NULL,
                PRIMARY KEY (photo_id, level)
            );",
        )?;
        Ok(Self {
            conn,
            dir: dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Writes the JPEG file and upserts its index row, replacing whatever
    /// was previously cached at `(photo_id, level)`.
    pub fn store(
        &self,
        photo_id: PhotoId,
        level: &str,
        params_hash: i64,
        jpeg_bytes: &[u8],
        width: u32,
        height: u32,
    ) -> Result<PathBuf> {
        // The params hash is part of the name so an edited preview has a new
        // URI: image loaders cache by URI and would otherwise keep showing
        // the stale file.
        let filename = format!("{}_{level}_{:016x}.jpg", photo_id.get(), params_hash as u64);
        let path = self.dir.join(&filename);
        let previous = self.lookup(photo_id, level)?;
        viberoom_core::fsutil::write_atomic(&path, jpeg_bytes)?;
        if let Some(old) = previous
            && old != path
        {
            let _ = std::fs::remove_file(old);
        }

        self.conn.execute(
            "INSERT INTO previews (photo_id, level, params_hash, filename, width, height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (photo_id, level) DO UPDATE SET
                params_hash = excluded.params_hash,
                filename = excluded.filename,
                width = excluded.width,
                height = excluded.height",
            params![photo_id.get(), level, params_hash, filename, width, height],
        )?;
        Ok(path)
    }

    /// Total size of the preview files on disk, in bytes.
    pub fn total_bytes(&self) -> u64 {
        self.files().iter().map(|(_, len, _)| len).sum()
    }

    /// `(path, bytes, modified)` for every preview file in the index.
    fn files(&self) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        let Ok(mut stmt) = self.conn.prepare("SELECT filename FROM previews") else {
            return Vec::new();
        };
        let names: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        names
            .into_iter()
            .filter_map(|n| {
                let path = self.dir.join(n);
                let meta = std::fs::metadata(&path).ok()?;
                Some((path, meta.len(), meta.modified().ok()?))
            })
            .collect()
    }

    /// Housekeeping for the disposable cache: drops index rows whose file
    /// is gone and files no row points at (e.g. left by a crash), then
    /// evicts the oldest previews until the files fit in `budget_bytes`.
    /// Everything removed regenerates on demand. Returns the bytes freed.
    pub fn trim(&self, budget_bytes: u64) -> Result<u64> {
        let rows: Vec<(String, i64, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT filename, photo_id, level FROM previews")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let known: std::collections::HashSet<&str> =
            rows.iter().map(|(f, _, _)| f.as_str()).collect();
        let mut freed = 0;
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let stray = name.ends_with(".jpg") && !known.contains(name.as_str())
                    || name.starts_with('.') && name.ends_with(".tmp");
                if stray && let Ok(meta) = e.metadata() {
                    freed += meta.len();
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let mut live = Vec::new();
        for (name, photo, level) in rows {
            match std::fs::metadata(self.dir.join(&name)) {
                Ok(meta) => live.push((name, photo, level, meta.len(), meta.modified().ok())),
                Err(_) => {
                    self.conn.execute(
                        "DELETE FROM previews WHERE photo_id = ?1 AND level = ?2",
                        params![photo, level],
                    )?;
                }
            }
        }
        let mut total: u64 = live.iter().map(|r| r.3).sum();
        // Big standard previews go first, oldest first; thumbnails last.
        live.sort_by_key(|r| (r.2 == LEVEL_L1, r.4));
        for (name, photo, level, len, _) in live {
            if total <= budget_bytes {
                break;
            }
            let _ = std::fs::remove_file(self.dir.join(&name));
            self.conn.execute(
                "DELETE FROM previews WHERE photo_id = ?1 AND level = ?2",
                params![photo, level],
            )?;
            total -= len;
            freed += len;
        }
        Ok(freed)
    }

    /// Gives `dst` its own copy of `src`'s preview at `level` (same pixels,
    /// same params hash). Virtual copies start as the same picture as their
    /// master, but previews are keyed by photo id, so they would otherwise
    /// stay blank until rendered. Returns false when there was nothing to copy
    /// or `dst` already has one.
    pub fn copy_entry(&self, src: PhotoId, dst: PhotoId, level: &str) -> Result<bool> {
        if self.lookup(dst, level)?.is_some() {
            return Ok(false);
        }
        let row: Option<(i64, u32, u32)> = self
            .conn
            .query_row(
                "SELECT params_hash, width, height FROM previews WHERE photo_id = ?1 AND level = ?2",
                params![src.get(), level],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (Some((hash, w, h)), Some(path)) = (row, self.lookup(src, level)?) else {
            return Ok(false);
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(false);
        };
        self.store(dst, level, hash, &bytes, w, h)?;
        Ok(true)
    }

    pub fn lookup(&self, photo_id: PhotoId, level: &str) -> Result<Option<PathBuf>> {
        let filename: Option<String> = self
            .conn
            .query_row(
                "SELECT filename FROM previews WHERE photo_id = ?1 AND level = ?2",
                params![photo_id.get(), level],
                |r| r.get(0),
            )
            .optional()?;
        Ok(filename.map(|f| self.dir.join(f)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn trim_evicts_oldest_large_previews_first_and_sweeps_strays() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open_at(dir.path()).unwrap();
        for id in 1..=3 {
            cache
                .store(PhotoId::new(id), LEVEL_L2, 1, &[0u8; 1000], 10, 10)
                .unwrap();
            cache
                .store(PhotoId::new(id), LEVEL_L1, 1, &[0u8; 100], 10, 10)
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::write(dir.path().join("stray.jpg"), [0u8; 50]).unwrap();
        assert_eq!(cache.total_bytes(), 3300);
        // Budget for one L2 and all thumbnails.
        let freed = cache.trim(1300).unwrap();
        assert_eq!(freed, 2000 + 50);
        assert!(cache.lookup(PhotoId::new(1), LEVEL_L2).unwrap().is_none());
        assert!(
            cache
                .lookup(PhotoId::new(3), LEVEL_L2)
                .unwrap()
                .unwrap()
                .exists()
        );
        for id in 1..=3 {
            assert!(
                cache
                    .lookup(PhotoId::new(id), LEVEL_L1)
                    .unwrap()
                    .unwrap()
                    .exists()
            );
        }
        assert!(!dir.path().join("stray.jpg").exists());
        assert_eq!(cache.total_bytes(), 1300);
    }

    #[test]
    fn store_then_lookup_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open_at(dir.path()).unwrap();

        assert!(cache.lookup(PhotoId::new(1), LEVEL_L1).unwrap().is_none());

        let path = cache
            .store(
                PhotoId::new(1),
                LEVEL_L1,
                DEFAULT_PARAMS_HASH,
                b"fake jpeg",
                320,
                213,
            )
            .unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"fake jpeg");

        let found = cache.lookup(PhotoId::new(1), LEVEL_L1).unwrap().unwrap();
        assert_eq!(found, path);
    }

    #[test]
    fn storing_again_overwrites_the_previous_preview() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open_at(dir.path()).unwrap();

        cache
            .store(PhotoId::new(1), LEVEL_L1, 0, b"old", 10, 10)
            .unwrap();
        let path = cache
            .store(PhotoId::new(1), LEVEL_L1, 0, b"new", 20, 20)
            .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn copy_entry_duplicates_a_preview_for_another_photo() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open_at(dir.path()).unwrap();
        let (a, b) = (PhotoId::new(1), PhotoId::new(2));
        assert!(
            !cache.copy_entry(a, b, LEVEL_L1).unwrap(),
            "nothing to copy yet"
        );
        cache.store(a, LEVEL_L1, 7, b"jpeg", 10, 20).unwrap();
        assert!(cache.copy_entry(a, b, LEVEL_L1).unwrap());
        let (pa, pb) = (
            cache.lookup(a, LEVEL_L1).unwrap().unwrap(),
            cache.lookup(b, LEVEL_L1).unwrap().unwrap(),
        );
        assert_ne!(
            pa, pb,
            "its own file, so trimming one never breaks the other"
        );
        assert_eq!(std::fs::read(pb).unwrap(), b"jpeg");
        assert!(!cache.copy_entry(a, b, LEVEL_L1).unwrap(), "already there");
    }
}
