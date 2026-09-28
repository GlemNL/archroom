//! Library's session-scoped undo/redo stack (plan §4.5: "Library keeps a
//! session-scoped command stack. Develop keeps a persisted history per
//! photo" — Develop's is separate, unbuilt-until-M3 territory; this is
//! only the Library one). Not persisted across restarts, matching
//! Lightroom.

use archroom_catalog::Result;
use archroom_catalog::command::Command;
use archroom_core::events::CatalogEvent;
use rusqlite::Connection;

#[derive(Default)]
pub struct UndoStack {
    undo: Vec<Box<dyn Command>>,
    redo: Vec<Box<dyn Command>>,
}

impl std::fmt::Debug for UndoStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UndoStack")
            .field("undo_depth", &self.undo.len())
            .field("redo_depth", &self.redo.len())
            .finish()
    }
}

impl UndoStack {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<String> {
        self.undo.last().map(|c| c.label())
    }

    pub fn redo_label(&self) -> Option<String> {
        self.redo.last().map(|c| c.label())
    }

    /// Applies `cmd`, pushes it onto the undo stack, and clears redo (a new
    /// action invalidates whatever was undone before it — standard
    /// undo-stack semantics).
    pub fn apply(&mut self, conn: &Connection, mut cmd: Box<dyn Command>) -> Result<CatalogEvent> {
        let event = cmd.apply(conn)?;
        self.undo.push(cmd);
        self.redo.clear();
        Ok(event)
    }

    pub fn undo(&mut self, conn: &Connection) -> Option<Result<CatalogEvent>> {
        let mut cmd = self.undo.pop()?;
        let result = cmd.revert(conn);
        if result.is_ok() {
            self.redo.push(cmd);
        } else {
            // Put it back — a failed revert shouldn't silently drop the
            // command from history.
            self.undo.push(cmd);
        }
        Some(result)
    }

    pub fn redo(&mut self, conn: &Connection) -> Option<Result<CatalogEvent>> {
        let mut cmd = self.redo.pop()?;
        let result = cmd.apply(conn);
        if result.is_ok() {
            self.undo.push(cmd);
        } else {
            self.redo.push(cmd);
        }
        Some(result)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use archroom_catalog::Catalog;
    use archroom_catalog::command::SetRating;
    use archroom_catalog::repo::{self, NewFile, NewPhoto};

    fn seed_photo(conn: &Connection) -> archroom_core::ids::PhotoId {
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

    fn rating_of(conn: &Connection, id: archroom_core::ids::PhotoId) -> i32 {
        conn.query_row("SELECT rating FROM photos WHERE id = ?1", [id.get()], |r| {
            r.get(0)
        })
        .unwrap()
    }

    #[test]
    fn undo_then_redo_restores_the_applied_value() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::create_or_open(dir.path().join("Test.arcat")).unwrap();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut stack = UndoStack::default();
        stack
            .apply(conn, Box::new(SetRating::new(vec![photo], 3)))
            .unwrap();
        assert_eq!(rating_of(conn, photo), 3);
        assert!(stack.can_undo());
        assert!(!stack.can_redo());

        stack.undo(conn).unwrap().unwrap();
        assert_eq!(rating_of(conn, photo), 0);
        assert!(!stack.can_undo());
        assert!(stack.can_redo());

        stack.redo(conn).unwrap().unwrap();
        assert_eq!(rating_of(conn, photo), 3);
    }

    #[test]
    fn a_new_command_clears_the_redo_stack() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::create_or_open(dir.path().join("Test.arcat")).unwrap();
        let conn = catalog.connection();
        let photo = seed_photo(conn);

        let mut stack = UndoStack::default();
        stack
            .apply(conn, Box::new(SetRating::new(vec![photo], 1)))
            .unwrap();
        stack.undo(conn).unwrap().unwrap();
        assert!(stack.can_redo());

        stack
            .apply(conn, Box::new(SetRating::new(vec![photo], 2)))
            .unwrap();
        assert!(!stack.can_redo(), "a new action should drop the old redo");
    }

    #[test]
    fn undo_on_an_empty_stack_is_a_harmless_none() {
        let mut stack = UndoStack::default();
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::create_or_open(dir.path().join("Test.arcat")).unwrap();
        assert!(stack.undo(catalog.connection()).is_none());
        assert!(stack.redo(catalog.connection()).is_none());
    }
}
