//! exiv2-backed metadata reading (`rexiv2`), for JPEG/PNG/TIFF (plan §5.1,
//! the M1 "Metadata through exiv2" bullet). Raws use LibRaw's own metadata
//! instead (`crate::libraw`), since that sidesteps exiv2's uneven raw-format
//! coverage (CR3/BMFF support on this platform is still unverified, per the
//! M0 exit note) and LibRaw already parses every format this app decodes.

use std::path::Path;
use std::sync::Once;

use archroom_core::{Error, Result};

use crate::types::ImageMetadata;

static REXIV2_INIT: Once = Once::new();

/// `rexiv2::initialize()` — GLib type registration and XMP namespace setup
/// — only needs to run once. `Once` makes the first caller's thread pay for
/// it and serializes against a second caller racing in before it's done;
/// every caller after that is a no-op check.
fn ensure_rexiv2_initialized() {
    REXIV2_INIT.call_once(|| {
        if let Err(e) = rexiv2::initialize() {
            tracing::warn!(error = %e, "rexiv2::initialize failed");
        }
    });
}

pub fn read_exif_summary(path: &Path) -> Result<ImageMetadata> {
    ensure_rexiv2_initialized();
    let meta = rexiv2::Metadata::new_from_path(path)
        .map_err(|e| Error::Other(format!("exiv2 open {}: {e}", path.display())))?;

    // `Orientation`'s discriminants (Unspecified=0, Normal=1, ..,
    // Rotate270=8) line up exactly with the EXIF orientation tag's 1..=8,
    // with 0 standing in for "tag absent" — the same convention `files.orientation`
    // uses.
    let orientation = meta.get_orientation() as i32;

    let width = meta.get_pixel_width().max(0) as u32;
    let height = meta.get_pixel_height().max(0) as u32;

    let capture_time = meta
        .get_tag_string("Exif.Photo.DateTimeOriginal")
        .or_else(|_| meta.get_tag_string("Exif.Image.DateTime"))
        .ok()
        .map(|s| normalize_exif_datetime(&s));

    let camera_make = non_empty(meta.get_tag_string("Exif.Image.Make").ok());
    let camera_model = non_empty(meta.get_tag_string("Exif.Image.Model").ok());
    let lens = non_empty(meta.get_tag_string("Exif.Photo.LensModel").ok());

    let focal_length = meta.get_focal_length().map(|v| v as f32);
    let aperture = meta.get_fnumber().map(|v| v as f32);
    let shutter = meta
        .get_exposure_time()
        .map(|r| *r.numer() as f32 / *r.denom() as f32);
    let iso = meta.get_iso_speed().map(|v| v.max(0) as u32);

    let gps = meta.get_gps_info().map(|g| (g.latitude, g.longitude));

    Ok(ImageMetadata {
        width,
        height,
        orientation,
        capture_time,
        camera_make,
        camera_model,
        lens,
        focal_length,
        aperture,
        shutter,
        iso,
        gps,
    })
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// `"2024:01:15 10:30:00"` (EXIF's date format) to `"2024-01-15T10:30:00"`,
/// so raw- and exiv2-derived `capture_time` values sort and compare the same
/// way.
fn normalize_exif_datetime(s: &str) -> String {
    let mut out = s.replacen(':', "-", 2);
    if let Some(idx) = out.find(' ') {
        out.replace_range(idx..idx + 1, "T");
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_exif_datetime_to_iso8601() {
        assert_eq!(
            normalize_exif_datetime("2024:01:15 10:30:00"),
            "2024-01-15T10:30:00"
        );
    }

    #[test]
    fn reading_a_missing_file_errors_instead_of_panicking() {
        let result = read_exif_summary(Path::new("/nonexistent/does-not-exist.jpg"));
        assert!(result.is_err());
    }
}
