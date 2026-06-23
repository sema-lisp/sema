//! Cooperative task scheduler for async concurrency.
//!
//! Manages multiple VM instances, each running an async task. Tasks yield
//! cooperatively via the `YieldReason` signal mechanism defined in `sema_core::async_signal`.
//!
//! # Architecture
//!
//! Each async task gets its own VM instance sharing globals and functions
//! with the parent. Native functions signal yield via `set_yield_signal(reason)`
//! and return `Ok(Value::nil())`. The VM checks `take_yield_signal()` after
//! native calls and returns `VmExecResult::AsyncYield(reason)`. On resume,
//! the scheduler sets `set_resume_value(val)` before re-running the VM.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use sema_core::{
    in_async_context, set_async_context, set_cancel_callback, set_run_scheduler_callback,
    set_spawn_callback, AsyncPromise, Env, EvalContext, PromiseState, SchedulerRunResult,
    SchedulerTarget, SemaError, Spur, Value, YieldReason,
};

use crate::debug::VmExecResult;
use crate::vm::{self, Closure, VM};

// ── Task types ─────────────────────────────────────────────────────

/// Current state of an async task.
enum TaskState {
    /// Ready to run (or resume).
    Ready,
    /// Blocked waiting on an external event.
    Blocked(YieldReason),
    /// Completed successfully.
    Done,
    /// Completed with an error.
    Failed,
}

/// A single async task managed by the scheduler.
#[allow(dead_code)]
struct Task {
    id: u64,
    vm: VM,
    closure: Rc<Closure>,
    promise: Rc<AsyncPromise>,
    state: TaskState,
    /// Whether `execute_async` has been called (false = first run).
    started: bool,
    /// Whether this task has been cancelled.
    cancelled: bool,
    /// Value to pass to the VM on resume (set by `wake_blocked_tasks`).
    resume_value: Option<Value>,
    /// Logical wake time (in scheduler virtual-clock milliseconds) for a
    /// sleep-blocked task; `None` when the task is not sleeping. The task
    /// resumes once `Scheduler::virtual_now >= wake_at`. See the module-level
    /// note on the virtual clock.
    wake_at: Option<u64>,
    /// This task's saved OTel context (span stack + conversation/session/user
    /// ids), type-erased as `Box<dyn Any>` so `sema-vm` need not depend on
    /// `sema-otel`. It is swapped into the otel thread-locals on task entry and
    /// taken back out (capturing any spans opened during the step) on task
    /// leave, so a task that parks mid-span cannot corrupt a sibling's stack.
    /// Seeded at spawn from `current_conversation_scope()` (ids only, empty
    /// stack) so conversation grouping survives the per-task isolation.
    otel: Box<dyn Any>,
}

// ── Scheduler ──────────────────────────────────────────────────────

/// Cooperative task scheduler managing multiple VM-backed async tasks.
pub struct Scheduler {
    tasks: Vec<Task>,
    next_id: u64,
    /// Shared global environment for spawning child VMs.
    globals: Rc<Env>,
    /// Native function spurs for resolving the native dispatch table in child VMs.
    native_spurs: Vec<Spur>,
    /// Virtual clock in milliseconds (see the module-level note). `async/sleep`
    /// and `async/timeout` are measured against this logical clock, not the
    /// wall clock. It only advances when no task can make progress (every task
    /// is blocked), at which point it jumps to the nearest pending deadline —
    /// so ordering by duration is exact and deterministic on every platform.
    /// On native targets the scheduler also waits the corresponding real time
    /// when it advances, preserving wall-clock pacing for CLI scripts; in WASM
    /// (where blocking the UI thread is forbidden) it advances instantly.
    virtual_now: u64,
}

impl Scheduler {
    /// Create a new scheduler with shared state from the parent VM.
    pub fn new(globals: Rc<Env>, native_spurs: Vec<Spur>) -> Self {
        Scheduler {
            tasks: Vec::new(),
            next_id: 1,
            globals,
            native_spurs,
            virtual_now: 0,
        }
    }

    /// Update shared VM context for future tasks without discarding existing tasks.
    fn update_context(&mut self, native_spurs: Vec<Spur>) {
        self.native_spurs = native_spurs;
    }

    /// Spawn a new async task from a thunk (zero-argument VM closure).
    ///
    /// Extracts the compiled closure from the thunk value, creates a
    /// dedicated VM for the task, and returns a promise that will be
    /// resolved when the task completes.
    pub fn spawn(
        &mut self,
        thunk: Value,
        _ctx: &EvalContext,
    ) -> Result<Rc<AsyncPromise>, SemaError> {
        let (closure, functions) = vm::extract_vm_closure(&thunk).ok_or_else(|| {
            SemaError::eval("async/spawn: argument must be a function (compiled VM closure)")
        })?;

        // The closure will run on a dedicated task VM whose stack differs from
        // the spawning VM's. Snapshot any still-open upvalue cells against the
        // spawning VM now, while it is paused, so they don't dangle on the task
        // VM stack (C1: keeping cells open across in-VM HOF calls means they may
        // still be Open here).
        vm::close_closure_upvalues_for_foreign_run(&closure);

        let id = self.next_id;
        self.next_id += 1;

        let promise = Rc::new(AsyncPromise {
            state: RefCell::new(PromiseState::Pending),
            task_id: std::cell::Cell::new(id),
        });

        // Use the function table from the thunk's own compilation context,
        // not the scheduler's — each eval_str_compiled produces different functions.
        let vm = VM::new_for_task(self.globals.clone(), functions, &self.native_spurs)?;

        self.tasks.push(Task {
            id,
            vm,
            closure,
            promise: promise.clone(),
            state: TaskState::Ready,
            started: false,
            cancelled: false,
            resume_value: None,
            wake_at: None,
            otel: sema_core::current_conversation_scope_boxed(),
        });

        Ok(promise)
    }

    /// Check blocked tasks and transition them to Ready if their
    /// blocking condition has been satisfied.
    fn wake_blocked_tasks(&mut self) {
        /// Result of checking a blocked task's wake condition.
        enum WakeAction {
            /// Still blocked — no change.
            Pending,
            /// Resume the task with this value.
            Resume(Value),
            /// Fail the task with this rejection message.
            Fail(String),
        }

        let now = self.virtual_now;
        for task in &mut self.tasks {
            if task.cancelled {
                if !matches!(task.state, TaskState::Done | TaskState::Failed) {
                    *task.promise.state.borrow_mut() = PromiseState::Cancelled;
                    task.state = TaskState::Failed;
                }
                continue;
            }
            if let TaskState::Blocked(ref reason) = task.state {
                let action = match reason {
                    YieldReason::AwaitPromise(p) => {
                        let state = p.state.borrow();
                        match &*state {
                            PromiseState::Resolved(v) => WakeAction::Resume(v.clone()),
                            PromiseState::Rejected(e) => WakeAction::Fail(e.clone()),
                            PromiseState::Cancelled => {
                                WakeAction::Fail("awaited task was cancelled".to_string())
                            }
                            PromiseState::Pending => WakeAction::Pending,
                        }
                    }
                    YieldReason::ChannelRecv(ch) => {
                        let mut buf = ch.buffer.borrow_mut();
                        if let Some(v) = buf.pop_front() {
                            WakeAction::Resume(v)
                        } else if ch.closed.get() {
                            WakeAction::Resume(Value::nil())
                        } else {
                            WakeAction::Pending
                        }
                    }
                    YieldReason::ChannelSend(ch, val) => {
                        if ch.closed.get() {
                            WakeAction::Fail(format!(
                                "channel/send: closed while sending {val}; value dropped (send was pending)"
                            ))
                        } else {
                            let mut buf = ch.buffer.borrow_mut();
                            if buf.len() < ch.capacity {
                                buf.push_back(val.clone());
                                WakeAction::Resume(Value::nil())
                            } else {
                                WakeAction::Pending
                            }
                        }
                    }
                    YieldReason::Sleep(ms) => {
                        // Arm the logical wake time on first sight, then resume
                        // once the virtual clock has reached it. Time only
                        // advances when every task is blocked (see
                        // `run_until_reentrant`), so a shorter sleep always
                        // wakes before a longer one regardless of spawn order.
                        let wake_at = *task.wake_at.get_or_insert(now.saturating_add(*ms));
                        if now >= wake_at {
                            task.wake_at = None;
                            WakeAction::Resume(Value::nil())
                        } else {
                            WakeAction::Pending
                        }
                    }
                    YieldReason::AwaitIo(h) => match h.poll() {
                        // Poll the offloaded future without blocking. The VM
                        // thread only ever parks in the all-blocked branch of
                        // `run_until_reentrant` (on `io_park`), never here.
                        sema_core::IoPoll::Pending => WakeAction::Pending,
                        sema_core::IoPoll::Ready(Ok(v)) => WakeAction::Resume(v),
                        sema_core::IoPoll::Ready(Err(msg)) => WakeAction::Fail(msg),
                    },
                };

                match action {
                    WakeAction::Resume(val) => {
                        task.resume_value = Some(val);
                        task.state = TaskState::Ready;
                    }
                    WakeAction::Fail(msg) => {
                        *task.promise.state.borrow_mut() = PromiseState::Rejected(msg);
                        task.state = TaskState::Failed;
                    }
                    WakeAction::Pending => {}
                }
            }
        }
    }

    /// Drop every task the scheduler is still holding.
    ///
    /// Called at the OUTERMOST scheduler exit (control returning to non-async
    /// Sema code), where any leftover task is genuinely abandoned: there is no
    /// surviving awaiter and the next top-level eval starts a fresh run. Dropping
    /// them HERE — on the VM thread, while the OTel thread-locals are still alive —
    /// is what prevents a span-owning `IoHandle` (e.g. an `llm/embed` abandoned by
    /// `async/timeout`) from surviving to thread/process teardown, where its
    /// detached `LlmSpan` would call `span.end()` against a destructed thread-local
    /// and abort the process (adversarial #7).
    ///
    /// Tasks of a still-active NESTED run are never touched: this only runs when
    /// the caller is returning to a non-async context (see `run_scheduler_callback`).
    /// After a normal `(async/all …)` the scheduler holds no tasks, so this is a
    /// no-op there.
    fn reap_leftover_tasks(&mut self) {
        self.tasks.clear();
    }

    /// Mark a task as cancelled and transition its promise into `Cancelled`.
    /// Returns true if this call actually transitioned the task into the
    /// cancelled state, false if the task was already terminal (Done /
    /// Failed / previously Cancelled) or if no task with that id exists.
    fn cancel_task(&mut self, task_id: u64) -> Result<bool, SemaError> {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) else {
            // Task already completed and was pruned — no transition.
            return Ok(false);
        };
        if matches!(task.state, TaskState::Done | TaskState::Failed) || task.cancelled {
            return Ok(false);
        }
        task.cancelled = true;
        *task.promise.state.borrow_mut() = PromiseState::Cancelled;
        task.state = TaskState::Failed;
        Ok(true)
    }
}

// ── Thread-local scheduler ─────────────────────────────────────────

thread_local! {
    static SCHEDULER: RefCell<Option<Scheduler>> = const { RefCell::new(None) };
}

/// Test/observability hook: the number of tasks the thread-local scheduler is
/// currently holding (Ready / Blocked / terminal-not-yet-reaped). 0 when no
/// scheduler is initialized. Used by the abandoned-task-reaping gate to prove a
/// stranded `AwaitIo` task (e.g. an embed abandoned by `async/timeout`) was reaped
/// during the run rather than surviving to thread/process teardown.
pub fn scheduler_task_count() -> usize {
    SCHEDULER.with(|s| s.borrow().as_ref().map_or(0, |sched| sched.tasks.len()))
}

/// Take the scheduler out of the thread-local temporarily.
/// The caller MUST put it back via `put_scheduler`.
fn take_scheduler() -> Result<Scheduler, SemaError> {
    SCHEDULER.with(|s| s.borrow_mut().take()).ok_or_else(|| {
        SemaError::eval("async scheduler not initialized (call init_scheduler first)")
    })
}

/// Put the scheduler back into the thread-local.
fn put_scheduler(sched: Scheduler) {
    SCHEDULER.with(|s| *s.borrow_mut() = Some(sched));
}

/// Spawn callback registered via `sema_core::set_spawn_callback`.
///
/// Called by the `async/spawn` stdlib function. Takes the scheduler
/// briefly to add the task, then puts it back immediately.
fn spawn_callback(ctx: &EvalContext, thunk: Value) -> Result<Value, SemaError> {
    let mut sched = take_scheduler()?;
    let result = sched.spawn(thunk, ctx);
    put_scheduler(sched);
    let promise = result?;
    Ok(Value::async_promise_from_rc(promise))
}

/// Spawn `closure` with pre-bound `args` as a scheduled task and run the
/// scheduler until it completes. Returns the closure's resolved value, or
/// propagates a rejection as `Err`.
///
/// Used by the VM closure fallback when invoked from a stdlib HOF inside
/// an async task: the inner closure has to run as a real task so its async
/// yield points (channel/send, channel/recv, await, sleep) can suspend
/// cleanly. Without this, the fallback's plain `vm.run` would translate the
/// yield into an "async yield outside of scheduler context" error and the
/// owning task would fail.
pub(crate) fn run_closure_as_inline_task(
    ctx: &EvalContext,
    closure: Rc<crate::vm::Closure>,
    functions: Rc<Vec<Rc<crate::chunk::Function>>>,
    args: &[Value],
) -> Result<Value, SemaError> {
    // The closure runs on a dedicated task VM; snapshot any still-open upvalue
    // cells against the owning (currently paused) VM before crossing onto the
    // foreign task-VM stack (C1).
    crate::vm::close_closure_upvalues_for_foreign_run(&closure);

    let mut sched = take_scheduler()?;

    let id = sched.next_id;
    sched.next_id += 1;
    let promise = Rc::new(AsyncPromise {
        state: RefCell::new(PromiseState::Pending),
        task_id: std::cell::Cell::new(id),
    });

    let mut vm = match VM::new_for_task(sched.globals.clone(), functions, &sched.native_spurs) {
        Ok(vm) => vm,
        Err(e) => {
            put_scheduler(sched);
            return Err(e);
        }
    };
    if let Err(e) = vm.setup_for_call(closure.clone(), args) {
        put_scheduler(sched);
        return Err(e);
    }

    sched.tasks.push(Task {
        id,
        vm,
        closure,
        promise: promise.clone(),
        // Frame already pushed by setup_for_call — scheduler should call
        // run_async (not execute_async) on the first tick.
        started: true,
        state: TaskState::Ready,
        cancelled: false,
        resume_value: None,
        wake_at: None,
        otel: sema_core::current_conversation_scope_boxed(),
    });

    // Re-enter the scheduler with this promise as the wait target. The
    // scheduler is re-entrant: it puts itself back into the thread-local
    // before each task step so nested HOF-callback yields work too.
    let target = sema_core::SchedulerTarget::One(promise.clone());
    let run_result = run_until_reentrant(&mut sched, ctx, &target);
    put_scheduler(sched);
    run_result?;

    let state = promise.state.borrow();
    match &*state {
        PromiseState::Resolved(v) => Ok(v.clone()),
        PromiseState::Rejected(e) => Err(SemaError::eval(e.clone())),
        PromiseState::Cancelled => Err(SemaError::eval("HOF callback task was cancelled")),
        PromiseState::Pending => Err(SemaError::eval(
            "HOF callback did not complete after scheduler run",
        )),
    }
}

/// Run-scheduler callback registered via `sema_core::set_run_scheduler_callback`.
///
/// Takes the scheduler out of the thread-local, runs it, puts it back.
/// During task execution, the scheduler is put back temporarily so that
/// re-entrant calls (nested async/spawn) can access it.
fn run_scheduler_callback(
    ctx: &EvalContext,
    target: SchedulerTarget,
) -> Result<SchedulerRunResult, SemaError> {
    let mut sched = take_scheduler()?;
    let result = run_until_reentrant(&mut sched, ctx, &target);

    // This callback is the registered top-level entry for `async/await`,
    // `async/all`, `async/any`, `async/run` and `async/timeout` — all of which
    // only call it when NOT already in an async context (otherwise they
    // `set_yield_signal` and let the running scheduler resume them). So reaching
    // here means we are the OUTERMOST run returning to non-async Sema code: any
    // task the scheduler still holds is abandoned (a timed-out / un-awaited
    // sibling parked on `AwaitIo` etc.). Reap them now, on the VM thread, while
    // the OTel thread-locals are still alive — never leaving a span-owning
    // `IoHandle` to drop at teardown (adversarial #7). The nested HOF-callback
    // run (`run_closure_as_inline_task`) goes through a different path and is
    // gated by `in_async_context()` being true, so it is never reaped here.
    if !sema_core::in_async_context() {
        sched.reap_leftover_tasks();
    }

    put_scheduler(sched);
    result
}

struct RunGoal<'a> {
    target: &'a SchedulerTarget,
    /// Logical deadline (scheduler virtual-clock ms) for a `Timeout` target;
    /// `None` for non-timeout goals. Measured against `Scheduler::virtual_now`.
    deadline: Option<u64>,
}

impl<'a> RunGoal<'a> {
    /// `start` is the scheduler's virtual clock when the goal begins, so a
    /// `Timeout(_, ms)` deadline is `start + ms` in logical time.
    fn new(target: &'a SchedulerTarget, start: u64) -> Self {
        let deadline = match target {
            SchedulerTarget::Timeout(_, ms) => Some(start.saturating_add(*ms)),
            _ => None,
        };
        RunGoal { target, deadline }
    }

    /// Goal status: `Some(Complete)` once the target promise(s) settle. Timing
    /// out is handled separately in the run loop (see `status`'s `Timeout` arm).
    fn status(&self) -> Option<SchedulerRunResult> {
        match self.target {
            SchedulerTarget::All => None,
            SchedulerTarget::One(promise) => {
                (!matches!(&*promise.state.borrow(), PromiseState::Pending))
                    .then_some(SchedulerRunResult::Complete)
            }
            SchedulerTarget::AllOf(promises) => {
                let any_rejected = promises
                    .iter()
                    .any(|p| matches!(&*p.state.borrow(), PromiseState::Rejected(_)));
                let all_done = promises
                    .iter()
                    .all(|p| !matches!(&*p.state.borrow(), PromiseState::Pending));
                (any_rejected || all_done).then_some(SchedulerRunResult::Complete)
            }
            SchedulerTarget::AnyOf(promises) => promises
                .iter()
                .any(|p| !matches!(&*p.state.borrow(), PromiseState::Pending))
                .then_some(SchedulerRunResult::Complete),
            SchedulerTarget::Timeout(promise, _) => {
                // Only the *resolved* case is decided here. Timing out is decided
                // in the run loop's all-blocked branch (after ready tasks have had
                // a turn and the virtual clock has actually advanced to the
                // deadline), so a 0 ms / very short timeout still lets work that is
                // synchronously ready complete instead of tripping pre-emptively.
                (!matches!(&*promise.state.borrow(), PromiseState::Pending))
                    .then_some(SchedulerRunResult::Complete)
            }
        }
    }

    /// Logical deadline to clamp virtual-time advancement against, if any.
    fn sleep_limit(&self) -> Option<u64> {
        self.deadline
    }
}

/// Scope guard that re-installs the scheduler into the caller's `&mut Scheduler`
/// after a task step, re-pushing the in-flight task if it is still active.
///
/// During a task step the real scheduler lives in the thread-local (so nested
/// async calls can reach it) while `*sched` holds an empty dummy. If the step
/// panics, this guard's `Drop` takes the scheduler back out of the thread-local
/// and writes it into `*sched` (re-pushing the running task), preventing the
/// empty dummy from being stranded there and the running task from being
/// silently dropped (VM-5). On the normal path the loop calls `reinstall`
/// explicitly so that a missing scheduler surfaces as an error.
struct ReinstallGuard<'a> {
    sched: &'a mut Scheduler,
    task: Option<Task>,
    /// True until `reinstall` runs; gates the Drop fallback so it only fires on
    /// an unexpected unwind (not after the normal-path reinstall).
    armed: bool,
    /// The otel context displaced when this task's context was installed into
    /// the thread-locals on task entry. Restored on leave (normal path AND
    /// panic unwind) so a sibling/parent's otel stack + ids are never corrupted
    /// by a task that parked mid-span. `None` once consumed.
    prev_otel: Option<Box<dyn Any>>,
}

impl ReinstallGuard<'_> {
    /// Swap the per-task otel context back out of the thread-locals — capturing
    /// any spans the task opened during its step into `task.otel` — and restore
    /// the otel context that was active before this task ran. Idempotent: a
    /// second call (Drop after the explicit reinstall) is a no-op.
    fn restore_otel(&mut self) {
        let Some(prev) = self.prev_otel.take() else {
            return;
        };
        // Take this task's (possibly mid-span) otel context out and stash it on
        // the task so it resumes with the same stack/ids next step.
        let task_otel = sema_core::take_task_otel();
        if let Some(task) = self.task.as_mut() {
            task.otel = task_otel;
        }
        // Restore the previously-active otel context.
        let _ = sema_core::install_task_otel(prev);
    }

    /// Take the scheduler back out of the thread-local, re-push the in-flight
    /// task (unless it reached a terminal state), drop terminal tasks left over
    /// from cancellations, and write the result into `*self.sched`. Disarms the
    /// guard so Drop does not reinstall a second time.
    fn reinstall(&mut self) -> Result<(), SemaError> {
        // Restore otel FIRST (while `self.task` is still owned by the guard) so
        // a parked task carries its span stack and the prev context is live
        // again before we re-push tasks into the scheduler.
        self.restore_otel();
        self.armed = false;
        let mut s = take_scheduler()?;
        if let Some(task) = self.task.take() {
            if !matches!(task.state, TaskState::Done | TaskState::Failed) {
                s.tasks.push(task);
            }
        }
        // Also drop terminal tasks left by cancelled tasks pushed earlier.
        s.tasks
            .retain(|t| !matches!(t.state, TaskState::Done | TaskState::Failed));
        *self.sched = s;
        Ok(())
    }
}

impl Drop for ReinstallGuard<'_> {
    fn drop(&mut self) {
        // If `reinstall` already ran on the normal path the guard is disarmed
        // and the scheduler has been taken back; nothing to do — but still make
        // sure the otel context was restored (idempotent).
        if !self.armed {
            self.restore_otel();
            return;
        }
        // Best-effort during unwind: if the scheduler is somehow absent we
        // cannot do anything useful, so leave `*self.sched` as-is. `reinstall`
        // restores the otel context first.
        let _ = self.reinstall();
    }
}

/// Run the scheduler event loop with re-entrant safety.
///
/// Before each task step, the scheduler is put back into the thread-local
/// so that nested `async/spawn` and `async/await` calls from within
/// task VMs can access it. After each step, the scheduler is taken back
/// out (it may have new tasks added by the step).
fn run_until_reentrant(
    sched: &mut Scheduler,
    ctx: &EvalContext,
    target: &SchedulerTarget,
) -> Result<SchedulerRunResult, SemaError> {
    const MAX_TICKS: u64 = 1_000_000;
    let goal = RunGoal::new(target, sched.virtual_now);

    for _ in 0..MAX_TICKS {
        // Honor cancellation between task steps too, so a Stop during an async
        // wait (e.g. a real `Atomics.wait` sleep that was woken early) aborts
        // promptly rather than only at the next VM loop back-edge. Drop all
        // pending tasks so a cancelled run leaves no dangling/sleeping tasks to
        // resurrect on a later eval.
        if sema_core::check_interrupt() {
            sched.tasks.clear();
            return Err(SemaError::eval("evaluation cancelled".to_string()));
        }

        if let Some(result) = goal.status() {
            return Ok(result);
        }

        sched.wake_blocked_tasks();

        if let Some(result) = goal.status() {
            return Ok(result);
        }

        let ready_idx = sched
            .tasks
            .iter()
            .position(|t| matches!(t.state, TaskState::Ready));

        let Some(idx) = ready_idx else {
            let has_blocked = sched
                .tasks
                .iter()
                .any(|t| matches!(t.state, TaskState::Blocked(_)));
            if has_blocked {
                // Park-on-IO short-circuit (checked BEFORE the virtual-clock
                // advance below): if at least one task is parked on an offloaded
                // `AwaitIo` future and no task is Ready, the VM thread should
                // wait for the real I/O to land — NOT jump the virtual clock
                // (which would force-fail the in-flight task via an
                // `async/timeout` deadline, B3). We re-check Ready each pass
                // (the `ready_idx` guard above already established none are
                // Ready here), so any runnable VM/tool work always pre-empts the
                // park on the next iteration.
                let has_await_io = sched
                    .tasks
                    .iter()
                    .any(|t| matches!(t.state, TaskState::Blocked(YieldReason::AwaitIo(_))));
                if has_await_io {
                    // Park on the process-global IO-completion signal for a
                    // small bound. `io_park` returns early on a completion
                    // notify; the bound keeps the `check_interrupt` cadence at
                    // the top of the loop live (so Ctrl-C still works) and
                    // bounds any missed notify. Then loop back to re-run
                    // `wake_blocked_tasks`, which polls the IO handles.
                    //
                    // Crucially, while IO is pending we keep the VIRTUAL CLOCK
                    // and TIMEOUTS live by advancing `virtual_now` by the REAL
                    // time we parked. The old `io_park(50); continue;` skipped
                    // this and starved any concurrent `async/sleep` sleeper
                    // (#2) and disabled `async/timeout` (#3) for the duration of
                    // the in-flight IO. We compute the nearest deadline exactly
                    // as the sleeper path below does, clamp the park to it (so
                    // we don't overshoot a near sleeper) and to 50 ms (so the
                    // `check_interrupt` cadence stays ~50 ms for Ctrl-C), park
                    // for real time, then advance `virtual_now` by the measured
                    // elapsed so sleepers wake and the timeout fires.
                    let next_sleep = sched
                        .tasks
                        .iter()
                        .filter(|t| matches!(t.state, TaskState::Blocked(YieldReason::Sleep(_))))
                        .filter_map(|t| t.wake_at)
                        .min();
                    let next = match (next_sleep, goal.sleep_limit()) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (Some(a), None) | (None, Some(a)) => Some(a),
                        (None, None) => None,
                    };
                    let to_deadline = next
                        .map(|t| t.saturating_sub(sched.virtual_now))
                        .unwrap_or(u64::MAX);
                    let park_ms = to_deadline.clamp(1, 50);

                    let t0 = std::time::Instant::now();
                    sema_core::io_park(park_ms);
                    let elapsed = t0.elapsed().as_millis() as u64;

                    // While IO is pending, virtual time tracks real time so
                    // sleepers wake (via `wake_blocked_tasks` at the loop top
                    // when `virtual_now >= wake_at`) and timeouts fire.
                    sched.virtual_now = sched.virtual_now.saturating_add(elapsed.max(1));

                    // Replicate the sleeper path's timeout check: if we've
                    // reached the `async/timeout` deadline with the target still
                    // pending, the operation has timed out (#3).
                    if goal.sleep_limit().is_some_and(|dl| sched.virtual_now >= dl) {
                        return Ok(SchedulerRunResult::TimedOut);
                    }
                    continue;
                }

                // Nothing can make progress right now, so advance the virtual
                // clock to the nearest pending deadline: the earliest sleeper's
                // wake time, clamped by any `async/timeout` deadline. Jumping to
                // that instant (rather than polling) is what makes sleep
                // ordering exact and deterministic. A sleeping task may unblock
                // channel/promise waiters when it resumes, so advancing time is
                // not a deadlock.
                let next_sleep = sched
                    .tasks
                    .iter()
                    .filter(|t| matches!(t.state, TaskState::Blocked(YieldReason::Sleep(_))))
                    .filter_map(|t| t.wake_at)
                    .min();
                let next = match (next_sleep, goal.sleep_limit()) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                if let Some(target_time) = next {
                    // Pace this clock advance in real wall-clock time. Default:
                    // sleep the OS thread on native; instant no-op in wasm (the
                    // UI thread must not block — advancing virtual_now below is
                    // enough for deterministic ordering). The playground Web
                    // Worker installs a blocking-sleep callback (Atomics.wait on
                    // a SharedArrayBuffer) to get real pacing in wasm too.
                    // `delta` is bounded ≤ ~1 day by the async/sleep +
                    // async/timeout caps, so it can never wedge for years.
                    let delta = target_time.saturating_sub(sched.virtual_now);
                    if delta > 0 {
                        sema_core::blocking_sleep_ms(delta);
                    }
                    sched.virtual_now = target_time;

                    // If we've reached the timeout deadline with the target still
                    // pending, the operation has timed out. Decided here (not in
                    // `status`) so a 0 ms timeout still lets synchronously-ready
                    // work finish before this point is ever reached.
                    if goal.sleep_limit().is_some_and(|dl| sched.virtual_now >= dl) {
                        return Ok(SchedulerRunResult::TimedOut);
                    }
                    continue; // Re-check: wake sleepers / make progress.
                }
                // No sleepers and no timeout — genuinely stuck (e.g. awaiting a
                // promise that can never resolve).
                return Err(SemaError::eval(
                    "async scheduler: all tasks blocked (deadlock detected)",
                ));
            }
            return Ok(SchedulerRunResult::Complete);
        };

        // Extract the task from the scheduler, put the scheduler back
        // into the thread-local, then run the task. This allows nested
        // async/spawn and async/await inside the task VM to access the
        // scheduler via the thread-local.
        //
        // We use `Vec::remove` (O(n)) rather than `swap_remove` (O(1)) so
        // that ready-task pickup preserves spawn order. swap_remove rotates
        // the queue and produces a LIFO-ish surface that surprises users
        // writing FIFO pipelines (e.g. (1 3 2) instead of (1 2 3) for three
        // sequential channel sends followed by sequential receives).
        // Task lists are typically small (<100 tasks), so the O(n) cost
        // is negligible.
        let mut task = sched.tasks.remove(idx);

        // Check if task was cancelled before running it
        if task.cancelled {
            *task.promise.state.borrow_mut() = PromiseState::Cancelled;
            task.state = TaskState::Failed;
            sched.tasks.push(task);
            continue;
        }

        // Move the real scheduler into the thread-local while the task runs so
        // that nested async/spawn etc. reach it through `take_scheduler`. A
        // scope guard owns the in-flight task and re-installs the scheduler
        // into `*sched` on Drop — including during a panic unwind. Without it,
        // a panic in task execution left the empty dummy in `*sched` and
        // silently dropped the running task, deadlocking callers (VM-5).
        let taken = std::mem::replace(sched, Scheduler::new(Rc::new(Env::new()), Vec::new()));
        put_scheduler(taken);
        // Install this task's otel context (span stack + conversation/session/
        // user ids) into the thread-locals for the duration of its step, holding
        // the displaced context in the guard so it is restored on leave — even
        // on a panic unwind. This keeps concurrent tasks' otel state isolated:
        // a task that parks mid-span carries that stack on its own `task.otel`
        // and never corrupts a sibling's.
        let task_otel = std::mem::replace(&mut task.otel, Box::new(()));
        let prev_otel = sema_core::install_task_otel(task_otel);
        let mut guard = ReinstallGuard {
            sched,
            task: Some(task),
            armed: true,
            prev_otel: Some(prev_otel),
        };
        let task = guard.task.as_mut().expect("in-flight task present");

        // Run the extracted task
        if let Some(val) = task.resume_value.take() {
            task.vm.replace_stack_top(val);
        }
        let prev_async = in_async_context();
        set_async_context(true);
        let result = if !task.started {
            task.started = true;
            task.vm.execute_async(task.closure.clone(), ctx)
        } else {
            task.vm.run_async(ctx)
        };
        set_async_context(prev_async);

        match result {
            Ok(VmExecResult::Finished(val)) => {
                *task.promise.state.borrow_mut() = PromiseState::Resolved(val);
                task.state = TaskState::Done;
            }
            Ok(VmExecResult::AsyncYield(reason)) => {
                task.state = TaskState::Blocked(reason);
            }
            Ok(_) => {}
            Err(e) => {
                *task.promise.state.borrow_mut() = PromiseState::Rejected(format!("{e}"));
                task.state = TaskState::Failed;
            }
        }

        // Reinstall the scheduler on the normal path. We do this explicitly
        // (rather than relying solely on the guard's Drop) so that a failure
        // to re-take the scheduler surfaces as an error instead of being
        // swallowed during unwind. `reinstall` disarms the guard so its Drop
        // does not run the reinstall a second time.
        guard.reinstall()?;
    }

    Err(SemaError::eval(
        "async scheduler: exceeded maximum ticks (possible infinite loop)",
    ))
}

/// Cancel callback registered via `sema_core::set_cancel_callback`.
///
/// Called by the `async/cancel` stdlib function. Takes the scheduler
/// briefly to cancel the task, then puts it back immediately.
fn cancel_callback(task_id: u64) -> Result<bool, SemaError> {
    let mut sched = take_scheduler()?;
    let result = sched.cancel_task(task_id);
    put_scheduler(sched);
    result
}

/// Initialize the thread-local scheduler and register the spawn/run callbacks.
///
/// Must be called before any async operations. Typically called once
/// during VM startup with the global environment and function table
/// from the compiled program.
pub fn init_scheduler(globals: Rc<Env>, native_spurs: Vec<Spur>) {
    SCHEDULER.with(|s| {
        let mut slot = s.borrow_mut();
        match slot.as_mut() {
            Some(sched) if Rc::ptr_eq(&sched.globals, &globals) => {
                sched.update_context(native_spurs);
            }
            _ => {
                *slot = Some(Scheduler::new(globals, native_spurs));
            }
        }
    });
    set_spawn_callback(spawn_callback);
    set_run_scheduler_callback(run_scheduler_callback);
    set_cancel_callback(cancel_callback);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VM-5: a panic during a task step must not strand the empty dummy
    /// scheduler in `*sched`. The `ReinstallGuard` should recover the real
    /// scheduler from the thread-local on unwind so callers don't lose tasks.
    #[test]
    fn test_reinstall_guard_restores_scheduler_on_panic() {
        // A real scheduler with a distinguishing marker, held by the "caller".
        let mut sched = Scheduler::new(Rc::new(Env::new()), Vec::new());
        sched.next_id = 4242;

        let recovered_marker = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Mirror the loop body: swap the real scheduler into the
            // thread-local while an (absent) task runs, guarded by ReinstallGuard.
            let taken =
                std::mem::replace(&mut sched, Scheduler::new(Rc::new(Env::new()), Vec::new()));
            put_scheduler(taken);
            let _guard = ReinstallGuard {
                sched: &mut sched,
                task: None,
                armed: true,
                prev_otel: None,
            };
            // Simulate a panic mid-step (e.g. an `unreachable!()` in the VM).
            panic!("simulated task panic");
        }));

        assert!(recovered_marker.is_err(), "panic should propagate");

        // The thread-local must be empty now (the guard took the scheduler
        // back out), and `sched` must hold the original real scheduler — not
        // the empty dummy that was swapped in.
        assert!(
            SCHEDULER.with(|s| s.borrow().is_none()),
            "scheduler should have been taken back out of the thread-local"
        );
        assert_eq!(
            sched.next_id, 4242,
            "the real scheduler (next_id=4242) must be restored into *sched, not the empty dummy"
        );
    }
}
