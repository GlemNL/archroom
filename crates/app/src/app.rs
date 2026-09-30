use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Receiver;
use tracing::{error, info};
use viberoom_catalog::Catalog;
use viberoom_jobs::{JobEventKind, Scheduler};
use viberoom_services::backup::{BackupJob, BackupOutcome, list_backups, restore_backup};
use viberoom_services::preview::TrimPreviewsJob;
use viberoom_shell::{AppCx, ExportRequestKind, ExportUi, ModuleRegistry, PreferencesUi};

/// `~/Pictures/Viberoom/Viberoom.arcat`, the default catalog location
/// (plan §4.7). Falls back to `~/Viberoom/Viberoom.arcat` if the platform
/// has no Pictures directory.
fn default_catalog_path() -> PathBuf {
    let pictures =
        directories::UserDirs::new().and_then(|d| d.picture_dir().map(|p| p.to_path_buf()));
    let base = pictures.unwrap_or_else(|| {
        directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_default()
    });
    base.join("Viberoom").join("Viberoom.arcat")
}

/// Why the catalog could not be used as-is.
#[derive(Debug)]
struct Problem {
    path: PathBuf,
    /// What went wrong, in words for the user.
    message: String,
    /// The catalog opened but failed its check; "Open anyway" is possible.
    damaged: bool,
    backups: Vec<PathBuf>,
}

/// Opens the catalog and its preview cache into `cx` and starts the daily
/// backup and cache housekeeping. A catalog that will not open, or fails its
/// quick check, is left closed and reported instead (plan §14).
fn open_catalog(cx: &mut AppCx, path: &Path) -> Result<Receiver<BackupOutcome>, Problem> {
    let problem = |message: String, damaged: bool| Problem {
        path: path.to_path_buf(),
        message,
        damaged,
        backups: list_backups(path),
    };
    let catalog = Catalog::create_or_open(path).map_err(|e| {
        error!(path = %path.display(), error = %e, "failed to open catalog");
        problem(format!("The catalog could not be opened: {e}"), false)
    })?;
    match catalog.quick_check() {
        Ok(true) => {}
        Ok(false) => {
            error!(path = %path.display(), "catalog failed its integrity check");
            return Err(problem(
                "The catalog failed its integrity check; it may be damaged.".into(),
                true,
            ));
        }
        Err(e) => {
            return Err(problem(
                format!("The catalog could not be checked: {e}"),
                true,
            ));
        }
    }
    Ok(finish_open(cx, catalog, path, true))
}

/// Adopts `catalog`. `take_backup` is false for a catalog the user opened
/// despite a failed check: a damaged file must never rotate good backups out.
fn finish_open(
    cx: &mut AppCx,
    catalog: Catalog,
    path: &Path,
    take_backup: bool,
) -> Receiver<BackupOutcome> {
    info!(path = %path.display(), "catalog ready");
    match viberoom_services::PreviewCache::open_for_catalog(path) {
        Ok(previews) => cx.previews = Some(previews),
        Err(e) => error!(error = %e, "failed to open preview cache"),
    }
    cx.settings.last_catalog = Some(path.to_path_buf());
    if let Err(e) = cx.settings.save() {
        error!(error = %e, "failed to save settings");
    }
    cx.catalog = Some(catalog);

    let (tx, rx) = crossbeam_channel::unbounded();
    if take_backup && cx.settings.backups_enabled {
        cx.jobs.submit(BackupJob {
            catalog_path: path.to_path_buf(),
            keep: cx.settings.backup_keep as usize,
            done: tx,
        });
    }
    cx.jobs.submit(TrimPreviewsJob {
        catalog_path: path.to_path_buf(),
        budget_bytes: u64::from(cx.settings.cache_budget_mb) * 1024 * 1024,
    });
    rx
}

pub struct ViberoomApp {
    cx: AppCx,
    registry: ModuleRegistry,
    job_events: Receiver<viberoom_jobs::JobEvent>,
    active_jobs: usize,
    show_side_panels: bool,
    export_ui: ExportUi,
    prefs_ui: PreferencesUi,
    /// A catalog that failed its startup check or would not open.
    problem: Option<Problem>,
    backup_rx: Receiver<BackupOutcome>,
    backup_note: Option<String>,
}

impl ViberoomApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        viberoom_ui::apply_theme(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let jobs = Arc::new(Scheduler::new(0));
        let job_events = jobs.events();
        let mut cx = AppCx::new(jobs);
        if let Some(rs) = cc.wgpu_render_state.clone() {
            cx.set_render_state(rs);
        }

        viberoom_services::set_preview_options(
            cx.settings.preview_long_edge,
            cx.settings.preview_jpeg_quality,
        );

        let catalog_path = cx
            .settings
            .last_catalog
            .clone()
            .unwrap_or_else(default_catalog_path);
        let (backup_rx, problem) = match open_catalog(&mut cx, &catalog_path) {
            Ok(rx) => (rx, None),
            Err(problem) => (crossbeam_channel::never(), Some(problem)),
        };

        let registry = ModuleRegistry::new(vec![
            Box::new(viberoom_module_library::LibraryModule::new()),
            Box::new(viberoom_module_develop::DevelopModule::new()),
        ]);

        let mut registry = registry;
        // Dev/test hook: `VIBEROOM_START_MODULE=develop` opens that module
        // with the first photo selected, for headless smoke tests.
        if let Ok(id) = std::env::var("VIBEROOM_START_MODULE") {
            if let Some(catalog) = &cx.catalog
                && let Ok(photos) = viberoom_services::repo::list_all_photos(
                    catalog.connection(),
                    viberoom_services::repo::PhotoSort::default(),
                )
                && let Some(first) = photos.first()
            {
                cx.selection.select_single(first.photo_id);
            }
            let target = registry.ids().find(|(m, _)| m.0 == id).map(|(m, _)| m);
            if let Some(target) = target {
                registry.switch_to(target, &mut cx);
            }
        }

        Self {
            cx,
            registry,
            job_events,
            active_jobs: 0,
            show_side_panels: true,
            export_ui: ExportUi::default(),
            prefs_ui: PreferencesUi::default(),
            problem,
            backup_rx,
            backup_note: None,
        }
    }

    fn drain_job_events(&mut self) {
        while let Ok(ev) = self.job_events.try_recv() {
            match ev.kind {
                JobEventKind::Started => self.active_jobs += 1,
                JobEventKind::Finished | JobEventKind::Cancelled | JobEventKind::Panicked => {
                    self.active_jobs = self.active_jobs.saturating_sub(1);
                }
                JobEventKind::Progress(_) => {}
            }
        }
    }

    fn problem_window(&mut self, ctx: &egui::Context) {
        let Some(problem) = &self.problem else { return };
        enum Choice {
            Restore,
            OpenAnyway,
            Quit,
        }
        let mut choice = None;
        egui::Window::new("Catalog problem")
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(&problem.message);
                ui.monospace(problem.path.display().to_string());
                ui.add_space(6.0);
                match problem.backups.first() {
                    Some(b) => {
                        ui.label(format!(
                            "Newest backup: {}",
                            b.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                        ));
                        ui.weak("Restoring keeps the current file next to it as *.damaged-<time>.");
                    }
                    None => {
                        ui.label("There is no backup to restore.");
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !problem.backups.is_empty(),
                            egui::Button::new("Restore newest backup"),
                        )
                        .clicked()
                    {
                        choice = Some(Choice::Restore);
                    }
                    if problem.damaged && ui.button("Open anyway").clicked() {
                        choice = Some(Choice::OpenAnyway);
                    }
                    if ui.button("Quit").clicked() {
                        choice = Some(Choice::Quit);
                    }
                });
            });
        let Some(choice) = choice else { return };
        let Some(problem) = self.problem.take() else {
            return;
        };
        match choice {
            Choice::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Choice::OpenAnyway => match Catalog::create_or_open(&problem.path) {
                Ok(catalog) => {
                    self.backup_rx = finish_open(&mut self.cx, catalog, &problem.path, false)
                }
                Err(e) => {
                    self.problem = Some(Problem {
                        message: format!("The catalog could not be opened: {e}"),
                        damaged: false,
                        ..problem
                    });
                }
            },
            Choice::Restore => {
                let restored = problem
                    .backups
                    .first()
                    .map(|b| restore_backup(&problem.path, b, std::time::SystemTime::now()));
                match restored {
                    Some(Ok(moved)) => {
                        info!(kept = %moved.display(), "restored the catalog from a backup");
                        match open_catalog(&mut self.cx, &problem.path) {
                            Ok(rx) => self.backup_rx = rx,
                            Err(p) => self.problem = Some(p),
                        }
                    }
                    Some(Err(e)) => {
                        self.problem = Some(Problem {
                            message: format!("The backup could not be restored: {e}"),
                            ..problem
                        });
                    }
                    None => self.problem = Some(problem),
                }
            }
        }
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        let tab_pressed = ctx.input(|i| i.key_pressed(egui::Key::Tab));
        if tab_pressed {
            self.show_side_panels = !self.show_side_panels;
        }

        // Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z) — plan §10.3. Library's is the
        // only undo stack that exists yet (Develop's is separate, persisted
        // per-photo history, M3 work), so this always targets it; harmless
        // in Develop today since nothing pushes to it from there.
        if ctx.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::Comma,
            ))
        }) {
            self.prefs_ui.toggle(&self.cx);
        }

        // Ctrl+Shift+E opens Export; Ctrl+Alt+Shift+E repeats the last one.
        // (`consume_shortcut` checks the modifiers held when the key event
        // happened, and the more specific shortcut goes first.)
        let export_kind = ctx.input_mut(|i| {
            let with_previous = egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT | egui::Modifiers::ALT,
                egui::Key::E,
            );
            let dialog = egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::E,
            );
            if i.consume_shortcut(&with_previous) {
                Some(ExportRequestKind::WithPrevious)
            } else if i.consume_shortcut(&dialog) {
                Some(ExportRequestKind::Dialog)
            } else {
                None
            }
        });
        if let Some(kind) = export_kind {
            self.cx.request_export(kind);
        }

        let (undo_pressed, redo_pressed, save_pressed) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let undo = cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z);
            let redo = (cmd && i.modifiers.shift && i.key_pressed(egui::Key::Z))
                || (cmd && i.key_pressed(egui::Key::Y));
            let save = cmd && i.key_pressed(egui::Key::S);
            (undo, redo, save)
        });
        if undo_pressed {
            if self.registry.active_mut().undo(&mut self.cx) {
            } else if let Some(Err(e)) = self.cx.undo() {
                error!(error = %e, "undo failed");
            }
        } else if redo_pressed {
            if self.registry.active_mut().redo(&mut self.cx) {
            } else if let Some(Err(e)) = self.cx.redo() {
                error!(error = %e, "redo failed");
            }
        } else if save_pressed {
            // Ctrl+S: write metadata to XMP for the selected photos (or
            // the active one), as a background job (plan §5.3, §10.3).
            let ids: Vec<_> = if self.cx.selection.selected_count() > 0 {
                self.cx.selection.selected().collect()
            } else {
                self.cx.selection.active.into_iter().collect()
            };
            if !ids.is_empty()
                && let Some(catalog_path) = self.cx.settings.last_catalog.clone()
            {
                self.cx
                    .jobs
                    .submit(viberoom_services::sidecar::SaveXmpJob::new(
                        catalog_path,
                        ids,
                    ));
            }
        }
    }
}

impl eframe::App for ViberoomApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_job_events();
        if let Ok(outcome) = self.backup_rx.try_recv()
            && let Some(e) = outcome.error
        {
            self.backup_note = Some(format!("The daily catalog backup failed: {e}"));
        }
        if self.problem.is_some() {
            self.problem_window(ctx);
            // Nothing else runs on a closed catalog.
            egui::CentralPanel::default().show(ctx, |_| {});
            return;
        }
        self.handle_global_shortcuts(ctx);

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Viberoom");
                ui.separator();

                let modules: Vec<_> = self
                    .registry
                    .ids()
                    .map(|(id, title)| (id, title.to_string()))
                    .collect();
                let active = self.registry.active_id();
                for (id, title) in modules {
                    if ui.selectable_label(id == active, title).clicked() {
                        self.registry.switch_to(id, &mut self.cx);
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(note) = &self.backup_note {
                        ui.colored_label(egui::Color32::from_rgb(0xe0, 0x9a, 0x2b), note);
                    }
                    if self.active_jobs > 0 {
                        ui.spinner();
                        ui.label(format!(
                            "({} job{})",
                            self.active_jobs,
                            if self.active_jobs == 1 { "" } else { "s" }
                        ));
                    }
                    if ui.button("Preferences…").on_hover_text("Ctrl+,").clicked() {
                        self.prefs_ui.toggle(&self.cx);
                    }
                });
            });
        });

        if self.show_side_panels {
            egui::SidePanel::left("left_panel")
                .min_width(200.0)
                .show(ctx, |ui| {
                    self.registry.active_mut().left_panel(ui, &mut self.cx);
                });
            egui::SidePanel::right("right_panel")
                .min_width(240.0)
                .show(ctx, |ui| {
                    self.registry.active_mut().right_panel(ui, &mut self.cx);
                });
        }

        egui::TopBottomPanel::bottom("filmstrip")
            .min_height(96.0)
            .show(ctx, |ui| {
                self.registry.active_mut().filmstrip(ui, &mut self.cx);
            });

        egui::TopBottomPanel::bottom("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                self.registry.active_mut().toolbar(ui, &mut self.cx);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.registry.active_mut().center(ui, &mut self.cx);
        });

        self.export_ui.show(ctx, &mut self.cx);
        self.prefs_ui.show(ctx, &mut self.cx);
    }
}
