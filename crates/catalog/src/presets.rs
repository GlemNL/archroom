//! User presets (plan §8.1). The catalog stores opaque JSON (the settings
//! groups a preset applies and the params it takes them from); the meaning
//! lives in `archroom-services`.

use rusqlite::{Connection, params};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetRow {
    pub id: i64,
    pub name: String,
    pub folder: String,
    /// JSON array of settings-group ids.
    pub groups: String,
    /// JSON `EditParams`.
    pub params: String,
}

pub fn list(conn: &Connection) -> Result<Vec<PresetRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, folder, groups, params FROM presets ORDER BY folder, name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(PresetRow {
            id: r.get(0)?,
            name: r.get(1)?,
            folder: r.get(2)?,
            groups: r.get(3)?,
            params: r.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn insert(
    conn: &Connection,
    name: &str,
    folder: &str,
    groups_json: &str,
    params_json: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO presets (name, folder, groups, params, created_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        params![name, folder, groups_json, params_json],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename(conn: &Connection, id: i64, name: &str) -> Result<()> {
    conn.execute(
        "UPDATE presets SET name = ?2 WHERE id = ?1",
        params![id, name],
    )?;
    Ok(())
}

/// Replaces what a preset applies ("Update with current settings").
pub fn update(conn: &Connection, id: i64, groups_json: &str, params_json: &str) -> Result<()> {
    conn.execute(
        "UPDATE presets SET groups = ?2, params = ?3 WHERE id = ?1",
        params![id, groups_json, params_json],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM presets WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::Catalog;

    #[test]
    fn presets_round_trip_and_sort_by_folder_then_name() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::create_or_open(dir.path().join("t.arcat")).unwrap();
        let conn = catalog.connection();
        insert(conn, "b", "Mine", "[\"basic_tone\"]", "{}").unwrap();
        let a = insert(conn, "a", "Mine", "[]", "{}").unwrap();
        insert(conn, "z", "Film", "[]", "{}").unwrap();
        let names: Vec<_> = list(conn).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["z", "a", "b"]);
        rename(conn, a, "renamed").unwrap();
        update(conn, a, "[\"crop\"]", "{\"x\":1}").unwrap();
        let row = list(conn).unwrap().into_iter().find(|p| p.id == a).unwrap();
        assert_eq!(
            (row.name.as_str(), row.groups.as_str()),
            ("renamed", "[\"crop\"]")
        );
        delete(conn, a).unwrap();
        assert_eq!(list(conn).unwrap().len(), 2);
    }
}
