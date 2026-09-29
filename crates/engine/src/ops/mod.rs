//! The M3 ops (plan §6.5/§6.6). Each is a params struct + metadata; GPU
//! kernels attach by `Op::ID` in the pipeline. Parameters are `f64` so the
//! persisted JSON is clean (no `0.3499999940395355`) and hashes stably.

use serde::{Deserialize, Serialize};

mod color;
mod curve;
mod detail;
mod effects;
mod geometry;

pub use color::{BAND_HUES_DEG, BANDS, BwMix, BwMixParams, Hsl, HslParams};
pub use curve::{LUT_SIZE, MonotoneCurve, ToneCurve, ToneCurveParams, build_luts};
pub use detail::{Clarity, ClarityParams, Noise, NoiseParams, Sharpen, SharpenParams};
pub use effects::{Vignette, VignetteParams};
pub use geometry::{Crop, CropParams, Straighten, StraightenParams};

use crate::op::{Curve, Op, ParamSpec, Registry, SettingsGroup, Stage, Track};

/// Every op this build knows.
pub fn default_registry() -> Registry {
    let mut r = Registry::default();
    r.register::<Straighten>();
    r.register::<Crop>();
    r.register::<WhiteBalance>();
    r.register::<Exposure>();
    r.register::<Profile>();
    r.register::<Tone>();
    r.register::<Presence>();
    r.register::<ToneCurve>();
    r.register::<Hsl>();
    r.register::<BwMix>();
    r.register::<Clarity>();
    r.register::<Noise>();
    r.register::<Sharpen>();
    r.register::<Vignette>();
    r
}

// --- White balance -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbMode {
    /// The camera's recorded white balance; params are ignored.
    #[default]
    AsShot,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WhiteBalanceParams {
    pub mode: WbMode,
    /// Kelvin (2000–50000). Only meaningful in `Custom`.
    pub temp: f64,
    /// −150…+150; describes the illuminant, so positive = greener light.
    pub tint: f64,
}

#[derive(Debug)]
pub struct WhiteBalance;

impl Op for WhiteBalance {
    type Params = WhiteBalanceParams;
    const ID: &'static str = "white_balance";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Scene;
    const ORDER: u16 = 10;
    const GROUP: SettingsGroup = SettingsGroup::WhiteBalance;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 2] = [
            ParamSpec {
                key: "temp",
                label: "Temp",
                min: 2000.0,
                max: 50000.0,
                default: 5500.0,
                fine: 10.0,
                coarse: 100.0,
                unit: "K",
                precision: 0,
                curve: Curve::Mired,
                track: Track::TempTint,
            },
            ParamSpec {
                key: "tint",
                label: "Tint",
                min: -150.0,
                max: 150.0,
                default: 0.0,
                fine: 1.0,
                coarse: 5.0,
                unit: "",
                precision: 0,
                curve: Curve::Linear,
                track: Track::TempTint,
            },
        ];
        &SPECS
    }

    fn is_identity(p: &Self::Params) -> bool {
        p.mode == WbMode::AsShot
    }
}

// --- Exposure ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ExposureParams {
    /// Stops, −5…+5.
    pub ev: f64,
}

#[derive(Debug)]
pub struct Exposure;

impl Op for Exposure {
    type Params = ExposureParams;
    const ID: &'static str = "exposure";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Scene;
    const ORDER: u16 = 20;
    const GROUP: SettingsGroup = SettingsGroup::BasicTone;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 1] = [ParamSpec {
            key: "ev",
            label: "Exposure",
            min: -5.0,
            max: 5.0,
            default: 0.0,
            fine: 0.01,
            coarse: 0.1,
            unit: "EV",
            precision: 2,
            curve: Curve::Linear,
            track: Track::Plain,
        }];
        &SPECS
    }
}

// --- Profile / treatment ---------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileName {
    #[default]
    Standard,
    Neutral,
    Linear,
    Monochrome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Treatment {
    #[default]
    Color,
    BlackWhite,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProfileParams {
    pub name: ProfileName,
    pub treatment: Treatment,
}

#[derive(Debug)]
pub struct Profile;

impl Op for Profile {
    type Params = ProfileParams;
    const ID: &'static str = "profile";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::ToneMap;
    const ORDER: u16 = 10;
    const GROUP: SettingsGroup = SettingsGroup::TreatmentProfile;

    fn specs() -> &'static [ParamSpec] {
        &[]
    }
}

// --- Tone ----------------------------------------------------------------

/// Highlights/Shadows run in the scene stage and Contrast/Whites/Blacks in
/// the tone mapper (plan §6.5), but they persist as one `tone` entry
/// (plan §5.2); the pipeline splits them when computing stage cache keys.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToneParams {
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
}

#[derive(Debug)]
pub struct Tone;

impl Op for Tone {
    type Params = ToneParams;
    const ID: &'static str = "tone";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::ToneMap;
    const ORDER: u16 = 20;
    const GROUP: SettingsGroup = SettingsGroup::BasicTone;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 5] = [
            ParamSpec::linear("contrast", "Contrast", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("highlights", "Highlights", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("shadows", "Shadows", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("whites", "Whites", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("blacks", "Blacks", -100.0, 100.0, 0.0, 1.0, 5.0),
        ];
        &SPECS
    }
}

// --- Presence (Vibrance, Saturation) --------------------------------------

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PresenceParams {
    pub vibrance: f64,
    pub saturation: f64,
}

#[derive(Debug)]
pub struct Presence;

impl Op for Presence {
    type Params = PresenceParams;
    const ID: &'static str = "presence";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Display;
    const ORDER: u16 = 30;
    const GROUP: SettingsGroup = SettingsGroup::Presence;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 2] = [
            ParamSpec::linear("vibrance", "Vibrance", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("saturation", "Saturation", -100.0, 100.0, 0.0, 1.0, 5.0),
        ];
        &SPECS
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::params::EditParams;

    #[test]
    fn registry_is_ordered_by_stage_then_order() {
        let r = default_registry();
        let ids: Vec<_> = r.iter().map(|o| o.id()).collect();
        assert_eq!(
            ids,
            [
                "straighten",
                "crop",
                "white_balance",
                "exposure",
                "noise",
                "clarity",
                "profile",
                "tone",
                "tone_curve",
                "hsl",
                "bw_mix",
                "presence",
                "sharpen",
                "vignette"
            ]
        );
    }

    #[test]
    fn every_spec_key_is_a_field_of_its_params_and_defaults_are_identity() {
        let r = default_registry();
        for op in r.iter() {
            let defaults = op.default_params();
            assert!(op.is_identity(&defaults) || op.id() == "white_balance");
            for spec in op.specs() {
                if op.id() == "white_balance" {
                    continue; // temp/tint are inert in the as-shot default; covered below
                }
                assert!(defaults.get(spec.key).is_some(), "{}.{}", op.id(), spec.key);
                assert!(spec.min < spec.max && (spec.min..=spec.max).contains(&spec.default));
            }
        }
    }

    #[test]
    fn white_balance_is_identity_only_as_shot() {
        let mut p = EditParams::default();
        p.set::<WhiteBalance>(WhiteBalanceParams {
            mode: WbMode::Custom,
            temp: 5600.0,
            tint: 8.0,
        });
        assert_eq!(p.ops["white_balance"]["temp"], 5600.0);
        p.set::<WhiteBalance>(WhiteBalanceParams::default());
        assert!(p.is_identity());
    }

    #[test]
    fn spec_example_json_from_the_plan_loads() {
        let p = EditParams::from_json(
            r#"{"process_version":1,"ops":{
              "profile":{"v":1,"name":"standard","treatment":"color"},
              "white_balance":{"v":1,"mode":"custom","temp":5600,"tint":8},
              "exposure":{"v":1,"ev":0.35},
              "tone":{"v":1,"contrast":12,"highlights":-40,"shadows":35,"whites":10,"blacks":-8}},
              "local":[]}"#,
        )
        .unwrap();
        assert_eq!(p.get::<Tone>().highlights, -40.0);
        assert_eq!(p.get::<WhiteBalance>().temp, 5600.0);
        assert_eq!(p.get::<Profile>().name, ProfileName::Standard);
    }
}
