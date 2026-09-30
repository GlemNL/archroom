//! Collections (plan §7.2): regular collections, collection sets, the
//! Quick Collection and smart collections, plus the one filtered photo
//! query the Grid uses for every source (folder, import, collection, all).

use viberoom_core::ids::{CollectionId, FolderId, ImportId, PhotoId};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::criteria::{self, Rule, SmartRules};
use crate::error::Result;
use crate::repo::{PHOTO_SUMMARY_COLUMNS, PhotoSort, PhotoSummary, photo_summary_from_row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    Regular,
    Set,
    Quick,
    Smart,
}

impl CollectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CollectionKind::Regular => "regular",
            CollectionKind::Set => "set",
            CollectionKind::Quick => "quick",
            CollectionKind::Smart => "smart",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "set" => CollectionKind::Set,
            "quick" => CollectionKind::Quick,
            "smart" => CollectionKind::Smart,
            _ => CollectionKind::Regular,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CollectionRow {
    pub id: CollectionId,
    pub parent_id: Option<CollectionId>,
    pub kind: CollectionKind,
    pub name: String,
    pub rules: Option<SmartRules>,
    /// Photo count: membership rows for regular/quick, a live count of the
    /// rules for smart, 0 for sets.
    pub count: i64,
}

/// Where the Grid's photos come from.
#[derive(Debug, Clone, PartialEq)]
pub enum PhotoSource {
    All,
    Folder(FolderId),
    Import(ImportId),
    Collection(CollectionId),
}

pub fn create_collection(
    conn: &Connection,
    kind: CollectionKind,
    name: &str,
    parent: Option<CollectionId>,
    rules: Option<&SmartRules>,
) -> Result<CollectionId> {
    conn.execute(
        "INSERT INTO collections (parent_id, kind, name, rules) VALUES (?1, ?2, ?3, ?4)",
        params![
            parent.map(CollectionId::get),
            kind.as_str(),
            name,
            rules.map(SmartRules::to_json)
        ],
    )?;
    Ok(CollectionId::new(conn.last_insert_rowid()))
}

pub fn rename_collection(conn: &Connection, id: CollectionId, name: &str) -> Result<()> {
    conn.execute(
        "UPDATE collections SET name = ?1 WHERE id = ?2",
        params![name, id.get()],
    )?;
    Ok(())
}

pub fn set_smart_rules(conn: &Connection, id: CollectionId, rules: &SmartRules) -> Result<()> {
    conn.execute(
        "UPDATE collections SET rules = ?1 WHERE id = ?2 AND kind = 'smart'",
        params![rules.to_json(), id.get()],
    )?;
    Ok(())
}

/// Deletes a collection and, for a set, everything nested under it.
/// Photos themselves are untouched.
pub fn delete_collection(conn: &Connection, id: CollectionId) -> Result<()> {
    let children: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM collections WHERE parent_id = ?1")?;
        let rows = stmt.query_map(params![id.get()], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for child in children {
        delete_collection(conn, CollectionId::new(child))?;
    }
    conn.execute(
        "DELETE FROM collection_photos WHERE collection_id = ?1",
        params![id.get()],
    )?;
    conn.execute("DELETE FROM collections WHERE id = ?1", params![id.get()])?;
    Ok(())
}

/// The single Quick Collection (plan §7.2), created on first use.
pub fn quick_collection(conn: &Connection) -> Result<CollectionId> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM collections WHERE kind = 'quick' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(id) => Ok(CollectionId::new(id)),
        None => create_collection(conn, CollectionKind::Quick, "Quick Collection", None, None),
    }
}

/// Adds photos to a regular/quick collection; already-present ones are
/// skipped. Returns the ids actually added (what undo must remove).
pub fn add_photos(
    conn: &Connection,
    collection: CollectionId,
    photos: &[PhotoId],
) -> Result<Vec<PhotoId>> {
    let mut added = Vec::new();
    for photo in photos {
        let n = conn.execute(
            "INSERT OR IGNORE INTO collection_photos (collection_id, photo_id, position)
             VALUES (?1, ?2, (SELECT coalesce(max(position), 0) + 1
                              FROM collection_photos WHERE collection_id = ?1))",
            params![collection.get(), photo.get()],
        )?;
        if n > 0 {
            added.push(*photo);
        }
    }
    Ok(added)
}

pub fn remove_photos(
    conn: &Connection,
    collection: CollectionId,
    photos: &[PhotoId],
) -> Result<()> {
    for photo in photos {
        conn.execute(
            "DELETE FROM collection_photos WHERE collection_id = ?1 AND photo_id = ?2",
            params![collection.get(), photo.get()],
        )?;
    }
    Ok(())
}

pub fn list_collections(conn: &Connection) -> Result<Vec<CollectionRow>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.parent_id, c.kind, c.name, c.rules,
                (SELECT count(*) FROM collection_photos cp WHERE cp.collection_id = c.id)
         FROM collections c ORDER BY c.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(CollectionRow {
            id: CollectionId::new(r.get(0)?),
            parent_id: r.get::<_, Option<i64>>(1)?.map(CollectionId::new),
            kind: CollectionKind::parse(&r.get::<_, String>(2)?),
            name: r.get(3)?,
            rules: r
                .get::<_, Option<String>>(4)?
                .and_then(|j| SmartRules::from_json(&j)),
            count: r.get(5)?,
        })
    })?;
    let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for row in &mut out {
        if row.kind == CollectionKind::Smart {
            let rules = row.rules.clone().unwrap_or_default();
            row.count = list_photos(
                conn,
                &PhotoSource::All,
                &rules.rules,
                rules.match_all,
                PhotoSort::ImportOrder,
            )?
            .len() as i64;
        }
    }
    Ok(out)
}

fn get_collection(
    conn: &Connection,
    id: CollectionId,
) -> Result<Option<(CollectionKind, Option<SmartRules>)>> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT kind, rules FROM collections WHERE id = ?1",
            params![id.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(k, j)| {
        (
            CollectionKind::parse(&k),
            j.and_then(|j| SmartRules::from_json(&j)),
        )
    }))
}

/// Photos from `source` narrowed by `rules` (combined with `match_all`).
/// A smart collection source contributes its own saved rules, ANDed with
/// the caller's filter rules.
pub fn list_photos(
    conn: &Connection,
    source: &PhotoSource,
    rules: &[Rule],
    match_all: bool,
    sort: PhotoSort,
) -> Result<Vec<PhotoSummary>> {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    let mut smart: Option<SmartRules> = None;

    match source {
        PhotoSource::All => {}
        PhotoSource::Folder(id) => {
            params.push(Value::from(id.get()));
            clauses.push(format!("f.folder_id = ?{}", params.len()));
        }
        PhotoSource::Import(id) => {
            params.push(Value::from(id.get()));
            clauses.push(format!("p.import_id = ?{}", params.len()));
        }
        PhotoSource::Collection(id) => match get_collection(conn, *id)? {
            Some((CollectionKind::Smart, r)) => smart = Some(r.unwrap_or_default()),
            Some(_) => {
                params.push(Value::from(id.get()));
                clauses.push(format!(
                    "p.id IN (SELECT photo_id FROM collection_photos WHERE collection_id = ?{})",
                    params.len()
                ));
            }
            None => return Ok(Vec::new()),
        },
    }

    if let Some(s) = &smart {
        let (sql, vals) = criteria::compose(&s.rules, s.match_all, params.len() as i32 + 1);
        if !sql.is_empty() {
            clauses.push(sql);
            params.extend(vals);
        }
    }
    let (sql, vals) = criteria::compose(rules, match_all, params.len() as i32 + 1);
    if !sql.is_empty() {
        clauses.push(sql);
        params.extend(vals);
    }

    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT {PHOTO_SUMMARY_COLUMNS} FROM photos p JOIN files f ON f.id = p.file_id
         {where_sql} ORDER BY {}",
        sort.order_by()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), photo_summary_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::Catalog;
    use crate::criteria::{FlagValue, RelOp};
    use crate::repo::{self, NewFile, NewPhoto};

    fn seed(conn: &Connection) -> Vec<PhotoId> {
        let folder = repo::upsert_folder_path(conn, std::path::Path::new("/p")).unwrap();
        (0..4)
            .map(|i| {
                let name = format!("img{i}.nef");
                let file = repo::insert_file(
                    conn,
                    &NewFile {
                        folder_id: folder,
                        filename: &name,
                        ext: "nef",
                        kind: "raw",
                        width: Some(10),
                        height: Some(10),
                        capture_time: Some(&format!("202{i}-05-0{}T10:00:00", i + 1)),
                        camera_model: Some(if i < 2 { "A" } else { "B" }),
                        ..Default::default()
                    },
                )
                .unwrap();
                let p = repo::insert_photo(
                    conn,
                    &NewPhoto {
                        file_id: file,
                        import_id: None,
                    },
                )
                .unwrap();
                conn.execute(
                    "UPDATE photos SET rating = ?1 WHERE id = ?2",
                    params![i, p.get()],
                )
                .unwrap();
                p
            })
            .collect()
    }

    fn open() -> (tempfile::TempDir, Catalog) {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::create_or_open(dir.path().join("T.arcat")).unwrap();
        (dir, c)
    }

    #[test]
    fn rules_filter_and_combine() {
        let (_d, c) = open();
        let conn = c.connection();
        seed(conn);
        let ids = |rules: &[Rule], all| {
            list_photos(conn, &PhotoSource::All, rules, all, PhotoSort::Filename)
                .unwrap()
                .iter()
                .map(|p| p.filename.clone())
                .collect::<Vec<_>>()
        };
        let r2 = Rule::Rating {
            op: RelOp::AtLeast,
            stars: 2,
        };
        assert_eq!(
            ids(std::slice::from_ref(&r2), true),
            ["img2.nef", "img3.nef"]
        );
        let cam = Rule::Camera { model: "A".into() };
        assert_eq!(ids(&[r2.clone(), cam.clone()], true), Vec::<String>::new());
        assert_eq!(ids(&[r2, cam], false).len(), 4);
        assert_eq!(
            ids(
                &[Rule::Flag {
                    value: FlagValue::Picked
                }],
                true
            )
            .len(),
            0
        );
        assert_eq!(ids(&[Rule::CaptureYear { year: 2021 }], true), ["img1.nef"]);
        assert_eq!(
            ids(
                &[Rule::Text {
                    contains: "img3".into()
                }],
                true
            ),
            ["img3.nef"]
        );
        assert_eq!(ids(&[Rule::Edited { edited: false }], true).len(), 4);
    }

    #[test]
    fn regular_and_smart_collections() {
        let (_d, c) = open();
        let conn = c.connection();
        let photos = seed(conn);

        let reg = create_collection(conn, CollectionKind::Regular, "Trip", None, None).unwrap();
        assert_eq!(add_photos(conn, reg, &photos[..2]).unwrap().len(), 2);
        assert!(
            add_photos(conn, reg, &photos[..1]).unwrap().is_empty(),
            "dupes skipped"
        );
        let got = list_photos(
            conn,
            &PhotoSource::Collection(reg),
            &[],
            true,
            PhotoSort::Filename,
        )
        .unwrap();
        assert_eq!(got.len(), 2);
        remove_photos(conn, reg, &photos[..1]).unwrap();

        let smart_rules = SmartRules {
            match_all: true,
            rules: vec![Rule::Rating {
                op: RelOp::AtLeast,
                stars: 3,
            }],
        };
        let smart = create_collection(
            conn,
            CollectionKind::Smart,
            "Best",
            None,
            Some(&smart_rules),
        )
        .unwrap();
        let got = list_photos(
            conn,
            &PhotoSource::Collection(smart),
            &[],
            true,
            PhotoSort::Filename,
        )
        .unwrap();
        assert_eq!(got.len(), 1);

        let rows = list_collections(conn).unwrap();
        let best = rows.iter().find(|r| r.id == smart).unwrap();
        assert_eq!((best.count, best.rules.clone()), (1, Some(smart_rules)));
        assert_eq!(rows.iter().find(|r| r.id == reg).unwrap().count, 1);

        assert_eq!(
            quick_collection(conn).unwrap(),
            quick_collection(conn).unwrap()
        );
        delete_collection(conn, reg).unwrap();
        assert!(
            list_photos(
                conn,
                &PhotoSource::Collection(reg),
                &[],
                true,
                PhotoSort::Filename
            )
            .unwrap()
            .is_empty()
        );
    }
}
