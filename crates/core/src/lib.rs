//! `archroom-core`: IDs, errors, settings, the event bus and tracing setup.
//! Depends on nothing else in the workspace (plan §4.2).

pub mod error;
pub mod events;
pub mod ids;
pub mod settings;
pub mod tracing_setup;

pub use error::{Error, Result};
