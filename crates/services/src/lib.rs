//! `viberoom-services`: import, export, sidecar sync, auto tone/WB (plan
//! §4.1). Depends on everything below the UI line except UI crates
//! (principle 6, plan §2) — a CLI can use it exactly like the app does.
//!
//! `import` lands in M1 Phase A; `export` in M5, `sidecar` in M2.
//!
//! Re-exports below: `shell`, the module crates and `app` may depend only
//! on `services` (plan §4.2's dependency rules, enforced by
//! `xtask check-deps`), not on `catalog`/`preview`/`jobs` directly — so
//! anything from those crates the UI side needs to *name* (not just call
//! inherent methods on a value it's handed) is re-exported here instead of
//! read straight off `viberoom_catalog`/`viberoom_preview`/`viberoom_jobs`.

pub mod backup;
pub mod develop;
mod error;
pub mod export;
pub mod import;
pub mod presets;
pub mod preview;
pub mod rerender;
pub mod session;
pub mod sidecar;
pub use error::{Error, Result};

pub use viberoom_catalog::develop as catalog_develop;
pub use viberoom_catalog::{collections, command, criteria, repo};
pub use viberoom_engine as engine;
pub use viberoom_jobs::JobHandle;
pub use viberoom_preview::{
    DEFAULT_PARAMS_HASH, L1_BUDGET_PX, L2_BUDGET_PX, LEVEL_L1, LEVEL_L2, PreviewCache,
    l2_budget_px, set_preview_options,
};
