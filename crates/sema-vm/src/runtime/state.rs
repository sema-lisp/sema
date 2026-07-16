//! Interpreter-owned runtime state and root lifecycle.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::vm::{
    close_closure_upvalues_for_foreign_run, close_closure_upvalues_with_owner,
    snapshot_escaping_call_with_owner,
};
use crate::{extract_vm_closure, VmExecResult, VM};
#[cfg(test)]
use sema_core::runtime::ExternalFailure;
use sema_core::runtime::{
    CancelReason, CancellationView, ExecutorShutdown, IdCounter, IoExecutor, NativeCall,
    NativeCallContext, NativeOutcome, NativeResult, ResourceGateId, ResumeInput, RootId,
    RuntimeRequest, RuntimeResponse, RuntimeScopedIdCounter, SettlementSeq, TaskContextHandle,
    TaskId, TaskOutcome, TaskSettlement, Trace, WaitKind,
};
use sema_core::runtime::{CancellationParent, LifetimeOwner, TaskRelations};
use sema_core::EvalContext;
#[cfg(test)]
use sema_core::Value;
use sema_core::YieldReason;

use super::channel::{ChannelClose, ChannelWake};
use super::{
    AcquireResult, ChannelRegistry, ContinuationFrame, DriveBudget, DriveState, GateResult,
    PendingResume, PromiseRegistry, PromiseState, ReadyScheduler, RegisterExternalError,
    ResourceGateRegistry, ResourceGateWake, RootRecord, RootState, RuntimeClock,
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

pub enum RootPoll {
    Pending,
    Ready(Rc<TaskSettlement>),
    Aborted(RuntimeFault),
    RuntimeDropped,
    InvariantViolation,
}

#[derive(Clone, Debug)]
pub struct ShutdownOptions {
    pub deadline: Instant,
    pub drive_budget: DriveBudget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShutdownReport {
    pub clean: bool,
    pub live_roots: usize,
    pub live_tasks: usize,
    pub active_waits: usize,
    pub retained_cleanup: usize,
    pub executor: Option<ExecutorShutdown>,
    pub cleanup_diagnostics: Vec<super::CleanupDiagnostic>,
    pub invariant_failures: Vec<ShutdownInvariantFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShutdownInvariantFailure {
    pub name: &'static str,
    pub diagnostic: super::CleanupDiagnostic,
}

pub struct Runtime {
    state: Rc<RefCell<RuntimeState>>,
}

pub struct RootHandle {
    runtime: Weak<RefCell<RuntimeState>>,
    id: RootId,
}

impl Trace for Runtime {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.state.try_borrow().is_ok_and(|state| state.trace(sink))
    }
}

struct RuntimeTask {
    record: TaskRecord,
    payload: TaskPayload,
    pending_resume: Option<PendingResume>,
    suspended_owner: Option<ReturnOwner>,
    vm_call: Option<VM>,
    vm_owner: Option<ReturnOwner>,
    context: TaskContextHandle,
    /// Pending resume for a VM-quantum task woken from an `async/await` (or
    /// `async/spawn`) park: the value to inject onto the parked frame's stack
    /// top before the next `run_quantum`, or a failure to settle the task with.
    vm_resume: Option<VmResume>,
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
    scopes: TaskScopes,
}

/// How a parked VM-quantum task should be resumed once its awaited promise
/// settles (or a spawn admission is decided).
enum VmResume {
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
}

/// The per-task dynamic scopes swapped around every quantum, in a fixed order. Each
/// entry's three seams reach a type-erased `sema-llm` / `sema-otel` thread-local; the
/// registrations are installed at interpreter startup and no-op (empty box) until then.
const TASK_SCOPE_SEAMS: [ScopeSeam; 3] = [
    // LLM dynamic scope (`llm/with-cache` / `with-budget` flags + shared budget `Rc`).
    ScopeSeam {
        capture: sema_core::current_llm_scope_boxed,
        take: sema_core::take_task_llm_scope,
        install: sema_core::install_task_llm_scope,
    },
    // OTel context (span stack + conversation/session/user ids). Capture seeds an
    // EMPTY span stack (ids only) so the child parents to its own trace root.
    ScopeSeam {
        capture: sema_core::current_conversation_scope_boxed,
        take: sema_core::take_task_otel,
        install: sema_core::install_task_otel,
    },
    // Leaf-usage accumulator scope (per-`workflow/step` LLM usage attribution).
    ScopeSeam {
        capture: sema_core::current_usage_scope_boxed,
        take: sema_core::take_task_usage_scope,
        install: sema_core::install_task_usage_scope,
    },
];

/// A task's captured per-quantum dynamic scopes, one slot per [`TASK_SCOPE_SEAMS`]
/// entry (same order). Empty (all `None`) for a root task, which runs directly
/// against the process thread-locals. Holds no GC-traceable `Value` — the scopes
/// carry only scalar snapshots and shared `Rc`s (budget/usage accounts), so this
/// needs no [`Trace`] edge.
#[derive(Default)]
struct TaskScopes {
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
struct TaskScopeSwap {
    displaced: [Option<Box<dyn std::any::Any>>; TASK_SCOPE_SEAMS.len()],
    restored: bool,
}

impl TaskScopeSwap {
    fn install(task: &mut RuntimeTask) -> Self {
        Self {
            displaced: std::array::from_fn(|i| {
                task.scopes.captured[i]
                    .take()
                    .map(TASK_SCOPE_SEAMS[i].install)
            }),
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
        for (i, displaced) in self.displaced.iter_mut().enumerate() {
            if let Some(prev) = displaced.take() {
                task.scopes.captured[i] = Some((TASK_SCOPE_SEAMS[i].take)());
                let _ = (TASK_SCOPE_SEAMS[i].install)(prev);
            }
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
        for (i, displaced) in self.displaced.iter_mut().enumerate() {
            if let Some(prev) = displaced.take() {
                let _ = (TASK_SCOPE_SEAMS[i].take)();
                let _ = (TASK_SCOPE_SEAMS[i].install)(prev);
            }
        }
    }
}

// Task 4 replaces this placeholder with the VM-backed PreparedRoot payload.
#[cfg_attr(not(test), allow(dead_code))]
enum TaskPayload {
    /// A real VM-backed root: `vm_call` drives execution and this payload is
    /// never invoked (the VM-quantum arm in `visit_ready` takes precedence).
    Vm,
    #[cfg(not(test))]
    UnavailableUntilTask4,
    #[cfg(test)]
    Test(TestPreparedTask),
}

struct RuntimeState {
    _context: Rc<EvalContext>,
    clock: Rc<dyn RuntimeClock>,
    waits: Option<WaitRuntime>,
    // Root admission is intentionally test-only until Task 4 supplies PreparedRoot.
    #[cfg_attr(not(test), allow(dead_code))]
    root_ids: RuntimeScopedIdCounter<RootId>,
    #[cfg_attr(not(test), allow(dead_code))]
    task_ids: IdCounter<TaskId>,
    settlement_ids: IdCounter<SettlementSeq>,
    promises: PromiseRegistry,
    channels: ChannelRegistry,
    resource_gates: ResourceGateRegistry,
    roots: HashMap<RootId, RootRecord>,
    tasks: HashMap<TaskId, RuntimeTask>,
    ready: ReadyScheduler,
    timers: TimerQueue,
    handle_cleanup: VecDeque<RootId>,
    pending: VecDeque<PendingStage>,
    protocol_waits: HashMap<super::WaitKey, ProtocolWait>,
    task_promises: HashMap<TaskId, sema_core::runtime::PromiseId>,
    drive_cursor: usize,
    drive_active: bool,
    active_instruction_limit: usize,
    turn_instructions: usize,
    shutting_down: bool,
    terminal_fault: Option<RuntimeFault>,
    /// Cooperative (headless) debug barrier. `Some((root, task, info))` while a
    /// task is paused at a breakpoint/step: the paused task is parked in `tasks`
    /// with its frames in `vm_call`, held OUT of the ready queue. While set, the
    /// drive loop runs no ready task and fires no timer — a runtime-wide
    /// stop-the-world barrier (external completions may still land in the inbox,
    /// they are just not delivered until resume). Cleared by
    /// [`Runtime::debug_resume`], which re-enqueues the paused task.
    debug_paused: Option<(RootId, TaskId, crate::debug::StopInfo)>,
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
        Ok(Self {
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
                drive_cursor: 0,
                drive_active: false,
                active_instruction_limit: usize::MAX,
                turn_instructions: 0,
                shutting_down: false,
                terminal_fault: None,
                debug_paused: None,
                stepping_task: None,
                dropped_protocol_completions: 0,
                origin_barrier_waits: 0,
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

    /// Submit a real VM-backed root: the task runs its pre-seeded VM through
    /// `run_quantum` and settles with the VM result. `vm_call` takes precedence
    /// in `visit_ready`, so the `Vm` payload is never invoked.
    pub fn submit_root(&self, vm: VM) -> Result<RootHandle, SubmitRootError> {
        let mut state = self.state.borrow_mut();
        if state.shutting_down || state.terminal_fault.is_some() {
            return Err(SubmitRootError::ShuttingDown);
        }
        if state.root_ids.is_exhausted() || state.task_ids.is_exhausted() {
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
                payload: TaskPayload::Vm,
                pending_resume: None,
                suspended_owner: None,
                vm_call: Some(vm),
                vm_owner: Some(ReturnOwner::Root),
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
    /// without a real VM closure.
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
                vm_owner: None,
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
        let start = self.state.borrow().clock.now();
        let mut work_items = 0;
        let mut root_visits = 0;
        let mut cleanup = 0;
        let mut completions = 0;
        let mut timers = 0;
        let mut no_progress = 0;
        let reserved_roots = self
            .state
            .borrow()
            .ready
            .root_count()
            .min(budget.root_visit_limit.get());
        // Reserve credits for at most work_item_limit - 1 roots so a ready-root
        // storm always leaves at least one work item for completions, timers,
        // cleanup, and pending stages (spec: each storm leaves progress room).
        let reserve_floor = reserved_roots.min(budget.work_item_limit.get().saturating_sub(1));

        while work_items < budget.work_item_limit.get() {
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
                    .and_then(|waits| waits.expired_quarantine(state.clock.now()))
            };
            if let Some(wait) = expired {
                return Err(RuntimeFault::Invariant {
                    message: format!(
                        "quarantine bound expired for wait {:?}/{:?}",
                        wait.id, wait.generation
                    ),
                });
            }
            if self
                .state
                .borrow()
                .clock
                .now()
                .saturating_duration_since(start)
                >= budget.wall_clock_limit
            {
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
                3 => self.advance_pending()?,
                4 if timers < budget.timer_limit.get() && self.fire_timer()? => {
                    timers += 1;
                    true
                }
                5 if root_visits < reserved_roots && self.visit_ready()? => {
                    root_visits += 1;
                    true
                }
                _ => false,
            };
            if progressed {
                work_items += 1;
                no_progress = 0;
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
        let ready_remaining = state
            .tasks
            .values()
            .any(|task| task.record.state_name() == super::StateName::Ready);
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
        let mut state = self.state.borrow_mut();
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
        let mut state = self.state.borrow_mut();
        let (_root, task_id, _info) = state.debug_paused.as_ref()?;
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
        let paused = self.state.borrow_mut().debug_paused.take();
        let Some((root, task_id, _info)) = paused else {
            return false;
        };
        {
            let mut state = self.state.borrow_mut();
            if let Some(task) = state.tasks.get_mut(&task_id) {
                task.record.request_cancellation(CancelReason::HostStop);
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

    fn fire_timer(&self) -> Result<bool, RuntimeFault> {
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
        let now = state.clock.now();
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
        // A VM task parked directly on a timer (legacy `async/sleep` via
        // `VmSleep`): wake it and re-run its frame. Observational `async/timeout`
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
            state
                .roots
                .get(&root)
                .is_some_and(RootRecord::is_reap_eligible)
                .then(|| state.roots.remove(&root))
                .flatten()
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

    fn cancel_waiting(&self) -> Result<bool, RuntimeFault> {
        {
            let mut state = self.state.borrow_mut();
            let selected = state.tasks.iter().find_map(|(id, task)| {
                let key = task.record.wait_key()?;
                task.record.cancellation()?;
                let wait = state.protocol_waits.get(&key)?;
                // UCR-3: a rendezvous-matched channel waiter is no longer queued
                // in the channel but still holds a `protocol_waits` entry while
                // its `ChannelWake` (carrying the committed value) is in flight.
                // Cancel-dropping it here would silently discard that value. Skip
                // it: the wake delivers the value and the sticky cancellation makes
                // settlement observe cancellation (UCR-1), so nothing is lost.
                if let ProtocolWaitKind::Channel { channel, .. } = &wait.kind {
                    if !state.channels.has_wait(*channel, key) {
                        return None;
                    }
                }
                Some((*id, key))
            });
            if let Some((task_id, key)) = selected {
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
                    ProtocolWaitKind::ResourceSlot { gate } => {
                        // A task cancelled while queued behind a busy gate: drop
                        // it from the FIFO queue so a later `release` skips it.
                        // (An owner cancelled mid-op releases the gate via its
                        // module cancel hook, not here.) If it was already GRANTED
                        // the slot (not in the queue — the gate owner) but never
                        // ran its acquire continuation, release the gate here so
                        // the next acquirer proceeds (no leak).
                        let gate = *gate;
                        let queued =
                            state
                                .resource_gates
                                .cancel_wait(gate, key)
                                .map_err(|error| RuntimeFault::Invariant {
                                    message: format!("resource slot cancel failed: {error:?}"),
                                })?;
                        if !queued {
                            let _ = state.resource_gates.take_wake(key);
                            release_owned_gate(&mut state, gate)?;
                        }
                    }
                    ProtocolWaitKind::OriginBarrier { .. } => {
                        // A cancelled `async/run` barrier: nothing external to tear
                        // down (no timer/registry). Just drop the wait and let the
                        // continuation raise on the cancellation below.
                        state.origin_barrier_waits = state.origin_barrier_waits.saturating_sub(1);
                    }
                }
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
        }
        {
            let mut state = self.state.borrow_mut();
            let timer_task = state.tasks.iter().find_map(|(id, task)| {
                (task.record.state_name() == super::StateName::Waiting
                    && task.record.cancellation().is_some())
                .then(|| task.record.wait_key().map(|key| (*id, key)))
                .flatten()
            });
            if let Some((task_id, key)) = timer_task.filter(|(_, key)| state.timers.cancel(*key)) {
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
        }
        let extracted = {
            let mut state = self.state.borrow_mut();
            let Some(task_id) = state.tasks.iter().find_map(|(id, task)| {
                (task.record.state_name() == super::StateName::Waiting
                    && task.record.cancellation().is_some())
                .then_some(*id)
            }) else {
                return Ok(false);
            };
            let task = state.tasks.remove(&task_id).expect("selected task exists");
            let waits = state.waits.take().ok_or_else(|| RuntimeFault::Invariant {
                message: "wait runtime already extracted".into(),
            })?;
            (task_id, task, waits, state.clock.now())
        };
        let (task_id, mut task, mut waits, now) = extracted;
        let key = task
            .record
            .wait_key()
            .expect("selected waiting task has key");
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
        // `waits.cancel` found no matching `WaitRuntime::active` entry and nothing
        // was woken, so this turn made no real progress on the task — it is still
        // Waiting. Every internal-wait kind that parks a task off `active`
        // (promise / promise-set / protocol / timer / channel) is drained by a
        // dedicated branch above; reaching here means an unhandled wait kind.
        // Report no progress rather than a false `Ok(true)`, which would spin the
        // shutdown cancel loop forever (the class of bug the channel branch fixes).
        Ok(false)
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

    fn advance_pending(&self) -> Result<bool, RuntimeFault> {
        let stage = self.state.borrow_mut().pending.pop_front();
        let Some(stage) = stage else {
            return Ok(false);
        };
        let next = match stage {
            PendingStage::Action(action) => {
                self.apply_action(action)?;
                return Ok(true);
            }
            PendingStage::Decode(pending) => PendingStage::Continue(pending.invoke_decoder()),
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
                PendingStage::Apply(task, owner, pending.invoke_continuation())
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
                if let Some((key, task)) = wakes.pop_front() {
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
                if let Some(wake) = close.next_wake() {
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
        let response = match wake.result {
            super::ChannelResult::Sent => {
                RuntimeResponse::Send(sema_core::runtime::ChannelSend::Sent)
            }
            super::ChannelResult::Received(value) => {
                RuntimeResponse::Receive(sema_core::runtime::ChannelReceive::Received(value))
            }
            super::ChannelResult::Closed => {
                let receive = self
                    .state
                    .borrow()
                    .protocol_waits
                    .get(&wake.key)
                    .is_some_and(|wait| {
                        matches!(wait.kind, ProtocolWaitKind::Channel { receive: true, .. })
                    });
                if receive {
                    RuntimeResponse::Receive(sema_core::runtime::ChannelReceive::Closed)
                } else {
                    RuntimeResponse::Send(sema_core::runtime::ChannelSend::Closed)
                }
            }
            super::ChannelResult::Waiting => return Ok(()),
        };
        self.finish_protocol_wait(wake.key, wake.task, Ok(response))
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
        let extracted = {
            let mut state = self.state.borrow_mut();
            let Some(wait) = state.protocol_waits.remove(&key) else {
                // The wait is gone (e.g. the task was cancelled). A rendezvous
                // value that arrives here would be silently lost, so record it.
                if matches!(
                    &response,
                    Ok(RuntimeResponse::Receive(
                        sema_core::runtime::ChannelReceive::Received(_)
                    ))
                ) {
                    state.dropped_protocol_completions += 1;
                }
                return Ok(());
            };
            if wait.task != task_id {
                state.protocol_waits.insert(key, wait);
                return Ok(());
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
            wait
        };
        let wait = extracted;
        let mut state = self.state.borrow_mut();
        state.pending.push_back(PendingStage::ApplyRuntimeResponse(
            task_id,
            wait.owner,
            wait.continuation,
            response,
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
        // Snapshot the task's cancellation for this quantum: every native driven
        // through the runtime ABI reads it via `NativeCallContext`. Mirrors
        // `invoke_callable`'s `CancellationView` construction.
        let cancellation = {
            let cancel = task.record.cancellation();
            CancellationView::new(cancel.is_some(), cancel.map(|request| request.reason))
        };
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
            Some(task_id.get())
        };
        let prev_task_id = sema_core::set_current_task_id(published_task_id);
        // Regression insurance for the crux invariant (verified on the audited
        // path): no `RuntimeState` borrow is held across the quantum. The debug
        // variant may BLOCK inside the quantum (`handle_debug_stop` parks the
        // thread serving DAP inspection); if a borrow were live here the blocking
        // stop would deadlock the state cell. Per the review this can never fire.
        debug_assert!(
            self.state.try_borrow_mut().is_ok(),
            "RuntimeState borrowed at quantum entry — a blocking debug stop would deadlock the state cell"
        );
        // When a native DAP session is registered on this thread (`ACTIVE_DEBUG`),
        // run the debug-aware quantum so breakpoints/steps inside this task — and
        // inside cooperative HOF callbacks, which also flow through
        // `run_parked_quantum` as enqueued callback-VM quanta — stop and serve
        // inspection against the stopped task's own VM. Otherwise the byte-
        // identical non-debug quantum.
        let quantum = if crate::vm::is_debug_session_active() {
            crate::vm::with_active_debug(|debug| {
                vm.run_quantum_debug(&context, instruction_limit, cancellation, debug)
            })
            .expect("debug session active but no DebugState registered")
        } else {
            vm.run_quantum(&context, instruction_limit, cancellation)
        };
        let _ = sema_core::set_current_task_id(prev_task_id);
        scopes.restore(task);
        drop(quantum_guard);
        self.state.borrow_mut().turn_instructions += quantum.instructions;
        let action = match quantum.outcome {
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
            // A native yielded the VM through the TLS yield signal (surfaced as
            // `AsyncYield`). The VM has already parked its frame (pc past the
            // call, a nil placeholder on the stack top) and stays in `vm_call`;
            // the runtime registers a native wait and, when it fires, re-runs
            // `run_quantum` — the frame resumes in place with the placeholder as
            // the resume value.
            // The only TLS yield signal left is `async/sleep`'s ctx-less value ABI:
            // every promise/channel/offloaded-I/O op suspends structurally through
            // the `NativeOutcome` ABI (`Suspend`/`Runtime`), handled by the `Pending`
            // arm below.
            Ok(VmExecResult::AsyncYield(reason)) => match reason {
                YieldReason::Sleep(ms) => {
                    task.vm_call = Some(vm);
                    TaskAction::VmSleep(task_id, ms)
                }
            },
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
        };
        Ok(action)
    }

    fn visit_ready(&self) -> Result<bool, RuntimeFault> {
        let (root, task_id, mut task) = {
            let mut state = self.state.borrow_mut();
            let Some((root, task_id)) = state.ready.dequeue() else {
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
                vm.resume_with_error(error);
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
            if let Some(VmResume::Value(value)) = resume {
                vm.replace_stack_top(value);
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
            TaskAction::VmSleep(task_id, ms) => {
                let mut state = self.state.borrow_mut();
                let deadline = state.clock.now() + Duration::from_millis(ms);
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
                    .ok_or_else(|| RuntimeFault::Invariant {
                        message: "sleeping VM task disappeared".into(),
                    })?
                    .record
                    .wait(key)
                {
                    state.timers.cancel(key);
                    return Err(RuntimeFault::Invariant {
                        message: format!("sleeping VM task failed to wait: {error:?}"),
                    });
                }
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
        let result = match suspend.wait {
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
            WaitKind::Channel(wait) => {
                install_channel_wait(&mut state, task_id, key, wait, owner, frame)
            }
            WaitKind::Timer(duration) => {
                install_timer_wait(&mut state, task_id, key, duration, owner, frame)
            }
            WaitKind::ResourceSlot(gate) => {
                install_resource_slot_wait(&mut state, task_id, key, gate, owner, frame)
            }
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
            let response = self
                .state
                .borrow_mut()
                .resource_gates
                .allocate()
                .map(RuntimeResponse::ResourceGate)
                .map_err(|_| {
                    sema_core::SemaError::eval("runtime resource gate identity exhausted")
                });
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
                let result = state
                    .resource_gates
                    .close(gate)
                    .map(|()| RuntimeResponse::Value(sema_core::Value::nil()))
                    .map_err(registry_error);
                // Every parked waiter fails `Closed`.
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
            let frame = if extract_vm_closure(&call.callable).is_some() {
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
        let (frame, result) =
            if let Some((closure, functions, native_fns)) = extract_vm_closure(&call.callable) {
                // A cooperative HOF (`map`/`for-each`/`foldl`/…) dispatches its
                // Sema callback on the FRESH callback VM created below. Any open
                // upvalues the callback captured — or that ride in its argument
                // data (e.g. a handler pulled from a map it iterates) — point
                // into the parked parent (HOF-invoking) VM's stack, not this
                // callback VM's. Close them to shared, still-live `Tracked` cells
                // against that parent VM first, mirroring `async/spawn`, so the
                // callback reads/writes the real cell (its `set!` write-back stays
                // visible to the defining frame) instead of dereferencing — or
                // silently clobbering — a foreign stack slot. This runs for EVERY
                // element dispatch (continuation-driven ones bypass the
                // structural-outcome seam), which is why it lives here.
                if let Some(parent_vm) = owner.parked_parent_vm_mut() {
                    snapshot_escaping_call_with_owner(parent_vm, &call.callable, &call.args);
                }
                let globals = closure
                    .globals
                    .clone()
                    .ok_or_else(|| RuntimeFault::Invariant {
                        message: "VM closure has no home environment".into(),
                    })?;
                let mut vm = VM::new_for_task_with_native_fns(globals, functions, native_fns);
                let frame = ContinuationFrame::vm_native(call.continuation);
                match vm.setup_for_call(closure, &call.args) {
                    Ok(()) => {
                        let mut state = self.state.borrow_mut();
                        let task = state.tasks.get_mut(&task_id).ok_or_else(|| {
                            RuntimeFault::Invariant {
                                message: "calling task disappeared".into(),
                            }
                        })?;
                        task.vm_call = Some(vm);
                        task.vm_owner = Some(ReturnOwner::Continuation(Box::new(owner), frame));
                        task.record
                            .yield_ready()
                            .map_err(|error| RuntimeFault::Invariant {
                                message: format!("VM callable failed to yield ready: {error:?}"),
                            })?;
                        let root = task.record.relations().origin_root;
                        state.ready.enqueue(root, task_id);
                        return Ok(());
                    }
                    Err(error) => (frame, Err(error)),
                }
            } else if let Some(native) = call.callable.as_native_fn_rc() {
                let _installed = eval_context.scope_task_context(context.clone());
                let mut task_context = context.borrow_mut();
                let mut native_context = NativeCallContext {
                    task_context: &mut task_context,
                    cancellation: CancellationView::new(
                        cancellation.is_some(),
                        cancellation.map(|request| request.reason),
                    ),
                };
                // Dispatch the native with the runtime-quantum flag active so a
                // callback native takes its cooperative path: a genuinely
                // driveable native (e.g. an agent tool that offloads I/O) RETURNS
                // its `NativeOutcome` (Suspend/Call) from `invoke_runtime`, which
                // we drive here; a *parking* native passed DIRECTLY as a HOF
                // callback (`(map channel/recv …)`) leaves a channel/promise/sleep
                // park yield that CANNOT suspend inside this Rust continuation, so
                // it is converted into the lambda-wrap guidance — parity with the
                // legacy `check_hof_yield`.
                let prev_q = sema_core::in_runtime_quantum();
                sema_core::set_runtime_quantum(true);
                let native_result =
                    native.invoke_runtime(&eval_context, &mut native_context, &call.args);
                sema_core::set_runtime_quantum(prev_q);
                let native_result = match sema_core::take_yield_signal() {
                    Some(_park) => Err(sema_core::SemaError::eval(
                        "yielding native passed directly to a higher-order function — \
                             wrap it in a lambda so the yield can suspend cleanly. \
                             For example, `(map (fn (x) (channel/recv x)) ...)` instead of \
                             `(map channel/recv ...)`.",
                    )),
                    None => native_result,
                };
                (ContinuationFrame::native(call.continuation), native_result)
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

    fn resume_continuation(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
        frame: ContinuationFrame,
        input: ResumeInput,
    ) -> Result<(), RuntimeFault> {
        let (context, cancellation) = {
            let state = self.state.borrow();
            let Some(task) = state.tasks.get(&task_id) else {
                return Err(RuntimeFault::Invariant {
                    message: "continuation task disappeared".into(),
                });
            };
            (task.context.clone(), task.record.cancellation())
        };
        let mut task_context = context.borrow_mut();
        let mut native_context = NativeCallContext {
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
        };
        self.state
            .borrow_mut()
            .pending
            .push_back(PendingStage::Apply(task_id, owner, resumed));
        Ok(())
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
            // them. See docs/plans/2026-07-13-unified-cooperative-runtime-task-04.md.
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
            sema_core::notify_task_reaped(task_id.get());
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

    /// Force-settle the requested `root` as `Failed` with a legacy-parity
    /// deadlock error. Called by the host drive loop when the runtime has gone
    /// fully idle — `DriveState::Idle { next_deadline: None,
    /// inbox_wakeup_required: false }` — yet
    /// the root is still `Running`: no task made progress this turn and there is
    /// no timer deadline nor pending external completion that could ever change
    /// that, so the root is parked on an intra-runtime wait (channel/promise)
    /// that nothing runnable can satisfy. That is a genuine deadlock.
    ///
    /// The error text mirrors what the legacy scheduler / synchronous channel
    /// ops produce, so `eval_str_via_runtime` matches the `eval_str` oracle:
    /// - root main task parked directly on `channel/recv` (top-level, no sender)
    ///   → "channel/recv: channel is empty";
    /// - root main task parked directly on `channel/send` (full, no receiver)
    ///   → "channel/send: channel is full";
    /// - otherwise (awaiting a never-settling promise, mutual await, a spawned
    ///   task that is itself blocked, …) → "async scheduler: all tasks blocked
    ///   (deadlock detected)". A channel op *inside* a spawn parks a child task,
    ///   leaving the root main task on a promise wait — so it falls here, exactly
    ///   like the legacy path where only a top-level (non-async) channel op errors
    ///   with the channel-specific message.
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
            let mut state = self.state.borrow_mut();
            match state.tasks.get(&main_task) {
                Some(task) if task.record.state_name() == super::StateName::Waiting => {}
                // The root main task is not parked (already resumed/settling): not
                // a deadlock this method can name. Let the caller decide.
                _ => return Ok(false),
            }
            // The root main task parked directly on a `channel/recv`/`channel/send`
            // is a `protocol_waits` Channel entry keyed by its wait key. Name the
            // deadlock with the legacy synchronous message (empty/full) so
            // `eval_str_via_runtime` matches the `eval_str` oracle.
            let channel_wait = state
                .tasks
                .get(&main_task)
                .and_then(|task| task.record.wait_key())
                .and_then(|key| match state.protocol_waits.get(&key) {
                    Some(ProtocolWait {
                        kind: ProtocolWaitKind::Channel { channel, receive },
                        ..
                    }) => Some((key, *channel, *receive)),
                    _ => None,
                });
            if let Some((key, channel, receive)) = channel_wait {
                // Deregister from the channel queue and drop the protocol wait so no
                // stale wait remains once the task is removed by `settle` below.
                let _ = state.channels.cancel_wait(channel, key);
                state.protocol_waits.remove(&key);
                if receive {
                    sema_core::SemaError::eval("channel/recv: channel is empty")
                } else {
                    sema_core::SemaError::eval("channel/send: channel is full").with_hint(
                        "Use async to run in an async context where send will yield until space is available",
                    )
                }
            } else {
                // Awaiting a promise (single or set) that can never settle — a
                // genuine cross-task deadlock. The main task's protocol wait is
                // dropped by `settle` below; any never-settling descendant tasks
                // stay parked but inert (they are Waiting, never Ready, so they
                // never re-enter the drive loop).
                sema_core::SemaError::eval("async scheduler: all tasks blocked (deadlock detected)")
            }
        };
        self.settle(root, main_task, TaskOutcome::Failed(error))?;
        Ok(true)
    }

    pub fn cancel_root(&self, root: RootId, reason: CancelReason) -> bool {
        let (task_id, newly) = {
            let mut state = self.state.borrow_mut();
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
            let task_id = match state.roots.get(&root).map(RootRecord::state) {
                Some(RootState::Running { main_task }) => *main_task,
                _ => return false,
            };
            let Some(task) = state.tasks.get_mut(&task_id) else {
                return false;
            };
            (task_id, task.record.request_cancellation(reason))
        };
        // Eagerly tear down an in-flight External/IO/ResourceSlot wait so a
        // root cancelled between drive turns aborts promptly (C2).
        if newly {
            let _ = deliver_cancel_teardown(&self.state, task_id);
        }
        newly
    }

    pub fn shutdown(&self, options: &ShutdownOptions) -> Result<ShutdownReport, RuntimeFault> {
        let mut terminal_fault = self.state.borrow().terminal_fault.clone();
        {
            let mut state = self.state.borrow_mut();
            state.shutting_down = true;
            for task in state.tasks.values_mut() {
                task.record
                    .request_cancellation(CancelReason::InterpreterShutdown);
            }
            // A cooperative debug session abandoned while paused would otherwise
            // freeze the drive loop (the barrier gates every source). Clear it and
            // re-enqueue the paused task so its shutdown cancellation settles it.
            if let Some((root, task_id, _)) = state.debug_paused.take() {
                state.ready.enqueue(root, task_id);
            }
        }
        loop {
            let state = match self.drive(&options.drive_budget) {
                Ok(state) => state,
                Err(fault) => {
                    terminal_fault.get_or_insert_with(|| fault.clone());
                    while matches!(self.cancel_waiting(), Ok(true)) {}
                    self.abort_terminal_state(&fault);
                    break;
                }
            };
            let now = self.state.borrow().clock.now();
            let cleanup_complete = {
                let state = self.state.borrow();
                state.tasks.is_empty()
                    && state
                        .waits
                        .as_ref()
                        .is_none_or(|waits| waits.active_len() == 0 && waits.cleanup_len() == 0)
            };
            if cleanup_complete
                || now >= options.deadline
                || !matches!(state, DriveState::Progress { .. })
            {
                break;
            }
        }
        let state = self.state.borrow();
        let active_waits = state.waits.as_ref().map_or(0, WaitRuntime::active_len);
        let retained_cleanup = state.waits.as_ref().map_or(0, WaitRuntime::cleanup_len);
        let cleanup_diagnostics = state.waits.as_ref().map_or_else(Vec::new, |waits| {
            waits.cleanup_diagnostics_at(state.clock.now())
        });
        let invariant_failures = cleanup_diagnostics
            .iter()
            .cloned()
            .map(|diagnostic| ShutdownInvariantFailure {
                name: "retained-cleanup",
                diagnostic,
            })
            .collect();
        let live_tasks = state.tasks.len();
        let mut report = ShutdownReport {
            clean: live_tasks == 0 && active_waits == 0 && retained_cleanup == 0,
            live_roots: state.roots.len(),
            live_tasks,
            active_waits,
            retained_cleanup,
            executor: None,
            cleanup_diagnostics,
            invariant_failures,
        };
        drop(state);
        let lease = self
            .state
            .borrow_mut()
            .waits
            .as_mut()
            .and_then(WaitRuntime::take_lease);
        if let Some(lease) = lease {
            report.executor = Some(lease.shutdown(options.deadline));
        }
        if matches!(report.executor, Some(ExecutorShutdown::DeadlineExceeded(_))) {
            report.clean = false;
        }
        if let Some(waits) = self.state.borrow_mut().waits.as_mut() {
            waits.close_inbox();
        }
        terminal_fault.map_or(Ok(report), Err)
    }

    fn abort_terminal_state(&self, fault: &RuntimeFault) {
        let (pending, protocol_waits, tasks) = {
            let mut state = self.state.borrow_mut();
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

    pub fn close_for_interpreter_drop(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.shutting_down = true;
            for task in state.tasks.values_mut() {
                task.record
                    .request_cancellation(CancelReason::InterpreterShutdown);
            }
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
            state: Rc::clone(&self.state),
        }
    }
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
fn deliver_cancel_teardown(
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
                    // A bare `Timer`/`VmSleep` key (in `timers` only): self-resolving,
                    // left to the scan.
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
    let Some(wait) = state.protocol_waits.remove(&key) else {
        return Ok(false);
    };
    let queued = state
        .resource_gates
        .cancel_wait(gate, key)
        .map_err(|error| RuntimeFault::Invariant {
            message: format!("resource slot cancel failed: {error:?}"),
        })?;
    if !queued {
        // Not in the queue: this task was granted the slot (it owns the gate) but
        // has not run its acquire continuation. Release the gate for it and drop
        // any buffered grant wake keyed to it.
        let _ = state.resource_gates.take_wake(key);
        release_owned_gate(&mut state, gate)?;
    }
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
    let mut unique = std::collections::HashSet::new();
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
/// `Returned(nil)` once the deadline elapses. Mirrors the `VmSleep` timer
/// registration, but delivers through a `NativeContinuation` rather than a
/// parked VM.
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

/// Walk `node`'s spawn-parent chain (`CancellationParent::Task`); true if
/// `ancestor` appears — i.e. `node` was spawned (transitively) by `ancestor`.
/// The spawn graph is a tree (a child's parent always predates it), so the walk
/// terminates at a `Root`/`Scope`/`None` parent or a missing task.
fn is_task_descendant_of(state: &RuntimeState, mut node: TaskId, ancestor: TaskId) -> bool {
    while let Some(task) = state.tasks.get(&node) {
        match task.record.relations().cancellation_parent {
            CancellationParent::Task(parent) => {
                if parent == ancestor {
                    return true;
                }
                node = parent;
            }
            _ => return false,
        }
    }
    false
}

/// Whether an `async/run` barrier for `caller` (whose origin root is `root`) may
/// release: true when no OTHER task under `root` is Ready, Running, or parked on
/// a SELF-RESOLVING wait. See [`Runtime::resolve_origin_barriers`] for the full
/// classification and rationale (the Reviewer-2 hole: `ResourceSlot` MUST be
/// cycle-forming, or the barrier hangs on a resource cycle).
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
                    // A NESTED barrier under a descendant of `caller` is a
                    // sub-graph that releases on its own, so the ancestor
                    // barrier must WAIT for it — otherwise both barriers'
                    // predicates hold at once when the shared descendant work
                    // settles, `resolve_origin_barriers` picks one by HashMap
                    // order, and if the ancestor wins its root settles and the
                    // inner task's continuation is silently dropped. A
                    // genuinely-independent barrier (neither task descends from
                    // the other) stays cycle-forming/excluded, so mutually-
                    // waiting barriers still can't deadlock each other.
                    Some(ProtocolWaitKind::OriginBarrier { .. }) => {
                        is_task_descendant_of(state, *id, caller)
                    }
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

fn install_channel_wait(
    state: &mut RuntimeState,
    task_id: TaskId,
    key: super::WaitKey,
    wait: sema_core::runtime::ChannelWait,
    owner: ReturnOwner,
    frame: ContinuationFrame,
) -> Result<(), ProtocolInstallError> {
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
        Err(error) => return Err(Box::new((owner, frame, registry_error(error)))),
    };
    if let Some(wake) = state.channels.pop_wake() {
        state.pending.push_back(PendingStage::ChannelWake(wake));
    }
    if result == super::ChannelResult::Waiting {
        if let Err(error) = state
            .tasks
            .get_mut(&task_id)
            .expect("protocol task exists")
            .record
            .wait(key)
        {
            let _ = state.channels.cancel_wait(channel, key);
            return Err(Box::new((
                owner,
                frame,
                sema_core::SemaError::eval(format!("channel wait transition failed: {error:?}")),
            )));
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
        return Ok(());
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
    state.pending.push_back(PendingStage::ApplyRuntimeResponse(
        task_id,
        owner,
        frame,
        Ok(response),
    ));
    Ok(())
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
    }
}

impl RootHandle {
    pub fn id(&self) -> RootId {
        self.id
    }

    pub fn poll_result(&self) -> RootPoll {
        let Some(runtime) = self.runtime.upgrade() else {
            return RootPoll::RuntimeDropped;
        };
        let state = runtime.borrow();
        match state.roots.get(&self.id).map(RootRecord::state) {
            Some(RootState::Settled(settlement)) => RootPoll::Ready(Rc::clone(settlement)),
            Some(RootState::Running { .. }) => RootPoll::Pending,
            Some(RootState::Aborted) => state
                .terminal_fault
                .clone()
                .map_or(RootPoll::InvariantViolation, RootPoll::Aborted),
            None => RootPoll::InvariantViolation,
        }
    }

    #[cfg(test)]
    pub(super) fn main_task_for_test(&self) -> Option<TaskId> {
        let runtime = self.runtime.upgrade()?;
        let state = runtime.borrow();
        match state.roots.get(&self.id).map(RootRecord::state) {
            Some(RootState::Running { main_task }) => Some(*main_task),
            _ => None,
        }
    }

    pub fn cancel(&self, reason: CancelReason) -> bool {
        let Some(runtime) = self.runtime.upgrade() else {
            return false;
        };
        let (task_id, newly) = {
            let mut state = runtime.borrow_mut();
            let task_id = match state.roots.get(&self.id).map(RootRecord::state) {
                Some(RootState::Running { main_task }) => *main_task,
                _ => return false,
            };
            let newly = state
                .tasks
                .get_mut(&task_id)
                .is_some_and(|task| task.record.request_cancellation(reason));
            (task_id, newly)
        };
        // Eagerly deliver in-flight wait teardown so a root cancelled between
        // drive turns (e.g. host Ctrl-C) aborts its offloaded op promptly (C2).
        if newly {
            let _ = deliver_cancel_teardown(&runtime, task_id);
        }
        newly
    }
}

impl Clone for RootHandle {
    fn clone(&self) -> Self {
        if let Some(runtime) = self.runtime.upgrade() {
            let mut state = runtime.borrow_mut();
            let root = state
                .roots
                .get_mut(&self.id)
                .expect("live root handle must reference a registered root");
            assert!(root.retain_handle(), "root handle count overflow");
        }
        Self {
            runtime: self.runtime.clone(),
            id: self.id,
        }
    }
}

impl Drop for RootHandle {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.upgrade() else {
            return;
        };
        let mut state = runtime.borrow_mut();
        let Some(root) = state.roots.get_mut(&self.id) else {
            return;
        };
        root.release_handle();
        if root.is_reap_eligible() {
            state.handle_cleanup.push_back(self.id);
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
    /// A VM root/child parked on `async/sleep`: arm a runtime timer for `ms`
    /// milliseconds and leave the VM in `vm_call` so `fire_timer` re-runs it.
    VmSleep(TaskId, u64),
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

enum ReturnOwner {
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
            Self::VmSleep(_, _) => true,
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
        sema_core::set_usage_scope_task_callbacks(usage_capture, usage_take, usage_install);
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
        sema_core::set_usage_scope_task_callbacks(usage_capture, usage_take, usage_install);
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
}
