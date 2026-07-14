//! Interpreter-owned runtime state and root lifecycle.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Instant;

use sema_core::runtime::{
    CancelReason, ExecutorShutdown, IdCounter, IoExecutor, NativeOutcome, NativeResult, RootId,
    RuntimeScopedIdCounter, SettlementSeq, TaskContextHandle, TaskId, TaskOutcome, TaskSettlement,
    Trace,
};
#[cfg(test)]
use sema_core::runtime::{CancellationParent, LifetimeOwner, TaskRelations};
use sema_core::EvalContext;
#[cfg(test)]
use sema_core::Value;

use super::{
    DriveBudget, DriveState, PendingResume, ReadyScheduler, RegisterExternalError, RootRecord,
    RootState, RuntimeClock, RuntimeCreateError, TaskRecord, WaitRuntime,
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
    RuntimeDropped,
    InvariantViolation,
}

#[derive(Clone, Debug)]
pub struct ShutdownOptions {
    pub deadline: Instant,
    pub drive_budget: DriveBudget,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShutdownReport {
    pub clean: bool,
    pub live_roots: usize,
    pub live_tasks: usize,
    pub active_waits: usize,
    pub retained_cleanup: usize,
    pub executor: Option<ExecutorShutdown>,
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
    handle_cleanup: VecDeque<RootId>,
    pending: VecDeque<PendingStage>,
    shutting_down: bool,
    terminal_fault: Option<RuntimeFault>,
    #[cfg(test)]
    force_settlement_exhaustion: bool,
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
                handle_cleanup: VecDeque::new(),
                pending: VecDeque::new(),
                shutting_down: false,
                terminal_fault: None,
                #[cfg(test)]
                force_settlement_exhaustion: false,
            })),
        })
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
        let root = root_ids
            .allocate()
            .map_err(|_| SubmitRootError::IdExhausted)?;
        let task = task_ids
            .allocate()
            .map_err(|_| SubmitRootError::IdExhausted)?;
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

        while work_items < budget.work_item_limit.get() {
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
            if cleanup < budget.cleanup_limit.get() && self.cleanup_one() {
                cleanup += 1;
                work_items += 1;
                continue;
            }
            if self.cancel_waiting()? {
                work_items += 1;
                continue;
            }
            if self.drain_completion() {
                work_items += 1;
                continue;
            }
            if cleanup < budget.cleanup_limit.get() && self.reap_one() {
                cleanup += 1;
                work_items += 1;
                continue;
            }
            if self.advance_pending()? {
                work_items += 1;
                continue;
            }
            if root_visits < budget.root_visit_limit.get() && self.visit_ready()? {
                root_visits += 1;
                work_items += 1;
                continue;
            }
            break;
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
                next_deadline: None,
                inbox_wakeup_required: state
                    .waits
                    .as_ref()
                    .is_some_and(|waits| waits.active_len() > 0),
                legacy_io_wakeup_required: false,
            })
        }
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
            (task_id, task, waits)
        };
        let (task_id, mut task, mut waits) = extracted;
        let key = task
            .record
            .wait_key()
            .expect("selected waiting task has key");
        let pending = waits.cancel(&mut task.record, key);
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
                    Err(RegisterExternalError::IdExhausted(_)) => {
                        drop(state);
                        self.settle(
                            root,
                            task_id,
                            TaskOutcome::Failed(sema_core::SemaError::eval(
                                "runtime wait identity exhausted",
                            )),
                        )
                    }
                }
            }
            Ok(NativeOutcome::Call(_)) => Err(RuntimeFault::Invariant {
                message: "native-to-Sema calls belong to Task 4".into(),
            }),
        }
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
        let original_fault = self.state.borrow().terminal_fault.clone();
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
                Err(fault) if original_fault.as_ref() == Some(&fault) => {
                    self.discard_one_terminal_task();
                    DriveState::Progress {
                        work_items: 1,
                        instructions: 0,
                        ready_remaining: false,
                    }
                }
                Err(fault) => return Err(fault),
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
        let live_tasks = state.tasks.len();
        let mut report = ShutdownReport {
            clean: live_tasks == 0 && active_waits == 0 && retained_cleanup == 0,
            live_roots: state.roots.len(),
            live_tasks,
            active_waits,
            retained_cleanup,
            executor: None,
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
        original_fault.map_or(Ok(report), Err)
    }

    fn discard_one_terminal_task(&self) {
        let removed = {
            let mut state = self.state.borrow_mut();
            let Some(task_id) = state.tasks.keys().next().copied() else {
                return;
            };
            state.tasks.remove(&task_id)
        };
        drop(removed);
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
    pub(super) fn force_settlement_exhaustion_for_test(&self) {
        self.state.borrow_mut().force_settlement_exhaustion = true;
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
            _ => true,
        }
    }
}
