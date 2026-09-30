//! `archroom-engine`: the `Op` registry, edit params, raw-prep color glue
//! and (from Phase B) the GPU pipeline (plan §4.1/§6).

pub mod analysis;
pub mod cache;
pub mod error;
pub mod geometry;
pub mod gpu;
pub mod op;
pub mod ops;
pub mod orientation;
pub mod params;
pub mod pipeline;
pub mod rawprep;
pub mod render_loop;
pub mod tone;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::needless_range_loop)]
mod pipeline_tests;

pub use op::{DynOp, Op, ParamSpec, Registry, SettingsGroup, Stage};
pub use params::{CURRENT_PROCESS_VERSION, EditParams};

pub use error::{Error, Result};
pub use orientation::Orientation;
