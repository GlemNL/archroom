//! Application preferences, stored as TOML under the XDG config directory
//! (`~/.config/archroom/config.toml`, see plan §4.7).
//!
//! `Settings` only holds user-visible preferences. Anything derived at
//! runtime (window geometry, panel open state) is `eframe`/`egui`'s own
//! persistence, not this file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CenterBackground {
    Black,
    #[default]
    DarkGray,
    MediumGray,
    White,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XmpAutoWrite {
    /// Only `Ctrl+S` writes sidecars (the Lightroom default, plan D5).
    #[default]
    Off,
    /// Write sidecars automatically after every metadata change.
    On,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The catalog to reopen on next launch, if any (D7: one catalog at a time).
    pub last_catalog: Option<PathBuf>,
    pub center_background: CenterBackground,
    pub xmp_auto_write: XmpAutoWrite,
    /// Standard-preview long edge in pixels (L2, plan §5.4).
    pub preview_long_edge: u32,
    pub preview_jpeg_quality: u8,
    /// LRU cache budget for 1:1 previews, in megabytes (plan §5.4).
    pub cache_budget_mb: u32,
    /// `wgpu` adapter name to prefer, or `None` for the default adapter.
    pub gpu_adapter: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_catalog: None,
            center_background: CenterBackground::default(),
            xmp_auto_write: XmpAutoWrite::default(),
            preview_long_edge: 2048,
            preview_jpeg_quality: 90,
            cache_budget_mb: 5 * 1024,
            gpu_adapter: None,
        }
    }
}

impl Settings {
    /// `~/.config/archroom/config.toml` (XDG on Linux).
    pub fn config_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "archroom")
            .ok_or_else(|| Error::Settings("no home directory".into()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        toml::from_str(&text).map_err(|e| Error::Settings(e.to_string()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| Error::Settings(e.to_string()))?;
        std::fs::write(path, text).map_err(|e| Error::io(path, e))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let settings = Settings::load_from(&path).expect("load");
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/config.toml");
        let settings = Settings {
            last_catalog: Some(PathBuf::from("/tmp/example.arcat")),
            preview_long_edge: 4096,
            ..Settings::default()
        };
        settings.save_to(&path).expect("save");

        let loaded = Settings::load_from(&path).expect("load");
        assert_eq!(loaded, settings);
    }
}
