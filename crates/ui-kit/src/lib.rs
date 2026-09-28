//! `archroom-ui`: reusable egui widgets and the app theme. Depends only on
//! `egui` and `archroom-core` (plan §4.2) — no catalog, no engine.

mod slider;
mod theme;

pub use slider::LrSlider;
pub use theme::{ACCENT, CanvasBackground, apply as apply_theme};
