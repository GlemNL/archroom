//! Clarity, sharpening and noise reduction (plan §6.5, §6.7).

use serde::{Deserialize, Serialize};

use crate::op::{Op, ParamSpec, SettingsGroup, Stage};

// --- Clarity -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClarityParams {
    pub clarity: f64,
}

#[derive(Debug)]
pub struct Clarity;

impl Op for Clarity {
    type Params = ClarityParams;
    const ID: &'static str = "clarity";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Scene;
    const ORDER: u16 = 40;
    const GROUP: SettingsGroup = SettingsGroup::Presence;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 1] = [ParamSpec::linear(
            "clarity", "Clarity", -100.0, 100.0, 0.0, 1.0, 5.0,
        )];
        &SPECS
    }
}

// --- Sharpening ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharpenParams {
    pub amount: f64,
    /// Blur radius in source pixels.
    pub radius: f64,
    /// Halo suppression: low values tame strong edges.
    pub detail: f64,
    /// Restricts sharpening to edges.
    pub masking: f64,
}

impl Default for SharpenParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius: 1.0,
            detail: 25.0,
            masking: 0.0,
        }
    }
}

#[derive(Debug)]
pub struct Sharpen;

impl Op for Sharpen {
    type Params = SharpenParams;
    const ID: &'static str = "sharpen";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Display;
    const ORDER: u16 = 60;
    const GROUP: SettingsGroup = SettingsGroup::Sharpening;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 4] = [
            ParamSpec::linear("amount", "Amount", 0.0, 150.0, 0.0, 1.0, 5.0),
            ParamSpec {
                precision: 1,
                ..ParamSpec::linear("radius", "Radius", 0.5, 3.0, 1.0, 0.1, 0.5)
            },
            ParamSpec::linear("detail", "Detail", 0.0, 100.0, 25.0, 1.0, 5.0),
            ParamSpec::linear("masking", "Masking", 0.0, 100.0, 0.0, 1.0, 5.0),
        ];
        &SPECS
    }

    fn is_identity(p: &Self::Params) -> bool {
        p.amount == 0.0
    }
}

// --- Noise reduction ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseParams {
    pub luma: f64,
    pub luma_detail: f64,
    pub luma_contrast: f64,
    pub color: f64,
    pub color_detail: f64,
    pub color_smoothness: f64,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self {
            luma: 0.0,
            luma_detail: 50.0,
            luma_contrast: 0.0,
            color: 0.0,
            color_detail: 50.0,
            color_smoothness: 50.0,
        }
    }
}

#[derive(Debug)]
pub struct Noise;

impl Op for Noise {
    type Params = NoiseParams;
    const ID: &'static str = "noise";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Scene;
    const ORDER: u16 = 30;
    const GROUP: SettingsGroup = SettingsGroup::NoiseReduction;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 6] = [
            ParamSpec::linear("luma", "Luminance", 0.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("luma_detail", "Detail", 0.0, 100.0, 50.0, 1.0, 5.0),
            ParamSpec::linear("luma_contrast", "Contrast", 0.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("color", "Color", 0.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("color_detail", "Detail", 0.0, 100.0, 50.0, 1.0, 5.0),
            ParamSpec::linear("color_smoothness", "Smoothness", 0.0, 100.0, 50.0, 1.0, 5.0),
        ];
        &SPECS
    }

    fn is_identity(p: &Self::Params) -> bool {
        p.luma == 0.0 && p.color == 0.0
    }
}
