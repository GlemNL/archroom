//! `archroom-module-develop`: the Develop module (plan §8). M0 only wires
//! up the module shell — the canvas, engine and panels arrive in M3/M4.

use archroom_shell::{AppCx, Module, ModuleId};

#[derive(Debug)]
pub struct DevelopModule;

impl DevelopModule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DevelopModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for DevelopModule {
    fn id(&self) -> ModuleId {
        ModuleId("develop")
    }

    fn title(&self) -> &str {
        "Develop"
    }

    fn left_panel(&mut self, ui: &mut egui::Ui, _cx: &mut AppCx) {
        ui.heading("Navigator");
        ui.add_space(8.0);
        ui.heading("Presets");
        ui.add_space(8.0);
        ui.heading("Snapshots");
        ui.add_space(8.0);
        ui.heading("History");
    }

    fn right_panel(&mut self, ui: &mut egui::Ui, _cx: &mut AppCx) {
        ui.heading("Histogram");
        ui.add_space(8.0);
        ui.heading("Basic");
    }

    fn center(&mut self, ui: &mut egui::Ui, _cx: &mut AppCx) {
        ui.centered_and_justified(|ui| {
            ui.label("Select a photo to develop.");
        });
    }
}
