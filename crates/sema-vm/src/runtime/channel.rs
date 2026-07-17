#![cfg_attr(not(test), allow(dead_code))]

use std::collections::VecDeque;

use hashbrown::HashMap;

use sema_core::runtime::{
    ChannelId, IdExhausted, RuntimeId, RuntimeScopedIdCounter, TaskId, Trace,
};
use sema_core::Value;

use super::{RegistryError, WaitKey};

#[derive(Clone, Debug, PartialEq)]
pub enum ChannelResult {
    Waiting,
    Sent,
    Received(Value),
    Closed,
}

pub struct ChannelWake {
    pub key: WaitKey,
    pub task: TaskId,
    pub result: ChannelResult,
}

pub enum CancelledChannelWait {
    Sender(Value),
    Receiver,
}

impl Trace for ChannelWake {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        if let ChannelResult::Received(value) = &self.result {
            sink(sema_core::cycle::GcEdge::Value(value));
        }
        true
    }
}
struct Sender {
    key: WaitKey,
    task: TaskId,
    value: Value,
}
struct Receiver {
    key: WaitKey,
    task: TaskId,
}
pub(crate) struct ChannelClose {
    senders: VecDeque<Sender>,
    receivers: VecDeque<Receiver>,
}

impl ChannelClose {
    pub(crate) fn next_wake(&mut self) -> Option<ChannelWake> {
        if let Some(sender) = self.senders.pop_front() {
            return Some(ChannelWake {
                key: sender.key,
                task: sender.task,
                result: ChannelResult::Closed,
            });
        }
        self.receivers.pop_front().map(|receiver| ChannelWake {
            key: receiver.key,
            task: receiver.task,
            result: ChannelResult::Closed,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.senders.is_empty() && self.receivers.is_empty()
    }
}
struct Channel {
    capacity: usize,
    closed: bool,
    buffer: VecDeque<Value>,
    senders: VecDeque<Sender>,
    receivers: VecDeque<Receiver>,
}
pub struct ChannelRegistry {
    runtime: RuntimeId,
    ids: RuntimeScopedIdCounter<ChannelId>,
    channels: HashMap<ChannelId, Channel>,
    wakes: VecDeque<ChannelWake>,
}

impl ChannelRegistry {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.channels.len()
    }
    /// How many receivers are still genuinely queued (unmatched) on `id`. Lets
    /// a test detect the instant `close`/`try_receive` moves a waiter out of
    /// this registry (into a `ChannelClose`, or popped as a wake) — the point
    /// at which its wake becomes a real staged `PendingStage` item rather than
    /// something that could still be matched inline.
    #[cfg(test)]
    pub(crate) fn receiver_queue_len(&self, id: ChannelId) -> usize {
        self.channels
            .get(&id)
            .map_or(0, |channel| channel.receivers.len())
    }
    pub fn new(runtime: RuntimeId, ids: RuntimeScopedIdCounter<ChannelId>) -> Self {
        Self {
            runtime,
            ids,
            channels: HashMap::new(),
            wakes: VecDeque::new(),
        }
    }
    pub fn allocate(&mut self, capacity: usize) -> Result<ChannelId, IdExhausted> {
        let id = self.ids.allocate()?;
        self.channels.insert(
            id,
            Channel {
                capacity,
                closed: false,
                buffer: VecDeque::new(),
                senders: VecDeque::new(),
                receivers: VecDeque::new(),
            },
        );
        Ok(id)
    }
    pub fn send(
        &mut self,
        id: ChannelId,
        key: WaitKey,
        task: TaskId,
        value: Value,
    ) -> Result<ChannelResult, RegistryError> {
        if key.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        let channel = self.channel_mut(id)?;
        if channel.closed {
            return Ok(ChannelResult::Closed);
        }
        if let Some(receiver) = channel.receivers.pop_front() {
            self.wakes.push_back(ChannelWake {
                key: receiver.key,
                task: receiver.task,
                result: ChannelResult::Received(value),
            });
            return Ok(ChannelResult::Sent);
        }
        if channel.buffer.len() < channel.capacity {
            channel.buffer.push_back(value);
            return Ok(ChannelResult::Sent);
        }
        channel.senders.push_back(Sender { key, task, value });
        Ok(ChannelResult::Waiting)
    }
    pub fn receive(
        &mut self,
        id: ChannelId,
        key: WaitKey,
        task: TaskId,
    ) -> Result<ChannelResult, RegistryError> {
        if key.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        if let Some(result) = self.dequeue(id)? {
            return Ok(result);
        }
        let channel = self.channel_mut(id)?;
        channel.receivers.push_back(Receiver { key, task });
        Ok(ChannelResult::Waiting)
    }
    pub(crate) fn close(&mut self, id: ChannelId) -> Result<Option<ChannelClose>, RegistryError> {
        let (senders, receivers) = {
            let channel = self.channel_mut(id)?;
            if channel.closed {
                return Ok(None);
            }
            channel.closed = true;
            (
                std::mem::take(&mut channel.senders),
                std::mem::take(&mut channel.receivers),
            )
        };
        Ok(Some(ChannelClose { senders, receivers }))
    }
    pub(crate) fn pop_wake(&mut self) -> Option<ChannelWake> {
        self.wakes.pop_front()
    }
    pub fn inspect(
        &mut self,
        id: ChannelId,
        query: sema_core::runtime::ChannelQuery,
    ) -> Result<Value, RegistryError> {
        let channel = self.channel_mut(id)?;
        Ok(match query {
            sema_core::runtime::ChannelQuery::Closed => Value::bool(channel.closed),
            sema_core::runtime::ChannelQuery::Count => Value::int(channel.buffer.len() as i64),
            sema_core::runtime::ChannelQuery::Empty => Value::bool(channel.buffer.is_empty()),
            sema_core::runtime::ChannelQuery::Full => {
                Value::bool(channel.capacity > 0 && channel.buffer.len() == channel.capacity)
            }
        })
    }
    pub fn try_receive(&mut self, id: ChannelId) -> Result<ChannelResult, RegistryError> {
        Ok(self.dequeue(id)?.unwrap_or(ChannelResult::Waiting))
    }
    pub fn cancel_wait(
        &mut self,
        id: ChannelId,
        key: WaitKey,
    ) -> Result<Option<CancelledChannelWait>, RegistryError> {
        if key.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        let channel = self.channel_mut(id)?;
        if let Some(i) = channel.senders.iter().position(|w| w.key == key) {
            let sender = channel.senders.remove(i).expect("located sender exists");
            return Ok(Some(CancelledChannelWait::Sender(sender.value)));
        }
        if let Some(i) = channel.receivers.iter().position(|w| w.key == key) {
            channel.receivers.remove(i);
            return Ok(Some(CancelledChannelWait::Receiver));
        }
        Ok(None)
    }
    /// Whether a waiter with `key` is still genuinely queued on this channel
    /// (as a sender or receiver). Returns `false` once a rendezvous has matched
    /// and popped it — even while its `ChannelWake` is still in flight. Used by
    /// `cancel_waiting` to avoid cancel-dropping a receiver whose committed value
    /// is already on the way (UCR-3): a matched waiter is let through so its wake
    /// delivers and settlement observes the cancellation.
    pub fn has_wait(&self, id: ChannelId, key: WaitKey) -> bool {
        self.channels.get(&id).is_some_and(|channel| {
            channel.senders.iter().any(|w| w.key == key)
                || channel.receivers.iter().any(|w| w.key == key)
        })
    }
    pub fn take_wake(&mut self, key: WaitKey) -> Option<ChannelWake> {
        let index = self.wakes.iter().position(|wake| wake.key == key)?;
        self.wakes.remove(index)
    }
    fn dequeue(&mut self, id: ChannelId) -> Result<Option<ChannelResult>, RegistryError> {
        let channel = self.channel_mut(id)?;
        if let Some(value) = channel.buffer.pop_front() {
            if let Some(sender) = channel.senders.pop_front() {
                channel.buffer.push_back(sender.value);
                self.wakes.push_back(ChannelWake {
                    key: sender.key,
                    task: sender.task,
                    result: ChannelResult::Sent,
                });
            }
            return Ok(Some(ChannelResult::Received(value)));
        }
        if let Some(sender) = channel.senders.pop_front() {
            self.wakes.push_back(ChannelWake {
                key: sender.key,
                task: sender.task,
                result: ChannelResult::Sent,
            });
            return Ok(Some(ChannelResult::Received(sender.value)));
        }
        Ok(channel.closed.then_some(ChannelResult::Closed))
    }
    fn channel_mut(&mut self, id: ChannelId) -> Result<&mut Channel, RegistryError> {
        if id.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        self.channels.get_mut(&id).ok_or(RegistryError::Unknown)
    }

    /// GC interior: emit one `GcEdge::Value` per buffered value (exact
    /// multiplicity — each is one strong `Rc` this registry owns). Senders'
    /// in-flight values are held by their (separately reachable) parked tasks,
    /// so they are NOT emitted here — mirroring the pre-migration inline
    /// `Channel` buffer, whose GC node only traced the buffer. An unknown id is
    /// a no-op (the channel was already evicted).
    pub(crate) fn gc_trace_buffer(
        &self,
        id: ChannelId,
        sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>),
    ) {
        if let Some(channel) = self.channels.get(&id) {
            for value in &channel.buffer {
                sink(sema_core::cycle::GcEdge::Value(value));
            }
        }
    }

    /// GC sever: drain a white channel's buffer, returning its contents for the
    /// collector to drop after all severing has completed.
    pub(crate) fn gc_sever_buffer(&mut self, id: ChannelId) -> Vec<Value> {
        match self.channels.get_mut(&id) {
            Some(channel) => channel.buffer.drain(..).collect(),
            None => Vec::new(),
        }
    }

    /// GC eviction: a channel whose handle is gone is unreachable. Remove its
    /// record so the registry stays O(live handles) — but only if no task is
    /// parked sending or receiving on it (a waiter keeps the record alive until
    /// it is reaped/cancelled, so its wake still finds the channel).
    pub(crate) fn gc_evict(&mut self, id: ChannelId) {
        if let Some(channel) = self.channels.get(&id) {
            if channel.senders.is_empty() && channel.receivers.is_empty() {
                self.channels.remove(&id);
            }
        }
    }
}

impl Trace for ChannelRegistry {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        for channel in self.channels.values() {
            for value in &channel.buffer {
                sink(sema_core::cycle::GcEdge::Value(value));
            }
            for sender in &channel.senders {
                sink(sema_core::cycle::GcEdge::Value(&sender.value));
            }
        }
        for wake in &self.wakes {
            if let ChannelResult::Received(value) = &wake.result {
                sink(sema_core::cycle::GcEdge::Value(value));
            }
        }
        true
    }
}

impl Trace for ChannelClose {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        for sender in &self.senders {
            sink(sema_core::cycle::GcEdge::Value(&sender.value));
        }
        true
    }
}
