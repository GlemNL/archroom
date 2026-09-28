//! Generates a preview at a given level/size and stores it in the
//! [`PreviewCache`] (plan §5.4). Shared by import (always generates L1) and
//! the Loupe view (generates L2 lazily, on first open of a photo).

use std::path::{Path, PathBuf};

use archroom_core::ids::PhotoId;
use archroom_io::{DecodeOptions, DecodedImage, decoder_for};
use archroom_preview::{
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

    let (jpeg, w, h) = match decoder.embedded_preview(path)? {
        Some(bytes) => generate_preview_from_jpeg_bytes(&bytes, budget_px)?,
        None => match decoder.decode(path, &DecodeOptions::default())? {
            DecodedImage::SceneLinear { rgb, .. } => {
                generate_preview_from_linear_rgb(&rgb, budget_px)?
            }
            DecodedImage::Rendered { rgb, .. } => {
                generate_preview_from_display_rgb(&rgb, budget_px)?
            }
        },
    };
    Ok(previews.store(photo_id, level, DEFAULT_PARAMS_HASH, &jpeg, w, h)?)
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
    use archroom_preview::LEVEL_L2;

    fn make_test_png(dir: &Path) -> PathBuf {
        let path = dir.join("a.png");
        let img =
            image::RgbImage::from_fn(40, 30, |x, y| image::Rgb([x as u8 * 5, y as u8 * 5, 200]));
        img.save(&path).unwrap();
        path
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
