//! Effects (plan §8.2): the post-crop vignette. It runs on the geometry
//! stage's output, so once cropping exists it is relative to the crop.

use serde::{Deserialize, Serialize};

use crate::op::{Op, ParamSpec, SettingsGroup, Stage};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VignetteParams {
    /// −100 (darken) … +100 (lighten).
    pub amount: f64,
    /// Where the effect starts, 0 (centre) … 100 (corners).
    pub midpoint: f64,
    /// −100 (squarer) … +100 (rounder).
    pub roundness: f64,
    pub feather: f64,
    /// Protects highlights from darkening.
    pub highlights: f64,
}

impl Default for VignetteParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            midpoint: 50.0,
            roundness: 0.0,
            feather: 50.0,
            highlights: 0.0,
        }
    }
}

#[derive(Debug)]
pub struct Vignette;

impl Op for Vignette {
    type Params = VignetteParams;
    const ID: &'static str = "vignette";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Display;
    const ORDER: u16 = 70;
    const GROUP: SettingsGroup = SettingsGroup::Effects;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 5] = [
            ParamSpec::linear("amount", "Amount", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("midpoint", "Midpoint", 0.0, 100.0, 50.0, 1.0, 5.0),
            ParamSpec::linear("roundness", "Roundness", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("feather", "Feather", 0.0, 100.0, 50.0, 1.0, 5.0),
            ParamSpec::linear("highlights", "Highlights", 0.0, 100.0, 0.0, 1.0, 5.0),
        ];
        &SPECS
    }

    fn is_identity(p: &Self::Params) -> bool {
        p.amount == 0.0
    }
}
