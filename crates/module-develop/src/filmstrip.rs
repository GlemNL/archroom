//! The Develop filmstrip (plan §7.3, shared with Library): the photos the
//! Library is listing, on the shared selection.

use archroom_core::ids::PhotoId;
use archroom_shell::AppCx;

const THUMB: f32 = 64.0;

pub fn show(ui: &mut egui::Ui, cx: &mut AppCx) {
    let photos: Vec<PhotoId> = cx.selection.visible.clone();
    if photos.is_empty() {
        ui.centered_and_justified(|ui| ui.weak("Open the Library first to fill the filmstrip"));
        return;
    }
    // Scroll to the active photo only when it changes, so the user can
    // still scroll the strip freely.
    let key = egui::Id::new("develop_filmstrip_active");
    let last: Option<i64> = ui.data(|d| d.get_temp(key));
    let now = cx.selection.active.map(PhotoId::get);
    let scroll_to_active = now != last;
    archroom_shell::wheel_scrolls_horizontally(ui);
    egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for &id in &photos {
                    thumb(ui, cx, id, &photos, scroll_to_active);
                }
            });
        });
    ui.data_mut(|d| match now {
        Some(v) => d.insert_temp(key, v),
        None => {
            d.remove_temp::<i64>(key);
        }
    });
}

fn thumb(
    ui: &mut egui::Ui,
    cx: &mut AppCx,
    id: PhotoId,
    ordered: &[PhotoId],
    scroll_to_active: bool,
) {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(THUMB), egui::Sense::click());
    let active = cx.selection.active == Some(id);
    if active && scroll_to_active {
        ui.scroll_to_rect(rect, None);
    }
    if !ui.is_rect_visible(rect) {
        return;
    }
    let selected = cx.selection.is_selected(id);
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(2),
            archroom_ui::ACCENT.linear_multiply(0.25),
        );
    }
    let inner = rect.shrink(2.0);
    let path = cx
        .previews
        .as_ref()
        .and_then(|p| p.lookup(id, archroom_services::LEVEL_L1).ok().flatten());
    match path {
        Some(path) => {
            ui.put(
                inner,
                egui::Image::from_uri(format!("file://{}", path.display()))
                    .fit_to_exact_size(inner.size())
                    .maintain_aspect_ratio(true),
            );
        }
        None => {
            painter.rect_filled(
                inner,
                egui::CornerRadius::same(2),
                egui::Color32::from_gray(0x2c),
            );
        }
    }
    if active {
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(2),
            egui::Stroke::new(2.0_f32, archroom_ui::ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    if resp.clicked() {
        let mods = ui.input(|i| i.modifiers);
        if mods.command {
            cx.selection.toggle(id);
        } else if mods.shift {
            cx.selection.select_range(ordered, id);
        } else {
            cx.selection.select_single(id);
        }
    }
}
