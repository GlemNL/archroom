//! Generates a preview at a given level/size and stores it in the
//! [`PreviewCache`] (plan §5.4). Shared by import (always generates L1) and
//! the Loupe view (generates L2 lazily, on first open of a photo).

use std::path::{Path, PathBuf};

use viberoom_core::ids::PhotoId;
use viberoom_io::{DecodeOptions, DecodedImage, decoder_for};
use viberoom_preview::{
    DEFAULT_PARAMS_HASH, PreviewCache, generate_preview_from_display_rgb,
    generate_preview_from_jpeg_bytes, generate_preview_from_linear_rgb,
};

use crate::error::{Error, Result};

/// Generates a preview for `path` at `budget_px` and stores it under
/// `level` in `previews`, returning the file it wrote. Prefers the
/// decoder's embedded thumbnail when there is one (fast); otherwise falls
/// back to a full decode (plan §5.4's L0→L1 relationship).
pub fn generate_and_store(
    previews: &PreviewCache,
    path: &Path,
    photo_id: PhotoId,
    level: &str,
    budget_px: u32,
) -> Result<PathBuf> {
    let decoder = decoder_for(path)
        .ok_or_else(|| Error::Other(format!("no decoder for {}", path.display())))?;

    // Neither the embedded thumbnails nor the decoders rotate, so turn the
    // pixels upright here, the way Develop does (EXIF, then user turns on top).
    let meta = decoder.metadata(path)?;
    let (jpeg, w, h) = match decoder.embedded_preview(path)? {
        Some(bytes) => {
            // Some cameras embed a thumbnail that is already upright; then
            // its aspect no longer matches the sensor's and it needs no turn.
            let mut orientation = meta.orientation;
            if matches!(orientation, 5..=8)
                && let Ok(size) = image::ImageReader::new(std::io::Cursor::new(&bytes))
                    .with_guessed_format()
                    .and_then(|r| r.into_dimensions().map_err(std::io::Error::other))
                && (size.0 >= size.1) != (meta.width >= meta.height)
            {
                orientation = 1;
            }
            generate_preview_from_jpeg_bytes(&bytes, budget_px, orientation)?
        }
        None => match decoder.decode(path, &DecodeOptions::default())? {
            DecodedImage::SceneLinear { rgb, .. } => {
                generate_preview_from_linear_rgb(&rgb, budget_px, meta.orientation)?
            }
            DecodedImage::Rendered { rgb, .. } => {
                generate_preview_from_display_rgb(&rgb, budget_px, meta.orientation)?
            }
        },
    };
    Ok(previews.store(photo_id, level, DEFAULT_PARAMS_HASH, &jpeg, w, h)?)
}

/// Gives each `(master, copy)` pair's copy the master's previews (L1 and L2)
/// so a new virtual copy shows its picture immediately.
pub fn copy_previews(previews: &PreviewCache, pairs: &[(PhotoId, PhotoId)]) {
    for &(src, dst) in pairs {
        for level in [viberoom_preview::LEVEL_L1, viberoom_preview::LEVEL_L2] {
            if let Err(e) = previews.copy_entry(src, dst, level) {
                tracing::warn!(error = %e, "could not copy a preview to a virtual copy");
            }
        }
    }
}

/// Fills in previews for virtual copies that have none (made before
/// `copy_previews` existed, or whose preview was trimmed): each copy borrows
/// from another photo using the same file.
pub fn backfill_virtual_copy_previews(
    conn: &rusqlite::Connection,
    previews: &PreviewCache,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT c.id, s.id FROM photos c JOIN photos s ON s.file_id = c.file_id AND s.id != c.id
         WHERE c.copy_name IS NOT NULL AND c.removed_at IS NULL AND s.removed_at IS NULL
         ORDER BY c.id, s.copy_name IS NOT NULL, s.id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((PhotoId::new(r.get(0)?), PhotoId::new(r.get(1)?)))
    })?;
    for row in rows {
        let (copy, sibling) = row?;
        if previews
            .lookup(copy, viberoom_preview::LEVEL_L1)
            .ok()
            .flatten()
            .is_none()
        {
            copy_previews(previews, &[(sibling, copy)]);
        }
    }
    Ok(())
}

/// Same as [`generate_and_store`], but skips work if `level` is already
/// cached — the common case for Loupe, which asks every frame until the
/// preview lands.
pub fn ensure_cached(
    previews: &PreviewCache,
    path: &Path,
    photo_id: PhotoId,
    level: &str,
    budget_px: u32,
) -> Result<PathBuf> {
    if let Some(existing) = previews.lookup(photo_id, level)? {
        return Ok(existing);
    }
    generate_and_store(previews, path, photo_id, level, budget_px)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use viberoom_preview::{LEVEL_L1, LEVEL_L2};

    fn make_test_png(dir: &Path) -> PathBuf {
        let path = dir.join("a.png");
        let img =
            image::RgbImage::from_fn(40, 30, |x, y| image::Rgb([x as u8 * 5, y as u8 * 5, 200]));
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn backfill_gives_virtual_copies_their_masters_preview() {
        use viberoom_catalog::command::{Command, CreateVirtualCopies};
        use viberoom_catalog::{Catalog, repo};
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::create_or_open(dir.path().join("t.arcat")).unwrap();
        let conn = catalog.connection();
        let folder = repo::upsert_folder_path(conn, dir.path()).unwrap();
        let file = repo::insert_file(
            conn,
            &repo::NewFile {
                folder_id: folder,
                filename: "a.jpg",
                ext: "jpg",
                kind: "jpeg",
                ..Default::default()
            },
        )
        .unwrap();
        let master = repo::insert_photo(
            conn,
            &repo::NewPhoto {
                file_id: file,
                import_id: None,
            },
        )
        .unwrap();
        let mut cmd = CreateVirtualCopies::new(vec![master]);
        cmd.apply(conn).unwrap();
        let copy = cmd.created()[0];

        let previews = PreviewCache::open_at(&dir.path().join("previews")).unwrap();
        previews
            .store(master, LEVEL_L1, DEFAULT_PARAMS_HASH, b"jpeg", 8, 6)
            .unwrap();
        assert!(previews.lookup(copy, LEVEL_L1).unwrap().is_none());

        backfill_virtual_copy_previews(conn, &previews).unwrap();
        let p = previews.lookup(copy, LEVEL_L1).unwrap().unwrap();
        assert_eq!(std::fs::read(p).unwrap(), b"jpeg");
    }

    #[test]
    fn ensure_cached_generates_once_then_reuses() {
        let src_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let path = make_test_png(src_dir.path());
        let previews = PreviewCache::open_at(cache_dir.path()).unwrap();
        let photo_id = PhotoId::new(1);

        let first = ensure_cached(&previews, &path, photo_id, LEVEL_L2, 2048).unwrap();
        assert!(first.exists());
        let first_bytes = std::fs::read(&first).unwrap();

        // Delete the source; a cache hit shouldn't need it again.
        std::fs::remove_file(&path).unwrap();
        let second = ensure_cached(&previews, &path, photo_id, LEVEL_L2, 2048).unwrap();
        assert_eq!(second, first);
        assert_eq!(std::fs::read(&second).unwrap(), first_bytes);
    }
}

/// Startup housekeeping for the preview cache: sweeps stray files and
/// evicts the oldest previews past the budget (they regenerate on demand).
#[derive(Debug)]
pub struct TrimPreviewsJob {
    pub catalog_path: std::path::PathBuf,
    pub budget_bytes: u64,
}

impl viberoom_jobs::Job for TrimPreviewsJob {
    fn label(&self) -> String {
        "Tidying the preview cache".to_string()
    }

    fn priority(&self) -> viberoom_jobs::Priority {
        viberoom_jobs::Priority::Background
    }

    fn run(self: Box<Self>, _cx: &viberoom_jobs::JobContext) {
        match viberoom_preview::PreviewCache::open_for_catalog(&self.catalog_path)
            .and_then(|cache| cache.trim(self.budget_bytes))
        {
            Ok(0) => {}
            Ok(freed) => tracing::info!(freed, "preview cache trimmed"),
            Err(e) => tracing::warn!(error = %e, "preview cache trim failed"),
        }
    }
}
