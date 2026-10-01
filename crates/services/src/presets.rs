//! Develop presets (plan §8.1): a named set of settings groups plus the
//! values they take. Built-ins live in code; user presets in the catalog;
//! either can be written to / read from a `.arpreset` file.

use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use viberoom_catalog::presets as store;
use viberoom_engine::ops::{
    BwMix, BwMixParams, Clarity, ClarityParams, Exposure, ExposureParams, Hsl, HslParams, Noise,
    NoiseParams, Presence, PresenceParams, Profile, ProfileName, ProfileParams, Sharpen,
    SharpenParams, Tone, ToneCurve, ToneCurveParams, ToneParams, Treatment, Vignette,
    VignetteParams, WbMode, WhiteBalance, WhiteBalanceParams, default_registry,
};
use viberoom_engine::{EditParams, SettingsGroup};

use crate::develop::paste_groups;
use crate::error::{Error, Result};

pub const USER_FOLDER: &str = "User Presets";
pub const BUILTIN_FOLDER: &str = "Viberoom";

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    /// Catalog row id; `None` for built-ins and freshly imported files.
    pub id: Option<i64>,
    pub name: String,
    pub folder: String,
    pub groups: Vec<SettingsGroup>,
    pub params: EditParams,
    pub builtin: bool,
}

/// The settings groups `params` has edits in.
pub fn groups_in(params: &EditParams) -> Vec<SettingsGroup> {
    let registry = default_registry();
    let mut groups: Vec<SettingsGroup> = params
        .ops
        .keys()
        .filter_map(|id| registry.get(id).map(|o| o.group()))
        .collect();
    if !params.local.is_empty() {
        groups.push(SettingsGroup::LocalAdjustments);
    }
    groups.sort();
    groups.dedup();
    groups
}

/// `params` reduced to the ops of `groups`, so a preset stores only what it
/// applies.
pub fn restrict(params: &EditParams, groups: &[SettingsGroup]) -> EditParams {
    let mut out = EditParams::default();
    paste_groups(&mut out, params, &portable(groups));
    out
}

/// `groups` without the ones tied to a specific photo (red eye circles and
/// local masks): presets never carry them.
fn portable(groups: &[SettingsGroup]) -> Vec<SettingsGroup> {
    groups
        .iter()
        .copied()
        .filter(|g| !g.is_photo_specific())
        .collect()
}

impl Preset {
    /// Applies the preset to `target`: each of its groups is replaced by the
    /// preset's values (or reset when the preset leaves it at defaults).
    pub fn apply(&self, target: &mut EditParams) {
        paste_groups(target, &self.params, &portable(&self.groups));
    }
}

// --- Built-ins -------------------------------------------------------------

fn builtin(name: &str, build: impl FnOnce(&mut EditParams)) -> Preset {
    let mut params = EditParams::default();
    build(&mut params);
    Preset {
        id: None,
        name: name.to_string(),
        folder: BUILTIN_FOLDER.to_string(),
        groups: groups_in(&params),
        params,
        builtin: true,
    }
}

fn bw(p: &mut EditParams) {
    p.set::<Profile>(ProfileParams {
        name: ProfileName::Standard,
        treatment: Treatment::BlackWhite,
    });
}

pub fn builtins() -> Vec<Preset> {
    vec![
        builtin("Punchy", |p| {
            p.set::<Tone>(ToneParams {
                contrast: 20.0,
                whites: 10.0,
                blacks: -10.0,
                ..Default::default()
            });
            p.set::<Presence>(PresenceParams {
                vibrance: 25.0,
                saturation: 10.0,
            });
            p.set::<Clarity>(ClarityParams { clarity: 20.0 });
        }),
        builtin("Vivid Landscape", |p| {
            p.set::<Tone>(ToneParams {
                contrast: 15.0,
                highlights: -30.0,
                shadows: 25.0,
                ..Default::default()
            });
            p.set::<Presence>(PresenceParams {
                vibrance: 35.0,
                saturation: 10.0,
            });
            p.set::<Clarity>(ClarityParams { clarity: 25.0 });
            p.set::<Sharpen>(SharpenParams {
                amount: 40.0,
                ..Default::default()
            });
        }),
        builtin("Soft Portrait", |p| {
            p.set::<Tone>(ToneParams {
                contrast: -10.0,
                highlights: -20.0,
                shadows: 20.0,
                ..Default::default()
            });
            p.set::<Presence>(PresenceParams {
                vibrance: 10.0,
                saturation: 0.0,
            });
            p.set::<Clarity>(ClarityParams { clarity: -20.0 });
        }),
        builtin("Faded Film", |p| {
            p.set::<ToneCurve>(ToneCurveParams {
                rgb: vec![[0.0, 0.06], [0.5, 0.5], [1.0, 0.94]],
                ..Default::default()
            });
            p.set::<Presence>(PresenceParams {
                vibrance: 0.0,
                saturation: -15.0,
            });
        }),
        builtin("Cinematic", |p| {
            p.set::<ToneCurve>(ToneCurveParams {
                rgb: vec![[0.0, 0.03], [0.25, 0.2], [0.75, 0.82], [1.0, 0.97]],
                ..Default::default()
            });
            p.set::<Hsl>(HslParams {
                hue: [0.0, -8.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0],
                sat: [0.0, 15.0, -10.0, -25.0, 20.0, 10.0, 0.0, 0.0],
                lum: [0.0; 8],
            });
            p.set::<Presence>(PresenceParams {
                vibrance: 10.0,
                saturation: -5.0,
            });
        }),
        builtin("Warm Glow", |p| {
            p.set::<WhiteBalance>(WhiteBalanceParams {
                mode: WbMode::Custom,
                temp: 6800.0,
                tint: 8.0,
            });
            p.set::<Exposure>(ExposureParams { ev: 0.15 });
            p.set::<Presence>(PresenceParams {
                vibrance: 15.0,
                saturation: 0.0,
            });
            p.set::<Vignette>(VignetteParams {
                amount: -25.0,
                ..Default::default()
            });
        }),
        builtin("B&W High Contrast", |p| {
            bw(p);
            p.set::<Tone>(ToneParams {
                contrast: 40.0,
                whites: 15.0,
                blacks: -15.0,
                ..Default::default()
            });
            p.set::<BwMix>(BwMixParams {
                mix: [30.0, 20.0, 0.0, -20.0, 0.0, -30.0, 0.0, 0.0],
            });
            p.set::<Clarity>(ClarityParams { clarity: 25.0 });
        }),
        builtin("B&W Soft", |p| {
            bw(p);
            p.set::<Tone>(ToneParams {
                contrast: -10.0,
                shadows: 15.0,
                ..Default::default()
            });
            p.set::<ToneCurve>(ToneCurveParams {
                rgb: vec![[0.0, 0.05], [1.0, 1.0]],
                ..Default::default()
            });
            p.set::<Clarity>(ClarityParams { clarity: -10.0 });
        }),
        builtin("Sharpen & Denoise", |p| {
            p.set::<Sharpen>(SharpenParams {
                amount: 45.0,
                radius: 0.9,
                detail: 25.0,
                masking: 20.0,
            });
            p.set::<Noise>(NoiseParams {
                luma: 20.0,
                color: 30.0,
                ..Default::default()
            });
        }),
    ]
}

// --- User presets ------------------------------------------------------------

fn from_row(row: store::PresetRow) -> Option<Preset> {
    Some(Preset {
        id: Some(row.id),
        name: row.name,
        folder: row.folder,
        groups: serde_json::from_str(&row.groups).ok()?,
        params: EditParams::from_json(&row.params).ok()?,
        builtin: false,
    })
}

/// Built-ins followed by the catalog's user presets (unreadable rows are
/// skipped, never fatal).
pub fn list_all(conn: &Connection) -> Result<Vec<Preset>> {
    let mut all = builtins();
    all.extend(store::list(conn)?.into_iter().filter_map(from_row));
    Ok(all)
}

pub fn save_user(
    conn: &Connection,
    name: &str,
    folder: &str,
    groups: &[SettingsGroup],
    params: &EditParams,
) -> Result<i64> {
    let restricted = restrict(params, groups);
    Ok(store::insert(
        conn,
        name,
        folder,
        &serde_json::to_string(groups).map_err(|e| Error::Other(e.to_string()))?,
        &restricted.to_json(),
    )?)
}

pub fn update_user(
    conn: &Connection,
    id: i64,
    groups: &[SettingsGroup],
    params: &EditParams,
) -> Result<()> {
    let restricted = restrict(params, groups);
    Ok(store::update(
        conn,
        id,
        &serde_json::to_string(groups).map_err(|e| Error::Other(e.to_string()))?,
        &restricted.to_json(),
    )?)
}

pub fn rename_user(conn: &Connection, id: i64, name: &str) -> Result<()> {
    Ok(store::rename(conn, id, name)?)
}

pub fn delete_user(conn: &Connection, id: i64) -> Result<()> {
    Ok(store::delete(conn, id)?)
}

// --- Files ----------------------------------------------------------------------

/// The `.arpreset` file format.
#[derive(Debug, Serialize, Deserialize)]
struct PresetFile {
    viberoom_preset: u32,
    name: String,
    groups: Vec<SettingsGroup>,
    params: serde_json::Value,
}

pub fn export_file(preset: &Preset, path: &Path) -> Result<()> {
    let file = PresetFile {
        viberoom_preset: 1,
        name: preset.name.clone(),
        groups: preset.groups.clone(),
        params: serde_json::to_value(&preset.params).map_err(|e| Error::Other(e.to_string()))?,
    };
    let text = serde_json::to_string_pretty(&file).map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(path, text).map_err(|e| Error::io(path, e))
}

pub fn import_file(path: &Path) -> Result<Preset> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let file: PresetFile = serde_json::from_str(&text)
        .map_err(|e| Error::Other(format!("{}: not a preset file ({e})", path.display())))?;
    if file.viberoom_preset != 1 {
        return Err(Error::Other(format!(
            "{}: unsupported preset version {}",
            path.display(),
            file.viberoom_preset
        )));
    }
    let params = EditParams::from_json(&file.params.to_string())
        .map_err(|e| Error::Other(format!("{}: bad settings ({e})", path.display())))?;
    Ok(Preset {
        id: None,
        name: file.name,
        folder: USER_FOLDER.to_string(),
        groups: file.groups,
        params,
        builtin: false,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use viberoom_catalog::Catalog;

    #[test]
    fn built_ins_are_non_empty_distinct_and_never_touch_per_photo_groups() {
        let all = builtins();
        assert!(all.len() >= 8);
        let mut names: Vec<_> = all.iter().map(|p| p.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len(), "unique names");
        for p in &all {
            assert!(!p.params.is_identity(), "{} does something", p.name);
            assert!(!p.groups.is_empty());
            assert!(
                !p.groups.iter().any(|g| matches!(
                    g,
                    SettingsGroup::Crop | SettingsGroup::Straighten | SettingsGroup::ProcessVersion
                )),
                "{}",
                p.name
            );
        }
    }

    #[test]
    fn applying_a_preset_replaces_only_its_groups() {
        let punchy = builtins().into_iter().find(|p| p.name == "Punchy").unwrap();
        let mut target = EditParams::default();
        target.set::<Exposure>(ExposureParams { ev: 1.0 });
        target.set::<Hsl>(HslParams {
            hue: [10.0; 8],
            ..Default::default()
        });
        target.set::<Tone>(ToneParams {
            shadows: 50.0,
            ..Default::default()
        });
        punchy.apply(&mut target);
        // Punchy owns BasicTone: exposure (same group) and the old shadows go.
        assert_eq!(target.get::<Tone>().contrast, 20.0);
        assert_eq!(target.get::<Tone>().shadows, 0.0);
        assert_eq!(target.get::<Exposure>().ev, 0.0);
        // Groups it doesn't mention stay.
        assert_eq!(target.get::<Hsl>().hue[0], 10.0);
    }

    #[test]
    fn user_presets_store_only_the_chosen_groups_and_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.arcat");
        let mut params = EditParams::default();
        params.set::<Exposure>(ExposureParams { ev: 0.5 });
        params.set::<Presence>(PresenceParams {
            vibrance: 20.0,
            saturation: 0.0,
        });
        {
            let catalog = Catalog::create_or_open(&path).unwrap();
            save_user(
                catalog.connection(),
                "Mine",
                USER_FOLDER,
                &[SettingsGroup::Presence],
                &params,
            )
            .unwrap();
        }
        let catalog = Catalog::create_or_open(&path).unwrap();
        let all = list_all(catalog.connection()).unwrap();
        let mine = all.iter().find(|p| p.name == "Mine").unwrap();
        assert!(!mine.builtin && mine.id.is_some());
        assert_eq!(mine.groups, [SettingsGroup::Presence]);
        assert!(mine.params.ops.contains_key("presence"));
        assert!(
            !mine.params.ops.contains_key("exposure"),
            "other groups dropped"
        );

        update_user(
            catalog.connection(),
            mine.id.unwrap(),
            &[SettingsGroup::BasicTone],
            &params,
        )
        .unwrap();
        rename_user(catalog.connection(), mine.id.unwrap(), "Renamed").unwrap();
        let again = list_all(catalog.connection()).unwrap();
        let mine = again.iter().find(|p| p.name == "Renamed").unwrap();
        assert!(mine.params.ops.contains_key("exposure"));
        delete_user(catalog.connection(), mine.id.unwrap()).unwrap();
        assert!(
            list_all(catalog.connection())
                .unwrap()
                .iter()
                .all(|p| p.builtin)
        );
    }

    #[test]
    fn preset_files_round_trip_and_reject_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let preset = builtins()
            .into_iter()
            .find(|p| p.name == "Cinematic")
            .unwrap();
        let file = dir.path().join("cinematic.arpreset");
        export_file(&preset, &file).unwrap();
        let back = import_file(&file).unwrap();
        assert_eq!(back.name, "Cinematic");
        assert_eq!(back.groups, preset.groups);
        assert_eq!(back.params, preset.params);
        assert!(!back.builtin && back.folder == USER_FOLDER);

        let bad = dir.path().join("bad.arpreset");
        std::fs::write(&bad, "{\"nope\":1}").unwrap();
        assert!(import_file(&bad).is_err());
        std::fs::write(
            &bad,
            "{\"viberoom_preset\":9,\"name\":\"x\",\"groups\":[],\"params\":{}}",
        )
        .unwrap();
        assert!(import_file(&bad).is_err());
    }
}
