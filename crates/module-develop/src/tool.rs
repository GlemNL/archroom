//! The canvas-tool framework (plan v0.2.0 §4.1): Crop and the local tools
//! (red eye, gradient, brush) implement one trait, so the Develop module
//! holds a single `Option<Box<dyn CanvasTool>>` and the tools exclude each
//! other. Crop edits the document directly (each gesture is a history
//! step); the local tools edit a *draft* that renders live and becomes one
//! history step on Apply.

use egui::{Key, Rect, Response, Ui};
use viberoom_services::engine::EditParams;
use viberoom_services::engine::geometry::Geometry;

use crate::basic::Change;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolId {
    Crop,
    RedEye,
    Gradient,
    Brush,
}

impl ToolId {
    pub const ALL: [ToolId; 4] = [
        ToolId::Crop,
        ToolId::RedEye,
        ToolId::Gradient,
        ToolId::Brush,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolId::Crop => "Crop",
            ToolId::RedEye => "Red Eye",
            ToolId::Gradient => "Gradient",
            ToolId::Brush => "Brush",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ToolId::Crop => "Crop & straighten (R)",
            ToolId::RedEye => "Red eye / pet eye (Shift+R)",
            ToolId::Gradient => "Linear gradient (M)",
            ToolId::Brush => "Brush zone (K)",
        }
    }
}

/// A parameter change produced by a tool.
#[derive(Debug)]
pub struct Outcome {
    pub params: EditParams,
    pub change: Change,
}

impl Outcome {
    pub fn new(label: &str, params: EditParams, immediate: bool) -> Option<Self> {
        Some(Self {
            params,
            change: Change {
                label: label.to_string(),
                immediate,
            },
        })
    }
}

/// What the options panel asks the module to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolAction {
    #[default]
    None,
    /// Commit (for draft tools) and close the tool.
    Apply,
    /// Close, discarding any draft.
    Cancel,
}

pub trait CanvasTool: std::fmt::Debug {
    fn id(&self) -> ToolId;

    /// Render the whole (straightened) canvas instead of the crop.
    fn ignore_crop(&self) -> bool {
        false
    }

    /// The local adjustment whose mask the overlay should tint.
    fn mask_overlay(&self) -> Option<String> {
        None
    }

    /// What to render: the document's params with the tool's draft on top.
    fn display_params(&self, doc: &EditParams) -> EditParams {
        doc.clone()
    }

    /// Draws the overlay and handles the pointer. `img` is where the
    /// rendered output is drawn; `geom` maps between output and source.
    fn canvas(
        &mut self,
        ui: &Ui,
        img: Rect,
        resp: &Response,
        geom: &Geometry,
        params: &EditParams,
    ) -> Option<Outcome>;

    /// The tool's block in the right panel.
    fn options(
        &mut self,
        ui: &mut Ui,
        params: &EditParams,
        geom: &Geometry,
    ) -> (Option<Outcome>, ToolAction);

    /// The canvas asked to apply and close the tool (e.g. a double click);
    /// reading it clears the request.
    fn take_close_request(&mut self) -> bool {
        false
    }

    /// Tool-specific keys: `O`, `X`, `'`, `[`, `]`.
    fn on_key(
        &mut self,
        key: Key,
        shift: bool,
        params: &EditParams,
        geom: &Geometry,
    ) -> Option<Outcome>;

    /// The tool is closing. `apply` is false for Esc/Cancel. Draft tools
    /// return the params to commit.
    fn finish(&mut self, apply: bool, doc: &EditParams) -> Option<Outcome>;

    /// Asks the tool to become `to` without closing; true when it can
    /// (the local tools share a draft across their three modes).
    fn switch_to(&mut self, _to: ToolId) -> bool {
        false
    }
}
