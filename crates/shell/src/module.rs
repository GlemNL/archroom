//! The top-level `Module` extension point (plan §4.3/§4.4): Library,
//! Develop, and later Map/Print. Adding one means a crate implementing
//! `Module`, plus one line in [`ModuleRegistry::with_builtins`]-style setup
//! in the `app` binary.

use crate::appcx::AppCx;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub &'static str);

/// Module-scoped keyboard actions, resolved from the keymap (plan §4.4).
/// Empty for now; the keymap and its actions arrive with Library's
/// shortcuts in M1/M2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {}

pub trait Module {
    fn id(&self) -> ModuleId;
    fn title(&self) -> &str;

    fn on_enter(&mut self, _cx: &mut AppCx) {}
    fn on_leave(&mut self, _cx: &mut AppCx) {}

    fn left_panel(&mut self, _ui: &mut egui::Ui, _cx: &mut AppCx) {}
    fn right_panel(&mut self, _ui: &mut egui::Ui, _cx: &mut AppCx) {}
    fn toolbar(&mut self, _ui: &mut egui::Ui, _cx: &mut AppCx) {}
    fn center(&mut self, ui: &mut egui::Ui, cx: &mut AppCx);
    /// Shared between Library and Develop in real Lightroom (plan §7.3);
    /// a no-op until a module has photos to show a strip of.
    fn filmstrip(&mut self, _ui: &mut egui::Ui, _cx: &mut AppCx) {}

    /// Ctrl+Z / Ctrl+Y: a module with its own history (Develop) handles it
    /// and returns true; otherwise the app falls back to the shared stack.
    fn undo(&mut self, _cx: &mut AppCx) -> bool {
        false
    }
    fn redo(&mut self, _cx: &mut AppCx) -> bool {
        false
    }

    fn handle_action(&mut self, _action: Action, _cx: &mut AppCx) -> bool {
        false
    }
}
