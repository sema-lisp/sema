use std::rc::Rc;

use sema_core::runtime::{RootId, TaskId, TaskSettlement, Trace};

#[derive(Debug)]
pub enum RootState {
    Running { main_task: TaskId },
    Settled(Rc<TaskSettlement>),
    Aborted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootTransitionError {
    WrongMainTask { expected: TaskId, actual: TaskId },
    AlreadySettled,
}

pub struct RootRecord {
    id: RootId,
    state: RootState,
    handle_count: usize,
    live_descendants: usize,
}

impl RootRecord {
    pub fn new(id: RootId, main_task: TaskId) -> Self {
        Self {
            id,
            state: RootState::Running { main_task },
            handle_count: 1,
            live_descendants: 1,
        }
    }

    pub fn id(&self) -> RootId {
        self.id
    }

    pub fn state(&self) -> &RootState {
        &self.state
    }

    pub fn handle_count(&self) -> usize {
        self.handle_count
    }

    pub fn retain_handle(&mut self) -> bool {
        let Some(count) = self.handle_count.checked_add(1) else {
            return false;
        };
        self.handle_count = count;
        true
    }

    fn checked_decrement(value: &mut usize, what: &'static str) {
        *value = value
            .checked_sub(1)
            .unwrap_or_else(|| panic!("{what} underflow"));
    }

    pub fn release_handle(&mut self) {
        Self::checked_decrement(&mut self.handle_count, "root handle count");
    }

    pub fn release_descendant(&mut self) {
        Self::checked_decrement(&mut self.live_descendants, "root descendant count");
    }

    pub fn is_reap_eligible(&self) -> bool {
        matches!(self.state, RootState::Settled(_) | RootState::Aborted)
            && self.handle_count == 0
            && self.live_descendants == 0
    }

    pub fn abort(&mut self) {
        self.state = RootState::Aborted;
        self.live_descendants = 0;
    }

    pub fn settle(
        &mut self,
        task: TaskId,
        settlement: Rc<TaskSettlement>,
    ) -> Result<(), RootTransitionError> {
        self.validate_settlement(task)?;
        self.state = RootState::Settled(settlement);
        Ok(())
    }

    pub(crate) fn validate_settlement(&self, task: TaskId) -> Result<(), RootTransitionError> {
        let RootState::Running { main_task } = &self.state else {
            return Err(RootTransitionError::AlreadySettled);
        };
        if task != *main_task {
            return Err(RootTransitionError::WrongMainTask {
                expected: *main_task,
                actual: task,
            });
        }
        Ok(())
    }
}

impl Trace for RootRecord {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match &self.state {
            RootState::Running { .. } | RootState::Aborted => true,
            RootState::Settled(settlement) => settlement.trace(sink),
        }
    }
}
