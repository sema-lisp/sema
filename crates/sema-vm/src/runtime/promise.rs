use std::collections::HashMap;
use std::rc::Rc;

use sema_core::runtime::{
    IdExhausted, PromiseId, RuntimeId, RuntimeScopedIdCounter, TaskId, TaskOutcome, TaskSettlement,
    Trace,
};

use super::WaitKey;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryError {
    WrongRuntime,
    Unknown,
    AlreadySettled,
    IdExhausted,
    NonMonotonicSettlement,
}

#[derive(Clone, Debug)]
pub enum PromiseState {
    Pending,
    Returned(Rc<TaskSettlement>),
    Failed(Rc<TaskSettlement>),
    Cancelled(Rc<TaskSettlement>),
}

struct PromiseRecord {
    task: Option<TaskId>,
    settlement: Option<Rc<TaskSettlement>>,
    waiters: HashMap<WaitKey, TaskId>,
}

pub struct PromiseRegistry {
    runtime: RuntimeId,
    ids: RuntimeScopedIdCounter<PromiseId>,
    records: HashMap<PromiseId, PromiseRecord>,
    last_settlement: Option<sema_core::runtime::SettlementSeq>,
}

impl PromiseRegistry {
    pub fn new(runtime: RuntimeId) -> Self {
        Self {
            runtime,
            ids: RuntimeScopedIdCounter::new(runtime),
            records: HashMap::new(),
            last_settlement: None,
        }
    }
    pub fn allocate_pending(&mut self, task: Option<TaskId>) -> Result<PromiseId, IdExhausted> {
        let id = self.ids.allocate()?;
        self.records.insert(
            id,
            PromiseRecord {
                task,
                settlement: None,
                waiters: HashMap::new(),
            },
        );
        Ok(id)
    }
    pub fn task(&self, id: PromiseId) -> Result<Option<TaskId>, RegistryError> {
        Ok(self.record(id)?.task)
    }
    pub fn state(&self, id: PromiseId) -> Result<PromiseState, RegistryError> {
        let Some(settlement) = &self.record(id)?.settlement else {
            return Ok(PromiseState::Pending);
        };
        Ok(match &settlement.outcome {
            TaskOutcome::Returned(_) => PromiseState::Returned(Rc::clone(settlement)),
            TaskOutcome::Failed(_) => PromiseState::Failed(Rc::clone(settlement)),
            TaskOutcome::Cancelled(_) => PromiseState::Cancelled(Rc::clone(settlement)),
        })
    }
    pub fn settle(
        &mut self,
        id: PromiseId,
        settlement: Rc<TaskSettlement>,
    ) -> Result<Vec<(WaitKey, TaskId)>, RegistryError> {
        if self
            .last_settlement
            .is_some_and(|last| settlement.sequence <= last)
        {
            return Err(RegistryError::NonMonotonicSettlement);
        }
        let sequence = settlement.sequence;
        let mut waiters: Vec<_> = {
            let record = self.record_mut(id)?;
            if record.settlement.is_some() {
                return Err(RegistryError::AlreadySettled);
            }
            record.settlement = Some(settlement);
            record.waiters.drain().collect()
        };
        self.last_settlement = Some(sequence);
        waiters.sort_by_key(|(key, _)| (key.id, key.generation));
        Ok(waiters)
    }
    pub fn observe(
        &mut self,
        id: PromiseId,
        key: WaitKey,
        task: TaskId,
    ) -> Result<bool, RegistryError> {
        let record = self.record_mut(id)?;
        if record.settlement.is_some() {
            return Ok(false);
        }
        record.waiters.insert(key, task);
        Ok(true)
    }
    pub fn cancel_observation(
        &mut self,
        id: PromiseId,
        key: WaitKey,
    ) -> Result<bool, RegistryError> {
        Ok(self.record_mut(id)?.waiters.remove(&key).is_some())
    }
    fn record(&self, id: PromiseId) -> Result<&PromiseRecord, RegistryError> {
        if id.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        self.records.get(&id).ok_or(RegistryError::Unknown)
    }
    fn record_mut(&mut self, id: PromiseId) -> Result<&mut PromiseRecord, RegistryError> {
        if id.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        self.records.get_mut(&id).ok_or(RegistryError::Unknown)
    }
}

impl Trace for PromiseRegistry {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.records.values().all(|record| {
            record
                .settlement
                .as_ref()
                .is_none_or(|settlement| settlement.trace(sink))
        })
    }
}

impl From<IdExhausted> for RegistryError {
    fn from(_: IdExhausted) -> Self {
        Self::IdExhausted
    }
}
