#![cfg_attr(not(test), allow(dead_code))]

//! Per-handle mutual-exclusion gates with a FIFO waiter queue.
//!
//! Six checkout-style stdlib modules (sqlite, kv, proc, pty, serial, stream)
//! share one non-`Send` resource per handle that at most one offloaded op may
//! hold at a time. The legacy design open-coded an `Available/CheckedOut`
//! availability slot plus an `Acquire`-phase poll loop that re-attempted the
//! checkout on every I/O poll. This registry replaces that poll loop with a
//! first-class runtime primitive that mirrors [`super::ChannelRegistry`]: a gate
//! is a `busy` flag plus a FIFO `WaitKey` queue; `acquire` grants the slot
//! immediately or parks; `release` wakes exactly the head waiter (transferring
//! ownership); `close` fails every parked waiter. A parked acquirer is a runtime
//! wait (`WaitKind::ResourceSlot`) — the runtime-task no-poll rule holds.
//!
//! Unlike a channel, a gate carries NO `Value`s (it coordinates access to a
//! thread-local resource the registry never sees), so it is GC-trivial: its
//! `Trace` emits no edges and it needs no interior sever/evict hooks.

use std::collections::VecDeque;

use hashbrown::HashMap;

use sema_core::runtime::{
    IdExhausted, ResourceGateId, RuntimeId, RuntimeScopedIdCounter, TaskId, Trace,
};

use super::{RegistryError, WaitKey};

/// Outcome of an [`ResourceGateRegistry::acquire`] attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquireResult {
    /// The slot was free; the requesting task now owns it.
    Acquired,
    /// The slot was busy; the requester was appended to the FIFO waiter queue
    /// and will be woken (via [`ResourceGateWake`]) when the owner releases.
    Parked,
}

/// Why a parked waiter was woken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateResult {
    /// The slot was released to this (previously head) waiter — it now owns it.
    Granted,
    /// The gate was closed while this waiter was parked; the acquire fails.
    Closed,
}

/// A woken waiter: its wait key + owning task + why it woke. Carries no `Value`,
/// so it needs no GC edge.
pub struct ResourceGateWake {
    pub key: WaitKey,
    pub task: TaskId,
    pub result: GateResult,
}

impl Trace for ResourceGateWake {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

struct Waiter {
    key: WaitKey,
    task: TaskId,
}

struct Gate {
    busy: bool,
    /// The task currently holding the slot (granted or acquired), if any. Set on
    /// an immediate acquire and on a release-transfer to the FIFO head; cleared
    /// when a release finds no waiter. Lets the runtime release a gate held by a
    /// task that is cancelled after being GRANTED the slot but before its acquire
    /// continuation runs (the granted-but-not-run leak): the continuation raises
    /// on the cancellation without releasing, so the runtime must release for it.
    owner: Option<TaskId>,
    waiters: VecDeque<Waiter>,
}

pub struct ResourceGateRegistry {
    runtime: RuntimeId,
    ids: RuntimeScopedIdCounter<ResourceGateId>,
    gates: HashMap<ResourceGateId, Gate>,
    wakes: VecDeque<ResourceGateWake>,
}

impl ResourceGateRegistry {
    pub fn new(runtime: RuntimeId, ids: RuntimeScopedIdCounter<ResourceGateId>) -> Self {
        Self {
            runtime,
            ids,
            gates: HashMap::new(),
            wakes: VecDeque::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.gates.len()
    }

    /// Allocate a fresh, free gate.
    pub fn allocate(&mut self) -> Result<ResourceGateId, IdExhausted> {
        let id = self.ids.allocate()?;
        self.gates.insert(
            id,
            Gate {
                busy: false,
                owner: None,
                waiters: VecDeque::new(),
            },
        );
        Ok(id)
    }

    /// Attempt to acquire `id`'s slot for `task` (whose wait is `key`). A free
    /// slot is granted immediately (`Acquired`); a busy slot parks the requester
    /// FIFO (`Parked`).
    pub fn acquire(
        &mut self,
        id: ResourceGateId,
        key: WaitKey,
        task: TaskId,
    ) -> Result<AcquireResult, RegistryError> {
        if key.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        let gate = self.gate_mut(id)?;
        if gate.busy {
            gate.waiters.push_back(Waiter { key, task });
            Ok(AcquireResult::Parked)
        } else {
            gate.busy = true;
            gate.owner = Some(task);
            Ok(AcquireResult::Acquired)
        }
    }

    /// Release `id`'s slot. If a waiter is queued, ownership transfers to the
    /// FIFO head (the gate stays `busy`) and its `Granted` wake is buffered for
    /// [`pop_wake`]. Otherwise the gate goes free.
    ///
    /// [`pop_wake`]: Self::pop_wake
    pub fn release(&mut self, id: ResourceGateId) -> Result<(), RegistryError> {
        let gate = self.gate_mut(id)?;
        if let Some(head) = gate.waiters.pop_front() {
            // Ownership transfers to the head waiter; the gate stays busy.
            gate.owner = Some(head.task);
            self.wakes.push_back(ResourceGateWake {
                key: head.key,
                task: head.task,
                result: GateResult::Granted,
            });
        } else {
            gate.busy = false;
            gate.owner = None;
        }
        Ok(())
    }

    /// Close `id`: fail every parked waiter with `Closed` and drop the gate
    /// record. Idempotent by absence (an unknown/closed id is a no-op).
    pub fn close(&mut self, id: ResourceGateId) -> Result<(), RegistryError> {
        if id.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        if let Some(mut gate) = self.gates.remove(&id) {
            for waiter in gate.waiters.drain(..) {
                self.wakes.push_back(ResourceGateWake {
                    key: waiter.key,
                    task: waiter.task,
                    result: GateResult::Closed,
                });
            }
        }
        Ok(())
    }

    /// Remove a parked waiter (a task cancelled while queued behind a busy gate).
    /// Returns whether a waiter matching `key` was found and removed. The gate's
    /// current owner is unaffected (a cancelled OWNER releases via
    /// [`release`](Self::release)); this only reaches the queue.
    pub fn cancel_wait(&mut self, id: ResourceGateId, key: WaitKey) -> Result<bool, RegistryError> {
        if key.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        let gate = self.gate_mut(id)?;
        if let Some(i) = gate.waiters.iter().position(|w| w.key == key) {
            gate.waiters.remove(i);
            return Ok(true);
        }
        Ok(false)
    }

    /// The gate whose slot `task` currently HOLDS (acquired or granted), if any.
    /// Used to release a gate held by a task cancelled after being granted the
    /// slot but before its acquire continuation ran (which would otherwise raise
    /// on the cancellation without releasing — the granted-but-not-run leak).
    pub fn owner_gate(&self, task: TaskId) -> Option<ResourceGateId> {
        self.gates
            .iter()
            .find_map(|(id, gate)| (gate.owner == Some(task)).then_some(*id))
    }

    /// The task currently holding `id`'s slot, if any (test/observability oracle
    /// for the granted-but-not-run gate-release path).
    #[cfg(test)]
    pub(crate) fn owner_of(&self, id: ResourceGateId) -> Option<TaskId> {
        self.gates.get(&id).and_then(|gate| gate.owner)
    }

    pub fn pop_wake(&mut self) -> Option<ResourceGateWake> {
        self.wakes.pop_front()
    }

    /// Remove a buffered wake for `key` (used when the woken task is torn down
    /// before the wake was delivered).
    pub fn take_wake(&mut self, key: WaitKey) -> Option<ResourceGateWake> {
        let index = self.wakes.iter().position(|wake| wake.key == key)?;
        self.wakes.remove(index)
    }

    fn gate_mut(&mut self, id: ResourceGateId) -> Result<&mut Gate, RegistryError> {
        if id.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        self.gates.get_mut(&id).ok_or(RegistryError::Unknown)
    }
}

impl Trace for ResourceGateRegistry {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        // A gate holds no `Value`s — nothing to trace.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::runtime::{
        CompletionDelivery, CompletionRegistrar, CompletionSender, ExternalCompletion, IdCounter,
        RuntimeId, WaitGeneration, WaitId,
    };
    use std::sync::Arc;

    struct ClosedInbox;
    impl CompletionSender for ClosedInbox {
        fn send(&self, _: ExternalCompletion) -> CompletionDelivery {
            CompletionDelivery::InboxClosed
        }
    }

    /// Mint a fresh runtime identity the sanctioned way (via the completion
    /// registrar) — `RuntimeId::allocate` is crate-private to sema-core.
    fn fresh_runtime() -> RuntimeId {
        let (runtime, _registrar, _issuers) =
            CompletionRegistrar::register(Arc::new(ClosedInbox)).unwrap();
        runtime
    }

    struct Fixture {
        runtime: RuntimeId,
        registry: ResourceGateRegistry,
        wait_ids: IdCounter<WaitId>,
        generation: WaitGeneration,
    }

    impl Fixture {
        fn new() -> Self {
            let runtime = fresh_runtime();
            let registry = ResourceGateRegistry::new(runtime, RuntimeScopedIdCounter::new(runtime));
            Self {
                runtime,
                registry,
                wait_ids: IdCounter::new(),
                generation: IdCounter::<WaitGeneration>::new().allocate().unwrap(),
            }
        }

        fn key(&mut self) -> WaitKey {
            WaitKey {
                runtime: self.runtime,
                id: self.wait_ids.allocate().unwrap(),
                generation: self.generation,
            }
        }
    }

    fn task(n: u64) -> TaskId {
        TaskId::try_from_raw(n).unwrap()
    }

    #[test]
    fn acquire_free_then_release_goes_idle() {
        let mut fx = Fixture::new();
        let gate = fx.registry.allocate().unwrap();
        let k = fx.key();
        assert_eq!(
            fx.registry.acquire(gate, k, task(1)).unwrap(),
            AcquireResult::Acquired
        );
        // No waiter: release frees the slot, no wake.
        fx.registry.release(gate).unwrap();
        assert!(fx.registry.pop_wake().is_none());
        // Slot is free again — a fresh acquire is immediate.
        let k2 = fx.key();
        assert_eq!(
            fx.registry.acquire(gate, k2, task(2)).unwrap(),
            AcquireResult::Acquired
        );
    }

    #[test]
    fn contended_acquire_parks_and_release_wakes_fifo_head() {
        let mut fx = Fixture::new();
        let gate = fx.registry.allocate().unwrap();
        let owner = fx.key();
        let first = fx.key();
        let second = fx.key();
        assert_eq!(
            fx.registry.acquire(gate, owner, task(1)).unwrap(),
            AcquireResult::Acquired
        );
        // Two acquirers queue behind the busy owner, FIFO.
        assert_eq!(
            fx.registry.acquire(gate, first, task(2)).unwrap(),
            AcquireResult::Parked
        );
        assert_eq!(
            fx.registry.acquire(gate, second, task(3)).unwrap(),
            AcquireResult::Parked
        );
        // Owner releases: the FIFO HEAD (first) is granted; the gate stays busy.
        fx.registry.release(gate).unwrap();
        let wake = fx.registry.pop_wake().expect("head woken");
        assert_eq!(wake.key, first);
        assert_eq!(wake.task, task(2));
        assert_eq!(wake.result, GateResult::Granted);
        assert!(
            fx.registry.pop_wake().is_none(),
            "only one wake per release"
        );
        // The new owner (first) releases: second is granted next.
        fx.registry.release(gate).unwrap();
        let wake = fx.registry.pop_wake().expect("second woken");
        assert_eq!(wake.key, second);
        assert_eq!(wake.result, GateResult::Granted);
        // Second releases: nothing queued, slot goes idle.
        fx.registry.release(gate).unwrap();
        assert!(fx.registry.pop_wake().is_none());
    }

    #[test]
    fn cancel_removes_a_parked_waiter_and_skips_it_on_release() {
        let mut fx = Fixture::new();
        let gate = fx.registry.allocate().unwrap();
        let owner = fx.key();
        let cancelled = fx.key();
        let survivor = fx.key();
        fx.registry.acquire(gate, owner, task(1)).unwrap();
        fx.registry.acquire(gate, cancelled, task(2)).unwrap();
        fx.registry.acquire(gate, survivor, task(3)).unwrap();
        // Cancel the FIFO-head waiter while it is still queued.
        assert!(fx.registry.cancel_wait(gate, cancelled).unwrap());
        // A second cancel of the same key finds nothing.
        assert!(!fx.registry.cancel_wait(gate, cancelled).unwrap());
        // Release now skips the cancelled waiter and grants the survivor.
        fx.registry.release(gate).unwrap();
        let wake = fx.registry.pop_wake().expect("survivor woken");
        assert_eq!(wake.key, survivor);
        assert_eq!(wake.result, GateResult::Granted);
    }

    #[test]
    fn close_fails_all_parked_waiters_and_drops_the_gate() {
        let mut fx = Fixture::new();
        let gate = fx.registry.allocate().unwrap();
        let owner = fx.key();
        let a = fx.key();
        let b = fx.key();
        fx.registry.acquire(gate, owner, task(1)).unwrap();
        fx.registry.acquire(gate, a, task(2)).unwrap();
        fx.registry.acquire(gate, b, task(3)).unwrap();
        fx.registry.close(gate).unwrap();
        // Every parked waiter fails Closed, in FIFO order.
        let first = fx.registry.pop_wake().expect("first closed");
        assert_eq!(first.key, a);
        assert_eq!(first.result, GateResult::Closed);
        let second = fx.registry.pop_wake().expect("second closed");
        assert_eq!(second.key, b);
        assert_eq!(second.result, GateResult::Closed);
        assert!(fx.registry.pop_wake().is_none());
        // The gate record is gone: operations on it are Unknown, close is a no-op.
        assert_eq!(fx.registry.len(), 0);
        let k = fx.key();
        assert!(matches!(
            fx.registry.acquire(gate, k, task(4)),
            Err(RegistryError::Unknown)
        ));
        fx.registry.close(gate).unwrap();
    }

    #[test]
    fn foreign_runtime_ids_are_rejected() {
        let mut fx = Fixture::new();
        let gate = fx.registry.allocate().unwrap();
        // A gate id minted for a different runtime.
        let other = fresh_runtime();
        let foreign_gate = RuntimeScopedIdCounter::<ResourceGateId>::new(other)
            .allocate()
            .unwrap();
        let k = fx.key();
        assert!(matches!(
            fx.registry.acquire(foreign_gate, k, task(1)),
            Err(RegistryError::WrongRuntime)
        ));
        // A wait key minted for a different runtime is rejected too.
        let foreign_key = WaitKey {
            runtime: other,
            id: IdCounter::<WaitId>::new().allocate().unwrap(),
            generation: IdCounter::<WaitGeneration>::new().allocate().unwrap(),
        };
        assert!(matches!(
            fx.registry.acquire(gate, foreign_key, task(1)),
            Err(RegistryError::WrongRuntime)
        ));
    }

    #[test]
    fn take_wake_removes_a_buffered_wake() {
        let mut fx = Fixture::new();
        let gate = fx.registry.allocate().unwrap();
        let owner = fx.key();
        let waiter = fx.key();
        fx.registry.acquire(gate, owner, task(1)).unwrap();
        fx.registry.acquire(gate, waiter, task(2)).unwrap();
        fx.registry.release(gate).unwrap();
        // The buffered grant can be reclaimed by key before pop.
        let taken = fx.registry.take_wake(waiter).expect("buffered wake");
        assert_eq!(taken.key, waiter);
        assert!(fx.registry.pop_wake().is_none());
    }
}
