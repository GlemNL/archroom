//! `archroom-shell`: the contract between the `app` binary and the module
//! crates (`Module`, `AppCx`, `ModuleRegistry`). Split out from `services`
//! (which must stay UI-agnostic, plan principle 6) and from `ui-kit` (which
//! must stay a dependency-light widget crate, plan §4.2) because `Module`
//! needs both `egui::Ui` and `AppCx` — and from the `app` binary itself,
//! because `module-library`/`module-develop` need to implement `Module`
//! without depending on the binary that depends on them.

mod appcx;
mod module;
mod registry;
mod selection;
mod undo;
mod view_input;

pub use appcx::{AppCx, RenderStateHandle};
pub use module::{Action, Module, ModuleId};
pub use registry::ModuleRegistry;
pub use selection::{LibrarySource, Selection};
pub use undo::UndoStack;
pub use view_input::{click_zoom_picker, wheel_scrolls_horizontally};
