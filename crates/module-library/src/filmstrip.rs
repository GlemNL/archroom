//! The filmstrip (plan §7.3): a horizontal strip of thumbnails, shared
//! selection with the Grid. Not virtualized like the Grid — a single
//! scrollable row of many thousand cells is much less common to actually
//! scroll through than the Grid, so this is a fast-follow if it turns out
//! to matter, not required for M1's virtualization goal.

use archroom_services::repo::PhotoSummary;
use archroom_shell::AppCx;

const THUMB_SIZE: f32 = 64.0;

pub fn show(ui: &mut egui::Ui, cx: &mut AppCx, photos: &[PhotoSummary]) {
    if photos.is_empty() {
        ui.centered_and_justified(|ui| ui.weak("No photos"));
        return;
    }

    archroom_shell::wheel_scrolls_horizontally(ui);
    egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for photo in photos {
                    show_thumb(ui, cx, photo);
                }
            });
        });
}

fn show_thumb(ui: &mut egui::Ui, cx: &mut AppCx, photo: &PhotoSummary) {
    let (rect, response) =
        ui.allocate_exact_size(egui::Vec2::splat(THUMB_SIZE), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let selected = cx.selection.is_selected(photo.photo_id);
    let is_active = cx.selection.active == Some(photo.photo_id);
    let painter = ui.painter().clone();

    if selected {
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(2),
            archroom_ui::ACCENT.linear_multiply(0.25),
        );
    }

    let preview_path = cx.previews.as_ref().and_then(|p| {
        p.lookup(photo.photo_id, archroom_services::LEVEL_L1)
            .ok()
            .flatten()
    });
    let inner = rect.shrink(2.0);
    match preview_path {
        Some(path) => {
            let mut image = egui::Image::from_uri(format!("file://{}", path.display()))
                .fit_to_exact_size(inner.size())
                .maintain_aspect_ratio(true);
            let angle = crate::grid::orientation_angle(photo);
            if angle != 0.0 {
                image = image.rotate(angle, egui::Vec2::splat(0.5));
            }
            ui.put(inner, image);
        }
        None => {
            painter.rect_filled(
                inner,
                egui::CornerRadius::same(2),
                egui::Color32::from_gray(0x2c),
            );
        }
    }

    if is_active {
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(2),
            egui::Stroke::new(2.0, archroom_ui::ACCENT),
            egui::StrokeKind::Inside,
        );
    }

    if response.clicked() {
        let modifiers = ui.input(|i| i.modifiers);
        if modifiers.command {
            cx.selection.toggle(photo.photo_id);
        } else {
            cx.selection.select_single(photo.photo_id);
        }
    }
    response.on_hover_text(&photo.filename);
}
