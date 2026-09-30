//! Preview generation (plan §5.4). No render pipeline exists until M3, so a
//! preview at any level comes from whichever is cheapest: the raw's
//! embedded JPEG when there is one, else a plain linear-to-sRGB gamma pass
//! over a full decode (no white balance or tone — that's M3's job; this is
//! only meant to be recognizable, not correct). L1 (grid/filmstrip) and L2
//! (Loupe) differ only in target size, so the same functions serve both —
//! callers pick [`L1_BUDGET_PX`] or [`L2_BUDGET_PX`].

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use fast_image_resize::images::Image as FirImage;
use fast_image_resize::{IntoImageView, Resizer};
use image::{DynamicImage, ExtendedColorType, ImageEncoder};
use viberoom_io::ImageF32;

use crate::error::{Error, Result};

pub const L1_BUDGET_PX: u32 = 320;
/// The plan's default "standard" preview size (§5.4); used for Loupe until
/// M3's real render pipeline replaces this decode-and-resize placeholder.
pub const L2_BUDGET_PX: u32 = 2048;
const DEFAULT_JPEG_QUALITY: u8 = 85;

static L2_BUDGET: AtomicU32 = AtomicU32::new(L2_BUDGET_PX);
static JPEG_QUALITY: AtomicU8 = AtomicU8::new(DEFAULT_JPEG_QUALITY);

/// The user's standard-preview long edge (Preferences); previews generated
/// from now on use it, existing ones keep their size until regenerated.
pub fn l2_budget_px() -> u32 {
    L2_BUDGET.load(Ordering::Relaxed)
}

pub fn jpeg_quality() -> u8 {
    JPEG_QUALITY.load(Ordering::Relaxed)
}

pub fn set_preview_options(long_edge: u32, jpeg_quality: u8) {
    L2_BUDGET.store(long_edge.clamp(512, 8192), Ordering::Relaxed);
    JPEG_QUALITY.store(jpeg_quality.clamp(40, 100), Ordering::Relaxed);
}

/// Re-encodes an already-JPEG embedded thumbnail at `budget_px`.
pub fn generate_preview_from_jpeg_bytes(
    jpeg_bytes: &[u8],
    budget_px: u32,
    orientation: i32,
) -> Result<(Vec<u8>, u32, u32)> {
    let src = image::load_from_memory(jpeg_bytes)
        .map_err(|e| Error::Other(format!("decode embedded thumbnail: {e}")))?;
    resize_and_encode_jpeg(&src, budget_px, orientation)
}

/// Builds a preview straight from decoded scene-linear pixels (the fallback
/// when a raw has no usable embedded thumbnail): a plain linear-to-sRGB
/// gamma, no white balance or tone.
pub fn generate_preview_from_linear_rgb(
    rgb: &ImageF32,
    budget_px: u32,
    orientation: i32,
) -> Result<(Vec<u8>, u32, u32)> {
    resize_and_encode_jpeg(
        &rgb_f32_to_image(rgb, |v| v.clamp(0.0, 1.0).powf(1.0 / 2.2)),
        budget_px,
        orientation,
    )
}

/// Builds a preview from pixels that are already display-referred (a
/// decoded JPEG/PNG/TIFF) — no gamma pass, since one is already baked in.
pub fn generate_preview_from_display_rgb(
    rgb: &ImageF32,
    budget_px: u32,
    orientation: i32,
) -> Result<(Vec<u8>, u32, u32)> {
    resize_and_encode_jpeg(
        &rgb_f32_to_image(rgb, |v| v.clamp(0.0, 1.0)),
        budget_px,
        orientation,
    )
}

/// JPEG-encodes tightly packed display-referred RGBA8 (the develop
/// pipeline's output, already at the preview's size).
pub fn encode_jpeg_from_rgba8(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for px in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, jpeg_quality())
        .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
        .map_err(|e| Error::Other(format!("jpeg encode: {e}")))?;
    Ok(out)
}

fn rgb_f32_to_image(rgb: &ImageF32, tone: impl Fn(f32) -> f32) -> DynamicImage {
    let mut buf = image::RgbImage::new(rgb.width, rgb.height);
    let channels = rgb.channels as usize;
    for (i, px) in buf.pixels_mut().enumerate() {
        let base = i * channels;
        let to_u8 = |v: f32| (tone(v) * 255.0).round() as u8;
        *px = image::Rgb([
            to_u8(rgb.data[base]),
            to_u8(rgb.data[base + 1]),
            to_u8(rgb.data[base + 2]),
        ]);
    }
    DynamicImage::ImageRgb8(buf)
}

/// Turns `src` upright per the EXIF `orientation` tag (1 or unknown: as is).
fn orient(src: &DynamicImage, orientation: i32) -> Option<DynamicImage> {
    let o = u8::try_from(orientation)
        .ok()
        .and_then(image::metadata::Orientation::from_exif)
        .filter(|o| *o != image::metadata::Orientation::NoTransforms)?;
    let mut img = src.clone();
    img.apply_orientation(o);
    Some(img)
}

fn resize_and_encode_jpeg(
    src: &DynamicImage,
    budget_px: u32,
    orientation: i32,
) -> Result<(Vec<u8>, u32, u32)> {
    let oriented = orient(src, orientation);
    let src = oriented.as_ref().unwrap_or(src);
    let (width, height) = (src.width(), src.height());
    let scale = (budget_px as f32 / width.max(height) as f32).min(1.0);
    let dst_width = ((width as f32 * scale).round() as u32).max(1);
    let dst_height = ((height as f32 * scale).round() as u32).max(1);

    let pixel_type = src
        .pixel_type()
        .ok_or_else(|| Error::Other("unsupported pixel type for resize".into()))?;
    let mut dst = FirImage::new(dst_width, dst_height, pixel_type);
    Resizer::new()
        .resize(src, &mut dst, None)
        .map_err(|e| Error::Other(format!("resize: {e}")))?;

    let color: ExtendedColorType = src.color().into();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, jpeg_quality())
        .write_image(dst.buffer(), dst_width, dst_height, color)
        .map_err(|e| Error::Other(format!("jpeg encode: {e}")))?;

    Ok((out, dst_width, dst_height))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn generates_a_downscaled_jpeg_from_linear_rgb() {
        let width = 800;
        let height = 400;
        let mut data = vec![0f32; (width * height * 4) as usize];
        for px in data.chunks_exact_mut(4) {
            px[0] = 0.5;
            px[1] = 0.25;
            px[2] = 0.75;
            px[3] = 1.0;
        }
        let rgb = ImageF32 {
            width,
            height,
            channels: 4,
            data,
        };

        let (jpeg, w, h) = generate_preview_from_linear_rgb(&rgb, 100, 1).unwrap();
        assert_eq!((w, h), (100, 50));
        assert!(!jpeg.is_empty());
        assert_eq!(
            &jpeg[0..2],
            &[0xFF, 0xD8],
            "must be a valid JPEG (SOI marker)"
        );

        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (100, 50));
    }

    #[test]
    fn never_upscales_a_small_source() {
        let rgb = ImageF32 {
            width: 50,
            height: 30,
            channels: 4,
            data: vec![0.5f32; 50 * 30 * 4],
        };
        let (_jpeg, w, h) = generate_preview_from_linear_rgb(&rgb, 320, 1).unwrap();
        assert_eq!((w, h), (50, 30));
    }

    #[test]
    fn exif_orientation_turns_the_preview_upright() {
        let rgb = ImageF32 {
            width: 4,
            height: 2,
            channels: 3,
            data: vec![0.5; 4 * 2 * 3],
        };
        let (_, w, h) = generate_preview_from_linear_rgb(&rgb, 100, 6).unwrap();
        assert_eq!((w, h), (2, 4));
    }
}
