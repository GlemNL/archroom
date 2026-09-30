//! The Loupe view (plan §7.3: fit/1:1, pan, previous/next). Shows the L2
//! preview (2048px) — there's no L3 1:1-original cache until M5, so "1:1"
//! here means 100% of the L2 preview, not the original pixels; that's
//! honest about what M1 has, not a rounding error.
//!
//! The L2 preview generates lazily on first view of a photo, synchronously
//! on the UI thread (an accepted tradeoff) — a raw with no cached L2 can
//! visibly stutter for about the time a full decode takes (~1s per the M0
//! benchmark) before its first paint. A background-job-based prefetch is a
//! fast-follow, not required for M1 Phase C.

use viberoom_core::ids::PhotoId;
use viberoom_core::settings::ClickZoom;
use viberoom_services::repo::{FolderRow, PhotoSummary};
use viberoom_shell::AppCx;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zoom {
    #[default]
    Fit,
    OneToOne,
    TwoToOne,
}

impl Zoom {
    /// The view a click on the photo toggles to; `None` when clicking is off.
    pub fn from_click(z: ClickZoom) -> Option<Zoom> {
        match z {
            ClickZoom::Off => None,
            ClickZoom::OneToOne => Some(Zoom::OneToOne),
            ClickZoom::TwoToOne => Some(Zoom::TwoToOne),
        }
    }
}

pub fn photo_absolute_path(
    folders: &[FolderRow],
    photo: &PhotoSummary,
) -> Option<std::path::PathBuf> {
    let folder = folders.iter().find(|f| f.id == photo.folder_id)?;
    Some(std::path::Path::new(&folder.path).join(&photo.filename))
}

pub fn show(
    ui: &mut egui::Ui,
    cx: &mut AppCx,
    photos: &[PhotoSummary],
    folders: &[FolderRow],
    zoom: Zoom,
) -> Option<Zoom> {
    let ordered: Vec<PhotoId> = photos.iter().map(|p| p.photo_id).collect();
    handle_prev_next(ui, cx, &ordered);

    ui.painter()
        .rect_filled(ui.available_rect_before_wrap(), 0.0, cx.center_background());

    let Some(active) = cx.selection.active else {
        ui.centered_and_justified(|ui| ui.label("Select a photo to view."));
        return None;
    };
    let Some(photo) = photos.iter().find(|p| p.photo_id == active) else {
        ui.centered_and_justified(|ui| ui.label("Select a photo to view."));
        return None;
    };
    let Some(path) = photo_absolute_path(folders, photo) else {
        ui.centered_and_justified(|ui| ui.label("This photo's file location is unknown."));
        return None;
    };

    let preview_path = cx.previews.as_ref().and_then(|previews| {
        match viberoom_services::preview::ensure_cached(
            previews,
            &path,
            photo.photo_id,
            viberoom_services::LEVEL_L2,
            viberoom_services::l2_budget_px(),
        ) {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "loupe: preview generation failed");
                None
            }
        }
    });

    let Some(preview_path) = preview_path else {
        ui.centered_and_justified(|ui| ui.label(format!("Couldn't render {}", photo.filename)));
        return None;
    };

    let uri = format!("file://{}", preview_path.display());
    let angle = crate::grid::orientation_angle(photo);
    let odd_turns = photo.user_orientation % 2 != 0;
    // The scroll offset that keeps the clicked image point under the
    // pointer, set at a zoom-in click and applied on the next frame (a
    // direct offset: `scroll_with_delta` would animate).
    let anchor_id = egui::Id::new("loupe_zoom_anchor");
    let anchor: Option<egui::Vec2> = ui.data_mut(|d| d.remove_temp(anchor_id));
    let viewport = ui.available_rect_before_wrap();
    let visual = |r: egui::Rect| {
        if odd_turns {
            egui::Rect::from_center_size(r.center(), egui::vec2(r.height(), r.width()))
        } else {
            r
        }
    };
    let mut clicked: Option<(egui::Pos2, egui::Vec2)> = None;
    let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
    if let Some(offset) = anchor {
        area = area.scroll_offset(offset);
    }
    area.show(ui, |ui| match zoom {
        Zoom::Fit => {
            let available = ui.available_size();
            // The rotated quad has swapped dimensions, so an odd
            // quarter turn must lay the un-rotated rect out against
            // swapped bounds for the result to still fit (plan §6.8's
            // display-time transform).
            let max_size = if odd_turns {
                egui::Vec2::new(available.y, available.x)
            } else {
                available
            };
            ui.centered_and_justified(|ui| {
                let mut image = egui::Image::from_uri(uri.clone())
                    .max_size(max_size)
                    .maintain_aspect_ratio(true);
                if angle != 0.0 {
                    image = image.rotate(angle, egui::Vec2::splat(0.5));
                }
                let r = ui.add(image.sense(egui::Sense::click()));
                if r.clicked() {
                    clicked = r.interact_pointer_pos().map(|p| {
                        let v = visual(r.rect);
                        (p, (p - v.min) / v.size())
                    });
                }
            });
        }
        Zoom::OneToOne | Zoom::TwoToOne => {
            let scale = if zoom == Zoom::TwoToOne { 2.0 } else { 1.0 };
            let mut image = egui::Image::from_uri(uri.clone())
                .fit_to_original_size(scale)
                .sense(egui::Sense::click());
            if angle != 0.0 {
                image = image.rotate(angle, egui::Vec2::splat(0.5));
            }
            let r = ui.add(image);
            if r.clicked() {
                clicked = r.interact_pointer_pos().map(|p| {
                    let v = visual(r.rect);
                    (p, (p - v.min) / v.size())
                });
            }
        }
    });
    // A left-click toggles between Fit and the configured zoom.
    if let Some((pos, frac)) = clicked
        && let Some(target) = Zoom::from_click(cx.settings.click_zoom)
    {
        if zoom == Zoom::Fit {
            let scale = if target == Zoom::TwoToOne { 2.0 } else { 1.0 };
            let tex = egui::Image::from_uri(uri.clone())
                .load_for_size(ui.ctx(), viewport.size())
                .ok()
                .and_then(|poll| poll.size());
            if let Some(tex) = tex {
                // Laid-out size `a`, painted (rotated) size `v`, both centred.
                let a = tex * scale;
                let v = if odd_turns { egui::vec2(a.y, a.x) } else { a };
                let point = a / 2.0 + (frac - egui::Vec2::splat(0.5)) * v;
                let max = (a - viewport.size()).max(egui::Vec2::ZERO);
                let offset = (point - (pos - viewport.min)).clamp(egui::Vec2::ZERO, max);
                ui.data_mut(|d| d.insert_temp(anchor_id, offset));
                ui.ctx().request_repaint();
            }
        }
        return Some(if zoom == Zoom::Fit { target } else { Zoom::Fit });
    }
    None
}

fn handle_prev_next(ui: &egui::Ui, cx: &mut AppCx, ordered: &[PhotoId]) {
    ui.input(|i| {
        if i.key_pressed(egui::Key::ArrowRight) {
            step(cx, ordered, 1);
        } else if i.key_pressed(egui::Key::ArrowLeft) {
            step(cx, ordered, -1);
        }
    });
}

fn step(cx: &mut AppCx, ordered: &[PhotoId], delta: isize) {
    if ordered.is_empty() {
        return;
    }
    let Some(active) = cx.selection.active else {
        cx.selection.select_single(ordered[0]);
        return;
    };
    let Some(idx) = ordered.iter().position(|&id| id == active) else {
        return;
    };
    let new_idx = (idx as isize + delta).clamp(0, ordered.len() as isize - 1) as usize;
    cx.selection.select_single(ordered[new_idx]);
}
