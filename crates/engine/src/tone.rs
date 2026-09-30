//! The tone mapper's parameters and its CPU reference (plan §6.5). The WGSL
//! in `shaders/tone.wgsl` implements exactly `tone_curve`; a GPU test
//! compares the two on a ramp.

use crate::ops::{ProfileName, ProfileParams, ToneParams};

/// Everything `tone.wgsl` needs, already resolved from profile + sliders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneUniform {
    /// Hable A, B, C, D.
    pub abcd: [f32; 4],
    /// Hable E, F, white point, exposure bias.
    pub efwb: [f32; 4],
    /// Contrast exponent (pivot 0.18), black lift, mode (0 = filmic, 1 = clamp), unused.
    pub cbm: [f32; 4],
}

const HABLE: [f32; 6] = [0.15, 0.50, 0.10, 0.20, 0.02, 0.30];

/// `rendered` files use an identity base curve (plan §6.5), so default
/// sliders reproduce the file unchanged.
pub fn tone_uniform(profile: &ProfileParams, tone: &ToneParams, rendered: bool) -> ToneUniform {
    let (bias, white, base_exp, clamp_only) = match profile.name {
        // The white point equals the bias so a clipped raw (scene 1.0) reaches
        // display white instead of stopping at ~75 % grey.
        ProfileName::Standard | ProfileName::Monochrome => (2.0, 2.0, 1.1, false),
        ProfileName::Neutral => (2.0, 2.0, 0.9, false),
        ProfileName::Linear => (1.0, 1.0, 1.0, true),
    };
    let clamp_only = clamp_only || rendered;
    let white = white * (-(tone.whites as f32) / 40.0).exp2();
    let contrast = base_exp * (1.0 + tone.contrast as f32 * 0.004);
    ToneUniform {
        abcd: [HABLE[0], HABLE[1], HABLE[2], HABLE[3]],
        efwb: [HABLE[4], HABLE[5], white, bias],
        cbm: [
            if clamp_only {
                1.0 + tone.contrast as f32 * 0.004
            } else {
                contrast
            },
            0.06 * tone.blacks as f32 / 100.0,
            if clamp_only { 1.0 } else { 0.0 },
            0.0,
        ],
    }
}

fn hable(u: &ToneUniform, x: f32) -> f32 {
    let [a, b, c, d] = u.abcd;
    let (e, f) = (u.efwb[0], u.efwb[1]);
    ((x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f)) - e / f
}

/// Scene luminance → display-linear luminance.
pub fn tone_curve(u: &ToneUniform, y: f32) -> f32 {
    let mut o = y.min(1.0);
    if u.cbm[2] < 0.5 {
        o = hable(u, u.efwb[3] * y) / hable(u, u.efwb[2]);
    }
    o = o.clamp(0.0, 1.0);
    o = 0.18 * (o / 0.18).powf(u.cbm[0]);
    o = o.clamp(0.0, 1.0);
    let k = 1.0 - o;
    (o + u.cbm[1] * k * k).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn std_curve(tone: ToneParams) -> ToneUniform {
        tone_uniform(&ProfileParams::default(), &tone, false)
    }

    #[test]
    fn standard_maps_middle_gray_near_middle_gray_and_white_to_one() {
        let u = std_curve(ToneParams::default());
        assert!(
            (tone_curve(&u, 0.18) - 0.18).abs() < 0.1,
            "{}",
            tone_curve(&u, 0.18)
        );
        assert!((tone_curve(&u, 1.0) - 1.0).abs() < 1e-4);
        assert!(tone_curve(&u, 0.0) < 1e-6);
    }

    #[test]
    fn the_curve_is_monotonic() {
        for tone in [
            ToneParams::default(),
            ToneParams {
                contrast: 80.0,
                blacks: -60.0,
                whites: 50.0,
                ..Default::default()
            },
            ToneParams {
                contrast: -80.0,
                blacks: 60.0,
                whites: -50.0,
                ..Default::default()
            },
        ] {
            let u = std_curve(tone);
            let mut last = -1.0;
            for i in 0..=400 {
                let y = (i as f32 / 400.0).powi(3) * 20.0;
                let o = tone_curve(&u, y);
                assert!(o >= last - 1e-6, "not monotonic at y={y}");
                last = o;
            }
        }
    }

    #[test]
    fn sliders_move_the_expected_way() {
        let base = std_curve(ToneParams::default());
        let hi_contrast = std_curve(ToneParams {
            contrast: 60.0,
            ..Default::default()
        });
        assert!(tone_curve(&hi_contrast, 0.5) > tone_curve(&base, 0.5));
        assert!(tone_curve(&hi_contrast, 0.05) < tone_curve(&base, 0.05));
        let lift = std_curve(ToneParams {
            blacks: 50.0,
            ..Default::default()
        });
        assert!(tone_curve(&lift, 0.01) > tone_curve(&base, 0.01));
        let bright_whites = std_curve(ToneParams {
            whites: 60.0,
            ..Default::default()
        });
        assert!(tone_curve(&bright_whites, 0.5) > tone_curve(&base, 0.5));
    }

    #[test]
    fn linear_profile_and_rendered_files_are_identity_at_default() {
        let lin = tone_uniform(
            &ProfileParams {
                name: ProfileName::Linear,
                ..Default::default()
            },
            &ToneParams::default(),
            false,
        );
        let rendered = tone_uniform(&ProfileParams::default(), &ToneParams::default(), true);
        for y in [0.0, 0.01, 0.2, 0.5, 0.99] {
            assert!((tone_curve(&lin, y) - y).abs() < 1e-6);
            assert!((tone_curve(&rendered, y) - y).abs() < 1e-6);
        }
    }
}
