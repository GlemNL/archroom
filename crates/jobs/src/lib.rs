//! `viberoom-jobs`: the background job scheduler (plan §4.6). The UI thread
//! never blocks; everything expensive — decoding, DB writes, preview
//! generation, export — runs here as a `Job`.

mod cancel;
mod job;
mod priority;
mod scheduler;

pub use cancel::CancellationToken;
pub use job::{Job, JobContext};
pub use priority::Priority;
pub use scheduler::{JobEvent, JobEventKind, JobHandle, JobId, Scheduler};
