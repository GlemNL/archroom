//! The Navigator (plan §8.1): a small view of the whole image with the
//! visible region outlined; click or drag to move it, plus zoom presets.

use crate::{DevelopModule, Zoom};

pub fn show(ui: &mut egui::Ui, m: &mut DevelopModule) {
    ui.horizontal(|ui| {
        for (z, name) in [
            (Zoom::Fit, "Fit"),
            (Zoom::OneToOne, "1:1"),
            (Zoom::TwoToOne, "2:1"),
        ] {
            if ui.selectable_label(m.zoom == z, name).clicked() && m.crop_tool.is_none() {
                m.zoom = z;
                m.pan = egui::Vec2::ZERO;
            }
        }
    });
    let (Some(doc), Some(view)) = (&m.doc, m.view) else {
        return;
    };
    let Some(tex) = doc.texture else { return };
    if view.image.width() < 1.0 || view.image.height() < 1.0 {
        return;
    }

    let aspect = view.image.width() / view.image.height();
    let h = (ui.available_width() / aspect).min(150.0);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(h * aspect, h), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.image(
        tex,
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    // The part of the image the canvas shows.
    let visible = view.canvas.intersect(view.image);
    let to_nav = |p: egui::Pos2| {
        egui::pos2(
            rect.left()
                + ((p.x - view.image.left()) / view.image.width()).clamp(0.0, 1.0) * rect.width(),
            rect.top()
                + ((p.y - view.image.top()) / view.image.height()).clamp(0.0, 1.0) * rect.height(),
        )
    };
    painter.rect_stroke(
        egui::Rect::from_min_max(to_nav(visible.min), to_nav(visible.max)),
        0.0,
        egui::Stroke::new(1.5_f32, archroom_ui::ACCENT),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0x50)),
        egui::StrokeKind::Inside,
    );

    if m.zoom != Zoom::Fit
        && m.crop_tool.is_none()
        && (resp.clicked() || resp.dragged())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        // Bring the clicked image point to the middle of the canvas.
        let u = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let v = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        m.pan = egui::vec2(
            -(u - 0.5) * view.image.width(),
            -(v - 0.5) * view.image.height(),
        );
    }
}
