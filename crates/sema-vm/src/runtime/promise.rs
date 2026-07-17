#![cfg_attr(not(test), allow(dead_code))]

use std::collections::VecDeque;

use hashbrown::HashMap;
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
    DuplicateWait,
    IdExhausted,
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
    waiters: VecDeque<(WaitKey, TaskId)>,
}

pub struct PromiseRegistry {
    runtime: RuntimeId,
    ids: RuntimeScopedIdCounter<PromiseId>,
    records: HashMap<PromiseId, PromiseRecord>,
}

impl PromiseRegistry {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }
    pub fn new(runtime: RuntimeId, ids: RuntimeScopedIdCounter<PromiseId>) -> Self {
        Self {
            runtime,
            ids,
            records: HashMap::new(),
        }
    }
    pub fn allocate_pending(&mut self, task: Option<TaskId>) -> Result<PromiseId, IdExhausted> {
        let id = self.reserve_id()?;
        self.insert_pending(id, task);
        Ok(id)
    }
    pub(crate) fn reserve_id(&mut self) -> Result<PromiseId, IdExhausted> {
        self.ids.allocate()
    }
    pub(crate) fn insert_pending(&mut self, id: PromiseId, task: Option<TaskId>) {
        self.records.insert(
            id,
            PromiseRecord {
                task,
                settlement: None,
                waiters: VecDeque::new(),
            },
        );
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
    ) -> Result<VecDeque<(WaitKey, TaskId)>, RegistryError> {
        let waiters = {
            let record = self.record_mut(id)?;
            if record.settlement.is_some() {
                return Err(RegistryError::AlreadySettled);
            }
            record.settlement = Some(settlement);
            std::mem::take(&mut record.waiters)
        };
        Ok(waiters)
    }
    pub fn observe(
        &mut self,
        id: PromiseId,
        key: WaitKey,
        task: TaskId,
    ) -> Result<bool, RegistryError> {
        if key.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        let record = self.record_mut(id)?;
        if record.settlement.is_some() {
            return Ok(false);
        }
        if record
            .waiters
            .iter()
            .any(|(registered, _)| *registered == key)
        {
            return Err(RegistryError::DuplicateWait);
        }
        record.waiters.push_back((key, task));
        Ok(true)
    }
    pub fn cancel_observation(
        &mut self,
        id: PromiseId,
        key: WaitKey,
    ) -> Result<bool, RegistryError> {
        if key.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        let waiters = &mut self.record_mut(id)?.waiters;
        let Some(index) = waiters
            .iter()
            .position(|(registered, _)| *registered == key)
        else {
            return Ok(false);
        };
        waiters.remove(index);
        Ok(true)
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

    /// GC interior: emit the promise's settled value (if any) as `GcEdge::Value`
    /// edges, so a cycle routed through a settled promise (a promise resolving
    /// to a structure that reaches the promise handle) is discovered. A pending
    /// promise, or an unknown id, emits nothing.
    pub(crate) fn gc_trace_settlement(
        &self,
        id: PromiseId,
        sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>),
    ) {
        if let Some(record) = self.records.get(&id) {
            if let Some(settlement) = &record.settlement {
                settlement.trace(sink);
            }
        }
    }

    /// GC sever: clear a white promise's settled value, breaking its edge to
    /// the settled value so the cycle's `Rc` cascade can run. The settled
    /// `Value` stays pinned by the collector's side-map handle until the pass
    /// completes, so dropping the settlement `Rc` here frees nothing early.
    pub(crate) fn gc_sever_settlement(&mut self, id: PromiseId) -> Vec<sema_core::Value> {
        if let Some(record) = self.records.get_mut(&id) {
            record.settlement = None;
        }
        Vec::new()
    }

    /// GC eviction: a settled promise whose handle is gone is unreachable —
    /// remove its record so the registry stays O(live handles). A pending
    /// promise is kept (a live task may still settle it), as is one with
    /// waiters.
    pub(crate) fn gc_evict(&mut self, id: PromiseId) {
        if let Some(record) = self.records.get(&id) {
            if record.settlement.is_some() && record.waiters.is_empty() {
                self.records.remove(&id);
            }
        }
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
