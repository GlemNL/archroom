//! The Develop left panel (plan §8.1): Navigator, Presets, Snapshots and
//! History, plus the copy/paste/sync/reset actions.

use viberoom_shell::AppCx;

use crate::copy_dialog::{CopyDialog, Mode};
use crate::{DevelopModule, history, navigator, presets_panel};

pub fn show(ui: &mut egui::Ui, m: &mut DevelopModule, cx: &mut AppCx) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("develop_left")
        .show(ui, |ui| {
            actions(ui, m, cx);
            ui.add_space(6.0);

            section(ui, "Navigator", true, |ui| navigator::show(ui, m));
            section(ui, "Presets", true, |ui| presets_panel::show(ui, m, cx));
            section(ui, "Snapshots", false, |ui| snapshots(ui, m, cx));
            section(ui, "History", true, |ui| history::show(ui, m, cx));
        });
}

fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(egui::RichText::new(title).heading().size(15.0))
        .default_open(open)
        .show(ui, body);
}

fn actions(ui: &mut egui::Ui, m: &mut DevelopModule, cx: &mut AppCx) {
    let has_doc = m.doc.is_some();
    let others_selected = cx
        .selection
        .selected()
        .any(|p| Some(p) != cx.selection.active);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(has_doc, egui::Button::new("Copy…"))
            .on_hover_text("Ctrl+Shift+C")
            .clicked()
            && let Some(doc) = &m.doc
        {
            m.copy_dialog = Some(CopyDialog::new(&m.registry, &doc.params, Mode::Copy));
        }
        if ui
            .add_enabled(has_doc && m.clipboard.is_some(), egui::Button::new("Paste"))
            .on_hover_text("Ctrl+Shift+V")
            .clicked()
            && let Some(c) = m.clipboard.take()
        {
            m.paste(cx, &c.source, &c.groups, "Paste Settings", true);
            m.clipboard = Some(c);
        }
        if ui
            .add_enabled(
                has_doc && m.previous.is_some(),
                egui::Button::new("Previous"),
            )
            .on_hover_text("Paste the previous photo's settings (Ctrl+Alt+V)")
            .clicked()
        {
            m.paste_previous(cx);
        }
        if ui
            .add_enabled(has_doc && others_selected, egui::Button::new("Sync…"))
            .on_hover_text("Apply this photo's settings to the other selected photos")
            .clicked()
            && let Some(doc) = &m.doc
        {
            m.copy_dialog = Some(CopyDialog::new(&m.registry, &doc.params, Mode::Sync));
        }
        if ui
            .add_enabled(has_doc, egui::Button::new("Virtual Copy"))
            .on_hover_text("Create a virtual copy with its own settings")
            .clicked()
        {
            m.virtual_copy(cx);
        }
        if ui
            .add_enabled(has_doc, egui::Button::new("Reset"))
            .clicked()
        {
            m.reset(cx);
        }
    });
}

fn snapshots(ui: &mut egui::Ui, m: &mut DevelopModule, cx: &mut AppCx) {
    if m.doc.is_none() {
        return;
    }
    let mut add = false;
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut m.snapshot_name)
                .hint_text("Snapshot name")
                .desired_width(ui.available_width() - 32.0),
        );
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            add = true;
        }
        if ui
            .button("+")
            .on_hover_text("Create snapshot (Ctrl+N)")
            .clicked()
        {
            add = true;
        }
    });
    if add {
        let name = std::mem::take(&mut m.snapshot_name);
        m.add_snapshot(cx, &name);
    }
    let rows = m
        .doc
        .as_ref()
        .map(|d| d.snapshots.clone())
        .unwrap_or_default();
    let mut apply = None;
    let mut delete = None;
    for row in &rows {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(false, &row.name)
                .on_hover_text("Click to apply")
                .clicked()
            {
                apply = Some(row.clone());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("✕")
                    .on_hover_text("Delete snapshot")
                    .clicked()
                {
                    delete = Some(row.id);
                }
            });
        });
    }
    if let Some(row) = apply {
        m.apply_snapshot(cx, &row);
    }
    if let Some(id) = delete {
        m.delete_snapshot(cx, id);
    }
}
