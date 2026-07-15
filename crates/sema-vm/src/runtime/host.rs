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
pub struct ThreadPoolExecutor {
    inner: Arc<PoolInner>,
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
            tx: Mutex::new(Some(tx)),
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
        Self { inner }
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
        // workers serves every attached runtime.
        Ok(Arc::new(ThreadPoolLease {
            inner: Arc::clone(&self.inner),
        }))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        self.inner.snapshot()
    }
}

struct ThreadPoolLease {
    inner: Arc<PoolInner>,
}

impl ExecutorLease for ThreadPoolLease {
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        self.inner.submit(submission)
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        self.inner.snapshot()
    }

    fn shutdown(&self, deadline: Instant) -> ExecutorShutdown {
        self.inner.shutdown(deadline)
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
    /// `None` once shutdown has begun: submissions are rejected and idle workers
    /// observe the channel disconnect and exit.
    tx: Mutex<Option<Sender<ExecutorDispatch>>>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    /// Count of dispatches currently executing on a worker; shutdown blocks on
    /// this reaching zero (bounded by the deadline).
    running: Mutex<usize>,
    idle_signal: Condvar,
    metrics: Metrics,
}

impl PoolInner {
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        let operation_id = submission.operation_id();
        let guard = self.tx.lock().expect("pool sender lock");
        let Some(tx) = guard.as_ref() else {
            return Err(submission.reject(SubmitErrorKind::ShuttingDown));
        };
        // Capacity is secured (the channel is unbounded) and the workers still
        // hold the receiver while the sender is present under this lock, so the
        // armed `Send` dispatch cannot be refused.
        self.metrics.queued.fetch_add(1, Ordering::Relaxed);
        tx.send(submission.into_dispatch())
            .expect("worker receiver alive while sender held");
        Ok(RunningSubmission::new(operation_id))
    }

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

    fn shutdown(&self, deadline: Instant) -> ExecutorShutdown {
        // Stop accepting work and disconnect idle workers.
        self.tx.lock().expect("pool sender lock").take();
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
        // Disconnect (idempotent) and join every worker. Idle workers have
        // already exited once the sender dropped; a worker still inside a job
        // exits as soon as that finite job returns, so the join is bounded.
        self.tx.lock().expect("pool sender lock").take();
        let handles = std::mem::take(&mut *self.handles.lock().expect("pool handles lock"));
        for handle in handles {
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
