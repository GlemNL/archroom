//! The Presets panel (plan §8.1): built-in and user presets in folders;
//! click applies, the context menu updates, renames, deletes and exports.

use archroom_services::presets::{self, Preset};
use archroom_shell::AppCx;

use crate::DevelopModule;
use crate::preset_dialog::PresetDialog;

enum Act {
    Apply(usize),
    Update(usize),
    Rename(usize),
    Delete(usize),
    Export(usize),
    New,
    Import,
}

pub fn show(ui: &mut egui::Ui, m: &mut DevelopModule, cx: &mut AppCx) {
    if !m.presets_loaded {
        m.reload_presets(cx);
    }
    let has_doc = m.doc.is_some();
    let mut act: Option<Act> = None;

    ui.horizontal(|ui| {
        if ui
            .add_enabled(has_doc, egui::Button::new("+ New…"))
            .on_hover_text("Create a preset from the current settings")
            .clicked()
        {
            act = Some(Act::New);
        }
        if ui.button("Import…").clicked() {
            act = Some(Act::Import);
        }
    });

    // Folders in first-seen order: built-ins, then the user's.
    let mut folders: Vec<&str> = Vec::new();
    for p in &m.presets {
        if !folders.contains(&p.folder.as_str()) {
            folders.push(&p.folder);
        }
    }
    for folder in folders {
        egui::CollapsingHeader::new(folder)
            .default_open(true)
            .show(ui, |ui| {
                for (i, p) in m
                    .presets
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.folder == folder)
                {
                    let resp = ui.add_enabled(
                        has_doc,
                        egui::Button::selectable(false, &p.name).frame_when_inactive(false),
                    );
                    if resp.clicked() {
                        act = Some(Act::Apply(i));
                    }
                    resp.context_menu(|ui| {
                        if !p.builtin {
                            if ui.button("Update with current settings").clicked() {
                                act = Some(Act::Update(i));
                                ui.close();
                            }
                            if ui.button("Rename…").clicked() {
                                act = Some(Act::Rename(i));
                                ui.close();
                            }
                        }
                        if ui.button("Export…").clicked() {
                            act = Some(Act::Export(i));
                            ui.close();
                        }
                        if !p.builtin && ui.button("Delete").clicked() {
                            act = Some(Act::Delete(i));
                            ui.close();
                        }
                    });
                }
            });
    }

    let Some(act) = act else { return };
    perform(m, cx, act);
}

fn perform(m: &mut DevelopModule, cx: &mut AppCx, act: Act) {
    match act {
        Act::New => m.preset_dialog = PresetDialog::new_from(m),
        Act::Apply(i) => {
            if let Some(p) = m.presets.get(i).cloned() {
                m.paste(
                    cx,
                    &p.params,
                    &p.groups,
                    &format!("Preset: {}", p.name),
                    true,
                );
            }
        }
        Act::Rename(i) => {
            if let Some(Preset {
                id: Some(id), name, ..
            }) = m.presets.get(i)
            {
                m.preset_dialog = Some(PresetDialog::rename(*id, name));
            }
        }
        Act::Update(i) => {
            if let (
                Some(Preset {
                    id: Some(id),
                    groups,
                    ..
                }),
                Some(doc),
                Some(catalog),
            ) = (m.presets.get(i), &m.doc, &cx.catalog)
                && let Err(e) = presets::update_user(catalog.connection(), *id, groups, &doc.params)
            {
                tracing::error!(error = %e, "failed to update preset");
            }
            m.reload_presets(cx);
        }
        Act::Delete(i) => {
            if let (Some(Preset { id: Some(id), .. }), Some(catalog)) =
                (m.presets.get(i), &cx.catalog)
                && let Err(e) = presets::delete_user(catalog.connection(), *id)
            {
                tracing::error!(error = %e, "failed to delete preset");
            }
            m.reload_presets(cx);
        }
        Act::Export(i) => {
            let Some(p) = m.presets.get(i) else { return };
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Archroom preset", &["arpreset"])
                .set_file_name(format!("{}.arpreset", p.name))
                .save_file()
                && let Err(e) = presets::export_file(p, &path)
            {
                tracing::error!(error = %e, "failed to export preset");
            }
        }
        Act::Import => {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Archroom preset", &["arpreset"])
                .pick_file()
            else {
                return;
            };
            match presets::import_file(&path) {
                Ok(p) => {
                    if let Some(catalog) = &cx.catalog
                        && let Err(e) = presets::save_user(
                            catalog.connection(),
                            &p.name,
                            presets::USER_FOLDER,
                            &p.groups,
                            &p.params,
                        )
                    {
                        tracing::error!(error = %e, "failed to store the imported preset");
                    }
                    m.reload_presets(cx);
                }
                Err(e) => tracing::error!(error = %e, "failed to import preset"),
            }
        }
    }
}
