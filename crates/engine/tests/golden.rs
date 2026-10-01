//! Golden-image suite (plan §13): a procedural chart is rendered through
//! the real GPU pipeline under a set of edit params and compared with the
//! blessed PNGs in `tests/golden/`. The chart is generated in code, so the
//! suite needs no raw fixtures and runs anywhere a Vulkan device (or
//! lavapipe: `VIBEROOM_ADAPTER=llvmpipe`) exists.
//!
//! Re-bless after an *intentional* rendering change: `just bless`
//! (`VIBEROOM_BLESS=1`), then review the PNG diffs in git.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use viberoom_color::cie::SRGB;
use viberoom_engine::EditParams;
use viberoom_engine::gpu::GpuContext;
use viberoom_engine::pipeline::{Pipeline, RenderRequest};
use viberoom_io::{CameraColor, DecodedImage, ImageF32};

const W: u32 = 384;
const H: u32 = 256;

/// Deterministic chart: colour patches, a 12-stop grey ramp, and a
/// dark/bright split carrying fine texture (what Highlights/Shadows act on).
fn chart() -> DecodedImage {
    let patches: [[f32; 3]; 12] = [
        [0.17, 0.10, 0.07],
        [0.60, 0.40, 0.32],
        [0.10, 0.16, 0.26],
        [0.10, 0.15, 0.06],
        [0.26, 0.24, 0.42],
        [0.10, 0.50, 0.40],
        [0.65, 0.28, 0.06],
        [0.06, 0.09, 0.36],
        [0.50, 0.08, 0.10],
        [0.09, 0.03, 0.12],
        [0.45, 0.55, 0.06],
        [0.70, 0.40, 0.03],
    ];
    let mut data = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            let px = if y < 96 {
                let p = patches[((y / 48) * 6 + x / 64) as usize % 12];
                if x % 64 < 4 || y % 48 < 4 {
                    [0.02; 3]
                } else {
                    p
                }
            } else if y < 144 {
                let stop = (x * 12 / W) as i32;
                let v = 0.9 * 0.5f32.powi(11 - stop);
                [v; 3]
            } else if y < 176 {
                let v = 0.002 + 3.0 * (x as f32 / (W - 1) as f32).powi(3);
                [v, v * 0.9, v * 0.8]
            } else {
                let base = if x < W / 2 { 0.03 } else { 0.7 };
                let tex = 1.0 + 0.15 * (((x * 7 + y * 13) % 11) as f32 / 10.0 - 0.5);
                [base * tex, base * tex * 0.95, base * tex * 0.85]
            };
            data.extend(px);
        }
    }
    DecodedImage::SceneLinear {
        rgb: ImageF32 {
            width: W,
            height: H,
            channels: 3,
            data,
        },
        camera: CameraColor {
            xyz_to_camera: SRGB.xyz_to_rgb().to_f32(),
            d65_mul: [1.0; 3],
            as_shot_mul: Some([1.15, 1.0, 0.9]),
        },
    }
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

/// (name, params JSON). Each is a real, plausible edit.
const CASES: &[(&str, &str)] = &[
    ("default", r#"{"process_version":1}"#),
    (
        "exposure_plus1",
        r#"{"process_version":1,"ops":{"exposure":{"v":1,"ev":1.0}}}"#,
    ),
    (
        "wb_custom_warm",
        r#"{"process_version":1,"ops":{"white_balance":{"v":1,"mode":"custom","temp":3800,"tint":10}}}"#,
    ),
    (
        "tone_contrast_whites",
        r#"{"process_version":1,"ops":{"tone":{"v":1,"contrast":40,"highlights":0,"shadows":0,"whites":30,"blacks":-20}}}"#,
    ),
    (
        "shadows_highlights",
        r#"{"process_version":1,"ops":{"tone":{"v":1,"contrast":0,"highlights":-70,"shadows":70,"whites":0,"blacks":0}}}"#,
    ),
    (
        "presence",
        r#"{"process_version":1,"ops":{"presence":{"v":1,"vibrance":60,"saturation":-15}}}"#,
    ),
    (
        "black_and_white",
        r#"{"process_version":1,"ops":{"profile":{"v":1,"name":"standard","treatment":"black_white"}}}"#,
    ),
    (
        "profile_neutral",
        r#"{"process_version":1,"ops":{"profile":{"v":1,"name":"neutral","treatment":"color"}}}"#,
    ),
    (
        "profile_linear",
        r#"{"process_version":1,"ops":{"profile":{"v":1,"name":"linear","treatment":"color"}}}"#,
    ),
    (
        "tone_curve",
        r#"{"process_version":1,"ops":{"tone_curve":{"v":1,"highlights":-30,"shadows":30,"rgb":[[0,0],[0.25,0.2],[0.75,0.82],[1,1]]}}}"#,
    ),
    (
        "hsl_shift",
        r#"{"process_version":1,"ops":{"hsl":{"v":1,"hue":[0,0,20,-30,0,0,0,0],"sat":[40,0,0,-40,0,50,0,0],"lum":[0,0,0,0,0,-30,0,0]}}}"#,
    ),
    (
        "bw_mix",
        r#"{"process_version":1,"ops":{"profile":{"v":1,"name":"standard","treatment":"black_white"},"bw_mix":{"v":1,"mix":[40,20,-20,-40,0,-60,0,0]}}}"#,
    ),
    (
        "clarity",
        r#"{"process_version":1,"ops":{"clarity":{"v":1,"clarity":60}}}"#,
    ),
    (
        "sharpen",
        r#"{"process_version":1,"ops":{"sharpen":{"v":1,"amount":80,"radius":1.2,"detail":30,"masking":20}}}"#,
    ),
    (
        "noise_reduction",
        r#"{"process_version":1,"ops":{"noise":{"v":1,"luma":60,"luma_detail":40,"luma_contrast":10,"color":50,"color_detail":50,"color_smoothness":50}}}"#,
    ),
    (
        "vignette",
        r#"{"process_version":1,"ops":{"vignette":{"v":1,"amount":-50,"midpoint":40,"roundness":20,"feather":60,"highlights":30}}}"#,
    ),
    (
        "red_eye",
        r#"{"process_version":1,"ops":{"red_eye":{"v":1,"spots":[{"x":0.4167,"y":0.2812,"r":0.043,"mode":"red","darken":60},{"x":0.83,"y":0.625,"r":0.03,"mode":"pet","darken":70,"catchlight":true}]}}}"#,
    ),
    (
        "local_gradient",
        r#"{"process_version":1,"ops":{},"local":[{"v":1,"id":"g1","name":"Gradient 1","enabled":true,"mask":{"kind":"linear","x":0.5,"y":0.45,"angle":5,"feather":0.25},"adjust":{"exposure":-1.2,"highlights":-30}}]}"#,
    ),
    (
        "local_brush",
        r#"{"process_version":1,"ops":{},"local":[{"v":1,"id":"z1","name":"Zone 1","enabled":true,"mask":{"kind":"brush","strokes":[{"size":0.12,"feather":0.6,"flow":1.0,"erase":false,"points":[[0.3,0.2],[0.45,0.25],[0.6,0.2]]}]},"adjust":{"exposure":0.8,"shadows":30}}]}"#,
    ),
    (
        "everything",
        r#"{"process_version":1,"ops":{"white_balance":{"v":1,"mode":"custom","temp":5200,"tint":-6},"exposure":{"v":1,"ev":0.4},"tone":{"v":1,"contrast":15,"highlights":-30,"shadows":35,"whites":10,"blacks":-8},"presence":{"v":1,"vibrance":25,"saturation":5}}}"#,
    ),
];

fn render(pipeline: &mut Pipeline, json: &str) -> Vec<u8> {
    let params = EditParams::from_json(json).expect("case params parse");
    pipeline
        .render(&RenderRequest::new(params, W))
        .expect("render");
    pipeline.read_output_rgba8().expect("readback").2
}

#[test]
fn renders_match_the_blessed_references() {
    let Some(gpu) = GpuContext::headless() else {
        eprintln!("no Vulkan adapter; skipping golden suite");
        return;
    };
    eprintln!("golden suite on {}", gpu.adapter_name);
    let mut pipeline = Pipeline::new(&gpu, &chart()).expect("pipeline");
    let bless = std::env::var_os("VIBEROOM_BLESS").is_some();
    let mut failures = Vec::new();

    for (name, json) in CASES {
        let got = render(&mut pipeline, json);
        let path = dir().join(format!("{name}.png"));
        if bless {
            std::fs::create_dir_all(dir()).expect("mkdir");
            image::save_buffer(&path, &got, W, H, image::ExtendedColorType::Rgba8).expect("write");
            continue;
        }
        let want = image::open(&path)
            .unwrap_or_else(|e| {
                panic!(
                    "missing reference {} ({e}); run `just bless`",
                    path.display()
                )
            })
            .to_rgba8();
        assert_eq!((want.width(), want.height()), (W, H), "{name}: size");
        let (mut sum, mut max, mut outliers) = (0u64, 0u32, 0u32);
        for (a, b) in got.chunks_exact(4).zip(want.as_raw().chunks_exact(4)) {
            let mut worst = 0;
            for c in 0..3 {
                let d = (a[c] as i32 - b[c] as i32).unsigned_abs();
                sum += d as u64;
                worst = worst.max(d);
            }
            max = max.max(worst);
            outliers += (worst > 16) as u32;
        }
        let mean = sum as f64 / (W * H * 3) as f64;
        // Cross-GPU tolerance: dither, filtering and f16 rounding differ by a few
        // levels, and hardware vs software bilinear can disagree along the chart's
        // hard edges — so bound the mean and the *fraction* of outlier pixels.
        let outlier_frac = outliers as f64 / (W * H) as f64;
        if mean > 1.0 || outlier_frac > 0.01 {
            failures.push(format!(
                "{name}: mean |Δ| {mean:.2}, max {max}, {:.2}% pixels off by >16",
                outlier_frac * 100.0
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "golden mismatches:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_case_is_distinct_from_the_default_render() {
    // Guards against a case silently rendering as a no-op.
    let Some(gpu) = GpuContext::headless() else {
        return;
    };
    let mut pipeline = Pipeline::new(&gpu, &chart()).expect("pipeline");
    let base = render(&mut pipeline, CASES[0].1);
    for (name, json) in &CASES[1..] {
        assert_ne!(
            render(&mut pipeline, json),
            base,
            "{name} rendered identically to default"
        );
    }
}
