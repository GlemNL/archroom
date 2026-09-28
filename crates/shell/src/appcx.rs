use std::sync::Arc;

use archroom_catalog::Catalog;
use archroom_core::events::{CatalogEvent, EventBus};
use archroom_core::settings::Settings;
use archroom_jobs::Scheduler;

/// Shared state and services passed to every `Module` method (plan §4.5).
/// Modules never call each other directly — only through `AppCx` and the
/// event bus.
///
/// M0 only wires up what the app shell itself needs: an open catalog, the
/// job scheduler, settings and the event bus. `selection` and `previews`
/// join once Library (M1/M2) needs them; `engine` joins once Develop (M3)
/// does.
#[derive(Debug)]
pub struct AppCx {
    pub catalog: Option<Catalog>,
    pub jobs: Arc<Scheduler>,
    pub events: Arc<EventBus<CatalogEvent>>,
    pub settings: Settings,
}

impl AppCx {
    pub fn new(jobs: Arc<Scheduler>) -> Self {
        Self {
            catalog: None,
            jobs,
            events: Arc::new(EventBus::new()),
            settings: Settings::load().unwrap_or_default(),
        }
    }

    pub fn catalog_open(&self) -> bool {
        self.catalog.is_some()
    }
}
