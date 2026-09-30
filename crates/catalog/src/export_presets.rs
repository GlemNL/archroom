//! User export presets (plan §9). The catalog stores each preset's settings
//! as opaque JSON; `archroom-services::export` owns their meaning.

use rusqlite::{Connection, params};

use crate::error::Result;

/// `(name, settings JSON)` for every preset, by name.
pub fn list(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt =
        conn.prepare("SELECT name, settings FROM export_presets ORDER BY name COLLATE NOCASE")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Saves `settings_json` under `name`, replacing an existing preset.
pub fn save(conn: &Connection, name: &str, settings_json: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO export_presets (name, settings) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET settings = excluded.settings",
        params![name, settings_json],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, name: &str) -> Result<()> {
    conn.execute("DELETE FROM export_presets WHERE name = ?1", params![name])?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::Catalog;

    #[test]
    fn save_replaces_by_name_and_lists_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::create_or_open(dir.path().join("t.arcat")).unwrap();
        let conn = catalog.connection();
        save(conn, "b", "{\"v\":1}").unwrap();
        save(conn, "a", "{}").unwrap();
        save(conn, "b", "{\"v\":2}").unwrap();
        let rows = list(conn).unwrap();
        assert_eq!(
            rows,
            [
                ("a".to_string(), "{}".to_string()),
                ("b".to_string(), "{\"v\":2}".to_string())
            ]
        );
        delete(conn, "a").unwrap();
        assert_eq!(list(conn).unwrap().len(), 1);
    }
}
