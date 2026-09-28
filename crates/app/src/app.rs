use std::path::PathBuf;
use std::sync::Arc;

use archroom_catalog::Catalog;
use archroom_jobs::{JobEventKind, Scheduler};
use archroom_shell::{AppCx, ModuleRegistry};
use crossbeam_channel::Receiver;
use tracing::{error, info};

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
}

impl ArchroomApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        archroom_ui::apply_theme(&cc.egui_ctx);

        let jobs = Arc::new(Scheduler::new(0));
        let job_events = jobs.events();
        let mut cx = AppCx::new(jobs);

        let catalog_path = cx
            .settings
            .last_catalog
            .clone()
            .unwrap_or_else(default_catalog_path);
        match Catalog::create_or_open(&catalog_path) {
            Ok(catalog) => {
                info!(path = %catalog_path.display(), "catalog ready");
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

        Self {
            cx,
            registry,
            job_events,
            active_jobs: 0,
            show_side_panels: true,
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
    }
}

impl eframe::App for ArchroomApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

        egui::TopBottomPanel::bottom("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                self.registry.active_mut().toolbar(ui, &mut self.cx);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.registry.active_mut().center(ui, &mut self.cx);
        });
    }
}
