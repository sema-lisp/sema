//! Async yield/resume signaling infrastructure.
//!
//! Thread-local signals for cooperative async scheduling. Native functions
//! (channel/recv, async/await, etc.) set `YIELD_SIGNAL` when they need to
//! suspend; the VM checks it after each native call. On resume, the scheduler
//! sets `RESUME_VALUE` so the native function can return the resolved value.
//!
//! Lives in sema-core (not sema-vm) so sema-stdlib can use it without
//! depending on sema-vm. Follows the same pattern as `set_eval_callback`.

use std::any::Any;
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
    /// Optional one-shot abort hook, run when the scheduler CANCELS a task parked on
    /// this handle (`async/cancel`, `async/timeout` expiry, or an interrupt) — NEVER
    /// on normal completion. It aborts the offloaded work where the runtime supports
    /// it (a tokio `AbortHandle::abort()` for the `spawn`-based http/shell offloads,
    /// dropping the in-flight future → connection torn down / `kill_on_drop` child
    /// killed). For `spawn_blocking` offloads (the LLM tier) there is no hook — a
    /// blocking closure cannot be interrupted, so cancellation stays best-effort
    /// there (the result is discarded). Built via [`with_abort`]; `None` for [`new`].
    ///
    /// [`with_abort`]: IoHandle::with_abort
    /// [`new`]: IoHandle::new
    abort: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl IoHandle {
    /// Create a handle from a poller closure with NO abort hook (cancellation is
    /// best-effort: the offloaded future runs to completion, its result discarded).
    /// The poll closure is called each time the scheduler checks for completion.
    pub fn new(f: impl FnMut() -> IoPoll + 'static) -> Self {
        Self {
            poll: RefCell::new(Box::new(f)),
            abort: RefCell::new(None),
        }
    }

    /// Create a handle with a one-shot `abort` hook. `abort` is called AT MOST ONCE,
    /// from the VM thread, when the scheduler cancels a task parked on this handle.
    /// It must be non-blocking (e.g. `tokio::task::AbortHandle::abort()`).
    pub fn with_abort(f: impl FnMut() -> IoPoll + 'static, abort: impl FnOnce() + 'static) -> Self {
        Self {
            poll: RefCell::new(Box::new(f)),
            abort: RefCell::new(Some(Box::new(abort))),
        }
    }

    /// Poll the offloaded future without blocking.
    pub fn poll(&self) -> IoPoll {
        (self.poll.borrow_mut())()
    }

    /// Run the abort hook if present, consuming it so it runs at most once. A no-op
    /// for handles built with [`new`], or after a prior `abort`. The scheduler calls
    /// this ONLY on cancellation/timeout/interrupt — never on normal completion.
    ///
    /// [`new`]: IoHandle::new
    pub fn abort(&self) {
        // Take the hook OUT before invoking it, releasing the RefCell borrow — so a
        // re-entrant or double abort is genuinely a no-op, never a BorrowMutError.
        let hook = self.abort.borrow_mut().take();
        if let Some(f) = hook {
            f();
        }
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
    /// A breakpoint fired inside an async task during a COOPERATIVE (WASM)
    /// debug session: the scheduler stopped driving and left the breakpointed
    /// task PAUSED (frames intact, not reaped) so it can resume on a later
    /// scheduler re-entry. The driving native (`async/await`, `async/all`,
    /// `async/timeout`, `async/race`) must NOT inspect the still-pending target
    /// promise on this result — it yields the main VM (so `run_cooperative`
    /// surfaces the stop to JS) and re-drives the scheduler on resume. Only ever
    /// produced when a headless `DebugState` is the active session; the blocking
    /// native DAP path never sees it (it blocks in `handle_debug_stop` instead).
    DebugPaused,
}

/// How a debug-paused scheduler-driving native should reconstruct its return
/// value once the cooperative debug session resumes the paused task and the
/// target promise(s) settle. Recorded by the native when the scheduler returns
/// [`SchedulerRunResult::DebugPaused`]; consumed by `run_cooperative` in
/// `sema-vm`, which re-drives the scheduler and then resumes the main VM with
/// the reconstructed value (`set_resume_value`).
#[derive(Clone)]
pub enum DebugCoopResume {
    /// `async/await` / `async/timeout`: resume with the single target promise's
    /// resolved value (rejection/cancel surface as the native's own error after
    /// resume, since the native re-runs and re-inspects the promise).
    Await(Rc<AsyncPromise>),
    /// `async/all`: resume with the list of all resolved values.
    All(Vec<Rc<AsyncPromise>>),
    /// `async/race` / `async/any`: resume with the first settled promise's value.
    Race(Vec<Rc<AsyncPromise>>),
    /// `async/run`: no value to reconstruct (returns nil).
    Run,
}

thread_local! {
    /// Set by native functions that need to yield. Checked by the VM after
    /// each native call. If set, the VM suspends the current task.
    static YIELD_SIGNAL: RefCell<Option<YieldReason>> = const { RefCell::new(None) };

    /// Set by a scheduler-driving native when the cooperative scheduler paused
    /// for a debug breakpoint inside a task. Carries the `SchedulerTarget` to
    /// re-drive on resume and how to reconstruct the native's value. Consumed by
    /// `run_cooperative` (sema-vm). See [`SchedulerRunResult::DebugPaused`].
    static DEBUG_COOP_RESUME: RefCell<Option<(SchedulerTarget, DebugCoopResume)>> =
        const { RefCell::new(None) };

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

// ── Cooperative debug-pause resume (WASM) ───────────────────────

/// Record how to resume a scheduler-driving native after a cooperative debug
/// pause. Called by the native when the scheduler returns `DebugPaused`.
pub fn set_debug_coop_resume(target: SchedulerTarget, how: DebugCoopResume) {
    DEBUG_COOP_RESUME.with(|s| *s.borrow_mut() = Some((target, how)));
}

/// Take the pending cooperative debug-pause resume, if any. Called by
/// `run_cooperative` (sema-vm) to re-drive the scheduler on resume.
pub fn take_debug_coop_resume() -> Option<(SchedulerTarget, DebugCoopResume)> {
    DEBUG_COOP_RESUME.with(|s| s.borrow_mut().take())
}

/// True if a cooperative debug pause is pending (a scheduler-driving native
/// yielded the main VM for a task breakpoint). Used by `run_cooperative` to
/// decide whether the AsyncYield it sees is a debug pause vs a real async park.
pub fn debug_coop_resume_pending() -> bool {
    DEBUG_COOP_RESUME.with(|s| s.borrow().is_some())
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

// ── Per-task OTel context swap (type-erased seam) ───────────────
//
// The cooperative scheduler runs many tasks on the one VM thread. The otel
// span stack + conversation/session/user ids live in `sema-otel` thread-locals,
// so a task that parks mid-span and yields to a sibling would otherwise share
// (and corrupt) that single stack. The scheduler swaps each task's otel context
// in on entry and out on leave.
//
// `sema-core` must not depend on `sema-otel`, so the actual take/install lives
// in `sema-otel` and is reached through these type-erased fn-pointer callbacks
// (`Box<dyn Any>` carries the `OtelTaskCtx`), registered once at startup by a
// crate that names both types — exactly mirroring `set_blocking_sleep_callback`.
// When no callback is installed (e.g. a unit test with no otel), both helpers
// are no-ops returning an empty box.

/// Take (mem::take) the current thread's otel task context, leaving it empty.
pub type OtelTakeFn = fn() -> Box<dyn Any>;
/// Install (mem::replace) an otel task context, returning the one displaced.
pub type OtelInstallFn = fn(Box<dyn Any>) -> Box<dyn Any>;
/// Capture the current conversation/session/user identity with an EMPTY span
/// stack — seeded onto a freshly-spawned task.
pub type OtelScopeFn = fn() -> Box<dyn Any>;

thread_local! {
    static OTEL_TAKE_CALLBACK: Cell<Option<OtelTakeFn>> = const { Cell::new(None) };
    static OTEL_INSTALL_CALLBACK: Cell<Option<OtelInstallFn>> = const { Cell::new(None) };
    static OTEL_SCOPE_CALLBACK: Cell<Option<OtelScopeFn>> = const { Cell::new(None) };
}

/// Register the per-task otel take/install/scope callbacks. Called once at
/// startup by `sema_otel::register_task_callbacks()`.
pub fn set_otel_task_callbacks(take: OtelTakeFn, install: OtelInstallFn, scope: OtelScopeFn) {
    OTEL_TAKE_CALLBACK.with(|cb| cb.set(Some(take)));
    OTEL_INSTALL_CALLBACK.with(|cb| cb.set(Some(install)));
    OTEL_SCOPE_CALLBACK.with(|cb| cb.set(Some(scope)));
}

/// Capture the current conversation scope (ids only, empty span stack) as a
/// type-erased otel task context to seed onto a newly-spawned task. Returns
/// `Box::new(())` when no callback is installed.
pub fn current_conversation_scope_boxed() -> Box<dyn Any> {
    match OTEL_SCOPE_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(),
        None => Box::new(()),
    }
}

/// Take the current otel task context out of the thread-locals (leaving them
/// empty). Returns `Box::new(())` when no callback is installed.
pub fn take_task_otel() -> Box<dyn Any> {
    match OTEL_TAKE_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(),
        None => Box::new(()),
    }
}

/// Install `ctx` into the otel thread-locals, returning the context it displaced
/// (so the caller can restore it). A no-op returning `Box::new(())` when no
/// callback is installed.
pub fn install_task_otel(ctx: Box<dyn Any>) -> Box<dyn Any> {
    match OTEL_INSTALL_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(ctx),
        None => Box::new(()),
    }
}

// ── Per-task "active leaf usage scope" seam ─────────────────────────
//
// The workflow runtime attributes per-leaf LLM usage by reading the accumulator
// frame active for the CURRENT TASK. Like the otel context above, this must be
// captured at task spawn (an inline agent thunk inherits the scope its
// `workflow/step` opened) and swapped in/out at each task step so concurrently
// running sibling tasks don't clobber each other's active frame. The actual
// `Rc<RefCell<LeafUsage>>` slot lives in `sema-llm`; `sema-core` reaches it
// through these type-erased fn-pointer callbacks (mirroring the otel seam), so it
// need not depend on `sema-llm`. No-ops returning an empty box when unregistered.

/// Capture the current thread's active leaf-usage scope (cloning its `Rc`) to
/// seed onto a freshly-spawned task. `Box::new(())` when unregistered/none active.
pub type UsageScopeCaptureFn = fn() -> Box<dyn Any>;
/// Take (mem::take) the current thread's active leaf-usage scope, leaving none.
pub type UsageScopeTakeFn = fn() -> Box<dyn Any>;
/// Install a leaf-usage scope into the thread-local, returning the one displaced.
pub type UsageScopeInstallFn = fn(Box<dyn Any>) -> Box<dyn Any>;

thread_local! {
    static USAGE_SCOPE_CAPTURE_CALLBACK: Cell<Option<UsageScopeCaptureFn>> = const { Cell::new(None) };
    static USAGE_SCOPE_TAKE_CALLBACK: Cell<Option<UsageScopeTakeFn>> = const { Cell::new(None) };
    static USAGE_SCOPE_INSTALL_CALLBACK: Cell<Option<UsageScopeInstallFn>> = const { Cell::new(None) };
}

/// Register the per-task leaf-usage-scope callbacks. Called once at startup by
/// `sema-llm` (the crate that owns the `LeafUsage` accumulator).
pub fn set_usage_scope_task_callbacks(
    capture: UsageScopeCaptureFn,
    take: UsageScopeTakeFn,
    install: UsageScopeInstallFn,
) {
    USAGE_SCOPE_CAPTURE_CALLBACK.with(|cb| cb.set(Some(capture)));
    USAGE_SCOPE_TAKE_CALLBACK.with(|cb| cb.set(Some(take)));
    USAGE_SCOPE_INSTALL_CALLBACK.with(|cb| cb.set(Some(install)));
}

/// Capture the active leaf-usage scope to seed a spawned task.
pub fn current_usage_scope_boxed() -> Box<dyn Any> {
    match USAGE_SCOPE_CAPTURE_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(),
        None => Box::new(()),
    }
}

/// Take the active leaf-usage scope out of the thread-local (leaving none).
pub fn take_task_usage_scope() -> Box<dyn Any> {
    match USAGE_SCOPE_TAKE_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(),
        None => Box::new(()),
    }
}

/// Install a leaf-usage scope into the thread-local, returning the one displaced.
pub fn install_task_usage_scope(ctx: Box<dyn Any>) -> Box<dyn Any> {
    match USAGE_SCOPE_INSTALL_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(ctx),
        None => Box::new(()),
    }
}

// ── Per-task LLM dynamic-scope seam ─────────────────────────────────
//
// `llm/with-cache`, `llm/with-budget`, and per-call `:tags`/`:metadata` set
// dynamically-scoped thread-locals in `sema-llm` for the extent of a thunk, then
// reset them. A task spawned inside that thunk reads those flags WHEN IT RUNS,
// which the cooperative scheduler can defer past the reset — so the flags leak
// across the task boundary (cache misses under-counted, and a `with-budget` cap
// failing to gate a concurrent fan-out). Like the otel context and the leaf-usage
// scope above, the scheduler captures this dynamic scope at task spawn and swaps it
// in/out at each task step so concurrent siblings stay isolated. The read-only flags
// (cache-enabled, tags, …) ride as a value snapshot; the budget frame rides as a
// shared `Rc` so all siblings in one `with-budget` charge ONE aggregate. The scope
// struct lives in `sema-llm`; `sema-core` reaches it through these type-erased
// fn-pointer callbacks (mirroring the usage-scope seam). No-ops returning an empty
// box when unregistered.

/// Capture the current thread's LLM dynamic scope (cloning read-only values and the
/// budget `Rc`) to seed onto a freshly-spawned task. `Box::new(())` when unregistered.
pub type LlmScopeCaptureFn = fn() -> Box<dyn Any>;
/// Take (mem::take) the current thread's LLM dynamic scope, leaving defaults.
pub type LlmScopeTakeFn = fn() -> Box<dyn Any>;
/// Install an LLM dynamic scope into the thread-locals, returning the one displaced.
pub type LlmScopeInstallFn = fn(Box<dyn Any>) -> Box<dyn Any>;

thread_local! {
    static LLM_SCOPE_CAPTURE_CALLBACK: Cell<Option<LlmScopeCaptureFn>> = const { Cell::new(None) };
    static LLM_SCOPE_TAKE_CALLBACK: Cell<Option<LlmScopeTakeFn>> = const { Cell::new(None) };
    static LLM_SCOPE_INSTALL_CALLBACK: Cell<Option<LlmScopeInstallFn>> = const { Cell::new(None) };
}

/// Register the per-task LLM dynamic-scope callbacks. Called once at startup by
/// `sema-llm` (the crate that owns the scope struct).
pub fn set_llm_scope_task_callbacks(
    capture: LlmScopeCaptureFn,
    take: LlmScopeTakeFn,
    install: LlmScopeInstallFn,
) {
    LLM_SCOPE_CAPTURE_CALLBACK.with(|cb| cb.set(Some(capture)));
    LLM_SCOPE_TAKE_CALLBACK.with(|cb| cb.set(Some(take)));
    LLM_SCOPE_INSTALL_CALLBACK.with(|cb| cb.set(Some(install)));
}

/// Capture the LLM dynamic scope to seed a spawned task.
pub fn current_llm_scope_boxed() -> Box<dyn Any> {
    match LLM_SCOPE_CAPTURE_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(),
        None => Box::new(()),
    }
}

/// Take the LLM dynamic scope out of the thread-locals (leaving defaults).
pub fn take_task_llm_scope() -> Box<dyn Any> {
    match LLM_SCOPE_TAKE_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(),
        None => Box::new(()),
    }
}

/// Install an LLM dynamic scope into the thread-locals, returning the one displaced.
pub fn install_task_llm_scope(ctx: Box<dyn Any>) -> Box<dyn Any> {
    match LLM_SCOPE_INSTALL_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(ctx),
        None => Box::new(()),
    }
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

/// Run the scheduler, optionally waiting for a specific promise. Returns the
/// scheduler run result so the caller can detect a cooperative debug pause
/// ([`SchedulerRunResult::DebugPaused`]).
pub fn call_run_scheduler(
    ctx: &EvalContext,
    target: Option<Rc<AsyncPromise>>,
) -> Result<SchedulerRunResult, SemaError> {
    let f = RUN_SCHEDULER_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval(
            "async: no async scheduler registered (async requires the VM backend)".to_string(),
        )
    })?;
    let target = match target {
        Some(promise) => SchedulerTarget::One(promise),
        None => SchedulerTarget::All,
    };
    f(ctx, target)
}

/// Run the scheduler until all target promises complete, or any target rejects.
pub fn call_run_scheduler_all_of(
    ctx: &EvalContext,
    targets: Vec<Rc<AsyncPromise>>,
) -> Result<SchedulerRunResult, SemaError> {
    let f = RUN_SCHEDULER_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval(
            "async: no async scheduler registered (async requires the VM backend)".to_string(),
        )
    })?;
    f(ctx, SchedulerTarget::AllOf(targets))
}

/// Run the scheduler until any target promise completes.
pub fn call_run_scheduler_any_of(
    ctx: &EvalContext,
    targets: Vec<Rc<AsyncPromise>>,
) -> Result<SchedulerRunResult, SemaError> {
    let f = RUN_SCHEDULER_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval(
            "async: no async scheduler registered (async requires the VM backend)".to_string(),
        )
    })?;
    f(ctx, SchedulerTarget::AnyOf(targets))
}

/// Run the scheduler with an explicit `SchedulerTarget`. Used by the cooperative
/// debug-resume path (`run_cooperative` in sema-vm) to re-drive the scheduler for
/// a paused task after a breakpoint, reusing the exact target the original native
/// recorded. Returns the scheduler run result (including `DebugPaused` if another
/// breakpoint fires).
pub fn call_run_scheduler_target(
    ctx: &EvalContext,
    target: SchedulerTarget,
) -> Result<SchedulerRunResult, SemaError> {
    let f = RUN_SCHEDULER_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
        SemaError::eval(
            "async: no async scheduler registered (async requires the VM backend)".to_string(),
        )
    })?;
    f(ctx, target)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn io_handle_abort_runs_once() {
        let count = Rc::new(Cell::new(0));
        let c = count.clone();
        let h = IoHandle::with_abort(|| IoPoll::Pending, move || c.set(c.get() + 1));
        assert_eq!(count.get(), 0, "abort not run until called");
        h.abort();
        assert_eq!(count.get(), 1, "abort runs on first call");
        h.abort();
        h.abort();
        assert_eq!(count.get(), 1, "abort is one-shot — later calls are no-ops");
    }

    #[test]
    fn io_handle_new_abort_is_noop() {
        // A handle with no abort hook must not panic when aborted.
        let h = IoHandle::new(|| IoPoll::Ready(Ok(Value::nil())));
        h.abort();
        // Poll still works after a (no-op) abort.
        assert!(matches!(h.poll(), IoPoll::Ready(Ok(_))));
    }

    #[test]
    fn io_handle_poll_works_after_abort() {
        let h = IoHandle::with_abort(|| IoPoll::Ready(Ok(Value::int(7))), || {});
        h.abort();
        match h.poll() {
            IoPoll::Ready(Ok(v)) => assert_eq!(v, Value::int(7)),
            _ => panic!("poll should still return Ready after abort"),
        }
    }
}
