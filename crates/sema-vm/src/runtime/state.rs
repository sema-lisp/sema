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
    NativeCallContext, NativeOutcome, NativeResult, ResumeInput, RootId, RuntimeRequest,
    RuntimeResponse, RuntimeScopedIdCounter, SettlementSeq, TaskContextHandle, TaskId, TaskOutcome,
    TaskSettlement, Trace, WaitKind,
};
use sema_core::runtime::{CancellationParent, LifetimeOwner, TaskRelations};
use sema_core::EvalContext;
#[cfg(test)]
use sema_core::Value;
use sema_core::YieldReason;

use super::channel::{ChannelClose, ChannelWake};
use super::{
    ChannelRegistry, ContinuationFrame, DriveBudget, DriveState, PendingResume, PromiseRegistry,
    PromiseState, ReadyScheduler, RegisterExternalError, RootRecord, RootState, RuntimeClock,
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
    /// The per-task LLM dynamic scope (`llm/with-cache` / `with-budget` state),
    /// captured from the spawner's thread-locals at `async/spawn` and installed
    /// around each of this task's quanta (restored after). `None` for the root
    /// task, which runs directly against the process thread-locals. The scope is a
    /// type-erased `sema-llm` struct reached through the registered scope seam; it
    /// carries only scalar state + a shared budget `Rc` (no GC-traceable `Value`),
    /// so it needs no trace edge.
    llm_scope: Option<Box<dyn std::any::Any>>,
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
    roots: HashMap<RootId, RootRecord>,
    tasks: HashMap<TaskId, RuntimeTask>,
    ready: ReadyScheduler,
    timers: TimerQueue,
    handle_cleanup: VecDeque<RootId>,
    pending: VecDeque<PendingStage>,
    protocol_waits: HashMap<super::WaitKey, ProtocolWait>,
    task_promises: HashMap<TaskId, sema_core::runtime::PromiseId>,
    /// VM-quantum tasks parked on a LEGACY `AwaitIo(IoHandle)` yield, mapped to
    /// their internal wait key and the poll handle. This is the
    /// `LegacyAwaitIoBridge`: every offloaded-I/O async op (llm/complete, embed,
    /// http, file, event/select, io) that arms an `IoHandle` and yields
    /// `AwaitIo` parks HERE instead of blocking the VM thread. The handle is
    /// NOT `Send` (it polls VM-thread completion state), so `poll_io_waits`
    /// drives it ON THE VM THREAD during drive turns; the offloaded job itself
    /// already runs on the IO pool. The key is minted by `issue_internal_wait`
    /// and is NEVER inserted into `WaitRuntime::active` (it is polled, not
    /// delivered via the completion inbox).
    io_waits: HashMap<TaskId, (super::WaitKey, Rc<sema_core::IoHandle>)>,
    drive_cursor: usize,
    drive_active: bool,
    active_instruction_limit: usize,
    turn_instructions: usize,
    shutting_down: bool,
    terminal_fault: Option<RuntimeFault>,
    // Diagnostic: protocol completions that carried an undelivered value but
    // arrived after their wait was gone. A nonzero count is a lost-message bug.
    dropped_protocol_completions: usize,
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

impl Runtime {
    pub fn new(
        context: Rc<EvalContext>,
        clock: Rc<dyn RuntimeClock>,
        executor: Arc<dyn IoExecutor>,
    ) -> Result<Self, RuntimeCreateError> {
        let (waits, issuers) = WaitRuntime::new_with_issuers(executor)?;
        let runtime_id = waits.runtime_id();
        let (root_ids, promise_ids, channel_ids) = issuers.into_parts();
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
                roots: HashMap::new(),
                tasks: HashMap::new(),
                ready: ReadyScheduler::new(),
                timers: TimerQueue::new(),
                handle_cleanup: VecDeque::new(),
                pending: VecDeque::new(),
                protocol_waits: HashMap::new(),
                task_promises: HashMap::new(),
                io_waits: HashMap::new(),
                drive_cursor: 0,
                drive_active: false,
                active_instruction_limit: usize::MAX,
                turn_instructions: 0,
                shutting_down: false,
                terminal_fault: None,
                dropped_protocol_completions: 0,
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
                llm_scope: None,
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
                llm_scope: None,
            },
        );
        state.ready.enqueue(root, task);
        Ok(RootHandle {
            runtime: Rc::downgrade(&self.state),
            id: root,
        })
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

    pub fn drive(&self, budget: &DriveBudget) -> Result<DriveState, RuntimeFault> {
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
            // Drive parked legacy-`AwaitIo` handles to completion ON THE VM
            // THREAD (they are not `Send`). A handle that lands wakes its task
            // (enqueued to `ready`, resumed via the source rotation below).
            if self.poll_io_waits()? {
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
            && state.io_waits.is_empty()
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
                // A task is parked on a legacy `AwaitIo` handle whose offloaded
                // job runs on the IO pool: the host must park the VM thread on
                // the IO-completion signal and re-drive so `poll_io_waits` lands
                // the result. Its key is not in `active`, so `inbox_wakeup` alone
                // never covers it.
                legacy_io_wakeup_required: !state.io_waits.is_empty(),
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
            Some(ProtocolWaitKind::Channel { .. }) | None => None,
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
                (task.record.cancellation().is_some() && state.protocol_waits.contains_key(&key))
                    .then_some((*id, key))
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
                        // TODO(UCR-3): if cancel_wait returns None the receiver/sender
                        // was already rendezvous-matched (its wake is in flight), so
                        // cancel-and-drop here can lose a committed value. Fix is to
                        // skip selecting such a wait (ChannelRegistry::has_wait) and let
                        // the wake deliver. Currently guarded by the
                        // dropped_protocol_completions diagnostic; not yet reproducible
                        // by hand. See docs/bugs/ucr-3-channel-rendezvous-cancel-drop.md.
                        let _ = state.channels.cancel_wait(*channel, key);
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
        {
            // A VM task parked on a legacy `AwaitIo` handle. It is tracked ONLY in
            // `io_waits` with an internal key never in
            // `WaitRuntime::active`, so the generic fallback would spin without
            // waking it. Handle it here: run the handle's abort hook (aborting the
            // offloaded future so an in-flight request/child is torn down where the
            // runtime supports it — see `IoHandle::with_abort`), drop the entry,
            // and wake it so the cancellation settles it Cancelled on its next
            // visit.
            let mut state = self.state.borrow_mut();
            let selected = state.tasks.iter().find_map(|(id, task)| {
                let key = task.record.wait_key()?;
                (task.record.cancellation().is_some()
                    && state.io_waits.get(id).map(|(k, _)| *k) == Some(key))
                .then_some((*id, key))
            });
            if let Some((task_id, key)) = selected {
                if let Some((_, handle)) = state.io_waits.remove(&task_id) {
                    handle.abort();
                }
                let task = state
                    .tasks
                    .get_mut(&task_id)
                    .expect("selected io-waiting task exists");
                task.record
                    .wake(key)
                    .map_err(|error| RuntimeFault::Invariant {
                        message: format!("cancelled io-wait task failed to wake: {error:?}"),
                    })?;
                let root = task.record.relations().origin_root;
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
        // Install this task's captured LLM dynamic scope (cache/budget/…) around the
        // quantum so a completion or fan-out spawned inside a `with-cache`/`with-budget`
        // extent sees it even after the wrapper's thunk (and its thread-local scope)
        // has ended. The quantum's own mutations persist back onto the task; the prior
        // (spawner/global) scope is restored afterwards. The root task carries no
        // captured scope and runs directly against the process thread-locals.
        let displaced_scope = task.llm_scope.take().map(sema_core::install_task_llm_scope);
        // Publish the running task's identity so natives that open a per-task slab
        // entry (`llm/stream`, `agent/run`) record the owning task, letting the
        // task-reaped sweep reclaim the entry (and its detached span) when the task
        // is cancelled mid-flight.
        let prev_task_id = sema_core::set_current_task_id(Some(task_id.get()));
        let quantum = vm.run_quantum(&context, instruction_limit, cancellation);
        let _ = sema_core::set_current_task_id(prev_task_id);
        if let Some(displaced) = displaced_scope {
            task.llm_scope = Some(sema_core::take_task_llm_scope());
            let _ = sema_core::install_task_llm_scope(displaced);
        }
        drop(quantum_guard);
        self.state.borrow_mut().turn_instructions += quantum.instructions;
        let action = match quantum.outcome {
            Ok(VmExecResult::QuantumExpired { .. }) => {
                task.vm_call = Some(vm);
                TaskAction::Yield(root, task_id)
            }
            // A native yielded the VM through the TLS yield signal (surfaced as
            // `AsyncYield`). The VM has already parked its frame (pc past the
            // call, a nil placeholder on the stack top) and stays in `vm_call`;
            // the runtime registers a native wait and, when it fires, re-runs
            // `run_quantum` — the frame resumes in place with the placeholder as
            // the resume value.
            Ok(VmExecResult::AsyncYield(reason)) => match reason {
                YieldReason::Sleep(ms) => {
                    task.vm_call = Some(vm);
                    TaskAction::VmSleep(task_id, ms)
                }
                // The promise ops (`async/spawn`, `async/await`, `async/cancel`,
                // `async/all`, `async/race`, `async/timeout`, the predicates,
                // `async/run`) and the channel ops (`channel/new`, `send`, `recv`,
                // `try-recv`, `close`, `closed?`, `count`, `empty?`, `full?`) no
                // longer use the TLS yield signal — they suspend structurally
                // through the `NativeOutcome` ABI (`Suspend`/`Runtime`), handled by
                // the `Pending` arm below. Reaching here with a legacy
                // promise/channel yield would be a routing bug: surface it as an
                // error rather than driving the retired bridge.
                YieldReason::Spawn(_)
                | YieldReason::Cancel(_)
                | YieldReason::AwaitPromise(_)
                | YieldReason::AwaitPromiseSet { .. }
                | YieldReason::ChannelSend(_, _)
                | YieldReason::ChannelRecv(_)
                | YieldReason::ChannelClose(_)
                | YieldReason::ChannelInspect(_, _)
                | YieldReason::ChannelTryRecv(_) => TaskAction::VmResult(
                    task_id,
                    task.vm_owner.take().expect("VM call has a return owner"),
                    Err(sema_core::SemaError::eval(
                        "legacy promise/channel yield reached the unified runtime (async ops are structural)",
                    )),
                ),
                // Legacy offloaded I/O (`AwaitIo`): an async op (llm/complete,
                // embed, http, file, event/select, io) offloaded its work to the
                // IO pool and armed a poll handle. Park this frame with a nil
                // placeholder on its stack top; the runtime drives the handle to
                // completion ON THE VM THREAD (`poll_io_waits`) and resumes the
                // frame with the decoded value (or raises the error in-frame).
                YieldReason::AwaitIo(handle) => {
                    task.vm_call = Some(vm);
                    TaskAction::VmAwaitIo(task_id, handle)
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
        self.state
            .borrow_mut()
            .pending
            .push_back(PendingStage::Action(action));
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
            TaskAction::VmAwaitIo(task_id, handle) => self.await_io(task_id, handle)?,
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
        // `async/run` (OriginBarrier): drain the currently-ready work of this
        // task's origin-root to a quiescent point, then resume with nil. Realized
        // as a zero-duration timer suspension: the virtual-clock rule in
        // `fire_timer` only fires the deadline once no task is Ready and nothing
        // is pending, so every ready sibling runs first. This is the CLOSEST safe
        // behaviour, not the plan's full transitive settle-barrier: a naive
        // barrier can deadlock on self-await and channel-rendezvous cycles, which
        // is worse than deferring a timer-blocked descendant's side effects. The
        // contract-vs-implementation gap and the deadlock hazards are documented
        // as ASYNC-RUN-BARRIER-1 in docs/deferred.md (a plan-owner decision).
        if let RuntimeRequest::OriginBarrier { continuation } = request {
            return self.install_protocol_suspend(
                task_id,
                owner,
                sema_core::runtime::NativeSuspend {
                    wait: WaitKind::Timer(Duration::ZERO),
                    continuation,
                },
            );
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
                            RuntimeResponse::Cancelled(
                                target
                                    .and_then(|target| state.tasks.get_mut(&target))
                                    .is_some_and(|task| {
                                        task.record.request_cancellation(CancelReason::Explicit)
                                    }),
                            )
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
            // A detached task is an origin-root child owned by the root, but not
            // the root's main task; it settles its own promise.
            let relations = TaskRelations {
                origin_root: root,
                cancellation_parent: CancellationParent::Root(root),
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
                    // Seed the child with a snapshot of the spawner's LLM dynamic
                    // scope (cache/budget/rate-limit/fallback state), read from the
                    // thread-locals the spawner is still running under. A concurrent
                    // fan-out spawned inside one `llm/with-budget` therefore captures
                    // the shared budget `Rc` and charges it as one aggregate, and an
                    // `llm/with-cache` completion deferred past the thunk's extent
                    // still sees the cache enabled — the scope rides the task, not
                    // the (already-restored) global.
                    llm_scope: Some(sema_core::current_llm_scope_boxed()),
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

    /// Park a VM task on a legacy `AwaitIo` handle (the `LegacyAwaitIoBridge`).
    /// The offloaded job already runs on the IO pool; here we poll the handle
    /// ONCE (it may already be done — a cached completion, a fast checkout) and,
    /// if still pending, register an internal wait and record the handle so
    /// `poll_io_waits` drives it to completion on the VM thread. A ready result
    /// resumes the frame in place with the decoded value, or raises the error at
    /// the parked call site (so an enclosing try/catch can catch it — matching
    /// the op's synchronous `Err` return).
    fn await_io(
        &self,
        task_id: TaskId,
        handle: Rc<sema_core::IoHandle>,
    ) -> Result<(), RuntimeFault> {
        // Poll OUTSIDE any `state` borrow: the poller closure runs post-call work
        // (decode, cache write, usage accounting) that may allocate GC values,
        // and GC tracing re-borrows the runtime state.
        match handle.poll() {
            sema_core::IoPoll::Ready(Ok(value)) => {
                return self.resume_running_vm(task_id, VmResume::Value(value));
            }
            sema_core::IoPoll::Ready(Err(msg)) => {
                return self.resume_running_vm(task_id, VmResume::Fail(await_io_error(msg)));
            }
            sema_core::IoPoll::Pending => {}
        }
        let mut state = self.state.borrow_mut();
        let key = state
            .waits
            .as_ref()
            .expect("wait runtime installed")
            .issue_internal_wait()
            .map_err(|_| RuntimeFault::IdExhausted { kind: "wait" })?;
        state
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "awaiting-io VM task disappeared".into(),
            })?
            .record
            .wait(key)
            .map_err(|error| RuntimeFault::Invariant {
                message: format!("awaiting-io VM task failed to wait: {error:?}"),
            })?;
        state.io_waits.insert(task_id, (key, handle));
        Ok(())
    }

    /// Drive every parked `AwaitIo` handle ON THE VM THREAD (the handles are not
    /// `Send`, so they cannot be polled on a worker). A handle that reports
    /// `Ready` wakes its task and injects the decoded value (or raises the error
    /// in-frame). Returns whether any task was woken. Bounded by the number of
    /// parked handles; the poll closures are non-blocking (`try_recv`).
    fn poll_io_waits(&self) -> Result<bool, RuntimeFault> {
        // Snapshot the parked handles (cloning the `Rc`s) and DROP the borrow
        // before polling: a poller may allocate GC values whose tracing
        // re-borrows the runtime state.
        let parked: Vec<(TaskId, super::WaitKey, Rc<sema_core::IoHandle>)> = {
            let state = self.state.borrow();
            if state.io_waits.is_empty() {
                return Ok(false);
            }
            state
                .io_waits
                .iter()
                .map(|(task_id, (key, handle))| (*task_id, *key, Rc::clone(handle)))
                .collect()
        };
        let mut ready: Vec<(TaskId, super::WaitKey, VmResume)> = Vec::new();
        for (task_id, key, handle) in parked {
            match handle.poll() {
                sema_core::IoPoll::Pending => {}
                sema_core::IoPoll::Ready(Ok(value)) => {
                    ready.push((task_id, key, VmResume::Value(value)));
                }
                sema_core::IoPoll::Ready(Err(msg)) => {
                    ready.push((task_id, key, VmResume::Fail(await_io_error(msg))));
                }
            }
        }
        if ready.is_empty() {
            return Ok(false);
        }
        let mut state = self.state.borrow_mut();
        for (task_id, key, resume) in ready {
            // The task may have been cancelled (and its io_wait aborted+dropped)
            // between the snapshot poll and here — skip it if the entry is gone.
            if state.io_waits.remove(&task_id).is_none() {
                continue;
            }
            let task = state
                .tasks
                .get_mut(&task_id)
                .ok_or_else(|| RuntimeFault::Invariant {
                    message: "io-wait task disappeared before wake".into(),
                })?;
            task.record
                .wake(key)
                .map_err(|error| RuntimeFault::Invariant {
                    message: format!("io-wait task failed to wake: {error:?}"),
                })?;
            task.vm_resume = Some(resume);
            let root = task.record.relations().origin_root;
            state.ready.enqueue(root, task_id);
        }
        Ok(true)
    }

    /// Move a Running VM task back to Ready, stamping the resume to apply on its
    /// next visit (a stack-top value injection, or a failure that settles it).
    fn resume_running_vm(&self, task_id: TaskId, resume: VmResume) -> Result<(), RuntimeFault> {
        let mut state = self.state.borrow_mut();
        let task = state
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "resuming VM task disappeared".into(),
            })?;
        task.vm_resume = Some(resume);
        let root = task.record.relations().origin_root;
        task.record
            .yield_ready()
            .map_err(|error| RuntimeFault::Invariant {
                message: format!("resuming VM task failed to yield ready: {error:?}"),
            })?;
        state.ready.enqueue(root, task_id);
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
    /// inbox_wakeup_required: false, legacy_io_wakeup_required: false }` — yet
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
        task.record.request_cancellation(reason)
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

/// Format a spawned-task rejection as an `async/await` error, stripping any
/// already-present prefix so chained awaits don't nest it. Mirrors the
/// stdlib `async/await` rejection formatting.
/// The error raised in-frame when a legacy `AwaitIo` poll reports `Ready(Err)`.
/// The offloaded op's poller already formatted the failure into a message
/// (identical to what the op's SYNCHRONOUS path stringifies), so surface it as a
/// plain catchable eval error — mirroring the legacy scheduler, which rejects the
/// task's promise with this same message string.
fn await_io_error(message: String) -> sema_core::SemaError {
    sema_core::SemaError::eval(message)
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

    pub fn cancel(&self, reason: CancelReason) -> bool {
        let Some(runtime) = self.runtime.upgrade() else {
            return false;
        };
        let mut state = runtime.borrow_mut();
        let task_id = match state.roots.get(&self.id).map(RootRecord::state) {
            Some(RootState::Running { main_task }) => *main_task,
            _ => return false,
        };
        state
            .tasks
            .get_mut(&task_id)
            .is_some_and(|task| task.record.request_cancellation(reason))
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
    /// Legacy `AwaitIo(IoHandle)`: park the task (`TaskId`) on the offloaded-I/O
    /// poll handle until it reports Ready, then resume the frame with the decoded
    /// value (or raise the error in-frame). The `LegacyAwaitIoBridge`.
    VmAwaitIo(TaskId, Rc<sema_core::IoHandle>),
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
            // An `IoHandle` is a boxed `FnMut` poller; it is not a GC candidate
            // and holds no live Sema `Value` (the result Value does not exist
            // until the offloaded job completes and the poller decodes it), so
            // there is no edge to trace — mirroring the legacy scheduler, which
            // also never traces a `Blocked(AwaitIo)` handle's internals.
            Self::VmAwaitIo(_, _) => true,
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
