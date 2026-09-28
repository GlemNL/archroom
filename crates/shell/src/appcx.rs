use std::sync::Arc;

use archroom_catalog::Catalog;
use archroom_core::events::{CatalogEvent, EventBus};
use archroom_core::settings::Settings;
use archroom_jobs::Scheduler;
use archroom_services::PreviewCache;

use crate::selection::Selection;

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
        }
    }

    pub fn catalog_open(&self) -> bool {
        self.catalog.is_some()
    }
}
