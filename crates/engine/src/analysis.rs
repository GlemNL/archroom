//! CPU analysis on a small proxy: Auto White Balance and Auto Tone (plan
//! §6.4/§6.5). Both return ordinary slider values the user keeps editing;
//! neither is a hidden mode.

use viberoom_color::Mat3d;
use viberoom_color::temp::xy_to_temp_tint;
use viberoom_io::ImageF32;

use crate::ops::{ProfileParams, ToneParams};
use crate::rawprep::SceneColor;
use crate::tone::{tone_curve, tone_uniform};

/// A downscaled copy of the decoded image (camera RGB for raws, linear
/// sRGB for rendered files).
#[derive(Debug, Clone)]
pub struct Proxy {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<[f32; 3]>,
}

/// Box-averages `image` down to at most `max_edge` on its long side.
/// `linearize` decodes sRGB-encoded (rendered) data first.
pub fn downscale(image: &ImageF32, max_edge: u32, linearize: bool) -> Proxy {
    let scale = (max_edge.max(1) as f32 / image.width.max(image.height) as f32).min(1.0);
    let w = ((image.width as f32 * scale).round() as u32).max(1);
    let h = ((image.height as f32 * scale).round() as u32).max(1);
    let ch = image.channels.max(3) as usize;
    let mut rgb = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let (y0, y1) = span(y, h, image.height);
        for x in 0..w {
            let (x0, x1) = span(x, w, image.width);
            let mut acc = [0.0f64; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy as usize * image.width as usize + sx as usize) * ch;
                    for (c, a) in acc.iter_mut().enumerate() {
                        let v = image.data[i + c];
                        *a += f64::from(if linearize {
                            viberoom_color::srgb_eotf(v)
                        } else {
                            v
                        });
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as f64;
            rgb.push([
                (acc[0] / n) as f32,
                (acc[1] / n) as f32,
                (acc[2] / n) as f32,
            ]);
        }
    }
    Proxy {
        width: w,
        height: h,
        rgb,
    }
}

fn span(i: u32, n: u32, src: u32) -> (u32, u32) {
    let a = (i as u64 * src as u64 / n as u64) as u32;
    let b = (((i as u64 + 1) * src as u64).div_ceil(n as u64) as u32).clamp(a + 1, src);
    (a, b)
}

/// Gray-world Auto WB: the illuminant that makes the mean of the usable
/// pixels neutral, as (Temp K, Tint). Clipped and very dark pixels are
/// ignored. `None` if nothing usable remains.
pub fn auto_wb(proxy: &Proxy, scene: &SceneColor) -> Option<(f64, f64)> {
    let mut sum = [0.0f64; 3];
    let mut n = 0u64;
    for p in &proxy.rgb {
        let m = p[0].max(p[1]).max(p[2]);
        let lum = (p[0] + p[1] + p[2]) / 3.0;
        if m >= 0.98 || lum < 0.02 {
            continue;
        }
        for c in 0..3 {
            sum[c] += p[c] as f64;
        }
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let mean = sum.map(|s| s / n as f64);
    let (temp, tint) = xy_to_temp_tint(scene.white_from_decoded(mean));
    Some((temp.round(), tint.round()))
}

/// Slider values chosen by Auto Tone.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoTone {
    pub exposure_ev: f64,
    pub tone: ToneParams,
}

fn percentile(sorted: &[f32], q: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * q).round() as usize]
}

/// Auto Tone (plan §6.5): exposure puts the median at middle gray, then
/// Whites/Blacks are solved (bisection on the real curve) so ~0.1 % of
/// pixels touch white/black, and Shadows/Highlights ease a wide range.
/// `to_working` maps proxy pixels to linear working RGB (the current WB).
pub fn auto_tone(
    proxy: &Proxy,
    to_working: &Mat3d,
    profile: &ProfileParams,
    rendered: bool,
) -> Option<AutoTone> {
    let mut lum: Vec<f32> = proxy
        .rgb
        .iter()
        .map(|p| {
            let c = to_working.mul_vec([p[0] as f64, p[1] as f64, p[2] as f64]);
            (0.2627 * c[0] + 0.6780 * c[1] + 0.0593 * c[2]).max(0.0) as f32
        })
        .filter(|v| *v > 1e-5)
        .collect();
    if lum.len() < 16 {
        return None;
    }
    lum.sort_by(|a, b| a.total_cmp(b));
    let (p001, p10, p50, p90, p999) = (
        percentile(&lum, 0.001),
        percentile(&lum, 0.10),
        percentile(&lum, 0.50),
        percentile(&lum, 0.90),
        percentile(&lum, 0.999),
    );

    let ev = f64::from((0.18 / p50).log2()).clamp(-3.0, 3.0) * 0.9;
    let gain = 2f32.powf(ev as f32);

    // Dynamic range after exposure, in stops between the 10th and 90th percentile.
    let range = (p90 / p10).log2();
    let ease = ((range - 6.0) * 12.0).clamp(0.0, 60.0) as f64;
    let mut tone = ToneParams {
        shadows: (ease * 0.8).round(),
        highlights: if ease > 0.0 { -ease.round() } else { 0.0 },
        ..Default::default()
    };

    // Solve whites/blacks on the real curve (monotonic in the slider).
    let solve = |target: f32, y: f32, set: &dyn Fn(&mut ToneParams, f64)| -> f64 {
        let (mut lo, mut hi) = (-100.0f64, 100.0f64);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            let mut t = ToneParams::default();
            set(&mut t, mid);
            let out = tone_curve(&tone_uniform(profile, &t, rendered), y);
            if out < target { lo = mid } else { hi = mid }
        }
        ((lo + hi) / 2.0).round()
    };
    // More whites = brighter, so the slider that hits the target output is the answer.
    tone.whites = solve(0.985, p999 * gain, &|t, v| t.whites = v).clamp(-100.0, 100.0);
    tone.blacks = solve(0.01, p001 * gain, &|t, v| t.blacks = v).clamp(-100.0, 100.0);
    if range < 4.0 {
        tone.contrast = 15.0;
    }
    Some(AutoTone {
        exposure_ev: (ev * 100.0).round() / 100.0,
        tone,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use viberoom_color::cat::Cat;
    use viberoom_color::cie::{D65, SRGB};
    use viberoom_io::CameraColor;

    fn scene() -> SceneColor {
        SceneColor::new(&CameraColor {
            xyz_to_camera: SRGB.xyz_to_rgb().to_f32(),
            d65_mul: [1.0; 3],
            as_shot_mul: None,
        })
        .unwrap()
    }

    fn proxy(f: impl Fn(usize) -> [f32; 3], n: usize) -> Proxy {
        Proxy {
            width: n as u32,
            height: 1,
            rgb: (0..n).map(f).collect(),
        }
    }

    #[test]
    fn downscale_averages_blocks_and_never_upscales() {
        let img = ImageF32 {
            width: 4,
            height: 2,
            channels: 3,
            data: (0..8).flat_map(|i| [i as f32; 3]).collect(),
        };
        let p = downscale(&img, 2, false);
        assert_eq!((p.width, p.height), (2, 1));
        assert_eq!(p.rgb[0], [2.5; 3]); // mean of 0,1,4,5
        assert_eq!(p.rgb[1], [4.5; 3]); // mean of 2,3,6,7
        assert_eq!(downscale(&img, 100, false).width, 4);
    }

    #[test]
    fn auto_wb_neutralises_a_colour_cast() {
        // A grey-ish scene under a warm light: every pixel has the cast.
        let cast = [1.3f32, 1.0, 0.7];
        let pr = proxy(
            |i| {
                let v = 0.1 + 0.5 * (i as f32 / 199.0);
                cast.map(|c| c * v * 0.7)
            },
            200,
        );
        let sc = scene();
        let (t, tint) = auto_wb(&pr, &sc).unwrap();
        let xy = viberoom_color::temp::temp_tint_to_xy(t, tint);
        let m = sc.decoded_to_working(xy, Cat::Cat16);
        let out = m.mul_vec([0.26 * 1.3, 0.26, 0.26 * 0.7]);
        assert!(
            (out[0] / out[1] - 1.0).abs() < 0.06 && (out[2] / out[1] - 1.0).abs() < 0.06,
            "{out:?} at {t} K/{tint}"
        );
        assert!(
            t < 5000.0,
            "warm light should read as a low temperature, got {t}"
        );
    }

    #[test]
    fn auto_wb_of_a_neutral_scene_is_near_daylight_and_ignores_clipped_pixels() {
        let pr = proxy(
            |i| {
                if i % 5 == 0 {
                    [1.0, 0.2, 0.2]
                } else {
                    [0.3; 3]
                }
            },
            100,
        );
        let (t, tint) = auto_wb(&pr, &scene()).unwrap();
        assert!(
            (t - 6504.0).abs() < 400.0 && (tint - 9.8).abs() < 4.0,
            "{t} K / {tint}"
        );
        assert!(auto_wb(&proxy(|_| [0.0; 3], 10), &scene()).is_none());
        let _ = D65;
    }

    #[test]
    fn auto_tone_brightens_dark_images_and_darkens_bright_ones() {
        let m = Mat3d::IDENTITY;
        let prof = ProfileParams::default();
        let dark = proxy(|i| [0.01 + 0.08 * (i as f32 / 999.0); 3], 1000);
        let bright = proxy(|i| [0.3 + 1.2 * (i as f32 / 999.0); 3], 1000);
        let d = auto_tone(&dark, &m, &prof, false).unwrap();
        let b = auto_tone(&bright, &m, &prof, false).unwrap();
        assert!(d.exposure_ev > 1.0, "{d:?}");
        assert!(b.exposure_ev < -0.5, "{b:?}");
        // The median of the corrected dark image lands near middle gray.
        let u = tone_uniform(&prof, &d.tone, false);
        let mid = tone_curve(&u, 0.05 * 2f32.powf(d.exposure_ev as f32));
        // (Auto Tone also stretches a narrow range toward white, so this lands a bit above 0.18.)
        assert!((0.08..0.65).contains(&mid), "{mid}");
    }

    #[test]
    fn auto_tone_whites_and_blacks_hit_their_targets() {
        let pr = proxy(|i| [0.002 + 4.0 * (i as f32 / 1999.0).powi(2); 3], 2000);
        let prof = ProfileParams::default();
        let a = auto_tone(&pr, &Mat3d::IDENTITY, &prof, false).unwrap();
        let u = tone_uniform(&prof, &a.tone, false);
        let gain = 2f32.powf(a.exposure_ev as f32);
        assert!((tone_curve(&u, 4.002 * gain) - 0.985).abs() < 0.05);
        assert!(a.tone.whites.abs() <= 100.0 && a.tone.blacks.abs() <= 100.0);
    }
}
