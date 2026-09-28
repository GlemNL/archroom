//! The Loupe view (plan §7.3: fit/1:1, pan, previous/next). Shows the L2
//! preview (2048px) — there's no L3 1:1-original cache until M5, so "1:1"
//! here means 100% of the L2 preview, not the original pixels; that's
//! honest about what M1 has, not a rounding error.
//!
//! The L2 preview generates lazily on first view of a photo, synchronously
//! on the UI thread (the same accepted M0-spike tradeoff, plan roadmap
//! note in `crates/io/src/libraw_spike.rs`) — a raw with no cached L2 can
//! visibly stutter for about the time a full decode takes (~1s per the M0
//! benchmark) before its first paint. A background-job-based prefetch is a
//! fast-follow, not required for M1 Phase C.

use archroom_core::ids::PhotoId;
use archroom_services::repo::{FolderRow, PhotoSummary};
use archroom_shell::AppCx;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zoom {
    #[default]
    Fit,
    OneToOne,
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
) {
    let ordered: Vec<PhotoId> = photos.iter().map(|p| p.photo_id).collect();
    handle_prev_next(ui, cx, &ordered);

    let Some(active) = cx.selection.active else {
        ui.centered_and_justified(|ui| ui.label("Select a photo to view."));
        return;
    };
    let Some(photo) = photos.iter().find(|p| p.photo_id == active) else {
        ui.centered_and_justified(|ui| ui.label("Select a photo to view."));
        return;
    };
    let Some(path) = photo_absolute_path(folders, photo) else {
        ui.centered_and_justified(|ui| ui.label("This photo's file location is unknown."));
        return;
    };

    let preview_path = cx.previews.as_ref().and_then(|previews| {
        match archroom_services::preview::ensure_cached(
            previews,
            &path,
            photo.photo_id,
            archroom_services::LEVEL_L2,
            archroom_services::L2_BUDGET_PX,
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
        return;
    };

    let uri = format!("file://{}", preview_path.display());
    let angle = crate::grid::orientation_angle(photo);
    let odd_turns = photo.user_orientation % 2 != 0;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| match zoom {
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
                    ui.add(image);
                });
            }
            Zoom::OneToOne => {
                let mut image = egui::Image::from_uri(uri.clone());
                if angle != 0.0 {
                    image = image.rotate(angle, egui::Vec2::splat(0.5));
                }
                ui.add(image);
            }
        });
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
