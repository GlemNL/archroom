//! HSL / Color Mixer and the B&W mixer (plan §6.6): eight hue bands in
//! OkLCh, each with a hue shift, chroma scale and lightness shift; under
//! the B&W treatment the same bands mix luminance.

use serde::{Deserialize, Serialize};

use crate::op::{Op, ParamSpec, SettingsGroup, Stage};

pub const BANDS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];

/// Band centres as OkLCh hue angles in degrees (sRGB primaries/secondaries
/// and their in-betweens), ascending.
pub const BAND_HUES_DEG: [f32; 8] = [29.0, 55.0, 110.0, 142.0, 195.0, 264.0, 303.0, 328.0];

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HslParams {
    pub hue: [f64; 8],
    pub sat: [f64; 8],
    pub lum: [f64; 8],
}

#[derive(Debug)]
pub struct Hsl;

impl Op for Hsl {
    type Params = HslParams;
    const ID: &'static str = "hsl";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Display;
    const ORDER: u16 = 20;
    const GROUP: SettingsGroup = SettingsGroup::HslColor;

    fn specs() -> &'static [ParamSpec] {
        &[]
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BwMixParams {
    pub mix: [f64; 8],
}

#[derive(Debug)]
pub struct BwMix;

impl Op for BwMix {
    type Params = BwMixParams;
    const ID: &'static str = "bw_mix";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Display;
    const ORDER: u16 = 25;
    const GROUP: SettingsGroup = SettingsGroup::BwMix;

    fn specs() -> &'static [ParamSpec] {
        &[]
    }
}
