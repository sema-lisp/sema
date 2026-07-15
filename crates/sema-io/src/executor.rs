//! The production [`IoExecutor`] behind `sema_core`'s executor seam, backed by
//! THE process-wide tokio pool (see the crate module docs). It replaces the
//! reactor-less `ThreadPoolExecutor` (sema-vm `runtime/host.rs`): its async tier
//! spawns each `ExecutorDispatch::Async` future on the shared runtime (real
//! reactor — a `reqwest`/`tokio::process` future no longer panics), and its
//! blocking tier offloads each `ExecutorDispatch::Blocking` job onto the pool's
//! admission-controlled `spawn_blocking` tier.
//!
//! Concurrency is the point: an async dispatch runs on the async workers and
//! pins no thread while suspended, so N concurrent `http/get`s overlap without
//! burning one worker apiece — the ceiling the old blocking-tier `io_block_on`
//! workaround imposed. Drop/cancel of an async dispatch is drop-on-cancel: the
//! runtime fires the wait's `CancelHook` on the VM thread, the in-flight future
//! is dropped, no worker is burned.
//!
//! Per-runtime accounting and the bounded lease `shutdown(deadline)` contract
//! mirror `ThreadPoolExecutor` exactly: each interpreter attaches one
//! `ProcessExecutorLease`, and lease shutdown stops accepting that runtime's
//! work and drains only its in-flight jobs (bounded by the deadline) before
//! unregistering — never touching another interpreter's jobs. The shared pool's
//! worker threads are the static process pool and outlive every lease.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use sema_core::runtime::{
    BlockingDispatchClass, CompletionDelivery, ExecutorAttachError, ExecutorDispatch,
    ExecutorDriveReport, ExecutorLease, ExecutorShutdown, ExecutorSnapshot, ExecutorSubmission,
    ExecutorTerminal, IoExecutor, RunningSubmission, RuntimeId, SubmissionRejected,
    SubmitErrorKind,
};

/// Construct the process-wide I/O executor as an `Arc<dyn IoExecutor>` for a
/// runtime to attach to. Backed by THE static tokio pool; callers reach the pool
/// only through this factory (and the `io_*` wrappers), never by building their
/// own runtime.
pub fn process_executor() -> Arc<dyn IoExecutor> {
    Arc::new(ProcessIoExecutor::new())
}

/// The production I/O executor over THE process-wide tokio pool.
pub struct ProcessIoExecutor {
    pool: Arc<ProcessPool>,
}

impl ProcessIoExecutor {
    pub fn new() -> Self {
        // Installing the backend here (idempotent) means a runtime attaching to
        // this executor has the pool ready before its first submission.
        crate::install();
        Self {
            pool: Arc::new(ProcessPool::new()),
        }
    }
}

impl Default for ProcessIoExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl IoExecutor for ProcessIoExecutor {
    fn attach_runtime(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError> {
        self.pool.register_runtime(runtime_id)?;
        Ok(Arc::new(ProcessExecutorLease {
            runtime_id,
            pool: Arc::clone(&self.pool),
        }))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        self.pool.snapshot()
    }
}

struct ProcessExecutorLease {
    runtime_id: RuntimeId,
    pool: Arc<ProcessPool>,
}

impl ExecutorLease for ProcessExecutorLease {
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        self.pool.submit(self.runtime_id, submission)
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        self.pool.snapshot()
    }

    fn shutdown(&self, deadline: Instant) -> ExecutorShutdown {
        self.pool.shutdown_runtime(self.runtime_id, deadline)
    }
}

/// Per-runtime registration slot: whether the lease still accepts work, and how
/// many of its jobs are in flight (submitted-but-not-yet-terminal). Lease
/// shutdown drains on `running` reaching zero.
struct RuntimeSlot {
    running: usize,
    accepting: bool,
}

struct ProcessPool {
    metrics: Metrics,
    /// Per-runtime registration + in-flight counts. A single `Condvar` (`idle`)
    /// wakes a draining `shutdown_runtime` when any runtime's `running` hits 0.
    runtimes: Mutex<HashMap<RuntimeId, RuntimeSlot>>,
    idle: Condvar,
}

impl ProcessPool {
    fn new() -> Self {
        Self {
            metrics: Metrics::default(),
            runtimes: Mutex::new(HashMap::new()),
            idle: Condvar::new(),
        }
    }

    fn register_runtime(&self, runtime_id: RuntimeId) -> Result<(), ExecutorAttachError> {
        let mut map = self.runtimes.lock().expect("pool runtimes lock");
        if map.contains_key(&runtime_id) {
            return Err(ExecutorAttachError::DuplicateRuntime { runtime_id });
        }
        map.insert(
            runtime_id,
            RuntimeSlot {
                running: 0,
                accepting: true,
            },
        );
        Ok(())
    }

    fn submit(
        self: &Arc<Self>,
        runtime_id: RuntimeId,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        let operation_id = submission.operation_id();
        {
            let mut map = self.runtimes.lock().expect("pool runtimes lock");
            match map.get_mut(&runtime_id) {
                None => return Err(submission.reject(SubmitErrorKind::RuntimeDetached)),
                Some(slot) if !slot.accepting => {
                    return Err(submission.reject(SubmitErrorKind::ShuttingDown));
                }
                // Count the job in flight for this runtime BEFORE arming, so a
                // concurrent `shutdown_runtime` drain observes it.
                Some(slot) => slot.running += 1,
            }
        }
        self.metrics.queued.fetch_add(1, Ordering::Relaxed);

        // Arm the dispatch (the admission linearization point) and route it onto
        // the matching pool tier. The completion callback records the terminal
        // and releases the runtime's in-flight count.
        match submission.into_dispatch() {
            ExecutorDispatch::Blocking(blocking) => {
                let quarantined = blocking.class() == BlockingDispatchClass::QuarantinedBounded;
                let pool = Arc::clone(self);
                crate::io_spawn_blocking(move || {
                    pool.job_started(quarantined);
                    let report = blocking.run();
                    pool.job_finished(runtime_id, quarantined, report);
                });
            }
            ExecutorDispatch::Async(async_dispatch) => {
                let future = async_dispatch.into_future();
                let pool = Arc::clone(self);
                // Async dispatches are interruptible-class; drop the returned
                // abort hook — cancellation is delivered through the wait's
                // `CancelHook` on the VM thread, which drops the in-flight
                // future, so the spawned task self-completes.
                let _abort = crate::io_spawn(async move {
                    pool.job_started(false);
                    let report = future.await;
                    pool.job_finished(runtime_id, false, report);
                });
            }
        }
        Ok(RunningSubmission::new(operation_id))
    }

    /// A dispatch began executing on its tier: move it from the queued gauge to
    /// the running gauge (interruptible vs quarantined).
    fn job_started(&self, quarantined: bool) {
        self.metrics.queued.fetch_sub(1, Ordering::Relaxed);
        let running = if quarantined {
            &self.metrics.running_quarantined
        } else {
            &self.metrics.running_interruptible
        };
        running.fetch_add(1, Ordering::Relaxed);
    }

    /// A dispatch reached its terminal: drop the running gauge, record the
    /// terminal, and release the runtime's in-flight count (waking a draining
    /// `shutdown_runtime`).
    fn job_finished(&self, runtime_id: RuntimeId, quarantined: bool, report: ExecutorDriveReport) {
        let running = if quarantined {
            &self.metrics.running_quarantined
        } else {
            &self.metrics.running_interruptible
        };
        running.fetch_sub(1, Ordering::Relaxed);
        record_terminal(&self.metrics, report);

        let mut map = self.runtimes.lock().expect("pool runtimes lock");
        if let Some(slot) = map.get_mut(&runtime_id) {
            slot.running -= 1;
            if slot.running == 0 {
                self.idle.notify_all();
            }
        }
    }

    fn shutdown_runtime(&self, runtime_id: RuntimeId, deadline: Instant) -> ExecutorShutdown {
        let mut map = self.runtimes.lock().expect("pool runtimes lock");
        if let Some(slot) = map.get_mut(&runtime_id) {
            // Stop accepting new work on this lease; sibling runtimes are
            // untouched (their slots keep `accepting = true`).
            slot.accepting = false;
        }
        loop {
            let running = map.get(&runtime_id).map(|s| s.running).unwrap_or(0);
            if running == 0 {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (guard, timeout) = self
                .idle
                .wait_timeout(map, deadline - now)
                .expect("pool idle wait");
            map = guard;
            if timeout.timed_out() {
                break;
            }
        }
        let drained = map.get(&runtime_id).map(|s| s.running).unwrap_or(0) == 0;
        // Unregister only once drained (or the deadline passed): a still-running
        // job whose entry is gone simply finds no slot in `job_finished` and
        // delivers to its (possibly closed) inbox — bounded, never blocking.
        map.remove(&runtime_id);
        drop(map);
        let snapshot = self.snapshot();
        if drained {
            ExecutorShutdown::Drained(snapshot)
        } else {
            ExecutorShutdown::DeadlineExceeded(snapshot)
        }
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        self.metrics.snapshot()
    }
}

#[derive(Default)]
struct Metrics {
    queued: AtomicUsize,
    running_interruptible: AtomicUsize,
    running_quarantined: AtomicUsize,
    completed: AtomicUsize,
    cancelled: AtomicUsize,
    panicked: AtomicUsize,
    undeliverable: AtomicUsize,
}

impl Metrics {
    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot {
            queued: self.queued.load(Ordering::Relaxed),
            running_interruptible: self.running_interruptible.load(Ordering::Relaxed),
            running_quarantined: self.running_quarantined.load(Ordering::Relaxed),
            completed: self.completed.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            panicked: self.panicked.load(Ordering::Relaxed),
            undeliverable: self.undeliverable.load(Ordering::Relaxed),
        }
    }
}

fn record_terminal(metrics: &Metrics, report: ExecutorDriveReport) {
    match report.terminal {
        ExecutorTerminal::Completed => {
            metrics.completed.fetch_add(1, Ordering::Relaxed);
        }
        ExecutorTerminal::Cancelled => {
            metrics.cancelled.fetch_add(1, Ordering::Relaxed);
        }
        ExecutorTerminal::WorkerPanic => {
            metrics.panicked.fetch_add(1, Ordering::Relaxed);
        }
    }
    if report.delivery == CompletionDelivery::InboxClosed {
        metrics.undeliverable.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod process_executor_tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use sema_core::cycle::GcEdge;
    use sema_core::runtime::{
        CancelDisposition, CancelHook, CancelHookError, CompletionDecoder, CompletionDelivery,
        CompletionKind, CompletionRegistrar, CompletionSender, DecodedCompletion,
        ExternalCompletion, ExternalFailure, InterruptibleResource, IoExecutor, NativeCallContext,
        PreparedExternalOperation, SendPayload, Trace,
    };
    use sema_core::Value;

    use super::ProcessIoExecutor;

    struct ChannelSender(Mutex<std::sync::mpsc::Sender<ExternalCompletion>>);

    impl CompletionSender for ChannelSender {
        fn send(&self, completion: ExternalCompletion) -> CompletionDelivery {
            self.0
                .lock()
                .unwrap()
                .send(completion)
                .map(|()| CompletionDelivery::Delivered)
                .unwrap_or(CompletionDelivery::InboxClosed)
        }
    }

    struct NilDecoder;
    impl Trace for NilDecoder {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }
    impl CompletionDecoder for NilDecoder {
        fn decode(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            _result: Result<SendPayload, ExternalFailure>,
        ) -> DecodedCompletion {
            Ok(Value::nil())
        }
    }

    struct NoopHook;
    impl Trace for NoopHook {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }
    impl CancelHook for NoopHook {
        fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
            Ok(CancelDisposition::Reaped)
        }
        fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
            Ok(CancelDisposition::Reaped)
        }
    }

    fn blocking_sleep_op(ms: u64) -> PreparedExternalOperation {
        PreparedExternalOperation::interruptible_blocking(
            CompletionKind::try_from_raw(1).unwrap(),
            Box::new(NilDecoder),
            InterruptibleResource::new("sleep", Box::new(NoopHook)),
            move || {
                std::thread::sleep(Duration::from_millis(ms));
                Ok(Box::new(()) as SendPayload)
            },
        )
    }

    /// Two blocking jobs submitted together overlap on the pool's blocking tier:
    /// both completions arrive in ~one sleep, not two.
    #[test]
    fn two_blocking_jobs_overlap() {
        let (tx, rx) = std::sync::mpsc::channel();
        let registrar_sender = Arc::new(ChannelSender(Mutex::new(tx)));
        let (runtime_id, registrar, _issuers) =
            CompletionRegistrar::register(registrar_sender).unwrap();

        let executor = ProcessIoExecutor::new();
        let lease = executor.attach_runtime(runtime_id).unwrap();

        let start = Instant::now();
        for _ in 0..2 {
            let identity = registrar
                .issue_identity(CompletionKind::try_from_raw(1).unwrap())
                .unwrap();
            let (_runtime, submission) = registrar
                .bind(identity, blocking_sleep_op(200))
                .unwrap()
                .split();
            assert!(lease.submit(submission).is_ok(), "submission admitted");
        }

        rx.recv_timeout(Duration::from_secs(2))
            .expect("first completion");
        rx.recv_timeout(Duration::from_secs(2))
            .expect("second completion");
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(400),
            "blocking jobs must overlap on the pool; took {elapsed:?}"
        );

        let shutdown_start = Instant::now();
        let _ = lease.shutdown(Instant::now());
        assert!(shutdown_start.elapsed() < Duration::from_millis(200));
        drop(lease);
        drop(executor);
    }

    /// Dropping the executor and its lease WITHOUT calling `shutdown` — while a
    /// job is still in flight — must return promptly (bounded). The shared pool
    /// outlives the lease, so the in-flight job keeps running and delivers into a
    /// now-closed inbox; nothing blocks on it.
    #[test]
    fn drop_without_shutdown_is_bounded() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let registrar_sender = Arc::new(ChannelSender(Mutex::new(tx)));
        let (runtime_id, registrar, _issuers) =
            CompletionRegistrar::register(registrar_sender).unwrap();
        let executor = ProcessIoExecutor::new();
        let lease = executor.attach_runtime(runtime_id).unwrap();
        let identity = registrar
            .issue_identity(CompletionKind::try_from_raw(1).unwrap())
            .unwrap();
        let (_runtime, submission) = registrar
            .bind(identity, blocking_sleep_op(80))
            .unwrap()
            .split();
        assert!(lease.submit(submission).is_ok(), "submission admitted");

        let start = Instant::now();
        drop(lease);
        drop(executor);
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "dropping the executor must not block on the in-flight job"
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    /// The `Runtime` construction path retains only the executor's LEASE. Dropping
    /// the `ProcessIoExecutor` struct while a lease is still alive must keep the
    /// lease usable so later submissions still run — the pool is process-wide and
    /// the lease holds its own `Arc<ProcessPool>`.
    #[test]
    fn executor_struct_dropped_before_lease_still_accepts() {
        let (tx, rx) = std::sync::mpsc::channel();
        let registrar_sender = Arc::new(ChannelSender(Mutex::new(tx)));
        let (runtime_id, registrar, _issuers) =
            CompletionRegistrar::register(registrar_sender).unwrap();

        let executor = ProcessIoExecutor::new();
        let lease = executor.attach_runtime(runtime_id).unwrap();
        drop(executor);

        let identity = registrar
            .issue_identity(CompletionKind::try_from_raw(1).unwrap())
            .unwrap();
        let (_runtime, submission) = registrar
            .bind(identity, blocking_sleep_op(10))
            .unwrap()
            .split();
        assert!(
            lease.submit(submission).is_ok(),
            "submission on a live lease must succeed after the executor struct dropped"
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("completion delivered after executor struct dropped");
        drop(lease);
    }

    /// Shutdown while a job is in flight is bounded by the deadline and never
    /// hangs.
    #[test]
    fn shutdown_is_bounded_with_in_flight_job() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let registrar_sender = Arc::new(ChannelSender(Mutex::new(tx)));
        let (runtime_id, registrar, _issuers) =
            CompletionRegistrar::register(registrar_sender).unwrap();
        let executor = ProcessIoExecutor::new();
        let lease = executor.attach_runtime(runtime_id).unwrap();
        let identity = registrar
            .issue_identity(CompletionKind::try_from_raw(1).unwrap())
            .unwrap();
        let (_runtime, submission) = registrar
            .bind(identity, blocking_sleep_op(150))
            .unwrap()
            .split();
        assert!(lease.submit(submission).is_ok(), "submission admitted");

        let start = Instant::now();
        let _ = lease.shutdown(Instant::now() + Duration::from_millis(20));
        assert!(
            start.elapsed() < Duration::from_millis(120),
            "shutdown must honor the deadline, not block for the full job"
        );
        drop(lease);
        drop(executor);
    }

    /// Duplicate attachment for the same runtime id returns `DuplicateRuntime`.
    #[test]
    fn duplicate_attach_is_rejected() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let registrar_sender = Arc::new(ChannelSender(Mutex::new(tx)));
        let (runtime_id, _registrar, _issuers) =
            CompletionRegistrar::register(registrar_sender).unwrap();
        let executor = ProcessIoExecutor::new();
        let _lease = executor.attach_runtime(runtime_id).unwrap();
        assert!(
            executor.attach_runtime(runtime_id).is_err(),
            "second attach of the same runtime id must be rejected"
        );
    }
}
