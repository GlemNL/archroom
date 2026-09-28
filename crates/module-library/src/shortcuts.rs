//! Keyboard shortcuts for rating/flag/label/rotate (plan §10.3, "Global"
//! scope — handled once per frame from Library's `center()`, so it applies
//! in both Grid and Loupe). Ctrl+Z/Ctrl+Y are handled globally in `app`
//! instead, since they're meaningful outside Library too in principle.

use archroom_core::ids::PhotoId;
use archroom_services::command::{RotatePhotos, SetColorLabel, SetFlag, SetRating};
use archroom_shell::AppCx;

use crate::grid::color_label_swatch;

pub const COLOR_LABELS: [&str; 5] = ["Red", "Yellow", "Green", "Blue", "Purple"];

const RATING_KEYS: [egui::Key; 6] = [
    egui::Key::Num0,
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Num5,
];
const LABEL_KEYS: [egui::Key; 4] = [
    egui::Key::Num6,
    egui::Key::Num7,
    egui::Key::Num8,
    egui::Key::Num9,
];

/// The photos a shortcut applies to: the selection if non-empty, else just
/// the active photo (matches Lightroom: acting with nothing selected but
/// something active still does something sensible).
pub fn targets(cx: &AppCx) -> Vec<PhotoId> {
    if cx.selection.selected_count() > 0 {
        cx.selection.selected().collect()
    } else {
        cx.selection.active.into_iter().collect()
    }
}

pub fn handle(ui: &egui::Ui, cx: &mut AppCx, ordered: &[PhotoId]) {
    if ui.ctx().wants_keyboard_input() {
        // A text field (Keywording, a search box) has focus — don't steal
        // its digits/letters as rating/flag/label shortcuts.
        return;
    }

    let (rating_key, label_key, flag_key, rotate, shift) = ui.input(|i| {
        let rating = RATING_KEYS
            .iter()
            .position(|&k| i.key_pressed(k))
            .map(|n| n as i32);
        let label = LABEL_KEYS.iter().position(|&k| i.key_pressed(k));
        let flag = if i.key_pressed(egui::Key::P) {
            Some(1)
        } else if i.key_pressed(egui::Key::X) {
            Some(-1)
        } else if i.key_pressed(egui::Key::U) {
            Some(0)
        } else {
            None
        };
        let rotate = if i.modifiers.command && i.key_pressed(egui::Key::OpenBracket) {
            Some(-1)
        } else if i.modifiers.command && i.key_pressed(egui::Key::CloseBracket) {
            Some(1)
        } else {
            None
        };
        (rating, label, flag, rotate, i.modifiers.shift)
    });

    let ids = targets(cx);
    if ids.is_empty()
        && rating_key.is_none()
        && label_key.is_none()
        && flag_key.is_none()
        && rotate.is_none()
    {
        return;
    }

    let mut applied = false;

    if let Some(rating) = rating_key
        && !ids.is_empty()
    {
        let _ = cx.apply_command(Box::new(SetRating::new(ids.clone(), rating)));
        applied = true;
    }
    if let Some(idx) = label_key
        && !ids.is_empty()
    {
        let name = COLOR_LABELS[idx].to_string();
        let _ = cx.apply_command(Box::new(SetColorLabel::new(ids.clone(), Some(name))));
        applied = true;
    }
    if let Some(flag) = flag_key
        && !ids.is_empty()
    {
        let _ = cx.apply_command(Box::new(SetFlag::new(ids.clone(), flag)));
        applied = true;
    }
    if let Some(delta) = rotate
        && !ids.is_empty()
    {
        let _ = cx.apply_command(Box::new(RotatePhotos::new(ids.clone(), delta)));
        applied = true;
    }

    // Shift+rating/flag applies and moves to the next photo (plan §10.3).
    // Rotate and labels don't auto-advance in Lightroom either — only
    // rating and flag do.
    if applied && shift && (rating_key.is_some() || flag_key.is_some()) {
        cx.selection.advance(ordered, 1);
    }
}

/// A small toolbar control for the color label, since only 4 of the 5
/// (Purple has no default shortcut) are keyboard-reachable.
pub fn label_picker(ui: &mut egui::Ui, cx: &mut AppCx) {
    let ids = targets(cx);
    ui.add_enabled_ui(!ids.is_empty(), |ui| {
        ui.menu_button("Label", |ui| {
            for name in COLOR_LABELS {
                let color = color_label_swatch(name).unwrap_or(egui::Color32::GRAY);
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::Vec2::splat(10.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, egui::CornerRadius::same(1), color);
                    if ui.button(name).clicked() {
                        let _ = cx.apply_command(Box::new(SetColorLabel::new(
                            ids.clone(),
                            Some(name.to_string()),
                        )));
                        ui.close();
                    }
                });
            }
            if ui.button("None").clicked() {
                let _ = cx.apply_command(Box::new(SetColorLabel::new(ids.clone(), None)));
                ui.close();
            }
        });
    });
}
