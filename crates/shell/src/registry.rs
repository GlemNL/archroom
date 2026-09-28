use crate::appcx::AppCx;
use crate::module::{Module, ModuleId};

/// Holds every registered top-level module and tracks which one is active,
/// calling `on_leave`/`on_enter` on switch (plan §4.3).
pub struct ModuleRegistry {
    modules: Vec<Box<dyn Module>>,
    active: usize,
}

impl std::fmt::Debug for ModuleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleRegistry")
            .field("modules", &self.modules.len())
            .field("active", &self.active_id())
            .finish()
    }
}

impl ModuleRegistry {
    pub fn new(modules: Vec<Box<dyn Module>>) -> Self {
        assert!(
            !modules.is_empty(),
            "a ModuleRegistry needs at least one module"
        );
        Self { modules, active: 0 }
    }

    pub fn active_id(&self) -> ModuleId {
        self.modules[self.active].id()
    }

    pub fn active_mut(&mut self) -> &mut dyn Module {
        self.modules[self.active].as_mut()
    }

    pub fn ids(&self) -> impl Iterator<Item = (ModuleId, &str)> {
        self.modules.iter().map(|m| (m.id(), m.title()))
    }

    /// Switches to the module with the given id, calling `on_leave` on the
    /// old one and `on_enter` on the new one. A no-op if `id` is already
    /// active or unknown.
    pub fn switch_to(&mut self, id: ModuleId, cx: &mut AppCx) {
        if id == self.active_id() {
            return;
        }
        let Some(next) = self.modules.iter().position(|m| m.id() == id) else {
            return;
        };
        self.modules[self.active].on_leave(cx);
        self.active = next;
        self.modules[self.active].on_enter(cx);
    }
}
