//! Crop / straighten ops (plan §6.8, §8.3). They are consumed by the
//! geometry pass through [`crate::geometry`], not by a per-op kernel.
//!
//! The straightened image sits in a canvas the size of the oriented source;
//! the crop rectangle is normalised to that canvas.

use serde::{Deserialize, Serialize};

use crate::op::{Op, ParamSpec, SettingsGroup, Stage};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StraightenParams {
    /// Degrees, −45…45; positive turns the image clockwise.
    pub angle: f64,
}

#[derive(Debug)]
pub struct Straighten;

impl Op for Straighten {
    type Params = StraightenParams;
    const ID: &'static str = "straighten";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Geometry;
    const ORDER: u16 = 10;
    const GROUP: SettingsGroup = SettingsGroup::Straighten;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 1] = [ParamSpec {
            precision: 2,
            unit: "°",
            ..ParamSpec::linear("angle", "Angle", -45.0, 45.0, 0.0, 0.01, 0.5)
        }];
        &SPECS
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CropParams {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    /// Aspect lock as width:height; `[0, 0]` is free.
    pub aspect: [f64; 2],
    /// Mirror left/right (applied after the quarter turns).
    pub flip_h: bool,
    /// Clockwise quarter turns, 0..=3.
    pub quarters: i32,
}

impl Default for CropParams {
    fn default() -> Self {
        Self {
            x0: 0.0,
            y0: 0.0,
            x1: 1.0,
            y1: 1.0,
            aspect: [0.0, 0.0],
            flip_h: false,
            quarters: 0,
        }
    }
}

impl CropParams {
    pub fn rect(&self) -> [f64; 4] {
        [self.x0, self.y0, self.x1, self.y1]
    }

    pub fn set_rect(&mut self, r: [f64; 4]) {
        [self.x0, self.y0, self.x1, self.y1] = r;
    }

    pub fn is_full_rect(&self) -> bool {
        self.rect() == [0.0, 0.0, 1.0, 1.0]
    }
}

#[derive(Debug)]
pub struct Crop;

impl Op for Crop {
    type Params = CropParams;
    const ID: &'static str = "crop";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Geometry;
    const ORDER: u16 = 20;
    const GROUP: SettingsGroup = SettingsGroup::Crop;

    fn specs() -> &'static [ParamSpec] {
        &[]
    }

    /// The aspect lock alone isn't an edit: an un-cropped, un-rotated image
    /// with a lock set renders the same as one without.
    fn is_identity(p: &Self::Params) -> bool {
        p.is_full_rect() && !p.flip_h && p.quarters.rem_euclid(4) == 0
    }
}
