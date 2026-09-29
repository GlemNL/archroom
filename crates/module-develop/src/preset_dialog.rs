//! The New Preset… and Rename dialogs (plan §8.1).

use archroom_services::engine::SettingsGroup;
use archroom_services::presets::{self, USER_FOLDER};
use archroom_shell::AppCx;

use crate::DevelopModule;
use crate::copy_dialog::{group_checkboxes, group_list};

#[derive(Debug)]
pub enum Mode {
    /// Create from the current settings, taking the checked groups.
    New(Vec<(SettingsGroup, bool)>),
    Rename(i64),
}

#[derive(Debug)]
pub struct PresetDialog {
    mode: Mode,
    name: String,
}

impl PresetDialog {
    /// A new preset from the open photo's settings. Crop, straighten and
    /// the process version are per-photo, so they start unchecked.
    pub fn new_from(m: &DevelopModule) -> Option<Self> {
        let doc = m.doc.as_ref()?;
        let mut groups = group_list(&m.registry, &doc.params);
        for (g, on) in &mut groups {
            if matches!(
                g,
                SettingsGroup::Crop | SettingsGroup::Straighten | SettingsGroup::ProcessVersion
            ) {
                *on = false;
            }
        }
        Some(Self {
            mode: Mode::New(groups),
            name: String::new(),
        })
    }

    pub fn rename(id: i64, current: &str) -> Self {
        Self {
            mode: Mode::Rename(id),
            name: current.to_string(),
        }
    }
}

pub fn show(ctx: &egui::Context, m: &mut DevelopModule, cx: &mut AppCx) {
    let Some(mut dialog) = m.preset_dialog.take() else {
        return;
    };
    let title = match dialog.mode {
        Mode::New(_) => "New Preset",
        Mode::Rename(_) => "Rename Preset",
    };
    let mut open = true;
    let mut save = false;
    let mut cancel = false;
    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                let resp = ui.text_edit_singleline(&mut dialog.name);
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    save = true;
                }
            });
            if let Mode::New(groups) = &mut dialog.mode {
                ui.add_space(4.0);
                group_checkboxes(ui, groups);
            }
            let valid = !dialog.name.trim().is_empty()
                && match &dialog.mode {
                    Mode::New(groups) => groups.iter().any(|g| g.1),
                    Mode::Rename(_) => true,
                };
            ui.horizontal(|ui| {
                if ui.add_enabled(valid, egui::Button::new("Save")).clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if save && !valid {
                save = false;
            }
        });
    if save && let Some(catalog) = &cx.catalog {
        let conn = catalog.connection();
        let name = dialog.name.trim();
        let result = match &dialog.mode {
            Mode::New(groups) => {
                let chosen: Vec<SettingsGroup> =
                    groups.iter().filter(|g| g.1).map(|g| g.0).collect();
                match &m.doc {
                    Some(doc) => presets::save_user(conn, name, USER_FOLDER, &chosen, &doc.params)
                        .map(|_| ()),
                    None => Ok(()),
                }
            }
            Mode::Rename(id) => presets::rename_user(conn, *id, name),
        };
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to save preset");
        }
        m.reload_presets(cx);
        return;
    }
    if open && !cancel && !save {
        m.preset_dialog = Some(dialog);
    }
}
