//! Catalog and Folders panels (plan §7.2). Folders is a flat indented list
//! rather than a real collapsible tree widget — good enough to browse and
//! to confirm import landed in the right place; collapse/expand state and
//! "Synchronize Folder" (rescanning disk for new/missing files, not just
//! re-reading the catalog) are Later/M2 polish.

use std::collections::HashMap;

use viberoom_core::ids::FolderId;
use viberoom_services::repo::FolderRow;
use viberoom_shell::{AppCx, LibrarySource};

use crate::photos::LibraryData;

pub fn catalog_panel(ui: &mut egui::Ui, cx: &mut AppCx, data: &LibraryData) {
    ui.heading("Catalog");
    ui.add_space(4.0);

    let selected_all = cx.selection.source == LibrarySource::AllPhotographs;
    if ui
        .selectable_label(
            selected_all,
            format!("All Photographs ({})", data.total_photo_count),
        )
        .clicked()
    {
        cx.selection.source = LibrarySource::AllPhotographs;
    }

    if let Some(import_id) = data.latest_import {
        let selected = cx.selection.source == LibrarySource::Import(import_id);
        if ui.selectable_label(selected, "Previous Import").clicked() {
            cx.selection.source = LibrarySource::Import(import_id);
        }
    } else {
        ui.weak("Previous Import");
    }
}

pub fn folders_panel(ui: &mut egui::Ui, cx: &mut AppCx, data: &mut LibraryData) {
    ui.horizontal(|ui| {
        ui.heading("Folders");
        if ui
            .small_button("🔄")
            .on_hover_text("Refresh from the catalog (a full disk resync is a later feature)")
            .clicked()
        {
            data.refresh(cx);
        }
    });
    ui.add_space(4.0);

    if data.folders.is_empty() {
        ui.weak("No folders yet.");
        return;
    }

    let depths = compute_depths(&data.folders);
    egui::ScrollArea::vertical()
        .id_salt("folders_scroll")
        .max_height(ui.available_height() * 0.5)
        .show(ui, |ui| {
            for folder in &data.folders {
                let depth = depths.get(&folder.id).copied().unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 12.0);
                    let selected = cx.selection.source == LibrarySource::Folder(folder.id);
                    let label = format!("{} ({})", folder.name, folder.file_count);
                    if ui.selectable_label(selected, label).clicked() {
                        cx.selection.source = LibrarySource::Folder(folder.id);
                    }
                    if ui
                        .small_button("📂")
                        .on_hover_text("Show in File Manager")
                        .clicked()
                        && let Err(e) = open::that(&folder.path)
                    {
                        tracing::warn!(path = %folder.path, error = %e, "failed to open file manager");
                    }
                });
            }
        });
}

fn compute_depths(folders: &[FolderRow]) -> HashMap<FolderId, usize> {
    let by_id: HashMap<FolderId, &FolderRow> = folders.iter().map(|f| (f.id, f)).collect();
    folders
        .iter()
        .map(|folder| {
            let mut depth = 0;
            let mut current = folder.parent_id;
            while let Some(pid) = current {
                depth += 1;
                current = by_id.get(&pid).and_then(|f| f.parent_id);
            }
            (folder.id, depth)
        })
        .collect()
}
