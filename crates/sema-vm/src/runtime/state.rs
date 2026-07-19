//! Interpreter-owned runtime state and root lifecycle.

use std::cell::RefCell;
use std::collections::VecDeque;

use hashbrown::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

// See the comment on this same import in `host_api.rs`: a wasm32-safe
// `Instant` substitute used throughout `crate::runtime`.
use web_time::Instant;

use crate::vm::{
    close_closure_upvalues_for_foreign_run, close_closure_upvalues_with_owner,
    snapshot_escaping_call_with_owner, snapshot_native_escaping_args_with_owner,
};
use crate::{
    extract_vm_closure, Closure, Function, VmExecResult, VmPendingOutcome, VmQuantumResult, VM,
};
#[cfg(test)]
use sema_core::runtime::ExternalFailure;
use sema_core::runtime::{
    multimethod_call, CancelReason, CancellationView, IdCounter, IoExecutor, NativeCall,
    NativeCallContext, NativeContinuation, NativeOutcome, NativeResult, ResourceGateCloseError,
    ResourceGateHandle, ResourceGateId, ResumeInput, RootId, RuntimeId, RuntimeRequest,
    RuntimeResponse, RuntimeScopedIdCounter, RuntimeTaskId, SettlementSeq, TaskContextHandle,
    TaskId, TaskOutcome, TaskSettlement, Trace, WaitKind,
};
use sema_core::runtime::{CancellationParent, LifetimeOwner, TaskRelations};
use sema_core::{Env, EvalContext, NativeFn, Value};

use super::channel::{ChannelClose, ChannelWake};
use super::wait::RuntimeCommand;
#[cfg(test)]
use super::RootHandle;
use super::{
    AcquireResult, ChannelRegistry, ContinuationFrame, DriveBudget, DriveState, GateResult,
    PendingResume, PromiseRegistry, PromiseState, ReadyScheduler, RegisterExternalError,
    RegistryError, ResourceGateRegistry, ResourceGateWake, RootRecord, RootState, RuntimeClock,
    RuntimeCreateError, TaskRecord, TimerQueue, WaitRuntime,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitRootError {
    IdExhausted,
    ShuttingDown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeFault {
    IdExhausted { kind: &'static str },
    Invariant { message: String },
}

pub struct Runtime {
    runtime_id: RuntimeId,
    pub(super) state: Rc<RefCell<RuntimeState>>,
}

/// Owns a freshly allocated gate until its response reaches the requesting
/// continuation. Sticky cancellation replaces that response before module code
/// can observe the id, so this wrapper closes the gate first and only then
/// forwards cancellation to the original continuation.
struct ResourceGateAllocationDelivery {
    gate: ResourceGateHandle,
    continuation: Box<dyn NativeContinuation>,
}

impl Trace for ResourceGateAllocationDelivery {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.gate.trace(sink) && self.continuation.trace(sink)
    }
}

impl NativeContinuation for ResourceGateAllocationDelivery {
    fn resume(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let Self { gate, continuation } = *self;
        match input {
            ResumeInput::Runtime(RuntimeResponse::ResourceGate(delivered))
                if delivered.id() == gate.id() =>
            {
                continuation.resume(
                    context,
                    ResumeInput::Runtime(RuntimeResponse::ResourceGate(delivered)),
                )
            }
            ResumeInput::Cancelled(reason) => {
                gate.close().map_err(|error| {
                    sema_core::SemaError::eval(format!(
                        "failed to close a cancelled resource-gate allocation: {error}"
                    ))
                })?;
                continuation.resume(context, ResumeInput::Cancelled(reason))
            }
            other => {
                gate.close().map_err(|error| {
                    sema_core::SemaError::eval(format!(
                        "failed to close an undeliverable resource-gate allocation: {error}"
                    ))
                })?;
                continuation.resume(context, other)
            }
        }
    }
}

/// Remove one gate and stage every Closed wake. Runtime requests and the
/// host-only weak capability share this exact registry transition.
fn close_resource_gate(
    state: &mut RuntimeState,
    gate: ResourceGateId,
) -> Result<bool, RegistryError> {
    let removed = state.resource_gates.close(gate)?;
    while let Some(wake) = state.resource_gates.pop_wake() {
        state
            .pending
            .push_back(PendingStage::ResourceGateWake(wake));
    }
    Ok(removed)
}

impl Trace for Runtime {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.state.try_borrow().is_ok_and(|state| state.trace(sink))
    }
}

pub(super) struct RuntimeTask {
    pub(super) record: TaskRecord,
    pub(super) payload: TaskPayload,
    pub(super) pending_resume: Option<PendingResume>,
    pub(super) suspended_owner: Option<ReturnOwner>,
    pub(super) vm_call: Option<VM>,
    pub(super) vm_owner: Option<ReturnOwner>,
    pub(super) context: TaskContextHandle,
    /// Pending resume for a VM-quantum task woken from an `async/await` (or
    /// `async/spawn`) park: the value to inject onto the parked frame's stack
    /// top before the next `run_quantum`, or a failure to settle the task with.
    pub(super) vm_resume: Option<VmResume>,
    /// The per-task dynamic scopes (LLM `with-cache`/`with-budget` state, OTel
    /// span-stack + conversation ids, leaf-usage accumulator), captured from the
    /// spawner's thread-locals at `async/spawn` and swapped into/out of the process
    /// thread-locals around each of this task's quanta (see [`TaskScopeSwap`]) so
    /// two interleaving tasks never share — and corrupt — one thread-local span
    /// stack / usage tally / dynamic LLM flag, and a spawned task parents to its own
    /// trace rather than a sibling's. Empty for the root task, which runs directly
    /// against the process thread-locals. Each scope is a type-erased `sema-llm` /
    /// `sema-otel` value reached through a registered seam ([`TASK_SCOPE_SEAMS`]);
    /// none holds a GC-traceable `Value`, so `scopes` needs no trace edge.
    pub(super) scopes: TaskScopes,
}

/// How a parked VM-quantum task should be resumed once its awaited promise
/// settles (or a spawn admission is decided).
pub(super) enum VmResume {
    /// Replace the parked frame's stack-top placeholder with this value and
    /// re-run the quantum (a resolved await / the spawned promise value).
    Value(sema_core::Value),
    /// The await target rejected/was cancelled, or spawn admission failed:
    /// settle the parked task with this error instead of resuming it.
    Fail(sema_core::SemaError),
}

impl Trace for VmResume {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Value(value) => {
                sink(sema_core::cycle::GcEdge::Value(value));
                true
            }
            Self::Fail(error) => error.trace(sink),
        }
    }
}

/// Apply a settled [`VmResume`] to a still-live VM's parked frame: inject a
/// resolved value onto its stack-top placeholder, or arm a rejection at the
/// parked call site. Shared by `visit_ready` (a task's `vm_call` resuming on
/// its next turn) and `run_parked_quantum`'s in-place channel handoff loop
/// (Task 0c-7) — the same application `reinstall_parent_vm_now` defers to a
/// later turn, applied here immediately because the VM was never parked.
fn apply_vm_resume(vm: &mut VM, resume: VmResume) {
    match resume {
        VmResume::Value(value) => vm.replace_stack_top(value),
        VmResume::Fail(error) => vm.resume_with_error(error),
    }
}

/// One per-quantum swappable dynamic scope: the three `sema-core` seam functions
/// that capture / take / install a single type-erased thread-local scope. All the
/// runtime's per-task dynamic contexts (LLM dynamic scope, OTel context, leaf-usage
/// scope) share this exact shape, so [`TaskScopeSwap`] and [`TaskScopes`] drive them
/// uniformly from the [`TASK_SCOPE_SEAMS`] table rather than repeating the swap logic
/// once per scope. Adding a new per-task dynamic thread-local is a one-line table
/// entry — but only for a scope that a spawned task must ISOLATE from its siblings
/// (see `docs/plans/evidence/unified-cooperative-runtime/task-06-context-matrix.md`
/// for why workflow/MCP context is NOT in this table).
struct ScopeSeam {
    /// Capture the spawner's live scope (cloning the relevant `Rc`/values) to seed
    /// a freshly-spawned child task.
    capture: fn() -> Box<dyn std::any::Any>,
    /// Take the current thread-local scope out (leaving it empty/default), returning
    /// the task's step-modified scope to carry across a suspension.
    take: fn() -> Box<dyn std::any::Any>,
    /// Install a scope into the thread-local, returning the one it displaced.
    install: fn(Box<dyn std::any::Any>) -> Box<dyn std::any::Any>,
    /// Fast-path predicate: does this captured scope carry no meaningful
    /// overrides (no allocation — a field peek, never a clone)? `true` for a
    /// scope registered by a feature (LLM/OTel/usage) never touched by the
    /// program. See [`TaskScopeSwap::install`] for how this combines with
    /// [`Self::ambient_is_empty`] to decide whether the swap can be skipped.
    captured_is_empty: fn(&Box<dyn std::any::Any>) -> bool,
    /// Fast-path predicate: is the CURRENT thread-local scope (before any swap)
    /// empty, without taking or boxing it? Used alongside
    /// [`Self::captured_is_empty`] — see [`TaskScopeSwap::install`].
    ambient_is_empty: fn() -> bool,
}

/// The per-task dynamic scopes swapped around every quantum, in a fixed order. Each
/// entry's three seams reach a type-erased `sema-llm` / `sema-otel` thread-local; the
/// registrations are installed at interpreter startup and no-op (empty box) until then.
const TASK_SCOPE_SEAMS: [ScopeSeam; 3] = [
    // LLM dynamic scope (cache/call snapshots + shared budget/cassette state).
    ScopeSeam {
        capture: sema_core::current_llm_scope_boxed,
        take: sema_core::take_task_llm_scope,
        install: sema_core::install_task_llm_scope,
        captured_is_empty: sema_core::llm_scope_captured_is_empty,
        ambient_is_empty: sema_core::llm_scope_ambient_is_empty,
    },
    // OTel context (span stack + conversation/session/user ids). Capture seeds an
    // EMPTY span stack (ids only) so the child parents to its own trace root.
    ScopeSeam {
        capture: sema_core::current_conversation_scope_boxed,
        take: sema_core::take_task_otel,
        install: sema_core::install_task_otel,
        captured_is_empty: sema_core::otel_captured_is_empty,
        ambient_is_empty: sema_core::otel_ambient_is_empty,
    },
    // Leaf-usage accumulator scope (per-`workflow/step` LLM usage attribution).
    ScopeSeam {
        capture: sema_core::current_usage_scope_boxed,
        take: sema_core::take_task_usage_scope,
        install: sema_core::install_task_usage_scope,
        captured_is_empty: sema_core::usage_scope_captured_is_empty,
        ambient_is_empty: sema_core::usage_scope_ambient_is_empty,
    },
];

/// A task's captured per-quantum dynamic scopes, one slot per [`TASK_SCOPE_SEAMS`]
/// entry (same order). Empty (all `None`) for a root task, which runs directly
/// against the process thread-locals. Holds no GC-traceable `Value` — the scopes
/// carry only scalar snapshots and shared `Rc`s (budget/usage accounts), so this
/// needs no [`Trace`] edge.
#[derive(Default)]
pub(super) struct TaskScopes {
    captured: [Option<Box<dyn std::any::Any>>; TASK_SCOPE_SEAMS.len()],
}

impl TaskScopes {
    /// Snapshot the spawner's live thread-local scopes to seed a freshly-spawned
    /// child (the `async/spawn` capture that used to read each seam's
    /// `current_*_boxed` inline).
    fn capture_for_spawn() -> Self {
        Self {
            captured: std::array::from_fn(|i| Some((TASK_SCOPE_SEAMS[i].capture)())),
        }
    }
}

/// Panic-safe swap of a task's per-quantum dynamic scopes into and out of the
/// process thread-locals.
///
/// `install` moves each captured scope out of the task and into its thread-local,
/// stashing the scope it displaced. `restore` takes the task's (possibly
/// step-modified) scopes back onto the task and reinstalls the displaced ones, in
/// lockstep across every seam. If the quantum unwinds before `restore` runs, `Drop`
/// still reinstalls the displaced scopes so a parent/sibling task's span stack and
/// usage tally are never corrupted (the faulting task's own mid-step scopes are
/// discarded — it will not resume). The seams are independent thread-locals, so the
/// order across them is immaterial; each is self-contained.
///
/// EMPTY-SCOPE FAST PATH: for a program that never touches a given seam's feature
/// (no LLM cache/budget/cassette, no OTel span, no leaf-usage attribution), every spawned
/// task's captured scope for that seam is a "default" value — and the swap would
/// box/unbox that default on every single quantum for nothing (malloc/free churn
/// visible in profiles even when the feature is unused). `install` skips the
/// take/install round-trip for a seam whose captured value AND the current
/// thread-local ("ambient") value are BOTH empty: installing an empty scope over
/// an empty ambient is a no-op by definition, so nothing is lost by leaving both
/// untouched instead of literally swapping them. This is NOT the same as "captured
/// is empty" alone — a task whose own scope is empty can still be entering a
/// thread whose ambient state is live (e.g. it was spawned inside a root-level
/// `llm/with-budget` that's still on the Rust call stack, or a prior task suspended
/// mid-scope without unwinding) — that case falls through to the ordinary swap so
/// the task's quantum genuinely sees ITS OWN (empty) scope rather than silently
/// inheriting the ambient one.
///
/// Skipping the swap for a seam means the quantum runs directly against whatever
/// is in the thread-local — which is fine AS LONG AS it stays empty for the whole
/// quantum (both empty scopes are semantically identical, so the quantum's own
/// spawn-capture / cache reads see the same "nothing" either way). But the
/// quantum's OWN code can open a fresh dynamic scope this step (e.g. a `with-budget`
/// entered for the first time) and suspend before it unwinds — leaving the
/// thread-local non-empty at quantum exit even though it was empty at entry. `restore`
/// (and `Drop`, for the panic/unwind path) re-checks ambient emptiness for every
/// skipped seam: if it's STILL empty, truly nothing happened and the task's
/// (unchanged) empty scope is left as-is; if it went non-empty, the now-live scope
/// is taken back onto the task (this materializes the swap we deferred, but only in
/// the rare case where the feature was actually exercised this quantum) so a
/// sibling task can never observe it.
struct TaskScopeSwap {
    displaced: [Option<Box<dyn std::any::Any>>; TASK_SCOPE_SEAMS.len()],
    /// `true` for a seam whose install was skipped (both captured and ambient were
    /// empty at entry) — no thread-local touched, no allocation made. Distinct from
    /// `displaced[i] == None`, which also covers the ordinary "nothing captured"
    /// case (a root task, or a nested swap inside an already-installed quantum) —
    /// those never ran the emptiness check and never need the exit re-check below.
    skipped: [bool; TASK_SCOPE_SEAMS.len()],
    restored: bool,
}

impl TaskScopeSwap {
    fn install(task: &mut RuntimeTask) -> Self {
        let mut displaced: [Option<Box<dyn std::any::Any>>; TASK_SCOPE_SEAMS.len()] =
            std::array::from_fn(|_| None);
        let mut skipped = [false; TASK_SCOPE_SEAMS.len()];
        for (i, seam) in TASK_SCOPE_SEAMS.iter().enumerate() {
            // No captured scope at all (root task; or a nested swap re-entering an
            // already-installed quantum, where the outer swap already took it) —
            // nothing to do, exactly as before the fast path existed.
            let Some(captured) = task.scopes.captured[i].as_ref() else {
                continue;
            };
            if (seam.captured_is_empty)(captured) && (seam.ambient_is_empty)() {
                skipped[i] = true;
                continue;
            }
            displaced[i] = task.scopes.captured[i].take().map(seam.install);
        }
        Self {
            displaced,
            skipped,
            restored: false,
        }
    }

    /// Normal-path unwind: capture the task's step-modified scopes back onto the
    /// task, then reinstall the displaced (spawner/global) scopes. Idempotent —
    /// a subsequent `Drop` is a no-op.
    fn restore(&mut self, task: &mut RuntimeTask) {
        if self.restored {
            return;
        }
        self.restored = true;
        for (i, seam) in TASK_SCOPE_SEAMS.iter().enumerate() {
            if self.skipped[i] {
                Self::reclaim_if_no_longer_empty(seam, &mut task.scopes.captured[i]);
                continue;
            }
            if let Some(prev) = self.displaced[i].take() {
                task.scopes.captured[i] = Some((seam.take)());
                let _ = (seam.install)(prev);
            }
        }
    }

    /// For a seam whose swap was skipped: if the quantum left the thread-local
    /// non-empty (it opened a fresh dynamic scope this step that hasn't unwound),
    /// take it back onto the task and reset the thread-local to empty — `take`
    /// already does the reset, so no `install` call is needed (the ambient value
    /// we deferred restoring was itself empty, which is exactly what `take` leaves
    /// behind). If it's still empty, nothing happened; leave the task's own
    /// (unchanged, still-empty) captured scope as-is.
    fn reclaim_if_no_longer_empty(
        seam: &ScopeSeam,
        captured_slot: &mut Option<Box<dyn std::any::Any>>,
    ) {
        if !(seam.ambient_is_empty)() {
            *captured_slot = Some((seam.take)());
        }
    }
}

impl Drop for TaskScopeSwap {
    fn drop(&mut self) {
        if self.restored {
            return;
        }
        // Unwind path: `restore` never ran, so we cannot reach `task`. Reinstall the
        // displaced scopes into the thread-locals so the parent/sibling context is
        // uncorrupted; the faulting task's own scopes are dropped with it.
        for (i, seam) in TASK_SCOPE_SEAMS.iter().enumerate() {
            if self.skipped[i] {
                // Mirror `reclaim_if_no_longer_empty`, but the faulting task's scope
                // is discarded (it will not resume) — just clear the thread-local so
                // it doesn't leak into whichever task runs next.
                if !(seam.ambient_is_empty)() {
                    let _ = (seam.take)();
                }
                continue;
            }
            if let Some(prev) = self.displaced[i].take() {
                let _ = (seam.take)();
                let _ = (seam.install)(prev);
            }
        }
    }
}

/// RAII guard for the two single-slot "current quantum" thread-locals —
/// `sema_core::CURRENT_TASK_ID` and `sema_core::CURRENT_ROOT` (published via
/// `set_current_task_id`/`set_current_root`) — published around every
/// quantum so natives (`llm/stream`, `agent/run`, output capture) can tell
/// which task/root is currently executing. Mirrors `TaskScopeSwap`'s
/// panic/early-return safety story, but for these two ids: both quantum
/// call sites run a `loop { .. }` containing `break`s AND at least one `?`
/// on a fallible call (`try_channel_handoff`, the debug-session lookup) that
/// can leave the function before reaching a plain post-loop restore
/// statement. A plain pair of `let prev = set_current_*(..); .. ; let _ =
/// set_current_*(prev);` restores correctly on the fallthrough path but
/// leaves the displaced (parent/sibling) id published on any early exit —
/// the next quantum on this thread would then run with a stale task/root
/// id, corrupting output-capture routing and the task-scoped slab lookups
/// natives use it for. `restore` is idempotent and `Drop` calls it, so an
/// early `?`-return or panic still restores the displaced ids exactly once.
struct QuantumIdGuard {
    prev_task_id: Option<RuntimeTaskId>,
    prev_root_id: Option<RootId>,
    restored: bool,
}

impl QuantumIdGuard {
    /// Publish `published_task_id`/`root` as the current quantum's identity,
    /// capturing the displaced (spawner/sibling) values to restore later.
    fn install(published_task_id: Option<RuntimeTaskId>, root: RootId) -> Self {
        let prev_task_id = sema_core::set_current_task_id(published_task_id);
        let prev_root_id = sema_core::set_current_root(Some(root));
        Self {
            prev_task_id,
            prev_root_id,
            restored: false,
        }
    }

    /// Normal-path restore. Idempotent — a subsequent `Drop` is a no-op.
    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = sema_core::set_current_task_id(self.prev_task_id);
        let _ = sema_core::set_current_root(self.prev_root_id);
    }
}

impl Drop for QuantumIdGuard {
    fn drop(&mut self) {
        // Unwind or early-`?`-return path: `restore` never ran explicitly.
        self.restore();
    }
}

// Task 4 replaces this placeholder with the VM-backed PreparedRoot payload.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum TaskPayload {
    /// A real VM-backed root: `vm_call` drives execution and this payload is
    /// never invoked (the VM-quantum arm in `visit_ready` takes precedence).
    Vm,
    #[cfg(not(test))]
    UnavailableUntilTask4,
    #[cfg(test)]
    Test(TestPreparedTask),
}

pub(super) struct RuntimeState {
    _context: Rc<EvalContext>,
    pub(super) clock: Rc<dyn RuntimeClock>,
    pub(super) waits: Option<WaitRuntime>,
    // Root admission is intentionally test-only until Task 4 supplies PreparedRoot.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) root_ids: RuntimeScopedIdCounter<RootId>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) task_ids: IdCounter<TaskId>,
    settlement_ids: IdCounter<SettlementSeq>,
    promises: PromiseRegistry,
    channels: ChannelRegistry,
    resource_gates: ResourceGateRegistry,
    pub(super) roots: HashMap<RootId, RootRecord>,
    pub(super) tasks: HashMap<TaskId, RuntimeTask>,
    pub(super) ready: ReadyScheduler,
    timers: TimerQueue,
    pub(super) handle_cleanup: VecDeque<RootId>,
    pending: VecDeque<PendingStage>,
    protocol_waits: HashMap<super::WaitKey, ProtocolWait>,
    task_promises: HashMap<TaskId, sema_core::runtime::PromiseId>,
    /// Dirty queue of tasks that had a cancellation recorded
    /// (`TaskRecord::request_cancellation` returned `true`). `cancel_waiting`
    /// pops candidates from here instead of scanning `tasks` — every site that
    /// records a cancellation pushes onto this queue (see call sites of
    /// `request_cancellation`). Popped ids are re-validated at pop time (task
    /// still exists, still `Waiting`, still holds the recorded cancellation)
    /// since a task can settle/reap between being queued and being popped;
    /// invalid ids are just dropped as transient. A candidate that IS a valid
    /// wait but not yet ready to tear down (UCR-3 channel-wake-in-flight skip,
    /// or `waits.cancel` finding no active entry yet) is either provably
    /// self-resolving (UCR-3: the in-flight wake finishes the protocol wait
    /// and settlement observes the sticky cancellation, UCR-1) or re-pushed so
    /// a later call retries it.
    pub(super) pending_cancel_waits: VecDeque<TaskId>,
    drive_cursor: usize,
    drive_active: bool,
    active_instruction_limit: usize,
    turn_instructions: usize,
    /// One reusable VM for `invoke_vm_callback_loop`'s in-place cooperative-HOF
    /// callback dispatch (Task C). Checked out (`take`n) for the duration of a
    /// single `invoke_callable` call and returned once that call's element
    /// chain settles without needing to park — killing the per-element (and,
    /// with this cache, the per-HOF-call) `VM::new_for_task_with_native_fns`
    /// allocation. `None` while checked out; a park path that consumes the VM
    /// into `task.vm_call` simply leaves this `None` and the next use refills
    /// it with a fresh allocation (see `take_scratch_callback_vm`).
    scratch_callback_vm: Option<VM>,
    pub(super) shutting_down: bool,
    pub(super) terminal_fault: Option<RuntimeFault>,
    /// Cooperative (headless) debug barrier. `Some((root, task, info))` while a
    /// task is paused at a breakpoint/step: the paused task is parked in `tasks`
    /// with its frames in `vm_call`, held OUT of the ready queue. While set, the
    /// drive loop runs no ready task and fires no timer — a runtime-wide
    /// stop-the-world barrier (external completions may still land in the inbox,
    /// they are just not delivered until resume). Cleared by
    /// [`Runtime::debug_resume`], which re-enqueues the paused task.
    pub(super) debug_paused: Option<(RootId, TaskId, crate::debug::StopInfo)>,
    /// The task the DAP frontend is stepping (StepInto/Over/Out), if any. The
    /// debug quantum applies step-mode stop logic only when running this task, so
    /// a step that suspends over an `await` lets siblings run (breakpoints only)
    /// and re-arms when the stepping task next runs. B3 refines the cross-sibling
    /// semantics; the field is threaded here so the barrier bookkeeping is final.
    /// Unused until B3 wires the per-task step gating (the current single-runnable
    /// step tests do not need it, since only the stepping task resumes).
    #[allow(dead_code)]
    stepping_task: Option<TaskId>,
    // Diagnostic: protocol completions that carried an undelivered value but
    // arrived after their wait was gone. A nonzero count is a lost-message bug.
    dropped_protocol_completions: usize,
    // Count of live `OriginBarrier` (`async/run`) protocol waits. A cheap
    // early-out for `resolve_origin_barriers`, which the drive loop calls every
    // iteration: zero means no barrier to re-evaluate. Incremented on install;
    // decremented when a barrier releases or is cancelled. Overcount is harmless
    // (a wasted scan); it is never undercounted (every install increments), so a
    // parked barrier is never missed.
    origin_barrier_waits: usize,
    /// Work-item debit owed for pending-stage hops a matched channel
    /// rendezvous collapsed into the current work item (`install_channel_wait`'s
    /// immediate-match fast path). Each collapsed hop (`ChannelWake`,
    /// `ApplyRuntimeResponse`/resume, the final `Apply`) would have cost one
    /// `work_items` increment under the staged path; `drive()` drains this into
    /// `work_items` right after the work item that produced it, so a
    /// channel-heavy pair of tasks cannot out-run `work_item_limit` and starve
    /// sibling roots just because the hops now run inline.
    channel_fast_path_credit: usize,
    /// Shared buffer for output captured from roots submitted with
    /// `capture_output: true` (see `sema_core::output_hook`). The SAME `Rc`
    /// is installed into this thread's `sema_core` output-capture sink at
    /// construction (`Runtime::new`) — `write_stdout`/`write_stderr` push
    /// into it directly from inside a driven quantum, keyed by the root
    /// published via `sema_core::set_current_root`. Drained by
    /// `Runtime::take_captured_output`.
    pub(super) output_sink: Rc<RefCell<Vec<sema_core::CapturedOutput>>>,
    #[cfg(test)]
    force_settlement_exhaustion: bool,
    #[cfg(test)]
    force_promise_exhaustion: bool,
    #[cfg(test)]
    force_channel_exhaustion: bool,
    #[cfg(test)]
    force_root_exhaustion: bool,
    #[cfg(test)]
    force_task_exhaustion: bool,
    #[cfg(test)]
    ready_visit_count: usize,
}

enum ProtocolWaitKind {
    Promises(sema_core::runtime::PromiseSetWait),
    Channel {
        channel: sema_core::runtime::ChannelId,
        receive: bool,
    },
    /// A bare `Timer(d)` suspension: the task's wait key is armed in the timer
    /// queue and the continuation resumes with `Returned(nil)` when it fires.
    Timer,
    /// Parked in a resource gate's FIFO queue (`WaitKind::ResourceSlot`): the
    /// task's wait key sits in the gate's waiter queue and the continuation
    /// resumes with `RuntimeResponse::Value(nil)` once the slot is granted (or a
    /// structured error if the gate is closed while parked).
    ResourceSlot {
        gate: ResourceGateId,
    },
    /// An `async/run` (`OriginBarrier`) suspension: the caller parks until every
    /// OTHER task under `root` has settled or come to rest on a cycle-forming
    /// wait (see [`Runtime::resolve_origin_barriers`]). No timer or registry
    /// wakes it — the drive loop re-evaluates the predicate every iteration and
    /// resumes the continuation with `Returned(nil)` once it holds.
    OriginBarrier {
        root: RootId,
    },
}

struct ProtocolWait {
    task: TaskId,
    kind: ProtocolWaitKind,
    owner: ReturnOwner,
    continuation: ContinuationFrame,
}

impl Trace for RuntimeState {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.waits.as_ref().is_none_or(|waits| waits.trace(sink))
            && self.roots.values().all(|root| root.trace(sink))
            && self.tasks.values().all(|task| task.trace(sink))
            && self.promises.trace(sink)
            && self.channels.trace(sink)
            && self.resource_gates.trace(sink)
            && self
                .protocol_waits
                .values()
                .all(|wait| wait.owner.trace(sink) && wait.continuation.trace(sink))
            && self.pending.iter().all(|stage| stage.trace(sink))
            && self
                .scratch_callback_vm
                .as_ref()
                .is_none_or(|vm| vm.trace(sink))
    }
}

impl Trace for RuntimeTask {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.record.trace(sink)
            && self
                .pending_resume
                .as_ref()
                .is_none_or(|pending| pending.trace(sink))
            && self.payload.trace(sink)
            && self
                .suspended_owner
                .as_ref()
                .is_none_or(|owner| owner.trace(sink))
            && self.vm_call.as_ref().is_none_or(|vm| vm.trace(sink))
            && self.vm_owner.as_ref().is_none_or(|owner| owner.trace(sink))
            && self.context.trace(sink)
            && self
                .vm_resume
                .as_ref()
                .is_none_or(|resume| resume.trace(sink))
    }
}

impl Trace for TaskPayload {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        #[cfg(not(test))]
        let _ = sink;
        match self {
            Self::Vm => true,
            #[cfg(not(test))]
            Self::UnavailableUntilTask4 => true,
            #[cfg(test)]
            Self::Test(task) => task.trace(sink),
        }
    }
}

// ── Cycle-collector interior hooks ────────────────────────────────
//
// A channel/promise HANDLE `Value` carries only an id; its mutable state (the
// channel buffer, the settled promise value) lives in this runtime's
// registries, not inline in the handle. The CORE-2 cycle collector must be able
// to see those buffered values as collector-internal edges (and sever them) or
// a cycle routed through a channel buffer — a closure captured into a channel
// that reaches the channel again — is pinned by the registry and never
// reclaimed. sema-core exposes a hook seam ([`sema_core::set_runtime_interior_hooks`]);
// the hooks below reach the currently-driving runtime through a thread-local so
// they can stay non-capturing `fn`s (invariant I2).

thread_local! {
    /// Stack of runtimes whose `drive` is on the call stack, innermost last.
    /// The collector's interior hooks resolve the buffered values of a
    /// channel/promise id against the innermost driving runtime. Empty when no
    /// drive is active (e.g. the interpreter-teardown collect, after the runtime
    /// is shut down) — the hooks then report no interior, which is safe.
    static CURRENT_RUNTIME: RefCell<Vec<Weak<RefCell<RuntimeState>>>> =
        const { RefCell::new(Vec::new()) };
}

/// The innermost driving runtime's state, if any is on the stack and still live.
fn current_runtime_state() -> Option<Rc<RefCell<RuntimeState>>> {
    CURRENT_RUNTIME.with(|stack| stack.borrow().last().and_then(Weak::upgrade))
}

/// Publishes `state` as the innermost driving runtime for the lifetime of the
/// guard, so interior hooks fired by a collection inside a driven VM quantum
/// resolve channel/promise ids against the right registries. Popped on every
/// exit path (RAII), including early returns and unwinds.
struct CurrentRuntimeGuard;

impl CurrentRuntimeGuard {
    fn install(state: &Rc<RefCell<RuntimeState>>) -> Self {
        CURRENT_RUNTIME.with(|stack| stack.borrow_mut().push(Rc::downgrade(state)));
        CurrentRuntimeGuard
    }
}

impl Drop for CurrentRuntimeGuard {
    fn drop(&mut self) {
        CURRENT_RUNTIME.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

fn interior_trace_channel(
    id: sema_core::runtime::ChannelId,
    sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>),
) -> bool {
    let Some(state) = current_runtime_state() else {
        return true;
    };
    // A borrow held here would mean a collection ran while the runtime state was
    // mutably borrowed — impossible at the drive safe points (the VM quantum
    // holds no state borrow). Abort cleanly (leak-safe) rather than risk it.
    let Ok(state) = state.try_borrow() else {
        return false;
    };
    state.channels.gc_trace_buffer(id, sink);
    true
}

fn interior_sever_channel(id: sema_core::runtime::ChannelId) -> Vec<sema_core::Value> {
    let Some(state) = current_runtime_state() else {
        return Vec::new();
    };
    let Ok(mut state) = state.try_borrow_mut() else {
        return Vec::new();
    };
    state.channels.gc_sever_buffer(id)
}

fn interior_evict_channel(id: sema_core::runtime::ChannelId) {
    if let Some(state) = current_runtime_state() {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.channels.gc_evict(id);
        }
    }
}

fn interior_trace_promise(
    id: sema_core::runtime::PromiseId,
    sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>),
) -> bool {
    let Some(state) = current_runtime_state() else {
        return true;
    };
    let Ok(state) = state.try_borrow() else {
        return false;
    };
    state.promises.gc_trace_settlement(id, sink);
    true
}

fn interior_sever_promise(id: sema_core::runtime::PromiseId) -> Vec<sema_core::Value> {
    let Some(state) = current_runtime_state() else {
        return Vec::new();
    };
    let Ok(mut state) = state.try_borrow_mut() else {
        return Vec::new();
    };
    state.promises.gc_sever_settlement(id)
}

fn interior_evict_promise(id: sema_core::runtime::PromiseId) {
    if let Some(state) = current_runtime_state() {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.promises.gc_evict(id);
        }
    }
}

/// Install the channel/promise interior hooks into the cycle collector. Wired
/// once per `Runtime::new`; the table is a set of non-capturing `fn`s, so
/// re-registering is idempotent and cheap.
fn register_runtime_interior_hooks() {
    sema_core::set_runtime_interior_hooks(Some(sema_core::RuntimeInteriorHooks {
        trace_channel: interior_trace_channel,
        sever_channel: interior_sever_channel,
        evict_channel: interior_evict_channel,
        trace_promise: interior_trace_promise,
        sever_promise: interior_sever_promise,
        evict_promise: interior_evict_promise,
    }));
}

impl Runtime {
    pub fn new(
        context: Rc<EvalContext>,
        clock: Rc<dyn RuntimeClock>,
        executor: Arc<dyn IoExecutor>,
    ) -> Result<Self, RuntimeCreateError> {
        let (waits, issuers) = WaitRuntime::new_with_issuers(executor)?;
        let runtime_id = waits.runtime_id();
        let (root_ids, promise_ids, channel_ids) = issuers.into_parts();
        // Wire the cycle collector's channel/promise interior hooks so a
        // collection during a driven quantum can trace/sever the registry-held
        // buffer and settled values (idempotent — a set of `fn` pointers).
        register_runtime_interior_hooks();
        let output_sink: Rc<RefCell<Vec<sema_core::CapturedOutput>>> =
            Rc::new(RefCell::new(Vec::new()));
        sema_core::register_output_capture_sink(runtime_id, &output_sink);
        Ok(Self {
            runtime_id,
            state: Rc::new(RefCell::new(RuntimeState {
                _context: context,
                clock,
                waits: Some(waits),
                root_ids,
                task_ids: IdCounter::new(),
                settlement_ids: IdCounter::new(),
                promises: PromiseRegistry::new(runtime_id, promise_ids),
                channels: ChannelRegistry::new(runtime_id, channel_ids),
                // Resource gates share the runtime identity so a gate id carries
                // its owning runtime; the counter is minted directly from that id
                // (not issued by the registrar's `RuntimeScopedIdIssuers`, which
                // predates this primitive) — a gate is a runtime-internal, GC-free
                // coordination record.
                resource_gates: ResourceGateRegistry::new(
                    runtime_id,
                    RuntimeScopedIdCounter::new(runtime_id),
                ),
                roots: HashMap::new(),
                tasks: HashMap::new(),
                ready: ReadyScheduler::new(),
                timers: TimerQueue::new(),
                handle_cleanup: VecDeque::new(),
                pending: VecDeque::new(),
                protocol_waits: HashMap::new(),
                task_promises: HashMap::new(),
                pending_cancel_waits: VecDeque::new(),
                drive_cursor: 0,
                drive_active: false,
                active_instruction_limit: usize::MAX,
                turn_instructions: 0,
                scratch_callback_vm: None,
                shutting_down: false,
                terminal_fault: None,
                debug_paused: None,
                stepping_task: None,
                dropped_protocol_completions: 0,
                origin_barrier_waits: 0,
                channel_fast_path_credit: 0,
                output_sink,
                #[cfg(test)]
                force_settlement_exhaustion: false,
                #[cfg(test)]
                force_promise_exhaustion: false,
                #[cfg(test)]
                force_channel_exhaustion: false,
                #[cfg(test)]
                force_root_exhaustion: false,
                #[cfg(test)]
                force_task_exhaustion: false,
                #[cfg(test)]
                ready_visit_count: 0,
            })),
        })
    }

    fn resource_gate_handle(&self, gate: ResourceGateId) -> ResourceGateHandle {
        let state = Rc::downgrade(&self.state);
        ResourceGateHandle::new(
            gate,
            Rc::new(move |gate| {
                let state = state
                    .upgrade()
                    .ok_or(ResourceGateCloseError::RuntimeUnavailable)?;
                let mut state = state
                    .try_borrow_mut()
                    .map_err(|_| ResourceGateCloseError::RuntimeBusy)?;
                close_resource_gate(&mut state, gate)
                    .map_err(|_| ResourceGateCloseError::WrongRuntime)
            }),
        )
    }

    #[cfg(test)]
    pub(super) fn set_drive_cursor_for_test(&self, cursor: usize) {
        self.state.borrow_mut().drive_cursor = cursor;
    }

    #[cfg(test)]
    pub(super) fn ready_visit_count_for_test(&self) -> usize {
        self.state.borrow().ready_visit_count
    }

    #[cfg(test)]
    pub(super) fn submit_test_root(
        &self,
        prepared: TestPreparedTask,
    ) -> Result<RootHandle, SubmitRootError> {
        let mut state = self.state.borrow_mut();
        if state.shutting_down || state.terminal_fault.is_some() {
            return Err(SubmitRootError::ShuttingDown);
        }
        if state.force_root_exhaustion
            || state.force_task_exhaustion
            || state.root_ids.is_exhausted()
            || state.task_ids.is_exhausted()
        {
            return Err(SubmitRootError::IdExhausted);
        }
        let root = state
            .root_ids
            .allocate()
            .map_err(|_| SubmitRootError::IdExhausted)?;
        let task = state
            .task_ids
            .allocate()
            .map_err(|_| SubmitRootError::IdExhausted)?;
        let relations = TaskRelations {
            origin_root: root,
            cancellation_parent: CancellationParent::Root(root),
            lifetime_owner: LifetimeOwner::Root(root),
        };
        state.roots.insert(root, RootRecord::new(root, task));
        state.tasks.insert(
            task,
            RuntimeTask {
                record: TaskRecord::new(task, relations),
                payload: TaskPayload::Test(prepared),
                pending_resume: None,
                suspended_owner: None,
                vm_call: None,
                vm_owner: None,
                context: TaskContextHandle::default(),
                vm_resume: None,
                scopes: TaskScopes::default(),
            },
        );
        state.ready.enqueue(root, task);
        Ok(RootHandle {
            runtime: Rc::downgrade(&self.state),
            id: root,
        })
    }

    /// Insert an extra task under an EXISTING root (sharing its `origin_root`),
    /// enqueued Ready — a same-origin-root sibling of the root's main task, the
    /// shape `async/spawn` produces. Used to build multi-task origin-root graphs
    /// (e.g. an `async/run` barrier caller with a `ResourceSlot`-parked sibling)
    /// without a real VM closure. `vm_owner: Some(ReturnOwner::Root)` mirrors
    /// `spawn_via_registry`'s real detached-child shape: it is what routes a
    /// cancellation observed while this task is still `Ready` (never run)
    /// through `settle_task`'s main-vs-registry-child distinction rather than
    /// `TaskAction::Settle`'s direct `self.settle()`, which asserts the
    /// settling task IS the root's main task — true for a synthetic root's
    /// own main task (which keeps `vm_owner: None`), never for a sibling.
    #[cfg(test)]
    pub(super) fn submit_test_child_under_root(
        &self,
        root: RootId,
        prepared: TestPreparedTask,
    ) -> TaskId {
        let mut state = self.state.borrow_mut();
        let task = state.task_ids.allocate().expect("child task identity");
        let relations = TaskRelations {
            origin_root: root,
            cancellation_parent: CancellationParent::Root(root),
            lifetime_owner: LifetimeOwner::Root(root),
        };
        state.tasks.insert(
            task,
            RuntimeTask {
                record: TaskRecord::new(task, relations),
                payload: TaskPayload::Test(prepared),
                pending_resume: None,
                suspended_owner: None,
                vm_call: None,
                vm_owner: Some(ReturnOwner::Root),
                context: TaskContextHandle::default(),
                vm_resume: None,
                scopes: TaskScopes::default(),
            },
        );
        state.ready.enqueue(root, task);
        task
    }

    /// Allocate a fresh `TaskId` WITHOUT inserting any record into
    /// `state.tasks` or `state.ready` — models a task that has already run
    /// to completion and been reaped (settled + removed), for tests that
    /// need a (now-gone) `cancellation_parent` task id to attach a
    /// grandchild to. `TestPreparedTask`'s synthetic settlement path only
    /// supports a root's MAIN task (see `submit_test_child_under_task`'s
    /// doc comment), so this sidesteps ever needing the returned id to
    /// actually run — the test only cares that it is ABSENT from
    /// `state.tasks` by the time `cancel_root` sweeps.
    #[cfg(test)]
    pub(super) fn allocate_task_id_for_test(&self) -> TaskId {
        self.state
            .borrow_mut()
            .task_ids
            .allocate()
            .expect("test task identity")
    }

    /// Insert a task whose CANCELLATION parent is another TASK (not the
    /// root directly), sharing `root`'s `origin_root` — the shape
    /// `spawn_via_registry` produces for a detached `async/spawn` child
    /// (`origin_root` = spawner's origin root, `cancellation_parent` =
    /// `Task(spawner)`). Used to build a grandchild whose live
    /// `cancellation_parent` chain to the root can be broken by removing
    /// `parent` from `state.tasks` (e.g. once `parent` settles), the exact
    /// shape CANCEL-ROOT-CASCADE-1's repro needs and `submit_test_child_
    /// under_root` (parented directly on the root) cannot express.
    /// `vm_owner: Some(ReturnOwner::Root)` for the same reason as
    /// `submit_test_child_under_root` — see its doc comment.
    #[cfg(test)]
    pub(super) fn submit_test_child_under_task(
        &self,
        root: RootId,
        parent: TaskId,
        prepared: TestPreparedTask,
    ) -> TaskId {
        let mut state = self.state.borrow_mut();
        let task = state.task_ids.allocate().expect("grandchild task identity");
        let relations = TaskRelations {
            origin_root: root,
            cancellation_parent: CancellationParent::Task(parent),
            lifetime_owner: LifetimeOwner::Root(root),
        };
        state.tasks.insert(
            task,
            RuntimeTask {
                record: TaskRecord::new(task, relations),
                payload: TaskPayload::Test(prepared),
                pending_resume: None,
                suspended_owner: None,
                vm_call: None,
                vm_owner: Some(ReturnOwner::Root),
                context: TaskContextHandle::default(),
                vm_resume: None,
                scopes: TaskScopes::default(),
            },
        );
        state.ready.enqueue(root, task);
        task
    }

    #[cfg(test)]
    pub(super) fn create_pending_promise_for_test(&self) -> sema_core::runtime::PromiseId {
        self.state
            .borrow_mut()
            .promises
            .allocate_pending(None)
            .expect("test promise identity")
    }

    #[cfg(test)]
    pub(super) fn submit_test_root_with_promise(
        &self,
        prepared: TestPreparedTask,
    ) -> Result<(RootHandle, sema_core::runtime::PromiseId), SubmitRootError> {
        let handle = self.submit_test_root(prepared)?;
        let mut state = self.state.borrow_mut();
        let task = match state.roots[&handle.id].state() {
            RootState::Running { main_task } => *main_task,
            RootState::Settled(_) | RootState::Aborted => {
                unreachable!("new test root is running")
            }
        };
        let promise = state
            .promises
            .allocate_pending(Some(task))
            .map_err(|_| SubmitRootError::IdExhausted)?;
        state.task_promises.insert(task, promise);
        drop(state);
        Ok((handle, promise))
    }

    #[cfg(test)]
    pub(super) fn settle_promise_for_test(
        &self,
        promise: sema_core::runtime::PromiseId,
        outcome: TaskOutcome,
    ) -> Rc<TaskSettlement> {
        let mut state = self.state.borrow_mut();
        let sequence = state
            .settlement_ids
            .allocate()
            .expect("test settlement identity");
        let settlement = Rc::new(TaskSettlement { sequence, outcome });
        let wakes = state
            .promises
            .settle(promise, Rc::clone(&settlement))
            .expect("test promise is pending");
        if !wakes.is_empty() {
            state.pending.push_back(PendingStage::PromiseWakes(wakes));
        }
        settlement
    }

    #[cfg(test)]
    pub(super) fn create_channel_for_test(&self, capacity: usize) -> sema_core::runtime::ChannelId {
        self.state
            .borrow_mut()
            .channels
            .allocate(capacity)
            .expect("test channel identity")
    }

    /// The number of tasks the runtime is currently holding (Ready / Running /
    /// Waiting / settled-but-not-yet-reaped). Used as a cancellation/reap oracle:
    /// after a program that cancels a task, a live-task count of 0 proves the
    /// cancelled task — and any descendant it transitively cancelled — settled
    /// terminal and was reaped, not orphaned. This is the unified runtime's
    /// analogue of the retired legacy `scheduler_task_count()`.
    pub fn live_task_count(&self) -> usize {
        self.state.borrow().tasks.len()
    }

    /// Number of live per-handle resource gates. This is a lifecycle
    /// observability oracle: terminal resource teardown returns it to its prior
    /// baseline, while ordinary operations retain their reusable gate.
    pub fn resource_gate_count(&self) -> usize {
        self.state.borrow().resource_gates.len()
    }

    /// Whether any task is still parked on a wait with a cancellation recorded —
    /// i.e. its wait teardown (executor abort / gate release / cancelled
    /// settlement) has not yet been delivered. A host post-settle drain counts
    /// this as pending progress so a one-shot program flushes every in-flight
    /// abort before returning rather than deferring it to `Interpreter::drop`
    /// (ASYNC-TIMEOUT-CANCEL-1). Request-time delivery (C2) makes this transient;
    /// it is the belt-and-suspenders backstop for scan-delivered teardown.
    pub fn has_pending_cancel_teardown(&self) -> bool {
        let state = self.state.borrow();
        state.tasks.values().any(|task| {
            task.record.cancellation().is_some()
                && task.record.state_name() == super::StateName::Waiting
        })
    }

    pub fn drive(&self, budget: &DriveBudget) -> Result<DriveState, RuntimeFault> {
        self.drive_selected(budget, None)
    }

    /// Drive runtime housekeeping plus VM quanta belonging only to `roots`.
    ///
    /// Completion decoding, cancellation, cleanup, and wake staging remain
    /// runtime-wide, but a ready task from another root is never executed. This
    /// lets hosts with distinct scheduling contracts share one persistent
    /// runtime without one host running another host's user code.
    pub fn drive_roots(
        &self,
        budget: &DriveBudget,
        roots: &[RootId],
    ) -> Result<DriveState, RuntimeFault> {
        self.drive_selected(budget, Some(roots))
    }

    fn drive_selected(
        &self,
        budget: &DriveBudget,
        selected_roots: Option<&[RootId]>,
    ) -> Result<DriveState, RuntimeFault> {
        // Publish this runtime for the whole drive so a cycle collection fired
        // inside a driven VM quantum (an explicit `(gc/collect)`, a `make_closure`
        // threshold, or the scheduler-idle safe point) can resolve channel/promise
        // interior against these registries. Popped on every exit path.
        let _current = CurrentRuntimeGuard::install(&self.state);
        let terminal_fault = self.state.borrow().terminal_fault.clone();
        if let Some(fault) = terminal_fault {
            while self.cleanup_one() {}
            return Err(fault);
        }
        {
            let mut state = self.state.borrow_mut();
            if state.drive_active {
                return Err(RuntimeFault::Invariant {
                    message: "runtime drive is already active".into(),
                });
            }
            state.drive_active = true;
            state.active_instruction_limit = budget.instruction_limit_per_task.get();
            state.turn_instructions = 0;
        }
        let _drive = ActiveDriveGuard(Rc::clone(&self.state));
        // Apply cross-thread commands (`RuntimeCommandHandle::cancel_root` /
        // `cancel_all`) before anything else this turn, including barrier
        // re-evaluation — a command observed here settles within this turn
        // rather than waiting for a later one.
        self.apply_pending_commands();
        let start = self.state.borrow().clock.now();
        // The quarantine-expiry and wall-clock-budget checks below read this
        // cached value rather than the clock directly; it is refreshed every
        // 64 iterations (see the `iters` counter in the loop), bounding how
        // stale either check can be against a real `Instant::now()`.
        let mut clock_now = start;
        let mut iters: u32 = 0;
        let mut work_items = 0;
        let mut root_visits = 0;
        let mut cleanup = 0;
        let mut completions = 0;
        let mut timers = 0;
        let mut no_progress = 0;
        let reserved_roots = {
            let state = self.state.borrow();
            selected_roots.map_or_else(
                || state.ready.root_count(),
                |roots| state.ready.root_count_for(roots),
            )
        }
        .min(budget.root_visit_limit.get());
        // Reserve credits for at most work_item_limit - 1 roots so a ready-root
        // storm always leaves at least one work item for completions, timers,
        // cleanup, and pending stages (spec: each storm leaves progress room).
        let reserve_floor = reserved_roots.min(budget.work_item_limit.get().saturating_sub(1));

        while work_items < budget.work_item_limit.get() {
            // Batch the clock reads that gate the checks below: a real
            // `Instant::now()` is a syscall, and this loop can spin thousands
            // of times per turn. Re-reading every 64 iterations (wrapping is
            // fine — only the low bits matter) trades a bounded overshoot on
            // the quarantine/wall-clock checks for skipping the syscall on
            // the other 63 iterations out of every 64.
            iters = iters.wrapping_add(1);
            if iters.is_multiple_of(64) {
                clock_now = self.state.borrow().clock.now();
            }
            // The cooperative debug barrier is armed (a task paused at a
            // breakpoint/step this turn, applied inline by `visit_ready`): stop
            // driving. No ready task runs and no timer fires until the host
            // resumes via `debug_resume`; the turn reports `DebugStopped` below.
            if self.state.borrow().debug_paused.is_some() {
                break;
            }
            // Re-evaluate parked `async/run` barriers against the current
            // origin-root graph BEFORE the source rotation, so the release
            // predicate is checked on every settlement/park transition this turn
            // (ASYNC-RUN-BARRIER-1). A released barrier resumes as one work item.
            if self.resolve_origin_barriers()? {
                work_items += 1;
                no_progress = 0;
                continue;
            }
            let expired = {
                let state = self.state.borrow();
                state
                    .waits
                    .as_ref()
                    .and_then(|waits| waits.expired_quarantine(clock_now))
            };
            if let Some(wait) = expired {
                return Err(RuntimeFault::Invariant {
                    message: format!(
                        "quarantine bound expired for wait {:?}/{:?}",
                        wait.id, wait.generation
                    ),
                });
            }
            if clock_now.saturating_duration_since(start) >= budget.wall_clock_limit {
                break;
            }
            if self.state.borrow().shutting_down && self.cancel_waiting()? {
                work_items += 1;
                no_progress = 0;
                continue;
            }
            let unvisited_reserved = reserve_floor.saturating_sub(root_visits);
            let remaining_credits = budget.work_item_limit.get() - work_items;
            let reserve_root = budget.work_item_limit.get() > 1
                && unvisited_reserved > 0
                && remaining_credits <= unvisited_reserved;
            let source = if reserve_root {
                5
            } else {
                let mut state = self.state.borrow_mut();
                let source = state.drive_cursor;
                state.drive_cursor = (state.drive_cursor + 1) % 6;
                source
            };
            let progressed = match source {
                0 if completions < budget.completion_limit.get() && self.drain_completion() => {
                    completions += 1;
                    true
                }
                1 if cleanup < budget.cleanup_limit.get()
                    && (self.cleanup_one() || self.reap_one()) =>
                {
                    cleanup += 1;
                    true
                }
                2 => self.cancel_waiting()?,
                3 => self.advance_pending_selected(selected_roots)?,
                4 if timers < budget.timer_limit.get() && self.fire_timer(clock_now)? => {
                    timers += 1;
                    true
                }
                5 if root_visits < reserved_roots
                    && self.visit_ready_selected(selected_roots)? =>
                {
                    root_visits += 1;
                    true
                }
                _ => false,
            };
            if progressed {
                work_items += 1;
                no_progress = 0;
                // A matched channel rendezvous collapses several pending-stage
                // hops into this one work item (`install_channel_wait`'s
                // immediate-match fast path) — debit the hops it would have
                // cost under the staged path so a channel-heavy pair of tasks
                // cannot out-run `work_item_limit` and starve sibling roots.
                let extra_credit =
                    std::mem::take(&mut self.state.borrow_mut().channel_fast_path_credit);
                work_items += extra_credit;
            } else {
                no_progress += 1;
                if no_progress == 6 {
                    break;
                }
            }
        }

        let state = self.state.borrow();
        let instructions = state.turn_instructions;
        if let Some(fault) = &state.terminal_fault {
            return Err(fault.clone());
        }
        // A cooperative debug stop armed the barrier during this turn: report it
        // so the host raises a stopped event and inspects the paused task. Takes
        // precedence over `Progress` (the stopping `visit_ready` counted a work
        // item) — the runtime is frozen, not merely making progress.
        if let Some((root, task, info)) = &state.debug_paused {
            return Ok(DriveState::DebugStopped {
                root: *root,
                task: *task,
                info: info.clone(),
            });
        }
        let ready_remaining = selected_roots.map_or_else(
            || state.ready.has_queued(),
            |roots| state.ready.has_queued_for(roots),
        );
        if selected_roots.is_none() {
            debug_assert_eq!(
                ready_remaining,
                state
                    .tasks
                    .values()
                    .any(|task| task.record.state_name() == super::StateName::Ready),
                "ready-queue membership must mirror Ready task records at turn boundaries"
            );
        }
        if work_items > 0 {
            Ok(DriveState::Progress {
                work_items,
                instructions,
                ready_remaining,
            })
        } else if state.shutting_down
            && state.waits.as_ref().is_none_or(WaitRuntime::is_closed)
            && state.tasks.is_empty()
            && state
                .waits
                .as_ref()
                .is_none_or(|waits| waits.active_len() == 0)
        {
            Ok(DriveState::ShutdownComplete)
        } else if state.roots.is_empty()
            && state
                .waits
                .as_ref()
                .is_none_or(|waits| waits.active_len() == 0)
        {
            Ok(DriveState::Quiescent)
        } else {
            Ok(DriveState::Idle {
                next_deadline: state.timers.next_deadline(),
                inbox_wakeup_required: state
                    .waits
                    .as_ref()
                    .is_some_and(|waits| waits.active_len() > 0),
            })
        }
    }

    /// Block the driving (VM) thread until an external completion lands on the
    /// inbox or `deadline` elapses, buffering it for the next [`drive`] turn.
    /// Called by a host drive loop when [`DriveState::Idle`] reports
    /// `inbox_wakeup_required`: a task is parked on an external operation running
    /// on a worker thread, so the VM thread has no work until that worker
    /// delivers. Returns `true` if a completion is now buffered. Bounded and
    /// wakeable (an arriving completion returns immediately); never busy-spins.
    ///
    /// [`drive`]: Self::drive
    pub fn block_on_inbox(&self, deadline: Option<Instant>) -> bool {
        self.state
            .borrow_mut()
            .waits
            .as_mut()
            .is_some_and(|waits| waits.block_on_inbox(deadline))
    }

    /// Whether a cooperative (headless) debug session is currently paused at a
    /// breakpoint/step (the runtime-wide barrier is armed).
    pub fn is_debug_paused(&self) -> bool {
        self.state.borrow().debug_paused.is_some()
    }

    /// Whether the cooperative debug barrier belongs to `root`.
    pub fn is_debug_paused_for(&self, root: RootId) -> bool {
        self.state
            .borrow()
            .debug_paused
            .as_ref()
            .is_some_and(|(paused_root, _, _)| *paused_root == root)
    }

    /// Clear the cooperative debug barrier and re-enqueue the paused task so the
    /// next [`drive`] resumes its frame. The host sets the step mode on its
    /// `DebugState` (reached via `ACTIVE_DEBUG` during the quantum) BEFORE calling
    /// this. Back-enqueue is correct: the barrier gated every sibling for the
    /// whole pause, so the paused task's position in the queue is irrelevant.
    ///
    /// A no-op (returns `false`) when nothing is paused.
    ///
    /// [`drive`]: Self::drive
    pub fn debug_resume(&self) -> bool {
        self.debug_resume_matching(None)
    }

    /// Resume only when the paused task belongs to `root`. A mismatched root
    /// leaves the barrier and task untouched.
    pub fn debug_resume_root(&self, root: RootId) -> bool {
        self.debug_resume_matching(Some(root))
    }

    fn debug_resume_matching(&self, expected_root: Option<RootId>) -> bool {
        let mut state = self.state.borrow_mut();
        if state
            .debug_paused
            .as_ref()
            .is_none_or(|(root, _, _)| expected_root.is_some_and(|expected| expected != *root))
        {
            return false;
        }
        let Some((root, task_id, _info)) = state.debug_paused.take() else {
            return false;
        };
        // The paused task is `Ready` (parked by the `DebugStop` handler) and holds
        // its frames in `vm_call`; enqueueing it lets the next `visit_ready`
        // resume the frame in place (no injected resume value — an ordinary
        // quantum re-entry that continues past the breakpoint under the step mode
        // the host just set).
        state.ready.enqueue(root, task_id);
        true
    }

    /// Reach the VM of the task paused at a cooperative debug stop, for
    /// inspection (stack trace / locals / evaluate). Returns `None` when no task
    /// is paused or its VM is not parked (should not happen while `debug_paused`
    /// is set). The paused task's VM never leaves the runtime — this borrows it in
    /// place, so resume is an ordinary quantum re-entry.
    pub fn with_paused_task_vm<R>(&self, f: impl FnOnce(&mut VM) -> R) -> Option<R> {
        self.with_paused_vm_matching(None, f)
    }

    /// Inspect the paused VM only when its task belongs to `root`.
    pub fn with_paused_root_vm<R>(&self, root: RootId, f: impl FnOnce(&mut VM) -> R) -> Option<R> {
        self.with_paused_vm_matching(Some(root), f)
    }

    fn with_paused_vm_matching<R>(
        &self,
        expected_root: Option<RootId>,
        f: impl FnOnce(&mut VM) -> R,
    ) -> Option<R> {
        let mut state = self.state.borrow_mut();
        let (root, task_id, _info) = state.debug_paused.as_ref()?;
        if expected_root.is_some_and(|expected| expected != *root) {
            return None;
        }
        let task_id = *task_id;
        let vm = state.tasks.get_mut(&task_id)?.vm_call.as_mut()?;
        Some(f(vm))
    }

    /// Abandon a cooperative debug session paused at a breakpoint (the Stop
    /// button): clear the barrier, request cancellation on the paused task, and
    /// cancel its root so an awaiting parent and siblings tear down. The paused
    /// task is re-enqueued so the next [`drive`] settles it Cancelled (dropping
    /// its parked frames) rather than resuming past the breakpoint. Bounded by the
    /// caller's subsequent drive; a no-op (`false`) when nothing is paused.
    ///
    /// This is what keeps an abandoned session from poisoning the next one on the
    /// same persistent runtime: without it the barrier would stay armed and freeze
    /// every future drive.
    ///
    /// [`drive`]: Self::drive
    pub fn debug_cancel_paused(&self) -> bool {
        self.debug_cancel_paused_matching(None)
    }

    /// Cancel only when the paused task belongs to `root`. A mismatched root
    /// cannot clear or cancel another debugger's barrier.
    pub fn debug_cancel_paused_root(&self, root: RootId) -> bool {
        self.debug_cancel_paused_matching(Some(root))
    }

    fn debug_cancel_paused_matching(&self, expected_root: Option<RootId>) -> bool {
        let paused =
            {
                let mut state = self.state.borrow_mut();
                if state.debug_paused.as_ref().is_none_or(|(root, _, _)| {
                    expected_root.is_some_and(|expected| expected != *root)
                }) {
                    return false;
                }
                state.debug_paused.take()
            };
        let Some((root, task_id, _info)) = paused else {
            return false;
        };
        {
            let mut state = self.state.borrow_mut();
            if let Some(task) = state.tasks.get_mut(&task_id) {
                if task.record.request_cancellation(CancelReason::HostStop) {
                    state.pending_cancel_waits.push_back(task_id);
                }
            }
            state.ready.enqueue(root, task_id);
        }
        // Cancel the root's main task too so a parent parked on `await` (and any
        // siblings) unwind rather than dangling once the paused child is dropped.
        self.cancel_root(root, CancelReason::HostStop);
        true
    }

    /// Re-evaluate every parked `async/run` barrier and resume the first whose
    /// origin-root graph has come to rest (ASYNC-RUN-BARRIER-1).
    ///
    /// A barrier releases (its caller resumes with nil) once NO OTHER task
    /// sharing the caller's origin root is Ready, Running, or parked on a
    /// SELF-RESOLVING wait — a `Timer`, an `External` operation, or a
    /// `Timeout`-mode `PromiseSet`, all of which complete on their own. Tasks
    /// parked on CYCLE-FORMING waits (`Promise`, all/race `PromiseSet`,
    /// `Channel`, `ResourceSlot`, or a nested barrier) are excluded: a barrier
    /// that waited on them could deadlock, because the awaited task may itself be
    /// excluded (a resource-slot holder blocked on a channel the barrier caller
    /// would service, a self-awaited parent, a rendezvous-blocked child).
    /// Transitivity is automatic — a self-resolving sleeper's awaiter becomes
    /// Ready when it fires, so the re-checked barrier keeps waiting for it.
    ///
    /// Called at the top of every `drive` iteration, so the predicate is
    /// re-evaluated on every origin-root settlement / park transition. Resumes at
    /// most one barrier per call: its resume marks it Ready, so a sibling barrier
    /// on the same root then observes it live and keeps waiting (the barriers
    /// serialize innermost-first rather than releasing en masse).
    fn resolve_origin_barriers(&self) -> Result<bool, RuntimeFault> {
        let mut state = self.state.borrow_mut();
        if state.origin_barrier_waits == 0 {
            return Ok(false);
        }
        // Only evaluate the release predicate at a fully-quiesced point. A wake
        // still sitting in `pending` (e.g. a settled sleeper's promise wake that
        // has not yet transitioned its awaiter to Ready) would make the awaiter
        // look settled/absent to `origin_barrier_released` and release the
        // barrier one turn too early. `fire_timer` guards the same deferred-wake
        // window; drain `pending` first and re-check next turn.
        if !state.pending.is_empty() {
            return Ok(false);
        }
        let candidates: Vec<(super::WaitKey, RootId, TaskId)> = state
            .protocol_waits
            .iter()
            .filter_map(|(key, wait)| match wait.kind {
                ProtocolWaitKind::OriginBarrier { root } => Some((*key, root, wait.task)),
                _ => None,
            })
            .collect();
        let Some(key) = candidates
            .into_iter()
            .find(|(_, root, caller)| origin_barrier_released(&state, *root, *caller))
            .map(|(key, _, _)| key)
        else {
            return Ok(false);
        };
        let wait = state
            .protocol_waits
            .remove(&key)
            .expect("barrier wait was just observed");
        state.origin_barrier_waits = state.origin_barrier_waits.saturating_sub(1);
        state
            .tasks
            .get_mut(&wait.task)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "origin barrier task disappeared".into(),
            })?
            .record
            .reject_wait(key)
            .map_err(|error| RuntimeFault::Invariant {
                message: format!("origin barrier wake transition failed: {error:?}"),
            })?;
        state.pending.push_back(PendingStage::Resume(
            wait.task,
            wait.owner,
            wait.continuation,
            ResumeInput::Returned(sema_core::Value::nil()),
        ));
        Ok(true)
    }

    /// `now` is the drive loop's cached `clock_now` (refreshed every 64
    /// iterations), not a fresh `Instant::now()` read — a timer wheel peek
    /// does not need finer freshness than the quarantine/wall-clock checks
    /// that share the same cache. Firing latency is therefore bounded by
    /// one 64-iteration window; timers are self-resolving (a due-but-not-
    /// yet-observed timer is simply re-checked, and fires, on the next
    /// window) so this never delays a timer past that bound.
    fn fire_timer(&self, now: Instant) -> Result<bool, RuntimeFault> {
        let mut state = self.state.borrow_mut();
        // Virtual-clock cooperative semantics: never fire a timer while any task
        // is still runnable. A cooperative scheduler drains all ready work to a
        // quiescent point before advancing the clock to the nearest deadline.
        // This is what makes `(async/timeout 0 (async 42))` return 42 (the ready
        // child settles the observed promise before the already-due 0ms deadline
        // trips) and what lets a shorter-sleeping sibling run/complete before a
        // longer sleeper's — or a select/retry backoff's — timer fires. Deferring
        // here is bounded: the drive loop still makes progress via `visit_ready`,
        // and once ready work quiesces this fires on the next turn.
        if state.ready.root_count() > 0 || !state.pending.is_empty() {
            return Ok(false);
        }
        let Some(key) = state.timers.pop_due(now) else {
            return Ok(false);
        };
        // A continuation parked on a bare `Timer(d)` suspension, or an
        // observational `Timeout` whose deadline beat every observed promise:
        // resume its continuation rather than a parked VM. A `Timer` resumes with
        // `Returned(nil)`; a `Timeout` deadline raises a structured `:timeout`
        // condition (the observed promises are left untouched — their producers
        // continue).
        if let Some(input) = match state.protocol_waits.get(&key).map(|wait| &wait.kind) {
            Some(ProtocolWaitKind::Timer) => Some(ResumeInput::Returned(sema_core::Value::nil())),
            Some(ProtocolWaitKind::Promises(set)) => match set.mode {
                sema_core::runtime::PromiseSetMode::Timeout(duration) => {
                    Some(ResumeInput::Failed(timeout_expired_condition(duration)))
                }
                _ => None,
            },
            Some(ProtocolWaitKind::Channel { .. })
            | Some(ProtocolWaitKind::ResourceSlot { .. })
            | Some(ProtocolWaitKind::OriginBarrier { .. })
            | None => None,
        } {
            let wait = state
                .protocol_waits
                .remove(&key)
                .expect("protocol timer wait was just observed");
            if let ProtocolWaitKind::Promises(set) = &wait.kind {
                for promise in &set.promises {
                    let _ = state.promises.cancel_observation(*promise, key);
                }
            }
            state
                .tasks
                .get_mut(&wait.task)
                .ok_or_else(|| RuntimeFault::Invariant {
                    message: "timer protocol task disappeared".into(),
                })?
                .record
                .reject_wait(key)
                .map_err(|error| RuntimeFault::Invariant {
                    message: format!("timer protocol wake transition failed: {error:?}"),
                })?;
            state.pending.push_back(PendingStage::Resume(
                wait.task,
                wait.owner,
                wait.continuation,
                input,
            ));
            return Ok(true);
        }
        let task_id = state
            .tasks
            .iter()
            .find_map(|(id, task)| (task.record.wait_key() == Some(key)).then_some(*id))
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "timer referenced missing waiting task".into(),
            })?;
        let root = state.tasks[&task_id].record.relations().origin_root;
        // A task parked directly on a bare timer key with no protocol-wait
        // entry: wake it and re-run its frame. Observational `async/timeout`
        // deadlines are delivered through the protocol-wait path above.
        state
            .tasks
            .get_mut(&task_id)
            .expect("timer task was selected")
            .record
            .wake(key)
            .map_err(|error| RuntimeFault::Invariant {
                message: format!("timer task failed to wake: {error:?}"),
            })?;
        state.ready.enqueue(root, task_id);
        Ok(true)
    }

    fn cleanup_one(&self) -> bool {
        let removed = {
            let mut state = self.state.borrow_mut();
            let Some(root) = state.handle_cleanup.pop_front() else {
                return false;
            };
            let removed = state
                .roots
                .get(&root)
                .is_some_and(RootRecord::is_reap_eligible)
                .then(|| state.roots.remove(&root))
                .flatten();
            if removed.is_some() {
                // Drop this root out of the capture set so a long-running host
                // (REPL, notebook server) submitting many capturing roots over
                // its lifetime doesn't leak entries in the thread-local set.
                sema_core::unmark_root_capturing(root);
            }
            removed
        };
        drop(removed);
        true
    }

    fn reap_one(&self) -> bool {
        let mut waits = {
            let mut state = self.state.borrow_mut();
            let Some(waits) = state.waits.take() else {
                return false;
            };
            if waits.cleanup_len() == 0 {
                state.waits = Some(waits);
                return false;
            }
            waits
        };
        waits.reap_cleanup(1);
        self.state.borrow_mut().waits = Some(waits);
        true
    }

    /// Tear down exactly one cancelled-while-waiting task, or report `Ok(false)`
    /// if none is currently actionable. Candidates come from
    /// `RuntimeState::pending_cancel_waits`, a dirty queue every
    /// `request_cancellation` call site pushes onto — not a scan of `tasks`.
    /// Each popped id is re-validated (still present, still `Waiting`, still
    /// carries the recorded cancellation) since a task can settle/reap or a
    /// wait can resolve between being queued and being popped; ids that fail
    /// validation are simply dropped (they were transient) rather than
    /// treated as "no progress".
    ///
    /// A cancellation can be requested against a task BEFORE it parks (e.g. a
    /// shutdown fan-out cancels every task, including ones still `Running` or
    /// `Ready`), so a popped id whose task is not yet `Waiting` is not
    /// necessarily transient — it may still park later. Such an id is pushed
    /// back onto the tail of the queue for a later call to retry, rather than
    /// dropped. To keep a single call from looping forever re-visiting the
    /// same not-yet-parked id, this scan is bounded to the queue's length at
    /// entry: each id gets at most one look this call, mirroring how the old
    /// full-`tasks`-table scan tried every task once per call.
    pub(super) fn cancel_waiting(&self) -> Result<bool, RuntimeFault> {
        let mut attempts = self.state.borrow().pending_cancel_waits.len();
        loop {
            if attempts == 0 {
                return Ok(false);
            }
            attempts -= 1;
            let mut state = self.state.borrow_mut();
            let Some(task_id) = state.pending_cancel_waits.pop_front() else {
                return Ok(false);
            };
            let Some(task) = state.tasks.get(&task_id) else {
                continue;
            };
            let Some(key) = task.record.wait_key() else {
                // Not parked (yet, or ever again) but the cancellation may
                // still be pending delivery once it does park — give it
                // another rotation instead of dropping it outright.
                if task.record.cancellation().is_some() {
                    state.pending_cancel_waits.push_back(task_id);
                }
                continue;
            };
            if task.record.cancellation().is_none() {
                continue;
            }

            if let Some(wait) = state.protocol_waits.get(&key) {
                // UCR-3: a rendezvous-matched channel waiter is no longer queued
                // in the channel but still holds a `protocol_waits` entry while
                // its `ChannelWake` (carrying the committed value) is in flight.
                // Cancel-dropping it here would silently discard that value. Skip
                // it: `consume_channel_wake` -> `finish_protocol_wait` delivers
                // the wake, removes this same `protocol_waits` entry, and
                // transitions the task off `Waiting` on its own; settlement then
                // observes the sticky cancellation (UCR-1). Nothing is lost, and
                // nothing more to track here — do not re-push.
                if let ProtocolWaitKind::Channel { channel, .. } = &wait.kind {
                    if !state.channels.has_wait(*channel, key) {
                        continue;
                    }
                }
                let resource_gate = match &wait.kind {
                    ProtocolWaitKind::ResourceSlot { gate } => Some(*gate),
                    _ => None,
                };
                let wait = if let Some(gate) = resource_gate {
                    remove_resource_slot_wait(&mut state, task_id, key, gate)?.expect(
                        "selected resource-slot protocol wait remains registered until teardown",
                    )
                } else {
                    let wait = state
                        .protocol_waits
                        .remove(&key)
                        .expect("selected protocol wait exists");
                    match &wait.kind {
                        ProtocolWaitKind::Promises(set) => {
                            for promise in &set.promises {
                                let _ = state.promises.cancel_observation(*promise, key);
                            }
                            if matches!(set.mode, sema_core::runtime::PromiseSetMode::Timeout(_)) {
                                state.timers.cancel(key);
                            }
                        }
                        ProtocolWaitKind::Timer => {
                            state.timers.cancel(key);
                        }
                        ProtocolWaitKind::Channel { channel, .. } => {
                            let _ = state.channels.cancel_wait(*channel, key);
                        }
                        ProtocolWaitKind::ResourceSlot { .. } => {
                            unreachable!("resource-slot teardown uses its transactional path")
                        }
                        ProtocolWaitKind::OriginBarrier { .. } => {
                            // A cancelled `async/run` barrier: nothing external to tear
                            // down (no timer/registry). Just drop the wait and let the
                            // continuation raise on the cancellation below.
                            state.origin_barrier_waits =
                                state.origin_barrier_waits.saturating_sub(1);
                        }
                    }
                    wait
                };
                let task = state.tasks.get_mut(&task_id).expect("selected task exists");
                task.record
                    .reject_wait(key)
                    .map_err(|error| RuntimeFault::Invariant {
                        message: format!("cancelled protocol task failed to resume: {error:?}"),
                    })?;
                state.pending.push_back(PendingStage::ApplyRuntimeResponse(
                    task_id,
                    wait.owner,
                    wait.continuation,
                    Err(sema_core::SemaError::eval("protocol wait cancelled")),
                ));
                return Ok(true);
            }

            if state.timers.cancel(key) {
                let root = state.tasks[&task_id].record.relations().origin_root;
                state
                    .tasks
                    .get_mut(&task_id)
                    .expect("timer task was selected")
                    .record
                    .wake(key)
                    .map_err(|error| RuntimeFault::Invariant {
                        message: format!("cancelled timer failed to wake: {error:?}"),
                    })?;
                state.ready.enqueue(root, task_id);
                return Ok(true);
            }

            // Neither a protocol wait nor a bare timer: fall back to the
            // generic `WaitRuntime` (external/promise/other) cancel path.
            let mut task = state.tasks.remove(&task_id).expect("selected task exists");
            let mut waits = state.waits.take().ok_or_else(|| RuntimeFault::Invariant {
                message: "wait runtime already extracted".into(),
            })?;
            let now = state.clock.now();
            drop(state);

            let pending = waits.cancel(&mut task.record, key, now);
            let root = task.record.relations().origin_root;
            let mut state = self.state.borrow_mut();
            state.waits = Some(waits);
            state.tasks.insert(task_id, task);
            if let Some(pending) = pending {
                state
                    .tasks
                    .get_mut(&task_id)
                    .expect("cancelled task restored")
                    .pending_resume = Some(pending);
                state.ready.enqueue(root, task_id);
                return Ok(true);
            }
            // `waits.cancel` found no matching `WaitRuntime::active` entry and
            // nothing was woken, so this turn made no real progress on the task
            // — it is still Waiting, still cancelled, and none of the dedicated
            // branches above claimed it. Re-queue it (the active entry may
            // appear on a later drive turn) and report no progress rather than
            // a false `Ok(true)`, which would spin the shutdown cancel loop
            // forever (the class of bug the channel branch fixes).
            state.pending_cancel_waits.push_back(task_id);
            return Ok(false);
        }
    }

    fn drain_completion(&self) -> bool {
        if self
            .state
            .borrow_mut()
            .waits
            .as_mut()
            .is_some_and(WaitRuntime::drain_unowned_one)
        {
            return true;
        }
        let task_id = self
            .state
            .borrow_mut()
            .waits
            .as_mut()
            .and_then(WaitRuntime::next_completion_task_id);
        if let Some(task_id) = task_id {
            let extracted = {
                let mut state = self.state.borrow_mut();
                let (Some(task), Some(waits)) = (state.tasks.remove(&task_id), state.waits.take())
                else {
                    return false;
                };
                (task, waits)
            };
            let (mut task, mut waits) = extracted;
            let drained = waits.drain_one(&mut task.record);
            let mut state = self.state.borrow_mut();
            state.waits = Some(waits);
            state.tasks.insert(task_id, task);
            if let Some((_route, pending)) = drained {
                if let Some(pending) = pending {
                    let task_id = pending.task_id();
                    let root = state
                        .tasks
                        .get(&task_id)
                        .expect("completion task remains registered")
                        .record
                        .relations()
                        .origin_root;
                    state
                        .tasks
                        .get_mut(&task_id)
                        .expect("completion task remains registered")
                        .pending_resume = Some(pending);
                    state.ready.enqueue(root, task_id);
                }
                return true;
            }
        }
        false
    }

    fn advance_pending_selected(
        &self,
        selected_roots: Option<&[RootId]>,
    ) -> Result<bool, RuntimeFault> {
        let stage = {
            let mut state = self.state.borrow_mut();
            let position = match selected_roots {
                Some(roots) => state
                    .pending
                    .iter()
                    .position(|stage| stage.belongs_to_roots(&state, roots)),
                None => (!state.pending.is_empty()).then_some(0),
            };
            position.and_then(|position| state.pending.remove(position))
        };
        let Some(stage) = stage else {
            return Ok(false);
        };
        let next = match stage {
            PendingStage::Action(action) => {
                self.apply_action(action)?;
                return Ok(true);
            }
            PendingStage::Decode(pending) => {
                let eval_context = Rc::clone(&self.state.borrow()._context);
                PendingStage::Continue(pending.invoke_decoder(&eval_context))
            }
            PendingStage::Continue(pending) => {
                let task = pending.task_id();
                let (owner, cancellation) = {
                    let mut state = self.state.borrow_mut();
                    state
                        .tasks
                        .get_mut(&task)
                        .and_then(|task| {
                            task.suspended_owner
                                .take()
                                .map(|owner| (owner, task.record.cancellation()))
                        })
                        .ok_or_else(|| RuntimeFault::Invariant {
                            message: "resumed task has no installed return owner".into(),
                        })?
                };
                // A sticky cancellation may have landed after the completion woke
                // this task Ready. Resume the continuation as cancelled — parity
                // with resume_continuation — instead of with the stale completion
                // value, so root/explicit/shutdown cancellation is not dropped.
                if let Some(request) = cancellation {
                    let frame = ContinuationFrame::native(pending.into_continuation());
                    self.resume_continuation(
                        task,
                        owner,
                        frame,
                        ResumeInput::Cancelled(request.reason),
                    )?;
                    return Ok(true);
                }
                let eval_context = Rc::clone(&self.state.borrow()._context);
                PendingStage::Apply(task, owner, pending.invoke_continuation(&eval_context))
            }
            PendingStage::Invoke(task, owner, call) => {
                self.invoke_callable(task, owner, call)?;
                return Ok(true);
            }
            PendingStage::Resume(task, owner, frame, input) => {
                self.resume_continuation(task, owner, frame, input)?;
                return Ok(true);
            }
            PendingStage::Apply(task, owner, result) => {
                self.apply_native_result(task, owner, result)?;
                return Ok(true);
            }
            PendingStage::DispatchRuntime(task, owner, request) => {
                self.dispatch_runtime(task, owner, request)?;
                return Ok(true);
            }
            PendingStage::ApplyRuntimeResponse(task, owner, frame, response) => {
                self.resume_continuation(
                    task,
                    owner,
                    frame,
                    response.map_or_else(ResumeInput::Failed, ResumeInput::Runtime),
                )?;
                return Ok(true);
            }
            PendingStage::PromiseWakes(mut wakes) => {
                let wake = match selected_roots {
                    Some(roots) => {
                        let state = self.state.borrow();
                        let position = wakes
                            .iter()
                            .position(|(_, task)| task_belongs_to_roots(&state, *task, roots));
                        position.and_then(|position| wakes.remove(position))
                    }
                    None => wakes.pop_front(),
                };
                if let Some((key, task)) = wake {
                    self.consume_promise_wake(key, task)?;
                }
                if !wakes.is_empty() {
                    self.state
                        .borrow_mut()
                        .pending
                        .push_back(PendingStage::PromiseWakes(wakes));
                }
                return Ok(true);
            }
            PendingStage::ChannelClose(mut close) => {
                let wake = match selected_roots {
                    Some(roots) => {
                        let state = self.state.borrow();
                        close.take_wake_for(|task| task_belongs_to_roots(&state, task, roots))
                    }
                    None => close.next_wake(),
                };
                if let Some(wake) = wake {
                    self.consume_channel_wake(wake)?;
                }
                if !close.is_empty() {
                    self.state
                        .borrow_mut()
                        .pending
                        .push_back(PendingStage::ChannelClose(close));
                }
                return Ok(true);
            }
            PendingStage::ChannelWake(wake) => {
                self.consume_channel_wake(wake)?;
                return Ok(true);
            }
            PendingStage::ResourceGateWake(wake) => {
                self.consume_resource_gate_wake(wake)?;
                return Ok(true);
            }
        };
        self.state.borrow_mut().pending.push_back(next);
        Ok(true)
    }

    fn consume_promise_wake(
        &self,
        key: super::WaitKey,
        task_id: TaskId,
    ) -> Result<(), RuntimeFault> {
        let response = {
            let state = self.state.borrow();
            let Some(wait) = state.protocol_waits.get(&key) else {
                return Ok(());
            };
            if wait.task != task_id {
                return Ok(());
            }
            let ProtocolWaitKind::Promises(set) = &wait.kind else {
                return Ok(());
            };
            promise_set_response(&state.promises, set)?
        };
        if let Some(response) = response {
            self.finish_protocol_wait(key, task_id, Ok(response))?;
        }
        Ok(())
    }

    fn consume_channel_wake(&self, wake: ChannelWake) -> Result<(), RuntimeFault> {
        // Every channel waiter is a structural `protocol_waits` entry now; deliver
        // the rendezvous result to its continuation via `finish_protocol_wait`.
        let response = {
            let state = self.state.borrow();
            channel_wake_response(&state.protocol_waits, wake.key, wake.result)
        };
        let Some(response) = response else {
            return Ok(());
        };
        self.finish_protocol_wait(wake.key, wake.task, response)
    }

    fn consume_resource_gate_wake(&self, wake: ResourceGateWake) -> Result<(), RuntimeFault> {
        // A granted slot resumes the parked acquirer with nil; a gate closed
        // while it was parked raises a structured error at the acquire site.
        let response = match wake.result {
            GateResult::Granted => Ok(RuntimeResponse::Value(sema_core::Value::nil())),
            GateResult::Closed => Err(sema_core::SemaError::eval(
                "resource gate closed while waiting for its slot",
            )),
        };
        self.finish_protocol_wait(wake.key, wake.task, response)
    }

    fn finish_protocol_wait(
        &self,
        key: super::WaitKey,
        task_id: TaskId,
        response: Result<RuntimeResponse, sema_core::SemaError>,
    ) -> Result<(), RuntimeFault> {
        let mut state = self.state.borrow_mut();
        let Some((owner, frame, response)) =
            finish_protocol_wait_now(&mut state, key, task_id, response)?
        else {
            return Ok(());
        };
        state.pending.push_back(PendingStage::ApplyRuntimeResponse(
            task_id, owner, frame, response,
        ));
        Ok(())
    }

    /// Run one quantum of a parked VM frame and map its outcome to a
    /// `TaskAction`. The caller has already applied any resume (a stack-top
    /// value via `replace_stack_top`, or a rejection armed via
    /// `resume_with_error`) to `vm`. On a cooperative stop (quantum expiry or an
    /// async yield) the VM is stashed back into `task.vm_call`; on completion or
    /// error the task's return owner is settled with the value/error.
    fn run_parked_quantum(
        &self,
        root: RootId,
        task_id: TaskId,
        task: &mut RuntimeTask,
        mut vm: VM,
    ) -> Result<TaskAction, RuntimeFault> {
        let (context, instruction_limit) = {
            let state = self.state.borrow();
            (Rc::clone(&state._context), state.active_instruction_limit)
        };
        let _task_context = context.scope_task_context(task.context.clone());
        let quantum_guard =
            context
                .enter_runtime_quantum()
                .map_err(|error| RuntimeFault::Invariant {
                    message: error.to_string(),
                })?;
        // A resuming parent VM may own `Tracked` upvalue cells whose captured
        // locals were mutated on a foreign callback VM (the cooperative HOF ABI)
        // while it was parked. Those writes live in the cell, not the parent's
        // stack slot the defining frame reads via `LOAD_LOCAL`. Refresh the
        // stack slots from the cells before running so the resumed frame observes
        // the callback's `set!` write-backs.
        vm.sync_tracked_upvalues_to_stack();
        // Install this task's captured per-task dynamic contexts around the quantum:
        // the LLM dynamic scope (cache/budget/…), the OTel context (span stack + ids),
        // and the leaf-usage accumulator scope. Two interleaving tasks otherwise share
        // — and corrupt — the one thread-local span stack / usage frame, and a fan-out
        // spawned inside a `with-cache`/`with-budget`/`workflow/step` extent must see
        // it even after the wrapper's thunk has ended. The quantum's own mutations
        // (spans opened this step, cache flips) persist back onto the task; the prior
        // (spawner/global) contexts are restored afterwards. The root task carries no
        // captured contexts and runs directly against the process thread-locals. The
        // swap is panic-safe: `TaskScopeSwap`'s `Drop` restores the displaced contexts
        // to the thread-locals even if the quantum unwinds, so a parent/sibling's span
        // stack and usage tally are never left corrupted.
        let mut scopes = TaskScopeSwap::install(task);
        // Publish the running task's identity so natives that open a per-task slab
        // entry (`llm/stream`, `agent/run`) record the owning task, letting the
        // task-reaped sweep reclaim the entry (and its detached span) when the task
        // is cancelled mid-flight. The ROOT MAIN task runs the user's top-level
        // program — semantically "top-level (non-task) code" — so it publishes `None`
        // (matching `current_task_id`'s contract): its slab entries aren't tied to a
        // cancellable task (top level can't be `async/cancel`led), and a native that
        // must reject a cooperative-scheduler-hostile op inside a SPAWNED task
        // (`http/serve`'s blocking accept loop) can tell it apart from the root.
        let is_root_main = {
            let state = self.state.borrow();
            matches!(
                state.roots.get(&root).map(RootRecord::state),
                Some(RootState::Running { main_task }) if *main_task == task_id
            )
        };
        let published_task_id = if is_root_main {
            None
        } else {
            Some(RuntimeTaskId::new(root.runtime(), task_id))
        };
        // Publish the running quantum's root (unlike task id, this is set for
        // the root main task too — its `println`s must tag correctly for a
        // capturing root) so `write_stdout`/`write_stderr` can route output
        // captured for this root instead of process stdout/stderr.
        // `QuantumIdGuard` restores both ids even if the loop below returns
        // early via `?` or panics — see its doc comment.
        let mut id_guard = QuantumIdGuard::install(published_task_id, root);
        // Regression insurance for the crux invariant (verified on the audited
        // path): no `RuntimeState` borrow is held across the quantum. The debug
        // variant may BLOCK inside the quantum (`handle_debug_stop` parks the
        // thread serving DAP inspection); if a borrow were live here the blocking
        // stop would deadlock the state cell. Per the review this can never fire.
        debug_assert!(
            self.state.try_borrow_mut().is_ok(),
            "RuntimeState borrowed at quantum entry — a blocking debug stop would deadlock the state cell"
        );
        // Task 0c-7: the loop below lets an in-place task-to-task channel
        // rendezvous handoff (below) feed its response straight back into
        // `vm` and re-run `run_quantum` on the SAME VM object, instead of
        // boxing/parking it and round-tripping through the pending queue.
        // `remaining_budget` is seeded once from the same total a single
        // (pre-0c7) quantum would receive, then debited per iteration — this
        // is Task C's `invoke_vm_callback_loop` budget-continuation pattern,
        // mirrored so a tight send/recv-spinning pair is still preempted at
        // the same total instruction count a single quantum always was.
        let mut remaining_budget = instruction_limit;
        let action = loop {
            let cancellation = {
                let cancel = task.record.cancellation();
                CancellationView::new(cancel.is_some(), cancel.map(|request| request.reason))
            };
            // When a native DAP session is registered on this thread (`ACTIVE_DEBUG`),
            // run the debug-aware quantum so breakpoints/steps inside this task — and
            // inside cooperative HOF callbacks, which also flow through
            // `run_parked_quantum` as enqueued callback-VM quanta — stop and serve
            // inspection against the stopped task's own VM. Otherwise the byte-
            // identical non-debug quantum.
            let quantum = if crate::vm::is_debug_session_active_for(root) {
                crate::vm::with_active_debug_for_root(root, |debug| {
                    vm.run_quantum_debug(&context, remaining_budget, cancellation, debug)
                })
                .expect("debug session active for root but no DebugState registered")
            } else {
                vm.run_quantum(&context, remaining_budget, cancellation)
            };
            self.state.borrow_mut().turn_instructions += quantum.instructions;
            remaining_budget = remaining_budget.saturating_sub(quantum.instructions);

            // Only a channel suspend with no cancellation landed is eligible for
            // the in-place handoff (item 4: a cancelled task takes the normal
            // path below instead of handing off). Anything else — a genuine
            // suspend of another kind, a budget expiry, completion, or error —
            // falls through to the UNMODIFIED `quantum_to_action` mapping,
            // reconstructing the exact `VmQuantumResult` this loop consumed
            // (its `instructions` were already folded into `turn_instructions`
            // above, so `quantum_to_action`, which never reads that field, gets
            // a harmless placeholder).
            let (wait, continuation) = match quantum.outcome {
                Ok(VmExecResult::Pending(VmPendingOutcome::Suspend(
                    sema_core::runtime::NativeSuspend {
                        wait: WaitKind::Channel(wait),
                        continuation,
                    },
                ))) if task.record.cancellation().is_none() => (wait, continuation),
                other_outcome => {
                    break self.quantum_to_action(
                        root,
                        task_id,
                        task,
                        vm,
                        VmQuantumResult {
                            outcome: other_outcome,
                            instructions: 0,
                        },
                    );
                }
            };
            let (channel, receive) = match &wait {
                sema_core::runtime::ChannelWait::Send { channel, .. } => (*channel, false),
                sema_core::runtime::ChannelWait::Receive { channel } => (*channel, true),
            };
            // A non-mutating registry predicate (see its doc for the FIFO
            // argument): only attempt the handoff when it is certain to
            // resolve without parking. A "no" here means NO registry
            // mutation has happened yet — the genuine-block case reaches
            // `install_channel_wait` exactly as it always has, byte-
            // identical to the pre-0c7 path (item 5).
            let immediate = self
                .state
                .borrow()
                .channels
                .would_resolve_immediately(channel, receive);
            if !immediate {
                break self.quantum_to_action(
                    root,
                    task_id,
                    task,
                    vm,
                    VmQuantumResult {
                        outcome: Ok(VmExecResult::Pending(VmPendingOutcome::Suspend(
                            sema_core::runtime::NativeSuspend {
                                wait: WaitKind::Channel(wait),
                                continuation,
                            },
                        ))),
                        instructions: 0,
                    },
                );
            }
            match self.try_channel_handoff(task_id, task, wait, continuation)? {
                ChannelHandoffOutcome::Applied(resume) => {
                    apply_vm_resume(&mut vm, resume);
                    // Credits mirror `complete_channel_rendezvous`'s exact
                    // pattern (item 6): +1 for the resume hop this replaces,
                    // +1 more because it resolved fully inline (no fallback
                    // queuing) — `drive()` folds this into `work_items` so a
                    // channel-heavy pair of tasks cannot look "free" and
                    // starve sibling roots of their turn.
                    self.state.borrow_mut().channel_fast_path_credit += 2;
                    continue;
                }
                ChannelHandoffOutcome::Deferred(result) => {
                    // The resumed continuation composed further (a chained
                    // `Call`/`Suspend`/`Runtime`, not a plain value/error) —
                    // the same rare case `apply_native_result_now`'s fallback
                    // arm handles. Box `vm` now (only on this cold path) and
                    // settle via the ordinary `VmResult` mapping, which
                    // `apply_action` queues as exactly the `PendingStage::Apply`
                    // the staged path would have produced (item 6's
                    // "single-step inline, no unbounded looping").
                    let owner = ReturnOwner::VmResume {
                        vm: Box::new(vm),
                        parent: Box::new(task.vm_owner.take().expect("VM call has a return owner")),
                    };
                    self.state.borrow_mut().channel_fast_path_credit += 1;
                    break TaskAction::VmResult(task_id, owner, result);
                }
                ChannelHandoffOutcome::GiveUp(wait, continuation) => {
                    // Wait-key identity exhausted before any registry mutation
                    // happened — give up on the fast path and let the ordinary
                    // (staged) path hit and handle the same exhaustion.
                    break self.quantum_to_action(
                        root,
                        task_id,
                        task,
                        vm,
                        VmQuantumResult {
                            outcome: Ok(VmExecResult::Pending(VmPendingOutcome::Suspend(
                                sema_core::runtime::NativeSuspend {
                                    wait: WaitKind::Channel(wait),
                                    continuation,
                                },
                            ))),
                            instructions: 0,
                        },
                    );
                }
            }
        };

        id_guard.restore();
        scopes.restore(task);
        drop(quantum_guard);
        Ok(action)
    }

    /// Map a completed [`VmQuantumResult`] to the [`TaskAction`] that drives it
    /// forward, mutating `task`'s `vm_call`/`vm_owner` as needed. Factored out of
    /// `run_parked_quantum` so `invoke_vm_callback_loop`'s in-place fast path
    /// (Task C) can reuse the EXACT SAME suspend-fallback mapping — for a quantum
    /// that expires its budget, hits a debug stop, sleeps, or suspends
    /// structurally (`Pending`) — instead of re-deriving it and risking drift.
    /// The `Pending`/`Finished`/`Err` arms consume `task.vm_owner` (via `.take()`),
    /// so the caller must have it populated with the owner this quantum's result
    /// resumes into before calling this — `run_parked_quantum` always does
    /// (it is set whenever a VM is parked); `invoke_vm_callback_loop` sets it
    /// explicitly right before falling back, since its in-place elements never
    /// otherwise touch `task.vm_owner`.
    fn quantum_to_action(
        &self,
        root: RootId,
        task_id: TaskId,
        task: &mut RuntimeTask,
        vm: VM,
        quantum: VmQuantumResult,
    ) -> TaskAction {
        match quantum.outcome {
            Ok(VmExecResult::QuantumExpired { .. }) => {
                task.vm_call = Some(vm);
                TaskAction::Yield(root, task_id)
            }
            // A cooperative (headless) debug session surfaced a breakpoint/step
            // stop out of the quantum (native DAP served it inline and never
            // returns `Stopped` here). Park the task with its frames intact —
            // exactly like `QuantumExpired` — and hand back a `DebugStop` so the
            // barrier is armed. The frame is mid-execution: `vm_owner` stays put
            // and the task re-enters this same frame on resume.
            Ok(VmExecResult::Stopped(info)) => {
                task.vm_call = Some(vm);
                TaskAction::DebugStop(root, task_id, info)
            }
            // A native suspended structurally through the runtime ABI (the VM
            // parked its frame — pc past the call, a nil placeholder on its stack
            // top). Move the parent VM OUT
            // of `vm_call` into the return owner so the continuation machine can
            // reuse `vm_call` for any callback VMs, then dispatch the carried
            // `NativeOutcome`; the parent VM is reinstalled and resumed with the
            // value once the outcome finishes driving (see `reinstall_parent_vm`).
            Ok(VmExecResult::Pending(pending)) => {
                let parent = task.vm_owner.take().expect("VM call has a return owner");
                let owner = ReturnOwner::VmResume {
                    vm: Box::new(vm),
                    parent: Box::new(parent),
                };
                TaskAction::VmResult(task_id, owner, Ok(pending.into_outcome()))
            }
            Ok(VmExecResult::Finished(value)) => TaskAction::VmResult(
                task_id,
                task.vm_owner.take().expect("VM call has a return owner"),
                Ok(NativeOutcome::Return(value)),
            ),
            Err(error) => TaskAction::VmResult(
                task_id,
                task.vm_owner.take().expect("VM call has a return owner"),
                Err(error),
            ),
            Ok(other) => TaskAction::VmResult(
                task_id,
                task.vm_owner.take().expect("VM call has a return owner"),
                Err(sema_core::SemaError::eval(format!(
                    "unsupported runtime VM stop: {other:?}"
                ))),
            ),
        }
    }

    fn visit_ready_selected(
        &self,
        selected_roots: Option<&[RootId]>,
    ) -> Result<bool, RuntimeFault> {
        let (root, task_id, mut task) = {
            let mut state = self.state.borrow_mut();
            let next = match selected_roots {
                Some(roots) => state.ready.dequeue_roots(roots),
                None => state.ready.dequeue(),
            };
            let Some((root, task_id)) = next else {
                return Ok(false);
            };
            #[cfg(test)]
            {
                state.ready_visit_count += 1;
            }
            let task = state
                .tasks
                .remove(&task_id)
                .ok_or_else(|| RuntimeFault::Invariant {
                    message: "ready scheduler referenced missing task".into(),
                })?;
            (root, task_id, task)
        };
        task.record
            .start()
            .map_err(|error| RuntimeFault::Invariant {
                message: format!("ready task failed to start: {error:?}"),
            })?;
        // A promise this VM task awaited (or a spawn admission) has settled. A
        // resolved value is injected onto the parked frame's stack top; a
        // rejection is RAISED at the parked call site (as if the awaiting native
        // had returned `Err`) so an enclosing try/catch can catch it. Both
        // re-run the frame's quantum in place — identical to the native's
        // already-settled fast path in `async_ops.rs`, regardless of whether the
        // promise was pending or settled when `await` ran. A rejection with NO
        // parked frame settles the task Failed directly.
        let resume = task.vm_resume.take();
        let action = if let Some(VmResume::Fail(error)) = resume {
            if let Some(mut vm) = task.vm_call.take() {
                // Re-run raising the rejection in-frame; uncaught, it surfaces
                // as `Err` out of `run_quantum` and settles the task Failed —
                // preserving the prior uncaught behavior.
                apply_vm_resume(&mut vm, VmResume::Fail(error));
                self.run_parked_quantum(root, task_id, &mut task, vm)?
            } else {
                TaskAction::VmResult(
                    task_id,
                    task.vm_owner
                        .take()
                        .expect("awaited VM task has a return owner"),
                    Err(error),
                )
            }
        } else if let Some(pending) = task.pending_resume.take() {
            TaskAction::Resume(pending)
        } else if let Some(cancel) = task.record.cancellation() {
            task.vm_call.take();
            match task.vm_owner.take() {
                Some(owner) => TaskAction::Cancel(task_id, owner, cancel.reason),
                None => TaskAction::Settle(root, task_id, TaskOutcome::Cancelled(cancel.reason)),
            }
        } else if let Some(mut vm) = task.vm_call.take() {
            if let Some(resume @ VmResume::Value(_)) = resume {
                apply_vm_resume(&mut vm, resume);
            }
            self.run_parked_quantum(root, task_id, &mut task, vm)?
        } else {
            match &mut task.payload {
                TaskPayload::Vm => {
                    return Err(RuntimeFault::Invariant {
                        message: "VM-backed root reached the payload arm without a vm_call".into(),
                    });
                }
                #[cfg(not(test))]
                TaskPayload::UnavailableUntilTask4 => {
                    return Err(RuntimeFault::Invariant {
                        message: "VM root execution belongs to Task 4".into(),
                    });
                }
                #[cfg(test)]
                TaskPayload::Test(prepared) => prepared.next(root, task_id),
            }
        };
        {
            let mut state = self.state.borrow_mut();
            if state.tasks.insert(task_id, task).is_some() {
                return Err(RuntimeFault::Invariant {
                    message: "task identity reused during extracted quantum".into(),
                });
            }
        }
        // A cooperative debug stop must arm the runtime-wide barrier THIS turn,
        // before any sibling `visit_ready`, `fire_timer`, or completion delivery
        // runs — apply it inline rather than deferring through the pending queue
        // (which the source rotation would interleave other work ahead of). The
        // task is already re-inserted above with its `vm_call` set.
        if matches!(action, TaskAction::DebugStop(..)) {
            self.apply_action(action)?;
        } else {
            self.state
                .borrow_mut()
                .pending
                .push_back(PendingStage::Action(action));
        }
        Ok(true)
    }

    fn apply_action(&self, action: TaskAction) -> Result<bool, RuntimeFault> {
        match action {
            TaskAction::Yield(root, task_id) => {
                let mut state = self.state.borrow_mut();
                let task =
                    state
                        .tasks
                        .get_mut(&task_id)
                        .ok_or_else(|| RuntimeFault::Invariant {
                            message: "yielded task disappeared".into(),
                        })?;
                task.record
                    .yield_ready()
                    .map_err(|error| RuntimeFault::Invariant {
                        message: format!("running task failed to yield: {error:?}"),
                    })?;
                state.ready.enqueue(root, task_id);
            }
            TaskAction::Settle(root, task_id, outcome) => self.settle(root, task_id, outcome)?,
            TaskAction::Cancel(task_id, owner, reason) => match owner {
                ReturnOwner::Root => {
                    let root = self
                        .state
                        .borrow()
                        .tasks
                        .get(&task_id)
                        .map(|task| task.record.relations().origin_root)
                        .ok_or_else(|| RuntimeFault::Invariant {
                            message: "cancelled task disappeared".into(),
                        })?;
                    self.settle_task(root, task_id, TaskOutcome::Cancelled(reason))?
                }
                ReturnOwner::Continuation(parent, frame) => self
                    .state
                    .borrow_mut()
                    .pending
                    .push_back(PendingStage::Resume(
                        task_id,
                        *parent,
                        frame,
                        ResumeInput::Cancelled(reason),
                    )),
                // A parent VM parked mid-`NativeOutcome` while its task is
                // cancelled: drop the parked VM and settle the task Cancelled (the
                // in-flight cooperative HOF cannot meaningfully resume).
                ReturnOwner::VmResume { vm: _, parent: _ } => {
                    let root = self
                        .state
                        .borrow()
                        .tasks
                        .get(&task_id)
                        .map(|task| task.record.relations().origin_root)
                        .ok_or_else(|| RuntimeFault::Invariant {
                            message: "cancelled task disappeared".into(),
                        })?;
                    self.settle_task(root, task_id, TaskOutcome::Cancelled(reason))?
                }
            },
            TaskAction::Native(task_id, result) => {
                self.state
                    .borrow_mut()
                    .pending
                    .push_back(PendingStage::Apply(task_id, ReturnOwner::Root, result));
            }
            TaskAction::VmResult(task_id, owner, result) => {
                self.state
                    .borrow_mut()
                    .pending
                    .push_back(PendingStage::Apply(task_id, owner, result));
            }
            TaskAction::DebugStop(root, task_id, info) => {
                let mut state = self.state.borrow_mut();
                let task =
                    state
                        .tasks
                        .get_mut(&task_id)
                        .ok_or_else(|| RuntimeFault::Invariant {
                            message: "debug-stopped task disappeared".into(),
                        })?;
                // Park the task Ready but OUT of the ready queue (the barrier
                // holds it; `debug_resume` enqueues it). The frames live in
                // `vm_call`; the record was `Running` after `visit_ready::start`.
                task.record
                    .yield_ready()
                    .map_err(|error| RuntimeFault::Invariant {
                        message: format!("debug-stopped task failed to park: {error:?}"),
                    })?;
                state.debug_paused = Some((root, task_id, info));
            }
            #[cfg(test)]
            TaskAction::Timer(task_id, deadline) => {
                let mut state = self.state.borrow_mut();
                let key = state
                    .waits
                    .as_ref()
                    .expect("wait runtime installed")
                    .issue_internal_wait()
                    .map_err(|_| RuntimeFault::IdExhausted { kind: "wait" })?;
                if !state.timers.insert(deadline, key) {
                    return Err(RuntimeFault::IdExhausted { kind: "timer" });
                }
                if let Err(error) = state
                    .tasks
                    .get_mut(&task_id)
                    .expect("timer task exists")
                    .record
                    .wait(key)
                {
                    state.timers.cancel(key);
                    return Err(RuntimeFault::Invariant {
                        message: format!("timer task failed to wait: {error:?}"),
                    });
                }
            }
            #[cfg(test)]
            TaskAction::NativeCall(task_id, call) => {
                let result = call();
                self.state
                    .borrow_mut()
                    .pending
                    .push_back(PendingStage::Apply(task_id, ReturnOwner::Root, result));
            }
            TaskAction::Resume(pending) => {
                self.state
                    .borrow_mut()
                    .pending
                    .push_back(PendingStage::Decode(pending));
            }
        }
        Ok(true)
    }

    fn apply_native_result(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
        result: NativeResult,
    ) -> Result<(), RuntimeFault> {
        match (owner, result) {
            (ReturnOwner::Continuation(parent, frame), Ok(NativeOutcome::Return(value))) => self
                .state
                .borrow_mut()
                .pending
                .push_back(PendingStage::Resume(
                    task_id,
                    *parent,
                    frame,
                    ResumeInput::Returned(value),
                )),
            (ReturnOwner::Continuation(parent, frame), Err(error)) => self
                .state
                .borrow_mut()
                .pending
                .push_back(PendingStage::Resume(
                    task_id,
                    *parent,
                    frame,
                    ResumeInput::Failed(error),
                )),
            // The runtime finished driving a parent VM's yielded `NativeOutcome`.
            // Reinstall the parked parent VM as the task's running VM and resume
            // it: a `Return` injects the value onto its parked stack top; an error
            // is RAISED at the parked call site (catchable by an enclosing
            // try/catch), matching the async-await resume contract.
            (ReturnOwner::VmResume { vm, parent }, Ok(NativeOutcome::Return(value))) => {
                return self.reinstall_parent_vm(task_id, *vm, *parent, VmResume::Value(value));
            }
            (ReturnOwner::VmResume { vm, parent }, Err(error)) => {
                return self.reinstall_parent_vm(task_id, *vm, *parent, VmResume::Fail(error));
            }
            (owner, result) => return self.apply_native_outcome(task_id, owner, result),
        }
        Ok(())
    }

    /// Reinstall a parent VM parked in a [`ReturnOwner::VmResume`] as the task's
    /// running VM and enqueue it Ready, so the next `visit_ready` resumes its
    /// parked frame (value injected via `replace_stack_top`, or an error raised
    /// via `resume_with_error`) — see `run_parked_quantum`'s `Pending` arm.
    fn reinstall_parent_vm(
        &self,
        task_id: TaskId,
        vm: VM,
        parent: ReturnOwner,
        resume: VmResume,
    ) -> Result<(), RuntimeFault> {
        let mut state = self.state.borrow_mut();
        reinstall_parent_vm_now(&mut state, task_id, vm, parent, resume)
    }

    fn apply_native_outcome(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
        result: NativeResult,
    ) -> Result<(), RuntimeFault> {
        let (root, cancellation) = {
            let state = self.state.borrow();
            let task = state
                .tasks
                .get(&task_id)
                .ok_or_else(|| RuntimeFault::Invariant {
                    message: "native result task disappeared".into(),
                })?;
            (
                task.record.relations().origin_root,
                task.record.cancellation(),
            )
        };
        match result {
            // A task that returns or fails while a cancellation is sticky settles
            // Cancelled: catching cancellation for cleanup cannot convert it into a
            // success or a plain failure (cancellation-cleanup mode).
            Ok(NativeOutcome::Return(value)) => {
                debug_assert!(matches!(owner, ReturnOwner::Root));
                match cancellation {
                    Some(request) => {
                        self.settle_task(root, task_id, TaskOutcome::Cancelled(request.reason))
                    }
                    None => self.settle_task(root, task_id, TaskOutcome::Returned(value)),
                }
            }
            Err(error) => {
                debug_assert!(matches!(owner, ReturnOwner::Root));
                match cancellation {
                    Some(request) => {
                        self.settle_task(root, task_id, TaskOutcome::Cancelled(request.reason))
                    }
                    None => self.settle_task(root, task_id, TaskOutcome::Failed(error)),
                }
            }
            Ok(NativeOutcome::Suspend(suspend)) => {
                if matches!(
                    &suspend.wait,
                    WaitKind::Promise(_)
                        | WaitKind::PromiseSet(_)
                        | WaitKind::Channel(_)
                        | WaitKind::Timer(_)
                        | WaitKind::ResourceSlot(_)
                ) {
                    return self.install_protocol_suspend(task_id, owner, suspend);
                }
                if !matches!(suspend.wait, sema_core::runtime::WaitKind::External(_)) {
                    self.state
                        .borrow_mut()
                        .pending
                        .push_back(PendingStage::Resume(
                            task_id,
                            owner,
                            ContinuationFrame::native(suspend.continuation),
                            ResumeInput::Failed(sema_core::SemaError::eval(
                                "runtime wait protocol is not active",
                            )),
                        ));
                    return Ok(());
                }
                let (mut task, mut waits) =
                    {
                        let mut state = self.state.borrow_mut();
                        let task = state.tasks.remove(&task_id).ok_or_else(|| {
                            RuntimeFault::Invariant {
                                message: "suspending task disappeared".into(),
                            }
                        })?;
                        let waits = state.waits.take().ok_or_else(|| RuntimeFault::Invariant {
                            message: "wait runtime already extracted".into(),
                        })?;
                        (task, waits)
                    };
                let registration =
                    waits.register_external(&mut task.record, suspend, task.context.clone());
                task.suspended_owner = Some(owner);
                let mut state = self.state.borrow_mut();
                state.waits = Some(waits);
                state.tasks.insert(task_id, task);
                match registration {
                    Ok(_) => Ok(()),
                    Err(RegisterExternalError::Rejected(pending)) => {
                        state.pending.push_back(PendingStage::Decode(*pending));
                        Ok(())
                    }
                    Err(RegisterExternalError::IdExhausted(kind, suspend)) => {
                        let owner = state
                            .tasks
                            .get_mut(&task_id)
                            .and_then(|task| task.suspended_owner.take())
                            .ok_or_else(|| RuntimeFault::Invariant {
                                message: "rejected suspend has no installed return owner".into(),
                            })?;
                        state.pending.push_back(PendingStage::Resume(
                            task_id,
                            owner,
                            ContinuationFrame::native(suspend.continuation),
                            ResumeInput::Failed(sema_core::SemaError::eval(format!(
                                "runtime {kind} identity exhausted"
                            ))),
                        ));
                        Ok(())
                    }
                }
            }
            Ok(NativeOutcome::Call(call)) => {
                self.state
                    .borrow_mut()
                    .pending
                    .push_back(PendingStage::Invoke(task_id, owner, call));
                Ok(())
            }
            Ok(NativeOutcome::Runtime(request)) => {
                self.state
                    .borrow_mut()
                    .pending
                    .push_back(PendingStage::DispatchRuntime(task_id, owner, request));
                Ok(())
            }
        }
    }

    fn install_protocol_suspend(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
        suspend: sema_core::runtime::NativeSuspend,
    ) -> Result<(), RuntimeFault> {
        let frame = ContinuationFrame::native(suspend.continuation);
        // Channel waits are dispatched separately, BEFORE taking the shared
        // `state` borrow below: `install_channel_wait_and_resume`'s
        // immediate-match fast path (Task D) needs to resume continuations
        // with no `RuntimeState` borrow outstanding (see
        // `ChannelRendezvousResume`'s doc) — a borrow this function would
        // otherwise hold across its entire body, for every `WaitKind`.
        let wait = match suspend.wait {
            WaitKind::Channel(wait) => {
                return self.install_channel_wait_and_resume(task_id, owner, frame, wait)
            }
            other => other,
        };
        let mut state = self.state.borrow_mut();
        let key = match state
            .waits
            .as_ref()
            .expect("wait runtime installed")
            .issue_internal_wait()
        {
            Ok(key) => key,
            Err(_) => {
                state.pending.push_back(PendingStage::ApplyRuntimeResponse(
                    task_id,
                    owner,
                    frame,
                    Err(sema_core::SemaError::eval(
                        "runtime wait identity exhausted",
                    )),
                ));
                return Ok(());
            }
        };
        let result = match wait {
            WaitKind::Promise(promise) => install_promise_wait(
                &mut state,
                task_id,
                key,
                sema_core::runtime::PromiseSetWait {
                    promises: vec![promise],
                    mode: sema_core::runtime::PromiseSetMode::Race,
                },
                owner,
                frame,
            ),
            WaitKind::PromiseSet(wait) => {
                install_promise_wait(&mut state, task_id, key, wait, owner, frame)
            }
            WaitKind::Timer(duration) => {
                install_timer_wait(&mut state, task_id, key, duration, owner, frame)
            }
            WaitKind::ResourceSlot(gate) => {
                install_resource_slot_wait(&mut state, task_id, key, gate, owner, frame)
            }
            WaitKind::Channel(_) => unreachable!("handled above"),
            WaitKind::External(_) => unreachable!("filtered protocol wait"),
        };
        if let Err(error) = result {
            let (owner, frame, error) = *error;
            state.pending.push_back(PendingStage::ApplyRuntimeResponse(
                task_id,
                owner,
                frame,
                Err(error),
            ));
        }
        Ok(())
    }

    /// Install a `Send`/`Receive` channel suspension and, on an immediate
    /// match, resume the settled continuation(s) inline (Task D) instead of
    /// queuing `PendingStage`s for a later `advance_pending` work item.
    ///
    /// `install_channel_wait` does the registry mutation and task-record
    /// bookkeeping under a short `RuntimeState` borrow and returns WITHOUT
    /// resuming anything; this method drops that borrow and only THEN resumes
    /// (`self`'s own outcome first, then the matched peer, if any) — see
    /// `ChannelRendezvousResume`'s doc for why `frame.resume` must never run
    /// under a `RuntimeState` borrow. Resuming `self` before the peer (rather
    /// than push order) preserves the staged path's observable settlement
    /// order — see the comment on that call below.
    fn install_channel_wait_and_resume(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
        frame: ContinuationFrame,
        wait: sema_core::runtime::ChannelWait,
    ) -> Result<(), RuntimeFault> {
        let key = {
            let mut state = self.state.borrow_mut();
            match state
                .waits
                .as_ref()
                .expect("wait runtime installed")
                .issue_internal_wait()
            {
                Ok(key) => key,
                Err(_) => {
                    state.pending.push_back(PendingStage::ApplyRuntimeResponse(
                        task_id,
                        owner,
                        frame,
                        Err(sema_core::SemaError::eval(
                            "runtime wait identity exhausted",
                        )),
                    ));
                    return Ok(());
                }
            }
        };
        let outcome = {
            let mut state = self.state.borrow_mut();
            install_channel_wait(&mut state, task_id, key, wait, owner, frame)
        };
        match outcome {
            Ok(ChannelWaitOutcome::Parked) => Ok(()),
            Ok(ChannelWaitOutcome::Matched { this, peer }) => {
                // No `RuntimeState` borrow is held here (the block above
                // dropped it) — safe to resume Sema-level continuations.
                //
                // `this` (the task whose op just resolved, e.g. the sender in
                // a send/recv rendezvous) is resumed BEFORE `peer` (the
                // matched waiter, e.g. the receiver). Under the staged path a
                // matched peer's `ChannelWake` was queued ahead of this
                // task's `ApplyRuntimeResponse`, but the peer's extra
                // `ChannelWake` hop meant this task's shorter pending chain
                // reached its final `Apply` — and so `ready`-enqueued —
                // first regardless (e.g. `channel/send` settles the sender's
                // task before the matched receiver's task resumes and
                // settles: `async_race_returns_first_resolved_in_list_order`).
                // Resuming `this` first here reproduces that order exactly.
                self.complete_channel_rendezvous(this)?;
                if let Some(peer) = peer {
                    self.complete_channel_rendezvous(*peer)?;
                }
                Ok(())
            }
            Err(ChannelWaitError::Protocol(error)) => {
                let (owner, frame, error) = *error;
                self.state
                    .borrow_mut()
                    .pending
                    .push_back(PendingStage::ApplyRuntimeResponse(
                        task_id,
                        owner,
                        frame,
                        Err(error),
                    ));
                Ok(())
            }
            Err(ChannelWaitError::Fault(fault)) => Err(fault),
        }
    }

    /// Resume one `ChannelRendezvousResume` and apply its result, crediting
    /// `channel_fast_path_credit` for every pending-stage hop this replaces
    /// (`ApplyRuntimeResponse`/`ChannelWake` -> `resume_continuation`, and
    /// `Apply` -> `apply_native_result` when it resolves fully inline) so
    /// `drive()` debits `work_items` honestly for the collapsed work.
    fn complete_channel_rendezvous(
        &self,
        resume: ChannelRendezvousResume,
    ) -> Result<(), RuntimeFault> {
        self.state.borrow_mut().channel_fast_path_credit += 1;
        let input = resume
            .response
            .map_or_else(ResumeInput::Failed, ResumeInput::Runtime);
        let resumed = self.resume_continuation_value(resume.task_id, resume.frame, input)?;
        let mut state = self.state.borrow_mut();
        if apply_native_result_now(&mut state, resume.task_id, resume.owner, resumed)? {
            state.channel_fast_path_credit += 1;
        }
        Ok(())
    }

    /// Attempt Task 0c-7's in-place task-to-task rendezvous handoff for a
    /// channel op the caller has already confirmed (via
    /// `ChannelRegistry::would_resolve_immediately`) will resolve without
    /// parking. Does the ONE mutating registry call (`send`/`receive` —
    /// exactly what `install_channel_wait` would have called; this function
    /// is only ever reached when that call is about to match, so there is no
    /// double-mutation and the genuine-block path in `install_channel_wait`
    /// is never touched by this one) and resumes `this` task's own
    /// continuation directly against `task`'s LOCAL `context`/`cancellation`
    /// — mirroring `invoke_vm_callback_loop`'s per-element resume (Task C),
    /// not `resume_continuation_value`, because `task_id` has not been
    /// reinserted into `state.tasks` yet at this point (the caller,
    /// `run_parked_quantum`, still owns it as `&mut RuntimeTask`).
    ///
    /// The matched peer (if any) is delivered via the EXISTING, unchanged
    /// Task-D machinery (`complete_channel_rendezvous`), which owns its own
    /// short borrows and is safe to call with none held here.
    fn try_channel_handoff(
        &self,
        task_id: TaskId,
        task: &mut RuntimeTask,
        wait: sema_core::runtime::ChannelWait,
        continuation: Box<dyn sema_core::runtime::NativeContinuation>,
    ) -> Result<ChannelHandoffOutcome, RuntimeFault> {
        let key = {
            let state = self.state.borrow();
            match state
                .waits
                .as_ref()
                .expect("wait runtime installed")
                .issue_internal_wait()
            {
                Ok(key) => key,
                // No registry mutation has happened yet — safe to bail out
                // entirely and let the ordinary (staged) path hit and handle
                // the identical exhaustion.
                Err(_) => return Ok(ChannelHandoffOutcome::GiveUp(wait, continuation)),
            }
        };
        let (receive, result) = {
            let mut state = self.state.borrow_mut();
            match &wait {
                sema_core::runtime::ChannelWait::Send { channel, value } => (
                    false,
                    state.channels.send(*channel, key, task_id, value.clone()),
                ),
                sema_core::runtime::ChannelWait::Receive { channel } => {
                    (true, state.channels.receive(*channel, key, task_id))
                }
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                return self.resume_this_inline(task, continuation, Err(registry_error(error)))
            }
        };
        // The caller only reaches here after `would_resolve_immediately` said
        // yes, and nothing else runs between that check and this mutation
        // (single-threaded, no reentrancy) — so this can never actually be
        // `Waiting`. Treat a desync as the invariant violation it would be
        // rather than silently mis-parking the task (no `owner` is available
        // here to install a genuine park, unlike `install_channel_wait`).
        if result == super::ChannelResult::Waiting {
            return Err(RuntimeFault::Invariant {
                message: "channel handoff: would_resolve_immediately predicate desynced from registry result".into(),
            });
        }
        let wake = { self.state.borrow_mut().channels.pop_wake() };
        let response = match (receive, result) {
            (true, super::ChannelResult::Received(value)) => {
                RuntimeResponse::Receive(sema_core::runtime::ChannelReceive::Received(value))
            }
            (true, super::ChannelResult::Closed) => {
                RuntimeResponse::Receive(sema_core::runtime::ChannelReceive::Closed)
            }
            (false, super::ChannelResult::Sent) => {
                RuntimeResponse::Send(sema_core::runtime::ChannelSend::Sent)
            }
            (false, super::ChannelResult::Closed) => {
                RuntimeResponse::Send(sema_core::runtime::ChannelSend::Closed)
            }
            (_, super::ChannelResult::Waiting) => unreachable!("handled above"),
            (true, super::ChannelResult::Sent) | (false, super::ChannelResult::Received(_)) => {
                unreachable!("channel result matches operation")
            }
        };
        // Deliver the matched peer's wake via the EXISTING Task-D inline
        // machinery, unchanged (item 1) — `complete_channel_rendezvous` owns
        // its own borrows and credits its own hops.
        if let Some(wake) = wake {
            let peer_response = {
                let state = self.state.borrow();
                channel_wake_response(&state.protocol_waits, wake.key, wake.result)
            };
            if let Some(peer_response) = peer_response {
                let resume = {
                    let mut state = self.state.borrow_mut();
                    // Replaces the `PendingStage::ChannelWake` ->
                    // `consume_channel_wake` hop, exactly like
                    // `install_channel_wait`'s own wake-consuming branch.
                    state.channel_fast_path_credit += 1;
                    finish_protocol_wait_now(&mut state, wake.key, wake.task, peer_response)?
                };
                if let Some((owner, frame, response)) = resume {
                    self.complete_channel_rendezvous(ChannelRendezvousResume {
                        task_id: wake.task,
                        owner,
                        frame,
                        response,
                    })?;
                }
            }
        }
        self.resume_this_inline(task, continuation, Ok(response))
    }

    /// Resume `this` task's own channel continuation with `response` against
    /// `task`'s LOCAL context, without going through `resume_continuation_value`
    /// (see `try_channel_handoff`'s doc for why). Re-checks cancellation
    /// immediately before resuming — mirroring `resume_continuation_value`'s
    /// own recheck (UCR-3 / item 4): a cancellation landing between the
    /// registry match and this resume must still be observed by the
    /// continuation, not silently overridden by the stale channel response.
    fn resume_this_inline(
        &self,
        task: &mut RuntimeTask,
        continuation: Box<dyn sema_core::runtime::NativeContinuation>,
        response: ChannelResponse,
    ) -> Result<ChannelHandoffOutcome, RuntimeFault> {
        let cancel = task.record.cancellation();
        let input = match cancel {
            Some(cancel) => ResumeInput::Cancelled(cancel.reason),
            None => response.map_or_else(ResumeInput::Failed, ResumeInput::Runtime),
        };
        let eval_context = Rc::clone(&self.state.borrow()._context);
        let mut task_context = task.context.borrow_mut();
        let mut native_context = NativeCallContext {
            eval_context: &eval_context,
            task_context: &mut task_context,
            cancellation: CancellationView::new(cancel.is_some(), cancel.map(|c| c.reason)),
        };
        let resumed = continuation.resume(&mut native_context, input);
        drop(task_context);
        Ok(match resumed {
            Ok(NativeOutcome::Return(value)) => {
                ChannelHandoffOutcome::Applied(VmResume::Value(value))
            }
            Err(error) => ChannelHandoffOutcome::Applied(VmResume::Fail(error)),
            other => ChannelHandoffOutcome::Deferred(other),
        })
    }

    fn dispatch_runtime(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
        request: RuntimeRequest,
    ) -> Result<(), RuntimeFault> {
        if let RuntimeRequest::PromiseSetWait { wait, continuation } = request {
            return self.install_protocol_suspend(
                task_id,
                owner,
                sema_core::runtime::NativeSuspend {
                    wait: WaitKind::PromiseSet(wait),
                    continuation,
                },
            );
        }
        if let RuntimeRequest::Spawn {
            callable,
            continuation,
        } = request
        {
            return self.spawn_via_registry(task_id, owner, callable, continuation);
        }
        // `async/run` (OriginBarrier): park the caller on a self-resolving-waits
        // barrier (ASYNC-RUN-BARRIER-1). The caller suspends while ANY other task
        // in its origin-root graph is Ready, Running, or parked on a SELF-RESOLVING
        // wait (Timer / External / a `Timeout`-mode PromiseSet — all of which
        // complete on their own), and resumes with nil once the residual graph has
        // settled or every remaining non-caller task is parked ONLY on a
        // CYCLE-FORMING wait (Promise, all/race PromiseSet, Channel, ResourceSlot,
        // or a nested barrier — waiting on those could deadlock). The predicate is
        // re-evaluated in `resolve_origin_barriers` on every drive iteration, so
        // transitivity is automatic: a self-resolving sleeper's awaiter becomes
        // Ready when it fires, and the re-checked barrier keeps waiting for it. See
        // ASYNC-RUN-BARRIER-1 (RESOLVED) in docs/deferred.md.
        if let RuntimeRequest::OriginBarrier { continuation } = request {
            let frame = ContinuationFrame::native(continuation);
            let mut state = self.state.borrow_mut();
            let key = match state
                .waits
                .as_ref()
                .expect("wait runtime installed")
                .issue_internal_wait()
            {
                Ok(key) => key,
                Err(_) => {
                    state.pending.push_back(PendingStage::ApplyRuntimeResponse(
                        task_id,
                        owner,
                        frame,
                        Err(sema_core::SemaError::eval(
                            "runtime wait identity exhausted",
                        )),
                    ));
                    return Ok(());
                }
            };
            if let Err(error) = install_origin_barrier_wait(&mut state, task_id, key, owner, frame)
            {
                let (owner, frame, error) = *error;
                state.pending.push_back(PendingStage::ApplyRuntimeResponse(
                    task_id,
                    owner,
                    frame,
                    Err(error),
                ));
            }
            return Ok(());
        }
        if let RuntimeRequest::CreateResourceGate { continuation } = request {
            let allocated = self.state.borrow_mut().resource_gates.allocate();
            let (frame, response) = match allocated {
                Ok(gate) => {
                    let gate = self.resource_gate_handle(gate);
                    let frame =
                        ContinuationFrame::native(Box::new(ResourceGateAllocationDelivery {
                            gate: gate.clone(),
                            continuation,
                        }));
                    (frame, Ok(RuntimeResponse::ResourceGate(gate)))
                }
                Err(_) => (
                    ContinuationFrame::native(continuation),
                    Err(sema_core::SemaError::eval(
                        "runtime resource gate identity exhausted",
                    )),
                ),
            };
            self.state
                .borrow_mut()
                .pending
                .push_back(PendingStage::ApplyRuntimeResponse(
                    task_id, owner, frame, response,
                ));
            return Ok(());
        }
        if let RuntimeRequest::ReleaseResourceGate { gate, continuation } = request {
            let response = {
                let mut state = self.state.borrow_mut();
                let result = state
                    .resource_gates
                    .release(gate)
                    .map(|()| RuntimeResponse::Value(sema_core::Value::nil()))
                    .map_err(registry_error);
                // A release grants the FIFO head (if any) — deliver that wake.
                while let Some(wake) = state.resource_gates.pop_wake() {
                    state
                        .pending
                        .push_back(PendingStage::ResourceGateWake(wake));
                }
                result
            };
            self.state
                .borrow_mut()
                .pending
                .push_back(PendingStage::ApplyRuntimeResponse(
                    task_id,
                    owner,
                    ContinuationFrame::native(continuation),
                    response,
                ));
            return Ok(());
        }
        if let RuntimeRequest::CloseResourceGate { gate, continuation } = request {
            let response = {
                let mut state = self.state.borrow_mut();
                close_resource_gate(&mut state, gate)
                    .map(|_| RuntimeResponse::Value(sema_core::Value::nil()))
                    .map_err(registry_error)
            };
            self.state
                .borrow_mut()
                .pending
                .push_back(PendingStage::ApplyRuntimeResponse(
                    task_id,
                    owner,
                    ContinuationFrame::native(continuation),
                    response,
                ));
            return Ok(());
        }
        if let RuntimeRequest::CreateSettledPromise {
            outcome,
            continuation,
        } = request
        {
            let (response, rejected_outcome, terminal_fault) = {
                let mut state = self.state.borrow_mut();
                #[cfg(test)]
                let promise_exhausted = state.force_promise_exhaustion;
                #[cfg(not(test))]
                let promise_exhausted = false;
                if promise_exhausted {
                    (
                        Err(sema_core::SemaError::eval(
                            "runtime promise identity exhausted",
                        )),
                        Some(outcome),
                        None,
                    )
                } else if let Ok(promise) = state.promises.reserve_id() {
                    #[cfg(test)]
                    let settlement_exhausted = state.force_settlement_exhaustion;
                    #[cfg(not(test))]
                    let settlement_exhausted = false;
                    if let Some(sequence) = (!settlement_exhausted)
                        .then(|| state.settlement_ids.allocate())
                        .transpose()
                        .ok()
                        .flatten()
                    {
                        let settlement = Rc::new(TaskSettlement { sequence, outcome });
                        state.promises.insert_pending(promise, None);
                        let wakes = state
                            .promises
                            .settle(promise, settlement)
                            .expect("reserved promise was inserted pending");
                        if !wakes.is_empty() {
                            state.pending.push_back(PendingStage::PromiseWakes(wakes));
                        }
                        (Ok(RuntimeResponse::Promise(promise)), None, None)
                    } else {
                        let fault = RuntimeFault::IdExhausted { kind: "settlement" };
                        state.shutting_down = true;
                        state.terminal_fault = Some(fault.clone());
                        (
                            Err(sema_core::SemaError::eval(
                                "runtime settlement identity exhausted",
                            )),
                            Some(outcome),
                            Some(fault),
                        )
                    }
                } else {
                    (
                        Err(sema_core::SemaError::eval(
                            "runtime promise identity exhausted",
                        )),
                        Some(outcome),
                        None,
                    )
                }
            };
            drop(rejected_outcome);
            self.state
                .borrow_mut()
                .pending
                .push_back(PendingStage::ApplyRuntimeResponse(
                    task_id,
                    owner,
                    ContinuationFrame::native(continuation),
                    response,
                ));
            return terminal_fault.map_or(Ok(()), Err);
        }
        // Tasks newly cancelled by this request (the direct target + any
        // transitively-cancelled descendants) whose in-flight wait teardown is
        // delivered eagerly, once the `state` borrow below is dropped (C2).
        let mut eager_cancel_targets: Vec<TaskId> = Vec::new();
        let (continuation, response) = {
            let mut state = self.state.borrow_mut();
            match request {
                RuntimeRequest::Spawn { .. } => unreachable!("spawn extracted before borrow"),
                RuntimeRequest::PromiseSetWait { .. } => {
                    unreachable!("promise wait extracted before borrow")
                }
                RuntimeRequest::CreateChannel {
                    capacity,
                    continuation,
                } => {
                    #[cfg(test)]
                    let exhausted = state.force_channel_exhaustion;
                    #[cfg(not(test))]
                    let exhausted = false;
                    let response = (!exhausted)
                        .then(|| state.channels.allocate(capacity))
                        .transpose()
                        .ok()
                        .flatten()
                        .ok_or(sema_core::runtime::IdExhausted)
                        .map(RuntimeResponse::Channel)
                        .map_err(|_| {
                            sema_core::SemaError::eval("runtime channel identity exhausted")
                        });
                    (continuation, response)
                }
                RuntimeRequest::CreateSettledPromise { .. } => {
                    unreachable!("settled promise extracted before borrow")
                }
                RuntimeRequest::InspectPromise {
                    promise,
                    continuation,
                } => {
                    let response = state
                        .promises
                        .state(promise)
                        .map(|promise| {
                            RuntimeResponse::Settlement(match promise {
                                PromiseState::Pending => None,
                                PromiseState::Returned(s)
                                | PromiseState::Failed(s)
                                | PromiseState::Cancelled(s) => Some(s),
                            })
                        })
                        .map_err(registry_error);
                    (continuation, response)
                }
                RuntimeRequest::CancelPromise {
                    promise,
                    continuation,
                } => {
                    let response = state
                        .promises
                        .task(promise)
                        .map(|target| {
                            let newly = target
                                .and_then(|target| state.tasks.get_mut(&target))
                                .is_some_and(|task| {
                                    task.record.request_cancellation(CancelReason::Explicit)
                                });
                            if newly {
                                if let Some(target) = target {
                                    eager_cancel_targets.push(target);
                                    state.pending_cancel_waits.push_back(target);
                                }
                            }
                            // Structured transitive cancel: propagate to every task
                            // (transitively) spawned by the cancelled target via the
                            // cancellation-parent graph, so a subprocess/IO awaited
                            // one `async/spawn` layer deeper is not orphaned running
                            // to completion. Descendants carry `CancelReason::Owner`
                            // (cancelled because their owner was). No-op when the
                            // target has no children (the common direct-cancel case).
                            if let Some(target) = target {
                                eager_cancel_targets.extend(cancel_descendants(&mut state, target));
                            }
                            RuntimeResponse::Cancelled(newly)
                        })
                        .map_err(registry_error);
                    (continuation, response)
                }
                RuntimeRequest::ChannelOp {
                    channel,
                    operation,
                    continuation,
                } => {
                    use sema_core::runtime::ChannelOperation;
                    let response = match operation {
                        ChannelOperation::Close => state.channels.close(channel).map(|close| {
                            if let Some(close) = close {
                                state.pending.push_back(PendingStage::ChannelClose(close));
                                RuntimeResponse::Value(sema_core::Value::TRUE)
                            } else {
                                RuntimeResponse::Value(sema_core::Value::FALSE)
                            }
                        }),
                        ChannelOperation::TryReceive => {
                            state.channels.try_receive(channel).map(|result| {
                                RuntimeResponse::Receive(match result {
                                    super::ChannelResult::Received(value) => {
                                        sema_core::runtime::ChannelReceive::Received(value)
                                    }
                                    super::ChannelResult::Closed => {
                                        sema_core::runtime::ChannelReceive::Closed
                                    }
                                    _ => sema_core::runtime::ChannelReceive::Empty,
                                })
                            })
                        }
                        ChannelOperation::Inspect(query) => state
                            .channels
                            .inspect(channel, query)
                            .map(RuntimeResponse::Value),
                    }
                    .map_err(registry_error);
                    (continuation, response)
                }
                RuntimeRequest::OriginBarrier { .. } => {
                    unreachable!("origin barrier extracted before borrow")
                }
                RuntimeRequest::CreateResourceGate { .. }
                | RuntimeRequest::ReleaseResourceGate { .. }
                | RuntimeRequest::CloseResourceGate { .. } => {
                    unreachable!("resource gate request extracted before borrow")
                }
            }
        };
        let mut state = self.state.borrow_mut();
        if let Some(wake) = state.channels.pop_wake() {
            state.pending.push_back(PendingStage::ChannelWake(wake));
        }
        state.pending.push_back(PendingStage::ApplyRuntimeResponse(
            task_id,
            owner,
            ContinuationFrame::native(continuation),
            response,
        ));
        drop(state);
        // Deliver wait teardown for every task this request cancelled, now that the
        // `state` borrow is dropped (the External abort hook may re-enter the
        // runtime, so it must run unborrowed). C2 eager delivery.
        for target in eager_cancel_targets {
            deliver_cancel_teardown(&self.state, target)?;
        }
        self.state
            .borrow()
            .terminal_fault
            .clone()
            .map_or(Ok(()), Err)
    }

    fn invoke_callable(
        &self,
        task_id: TaskId,
        mut owner: ReturnOwner,
        call: NativeCall,
    ) -> Result<(), RuntimeFault> {
        let (eval_context, context, cancellation) = {
            let state = self.state.borrow();
            let task = state
                .tasks
                .get(&task_id)
                .ok_or_else(|| RuntimeFault::Invariant {
                    message: "calling task disappeared".into(),
                })?;
            (
                Rc::clone(&state._context),
                task.context.clone(),
                task.record.cancellation(),
            )
        };
        if let Some(cancellation) = cancellation {
            let frame = if extract_vm_closure(&call.callable).is_some()
                || call.callable.as_multimethod_rc().is_some()
            {
                ContinuationFrame::vm_native(call.continuation)
            } else {
                ContinuationFrame::native(call.continuation)
            };
            self.state
                .borrow_mut()
                .pending
                .push_back(PendingStage::Resume(
                    task_id,
                    owner,
                    frame,
                    ResumeInput::Cancelled(cancellation.reason),
                ));
            return Ok(());
        }
        if call.callable.as_multimethod_rc().is_some() {
            let dispatch =
                multimethod_call(call.callable, call.args, call.continuation).map_err(|error| {
                    RuntimeFault::Invariant {
                        message: error.to_string(),
                    }
                })?;
            return self.invoke_callable(task_id, owner, dispatch);
        }
        let (frame, result) =
            if let Some((closure, functions, native_fns)) = extract_vm_closure(&call.callable) {
                // A cooperative HOF (`map`/`for-each`/`foldl`/…) dispatches its
                // Sema callback on a callback VM. See `invoke_vm_callback_loop`
                // for the full element-chain dispatch (Task C: it runs the whole
                // non-yielding element chain in place, on one reused scratch VM,
                // instead of round-tripping the ready queue per element).
                return self
                    .invoke_vm_callback_loop(task_id, owner, call, closure, functions, native_fns);
            } else if let Some(native) = call.callable.as_native_fn_rc() {
                if !native.escaping_args().is_empty() {
                    if let Some(parent_vm) = owner.parked_parent_vm_mut() {
                        snapshot_native_escaping_args_with_owner(parent_vm, &native, &call.args);
                    }
                }
                let _installed = eval_context.scope_task_context(context.clone());
                let mut task_context = context.borrow_mut();
                let mut native_context = NativeCallContext {
                    eval_context: &eval_context,
                    task_context: &mut task_context,
                    cancellation: CancellationView::new(
                        cancellation.is_some(),
                        cancellation.map(|request| request.reason),
                    ),
                };
                // Dispatch the native with the runtime-quantum flag active (a
                // ctx-less native like `mcp/call` reads `in_runtime_quantum()`
                // internally to route cooperatively) so it takes its structural
                // ABI: `invoke_runtime` returns the native's `NativeOutcome`
                // (Suspend/Call/Return) directly, driven by the caller's
                // `PendingStage::Apply`.
                let _quantum_guard = eval_context.enter_runtime_quantum().map_err(|error| {
                    RuntimeFault::Invariant {
                        message: error.to_string(),
                    }
                })?;
                let native_result = native.invoke_runtime(&mut native_context, &call.args);
                (ContinuationFrame::native(call.continuation), native_result)
            } else if let Some(keyword) = call.callable.as_keyword_spur() {
                let result = if call.args.len() != 1 {
                    Err(sema_core::SemaError::arity(
                        sema_core::resolve(keyword),
                        "1",
                        call.args.len(),
                    ))
                } else {
                    let key = Value::keyword_from_spur(keyword);
                    let arg = &call.args[0];
                    if let Some(map) = arg.as_map_rc() {
                        Ok(map.get(&key).cloned().unwrap_or_else(Value::nil))
                    } else if let Some(map) = arg.as_hashmap_rc() {
                        Ok(map.get(&key).cloned().unwrap_or_else(Value::nil))
                    } else {
                        Err(sema_core::SemaError::type_error(
                            "map or hashmap",
                            arg.type_name(),
                        ))
                    }
                };
                (
                    ContinuationFrame::native(call.continuation),
                    result.map(NativeOutcome::Return),
                )
            } else {
                (
                    ContinuationFrame::vm_native(call.continuation),
                    Err(sema_core::SemaError::type_error(
                        "callable",
                        call.callable.type_name(),
                    )),
                )
            };
        self.state
            .borrow_mut()
            .pending
            .push_back(PendingStage::Apply(
                task_id,
                ReturnOwner::Continuation(Box::new(owner), frame),
                result,
            ));
        Ok(())
    }

    /// Take the reusable scratch VM (`RuntimeState::scratch_callback_vm`),
    /// re-targeting it at `globals`/`functions`/`native_fns` via
    /// [`VM::reset_for_task_with_native_fns`] (reusing its `stack`/`frames`/
    /// `inline_cache` allocations), or build a fresh one if the slot is empty
    /// (first use, or a prior in-place chain's VM got consumed into a parked
    /// task and never returned).
    fn take_scratch_callback_vm(
        &self,
        globals: Rc<Env>,
        functions: Rc<Vec<Rc<Function>>>,
        native_fns: Rc<Vec<Rc<NativeFn>>>,
    ) -> VM {
        match self.state.borrow_mut().scratch_callback_vm.take() {
            Some(mut vm) => {
                vm.reset_for_task_with_native_fns(globals, functions, native_fns);
                vm
            }
            None => VM::new_for_task_with_native_fns(globals, functions, native_fns),
        }
    }

    /// Drive a cooperative HOF callback's ENTIRE non-yielding element chain
    /// in place, inside this one drive work item, instead of round-tripping
    /// each element through `PendingStage::Invoke` -> ready-queue enqueue ->
    /// `visit_ready` quantum -> `PendingStage::Apply`/`Resume` (>= 4 drive-loop
    /// iterations and a fresh `VM::new_for_task_with_native_fns` PER ELEMENT —
    /// ~28k instructions/element of pure scheduler overhead, the dominant cost
    /// of `filter`/`map`/`foldl`/`for-each` under the unified runtime; see
    /// `docs/plans/archive/2026-07-16-runtime-fast-path-recovery.md` Task C).
    ///
    /// `call` is the FIRST element's `NativeCall` (already known to target a VM
    /// closure — `invoke_callable` extracted it before delegating here). The
    /// upvalue-closing snapshot against the parked parent VM runs for EVERY
    /// element (it is not safe to hoist
    /// to a one-time snapshot before the loop): it walks not just the
    /// callable's captured env but each element's `args`, which can carry
    /// DIFFERENT closures with their own open upvalues on the parent VM's
    /// stack from element to element (e.g. `(for-each (fn (entry) ((cadr
    /// entry) ev)) (map/entries handlers))` — each `entry` embeds a different
    /// handler closure that must be closed before it can be called from a
    /// foreign VM).
    ///
    /// One scratch VM (`take_scratch_callback_vm`) runs each element's
    /// `setup_for_call` + `run_quantum` back to back — `NativeContinuation::
    /// resume` (e.g. `FilterContinuation`/`MapContinuation` in
    /// `sema-stdlib::list`) is called DIRECTLY on a `Finished` quantum, and if
    /// it hands back another `NativeOutcome::Call`, the loop continues without
    /// ever touching `state.tasks` or the ready queue. `task` is held out of
    /// `state.tasks` for the loop's duration (mirroring `visit_ready`, which
    /// does the same for a single quantum) and reinserted once the chain
    /// settles, hands off, or falls back.
    ///
    /// Every quantum's instructions are debited from ONE shared budget
    /// (`remaining_budget`, seeded from `active_instruction_limit` — the SAME
    /// total a single parked quantum would receive) so a multi-million-element
    /// chain still yields to sibling roots instead of running unboundedly; on
    /// exhaustion the in-flight element falls back to exactly today's parked
    /// path for the rest of the chain (`quantum_to_action`'s `QuantumExpired`
    /// arm — reached here by simply letting `run_quantum`'s own budget check
    /// fire at `remaining_budget == 0`, no separate precheck needed).
    ///
    /// A callback that suspends for real (channel/promise/sleep/spawn, a
    /// nested HOF, a debug stop) surfaces as anything other than `Finished`/
    /// `Err` from the quantum; that element falls back to EXACTLY today's
    /// parked path via `quantum_to_action` — the shared mapping
    /// `run_parked_quantum` uses — so the fallback can never drift from the
    /// slow path's semantics. A bare runtime-only native's "requires runtime
    /// invocation" value-ABI stub, and a dual-ABI native's own "cannot suspend
    /// from a synchronous callback" error, surface unchanged: they fire inside
    /// `NativeContinuation::resume` implementations / the native-fn arm of
    /// `invoke_callable`, neither of which this function changes.
    ///
    /// Cancellation is re-read fresh from `task.record` before EVERY element
    /// (not just the first): a cancellation recorded against this task before
    /// its `PendingStage::Invoke` was even popped — the only way it CAN land,
    /// since nothing else runs on this thread while this loop is executing —
    /// aborts the chain the same way `visit_ready`'s pre-quantum cancellation
    /// check does: the about-to-run element's VM frame is discarded UNRUN and
    /// its continuation is resumed with `ResumeInput::Cancelled` directly
    /// (mirroring `resume_continuation`'s unconditional cancellation
    /// override). A cancellation that arrives WHILE this loop is running can
    /// only do so at a fallback boundary (budget exhaustion or a real
    /// suspend), where the task is reinserted into `state.tasks` and control
    /// returns to the drive loop — the same granularity a single-element
    /// quantum already provided.
    fn invoke_vm_callback_loop(
        &self,
        task_id: TaskId,
        mut owner: ReturnOwner,
        call: NativeCall,
        closure: Rc<Closure>,
        functions: Rc<Vec<Rc<Function>>>,
        native_fns: Rc<Vec<Rc<NativeFn>>>,
    ) -> Result<(), RuntimeFault> {
        let globals = closure
            .globals
            .clone()
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "VM closure has no home environment".into(),
            })?;

        let (eval_context, root, mut remaining_budget, mut task) = {
            let mut state = self.state.borrow_mut();
            let task = state
                .tasks
                .remove(&task_id)
                .ok_or_else(|| RuntimeFault::Invariant {
                    message: "calling task disappeared".into(),
                })?;
            let eval_context = Rc::clone(&state._context);
            let root = task.record.relations().origin_root;
            let remaining = state.active_instruction_limit;
            (eval_context, root, remaining, task)
        };

        let mut vm =
            self.take_scratch_callback_vm(globals.clone(), functions.clone(), native_fns.clone());
        let mut loaded_globals = globals;
        let mut loaded_functions = functions;
        let mut loaded_native_fns = native_fns;

        let quantum_guard = match eval_context.enter_runtime_quantum() {
            Ok(guard) => guard,
            Err(error) => {
                self.state.borrow_mut().scratch_callback_vm = Some(vm);
                let mut state = self.state.borrow_mut();
                state.tasks.insert(task_id, task);
                return Err(RuntimeFault::Invariant {
                    message: error.to_string(),
                });
            }
        };
        let _task_context = eval_context.scope_task_context(task.context.clone());
        let is_root_main = {
            let state = self.state.borrow();
            matches!(
                state.roots.get(&root).map(RootRecord::state),
                Some(RootState::Running { main_task }) if *main_task == task_id
            )
        };
        let published_task_id = if is_root_main {
            None
        } else {
            Some(RuntimeTaskId::new(root.runtime(), task_id))
        };
        // `QuantumIdGuard` restores both ids even on an early `?`-return or
        // panic inside the loop below — see its doc comment.
        let mut id_guard = QuantumIdGuard::install(published_task_id, root);
        let mut scopes = TaskScopeSwap::install(&mut task);
        debug_assert!(
            self.state.try_borrow_mut().is_ok(),
            "RuntimeState borrowed at in-place HOF loop entry — a blocking debug stop would deadlock the state cell"
        );

        enum ElementOutcome {
            Handoff(NativeCall),
            Settled(NativeResult),
            // Carries the raw quantum result and the not-yet-resumed
            // continuation rather than a fully-built `TaskAction`: building the
            // `TaskAction` needs to move `vm`/`owner`, and a move inside a
            // `loop { .. break ..; }` body is rejected by the borrow checker
            // even though this arm only ever executes once (the loop ends at
            // every `break`) — deferring the move to the single post-loop
            // match arm below sidesteps that false conflict.
            Suspended(
                VmQuantumResult,
                Box<dyn sema_core::runtime::NativeContinuation>,
            ),
        }

        let mut current_call = call;
        let outcome = loop {
            if let Some(cancel) = task.record.cancellation() {
                let mut task_context = task.context.borrow_mut();
                let mut native_context = NativeCallContext {
                    eval_context: &eval_context,
                    task_context: &mut task_context,
                    cancellation: CancellationView::new(true, Some(cancel.reason)),
                };
                let resumed = current_call
                    .continuation
                    .resume(&mut native_context, ResumeInput::Cancelled(cancel.reason));
                break ElementOutcome::Settled(resumed);
            }
            let Some((next_closure, next_functions, next_native_fns)) =
                extract_vm_closure(&current_call.callable)
            else {
                break ElementOutcome::Handoff(current_call);
            };
            let Some(next_globals) = next_closure.globals.clone() else {
                let mut task_context = task.context.borrow_mut();
                let mut native_context = NativeCallContext {
                    eval_context: &eval_context,
                    task_context: &mut task_context,
                    cancellation: CancellationView::default(),
                };
                let resumed = current_call.continuation.resume(
                    &mut native_context,
                    ResumeInput::Failed(sema_core::SemaError::eval(
                        "VM closure has no home environment",
                    )),
                );
                break ElementOutcome::Settled(resumed);
            };
            if !Rc::ptr_eq(&loaded_globals, &next_globals)
                || !Rc::ptr_eq(&loaded_functions, &next_functions)
                || !Rc::ptr_eq(&loaded_native_fns, &next_native_fns)
            {
                vm.reset_for_task_with_native_fns(
                    next_globals.clone(),
                    next_functions.clone(),
                    next_native_fns.clone(),
                );
                loaded_globals = next_globals;
                loaded_functions = next_functions;
                loaded_native_fns = next_native_fns;
            }
            // MUST run per element, not once for the chain: it walks not just
            // the callable but `args`, which can carry a DIFFERENT closure
            // with its own open upvalues on the parent VM's stack on every
            // element (see this function's doc comment).
            if let Some(parent_vm) = owner.parked_parent_vm_mut() {
                snapshot_escaping_call_with_owner(
                    parent_vm,
                    &current_call.callable,
                    &current_call.args,
                );
            }
            if let Err(error) = vm.setup_for_call(next_closure, &current_call.args) {
                let mut task_context = task.context.borrow_mut();
                let mut native_context = NativeCallContext {
                    eval_context: &eval_context,
                    task_context: &mut task_context,
                    cancellation: CancellationView::default(),
                };
                let resumed = current_call
                    .continuation
                    .resume(&mut native_context, ResumeInput::Failed(error));
                break ElementOutcome::Settled(resumed);
            }
            let cancellation_view = CancellationView::default();
            let quantum = if crate::vm::is_debug_session_active_for(root) {
                crate::vm::with_active_debug_for_root(root, |debug| {
                    vm.run_quantum_debug(&eval_context, remaining_budget, cancellation_view, debug)
                })
                .expect("debug session active for root but no DebugState registered")
            } else {
                vm.run_quantum(&eval_context, remaining_budget, cancellation_view)
            };
            self.state.borrow_mut().turn_instructions += quantum.instructions;
            remaining_budget = remaining_budget.saturating_sub(quantum.instructions);
            match quantum.outcome {
                Ok(VmExecResult::Finished(value)) => {
                    let mut task_context = task.context.borrow_mut();
                    let mut native_context = NativeCallContext {
                        eval_context: &eval_context,
                        task_context: &mut task_context,
                        cancellation: CancellationView::default(),
                    };
                    let resumed = current_call
                        .continuation
                        .resume(&mut native_context, ResumeInput::Returned(value));
                    match resumed {
                        Ok(NativeOutcome::Call(next)) => {
                            current_call = next;
                            continue;
                        }
                        other => break ElementOutcome::Settled(other),
                    }
                }
                Err(error) => {
                    let mut task_context = task.context.borrow_mut();
                    let mut native_context = NativeCallContext {
                        eval_context: &eval_context,
                        task_context: &mut task_context,
                        cancellation: CancellationView::default(),
                    };
                    let resumed = current_call
                        .continuation
                        .resume(&mut native_context, ResumeInput::Failed(error));
                    match resumed {
                        Ok(NativeOutcome::Call(next)) => {
                            current_call = next;
                            continue;
                        }
                        other => break ElementOutcome::Settled(other),
                    }
                }
                _ => {
                    // Genuine suspend (structural `Pending`, `Stopped`) or a
                    // budget expiry: fall back to exactly
                    // today's parked path via the shared mapping (applied
                    // after the loop, once `vm`/`owner` can be moved safely).
                    break ElementOutcome::Suspended(quantum, current_call.continuation);
                }
            }
        };

        id_guard.restore();
        scopes.restore(&mut task);
        drop(quantum_guard);

        match outcome {
            ElementOutcome::Settled(result) => {
                self.state.borrow_mut().scratch_callback_vm = Some(vm);
                let mut state = self.state.borrow_mut();
                if state.tasks.insert(task_id, task).is_some() {
                    return Err(RuntimeFault::Invariant {
                        message: "task identity reused during in-place HOF loop".into(),
                    });
                }
                state
                    .pending
                    .push_back(PendingStage::Apply(task_id, owner, result));
                Ok(())
            }
            ElementOutcome::Handoff(next_call) => {
                self.state.borrow_mut().scratch_callback_vm = Some(vm);
                let mut state = self.state.borrow_mut();
                if state.tasks.insert(task_id, task).is_some() {
                    return Err(RuntimeFault::Invariant {
                        message: "task identity reused during in-place HOF loop".into(),
                    });
                }
                state
                    .pending
                    .push_back(PendingStage::Invoke(task_id, owner, next_call));
                Ok(())
            }
            ElementOutcome::Suspended(quantum, continuation) => {
                // `vm_owner` must be populated before `quantum_to_action` runs —
                // this loop never sets it, since every element that finishes
                // cleanly bypasses it entirely — with exactly the
                // `ReturnOwner::Continuation` shape the parked path expects.
                task.vm_owner = Some(ReturnOwner::Continuation(
                    Box::new(owner),
                    ContinuationFrame::vm_native(continuation),
                ));
                let action = self.quantum_to_action(root, task_id, &mut task, vm, quantum);
                // `vm` was consumed into `task.vm_call`/`ReturnOwner::VmResume`
                // by `quantum_to_action` (or is absent when the outcome settled
                // directly, e.g. an uncaught `Err` from a `Pending` mapping) —
                // never returned to the scratch slot; it refills on next use.
                {
                    let mut state = self.state.borrow_mut();
                    if state.tasks.insert(task_id, task).is_some() {
                        return Err(RuntimeFault::Invariant {
                            message: "task identity reused during in-place HOF loop".into(),
                        });
                    }
                }
                if matches!(action, TaskAction::DebugStop(..)) {
                    self.apply_action(action)?;
                } else {
                    self.state
                        .borrow_mut()
                        .pending
                        .push_back(PendingStage::Action(action));
                }
                Ok(())
            }
        }
    }

    fn resume_continuation(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
        frame: ContinuationFrame,
        input: ResumeInput,
    ) -> Result<(), RuntimeFault> {
        let resumed = self.resume_continuation_value(task_id, frame, input)?;
        self.state
            .borrow_mut()
            .pending
            .push_back(PendingStage::Apply(task_id, owner, resumed));
        Ok(())
    }

    /// Resume a continuation with `input` and return the `NativeResult`,
    /// without queuing anything. Factored out of `resume_continuation` so
    /// `install_channel_wait_and_resume`'s inline fast path (Task D) can
    /// apply the result immediately instead of via `PendingStage::Apply`.
    ///
    /// Scopes its `self.state` borrows exactly as `resume_continuation`
    /// always has: `frame.resume` — which may run arbitrary Sema-level
    /// evaluation — is called with NO borrow of `self.state` held (see
    /// `ChannelRendezvousResume`'s doc for why that matters: a collection
    /// pass triggered inside `frame.resume` needs to `try_borrow()` the same
    /// `RefCell` to trace channel buffers, and silently skips tracing them if
    /// it's already held).
    fn resume_continuation_value(
        &self,
        task_id: TaskId,
        frame: ContinuationFrame,
        input: ResumeInput,
    ) -> Result<NativeResult, RuntimeFault> {
        let (eval_context, context, cancellation) = {
            let state = self.state.borrow();
            let Some(task) = state.tasks.get(&task_id) else {
                return Err(RuntimeFault::Invariant {
                    message: "continuation task disappeared".into(),
                });
            };
            (
                Rc::clone(&state._context),
                task.context.clone(),
                task.record.cancellation(),
            )
        };
        let mut task_context = context.borrow_mut();
        let mut native_context = NativeCallContext {
            eval_context: &eval_context,
            task_context: &mut task_context,
            cancellation: CancellationView::new(
                cancellation.is_some(),
                cancellation.map(|request| request.reason),
            ),
        };
        let input = cancellation
            .map(|request| ResumeInput::Cancelled(request.reason))
            .unwrap_or(input);
        let resumed = frame.resume(&mut native_context, input);
        drop(task_context);
        if !self.state.borrow().tasks.contains_key(&task_id) {
            return Err(RuntimeFault::Invariant {
                message: "continuation task disappeared".into(),
            });
        }
        Ok(resumed)
    }

    /// Admit a detached child for a `RuntimeRequest::Spawn` and resume the
    /// spawner's continuation with the child's canonical registry promise. It
    /// allocates a `PromiseRegistry` promise bound to the child task, so the
    /// child settles through the checked `settle`/`task_promises` path and its
    /// `TaskSettlement` (preserving the real outcome) reaches the continuation as
    /// `RuntimeResponse::Promise(id)`. This is the sole `async/spawn` path.
    fn spawn_via_registry(
        &self,
        spawner: TaskId,
        mut owner: ReturnOwner,
        thunk: sema_core::Value,
        continuation: Box<dyn sema_core::runtime::NativeContinuation>,
    ) -> Result<(), RuntimeFault> {
        let frame = ContinuationFrame::native(continuation);
        let respond_err = |owner: ReturnOwner,
                           frame: ContinuationFrame,
                           error: sema_core::SemaError|
         -> Result<(), RuntimeFault> {
            self.state
                .borrow_mut()
                .pending
                .push_back(PendingStage::ApplyRuntimeResponse(
                    spawner,
                    owner,
                    frame,
                    Err(error),
                ));
            Ok(())
        };
        let Some((closure, functions, native_fns)) = extract_vm_closure(&thunk) else {
            return respond_err(
                owner,
                frame,
                sema_core::SemaError::eval(
                    "async/spawn: argument must be a function (compiled VM closure)",
                ),
            );
        };
        // The native that yielded `Spawn` is running inside a VM quantum whose VM
        // is parked in `owner`; snapshot any still-open upvalue cells the thunk
        // captures against it. Fall back to the guard-free snapshot when there is
        // no parked VM (e.g. a runtime-native spawner in tests).
        match owner.parked_parent_vm_mut() {
            Some(spawning_vm) => close_closure_upvalues_with_owner(spawning_vm, &closure),
            None => close_closure_upvalues_for_foreign_run(&closure),
        }
        let Some(globals) = closure.globals.clone() else {
            return respond_err(
                owner,
                frame,
                sema_core::SemaError::eval("async/spawn: thunk closure has no home environment"),
            );
        };
        let mut vm = VM::new_for_task_with_native_fns(globals, functions, native_fns);
        if let Err(error) = vm.setup_for_call(closure, &[]) {
            return respond_err(owner, frame, error);
        }
        let (root, child, promise) = {
            let mut state = self.state.borrow_mut();
            if state.task_ids.is_exhausted() {
                drop(state);
                return respond_err(
                    owner,
                    frame,
                    sema_core::SemaError::eval("async/spawn: task identity exhausted"),
                );
            }
            let child = state
                .task_ids
                .allocate()
                .map_err(|_| RuntimeFault::IdExhausted { kind: "task" })?;
            let root = state
                .tasks
                .get(&spawner)
                .ok_or_else(|| RuntimeFault::Invariant {
                    message: "spawning task disappeared".into(),
                })?
                .record
                .relations()
                .origin_root;
            let promise = match state.promises.allocate_pending(Some(child)) {
                Ok(promise) => promise,
                Err(_) => {
                    drop(state);
                    return respond_err(
                        owner,
                        frame,
                        sema_core::SemaError::eval("async/spawn: promise identity exhausted"),
                    );
                }
            };
            // A detached task is an origin-root child owned by the root (normal
            // root settlement does not cancel it), but its CANCELLATION parent is
            // the spawning TASK, not the root. This is the structured-concurrency
            // cancel edge the plan's "async/cancel through the cancellation-parent
            // graph" relies on: explicitly cancelling a task transitively cancels
            // the tasks it spawned (e.g. a task parked on `async/await` of a child
            // it spawned — the child must not be orphaned running its subprocess).
            // It does NOT make `async/await`/`all`/`race`/`timeout` cancel SUPPLIED
            // promises: those observe producers spawned elsewhere (a different
            // cancellation parent), so an observer's cancellation never reaches
            // them. See docs/plans/archive/2026-07-13-unified-cooperative-runtime-task-04.md.
            let relations = TaskRelations {
                origin_root: root,
                cancellation_parent: CancellationParent::Task(spawner),
                lifetime_owner: LifetimeOwner::Root(root),
            };
            state.tasks.insert(
                child,
                RuntimeTask {
                    record: TaskRecord::new(child, relations),
                    payload: TaskPayload::Vm,
                    pending_resume: None,
                    suspended_owner: None,
                    vm_call: Some(vm),
                    vm_owner: Some(ReturnOwner::Root),
                    context: TaskContextHandle::default(),
                    vm_resume: None,
                    // Seed the child with a snapshot of the spawner's live dynamic
                    // scopes, read from the thread-locals the spawner is still running
                    // under. Per seam (`TASK_SCOPE_SEAMS`): the LLM dynamic scope so a
                    // concurrent fan-out inside one `llm/with-budget` captures the
                    // shared budget `Rc` (charged as one aggregate) and a deferred
                    // `with-cache` completion still sees the cache enabled; the OTel
                    // identity (conversation/session/user ids) with an EMPTY span stack
                    // so the child parents to its own trace root, not a sibling's open
                    // span; and the leaf-usage scope its `workflow/step` opened so the
                    // fan-out's LLM usage attributes to the spawning step. Each scope
                    // rides the task and is swapped per quantum, not the (already
                    // restored) global.
                    scopes: TaskScopes::capture_for_spawn(),
                },
            );
            state.task_promises.insert(child, promise);
            (root, child, promise)
        };
        // Resume the spawner with the child's promise FIRST — enqueued Ready
        // AHEAD of the child — then enqueue the child behind it. This preserves
        // the legacy cooperative order: the child does not run until the spawner
        // next suspends, so a same-quantum observation of the promise
        // (`async/pending?`, `async/cancel`) sees it Pending, and a post-spawn op
        // on the spawner (e.g. `channel/close`) runs before the child's first
        // quantum. `async/spawn` always parks its VM, so `owner` is a `VmResume`:
        // resume it directly with the promise handle (the continuation only maps
        // the id to the handle, which we do here) so its Ready enqueue is not
        // deferred behind the child through the pending-stage chain.
        match owner {
            ReturnOwner::VmResume { vm, parent } => {
                drop(frame);
                self.reinstall_parent_vm(
                    spawner,
                    *vm,
                    *parent,
                    VmResume::Value(sema_core::Value::async_promise_id(promise)),
                )?;
                self.state.borrow_mut().ready.enqueue(root, child);
            }
            other => {
                self.state
                    .borrow_mut()
                    .pending
                    .push_back(PendingStage::ApplyRuntimeResponse(
                        spawner,
                        other,
                        frame,
                        Ok(RuntimeResponse::Promise(promise)),
                    ));
                self.state.borrow_mut().ready.enqueue(root, child);
            }
        }
        Ok(())
    }

    /// Settle a task by identity: a root main task settles its root; a detached
    /// child (`spawn_via_registry`) settles its canonical registry promise,
    /// waking every registered observer.
    fn settle_task(
        &self,
        root: RootId,
        task_id: TaskId,
        outcome: TaskOutcome,
    ) -> Result<(), RuntimeFault> {
        // A cancelled task's bytecode never runs again, so its `__stream-finish` /
        // `__agent-finish` cleanup can't run: notify the reap seam so `sema-llm`
        // reclaims any per-task slab entry (and ends its detached span) this task
        // owned. Idempotent by absence for a normally-finished task.
        if matches!(outcome, TaskOutcome::Cancelled(_)) {
            sema_core::notify_task_reaped(RuntimeTaskId::new(root.runtime(), task_id));
        }
        let is_root_main = {
            let state = self.state.borrow();
            matches!(
                state.roots.get(&root).map(RootRecord::state),
                Some(RootState::Running { main_task }) if *main_task == task_id
            )
        };
        if is_root_main {
            self.settle(root, task_id, outcome)
        } else {
            self.settle_registry_child(task_id, outcome)
        }
    }

    /// Settle a detached registry child: allocate its settlement sequence, drop
    /// the task, and settle its canonical `PromiseRegistry` promise (waking every
    /// registered observer). A detached child is not its root's main task, so it
    /// never transitions the root — the root settles on its own main task.
    fn settle_registry_child(
        &self,
        task_id: TaskId,
        outcome: TaskOutcome,
    ) -> Result<(), RuntimeFault> {
        let mut state = self.state.borrow_mut();
        #[cfg(test)]
        let exhausted = state.force_settlement_exhaustion;
        #[cfg(not(test))]
        let exhausted = false;
        let sequence = match (!exhausted)
            .then(|| state.settlement_ids.allocate())
            .transpose()
            .ok()
            .flatten()
        {
            Some(sequence) => sequence,
            None => {
                let fault = RuntimeFault::IdExhausted { kind: "settlement" };
                state.shutting_down = true;
                state.terminal_fault = Some(fault.clone());
                if let Some(task) = state.tasks.get_mut(&task_id) {
                    task.record
                        .yield_ready()
                        .map_err(|error| RuntimeFault::Invariant {
                            message: format!("terminal detached child failed to yield: {error:?}"),
                        })?;
                    let root = task.record.relations().origin_root;
                    state.ready.enqueue(root, task_id);
                }
                return Err(fault);
            }
        };
        let mut task = state
            .tasks
            .remove(&task_id)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "settling detached child disappeared".into(),
            })?;
        let settlement = task
            .record
            .settle(sequence, outcome)
            .expect("live detached child settlement is infallible");
        if let Some(promise) = state.task_promises.remove(&task_id) {
            let wakes = state
                .promises
                .settle(promise, Rc::clone(&settlement))
                .map_err(|error| RuntimeFault::Invariant {
                    message: format!("detached child promise settlement failed: {error:?}"),
                })?;
            if !wakes.is_empty() {
                state.pending.push_back(PendingStage::PromiseWakes(wakes));
            }
        }
        drop(state);
        drop(task);
        Ok(())
    }

    fn settle(
        &self,
        root: RootId,
        task_id: TaskId,
        outcome: TaskOutcome,
    ) -> Result<(), RuntimeFault> {
        let mut state = self.state.borrow_mut();
        state
            .tasks
            .get(&task_id)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "settling task disappeared".into(),
            })?;
        state
            .roots
            .get(&root)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "settling task root disappeared".into(),
            })?
            .validate_settlement(task_id)
            .map_err(|error| RuntimeFault::Invariant {
                message: format!("root settlement transition failed: {error:?}"),
            })?;
        #[cfg(test)]
        let exhausted = state.force_settlement_exhaustion;
        #[cfg(not(test))]
        let exhausted = false;
        let sequence = match (!exhausted)
            .then(|| state.settlement_ids.allocate())
            .transpose()
            .ok()
            .flatten()
        {
            Some(sequence) => sequence,
            None => {
                let fault = RuntimeFault::IdExhausted { kind: "settlement" };
                state.shutting_down = true;
                state.terminal_fault = Some(fault.clone());
                if let Some(task) = state.tasks.get_mut(&task_id) {
                    task.record
                        .yield_ready()
                        .map_err(|error| RuntimeFault::Invariant {
                            message: format!("terminal task failed to yield: {error:?}"),
                        })?;
                    state.ready.enqueue(root, task_id);
                }
                return Err(fault);
            }
        };
        let mut task = state.tasks.remove(&task_id).expect("task prevalidated");
        let settlement = task
            .record
            .settle(sequence, outcome)
            .expect("live task settlement is infallible");
        if let Some(promise) = state.task_promises.remove(&task_id) {
            let wakes = state
                .promises
                .settle(promise, Rc::clone(&settlement))
                .map_err(|error| RuntimeFault::Invariant {
                    message: format!("task promise settlement failed: {error:?}"),
                })?;
            if !wakes.is_empty() {
                state.pending.push_back(PendingStage::PromiseWakes(wakes));
            }
        }
        let root_record = state.roots.get_mut(&root).expect("root prevalidated");
        root_record
            .settle(task_id, settlement)
            .expect("root settlement was prevalidated");
        root_record.release_descendant();
        if root_record.is_reap_eligible() {
            state.handle_cleanup.push_back(root);
        }
        drop(state);
        drop(task);
        Ok(())
    }

    /// Force-settle the requested `root` as `Failed` with the public deadlock
    /// diagnostic. Called by the host drive loop when the runtime has gone
    /// fully idle — `DriveState::Idle { next_deadline: None,
    /// inbox_wakeup_required: false }` — yet
    /// the root is still `Running`: no task made progress this turn and there is
    /// no timer deadline nor pending external completion that could ever change
    /// that, so the root is parked on an intra-runtime wait (channel/promise)
    /// that nothing runnable can satisfy. That is a genuine deadlock.
    ///
    /// The error text distinguishes synchronous channel exhaustion from an
    /// async task graph with no runnable producer:
    /// - root main task parked directly on `channel/recv` (top-level, no sender)
    ///   → "channel/recv: channel is empty";
    /// - root main task parked directly on `channel/send` (full, no receiver)
    ///   → "channel/send: channel is full";
    /// - otherwise (awaiting a never-settling promise, mutual await, a spawned
    ///   task that is itself blocked, …) → "async scheduler: all tasks blocked
    ///   (deadlock detected)". A channel op *inside* a spawn parks a child task,
    ///   leaving the root main task on a promise wait, so it uses the async
    ///   deadlock diagnostic rather than a direct channel error.
    ///
    /// Returns `Ok(true)` when it settled the root; `Ok(false)` when the root was
    /// not force-settleable (already settled/aborted, or its main task was not
    /// parked) so the caller can fall back to its unsupported-suspension error
    /// rather than inventing a settlement.
    pub fn settle_deadlocked_root(&self, root: RootId) -> Result<bool, RuntimeFault> {
        let main_task = {
            let state = self.state.borrow();
            match state.roots.get(&root).map(RootRecord::state) {
                Some(RootState::Running { main_task }) => *main_task,
                _ => return Ok(false),
            }
        };
        let error = {
            let state = self.state.borrow();
            match state.tasks.get(&main_task) {
                Some(task) if task.record.state_name() == super::StateName::Waiting => {}
                // The root main task is not parked (already resumed/settling): not
                // a deadlock this method can name. Let the caller decide.
                _ => return Ok(false),
            }
            // The root main task parked directly on a `channel/recv`/`channel/send`
            // is a `protocol_waits` Channel entry keyed by its wait key. Name the
            // deadlock with the channel's synchronous message (empty/full),
            // which is the most specific diagnostic available.
            let channel_receive = state
                .tasks
                .get(&main_task)
                .and_then(|task| task.record.wait_key())
                .and_then(|key| match state.protocol_waits.get(&key) {
                    Some(ProtocolWait {
                        kind: ProtocolWaitKind::Channel { receive, .. },
                        ..
                    }) => Some(*receive),
                    _ => None,
                });
            if let Some(receive) = channel_receive {
                if receive {
                    sema_core::SemaError::eval("channel/recv: channel is empty")
                } else {
                    sema_core::SemaError::eval("channel/send: channel is full").with_hint(
                        "Use async to run in an async context where send will yield until space is available",
                    )
                }
            } else {
                // Awaiting a promise (single or set) that can never settle is a
                // genuine cross-task deadlock. Descendant tasks remain parked so
                // a later root may still satisfy their dependency.
                sema_core::SemaError::eval("async scheduler: all tasks blocked (deadlock detected)")
            }
        };
        self.deregister_deadlocked_task_wait(main_task)?;
        self.settle(root, main_task, TaskOutcome::Failed(error))?;
        Ok(true)
    }

    /// Remove every registry edge owned by a deadlocked task before its record is
    /// force-settled and dropped. A later root may wake the dependency, but that
    /// wake must no longer target the removed task.
    fn deregister_deadlocked_task_wait(&self, task_id: TaskId) -> Result<(), RuntimeFault> {
        let key = {
            let state = self.state.borrow();
            state
                .tasks
                .get(&task_id)
                .and_then(|task| task.record.wait_key())
                .ok_or_else(|| RuntimeFault::Invariant {
                    message: "deadlocked task has no wait key".into(),
                })?
        };

        {
            let mut state = self.state.borrow_mut();
            let resource_gate = state.protocol_waits.get(&key).and_then(|wait| {
                if let ProtocolWaitKind::ResourceSlot { gate } = &wait.kind {
                    Some(*gate)
                } else {
                    None
                }
            });
            if let Some(gate) = resource_gate {
                remove_resource_slot_wait(&mut state, task_id, key, gate)?.expect(
                    "selected resource-slot protocol wait remains registered until teardown",
                );
                return Ok(());
            }
            if let Some(wait) = state.protocol_waits.remove(&key) {
                match wait.kind {
                    ProtocolWaitKind::Promises(set) => {
                        for promise in set.promises {
                            let _ = state.promises.cancel_observation(promise, key);
                        }
                        if matches!(set.mode, sema_core::runtime::PromiseSetMode::Timeout(_)) {
                            state.timers.cancel(key);
                        }
                    }
                    ProtocolWaitKind::Timer => {
                        state.timers.cancel(key);
                    }
                    ProtocolWaitKind::Channel { channel, .. } => {
                        let _ = state.channels.cancel_wait(channel, key);
                        let _ = state.channels.take_wake(key);
                    }
                    ProtocolWaitKind::ResourceSlot { .. } => {
                        unreachable!("resource-slot teardown uses its transactional path")
                    }
                    ProtocolWaitKind::OriginBarrier { .. } => {
                        state.origin_barrier_waits = state.origin_barrier_waits.saturating_sub(1);
                    }
                }
                return Ok(());
            }
            if state.timers.cancel(key) {
                return Ok(());
            }
            if !state
                .waits
                .as_ref()
                .is_some_and(|waits| waits.is_active(key))
            {
                return Err(RuntimeFault::Invariant {
                    message: "deadlocked task wait registration disappeared".into(),
                });
            }
        }

        let (mut task, mut waits, now) = {
            let mut state = self.state.borrow_mut();
            let waits = state.waits.take().ok_or_else(|| RuntimeFault::Invariant {
                message: "wait runtime already extracted".into(),
            })?;
            let Some(task) = state.tasks.remove(&task_id) else {
                state.waits = Some(waits);
                return Err(RuntimeFault::Invariant {
                    message: "deadlocked external task disappeared".into(),
                });
            };
            let now = state.clock.now();
            (task, waits, now)
        };
        task.record.request_cancellation(CancelReason::Owner);
        let pending = waits.cancel(&mut task.record, key, now);
        let removed = pending.is_some();
        {
            let mut state = self.state.borrow_mut();
            state.waits = Some(waits);
            state.tasks.insert(task_id, task);
        }
        drop(pending);
        if removed {
            Ok(())
        } else {
            Err(RuntimeFault::Invariant {
                message: "deadlocked external wait could not be deregistered".into(),
            })
        }
    }

    /// Drain and apply commands enqueued via `RuntimeCommandHandle`
    /// (`host_api.rs`) — the runtime's only cross-thread control surface.
    /// Called once at the top of every [`drive`](Self::drive) turn, before
    /// source rotation. Each `RuntimeCommand` is applied by calling the
    /// existing `cancel_root` (same as a host-owned `RootHandle::cancel` or
    /// `debug_cancel_paused`) — this never reaches into `RuntimeState`
    /// fields directly, so cancellation observed here has exactly the same
    /// semantics (C2 eager teardown included) as a same-thread cancel.
    fn apply_pending_commands(&self) {
        let commands = {
            let mut state = self.state.borrow_mut();
            state
                .waits
                .as_mut()
                .map(WaitRuntime::drain_commands)
                .unwrap_or_default()
        };
        for command in commands {
            match command {
                RuntimeCommand::CancelRoot(root) => {
                    self.cancel_root(root, CancelReason::HostStop);
                }
                RuntimeCommand::CancelAll => {
                    let roots: Vec<RootId> = self.state.borrow().roots.keys().copied().collect();
                    for root in roots {
                        self.cancel_root(root, CancelReason::HostStop);
                    }
                }
            }
        }
    }

    /// Cancel a root's main task AND sweep every other live task whose
    /// `origin_root` is this root (CANCEL-ROOT-CASCADE-1).
    ///
    /// The main task alone is not enough: `cancel_descendants` (the
    /// `async/cancel`/`CancelPromise` path) walks the LIVE
    /// `cancellation_parent` chain, which a fire-and-forget descendant falls
    /// out of the moment its spawning task settles and is removed from
    /// `state.tasks` — a grandchild spawned by a task that has already
    /// returned would never be reached and would leak (run to completion /
    /// stay parked forever in a persistent runtime). `origin_root` survives
    /// that removal (it is copied onto every descendant at spawn time,
    /// unlike `cancellation_parent`, which points at a specific, possibly
    /// now-gone, task), so sweeping by it reaches every task under this
    /// root regardless of how deep, or how settled its intermediate
    /// spawners are.
    ///
    /// Returns whether the MAIN task was newly cancelled by this call
    /// (unchanged contract: `false` for an unknown/already-settled root, or
    /// a second call on an already-cancelled root — idempotent).
    pub fn cancel_root(&self, root: RootId, reason: CancelReason) -> bool {
        cancel_origin_root(&self.state, root, reason)
    }

    pub(super) fn abort_terminal_state(&self, fault: &RuntimeFault) {
        abort_runtime_state(&self.state, fault);
    }

    pub fn close_for_interpreter_drop(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.shutting_down = true;
            for task in state.tasks.values_mut() {
                task.record
                    .request_cancellation(CancelReason::InterpreterShutdown);
            }
            let all_task_ids: Vec<TaskId> = state.tasks.keys().copied().collect();
            state.pending_cancel_waits.extend(all_task_ids);
            state.debug_paused = None;
        }
        while matches!(self.cancel_waiting(), Ok(true)) {}
        let (lease, deadline) = {
            let mut state = self.state.borrow_mut();
            let deadline = state.clock.now();
            (
                state.waits.as_mut().and_then(WaitRuntime::take_lease),
                deadline,
            )
        };
        if let Some(lease) = lease {
            lease.shutdown(deadline);
        }
        if let Some(waits) = self.state.borrow_mut().waits.as_mut() {
            waits.close_inbox();
        }
    }

    #[cfg(test)]
    pub(super) fn root_count(&self) -> usize {
        self.state.borrow().roots.len()
    }

    #[cfg(test)]
    pub(super) fn task_count(&self) -> usize {
        self.state.borrow().tasks.len()
    }

    #[cfg(test)]
    pub(super) fn only_task_state_for_test(&self) -> super::StateName {
        let state = self.state.borrow();
        state
            .tasks
            .values()
            .next()
            .expect("one task")
            .record
            .state_name()
    }

    #[cfg(test)]
    pub(super) fn timer_count_for_test(&self) -> usize {
        self.state.borrow().timers.scheduled_len()
    }

    #[cfg(test)]
    pub(super) fn active_wait_count_for_test(&self) -> usize {
        self.state
            .borrow()
            .waits
            .as_ref()
            .map_or(0, WaitRuntime::active_len)
    }

    #[cfg(test)]
    pub(super) fn resource_gate_owner_for_test(&self, gate: ResourceGateId) -> Option<TaskId> {
        self.state.borrow().resource_gates.owner_of(gate)
    }

    #[cfg(test)]
    pub(super) fn resource_gate_count_for_test(&self) -> usize {
        self.resource_gate_count()
    }

    #[cfg(test)]
    pub(super) fn create_resource_gate_handle_for_test(&self) -> ResourceGateHandle {
        let gate = self
            .state
            .borrow_mut()
            .resource_gates
            .allocate()
            .expect("test resource-gate identity");
        self.resource_gate_handle(gate)
    }

    #[cfg(test)]
    pub(super) fn create_resource_gate_for_test(&self) -> ResourceGateId {
        self.state
            .borrow_mut()
            .resource_gates
            .allocate()
            .expect("test resource-gate identity")
    }

    #[cfg(test)]
    pub(super) fn forge_resource_slot_gate_for_test(
        &self,
        task_id: TaskId,
        replacement: ResourceGateId,
    ) {
        let mut state = self.state.borrow_mut();
        let key = state
            .tasks
            .get(&task_id)
            .and_then(|task| task.record.wait_key())
            .expect("resource-slot task is waiting");
        let wait = state
            .protocol_waits
            .get_mut(&key)
            .expect("resource-slot protocol wait exists");
        let ProtocolWaitKind::ResourceSlot { gate } = &mut wait.kind else {
            panic!("task is waiting on a resource slot")
        };
        *gate = replacement;
    }

    #[cfg(test)]
    pub(super) fn active_wait_key_for_test(&self) -> super::WaitKey {
        self.state
            .borrow()
            .waits
            .as_ref()
            .expect("wait runtime")
            .first_active_key_for_test()
    }

    #[cfg(test)]
    pub(super) fn late_completion_count_for_test(&self) -> usize {
        self.state
            .borrow()
            .waits
            .as_ref()
            .map_or(0, WaitRuntime::late_completions)
    }

    #[cfg(test)]
    pub(super) fn cleanup_count_for_test(&self) -> usize {
        self.state
            .borrow()
            .waits
            .as_ref()
            .map_or(0, WaitRuntime::cleanup_len)
    }

    #[cfg(test)]
    pub(super) fn quarantine_reaped_count_for_test(&self) -> usize {
        self.state
            .borrow()
            .waits
            .as_ref()
            .map_or(0, WaitRuntime::quarantine_reaped)
    }

    #[cfg(test)]
    pub(super) fn cleanup_diagnostics_for_test(&self) -> Vec<super::CleanupDiagnostic> {
        let state = self.state.borrow();
        state.waits.as_ref().map_or_else(Vec::new, |waits| {
            waits.cleanup_diagnostics_at(state.clock.now())
        })
    }

    #[cfg(test)]
    pub(super) fn retain_descendant_for_test(&self, root: RootId) {
        assert!(self
            .state
            .borrow_mut()
            .roots
            .get_mut(&root)
            .expect("root")
            .retain_descendant());
    }

    #[cfg(test)]
    pub(super) fn release_descendant_for_test(&self, root: RootId) {
        let mut state = self.state.borrow_mut();
        let record = state.roots.get_mut(&root).expect("root");
        record.release_descendant();
        if record.is_reap_eligible() {
            state.handle_cleanup.push_back(root);
        }
    }

    #[cfg(test)]
    pub(super) fn force_settlement_exhaustion_for_test(&self) {
        self.state.borrow_mut().force_settlement_exhaustion = true;
    }

    #[cfg(test)]
    pub(super) fn force_registry_exhaustion_for_test(&self, kind: &'static str) {
        let mut state = self.state.borrow_mut();
        match kind {
            "promise" => state.force_promise_exhaustion = true,
            "channel" => state.force_channel_exhaustion = true,
            _ => panic!("unknown registry allocator: {kind}"),
        }
    }

    #[cfg(test)]
    pub(super) fn registry_counts_for_test(&self) -> (usize, usize) {
        let state = self.state.borrow();
        (state.promises.len(), state.channels.len())
    }

    #[cfg(test)]
    pub(super) fn protocol_wait_count_for_test(&self) -> usize {
        self.state.borrow().protocol_waits.len()
    }

    #[cfg(test)]
    pub(super) fn origin_barrier_wait_count_for_test(&self) -> usize {
        self.state.borrow().origin_barrier_waits
    }

    #[cfg(test)]
    pub(super) fn channel_receiver_queue_len_for_test(
        &self,
        channel: sema_core::runtime::ChannelId,
    ) -> usize {
        self.state.borrow().channels.receiver_queue_len(channel)
    }

    #[cfg(test)]
    pub(super) fn dropped_protocol_completions_for_test(&self) -> usize {
        self.state.borrow().dropped_protocol_completions
    }

    #[cfg(test)]
    pub(super) fn force_timer_failure_for_test(&self, kind: &str) {
        let mut state = self.state.borrow_mut();
        match kind {
            "sequence" => state.timers.force_sequence_exhaustion_for_test(),
            "duplicate" => state.timers.force_duplicate_for_test(),
            _ => panic!("unknown timer failure kind: {kind}"),
        }
    }

    #[cfg(test)]
    pub(super) fn force_admission_exhaustion_for_test(&self, kind: &str) {
        let mut state = self.state.borrow_mut();
        match kind {
            "root" => state.force_root_exhaustion = true,
            "task" => state.force_task_exhaustion = true,
            _ => panic!("unknown admission identity kind: {kind}"),
        }
    }

    #[cfg(test)]
    pub(super) fn force_completion_identity_exhaustion_for_test(&self, kind: &str) {
        self.state
            .borrow_mut()
            .waits
            .as_mut()
            .expect("wait runtime")
            .force_identity_exhaustion_for_test(kind);
    }

    #[cfg(test)]
    pub(super) fn forge_completion_for_test(
        &self,
        mutation: super::wait::ForgedCompletionMutation,
        result: Result<sema_core::runtime::SendPayload, ExternalFailure>,
    ) {
        let mut state = self.state.borrow_mut();
        let waits = state.waits.as_mut().expect("wait runtime");
        let key = waits.first_active_key_for_test();
        waits.forge_active_completion_for_test(key, mutation, result);
    }

    #[cfg(test)]
    pub(super) fn abort_terminal_for_test(&self) {
        self.abort_terminal_state(&RuntimeFault::Invariant {
            message: "test terminal abort".into(),
        });
    }

    #[cfg(test)]
    pub(super) fn clone_for_test(&self) -> Self {
        Self {
            runtime_id: self.runtime_id,
            state: Rc::clone(&self.state),
        }
    }
}

/// Canonical host cancellation path for a root and every live task carrying its
/// origin identity. Both [`Runtime::cancel_root`] and `RootHandle::cancel` route
/// here so descendant reachability, eager wait teardown, and debug barriers have
/// one implementation.
pub(super) fn cancel_origin_root(
    cell: &Rc<RefCell<RuntimeState>>,
    root: RootId,
    reason: CancelReason,
) -> bool {
    let (main_newly, targets) = {
        let mut state = cell.borrow_mut();
        if root.runtime()
            != state
                .waits
                .as_ref()
                .expect("runtime wait owner is installed")
                .runtime_id()
            || !state.roots.contains_key(&root)
        {
            return false;
        }
        let main_task = match state.roots.get(&root).map(RootRecord::state) {
            Some(RootState::Running { main_task }) => *main_task,
            _ => return false,
        };
        if state.tasks.get(&main_task).is_none() {
            return false;
        }
        let mut targets = Vec::new();
        let main_newly = {
            let task = state.tasks.get_mut(&main_task).expect("main task present");
            task.record.request_cancellation(reason)
        };
        if main_newly {
            state.pending_cancel_waits.push_back(main_task);
            targets.push(main_task);
        }
        let descendants: Vec<TaskId> = state
            .tasks
            .iter()
            .filter_map(|(id, task)| {
                (*id != main_task && task.record.relations().origin_root == root).then_some(*id)
            })
            .collect();
        for id in descendants {
            if let Some(task) = state.tasks.get_mut(&id) {
                if task.record.request_cancellation(CancelReason::Owner) {
                    state.pending_cancel_waits.push_back(id);
                    targets.push(id);
                }
            }
        }
        if state
            .debug_paused
            .as_ref()
            .is_some_and(|(paused_root, _, _)| *paused_root == root)
        {
            let (_, paused_task, _) = state
                .debug_paused
                .take()
                .expect("matching debug barrier exists");
            state.ready.enqueue(root, paused_task);
        }
        (main_newly, targets)
    };
    for target in targets {
        if let Err(fault) = deliver_cancel_teardown(cell, target) {
            abort_runtime_state(cell, &fault);
            break;
        }
    }
    main_newly
}

fn abort_runtime_state(cell: &RefCell<RuntimeState>, fault: &RuntimeFault) {
    let (pending, protocol_waits, tasks) = {
        let mut state = cell.borrow_mut();
        state.terminal_fault = Some(fault.clone());
        let pending = std::mem::take(&mut state.pending);
        state.ready = ReadyScheduler::new();
        let mut newly_eligible = Vec::new();
        for (id, root) in &mut state.roots {
            if root.abort_running() && root.is_reap_eligible() {
                newly_eligible.push(*id);
            }
        }
        state.handle_cleanup.extend(newly_eligible);
        state.origin_barrier_waits = 0;
        (
            pending,
            std::mem::take(&mut state.protocol_waits),
            std::mem::take(&mut state.tasks),
        )
    };
    drop(pending);
    drop(protocol_waits);
    drop(tasks);
}

/// Transitively request cancellation of every live task whose cancellation-parent
/// chain leads back to `parent` (its spawned descendants), marking each with
/// `CancelReason::Owner`. Returns the task ids that were NEWLY cancelled by this
/// call so the caller can deliver eager wait teardown (DECISION C2) to each —
/// aborting a descendant parked on an External/IO wait (killing its in-flight
/// subprocess/request) rather than waiting for the next drive scan. `parent`
/// itself is cancelled by the caller; this only reaches its children. BFS over an
/// immutable snapshot of the parent→child edges, so it terminates even if a
/// (malformed) cycle existed: a task already marked cancelled is not revisited.
fn cancel_descendants(state: &mut RuntimeState, parent: TaskId) -> Vec<TaskId> {
    let mut newly = Vec::new();
    let mut frontier = vec![parent];
    while let Some(current) = frontier.pop() {
        let children: Vec<TaskId> = state
            .tasks
            .iter()
            .filter_map(|(id, task)| {
                (task.record.relations().cancellation_parent == CancellationParent::Task(current))
                    .then_some(*id)
            })
            .collect();
        for child in children {
            if let Some(task) = state.tasks.get_mut(&child) {
                // Only descend into a child we actually newly-cancelled; a child
                // already cancelled was already walked (or is being walked),
                // which bounds the traversal even under a malformed cycle.
                if task.record.request_cancellation(CancelReason::Owner) {
                    newly.push(child);
                    frontier.push(child);
                    state.pending_cancel_waits.push_back(child);
                }
            }
        }
    }
    newly
}

/// Which eager wait teardown a cancelled task needs (DECISION C2). Only the
/// in-flight kinds — External / ResourceSlot, plus a granted-but-not-run
/// resource gate — are delivered eagerly; Promise / bare Timer / Channel waits
/// carry no offloaded work to abort and are left to the per-drive-turn
/// `cancel_waiting` scan.
enum EagerTeardown {
    ResourceSlot(super::WaitKey, ResourceGateId),
    External,
    GrantedGate,
    None,
}

/// Deliver wait teardown for a task at cancellation-REQUEST time (DECISION C2).
///
/// When a cancellation has just been recorded on a task parked on an External /
/// ResourceSlot wait (or holding a granted-but-not-run resource gate), tear the
/// wait down SYNCHRONOUSLY right now rather than waiting for the per-drive-turn
/// `cancel_waiting` scan — so a settled root followed by process exit never
/// leaves an in-flight subprocess/request/gate un-aborted
/// (ASYNC-TIMEOUT-CANCEL-1).
///
/// Exactly-once: the wait registration (`protocol_waits` / `WaitRuntime::active`
/// / the gate queue-or-ownership) is removed HERE, so the drive-scan
/// `cancel_waiting` then finds the task no longer Waiting and has nothing to
/// double-abort. Returns whether teardown ran.
pub(super) fn deliver_cancel_teardown(
    cell: &RefCell<RuntimeState>,
    task_id: TaskId,
) -> Result<bool, RuntimeFault> {
    let kind = {
        let state = cell.borrow();
        let Some(task) = state.tasks.get(&task_id) else {
            return Ok(false);
        };
        if task.record.cancellation().is_none() {
            return Ok(false);
        }
        match task.record.state_name() {
            super::StateName::Waiting => {
                let key = task.record.wait_key().expect("waiting task has a wait key");
                match state.protocol_waits.get(&key).map(|wait| &wait.kind) {
                    Some(ProtocolWaitKind::ResourceSlot { gate }) => {
                        EagerTeardown::ResourceSlot(key, *gate)
                    }
                    // Promise / bare Timer / Channel waits: nothing to abort.
                    Some(_) => EagerTeardown::None,
                    None if state
                        .waits
                        .as_ref()
                        .is_some_and(|waits| waits.is_active(key)) =>
                    {
                        EagerTeardown::External
                    }
                    // A bare `Timer` key (in `timers` only, no protocol-wait
                    // entry): self-resolving, left to the scan.
                    None => EagerTeardown::None,
                }
            }
            // Granted-but-not-run: the task was handed a gate slot (it is the gate
            // owner) but its acquire continuation has not run — that continuation
            // raises on the cancellation WITHOUT releasing, so release for it.
            super::StateName::Running if state.resource_gates.owner_gate(task_id).is_some() => {
                EagerTeardown::GrantedGate
            }
            _ => EagerTeardown::None,
        }
    };
    match kind {
        EagerTeardown::None => Ok(false),
        EagerTeardown::ResourceSlot(key, gate) => {
            eager_resource_slot_teardown(cell, task_id, key, gate)
        }
        EagerTeardown::External => eager_external_teardown(cell, task_id),
        EagerTeardown::GrantedGate => eager_release_granted_gate(cell, task_id),
    }
}

/// Eager teardown of a task parked on a `ResourceSlot` wait. If it is still
/// queued behind a busy gate, drop it from the FIFO queue; if it was already
/// GRANTED the slot (window A — the gate owner, not in the queue), release the
/// gate so the next acquirer proceeds. Either way its acquire continuation is
/// resumed with the cancellation, which raises without further gate work.
fn eager_resource_slot_teardown(
    cell: &RefCell<RuntimeState>,
    task_id: TaskId,
    key: super::WaitKey,
    gate: ResourceGateId,
) -> Result<bool, RuntimeFault> {
    let mut state = cell.borrow_mut();
    let Some(wait) = remove_resource_slot_wait(&mut state, task_id, key, gate)? else {
        return Ok(false);
    };
    state
        .tasks
        .get_mut(&task_id)
        .expect("resource slot task exists")
        .record
        .reject_wait(key)
        .map_err(|error| RuntimeFault::Invariant {
            message: format!("cancelled resource slot task failed to resume: {error:?}"),
        })?;
    state.pending.push_back(PendingStage::ApplyRuntimeResponse(
        task_id,
        wait.owner,
        wait.continuation,
        Err(sema_core::SemaError::eval("protocol wait cancelled")),
    ));
    Ok(true)
}

/// Detach a resource-slot protocol wait only after its gate-side state has a
/// complete teardown disposition. A queued waiter is removed from the FIFO; a
/// waiter with committed ownership has that slot released; a gate removed by
/// `close` is accepted only while its exact `Closed` wake is still pending.
/// Registry identity errors and unwitnessed absence leave the protocol entry
/// intact so the caller can surface the invariant without stranding the task.
fn remove_resource_slot_wait(
    state: &mut RuntimeState,
    task_id: TaskId,
    key: super::WaitKey,
    gate: ResourceGateId,
) -> Result<Option<ProtocolWait>, RuntimeFault> {
    let Some(wait) = state.protocol_waits.get(&key) else {
        return Ok(None);
    };
    if wait.task != task_id
        || !matches!(wait.kind, ProtocolWaitKind::ResourceSlot { gate: wait_gate } if wait_gate == gate)
        || state
            .tasks
            .get(&task_id)
            .and_then(|task| task.record.wait_key())
            != Some(key)
    {
        return Err(RuntimeFault::Invariant {
            message: format!(
                "resource slot teardown ownership mismatch for task {task_id:?}, wait {key:?}, gate {gate:?}"
            ),
        });
    }

    let closed_wake_pending = state.pending.iter().any(|stage| {
        matches!(
            stage,
            PendingStage::ResourceGateWake(wake)
                if wake.key == key
                    && wake.task == task_id
                    && wake.result == GateResult::Closed
        )
    });
    if !closed_wake_pending {
        match state.resource_gates.owner_gate(task_id) {
            Some(owner_gate) if owner_gate == gate => {
                // A grant was committed but its wake has not run. Releasing the
                // live owner is infallible after the ownership check; any staged
                // grant wake remains queued and becomes a harmless late no-op.
                release_owned_gate(state, gate)?;
            }
            Some(owner_gate) => {
                return Err(RuntimeFault::Invariant {
                    message: format!(
                        "resource slot task {task_id:?} owns {owner_gate:?}, not registered gate {gate:?}"
                    ),
                });
            }
            None => match state.resource_gates.cancel_wait(gate, key) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(RuntimeFault::Invariant {
                        message: format!(
                            "resource slot wait {key:?} is neither queued nor owned for gate {gate:?}"
                        ),
                    });
                }
                Err(RegistryError::Unknown) => {
                    return Err(RuntimeFault::Invariant {
                        message: format!(
                            "resource slot gate {gate:?} disappeared without a pending Closed wake for task {task_id:?}, wait {key:?}"
                        ),
                    });
                }
                Err(error) => {
                    return Err(RuntimeFault::Invariant {
                        message: format!("resource slot teardown failed: {error:?}"),
                    });
                }
            },
        }
    }

    Ok(state.protocol_waits.remove(&key))
}

/// Eager teardown of a task parked on an External wait: run the executor cancel
/// path / resource abort hook via `WaitRuntime::cancel` and arm the cancelled
/// resume. The hook is invoked with NO state borrow held (matching the drive-scan
/// path), so a hook that re-enters the runtime (e.g. polls its root handle) does
/// not deadlock the state cell.
fn eager_external_teardown(
    cell: &RefCell<RuntimeState>,
    task_id: TaskId,
) -> Result<bool, RuntimeFault> {
    let (mut task, mut waits, now) = {
        let mut state = cell.borrow_mut();
        let task = state
            .tasks
            .remove(&task_id)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "cancelled external task disappeared".into(),
            })?;
        let waits = state.waits.take().ok_or_else(|| RuntimeFault::Invariant {
            message: "wait runtime already extracted".into(),
        })?;
        let now = state.clock.now();
        (task, waits, now)
    };
    let key = task
        .record
        .wait_key()
        .expect("external-waiting task has a wait key");
    let pending = waits.cancel(&mut task.record, key, now);
    let root = task.record.relations().origin_root;
    let mut state = cell.borrow_mut();
    state.waits = Some(waits);
    state.tasks.insert(task_id, task);
    if let Some(pending) = pending {
        state
            .tasks
            .get_mut(&task_id)
            .expect("external task restored")
            .pending_resume = Some(pending);
        state.ready.enqueue(root, task_id);
        return Ok(true);
    }
    Ok(false)
}

/// Eager release of a gate whose slot a Running task HOLDS after being granted it
/// but before running its acquire continuation (windows B/C). The task's pending
/// grant resume will resolve to the cancellation and raise without releasing, so
/// the runtime releases here to keep the gate from leaking.
fn eager_release_granted_gate(
    cell: &RefCell<RuntimeState>,
    task_id: TaskId,
) -> Result<bool, RuntimeFault> {
    let mut state = cell.borrow_mut();
    let Some(gate) = state.resource_gates.owner_gate(task_id) else {
        return Ok(false);
    };
    release_owned_gate(&mut state, gate)?;
    Ok(true)
}

/// Release a gate the runtime holds on behalf of a cancelled owner, forwarding
/// any resulting grant wake (transfer to the FIFO head) into the pending queue.
fn release_owned_gate(state: &mut RuntimeState, gate: ResourceGateId) -> Result<(), RuntimeFault> {
    state
        .resource_gates
        .release(gate)
        .map_err(|error| RuntimeFault::Invariant {
            message: format!("resource gate release failed: {error:?}"),
        })?;
    while let Some(wake) = state.resource_gates.pop_wake() {
        state
            .pending
            .push_back(PendingStage::ResourceGateWake(wake));
    }
    Ok(())
}

/// The structured `:timeout` condition raised into a continuation parked on an
/// observational `Timeout` promise-set wait whose deadline elapsed first. It
/// carries the stable `{:type :timeout :duration-ms …}` fields the plan mandates.
fn timeout_expired_condition(duration: Duration) -> sema_core::SemaError {
    let duration_ms = duration.as_millis().min(u64::MAX as u128) as u64;
    sema_core::SemaError::timeout_condition(
        &format!("async/timeout: operation timed out after {duration_ms} ms"),
        "async/timeout",
        duration_ms,
        None,
    )
}

fn registry_error(error: super::RegistryError) -> sema_core::SemaError {
    sema_core::SemaError::eval(match error {
        super::RegistryError::WrongRuntime => "runtime handle belongs to another runtime",
        super::RegistryError::Unknown => "runtime handle is stale or unknown",
        super::RegistryError::AlreadySettled => "promise is already settled",
        super::RegistryError::DuplicateWait => "runtime wait is already registered",
        super::RegistryError::IdExhausted => "runtime identity exhausted",
    })
}

type ProtocolInstallError = Box<(ReturnOwner, ContinuationFrame, sema_core::SemaError)>;

/// `install_channel_wait`'s error channel: it can fail exactly like the other
/// `install_*_wait` helpers (`ProtocolInstallError`, delivered to the caller's
/// continuation as a catchable Sema-level error) OR, on its immediate-match
/// fast path, hit one of the same "task disappeared" invariant faults that
/// `resume_continuation`/`finish_protocol_wait` guard against elsewhere. Those
/// two error classes are not interchangeable — a `RuntimeFault` is an
/// unrecoverable runtime-invariant violation, never a per-task catchable
/// error — so this carries both variants distinctly. `From<RuntimeFault>` lets
/// the fast path use `?` for the fault variant.
enum ChannelWaitError {
    Protocol(ProtocolInstallError),
    Fault(RuntimeFault),
}

impl From<RuntimeFault> for ChannelWaitError {
    fn from(fault: RuntimeFault) -> Self {
        Self::Fault(fault)
    }
}

/// Compute the `RuntimeResponse` a channel wake resumes its waiter with.
/// Shared by the staged path (`consume_channel_wake`, reached via a queued
/// `ChannelWake`/`ChannelClose` pending stage) and `install_channel_wait`'s
/// immediate-match fast path (Task D), which resumes the matched peer inline
/// instead of queuing its wake. `None` for `ChannelResult::Waiting`, which
/// `ChannelWake` never actually carries (every wake is pushed with a settled
/// result) — kept as a defensive no-op mirroring the staged path's own arm.
fn channel_wake_response(
    protocol_waits: &HashMap<super::WaitKey, ProtocolWait>,
    key: super::WaitKey,
    result: super::ChannelResult,
) -> Option<ChannelResponse> {
    Some(Ok(match result {
        super::ChannelResult::Sent => RuntimeResponse::Send(sema_core::runtime::ChannelSend::Sent),
        super::ChannelResult::Received(value) => {
            RuntimeResponse::Receive(sema_core::runtime::ChannelReceive::Received(value))
        }
        super::ChannelResult::Closed => {
            let receive = protocol_waits.get(&key).is_some_and(|wait| {
                matches!(wait.kind, ProtocolWaitKind::Channel { receive: true, .. })
            });
            if receive {
                RuntimeResponse::Receive(sema_core::runtime::ChannelReceive::Closed)
            } else {
                RuntimeResponse::Send(sema_core::runtime::ChannelSend::Closed)
            }
        }
        super::ChannelResult::Waiting => return None,
    }))
}

/// Core of `Runtime::reinstall_parent_vm`, factored out for reuse by the
/// ordinary `&self` method and `install_channel_wait_and_resume`'s inline fast
/// path.
fn reinstall_parent_vm_now(
    state: &mut RuntimeState,
    task_id: TaskId,
    vm: VM,
    parent: ReturnOwner,
    resume: VmResume,
) -> Result<(), RuntimeFault> {
    let task = state
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| RuntimeFault::Invariant {
            message: "parent VM task disappeared before reinstall".into(),
        })?;
    task.vm_call = Some(vm);
    task.vm_owner = Some(parent);
    task.vm_resume = Some(resume);
    task.record
        .yield_ready()
        .map_err(|error| RuntimeFault::Invariant {
            message: format!("reinstalled parent VM failed to yield ready: {error:?}"),
        })?;
    let root = task.record.relations().origin_root;
    state.ready.enqueue(root, task_id);
    Ok(())
}

/// Apply a resumed continuation's `NativeResult` inline, mirroring
/// `Runtime::apply_native_result`'s two hot branches — a parked parent VM
/// (`ReturnOwner::VmResume`) resuming with a plain `Return`/error, the shape a
/// channel op's continuation produces — without the `PendingStage::Apply`
/// round-trip. Anything else (chained `ReturnOwner::Continuation` composition,
/// or a resumed continuation that itself yields `NativeOutcome::Call`/`Suspend`)
/// falls back to queuing exactly the `PendingStage::Apply` the staged path
/// would have produced, so `advance_pending` replays it through the unmodified
/// `apply_native_result`/`apply_native_outcome` — single-step inline, no
/// unbounded in-place looping. Returns `true` when it resolved fully inline
/// (the caller owes one fewer collapsed-hop credit).
fn apply_native_result_now(
    state: &mut RuntimeState,
    task_id: TaskId,
    owner: ReturnOwner,
    result: NativeResult,
) -> Result<bool, RuntimeFault> {
    match (owner, result) {
        (ReturnOwner::VmResume { vm, parent }, Ok(NativeOutcome::Return(value))) => {
            reinstall_parent_vm_now(state, task_id, *vm, *parent, VmResume::Value(value))?;
            Ok(true)
        }
        (ReturnOwner::VmResume { vm, parent }, Err(error)) => {
            reinstall_parent_vm_now(state, task_id, *vm, *parent, VmResume::Fail(error))?;
            Ok(true)
        }
        (owner, result) => {
            state
                .pending
                .push_back(PendingStage::Apply(task_id, owner, result));
            Ok(false)
        }
    }
}

/// A channel/protocol response still awaiting delivery to a continuation.
type ChannelResponse = Result<RuntimeResponse, sema_core::SemaError>;

/// What `finish_protocol_wait_now` hands back for the caller to resume: the
/// wait's owner/continuation plus the response to resume it with.
type ProtocolWaitResume = (ReturnOwner, ContinuationFrame, ChannelResponse);

/// Core of `Runtime::finish_protocol_wait`, factored out for `install_channel_wait`'s
/// inline fast path. Returns `Some((owner, frame, response))` when the wait was
/// genuinely live and owned by `task_id` (mirroring the staged path's single
/// `PendingStage::ApplyRuntimeResponse` push); `None` for the two silent-drop
/// cases the staged path also has (wait already gone, or owned by a different
/// task) — the `dropped_protocol_completions` bookkeeping is preserved.
fn finish_protocol_wait_now(
    state: &mut RuntimeState,
    key: super::WaitKey,
    task_id: TaskId,
    response: ChannelResponse,
) -> Result<Option<ProtocolWaitResume>, RuntimeFault> {
    let Some(wait) = state.protocol_waits.remove(&key) else {
        // The wait is gone (e.g. the task was cancelled). A rendezvous value
        // that arrives here would be silently lost, so record it.
        if matches!(
            &response,
            Ok(RuntimeResponse::Receive(
                sema_core::runtime::ChannelReceive::Received(_)
            ))
        ) {
            state.dropped_protocol_completions += 1;
        }
        return Ok(None);
    };
    if wait.task != task_id {
        state.protocol_waits.insert(key, wait);
        return Ok(None);
    }
    if let ProtocolWaitKind::Promises(set) = &wait.kind {
        for promise in &set.promises {
            let _ = state.promises.cancel_observation(*promise, key);
        }
        // A `Timeout` armed a deadline timer under this key; the observed
        // promise won, so deregister the timer before it fires stale.
        if matches!(set.mode, sema_core::runtime::PromiseSetMode::Timeout(_)) {
            state.timers.cancel(key);
        }
    }
    let task = state
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| RuntimeFault::Invariant {
            message: "protocol wake task disappeared".into(),
        })?;
    task.record
        .reject_wait(key)
        .map_err(|error| RuntimeFault::Invariant {
            message: format!("protocol wake transition failed: {error:?}"),
        })?;
    Ok(Some((wait.owner, wait.continuation, response)))
}

/// Resume one settled channel waiter (self or the matched peer) inline within
/// `install_channel_wait`'s immediate-match fast path, crediting
/// `channel_fast_path_credit` for every pending-stage hop this replaces so
/// `drive()` debits `work_items` honestly for the collapsed work.
///
/// One waiter (self or the matched peer) whose continuation
/// `install_channel_wait_and_resume` still owes a resume+apply pass. Carried
/// OUT of `install_channel_wait` rather than resumed inline there: that
/// function runs under a live `RuntimeState` borrow (registry/task-record
/// bookkeeping needs it), and `ContinuationFrame::resume` must never run
/// under one — it can invoke arbitrary Sema-level evaluation, and a
/// collection pass it triggers needs to trace the SAME `RuntimeState` (e.g.
/// `interior_trace_channel` tracing channel buffers). A collection that finds
/// the state already borrowed just `try_borrow()`s, fails, and silently skips
/// tracing those roots this pass — not a soundness bug, but it starves the
/// collector exactly like `cyclic_data_churn_collected_mid_eval` catches.
/// `install_channel_wait_and_resume` drops its borrow before resuming any of
/// these, matching `resume_continuation`'s own long-standing discipline of
/// never holding `self.state` across a `frame.resume` call.
struct ChannelRendezvousResume {
    task_id: TaskId,
    owner: ReturnOwner,
    frame: ContinuationFrame,
    response: ChannelResponse,
}

/// `install_channel_wait`'s result: either the task parked (unchanged from
/// the staged path), or its op matched immediately and one or two waiters
/// (this task, and the peer it matched with, if any) are ready to resume —
/// see `ChannelRendezvousResume` for why that resume happens outside this
/// function. `peer` is boxed so the common `Parked`/no-peer variants stay
/// cheap to move — a matched rendezvous is the less frequent shape.
enum ChannelWaitOutcome {
    Parked,
    Matched {
        this: ChannelRendezvousResume,
        peer: Option<Box<ChannelRendezvousResume>>,
    },
}

/// `try_channel_handoff`'s result: `this` task's own resume, or a reason it
/// could not be applied in place.
enum ChannelHandoffOutcome {
    /// The op resolved immediately and the resumed continuation settled as a
    /// plain value/error — apply directly to the still-unboxed `vm`'s stack
    /// via `apply_vm_resume` and loop.
    Applied(VmResume),
    /// The op resolved immediately, but the resumed continuation composed
    /// further (a chained `Call`/`Suspend`/`Runtime`, not a plain value/
    /// error) — the same rare case `apply_native_result_now`'s fallback arm
    /// handles. The caller boxes `vm` and settles via the ordinary
    /// `TaskAction::VmResult` mapping.
    Deferred(NativeResult),
    /// Wait-key identity was exhausted before any registry mutation
    /// happened — hands the wait/continuation back so the caller can fall
    /// through to the ordinary (staged) path, which hits and handles the
    /// identical exhaustion.
    GiveUp(
        sema_core::runtime::ChannelWait,
        Box<dyn sema_core::runtime::NativeContinuation>,
    ),
}

fn promise_set_response(
    promises: &PromiseRegistry,
    wait: &sema_core::runtime::PromiseSetWait,
) -> Result<Option<RuntimeResponse>, RuntimeFault> {
    let mut settled = Vec::with_capacity(wait.promises.len());
    let mut fail_fast = Vec::new();
    for promise in &wait.promises {
        match promises
            .state(*promise)
            .map_err(|error| RuntimeFault::Invariant {
                message: format!("registered promise wait became invalid: {error:?}"),
            })? {
            PromiseState::Pending => settled.push(None),
            PromiseState::Returned(value) => settled.push(Some(value)),
            PromiseState::Failed(value) | PromiseState::Cancelled(value) => {
                fail_fast.push(Rc::clone(&value));
                settled.push(Some(value));
            }
        }
    }
    Ok(match wait.mode {
        // A `Timeout` observes like a single-winner race: the lowest-sequence
        // settled promise wins (deterministically, even at `ms == 0`); the
        // deadline itself is delivered by the timer path, not here.
        sema_core::runtime::PromiseSetMode::Race
        | sema_core::runtime::PromiseSetMode::Timeout(_) => settled
            .into_iter()
            .flatten()
            .min_by_key(|settlement| settlement.sequence)
            .map(|settlement| RuntimeResponse::Settlement(Some(settlement))),
        sema_core::runtime::PromiseSetMode::All if !fail_fast.is_empty() => fail_fast
            .into_iter()
            .min_by_key(|settlement| settlement.sequence)
            .map(|settlement| RuntimeResponse::Settlement(Some(settlement))),
        sema_core::runtime::PromiseSetMode::All if settled.iter().all(Option::is_some) => Some(
            RuntimeResponse::Settlements(settled.into_iter().flatten().collect()),
        ),
        sema_core::runtime::PromiseSetMode::All => None,
    })
}

fn install_promise_wait(
    state: &mut RuntimeState,
    task_id: TaskId,
    key: super::WaitKey,
    wait: sema_core::runtime::PromiseSetWait,
    owner: ReturnOwner,
    frame: ContinuationFrame,
) -> Result<(), ProtocolInstallError> {
    if wait.promises.is_empty() && !matches!(wait.mode, sema_core::runtime::PromiseSetMode::All) {
        return Err(Box::new((
            owner,
            frame,
            sema_core::SemaError::eval("promise race requires at least one promise"),
        )));
    }
    let response = match promise_set_response(&state.promises, &wait) {
        Ok(response) => response,
        Err(fault) => {
            return Err(Box::new((
                owner,
                frame,
                sema_core::SemaError::eval(format!("{fault:?}")),
            )))
        }
    };
    if let Some(response) = response {
        state.pending.push_back(PendingStage::ApplyRuntimeResponse(
            task_id,
            owner,
            frame,
            Ok(response),
        ));
        return Ok(());
    }
    let mut observed = Vec::new();
    let mut unique = hashbrown::HashSet::new();
    for promise in &wait.promises {
        if !unique.insert(*promise) {
            continue;
        }
        match state.promises.observe(*promise, key, task_id) {
            Ok(true) => observed.push(*promise),
            Ok(false) => {}
            Err(error) => {
                for promise in observed {
                    let _ = state.promises.cancel_observation(promise, key);
                }
                return Err(Box::new((owner, frame, registry_error(error))));
            }
        }
    }
    // A `Timeout` arms a deadline timer under the SAME wait key: whichever of the
    // observed promise or the deadline fires first wakes the one task and
    // deregisters the other (`finish_protocol_wait`/`fire_timer`).
    if let sema_core::runtime::PromiseSetMode::Timeout(duration) = wait.mode {
        let deadline = state.clock.now() + duration;
        if !state.timers.insert(deadline, key) {
            for promise in observed {
                let _ = state.promises.cancel_observation(promise, key);
            }
            return Err(Box::new((
                owner,
                frame,
                sema_core::SemaError::eval("runtime timer identity exhausted"),
            )));
        }
    }
    if let Err(error) = state
        .tasks
        .get_mut(&task_id)
        .expect("protocol task exists")
        .record
        .wait(key)
    {
        if matches!(wait.mode, sema_core::runtime::PromiseSetMode::Timeout(_)) {
            state.timers.cancel(key);
        }
        for promise in observed {
            let _ = state.promises.cancel_observation(promise, key);
        }
        return Err(Box::new((
            owner,
            frame,
            sema_core::SemaError::eval(format!("promise wait transition failed: {error:?}")),
        )));
    }
    state.protocol_waits.insert(
        key,
        ProtocolWait {
            task: task_id,
            kind: ProtocolWaitKind::Promises(wait),
            owner,
            continuation: frame,
        },
    );
    Ok(())
}

/// Register a `Timer(duration)` suspension: arm the task's wait key in the timer
/// queue and stash its continuation so `fire_timer` resumes it with
/// `Returned(nil)` once the deadline elapses.
fn install_timer_wait(
    state: &mut RuntimeState,
    task_id: TaskId,
    key: super::WaitKey,
    duration: Duration,
    owner: ReturnOwner,
    frame: ContinuationFrame,
) -> Result<(), ProtocolInstallError> {
    let deadline = state.clock.now() + duration;
    if !state.timers.insert(deadline, key) {
        return Err(Box::new((
            owner,
            frame,
            sema_core::SemaError::eval("runtime timer identity exhausted"),
        )));
    }
    if let Err(error) = state
        .tasks
        .get_mut(&task_id)
        .expect("protocol task exists")
        .record
        .wait(key)
    {
        state.timers.cancel(key);
        return Err(Box::new((
            owner,
            frame,
            sema_core::SemaError::eval(format!("timer wait transition failed: {error:?}")),
        )));
    }
    state.protocol_waits.insert(
        key,
        ProtocolWait {
            task: task_id,
            kind: ProtocolWaitKind::Timer,
            owner,
            continuation: frame,
        },
    );
    Ok(())
}

/// Whether an `async/run` barrier for `caller` (whose origin root is `root` and
/// may release: true when no OTHER task under `root` is Ready, Running, or
/// parked on a SELF-RESOLVING wait. See [`Runtime::resolve_origin_barriers`] for
/// the full classification and rationale (the Reviewer-2 hole: `ResourceSlot`
/// MUST be cycle-forming, or the barrier hangs on a resource cycle).
fn origin_barrier_released(state: &RuntimeState, root: RootId, caller: TaskId) -> bool {
    !state.tasks.iter().any(|(id, task)| {
        if *id == caller {
            return false;
        }
        if task.record.relations().origin_root != root {
            return false;
        }
        match task.record.state_name() {
            // Still-live: a runnable sibling can spawn/settle more work.
            super::StateName::Ready | super::StateName::Running => true,
            // Done: no longer part of the residual graph.
            super::StateName::Settled => false,
            super::StateName::Waiting => {
                let Some(key) = task.record.wait_key() else {
                    // A Waiting task always carries a key; treat a missing one as
                    // still-live rather than silently releasing early.
                    return true;
                };
                match state.protocol_waits.get(&key).map(|wait| &wait.kind) {
                    // Self-resolving — the barrier WAITS on these.
                    Some(ProtocolWaitKind::Timer) => true,
                    Some(ProtocolWaitKind::Promises(set)) => {
                        matches!(set.mode, sema_core::runtime::PromiseSetMode::Timeout(_))
                    }
                    // Cycle-forming — the barrier does NOT wait on these.
                    Some(ProtocolWaitKind::Channel { .. })
                    | Some(ProtocolWaitKind::ResourceSlot { .. }) => false,
                    // Another same-root barrier: order by the barrier task's
                    // SPAWN order (`TaskId`, allocated monotonically). A
                    // descendant is always spawned AFTER its ancestor (the
                    // ancestor must run to spawn it), so a higher `TaskId` means
                    // "nested deeper / spawned later" — THIS caller WAITS for a
                    // higher-id barrier and EXCLUDES a lower one. This strict
                    // total order releases the highest-id eligible barrier first
                    // (its continuation runs, it settles, the next-lower one then
                    // proceeds), so a nested inner barrier is never starved by an
                    // ancestor racing it, two barriers can never mutually wait
                    // (no deadlock), and it is robust to a reaped intermediate
                    // spawner. (Park order is NOT usable here: an outer task that
                    // suspends before its own `async/run` lets a descendant park
                    // first, inverting park order against nesting order.)
                    Some(ProtocolWaitKind::OriginBarrier { .. }) => *id > caller,
                    // No protocol entry ⇒ an External wait held in the
                    // `WaitRuntime` (a real in-flight I/O op): self-resolving.
                    None => true,
                }
            }
        }
    })
}

/// Register an `async/run` (`OriginBarrier`) suspension: park the caller with an
/// internal wait key and record its origin root. Nothing arms a timer or a
/// registry — `resolve_origin_barriers` re-evaluates the release predicate every
/// drive iteration and resumes the continuation with `Returned(nil)` once it holds.
fn install_origin_barrier_wait(
    state: &mut RuntimeState,
    task_id: TaskId,
    key: super::WaitKey,
    owner: ReturnOwner,
    frame: ContinuationFrame,
) -> Result<(), ProtocolInstallError> {
    let root = state
        .tasks
        .get(&task_id)
        .expect("protocol task exists")
        .record
        .relations()
        .origin_root;
    if let Err(error) = state
        .tasks
        .get_mut(&task_id)
        .expect("protocol task exists")
        .record
        .wait(key)
    {
        return Err(Box::new((
            owner,
            frame,
            sema_core::SemaError::eval(format!("origin barrier wait transition failed: {error:?}")),
        )));
    }
    state.protocol_waits.insert(
        key,
        ProtocolWait {
            task: task_id,
            kind: ProtocolWaitKind::OriginBarrier { root },
            owner,
            continuation: frame,
        },
    );
    state.origin_barrier_waits += 1;
    Ok(())
}

/// Register a `ResourceSlot(gate)` acquire suspension. A free gate is granted
/// immediately (resume with nil, no wait recorded); a busy gate parks the task
/// FIFO in the gate's queue with a `ResourceSlot` protocol wait, resumed by the
/// owner's later `release` (or failed by `close`). Mirrors `install_channel_wait`'s
/// immediate-vs-parked split.
fn install_resource_slot_wait(
    state: &mut RuntimeState,
    task_id: TaskId,
    key: super::WaitKey,
    gate: ResourceGateId,
    owner: ReturnOwner,
    frame: ContinuationFrame,
) -> Result<(), ProtocolInstallError> {
    match state.resource_gates.acquire(gate, key, task_id) {
        Ok(AcquireResult::Acquired) => {
            state.pending.push_back(PendingStage::ApplyRuntimeResponse(
                task_id,
                owner,
                frame,
                Ok(RuntimeResponse::Value(sema_core::Value::nil())),
            ));
            Ok(())
        }
        Ok(AcquireResult::Parked) => {
            if let Err(error) = state
                .tasks
                .get_mut(&task_id)
                .expect("protocol task exists")
                .record
                .wait(key)
            {
                let _ = state.resource_gates.cancel_wait(gate, key);
                return Err(Box::new((
                    owner,
                    frame,
                    sema_core::SemaError::eval(format!(
                        "resource gate wait transition failed: {error:?}"
                    )),
                )));
            }
            state.protocol_waits.insert(
                key,
                ProtocolWait {
                    task: task_id,
                    kind: ProtocolWaitKind::ResourceSlot { gate },
                    owner,
                    continuation: frame,
                },
            );
            Ok(())
        }
        Err(error) => Err(Box::new((owner, frame, registry_error(error)))),
    }
}

/// Register a `Send`/`Receive` channel suspension. A genuine park (no ready
/// peer) is unchanged from the staged path: the task is parked FIFO in the
/// channel's queue with a `Channel` protocol wait, resumed by a later matching
/// op or `close`.
///
/// An IMMEDIATE MATCH (a ready peer, or spare buffer capacity) is Task D's
/// fast path: instead of queuing a `ChannelWake` for the matched peer and an
/// `ApplyRuntimeResponse` for this task — each a separate `advance_pending`
/// work item under the staged path, ~10 drive iterations end to end for one
/// rendezvous — both settled waiters are returned as `ChannelRendezvousResume`s
/// for `install_channel_wait_and_resume` to resume inline. This function itself
/// never resumes a continuation (it runs under a live `RuntimeState` borrow;
/// see `ChannelRendezvousResume`'s doc for why that must stay borrow-only).
/// The peer's `finish_protocol_wait` bookkeeping (`protocol_waits` removal +
/// its task's wait->ready transition) IS done here, inline — unlike resuming
/// a continuation, it never runs Sema-level code, so it's safe under this
/// borrow, and it credits `channel_fast_path_credit` for the `ChannelWake`
/// hop it replaces (`drive()` folds the credit into `work_items` after this
/// work item, so a matched rendezvous cannot look "free" and starve sibling
/// roots). Cancellation is not bypassed: `resume_continuation_value` (called
/// later, by the caller) re-checks the task's cancellation and resumes as
/// `Cancelled` if it landed in the meantime (UCR-3), exactly like the staged
/// path.
fn install_channel_wait(
    state: &mut RuntimeState,
    task_id: TaskId,
    key: super::WaitKey,
    wait: sema_core::runtime::ChannelWait,
    owner: ReturnOwner,
    frame: ContinuationFrame,
) -> Result<ChannelWaitOutcome, ChannelWaitError> {
    let (channel, receive, result) = match wait {
        sema_core::runtime::ChannelWait::Send { channel, value } => (
            channel,
            false,
            state.channels.send(channel, key, task_id, value),
        ),
        sema_core::runtime::ChannelWait::Receive { channel } => {
            (channel, true, state.channels.receive(channel, key, task_id))
        }
    };
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            return Err(ChannelWaitError::Protocol(Box::new((
                owner,
                frame,
                registry_error(error),
            ))))
        }
    };
    // A wake can only be present here when `result != Waiting` (this task's
    // own op is what resolved the peer), so it is always `None` on the park
    // path below.
    let wake = state.channels.pop_wake();
    if result == super::ChannelResult::Waiting {
        if let Err(error) = state
            .tasks
            .get_mut(&task_id)
            .expect("protocol task exists")
            .record
            .wait(key)
        {
            let _ = state.channels.cancel_wait(channel, key);
            return Err(ChannelWaitError::Protocol(Box::new((
                owner,
                frame,
                sema_core::SemaError::eval(format!("channel wait transition failed: {error:?}")),
            ))));
        }
        state.protocol_waits.insert(
            key,
            ProtocolWait {
                task: task_id,
                kind: ProtocolWaitKind::Channel { channel, receive },
                owner,
                continuation: frame,
            },
        );
        debug_assert!(wake.is_none(), "a wake implies an immediate match");
        return Ok(ChannelWaitOutcome::Parked);
    }
    let response = match (receive, result) {
        (true, super::ChannelResult::Received(value)) => {
            RuntimeResponse::Receive(sema_core::runtime::ChannelReceive::Received(value))
        }
        (true, super::ChannelResult::Closed) => {
            RuntimeResponse::Receive(sema_core::runtime::ChannelReceive::Closed)
        }
        (false, super::ChannelResult::Sent) => {
            RuntimeResponse::Send(sema_core::runtime::ChannelSend::Sent)
        }
        (false, super::ChannelResult::Closed) => {
            RuntimeResponse::Send(sema_core::runtime::ChannelSend::Closed)
        }
        (_, super::ChannelResult::Waiting) => unreachable!("handled waiting channel"),
        (true, super::ChannelResult::Sent) | (false, super::ChannelResult::Received(_)) => {
            unreachable!("channel result matches operation")
        }
    };
    let this = ChannelRendezvousResume {
        task_id,
        owner,
        frame,
        response: Ok(response),
    };
    // This task was never installed as a protocol wait (it matched
    // immediately), so unlike the peer below there is no `finish_protocol_wait`
    // bookkeeping to fold in.
    let peer = match wake {
        Some(wake) => {
            match channel_wake_response(&state.protocol_waits, wake.key, wake.result) {
                Some(peer_response) => {
                    // Replaces the `PendingStage::ChannelWake` ->
                    // `consume_channel_wake` hop.
                    state.channel_fast_path_credit += 1;
                    finish_protocol_wait_now(state, wake.key, wake.task, peer_response)?.map(
                        |(owner, frame, response)| {
                            Box::new(ChannelRendezvousResume {
                                task_id: wake.task,
                                owner,
                                frame,
                                response,
                            })
                        },
                    )
                }
                None => None,
            }
        }
        None => None,
    };
    Ok(ChannelWaitOutcome::Matched { this, peer })
}

struct ActiveDriveGuard(Rc<RefCell<RuntimeState>>);

impl Drop for ActiveDriveGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.try_borrow_mut() {
            state.drive_active = false;
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.close_for_interpreter_drop();
        if Rc::strong_count(&self.state) == 1 {
            sema_core::unregister_output_capture_sink(self.runtime_id);
        }
    }
}

// Yield/native actions are produced by the test payload until Task 4 connects VM execution.
#[cfg_attr(not(test), allow(dead_code))]
enum TaskAction {
    Yield(RootId, TaskId),
    Settle(RootId, TaskId, TaskOutcome),
    Cancel(TaskId, ReturnOwner, CancelReason),
    Native(TaskId, NativeResult),
    VmResult(TaskId, ReturnOwner, NativeResult),
    /// A cooperative (headless) debug session hit a breakpoint/step inside this
    /// task. The task's frames are parked in `vm_call`; arm the runtime-wide
    /// debug barrier (`debug_paused`) and hold the task out of the ready queue
    /// until the host resumes it (`debug_resume`).
    DebugStop(RootId, TaskId, crate::debug::StopInfo),
    #[cfg(test)]
    Timer(TaskId, Instant),
    #[cfg(test)]
    NativeCall(TaskId, Box<dyn FnOnce() -> NativeResult>),
    Resume(PendingResume),
}

enum PendingStage {
    Action(TaskAction),
    Decode(PendingResume),
    Continue(PendingResume),
    Invoke(TaskId, ReturnOwner, NativeCall),
    Resume(TaskId, ReturnOwner, ContinuationFrame, ResumeInput),
    Apply(TaskId, ReturnOwner, NativeResult),
    DispatchRuntime(TaskId, ReturnOwner, RuntimeRequest),
    ApplyRuntimeResponse(
        TaskId,
        ReturnOwner,
        ContinuationFrame,
        Result<RuntimeResponse, sema_core::SemaError>,
    ),
    PromiseWakes(VecDeque<(super::WaitKey, TaskId)>),
    ChannelClose(ChannelClose),
    ChannelWake(ChannelWake),
    ResourceGateWake(ResourceGateWake),
}

fn task_belongs_to_roots(state: &RuntimeState, task: TaskId, roots: &[RootId]) -> bool {
    state
        .tasks
        .get(&task)
        .is_some_and(|task| roots.contains(&task.record.relations().origin_root))
}

impl TaskAction {
    fn belongs_to_roots(&self, state: &RuntimeState, roots: &[RootId]) -> bool {
        match self {
            Self::Yield(root, _) | Self::Settle(root, _, _) | Self::DebugStop(root, _, _) => {
                roots.contains(root)
            }
            Self::Cancel(task, _, _) | Self::Native(task, _) | Self::VmResult(task, _, _) => {
                task_belongs_to_roots(state, *task, roots)
            }
            #[cfg(test)]
            Self::Timer(task, _) | Self::NativeCall(task, _) => {
                task_belongs_to_roots(state, *task, roots)
            }
            Self::Resume(pending) => task_belongs_to_roots(state, pending.task_id(), roots),
        }
    }
}

impl PendingStage {
    fn belongs_to_roots(&self, state: &RuntimeState, roots: &[RootId]) -> bool {
        match self {
            Self::Action(action) => action.belongs_to_roots(state, roots),
            Self::Decode(pending) | Self::Continue(pending) => {
                task_belongs_to_roots(state, pending.task_id(), roots)
            }
            Self::Invoke(task, _, _)
            | Self::Resume(task, _, _, _)
            | Self::Apply(task, _, _)
            | Self::DispatchRuntime(task, _, _)
            | Self::ApplyRuntimeResponse(task, _, _, _)
            | Self::ChannelWake(ChannelWake { task, .. })
            | Self::ResourceGateWake(ResourceGateWake { task, .. }) => {
                task_belongs_to_roots(state, *task, roots)
            }
            Self::PromiseWakes(wakes) => wakes
                .iter()
                .any(|(_, task)| task_belongs_to_roots(state, *task, roots)),
            Self::ChannelClose(close) => {
                close.has_task(|task| task_belongs_to_roots(state, task, roots))
            }
        }
    }
}

pub(super) enum ReturnOwner {
    Root,
    Continuation(Box<ReturnOwner>, ContinuationFrame),
    /// A parent VM quantum that returned a `NativeOutcome` structurally (surfaced
    /// as `VmExecResult::Pending`) and is parked OUT of `task.vm_call` while the
    /// runtime drives that outcome's continuation on the same task (Task 04). The
    /// continuation machine reuses `task.vm_call` for each callback VM, so the
    /// parked parent rides here instead. When the driven outcome finally
    /// `Return`s (or errors), the parent VM is reinstalled as the task's running
    /// VM and resumed with the value (or the raised error). `parent` is the
    /// owner the parent VM itself settles through (normally `Root`).
    VmResume {
        vm: Box<VM>,
        parent: Box<ReturnOwner>,
    },
}

impl ReturnOwner {
    /// The parked parent (HOF-invoking) VM this owner carries, if any. A
    /// cooperative HOF parks its VM in a `VmResume` (possibly under
    /// `Continuation` frames while its callback continuation is driven); that VM
    /// still owns the stack slots any escaping callback upvalue points into, so
    /// `invoke_callable` closes those upvalues against it before running the
    /// callback on a foreign VM.
    fn parked_parent_vm_mut(&mut self) -> Option<&mut VM> {
        match self {
            ReturnOwner::VmResume { vm, .. } => Some(vm),
            ReturnOwner::Continuation(parent, _) => parent.parked_parent_vm_mut(),
            ReturnOwner::Root => None,
        }
    }
}

impl Trace for PendingStage {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Action(action) => action.trace(sink),
            Self::Decode(pending) | Self::Continue(pending) => pending.trace(sink),
            Self::Invoke(_, owner, call) => owner.trace(sink) && call.trace(sink),
            Self::Resume(_, owner, frame, input) => {
                owner.trace(sink) && frame.trace(sink) && input.trace(sink)
            }
            Self::Apply(_, owner, result) => {
                owner.trace(sink)
                    && match result {
                        Ok(outcome) => outcome.trace(sink),
                        Err(error) => error.trace(sink),
                    }
            }
            Self::DispatchRuntime(_, owner, request) => owner.trace(sink) && request.trace(sink),
            Self::ApplyRuntimeResponse(_, owner, frame, response) => {
                owner.trace(sink)
                    && frame.trace(sink)
                    && match response {
                        Ok(response) => response.trace(sink),
                        Err(error) => error.trace(sink),
                    }
            }
            Self::PromiseWakes(_) => true,
            Self::ChannelClose(close) => close.trace(sink),
            Self::ChannelWake(wake) => wake.trace(sink),
            Self::ResourceGateWake(wake) => wake.trace(sink),
        }
    }
}

impl Trace for ReturnOwner {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Root => true,
            Self::Continuation(parent, frame) => parent.trace(sink) && frame.trace(sink),
            Self::VmResume { vm, parent } => vm.trace(sink) && parent.trace(sink),
        }
    }
}

impl Trace for TaskAction {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Settle(_, _, outcome) => outcome.trace(sink),
            Self::Cancel(_, owner, _) => owner.trace(sink),
            Self::Native(_, result) => match result {
                Ok(outcome) => outcome.trace(sink),
                Err(error) => error.trace(sink),
            },
            Self::VmResult(_, owner, result) => {
                owner.trace(sink)
                    && match result {
                        Ok(outcome) => outcome.trace(sink),
                        Err(error) => error.trace(sink),
                    }
            }
            // A `StopInfo` is a reason + source location; it holds no Sema
            // `Value`, so there is no GC edge to trace.
            Self::DebugStop(_, _, _) => true,
            #[cfg(test)]
            Self::NativeCall(_, _) => true,
            #[cfg(test)]
            Self::Timer(_, _) => true,
            Self::Resume(pending) => pending.trace(sink),
            Self::Yield(_, _) => true,
        }
    }
}

#[cfg(test)]
pub(super) enum TestPreparedTask {
    Return(Option<Value>),
    YieldForever,
    Native(Option<NativeResult>),
    NativeCall(Option<Box<dyn FnOnce() -> NativeResult>>),
    DebugStop(Option<crate::debug::StopInfo>),
    TimerReturn {
        deadline: Option<Instant>,
        value: Option<Value>,
    },
}

#[cfg(test)]
impl TestPreparedTask {
    pub(super) fn returned(value: Value) -> Self {
        Self::Return(Some(value))
    }

    pub(super) fn yield_forever() -> Self {
        Self::YieldForever
    }

    pub(super) fn native(result: NativeResult) -> Self {
        Self::Native(Some(result))
    }

    pub(super) fn native_call(call: impl FnOnce() -> NativeResult + 'static) -> Self {
        Self::NativeCall(Some(Box::new(call)))
    }

    pub(super) fn debug_stop() -> Self {
        Self::DebugStop(Some(crate::debug::StopInfo {
            reason: crate::debug::StopReason::Breakpoint,
            file: None,
            line: 1,
        }))
    }

    pub(super) fn timer_returned(deadline: Instant, value: Value) -> Self {
        Self::TimerReturn {
            deadline: Some(deadline),
            value: Some(value),
        }
    }

    fn next(&mut self, root: RootId, task: TaskId) -> TaskAction {
        match self {
            Self::Return(value) => TaskAction::Settle(
                root,
                task,
                TaskOutcome::Returned(value.take().unwrap_or(Value::NIL)),
            ),
            Self::YieldForever => TaskAction::Yield(root, task),
            Self::Native(result) => TaskAction::Native(
                task,
                result
                    .take()
                    .unwrap_or_else(|| Err(sema_core::SemaError::eval("test task resumed twice"))),
            ),
            Self::NativeCall(call) => TaskAction::NativeCall(
                task,
                call.take().expect("test native callable executes once"),
            ),
            Self::DebugStop(info) => TaskAction::DebugStop(
                root,
                task,
                info.take().expect("test debug stop executes once"),
            ),
            Self::TimerReturn { deadline, value } => deadline.take().map_or_else(
                || {
                    TaskAction::Settle(
                        root,
                        task,
                        TaskOutcome::Returned(value.take().unwrap_or(Value::NIL)),
                    )
                },
                |deadline| TaskAction::Timer(task, deadline),
            ),
        }
    }
}

#[cfg(test)]
impl Trace for TestPreparedTask {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Return(Some(value)) => {
                sink(sema_core::cycle::GcEdge::Value(value));
                true
            }
            Self::Native(Some(Ok(outcome))) => outcome.trace(sink),
            Self::Native(Some(Err(error))) => error.trace(sink),
            Self::NativeCall(_) => true,
            _ => true,
        }
    }
}

#[cfg(test)]
mod scope_swap_tests {
    use super::*;
    use sema_core::runtime::{
        CompletionDelivery, CompletionRegistrar, CompletionSender, ExternalCompletion,
    };
    use std::any::Any;
    use std::cell::{Cell, RefCell};

    struct ClosedInbox;
    impl CompletionSender for ClosedInbox {
        fn send(&self, _: ExternalCompletion) -> CompletionDelivery {
            CompletionDelivery::InboxClosed
        }
    }

    // ── Modeled otel span stack ──────────────────────────────────────
    // A raw thread-local `Vec<u64>` standing in for the real `sema-otel`
    // span stack: pushing an id models opening a span. Two interleaving
    // tasks that shared this one stack would see each other's ids.
    thread_local! {
        static STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
        static ACTIVE_USAGE: Cell<u64> = const { Cell::new(0) };
    }

    fn otel_take() -> Box<dyn Any> {
        Box::new(STACK.with(|s| std::mem::take(&mut *s.borrow_mut())))
    }
    fn otel_install(ctx: Box<dyn Any>) -> Box<dyn Any> {
        let incoming = ctx.downcast::<Vec<u64>>().map(|b| *b).unwrap_or_default();
        Box::new(STACK.with(|s| std::mem::replace(&mut *s.borrow_mut(), incoming)))
    }
    fn otel_scope() -> Box<dyn Any> {
        Box::new(Vec::<u64>::new())
    }
    fn otel_is_empty(ctx: &Box<dyn Any>) -> bool {
        ctx.downcast_ref::<Vec<u64>>().is_none_or(Vec::is_empty)
    }
    fn otel_ambient_is_empty() -> bool {
        STACK.with(|s| s.borrow().is_empty())
    }

    fn usage_take() -> Box<dyn Any> {
        Box::new(ACTIVE_USAGE.with(|a| a.replace(0)))
    }
    fn usage_install(ctx: Box<dyn Any>) -> Box<dyn Any> {
        let incoming = ctx.downcast::<u64>().map(|b| *b).unwrap_or(0);
        Box::new(ACTIVE_USAGE.with(|a| a.replace(incoming)))
    }
    fn usage_capture() -> Box<dyn Any> {
        Box::new(ACTIVE_USAGE.with(|a| a.get()))
    }
    fn usage_is_empty(ctx: &Box<dyn Any>) -> bool {
        ctx.downcast_ref::<u64>().is_none_or(|v| *v == 0)
    }
    fn usage_ambient_is_empty() -> bool {
        ACTIVE_USAGE.with(|a| a.get() == 0)
    }

    fn make_task(seed_span: u64, seed_usage: u64) -> RuntimeTask {
        let (_runtime, _registrar, issuers) =
            CompletionRegistrar::register(Arc::new(ClosedInbox)).unwrap();
        let (mut root_ids, _, _) = issuers.into_parts();
        let mut tasks = IdCounter::<TaskId>::new();
        let root = root_ids.allocate().unwrap();
        let id = tasks.allocate().unwrap();
        let relations = TaskRelations {
            origin_root: root,
            cancellation_parent: CancellationParent::Root(root),
            lifetime_owner: LifetimeOwner::Root(root),
        };
        RuntimeTask {
            record: TaskRecord::new(id, relations),
            payload: TaskPayload::Test(TestPreparedTask::yield_forever()),
            pending_resume: None,
            suspended_owner: None,
            vm_call: None,
            vm_owner: None,
            context: TaskContextHandle::default(),
            vm_resume: None,
            // Slots ordered per `TASK_SCOPE_SEAMS`: [LLM, OTel, usage]. No LLM scope;
            // seed the modeled OTel span stack and leaf-usage scope.
            scopes: TaskScopes {
                captured: [
                    None,
                    Some(Box::new(vec![seed_span])),
                    Some(Box::new(seed_usage)),
                ],
            },
        }
    }

    /// Two interleaving tasks must not share the thread-local otel span stack or
    /// the active leaf-usage scope: each `install`/`restore` round-trip installs
    /// ONLY that task's context, and a span opened during a task's quantum stays on
    /// that task (never leaks onto a sibling). Without `TaskScopeSwap` handling otel
    /// + usage, both tasks would push onto one shared stack and cross-attribute.
    #[test]
    fn interleaved_quanta_keep_otel_and_usage_isolated() {
        sema_core::set_otel_task_callbacks(otel_take, otel_install, otel_scope);
        sema_core::set_otel_empty_callbacks(otel_is_empty, otel_ambient_is_empty);
        sema_core::set_usage_scope_task_callbacks(usage_capture, usage_take, usage_install);
        sema_core::set_usage_scope_empty_callbacks(usage_is_empty, usage_ambient_is_empty);
        // Start from a clean thread-local state (other tests on this thread may have
        // left the modeled stack populated).
        STACK.with(|s| s.borrow_mut().clear());
        ACTIVE_USAGE.with(|a| a.set(0));

        let mut a = make_task(10, 1);
        let mut b = make_task(20, 2);

        // Task A quantum: sees its own span stack [10] + usage 1, opens a child span.
        {
            let mut swap = TaskScopeSwap::install(&mut a);
            STACK.with(|s| assert_eq!(*s.borrow(), vec![10]));
            ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 1));
            STACK.with(|s| s.borrow_mut().push(11)); // open span during the quantum
            swap.restore(&mut a);
        }
        // Restored to the empty (spawner/global) context; A carries its opened span.
        STACK.with(|s| assert!(s.borrow().is_empty()));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 0));

        // Task B quantum interleaves: it must see ITS OWN [20] + usage 2, never A's.
        {
            let mut swap = TaskScopeSwap::install(&mut b);
            STACK.with(|s| assert_eq!(*s.borrow(), vec![20]));
            ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 2));
            STACK.with(|s| s.borrow_mut().push(21));
            swap.restore(&mut b);
        }

        // Task A resumes: it must observe only its own carried stack [10, 11] and
        // usage 1 — B's span 21 and usage 2 are absent (no cross-task leak).
        {
            let mut swap = TaskScopeSwap::install(&mut a);
            STACK.with(|s| assert_eq!(*s.borrow(), vec![10, 11]));
            ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 1));
            swap.restore(&mut a);
        }

        // Both tasks retain their own accumulated context across the interleave.
        // Slot 1 is the OTel span stack (per `TASK_SCOPE_SEAMS` order).
        let a_stack = a.scopes.captured[1]
            .take()
            .unwrap()
            .downcast::<Vec<u64>>()
            .unwrap();
        let b_stack = b.scopes.captured[1]
            .take()
            .unwrap()
            .downcast::<Vec<u64>>()
            .unwrap();
        assert_eq!(*a_stack, vec![10, 11]);
        assert_eq!(*b_stack, vec![20, 21]);
    }

    /// `TaskScopeSwap::Drop` reinstalls the displaced contexts even if the quantum
    /// unwinds before `restore` runs, so a parent/sibling's stack is never left
    /// corrupted by a faulting task.
    #[test]
    fn drop_restores_displaced_contexts_on_unwind() {
        sema_core::set_otel_task_callbacks(otel_take, otel_install, otel_scope);
        sema_core::set_otel_empty_callbacks(otel_is_empty, otel_ambient_is_empty);
        sema_core::set_usage_scope_task_callbacks(usage_capture, usage_take, usage_install);
        sema_core::set_usage_scope_empty_callbacks(usage_is_empty, usage_ambient_is_empty);
        STACK.with(|s| {
            let mut s = s.borrow_mut();
            s.clear();
            s.push(99); // a parent span active before the task ran
        });
        ACTIVE_USAGE.with(|a| a.set(7));

        let mut task = make_task(30, 3);
        {
            let _swap = TaskScopeSwap::install(&mut task);
            STACK.with(|s| assert_eq!(*s.borrow(), vec![30]));
            ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 3));
            // Drop without calling restore (models a panic mid-quantum).
        }
        // The parent's context is back in the thread-locals, uncorrupted.
        STACK.with(|s| assert_eq!(*s.borrow(), vec![99]));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 7));
    }

    fn setup_empty_seams() {
        sema_core::set_otel_task_callbacks(otel_take, otel_install, otel_scope);
        sema_core::set_otel_empty_callbacks(otel_is_empty, otel_ambient_is_empty);
        sema_core::set_usage_scope_task_callbacks(usage_capture, usage_take, usage_install);
        sema_core::set_usage_scope_empty_callbacks(usage_is_empty, usage_ambient_is_empty);
    }

    /// A task whose captured otel/usage scopes are both empty (as they are for
    /// every spawned task on a program that never touches these features).
    fn make_empty_task() -> RuntimeTask {
        let mut task = make_task(0, 0);
        task.scopes.captured[1] = Some(Box::new(Vec::<u64>::new()));
        task.scopes.captured[2] = Some(Box::new(0u64));
        task
    }

    /// The empty-scope fast path (Task E): when a task's captured scope AND the
    /// thread-local ambient scope are BOTH empty, `install` must skip the
    /// take/install round-trip entirely rather than touch the thread-locals.
    #[test]
    fn fast_path_skips_swap_when_both_empty() {
        setup_empty_seams();
        STACK.with(|s| s.borrow_mut().clear());
        ACTIVE_USAGE.with(|a| a.set(0));

        let mut task = make_empty_task();
        let mut swap = TaskScopeSwap::install(&mut task);
        // Both non-LLM seams took the fast path; nothing was displaced.
        assert!(swap.skipped[1], "otel seam should take the fast path");
        assert!(swap.skipped[2], "usage seam should take the fast path");
        assert!(swap.displaced[1].is_none());
        assert!(swap.displaced[2].is_none());
        // The task's own captured slot is left untouched (still present, still
        // empty) rather than taken into thread-locals.
        assert!(task.scopes.captured[1].is_some());
        assert!(task.scopes.captured[2].is_some());
        // Ambient thread-locals were never disturbed.
        STACK.with(|s| assert!(s.borrow().is_empty()));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 0));

        swap.restore(&mut task);
        STACK.with(|s| assert!(s.borrow().is_empty()));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 0));
        assert!(task.scopes.captured[1].is_some());
        assert!(task.scopes.captured[2].is_some());
    }

    /// The BOTH-empty condition is required, not just "captured is empty": a task
    /// with an empty captured scope entering a thread whose ambient scope is
    /// LIVE (e.g. spawned inside a still-on-the-stack `llm/with-budget`, or a
    /// prior task's scope wasn't fully unwound) must still swap so the quantum
    /// sees its OWN empty scope, never the ambient one.
    #[test]
    fn ambient_nonempty_forces_real_swap_even_when_captured_is_empty() {
        setup_empty_seams();
        // Ambient carries a live "parent" span + usage (models a root-level scope
        // still on the Rust call stack when this task's quantum runs).
        STACK.with(|s| {
            let mut s = s.borrow_mut();
            s.clear();
            s.push(99);
        });
        ACTIVE_USAGE.with(|a| a.set(7));

        let mut task = make_empty_task();
        let mut swap = TaskScopeSwap::install(&mut task);
        assert!(
            !swap.skipped[1] && !swap.skipped[2],
            "must not skip when ambient is non-empty"
        );
        // The quantum sees ITS OWN empty scope, not the ambient [99]/7.
        STACK.with(|s| assert!(s.borrow().is_empty()));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 0));

        swap.restore(&mut task);
        // Ambient is restored exactly as it was; untouched by the empty task.
        STACK.with(|s| assert_eq!(*s.borrow(), vec![99]));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 7));
    }

    /// A skipped seam whose quantum opens a fresh dynamic scope THIS STEP (and
    /// doesn't unwind it before the quantum ends) must have that scope reclaimed
    /// onto the task at `restore` time, and the thread-local reset to empty — or
    /// it would leak into whichever task runs next.
    #[test]
    fn fast_path_reclaims_scope_opened_during_the_quantum() {
        setup_empty_seams();
        STACK.with(|s| s.borrow_mut().clear());
        ACTIVE_USAGE.with(|a| a.set(0));

        let mut task = make_empty_task();
        let mut swap = TaskScopeSwap::install(&mut task);
        assert!(swap.skipped[1] && swap.skipped[2]);
        // The quantum opens its own span and usage this step, directly against
        // the (skipped-over) thread-local — exactly as it would if the swap had
        // installed an empty scope and the quantum wrote into it.
        STACK.with(|s| s.borrow_mut().push(42));
        ACTIVE_USAGE.with(|a| a.set(5));
        swap.restore(&mut task);

        // Reclaimed onto the task, and the thread-local reset to empty so a
        // sibling task never observes it.
        STACK.with(|s| assert!(s.borrow().is_empty()));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 0));
        let stack = task.scopes.captured[1]
            .take()
            .unwrap()
            .downcast::<Vec<u64>>()
            .unwrap();
        assert_eq!(*stack, vec![42]);
        let usage = task.scopes.captured[2]
            .take()
            .unwrap()
            .downcast::<u64>()
            .unwrap();
        assert_eq!(*usage, 5);
    }

    /// Same reclaim behavior on the `Drop` (panic-unwind) path.
    #[test]
    fn fast_path_reclaims_on_drop_when_opened_mid_quantum() {
        setup_empty_seams();
        STACK.with(|s| s.borrow_mut().clear());
        ACTIVE_USAGE.with(|a| a.set(0));

        let mut task = make_empty_task();
        {
            let swap = TaskScopeSwap::install(&mut task);
            assert!(swap.skipped[1] && swap.skipped[2]);
            STACK.with(|s| s.borrow_mut().push(7));
            ACTIVE_USAGE.with(|a| a.set(9));
            // Drop without calling restore (models a panic mid-quantum). The
            // faulting task's own scope is discarded either way, but the
            // thread-local must not leak the opened span/usage to the next task.
        }
        STACK.with(|s| assert!(s.borrow().is_empty()));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 0));
    }

    /// Full interleave with the fast path mixed in: a NON-EMPTY task and an
    /// EMPTY task run alternating quanta on the same thread, and the empty task
    /// opens its OWN scope mid-quantum while the empty-skip fast path is active.
    /// This is the P-hotfix invariant under the fast path: the non-empty task's
    /// restore must clear the thread-local to empty (so the empty task's install
    /// legitimately skips), the empty task's mid-quantum scope must be reclaimed
    /// onto it (never leaking onto the non-empty task), and the non-empty task
    /// must see ONLY its own step-modified scope when it resumes.
    #[test]
    fn interleave_nonempty_then_empty_skip_with_midquantum_open() {
        setup_empty_seams();
        STACK.with(|s| s.borrow_mut().clear());
        ACTIVE_USAGE.with(|a| a.set(0));

        let mut ne = make_task(10, 1); // non-empty: span [10], usage 1
        let mut e = make_empty_task(); // empty captured: span [], usage 0

        // Quantum 1 — non-empty task: real swap installs its own scope.
        {
            let mut swap = TaskScopeSwap::install(&mut ne);
            assert!(
                !swap.skipped[1] && !swap.skipped[2],
                "non-empty must not skip"
            );
            STACK.with(|s| assert_eq!(*s.borrow(), vec![10]));
            ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 1));
            STACK.with(|s| s.borrow_mut().push(11)); // open a child span this step
            ACTIVE_USAGE.with(|u| u.set(5)); // accrue usage this step
            swap.restore(&mut ne);
        }
        // Restore must have cleared the thread-local to empty (displaced ambient
        // was empty) — this is what lets the empty task's install skip below.
        STACK.with(|s| assert!(s.borrow().is_empty(), "ne restore must clear ambient"));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 0));

        // Quantum 2 — empty task: fast-path skip fires (ambient is now empty),
        // and the quantum opens its OWN scope directly against the thread-local.
        {
            let mut swap = TaskScopeSwap::install(&mut e);
            assert!(swap.skipped[1] && swap.skipped[2], "empty task must skip");
            // The empty task must NOT observe the non-empty task's leftover.
            STACK.with(|s| assert!(s.borrow().is_empty(), "empty task saw ne's span"));
            ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 0, "empty task saw ne's usage"));
            STACK.with(|s| s.borrow_mut().push(99)); // empty task opens its own span
            ACTIVE_USAGE.with(|u| u.set(7));
            swap.restore(&mut e);
        }
        // Empty task's mid-quantum scope reclaimed onto it; thread-local cleared.
        STACK.with(|s| assert!(s.borrow().is_empty(), "e restore must clear ambient"));
        ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 0));

        // Quantum 3 — non-empty task resumes: its captured scope is now the
        // step-modified [10, 11]/5, and it must see ONLY that — never the empty
        // task's [99]/7.
        {
            let mut swap = TaskScopeSwap::install(&mut ne);
            assert!(!swap.skipped[1] && !swap.skipped[2]);
            STACK.with(|s| assert_eq!(*s.borrow(), vec![10, 11], "ne inherited e's span"));
            ACTIVE_USAGE.with(|u| assert_eq!(u.get(), 5, "ne inherited e's usage"));
            swap.restore(&mut ne);
        }

        // The empty task carries its own reclaimed [99]/7, isolated from ne.
        let e_stack = e.scopes.captured[1]
            .take()
            .unwrap()
            .downcast::<Vec<u64>>()
            .unwrap();
        assert_eq!(*e_stack, vec![99]);
        let e_usage = e.scopes.captured[2]
            .take()
            .unwrap()
            .downcast::<u64>()
            .unwrap();
        assert_eq!(*e_usage, 7);
    }
}
