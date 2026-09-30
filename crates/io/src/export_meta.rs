//! Writes the catalog's metadata into an exported file (plan §9's Metadata
//! row) through exiv2. The exported pixels are already display-oriented, so
//! no orientation tag is written.

use std::path::Path;

use viberoom_core::{Error, Result};

/// What an export embeds; the caller filters it down (copyright only, no
/// location, keywords marked include-on-export) before it gets here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportMetadata {
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    /// `YYYY-MM-DDTHH:MM:SS`, as stored in the catalog.
    pub capture_time: Option<String>,
    pub focal_length: Option<f64>,
    pub aperture: Option<f64>,
    /// Exposure time in seconds.
    pub shutter: Option<f64>,
    pub iso: Option<i64>,
    pub gps: Option<(f64, f64)>,
    pub title: Option<String>,
    pub caption: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    /// Flat keywords (leaf names).
    pub keywords: Vec<String>,
    /// Full keyword paths (`a|b|c`), for `lr:hierarchicalSubject`.
    pub hierarchical_keywords: Vec<String>,
}

fn rational(v: f64) -> String {
    format!("{}/1000", (v * 1000.0).round() as i64)
}

/// `2024-01-15T10:30:00` → `2024:01:15 10:30:00` (EXIF's form).
fn exif_datetime(iso: &str) -> String {
    let mut s = iso.replace('T', " ");
    let mut n = 0;
    s = s
        .chars()
        .map(|c| {
            if c == '-' && n < 2 {
                n += 1;
                ':'
            } else {
                c
            }
        })
        .collect();
    s
}

/// Embeds `meta` into the already-encoded file at `path`.
pub fn write_export_metadata(path: &Path, meta: &ExportMetadata) -> Result<()> {
    crate::metadata::ensure_rexiv2_initialized();
    let err = |e: rexiv2::Rexiv2Error| Error::Other(format!("exiv2 {}: {e}", path.display()));
    let m = rexiv2::Metadata::new_from_path(path).map_err(err)?;
    let set = |tag: &str, v: &Option<String>| -> Result<()> {
        match v {
            Some(v) if !v.is_empty() => m.set_tag_string(tag, v).map_err(err),
            _ => Ok(()),
        }
    };
    set("Exif.Image.Make", &meta.camera_make)?;
    set("Exif.Image.Model", &meta.camera_model)?;
    set("Exif.Photo.LensModel", &meta.lens)?;
    if let Some(t) = &meta.capture_time {
        m.set_tag_string("Exif.Photo.DateTimeOriginal", &exif_datetime(t))
            .map_err(err)?;
    }
    if let Some(v) = meta.focal_length {
        m.set_tag_string("Exif.Photo.FocalLength", &rational(v))
            .map_err(err)?;
    }
    if let Some(v) = meta.aperture {
        m.set_tag_string("Exif.Photo.FNumber", &rational(v))
            .map_err(err)?;
    }
    if let Some(v) = meta.shutter {
        // Shutters are usually 1/N: keep them exact.
        let s = if v > 0.0 && v < 1.0 {
            format!("1/{}", (1.0 / v).round() as i64)
        } else {
            rational(v)
        };
        m.set_tag_string("Exif.Photo.ExposureTime", &s)
            .map_err(err)?;
    }
    if let Some(v) = meta.iso {
        m.set_tag_numeric("Exif.Photo.ISOSpeedRatings", v as i32)
            .map_err(err)?;
    }
    if let Some((lat, lon)) = meta.gps {
        let _ = m.set_gps_info(&rexiv2::GpsInfo {
            latitude: lat,
            longitude: lon,
            altitude: 0.0,
        });
    }
    set("Xmp.dc.title", &meta.title)?;
    set("Iptc.Application2.ObjectName", &meta.title)?;
    set("Xmp.dc.description", &meta.caption)?;
    set("Iptc.Application2.Caption", &meta.caption)?;
    if let Some(c) = meta.creator.as_deref().filter(|c| !c.is_empty()) {
        m.set_tag_string("Exif.Image.Artist", c).map_err(err)?;
        m.set_tag_string("Iptc.Application2.Byline", c)
            .map_err(err)?;
        m.set_tag_multiple_strings("Xmp.dc.creator", &[c])
            .map_err(err)?;
    }
    if let Some(c) = meta.copyright.as_deref().filter(|c| !c.is_empty()) {
        m.set_tag_string("Exif.Image.Copyright", c).map_err(err)?;
        m.set_tag_string("Iptc.Application2.Copyright", c)
            .map_err(err)?;
        m.set_tag_string("Xmp.dc.rights", c).map_err(err)?;
    }
    if !meta.keywords.is_empty() {
        let k: Vec<&str> = meta.keywords.iter().map(String::as_str).collect();
        m.set_tag_multiple_strings("Iptc.Application2.Keywords", &k)
            .map_err(err)?;
        m.set_tag_multiple_strings("Xmp.dc.subject", &k)
            .map_err(err)?;
    }
    if !meta.hierarchical_keywords.is_empty() {
        let k: Vec<&str> = meta
            .hierarchical_keywords
            .iter()
            .map(String::as_str)
            .collect();
        m.set_tag_multiple_strings("Xmp.lr.hierarchicalSubject", &k)
            .map_err(err)?;
    }
    m.save_to_file(path).map_err(err)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn datetime_uses_exif_separators() {
        assert_eq!(exif_datetime("2024-01-15T10:30:00"), "2024:01:15 10:30:00");
    }
}
