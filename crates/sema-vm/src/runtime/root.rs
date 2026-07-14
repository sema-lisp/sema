use std::rc::Rc;

use sema_core::runtime::{RootId, TaskId, TaskSettlement};

#[derive(Debug)]
pub enum RootState {
    Running { main_task: TaskId },
    Settled(Rc<TaskSettlement>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootTransitionError {
    WrongMainTask { expected: TaskId, actual: TaskId },
    AlreadySettled,
}

pub struct RootRecord {
    id: RootId,
    state: RootState,
}

impl RootRecord {
    pub fn new(id: RootId, main_task: TaskId) -> Self {
        Self {
            id,
            state: RootState::Running { main_task },
        }
    }

    pub fn id(&self) -> RootId {
        self.id
    }

    pub fn state(&self) -> &RootState {
        &self.state
    }

    pub fn settle(
        &mut self,
        task: TaskId,
        settlement: Rc<TaskSettlement>,
    ) -> Result<(), RootTransitionError> {
        let RootState::Running { main_task } = &self.state else {
            return Err(RootTransitionError::AlreadySettled);
        };
        if task != *main_task {
            return Err(RootTransitionError::WrongMainTask {
                expected: *main_task,
                actual: task,
            });
        }
        self.state = RootState::Settled(settlement);
        Ok(())
    }
}
