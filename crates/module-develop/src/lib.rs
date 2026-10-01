//! `viberoom-module-develop`: the Develop module (plan §8). One open photo
//! at a time: the engine renders it on a worker thread (`Session`), the
//! canvas draws the resulting texture, and edits are persisted through
//! `services::develop` as coalesced history steps.

mod basic;
mod canvas;
mod copy_dialog;
mod crop_tool;
mod filmstrip;
mod histogram;
mod history;
mod left;
mod local_tool;
mod navigator;
mod panels;
mod preset_dialog;
mod presets_panel;
mod tool;

use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use viberoom_core::ids::PhotoId;
use viberoom_services::catalog_develop::{HistoryRow, SnapshotRow};
use viberoom_services::engine::ops::{Profile, Treatment, default_registry};
use viberoom_services::engine::pipeline::{Histogram, RenderRequest};
use viberoom_services::engine::{EditParams, Orientation, Registry};
use viberoom_services::presets::Preset;
use viberoom_services::repo::{self, PhotoFileInfo};
use viberoom_services::rerender::RerenderPreviewsJob;
use viberoom_services::session::{Session, open_in_background};
use viberoom_services::{catalog_develop, develop};
use viberoom_shell::{AppCx, Module, ModuleId};

use tool::{CanvasTool, ToolAction, ToolId};

/// Slider drags on one control become a history step this long after the
/// last movement (a database write per frame would stutter).
const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zoom {
    Fit,
    OneToOne,
    TwoToOne,
    ThreeToOne,
    FiveToOne,
}

impl Zoom {
    fn factor(self) -> Option<f32> {
        match self {
            Zoom::Fit => None,
            Zoom::OneToOne => Some(1.0),
            Zoom::TwoToOne => Some(2.0),
            Zoom::ThreeToOne => Some(3.0),
            Zoom::FiveToOne => Some(5.0),
        }
    }
}

/// What a render was asked for, to skip identical requests.
type RenderKey = ([u8; 16], u32, bool, bool, bool, Option<String>);

#[derive(Debug)]
struct Doc {
    photo: PhotoId,
    info: Option<PhotoFileInfo>,
    session: Option<Session>,
    loading: Option<Receiver<Result<Session, String>>>,
    error: Option<String>,
    params: EditParams,
    history: Vec<HistoryRow>,
    snapshots: Vec<SnapshotRow>,
    /// The history seq the user is at after an undo; `None` = the newest.
    cursor: Option<i64>,
    dirty: Option<(String, Instant)>,
    texture: Option<egui::TextureId>,
    histogram: Option<Histogram>,
    exif_line: String,
    /// Saved edits the Library previews don't reflect yet.
    previews_stale: bool,
    last_key: Option<RenderKey>,
    submitted: u64,
    received: u64,
}

#[derive(Debug)]
struct Clipboard {
    source: EditParams,
    groups: Vec<viberoom_services::engine::SettingsGroup>,
}

/// Where the canvas and image were drawn last frame (for the Navigator).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ViewInfo {
    canvas: egui::Rect,
    image: egui::Rect,
}

#[derive(Debug)]
pub struct DevelopModule {
    doc: Option<Doc>,
    registry: Registry,
    zoom: Zoom,
    /// The zoom last frame, to notice a change and start the animation.
    last_zoom: Zoom,
    zoom_anim: Option<canvas::ZoomAnim>,
    /// The image size and pan actually drawn last frame (mid-animation
    /// these differ from the zoom's own).
    drawn: (egui::Vec2, egui::Vec2),
    pan: egui::Vec2,
    clip: bool,
    before: bool,
    eyedropper: bool,
    /// Longest edge (px) the last layout wanted the render to have.
    want_edge: u32,
    panels: panels::State,
    /// The open canvas tool, if any: crop (`R`), red eye (`Shift+R`),
    /// gradient (`M`) or brush (`K`).
    tool: Option<Box<dyn CanvasTool>>,
    view: Option<ViewInfo>,
    presets: Vec<Preset>,
    presets_loaded: bool,
    preset_dialog: Option<preset_dialog::PresetDialog>,
    snapshot_name: String,
    clipboard: Option<Clipboard>,
    copy_dialog: Option<copy_dialog::CopyDialog>,
    previous: Option<PhotoId>,
    last_pass: u64,
    /// Dev/test hook: `VIBEROOM_START_TOOL=crop|redeye|gradient|brush`
    /// opens that tool as soon as the first photo has loaded.
    start_tool: Option<ToolId>,
}

impl DevelopModule {
    pub fn new() -> Self {
        Self {
            doc: None,
            registry: default_registry(),
            zoom: Zoom::Fit,
            last_zoom: Zoom::Fit,
            zoom_anim: None,
            drawn: (egui::Vec2::ZERO, egui::Vec2::ZERO),
            pan: egui::Vec2::ZERO,
            clip: false,
            before: false,
            eyedropper: false,
            want_edge: 1536,
            panels: panels::State::default(),
            tool: None,
            view: None,
            presets: Vec::new(),
            presets_loaded: false,
            preset_dialog: None,
            snapshot_name: String::new(),
            clipboard: None,
            copy_dialog: None,
            previous: None,
            last_pass: u64::MAX,
            start_tool: std::env::var("VIBEROOM_START_TOOL")
                .ok()
                .and_then(|v| match v.as_str() {
                    "crop" => Some(ToolId::Crop),
                    "redeye" => Some(ToolId::RedEye),
                    "gradient" => Some(ToolId::Gradient),
                    "brush" => Some(ToolId::Brush),
                    _ => None,
                }),
        }
    }

    // --- Per-frame bookkeeping -------------------------------------------

    /// Runs once per egui pass, from whichever panel draws first (the side
    /// panels can be hidden with Tab).
    fn sync(&mut self, ctx: &egui::Context, cx: &mut AppCx) {
        let pass = ctx.cumulative_pass_nr();
        if self.last_pass == pass {
            return;
        }
        self.last_pass = pass;

        let active = cx.selection.active;
        if self.doc.as_ref().map(|d| d.photo) != active {
            self.switch(cx, active);
        }
        self.flush_if_due(cx);
        let mut busy = false;
        let mut open_tool = None;
        if let Some(doc) = &mut self.doc {
            if let Some(rx) = &doc.loading {
                match rx.try_recv() {
                    Ok(Ok(session)) => {
                        doc.session = Some(session);
                        doc.loading = None;
                        doc.last_key = None;
                        open_tool = self.start_tool.take();
                    }
                    Ok(Err(e)) => {
                        doc.error = Some(e);
                        doc.loading = None;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => busy = true,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => doc.loading = None,
                }
            }
            if let Some(session) = &doc.session {
                if let Some(res) = session.render.take_result() {
                    doc.received = doc.received.max(res.id);
                    if let Some(e) = res.error {
                        doc.error = Some(e);
                    } else {
                        doc.error = None;
                        doc.histogram = res.histogram;
                        if let (Some(tex), Some(rs)) = (res.texture, &cx.render_state) {
                            let view = tex.create_view(&Default::default());
                            let mut renderer = rs.0.renderer.write();
                            match doc.texture {
                                Some(id) => renderer.update_egui_texture_from_wgpu_texture(
                                    &rs.0.device,
                                    &view,
                                    egui_wgpu::wgpu::FilterMode::Linear,
                                    id,
                                ),
                                None => {
                                    doc.texture = Some(renderer.register_native_texture(
                                        &rs.0.device,
                                        &view,
                                        egui_wgpu::wgpu::FilterMode::Linear,
                                    ));
                                }
                            }
                        }
                    }
                }
                let params = if self.before {
                    EditParams::default()
                } else if let Some(tool) = &self.tool {
                    tool.display_params(&doc.params)
                } else {
                    doc.params.clone()
                };
                let ignore_crop = self.tool.as_ref().is_some_and(|t| t.ignore_crop());
                let overlay = self.tool.as_ref().and_then(|t| t.mask_overlay());
                let key: RenderKey = (
                    params.hash(),
                    self.want_edge,
                    self.clip,
                    self.before,
                    ignore_crop,
                    overlay.clone(),
                );
                if doc.last_key.as_ref() != Some(&key) {
                    let mut req = RenderRequest::new(params, self.want_edge);
                    req.orientation = session.orientation;
                    req.clip_overlay = self.clip;
                    req.ignore_crop = ignore_crop;
                    req.mask_overlay = overlay;
                    req.want_histogram = true;
                    doc.submitted = session.render.submit(req);
                    doc.last_key = Some(key);
                }
                busy |= doc.received < doc.submitted;
            }
            busy |= doc.dirty.is_some();
        }
        if let Some(id) = open_tool {
            self.open_tool(cx, id);
        }
        if busy {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    /// Queues a background re-render of the Library previews for `photo`.
    fn schedule_previews(cx: &AppCx, photo: PhotoId, params: EditParams) {
        let (Some(gpu), Some(catalog), Some(catalog_path)) =
            (&cx.gpu, &cx.catalog, cx.settings.last_catalog.clone())
        else {
            return;
        };
        let Ok(Some(info)) = repo::photo_file_info(catalog.connection(), photo) else {
            return;
        };
        cx.jobs.submit(RerenderPreviewsJob {
            catalog_path,
            gpu: gpu.clone(),
            photo,
            info,
            params,
        });
    }

    /// Re-renders the open photo's previews if edits changed them.
    fn refresh_previews(&mut self, cx: &AppCx) {
        if let Some(doc) = &mut self.doc
            && std::mem::take(&mut doc.previews_stale)
        {
            Self::schedule_previews(cx, doc.photo, doc.params.clone());
        }
    }

    /// Opens `photo` (or closes the document when nothing is active).
    fn switch(&mut self, cx: &mut AppCx, photo: Option<PhotoId>) {
        self.flush(cx);
        self.refresh_previews(cx);
        if let Some(old) = self.doc.take() {
            self.previous = Some(old.photo);
            if let (Some(id), Some(rs)) = (old.texture, &cx.render_state) {
                rs.0.renderer.write().free_texture(&id);
            }
        }
        self.eyedropper = false;
        self.tool = None;
        self.before = false;
        self.pan = egui::Vec2::ZERO;
        self.zoom = Zoom::Fit;
        self.last_zoom = Zoom::Fit;
        self.zoom_anim = None;
        let (Some(photo), Some(catalog)) = (photo, &cx.catalog) else {
            return;
        };
        let conn = catalog.connection();
        let info = repo::photo_file_info(conn, photo).ok().flatten();
        let mut doc = Doc {
            photo,
            info: info.clone(),
            session: None,
            loading: None,
            error: None,
            params: develop::load_params(conn, photo).unwrap_or_default(),
            history: catalog_develop::list_history(conn, photo).unwrap_or_default(),
            snapshots: catalog_develop::list_snapshots(conn, photo).unwrap_or_default(),
            cursor: None,
            dirty: None,
            texture: None,
            histogram: None,
            previews_stale: false,
            exif_line: repo::photo_exif(conn, photo)
                .map(|e| exif_line(&e))
                .unwrap_or_default(),
            last_key: None,
            submitted: 0,
            received: 0,
        };
        match (&cx.gpu, info) {
            (Some(gpu), Some(info)) => {
                doc.loading = Some(open_in_background(&cx.jobs, gpu.clone(), photo, info));
            }
            (None, _) => doc.error = Some("No GPU available for Develop".into()),
            (_, None) => doc.error = Some("This photo's file could not be found".into()),
        }
        self.doc = Some(doc);
    }

    // --- Editing ---------------------------------------------------------

    /// Applies `change` to the open photo's params. Slider drags coalesce
    /// (persisted after `SAVE_DEBOUNCE`); `immediate` ones persist now.
    fn apply(&mut self, cx: &mut AppCx, params: EditParams, change: basic::Change) {
        let Some(doc) = &mut self.doc else { return };
        if params == doc.params {
            return;
        }
        if doc.dirty.as_ref().is_some_and(|(l, _)| *l != change.label) {
            self.flush(cx);
        }
        let Some(doc) = &mut self.doc else { return };
        doc.params = params;
        doc.dirty = Some((change.label, Instant::now()));
        if change.immediate {
            self.flush(cx);
        }
    }

    fn flush_if_due(&mut self, cx: &mut AppCx) {
        if self
            .doc
            .as_ref()
            .and_then(|d| d.dirty.as_ref())
            .is_some_and(|(_, at)| at.elapsed() >= SAVE_DEBOUNCE)
        {
            self.flush(cx);
        }
    }

    /// Writes pending edits as a history step and announces them.
    fn flush(&mut self, cx: &mut AppCx) {
        let (Some(doc), Some(catalog)) = (&mut self.doc, &cx.catalog) else {
            return;
        };
        let Some((label, _)) = doc.dirty.take() else {
            return;
        };
        let conn = catalog.connection();
        match develop::save_edit(conn, doc.photo, &label, &doc.params, doc.cursor) {
            Ok((_, event)) => {
                doc.cursor = None;
                doc.previews_stale = true;
                doc.history = catalog_develop::list_history(conn, doc.photo).unwrap_or_default();
                cx.events.publish(event);
            }
            Err(e) => tracing::error!(error = %e, "failed to save develop settings"),
        }
    }

    /// Moves to a history state without recording a step.
    fn jump(&mut self, cx: &mut AppCx, seq: i64, params: EditParams) {
        self.flush(cx);
        let (Some(doc), Some(catalog)) = (&mut self.doc, &cx.catalog) else {
            return;
        };
        match develop::apply_params(catalog.connection(), doc.photo, &params) {
            Ok(event) => {
                let newest = doc.history.last().map_or(0, |r| r.seq);
                doc.cursor = (seq != newest).then_some(seq);
                doc.previews_stale = true;
                doc.params = params;
                cx.events.publish(event);
            }
            Err(e) => tracing::error!(error = %e, "failed to apply history step"),
        }
    }

    fn reset(&mut self, cx: &mut AppCx) {
        self.flush(cx);
        let (Some(doc), Some(catalog)) = (&mut self.doc, &cx.catalog) else {
            return;
        };
        if doc.params.is_identity() {
            return;
        }
        let conn = catalog.connection();
        match develop::reset(conn, doc.photo, doc.cursor) {
            Ok((_, event)) => {
                doc.params = EditParams::default();
                doc.previews_stale = true;
                doc.cursor = None;
                doc.history = catalog_develop::list_history(conn, doc.photo).unwrap_or_default();
                cx.events.publish(event);
            }
            Err(e) => tracing::error!(error = %e, "failed to reset develop settings"),
        }
    }

    fn walk(&mut self, cx: &mut AppCx, redo: bool) -> bool {
        self.flush(cx);
        let Some(doc) = &self.doc else { return false };
        let target = if redo {
            develop::redo_target(&doc.history, doc.cursor)
        } else {
            develop::undo_target(&doc.history, doc.cursor)
        };
        if let Some((seq, params)) = target {
            self.jump(cx, seq, params);
        }
        true
    }

    /// Pastes `groups` of `source` onto the active photo and any other
    /// selected photos (plan §8.4).
    fn paste(
        &mut self,
        cx: &mut AppCx,
        source: &EditParams,
        groups: &[viberoom_services::engine::SettingsGroup],
        label: &str,
        include_active: bool,
    ) {
        let Some(doc) = &self.doc else { return };
        let active = doc.photo;
        if include_active {
            let mut params = doc.params.clone();
            develop::paste_groups(&mut params, source, groups);
            self.apply(
                cx,
                params,
                basic::Change {
                    label: label.to_string(),
                    immediate: true,
                },
            );
        }
        let others: Vec<PhotoId> = cx.selection.selected().filter(|p| *p != active).collect();
        let Some(catalog) = &cx.catalog else { return };
        for photo in others {
            let conn = catalog.connection();
            let Ok(mut p) = develop::load_params(conn, photo) else {
                continue;
            };
            develop::paste_groups(&mut p, source, groups);
            match develop::save_edit(conn, photo, label, &p, None) {
                Ok((_, event)) => {
                    cx.events.publish(event);
                    Self::schedule_previews(cx, photo, p);
                }
                Err(e) => tracing::error!(error = %e, "paste failed"),
            }
        }
    }

    fn paste_previous(&mut self, cx: &mut AppCx) {
        let (Some(prev), Some(catalog)) = (self.previous, &cx.catalog) else {
            return;
        };
        let Ok(source) = develop::load_params(catalog.connection(), prev) else {
            return;
        };
        let groups: Vec<_> = self
            .registry
            .iter()
            .map(|o| o.group())
            .filter(|g| {
                !g.is_photo_specific()
                    && !matches!(
                        g,
                        viberoom_services::engine::SettingsGroup::Crop
                            | viberoom_services::engine::SettingsGroup::Straighten
                            | viberoom_services::engine::SettingsGroup::ProcessVersion
                    )
            })
            .collect();
        self.paste(cx, &source, &groups, "Paste Previous", true);
    }

    /// Synchronizes `groups` of the active photo's settings to the rest of
    /// the selection (plan §8.4).
    fn sync_selected(
        &mut self,
        cx: &mut AppCx,
        groups: &[viberoom_services::engine::SettingsGroup],
    ) {
        self.flush(cx);
        let Some(doc) = &self.doc else { return };
        let source = doc.params.clone();
        self.paste(cx, &source, groups, "Sync Settings", false);
    }

    fn reload_snapshots(&mut self, cx: &AppCx) {
        if let (Some(doc), Some(catalog)) = (&mut self.doc, &cx.catalog) {
            doc.snapshots = catalog_develop::list_snapshots(catalog.connection(), doc.photo)
                .unwrap_or_default();
        }
    }

    /// `Ctrl+N` / the + button: saves the current state as a named snapshot.
    fn add_snapshot(&mut self, cx: &mut AppCx, name: &str) {
        self.flush(cx);
        let (Some(doc), Some(catalog)) = (&self.doc, &cx.catalog) else {
            return;
        };
        let name = if name.trim().is_empty() {
            format!("Snapshot {}", doc.snapshots.len() + 1)
        } else {
            name.trim().to_string()
        };
        if let Err(e) = catalog_develop::add_snapshot(
            catalog.connection(),
            doc.photo,
            &name,
            &doc.params.to_json(),
        ) {
            tracing::error!(error = %e, "failed to add snapshot");
        }
        self.reload_snapshots(cx);
    }

    fn delete_snapshot(&mut self, cx: &mut AppCx, id: i64) {
        if let Some(catalog) = &cx.catalog
            && let Err(e) = catalog_develop::delete_snapshot(catalog.connection(), id)
        {
            tracing::error!(error = %e, "failed to delete snapshot");
        }
        self.reload_snapshots(cx);
    }

    fn apply_snapshot(&mut self, cx: &mut AppCx, row: &SnapshotRow) {
        let params = EditParams::from_json(&row.params).unwrap_or_default();
        self.apply(
            cx,
            params,
            basic::Change {
                label: format!("Snapshot: {}", row.name),
                immediate: true,
            },
        );
    }

    fn reload_presets(&mut self, cx: &AppCx) {
        if let Some(catalog) = &cx.catalog {
            self.presets = viberoom_services::presets::list_all(catalog.connection())
                .unwrap_or_else(|_| viberoom_services::presets::builtins());
        }
        self.presets_loaded = true;
    }

    /// Makes a virtual copy of the active photo and opens it (plan §7.4).
    fn virtual_copy(&mut self, cx: &mut AppCx) {
        use viberoom_services::command::CreateVirtualCopies;
        self.flush(cx);
        let Some(photo) = self.doc.as_ref().map(|d| d.photo) else {
            return;
        };
        match cx.apply_command_event(Box::new(CreateVirtualCopies::new(vec![photo]))) {
            Ok(Some(viberoom_core::events::CatalogEvent::PhotosAdded { ids, .. })) => {
                if let Some(&copy) = ids.first() {
                    cx.selection.select_single(copy);
                }
            }
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "failed to create a virtual copy"),
        }
    }

    /// Opens `id`, or closes it when it is already open. A local tool asked
    /// to become another local tool keeps its draft; anything else commits
    /// the open tool first.
    fn open_tool(&mut self, cx: &mut AppCx, id: ToolId) {
        if let Some(t) = &mut self.tool {
            if t.id() == id {
                self.close_tool(cx, true);
                return;
            }
            if t.switch_to(id) {
                return;
            }
            self.close_tool(cx, true);
        }
        let Some(doc) = &self.doc else { return };
        if doc.session.is_none() {
            return;
        }
        self.tool = Some(match id {
            ToolId::Crop => Box::new(crop_tool::CropTool::new(&doc.params)),
            _ => Box::new(local_tool::LocalTool::new(id, &doc.params)),
        });
        self.eyedropper = false;
        self.zoom = Zoom::Fit;
        self.pan = egui::Vec2::ZERO;
    }

    /// Closes the open tool; `apply` commits a draft tool's edits as one
    /// history step.
    fn close_tool(&mut self, cx: &mut AppCx, apply: bool) {
        let Some(mut tool) = self.tool.take() else {
            return;
        };
        let Some(doc) = &self.doc else { return };
        if let Some(out) = tool.finish(apply, &doc.params) {
            self.apply(cx, out.params, out.change);
        }
    }

    fn apply_outcome(&mut self, cx: &mut AppCx, outcome: tool::Outcome) {
        self.apply(cx, outcome.params, outcome.change);
    }

    /// The geometry the open tool works in.
    fn tool_geometry(&self) -> Option<viberoom_services::engine::geometry::Geometry> {
        let doc = self.doc.as_ref()?;
        let session = doc.session.as_ref()?;
        let ignore = self.tool.as_ref().is_some_and(|t| t.ignore_crop());
        Some(viberoom_services::engine::geometry::resolve(
            session.source_size,
            session.orientation,
            &doc.params,
            ignore,
        ))
    }

    fn toggle_treatment(&mut self, cx: &mut AppCx) {
        let Some(doc) = &self.doc else { return };
        let mut params = doc.params.clone();
        let mut p = params.get::<Profile>();
        p.treatment = match p.treatment {
            Treatment::Color => Treatment::BlackWhite,
            Treatment::BlackWhite => Treatment::Color,
        };
        params.set::<Profile>(p);
        self.apply(
            cx,
            params,
            basic::Change {
                label: "Treatment".into(),
                immediate: true,
            },
        );
    }

    fn auto_tone(&mut self, cx: &mut AppCx) {
        use viberoom_services::engine::ops::{Exposure, ExposureParams, Tone};
        let Some(doc) = &self.doc else { return };
        let Some(session) = &doc.session else { return };
        let mut params = doc.params.clone();
        if let Some(a) = session.analysis.auto_tone(&params) {
            params.set::<Exposure>(ExposureParams { ev: a.exposure_ev });
            params.set::<Tone>(a.tone);
            self.apply(
                cx,
                params,
                basic::Change {
                    label: "Auto Tone".into(),
                    immediate: true,
                },
            );
        }
    }

    fn auto_wb(&mut self, cx: &mut AppCx) {
        use viberoom_services::engine::ops::WhiteBalance;
        let Some(doc) = &self.doc else { return };
        let Some(session) = &doc.session else { return };
        let mut params = doc.params.clone();
        if let Some(wb) = session.analysis.auto_wb() {
            params.set::<WhiteBalance>(wb);
            self.apply(
                cx,
                params,
                basic::Change {
                    label: "Auto White Balance".into(),
                    immediate: true,
                },
            );
        }
    }
}

fn exif_line(e: &repo::PhotoExif) -> String {
    let mut parts = Vec::new();
    if let Some(iso) = e.iso {
        parts.push(format!("ISO {iso}"));
    }
    if let Some(f) = e.focal_length {
        parts.push(format!("{f:.0} mm"));
    }
    if let Some(a) = e.aperture {
        parts.push(format!("f/{a:.1}"));
    }
    if let Some(s) = e.shutter {
        parts.push(if s > 0.0 && s < 1.0 {
            format!("1/{:.0} s", 1.0 / s)
        } else {
            format!("{s:.1} s")
        });
    }
    parts.join("  ·  ")
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

    fn on_enter(&mut self, cx: &mut AppCx) {
        // The Library may have rotated the open photo since it was loaded.
        let (Some(catalog), Some(doc)) = (&cx.catalog, &mut self.doc) else {
            return;
        };
        let Ok(Some(info)) = repo::photo_file_info(catalog.connection(), doc.photo) else {
            return;
        };
        if let Some(session) = &mut doc.session {
            let orientation =
                Orientation::from_exif(info.exif_orientation).rotated(info.user_orientation);
            if session.orientation != orientation {
                session.orientation = orientation;
                doc.last_key = None;
            }
        }
        doc.info = Some(info);
    }

    fn on_leave(&mut self, cx: &mut AppCx) {
        self.flush(cx);
        self.refresh_previews(cx);
    }

    fn left_panel(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        self.sync(ui.ctx(), cx);
        left::show(ui, self, cx);
    }

    fn right_panel(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        self.sync(ui.ctx(), cx);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading("Histogram");
                let clip_on = self.clip;
                let Some(doc) = &self.doc else {
                    ui.label("No photo selected.");
                    return;
                };
                if histogram::show(ui, doc.histogram.as_ref(), clip_on) {
                    self.clip = !self.clip;
                }
                if !doc.exif_line.is_empty() {
                    ui.small(&doc.exif_line);
                }
                ui.add_space(8.0);
                let mut strip = None;
                ui.horizontal(|ui| {
                    let open = self.tool.as_ref().map(|t| t.id());
                    for id in ToolId::ALL {
                        if ui
                            .add_enabled(
                                doc.session.is_some(),
                                egui::Button::selectable(open == Some(id), id.label()),
                            )
                            .on_hover_text(id.hint())
                            .clicked()
                        {
                            strip = Some(id);
                        }
                    }
                });
                if let Some(id) = strip {
                    self.open_tool(cx, id);
                }
                let geom = self.tool_geometry();
                let mut tool_out = None;
                let mut action = ToolAction::None;
                if let (Some(mut tool), Some(geom), Some(doc)) = (self.tool.take(), geom, &self.doc)
                {
                    ui.add_space(6.0);
                    (tool_out, action) = tool.options(ui, &doc.params, &geom);
                    self.tool = Some(tool);
                    ui.add_space(8.0);
                }
                if let Some(o) = tool_out {
                    self.apply_outcome(cx, o);
                }
                match action {
                    ToolAction::Apply => self.close_tool(cx, true),
                    ToolAction::Cancel => self.close_tool(cx, false),
                    ToolAction::None => {}
                }
                let Some(doc) = &self.doc else { return };
                ui.heading("Basic");

                let mut params = doc.params.clone();
                let info = basic::Info {
                    as_shot: doc.session.as_ref().and_then(|s| s.as_shot),
                    is_raw: doc.session.as_ref().is_none_or(|s| s.is_raw),
                    analysis: doc.session.as_ref().map(|s| &s.analysis),
                };
                let mut eyedropper = self.eyedropper;
                let mut change = basic::show(ui, &mut params, &info, &mut eyedropper);
                self.eyedropper = eyedropper;
                ui.add_space(10.0);
                let zoomed_in = self.zoom != Zoom::Fit;
                if let Some(c) = panels::show(ui, &mut params, &mut self.panels, zoomed_in) {
                    change = Some(c);
                }
                if let Some(change) = change {
                    self.apply(cx, params, change);
                }
            });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        self.sync(ui.ctx(), cx);
        canvas::toolbar(ui, self, cx);
    }

    fn center(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        self.sync(ui.ctx(), cx);
        canvas::show(ui, self, cx);
        copy_dialog::show(ui.ctx(), self, cx);
        preset_dialog::show(ui.ctx(), self, cx);
    }

    fn filmstrip(&mut self, ui: &mut egui::Ui, cx: &mut AppCx) {
        self.sync(ui.ctx(), cx);
        filmstrip::show(ui, cx);
    }

    fn undo(&mut self, cx: &mut AppCx) -> bool {
        self.walk(cx, false)
    }

    fn redo(&mut self, cx: &mut AppCx) -> bool {
        self.walk(cx, true)
    }
}
