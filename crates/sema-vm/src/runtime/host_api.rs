//! The host-facing surface of the runtime: submitting roots, polling their
//! result, and shutting the runtime down. This is the API a host (CLI,
//! notebook, MCP, DAP, wasm) drives a program through; the drive-loop
//! internals it calls into (`Runtime::drive`, `cancel_waiting`,
//! `abort_terminal_state`, `deliver_cancel_teardown`, `RuntimeState`) stay in
//! `state.rs`.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Instant;

use sema_core::runtime::{
    CancelReason, CancellationParent, ExecutorShutdown, LifetimeOwner, RootId, TaskContextHandle,
    TaskId, TaskRelations, TaskSettlement,
};

use crate::VM;

use super::state::{
    deliver_cancel_teardown, ReturnOwner, Runtime, RuntimeFault, RuntimeState, RuntimeTask,
    SubmitRootError, TaskPayload, TaskScopes,
};
use super::wait::{CommandChannel, RuntimeCommand};
use super::{DriveBudget, DriveState, RootRecord, RootState, TaskRecord, WaitRuntime};

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

pub struct RootHandle {
    pub(super) runtime: Weak<RefCell<RuntimeState>>,
    pub(super) id: RootId,
}

/// The runtime's only `Send + Sync` control surface: lets a host cancel a
/// root, or every root, from a thread other than the one driving
/// `Runtime::drive` (a signal handler, a watchdog thread, a notebook
/// server's request handler). Holds no `Rc`/`Value`/`Env` — only a channel
/// sender and a shared dirty flag (`CommandChannel`, `wait.rs`), so it
/// carries nothing the cycle collector needs to trace and cannot form a
/// cross-thread aliasing hazard on GC state (Invariant I2).
///
/// A command rides the same inbox channel an `IoExecutor` uses to deliver
/// completions (see `RuntimeCommand`/`InboxItem` in `wait.rs`): enqueuing one
/// sets the same dirty flag a completion would, so a driving thread parked in
/// `block_on_inbox` wakes for a command exactly as it would for a completion
/// — no second channel, no select loop. Commands are drained and applied on
/// the runtime's own drive thread, at the top of every `drive` turn, before
/// source rotation (`Runtime::apply_pending_commands`, `state.rs`) — this
/// handle never touches `RuntimeState` itself, only the existing
/// `Runtime::cancel_root`.
#[derive(Clone)]
pub struct RuntimeCommandHandle {
    channel: CommandChannel,
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    #[expect(dead_code, reason = "compile-time-only Send+Sync witness")]
    fn check() {
        assert_send_sync::<RuntimeCommandHandle>();
    }
};

impl RuntimeCommandHandle {
    pub(super) fn new(channel: CommandChannel) -> Self {
        Self { channel }
    }

    /// Request cancellation of `root`. Returns `false` if the runtime has
    /// been dropped or shut down (the inbox channel is closed) — the command
    /// was never delivered. `true` means the command was enqueued; it does
    /// not guarantee `root` was still live when the drive loop processed it
    /// (a root that settles first makes the cancellation an inert no-op,
    /// exactly like calling `RootHandle::cancel` on an already-settled root).
    pub fn cancel_root(&self, root: RootId) -> bool {
        self.channel.send(RuntimeCommand::CancelRoot(root))
    }

    /// Request cancellation of every live root (the Ctrl-C shape). Same
    /// delivery/liveness semantics as [`cancel_root`](Self::cancel_root).
    pub fn cancel_all(&self) -> bool {
        self.channel.send(RuntimeCommand::CancelAll)
    }
}

impl Runtime {
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

    /// A `Send + Sync` handle for cancelling roots from another thread. See
    /// [`RuntimeCommandHandle`].
    pub fn command_handle(&self) -> RuntimeCommandHandle {
        let channel = self
            .state
            .borrow()
            .waits
            .as_ref()
            .expect("open runtime has waits installed")
            .command_channel();
        RuntimeCommandHandle::new(channel)
    }

    pub fn shutdown(&self, options: &ShutdownOptions) -> Result<ShutdownReport, RuntimeFault> {
        let mut terminal_fault = self.state.borrow().terminal_fault.clone();
        {
            let mut state = self.state.borrow_mut();
            state.shutting_down = true;
            // Shutdown drains via `cancel_waiting`'s dirty queue below, so every
            // task cancelled here (whether or not it is currently `Waiting`) must
            // be seeded onto the queue — `cancel_waiting` re-validates at pop
            // time and drops anything that isn't actually a waiting candidate.
            for task in state.tasks.values_mut() {
                task.record
                    .request_cancellation(CancelReason::InterpreterShutdown);
            }
            let all_task_ids: Vec<TaskId> = state.tasks.keys().copied().collect();
            state.pending_cancel_waits.extend(all_task_ids);
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
            if newly {
                state.pending_cancel_waits.push_back(task_id);
            }
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
