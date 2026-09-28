//! Undoable catalog mutations (plan §4.3/§4.4: "Undoable action | Command |
//! Anywhere"). Adapted from the plan's sketch to what the rest of the
//! catalog crate already uses: `apply`/`revert` take a plain
//! `&rusqlite::Connection` (matching every `repo.rs` function) rather than
//! a separate `CatalogTx` wrapper, and return `Result<CatalogEvent>`
//! directly rather than a `ChangeSet` that would just get converted to one
//! — `archroom_core::events::CatalogEvent` already models "what changed."
//!
//! Each command captures the previous per-photo values on `apply`, so
//! `revert` can restore them exactly — the session-scoped undo stack that
//! holds these (plan §4.5) lives in `archroom-shell`, not here.

use archroom_core::events::{CatalogEvent, PhotoField};
use archroom_core::ids::PhotoId;
use rusqlite::{Connection, params};

use crate::error::Result;

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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::Catalog;
    use crate::repo::{self, NewFile, NewPhoto};

    fn seed_photo(conn: &Connection) -> PhotoId {
        let folder = repo::upsert_folder_path(conn, std::path::Path::new("/a")).unwrap();
        let file_id = repo::insert_file(
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
        repo::insert_photo(
            conn,
            &NewPhoto {
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
}
