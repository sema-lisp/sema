//! Interpreter-owned runtime state and root lifecycle.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Instant;

use crate::{extract_vm_closure, VmExecResult, VM};
#[cfg(test)]
use sema_core::runtime::ExternalFailure;
use sema_core::runtime::{
    CancelReason, CancellationView, ExecutorShutdown, IdCounter, IoExecutor, NativeCall,
    NativeCallContext, NativeOutcome, NativeResult, ResumeInput, RootId, RuntimeRequest,
    RuntimeResponse, RuntimeScopedIdCounter, SettlementSeq, TaskContextHandle, TaskId, TaskOutcome,
    TaskSettlement, Trace,
};
#[cfg(test)]
use sema_core::runtime::{CancellationParent, LifetimeOwner, TaskRelations};
use sema_core::EvalContext;
#[cfg(test)]
use sema_core::Value;

use super::channel::ChannelClose;
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
}

// Task 4 replaces this placeholder with the VM-backed PreparedRoot payload.
#[cfg_attr(not(test), allow(dead_code))]
enum TaskPayload {
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
    drive_cursor: usize,
    drive_active: bool,
    active_instruction_limit: usize,
    turn_instructions: usize,
    shutting_down: bool,
    terminal_fault: Option<RuntimeFault>,
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

impl Trace for RuntimeState {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.waits.as_ref().is_none_or(|waits| waits.trace(sink))
            && self.roots.values().all(|root| root.trace(sink))
            && self.tasks.values().all(|task| task.trace(sink))
            && self.promises.trace(sink)
            && self.channels.trace(sink)
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
    }
}

impl Trace for TaskPayload {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        #[cfg(not(test))]
        let _ = sink;
        match self {
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
                drive_cursor: 0,
                drive_active: false,
                active_instruction_limit: usize::MAX,
                turn_instructions: 0,
                shutting_down: false,
                terminal_fault: None,
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
            },
        );
        state.ready.enqueue(root, task);
        Ok(RootHandle {
            runtime: Rc::downgrade(&self.state),
            id: root,
        })
    }

    pub fn drive(&self, budget: &DriveBudget) -> Result<DriveState, RuntimeFault> {
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
            let unvisited_reserved = reserved_roots - root_visits;
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
                legacy_io_wakeup_required: false,
            })
        }
    }

    fn fire_timer(&self) -> Result<bool, RuntimeFault> {
        let mut state = self.state.borrow_mut();
        let now = state.clock.now();
        let Some(key) = state.timers.pop_due(now) else {
            return Ok(false);
        };
        let task_id = state
            .tasks
            .iter()
            .find_map(|(id, task)| (task.record.wait_key() == Some(key)).then_some(*id))
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "timer referenced missing waiting task".into(),
            })?;
        let root = state.tasks[&task_id].record.relations().origin_root;
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
        }
        Ok(true)
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
                let owner = self
                    .state
                    .borrow_mut()
                    .tasks
                    .get_mut(&task)
                    .and_then(|task| task.suspended_owner.take())
                    .ok_or_else(|| RuntimeFault::Invariant {
                        message: "resumed task has no installed return owner".into(),
                    })?;
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
                wakes.pop_front();
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
                    self.state.borrow_mut().channels.emit_wake(wake);
                }
                if !close.is_empty() {
                    self.state
                        .borrow_mut()
                        .pending
                        .push_back(PendingStage::ChannelClose(close));
                }
                return Ok(true);
            }
        };
        self.state.borrow_mut().pending.push_back(next);
        Ok(true)
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
        let action = if let Some(pending) = task.pending_resume.take() {
            TaskAction::Resume(pending)
        } else if let Some(cancel) = task.record.cancellation() {
            task.vm_call.take();
            match task.vm_owner.take() {
                Some(owner) => TaskAction::Cancel(task_id, owner, cancel.reason),
                None => TaskAction::Settle(root, task_id, TaskOutcome::Cancelled(cancel.reason)),
            }
        } else if let Some(mut vm) = task.vm_call.take() {
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
            let quantum = vm.run_quantum(&context, instruction_limit);
            drop(quantum_guard);
            self.state.borrow_mut().turn_instructions += quantum.instructions;
            match quantum.outcome {
                Ok(VmExecResult::QuantumExpired { .. }) => {
                    task.vm_call = Some(vm);
                    TaskAction::Yield(root, task_id)
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
        } else {
            match &mut task.payload {
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
                    self.settle(root, task_id, TaskOutcome::Cancelled(reason))?
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
            (owner, result) => return self.apply_native_outcome(task_id, owner, result),
        }
        Ok(())
    }

    fn apply_native_outcome(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
        result: NativeResult,
    ) -> Result<(), RuntimeFault> {
        let root = self
            .state
            .borrow()
            .tasks
            .get(&task_id)
            .map(|task| task.record.relations().origin_root)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "native result task disappeared".into(),
            })?;
        match result {
            Ok(NativeOutcome::Return(value)) => {
                debug_assert!(matches!(owner, ReturnOwner::Root));
                self.settle(root, task_id, TaskOutcome::Returned(value))
            }
            Err(error) => {
                debug_assert!(matches!(owner, ReturnOwner::Root));
                self.settle(root, task_id, TaskOutcome::Failed(error))
            }
            Ok(NativeOutcome::Suspend(suspend)) => {
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

    fn dispatch_runtime(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
        request: RuntimeRequest,
    ) -> Result<(), RuntimeFault> {
        if let RuntimeRequest::Spawn {
            callable,
            continuation,
        } = request
        {
            let response = Err(sema_core::SemaError::eval(
                "runtime spawn admission is unavailable",
            ));
            drop(callable);
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
        let (continuation, response) = {
            let mut state = self.state.borrow_mut();
            match request {
                RuntimeRequest::Spawn { .. } => unreachable!("spawn extracted before borrow"),
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
                RuntimeRequest::CreateSettledPromise {
                    outcome,
                    continuation,
                } => {
                    let response = (|| {
                        #[cfg(test)]
                        if state.force_promise_exhaustion {
                            return Err(sema_core::SemaError::eval(
                                "runtime promise identity exhausted",
                            ));
                        }
                        let promise = state.promises.reserve_id().map_err(|_| {
                            sema_core::SemaError::eval("runtime promise identity exhausted")
                        })?;
                        #[cfg(test)]
                        let settlement_exhausted = state.force_settlement_exhaustion;
                        #[cfg(not(test))]
                        let settlement_exhausted = false;
                        let sequence = (!settlement_exhausted)
                            .then(|| state.settlement_ids.allocate())
                            .transpose()
                            .ok()
                            .flatten()
                            .ok_or_else(|| {
                                let fault = RuntimeFault::IdExhausted { kind: "settlement" };
                                state.shutting_down = true;
                                state.terminal_fault = Some(fault);
                                sema_core::SemaError::eval("runtime settlement identity exhausted")
                            })?;
                        let settlement = Rc::new(TaskSettlement { sequence, outcome });
                        state.promises.insert_pending(promise, None);
                        let wakes = state
                            .promises
                            .settle(promise, settlement)
                            .map_err(registry_error)?;
                        if !wakes.is_empty() {
                            state.pending.push_back(PendingStage::PromiseWakes(wakes));
                        }
                        Ok(RuntimeResponse::Promise(promise))
                    })();
                    (continuation, response)
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
                RuntimeRequest::OriginBarrier { continuation } => (
                    continuation,
                    Ok(RuntimeResponse::Value(sema_core::Value::NIL)),
                ),
            }
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
        self.state
            .borrow()
            .terminal_fault
            .clone()
            .map_or(Ok(()), Err)
    }

    fn invoke_callable(
        &self,
        task_id: TaskId,
        owner: ReturnOwner,
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
                (
                    ContinuationFrame::native(call.continuation),
                    native.invoke_runtime(&eval_context, &mut native_context, &call.args),
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
        let (pending, tasks) = {
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
            (pending, std::mem::take(&mut state.tasks))
        };
        drop(pending);
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

fn registry_error(error: super::RegistryError) -> sema_core::SemaError {
    sema_core::SemaError::eval(match error {
        super::RegistryError::WrongRuntime => "runtime handle belongs to another runtime",
        super::RegistryError::Unknown => "runtime handle is stale or unknown",
        super::RegistryError::AlreadySettled => "promise is already settled",
        super::RegistryError::DuplicateWait => "runtime wait is already registered",
        super::RegistryError::IdExhausted => "runtime identity exhausted",
    })
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
}

enum ReturnOwner {
    Root,
    Continuation(Box<ReturnOwner>, ContinuationFrame),
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
        }
    }
}

impl Trace for ReturnOwner {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Root => true,
            Self::Continuation(parent, frame) => parent.trace(sink) && frame.trace(sink),
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
