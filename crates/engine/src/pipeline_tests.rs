//! GPU pipeline tests. The synthetic "camera" is linear sRGB (identity
//! matrices, unit D65 multipliers), so expected values can be computed by
//! hand; tests skip quietly when no Vulkan adapter is available.

use viberoom_color::cie::SRGB;
use viberoom_color::srgb_oetf;
use viberoom_io::{CameraColor, DecodedImage, ImageF32};

use crate::Orientation;
use crate::gpu::GpuContext;
use crate::ops::*;
use crate::params::EditParams;
use crate::pipeline::{Pipeline, RenderRequest};
use crate::tone::{tone_curve, tone_uniform};

fn gpu() -> Option<GpuContext> {
    let g = GpuContext::headless();
    if g.is_none() {
        eprintln!("no Vulkan adapter; skipping GPU test");
    }
    g
}

fn camera(as_shot_mul: Option<[f32; 3]>) -> CameraColor {
    CameraColor {
        xyz_to_camera: SRGB.xyz_to_rgb().to_f32(),
        d65_mul: [1.0; 3],
        as_shot_mul,
    }
}

fn raw(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3], cam: CameraColor) -> DecodedImage {
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.extend(f(x, y));
        }
    }
    DecodedImage::SceneLinear {
        rgb: ImageF32 {
            width: w,
            height: h,
            channels: 3,
            data,
        },
        camera: cam,
    }
}

fn linear_profile() -> EditParams {
    let mut p = EditParams::default();
    p.set::<Profile>(ProfileParams {
        name: ProfileName::Linear,
        treatment: Treatment::Color,
    });
    p
}

fn render(pl: &mut Pipeline, p: EditParams, edge: u32) -> (u32, u32, Vec<u8>) {
    pl.render(&RenderRequest::new(p, edge)).unwrap();
    pl.read_output_rgba8().unwrap()
}

fn px(out: &(u32, u32, Vec<u8>), x: u32, y: u32) -> [f32; 3] {
    let i = ((y * out.0 + x) * 4) as usize;
    [out.2[i] as f32, out.2[i + 1] as f32, out.2[i + 2] as f32]
}

fn enc(v: f32) -> f32 {
    srgb_oetf(v.clamp(0.0, 1.0)) * 255.0
}

fn assert_px(got: [f32; 3], want: [f32; 3], tol: f32, what: &str) {
    for c in 0..3 {
        assert!(
            (got[c] - want[c]).abs() <= tol,
            "{what}: got {got:?}, want {want:?}"
        );
    }
}

#[test]
fn linear_profile_is_an_srgb_identity_and_exposure_adds_stops() {
    let Some(g) = gpu() else { return };
    let img = raw(
        16,
        16,
        |x, _| {
            let v = x as f32 / 15.0;
            [v, v * 0.5, v * 0.25]
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let out = render(&mut pl, linear_profile(), 16);
    for x in [0u32, 5, 10, 15] {
        let v = x as f32 / 15.0;
        assert_px(
            px(&out, x, 3),
            [enc(v), enc(v * 0.5), enc(v * 0.25)],
            3.0,
            "identity",
        );
    }
    let mut p = linear_profile();
    p.set::<Exposure>(ExposureParams { ev: 1.0 });
    let out = render(&mut pl, p, 16);
    let v = 5.0 / 15.0;
    assert_px(
        px(&out, 5, 3),
        [enc(v * 2.0), enc(v), enc(v * 0.5)],
        3.0,
        "+1 EV",
    );
}

#[test]
fn as_shot_white_balance_neutralises_the_recorded_illuminant() {
    let Some(g) = gpu() else { return };
    // A neutral surface under a warm light reads (0.5, 1, 1) relative to green.
    let img = raw(8, 8, |_, _| [0.25, 0.5, 0.5], camera(Some([2.0, 1.0, 1.0])));
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let out = render(&mut pl, linear_profile(), 8);
    let c = px(&out, 4, 4);
    assert!(
        (c[0] - c[1]).abs() <= 3.0 && (c[2] - c[1]).abs() <= 3.0,
        "not neutral: {c:?}"
    );
    assert!(pl.as_shot_temp_tint().is_some());

    // Custom 6500 K/0 tint on a D65-ish white must leave a neutral neutral.
    let mut p = linear_profile();
    p.set::<WhiteBalance>(WhiteBalanceParams {
        mode: WbMode::Custom,
        temp: 6504.0,
        tint: 0.0,
    });
    let neutral = raw(8, 8, |_, _| [0.4, 0.4, 0.4], camera(None));
    let mut pl2 = Pipeline::new(&g, &neutral).unwrap();
    let c = px(&render(&mut pl2, p, 8), 4, 4);
    assert!(
        (c[0] - c[2]).abs() <= 6.0,
        "6500 K should be ~neutral: {c:?}"
    );
}

#[test]
fn standard_profile_matches_the_cpu_tone_curve() {
    let Some(g) = gpu() else { return };
    let img = raw(
        32,
        1,
        |x, _| {
            let v = 0.01 * 1.25f32.powi(x as i32);
            [v, v, v]
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let mut p = EditParams::default();
    p.set::<Tone>(ToneParams {
        contrast: 30.0,
        whites: 20.0,
        blacks: -10.0,
        ..Default::default()
    });
    let out = render(&mut pl, p.clone(), 32);
    let u = tone_uniform(&ProfileParams::default(), &p.get::<Tone>(), false);
    for x in 0..32u32 {
        let v = 0.01 * 1.25f32.powi(x as i32);
        let want = enc(tone_curve(&u, v));
        let got = px(&out, x, 0);
        assert!(
            (got[1] - want).abs() <= 4.0,
            "x={x} y={v}: got {} want {want}",
            got[1]
        );
    }
}

#[test]
fn only_the_stages_downstream_of_a_change_rerun() {
    let Some(g) = gpu() else { return };
    let img = raw(
        64,
        48,
        |x, y| [x as f32 / 64.0, y as f32 / 48.0, 0.3],
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let req = |p: EditParams| RenderRequest::new(p, 64);

    let first = pl.render(&req(EditParams::default())).unwrap().ran;
    assert!(first.geometry && first.scene && first.tone && first.display && first.output);
    let again = pl.render(&req(EditParams::default())).unwrap().ran;
    assert_eq!(
        again,
        Default::default(),
        "identical request = full cache hit"
    );

    let mut p = EditParams::default();
    p.set::<Presence>(PresenceParams {
        vibrance: 40.0,
        saturation: 0.0,
    });
    let r = pl.render(&req(p.clone())).unwrap().ran;
    assert!(
        !r.geometry && !r.scene && !r.tone && r.display && r.output,
        "{r:?}"
    );

    p.set::<Exposure>(ExposureParams { ev: 0.5 });
    let r = pl.render(&req(p.clone())).unwrap().ran;
    assert!(
        !r.geometry && r.scene && r.tone && r.display && r.output && !r.highlights_shadows,
        "{r:?}"
    );

    p.set::<Tone>(ToneParams {
        shadows: 50.0,
        ..Default::default()
    });
    let r = pl.render(&req(p.clone())).unwrap().ran;
    assert!(r.highlights_shadows && r.tone, "{r:?}");

    // Contrast is a tone-map change: the (expensive) highlights/shadows base stays cached.
    p.set::<Tone>(ToneParams {
        shadows: 50.0,
        contrast: 20.0,
        ..Default::default()
    });
    let r = pl.render(&req(p)).unwrap().ran;
    assert!(!r.highlights_shadows && !r.scene && r.tone, "{r:?}");

    let mut r2 = req(EditParams::default());
    r2.max_edge = 32;
    assert!(
        pl.render(&r2).unwrap().ran.geometry,
        "a new proxy size re-runs geometry"
    );
}

#[test]
fn shadows_lift_dark_areas_and_leave_bright_ones_alone() {
    let Some(g) = gpu() else { return };
    let img = raw(
        64,
        32,
        |x, _| {
            let v = if x < 32 { 0.02 } else { 0.9 };
            [v, v, v]
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let base = render(&mut pl, linear_profile(), 64);
    let mut p = linear_profile();
    p.set::<Tone>(ToneParams {
        shadows: 100.0,
        ..Default::default()
    });
    let lifted = render(&mut pl, p, 64);
    assert!(
        px(&lifted, 8, 16)[1] > px(&base, 8, 16)[1] + 20.0,
        "shadows should brighten the dark half"
    );
    assert!(
        (px(&lifted, 56, 16)[1] - px(&base, 56, 16)[1]).abs() <= 8.0,
        "bright half should barely move"
    );
}

#[test]
fn histogram_counts_every_pixel_once_per_channel() {
    let Some(g) = gpu() else { return };
    let img = raw(20, 10, |_, _| [0.2, 0.2, 0.2], camera(None));
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let mut req = RenderRequest::new(linear_profile(), 20);
    req.want_histogram = true;
    pl.render(&req).unwrap();
    let h = pl.histogram().unwrap();
    for ch in 0..4 {
        assert_eq!(h.bins[ch].iter().sum::<u32>(), 200);
    }
    let peak = h.bins[1]
        .iter()
        .enumerate()
        .max_by_key(|(_, v)| **v)
        .unwrap()
        .0 as f32;
    assert!((peak - enc(0.2)).abs() <= 3.0);
}

#[test]
fn exif_rotation_and_box_downscale() {
    let Some(g) = gpu() else { return };
    // 2×1: red | blue. EXIF 6 rotates it clockwise: red on top, blue below.
    let img = raw(
        2,
        1,
        |x, _| {
            if x == 0 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 0.0, 1.0]
            }
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let mut req = RenderRequest::new(linear_profile(), 16);
    req.orientation = Orientation::from_exif(6);
    pl.render(&req).unwrap();
    let out = pl.read_output_rgba8().unwrap();
    assert_eq!((out.0, out.1), (1, 2));
    assert!(px(&out, 0, 0)[0] > 200.0 && px(&out, 0, 1)[2] > 200.0);

    // A 1-px checkerboard averaged down is mid grey in *linear* light.
    let chk = raw(
        64,
        64,
        |x, y| {
            let v = ((x + y) % 2) as f32;
            [v, v, v]
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &chk).unwrap();
    let out = render(&mut pl, linear_profile(), 8);
    assert!((px(&out, 3, 3)[1] - enc(0.5)).abs() <= 3.0);
}

#[test]
fn clipping_overlay_flags_blown_and_crushed_pixels() {
    let Some(g) = gpu() else { return };
    let img = raw(
        2,
        1,
        |x, _| if x == 0 { [3.0; 3] } else { [0.0; 3] },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let mut req = RenderRequest::new(linear_profile(), 2);
    req.clip_overlay = true;
    pl.render(&req).unwrap();
    let out = pl.read_output_rgba8().unwrap();
    assert_px(px(&out, 0, 0), [255.0, 0.0, 0.0], 1.0, "blown");
    assert_px(px(&out, 1, 0), [0.0, 0.0, 255.0], 1.0, "crushed");
}

// --- M4 ops ------------------------------------------------------------------

fn gray_image(w: u32, h: u32, v: f32) -> DecodedImage {
    raw(w, h, |_, _| [v; 3], camera(None))
}

fn run(pl: &mut Pipeline, p: &EditParams, edge: u32) -> (crate::pipeline::RenderStats, Vec<u8>) {
    let stats = pl.render(&RenderRequest::new(p.clone(), edge)).unwrap();
    (stats, pl.read_output_rgba8().unwrap().2)
}

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[test]
fn tone_curve_maps_the_encoded_midpoint_and_identity_skips_the_pass() {
    let Some(g) = gpu() else { return };
    // Linear value whose sRGB encoding is 0.5.
    let lin = 0.214_041_14;
    let mut pl = Pipeline::new(&g, &gray_image(16, 16, lin)).unwrap();

    let (stats, _) = run(&mut pl, &linear_profile(), 16);
    assert!(!stats.ran.curve, "identity curve must not run");

    let mut p = linear_profile();
    p.set::<ToneCurve>(ToneCurveParams {
        rgb: vec![[0.0, 0.0], [0.5, 0.75], [1.0, 1.0]],
        ..Default::default()
    });
    let out = render(&mut pl, p, 16);
    assert_px(px(&out, 8, 8), [0.75 * 255.0; 3], 2.0, "curve midpoint");

    let mut p = linear_profile();
    p.set::<ToneCurve>(ToneCurveParams {
        red: vec![[0.0, 0.0], [0.5, 0.75], [1.0, 1.0]],
        ..Default::default()
    });
    let out = render(&mut pl, p, 16);
    let c = px(&out, 8, 8);
    // The output stage moves Rec.2020 to sRGB, so a red-only curve reads as
    // a saturated red rather than "R = 191, G = B = 127".
    assert!(c[0] > c[1] + 50.0 && c[0] > c[2] + 50.0, "{c:?}");
}

#[test]
fn hsl_desaturates_only_its_band_and_bw_mix_darkens_a_colour() {
    let Some(g) = gpu() else { return };
    let img = raw(
        16,
        16,
        |x, _| {
            if x < 8 {
                [0.6, 0.05, 0.05]
            } else {
                [0.05, 0.1, 0.6]
            }
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let base = render(&mut pl, linear_profile(), 16);
    let (red0, blue0) = (px(&base, 2, 8), px(&base, 12, 8));

    let mut p = linear_profile();
    let mut hsl = HslParams::default();
    hsl.sat[0] = -100.0;
    p.set::<Hsl>(hsl);
    let out = render(&mut pl, p, 16);
    let (red1, blue1) = (px(&out, 2, 8), px(&out, 12, 8));
    let spread = |c: [f32; 3]| c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
    assert!(spread(red1) < spread(red0) * 0.4, "{red0:?} -> {red1:?}");
    assert_px(blue1, blue0, 3.0, "blue is outside the red band");

    // B&W mix: lowering Red darkens the red patch relative to the blue one.
    let mut p = linear_profile();
    p.set::<Profile>(ProfileParams {
        name: ProfileName::Linear,
        treatment: Treatment::BlackWhite,
    });
    let flat = render(&mut pl, p.clone(), 16);
    let mut mix = BwMixParams::default();
    mix.mix[0] = -100.0;
    p.set::<BwMix>(mix);
    let mixed = render(&mut pl, p, 16);
    assert!(
        luminance(px(&mixed, 2, 8)) < luminance(px(&flat, 2, 8)) - 10.0,
        "red got darker"
    );
    assert!(
        (luminance(px(&mixed, 12, 8)) - luminance(px(&flat, 12, 8))).abs() < 4.0,
        "blue barely moved"
    );
}

#[test]
fn sharpening_overshoots_at_an_edge_and_leaves_flat_areas_alone() {
    let Some(g) = gpu() else { return };
    let img = raw(
        64,
        16,
        |x, _| if x < 32 { [0.1; 3] } else { [0.4; 3] },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let base = render(&mut pl, linear_profile(), 64);
    let mut p = linear_profile();
    p.set::<Sharpen>(SharpenParams {
        amount: 100.0,
        radius: 1.5,
        detail: 100.0,
        masking: 0.0,
    });
    let (stats, _) = run(&mut pl, &p, 64);
    assert!(stats.ran.sharpen);
    let out = pl.read_output_rgba8().unwrap();
    let out = (out.0, out.1, out.2);
    assert!(
        px(&out, 33, 8)[0] > px(&base, 33, 8)[0] + 5.0,
        "bright side overshoots"
    );
    assert!(
        px(&out, 30, 8)[0] < px(&base, 30, 8)[0] - 3.0,
        "dark side undershoots"
    );
    assert_px(px(&out, 4, 8), px(&base, 4, 8), 1.5, "flat dark area");
    assert_px(px(&out, 60, 8), px(&base, 60, 8), 1.5, "flat bright area");

    // Masking restricts sharpening to edges: a flat area stays flat either way,
    // and the edge is still sharpened.
    let mut masked = p.clone();
    masked.set::<Sharpen>(SharpenParams {
        amount: 100.0,
        radius: 1.5,
        detail: 100.0,
        masking: 100.0,
    });
    let out = render(&mut pl, masked, 64);
    assert!(px(&out, 33, 8)[0] > px(&base, 33, 8)[0] + 3.0);
}

#[test]
fn vignette_darkens_the_corners_but_not_the_centre() {
    let Some(g) = gpu() else { return };
    let mut pl = Pipeline::new(&g, &gray_image(64, 48, 0.3)).unwrap();
    let base = render(&mut pl, linear_profile(), 64);
    let mut p = linear_profile();
    p.set::<Vignette>(VignetteParams {
        amount: -100.0,
        ..Default::default()
    });
    let out = render(&mut pl, p.clone(), 64);
    assert_px(px(&out, 32, 24), px(&base, 32, 24), 2.0, "centre");
    assert!(px(&out, 0, 0)[0] < 8.0, "amount -100 blacks out the corner");
    assert!(
        px(&out, 20, 4)[0] < px(&base, 20, 4)[0],
        "the falloff is smooth"
    );

    p.set::<Vignette>(VignetteParams {
        amount: 100.0,
        ..Default::default()
    });
    let out = render(&mut pl, p, 64);
    assert!(
        px(&out, 1, 1)[0] > px(&base, 1, 1)[0] + 40.0,
        "lightened corner"
    );
}

#[test]
fn clarity_boosts_fine_contrast_around_mid_gray() {
    let Some(g) = gpu() else { return };
    let img = raw(
        128,
        128,
        |x, y| {
            if (x + y) % 2 == 0 {
                [0.15; 3]
            } else {
                [0.21; 3]
            }
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let base = render(&mut pl, linear_profile(), 128);
    let d0 = (px(&base, 64, 64)[0] - px(&base, 65, 64)[0]).abs();
    let mut p = linear_profile();
    p.set::<Clarity>(ClarityParams { clarity: 100.0 });
    let (stats, _) = run(&mut pl, &p, 128);
    assert!(stats.ran.clarity);
    let out = pl.read_output_rgba8().unwrap();
    let out = (out.0, out.1, out.2);
    let d1 = (px(&out, 64, 64)[0] - px(&out, 65, 64)[0]).abs();
    assert!(d1 > d0 * 1.3, "contrast {d0} -> {d1}");

    p.set::<Clarity>(ClarityParams { clarity: -100.0 });
    let out = render(&mut pl, p, 128);
    let d2 = (px(&out, 64, 64)[0] - px(&out, 65, 64)[0]).abs();
    assert!(d2 < d0 * 0.8, "negative clarity softens: {d0} -> {d2}");
}

#[test]
fn noise_reduction_smooths_noise_and_keeps_a_step_edge() {
    let Some(g) = gpu() else { return };
    let noise = |x: u32, y: u32| {
        let h = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
        let h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        ((h ^ (h >> 16)) & 0xffff) as f32 / 65535.0 - 0.5
    };
    let img = raw(
        96,
        32,
        |x, y| {
            let base = if x < 48 { 0.1 } else { 0.4 };
            let n = 1.0 + 0.3 * noise(x, y);
            [base * n; 3]
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let var = |out: &(u32, u32, Vec<u8>), x0: u32, x1: u32| {
        let vals: Vec<f32> = (0..32)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| px(out, x, y)[0])
            .collect();
        let m = vals.iter().sum::<f32>() / vals.len() as f32;
        vals.iter().map(|v| (v - m).powi(2)).sum::<f32>() / vals.len() as f32
    };
    let base = render(&mut pl, linear_profile(), 96);
    let mut p = linear_profile();
    p.set::<Noise>(NoiseParams {
        luma: 100.0,
        ..Default::default()
    });
    let (stats, _) = run(&mut pl, &p, 96);
    assert!(stats.ran.noise);
    let out = pl.read_output_rgba8().unwrap();
    let out = (out.0, out.1, out.2);
    assert!(
        var(&out, 8, 40) < var(&base, 8, 40) * 0.6,
        "flat area is smoother"
    );
    let (lo, hi) = (px(&out, 40, 16)[0], px(&out, 56, 16)[0]);
    assert!(hi - lo > 60.0, "the step edge survives: {lo} {hi}");
}

// --- Geometry ops ---------------------------------------------------------------

fn red_blue() -> DecodedImage {
    raw(
        2,
        1,
        |x, _| {
            if x == 0 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 0.0, 1.0]
            }
        },
        camera(None),
    )
}

#[test]
fn crop_keeps_only_the_selected_region_and_sizes_the_output() {
    let Some(g) = gpu() else { return };
    let img = raw(
        100,
        50,
        |x, _| {
            if x < 50 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 0.0, 1.0]
            }
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let mut p = linear_profile();
    p.set::<Crop>(CropParams {
        x0: 0.5,
        x1: 1.0,
        ..Default::default()
    });
    let out = render(&mut pl, p.clone(), 100);
    assert_eq!((out.0, out.1), (50, 50));
    assert!(
        px(&out, 25, 25)[2] > 240.0 && px(&out, 25, 25)[0] < 5.0,
        "all blue"
    );

    // The crop tool renders the whole canvas.
    let mut req = RenderRequest::new(p, 100);
    req.ignore_crop = true;
    pl.render(&req).unwrap();
    let out = pl.read_output_rgba8().unwrap();
    assert_eq!((out.0, out.1), (100, 50));
}

#[test]
fn quarter_turns_and_flips_match_their_exif_equivalents() {
    let Some(g) = gpu() else { return };
    let mut pl = Pipeline::new(&g, &red_blue()).unwrap();

    let mut p = linear_profile();
    p.set::<Crop>(CropParams {
        quarters: 1,
        ..Default::default()
    });
    let out = render(&mut pl, p, 16);
    assert_eq!((out.0, out.1), (1, 2));
    assert!(
        px(&out, 0, 0)[0] > 200.0 && px(&out, 0, 1)[2] > 200.0,
        "red on top"
    );

    let mut p = linear_profile();
    p.set::<Crop>(CropParams {
        flip_h: true,
        ..Default::default()
    });
    let out = render(&mut pl, p, 16);
    assert_eq!((out.0, out.1), (2, 1));
    assert!(
        px(&out, 0, 0)[2] > 200.0 && px(&out, 1, 0)[0] > 200.0,
        "blue | red"
    );

    // Composes with the file's own orientation: EXIF 6 plus a quarter turn
    // is a half turn.
    let mut p = linear_profile();
    p.set::<Crop>(CropParams {
        quarters: 1,
        ..Default::default()
    });
    let mut req = RenderRequest::new(p, 16);
    req.orientation = Orientation::from_exif(6);
    pl.render(&req).unwrap();
    let out = pl.read_output_rgba8().unwrap();
    assert_eq!((out.0, out.1), (2, 1));
    assert!(
        px(&(out.0, out.1, out.2.clone()), 0, 0)[2] > 200.0,
        "rotated 180°: blue first"
    );
}

#[test]
fn straighten_tilts_a_horizontal_line_and_blacks_out_beyond_the_image() {
    let Some(g) = gpu() else { return };
    // A bright horizontal band across the middle of a dark frame.
    let img = raw(
        300,
        200,
        |_, y| {
            if (99..102).contains(&y) {
                [0.8; 3]
            } else {
                [0.02; 3]
            }
        },
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    // Intensity-weighted centre row of the band in column `x`.
    let band_row = |out: &(u32, u32, Vec<u8>), x: u32| -> f32 {
        let (mut sum, mut wsum) = (0.0, 0.0);
        for y in 0..out.1 {
            let w = (px(out, x, y)[1] - 40.0).max(0.0);
            sum += w * y as f32;
            wsum += w;
        }
        sum / wsum.max(1e-6)
    };
    let mut p = linear_profile();
    p.set::<Crop>(CropParams {
        x0: 0.3,
        y0: 0.3,
        x1: 0.7,
        y1: 0.7,
        ..Default::default()
    });
    let flat = render(&mut pl, p.clone(), 300);
    assert_eq!((flat.0, flat.1), (120, 80));
    assert!((band_row(&flat, 5) - band_row(&flat, 114)).abs() < 0.3);

    p.set::<Straighten>(StraightenParams { angle: 10.0 });
    let tilted = render(&mut pl, p, 300);
    // Turning the image clockwise lowers its right side.
    let drop = band_row(&tilted, 114) - band_row(&tilted, 5);
    let expected = 109.0 * 10f32.to_radians().tan();
    assert!(
        (drop - expected).abs() < 3.0,
        "drop {drop}, expected ~{expected}"
    );

    // A full-frame crop of a rotated image shows black corners.
    let mut p = linear_profile();
    p.set::<Straighten>(StraightenParams { angle: 20.0 });
    let out = render(&mut pl, p, 300);
    assert_px(
        px(&out, 1, 1),
        [0.0; 3],
        1.0,
        "corner outside the rotated image",
    );
    assert!(px(&out, 150, 100)[1] > 5.0, "centre still has the picture");
}

#[test]
fn geometry_changes_rerun_the_pipeline_but_defaults_stay_pixel_identical() {
    let Some(g) = gpu() else { return };
    let img = raw(
        64,
        48,
        |x, y| [x as f32 / 63.0, y as f32 / 47.0, 0.3],
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let base = render(&mut pl, linear_profile(), 64);
    let mut p = linear_profile();
    // Present-but-identity geometry ops must not change a pixel.
    p.set::<Straighten>(StraightenParams { angle: 0.0 });
    p.set::<Crop>(CropParams {
        aspect: [3.0, 2.0],
        ..Default::default()
    });
    assert_eq!(
        p.hash(),
        linear_profile().hash(),
        "identity ops aren't stored"
    );
    let same = render(&mut pl, p, 64);
    assert_eq!(base.2, same.2);
}

#[test]
fn sixteen_bit_output_matches_eight_bit_and_space_changes_encoding() {
    use viberoom_color::icc::OutputSpace;
    let Some(g) = gpu() else { return };
    let img = raw(
        16,
        16,
        |x, y| [x as f32 / 15.0, y as f32 / 15.0, 0.3],
        camera(None),
    );
    let mut pl = Pipeline::new(&g, &img).unwrap();
    let eight = render(&mut pl, linear_profile(), 16);

    let mut req = RenderRequest::new(linear_profile(), 16);
    req.depth16 = true;
    pl.render(&req).unwrap();
    let (w, h, d16) = pl.read_output_rgba16().unwrap();
    assert_eq!((w, h), (16, 16));
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            for c in 0..3 {
                let got = d16[i + c] as f32 / 65535.0 * 255.0;
                let want = eight.2[i + c] as f32;
                // 8-bit output is dithered by up to half a step.
                assert!((got - want).abs() <= 1.6, "({x},{y},{c}): {got} vs {want}");
            }
            assert_eq!(d16[i + 3], 65535);
        }
    }

    // A saturated pixel encodes differently in ProPhoto (gamma 1.8) than sRGB.
    let mut p3 = RenderRequest::new(linear_profile(), 16);
    p3.depth16 = true;
    p3.space = OutputSpace::ProPhoto;
    pl.render(&p3).unwrap();
    let (_, _, pp) = pl.read_output_rgba16().unwrap();
    assert_ne!(pp[..3], d16[..3].to_vec()[..], "space had no effect");
    let mid = ((8 * 16 + 8) * 4) as usize;
    assert_ne!(pp[mid + 1], d16[mid + 1]);
}
