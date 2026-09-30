//! Wires up `tracing`. Binaries call [`init`] once at startup; libraries
//! only ever emit spans and events.

use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

use crate::error::{Error, Result};

/// `~/.local/state/viberoom/logs/` (plan §4.7). File logging (rolled by
/// day) is wired up once a binary needs it; for now this just gives callers
/// a stable path to create.
pub fn log_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "viberoom")
        .ok_or_else(|| Error::Settings("no home directory".into()))?;
    // `directories` maps `state_dir()` to `~/.local/state/<app>` on Linux;
    // it's `None` on platforms without a distinct state dir (macOS, Windows).
    let base: PathBuf = dirs
        .state_dir()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| dirs.data_dir().join("state"));
    Ok(base.join("logs"))
}

/// Installs a stderr-logging subscriber. `RUST_LOG` overrides the default
/// filter (`info`, and `warn` for noisy third-party crates).
pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}
