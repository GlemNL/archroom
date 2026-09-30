//! Neutral dark theme (plan §10.2): low-saturation chrome, one accent color.
//! Applied once at startup; panels never hand-roll their own colors.

use egui::{Color32, Context, CornerRadius, Stroke, Visuals};

/// The single accent color used for selection, focus rings and the active
/// module tab.
pub const ACCENT: Color32 = Color32::from_rgb(0x4d, 0x8d, 0xff);

pub fn apply(ctx: &Context) {
    let mut visuals = Visuals::dark();

    visuals.override_text_color = Some(Color32::from_gray(0xe6));
    visuals.window_fill = Color32::from_gray(0x1e);
    visuals.panel_fill = Color32::from_gray(0x1e);
    visuals.faint_bg_color = Color32::from_gray(0x24);
    visuals.extreme_bg_color = Color32::from_gray(0x14);
    visuals.code_bg_color = Color32::from_gray(0x14);

    visuals.widgets.noninteractive.bg_fill = Color32::from_gray(0x24);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_gray(0xb0));

    visuals.widgets.inactive.bg_fill = Color32::from_gray(0x2c);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_gray(0xc8));

    visuals.widgets.hovered.bg_fill = Color32::from_gray(0x38);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::from_gray(0xf0));
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);

    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);

    visuals.selection.bg_fill = ACCENT.linear_multiply(0.5);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    let radius = CornerRadius::same(3);
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.corner_radius = radius;

    ctx.set_visuals(visuals);
}

/// The image-canvas backdrop, a user preference (plan §10.2: "it matters
/// when judging tonality").
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanvasBackground {
    Black,
    DarkGray,
    MediumGray,
    White,
}

impl CanvasBackground {
    pub fn color(self) -> Color32 {
        match self {
            Self::Black => Color32::BLACK,
            Self::DarkGray => Color32::from_gray(0x20),
            Self::MediumGray => Color32::from_gray(0x80),
            Self::White => Color32::WHITE,
        }
    }
}
