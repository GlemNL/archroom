//! Small input helpers shared by Library and Develop.

use viberoom_core::settings::ClickZoom;

use crate::AppCx;

/// Lets the mouse wheel scroll a horizontal-only `ScrollArea` (the
/// filmstrip) while the pointer is over `ui`: egui ignores the wheel's
/// vertical delta there, so it is folded into the horizontal one. Call it
/// before showing the area.
pub fn wheel_scrolls_horizontally(ui: &egui::Ui) {
    if !ui.rect_contains_pointer(ui.max_rect()) {
        return;
    }
    ui.input_mut(|i| {
        let d = i.smooth_scroll_delta;
        if d.y != 0.0 {
            i.smooth_scroll_delta = egui::vec2(d.x + d.y, 0.0);
        }
    });
}

/// The toolbar picker for what a click on a full-screen photo zooms to.
pub fn click_zoom_picker(ui: &mut egui::Ui, cx: &mut AppCx) {
    let before = cx.settings.click_zoom;
    let name = |z: ClickZoom| match z {
        ClickZoom::Off => "Off",
        ClickZoom::OneToOne => "1:1",
        ClickZoom::TwoToOne => "2:1",
    };
    ui.label("Click zoom:");
    egui::ComboBox::from_id_salt("click_zoom")
        .selected_text(name(before))
        .width(60.0)
        .show_ui(ui, |ui| {
            for z in [ClickZoom::TwoToOne, ClickZoom::OneToOne, ClickZoom::Off] {
                ui.selectable_value(&mut cx.settings.click_zoom, z, name(z));
            }
        })
        .response
        .on_hover_text("What a left-click on a full-screen photo toggles to from Fit");
    if cx.settings.click_zoom != before
        && let Err(e) = cx.settings.save()
    {
        tracing::error!(error = %e, "failed to save settings");
    }
}
