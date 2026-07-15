//! Minimal production host adapters for driving the runtime from an interpreter.
//!
//! `MonotonicClock` is the real wall-clock source. `NullExecutor` accepts no
//! external work and is suitable for evaluating purely synchronous roots (which
//! never submit I/O). `ThreadPoolExecutor` is the real I/O executor: it runs
//! each submitted `Send` job on a worker thread so genuinely-blocking external
//! operations overlap instead of serializing on the VM thread.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::JoinHandle;
use std::time::Instant;

use sema_core::runtime::{
    BlockingDispatchClass, CompletionDelivery, ExecutorAttachError, ExecutorDispatch,
    ExecutorDriveReport, ExecutorLease, ExecutorShutdown, ExecutorSnapshot, ExecutorSubmission,
    ExecutorTerminal, IoExecutor, RunningSubmission, RuntimeId, SubmissionRejected,
    SubmitErrorKind,
};

use super::RuntimeClock;

/// Production monotonic clock backed by `Instant::now()`.
pub struct MonotonicClock;

impl RuntimeClock for MonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Executor that accepts no external work: any submission is rejected. A runtime
/// built with it can drive synchronous roots (which never submit I/O) but cannot
/// service real external operations.
pub struct NullExecutor;

struct NullLease;

impl IoExecutor for NullExecutor {
    fn attach_runtime(
        &self,
        _runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError> {
        Ok(Arc::new(NullLease))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }
}

impl ExecutorLease for NullLease {
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        Err(submission.reject(SubmitErrorKind::Capacity))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }

    fn shutdown(&self, _deadline: Instant) -> ExecutorShutdown {
        ExecutorShutdown::Drained(ExecutorSnapshot::default())
    }
}

/// Real I/O executor: a small fixed pool of worker threads that run each
/// submitted `Send` dispatch to completion off the VM thread. The send-only
/// boundary is upheld by the runtime — a submitted `ExecutorDispatch` carries no
/// `Rc`/`Value` (only the boxed `Send` job + the `Send` completion sink); the
/// worker delivers the raw completion into the runtime inbox and the VM thread
/// decodes it. Concurrency is genuine: two blocking jobs land on two workers and
/// overlap, so `async/spawn`ed blocking operations no longer serialize.
///
/// Worker liveness is tied to the channel's senders via RAII, not to this
/// struct's `Drop`. The executor and every lease it hands out each hold their
/// OWN `Sender` clone; the workers hold the shared `Receiver`. The channel
/// disconnects — and idle workers exit — exactly when the executor AND all of
/// its leases have dropped (every `Sender` clone gone), never merely because the
/// top-level `ThreadPoolExecutor` struct was dropped while a lease is still
/// alive. That is what lets `Runtime::new` retain only the lease (dropping the
/// executor struct immediately) and keep submitting successfully.
pub struct ThreadPoolExecutor {
    inner: Arc<PoolInner>,
    /// The executor's own `Sender` clone. Held only to keep the worker channel
    /// connected for as long as the executor lives; leases carry their own
    /// clones for actual submission.
    tx: Sender<ExecutorDispatch>,
}

impl ThreadPoolExecutor {
    /// Build a pool sized to the machine (clamped to `[2, 8]` so blocking jobs
    /// always have at least two workers to overlap on, without spawning an
    /// unbounded number of threads).
    pub fn new() -> Self {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 8);
        Self::with_workers(workers)
    }

    pub fn with_workers(workers: usize) -> Self {
        let workers = workers.max(1);
        let (tx, rx) = mpsc::channel::<ExecutorDispatch>();
        let inner = Arc::new(PoolInner {
            handles: Mutex::new(Vec::with_capacity(workers)),
            running: Mutex::new(0),
            idle_signal: Condvar::new(),
            metrics: Metrics::default(),
        });
        let rx = Arc::new(Mutex::new(rx));
        let mut handles = inner.handles.lock().expect("fresh pool lock");
        for _ in 0..workers {
            let rx = Arc::clone(&rx);
            let inner = Arc::clone(&inner);
            handles.push(std::thread::spawn(move || worker_loop(&rx, &inner)));
        }
        drop(handles);
        Self { inner, tx }
    }
}

impl Default for ThreadPoolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl IoExecutor for ThreadPoolExecutor {
    fn attach_runtime(
        &self,
        _runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError> {
        // The pool is stateless per runtime — each dispatch already carries the
        // runtime-scoped completion sink — so a fresh lease sharing the same
        // workers serves every attached runtime. The lease gets its OWN `Sender`
        // clone so it keeps the worker channel connected independently of the
        // executor struct's lifetime.
        Ok(Arc::new(ThreadPoolLease {
            inner: Arc::clone(&self.inner),
            tx: Mutex::new(Some(self.tx.clone())),
        }))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        self.inner.snapshot()
    }
}

struct ThreadPoolLease {
    inner: Arc<PoolInner>,
    /// This lease's own `Sender` clone, routing into the shared worker pool.
    /// `None` once this lease's `shutdown` was called: further submissions on
    /// this lease are rejected, but sibling leases (and the executor) keep their
    /// own senders — so the pool stays up for them.
    tx: Mutex<Option<Sender<ExecutorDispatch>>>,
}

impl ExecutorLease for ThreadPoolLease {
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        let operation_id = submission.operation_id();
        let guard = self.tx.lock().expect("pool sender lock");
        let Some(tx) = guard.as_ref() else {
            return Err(submission.reject(SubmitErrorKind::ShuttingDown));
        };
        // The channel is unbounded and the workers hold the receiver as long as
        // any sender is alive; this lease holds one under the lock, so the armed
        // `Send` dispatch cannot be refused.
        self.inner.metrics.queued.fetch_add(1, Ordering::Relaxed);
        tx.send(submission.into_dispatch())
            .expect("worker receiver alive while sender held");
        Ok(RunningSubmission::new(operation_id))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        self.inner.snapshot()
    }

    fn shutdown(&self, deadline: Instant) -> ExecutorShutdown {
        // Close only THIS lease's sender (stop accepting work on this lease) and
        // then wait, bounded by the deadline, for in-flight jobs to drain. Sibling
        // leases and the executor keep their own senders, so the pool is not torn
        // down out from under them.
        self.tx.lock().expect("pool sender lock").take();
        self.inner.drain(deadline)
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

struct PoolInner {
    handles: Mutex<Vec<JoinHandle<()>>>,
    /// Count of dispatches currently executing on a worker; a lease `shutdown`
    /// blocks on this reaching zero (bounded by the deadline).
    running: Mutex<usize>,
    idle_signal: Condvar,
    metrics: Metrics,
}

impl PoolInner {
    fn snapshot(&self) -> ExecutorSnapshot {
        let m = &self.metrics;
        ExecutorSnapshot {
            queued: m.queued.load(Ordering::Relaxed),
            running_interruptible: m.running_interruptible.load(Ordering::Relaxed),
            running_quarantined: m.running_quarantined.load(Ordering::Relaxed),
            completed: m.completed.load(Ordering::Relaxed),
            cancelled: m.cancelled.load(Ordering::Relaxed),
            panicked: m.panicked.load(Ordering::Relaxed),
            undeliverable: m.undeliverable.load(Ordering::Relaxed),
        }
    }

    /// Bounded drain: wait for in-flight jobs to finish, capped by `deadline`.
    /// The caller (a lease's `shutdown`) has already closed its own sender; this
    /// does NOT disconnect the channel (sibling leases / the executor may still
    /// hold senders), it only waits out the currently-running jobs.
    fn drain(&self, deadline: Instant) -> ExecutorShutdown {
        let mut running = self.running.lock().expect("pool running lock");
        while *running > 0 {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (guard, timeout) = self
                .idle_signal
                .wait_timeout(running, deadline - now)
                .expect("pool idle wait");
            running = guard;
            if timeout.timed_out() {
                break;
            }
        }
        let drained = *running == 0;
        drop(running);
        let snapshot = self.snapshot();
        if drained {
            ExecutorShutdown::Drained(snapshot)
        } else {
            ExecutorShutdown::DeadlineExceeded(snapshot)
        }
    }
}

impl Drop for PoolInner {
    fn drop(&mut self) {
        // `PoolInner` only drops once its last `Arc` ref is released. Every
        // sender clone lives outside `PoolInner` (in the executor + its leases),
        // and each worker holds an `Arc<PoolInner>` — so this `Drop` cannot run
        // until all senders are gone, the channel has disconnected, and the
        // workers have begun exiting. Idle workers have already left `recv`; a
        // worker still inside a job exits as soon as that finite job returns, so
        // the join below is bounded.
        let handles = std::mem::take(&mut *self.handles.lock().expect("pool handles lock"));
        // A worker holds its own `Arc<PoolInner>`, and `PoolInner` owns that
        // worker's `JoinHandle` — so once the tx drops and the workers exit,
        // the LAST worker to release its `Arc` runs *this* `Drop` on its own
        // thread. Joining that thread's own handle would be a self-join
        // (`EDEADLK` — "Resource deadlock avoided"). Skip the current thread's
        // handle and detach it; it is already returning from `worker_loop`, so
        // nothing leaks and the remaining workers still join cleanly.
        let current = std::thread::current().id();
        for handle in handles {
            if handle.thread().id() == current {
                continue;
            }
            let _ = handle.join();
        }
    }
}

fn worker_loop(rx: &Arc<Mutex<Receiver<ExecutorDispatch>>>, inner: &Arc<PoolInner>) {
    loop {
        // Hold the receiver lock only long enough to dequeue one dispatch; the
        // lock is released before the (possibly long) job runs, so other workers
        // pick up sibling jobs and run concurrently.
        let dispatch = {
            let guard = rx.lock().expect("pool receiver lock");
            match guard.recv() {
                Ok(dispatch) => dispatch,
                Err(_) => return,
            }
        };
        inner.metrics.queued.fetch_sub(1, Ordering::Relaxed);
        let quarantined = matches!(
            &dispatch,
            ExecutorDispatch::Blocking(blocking)
                if blocking.class() == BlockingDispatchClass::QuarantinedBounded
        );
        let running_metric = if quarantined {
            &inner.metrics.running_quarantined
        } else {
            &inner.metrics.running_interruptible
        };
        running_metric.fetch_add(1, Ordering::Relaxed);
        *inner.running.lock().expect("pool running lock") += 1;

        let report = run_dispatch(dispatch);

        running_metric.fetch_sub(1, Ordering::Relaxed);
        record_terminal(&inner.metrics, report);
        {
            let mut running = inner.running.lock().expect("pool running lock");
            *running -= 1;
            if *running == 0 {
                inner.idle_signal.notify_all();
            }
        }
    }
}

fn run_dispatch(dispatch: ExecutorDispatch) -> ExecutorDriveReport {
    match dispatch {
        ExecutorDispatch::Blocking(blocking) => blocking.run(),
        // Async dispatches are polled to completion on the worker thread with a
        // minimal thread-parking waker (sema-vm carries no async runtime). The
        // foundation's migrated ops are blocking; this keeps async submissions
        // correct should a caller produce one.
        ExecutorDispatch::Async(async_dispatch) => block_on(async_dispatch.into_future()),
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

/// Drive a `Send` future to completion on the current worker thread, parking the
/// thread between polls. Sufficient for self-completing futures (the executor's
/// own dispatch futures); no external reactor is involved.
fn block_on<F: Future>(future: F) -> F::Output {
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod thread_pool_tests {
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

    use super::ThreadPoolExecutor;

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

    /// Two blocking jobs submitted together run on separate workers and overlap:
    /// both completions arrive in ~one sleep, not two.
    #[test]
    fn two_blocking_jobs_overlap() {
        let (tx, rx) = std::sync::mpsc::channel();
        let registrar_sender = Arc::new(ChannelSender(Mutex::new(tx)));
        let (runtime_id, registrar, _issuers) =
            CompletionRegistrar::register(registrar_sender).unwrap();

        let executor = ThreadPoolExecutor::with_workers(2);
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

        // Both completions land after ~200ms (overlapping), not ~400ms.
        rx.recv_timeout(Duration::from_secs(2))
            .expect("first completion");
        rx.recv_timeout(Duration::from_secs(2))
            .expect("second completion");
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(350),
            "blocking jobs must overlap on the pool; took {elapsed:?}"
        );

        // Shutdown with an already-past deadline returns promptly and bounded.
        let shutdown_start = Instant::now();
        let _ = lease.shutdown(Instant::now());
        assert!(shutdown_start.elapsed() < Duration::from_millis(200));
        drop(lease);
        drop(executor);
    }

    /// Dropping the executor and its lease WITHOUT calling `shutdown` — while a
    /// job is still in flight — must return promptly (bounded) and never deadlock
    /// or hang. The `ThreadPoolExecutor::Drop` backstop disconnects the sender so
    /// idle workers exit, and the in-flight worker detaches once its finite job
    /// returns (delivering its completion to a now-closed inbox). Guards against
    /// the self-join `EDEADLK` when the last `Arc<PoolInner>` releases on a worker
    /// thread.
    #[test]
    fn drop_without_shutdown_is_bounded() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let registrar_sender = Arc::new(ChannelSender(Mutex::new(tx)));
        let (runtime_id, registrar, _issuers) =
            CompletionRegistrar::register(registrar_sender).unwrap();
        let executor = ThreadPoolExecutor::with_workers(2);
        let lease = executor.attach_runtime(runtime_id).unwrap();
        let identity = registrar
            .issue_identity(CompletionKind::try_from_raw(1).unwrap())
            .unwrap();
        let (_runtime, submission) = registrar
            .bind(identity, blocking_sleep_op(80))
            .unwrap()
            .split();
        assert!(lease.submit(submission).is_ok(), "submission admitted");

        // No `shutdown` call: just drop the lease then the executor. Both must
        // return promptly without waiting out the job and without a self-join
        // panic (the in-flight worker keeps `PoolInner` alive until its job ends,
        // then runs `Drop` on itself — the self-join must be skipped, not aborted).
        let start = Instant::now();
        drop(lease);
        drop(executor);
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "dropping the executor must not block on the in-flight job"
        );
        // Give the detached worker time to finish its job and self-drop the pool;
        // a self-join regression would panic/abort on that worker thread.
        std::thread::sleep(Duration::from_millis(200));
    }

    /// Regression: the `Runtime` construction path retains only the executor's
    /// LEASE (`Runtime::new` drops the `ThreadPoolExecutor` struct right after
    /// attaching). Dropping the executor struct while a lease is still alive must
    /// keep the worker pool up so later submissions on that lease still run —
    /// otherwise every `mcp/call` (and any executor-backed external wait) is
    /// rejected with "external operation rejected". This is the exact shape of the
    /// `mcp_runtime_test` regression.
    #[test]
    fn executor_struct_dropped_before_lease_still_accepts() {
        let (tx, rx) = std::sync::mpsc::channel();
        let registrar_sender = Arc::new(ChannelSender(Mutex::new(tx)));
        let (runtime_id, registrar, _issuers) =
            CompletionRegistrar::register(registrar_sender).unwrap();

        let executor = ThreadPoolExecutor::with_workers(2);
        let lease = executor.attach_runtime(runtime_id).unwrap();
        // Drop the top-level executor struct BEFORE submitting anything. The
        // lease holds its own sender clone, so the workers stay up.
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
        // The job actually runs and its completion is delivered.
        rx.recv_timeout(Duration::from_secs(2))
            .expect("completion delivered after executor struct dropped");
        drop(lease);
    }

    /// Shutdown while a job is in flight is bounded and never hangs.
    #[test]
    fn shutdown_is_bounded_with_in_flight_job() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let registrar_sender = Arc::new(ChannelSender(Mutex::new(tx)));
        let (runtime_id, registrar, _issuers) =
            CompletionRegistrar::register(registrar_sender).unwrap();
        let executor = ThreadPoolExecutor::with_workers(2);
        let lease = executor.attach_runtime(runtime_id).unwrap();
        let identity = registrar
            .issue_identity(CompletionKind::try_from_raw(1).unwrap())
            .unwrap();
        let (_runtime, submission) = registrar
            .bind(identity, blocking_sleep_op(150))
            .unwrap()
            .split();
        assert!(lease.submit(submission).is_ok(), "submission admitted");

        // A near-immediate deadline returns without waiting out the whole job.
        let start = Instant::now();
        let _ = lease.shutdown(Instant::now() + Duration::from_millis(20));
        assert!(
            start.elapsed() < Duration::from_millis(120),
            "shutdown must honor the deadline, not block for the full job"
        );
        drop(lease);
        drop(executor);
    }
}
