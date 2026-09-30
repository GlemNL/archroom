//! The Basic panel (plan §8.2): Treatment, Profile, White Balance, Tone and
//! Presence. Sliders come from each op's `ParamSpec`s.

use viberoom_services::engine::op::ParamSpec;
use viberoom_services::engine::ops::{
    Clarity, Exposure, Presence, Profile, ProfileName, Tone, Treatment, WbMode, WhiteBalance,
    WhiteBalanceParams,
};
use viberoom_services::engine::{EditParams, Op};
use viberoom_services::session::Analysis;
use viberoom_ui::LrSlider;

/// What the panel changed this frame. `immediate` edits (buttons, menus)
/// are written to history at once; slider drags wait for the debounce.
#[derive(Debug, Clone)]
pub struct Change {
    pub label: String,
    pub immediate: bool,
}

pub struct Info<'a> {
    pub as_shot: Option<(f64, f64)>,
    pub is_raw: bool,
    pub analysis: Option<&'a Analysis>,
}

const WB_PRESETS: [(&str, f64, f64); 6] = [
    ("Daylight", 5500.0, 10.0),
    ("Cloudy", 6500.0, 10.0),
    ("Shade", 7500.0, 10.0),
    ("Tungsten", 2850.0, 0.0),
    ("Fluorescent", 3800.0, 21.0),
    ("Flash", 5500.0, 0.0),
];

fn round_to(v: f64, precision: u8) -> f64 {
    let k = 10f64.powi(i32::from(precision));
    (v * k).round() / k
}

/// One `ParamSpec` slider; true when the value moved (drag, wheel, typed or
/// double-click reset).
pub fn spec_slider(ui: &mut egui::Ui, spec: &ParamSpec, value: &mut f64) -> bool {
    let before = *value as f32;
    let mut x = before;
    ui.add(
        LrSlider::new(spec.label, &mut x, spec.min as f32..=spec.max as f32)
            .default_value(spec.default as f32)
            .step(spec.fine as f32, spec.coarse as f32)
            .decimals(spec.precision as usize)
            .suffix(spec.unit),
    );
    if x == before {
        return false;
    }
    *value = round_to(f64::from(x), spec.precision);
    true
}

fn temp_format(x: f32) -> String {
    format!("{:.0}", 1e6 / (-x).max(1.0))
}

fn temp_parse(s: &str) -> Option<f32> {
    let k: f32 = s.trim().trim_end_matches(['K', 'k']).trim().parse().ok()?;
    Some(-1e6 / k.clamp(2000.0, 50000.0))
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong());
}

pub fn show(
    ui: &mut egui::Ui,
    params: &mut EditParams,
    info: &Info<'_>,
    eyedropper: &mut bool,
) -> Option<Change> {
    let mut change: Option<Change> = None;
    let mut mark = |label: &str, immediate: bool| {
        change = Some(Change {
            label: label.to_string(),
            immediate,
        });
    };

    // --- Treatment / Profile ---
    let mut profile = params.get::<Profile>();
    ui.horizontal(|ui| {
        ui.label("Treatment");
        for (t, name) in [(Treatment::Color, "Color"), (Treatment::BlackWhite, "B&W")] {
            if ui.selectable_label(profile.treatment == t, name).clicked() && profile.treatment != t
            {
                profile.treatment = t;
                params.set::<Profile>(profile.clone());
                mark("Treatment", true);
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("Profile");
        egui::ComboBox::from_id_salt("profile")
            .selected_text(format!("{:?}", profile.name))
            .show_ui(ui, |ui| {
                for n in [
                    ProfileName::Standard,
                    ProfileName::Neutral,
                    ProfileName::Linear,
                    ProfileName::Monochrome,
                ] {
                    if ui
                        .selectable_value(&mut profile.name, n, format!("{n:?}"))
                        .clicked()
                    {
                        params.set::<Profile>(profile.clone());
                        mark("Profile", true);
                    }
                }
            });
    });

    // --- White balance ---
    section(ui, "White Balance");
    ui.add_enabled_ui(info.is_raw, |ui| {
        let mut wb = params.get::<WhiteBalance>();
        let (temp, tint) = match (wb.mode, info.as_shot) {
            (WbMode::AsShot, Some(a)) => a,
            _ => (if wb.temp > 0.0 { wb.temp } else { 5500.0 }, wb.tint),
        };
        let current = match wb.mode {
            WbMode::AsShot => "As Shot".to_string(),
            WbMode::Custom => WB_PRESETS
                .iter()
                .find(|(_, t, n)| *t == wb.temp && *n == wb.tint)
                .map_or("Custom".to_string(), |(name, ..)| (*name).to_string()),
        };
        ui.horizontal(|ui| {
            ui.label("WB");
            egui::ComboBox::from_id_salt("wb_preset")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(false, "As Shot").clicked() {
                        params.set::<WhiteBalance>(WhiteBalanceParams::default());
                        mark("White Balance", true);
                    }
                    if ui
                        .add_enabled(
                            info.analysis.is_some(),
                            egui::Button::selectable(false, "Auto"),
                        )
                        .clicked()
                        && let Some(a) = info.analysis.and_then(Analysis::auto_wb)
                    {
                        params.set::<WhiteBalance>(a);
                        mark("Auto White Balance", true);
                    }
                    for (name, t, n) in WB_PRESETS {
                        if ui.selectable_label(false, name).clicked() {
                            params.set::<WhiteBalance>(WhiteBalanceParams {
                                mode: WbMode::Custom,
                                temp: t,
                                tint: n,
                            });
                            mark("White Balance", true);
                        }
                    }
                });
            if ui
                .selectable_label(*eyedropper, "⌖")
                .on_hover_text("White balance selector (W)")
                .clicked()
            {
                *eyedropper = !*eyedropper;
            }
        });

        let mut x = (-1e6 / temp) as f32;
        let x0 = x;
        ui.add(
            LrSlider::new("Temp", &mut x, -500.0..=-20.0)
                .default_value(info.as_shot.map_or(-1e6 / 5500.0, |a| (-1e6 / a.0) as f32))
                .step(1.0, 10.0)
                .display(temp_format, temp_parse),
        );
        let mut new_temp = temp;
        if x != x0 {
            new_temp = (f64::from(1e6 / (-x).max(1.0)))
                .round()
                .clamp(2000.0, 50000.0);
        }
        let mut new_tint = tint;
        let tint_changed = spec_slider(ui, &WhiteBalance::specs()[1], &mut new_tint);
        if x != x0 || tint_changed {
            wb.mode = WbMode::Custom;
            wb.temp = new_temp;
            wb.tint = new_tint;
            params.set::<WhiteBalance>(wb);
            mark("White Balance", false);
        }
    });

    // --- Tone ---
    section(ui, "Tone");
    ui.horizontal(|ui| {
        if ui
            .add_enabled(info.analysis.is_some(), egui::Button::new("Auto"))
            .on_hover_text("Auto Tone (Ctrl+U)")
            .clicked()
            && let Some(a) = info.analysis.and_then(|an| an.auto_tone(params))
        {
            params.set::<Exposure>(viberoom_services::engine::ops::ExposureParams {
                ev: a.exposure_ev,
            });
            params.set::<Tone>(a.tone);
            mark("Auto Tone", true);
        }
    });
    let mut exposure = params.get::<Exposure>();
    if spec_slider(ui, &Exposure::specs()[0], &mut exposure.ev) {
        params.set::<Exposure>(exposure);
        mark("Exposure", false);
    }
    let mut tone = params.get::<Tone>();
    let specs = Tone::specs();
    let mut tone_changed: Option<&str> = None;
    for (spec, field) in specs.iter().zip([
        &mut tone.contrast,
        &mut tone.highlights,
        &mut tone.shadows,
        &mut tone.whites,
        &mut tone.blacks,
    ]) {
        if spec_slider(ui, spec, field) {
            tone_changed = Some(spec.label);
        }
    }
    if let Some(label) = tone_changed {
        params.set::<Tone>(tone);
        mark(label, false);
    }

    // --- Presence ---
    section(ui, "Presence");
    let mut clarity = params.get::<Clarity>();
    if spec_slider(ui, &Clarity::specs()[0], &mut clarity.clarity) {
        params.set::<Clarity>(clarity);
        mark("Clarity", false);
    }
    let mut presence = params.get::<Presence>();
    let mut presence_changed: Option<&str> = None;
    for (spec, field) in Presence::specs()
        .iter()
        .zip([&mut presence.vibrance, &mut presence.saturation])
    {
        if spec_slider(ui, spec, field) {
            presence_changed = Some(spec.label);
        }
    }
    if let Some(label) = presence_changed {
        params.set::<Presence>(presence);
        mark(label, false);
    }

    change
}
