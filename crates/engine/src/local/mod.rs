//! Local adjustments (plan v0.2.0 §2.3): a linear gradient or a painted
//! zone that gets its own Exposure, Contrast, Highlights, Shadows, Whites
//! and Blacks. `EditParams.local` stays a list of JSON values on disk so an
//! entry this build doesn't understand (a mask kind from a newer version)
//! survives a round trip; [`EditParams::local_adjustments`] parses the
//! entries it knows.

mod brush;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xxhash_rust::xxh3::xxh3_64;

pub use brush::{BrushRaster, MAX_MASK_EDGE};

use crate::params::EditParams;

/// Most local adjustments one photo may carry.
pub const MAX_LOCAL: usize = 16;

/// The version written into every entry.
pub const LOCAL_VERSION: u32 = 1;

/// The constants that make the local sliders feel like the global ones.
/// First guesses, to be tuned against Lightroom (plan §3.2); the shader
/// receives them as uniforms so tuning never touches the kernel.
pub mod tuning {
    /// Contrast slider at ±100: EV added per EV away from middle gray.
    pub const CONTRAST_SLOPE: f32 = 0.5;
    /// Whites at ±100: EV gained in the upper range.
    pub const WHITES_EV: f32 = 1.0;
    /// Blacks at ±100: EV gained in the lower range.
    pub const BLACKS_EV: f32 = 1.0;
    /// EV (around middle gray) the whites weight rises over.
    pub const WHITES_RANGE: [f32; 2] = [1.5, 3.5];
    /// EV (around middle gray) the blacks weight falls over.
    pub const BLACKS_RANGE: [f32; 2] = [-6.0, -3.0];
}

/// The six Basic tone sliders, with the global ranges.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalTone {
    /// Stops, −5…+5.
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
}

impl LocalTone {
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }

    /// The values as the kernel takes them: exposure in EV, the rest −1…1.
    pub fn unit(&self) -> [f32; 6] {
        let pct = |v: f64| (v / 100.0).clamp(-1.0, 1.0) as f32;
        [
            self.exposure.clamp(-5.0, 5.0) as f32,
            pct(self.contrast),
            pct(self.highlights),
            pct(self.shadows),
            pct(self.whites),
            pct(self.blacks),
        ]
    }
}

/// A line through `(x, y)` at `angle`; the mask is 1 on the side its normal
/// points to (below the line at 0°) and 0 on the other, with a smoothstep
/// ramp of total width `feather` centred on the line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinearMask {
    /// A point on the line, normalised to the decoded source.
    pub x: f64,
    pub y: f64,
    /// Degrees in the source pixel frame (y down, 0° = +x).
    pub angle: f64,
    /// Transition width as a fraction of the source diagonal.
    pub feather: f64,
}

impl LinearMask {
    /// Mask value at the source position `(x, y)` (normalised) of a
    /// `src`-pixel image.
    pub fn weight(&self, x: f64, y: f64, src: (u32, u32)) -> f64 {
        let (w, h) = (f64::from(src.0), f64::from(src.1));
        let (s, c) = self.angle.to_radians().sin_cos();
        let d = (x - self.x) * w * -s + (y - self.y) * h * c;
        let width = (self.feather * w.hypot(h)).max(1e-6);
        smoothstep((d / width + 0.5).clamp(0.0, 1.0))
    }
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// One brush stroke: soft discs stamped along `points`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    /// Diameter as a fraction of the source diagonal.
    pub size: f64,
    /// 0 (hard edge) … 1 (soft from the centre), the soft fraction of the
    /// radius.
    pub feather: f64,
    /// Opacity of a dab, 0–1.
    pub flow: f64,
    /// An erase stroke removes from the zone instead of adding to it.
    pub erase: bool,
    /// Normalised to the decoded source.
    pub points: Vec<[f64; 2]>,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            size: 0.03,
            feather: 0.5,
            flow: 1.0,
            erase: false,
            points: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaskDef {
    Linear(LinearMask),
    Brush { strokes: Vec<Stroke> },
}

impl MaskDef {
    /// Stable hash of the brush strokes (0 for a gradient).
    pub fn strokes_hash(&self) -> u64 {
        match self {
            MaskDef::Brush { strokes } => xxh3_64(
                serde_json::to_string(strokes)
                    .unwrap_or_default()
                    .as_bytes(),
            ),
            MaskDef::Linear(_) => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalAdjustment {
    /// Algorithm version (always [`LOCAL_VERSION`] when written).
    #[serde(default = "version")]
    pub v: u32,
    /// Unique within the photo.
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub mask: MaskDef,
    #[serde(default)]
    pub adjust: LocalTone,
}

fn version() -> u32 {
    LOCAL_VERSION
}

fn yes() -> bool {
    true
}

impl LocalAdjustment {
    pub fn new(id: impl Into<String>, name: impl Into<String>, mask: MaskDef) -> Self {
        Self {
            v: LOCAL_VERSION,
            id: id.into(),
            name: name.into(),
            enabled: true,
            mask,
            adjust: LocalTone::default(),
        }
    }

    /// True when the adjustment changes nothing (disabled, all sliders at
    /// zero, or an empty brush zone).
    pub fn is_identity(&self) -> bool {
        !self.enabled
            || self.adjust.is_zero()
            || matches!(&self.mask, MaskDef::Brush { strokes } if strokes.iter().all(|s| s.erase || s.points.is_empty()))
    }
}

impl EditParams {
    /// The local adjustments this build understands, in order. Entries with
    /// an unknown version or mask kind are skipped here but kept in
    /// `local`.
    pub fn local_adjustments(&self) -> Vec<LocalAdjustment> {
        self.local
            .iter()
            .filter_map(|v| serde_json::from_value::<LocalAdjustment>(v.clone()).ok())
            .filter(|a| a.v == LOCAL_VERSION)
            .collect()
    }

    /// Replaces the adjustments this build understands with `list` (entries
    /// whose sliders are all zero are dropped, like identity ops; at most
    /// [`MAX_LOCAL`] are kept). Entries it doesn't understand stay,
    /// after the known ones.
    pub fn set_local_adjustments(&mut self, list: &[LocalAdjustment]) {
        self.write_local(list, false);
    }

    /// Like [`set_local_adjustments`](Self::set_local_adjustments) but keeps
    /// entries whose sliders are all zero: what a tool renders while a mask
    /// is being placed, before it adjusts anything.
    pub fn set_local_draft(&mut self, list: &[LocalAdjustment]) {
        self.write_local(list, true);
    }

    fn write_local(&mut self, list: &[LocalAdjustment], keep_zero: bool) {
        let unknown: Vec<Value> = self
            .local
            .iter()
            .filter(|v| {
                serde_json::from_value::<LocalAdjustment>((*v).clone())
                    .map(|a| a.v != LOCAL_VERSION)
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        self.local = list
            .iter()
            .filter(|a| keep_zero || !a.adjust.is_zero())
            .take(MAX_LOCAL)
            .filter_map(|a| {
                let mut a = a.clone();
                a.v = LOCAL_VERSION;
                serde_json::to_value(a).ok()
            })
            .chain(unknown)
            .collect();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn gradient(id: &str) -> LocalAdjustment {
        let mut a = LocalAdjustment::new(
            id,
            "Gradient 1",
            MaskDef::Linear(LinearMask {
                x: 0.5,
                y: 0.42,
                angle: 3.5,
                feather: 0.15,
            }),
        );
        a.adjust.exposure = -0.8;
        a.adjust.highlights = -30.0;
        a
    }

    #[test]
    fn typed_entries_round_trip_through_the_params_json() {
        let mut p = EditParams::default();
        let mut zone = LocalAdjustment::new(
            "p0aa",
            "Zone 1",
            MaskDef::Brush {
                strokes: vec![Stroke {
                    points: vec![[0.31, 0.62], [0.33, 0.61]],
                    ..Default::default()
                }],
            },
        );
        zone.adjust.shadows = 25.0;
        p.set_local_adjustments(&[gradient("k3f9"), zone.clone()]);
        let back = EditParams::from_json(&p.to_json()).unwrap();
        assert_eq!(back.local_adjustments(), [gradient("k3f9"), zone]);
        assert_eq!(back.local[0]["mask"]["kind"], "linear");
        assert_eq!(back.local[0]["v"], 1);
    }

    #[test]
    fn the_plan_example_loads_with_defaults_for_missing_fields() {
        let p = EditParams::from_json(
            r#"{"process_version":1,"ops":{},"local":[
              {"v":1,"id":"p0aa","name":"Zone 1","enabled":true,
               "mask":{"kind":"brush","strokes":[
                 {"size":0.03,"feather":0.5,"flow":1.0,"erase":false,
                  "points":[[0.31,0.62],[0.33,0.61],[0.36,0.60]]}]},
               "adjust":{"exposure":0.4,"shadows":25}}]}"#,
        )
        .unwrap();
        let a = &p.local_adjustments()[0];
        assert_eq!(a.adjust.exposure, 0.4);
        assert_eq!(a.adjust.contrast, 0.0);
        assert!(matches!(&a.mask, MaskDef::Brush { strokes } if strokes[0].points.len() == 3));
    }

    #[test]
    fn unknown_mask_kinds_and_versions_are_skipped_but_kept() {
        let json = r#"{"process_version":1,"ops":{},"local":[
          {"v":1,"id":"a","mask":{"kind":"linear","x":0.5,"y":0.5,"angle":0,"feather":0.1},
           "adjust":{"exposure":1}},
          {"v":1,"id":"r","mask":{"kind":"radial","cx":0.5},"adjust":{"exposure":1}},
          {"v":7,"id":"f","mask":{"kind":"linear","x":0.5,"y":0.5,"angle":0,"feather":0.1},
           "adjust":{"exposure":1}}]}"#;
        let mut p = EditParams::from_json(json).unwrap();
        assert_eq!(p.local_adjustments().len(), 1);
        // Editing the known one keeps the other two.
        let mut list = p.local_adjustments();
        list[0].adjust.exposure = 2.0;
        p.set_local_adjustments(&list);
        assert_eq!(p.local.len(), 3);
        assert_eq!(p.local[0]["adjust"]["exposure"], 2.0);
        assert_eq!(p.local[1]["id"], "r");
        assert_eq!(p.local[2]["id"], "f");
        // Removing every known one still keeps them.
        p.set_local_adjustments(&[]);
        assert_eq!(p.local.len(), 2);
    }

    #[test]
    fn all_zero_adjustments_are_dropped_and_the_count_is_capped() {
        let mut p = EditParams::default();
        let mut zero = gradient("z");
        zero.adjust = LocalTone::default();
        p.set_local_adjustments(&[zero]);
        assert!(p.is_identity());
        let many: Vec<_> = (0..MAX_LOCAL + 3)
            .map(|i| gradient(&format!("g{i}")))
            .collect();
        p.set_local_adjustments(&many);
        assert_eq!(p.local.len(), MAX_LOCAL);
    }

    #[test]
    fn a_linear_mask_is_zero_one_and_half_where_it_should_be() {
        let m = LinearMask {
            x: 0.5,
            y: 0.5,
            angle: 0.0,
            feather: 0.1,
        };
        let src = (1000, 500);
        let diag = 1000f64.hypot(500.0);
        // Horizontal line: 1 below, 0 above, 0.5 on it.
        assert_eq!(m.weight(0.3, 0.9, src), 1.0);
        assert_eq!(m.weight(0.3, 0.1, src), 0.0);
        assert!((m.weight(0.3, 0.5, src) - 0.5).abs() < 1e-12);
        // The ramp is `feather × diagonal` wide.
        let half = 0.05 * diag / 500.0;
        assert!(m.weight(0.3, 0.5 + half * 0.999, src) > 0.99);
        assert!(m.weight(0.3, 0.5 - half * 0.999, src) < 0.01);
        // +180° swaps the affected side.
        let flipped = LinearMask { angle: 180.0, ..m };
        assert_eq!(flipped.weight(0.3, 0.9, src), 0.0);
        assert_eq!(flipped.weight(0.3, 0.1, src), 1.0);
        // A 90° line: the normal points to −x.
        let v = LinearMask { angle: 90.0, ..m };
        assert_eq!(v.weight(0.1, 0.5, src), 1.0);
        assert_eq!(v.weight(0.9, 0.5, src), 0.0);
    }
}
