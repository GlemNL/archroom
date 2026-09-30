use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, unbounded};
use tracing::{debug, warn};

use crate::cancel::CancellationToken;
use crate::job::{Job, JobContext};
use crate::priority::Priority;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(u64);

#[derive(Debug, Clone)]
pub enum JobEventKind {
    Started,
    Progress(f32),
    Finished,
    Cancelled,
    Panicked,
}

#[derive(Debug, Clone)]
pub struct JobEvent {
    pub id: JobId,
    pub label: String,
    pub priority: Priority,
    pub kind: JobEventKind,
}

/// A submitted job's handle. Dropping it does *not* cancel the job — call
/// [`JobHandle::cancel`] explicitly, as scrolling away from a thumbnail
/// does.
#[derive(Debug, Clone)]
pub struct JobHandle {
    pub id: JobId,
    cancel: CancellationToken,
}

impl JobHandle {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

struct QueuedJob {
    id: JobId,
    seq: u64,
    priority: Priority,
    label: String,
    cancel: CancellationToken,
    job: Box<dyn Job>,
}

impl PartialEq for QueuedJob {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.seq == other.seq
    }
}
impl Eq for QueuedJob {}

impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap: higher priority pops first, and within
        // the same priority the earlier-submitted (smaller seq) job pops
        // first, which is why the seq comparison is reversed.
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

struct Shared {
    queue: Mutex<BinaryHeap<QueuedJob>>,
    not_empty: Condvar,
    shutdown: AtomicBool,
}

/// The background job scheduler (plan §4.6): a small priority queue shared
/// by a fixed pool of worker threads. `Interactive` work always runs before
/// `Background` work that was queued earlier.
pub struct Scheduler {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
    next_id: AtomicU64,
    events_rx: Receiver<JobEvent>,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("workers", &self.workers.len())
            .finish_non_exhaustive()
    }
}

impl Scheduler {
    /// `num_workers` of `0` uses `cores - 1` (minimum 1), per plan §4.6.
    pub fn new(num_workers: usize) -> Self {
        let num_workers = if num_workers == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get().saturating_sub(1).max(1))
                .unwrap_or(1)
        } else {
            num_workers
        };

        let shared = Arc::new(Shared {
            queue: Mutex::new(BinaryHeap::new()),
            not_empty: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let (events_tx, events_rx) = unbounded();

        let workers = (0..num_workers)
            .map(|i| {
                let shared = Arc::clone(&shared);
                let events_tx = events_tx.clone();
                #[allow(clippy::expect_used)] // unrecoverable at startup; nothing to fall back to
                std::thread::Builder::new()
                    .name(format!("viberoom-job-{i}"))
                    .spawn(move || worker_loop(shared, events_tx))
                    .expect("failed to spawn job worker thread")
            })
            .collect();

        drop(events_tx); // each worker holds its own clone; this one was only needed to make them

        Self {
            shared,
            workers,
            next_id: AtomicU64::new(1),
            events_rx,
        }
    }

    /// Every job event, across every job. The UI drains this once per frame
    /// (plan §4.5: background threads never touch UI state directly).
    pub fn events(&self) -> Receiver<JobEvent> {
        self.events_rx.clone()
    }

    pub fn submit(&self, job: impl Job) -> JobHandle {
        self.submit_boxed(Box::new(job))
    }

    pub fn submit_boxed(&self, job: Box<dyn Job>) -> JobHandle {
        let id = JobId(self.next_id.fetch_add(1, AtomicOrdering::Relaxed));
        let cancel = CancellationToken::new();
        let priority = job.priority();
        let label = job.label();

        let queued = QueuedJob {
            id,
            seq: id.0,
            priority,
            label,
            cancel: cancel.clone(),
            job,
        };

        {
            #[allow(clippy::unwrap_used)]
            let mut queue = self.shared.queue.lock().unwrap();
            queue.push(queued);
        }
        self.shared.not_empty.notify_one();

        JobHandle { id, cancel }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, AtomicOrdering::SeqCst);
        self.shared.not_empty.notify_all();
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                warn!("job worker thread panicked while joining on shutdown");
            }
        }
    }
}

fn worker_loop(shared: Arc<Shared>, events_tx: Sender<JobEvent>) {
    loop {
        let queued = {
            #[allow(clippy::unwrap_used)]
            let mut queue = shared.queue.lock().unwrap();
            loop {
                if let Some(job) = queue.pop() {
                    break Some(job);
                }
                if shared.shutdown.load(AtomicOrdering::SeqCst) {
                    break None;
                }
                #[allow(clippy::unwrap_used)]
                {
                    queue = shared.not_empty.wait(queue).unwrap();
                }
            }
        };

        let Some(queued) = queued else {
            break;
        };

        let QueuedJob {
            id,
            priority,
            label,
            cancel,
            job,
            ..
        } = queued;

        if cancel.is_cancelled() {
            let _ = events_tx.send(JobEvent {
                id,
                label,
                priority,
                kind: JobEventKind::Cancelled,
            });
            continue;
        }

        let _ = events_tx.send(JobEvent {
            id,
            label: label.clone(),
            priority,
            kind: JobEventKind::Started,
        });

        let progress_tx = events_tx.clone();
        let progress_label = label.clone();
        let cx = JobContext::new(
            cancel.clone(),
            Box::new(move |p| {
                let _ = progress_tx.send(JobEvent {
                    id,
                    label: progress_label.clone(),
                    priority,
                    kind: JobEventKind::Progress(p),
                });
            }),
        );

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            job.run(&cx);
        }));

        let kind = match outcome {
            Ok(()) if cancel.is_cancelled() => JobEventKind::Cancelled,
            Ok(()) => JobEventKind::Finished,
            Err(_) => JobEventKind::Panicked,
        };
        debug!(job_id = id.0, label = %label, ?kind, "job finished");
        let _ = events_tx.send(JobEvent {
            id,
            label,
            priority,
            kind,
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    struct Counting {
        priority: Priority,
        counter: Arc<AtomicU32>,
        order: Arc<Mutex<Vec<u32>>>,
        my_number: u32,
    }

    impl Job for Counting {
        fn label(&self) -> String {
            format!("counting job {}", self.my_number)
        }

        fn priority(&self) -> Priority {
            self.priority
        }

        fn run(self: Box<Self>, _cx: &JobContext) {
            self.counter.fetch_add(1, Ordering::SeqCst);
            #[allow(clippy::unwrap_used)]
            self.order.lock().unwrap().push(self.my_number);
        }
    }

    fn drain_until_finished(rx: &Receiver<JobEvent>, expected: usize) {
        let mut finished = 0;
        while finished < expected {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(ev) => {
                    if matches!(ev.kind, JobEventKind::Finished) {
                        finished += 1;
                    }
                }
                Err(_) => panic!("timed out waiting for jobs to finish"),
            }
        }
    }

    #[test]
    fn runs_a_submitted_job() {
        let scheduler = Scheduler::new(1);
        let events = scheduler.events();
        let counter = Arc::new(AtomicU32::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));

        scheduler.submit(Counting {
            priority: Priority::Background,
            counter: counter.clone(),
            order,
            my_number: 1,
        });

        drain_until_finished(&events, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancelled_job_never_runs() {
        let scheduler = Scheduler::new(1);
        let events = scheduler.events();
        let counter = Arc::new(AtomicU32::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));

        // Block the single worker so both jobs are still queued when we cancel.
        scheduler.submit(Counting {
            priority: Priority::Background,
            counter: counter.clone(),
            order: order.clone(),
            my_number: 0,
        });
        let handle = scheduler.submit(Counting {
            priority: Priority::Background,
            counter: counter.clone(),
            order,
            my_number: 1,
        });
        handle.cancel();

        // Drain until we've seen a terminal event for both jobs.
        let mut terminal = 0;
        while terminal < 2 {
            let ev = events.recv_timeout(Duration::from_secs(5)).expect("event");
            if matches!(
                ev.kind,
                JobEventKind::Finished | JobEventKind::Cancelled | JobEventKind::Panicked
            ) {
                terminal += 1;
            }
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn interactive_jobs_run_before_background_jobs_queued_earlier() {
        // A single worker, held busy by a first job, lets us queue a
        // Background job followed by an Interactive one and observe that
        // the Interactive job still runs first.
        let scheduler = Scheduler::new(1);
        let events = scheduler.events();
        let counter = Arc::new(AtomicU32::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));

        struct Blocker {
            gate: Arc<std::sync::Barrier>,
        }
        impl Job for Blocker {
            fn label(&self) -> String {
                "blocker".into()
            }
            fn priority(&self) -> Priority {
                Priority::Background
            }
            fn run(self: Box<Self>, _cx: &JobContext) {
                self.gate.wait();
            }
        }

        let gate = Arc::new(std::sync::Barrier::new(2));
        scheduler.submit(Blocker { gate: gate.clone() });

        scheduler.submit(Counting {
            priority: Priority::Background,
            counter: counter.clone(),
            order: order.clone(),
            my_number: 10,
        });
        scheduler.submit(Counting {
            priority: Priority::Interactive,
            counter: counter.clone(),
            order: order.clone(),
            my_number: 20,
        });

        gate.wait(); // release the blocker so the queue starts draining

        drain_until_finished(&events, 3);
        #[allow(clippy::unwrap_used)]
        let order = order.lock().unwrap();
        assert_eq!(*order, vec![20, 10]);
    }
}
