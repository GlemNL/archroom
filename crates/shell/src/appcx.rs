use std::sync::Arc;

use archroom_catalog::Catalog;
use archroom_catalog::command::Command;
use archroom_core::events::{CatalogEvent, EventBus};
use archroom_core::settings::Settings;
use archroom_jobs::Scheduler;
use archroom_services::PreviewCache;

use crate::selection::Selection;
use crate::undo::UndoStack;

/// Shared state and services passed to every `Module` method (plan §4.5).
/// Modules never call each other directly — only through `AppCx` and the
/// event bus.
///
/// `engine` joins once Develop (M3) needs it.
#[derive(Debug)]
pub struct AppCx {
    pub catalog: Option<Catalog>,
    /// The L1/L2 preview cache for `catalog` (plan §5.4). Opened alongside
    /// the catalog — they're always paired, since previews live next to it
    /// on disk (§4.7).
    pub previews: Option<PreviewCache>,
    pub jobs: Arc<Scheduler>,
    pub events: Arc<EventBus<CatalogEvent>>,
    pub settings: Settings,
    pub selection: Selection,
    /// Library's session-scoped undo stack (plan §4.5). Prefer
    /// [`AppCx::apply_command`]/[`AppCx::undo`]/[`AppCx::redo`] over using
    /// this directly — they also publish the resulting `CatalogEvent`.
    pub undo: UndoStack,
}

impl AppCx {
    pub fn new(jobs: Arc<Scheduler>) -> Self {
        Self {
            catalog: None,
            previews: None,
            jobs,
            events: Arc::new(EventBus::new()),
            settings: Settings::load().unwrap_or_default(),
            selection: Selection::default(),
            undo: UndoStack::default(),
        }
    }

    pub fn catalog_open(&self) -> bool {
        self.catalog.is_some()
    }

    /// Applies a `Command` against the open catalog, pushes it onto the
    /// undo stack, and publishes the resulting `CatalogEvent` — the one
    /// path every mutation should go through (plan §4.5). A no-op if no
    /// catalog is open.
    pub fn apply_command(&mut self, cmd: Box<dyn Command>) -> archroom_catalog::Result<()> {
        let Some(catalog) = &self.catalog else {
            return Ok(());
        };
        let event = self.undo.apply(catalog.connection(), cmd)?;
        self.events.publish(event);
        Ok(())
    }

    pub fn undo(&mut self) -> Option<archroom_catalog::Result<()>> {
        let catalog = self.catalog.as_ref()?;
        let result = self.undo.undo(catalog.connection())?;
        Some(result.map(|event| self.events.publish(event)))
    }

    pub fn redo(&mut self) -> Option<archroom_catalog::Result<()>> {
        let catalog = self.catalog.as_ref()?;
        let result = self.undo.redo(catalog.connection())?;
        Some(result.map(|event| self.events.publish(event)))
    }
}
