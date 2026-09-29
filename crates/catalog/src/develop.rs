//! Persistence for Develop (plan §5.1/§5.2): the current settings per photo,
//! its linear edit history and named snapshots. The catalog stores params
//! as opaque JSON text plus the caller-computed hash — `EditParams` lives in
//! `archroom-engine`, which this crate must not depend on.

use archroom_core::ids::PhotoId;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevelopRow {
    pub process_version: u32,
    pub params: String,
    pub params_hash: Vec<u8>,
}

pub fn get_settings(conn: &Connection, photo: PhotoId) -> Result<Option<DevelopRow>> {
    conn.query_row(
        "SELECT process_version, params, params_hash FROM develop_settings WHERE photo_id = ?1",
        params![photo.get()],
        |r| {
            Ok(DevelopRow {
                process_version: r.get(0)?,
                params: r.get(1)?,
                params_hash: r.get(2)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn set_settings(conn: &Connection, photo: PhotoId, row: &DevelopRow) -> Result<()> {
    conn.execute(
        "INSERT INTO develop_settings (photo_id, process_version, params, params_hash)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(photo_id) DO UPDATE SET
           process_version = excluded.process_version,
           params = excluded.params,
           params_hash = excluded.params_hash",
        params![
            photo.get(),
            row.process_version,
            row.params,
            row.params_hash
        ],
    )?;
    Ok(())
}

/// Removes the photo's settings row (the photo is "unedited" again).
pub fn clear_settings(conn: &Connection, photo: PhotoId) -> Result<()> {
    conn.execute(
        "DELETE FROM develop_settings WHERE photo_id = ?1",
        params![photo.get()],
    )?;
    Ok(())
}

/// Which photos have a settings row — the grid's "edited" badge source.
pub fn edited_photo_ids(conn: &Connection, ids: &[PhotoId]) -> Result<Vec<PhotoId>> {
    let mut stmt = conn.prepare("SELECT 1 FROM develop_settings WHERE photo_id = ?1")?;
    let mut out = Vec::new();
    for id in ids {
        if stmt.exists(params![id.get()])? {
            out.push(*id);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub id: i64,
    pub seq: i64,
    pub label: String,
    pub params: String,
}

pub fn list_history(conn: &Connection, photo: PhotoId) -> Result<Vec<HistoryRow>> {
    let mut stmt = conn
        .prepare("SELECT id, seq, label, params FROM history WHERE photo_id = ?1 ORDER BY seq")?;
    let rows = stmt.query_map(params![photo.get()], |r| {
        Ok(HistoryRow {
            id: r.get(0)?,
            seq: r.get(1)?,
            label: r.get(2)?,
            params: r.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Appends a history step holding the state *after* the edit. If the newest
/// step has the same `label` and is at most `coalesce_secs` old, that step
/// is overwritten instead — dragging one slider is one step (plan §8.1).
/// Returns the seq of the row written.
pub fn record_history(
    conn: &Connection,
    photo: PhotoId,
    label: &str,
    params_json: &str,
    coalesce_secs: i64,
) -> Result<i64> {
    let last: Option<(i64, i64, String, i64)> = conn
        .query_row(
            "SELECT id, seq, label, CAST(strftime('%s','now') AS INTEGER)
                    - CAST(strftime('%s', created_at) AS INTEGER)
             FROM history WHERE photo_id = ?1 ORDER BY seq DESC LIMIT 1",
            params![photo.get()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    if let Some((id, seq, last_label, age)) = &last
        && last_label == label
        && *age <= coalesce_secs
    {
        conn.execute(
            "UPDATE history SET params = ?1, created_at = datetime('now') WHERE id = ?2",
            params![params_json, id],
        )?;
        return Ok(*seq);
    }
    let seq = last.map_or(1, |(_, seq, _, _)| seq + 1);
    conn.execute(
        "INSERT INTO history (photo_id, seq, label, params, created_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        params![photo.get(), seq, label, params_json],
    )?;
    Ok(seq)
}

/// Drops every step after `seq` (a new edit made after undoing forgets the
/// undone future, as in any linear history).
pub fn truncate_history_after(conn: &Connection, photo: PhotoId, seq: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM history WHERE photo_id = ?1 AND seq > ?2",
        params![photo.get(), seq],
    )?;
    Ok(())
}

pub fn clear_history(conn: &Connection, photo: PhotoId) -> Result<()> {
    conn.execute(
        "DELETE FROM history WHERE photo_id = ?1",
        params![photo.get()],
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRow {
    pub id: i64,
    pub name: String,
    pub params: String,
}

pub fn add_snapshot(
    conn: &Connection,
    photo: PhotoId,
    name: &str,
    params_json: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO snapshots (photo_id, name, params, created_at)
         VALUES (?1, ?2, ?3, datetime('now'))",
        params![photo.get(), name, params_json],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_snapshots(conn: &Connection, photo: PhotoId) -> Result<Vec<SnapshotRow>> {
    let mut stmt =
        conn.prepare("SELECT id, name, params FROM snapshots WHERE photo_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![photo.get()], |r| {
        Ok(SnapshotRow {
            id: r.get(0)?,
            name: r.get(1)?,
            params: r.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn delete_snapshot(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM snapshots WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::Catalog;
    use crate::repo::{self, NewFile, NewPhoto};

    fn photo(conn: &Connection) -> PhotoId {
        let folder = repo::upsert_folder_path(conn, std::path::Path::new("/p")).unwrap();
        let file = repo::insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename: "a.nef",
                ext: "nef",
                kind: "raw",
                ..Default::default()
            },
        )
        .unwrap();
        repo::insert_photo(
            conn,
            &NewPhoto {
                file_id: file,
                import_id: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn settings_round_trip_across_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("T.arcat");
        let id;
        let row = DevelopRow {
            process_version: 1,
            params: r#"{"ops":{"exposure":{"ev":0.5,"v":1}},"process_version":1}"#.into(),
            params_hash: vec![7; 16],
        };
        {
            let c = Catalog::create_or_open(&path).unwrap();
            id = photo(c.connection());
            assert!(get_settings(c.connection(), id).unwrap().is_none());
            set_settings(c.connection(), id, &row).unwrap();
            set_settings(c.connection(), id, &row).unwrap(); // upsert
            assert_eq!(edited_photo_ids(c.connection(), &[id]).unwrap(), vec![id]);
        }
        let c = Catalog::create_or_open(&path).unwrap();
        assert_eq!(get_settings(c.connection(), id).unwrap(), Some(row));
        clear_settings(c.connection(), id).unwrap();
        assert!(edited_photo_ids(c.connection(), &[id]).unwrap().is_empty());
    }

    #[test]
    fn history_coalesces_same_label_and_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::create_or_open(dir.path().join("T.arcat")).unwrap();
        let conn = c.connection();
        let id = photo(conn);

        assert_eq!(record_history(conn, id, "Exposure", "{1}", 60).unwrap(), 1);
        assert_eq!(
            record_history(conn, id, "Exposure", "{2}", 60).unwrap(),
            1,
            "coalesced"
        );
        assert_eq!(record_history(conn, id, "Contrast", "{3}", 60).unwrap(), 2);
        assert_eq!(
            record_history(conn, id, "Contrast", "{4}", -1).unwrap(),
            3,
            "outside the window a same-label edit is a new step"
        );
        let rows = list_history(conn, id).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.params.as_str()).collect::<Vec<_>>(),
            ["{2}", "{3}", "{4}"]
        );

        truncate_history_after(conn, id, 2).unwrap();
        assert_eq!(list_history(conn, id).unwrap().len(), 2);
        clear_history(conn, id).unwrap();
        assert!(list_history(conn, id).unwrap().is_empty());
    }

    #[test]
    fn snapshots_add_list_delete() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::create_or_open(dir.path().join("T.arcat")).unwrap();
        let conn = c.connection();
        let id = photo(conn);
        let s = add_snapshot(conn, id, "Warm", "{}").unwrap();
        assert_eq!(list_snapshots(conn, id).unwrap()[0].name, "Warm");
        delete_snapshot(conn, s).unwrap();
        assert!(list_snapshots(conn, id).unwrap().is_empty());
    }
}
