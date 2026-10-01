use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tracing::info;

use crate::error::{Error, Result};
use crate::schema;

/// A single open catalog (D7: one at a time). Wraps the SQLite connection
/// used for writes; read-only queries will grow a small pool of their own
/// connections once M1 needs concurrent readers (plan §4.6).
pub struct Catalog {
    conn: Connection,
    path: PathBuf,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Catalog")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Catalog {
    /// Opens `path`, creating and migrating it if it doesn't exist yet.
    pub fn create_or_open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }

        let mut conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        schema::migrations().to_latest(&mut conn)?;

        info!(path = %path.display(), "catalog opened");
        Ok(Self { conn, path })
    }

    /// Makes photos removed in an earlier session final (catalog rows only;
    /// files on disk are never touched). The app calls this once at start-up,
    /// *not* from `create_or_open`: background jobs reopen the catalog
    /// mid-session and must not purge photos whose removal is still undoable.
    pub fn purge_removed(&self) -> Result<usize> {
        let purged = crate::repo::purge_removed(&self.conn)?;
        if purged > 0 {
            info!(purged, "purged removed photos");
        }
        Ok(purged)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Copies the catalog to `dest` using SQLite's online backup API, so it
    /// can run against a catalog that's still being written to (plan §4.7).
    pub fn backup_to(&self, dest: impl AsRef<Path>) -> Result<()> {
        let dest = dest.as_ref();
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let mut dest_conn = Connection::open(dest)?;
        let backup = rusqlite::backup::Backup::new(&self.conn, &mut dest_conn)?;
        // Big steps: at 5 pages per 250 ms a 200 MB catalog would take an hour.
        backup.run_to_completion(2048, std::time::Duration::from_millis(5), None)?;
        info!(dest = %dest.display(), "catalog backed up");
        Ok(())
    }

    /// `PRAGMA quick_check`: like `integrity_check` without verifying that
    /// indexes match their tables, so it is cheap enough for every launch.
    pub fn quick_check(&self) -> Result<bool> {
        let result: String = self
            .conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        Ok(result == "ok")
    }

    /// `PRAGMA integrity_check`, run on open in the app (plan §14: catalog
    /// corruption is a high-impact risk).
    pub fn integrity_check(&self) -> Result<bool> {
        let result: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        Ok(result == "ok")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn create_or_open_creates_parent_dirs_and_migrates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/Viberoom.arcat");

        let catalog = Catalog::create_or_open(&path).expect("create");
        assert!(path.exists());
        assert!(catalog.integrity_check().expect("integrity check"));
    }

    #[test]
    fn reopening_an_existing_catalog_does_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("Viberoom.arcat");

        Catalog::create_or_open(&path).expect("create");
        let reopened = Catalog::create_or_open(&path).expect("reopen");
        assert!(reopened.integrity_check().expect("integrity check"));
    }

    #[test]
    fn backup_to_produces_a_valid_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src_path = dir.path().join("Viberoom.arcat");
        let dest_path = dir.path().join("Backups/Viberoom-backup.arcat");

        let catalog = Catalog::create_or_open(&src_path).expect("create");
        catalog.backup_to(&dest_path).expect("backup");

        let backup = Catalog::create_or_open(&dest_path).expect("open backup");
        assert!(backup.integrity_check().expect("integrity check"));
    }
}
