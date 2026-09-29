//! Collections panel (plan §7.2): Quick Collection, regular and smart
//! collections, and sets. Membership changes go through undoable
//! `Command`s; creating/renaming/deleting a collection is direct catalog
//! housekeeping (like Lightroom, not on the undo stack).

use archroom_core::events::CatalogEvent;
use archroom_core::ids::CollectionId;
use archroom_services::collections::{self, CollectionKind, CollectionRow};
use archroom_services::command::{AddToCollection, RemoveFromCollection};
use archroom_services::criteria::SmartRules;
use archroom_shell::{AppCx, LibrarySource};

use crate::filter_bar;
use crate::photos::LibraryData;
use crate::shortcuts;

/// What the "name it" row is currently creating.
#[derive(Debug, Clone)]
enum Pending {
    Create {
        kind: CollectionKind,
        parent: Option<CollectionId>,
    },
    Rename(CollectionId),
}

#[derive(Debug)]
struct SmartEditor {
    /// `None` = a new smart collection being created.
    id: Option<CollectionId>,
    name: String,
    rules: SmartRules,
}

#[derive(Debug, Default)]
pub struct CollectionsUi {
    pending: Option<Pending>,
    name: String,
    smart: Option<SmartEditor>,
}

/// `B` (plan §10.3): toggles the selection into the Quick Collection.
pub fn add_to_quick(cx: &mut AppCx) {
    let ids = shortcuts::targets(cx);
    if ids.is_empty() {
        return;
    }
    let Some(catalog) = &cx.catalog else { return };
    match collections::quick_collection(catalog.connection()) {
        Ok(quick) => {
            let _ = cx.apply_command(Box::new(AddToCollection::new(quick, ids)));
        }
        Err(e) => tracing::error!(error = %e, "quick collection unavailable"),
    }
}

pub fn panel(ui: &mut egui::Ui, cx: &mut AppCx, data: &mut LibraryData, state: &mut CollectionsUi) {
    ui.horizontal(|ui| {
        ui.heading("Collections");
        ui.menu_button("+", |ui| {
            if ui.button("New collection…").clicked() {
                state.begin(Pending::Create {
                    kind: CollectionKind::Regular,
                    parent: None,
                });
                ui.close();
            }
            if ui.button("New collection set…").clicked() {
                state.begin(Pending::Create {
                    kind: CollectionKind::Set,
                    parent: None,
                });
                ui.close();
            }
            if ui.button("New smart collection…").clicked() {
                state.smart = Some(SmartEditor {
                    id: None,
                    name: String::new(),
                    rules: SmartRules::default(),
                });
                ui.close();
            }
        });
    });
    ui.add_space(4.0);

    let rows = data.collections.clone();
    // Quick Collection first, then the tree.
    if let Some(quick) = rows.iter().find(|r| r.kind == CollectionKind::Quick) {
        row(ui, cx, state, quick, 0, &rows);
    } else {
        ui.weak("Quick Collection (0) — press B");
    }
    tree(ui, cx, state, &rows, None, 0);

    name_row(ui, cx, state);
    smart_window(ui, cx, state);
}

impl CollectionsUi {
    fn begin(&mut self, pending: Pending) {
        self.name.clear();
        if let Pending::Rename(_) = pending {
            // filled by the caller
        }
        self.pending = Some(pending);
    }
}

fn tree(
    ui: &mut egui::Ui,
    cx: &mut AppCx,
    state: &mut CollectionsUi,
    rows: &[CollectionRow],
    parent: Option<CollectionId>,
    depth: usize,
) {
    for r in rows
        .iter()
        .filter(|r| r.parent_id == parent && r.kind != CollectionKind::Quick)
    {
        row(ui, cx, state, r, depth, rows);
        if r.kind == CollectionKind::Set {
            tree(ui, cx, state, rows, Some(r.id), depth + 1);
        }
    }
}

fn row(
    ui: &mut egui::Ui,
    cx: &mut AppCx,
    state: &mut CollectionsUi,
    r: &CollectionRow,
    depth: usize,
    _all: &[CollectionRow],
) {
    let icon = match r.kind {
        CollectionKind::Set => "📁",
        CollectionKind::Smart => "⚙",
        CollectionKind::Quick => "⚡",
        CollectionKind::Regular => "▣",
    };
    let label = if r.kind == CollectionKind::Set {
        format!("{icon} {}", r.name)
    } else {
        format!("{icon} {} ({})", r.name, r.count)
    };
    let selected = cx.selection.source == LibrarySource::Collection(r.id);
    let response = ui
        .horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0);
            ui.selectable_label(selected, label)
        })
        .inner;
    if response.clicked() && r.kind != CollectionKind::Set {
        cx.selection.source = LibrarySource::Collection(r.id);
    }
    response.context_menu(|ui| {
        let targets = shortcuts::targets(cx);
        let has_members = matches!(r.kind, CollectionKind::Regular | CollectionKind::Quick);
        if has_members {
            if ui
                .add_enabled(!targets.is_empty(), egui::Button::new("Add selection"))
                .clicked()
            {
                let _ = cx.apply_command(Box::new(AddToCollection::new(r.id, targets.clone())));
                ui.close();
            }
            if ui
                .add_enabled(!targets.is_empty(), egui::Button::new("Remove selection"))
                .clicked()
            {
                let _ = cx.apply_command(Box::new(RemoveFromCollection::new(r.id, targets)));
                ui.close();
            }
        }
        if r.kind == CollectionKind::Set {
            if ui.button("New collection inside…").clicked() {
                state.begin(Pending::Create {
                    kind: CollectionKind::Regular,
                    parent: Some(r.id),
                });
                ui.close();
            }
            if ui.button("New set inside…").clicked() {
                state.begin(Pending::Create {
                    kind: CollectionKind::Set,
                    parent: Some(r.id),
                });
                ui.close();
            }
        }
        if r.kind == CollectionKind::Smart && ui.button("Edit rules…").clicked() {
            state.smart = Some(SmartEditor {
                id: Some(r.id),
                name: r.name.clone(),
                rules: r.rules.clone().unwrap_or_default(),
            });
            ui.close();
        }
        if r.kind != CollectionKind::Quick {
            if ui.button("Rename…").clicked() {
                state.begin(Pending::Rename(r.id));
                state.name = r.name.clone();
                ui.close();
            }
            if ui.button("Delete").clicked() {
                let result = cx
                    .catalog
                    .as_ref()
                    .map(|c| collections::delete_collection(c.connection(), r.id));
                finish(cx, result, r.id);
                if cx.selection.source == LibrarySource::Collection(r.id) {
                    cx.selection.source = LibrarySource::AllPhotographs;
                }
                ui.close();
            }
        }
    });
}

/// Publishes `CollectionsChanged` after a direct catalog mutation, or logs
/// its failure.
fn finish<E: std::fmt::Display>(cx: &mut AppCx, result: Option<Result<(), E>>, id: CollectionId) {
    match result {
        Some(Ok(())) => cx
            .events
            .publish(CatalogEvent::CollectionsChanged { ids: vec![id] }),
        Some(Err(e)) => tracing::error!(error = %e, "collection change failed"),
        None => {}
    }
}

fn name_row(ui: &mut egui::Ui, cx: &mut AppCx, state: &mut CollectionsUi) {
    let Some(pending) = state.pending.clone() else {
        return;
    };
    let mut done = false;
    ui.horizontal(|ui| {
        let edit = ui.add(egui::TextEdit::singleline(&mut state.name).hint_text("Name"));
        edit.request_focus();
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("OK").clicked() || enter) && !state.name.trim().is_empty() {
            let name = state.name.trim().to_string();
            match pending {
                Pending::Create { kind, parent } => {
                    let id = cx.catalog.as_ref().and_then(|c| {
                        collections::create_collection(c.connection(), kind, &name, parent, None)
                            .ok()
                    });
                    if let Some(id) = id {
                        cx.events
                            .publish(CatalogEvent::CollectionsChanged { ids: vec![id] });
                    }
                }
                Pending::Rename(id) => {
                    let result = cx
                        .catalog
                        .as_ref()
                        .map(|c| collections::rename_collection(c.connection(), id, &name));
                    finish(cx, result, id);
                }
            }
            done = true;
        }
        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            done = true;
        }
    });
    if done {
        state.pending = None;
    }
}

fn smart_window(ui: &mut egui::Ui, cx: &mut AppCx, state: &mut CollectionsUi) {
    let Some(editor) = &mut state.smart else {
        return;
    };
    let mut open = true;
    let mut save = false;
    egui::Window::new(if editor.id.is_some() {
        "Edit Smart Collection"
    } else {
        "New Smart Collection"
    })
    .open(&mut open)
    .collapsible(false)
    .show(ui.ctx(), |ui| {
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut editor.name);
        });
        ui.separator();
        filter_bar::rules_editor(ui, &mut editor.rules);
        ui.separator();
        save = ui
            .add_enabled(!editor.name.trim().is_empty(), egui::Button::new("Save"))
            .clicked();
    });

    if save {
        let name = editor.name.trim().to_string();
        let rules = editor.rules.clone();
        let id = editor.id;
        let saved = cx.catalog.as_ref().and_then(|c| {
            let conn = c.connection();
            match id {
                Some(id) => collections::rename_collection(conn, id, &name)
                    .and_then(|()| collections::set_smart_rules(conn, id, &rules))
                    .ok()
                    .map(|()| id),
                None => collections::create_collection(
                    conn,
                    CollectionKind::Smart,
                    &name,
                    None,
                    Some(&rules),
                )
                .ok(),
            }
        });
        if let Some(id) = saved {
            cx.events
                .publish(CatalogEvent::CollectionsChanged { ids: vec![id] });
            cx.selection.source = LibrarySource::Collection(id);
        }
        state.smart = None;
    } else if !open {
        state.smart = None;
    }
}
