use std::collections::{HashMap, VecDeque};

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
struct Sender {
    key: WaitKey,
    task: TaskId,
    value: Value,
}
struct Receiver {
    key: WaitKey,
    task: TaskId,
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
    pub fn new(runtime: RuntimeId) -> Self {
        Self {
            runtime,
            ids: RuntimeScopedIdCounter::new(runtime),
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
            return Ok(ChannelResult::Received(value));
        }
        if let Some(sender) = channel.senders.pop_front() {
            self.wakes.push_back(ChannelWake {
                key: sender.key,
                task: sender.task,
                result: ChannelResult::Sent,
            });
            return Ok(ChannelResult::Received(sender.value));
        }
        if channel.closed {
            return Ok(ChannelResult::Closed);
        }
        channel.receivers.push_back(Receiver { key, task });
        Ok(ChannelResult::Waiting)
    }
    pub fn close(&mut self, id: ChannelId) -> Result<bool, RegistryError> {
        let (senders, receivers) = {
            let channel = self.channel_mut(id)?;
            if channel.closed {
                return Ok(false);
            }
            channel.closed = true;
            (
                std::mem::take(&mut channel.senders),
                std::mem::take(&mut channel.receivers),
            )
        };
        self.wakes.extend(senders.into_iter().map(|w| ChannelWake {
            key: w.key,
            task: w.task,
            result: ChannelResult::Closed,
        }));
        self.wakes
            .extend(receivers.into_iter().map(|w| ChannelWake {
                key: w.key,
                task: w.task,
                result: ChannelResult::Closed,
            }));
        Ok(true)
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
        let channel = self.channel_mut(id)?;
        Ok(channel.buffer.pop_front().map_or_else(
            || {
                if channel.closed {
                    ChannelResult::Closed
                } else {
                    ChannelResult::Waiting
                }
            },
            ChannelResult::Received,
        ))
    }
    pub fn cancel_wait(&mut self, id: ChannelId, key: WaitKey) -> Result<bool, RegistryError> {
        let channel = self.channel_mut(id)?;
        if let Some(i) = channel.senders.iter().position(|w| w.key == key) {
            channel.senders.remove(i);
            return Ok(true);
        }
        if let Some(i) = channel.receivers.iter().position(|w| w.key == key) {
            channel.receivers.remove(i);
            return Ok(true);
        }
        Ok(false)
    }
    pub fn take_wake(&mut self, key: WaitKey) -> Option<ChannelWake> {
        let index = self.wakes.iter().position(|wake| wake.key == key)?;
        self.wakes.remove(index)
    }
    fn channel_mut(&mut self, id: ChannelId) -> Result<&mut Channel, RegistryError> {
        if id.runtime() != self.runtime {
            return Err(RegistryError::WrongRuntime);
        }
        self.channels.get_mut(&id).ok_or(RegistryError::Unknown)
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
