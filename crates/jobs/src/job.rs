use crate::cancel::CancellationToken;

/// Handed to a running job so it can check for cancellation and report
/// progress. Constructed by the scheduler; jobs never build one themselves.
pub struct JobContext {
    cancel: CancellationToken,
    report_progress: Box<dyn Fn(f32) + Send + Sync>,
}

impl std::fmt::Debug for JobContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobContext")
            .field("cancel", &self.cancel)
            .finish_non_exhaustive()
    }
}

impl JobContext {
    pub(crate) fn new(
        cancel: CancellationToken,
        report_progress: Box<dyn Fn(f32) + Send + Sync>,
    ) -> Self {
        Self {
            cancel,
            report_progress,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// `progress` is a fraction in `0.0..=1.0`. Jobs that don't know their
    /// total up front (a folder scan) can just skip calling this.
    pub fn report_progress(&self, progress: f32) {
        (self.report_progress)(progress.clamp(0.0, 1.0));
    }
}

/// Background work (plan §4.3, extension point "Background work"). One
/// `.rs` type per kind of job: a run function, a priority and cancellation
/// checks.
pub trait Job: Send + 'static {
    /// A short human-readable label for the activity indicator, e.g.
    /// `"Importing 214 photos"`.
    fn label(&self) -> String;

    fn priority(&self) -> crate::Priority;

    fn run(self: Box<Self>, cx: &JobContext);
}
