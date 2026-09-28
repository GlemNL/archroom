//! A thin repository layer over a plain `rusqlite::Connection` (plan §5.1),
//! covering what import needs: folders, files and photos. Free functions
//! rather than `Catalog` methods, since import runs against its own
//! connection to the same `.arcat` file (see the M1 plan note on why a
//! background job doesn't share the UI thread's `Catalog`).

use archroom_core::ids::{FileId, FolderId, ImportId, KeywordId, PhotoId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

/// Finds or creates the `folders` row for `path`, walking up and creating
/// every missing ancestor so the Folders tree (plan §7.2) has a full chain
/// to walk later. Callers importing many files from the same tree should
/// cache the result themselves — this always does at least one query.
pub fn upsert_folder_path(conn: &Connection, path: &std::path::Path) -> Result<FolderId> {
    let path_str = path.to_string_lossy();
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM folders WHERE path = ?1",
            params![path_str],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(FolderId::new(id));
    }

    // Stop at the filesystem root rather than creating a folder row for it:
    // nobody browses a Folders panel entry named "/".
    let parent_id = match path.parent() {
        Some(parent) if parent != path && parent.file_name().is_some() => {
            Some(upsert_folder_path(conn, parent)?)
        }
        _ => None,
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_str.to_string());

    conn.execute(
        "INSERT INTO folders (parent_id, path, name) VALUES (?1, ?2, ?3)",
        params![parent_id.map(FolderId::get), path_str, name],
    )?;
    Ok(FolderId::new(conn.last_insert_rowid()))
}

/// A duplicate-detection candidate: an already-catalogued file with the same
/// filename, size and capture time as one about to be imported (plan §7.1).
/// The caller confirms with a quick-hash comparison before treating it as a
/// true duplicate, since filename/size/time can coincide by chance.
#[derive(Debug, Clone)]
pub struct DuplicateCandidate {
    pub file_id: FileId,
    pub quick_hash: Option<Vec<u8>>,
}

pub fn find_duplicate_candidate(
    conn: &Connection,
    filename: &str,
    size: Option<i64>,
    capture_time: Option<&str>,
) -> Result<Option<DuplicateCandidate>> {
    conn.query_row(
        "SELECT id, quick_hash FROM files
         WHERE filename = ?1 AND size IS ?2 AND capture_time IS ?3
         LIMIT 1",
        params![filename, size, capture_time],
        |row| {
            Ok(DuplicateCandidate {
                file_id: FileId::new(row.get(0)?),
                quick_hash: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// The exact-path duplicate check: re-importing the same folder is a no-op
/// because `(folder_id, filename)` is `UNIQUE`.
pub fn find_file(conn: &Connection, folder_id: FolderId, filename: &str) -> Result<Option<FileId>> {
    conn.query_row(
        "SELECT id FROM files WHERE folder_id = ?1 AND filename = ?2",
        params![folder_id.get(), filename],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map(|opt| opt.map(FileId::new))
    .map_err(Into::into)
}

#[derive(Debug, Clone, Default)]
pub struct NewFile<'a> {
    pub folder_id: FolderId,
    pub filename: &'a str,
    pub ext: &'a str,
    pub kind: &'a str,
    pub size: Option<i64>,
    pub mtime: Option<i64>,
    pub quick_hash: Option<&'a [u8]>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<i32>,
    pub capture_time: Option<&'a str>,
    pub camera_make: Option<&'a str>,
    pub camera_model: Option<&'a str>,
    pub lens: Option<&'a str>,
    pub focal_length: Option<f32>,
    pub aperture: Option<f32>,
    pub shutter: Option<f32>,
    pub iso: Option<u32>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

pub fn insert_file(conn: &Connection, f: &NewFile<'_>) -> Result<FileId> {
    conn.execute(
        "INSERT INTO files (
            folder_id, filename, ext, kind, size, mtime, quick_hash,
            width, height, orientation, capture_time,
            camera_make, camera_model, lens, focal_length, aperture, shutter, iso,
            gps_lat, gps_lon
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
        )",
        params![
            f.folder_id.get(),
            f.filename,
            f.ext,
            f.kind,
            f.size,
            f.mtime,
            f.quick_hash,
            f.width,
            f.height,
            f.orientation,
            f.capture_time,
            f.camera_make,
            f.camera_model,
            f.lens,
            f.focal_length,
            f.aperture,
            f.shutter,
            f.iso,
            f.gps_lat,
            f.gps_lon,
        ],
    )?;
    Ok(FileId::new(conn.last_insert_rowid()))
}

pub fn create_import(conn: &Connection, source: &str, mode: &str) -> Result<ImportId> {
    conn.execute(
        "INSERT INTO imports (started_at, source, mode, count) VALUES (datetime('now'), ?1, ?2, 0)",
        params![source, mode],
    )?;
    Ok(ImportId::new(conn.last_insert_rowid()))
}

pub fn finish_import(conn: &Connection, id: ImportId, count: u32) -> Result<()> {
    conn.execute(
        "UPDATE imports SET count = ?2 WHERE id = ?1",
        params![id.get(), count],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct NewPhoto {
    pub file_id: FileId,
    pub import_id: Option<ImportId>,
}

pub fn insert_photo(conn: &Connection, p: &NewPhoto) -> Result<PhotoId> {
    conn.execute(
        "INSERT INTO photos (file_id, import_id, imported_at) VALUES (?1, ?2, datetime('now'))",
        params![p.file_id.get(), p.import_id.map(ImportId::get)],
    )?;
    Ok(PhotoId::new(conn.last_insert_rowid()))
}

// ---------------------------------------------------------------------
// Read queries (plan §7.3/§7.2): what the Grid, Loupe and the Catalog and
// Folders panels need. All read-only, so plain `&Connection` is enough —
// no transaction needed.
// ---------------------------------------------------------------------

/// A row the Grid/Loupe/filmstrip need: enough of `files` + `photos` joined
/// together to render a cell and its badges, without a second query per
/// photo.
#[derive(Debug, Clone)]
pub struct PhotoSummary {
    pub photo_id: PhotoId,
    pub file_id: FileId,
    pub folder_id: FolderId,
    pub filename: String,
    pub kind: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<i32>,
    pub capture_time: Option<String>,
    pub camera_model: Option<String>,
    pub rating: i32,
    pub flag: i32,
    pub color_label: Option<String>,
    pub missing: bool,
    /// Quarter turns applied by the user (0..=3), plan §7.4's non-destructive rotate.
    pub user_orientation: i32,
}

const PHOTO_SUMMARY_COLUMNS: &str =
    "p.id, f.id, f.folder_id, f.filename, f.kind, f.width, f.height,
     f.orientation, f.capture_time, f.camera_model, p.rating, p.flag, p.color_label, f.missing,
     p.user_orientation";

fn photo_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PhotoSummary> {
    Ok(PhotoSummary {
        photo_id: PhotoId::new(row.get(0)?),
        file_id: FileId::new(row.get(1)?),
        folder_id: FolderId::new(row.get(2)?),
        filename: row.get(3)?,
        kind: row.get(4)?,
        width: row.get::<_, Option<i64>>(5)?.map(|v| v as u32),
        height: row.get::<_, Option<i64>>(6)?.map(|v| v as u32),
        orientation: row.get(7)?,
        capture_time: row.get(8)?,
        camera_model: row.get(9)?,
        rating: row.get(10)?,
        flag: row.get(11)?,
        color_label: row.get(12)?,
        missing: row.get::<_, i64>(13)? != 0,
        user_orientation: row.get(14)?,
    })
}

/// How to order the Grid (plan §7.4's sort list; a subset for M1 — edit
/// time, rating, flag, label and file type join once those actions exist).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhotoSort {
    /// Most recent first (Lightroom's default "Capture Time" sort).
    #[default]
    CaptureTime,
    Filename,
    ImportOrder,
}

impl PhotoSort {
    fn order_by(self) -> &'static str {
        match self {
            PhotoSort::CaptureTime => "f.capture_time DESC, f.filename ASC",
            PhotoSort::Filename => "f.filename ASC",
            PhotoSort::ImportOrder => "p.id ASC",
        }
    }
}

pub fn list_all_photos(conn: &Connection, sort: PhotoSort) -> Result<Vec<PhotoSummary>> {
    let sql = format!(
        "SELECT {PHOTO_SUMMARY_COLUMNS} FROM photos p JOIN files f ON f.id = p.file_id
         ORDER BY {}",
        sort.order_by()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], photo_summary_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn list_photos_for_folder(
    conn: &Connection,
    folder_id: FolderId,
    sort: PhotoSort,
) -> Result<Vec<PhotoSummary>> {
    let sql = format!(
        "SELECT {PHOTO_SUMMARY_COLUMNS} FROM photos p JOIN files f ON f.id = p.file_id
         WHERE f.folder_id = ?1
         ORDER BY {}",
        sort.order_by()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![folder_id.get()], photo_summary_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn list_photos_for_import(
    conn: &Connection,
    import_id: ImportId,
    sort: PhotoSort,
) -> Result<Vec<PhotoSummary>> {
    let sql = format!(
        "SELECT {PHOTO_SUMMARY_COLUMNS} FROM photos p JOIN files f ON f.id = p.file_id
         WHERE p.import_id = ?1
         ORDER BY {}",
        sort.order_by()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![import_id.get()], photo_summary_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn latest_import_id(conn: &Connection) -> Result<Option<ImportId>> {
    conn.query_row(
        "SELECT id FROM imports ORDER BY id DESC LIMIT 1",
        [],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map(|opt| opt.map(ImportId::new))
    .map_err(Into::into)
}

pub fn count_all_photos(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT count(*) FROM photos", [], |row| row.get(0))
        .map_err(Into::into)
}

/// A `folders` row plus how many files sit directly in it. Not a recursive
/// rollup across subfolders (a Folders-panel nicety, Later) — just this
/// folder's own count, which is enough to confirm import landed somewhere
/// sane.
#[derive(Debug, Clone)]
pub struct FolderRow {
    pub id: FolderId,
    pub parent_id: Option<FolderId>,
    pub name: String,
    pub path: String,
    pub file_count: i64,
}

pub fn list_folders(conn: &Connection) -> Result<Vec<FolderRow>> {
    let mut stmt = conn.prepare(
        "SELECT fo.id, fo.parent_id, fo.name, fo.path, count(fi.id)
         FROM folders fo LEFT JOIN files fi ON fi.folder_id = fo.id
         GROUP BY fo.id
         ORDER BY fo.path",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FolderRow {
            id: FolderId::new(row.get(0)?),
            parent_id: row.get::<_, Option<i64>>(1)?.map(FolderId::new),
            name: row.get(2)?,
            path: row.get(3)?,
            file_count: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

// ---------------------------------------------------------------------
// Keywords (plan §7.5): flat CRUD on `keywords` plus the `photo_keywords`
// join. `photo_fts.keywords` is NOT covered by a trigger — unlike
// title/caption it has no single source column to watch, so every caller
// that adds or removes a photo_keywords row must also call
// `resync_photo_fts_keywords` for the affected photo(s) in the same
// transaction.
// ---------------------------------------------------------------------

/// A `keywords` row plus how many photos it's attached to, for the Keyword
/// List panel (plan §7.5).
#[derive(Debug, Clone)]
pub struct KeywordRow {
    pub id: KeywordId,
    pub parent_id: Option<KeywordId>,
    pub name: String,
    pub include_on_export: bool,
    pub photo_count: i64,
}

/// Finds or creates the `keywords` row named `name` under `parent_id`.
pub fn upsert_keyword(
    conn: &Connection,
    parent_id: Option<KeywordId>,
    name: &str,
) -> Result<KeywordId> {
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM keywords WHERE parent_id IS ?1 AND name = ?2",
            params![parent_id.map(KeywordId::get), name],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(KeywordId::new(id));
    }
    conn.execute(
        "INSERT INTO keywords (parent_id, name) VALUES (?1, ?2)",
        params![parent_id.map(KeywordId::get), name],
    )?;
    Ok(KeywordId::new(conn.last_insert_rowid()))
}

pub fn list_keywords(conn: &Connection) -> Result<Vec<KeywordRow>> {
    let mut stmt = conn.prepare(
        "SELECT k.id, k.parent_id, k.name, k.include_on_export, count(pk.photo_id)
         FROM keywords k LEFT JOIN photo_keywords pk ON pk.keyword_id = k.id
         GROUP BY k.id
         ORDER BY k.name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(KeywordRow {
            id: KeywordId::new(row.get(0)?),
            parent_id: row.get::<_, Option<i64>>(1)?.map(KeywordId::new),
            name: row.get(2)?,
            include_on_export: row.get::<_, i64>(3)? != 0,
            photo_count: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn list_keywords_for_photo(conn: &Connection, photo_id: PhotoId) -> Result<Vec<KeywordId>> {
    let mut stmt = conn.prepare(
        "SELECT keyword_id FROM photo_keywords WHERE photo_id = ?1 ORDER BY keyword_id",
    )?;
    let rows = stmt.query_map(params![photo_id.get()], |row| {
        Ok(KeywordId::new(row.get(0)?))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn add_photo_keyword(conn: &Connection, photo_id: PhotoId, keyword_id: KeywordId) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO photo_keywords (photo_id, keyword_id) VALUES (?1, ?2)",
        params![photo_id.get(), keyword_id.get()],
    )?;
    resync_photo_fts_keywords(conn, photo_id)
}

pub fn remove_photo_keyword(
    conn: &Connection,
    photo_id: PhotoId,
    keyword_id: KeywordId,
) -> Result<()> {
    conn.execute(
        "DELETE FROM photo_keywords WHERE photo_id = ?1 AND keyword_id = ?2",
        params![photo_id.get(), keyword_id.get()],
    )?;
    resync_photo_fts_keywords(conn, photo_id)
}

/// Rebuilds `photo_fts.keywords` for one photo from its current
/// `photo_keywords` rows. `photo_fts` is a standalone (non-external-content)
/// FTS5 table, so it must be updated with a delete+insert rather than a
/// plain `UPDATE`.
fn resync_photo_fts_keywords(conn: &Connection, photo_id: PhotoId) -> Result<()> {
    let joined: String = conn.query_row(
        "SELECT coalesce(group_concat(k.name, ' '), '')
         FROM photo_keywords pk JOIN keywords k ON k.id = pk.keyword_id
         WHERE pk.photo_id = ?1",
        params![photo_id.get()],
        |row| row.get(0),
    )?;
    let (filename, title, caption): (String, Option<String>, Option<String>) = conn.query_row(
        "SELECT f.filename, p.title, p.caption
         FROM photos p JOIN files f ON f.id = p.file_id
         WHERE p.id = ?1",
        params![photo_id.get()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    conn.execute(
        "DELETE FROM photo_fts WHERE rowid = ?1",
        params![photo_id.get()],
    )?;
    conn.execute(
        "INSERT INTO photo_fts (rowid, filename, title, caption, keywords) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![photo_id.get(), filename, title, caption, joined],
    )?;
    Ok(())
}

/// The read-only EXIF block the Metadata panel shows for one photo (plan
/// §7.6): straight out of `files`, in display form. A separate query from
/// `PhotoSummary` because the Grid never needs lens/exposure/GPS columns
/// but the panel always does.
#[derive(Debug, Clone, Default)]
pub struct PhotoExif {
    pub filename: String,
    pub kind: String,
    pub file_size: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub capture_time: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<f64>,
    pub aperture: Option<f64>,
    pub shutter: Option<f64>,
    pub iso: Option<i64>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

pub fn photo_exif(conn: &Connection, photo_id: PhotoId) -> Result<PhotoExif> {
    conn.query_row(
        "SELECT f.filename, f.kind, f.size, f.width, f.height, f.capture_time,
                f.camera_make, f.camera_model, f.lens, f.focal_length,
                f.aperture, f.shutter, f.iso, f.gps_lat, f.gps_lon
         FROM photos p JOIN files f ON f.id = p.file_id
         WHERE p.id = ?1",
        params![photo_id.get()],
        |row| {
            Ok(PhotoExif {
                filename: row.get(0)?,
                kind: row.get(1)?,
                file_size: row.get(2)?,
                width: row.get::<_, Option<i64>>(3)?.map(|v| v as u32),
                height: row.get::<_, Option<i64>>(4)?.map(|v| v as u32),
                capture_time: row.get(5)?,
                camera_make: row.get(6)?,
                camera_model: row.get(7)?,
                lens: row.get(8)?,
                focal_length: row.get(9)?,
                aperture: row.get(10)?,
                shutter: row.get(11)?,
                iso: row.get(12)?,
                gps_lat: row.get(13)?,
                gps_lon: row.get(14)?,
            })
        },
    )
    .map_err(Into::into)
}

/// The editable IPTC core (plan §7.6): `photos` columns the Metadata panel
/// writes back through the `SetIptc` command.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoIptc {
    pub title: Option<String>,
    pub caption: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
}

pub fn photo_iptc(conn: &Connection, photo_id: PhotoId) -> Result<PhotoIptc> {
    conn.query_row(
        "SELECT title, caption, creator, copyright FROM photos WHERE id = ?1",
        params![photo_id.get()],
        |row| {
            Ok(PhotoIptc {
                title: row.get(0)?,
                caption: row.get(1)?,
                creator: row.get(2)?,
                copyright: row.get(3)?,
            })
        },
    )
    .map_err(Into::into)
}

/// Every keyword attached to the photo, as its full display path
/// (`"Places > France"`, plan §7.6) — the flat list the Keywording panel's
/// chips and the XMP `dc:subject` round-trip both use.
pub fn keyword_paths_for_photo(conn: &Connection, photo_id: PhotoId) -> Result<Vec<String>> {
    let ids = list_keywords_for_photo(conn, photo_id)?;
    let by_id: std::collections::HashMap<KeywordId, (Option<KeywordId>, String)> =
        list_keywords(conn)?
            .into_iter()
            .map(|k| (k.id, (k.parent_id, k.name)))
            .collect();
    let mut paths = Vec::with_capacity(ids.len());
    for id in ids {
        // Walk up from the keyword; a cycle or a missing parent just
        // truncates the path (both are impossible through `upsert_keyword`).
        let mut segments: Vec<String> = Vec::new();
        let mut cur = Some(id);
        while let Some(kid) = cur
            && let Some((parent, name)) = by_id.get(&kid)
        {
            segments.push(name.clone());
            cur = *parent;
        }
        if !segments.is_empty() {
            segments.reverse();
            paths.push(segments.join(" > "));
        }
    }
    paths.sort();
    Ok(paths)
}

/// The `keywords.include_on_export` toggle (plan §7.6's hierarchical
/// keywords row). Right-click in the Keyword List panel.
pub fn set_keyword_include_on_export(
    conn: &Connection,
    keyword_id: KeywordId,
    include: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE keywords SET include_on_export = ?2 WHERE id = ?1",
        params![keyword_id.get(), include],
    )?;
    Ok(())
}


/// Everything a sidecar write needs for one photo (plan §5.3's mapping):
/// the image's path on disk plus the catalog fields `io::xmp` serializes.
/// `None` when the photo no longer exists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoXmpSnapshot {
    pub file_id: FileId,
    pub image_path: std::path::PathBuf,
    /// True when another file in the same folder shares the basename
    /// (a RAW+JPEG pair), so the sidecar must use the full
    /// `IMG_0001.CR3.xmp` name to avoid colliding (plan D5).
    pub same_stem_sibling: bool,
    pub rating: i32,
    pub flag: i32,
    pub color_label: Option<String>,
    pub keywords: Vec<String>,
    pub title: Option<String>,
    pub caption: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
}

pub fn photo_xmp_snapshot(conn: &Connection, photo_id: PhotoId) -> Result<Option<PhotoXmpSnapshot>> {
    let Some((file_id, folder_path, filename)) = conn
        .query_row(
            "SELECT f.id, fo.path, f.filename
             FROM photos p JOIN files f ON f.id = p.file_id JOIN folders fo ON fo.id = f.folder_id
             WHERE p.id = ?1",
            params![photo_id.get()],
            |row| {
                Ok((
                    FileId::new(row.get::<_, i64>(0)?),
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };

    let stem = std::path::Path::new(&filename)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| filename.clone());
    let same_stem_sibling: bool = conn.query_row(
        "SELECT count(*) FROM files
         WHERE folder_id = (SELECT folder_id FROM files WHERE id = ?1)
           AND id != ?1
           AND substr(filename, 1, ?3) = ?2 || '.'",
        params![file_id.get(), stem, (stem.len() + 1) as i64],
        |r| r.get::<_, i64>(0),
    )? > 0;

    let (rating, flag, color_label, title, caption, creator, copyright) = conn.query_row(
        "SELECT rating, flag, color_label, title, caption, creator, copyright
         FROM photos WHERE id = ?1",
        params![photo_id.get()],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        },
    )?;

    // Flat keyword names (dc:subject): every attached keyword's own name,
    // not its full path — hierarchicalSubject is Later (plan §5.3).
    let keywords: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT k.name FROM photo_keywords pk JOIN keywords k ON k.id = pk.keyword_id
             WHERE pk.photo_id = ?1 ORDER BY k.name",
        )?;
        let rows = stmt.query_map(params![photo_id.get()], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    Ok(Some(PhotoXmpSnapshot {
        file_id,
        image_path: std::path::PathBuf::from(folder_path).join(filename),
        same_stem_sibling,
        rating,
        flag,
        color_label,
        keywords,
        title,
        caption,
        creator,
        copyright,
    }))
}

/// Records (or clears) `files.sidecar_mtime`, so folder sync can later spot
/// a sidecar that changed on disk since the last write (plan §5.3, MVP+).
pub fn set_sidecar_mtime(conn: &Connection, file_id: FileId, mtime: Option<i64>) -> Result<()> {
    conn.execute(
        "UPDATE files SET sidecar_mtime = ?2 WHERE id = ?1",
        params![file_id.get(), mtime],
    )?;
    Ok(())
}

/// The metadata a sidecar brings to a freshly imported photo (plan §5.3
/// "existing sidecars are read on import"): rating, flag, label and the
/// IPTC core in one UPDATE. Keywords go through `upsert_keyword`/
/// `add_photo_keyword` separately. The `photos` UPDATE fires the existing
/// FTS trigger, keeping `title`/`caption` searchable.
#[derive(Debug, Clone, Copy, Default)]
pub struct SidecarMetadata<'a> {
    pub rating: i32,
    pub flag: i32,
    pub color_label: Option<&'a str>,
    pub title: Option<&'a str>,
    pub caption: Option<&'a str>,
    pub creator: Option<&'a str>,
    pub copyright: Option<&'a str>,
}

pub fn apply_sidecar_metadata(
    conn: &Connection,
    photo_id: PhotoId,
    m: SidecarMetadata<'_>,
) -> Result<()> {
    conn.execute(
        "UPDATE photos SET rating = ?2, flag = ?3, color_label = ?4,
                           title = ?5, caption = ?6, creator = ?7, copyright = ?8
         WHERE id = ?1",
        params![
            photo_id.get(),
            m.rating,
            m.flag,
            m.color_label,
            m.title,
            m.caption,
            m.creator,
            m.copyright,
        ],
    )?;
    Ok(())
}

/// Resolves a full keyword path (`["Places", "France"]`) to its row id, or
/// `None` when any segment is missing. The command layer's lookup for
/// `RemoveKeywords` and the Keyword List panel.
pub fn find_keyword_by_path(conn: &Connection, path: &[&str]) -> Result<Option<KeywordId>> {
    let mut current: Option<KeywordId> = None;
    for segment in path {
        let found: Option<i64> = conn
            .query_row(
                "SELECT id FROM keywords WHERE parent_id IS ?1 AND name = ?2",
                params![current.map(KeywordId::get), segment],
                |row| row.get(0),
            )
            .optional()?;
        match found {
            Some(id) => current = Some(KeywordId::new(id)),
            None => return Ok(None),
        }
    }
    Ok(current)
}

/// Finds or creates the whole chain for a keyword path and returns the
/// leaf's id — `AddKeywords`'s hierarchy support (plan §7.6).
pub fn upsert_keyword_path(conn: &Connection, path: &[&str]) -> Result<KeywordId> {
    let mut current: Option<KeywordId> = None;
    for segment in path {
        current = Some(upsert_keyword(conn, current, segment)?);
    }
    current.ok_or_else(|| {
        crate::Error::Other("keyword path is empty".into())
    })
}

/// Splits a user-typed keyword into path segments: `Places > France >
/// Paris`, `Places>France`, and `Paris` all work (plan §7.6).
pub fn split_keyword_path(input: &str) -> Vec<String> {
    input
        .split('>')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::Catalog;

    fn open_test_catalog() -> (tempfile::TempDir, Catalog) {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = Catalog::create_or_open(dir.path().join("Test.arcat")).expect("create");
        (dir, catalog)
    }

    #[test]
    fn upsert_folder_path_creates_the_full_ancestor_chain() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();

        let leaf = upsert_folder_path(conn, std::path::Path::new("/a/b/c")).unwrap();
        let again = upsert_folder_path(conn, std::path::Path::new("/a/b/c")).unwrap();
        assert_eq!(leaf, again, "the same path must resolve to the same row");

        let count: i64 = conn
            .query_row("SELECT count(*) FROM folders", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 3, "/a, /a/b, /a/b/c — the root itself is elided");
    }

    #[test]
    fn insert_file_then_find_file_round_trips() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let folder = upsert_folder_path(conn, std::path::Path::new("/photos")).unwrap();

        assert!(find_file(conn, folder, "img.nef").unwrap().is_none());

        let file_id = insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename: "img.nef",
                ext: "nef",
                kind: "raw",
                camera_make: Some("Nikon"),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(find_file(conn, folder, "img.nef").unwrap(), Some(file_id));
    }

    #[test]
    fn duplicate_candidate_matches_on_filename_size_and_capture_time() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let folder = upsert_folder_path(conn, std::path::Path::new("/a")).unwrap();

        insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename: "img.nef",
                ext: "nef",
                kind: "raw",
                size: Some(1234),
                capture_time: Some("2026-01-01T00:00:00"),
                quick_hash: Some(&[1, 2, 3, 4]),
                ..Default::default()
            },
        )
        .unwrap();

        let hit =
            find_duplicate_candidate(conn, "img.nef", Some(1234), Some("2026-01-01T00:00:00"))
                .unwrap()
                .expect("should find the candidate");
        assert_eq!(hit.quick_hash, Some(vec![1, 2, 3, 4]));

        assert!(
            find_duplicate_candidate(conn, "img.nef", Some(999), Some("2026-01-01T00:00:00"))
                .unwrap()
                .is_none(),
            "a different size must not match"
        );
    }

    #[test]
    fn import_and_photo_round_trip() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let folder = upsert_folder_path(conn, std::path::Path::new("/a")).unwrap();
        let file_id = insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename: "img.nef",
                ext: "nef",
                kind: "raw",
                ..Default::default()
            },
        )
        .unwrap();

        let import_id = create_import(conn, "/a", "add").unwrap();
        let photo_id = insert_photo(
            conn,
            &NewPhoto {
                file_id,
                import_id: Some(import_id),
            },
        )
        .unwrap();
        finish_import(conn, import_id, 1).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count FROM imports WHERE id = ?1",
                params![import_id.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let stored_file: i64 = conn
            .query_row(
                "SELECT file_id FROM photos WHERE id = ?1",
                params![photo_id.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored_file, file_id.get());
    }

    fn seed_photo(
        conn: &Connection,
        folder: FolderId,
        filename: &str,
        capture_time: &str,
        import_id: ImportId,
    ) -> PhotoId {
        let file_id = insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename,
                ext: "jpg",
                kind: "jpeg",
                capture_time: Some(capture_time),
                ..Default::default()
            },
        )
        .unwrap();
        insert_photo(
            conn,
            &NewPhoto {
                file_id,
                import_id: Some(import_id),
            },
        )
        .unwrap()
    }

    #[test]
    fn list_all_photos_sorts_by_capture_time_descending_by_default() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let folder = upsert_folder_path(conn, std::path::Path::new("/a")).unwrap();
        let import_id = create_import(conn, "/a", "add").unwrap();

        let older = seed_photo(conn, folder, "a.jpg", "2026-01-01T00:00:00", import_id);
        let newer = seed_photo(conn, folder, "b.jpg", "2026-06-01T00:00:00", import_id);

        let photos = list_all_photos(conn, PhotoSort::CaptureTime).unwrap();
        let ids: Vec<_> = photos.iter().map(|p| p.photo_id).collect();
        assert_eq!(ids, vec![newer, older]);

        let by_name = list_all_photos(conn, PhotoSort::Filename).unwrap();
        assert_eq!(by_name[0].filename, "a.jpg");
    }

    #[test]
    fn list_photos_for_folder_and_import_filter_correctly() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let folder_a = upsert_folder_path(conn, std::path::Path::new("/a")).unwrap();
        let folder_b = upsert_folder_path(conn, std::path::Path::new("/b")).unwrap();
        let import1 = create_import(conn, "/a", "add").unwrap();
        let import2 = create_import(conn, "/b", "add").unwrap();

        let p1 = seed_photo(conn, folder_a, "a.jpg", "2026-01-01T00:00:00", import1);
        let p2 = seed_photo(conn, folder_b, "b.jpg", "2026-01-02T00:00:00", import2);

        let in_folder_a = list_photos_for_folder(conn, folder_a, PhotoSort::Filename).unwrap();
        assert_eq!(
            in_folder_a.iter().map(|p| p.photo_id).collect::<Vec<_>>(),
            vec![p1]
        );

        let in_import2 = list_photos_for_import(conn, import2, PhotoSort::Filename).unwrap();
        assert_eq!(
            in_import2.iter().map(|p| p.photo_id).collect::<Vec<_>>(),
            vec![p2]
        );

        assert_eq!(latest_import_id(conn).unwrap(), Some(import2));
        assert_eq!(count_all_photos(conn).unwrap(), 2);
    }

    #[test]
    fn list_folders_reports_direct_file_counts() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let parent = upsert_folder_path(conn, std::path::Path::new("/a")).unwrap();
        let child = upsert_folder_path(conn, std::path::Path::new("/a/b")).unwrap();
        let import_id = create_import(conn, "/a", "add").unwrap();
        seed_photo(conn, child, "x.jpg", "2026-01-01T00:00:00", import_id);

        let folders = list_folders(conn).unwrap();
        let parent_row = folders.iter().find(|f| f.id == parent).unwrap();
        let child_row = folders.iter().find(|f| f.id == child).unwrap();

        assert_eq!(parent_row.parent_id, None);
        assert_eq!(parent_row.file_count, 0);
        assert_eq!(child_row.parent_id, Some(parent));
        assert_eq!(child_row.file_count, 1);
    }

    fn seed_bare_photo(conn: &Connection) -> PhotoId {
        let folder = upsert_folder_path(conn, std::path::Path::new("/a")).unwrap();
        let file_id = insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename: "a.jpg",
                ext: "jpg",
                kind: "jpeg",
                ..Default::default()
            },
        )
        .unwrap();
        insert_photo(
            conn,
            &NewPhoto {
                file_id,
                import_id: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn upsert_keyword_dedupes_by_parent_and_name() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();

        let a = upsert_keyword(conn, None, "Animals").unwrap();
        let again = upsert_keyword(conn, None, "Animals").unwrap();
        assert_eq!(a, again);

        let child = upsert_keyword(conn, Some(a), "Dogs").unwrap();
        assert_ne!(a, child);
    }

    #[test]
    fn add_and_remove_photo_keyword_updates_fts_and_counts() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_bare_photo(conn);
        let kw = upsert_keyword(conn, None, "Sunset").unwrap();

        add_photo_keyword(conn, photo, kw).unwrap();
        assert_eq!(list_keywords_for_photo(conn, photo).unwrap(), vec![kw]);

        let matched: i64 = conn
            .query_row(
                "SELECT count(*) FROM photo_fts WHERE photo_fts MATCH 'Sunset'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(matched, 1);

        let rows = list_keywords(conn).unwrap();
        assert_eq!(rows.iter().find(|r| r.id == kw).unwrap().photo_count, 1);

        remove_photo_keyword(conn, photo, kw).unwrap();
        assert!(list_keywords_for_photo(conn, photo).unwrap().is_empty());

        let matched: i64 = conn
            .query_row(
                "SELECT count(*) FROM photo_fts WHERE photo_fts MATCH 'Sunset'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(matched, 0);
    }

    #[test]
    fn photo_xmp_snapshot_collects_fields_and_detects_stem_siblings() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let folder = upsert_folder_path(conn, std::path::Path::new("/raw+jpg")).unwrap();
        let raw = insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename: "IMG_0001.CR3",
                ext: "cr3",
                kind: "raw",
                ..Default::default()
            },
        )
        .unwrap();
        insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename: "IMG_0001.jpg",
                ext: "jpg",
                kind: "jpeg",
                ..Default::default()
            },
        )
        .unwrap();
        let lone = insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename: "solo.jpg",
                ext: "jpg",
                kind: "jpeg",
                ..Default::default()
            },
        )
        .unwrap();
        let raw_photo = insert_photo(conn, &NewPhoto { file_id: raw, import_id: None }).unwrap();
        let lone_photo = insert_photo(conn, &NewPhoto { file_id: lone, import_id: None }).unwrap();

        conn.execute(
            "UPDATE photos SET rating = 3, title = 'Sunrise' WHERE id = ?1",
            params![raw_photo.get()],
        )
        .unwrap();
        let kw = upsert_keyword(conn, None, "Sunrise").unwrap();
        add_photo_keyword(conn, raw_photo, kw).unwrap();

        let snap = photo_xmp_snapshot(conn, raw_photo).unwrap().unwrap();
        assert_eq!(snap.image_path, std::path::PathBuf::from("/raw+jpg/IMG_0001.CR3"));
        assert!(snap.same_stem_sibling, "the RAW+JPEG pair collides on IMG_0001.xmp");
        assert_eq!(snap.rating, 3);
        assert_eq!(snap.title.as_deref(), Some("Sunrise"));
        assert_eq!(snap.keywords, vec!["Sunrise"]);

        let lone_snap = photo_xmp_snapshot(conn, lone_photo).unwrap().unwrap();
        assert!(!lone_snap.same_stem_sibling);

        // A stem that merely shares a prefix must not count as a sibling.
        let prefix = insert_file(
            conn,
            &NewFile {
                folder_id: folder,
                filename: "IMG_00010.jpg",
                ext: "jpg",
                kind: "jpeg",
                ..Default::default()
            },
        )
        .unwrap();
        let prefix_photo = insert_photo(conn, &NewPhoto { file_id: prefix, import_id: None }).unwrap();
        assert!(!photo_xmp_snapshot(conn, prefix_photo).unwrap().unwrap().same_stem_sibling);
    }
}
