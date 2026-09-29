//! `archroom-ui`: reusable egui widgets and the app theme. Depends only on
//! `egui` and `archroom-core` (plan §4.2) — no catalog, no engine.

mod curve_editor;
mod slider;
mod theme;

pub use curve_editor::{CurveEdit, curve_editor};
pub use slider::LrSlider;
pub use theme::{ACCENT, CanvasBackground, apply as apply_theme};
