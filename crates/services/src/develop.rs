//! Develop persistence glue (plan §5.2/§8.1): `EditParams` ⇄ the catalog's
//! `develop_settings`/`history` tables. The catalog stores opaque JSON;
//! the hash and the identity rule ("unedited ⇒ no row") live here.

use rusqlite::Connection;
use viberoom_catalog::develop::{self, DevelopRow};
use viberoom_core::events::{CatalogEvent, PhotoField};
use viberoom_core::ids::PhotoId;
use viberoom_engine::EditParams;

use crate::error::Result;

/// Slider drags on one control within this window are a single history step.
pub const COALESCE_SECS: i64 = 2;

/// The photo's current settings; defaults when it has never been edited or
/// its stored JSON can't be read (never fail a load over bad params).
pub fn load_params(conn: &Connection, photo: PhotoId) -> Result<EditParams> {
    let Some(row) = develop::get_settings(conn, photo)? else {
        return Ok(EditParams::default());
    };
    Ok(EditParams::from_json(&row.params).unwrap_or_else(|e| {
        tracing::warn!(photo = photo.get(), error = %e, "unreadable develop settings; using defaults");
        EditParams::default()
    }))
}

fn changed(photo: PhotoId) -> CatalogEvent {
    CatalogEvent::PhotosChanged {
        ids: vec![photo],
        fields: vec![PhotoField::DevelopSettings],
    }
}

fn write_current(conn: &Connection, photo: PhotoId, params: &EditParams) -> Result<()> {
    if params.is_identity() {
        develop::clear_settings(conn, photo)?;
    } else {
        develop::set_settings(
            conn,
            photo,
            &DevelopRow {
                process_version: params.process_version,
                params: params.to_json(),
                params_hash: params.hash().to_vec(),
            },
        )?;
    }
    Ok(())
}

/// Stores `params` as the photo's current settings and records a history
/// step named `label`. `cursor` is the history seq the user is currently at
/// (after undo); steps beyond it are forgotten first. Returns the step's seq
/// and the event to publish.
pub fn save_edit(
    conn: &Connection,
    photo: PhotoId,
    label: &str,
    params: &EditParams,
    cursor: Option<i64>,
) -> Result<(i64, CatalogEvent)> {
    if let Some(seq) = cursor {
        develop::truncate_history_after(conn, photo, seq)?;
    }
    write_current(conn, photo, params)?;
    let seq = develop::record_history(conn, photo, label, &params.to_json(), COALESCE_SECS)?;
    Ok((seq, changed(photo)))
}

/// Sets current settings without touching history (undo/redo/snapshot apply).
pub fn apply_params(
    conn: &Connection,
    photo: PhotoId,
    params: &EditParams,
) -> Result<CatalogEvent> {
    write_current(conn, photo, params)?;
    Ok(changed(photo))
}

/// Resets to defaults as one undoable history step ("Reset").
pub fn reset(
    conn: &Connection,
    photo: PhotoId,
    cursor: Option<i64>,
) -> Result<(i64, CatalogEvent)> {
    save_edit(conn, photo, "Reset", &EditParams::default(), cursor)
}

/// Where a history walk lands: the seq to treat as "current" (0 = before
/// the first step) and the params at that point.
pub type HistoryTarget = (i64, EditParams);

fn params_of(row: &develop::HistoryRow) -> EditParams {
    EditParams::from_json(&row.params).unwrap_or_default()
}

/// Index into `rows` of the current step; `None` means before the first.
fn position(rows: &[develop::HistoryRow], cursor: Option<i64>) -> Option<usize> {
    match cursor {
        None => rows.len().checked_sub(1),
        Some(seq) => rows.iter().rposition(|r| r.seq <= seq),
    }
}

/// The state one step before `cursor` (`None` = at the newest step).
pub fn undo_target(rows: &[develop::HistoryRow], cursor: Option<i64>) -> Option<HistoryTarget> {
    let i = position(rows, cursor)?;
    Some(match i.checked_sub(1) {
        Some(prev) => (rows[prev].seq, params_of(&rows[prev])),
        None => (0, EditParams::default()),
    })
}

/// The state one step after `cursor`; `None` when already at the newest.
pub fn redo_target(rows: &[develop::HistoryRow], cursor: Option<i64>) -> Option<HistoryTarget> {
    let next = position(rows, cursor).map_or(0, |i| i + 1);
    let row = rows.get(next)?;
    Some((row.seq, params_of(row)))
}

/// Copies the settings groups `groups` from `source` onto `target`: a group
/// the source leaves at its defaults resets the target's (copy/paste,
/// presets and sync all work this way, plan §8.4).
pub fn paste_groups(
    target: &mut EditParams,
    source: &EditParams,
    groups: &[viberoom_engine::SettingsGroup],
) {
    let registry = viberoom_engine::ops::default_registry();
    for op in registry.iter() {
        if !groups.contains(&op.group()) {
            continue;
        }
        match source.ops.get(op.id()) {
            Some(v) => {
                target.ops.insert(op.id().to_string(), v.clone());
            }
            None => {
                target.ops.remove(op.id());
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use viberoom_catalog::Catalog;
    use viberoom_catalog::repo::{self, NewFile, NewPhoto};
    use viberoom_engine::ops::{Exposure, ExposureParams};

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
    fn edits_persist_render_identically_after_reopen_and_reset_clears_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("T.arcat");
        let (id, params);
        {
            let c = Catalog::create_or_open(&path).unwrap();
            id = photo(c.connection());
            let mut p = EditParams::default();
            p.set::<Exposure>(ExposureParams { ev: 0.35 });
            params = p;
            save_edit(c.connection(), id, "Exposure", &params, None).unwrap();
        }
        let c = Catalog::create_or_open(&path).unwrap();
        let conn = c.connection();
        let loaded = load_params(conn, id).unwrap();
        assert_eq!(loaded, params);
        assert_eq!(
            loaded.hash(),
            params.hash(),
            "same params ⇒ same render cache key"
        );
        let stored = develop::get_settings(conn, id).unwrap().unwrap();
        assert_eq!(stored.params_hash, params.hash().to_vec());

        let (seq, _) = reset(conn, id, None).unwrap();
        assert_eq!(seq, 2);
        assert!(load_params(conn, id).unwrap().is_identity());
        assert!(
            develop::get_settings(conn, id).unwrap().is_none(),
            "unedited ⇒ no row"
        );
        assert_eq!(develop::list_history(conn, id).unwrap().len(), 2);
    }

    #[test]
    fn editing_after_undo_forgets_the_undone_future() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::create_or_open(dir.path().join("T.arcat")).unwrap();
        let conn = c.connection();
        let id = photo(conn);
        let mk = |ev| {
            let mut p = EditParams::default();
            p.set::<Exposure>(ExposureParams { ev });
            p
        };
        save_edit(conn, id, "Exposure", &mk(1.0), None).unwrap();
        save_edit(conn, id, "Contrast", &mk(2.0), None).unwrap();
        save_edit(conn, id, "Shadows", &mk(3.0), None).unwrap();
        // User undid to step 1, then edited: steps 2 and 3 are dropped.
        let (seq, _) = save_edit(conn, id, "Blacks", &mk(4.0), Some(1)).unwrap();
        assert_eq!(seq, 2);
        let labels: Vec<_> = develop::list_history(conn, id)
            .unwrap()
            .into_iter()
            .map(|r| r.label)
            .collect();
        assert_eq!(labels, ["Exposure", "Blacks"]);
    }

    fn row(seq: i64, ev: f64) -> develop::HistoryRow {
        let mut p = EditParams::default();
        p.set::<Exposure>(ExposureParams { ev });
        develop::HistoryRow {
            id: seq,
            seq,
            label: "Exposure".into(),
            params: p.to_json(),
        }
    }

    #[test]
    fn undo_and_redo_walk_the_history() {
        let rows = [row(1, 0.1), row(2, 0.2), row(3, 0.3)];
        let (seq, p) = undo_target(&rows, None).unwrap();
        assert_eq!((seq, p.get::<Exposure>().ev), (2, 0.2));
        let (seq, p) = undo_target(&rows, Some(1)).unwrap();
        assert_eq!((seq, p.is_identity()), (0, true));
        assert!(undo_target(&rows, Some(0)).is_none());
        let (seq, p) = redo_target(&rows, Some(0)).unwrap();
        assert_eq!((seq, p.get::<Exposure>().ev), (1, 0.1));
        assert!(redo_target(&rows, None).is_none());
        assert!(redo_target(&rows, Some(3)).is_none());
    }

    #[test]
    fn paste_resets_selected_groups_the_source_leaves_default() {
        use viberoom_engine::SettingsGroup;
        let mut target = EditParams::default();
        target.set::<Exposure>(ExposureParams { ev: 1.0 });
        let source = EditParams::default();
        paste_groups(&mut target, &source, &[SettingsGroup::WhiteBalance]);
        assert_eq!(target.get::<Exposure>().ev, 1.0, "other groups untouched");
        paste_groups(&mut target, &source, &[SettingsGroup::BasicTone]);
        assert!(target.is_identity());
    }
}
