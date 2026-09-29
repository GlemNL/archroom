use std::sync::Arc;

use archroom_catalog::Catalog;
use archroom_catalog::command::Command;
use archroom_core::events::{CatalogEvent, EventBus, PhotoField};
use archroom_core::settings::{Settings, XmpAutoWrite};
use archroom_jobs::Scheduler;
use archroom_services::PreviewCache;
use archroom_services::sidecar::SaveXmpJob;

use crate::selection::Selection;
use crate::undo::UndoStack;

/// `egui_wgpu::RenderState` with a `Debug` impl so `AppCx` can derive it.
#[derive(Clone)]
pub struct RenderStateHandle(pub egui_wgpu::RenderState);

impl std::fmt::Debug for RenderStateHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RenderState")
    }
}

/// Shared state and services passed to every `Module` method (plan §4.5).
/// Modules never call each other directly — only through `AppCx` and the
/// event bus.
///
/// The GPU is `None` when the app runs without a wgpu backend; Develop then
/// shows a message instead of a canvas.
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
    /// The device egui draws with, shared with the engine (plan D1).
    pub gpu: Option<archroom_services::engine::gpu::GpuContext>,
    /// For registering engine textures with egui.
    pub render_state: Option<RenderStateHandle>,
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
            gpu: None,
            render_state: None,
        }
    }

    /// Hands over egui's wgpu device so modules can render with the engine.
    pub fn set_render_state(&mut self, rs: egui_wgpu::RenderState) {
        let name = rs.adapter.get_info().name;
        self.gpu = Some(archroom_services::engine::gpu::GpuContext::from_parts(
            rs.device.clone(),
            rs.queue.clone(),
            &name,
        ));
        self.render_state = Some(RenderStateHandle(rs));
    }

    pub fn catalog_open(&self) -> bool {
        self.catalog.is_some()
    }

    /// Applies a `Command` against the open catalog, pushes it onto the
    /// undo stack, and publishes the resulting `CatalogEvent` — the one
    /// path every mutation should go through (plan §4.5). A no-op if no
    /// catalog is open.
    pub fn apply_command(&mut self, cmd: Box<dyn Command>) -> archroom_catalog::Result<()> {
        self.apply_command_event(cmd).map(|_| ())
    }

    /// Like [`AppCx::apply_command`], but also returns the event the command
    /// produced (e.g. the ids `CreateVirtualCopies` made). `None` when no
    /// catalog is open.
    pub fn apply_command_event(
        &mut self,
        cmd: Box<dyn Command>,
    ) -> archroom_catalog::Result<Option<CatalogEvent>> {
        let Some(catalog) = &self.catalog else {
            return Ok(None);
        };
        let event = self.undo.apply(catalog.connection(), cmd)?;
        self.events.publish(event.clone());

        // Auto-write (plan §5.3): a metadata change to N photos schedules
        // N sidecar writes as a background job, never on the UI thread
        // (plan §4.6). Off by default, like Lightroom (D5).
        if self.settings.xmp_auto_write == XmpAutoWrite::On
            && let CatalogEvent::PhotosChanged { ids, fields } = &event
            && fields.iter().any(|f| {
                matches!(
                    f,
                    PhotoField::Rating
                        | PhotoField::Flag
                        | PhotoField::ColorLabel
                        | PhotoField::Keywords
                        | PhotoField::Metadata
                )
            })
            && let Some(catalog_path) = self.settings.last_catalog.clone()
        {
            self.jobs.submit(SaveXmpJob::new(catalog_path, ids.clone()));
        }

        Ok(Some(event))
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
