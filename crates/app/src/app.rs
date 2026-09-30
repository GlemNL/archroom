use std::path::PathBuf;
use std::sync::Arc;

use archroom_catalog::Catalog;
use archroom_jobs::{JobEventKind, Scheduler};
use archroom_shell::{AppCx, ExportRequestKind, ExportUi, ModuleRegistry};
use crossbeam_channel::Receiver;
use tracing::{error, info};

use crate::spike_view::SpikeView;

/// `~/Pictures/Archroom/Archroom.arcat`, the default catalog location
/// (plan §4.7). Falls back to `~/Archroom/Archroom.arcat` if the platform
/// has no Pictures directory.
fn default_catalog_path() -> PathBuf {
    let pictures =
        directories::UserDirs::new().and_then(|d| d.picture_dir().map(|p| p.to_path_buf()));
    let base = pictures.unwrap_or_else(|| {
        directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_default()
    });
    base.join("Archroom").join("Archroom.arcat")
}

pub struct ArchroomApp {
    cx: AppCx,
    registry: ModuleRegistry,
    job_events: Receiver<archroom_jobs::JobEvent>,
    active_jobs: usize,
    show_side_panels: bool,
    show_spike_window: bool,
    spike_view: SpikeView,
    export_ui: ExportUi,
}

impl ArchroomApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        archroom_ui::apply_theme(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let jobs = Arc::new(Scheduler::new(0));
        let job_events = jobs.events();
        let mut cx = AppCx::new(jobs);
        if let Some(rs) = cc.wgpu_render_state.clone() {
            cx.set_render_state(rs);
        }

        let catalog_path = cx
            .settings
            .last_catalog
            .clone()
            .unwrap_or_else(default_catalog_path);
        match Catalog::create_or_open(&catalog_path) {
            Ok(catalog) => {
                info!(path = %catalog_path.display(), "catalog ready");

                match archroom_services::PreviewCache::open_for_catalog(&catalog_path) {
                    Ok(previews) => cx.previews = Some(previews),
                    Err(e) => error!(error = %e, "failed to open preview cache"),
                }

                cx.settings.last_catalog = Some(catalog_path);
                if let Err(e) = cx.settings.save() {
                    error!(error = %e, "failed to save settings");
                }
                cx.catalog = Some(catalog);
            }
            Err(e) => {
                error!(path = %catalog_path.display(), error = %e, "failed to open catalog");
            }
        }

        let registry = ModuleRegistry::new(vec![
            Box::new(archroom_module_library::LibraryModule::new()),
            Box::new(archroom_module_develop::DevelopModule::new()),
        ]);

        let mut registry = registry;
        // Dev/test hook: `ARCHROOM_START_MODULE=develop` opens that module
        // with the first photo selected, for headless smoke tests.
        if let Ok(id) = std::env::var("ARCHROOM_START_MODULE") {
            if let Some(catalog) = &cx.catalog
                && let Ok(photos) = archroom_services::repo::list_all_photos(
                    catalog.connection(),
                    archroom_services::repo::PhotoSort::default(),
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
            show_spike_window: false,
            spike_view: SpikeView::default(),
            export_ui: ExportUi::default(),
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

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        let tab_pressed = ctx.input(|i| i.key_pressed(egui::Key::Tab));
        if tab_pressed {
            self.show_side_panels = !self.show_side_panels;
        }

        // Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z) — plan §10.3. Library's is the
        // only undo stack that exists yet (Develop's is separate, persisted
        // per-photo history, M3 work), so this always targets it; harmless
        // in Develop today since nothing pushes to it from there.
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
                    .submit(archroom_services::sidecar::SaveXmpJob::new(
                        catalog_path,
                        ids,
                    ));
            }
        }
    }
}

impl eframe::App for ArchroomApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.drain_job_events();
        self.handle_global_shortcuts(ctx);

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Archroom");
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
                    if self.active_jobs > 0 {
                        ui.spinner();
                        ui.label(format!(
                            "({} job{})",
                            self.active_jobs,
                            if self.active_jobs == 1 { "" } else { "s" }
                        ));
                    }
                    ui.toggle_value(&mut self.show_spike_window, "Spike (M0)");
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

        if self.show_spike_window {
            if let Some(render_state) = frame.wgpu_render_state().cloned() {
                egui::Window::new("M0 GPU Spike")
                    .open(&mut self.show_spike_window)
                    .default_size([640.0, 480.0])
                    .show(ctx, |ui| {
                        self.spike_view.ui(ui, &render_state);
                    });
            }
        }
    }
}
