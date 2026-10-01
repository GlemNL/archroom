//! Red-eye and pet-eye correction (plan v0.2.0 §2.2). Each spot is a
//! circle placed on an eye, stored in source-normalised coordinates so
//! re-cropping or rotating the photo leaves it on the eye.

use serde::{Deserialize, Serialize};

use crate::op::{Op, ParamSpec, SettingsGroup, Stage};

/// Most spots one photo may carry (the kernel's uniform array size).
pub const MAX_SPOTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EyeMode {
    /// Red pupils from on-camera flash.
    #[default]
    Red,
    /// The white, green or yellow glow in animals' eyes.
    Pet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EyeSpot {
    /// Centre, normalised to the decoded source.
    pub x: f64,
    pub y: f64,
    /// Radius as a fraction of the source diagonal.
    pub r: f64,
    pub mode: EyeMode,
    /// How dark the corrected pupil becomes, 0–100.
    pub darken: f64,
    /// Pet mode only: add a small specular dot.
    pub catchlight: bool,
}

impl Default for EyeSpot {
    fn default() -> Self {
        Self {
            x: 0.5,
            y: 0.5,
            r: 0.01,
            mode: EyeMode::Red,
            darken: 50.0,
            catchlight: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RedEyeParams {
    pub spots: Vec<EyeSpot>,
}

#[derive(Debug)]
pub struct RedEye;

impl Op for RedEye {
    type Params = RedEyeParams;
    const ID: &'static str = "red_eye";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Scene;
    /// After the scene matrix (white balance, exposure), before noise
    /// reduction.
    const ORDER: u16 = 25;
    const GROUP: SettingsGroup = SettingsGroup::RedEye;

    fn specs() -> &'static [ParamSpec] {
        &[]
    }
}
