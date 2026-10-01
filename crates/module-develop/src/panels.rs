//! The M4 right-panel sections (plan §8.2): Tone Curve, HSL / B&W, Detail
//! and Effects. Each edits a working copy of the params and reports what
//! changed, like the Basic panel.

use viberoom_services::engine::ops::{
    BANDS, BwMix, Hsl, MonotoneCurve, Noise, Profile, ProfileName, Sharpen, ToneCurve, Treatment,
    Vignette,
};
use viberoom_services::engine::{EditParams, Op};
use viberoom_ui::{CurveEdit, LrSlider, curve_editor};

use crate::basic::{Change, spec_slider};

#[derive(Debug, Default)]
pub struct State {
    /// 0 = RGB, 1..=3 = R, G, B.
    curve_channel: usize,
    /// 0 = Hue, 1 = Saturation, 2 = Luminance, 3 = All.
    hsl_tab: usize,
}

/// Approximate swatch colours for the eight bands.
const BAND_COLORS: [[u8; 3]; 8] = [
    [230, 60, 60],
    [235, 140, 50],
    [230, 210, 60],
    [80, 200, 90],
    [70, 200, 200],
    [80, 120, 240],
    [160, 90, 220],
    [220, 80, 200],
];

fn section(
    ui: &mut egui::Ui,
    title: &str,
    reset_ids: &[&str],
    params: &mut EditParams,
    change: &mut Option<Change>,
    body: impl FnOnce(&mut egui::Ui, &mut EditParams, &mut Option<Change>),
) {
    egui::CollapsingHeader::new(egui::RichText::new(title).heading().size(15.0))
        .default_open(false)
        .show(ui, |ui| {
            let active = reset_ids.iter().any(|id| params.ops.contains_key(*id));
            if ui
                .add_enabled(active, egui::Button::new("Reset").small())
                .on_hover_text(format!("Reset {title}"))
                .clicked()
            {
                for id in reset_ids {
                    params.ops.remove(*id);
                }
                *change = Some(Change {
                    label: format!("Reset {title}"),
                    immediate: true,
                });
            }
            body(ui, params, change);
        });
}

fn band_slider(ui: &mut egui::Ui, band: usize, value: &mut f64) -> bool {
    let before = *value as f32;
    let mut x = before;
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 12.0), egui::Sense::hover());
        let c = BAND_COLORS[band];
        ui.painter()
            .rect_filled(rect, 2.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
        ui.add(
            LrSlider::new(BANDS[band], &mut x, -100.0..=100.0)
                .step(1.0, 5.0)
                .decimals(0)
                .label_width(62.0)
                .accent(egui::Color32::from_rgb(c[0], c[1], c[2])),
        );
    });
    if x == before {
        return false;
    }
    *value = f64::from(x).round();
    true
}

pub fn show(
    ui: &mut egui::Ui,
    params: &mut EditParams,
    state: &mut State,
    zoomed_in: bool,
) -> Option<Change> {
    let mut change: Option<Change> = None;

    section(
        ui,
        "Tone Curve",
        &[ToneCurve::ID],
        params,
        &mut change,
        |ui, params, change| tone_curve(ui, params, state, change),
    );
    section(
        ui,
        "HSL / B&W",
        &[Hsl::ID, BwMix::ID],
        params,
        &mut change,
        |ui, params, change| hsl(ui, params, state, change),
    );
    section(
        ui,
        "Detail",
        &[Sharpen::ID, Noise::ID],
        params,
        &mut change,
        |ui, params, change| detail(ui, params, zoomed_in, change),
    );
    section(
        ui,
        "Effects",
        &[Vignette::ID],
        params,
        &mut change,
        |ui, params, change| {
            ui.label("Post-Crop Vignetting");
            let mut v = params.get::<Vignette>();
            let mut moved = None;
            for (spec, field) in Vignette::specs().iter().zip([
                &mut v.amount,
                &mut v.midpoint,
                &mut v.roundness,
                &mut v.feather,
                &mut v.highlights,
            ]) {
                if spec_slider(ui, spec, field) {
                    moved = Some(spec.label);
                }
            }
            if let Some(label) = moved {
                params.set::<Vignette>(v);
                *change = Some(Change {
                    label: format!("Vignette {label}"),
                    immediate: false,
                });
            }
        },
    );
    change
}

fn tone_curve(
    ui: &mut egui::Ui,
    params: &mut EditParams,
    state: &mut State,
    change: &mut Option<Change>,
) {
    let mut p = params.get::<ToneCurve>();
    let mut moved: Option<&str> = None;

    for (spec, field) in ToneCurve::specs().iter().zip([
        &mut p.highlights,
        &mut p.lights,
        &mut p.darks,
        &mut p.shadows,
    ]) {
        if spec_slider(ui, spec, field) {
            moved = Some(spec.label);
        }
    }
    ui.add_space(4.0);
    ui.label("Region split points");
    for (i, name) in ["Shadows / Darks", "Darks / Lights", "Lights / Highlights"]
        .iter()
        .enumerate()
    {
        let before = p.splits[i] as f32;
        let mut x = before;
        ui.add(
            LrSlider::new(name, &mut x, 5.0..=95.0)
                .default_value(viberoom_services::engine::ops::ToneCurve::default_split(i) as f32)
                .step(1.0, 5.0)
                .decimals(0)
                .label_width(110.0),
        );
        if x != before {
            p.splits[i] = f64::from(x).round();
            moved = Some("Split");
        }
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (i, (name, _)) in [("RGB", 0), ("R", 1), ("G", 2), ("B", 3)]
            .iter()
            .enumerate()
        {
            if ui
                .selectable_label(state.curve_channel == i, *name)
                .clicked()
            {
                state.curve_channel = i;
            }
        }
    });
    let (points, color) = match state.curve_channel {
        1 => (&mut p.red, egui::Color32::from_rgb(230, 70, 70)),
        2 => (&mut p.green, egui::Color32::from_rgb(70, 200, 90)),
        3 => (&mut p.blue, egui::Color32::from_rgb(80, 130, 240)),
        _ => (&mut p.rgb, egui::Color32::from_gray(0xe0)),
    };
    let curve = MonotoneCurve::new(points.as_slice());
    let samples: Vec<f32> = (0..=128)
        .map(|i| curve.eval(f64::from(i) / 128.0) as f32)
        .collect();
    let (_, edit) = curve_editor(
        ui,
        egui::Id::new(("tone_curve", state.curve_channel)),
        points,
        &samples,
        color,
    );
    if edit == CurveEdit::Changed {
        moved = Some("Point Curve");
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !points.is_empty(),
                egui::Button::new("Reset channel").small(),
            )
            .clicked()
        {
            points.clear();
            moved = Some("Point Curve");
        }
    });

    if let Some(label) = moved {
        params.set::<ToneCurve>(p);
        *change = Some(Change {
            label: format!("Tone Curve {label}"),
            immediate: false,
        });
    }
}

fn hsl(ui: &mut egui::Ui, params: &mut EditParams, state: &mut State, change: &mut Option<Change>) {
    let profile = params.get::<Profile>();
    let bw = profile.treatment == Treatment::BlackWhite || profile.name == ProfileName::Monochrome;

    if bw {
        ui.label("Black & White Mix");
        let mut m = params.get::<BwMix>();
        let mut moved = false;
        for band in 0..8 {
            moved |= band_slider(ui, band, &mut m.mix[band]);
        }
        if moved {
            params.set::<BwMix>(m);
            *change = Some(Change {
                label: "B&W Mix".into(),
                immediate: false,
            });
        }
        return;
    }

    ui.horizontal(|ui| {
        for (i, name) in ["Hue", "Saturation", "Luminance", "All"].iter().enumerate() {
            if ui.selectable_label(state.hsl_tab == i, *name).clicked() {
                state.hsl_tab = i;
            }
        }
    });
    let mut h = params.get::<Hsl>();
    let mut moved: Option<&str> = None;
    let groups: [(&str, usize); 3] = [("Hue", 0), ("Saturation", 1), ("Luminance", 2)];
    for (name, which) in groups {
        if state.hsl_tab != 3 && state.hsl_tab != which {
            continue;
        }
        if state.hsl_tab == 3 {
            ui.label(egui::RichText::new(name).strong());
        }
        for band in 0..8 {
            let field = match which {
                0 => &mut h.hue[band],
                1 => &mut h.sat[band],
                _ => &mut h.lum[band],
            };
            if band_slider(ui, band, field) {
                moved = Some(name);
            }
        }
    }
    if let Some(label) = moved {
        params.set::<Hsl>(h);
        *change = Some(Change {
            label: format!("HSL {label}"),
            immediate: false,
        });
    }
}

fn detail(
    ui: &mut egui::Ui,
    params: &mut EditParams,
    zoomed_in: bool,
    change: &mut Option<Change>,
) {
    ui.label(egui::RichText::new("Sharpening").strong());
    let mut s = params.get::<Sharpen>();
    let mut moved = None;
    for (spec, field) in
        Sharpen::specs()
            .iter()
            .zip([&mut s.amount, &mut s.radius, &mut s.detail, &mut s.masking])
    {
        if spec_slider(ui, spec, field) {
            moved = Some(spec.label);
        }
    }
    if let Some(label) = moved {
        params.set::<Sharpen>(s);
        *change = Some(Change {
            label: format!("Sharpen {label}"),
            immediate: false,
        });
    }

    ui.add_space(6.0);
    ui.label(egui::RichText::new("Noise Reduction").strong());
    let mut n = params.get::<Noise>();
    let mut moved = None;
    let specs = Noise::specs();
    for (i, (spec, field)) in specs
        .iter()
        .zip([
            &mut n.luma,
            &mut n.luma_detail,
            &mut n.luma_contrast,
            &mut n.color,
            &mut n.color_detail,
            &mut n.color_smoothness,
        ])
        .enumerate()
    {
        if i == 3 {
            ui.add_space(4.0);
            ui.label("Color");
        } else if i == 0 {
            ui.label("Luminance");
        }
        if spec_slider(ui, spec, field) {
            moved = Some(if i < 3 { "Luminance" } else { "Color" });
        }
    }
    if let Some(label) = moved {
        params.set::<Noise>(n);
        *change = Some(Change {
            label: format!("Noise Reduction {label}"),
            immediate: false,
        });
    }

    let active = params.ops.contains_key(Sharpen::ID) || params.ops.contains_key(Noise::ID);
    if active && !zoomed_in {
        ui.add_space(4.0);
        ui.small("Zoom to 1:1 for an accurate preview of sharpening and noise reduction.");
    }
}
