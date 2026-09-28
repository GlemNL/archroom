//! The `previews.db` index plus the JPEG files it points at (plan §5.4,
//! §4.7, decision D6): stored next to the catalog in `Archroom Previews/`
//! so the catalog stays portable, but as a separate SQLite file from the
//! catalog itself, since preview *files* are disposable and shouldn't share
//! a WAL/transaction log with catalog data that isn't.

use std::path::{Path, PathBuf};

use archroom_core::ids::PhotoId;
use rusqlite::{Connection, OptionalExtension, params};

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
    /// `Archroom Previews/` next to it.
    pub fn open_for_catalog(catalog_path: &Path) -> Result<Self> {
        let base = catalog_path.parent().unwrap_or_else(|| Path::new("."));
        Self::open_at(&base.join("Archroom Previews"))
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
        let filename = format!("{}_{level}.jpg", photo_id.get());
        let path = self.dir.join(&filename);
        std::fs::write(&path, jpeg_bytes).map_err(|e| Error::io(&path, e))?;

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
}
