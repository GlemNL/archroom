//! "Locate…" for a photo whose original is missing:
//! pick the file, see why it might not be the right one, then relink. Only the
//! catalog's idea of where the file lives changes; no file is touched.

use std::path::PathBuf;

use viberoom_core::events::{CatalogEvent, PhotoField};
use viberoom_core::ids::PhotoId;
use viberoom_services::library::{relink, relink_missing_in_folder, relink_problems};
use viberoom_shell::AppCx;

#[derive(Debug)]
pub struct LocateDialog {
    photo: PhotoId,
    filename: String,
    candidate: Option<PathBuf>,
    problems: Vec<String>,
    also_others: bool,
    error: Option<String>,
    pub should_close: bool,
}

impl LocateDialog {
    pub fn open(photo: PhotoId, filename: &str) -> Self {
        Self {
            photo,
            filename: filename.to_string(),
            candidate: None,
            problems: Vec::new(),
            also_others: true,
            error: None,
            should_close: false,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        ui.label(format!("Find the original of {}.", self.filename));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Choose file…").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                self.error = None;
                self.problems = cx
                    .catalog
                    .as_ref()
                    .map(|c| relink_problems(c.connection(), self.photo, &path))
                    .and_then(|r| match r {
                        Ok(p) => Some(p),
                        Err(e) => {
                            self.error = Some(e.to_string());
                            None
                        }
                    })
                    .unwrap_or_default();
                self.candidate = Some(path);
            }
            match &self.candidate {
                Some(p) => ui.monospace(p.display().to_string()),
                None => ui.weak("No file chosen"),
            };
        });

        for p in &self.problems {
            ui.colored_label(egui::Color32::from_rgb(0xe5, 0xa0, 0x35), format!("⚠ {p}"));
        }
        ui.add_space(4.0);
        ui.checkbox(
            &mut self.also_others,
            "Also locate other missing files in the same folder",
        );
        if let Some(e) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(0xe5, 0x39, 0x35), e);
        }
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                self.should_close = true;
            }
            let label = if self.problems.is_empty() {
                "Locate"
            } else {
                "Locate anyway"
            };
            if ui
                .add_enabled(self.candidate.is_some(), egui::Button::new(label))
                .clicked()
            {
                self.apply(cx);
            }
        });
    }

    fn apply(&mut self, cx: &mut AppCx) {
        let (Some(catalog), Some(path)) = (&cx.catalog, &self.candidate) else {
            return;
        };
        let conn = catalog.connection();
        let mut changed = match relink(conn, self.photo, path) {
            Ok(ids) => ids,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        if self.also_others
            && let Some(dir) = path.parent()
        {
            match relink_missing_in_folder(conn, dir) {
                Ok(more) => changed.extend(more),
                Err(e) => tracing::warn!(error = %e, "relinking the rest of the folder failed"),
            }
        }
        cx.events.publish(CatalogEvent::PhotosChanged {
            ids: changed,
            fields: vec![PhotoField::Missing],
        });
        self.should_close = true;
    }
}
