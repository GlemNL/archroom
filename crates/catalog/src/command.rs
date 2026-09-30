//! Undoable catalog mutations (plan §4.3/§4.4: "Undoable action | Command |
//! Anywhere"). Adapted from the plan's sketch to what the rest of the
//! catalog crate already uses: `apply`/`revert` take a plain
//! `&rusqlite::Connection` (matching every `repo.rs` function) rather than
//! a separate `CatalogTx` wrapper, and return `Result<CatalogEvent>`
//! directly rather than a `ChangeSet` that would just get converted to one
//! — `viberoom_core::events::CatalogEvent` already models "what changed."
//!
//! Each command captures the previous per-photo values on `apply`, so
//! `revert` can restore them exactly — the session-scoped undo stack that
//! holds these (plan §4.5) lives in `viberoom-shell`, not here.

use rusqlite::{Connection, OptionalExtension, params};
use viberoom_core::events::{CatalogEvent, PhotoField};
use viberoom_core::ids::{CollectionId, KeywordId, PhotoId};

use crate::error::Result;
use crate::{collections, repo};

pub trait Command: Send {
    fn label(&self) -> String;
    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent>;
    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent>;
}

fn changed(ids: &[PhotoId], field: PhotoField) -> CatalogEvent {
    CatalogEvent::PhotosChanged {
        ids: ids.to_vec(),
        fields: vec![field],
    }
}

fn photo_count_suffix(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[derive(Debug)]
pub struct SetRating {
    photo_ids: Vec<PhotoId>,
    new_rating: i32,
    previous: Vec<(PhotoId, i32)>,
}

impl SetRating {
    pub fn new(photo_ids: Vec<PhotoId>, rating: i32) -> Self {
        Self {
            photo_ids,
            new_rating: rating.clamp(0, 5),
            previous: Vec::new(),
        }
    }
}

impl Command for SetRating {
    fn label(&self) -> String {
        format!(
            "Set Rating {} ({} photo{})",
            self.new_rating,
            self.photo_ids.len(),
            photo_count_suffix(self.photo_ids.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        self.previous.clear();
        for &id in &self.photo_ids {
            let prev: i32 = conn.query_row(
                "SELECT rating FROM photos WHERE id = ?1",
                params![id.get()],
                |r| r.get(0),
            )?;
            self.previous.push((id, prev));
            conn.execute(
                "UPDATE photos SET rating = ?2 WHERE id = ?1",
                params![id.get(), self.new_rating],
            )?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Rating))
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        for &(id, prev) in &self.previous {
            conn.execute(
                "UPDATE photos SET rating = ?2 WHERE id = ?1",
                params![id.get(), prev],
            )?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Rating))
    }
}

/// -1 reject, 0 none, 1 pick (plan §5.1).
#[derive(Debug)]
pub struct SetFlag {
    photo_ids: Vec<PhotoId>,
    new_flag: i32,
    previous: Vec<(PhotoId, i32)>,
}

impl SetFlag {
    pub fn new(photo_ids: Vec<PhotoId>, flag: i32) -> Self {
        Self {
            photo_ids,
            new_flag: flag.clamp(-1, 1),
            previous: Vec::new(),
        }
    }
}

impl Command for SetFlag {
    fn label(&self) -> String {
        let word = match self.new_flag {
            1 => "Pick",
            -1 => "Reject",
            _ => "Unflag",
        };
        format!(
            "{word} ({} photo{})",
            self.photo_ids.len(),
            photo_count_suffix(self.photo_ids.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        self.previous.clear();
        for &id in &self.photo_ids {
            let prev: i32 = conn.query_row(
                "SELECT flag FROM photos WHERE id = ?1",
                params![id.get()],
                |r| r.get(0),
            )?;
            self.previous.push((id, prev));
            conn.execute(
                "UPDATE photos SET flag = ?2 WHERE id = ?1",
                params![id.get(), self.new_flag],
            )?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Flag))
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        for &(id, prev) in &self.previous {
            conn.execute(
                "UPDATE photos SET flag = ?2 WHERE id = ?1",
                params![id.get(), prev],
            )?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Flag))
    }
}

/// `None`, or one of Lightroom's five Adobe-convention names: Red, Yellow,
/// Green, Blue, Purple (plan D5, §5.3's XMP mapping).
#[derive(Debug)]
pub struct SetColorLabel {
    photo_ids: Vec<PhotoId>,
    new_label: Option<String>,
    previous: Vec<(PhotoId, Option<String>)>,
}

impl SetColorLabel {
    pub fn new(photo_ids: Vec<PhotoId>, label: Option<String>) -> Self {
        Self {
            photo_ids,
            new_label: label,
            previous: Vec::new(),
        }
    }
}

impl Command for SetColorLabel {
    fn label(&self) -> String {
        format!(
            "Set Label {} ({} photo{})",
            self.new_label.as_deref().unwrap_or("None"),
            self.photo_ids.len(),
            photo_count_suffix(self.photo_ids.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        self.previous.clear();
        for &id in &self.photo_ids {
            let prev: Option<String> = conn.query_row(
                "SELECT color_label FROM photos WHERE id = ?1",
                params![id.get()],
                |r| r.get(0),
            )?;
            self.previous.push((id, prev));
            conn.execute(
                "UPDATE photos SET color_label = ?2 WHERE id = ?1",
                params![id.get(), self.new_label],
            )?;
        }
        Ok(changed(&self.photo_ids, PhotoField::ColorLabel))
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        for (id, prev) in &self.previous {
            conn.execute(
                "UPDATE photos SET color_label = ?2 WHERE id = ?1",
                params![id.get(), prev],
            )?;
        }
        Ok(changed(&self.photo_ids, PhotoField::ColorLabel))
    }
}

/// Non-destructive rotate (plan §7.4): `delta` is in quarter turns
/// (+1 = 90° CW, -1 = 90° CCW), wrapped into `user_orientation`'s 0..=3.
#[derive(Debug)]
pub struct RotatePhotos {
    photo_ids: Vec<PhotoId>,
    delta: i32,
    previous: Vec<(PhotoId, i32)>,
}

impl RotatePhotos {
    pub fn new(photo_ids: Vec<PhotoId>, delta: i32) -> Self {
        Self {
            photo_ids,
            delta,
            previous: Vec::new(),
        }
    }
}

impl Command for RotatePhotos {
    fn label(&self) -> String {
        let dir = if self.delta >= 0 { "Right" } else { "Left" };
        format!(
            "Rotate {dir} ({} photo{})",
            self.photo_ids.len(),
            photo_count_suffix(self.photo_ids.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        self.previous.clear();
        for &id in &self.photo_ids {
            let prev: i32 = conn.query_row(
                "SELECT user_orientation FROM photos WHERE id = ?1",
                params![id.get()],
                |r| r.get(0),
            )?;
            self.previous.push((id, prev));
            let next = (prev + self.delta).rem_euclid(4);
            conn.execute(
                "UPDATE photos SET user_orientation = ?2 WHERE id = ?1",
                params![id.get(), next],
            )?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Orientation))
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        for &(id, prev) in &self.previous {
            conn.execute(
                "UPDATE photos SET user_orientation = ?2 WHERE id = ?1",
                params![id.get(), prev],
            )?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Orientation))
    }
}

/// Adds one or more keywords to a set of photos (plan §7.5). Each name is
/// a full path — `Places > France > Paris` creates the missing chain and
/// attaches the leaf (plan §7.6's hierarchical keywords); a bare `Paris`
/// is a one-segment path. Revert only removes the `(photo, keyword)`
/// pairs this command actually added — a keyword already on a photo
/// before `apply` stays untouched.
#[derive(Debug)]
pub struct AddKeywords {
    photo_ids: Vec<PhotoId>,
    names: Vec<String>,
    added: Vec<(PhotoId, KeywordId)>,
}

impl AddKeywords {
    pub fn new(photo_ids: Vec<PhotoId>, names: Vec<String>) -> Self {
        Self {
            photo_ids,
            names,
            added: Vec::new(),
        }
    }
}

impl Command for AddKeywords {
    fn label(&self) -> String {
        format!(
            "Add Keyword{} ({} photo{})",
            photo_count_suffix(self.names.len()),
            self.photo_ids.len(),
            photo_count_suffix(self.photo_ids.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        self.added.clear();
        for name in &self.names {
            let segments = repo::split_keyword_path(name);
            if segments.is_empty() {
                continue;
            }
            let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
            let keyword_id = repo::upsert_keyword_path(conn, &refs)?;
            for &photo_id in &self.photo_ids {
                let already_present: bool = conn.query_row(
                    "SELECT count(*) FROM photo_keywords WHERE photo_id = ?1 AND keyword_id = ?2",
                    params![photo_id.get(), keyword_id.get()],
                    |r| r.get::<_, i64>(0),
                )? > 0;
                if !already_present {
                    repo::add_photo_keyword(conn, photo_id, keyword_id)?;
                    self.added.push((photo_id, keyword_id));
                }
            }
        }
        Ok(changed(&self.photo_ids, PhotoField::Keywords))
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        for &(photo_id, keyword_id) in &self.added {
            repo::remove_photo_keyword(conn, photo_id, keyword_id)?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Keywords))
    }
}

/// Removes one or more keywords (by full path, matching
/// [`AddKeywords`]) from a set of photos. Revert only re-adds the
/// `(photo, keyword)` pairs this command actually removed.
#[derive(Debug)]
pub struct RemoveKeywords {
    photo_ids: Vec<PhotoId>,
    names: Vec<String>,
    removed: Vec<(PhotoId, KeywordId)>,
}

impl RemoveKeywords {
    pub fn new(photo_ids: Vec<PhotoId>, names: Vec<String>) -> Self {
        Self {
            photo_ids,
            names,
            removed: Vec::new(),
        }
    }
}

impl Command for RemoveKeywords {
    fn label(&self) -> String {
        format!(
            "Remove Keyword{} ({} photo{})",
            photo_count_suffix(self.names.len()),
            self.photo_ids.len(),
            photo_count_suffix(self.photo_ids.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        self.removed.clear();
        for name in &self.names {
            let segments = repo::split_keyword_path(name);
            if segments.is_empty() {
                continue;
            }
            let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
            let Some(keyword_id) = repo::find_keyword_by_path(conn, &refs)? else {
                continue;
            };
            for &photo_id in &self.photo_ids {
                let present: bool = conn.query_row(
                    "SELECT count(*) FROM photo_keywords WHERE photo_id = ?1 AND keyword_id = ?2",
                    params![photo_id.get(), keyword_id.get()],
                    |r| r.get::<_, i64>(0),
                )? > 0;
                if present {
                    repo::remove_photo_keyword(conn, photo_id, keyword_id)?;
                    self.removed.push((photo_id, keyword_id));
                }
            }
        }
        Ok(changed(&self.photo_ids, PhotoField::Keywords))
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        for &(photo_id, keyword_id) in &self.removed {
            repo::add_photo_keyword(conn, photo_id, keyword_id)?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Keywords))
    }
}

/// One editable IPTC core field (plan §7.6's Metadata panel): the `photos`
/// columns the panel writes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IptcField {
    Title,
    Caption,
    Creator,
    Copyright,
}

impl IptcField {
    pub fn label(self) -> &'static str {
        match self {
            IptcField::Title => "Title",
            IptcField::Caption => "Caption",
            IptcField::Creator => "Creator",
            IptcField::Copyright => "Copyright",
        }
    }

    fn column(self) -> &'static str {
        match self {
            IptcField::Title => "title",
            IptcField::Caption => "caption",
            IptcField::Creator => "creator",
            IptcField::Copyright => "copyright",
        }
    }
}

/// Sets one IPTC field on a set of photos (plan §7.6): `None` clears it.
/// Revert restores every old per-photo value — batch edits where the
/// photos started with different values still undo exactly.
#[derive(Debug)]
pub struct SetIptc {
    photo_ids: Vec<PhotoId>,
    field: IptcField,
    value: Option<String>,
    old: Vec<(PhotoId, Option<String>)>,
}

impl SetIptc {
    pub fn new(photo_ids: Vec<PhotoId>, field: IptcField, value: Option<String>) -> Self {
        Self {
            photo_ids,
            field,
            value,
            old: Vec::new(),
        }
    }
}

impl Command for SetIptc {
    fn label(&self) -> String {
        format!(
            "{} {} ({} photo{})",
            if self.value.is_some() { "Set" } else { "Clear" },
            self.field.label(),
            self.photo_ids.len(),
            photo_count_suffix(self.photo_ids.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        self.old.clear();
        let sql = format!("SELECT {} FROM photos WHERE id = ?1", self.field.column());
        for &photo_id in &self.photo_ids {
            let previous = conn
                .query_row(&sql, params![photo_id.get()], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .optional()?
                .flatten();
            self.old.push((photo_id, previous));
        }
        let sql = format!(
            "UPDATE photos SET {} = ?1 WHERE id = ?2",
            self.field.column()
        );
        for &photo_id in &self.photo_ids {
            conn.execute(&sql, params![self.value.as_deref(), photo_id.get()])?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Metadata))
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        for &(photo_id, ref previous) in &self.old {
            conn.execute(
                &format!(
                    "UPDATE photos SET {} = ?2 WHERE id = ?1",
                    self.field.column()
                ),
                params![photo_id.get(), previous],
            )?;
        }
        Ok(changed(&self.photo_ids, PhotoField::Metadata))
    }
}

/// Adds photos to a regular or Quick collection (plan §7.2). Undo removes
/// exactly the photos that were newly added, so pre-existing members stay.
#[derive(Debug)]
pub struct AddToCollection {
    collection: CollectionId,
    photo_ids: Vec<PhotoId>,
    added: Vec<PhotoId>,
}

impl AddToCollection {
    pub fn new(collection: CollectionId, photo_ids: Vec<PhotoId>) -> Self {
        Self {
            collection,
            photo_ids,
            added: Vec::new(),
        }
    }
}

impl Command for AddToCollection {
    fn label(&self) -> String {
        format!(
            "Add to collection ({} photo{})",
            self.photo_ids.len(),
            photo_count_suffix(self.photo_ids.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        self.added = collections::add_photos(conn, self.collection, &self.photo_ids)?;
        Ok(CatalogEvent::CollectionsChanged {
            ids: vec![self.collection],
        })
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        collections::remove_photos(conn, self.collection, &self.added)?;
        Ok(CatalogEvent::CollectionsChanged {
            ids: vec![self.collection],
        })
    }
}

/// Removes photos from a regular or Quick collection; undo re-adds them.
#[derive(Debug)]
pub struct RemoveFromCollection {
    collection: CollectionId,
    photo_ids: Vec<PhotoId>,
}

impl RemoveFromCollection {
    pub fn new(collection: CollectionId, photo_ids: Vec<PhotoId>) -> Self {
        Self {
            collection,
            photo_ids,
        }
    }
}

impl Command for RemoveFromCollection {
    fn label(&self) -> String {
        format!(
            "Remove from collection ({} photo{})",
            self.photo_ids.len(),
            photo_count_suffix(self.photo_ids.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        collections::remove_photos(conn, self.collection, &self.photo_ids)?;
        Ok(CatalogEvent::CollectionsChanged {
            ids: vec![self.collection],
        })
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        collections::add_photos(conn, self.collection, &self.photo_ids)?;
        Ok(CatalogEvent::CollectionsChanged {
            ids: vec![self.collection],
        })
    }
}

/// Creates virtual copies (plan §7.4): new `photos` rows over the same
/// file that carry the source's rating, flag, label, metadata, keywords and
/// develop settings, and edit independently afterwards. Undo deletes them.
#[derive(Debug)]
pub struct CreateVirtualCopies {
    sources: Vec<PhotoId>,
    created: Vec<PhotoId>,
}

impl CreateVirtualCopies {
    pub fn new(sources: Vec<PhotoId>) -> Self {
        Self {
            sources,
            created: Vec::new(),
        }
    }

    pub fn created(&self) -> &[PhotoId] {
        &self.created
    }
}

impl Command for CreateVirtualCopies {
    fn label(&self) -> String {
        format!(
            "Create Virtual Cop{} ({} photo{})",
            if self.sources.len() == 1 { "y" } else { "ies" },
            self.sources.len(),
            photo_count_suffix(self.sources.len())
        )
    }

    fn apply(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        self.created.clear();
        for &src in &self.sources {
            let existing: i64 = conn.query_row(
                "SELECT count(*) FROM photos
                 WHERE file_id = (SELECT file_id FROM photos WHERE id = ?1)
                   AND copy_name IS NOT NULL",
                params![src.get()],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT INTO photos (file_id, copy_name, rating, flag, color_label, title,
                                     caption, creator, copyright, user_orientation,
                                     import_id, imported_at)
                 SELECT file_id, ?2, rating, flag, color_label, title, caption, creator,
                        copyright, user_orientation, import_id, datetime('now')
                 FROM photos WHERE id = ?1",
                params![src.get(), format!("Copy {}", existing + 1)],
            )?;
            let copy = PhotoId::new(conn.last_insert_rowid());
            conn.execute(
                "INSERT INTO photo_keywords (photo_id, keyword_id)
                 SELECT ?2, keyword_id FROM photo_keywords WHERE photo_id = ?1",
                params![src.get(), copy.get()],
            )?;
            conn.execute(
                "INSERT INTO develop_settings (photo_id, process_version, params, params_hash)
                 SELECT ?2, process_version, params, params_hash
                 FROM develop_settings WHERE photo_id = ?1",
                params![src.get(), copy.get()],
            )?;
            self.created.push(copy);
        }
        Ok(CatalogEvent::PhotosAdded {
            ids: self.created.clone(),
            import_id: None,
        })
    }

    fn revert(&mut self, conn: &Connection) -> Result<CatalogEvent> {
        for id in &self.created {
            for table in [
                "photo_keywords",
                "develop_settings",
                "history",
                "snapshots",
                "collection_photos",
            ] {
                conn.execute(
                    &format!("DELETE FROM {table} WHERE photo_id = ?1"),
                    params![id.get()],
                )?;
            }
            conn.execute("DELETE FROM photos WHERE id = ?1", params![id.get()])?;
        }
        Ok(CatalogEvent::PhotosRemoved {
            ids: std::mem::take(&mut self.created),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::Catalog;
    use crate::repo;

    fn seed_photo(conn: &Connection) -> PhotoId {
        seed_photo_named(conn, "a.jpg")
    }

    fn seed_photo_named(conn: &Connection, filename: &str) -> PhotoId {
        let folder = repo::upsert_folder_path(conn, std::path::Path::new("/a")).unwrap();
        let file_id = repo::insert_file(
            conn,
            &repo::NewFile {
                folder_id: folder,
                filename,
                ext: "jpg",
                kind: "jpeg",
                ..Default::default()
            },
        )
        .unwrap();
        repo::insert_photo(
            conn,
            &repo::NewPhoto {
                file_id,
                import_id: None,
            },
        )
        .unwrap()
    }

    fn open_test_catalog() -> (tempfile::TempDir, Catalog) {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = Catalog::create_or_open(dir.path().join("Test.arcat")).expect("create");
        (dir, catalog)
    }

    #[test]
    fn set_rating_applies_and_reverts() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut cmd = SetRating::new(vec![photo], 4);
        cmd.apply(conn).unwrap();
        let rating: i32 = conn
            .query_row(
                "SELECT rating FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rating, 4);

        cmd.revert(conn).unwrap();
        let rating: i32 = conn
            .query_row(
                "SELECT rating FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rating, 0);
    }

    #[test]
    fn set_rating_clamps_out_of_range_values() {
        assert_eq!(SetRating::new(vec![], 9).new_rating, 5);
        assert_eq!(SetRating::new(vec![], -3).new_rating, 0);
    }

    #[test]
    fn set_flag_round_trips() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut cmd = SetFlag::new(vec![photo], 1);
        cmd.apply(conn).unwrap();
        let flag: i32 = conn
            .query_row(
                "SELECT flag FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(flag, 1);

        cmd.revert(conn).unwrap();
        let flag: i32 = conn
            .query_row(
                "SELECT flag FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(flag, 0);
    }

    #[test]
    fn set_color_label_round_trips_through_none() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut cmd = SetColorLabel::new(vec![photo], Some("Red".to_string()));
        cmd.apply(conn).unwrap();
        let label: Option<String> = conn
            .query_row(
                "SELECT color_label FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(label, Some("Red".to_string()));

        cmd.revert(conn).unwrap();
        let label: Option<String> = conn
            .query_row(
                "SELECT color_label FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(label, None);
    }

    #[test]
    fn rotate_wraps_and_reverts() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut right = RotatePhotos::new(vec![photo], 1);
        right.apply(conn).unwrap();
        let o: i32 = conn
            .query_row(
                "SELECT user_orientation FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(o, 1);

        // 0 - 1 wraps to 3, not -1.
        let mut left = RotatePhotos::new(vec![photo], -1);
        left.apply(conn).unwrap();
        let o: i32 = conn
            .query_row(
                "SELECT user_orientation FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(o, 0);

        left.revert(conn).unwrap();
        let o: i32 = conn
            .query_row(
                "SELECT user_orientation FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(o, 1);
    }

    #[test]
    fn add_keywords_creates_and_attaches_then_reverts() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut cmd = AddKeywords::new(vec![photo], vec!["Sunset".into(), "Beach".into()]);
        cmd.apply(conn).unwrap();
        assert_eq!(repo::list_keywords_for_photo(conn, photo).unwrap().len(), 2);

        cmd.revert(conn).unwrap();
        assert!(
            repo::list_keywords_for_photo(conn, photo)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn add_keywords_does_not_disturb_an_already_present_keyword_on_revert() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);
        let kw = repo::upsert_keyword(conn, None, "Sunset").unwrap();
        repo::add_photo_keyword(conn, photo, kw).unwrap();

        let mut cmd = AddKeywords::new(vec![photo], vec!["Sunset".into()]);
        cmd.apply(conn).unwrap();
        cmd.revert(conn).unwrap();

        assert_eq!(
            repo::list_keywords_for_photo(conn, photo).unwrap(),
            vec![kw]
        );
    }

    #[test]
    fn remove_keywords_detaches_then_reverts() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);
        let kw = repo::upsert_keyword(conn, None, "Sunset").unwrap();
        repo::add_photo_keyword(conn, photo, kw).unwrap();

        let mut cmd = RemoveKeywords::new(vec![photo], vec!["Sunset".into()]);
        cmd.apply(conn).unwrap();
        assert!(
            repo::list_keywords_for_photo(conn, photo)
                .unwrap()
                .is_empty()
        );

        cmd.revert(conn).unwrap();
        assert_eq!(
            repo::list_keywords_for_photo(conn, photo).unwrap(),
            vec![kw]
        );
    }

    #[test]
    fn remove_keywords_ignores_unknown_names() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut cmd = RemoveKeywords::new(vec![photo], vec!["Nonexistent".into()]);
        cmd.apply(conn).unwrap();
        cmd.revert(conn).unwrap();
    }

    #[test]
    fn add_keywords_creates_the_full_hierarchy_and_attaches_the_leaf() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut cmd = AddKeywords::new(vec![photo], vec!["Places > France > Paris".into()]);
        cmd.apply(conn).unwrap();

        assert_eq!(
            repo::keyword_paths_for_photo(conn, photo).unwrap(),
            vec!["Places > France > Paris"]
        );

        // Revert detaches the leaf; the now-unused chain stays in the
        // keyword table (like Lightroom, keywords aren't garbage-collected).
        cmd.revert(conn).unwrap();
        assert!(
            repo::list_keywords_for_photo(conn, photo)
                .unwrap()
                .is_empty()
        );
        assert!(
            repo::find_keyword_by_path(conn, &["Places", "France", "Paris"])
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn remove_keywords_matches_by_full_path() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);
        let mut cmd = AddKeywords::new(
            vec![photo],
            vec!["Places > France > Paris".into(), "Paris".into()],
        );
        cmd.apply(conn).unwrap();

        // "Paris" alone is a different keyword than "Places > France >
        // Paris": removing it must leave the hierarchical one attached.
        let mut cmd = RemoveKeywords::new(vec![photo], vec!["Paris".into()]);
        cmd.apply(conn).unwrap();
        assert_eq!(
            repo::keyword_paths_for_photo(conn, photo).unwrap(),
            vec!["Places > France > Paris"]
        );

        let mut cmd = RemoveKeywords::new(vec![photo], vec!["Places > France > Paris".into()]);
        cmd.apply(conn).unwrap();
        assert!(
            repo::keyword_paths_for_photo(conn, photo)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn set_iptc_applies_and_reverts_per_photo() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let a = seed_photo(conn);
        let b = seed_photo_named(conn, "b.jpg");
        conn.execute(
            "UPDATE photos SET title = 'Old A' WHERE id = ?1",
            params![a.get()],
        )
        .unwrap();

        let mut cmd = SetIptc::new(vec![a, b], IptcField::Title, Some("New".into()));
        cmd.apply(conn).unwrap();
        for id in [a, b] {
            let title: Option<String> = conn
                .query_row(
                    "SELECT title FROM photos WHERE id = ?1",
                    params![id.get()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(title.as_deref(), Some("New"));
        }

        // Revert restores what each photo had before, not a blanket value.
        cmd.revert(conn).unwrap();
        let title_a: Option<String> = conn
            .query_row(
                "SELECT title FROM photos WHERE id = ?1",
                params![a.get()],
                |r| r.get(0),
            )
            .unwrap();
        let title_b: Option<String> = conn
            .query_row(
                "SELECT title FROM photos WHERE id = ?1",
                params![b.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title_a.as_deref(), Some("Old A"));
        assert_eq!(title_b, None);
    }

    #[test]
    fn set_iptc_clear_sets_the_field_to_null() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut cmd = SetIptc::new(vec![photo], IptcField::Creator, Some("Me".into()));
        cmd.apply(conn).unwrap();
        let mut cmd = SetIptc::new(vec![photo], IptcField::Creator, None);
        cmd.apply(conn).unwrap();
        let creator: Option<String> = conn
            .query_row(
                "SELECT creator FROM photos WHERE id = ?1",
                params![photo.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(creator, None);
    }

    #[test]
    fn add_to_collection_undo_removes_only_new_members() {
        let (_dir, catalog) = open_test_catalog();
        let conn = catalog.connection();
        let a = seed_photo_named(conn, "a.jpg");
        let b = seed_photo_named(conn, "b.jpg");
        let coll = crate::collections::create_collection(
            conn,
            crate::collections::CollectionKind::Regular,
            "C",
            None,
            None,
        )
        .unwrap();
        crate::collections::add_photos(conn, coll, &[a]).unwrap();

        let count = |conn: &Connection| -> i64 {
            conn.query_row("SELECT count(*) FROM collection_photos", [], |r| r.get(0))
                .unwrap()
        };
        let mut cmd = AddToCollection::new(coll, vec![a, b]);
        cmd.apply(conn).unwrap();
        assert_eq!(count(conn), 2);
        cmd.revert(conn).unwrap();
        assert_eq!(count(conn), 1, "pre-existing member `a` must survive undo");

        let mut rm = RemoveFromCollection::new(coll, vec![a]);
        rm.apply(conn).unwrap();
        assert_eq!(count(conn), 0);
        rm.revert(conn).unwrap();
        assert_eq!(count(conn), 1);
    }

    #[test]
    fn virtual_copies_carry_settings_and_undo_removes_them() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::create_or_open(dir.path().join("t.arcat")).unwrap();
        let conn = catalog.connection();
        let src = seed_photo(conn);
        conn.execute(
            "UPDATE photos SET rating = 4 WHERE id = ?1",
            params![src.get()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO develop_settings (photo_id, process_version, params, params_hash)
             VALUES (?1, 1, '{}', x'00')",
            params![src.get()],
        )
        .unwrap();

        let mut cmd = CreateVirtualCopies::new(vec![src]);
        assert!(matches!(
            cmd.apply(conn).unwrap(),
            CatalogEvent::PhotosAdded { .. }
        ));
        let copy = cmd.created()[0];
        let (rating, name, file_same): (i32, String, bool) = conn
            .query_row(
                "SELECT c.rating, c.copy_name, c.file_id = s.file_id
                 FROM photos c, photos s WHERE c.id = ?1 AND s.id = ?2",
                params![copy.get(), src.get()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((rating, name.as_str(), file_same), (4, "Copy 1", true));
        let has_settings: i64 = conn
            .query_row(
                "SELECT count(*) FROM develop_settings WHERE photo_id = ?1",
                params![copy.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_settings, 1);

        // A second copy of the same photo gets the next number.
        let mut second = CreateVirtualCopies::new(vec![src]);
        second.apply(conn).unwrap();
        let n: String = conn
            .query_row(
                "SELECT copy_name FROM photos WHERE id = ?1",
                params![second.created()[0].get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, "Copy 2");

        cmd.revert(conn).unwrap();
        let left: i64 = conn
            .query_row(
                "SELECT count(*) FROM photos WHERE id = ?1",
                params![copy.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }
}
