//! The Export dialog and its progress readout (plan §9, §10.3). Lives in the
//! shell so both Library and Develop can open it through
//! [`AppCx::request_export`], and so a running export keeps its status
//! window after the dialog closes ("the user can keep working").

use std::path::PathBuf;

use viberoom_core::events::CatalogEvent;
use viberoom_core::ids::PhotoId;
use viberoom_services::JobHandle;
use viberoom_services::export::job::{ExportJob, ExportRequest};
use viberoom_services::export::{
    Conflict, Destination, ExportPreset, ExportSettings, Format, MetadataMode, NameContext,
    OutputSpace, Resize, TiffCompression, all_presets, delete_preset, expand_name, load_last,
    output_size, save_last, save_preset,
};
use crossbeam_channel::Receiver;

use crate::AppCx;

/// What a module asks the shell for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportRequestKind {
    /// Open the dialog for the current selection.
    Dialog,
    /// Export the selection with the last-used settings, no dialog.
    WithPrevious,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResizeKind {
    Full,
    LongEdge,
    ShortEdge,
    Fit,
    Megapixels,
    Percent,
}

impl ResizeKind {
    const ALL: [ResizeKind; 6] = [
        Self::Full,
        Self::LongEdge,
        Self::ShortEdge,
        Self::Fit,
        Self::Megapixels,
        Self::Percent,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Full => "Full size",
            Self::LongEdge => "Long edge",
            Self::ShortEdge => "Short edge",
            Self::Fit => "Fit in box",
            Self::Megapixels => "Megapixels",
            Self::Percent => "Percent",
        }
    }

    fn of(r: Resize) -> Self {
        match r {
            Resize::Full => Self::Full,
            Resize::LongEdge(_) => Self::LongEdge,
            Resize::ShortEdge(_) => Self::ShortEdge,
            Resize::Fit { .. } => Self::Fit,
            Resize::Megapixels(_) => Self::Megapixels,
            Resize::Percent(_) => Self::Percent,
        }
    }

    /// The default value when the user switches to this kind.
    fn default_resize(self) -> Resize {
        match self {
            Self::Full => Resize::Full,
            Self::LongEdge => Resize::LongEdge(2048),
            Self::ShortEdge => Resize::ShortEdge(1080),
            Self::Fit => Resize::Fit { w: 2048, h: 2048 },
            Self::Megapixels => Resize::Megapixels(8.0),
            Self::Percent => Resize::Percent(50.0),
        }
    }
}

#[derive(Debug)]
struct Dialog {
    photos: Vec<PhotoId>,
    presets: Vec<ExportPreset>,
    settings: ExportSettings,
    preset_name: String,
    error: Option<String>,
}

#[derive(Debug)]
enum Phase {
    Running {
        done: u32,
        total: u32,
    },
    Finished {
        exported: u32,
        skipped: u32,
        failed: u32,
        cancelled: bool,
    },
}

#[derive(Debug)]
struct Status {
    export_id: u64,
    handle: JobHandle,
    phase: Phase,
    folder: Option<PathBuf>,
}

/// The dialog plus at most one tracked export.
#[derive(Debug, Default)]
pub struct ExportUi {
    dialog: Option<Dialog>,
    status: Option<Status>,
    events: Option<Receiver<CatalogEvent>>,
    next_id: u64,
}

/// The photos an export acts on: the selection in grid order, else the
/// active photo.
fn targets(cx: &AppCx) -> Vec<PhotoId> {
    let mut ids: Vec<PhotoId> = cx
        .selection
        .visible
        .iter()
        .copied()
        .filter(|id| cx.selection.is_selected(*id))
        .collect();
    for id in cx.selection.selected() {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        ids.extend(cx.selection.active);
    }
    ids
}

impl ExportUi {
    /// Call once per frame after the modules ran.
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut AppCx) {
        if let Some(kind) = cx.take_export_request() {
            match kind {
                ExportRequestKind::Dialog => self.open_dialog(cx),
                ExportRequestKind::WithPrevious => self.with_previous(cx),
            }
        }
        self.poll_events();
        self.dialog_window(ctx, cx);
        self.status_window(ctx);
    }

    fn open_dialog(&mut self, cx: &AppCx) {
        let photos = targets(cx);
        let (Some(catalog), false) = (&cx.catalog, photos.is_empty()) else {
            return;
        };
        let conn = catalog.connection();
        let presets = all_presets(conn).unwrap_or_default();
        let settings = load_last(conn).unwrap_or_default();
        self.dialog = Some(Dialog {
            photos,
            presets,
            settings,
            preset_name: String::new(),
            error: None,
        });
    }

    fn with_previous(&mut self, cx: &AppCx) {
        let photos = targets(cx);
        let Some(catalog) = &cx.catalog else { return };
        let settings = load_last(catalog.connection()).unwrap_or_default();
        if photos.is_empty() {
            return;
        }
        if let Err(e) = self.start(cx, photos, settings) {
            tracing::warn!(error = %e, "export with previous failed to start");
        }
    }

    fn start(
        &mut self,
        cx: &AppCx,
        photos: Vec<PhotoId>,
        settings: ExportSettings,
    ) -> Result<(), String> {
        if self
            .status
            .as_ref()
            .is_some_and(|s| matches!(s.phase, Phase::Running { .. }))
        {
            return Err("an export is already running".into());
        }
        let gpu = cx.gpu.clone().ok_or("no GPU is available")?;
        let catalog_path = cx
            .settings
            .last_catalog
            .clone()
            .ok_or("no catalog is open")?;
        if let Some(c) = &cx.catalog {
            let _ = save_last(c.connection(), &settings);
        }
        let folder = match &settings.destination {
            Destination::Folder(dir) => Some(dir.clone()),
            _ => None,
        };
        self.next_id += 1;
        let export_id = self.next_id;
        let total = photos.len() as u32;
        let job = ExportJob::new(
            ExportRequest {
                catalog_path,
                photos,
                settings,
            },
            gpu,
            cx.events.clone(),
            export_id,
        );
        self.events = Some(cx.events.subscribe());
        self.status = Some(Status {
            export_id,
            handle: cx.jobs.submit(job),
            phase: Phase::Running { done: 0, total },
            folder,
        });
        Ok(())
    }

    fn poll_events(&mut self) {
        let (Some(rx), Some(status)) = (&self.events, &mut self.status) else {
            return;
        };
        while let Ok(ev) = rx.try_recv() {
            match ev {
                CatalogEvent::ExportProgress {
                    export_id,
                    done,
                    total,
                } if export_id == status.export_id => {
                    status.phase = Phase::Running { done, total };
                }
                CatalogEvent::ExportFinished {
                    export_id,
                    exported,
                    skipped,
                    failed,
                    cancelled,
                } if export_id == status.export_id => {
                    status.phase = Phase::Finished {
                        exported,
                        skipped,
                        failed,
                        cancelled,
                    };
                }
                _ => {}
            }
        }
    }

    fn status_window(&mut self, ctx: &egui::Context) {
        let Some(status) = &self.status else { return };
        let mut dismiss = false;
        let mut cancel = false;
        egui::Window::new("Export")
            .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -12.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| match &status.phase {
                Phase::Running { done, total } => {
                    ui.add(
                        egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32)
                            .text(format!("{done}/{total}")),
                    );
                    ctx.request_repaint();
                    cancel = ui.button("Cancel").clicked();
                }
                Phase::Finished {
                    exported,
                    skipped,
                    failed,
                    cancelled,
                } => {
                    let mut text = format!("Exported {exported}");
                    if *skipped > 0 {
                        text.push_str(&format!(", skipped {skipped}"));
                    }
                    if *cancelled {
                        text.push_str(" (cancelled)");
                    }
                    ui.label(text);
                    if *failed > 0 {
                        ui.colored_label(
                            egui::Color32::from_rgb(0xe5, 0x39, 0x35),
                            format!("{failed} failed — see the log for details"),
                        );
                    }
                    ui.horizontal(|ui| {
                        if let Some(dir) = &status.folder
                            && *exported > 0
                            && ui.button("Show in folder").clicked()
                        {
                            let _ = open::that(dir);
                        }
                        dismiss = ui.button("Dismiss").clicked();
                    });
                }
            });
        if cancel {
            status.handle.cancel();
        }
        if dismiss {
            self.status = None;
        }
    }

    fn dialog_window(&mut self, ctx: &egui::Context, cx: &mut AppCx) {
        let Some(mut d) = self.dialog.take() else {
            return;
        };
        let mut open = true;
        let mut go = false;
        let mut cancel = false;
        let busy = self
            .status
            .as_ref()
            .is_some_and(|s| matches!(s.phase, Phase::Running { .. }));
        egui::Window::new(format!(
            "Export {} photo{}",
            d.photos.len(),
            if d.photos.len() == 1 { "" } else { "s" }
        ))
        .open(&mut open)
        .collapsible(false)
        .default_width(460.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(560.0)
                .show(ui, |ui| {
                    settings_ui(ui, &mut d, cx);
                });
            ui.separator();
            if let Some(e) = &d.error {
                ui.colored_label(egui::Color32::from_rgb(0xe5, 0x39, 0x35), e);
            }
            if busy {
                ui.weak("An export is already running.");
            }
            ui.horizontal(|ui| {
                go = ui.add_enabled(!busy, egui::Button::new("Export")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if go {
            match self.start(cx, d.photos.clone(), d.settings.clone()) {
                Ok(()) => return,
                Err(e) => d.error = Some(e),
            }
        }
        if open && !cancel {
            self.dialog = Some(d);
        }
    }
}

fn settings_ui(ui: &mut egui::Ui, d: &mut Dialog, cx: &AppCx) {
    // Presets.
    ui.horizontal(|ui| {
        ui.label("Preset:");
        let mut picked = None;
        egui::ComboBox::from_id_salt("export_preset")
            .selected_text("Choose…")
            .show_ui(ui, |ui| {
                for p in &d.presets {
                    if ui.selectable_label(false, &p.name).clicked() {
                        picked = Some(p.settings.clone());
                    }
                }
            });
        if let Some(s) = picked {
            d.settings = s;
        }
        ui.text_edit_singleline(&mut d.preset_name)
            .on_hover_text("Name for a new preset");
        let named = !d.preset_name.trim().is_empty();
        if ui.add_enabled(named, egui::Button::new("Save")).clicked()
            && let Some(catalog) = &cx.catalog
        {
            match save_preset(catalog.connection(), &d.preset_name, &d.settings) {
                Ok(()) => {
                    d.presets = all_presets(catalog.connection()).unwrap_or_default();
                    d.error = None;
                }
                Err(e) => d.error = Some(e.to_string()),
            }
        }
        let deletable = d
            .presets
            .iter()
            .any(|p| !p.builtin && p.name == d.preset_name.trim());
        if ui
            .add_enabled(deletable, egui::Button::new("Delete"))
            .clicked()
            && let Some(catalog) = &cx.catalog
        {
            let _ = delete_preset(catalog.connection(), d.preset_name.trim());
            d.presets = all_presets(catalog.connection()).unwrap_or_default();
        }
    });
    ui.separator();

    let s = &mut d.settings;
    egui::Grid::new("export_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Format");
            ui.horizontal(|ui| {
                for f in [Format::Jpeg, Format::Tiff, Format::Png] {
                    ui.selectable_value(&mut s.format, f, f.label());
                }
            });
            ui.end_row();

            match s.format {
                Format::Jpeg => {
                    ui.label("Quality");
                    let mut q = i32::from(s.jpeg_quality);
                    ui.add(egui::Slider::new(&mut q, 1..=100));
                    s.jpeg_quality = q as u8;
                    ui.end_row();
                }
                Format::Tiff | Format::Png => {
                    ui.label("Bit depth");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut s.bit_depth, 8, "8-bit");
                        ui.selectable_value(&mut s.bit_depth, 16, "16-bit");
                    });
                    ui.end_row();
                    if s.format == Format::Tiff {
                        ui.label("Compression");
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut s.tiff_compression,
                                TiffCompression::None,
                                "None",
                            );
                            ui.selectable_value(
                                &mut s.tiff_compression,
                                TiffCompression::Lzw,
                                "LZW",
                            );
                            ui.selectable_value(
                                &mut s.tiff_compression,
                                TiffCompression::Zip,
                                "ZIP",
                            );
                        });
                        ui.end_row();
                    }
                }
            }

            ui.label("Color space");
            egui::ComboBox::from_id_salt("export_space")
                .selected_text(s.space.label())
                .show_ui(ui, |ui| {
                    for sp in OutputSpace::ALL {
                        ui.selectable_value(&mut s.space, sp, sp.label());
                    }
                });
            ui.end_row();

            ui.label("Resize");
            ui.horizontal(|ui| {
                let mut kind = ResizeKind::of(s.resize);
                egui::ComboBox::from_id_salt("export_resize")
                    .selected_text(kind.label())
                    .show_ui(ui, |ui| {
                        for k in ResizeKind::ALL {
                            ui.selectable_value(&mut kind, k, k.label());
                        }
                    });
                if kind != ResizeKind::of(s.resize) {
                    s.resize = kind.default_resize();
                }
                match &mut s.resize {
                    Resize::Full => {}
                    Resize::LongEdge(n) | Resize::ShortEdge(n) => {
                        ui.add(egui::DragValue::new(n).range(16..=20_000).suffix(" px"));
                    }
                    Resize::Fit { w, h } => {
                        ui.add(egui::DragValue::new(w).range(16..=20_000));
                        ui.label("×");
                        ui.add(egui::DragValue::new(h).range(16..=20_000).suffix(" px"));
                    }
                    Resize::Megapixels(mp) => {
                        ui.add(egui::DragValue::new(mp).range(0.1..=200.0).suffix(" MP"));
                    }
                    Resize::Percent(p) => {
                        ui.add(egui::DragValue::new(p).range(1.0..=100.0).suffix(" %"));
                    }
                }
            });
            ui.end_row();
            ui.label("");
            ui.weak(match s.resize {
                Resize::Full => "Never enlarges.".to_string(),
                r => {
                    let (w, h) = output_size(r, (6000, 4000));
                    format!("A 6000×4000 photo becomes {w}×{h}. Never enlarges.")
                }
            });
            ui.end_row();

            if s.format != Format::Png {
                ui.label("Resolution");
                ui.add(
                    egui::DragValue::new(&mut s.ppi)
                        .range(1..=1200)
                        .suffix(" ppi"),
                );
                ui.end_row();
            }
        });

    ui.separator();
    egui::Grid::new("export_where")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Destination");
            ui.vertical(|ui| {
                let is = |x: &Destination, y: &str| {
                    matches!(
                        (x, y),
                        (Destination::Folder(_), "folder")
                            | (Destination::SameAsOriginal, "same")
                            | (Destination::Subfolder(_), "sub")
                    )
                };
                if ui
                    .radio(is(&s.destination, "folder"), "A chosen folder")
                    .clicked()
                    && !is(&s.destination, "folder")
                {
                    s.destination = Destination::Folder(
                        directories_pictures().unwrap_or_else(|| PathBuf::from(".")),
                    );
                }
                if let Destination::Folder(dir) = &mut s.destination {
                    ui.horizontal(|ui| {
                        ui.monospace(dir.display().to_string());
                        if ui.button("Choose…").clicked()
                            && let Some(picked) = rfd::FileDialog::new().pick_folder()
                        {
                            *dir = picked;
                        }
                    });
                }
                if ui
                    .radio(is(&s.destination, "same"), "Same folder as the original")
                    .clicked()
                {
                    s.destination = Destination::SameAsOriginal;
                }
                if ui
                    .radio(is(&s.destination, "sub"), "A subfolder of the original's")
                    .clicked()
                    && !is(&s.destination, "sub")
                {
                    s.destination = Destination::Subfolder("Exports".into());
                }
                if let Destination::Subfolder(name) = &mut s.destination {
                    ui.text_edit_singleline(name);
                }
            });
            ui.end_row();

            ui.label("File name");
            ui.vertical(|ui| {
                ui.text_edit_singleline(&mut s.naming);
                let example = expand_name(
                    &s.naming,
                    &NameContext {
                        stem: "IMG_0001",
                        seq: s.sequence_start,
                        capture_time: Some("2026-09-27T14:05:09"),
                        title: Some("Title"),
                        custom: &s.custom_text,
                    },
                );
                ui.weak(format!("{example}.{}", s.format.extension()));
                ui.weak("{filename} {seq:4} {date:%Y%m%d} {title} {custom}");
            });
            ui.end_row();

            ui.label("Custom text");
            ui.text_edit_singleline(&mut s.custom_text);
            ui.end_row();

            ui.label("Start number");
            ui.add(egui::DragValue::new(&mut s.sequence_start).range(0..=999_999));
            ui.end_row();

            ui.label("If a file exists");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.conflict, Conflict::Unique, "Rename");
                ui.selectable_value(&mut s.conflict, Conflict::Overwrite, "Overwrite");
                ui.selectable_value(&mut s.conflict, Conflict::Skip, "Skip");
            });
            ui.end_row();
        });

    ui.separator();
    egui::Grid::new("export_meta")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Metadata");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.metadata, MetadataMode::All, "All");
                ui.selectable_value(
                    &mut s.metadata,
                    MetadataMode::CopyrightOnly,
                    "Copyright only",
                );
                ui.selectable_value(&mut s.metadata, MetadataMode::None, "None");
            });
            ui.end_row();
            if s.metadata == MetadataMode::All {
                ui.label("");
                ui.checkbox(&mut s.remove_location, "Remove location");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut s.hierarchical_keywords, "Write keyword hierarchy");
                ui.end_row();
            }
            ui.label("After export");
            ui.checkbox(&mut s.show_in_file_manager, "Show in file manager");
            ui.end_row();
        });
}

fn directories_pictures() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures"))
}
