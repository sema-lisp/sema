use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use sema_core::runtime::{
    CancelReason, IdCounter, IoExecutor, NativeOutcome, NativeResult, RootId,
    RuntimeScopedIdCounter, SettlementSeq, TaskContextHandle, TaskId, TaskOutcome, TaskSettlement,
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
}

pub struct Runtime {
    state: Rc<RefCell<RuntimeState>>,
}

pub struct RootHandle {
    runtime: Weak<RefCell<RuntimeState>>,
    id: RootId,
}

struct RuntimeTask {
    record: TaskRecord,
    payload: TaskPayload,
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
    waits: WaitRuntime,
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
                waits,
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
            if self.advance_pending()? {
                work_items += 1;
                continue;
            }
            if self.drain_completion() {
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
        } else if state.roots.is_empty() && state.waits.active_len() == 0 {
            Ok(DriveState::Quiescent)
        } else {
            Ok(DriveState::Idle {
                next_deadline: None,
                inbox_wakeup_required: state.waits.active_len() > 0,
                legacy_io_wakeup_required: false,
            })
        }
    }

    fn cleanup_one(&self) -> bool {
        let mut state = self.state.borrow_mut();
        let Some(root) = state.handle_cleanup.pop_front() else {
            return false;
        };
        if state
            .roots
            .get(&root)
            .is_some_and(RootRecord::is_reap_eligible)
        {
            state.roots.remove(&root);
        }
        true
    }

    fn drain_completion(&self) -> bool {
        let mut state = self.state.borrow_mut();
        let task_ids = state.tasks.keys().copied().collect::<Vec<_>>();
        for task_id in task_ids {
            let Some(mut task) = state.tasks.remove(&task_id) else {
                continue;
            };
            let drained = state.waits.drain_one(&mut task.record);
            state.tasks.insert(task_id, task);
            if let Some((_route, pending)) = drained {
                if let Some(pending) = pending {
                    state.pending.push_back(PendingStage::Decode(pending));
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
        let action = {
            let mut state = self.state.borrow_mut();
            let Some((root, task_id)) = state.ready.dequeue() else {
                return Ok(false);
            };
            let task = state
                .tasks
                .get_mut(&task_id)
                .ok_or_else(|| RuntimeFault::Invariant {
                    message: "ready scheduler referenced missing task".into(),
                })?;
            task.record
                .start()
                .map_err(|error| RuntimeFault::Invariant {
                    message: format!("ready task failed to start: {error:?}"),
                })?;
            if let Some(cancel) = task.record.cancellation() {
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
            }
        };
        self.apply_action(action)
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
                let mut state = self.state.borrow_mut();
                let mut task =
                    state
                        .tasks
                        .remove(&task_id)
                        .ok_or_else(|| RuntimeFault::Invariant {
                            message: "suspending task disappeared".into(),
                        })?;
                let registration = state.waits.register_external(
                    &mut task.record,
                    suspend,
                    TaskContextHandle::default(),
                );
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
        let sequence = match state.settlement_ids.allocate() {
            Ok(sequence) => sequence,
            Err(_) => {
                let fault = RuntimeFault::IdExhausted { kind: "settlement" };
                state.shutting_down = true;
                state.terminal_fault = Some(fault.clone());
                return Err(fault);
            }
        };
        let mut task = state
            .tasks
            .remove(&task_id)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "settling task disappeared".into(),
            })?;
        let settlement =
            task.record
                .settle(sequence, outcome)
                .map_err(|error| RuntimeFault::Invariant {
                    message: format!("task settlement transition failed: {error:?}"),
                })?;
        let root_record = state
            .roots
            .get_mut(&root)
            .ok_or_else(|| RuntimeFault::Invariant {
                message: "settling task root disappeared".into(),
            })?;
        root_record
            .settle(task_id, settlement)
            .map_err(|error| RuntimeFault::Invariant {
                message: format!("root settlement transition failed: {error:?}"),
            })?;
        root_record.release_descendant();
        if root_record.is_reap_eligible() {
            state.handle_cleanup.push_back(root);
        }
        Ok(())
    }

    pub fn cancel_root(&self, root: RootId, reason: CancelReason) -> bool {
        let mut state = self.state.borrow_mut();
        if root.runtime() != state.waits.runtime_id() || !state.roots.contains_key(&root) {
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

    #[cfg(test)]
    pub(super) fn root_count(&self) -> usize {
        self.state.borrow().roots.len()
    }

    #[cfg(test)]
    pub(super) fn task_count(&self) -> usize {
        self.state.borrow().tasks.len()
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
            Some(RootState::Running { .. }) | None => RootPoll::Pending,
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
            if let Some(root) = runtime.borrow_mut().roots.get_mut(&self.id) {
                let _ = root.retain_handle();
            }
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
}

enum PendingStage {
    Decode(PendingResume),
    Continue(PendingResume),
    Apply(TaskId, NativeResult),
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
