//! Async yield/resume signaling infrastructure.
//!
//! Thread-local signals and per-task context seams shared between `sema-core`,
//! `sema-stdlib`, `sema-llm`, and the unified runtime (`sema-vm`). Lives in
//! sema-core (not sema-vm) so `sema-stdlib` can use it without depending on
//! sema-vm. Follows the same pattern as `set_eval_callback`.
//!
//! The unified cooperative runtime drives async ops structurally through the
//! `NativeOutcome` ABI (`Suspend`/`Runtime`); the one remaining TLS yield signal
//! is [`YieldReason::Sleep`], which a ctx-less `async/sleep` uses to surface a
//! timer wait when it cannot suspend structurally.

use std::any::Any;
use std::cell::{Cell, RefCell};

use crate::value::Value;

/// Reason a task is yielding control back to the runtime via the TLS yield
/// signal. Promise/channel/IO ops suspend structurally through the
/// `NativeOutcome` ABI; only `async/sleep`'s ctx-less value ABI still yields.
#[derive(Debug, Clone)]
pub enum YieldReason {
    /// Sleeping for a duration in milliseconds.
    Sleep(u64),
}

thread_local! {
    /// Set by native functions that need to yield. Checked by the VM after
    /// each native call. If set, the VM suspends the current task.
    static YIELD_SIGNAL: RefCell<Option<YieldReason>> = const { RefCell::new(None) };

    /// Set by the runtime before resuming a yielded task. The native function
    /// that previously yielded checks this first and returns it instead of
    /// re-executing the operation. Vestigial under the structural resume path
    /// (the runtime injects the resume value onto the parked frame's stack top),
    /// kept for the ctx-less `async/sleep` symmetry.
    static RESUME_VALUE: RefCell<Option<Value>> = const { RefCell::new(None) };

    /// Whether a unified-runtime VM quantum is currently executing on this
    /// thread. Set for the lifetime of one `run_quantum` by the runtime's
    /// `RuntimeQuantumGuard`. Native functions that yield via the TLS yield
    /// signal (e.g. `async/sleep`) check this to decide between yielding to the
    /// runtime and running synchronously.
    static IN_RUNTIME_QUANTUM: Cell<bool> = const { Cell::new(false) };
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

/// Take the resume value (clearing it). Called by the native function that
/// previously yielded, returning this instead of re-executing.
pub fn take_resume_value() -> Option<Value> {
    RESUME_VALUE.with(|r| r.borrow_mut().take())
}

// ── Runtime-quantum flag ────────────────────────────────────────

/// True while a unified-runtime VM quantum is executing on this thread. A
/// yielding native (`async/sleep`, …) surfaces the yield signal so the runtime
/// can register a native wait, rather than running synchronously.
pub fn in_runtime_quantum() -> bool {
    IN_RUNTIME_QUANTUM.with(|c| c.get())
}

/// Set whether a unified-runtime VM quantum is executing on this thread.
/// Called only by the runtime's `RuntimeQuantumGuard` (enter/drop).
pub fn set_runtime_quantum(val: bool) {
    IN_RUNTIME_QUANTUM.with(|c| c.set(val));
}

// ── Promise-driven quantum flag (wasm) ───────────────────────────

thread_local! {
    /// True only while a wasm macrotask-driven turn (`sema-wasm`'s
    /// `driver.rs`, `evalPromise`) is pumping the runtime on this thread.
    ///
    /// `runtime_quantum_active()`/`in_runtime_quantum()` is true for EVERY
    /// live runtime quantum, old and new alike — every `eval*` entry point
    /// (wasm and native) drives the same `Runtime::drive`, so a dual-ABI
    /// native cannot use that flag to tell "a genuinely non-blocking,
    /// externally-driven caller that can suspend a wasm native on a real
    /// external wait" apart from "a synchronous caller that is about to
    /// block the calling thread waiting for this native's result"
    /// (`drive_handle_to_settlement` → `block_on_inbox`, which cannot work
    /// on wasm32: no OS threads to block, and no reactor to service the
    /// suspended op). This flag is the explicit, narrower discriminator: set
    /// ONLY by the macrotask driver's own drive turn, so a dual-ABI wasm
    /// native can suspend structurally when (and only when) something is
    /// actually pumping the runtime asynchronously across turns, and must
    /// fall back to its legacy, synchronous behavior everywhere else —
    /// including inside every OTHER live runtime quantum.
    static PROMISE_DRIVEN_QUANTUM: Cell<bool> = const { Cell::new(false) };
}

/// True only inside a wasm macrotask driver's own drive turn (see
/// [`PROMISE_DRIVEN_QUANTUM`]).
pub fn promise_driven_quantum_active() -> bool {
    PROMISE_DRIVEN_QUANTUM.with(|c| c.get())
}

/// Set the promise-driven-quantum flag, returning the PREVIOUS value so a
/// caller can restore it (RAII guard pattern, mirroring
/// [`RuntimeQuantumGuard`](crate::context::RuntimeQuantumGuard)). Intended to
/// be called only by the wasm macrotask driver around its own `drive_turn`
/// call, symmetrically set/unset (including on unwind) so a panic mid-turn
/// can never leave a subsequent, unrelated legacy `eval` call thinking it is
/// promise-driven.
pub fn set_promise_driven_quantum(val: bool) -> bool {
    PROMISE_DRIVEN_QUANTUM.with(|c| c.replace(val))
}

// ── Task-reaped callback ────────────────────────────────────────

/// Callback type for observing a task's transition into a terminal state it
/// will NEVER resume from (cancellation via `async/cancel`, `async/timeout`
/// expiry, transitive await-tree cancellation, or an interrupt). Takes the
/// reaped task's id.
///
/// Fired by the runtime on the VM thread, with the OTel thread-locals still
/// alive — but with the reaped task's own per-task contexts (otel span stack,
/// usage/LLM scopes) NOT installed; the cancellation driver's are. NEVER fired
/// on ordinary completion (Done) or on a task's own error exit (Failed via a
/// Sema error) — those paths run their own cleanup in bytecode; only a
/// cancellation leaves per-task native state (e.g. an agent-run slab entry in
/// `sema-llm`) with no other reclamation point.
pub type TaskReapedFn = fn(u64);

thread_local! {
    static TASK_REAPED_CALLBACK: Cell<Option<TaskReapedFn>> = const { Cell::new(None) };
}

/// Register the task-reaped callback. Called by `sema-llm` at builtin
/// registration (the crate that owns per-task native state needing reclamation).
pub fn set_task_reaped_callback(f: TaskReapedFn) {
    TASK_REAPED_CALLBACK.with(|cb| cb.set(Some(f)));
}

/// Notify the registered callback that `task_id` was reaped (cancelled and will
/// never resume). Cheap no-op when no callback is installed. See
/// [`TaskReapedFn`] for the firing contract.
pub fn notify_task_reaped(task_id: u64) {
    if let Some(f) = TASK_REAPED_CALLBACK.with(|cb| cb.get()) {
        f(task_id);
    }
}

// ── Current task id ─────────────────────────────────────────────

thread_local! {
    /// The runtime task id currently executing on this thread, if any. Set by
    /// the runtime around each task step so natives that stash per-task state
    /// (e.g. `__agent-begin`'s slab entry) can stamp it with its owning task for
    /// later reclamation via the task-reaped callback. `None` outside any task
    /// step (top-level code).
    static CURRENT_TASK_ID: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The id of the task currently being stepped by the runtime, or `None` when
/// running top-level (non-task) code.
pub fn current_task_id() -> Option<u64> {
    CURRENT_TASK_ID.with(|c| c.get())
}

/// Install `id` as the current task id, returning the displaced value so the
/// caller can restore it on step leave (nested inline-task runs stack correctly).
pub fn set_current_task_id(id: Option<u64>) -> Option<u64> {
    CURRENT_TASK_ID.with(|c| c.replace(id))
}

// ── Blocking-sleep callback ─────────────────────────────────────

/// Callback type for blocking the current thread for real wall-clock time when
/// the runtime advances its virtual clock past a sleep. Takes a duration in
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

/// Block for `ms` milliseconds of real wall-clock time as part of advancing the
/// runtime's virtual clock. If a host installed a callback (see
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

// ── Per-task OTel context swap (type-erased seam) ───────────────
//
// The cooperative runtime runs many tasks on the one VM thread. The otel span
// stack + conversation/session/user ids live in `sema-otel` thread-locals, so a
// task that parks mid-span and yields to a sibling would otherwise share (and
// corrupt) that single stack. The runtime swaps each task's otel context in on
// entry and out on leave.
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

/// Check whether a captured otel task context carries no span/identity state
/// (fast-path predicate for the runtime's `TaskScopeSwap` — see sema-vm
/// `state.rs`). `true` when unregistered (nothing to isolate).
pub type OtelIsEmptyFn = fn(&Box<dyn Any>) -> bool;
/// Peek (no mutation, no allocation) whether the CURRENT thread's otel context
/// is empty, without taking or boxing it.
pub type OtelAmbientEmptyFn = fn() -> bool;

thread_local! {
    static OTEL_TAKE_CALLBACK: Cell<Option<OtelTakeFn>> = const { Cell::new(None) };
    static OTEL_INSTALL_CALLBACK: Cell<Option<OtelInstallFn>> = const { Cell::new(None) };
    static OTEL_SCOPE_CALLBACK: Cell<Option<OtelScopeFn>> = const { Cell::new(None) };
    static OTEL_IS_EMPTY_CALLBACK: Cell<Option<OtelIsEmptyFn>> = const { Cell::new(None) };
    static OTEL_AMBIENT_EMPTY_CALLBACK: Cell<Option<OtelAmbientEmptyFn>> = const { Cell::new(None) };
}

/// Register the per-task otel take/install/scope callbacks. Called once at
/// startup by `sema_otel::register_task_callbacks()`.
pub fn set_otel_task_callbacks(take: OtelTakeFn, install: OtelInstallFn, scope: OtelScopeFn) {
    OTEL_TAKE_CALLBACK.with(|cb| cb.set(Some(take)));
    OTEL_INSTALL_CALLBACK.with(|cb| cb.set(Some(install)));
    OTEL_SCOPE_CALLBACK.with(|cb| cb.set(Some(scope)));
}

/// Register the otel empty-scope fast-path predicates. Called once at startup
/// by `sema_otel::register_task_callbacks()` alongside [`set_otel_task_callbacks`].
pub fn set_otel_empty_callbacks(is_empty: OtelIsEmptyFn, ambient_empty: OtelAmbientEmptyFn) {
    OTEL_IS_EMPTY_CALLBACK.with(|cb| cb.set(Some(is_empty)));
    OTEL_AMBIENT_EMPTY_CALLBACK.with(|cb| cb.set(Some(ambient_empty)));
}

/// Whether a captured otel task context is empty (no spans, no identity). `true`
/// when no callback is registered (nothing to isolate).
pub fn otel_captured_is_empty(ctx: &Box<dyn Any>) -> bool {
    match OTEL_IS_EMPTY_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(ctx),
        None => true,
    }
}

/// Whether the CURRENT thread's otel context is empty. `true` when no callback
/// is registered.
pub fn otel_ambient_is_empty() -> bool {
    match OTEL_AMBIENT_EMPTY_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(),
        None => true,
    }
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

/// Check whether a captured leaf-usage scope carries no active accumulator
/// (fast-path predicate for the runtime's `TaskScopeSwap`). `true` when
/// unregistered (nothing to isolate).
pub type UsageScopeIsEmptyFn = fn(&Box<dyn Any>) -> bool;
/// Peek (no mutation, no allocation) whether the CURRENT thread's active
/// leaf-usage scope is empty, without taking or boxing it.
pub type UsageScopeAmbientEmptyFn = fn() -> bool;

thread_local! {
    static USAGE_SCOPE_CAPTURE_CALLBACK: Cell<Option<UsageScopeCaptureFn>> = const { Cell::new(None) };
    static USAGE_SCOPE_TAKE_CALLBACK: Cell<Option<UsageScopeTakeFn>> = const { Cell::new(None) };
    static USAGE_SCOPE_INSTALL_CALLBACK: Cell<Option<UsageScopeInstallFn>> = const { Cell::new(None) };
    static USAGE_SCOPE_IS_EMPTY_CALLBACK: Cell<Option<UsageScopeIsEmptyFn>> = const { Cell::new(None) };
    static USAGE_SCOPE_AMBIENT_EMPTY_CALLBACK: Cell<Option<UsageScopeAmbientEmptyFn>> = const { Cell::new(None) };
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

/// Register the usage-scope empty fast-path predicates. Called once at startup
/// by `sema-llm` alongside [`set_usage_scope_task_callbacks`].
pub fn set_usage_scope_empty_callbacks(
    is_empty: UsageScopeIsEmptyFn,
    ambient_empty: UsageScopeAmbientEmptyFn,
) {
    USAGE_SCOPE_IS_EMPTY_CALLBACK.with(|cb| cb.set(Some(is_empty)));
    USAGE_SCOPE_AMBIENT_EMPTY_CALLBACK.with(|cb| cb.set(Some(ambient_empty)));
}

/// Whether a captured leaf-usage scope is empty (no active accumulator). `true`
/// when no callback is registered.
pub fn usage_scope_captured_is_empty(ctx: &Box<dyn Any>) -> bool {
    match USAGE_SCOPE_IS_EMPTY_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(ctx),
        None => true,
    }
}

/// Whether the CURRENT thread's active leaf-usage scope is empty. `true` when
/// no callback is registered.
pub fn usage_scope_ambient_is_empty() -> bool {
    match USAGE_SCOPE_AMBIENT_EMPTY_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(),
        None => true,
    }
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
// which the cooperative runtime can defer past the reset — so the flags leak
// across the task boundary (cache misses under-counted, and a `with-budget` cap
// failing to gate a concurrent fan-out). Like the otel context and the leaf-usage
// scope above, the runtime captures this dynamic scope at task spawn and swaps it
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

/// Check whether a captured LLM dynamic scope carries no overrides (cache off,
/// no tags/metadata, no active budget) — fast-path predicate for the runtime's
/// `TaskScopeSwap`. `true` when unregistered (nothing to isolate).
pub type LlmScopeIsEmptyFn = fn(&Box<dyn Any>) -> bool;
/// Peek (no mutation, no allocation) whether the CURRENT thread's LLM dynamic
/// scope is empty/default, without taking or boxing it.
pub type LlmScopeAmbientEmptyFn = fn() -> bool;

thread_local! {
    static LLM_SCOPE_CAPTURE_CALLBACK: Cell<Option<LlmScopeCaptureFn>> = const { Cell::new(None) };
    static LLM_SCOPE_TAKE_CALLBACK: Cell<Option<LlmScopeTakeFn>> = const { Cell::new(None) };
    static LLM_SCOPE_INSTALL_CALLBACK: Cell<Option<LlmScopeInstallFn>> = const { Cell::new(None) };
    static LLM_SCOPE_IS_EMPTY_CALLBACK: Cell<Option<LlmScopeIsEmptyFn>> = const { Cell::new(None) };
    static LLM_SCOPE_AMBIENT_EMPTY_CALLBACK: Cell<Option<LlmScopeAmbientEmptyFn>> = const { Cell::new(None) };
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

/// Register the LLM-scope empty fast-path predicates. Called once at startup by
/// `sema-llm` alongside [`set_llm_scope_task_callbacks`].
pub fn set_llm_scope_empty_callbacks(
    is_empty: LlmScopeIsEmptyFn,
    ambient_empty: LlmScopeAmbientEmptyFn,
) {
    LLM_SCOPE_IS_EMPTY_CALLBACK.with(|cb| cb.set(Some(is_empty)));
    LLM_SCOPE_AMBIENT_EMPTY_CALLBACK.with(|cb| cb.set(Some(ambient_empty)));
}

/// Whether a captured LLM dynamic scope is empty (no cache/budget/tags
/// overrides). `true` when no callback is registered.
pub fn llm_scope_captured_is_empty(ctx: &Box<dyn Any>) -> bool {
    match LLM_SCOPE_IS_EMPTY_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(ctx),
        None => true,
    }
}

/// Whether the CURRENT thread's LLM dynamic scope is empty/default. `true`
/// when no callback is registered.
pub fn llm_scope_ambient_is_empty() -> bool {
    match LLM_SCOPE_AMBIENT_EMPTY_CALLBACK.with(|cb| cb.get()) {
        Some(f) => f(),
        None => true,
    }
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
