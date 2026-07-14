use std::rc::Rc;

use sema_core::runtime::{
    CancelReason, NativeContinuation, ResumeInput, SettlementSeq, TaskId, TaskOutcome,
    TaskRelations, TaskSettlement, Trace, WaitGeneration, WaitId,
};

/// The explicit owner of a callable result while its caller is suspended.
pub enum ContinuationFrame {
    Native {
        continuation: Box<dyn NativeContinuation>,
    },
    VmNativeBoundary {
        continuation: Box<dyn NativeContinuation>,
    },
}

impl ContinuationFrame {
    pub fn native(continuation: Box<dyn NativeContinuation>) -> Self {
        Self::Native { continuation }
    }

    pub fn vm_native(continuation: Box<dyn NativeContinuation>) -> Self {
        Self::VmNativeBoundary { continuation }
    }

    fn into_continuation(self) -> Box<dyn NativeContinuation> {
        match self {
            Self::Native { continuation } | Self::VmNativeBoundary { continuation } => continuation,
        }
    }

    pub fn resume(
        self,
        context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        self.into_continuation().resume(context, input)
    }
}

impl Trace for ContinuationFrame {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Native { continuation } | Self::VmNativeBoundary { continuation } => {
                continuation.trace(sink)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WaitKey {
    pub id: WaitId,
    pub generation: WaitGeneration,
}

impl Trace for TaskRecord {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match &self.state {
            TaskState::Settled(settlement) => settlement.trace(sink),
            _ => true,
        }
    }
}

#[derive(Debug)]
pub enum TaskState {
    Ready,
    Running,
    Waiting(WaitKey),
    Settled(Rc<TaskSettlement>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateName {
    Ready,
    Running,
    Waiting,
    Settled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskTransitionError {
    Invalid { from: StateName, to: StateName },
    WaitMismatch { expected: WaitKey, actual: WaitKey },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CancellationRequest {
    pub reason: CancelReason,
}

pub struct TaskRecord {
    id: TaskId,
    relations: TaskRelations,
    state: TaskState,
    cancellation: Option<CancellationRequest>,
}

impl TaskRecord {
    pub fn new(id: TaskId, relations: TaskRelations) -> Self {
        Self {
            id,
            relations,
            state: TaskState::Ready,
            cancellation: None,
        }
    }

    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn relations(&self) -> TaskRelations {
        self.relations
    }

    pub fn state_name(&self) -> StateName {
        state_name(&self.state)
    }

    pub fn wait_key(&self) -> Option<WaitKey> {
        match self.state {
            TaskState::Waiting(key) => Some(key),
            _ => None,
        }
    }

    pub fn settlement(&self) -> Option<&Rc<TaskSettlement>> {
        match &self.state {
            TaskState::Settled(settlement) => Some(settlement),
            _ => None,
        }
    }

    pub fn cancellation(&self) -> Option<CancellationRequest> {
        self.cancellation
    }

    pub fn start(&mut self) -> Result<(), TaskTransitionError> {
        self.transition(StateName::Running, |state| match state {
            TaskState::Ready => Some(TaskState::Running),
            _ => None,
        })
    }

    pub fn yield_ready(&mut self) -> Result<(), TaskTransitionError> {
        self.transition(StateName::Ready, |state| match state {
            TaskState::Running => Some(TaskState::Ready),
            _ => None,
        })
    }

    pub fn wait(&mut self, key: WaitKey) -> Result<(), TaskTransitionError> {
        self.transition(StateName::Waiting, |state| match state {
            TaskState::Running => Some(TaskState::Waiting(key)),
            _ => None,
        })
    }

    pub fn wake(&mut self, actual: WaitKey) -> Result<(), TaskTransitionError> {
        if let TaskState::Waiting(expected) = self.state {
            if expected != actual {
                return Err(TaskTransitionError::WaitMismatch { expected, actual });
            }
        }
        self.transition(StateName::Ready, |state| match state {
            TaskState::Waiting(_) => Some(TaskState::Ready),
            _ => None,
        })
    }

    pub(crate) fn reject_wait(&mut self, actual: WaitKey) -> Result<(), TaskTransitionError> {
        if let TaskState::Waiting(expected) = self.state {
            if expected != actual {
                return Err(TaskTransitionError::WaitMismatch { expected, actual });
            }
        }
        self.transition(StateName::Running, |state| match state {
            TaskState::Waiting(_) => Some(TaskState::Running),
            _ => None,
        })
    }

    pub fn settle(
        &mut self,
        sequence: SettlementSeq,
        outcome: TaskOutcome,
    ) -> Result<Rc<TaskSettlement>, TaskTransitionError> {
        let from = self.state_name();
        if from == StateName::Settled {
            return Err(TaskTransitionError::Invalid {
                from,
                to: StateName::Settled,
            });
        }
        let settlement = Rc::new(TaskSettlement { sequence, outcome });
        self.state = TaskState::Settled(Rc::clone(&settlement));
        Ok(settlement)
    }

    pub fn request_cancellation(&mut self, reason: CancelReason) -> bool {
        if self.cancellation.is_some() {
            return false;
        }
        self.cancellation = Some(CancellationRequest { reason });
        true
    }

    fn transition(
        &mut self,
        to: StateName,
        next: impl FnOnce(&TaskState) -> Option<TaskState>,
    ) -> Result<(), TaskTransitionError> {
        let from = self.state_name();
        let Some(next_state) = next(&self.state) else {
            return Err(TaskTransitionError::Invalid { from, to });
        };
        self.state = next_state;
        Ok(())
    }
}

fn state_name(state: &TaskState) -> StateName {
    match state {
        TaskState::Ready => StateName::Ready,
        TaskState::Running => StateName::Running,
        TaskState::Waiting(_) => StateName::Waiting,
        TaskState::Settled(_) => StateName::Settled,
    }
}
