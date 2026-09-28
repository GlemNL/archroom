//! The import dialog (plan §7.1): portal folder picker (`rfd`), Add/Copy
//! modes, and a progress readout driven by `ImportJob`'s `CatalogEvent`s.
//!
//! Scoped down from the full §7.1 spec for M1 Phase B: it imports
//! everything supported it finds under the source folder (no per-file
//! candidate grid with checkboxes — `run_import`'s dedupe already skips
//! anything already catalogued, so re-running against the same folder is
//! safe), doesn't read existing XMP sidecars (no XMP reader exists yet —
//! that's metadata *writing* in M2, and reading is a natural pairing with
//! it), and doesn't apply keywords/metadata on import (needs the keyword
//! tables M2 builds). The destination template
//! (`{dest}/{YYYY}/{YYYY-MM-DD}/`) is fixed rather than user-editable.

use std::path::PathBuf;

use archroom_core::events::CatalogEvent;
use archroom_core::ids::ImportId;
use archroom_services::JobHandle;
use archroom_services::import::{ImportJob, ImportMode, ImportOptions};
use archroom_shell::AppCx;
use crossbeam_channel::Receiver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModeChoice {
    Add,
    Copy,
}

#[derive(Debug)]
enum Status {
    Idle,
    Running {
        scanned: u32,
        imported: u32,
        total: u32,
    },
    Done {
        scanned: u32,
        imported: u32,
        skipped: u32,
    },
    Error(String),
}

#[derive(Debug)]
pub struct ImportDialogState {
    source: Option<PathBuf>,
    mode: ModeChoice,
    dest: Option<PathBuf>,
    status: Status,
    job_handle: Option<JobHandle>,
    tracked_import_id: Option<ImportId>,
    events_rx: Receiver<CatalogEvent>,
    pub should_close: bool,
}

impl ImportDialogState {
    pub fn open(cx: &AppCx) -> Self {
        Self {
            source: None,
            mode: ModeChoice::Add,
            dest: None,
            status: Status::Idle,
            job_handle: None,
            tracked_import_id: None,
            events_rx: cx.events.subscribe(),
            should_close: false,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        self.poll_events();

        ui.horizontal(|ui| {
            ui.label("Source:");
            ui.monospace(path_label(&self.source));
            if ui.button("Choose…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.source = Some(dir);
            }
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.mode, ModeChoice::Add, "Add (reference in place)");
            ui.radio_value(&mut self.mode, ModeChoice::Copy, "Copy to a new location");
        });

        if self.mode == ModeChoice::Copy {
            ui.horizontal(|ui| {
                ui.label("Destination:");
                ui.monospace(path_label(&self.dest));
                if ui.button("Choose…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    self.dest = Some(dir);
                }
            });
            ui.weak("Files land under {destination}/{YYYY}/{YYYY-MM-DD}/.");
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        match &self.status {
            Status::Idle => {
                let ready =
                    self.source.is_some() && (self.mode == ModeChoice::Add || self.dest.is_some());
                if ui
                    .add_enabled(ready, egui::Button::new("Start Import"))
                    .clicked()
                {
                    self.start(cx);
                }
            }
            Status::Running {
                scanned,
                imported,
                total,
            } => {
                let frac = if *total > 0 {
                    *scanned as f32 / *total as f32
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(frac)
                        .text(format!("{imported} imported · {scanned}/{total} scanned")),
                );
                ui.ctx().request_repaint();
                if ui.button("Cancel").clicked()
                    && let Some(handle) = &self.job_handle
                {
                    handle.cancel();
                }
            }
            Status::Done {
                scanned,
                imported,
                skipped,
            } => {
                ui.label(format!(
                    "Imported {imported} of {scanned} scanned ({skipped} duplicate{} skipped).",
                    if *skipped == 1 { "" } else { "s" }
                ));
                if ui.button("Close").clicked() {
                    self.should_close = true;
                }
            }
            Status::Error(e) => {
                ui.colored_label(egui::Color32::from_rgb(0xe5, 0x39, 0x35), e);
                if ui.button("Close").clicked() {
                    self.should_close = true;
                }
            }
        }
    }

    fn start(&mut self, cx: &mut AppCx) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(catalog_path) = cx.settings.last_catalog.clone() else {
            self.status = Status::Error("No catalog is open.".to_string());
            return;
        };
        let mode = match &self.mode {
            ModeChoice::Add => ImportMode::Add,
            ModeChoice::Copy => match self.dest.clone() {
                Some(dest) => ImportMode::Copy { dest },
                None => return,
            },
        };

        let opts = ImportOptions {
            catalog_path,
            source,
            mode,
        };
        let job = ImportJob::new(opts, cx.events.clone());
        self.job_handle = Some(cx.jobs.submit(job));
        self.tracked_import_id = None;
        self.status = Status::Running {
            scanned: 0,
            imported: 0,
            total: 0,
        };
    }

    fn poll_events(&mut self) {
        while let Ok(ev) = self.events_rx.try_recv() {
            match ev {
                CatalogEvent::ImportProgress {
                    import_id,
                    scanned,
                    imported,
                    total_hint,
                } => {
                    // The first progress event after `start()` is this
                    // dialog's own job — only one import runs from the UI
                    // at a time, so that's an unambiguous match.
                    if self.tracked_import_id.is_none() {
                        self.tracked_import_id = Some(import_id);
                    }
                    if self.tracked_import_id == Some(import_id) {
                        self.status = Status::Running {
                            scanned,
                            imported,
                            total: total_hint.unwrap_or(scanned),
                        };
                    }
                }
                CatalogEvent::ImportFinished {
                    import_id,
                    imported,
                } => {
                    if self.tracked_import_id == Some(import_id) {
                        let scanned = match self.status {
                            Status::Running { scanned, .. } => scanned,
                            _ => imported,
                        };
                        self.status = Status::Done {
                            scanned,
                            imported,
                            skipped: scanned.saturating_sub(imported),
                        };
                    }
                }
                _ => {}
            }
        }
    }
}

fn path_label(path: &Option<PathBuf>) -> String {
    path.as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(none selected)".to_string())
}
