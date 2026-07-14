//! Interpreter-owned runtime state and root lifecycle.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Instant;

#[cfg(test)]
use sema_core::runtime::ExternalFailure;
use sema_core::runtime::{
    CancelReason, CancellationView, ExecutorShutdown, IdCounter, IoExecutor, NativeCallContext,
    NativeOutcome, NativeResult, ResumeInput, RootId, RuntimeScopedIdCounter, SettlementSeq,
    TaskContextHandle, TaskId, TaskOutcome, TaskSettlement, Trace,
};
#[cfg(test)]
use sema_core::runtime::{CancellationParent, LifetimeOwner, TaskRelations};
use sema_core::EvalContext;
#[cfg(test)]
use sema_core::Value;

use super::{
    ContinuationFrame, DriveBudget, DriveState, PendingResume, ReadyScheduler,
    RegisterExternalError, RootRecord, RootState, RuntimeClock, RuntimeCreateError, TaskRecord,
    TimerQueue, WaitRuntime,
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
    continuations: Vec<ContinuationFrame>,
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
    roots: HashMap<RootId, RootRecord>,
    tasks: HashMap<TaskId, RuntimeTask>,
    ready: ReadyScheduler,
    timers: TimerQueue,
    handle_cleanup: VecDeque<RootId>,
    pending: VecDeque<PendingStage>,
    drive_cursor: usize,
    shutting_down: bool,
    terminal_fault: Option<RuntimeFault>,
    #[cfg(test)]
    force_settlement_exhaustion: bool,
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
            && self.continuations.iter().all(|frame| frame.trace(sink))
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
        let waits = WaitRuntime::new(executor)?;
        let root_ids = RuntimeScopedIdCounter::new(waits.runtime_id());
        Ok(Self {
            state: Rc::new(RefCell::new(RuntimeState {
                _context: context,
                clock,
                waits: Some(waits),
                root_ids,
                task_ids: IdCounter::new(),
                settlement_ids: IdCounter::new(),
                roots: HashMap::new(),
                tasks: HashMap::new(),
                ready: ReadyScheduler::new(),
                timers: TimerQueue::new(),
                handle_cleanup: VecDeque::new(),
                pending: VecDeque::new(),
                drive_cursor: 0,
                shutting_down: false,
                terminal_fault: None,
                #[cfg(test)]
                force_settlement_exhaustion: false,
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
        let mut root_ids = state.root_ids.clone();
        let mut task_ids = state.task_ids.clone();
        let root = (!state.force_root_exhaustion)
            .then(|| root_ids.allocate())
            .transpose()
            .ok()
            .flatten()
            .ok_or(SubmitRootError::IdExhausted)?;
        let task = (!state.force_task_exhaustion)
            .then(|| task_ids.allocate())
            .transpose()
            .ok()
            .flatten()
            .ok_or(SubmitRootError::IdExhausted)?;
        state.root_ids = root_ids;
        state.task_ids = task_ids;
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
                continuations: Vec::new(),
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
                instructions: 0,
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
                PendingStage::Apply(task, pending.invoke_continuation())
            }
            PendingStage::Apply(task, result) => {
                self.apply_native_result(task, result)?;
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
            TaskAction::Settle(root, task_id, TaskOutcome::Cancelled(cancel.reason))
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
            TaskAction::Native(task_id, result) => self.apply_native_result(task_id, result)?,
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
                    .push_back(PendingStage::Apply(task_id, result));
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
        result: NativeResult,
    ) -> Result<(), RuntimeFault> {
        let has_continuation = self
            .state
            .borrow()
            .tasks
            .get(&task_id)
            .is_some_and(|task| !task.continuations.is_empty());
        if has_continuation {
            match result {
                Ok(NativeOutcome::Return(value)) => {
                    return self.resume_continuation(task_id, ResumeInput::Returned(value));
                }
                Err(error) => {
                    return self.resume_continuation(task_id, ResumeInput::Failed(error));
                }
                other => return self.apply_native_outcome(task_id, other),
            }
        }
        self.apply_native_outcome(task_id, result)
    }

    fn apply_native_outcome(
        &self,
        task_id: TaskId,
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
                self.settle(root, task_id, TaskOutcome::Returned(value))
            }
            Err(error) => self.settle(root, task_id, TaskOutcome::Failed(error)),
            Ok(NativeOutcome::Suspend(suspend)) => {
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
                let registration = waits.register_external(
                    &mut task.record,
                    suspend,
                    TaskContextHandle::default(),
                );
                let mut state = self.state.borrow_mut();
                state.waits = Some(waits);
                state.tasks.insert(task_id, task);
                match registration {
                    Ok(_) => Ok(()),
                    Err(RegisterExternalError::Rejected(pending)) => {
                        state.pending.push_back(PendingStage::Decode(*pending));
                        Ok(())
                    }
                    Err(RegisterExternalError::IdExhausted(kind, _)) => {
                        drop(state);
                        self.settle(
                            root,
                            task_id,
                            TaskOutcome::Failed(sema_core::SemaError::eval(format!(
                                "runtime {kind} identity exhausted"
                            ))),
                        )
                    }
                }
            }
            Ok(NativeOutcome::Call(call)) => {
                let mut state = self.state.borrow_mut();
                let eval_context = Rc::clone(&state._context);
                let task =
                    state
                        .tasks
                        .get_mut(&task_id)
                        .ok_or_else(|| RuntimeFault::Invariant {
                            message: "calling task disappeared".into(),
                        })?;
                task.continuations
                    .push(ContinuationFrame::new(call.continuation));
                let native =
                    call.callable
                        .as_native_fn_rc()
                        .ok_or_else(|| RuntimeFault::Invariant {
                            message:
                                "Sema closure continuation execution is deferred to Task 4 Step 4"
                                    .into(),
                        })?;
                let mut task_context = task.context.borrow_mut();
                let mut context = NativeCallContext {
                    task_context: &mut task_context,
                    cancellation: CancellationView::new(false, None),
                };
                let result = native.invoke_runtime(&eval_context, &mut context, &call.args);
                drop(task_context);
                state
                    .pending
                    .push_back(PendingStage::Apply(task_id, result));
                Ok(())
            }
        }
    }

    fn resume_continuation(&self, task_id: TaskId, input: ResumeInput) -> Result<(), RuntimeFault> {
        let mut state = self.state.borrow_mut();
        let Some(task) = state.tasks.get_mut(&task_id) else {
            return Err(RuntimeFault::Invariant {
                message: "continuation task disappeared".into(),
            });
        };
        let Some(frame) = task.continuations.pop() else {
            return Err(RuntimeFault::Invariant {
                message: "continuation frame disappeared".into(),
            });
        };
        let mut task_context = task.context.borrow_mut();
        let cancellation = task.record.cancellation();
        let mut context = NativeCallContext {
            task_context: &mut task_context,
            cancellation: CancellationView::new(
                cancellation.is_some(),
                cancellation.map(|c| c.reason),
            ),
        };
        let resumed = frame.resume(&mut context, input);
        drop(task_context);
        state
            .pending
            .push_back(PendingStage::Apply(task_id, resumed));
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
    Native(TaskId, NativeResult),
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
    Apply(TaskId, NativeResult),
}

impl Trace for PendingStage {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Action(action) => action.trace(sink),
            Self::Decode(pending) | Self::Continue(pending) => pending.trace(sink),
            Self::Apply(_, result) => match result {
                Ok(outcome) => outcome.trace(sink),
                Err(error) => error.trace(sink),
            },
        }
    }
}

impl Trace for TaskAction {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Settle(_, _, outcome) => outcome.trace(sink),
            Self::Native(_, result) => match result {
                Ok(outcome) => outcome.trace(sink),
                Err(error) => error.trace(sink),
            },
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
