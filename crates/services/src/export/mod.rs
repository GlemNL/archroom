//! Export (plan §9): settings, presets, file naming and resize math (pure,
//! unit-tested here), the encoders (`encode`) and the batch job (`job`).

pub mod encode;
pub mod job;

use std::path::{Path, PathBuf};

use archroom_color::icc::OutputSpace;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Jpeg,
    Tiff,
    Png,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Tiff => "tif",
            Format::Png => "png",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Tiff => "TIFF",
            Format::Png => "PNG",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TiffCompression {
    None,
    Lzw,
    Zip,
}

/// How big the exported image is (plan §9's Resize row). Exports never
/// enlarge: the engine renders at most the source resolution.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Resize {
    Full,
    LongEdge(u32),
    ShortEdge(u32),
    /// Fit inside a `w`×`h` box.
    Fit {
        w: u32,
        h: u32,
    },
    Megapixels(f32),
    Percent(f32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Destination {
    Folder(PathBuf),
    SameAsOriginal,
    /// A subfolder of each original's folder.
    Subfolder(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Conflict {
    /// `name-1.jpg`, `name-2.jpg`, …
    Unique,
    Overwrite,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetadataMode {
    All,
    CopyrightOnly,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub format: Format,
    /// 1–100; JPEG only.
    pub jpeg_quality: u8,
    /// 8 or 16; JPEG is always 8.
    pub bit_depth: u8,
    pub tiff_compression: TiffCompression,
    pub space: OutputSpace,
    pub resize: Resize,
    /// Written to JPEG and TIFF.
    pub ppi: u32,
    pub destination: Destination,
    /// Tokens: `{filename}`, `{seq:4}`, `{date:%Y%m%d}`, `{title}`, `{custom}`.
    pub naming: String,
    pub custom_text: String,
    pub sequence_start: u32,
    pub conflict: Conflict,
    pub metadata: MetadataMode,
    pub remove_location: bool,
    /// Also write `lr:hierarchicalSubject` (keyword paths).
    pub hierarchical_keywords: bool,
    pub show_in_file_manager: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: Format::Jpeg,
            jpeg_quality: 92,
            bit_depth: 8,
            tiff_compression: TiffCompression::Lzw,
            space: OutputSpace::Srgb,
            resize: Resize::Full,
            ppi: 300,
            destination: Destination::Subfolder("Exports".into()),
            naming: "{filename}".into(),
            custom_text: String::new(),
            sequence_start: 1,
            conflict: Conflict::Unique,
            metadata: MetadataMode::All,
            remove_location: false,
            hierarchical_keywords: true,
            show_in_file_manager: false,
        }
    }
}

impl ExportSettings {
    /// The depth actually rendered and written (JPEG has no 16-bit).
    pub fn effective_depth(&self) -> u8 {
        if self.format == Format::Jpeg || self.bit_depth != 16 {
            8
        } else {
            16
        }
    }
}

/// A named settings set: built-in (read-only) or saved in the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportPreset {
    pub name: String,
    pub settings: ExportSettings,
    pub builtin: bool,
}

pub fn builtin_presets() -> Vec<ExportPreset> {
    let base = ExportSettings::default();
    let preset = |name: &str, settings: ExportSettings| ExportPreset {
        name: name.into(),
        settings,
        builtin: true,
    };
    vec![
        preset("JPEG sRGB full size", base.clone()),
        preset(
            "JPEG 2048 px for web",
            ExportSettings {
                resize: Resize::LongEdge(2048),
                jpeg_quality: 85,
                ppi: 72,
                metadata: MetadataMode::CopyrightOnly,
                ..base.clone()
            },
        ),
        preset(
            "TIFF 16-bit ProPhoto",
            ExportSettings {
                format: Format::Tiff,
                bit_depth: 16,
                space: OutputSpace::ProPhoto,
                ..base
            },
        ),
    ]
}

/// Built-ins first, then the catalog's saved presets.
pub fn all_presets(conn: &Connection) -> Result<Vec<ExportPreset>> {
    let mut presets = builtin_presets();
    for (name, json) in archroom_catalog::export_presets::list(conn)? {
        match serde_json::from_str(&json) {
            Ok(settings) => presets.push(ExportPreset {
                name,
                settings,
                builtin: false,
            }),
            Err(e) => tracing::warn!(preset = name, error = %e, "unreadable export preset"),
        }
    }
    Ok(presets)
}

pub fn save_preset(conn: &Connection, name: &str, settings: &ExportSettings) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Other("a preset needs a name".into()));
    }
    if builtin_presets().iter().any(|p| p.name == name) {
        return Err(Error::Other(format!("\"{name}\" is a built-in preset")));
    }
    let json = serde_json::to_string(settings).map_err(|e| Error::Other(e.to_string()))?;
    archroom_catalog::export_presets::save(conn, name, &json)?;
    Ok(())
}

pub fn delete_preset(conn: &Connection, name: &str) -> Result<()> {
    archroom_catalog::export_presets::delete(conn, name)?;
    Ok(())
}

// ---- resize ----------------------------------------------------------------

/// The exported size for a `(w, h)` source (after crop and rotation), never
/// larger than the source.
pub fn output_size(resize: Resize, (w, h): (u32, u32)) -> (u32, u32) {
    let (fw, fh) = (f64::from(w.max(1)), f64::from(h.max(1)));
    let scale = match resize {
        Resize::Full => 1.0,
        Resize::LongEdge(n) => f64::from(n) / fw.max(fh),
        Resize::ShortEdge(n) => f64::from(n) / fw.min(fh),
        Resize::Fit { w, h } => (f64::from(w) / fw).min(f64::from(h) / fh),
        Resize::Megapixels(mp) => (f64::from(mp) * 1e6 / (fw * fh)).sqrt(),
        Resize::Percent(p) => f64::from(p) / 100.0,
    };
    let scale = if scale.is_finite() && scale > 0.0 {
        scale.min(1.0)
    } else {
        1.0
    };
    (
        ((fw * scale).round() as u32).max(1),
        ((fh * scale).round() as u32).max(1),
    )
}

// ---- naming ----------------------------------------------------------------

/// What a naming template can draw on for one photo.
#[derive(Debug, Clone, Copy)]
pub struct NameContext<'a> {
    /// The original's file name without extension.
    pub stem: &'a str,
    /// 1-based position in the batch, already offset by `sequence_start`.
    pub seq: u32,
    /// `YYYY-MM-DDTHH:MM:SS`, as stored in the catalog.
    pub capture_time: Option<&'a str>,
    pub title: Option<&'a str>,
    pub custom: &'a str,
}

fn strftime(fmt: &str, time: Option<&str>) -> String {
    // Fields by position in `YYYY-MM-DDTHH:MM:SS`.
    let field = |a: usize, b: usize| time.and_then(|t| t.get(a..b)).filter(|s| !s.is_empty());
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(field(0, 4).unwrap_or("0000")),
            Some('m') => out.push_str(field(5, 7).unwrap_or("00")),
            Some('d') => out.push_str(field(8, 10).unwrap_or("00")),
            Some('H') => out.push_str(field(11, 13).unwrap_or("00")),
            Some('M') => out.push_str(field(14, 16).unwrap_or("00")),
            Some('S') => out.push_str(field(17, 19).unwrap_or("00")),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Expands `template` into a file name stem. Unknown tokens stay literal;
/// path separators and control characters are replaced so a name can never
/// leave its folder.
pub fn expand_name(template: &str, cx: &NameContext<'_>) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            rest = &rest[open..];
            break;
        };
        let token = &rest[open + 1..open + close];
        let (name, arg) = token
            .split_once(':')
            .map_or((token, None), |(n, a)| (n, Some(a)));
        match name {
            "filename" => out.push_str(cx.stem),
            "seq" => {
                let width = arg.and_then(|a| a.parse::<usize>().ok()).unwrap_or(1);
                out.push_str(&format!("{:0width$}", cx.seq, width = width.min(12)));
            }
            "date" => out.push_str(&strftime(arg.unwrap_or("%Y%m%d"), cx.capture_time)),
            "title" => out.push_str(cx.title.unwrap_or("")),
            "custom" => out.push_str(cx.custom),
            _ => out.push_str(&rest[open..=open + close]),
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    let clean: String = out
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() {
        "export".into()
    } else {
        clean
    }
}

/// Picks the file to write for `stem.ext` in `dir` under `policy`; `None`
/// means skip.
pub fn resolve_conflict(dir: &Path, stem: &str, ext: &str, policy: Conflict) -> Option<PathBuf> {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return Some(first);
    }
    match policy {
        Conflict::Overwrite => Some(first),
        Conflict::Skip => None,
        Conflict::Unique => (1..)
            .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
            .find(|p| !p.exists()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn cx() -> NameContext<'static> {
        NameContext {
            stem: "_7808140",
            seq: 7,
            capture_time: Some("2026-09-27T14:05:09"),
            title: Some("Dawn"),
            custom: "ACME",
        }
    }

    #[test]
    fn tokens_expand() {
        assert_eq!(
            expand_name("{date:%Y%m%d}_{seq:4}_{filename}", &cx()),
            "20260927_0007__7808140"
        );
        assert_eq!(expand_name("{custom}-{title}-{seq}", &cx()), "ACME-Dawn-7");
        assert_eq!(
            expand_name("{date:%Y-%m-%d %H.%M.%S}", &cx()),
            "2026-09-27 14.05.09"
        );
        assert_eq!(expand_name("{date}", &cx()), "20260927");
    }

    #[test]
    fn unknown_tokens_and_hostile_names_are_contained() {
        assert_eq!(expand_name("a{nope}b", &cx()), "a{nope}b");
        assert_eq!(expand_name("a{open", &cx()), "a{open");
        assert_eq!(expand_name("../x/{filename}", &cx()), "_x__7808140");
        assert!(!expand_name("../../etc/{title}", &cx()).contains('/'));
        assert_eq!(expand_name("", &cx()), "export");
        assert_eq!(
            expand_name(
                "{title}",
                &NameContext {
                    title: None,
                    ..cx()
                }
            ),
            "export"
        );
    }

    #[test]
    fn undated_photos_do_not_panic() {
        let c = NameContext {
            capture_time: None,
            ..cx()
        };
        assert_eq!(expand_name("{date:%Y%m%d}", &c), "00000000");
    }

    #[test]
    fn resize_modes_never_enlarge() {
        let src = (6000, 4000);
        assert_eq!(output_size(Resize::Full, src), src);
        assert_eq!(output_size(Resize::LongEdge(2048), src), (2048, 1365));
        assert_eq!(output_size(Resize::ShortEdge(1000), src), (1500, 1000));
        assert_eq!(
            output_size(Resize::Fit { w: 1000, h: 1000 }, src),
            (1000, 667)
        );
        assert_eq!(output_size(Resize::Percent(50.0), src), (3000, 2000));
        assert_eq!(output_size(Resize::LongEdge(10_000), src), src);
        let (w, h) = output_size(Resize::Megapixels(6.0), src);
        assert!((f64::from(w) * f64::from(h) - 6e6).abs() < 2e4, "{w}x{h}");
        // Portrait long edge.
        assert_eq!(output_size(Resize::LongEdge(600), (400, 800)), (300, 600));
        assert_eq!(output_size(Resize::Percent(0.0), src), src);
    }

    #[test]
    fn conflict_policies() {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| dir.path().join(n);
        assert_eq!(
            resolve_conflict(dir.path(), "a", "jpg", Conflict::Skip),
            Some(p("a.jpg"))
        );
        std::fs::write(p("a.jpg"), b"x").unwrap();
        assert_eq!(
            resolve_conflict(dir.path(), "a", "jpg", Conflict::Skip),
            None
        );
        assert_eq!(
            resolve_conflict(dir.path(), "a", "jpg", Conflict::Overwrite),
            Some(p("a.jpg"))
        );
        assert_eq!(
            resolve_conflict(dir.path(), "a", "jpg", Conflict::Unique),
            Some(p("a-1.jpg"))
        );
        std::fs::write(p("a-1.jpg"), b"x").unwrap();
        assert_eq!(
            resolve_conflict(dir.path(), "a", "jpg", Conflict::Unique),
            Some(p("a-2.jpg"))
        );
    }

    #[test]
    fn settings_survive_json_and_tolerate_missing_fields() {
        let s = ExportSettings {
            format: Format::Tiff,
            bit_depth: 16,
            resize: Resize::Fit { w: 10, h: 20 },
            ..Default::default()
        };
        let back: ExportSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let partial: ExportSettings = serde_json::from_str(r#"{"jpeg_quality": 50}"#).unwrap();
        assert_eq!(partial.jpeg_quality, 50);
        assert_eq!(partial.format, Format::Jpeg);
        assert_eq!(s.effective_depth(), 16);
        assert_eq!(
            ExportSettings {
                bit_depth: 16,
                ..Default::default()
            }
            .effective_depth(),
            8
        );
    }

    #[test]
    fn presets_round_trip_and_builtins_are_protected() {
        let dir = tempfile::tempdir().unwrap();
        let cat = archroom_catalog::Catalog::create_or_open(dir.path().join("t.arcat")).unwrap();
        let conn = cat.connection();
        assert_eq!(all_presets(conn).unwrap().len(), 3);
        let s = ExportSettings {
            jpeg_quality: 70,
            ..Default::default()
        };
        save_preset(conn, "Mine", &s).unwrap();
        let all = all_presets(conn).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(all[3].settings, s);
        assert!(save_preset(conn, "JPEG sRGB full size", &s).is_err());
        assert!(save_preset(conn, "  ", &s).is_err());
        delete_preset(conn, "Mine").unwrap();
        assert_eq!(all_presets(conn).unwrap().len(), 3);
    }
}
