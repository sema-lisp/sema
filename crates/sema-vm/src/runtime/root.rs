use std::rc::Rc;

use sema_core::runtime::{RootId, TaskId, TaskSettlement, Trace};

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

    pub fn release_handle(&mut self) {
        self.handle_count = self.handle_count.saturating_sub(1);
    }

    pub fn release_descendant(&mut self) {
        self.live_descendants = self.live_descendants.saturating_sub(1);
    }

    pub fn is_reap_eligible(&self) -> bool {
        matches!(self.state, RootState::Settled(_))
            && self.handle_count == 0
            && self.live_descendants == 0
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

impl Trace for RootRecord {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match &self.state {
            RootState::Running { .. } => true,
            RootState::Settled(settlement) => settlement.trace(sink),
        }
    }
}
