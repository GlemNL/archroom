//! The Library grid (plan §7.3): virtualized via `egui::ScrollArea::show_rows`
//! (only visible rows are laid out, so this scales to the 50k+-photo goal
//! without a hand-rolled viewport-culling scheme), a thumbnail-size slider,
//! a click/shift/ctrl selection model shared with the filmstrip and Loupe,
//! and rating/flag/label/missing badges — all wired even though nothing
//! sets rating/flag/label yet (that's M2); the point is the Grid doesn't
//! need to change when M2 lands.

use egui::{Color32, CornerRadius, Sense, Stroke, Vec2};
use viberoom_core::ids::PhotoId;
use viberoom_services::repo::PhotoSummary;
use viberoom_shell::AppCx;

const LABEL_HEIGHT: f32 = 20.0;
const CELL_SPACING: f32 = 6.0;
const CELL_PADDING: f32 = 4.0;

/// Returns the photo that was double-clicked, if any — the caller (which
/// owns the Grid/Loupe view state) decides what that means (open Loupe).
pub fn show(
    ui: &mut egui::Ui,
    cx: &mut AppCx,
    photos: &[PhotoSummary],
    thumbnail_size: f32,
) -> Option<PhotoId> {
    let ordered = photos.iter().map(|p| p.photo_id).collect::<Vec<_>>();
    let cell_size = thumbnail_size + CELL_PADDING * 2.0;
    let row_height = cell_size + LABEL_HEIGHT;

    let available_width = ui.available_width();
    let columns =
        (((available_width + CELL_SPACING) / (cell_size + CELL_SPACING)).floor() as usize).max(1);
    let total_rows = photos.len().div_ceil(columns);

    handle_select_all_shortcut(ui, cx, &ordered);

    let mut double_clicked = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_height, total_rows, |ui, row_range| {
            for row in row_range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = CELL_SPACING;
                    for col in 0..columns {
                        let Some(photo) = photos.get(row * columns + col) else {
                            break;
                        };
                        if show_cell(ui, cx, photo, &ordered, thumbnail_size) {
                            double_clicked = Some(photo.photo_id);
                        }
                    }
                });
            }
        });
    double_clicked
}

fn handle_select_all_shortcut(ui: &egui::Ui, cx: &mut AppCx, ordered: &[PhotoId]) {
    ui.input(|i| {
        let cmd = i.modifiers.command;
        if cmd && i.key_pressed(egui::Key::A) {
            cx.selection.select_all(ordered);
        } else if cmd && i.key_pressed(egui::Key::D) {
            cx.selection.clear();
        }
    });
}

/// Returns `true` if this cell was just double-clicked.
fn show_cell(
    ui: &mut egui::Ui,
    cx: &mut AppCx,
    photo: &PhotoSummary,
    ordered: &[PhotoId],
    size: f32,
) -> bool {
    let cell_size = Vec2::new(
        size + CELL_PADDING * 2.0,
        size + CELL_PADDING * 2.0 + LABEL_HEIGHT,
    );
    let (rect, response) = ui.allocate_exact_size(cell_size, Sense::click());

    if !ui.is_rect_visible(rect) {
        return false;
    }

    let selected = cx.selection.is_selected(photo.photo_id);
    let is_active = cx.selection.active == Some(photo.photo_id);

    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(
            rect,
            CornerRadius::same(3),
            viberoom_ui::ACCENT.linear_multiply(0.25),
        );
    }
    if is_active {
        painter.rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(2.0_f32, viberoom_ui::ACCENT),
            egui::StrokeKind::Inside,
        );
    }

    let image_rect =
        egui::Rect::from_min_size(rect.min + Vec2::splat(CELL_PADDING), Vec2::splat(size));

    let preview_path = cx.previews.as_ref().and_then(|p| {
        p.lookup(photo.photo_id, viberoom_services::LEVEL_L1)
            .ok()
            .flatten()
    });

    match preview_path {
        Some(path) => {
            let uri = format!("file://{}", path.display());
            let mut image = egui::Image::from_uri(uri)
                .fit_to_exact_size(Vec2::splat(size))
                .maintain_aspect_ratio(true);
            let angle = orientation_angle(photo);
            if angle != 0.0 {
                // Display-time quarter turn (plan §6.8): the stored
                // preview pixels stay un-rotated; only the drawn quad
                // turns. The square cell keeps the rotated content inside.
                image = image.rotate(angle, Vec2::splat(0.5));
            }
            ui.put(image_rect, image);
        }
        None => {
            painter.rect_filled(image_rect, CornerRadius::same(2), Color32::from_gray(0x2c));
            painter.text(
                image_rect.center(),
                egui::Align2::CENTER_CENTER,
                "…",
                egui::FontId::proportional(18.0),
                Color32::from_gray(0x80),
            );
        }
    }

    // Badges: rating (dots), flag, missing. All read `0`/`None` for every
    // M1 photo today (rating/flag editing is M2) — this just proves the
    // rendering doesn't need to change once M2 starts writing them.
    let badge_y = rect.min.y + CELL_PADDING;
    if photo.flag != 0 {
        let color = if photo.flag > 0 {
            Color32::from_rgb(0x4c, 0xaf, 0x50)
        } else {
            Color32::from_rgb(0xe5, 0x39, 0x35)
        };
        painter.circle_filled(
            egui::pos2(rect.min.x + CELL_PADDING + 5.0, badge_y + 5.0),
            4.0,
            color,
        );
    }
    if photo.missing {
        painter.text(
            egui::pos2(rect.max.x - CELL_PADDING, badge_y),
            egui::Align2::RIGHT_TOP,
            "⚠",
            egui::FontId::proportional(14.0),
            Color32::from_rgb(0xe5, 0x39, 0x35),
        );
    }
    if photo.rating > 0 {
        let stars = "★".repeat(photo.rating.clamp(0, 5) as usize);
        painter.text(
            egui::pos2(
                rect.min.x + CELL_PADDING,
                rect.max.y - LABEL_HEIGHT - CELL_PADDING,
            ),
            egui::Align2::LEFT_BOTTOM,
            stars,
            egui::FontId::proportional(11.0),
            Color32::from_rgb(0xff, 0xc1, 0x07),
        );
    }
    if let Some(color) = photo.color_label.as_deref().and_then(color_label_swatch) {
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(
                    rect.max.x - 14.0,
                    rect.max.y - LABEL_HEIGHT - CELL_PADDING - 6.0,
                ),
                Vec2::new(10.0, 6.0),
            ),
            CornerRadius::same(1),
            color,
        );
    }
    if photo.user_orientation != 0 {
        painter.text(
            egui::pos2(rect.min.x + CELL_PADDING, badge_y),
            egui::Align2::LEFT_TOP,
            "⟳",
            egui::FontId::proportional(13.0),
            Color32::from_gray(0xd0),
        );
    }

    if let Some(name) = &photo.copy_name {
        painter.text(
            egui::pos2(
                rect.min.x + CELL_PADDING,
                rect.max.y - LABEL_HEIGHT - CELL_PADDING - 14.0,
            ),
            egui::Align2::LEFT_BOTTOM,
            format!("⧉ {name}"),
            egui::FontId::proportional(10.0),
            Color32::from_gray(0xd0),
        );
    }
    if photo.edited {
        painter.text(
            egui::pos2(rect.max.x - CELL_PADDING, rect.min.y + CELL_PADDING + 16.0),
            egui::Align2::RIGHT_TOP,
            "✎",
            egui::FontId::proportional(13.0),
            Color32::from_gray(0xd0),
        );
    }

    // Filename label.
    ui.painter().text(
        egui::pos2(rect.center().x, rect.max.y - LABEL_HEIGHT / 2.0),
        egui::Align2::CENTER_CENTER,
        truncate_filename(&photo.filename, 22),
        egui::FontId::proportional(11.0),
        ui.visuals().text_color(),
    );

    if response.clicked() {
        let modifiers = ui.input(|i| i.modifiers);
        if modifiers.shift {
            cx.selection.select_range(ordered, photo.photo_id);
        } else if modifiers.command {
            cx.selection.toggle(photo.photo_id);
        } else {
            cx.selection.select_single(photo.photo_id);
        }
    }
    let double_clicked = response.double_clicked();

    response.on_hover_text(&photo.filename);
    double_clicked
}

/// Adobe's five label names (plan D5) to a swatch color for the badge.
pub fn color_label_swatch(label: &str) -> Option<Color32> {
    match label {
        "Red" => Some(Color32::from_rgb(0xe5, 0x39, 0x35)),
        "Yellow" => Some(Color32::from_rgb(0xfd, 0xd8, 0x35)),
        "Green" => Some(Color32::from_rgb(0x4c, 0xaf, 0x50)),
        "Blue" => Some(Color32::from_rgb(0x42, 0x85, 0xf4)),
        "Purple" => Some(Color32::from_rgb(0x9c, 0x27, 0xb0)),
        _ => None,
    }
}

/// The display-time quarter-turn angle (radians, clockwise) for a photo's
/// user rotation (plan §6.8 / §7.4): `user_orientation` is 0..=3
/// clockwise quarter turns, stored non-destructively.
pub fn orientation_angle(photo: &PhotoSummary) -> f32 {
    photo.user_orientation as f32 * std::f32::consts::FRAC_PI_2
}

fn truncate_filename(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        name.to_string()
    } else {
        let head: String = name.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}
