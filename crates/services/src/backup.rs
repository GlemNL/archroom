//! Catalog backups and recovery (plan §14: catalog corruption is a
//! high-impact risk). Backups are SQLite online-backup copies in
//! `Viberoom Backups/` next to the catalog, taken at most once a day at
//! startup (only from a catalog that just passed its check), newest N kept.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use viberoom_catalog::Catalog;
use viberoom_jobs::{Job, JobContext, Priority};
use crossbeam_channel::Sender;

use crate::error::{Error, Result};

const PREFIX: &str = "Viberoom-";
const EXT: &str = "arcat";

pub fn backup_dir(catalog_path: &Path) -> PathBuf {
    catalog_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("Viberoom Backups")
}

/// `YYYYMMDD-HHMMSS` (UTC) for `t`.
fn stamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Existing backups, newest first.
pub fn list_backups(catalog_path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(backup_dir(catalog_path)) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(&format!(".{EXT}")))
        })
        .collect();
    // The timestamp in the name sorts chronologically.
    found.sort();
    found.reverse();
    found
}

/// Backs `catalog` up unless the newest backup is younger than `min_age`,
/// then prunes to the newest `keep`. Returns the new file, if one was made.
pub fn scheduled_backup(
    catalog: &Catalog,
    keep: usize,
    min_age: Duration,
    now: SystemTime,
) -> Result<Option<PathBuf>> {
    let existing = list_backups(catalog.path());
    let fresh = existing
        .first()
        .and_then(|p| std::fs::metadata(p).ok()?.modified().ok())
        .and_then(|m| now.duration_since(m).ok())
        .is_some_and(|age| age < min_age);
    if fresh {
        return Ok(None);
    }
    let dir = backup_dir(catalog.path());
    let dest = dir.join(format!("{PREFIX}{}.{EXT}", stamp(now)));
    // Back up to a temp name, then rename: a killed backup never looks valid.
    let part = dir.join(format!(".{PREFIX}{}.part", stamp(now)));
    let result = catalog
        .backup_to(&part)
        .map_err(Error::from)
        .and_then(|()| std::fs::rename(&part, &dest).map_err(|e| Error::io(&dest, e)));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    for old in list_backups(catalog.path()).into_iter().skip(keep.max(1)) {
        let _ = std::fs::remove_file(old);
    }
    Ok(Some(dest))
}

/// Replaces the catalog at `catalog_path` with `backup`. The current file
/// (and its `-wal`/`-shm`) is kept beside it as `*.damaged-<stamp>`, never
/// deleted. The catalog must not be open. Returns where the old one went.
pub fn restore_backup(catalog_path: &Path, backup: &Path, now: SystemTime) -> Result<PathBuf> {
    let name = catalog_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Viberoom.arcat");
    let moved = catalog_path.with_file_name(format!("{name}.damaged-{}", stamp(now)));
    if catalog_path.exists() {
        std::fs::rename(catalog_path, &moved).map_err(|e| Error::io(catalog_path, e))?;
    }
    for suffix in ["-wal", "-shm"] {
        let side = catalog_path.with_file_name(format!("{name}{suffix}"));
        if side.exists() {
            let to = catalog_path.with_file_name(format!("{name}.damaged-{}{suffix}", stamp(now)));
            let _ = std::fs::rename(side, to);
        }
    }
    // Copy through a temp file so an interrupted restore can be re-run.
    let part = catalog_path.with_file_name(format!(".{name}.restore"));
    std::fs::copy(backup, &part).map_err(|e| Error::io(backup, e))?;
    std::fs::rename(&part, catalog_path).map_err(|e| Error::io(catalog_path, e))?;
    Ok(moved)
}

/// What the startup health job found.
#[derive(Debug)]
pub struct BackupOutcome {
    pub created: Option<PathBuf>,
    pub error: Option<String>,
}

/// Takes the daily backup off the UI thread.
#[derive(Debug)]
pub struct BackupJob {
    pub catalog_path: PathBuf,
    pub keep: usize,
    pub done: Sender<BackupOutcome>,
}

impl Job for BackupJob {
    fn label(&self) -> String {
        "Backing up the catalog".to_string()
    }

    fn priority(&self) -> Priority {
        Priority::Background
    }

    fn run(self: Box<Self>, _cx: &JobContext) {
        let result = Catalog::create_or_open(&self.catalog_path)
            .map_err(Error::from)
            .and_then(|c| {
                scheduled_backup(
                    &c,
                    self.keep,
                    Duration::from_secs(24 * 3600),
                    SystemTime::now(),
                )
            });
        let outcome = match result {
            Ok(created) => BackupOutcome {
                created,
                error: None,
            },
            Err(e) => {
                tracing::warn!(error = %e, "scheduled backup failed");
                BackupOutcome {
                    created: None,
                    error: Some(e.to_string()),
                }
            }
        };
        let _ = self.done.send(outcome);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn stamps_are_utc_civil_time() {
        assert_eq!(stamp(at(0)), "19700101-000000");
        assert_eq!(stamp(at(951_782_400)), "20000229-000000"); // a leap day
        assert_eq!(stamp(at(1_790_000_000)), "20260921-141320");
    }

    #[test]
    fn daily_backups_prune_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Viberoom.arcat");
        let catalog = Catalog::create_or_open(&path).unwrap();
        catalog
            .connection()
            .execute(
                "INSERT INTO schema_meta(key, value) VALUES ('mark', 'v1')",
                [],
            )
            .unwrap();

        let day = 86_400;
        let t0 = 1_800_000_000;
        let first = scheduled_backup(&catalog, 2, Duration::from_secs(day), at(t0)).unwrap();
        assert!(first.is_some());
        // Same day: skipped. (File mtimes are "now", so age the clock back.)
        let again = scheduled_backup(&catalog, 2, Duration::from_secs(day), SystemTime::now());
        assert!(again.unwrap().is_none());
        for n in 1..=3 {
            let t = at(t0 + n * day);
            assert!(
                scheduled_backup(&catalog, 2, Duration::ZERO, t)
                    .unwrap()
                    .is_some()
            );
        }
        let kept = list_backups(&path);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept[0] > kept[1], "newest first");
        assert!(
            !dir.path()
                .join("Viberoom Backups")
                .read_dir()
                .unwrap()
                .any(|e| { e.unwrap().file_name().to_string_lossy().starts_with('.') })
        );

        // Damage the catalog, then restore the newest backup.
        drop(catalog);
        std::fs::write(&path, b"this is not a database").unwrap();
        let moved = restore_backup(&path, &kept[0], at(t0)).unwrap();
        assert_eq!(std::fs::read(&moved).unwrap(), b"this is not a database");
        let restored = Catalog::create_or_open(&path).unwrap();
        assert!(restored.quick_check().unwrap());
        let mark: String = restored
            .connection()
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'mark'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mark, "v1");
    }
}
