//! The Preferences dialog (plan §10, M5): catalog location, backups,
//! preview size and quality, cache budget, XMP behaviour, GPU adapter and
//! the center background. Every change is saved immediately; the ones that
//! only apply at launch say so.

use std::path::PathBuf;

use archroom_core::settings::{CenterBackground, ClickZoom, XmpAutoWrite};
use archroom_services::backup::list_backups;
use archroom_services::engine::gpu::GpuContext;

use crate::AppCx;

#[derive(Debug, Default)]
pub struct PreferencesUi {
    open: bool,
    /// Looked up when the dialog opens, not every frame.
    adapters: Vec<String>,
    cache_bytes: u64,
    backups: usize,
    error: Option<String>,
}

const PREVIEW_EDGES: [u32; 6] = [1024, 1536, 2048, 2560, 3072, 4096];

impl PreferencesUi {
    pub fn toggle(&mut self, cx: &AppCx) {
        self.open = !self.open;
        if self.open {
            self.adapters = GpuContext::adapter_names();
            self.cache_bytes = cx.previews.as_ref().map_or(0, |p| p.total_bytes());
            self.backups = cx
                .settings
                .last_catalog
                .as_deref()
                .map_or(0, |p| list_backups(p).len());
            self.error = None;
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, cx: &mut AppCx) {
        if !self.open {
            return;
        }
        let mut open = true;
        let before = cx.settings.clone();
        egui::Window::new("Preferences")
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .show(ctx, |ui| self.ui(ui, cx));
        if !open {
            self.open = false;
        }
        if cx.settings != before {
            archroom_services::set_preview_options(
                cx.settings.preview_long_edge,
                cx.settings.preview_jpeg_quality,
            );
            self.error = cx.settings.save().err().map(|e| e.to_string());
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        let s = &mut cx.settings;
        egui::Grid::new("prefs")
            .num_columns(2)
            .spacing([14.0, 8.0])
            .show(ui, |ui| {
                ui.label("Catalog");
                ui.vertical(|ui| {
                    ui.monospace(
                        s.last_catalog
                            .as_ref()
                            .map_or_else(|| "(none)".to_string(), |p| p.display().to_string()),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Open another…").clicked()
                            && let Some(p) = rfd::FileDialog::new()
                                .add_filter("Archroom catalog", &["arcat"])
                                .pick_file()
                        {
                            s.last_catalog = Some(p);
                        }
                        if ui.button("New…").clicked()
                            && let Some(p) = rfd::FileDialog::new()
                                .add_filter("Archroom catalog", &["arcat"])
                                .set_file_name("Archroom.arcat")
                                .save_file()
                        {
                            s.last_catalog = Some(with_arcat_extension(p));
                        }
                    });
                    ui.weak("Takes effect the next time Archroom starts.");
                });
                ui.end_row();

                ui.label("Backups");
                ui.vertical(|ui| {
                    ui.checkbox(
                        &mut s.backups_enabled,
                        "Back up the catalog daily at startup",
                    );
                    ui.add_enabled_ui(s.backups_enabled, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Keep");
                            ui.add(egui::DragValue::new(&mut s.backup_keep).range(1..=60));
                            ui.label(format!("(now {})", self.backups));
                        });
                    });
                });
                ui.end_row();

                ui.label("Standard preview");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("prefs_edge")
                        .selected_text(format!("{} px", s.preview_long_edge))
                        .show_ui(ui, |ui| {
                            for e in PREVIEW_EDGES {
                                ui.selectable_value(&mut s.preview_long_edge, e, format!("{e} px"));
                            }
                        });
                    ui.label("JPEG quality");
                    let mut q = i32::from(s.preview_jpeg_quality);
                    ui.add(egui::Slider::new(&mut q, 60..=100));
                    s.preview_jpeg_quality = q as u8;
                });
                ui.end_row();
                ui.label("");
                ui.weak("Applies to previews made from now on.");
                ui.end_row();

                ui.label("Preview cache");
                ui.horizontal(|ui| {
                    let mut gb = f64::from(s.cache_budget_mb) / 1024.0;
                    ui.add(
                        egui::DragValue::new(&mut gb)
                            .range(0.25..=500.0)
                            .speed(0.25)
                            .suffix(" GB"),
                    );
                    s.cache_budget_mb = (gb * 1024.0).round() as u32;
                    ui.weak(format!(
                        "limit; using {}. Trimmed at startup, oldest first.",
                        human_bytes(self.cache_bytes)
                    ));
                });
                ui.end_row();

                ui.label("XMP sidecars");
                let mut on = s.xmp_auto_write == XmpAutoWrite::On;
                ui.checkbox(&mut on, "Write automatically after metadata changes");
                s.xmp_auto_write = if on {
                    XmpAutoWrite::On
                } else {
                    XmpAutoWrite::Off
                };
                ui.end_row();

                ui.label("Graphics card");
                ui.vertical(|ui| {
                    let shown = s.gpu_adapter.clone().unwrap_or_else(|| "Automatic".into());
                    egui::ComboBox::from_id_salt("prefs_gpu")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.gpu_adapter, None, "Automatic");
                            for name in &self.adapters {
                                ui.selectable_value(&mut s.gpu_adapter, Some(name.clone()), name);
                            }
                        });
                    ui.weak("Takes effect the next time Archroom starts.");
                });
                ui.end_row();

                ui.label("Center background");
                ui.horizontal(|ui| {
                    for (bg, name) in [
                        (CenterBackground::Black, "Black"),
                        (CenterBackground::DarkGray, "Dark gray"),
                        (CenterBackground::MediumGray, "Medium gray"),
                        (CenterBackground::White, "White"),
                    ] {
                        ui.selectable_value(&mut s.center_background, bg, name);
                    }
                });
                ui.end_row();

                ui.label("Click on a photo");
                ui.horizontal(|ui| {
                    for (z, name) in [
                        (ClickZoom::Off, "Nothing"),
                        (ClickZoom::OneToOne, "Zoom 1:1"),
                        (ClickZoom::TwoToOne, "Zoom 2:1"),
                    ] {
                        ui.selectable_value(&mut s.click_zoom, z, name);
                    }
                });
                ui.end_row();
            });
        if let Some(e) = &self.error {
            ui.colored_label(
                egui::Color32::from_rgb(0xe5, 0x39, 0x35),
                format!("Could not save preferences: {e}"),
            );
        }
    }
}

fn with_arcat_extension(mut p: PathBuf) -> PathBuf {
    if p.extension().is_none_or(|e| e != "arcat") {
        p.set_extension("arcat");
    }
    p
}

fn human_bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let b = b as f64;
    if b >= 1024.0 * MB {
        format!("{:.1} GB", b / (1024.0 * MB))
    } else {
        format!("{:.0} MB", b / MB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_paths_get_the_extension() {
        assert_eq!(
            with_arcat_extension("/a/b".into()),
            PathBuf::from("/a/b.arcat")
        );
        assert_eq!(
            with_arcat_extension("/a/b.arcat".into()),
            PathBuf::from("/a/b.arcat")
        );
        assert_eq!(
            with_arcat_extension("/a/b.db".into()),
            PathBuf::from("/a/b.arcat")
        );
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_bytes(0), "0 MB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
