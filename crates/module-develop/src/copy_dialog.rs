//! The Copy… / Sync… group-selection dialog (plan §8.4).

use viberoom_services::engine::{EditParams, Registry, SettingsGroup};
use viberoom_shell::AppCx;

use crate::{Clipboard, DevelopModule};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Put the checked groups on the clipboard.
    Copy,
    /// Apply the checked groups to the rest of the selection.
    Sync,
}

/// Every group the registry has an op for, in panel order, with those the
/// photo actually has edits in pre-checked (all of them when it has none).
/// Per-photo groups (crop, straighten) start unchecked unless edited.
pub fn group_list(registry: &Registry, params: &EditParams) -> Vec<(SettingsGroup, bool)> {
    let mut groups: Vec<SettingsGroup> = registry.iter().map(|o| o.group()).collect();
    groups.push(SettingsGroup::LocalAdjustments);
    groups.sort();
    groups.dedup();
    let edited: Vec<SettingsGroup> = registry
        .iter()
        .filter(|o| params.ops.contains_key(o.id()))
        .map(|o| o.group())
        .collect();
    // Circles and masks belong to one photo: never pre-checked.
    groups
        .into_iter()
        .map(|g| {
            (
                g,
                !g.is_photo_specific() && (edited.is_empty() || edited.contains(&g)),
            )
        })
        .collect()
}

/// Draws the check-all/none buttons and the group checkboxes.
pub fn group_checkboxes(ui: &mut egui::Ui, groups: &mut [(SettingsGroup, bool)]) {
    ui.horizontal(|ui| {
        if ui.button("Check All").clicked() {
            groups.iter_mut().for_each(|g| g.1 = true);
        }
        if ui.button("Check None").clicked() {
            groups.iter_mut().for_each(|g| g.1 = false);
        }
    });
    ui.separator();
    for (group, on) in groups.iter_mut() {
        ui.checkbox(on, group.label());
    }
    ui.separator();
}

#[derive(Debug)]
pub struct CopyDialog {
    groups: Vec<(SettingsGroup, bool)>,
    mode: Mode,
}

impl CopyDialog {
    pub fn new(registry: &Registry, params: &EditParams, mode: Mode) -> Self {
        Self {
            groups: group_list(registry, params),
            mode,
        }
    }
}

pub fn show(ctx: &egui::Context, m: &mut DevelopModule, cx: &mut AppCx) {
    let Some(mut dialog) = m.copy_dialog.take() else {
        return;
    };
    let (title, confirm) = match dialog.mode {
        Mode::Copy => ("Copy Settings", "Copy"),
        Mode::Sync => ("Synchronize Settings", "Synchronize"),
    };
    let mut open = true;
    let mut done = false;
    let mut chosen: Option<Vec<SettingsGroup>> = None;
    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            group_checkboxes(ui, &mut dialog.groups);
            let any = dialog.groups.iter().any(|g| g.1);
            ui.horizontal(|ui| {
                if ui.add_enabled(any, egui::Button::new(confirm)).clicked() {
                    chosen = Some(dialog.groups.iter().filter(|g| g.1).map(|g| g.0).collect());
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });
        });
    if let Some(groups) = chosen {
        match dialog.mode {
            Mode::Copy => {
                if let Some(doc) = &m.doc {
                    m.clipboard = Some(Clipboard {
                        source: doc.params.clone(),
                        groups,
                    });
                }
            }
            Mode::Sync => m.sync_selected(cx, &groups),
        }
    }
    if open && !done {
        m.copy_dialog = Some(dialog);
    }
}
