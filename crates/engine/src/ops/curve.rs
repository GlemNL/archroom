//! Tone curve op (plan §6.6): a parametric curve (Highlights, Lights,
//! Darks, Shadows and three region split points) followed by a point curve
//! (RGB composite, then per-channel R, G and B). The pipeline bakes both
//! into 4096-entry LUTs; this file also holds their CPU reference.

use serde::{Deserialize, Serialize};

use crate::op::{Op, ParamSpec, SettingsGroup, Stage};

pub const LUT_SIZE: usize = 4096;
pub const DEFAULT_SPLITS: [f64; 3] = [25.0, 50.0, 75.0];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToneCurveParams {
    pub highlights: f64,
    pub lights: f64,
    pub darks: f64,
    pub shadows: f64,
    /// Region boundaries in percent: shadows|darks, darks|lights, lights|highlights.
    pub splits: [f64; 3],
    /// Control points `[x, y]` in 0..1, sorted by x. Empty = identity.
    pub rgb: Vec<[f64; 2]>,
    pub red: Vec<[f64; 2]>,
    pub green: Vec<[f64; 2]>,
    pub blue: Vec<[f64; 2]>,
}

impl Default for ToneCurveParams {
    fn default() -> Self {
        Self {
            highlights: 0.0,
            lights: 0.0,
            darks: 0.0,
            shadows: 0.0,
            splits: DEFAULT_SPLITS,
            rgb: Vec::new(),
            red: Vec::new(),
            green: Vec::new(),
            blue: Vec::new(),
        }
    }
}

fn is_identity_points(p: &[[f64; 2]]) -> bool {
    p.iter().all(|q| (q[0] - q[1]).abs() < 1e-9)
}

impl ToneCurveParams {
    pub fn is_parametric_identity(&self) -> bool {
        self.highlights == 0.0 && self.lights == 0.0 && self.darks == 0.0 && self.shadows == 0.0
    }
}

#[derive(Debug)]
pub struct ToneCurve;

impl ToneCurve {
    /// The default position (percent) of region split `i` (0..3).
    pub fn default_split(i: usize) -> f64 {
        DEFAULT_SPLITS[i.min(2)]
    }
}

impl Op for ToneCurve {
    type Params = ToneCurveParams;
    const ID: &'static str = "tone_curve";
    const VERSION: u32 = 1;
    const STAGE: Stage = Stage::Display;
    const ORDER: u16 = 10;
    const GROUP: SettingsGroup = SettingsGroup::ToneCurve;

    fn specs() -> &'static [ParamSpec] {
        static SPECS: [ParamSpec; 4] = [
            ParamSpec::linear("highlights", "Highlights", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("lights", "Lights", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("darks", "Darks", -100.0, 100.0, 0.0, 1.0, 5.0),
            ParamSpec::linear("shadows", "Shadows", -100.0, 100.0, 0.0, 1.0, 5.0),
        ];
        &SPECS
    }

    fn is_identity(p: &Self::Params) -> bool {
        p.is_parametric_identity()
            && p.splits == DEFAULT_SPLITS
            && is_identity_points(&p.rgb)
            && is_identity_points(&p.red)
            && is_identity_points(&p.green)
            && is_identity_points(&p.blue)
    }
}

/// Raised-cosine bump: 1 at the centre, 0 at ±1.
fn bump(t: f64) -> f64 {
    if t.abs() >= 1.0 {
        0.0
    } else {
        let c = (std::f64::consts::FRAC_PI_2 * t).cos();
        c * c
    }
}

/// The parametric part: `x` plus a smooth bump per region. `build_luts`
/// makes it non-decreasing so extreme slider settings flatten instead of
/// inverting.
fn parametric(p: &ToneCurveParams, x: f64) -> f64 {
    const AMPLITUDE: f64 = 0.15;
    let s = p.splits.map(|v| (v / 100.0).clamp(0.02, 0.98));
    let (s1, s2, s3) = (s[0], s[1].max(s[0] + 0.01), s[2].max(s[1] + 0.02));
    let centers = [s1 / 2.0, (s1 + s2) / 2.0, (s2 + s3) / 2.0, (s3 + 1.0) / 2.0];
    let edges = [0.0, centers[0], centers[1], centers[2], centers[3], 1.0];
    let amounts = [p.shadows, p.darks, p.lights, p.highlights];
    let mut y = x;
    for i in 0..4 {
        let (left, c, right) = (edges[i], centers[i], edges[i + 2]);
        let half = if x < c { c - left } else { right - c }.max(1e-6);
        y += AMPLITUDE * amounts[i] / 100.0 * bump((x - c) / half);
    }
    y.clamp(0.0, 1.0)
}

/// Monotone cubic interpolation (Fritsch–Carlson): no overshoot between
/// control points.
#[derive(Debug, Clone)]
pub struct MonotoneCurve {
    xs: Vec<f64>,
    ys: Vec<f64>,
    tangents: Vec<f64>,
}

impl MonotoneCurve {
    /// `points` need not be sorted; fewer than two make the identity.
    pub fn new(points: &[[f64; 2]]) -> Self {
        let mut pts: Vec<[f64; 2]> = points
            .iter()
            .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
            .collect();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts.dedup_by(|b, a| (a[0] - b[0]).abs() < 1e-6);
        if pts.len() < 2 {
            pts = vec![[0.0, 0.0], [1.0, 1.0]];
        }
        let n = pts.len();
        let xs: Vec<f64> = pts.iter().map(|p| p[0]).collect();
        let ys: Vec<f64> = pts.iter().map(|p| p[1]).collect();
        let d: Vec<f64> = (0..n - 1)
            .map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]))
            .collect();
        let mut m = vec![0.0; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            m[i] = if d[i - 1] * d[i] <= 0.0 {
                0.0
            } else {
                (d[i - 1] + d[i]) / 2.0
            };
        }
        for i in 0..n - 1 {
            if d[i] == 0.0 {
                m[i] = 0.0;
                m[i + 1] = 0.0;
                continue;
            }
            let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
            let r = a.hypot(b);
            if r > 3.0 {
                m[i] = 3.0 * a / r * d[i];
                m[i + 1] = 3.0 * b / r * d[i];
            }
        }
        Self {
            xs,
            ys,
            tangents: m,
        }
    }

    pub fn eval(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let i = self.xs.partition_point(|v| *v <= x) - 1;
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[i]
            + (t3 - 2.0 * t2 + t) * h * self.tangents[i]
            + (-2.0 * t3 + 3.0 * t2) * self.ys[i + 1]
            + (t3 - t2) * h * self.tangents[i + 1];
        y.clamp(0.0, 1.0)
    }
}

/// The R, G and B lookup tables (`LUT_SIZE` entries over 0..1): parametric
/// curve, then the RGB point curve, then the channel's own curve.
pub fn build_luts(p: &ToneCurveParams) -> [Vec<f32>; 3] {
    // Enforce a non-decreasing parametric curve (see `parametric`).
    let mut floor = 0.0f64;
    let para: Vec<f64> = (0..LUT_SIZE)
        .map(|i| {
            floor = floor.max(parametric(p, i as f64 / (LUT_SIZE - 1) as f64));
            floor
        })
        .collect();
    let rgb = MonotoneCurve::new(&p.rgb);
    let chans = [&p.red, &p.green, &p.blue].map(|pts| MonotoneCurve::new(pts));
    let mut out = [
        Vec::with_capacity(LUT_SIZE),
        Vec::with_capacity(LUT_SIZE),
        Vec::with_capacity(LUT_SIZE),
    ];
    for &para_v in &para {
        let v = rgb.eval(para_v);
        for c in 0..3 {
            out[c].push(chans[c].eval(v) as f32);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::needless_range_loop)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity_and_the_lut_is_a_ramp() {
        assert!(ToneCurve::is_identity(&ToneCurveParams::default()));
        let luts = build_luts(&ToneCurveParams::default());
        for c in 0..3 {
            for i in [0, 1000, 4095] {
                let x = i as f32 / 4095.0;
                assert!((luts[c][i] - x).abs() < 1e-5, "{c} {i}");
            }
        }
    }

    #[test]
    fn a_diagonal_point_list_is_identity() {
        let p = ToneCurveParams {
            rgb: vec![[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]],
            ..Default::default()
        };
        assert!(ToneCurve::is_identity(&p));
    }

    #[test]
    fn point_curve_hits_its_control_points_without_overshoot() {
        let c = MonotoneCurve::new(&[[0.0, 0.0], [0.25, 0.1], [0.5, 0.6], [0.6, 0.62], [1.0, 1.0]]);
        assert!((c.eval(0.25) - 0.1).abs() < 1e-9);
        assert!((c.eval(0.5) - 0.6).abs() < 1e-9);
        let mut prev = 0.0;
        for i in 0..=1000 {
            let y = c.eval(i as f64 / 1000.0);
            assert!(y >= prev - 1e-12, "monotone at {i}");
            assert!((0.0..=1.0).contains(&y));
            prev = y;
        }
    }

    #[test]
    fn parametric_sliders_move_their_own_region_and_stay_monotone() {
        let p = ToneCurveParams {
            shadows: 100.0,
            highlights: -100.0,
            ..Default::default()
        };
        assert!(parametric(&p, 0.12) > 0.12 + 0.05, "shadows lifted");
        assert!(parametric(&p, 0.88) < 0.88 - 0.05, "highlights pulled down");
        assert!((parametric(&p, 0.5) - 0.5).abs() < 0.02, "mids barely move");
        assert_eq!(parametric(&p, 0.0), 0.0);
        assert!((parametric(&p, 1.0) - 1.0).abs() < 1e-9);
        let luts = build_luts(&p);
        assert!(luts[0].windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn channel_curves_only_touch_their_channel() {
        let p = ToneCurveParams {
            red: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
            ..Default::default()
        };
        let l = build_luts(&p);
        assert!(l[0][2048] > 0.65);
        assert!((l[1][2048] - 0.5).abs() < 1e-3);
    }
}
