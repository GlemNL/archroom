//! `archroom-module-library`: the Library module (plan §7). M0 only wires
//! up the module shell — grid, import and the rest arrive in M1/M2.

use archroom_shell::{AppCx, Module, ModuleId};

#[derive(Debug)]
pub struct LibraryModule;

impl LibraryModule {
    pub fn new() -> Self {
        Self
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

    fn left_panel(&mut self, ui: &mut egui::Ui, _cx: &mut AppCx) {
        ui.heading("Catalog");
        ui.label("All Photographs");
        ui.label("Previous Import");
        ui.label("Quick Collection");
        ui.add_space(8.0);
        ui.heading("Folders");
        ui.add_space(8.0);
        ui.heading("Collections");
    }

    fn right_panel(&mut self, ui: &mut egui::Ui, _cx: &mut AppCx) {
        ui.heading("Histogram");
        ui.add_space(8.0);
        ui.heading("Metadata");
        ui.add_space(8.0);
        ui.heading("Keywording");
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, _cx: &mut AppCx) {
        ui.label("Grid  ·  Loupe  ·  Compare");
    }

    fn center(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        ui.centered_and_justified(|ui| {
            if cx.catalog_open() {
                ui.label("No photos yet — import to get started.");
            } else {
                ui.label("Open or create a catalog to get started.");
            }
        });
    }
}
