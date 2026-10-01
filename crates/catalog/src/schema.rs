//! Catalog schema, v1 (plan §5.1). Each element of [`migrations`] is one
//! `rusqlite_migration::M`; a later schema change appends a new one and
//! never edits an already-released migration.

use rusqlite_migration::{M, Migrations};

const V1: &str = r#"
CREATE TABLE folders (
  id        INTEGER PRIMARY KEY,
  parent_id INTEGER REFERENCES folders(id),
  path      TEXT NOT NULL UNIQUE,
  name      TEXT NOT NULL
);

CREATE TABLE files (
  id INTEGER PRIMARY KEY,
  folder_id INTEGER NOT NULL REFERENCES folders(id),
  filename TEXT NOT NULL,
  ext TEXT NOT NULL,
  kind TEXT NOT NULL,
  size INTEGER,
  mtime INTEGER,
  quick_hash BLOB,
  width INTEGER,
  height INTEGER,
  orientation INTEGER,
  capture_time TEXT,
  camera_make TEXT,
  camera_model TEXT,
  lens TEXT,
  focal_length REAL,
  aperture REAL,
  shutter REAL,
  iso INTEGER,
  gps_lat REAL,
  gps_lon REAL,
  sidecar_mtime INTEGER,
  missing INTEGER NOT NULL DEFAULT 0,
  UNIQUE (folder_id, filename)
);
CREATE INDEX idx_files_folder_id ON files(folder_id);
CREATE INDEX idx_files_capture_time ON files(capture_time);
CREATE INDEX idx_files_camera_model ON files(camera_model);
CREATE INDEX idx_files_lens ON files(lens);

CREATE TABLE imports (
  id INTEGER PRIMARY KEY,
  started_at TEXT,
  source TEXT,
  mode TEXT,
  count INTEGER
);

CREATE TABLE photos (
  id INTEGER PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES files(id),
  copy_name TEXT,
  rating INTEGER NOT NULL DEFAULT 0,
  flag INTEGER NOT NULL DEFAULT 0,
  color_label TEXT,
  title TEXT,
  caption TEXT,
  creator TEXT,
  copyright TEXT,
  user_orientation INTEGER NOT NULL DEFAULT 0,
  import_id INTEGER REFERENCES imports(id),
  imported_at TEXT,
  edited_at TEXT
);
CREATE INDEX idx_photos_file_id ON photos(file_id);
CREATE INDEX idx_photos_rating ON photos(rating);
CREATE INDEX idx_photos_flag ON photos(flag);
CREATE INDEX idx_photos_color_label ON photos(color_label);
CREATE INDEX idx_photos_import_id ON photos(import_id);

CREATE TABLE develop_settings (
  photo_id INTEGER PRIMARY KEY REFERENCES photos(id),
  process_version INTEGER NOT NULL,
  params TEXT NOT NULL,
  params_hash BLOB NOT NULL
);

CREATE TABLE history (
  id INTEGER PRIMARY KEY,
  photo_id INTEGER NOT NULL REFERENCES photos(id),
  seq INTEGER NOT NULL,
  label TEXT NOT NULL,
  params TEXT NOT NULL,
  created_at TEXT
);
CREATE INDEX idx_history_photo_id ON history(photo_id);

CREATE TABLE snapshots (
  id INTEGER PRIMARY KEY,
  photo_id INTEGER NOT NULL REFERENCES photos(id),
  name TEXT NOT NULL,
  params TEXT NOT NULL,
  created_at TEXT
);
CREATE INDEX idx_snapshots_photo_id ON snapshots(photo_id);

CREATE TABLE keywords (
  id INTEGER PRIMARY KEY,
  parent_id INTEGER REFERENCES keywords(id),
  name TEXT NOT NULL,
  include_on_export INTEGER NOT NULL DEFAULT 1,
  UNIQUE (parent_id, name)
);

CREATE TABLE photo_keywords (
  photo_id INTEGER NOT NULL REFERENCES photos(id),
  keyword_id INTEGER NOT NULL REFERENCES keywords(id),
  PRIMARY KEY (photo_id, keyword_id)
);
CREATE INDEX idx_photo_keywords_keyword_id ON photo_keywords(keyword_id);

CREATE TABLE collections (
  id INTEGER PRIMARY KEY,
  parent_id INTEGER REFERENCES collections(id),
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  rules TEXT,
  sort TEXT
);

CREATE TABLE collection_photos (
  collection_id INTEGER NOT NULL REFERENCES collections(id),
  photo_id INTEGER NOT NULL REFERENCES photos(id),
  position REAL,
  PRIMARY KEY (collection_id, photo_id)
);
CREATE INDEX idx_collection_photos_photo_id ON collection_photos(photo_id);

-- Standalone (not external-content) FTS5 table: it keeps its own copy of the
-- text, so plain INSERT/DELETE against it (keyed by rowid = photo id) is
-- enough to stay in sync; no special `photo_fts` command column is needed.
CREATE VIRTUAL TABLE photo_fts USING fts5(filename, title, caption, keywords);

CREATE TRIGGER trg_photos_ai AFTER INSERT ON photos BEGIN
  INSERT INTO photo_fts(rowid, filename, title, caption, keywords)
  VALUES (
    new.id,
    (SELECT filename FROM files WHERE id = new.file_id),
    new.title,
    new.caption,
    ''
  );
END;

CREATE TRIGGER trg_photos_ad AFTER DELETE ON photos BEGIN
  DELETE FROM photo_fts WHERE rowid = old.id;
END;

CREATE TRIGGER trg_photos_au AFTER UPDATE OF title, caption ON photos BEGIN
  DELETE FROM photo_fts WHERE rowid = old.id;
  INSERT INTO photo_fts(rowid, filename, title, caption, keywords)
  VALUES (
    new.id,
    (SELECT filename FROM files WHERE id = new.file_id),
    new.title,
    new.caption,
    ''
  );
END;

CREATE TABLE schema_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
INSERT INTO schema_meta(key, value) VALUES ('created_by', 'viberoom');
"#;

/// Develop presets (plan §8.1): a named set of settings groups plus the
/// params they take from.
const V2: &str = r#"
CREATE TABLE presets (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  folder TEXT NOT NULL DEFAULT 'User Presets',
  groups TEXT NOT NULL,
  params TEXT NOT NULL,
  created_at TEXT
);
"#;

/// Export presets (plan §9): a named, opaque JSON blob of export settings;
/// its meaning lives in `viberoom-services`.
const V3: &str = r#"
CREATE TABLE export_presets (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  settings TEXT NOT NULL
);
"#;

/// Soft delete: "Remove from catalog" only stamps
/// `removed_at`, so undo can bring the photo back with everything it had.
/// Rows are purged for good at the next catalog open (`repo::purge_removed`).
const V4: &str = r#"
ALTER TABLE photos ADD COLUMN removed_at TEXT;
CREATE INDEX idx_photos_removed_at ON photos(removed_at);
"#;

pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(V1), M::up(V2), M::up(V3), M::up(V4)])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_cleanly_to_a_fresh_connection() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open");
        migrations().to_latest(&mut conn).expect("migrate");

        let table_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'photos'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        assert_eq!(table_count, 1);
    }

    #[test]
    fn migrations_validate() {
        migrations().validate().expect("migrations should validate");
    }
}
