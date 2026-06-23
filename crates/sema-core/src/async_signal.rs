//! Async yield/resume signaling infrastructure.
//!
//! Thread-local signals for cooperative async scheduling. Native functions
//! (channel/recv, async/await, etc.) set `YIELD_SIGNAL` when they need to
//! suspend; the VM checks it after each native call. On resume, the scheduler
//! sets `RESUME_VALUE` so the native function can return the resolved value.
//!
//! Lives in sema-core (not sema-vm) so sema-stdlib can use it without
//! depending on sema-vm. Follows the same pattern as `set_eval_callback`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Condvar, Mutex};

use crate::value::{AsyncPromise, Channel, Value};
use crate::{EvalContext, SemaError};

/// Result of polling an offloaded I/O future from the VM thread.
///
/// The poller closure (created in `sema-llm`, never in `sema-core`) owns the
/// channel/receiver to the background runtime and translates the wire result
/// into a Sema `Value` on the VM thread, so no non-`Send` value ever crosses
/// the thread boundary.
pub enum IoPoll {
    /// The offloaded future has not completed yet — keep the task parked.
    Pending,
    /// The future completed. `Ok` resumes the task with the value; `Err`
    /// rejects the task's promise with the message.
    Ready(Result<Value, String>),
}

/// A non-blocking handle to an offloaded I/O future, polled by the scheduler.
///
/// Holds a boxed `FnMut` poller (built in `sema-llm`) so `sema-core` stays free
/// of both tokio and `sema-llm` types. Wrapped in `Rc` inside
/// [`YieldReason::AwaitIo`]; it never crosses a thread boundary (the scheduler
/// lives in a `thread_local`).
pub struct IoHandle {
    poll: RefCell<Box<dyn FnMut() -> IoPoll>>,
}

impl IoHandle {
    /// Create a handle from a poller closure. The closure is called each time
    /// the scheduler checks whether the offloaded future has completed.
    pub fn new(f: impl FnMut() -> IoPoll + 'static) -> Self {
        Self {
            poll: RefCell::new(Box::new(f)),
        }
    }

    /// Poll the offloaded future without blocking.
    pub fn poll(&self) -> IoPoll {
        (self.poll.borrow_mut())()
    }
}

// `IoHandle` holds a `Box<dyn FnMut>`, which is neither `Debug` nor `Clone`.
// A manual `Debug` impl keeps the `#[derive(Debug, Clone)]` on `YieldReason`
// compiling once the handle is wrapped in `Rc` (which restores `Clone`).
impl std::fmt::Debug for IoHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IoHandle")
    }
}

/// Reason a task is yielding control back to the scheduler.
#[derive(Debug, Clone)]
pub enum YieldReason {
    /// Waiting for a promise to resolve.
    AwaitPromise(Rc<AsyncPromise>),
    /// Waiting to receive from an empty channel.
    ChannelRecv(Rc<Channel>),
    /// Waiting to send to a full channel (carries the value to send).
    ChannelSend(Rc<Channel>, Value),
    /// Sleeping for a duration in milliseconds.
    Sleep(u64),
    /// Waiting for an offloaded I/O future (e.g. an HTTP round-trip running on a
    /// background runtime) to complete. The scheduler polls the handle and parks
    /// the VM thread on the process-global IO-completion signal while in flight.
    AwaitIo(Rc<IoHandle>),
}

/// What condition the scheduler should run until.
#[derive(Clone)]
pub enum SchedulerTarget {
    /// Run all currently scheduled work until no ready tasks remain.
    All,
    /// Run until one promise is no longer pending.
    One(Rc<AsyncPromise>),
    /// Run until all promises are complete, or any one rejects.
    AllOf(Vec<Rc<AsyncPromise>>),
    /// Run until any promise completes.
    AnyOf(Vec<Rc<AsyncPromise>>),
    /// Run until one promise completes or the duration elapses.
    Timeout(Rc<AsyncPromise>, u64),
}

/// Result of a scheduler run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerRunResult {
    Complete,
    TimedOut,
}

thread_local! {
    /// Set by native functions that need to yield. Checked by the VM after
    /// each native call. If set, the VM suspends the current task.
    static YIELD_SIGNAL: RefCell<Option<YieldReason>> = const { RefCell::new(None) };

    /// Set by the scheduler before resuming a yielded task. The native
    /// function that previously yielded checks this first and returns it
    /// instead of re-executing the operation.
    static RESUME_VALUE: RefCell<Option<Value>> = const { RefCell::new(None) };

    /// Whether we are currently executing inside an async task.
    /// Native functions check this to decide between yielding and erroring.
    static IN_ASYNC_CONTEXT: Cell<bool> = const { Cell::new(false) };
}

// ── Yield signal ────────────────────────────────────────────────

/// Set the yield signal. Called by native functions that need to suspend.
pub fn set_yield_signal(reason: YieldReason) {
    YIELD_SIGNAL.with(|s| *s.borrow_mut() = Some(reason));
}

/// Take the yield signal (clearing it). Called by the VM after native calls.
pub fn take_yield_signal() -> Option<YieldReason> {
    YIELD_SIGNAL.with(|s| s.borrow_mut().take())
}

// ── Resume value ────────────────────────────────────────────────

/// Set the resume value. Called by the scheduler before resuming a task.
pub fn set_resume_value(val: Value) {
    RESUME_VALUE.with(|r| *r.borrow_mut() = Some(val));
}

/// Take the resume value (clearing it). Called by the native function
/// that previously yielded, returning this instead of re-executing.
pub fn take_resume_value() -> Option<Value> {
    RESUME_VALUE.with(|r| r.borrow_mut().take())
}

// ── Async context ───────────────────────────────────────────────

/// Check if we are currently inside an async task.
pub fn in_async_context() -> bool {
    IN_ASYNC_CONTEXT.with(|c| c.get())
}

/// Set whether we are inside an async task.
pub fn set_async_context(val: bool) {
    IN_ASYNC_CONTEXT.with(|c| c.set(val));
}

// ── Spawn callback ──────────────────────────────────────────────

/// Callback type for spawning async tasks.
/// Takes the thunk (zero-arg function) and returns the promise value.
/// Registered by the scheduler in sema-vm at startup.
pub type SpawnCallbackFn = fn(&EvalContext, Value) -> Result<Value, SemaError>;

thread_local! {
    static SPAWN_CALLBACK: Cell<Option<SpawnCallbackFn>> = const { Cell::new(None) };
}

/// Register the spawn callback. Called by the scheduler during init.
pub fn set_spawn_callback(f: SpawnCallbackFn) {
    SPAWN_CALLBACK.with(|cb| cb.set(Some(f)));
}

/// Spawn an async task via the registered callback.
/// Returns an error if no scheduler has been registered.
pub fn call_spawn_callback(ctx: &EvalContext, thunk: Value) -> Result<Value, SemaError> {
    let f = SPAWN_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval(
            "async/spawn: no async scheduler registered (async requires the VM backend)"
                .to_string(),
        )
    })?;
    f(ctx, thunk)
}

// ── Run-scheduler callback ──────────────────────────────────────

/// Callback type for running the scheduler until a promise resolves.
/// Takes an optional promise to wait for (None = run all tasks).
pub type RunSchedulerCallbackFn =
    fn(&EvalContext, SchedulerTarget) -> Result<SchedulerRunResult, SemaError>;

thread_local! {
    static RUN_SCHEDULER_CALLBACK: Cell<Option<RunSchedulerCallbackFn>> = const { Cell::new(None) };
}

/// Register the run-scheduler callback.
pub fn set_run_scheduler_callback(f: RunSchedulerCallbackFn) {
    RUN_SCHEDULER_CALLBACK.with(|cb| cb.set(Some(f)));
}

// ── Cancel callback ─────────────────────────────────────────────

/// Callback type for cancelling an async task by its task ID.
///
/// Returns `Ok(true)` if the call actually transitioned the task into
/// `Cancelled`, `Ok(false)` if the task was already terminal (Done /
/// Failed / Cancelled) or if no task with that id exists (e.g. a
/// never-spawned promise like `async/resolved`).
pub type CancelCallbackFn = fn(u64) -> Result<bool, SemaError>;

thread_local! {
    static CANCEL_CALLBACK: Cell<Option<CancelCallbackFn>> = const { Cell::new(None) };
}

/// Register the cancel callback. Called by the scheduler during init.
pub fn set_cancel_callback(f: CancelCallbackFn) {
    CANCEL_CALLBACK.with(|cb| cb.set(Some(f)));
}

/// Cancel an async task by its task ID. Returns true if the call
/// actually transitioned the task to `Cancelled`; false otherwise.
pub fn call_cancel_callback(task_id: u64) -> Result<bool, SemaError> {
    let f = CANCEL_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval("async/cancel: no async scheduler registered".to_string())
    })?;
    f(task_id)
}

// ── Blocking-sleep callback ─────────────────────────────────────

/// Callback type for blocking the current thread for real wall-clock time when
/// the scheduler advances its virtual clock past a sleep. Takes a duration in
/// milliseconds (already bounded by the `async/sleep`/`async/timeout` caps).
pub type BlockingSleepFn = fn(u64);

thread_local! {
    static BLOCKING_SLEEP_CALLBACK: Cell<Option<BlockingSleepFn>> = const { Cell::new(None) };
}

/// Install a blocking-sleep callback. Used by the playground Web Worker to do a
/// real `Atomics.wait` on a `SharedArrayBuffer`, so `async/sleep` paces in real
/// time even in wasm (where the default is an instant no-op so the UI thread is
/// never blocked). Native does not normally install one — it uses the
/// `std::thread::sleep` default below.
pub fn set_blocking_sleep_callback(f: BlockingSleepFn) {
    BLOCKING_SLEEP_CALLBACK.with(|cb| cb.set(Some(f)));
}

/// Remove any installed blocking-sleep callback, restoring the platform default.
pub fn clear_blocking_sleep_callback() {
    BLOCKING_SLEEP_CALLBACK.with(|cb| cb.set(None));
}

// ── Interrupt (cancellation) callback ───────────────────────────

/// Callback that returns true when the running evaluation should be cancelled.
/// The playground Web Worker installs one that reads a shared cancel flag
/// (`Atomics.load` on the control SAB) so a Stop button can interrupt a running
/// program — including one blocked in a real `Atomics.wait` sleep.
pub type InterruptCallbackFn = fn() -> bool;

thread_local! {
    static INTERRUPT_CALLBACK: Cell<Option<InterruptCallbackFn>> = const { Cell::new(None) };
}

/// Install the interrupt/cancellation check. See [`check_interrupt`].
pub fn set_interrupt_callback(f: InterruptCallbackFn) {
    INTERRUPT_CALLBACK.with(|cb| cb.set(Some(f)));
}

/// Remove any installed interrupt callback.
pub fn clear_interrupt_callback() {
    INTERRUPT_CALLBACK.with(|cb| cb.set(None));
}

/// True if a cancellation has been requested via the installed interrupt
/// callback. Cheap no-op (false) when none is installed.
#[inline]
pub fn check_interrupt() -> bool {
    INTERRUPT_CALLBACK.with(|cb| cb.get()).is_some_and(|f| f())
}

/// Block for `ms` milliseconds of real wall-clock time as part of advancing the
/// scheduler's virtual clock. If a host installed a callback (see
/// [`set_blocking_sleep_callback`]) it is used. Otherwise the default is: sleep
/// the OS thread on native, and no-op in wasm (the main thread must not block —
/// the caller still advances virtual time afterward, preserving sleep ordering).
pub fn blocking_sleep_ms(ms: u64) {
    if let Some(f) = BLOCKING_SLEEP_CALLBACK.with(|cb| cb.get()) {
        f(ms);
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(std::time::Duration::from_millis(ms));
    #[cfg(target_arch = "wasm32")]
    let _ = ms; // no-op: advancing virtual time (caller) is enough for ordering
}

// ── IO-completion signal ────────────────────────────────────────

/// Process-global IO-completion signal: a generation counter bumped each time
/// an offloaded future finishes, plus a condvar so a parked VM thread wakes
/// promptly. A missed notification is bounded by the park timeout — callers
/// (the scheduler) loop and re-poll their `IoHandle`s, so correctness never
/// depends on catching every notify.
///
/// This lives in `sema-core` (not tokio-land) so the park primitive is reachable
/// from `sema-vm`'s scheduler without a tokio dependency. The bumping side is
/// called by the offloaded future in `sema-llm`.
static IO_SIGNAL: (Mutex<u64>, Condvar) = (Mutex::new(0), Condvar::new());

/// Notify any thread parked in [`io_park`] that an offloaded future has
/// completed. Bumps the generation counter and wakes all waiters. Safe to call
/// from a background runtime thread.
pub fn notify_io_complete() {
    let (lock, cvar) = &IO_SIGNAL;
    if let Ok(mut gen) = lock.lock() {
        *gen = gen.wrapping_add(1);
        cvar.notify_all();
    }
}

/// Park the current (VM) thread on the IO-completion signal for up to
/// `timeout_ms` milliseconds, returning early if a [`notify_io_complete`]
/// arrives. The caller must re-poll its in-flight handles after this returns
/// (whether woken by a notify or by the timeout) — a missed notify is bounded
/// by `timeout_ms`, so the caller's poll loop stays live.
pub fn io_park(timeout_ms: u64) {
    let (lock, cvar) = &IO_SIGNAL;
    if let Ok(gen) = lock.lock() {
        let _ = cvar.wait_timeout(gen, std::time::Duration::from_millis(timeout_ms));
    }
}

/// Run the scheduler, optionally waiting for a specific promise.
pub fn call_run_scheduler(
    ctx: &EvalContext,
    target: Option<Rc<AsyncPromise>>,
) -> Result<(), SemaError> {
    let f = RUN_SCHEDULER_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval(
            "async: no async scheduler registered (async requires the VM backend)".to_string(),
        )
    })?;
    let target = match target {
        Some(promise) => SchedulerTarget::One(promise),
        None => SchedulerTarget::All,
    };
    f(ctx, target).map(|_| ())
}

/// Run the scheduler until all target promises complete, or any target rejects.
pub fn call_run_scheduler_all_of(
    ctx: &EvalContext,
    targets: Vec<Rc<AsyncPromise>>,
) -> Result<(), SemaError> {
    let f = RUN_SCHEDULER_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval(
            "async: no async scheduler registered (async requires the VM backend)".to_string(),
        )
    })?;
    f(ctx, SchedulerTarget::AllOf(targets)).map(|_| ())
}

/// Run the scheduler until any target promise completes.
pub fn call_run_scheduler_any_of(
    ctx: &EvalContext,
    targets: Vec<Rc<AsyncPromise>>,
) -> Result<(), SemaError> {
    let f = RUN_SCHEDULER_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval(
            "async: no async scheduler registered (async requires the VM backend)".to_string(),
        )
    })?;
    f(ctx, SchedulerTarget::AnyOf(targets)).map(|_| ())
}

/// Run the scheduler until the target promise completes or the duration elapses.
pub fn call_run_scheduler_timeout(
    ctx: &EvalContext,
    target: Rc<AsyncPromise>,
    timeout_ms: u64,
) -> Result<SchedulerRunResult, SemaError> {
    let f = RUN_SCHEDULER_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval(
            "async: no async scheduler registered (async requires the VM backend)".to_string(),
        )
    })?;
    f(ctx, SchedulerTarget::Timeout(target, timeout_ms))
}
