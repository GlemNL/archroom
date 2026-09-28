//! `archroom-module-library`: the Library module (plan §7). M1 Phase B/C
//! wires up import, the Grid, Loupe and filmstrip against the real
//! catalog; Organize (ratings/flags/keywords/collections/filter bar) is
//! M2.

mod filmstrip;
mod grid;
mod import_dialog;
mod left_panel;
mod loupe;
mod photos;
mod shortcuts;

use archroom_services::command::{RotatePhotos, SetFlag};
use archroom_shell::{AppCx, Module, ModuleId};
use import_dialog::ImportDialogState;
use photos::LibraryData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Grid,
    Loupe,
}

#[derive(Debug)]
pub struct LibraryModule {
    data: LibraryData,
    view: View,
    zoom: loupe::Zoom,
    thumbnail_size: f32,
    import_dialog: Option<ImportDialogState>,
}

impl LibraryModule {
    pub fn new() -> Self {
        Self {
            data: LibraryData::default(),
            view: View::Grid,
            zoom: loupe::Zoom::default(),
            thumbnail_size: 160.0,
            import_dialog: None,
        }
    }
}

impl Default for LibraryModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for LibraryModule {
    fn id(&self) -> ModuleId {
        ModuleId("library")
    }

    fn title(&self) -> &str {
        "Library"
    }

    fn left_panel(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        // The first Module method `app` calls each frame (plan §10.1's
        // layout order), so everything else this frame sees fresh data.
        self.data.sync(cx);

        left_panel::catalog_panel(ui, cx, &self.data);
        ui.add_space(12.0);
        left_panel::folders_panel(ui, cx, &mut self.data);
    }

    fn right_panel(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        ui.label(format!(
            "{} of {} selected",
            cx.selection.selected_count(),
            self.data.photos.len()
        ));
        ui.add_space(8.0);
        ui.heading("Histogram");
        ui.add_space(8.0);
        ui.heading("Metadata");
        ui.add_space(8.0);
        ui.heading("Keywording");
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        if ui
            .selectable_label(self.view == View::Grid, "Grid")
            .clicked()
        {
            self.view = View::Grid;
        }
        if ui
            .selectable_label(self.view == View::Loupe, "Loupe")
            .clicked()
        {
            self.view = View::Loupe;
        }
        ui.separator();

        match self.view {
            View::Grid => {
                ui.label("Thumbnail size:");
                ui.add(egui::Slider::new(&mut self.thumbnail_size, 80.0..=320.0).show_value(false));
            }
            View::Loupe => {
                if ui
                    .selectable_label(self.zoom == loupe::Zoom::Fit, "Fit")
                    .clicked()
                {
                    self.zoom = loupe::Zoom::Fit;
                }
                if ui
                    .selectable_label(self.zoom == loupe::Zoom::OneToOne, "1:1")
                    .clicked()
                {
                    self.zoom = loupe::Zoom::OneToOne;
                }
            }
        }

        ui.separator();
        let has_target = cx.selection.selected_count() > 0 || cx.selection.active.is_some();
        ui.add_enabled_ui(has_target, |ui| {
            if ui.button("Pick").on_hover_text("P").clicked() {
                let _ = cx.apply_command(Box::new(SetFlag::new(shortcuts::targets(cx), 1)));
            }
            if ui.button("Reject").on_hover_text("X").clicked() {
                let _ = cx.apply_command(Box::new(SetFlag::new(shortcuts::targets(cx), -1)));
            }
            shortcuts::label_picker(ui, cx);
            if ui
                .button("⟲")
                .on_hover_text("Rotate left (Ctrl+[)")
                .clicked()
            {
                let _ = cx.apply_command(Box::new(RotatePhotos::new(shortcuts::targets(cx), -1)));
            }
            if ui
                .button("⟳")
                .on_hover_text("Rotate right (Ctrl+])")
                .clicked()
            {
                let _ = cx.apply_command(Box::new(RotatePhotos::new(shortcuts::targets(cx), 1)));
            }
        });

        ui.separator();
        if ui
            .add_enabled(cx.undo.can_undo(), egui::Button::new("↶"))
            .on_hover_text(cx.undo.undo_label().unwrap_or_else(|| "Undo".to_string()))
            .clicked()
        {
            let _ = cx.undo();
        }
        if ui
            .add_enabled(cx.undo.can_redo(), egui::Button::new("↷"))
            .on_hover_text(cx.undo.redo_label().unwrap_or_else(|| "Redo".to_string()))
            .clicked()
        {
            let _ = cx.redo();
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let enabled = cx.catalog_open() && self.import_dialog.is_none();
            if ui
                .add_enabled(enabled, egui::Button::new("Import…"))
                .clicked()
            {
                self.import_dialog = Some(ImportDialogState::open(cx));
            }
        });
    }

    fn center(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        if !cx.catalog_open() {
            ui.centered_and_justified(|ui| {
                ui.label("Open or create a catalog to get started.");
            });
            return;
        }

        if self.data.photos.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("No photos yet — import to get started.");
            });
        } else {
            let ordered = self.data.ordered_ids();
            shortcuts::handle(ui, cx, &ordered);

            match self.view {
                View::Grid => {
                    if let Some(id) = grid::show(ui, cx, &self.data.photos, self.thumbnail_size) {
                        cx.selection.select_single(id);
                        self.view = View::Loupe;
                    }
                }
                View::Loupe => {
                    loupe::show(ui, cx, &self.data.photos, &self.data.folders, self.zoom);
                }
            }
        }

        if let Some(dialog) = &mut self.import_dialog {
            let mut still_open = true;
            egui::Window::new("Import Photos")
                .open(&mut still_open)
                .collapsible(false)
                .default_size([560.0, 260.0])
                .show(ui.ctx(), |ui| dialog.ui(ui, cx));
            if !still_open || dialog.should_close {
                self.import_dialog = None;
            }
        }
    }

    fn filmstrip(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        filmstrip::show(ui, cx, &self.data.photos);
    }
}
