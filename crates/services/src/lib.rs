//! `archroom-services`: import, export, sidecar sync, auto tone/WB (plan
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
//! read straight off `archroom_catalog`/`archroom_preview`/`archroom_jobs`.

mod error;
pub mod import;
pub mod preview;

pub use error::{Error, Result};

pub use archroom_catalog::{command, repo};
pub use archroom_jobs::JobHandle;
pub use archroom_preview::{
    DEFAULT_PARAMS_HASH, L1_BUDGET_PX, L2_BUDGET_PX, LEVEL_L1, LEVEL_L2, PreviewCache,
};
