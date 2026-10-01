//! The Delete/Backspace confirmation. Two actions, both
//! recoverable: *Remove from Catalog* only forgets the photos (the files stay
//! where they are) and *Move to Trash* sends the originals to the system
//! trash. Undo (Ctrl+Z) reverses either. There is no permanent delete here,
//! and Cancel is the focused default so a stray Enter does nothing.

use viberoom_core::ids::PhotoId;
use viberoom_services::command::{Removal, RemovePhotos};
use viberoom_services::library::{SystemTrash, TrashPhotos, TrashPlan, plan_trash};
use viberoom_shell::AppCx;

#[derive(Debug)]
pub struct RemoveDialog {
    ids: Vec<PhotoId>,
    plan: TrashPlan,
    error: Option<String>,
    pub should_close: bool,
}

impl RemoveDialog {
    /// `None` when nothing is targeted or no catalog is open.
    pub fn open(cx: &AppCx, ids: Vec<PhotoId>) -> Option<Self> {
        if ids.is_empty() {
            return None;
        }
        let catalog = cx.catalog.as_ref()?;
        let (plan, error) = match plan_trash(catalog.connection(), &ids) {
            Ok(plan) => (plan, None),
            Err(e) => (
                TrashPlan {
                    photos: ids.clone(),
                    ..TrashPlan::default()
                },
                Some(format!("Could not work out what Trash would do: {e}")),
            ),
        };
        Some(Self {
            ids,
            plan,
            error,
            should_close: false,
        })
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        let n = self.ids.len();
        let photos = if n == 1 {
            "this photo".to_string()
        } else {
            format!("these {n} photos")
        };
        ui.label(format!("What should happen to {photos}?"));
        ui.add_space(6.0);

        ui.strong("Remove from Catalog");
        ui.label("The photo disappears from Viberoom. The file stays where it is on disk, and you can undo this.");
        ui.add_space(6.0);

        ui.strong("Move to Trash");
        let files = self
            .plan
            .files
            .iter()
            .filter(|p| p.extension().is_none_or(|e| !e.eq_ignore_ascii_case("xmp")))
            .count();
        let sidecars = self.plan.files.len() - files;
        let mut line = format!(
            "{files} file{} (and {sidecars} sidecar{}) move to your system trash, where you can restore them. This can be undone too.",
            if files == 1 { "" } else { "s" },
            if sidecars == 1 { "" } else { "s" },
        );
        if self.plan.kept_shared > 0 {
            line.push_str(&format!(
                " {} file{} stay{}: a virtual copy that is not selected still uses {}.",
                self.plan.kept_shared,
                if self.plan.kept_shared == 1 { "" } else { "s" },
                if self.plan.kept_shared == 1 { "s" } else { "" },
                if self.plan.kept_shared == 1 {
                    "it"
                } else {
                    "them"
                },
            ));
        }
        if self.plan.already_missing > 0 {
            line.push_str(&format!(
                " {} original{} already missing from disk.",
                self.plan.already_missing,
                if self.plan.already_missing == 1 {
                    " is"
                } else {
                    "s are"
                },
            ));
        }
        ui.label(line);

        if let Some(e) = &self.error {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::from_rgb(0xe5, 0x39, 0x35), e);
        }
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            let cancel = ui.button("Cancel");
            if !cancel.has_focus() && ui.memory(|m| m.focused().is_none()) {
                cancel.request_focus();
            }
            if cancel.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.should_close = true;
            }
            if ui.button("Remove from Catalog").clicked() {
                let ids = self.ids.clone();
                match cx.apply_command(Box::new(RemovePhotos::new(ids, Removal::Catalog))) {
                    Ok(()) => self.should_close = true,
                    Err(e) => self.error = Some(format!("Could not remove: {e}")),
                }
            }
            let trash =
                egui::Button::new("Move to Trash").fill(egui::Color32::from_rgb(0x8a, 0x2b, 0x28));
            if ui.add(trash).clicked() {
                let cmd = TrashPhotos::new(self.plan.clone(), Box::new(SystemTrash));
                match cx.apply_command(Box::new(cmd)) {
                    Ok(()) => self.should_close = true,
                    // The catalog and every file are as they were (the
                    // command rolls back), so the dialog stays for a retry.
                    Err(e) => self.error = Some(format!("Nothing was changed. {e}")),
                }
            }
        });
    }
}
