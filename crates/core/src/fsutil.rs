//! Crash-safe file writes: a power cut or a kill mid-write must leave either
//! the old file or the new one, never a torn mix (plan §14).

use std::io::Write;
use std::path::Path;

use crate::error::{Error, Result};

/// Writes `bytes` to a sibling temp file, syncs it, then renames it over
/// `path` (atomic on the same filesystem).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::io(path, e));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_file_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.toml");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn a_failed_write_keeps_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.toml");
        write_atomic(&p, b"keep").unwrap();
        // The target's parent is gone, so the temp file can't be created.
        assert!(write_atomic(&dir.path().join("missing/a.toml"), b"x").is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"keep");
    }
}
