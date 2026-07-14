use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Instant;

use super::WaitKey;

#[derive(Default)]
pub struct TimerQueue {
    deadlines: BTreeMap<Instant, VecDeque<WaitKey>>,
    reverse: HashMap<WaitKey, Instant>,
}

impl TimerQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, deadline: Instant, key: WaitKey) -> bool {
        if self.reverse.contains_key(&key) {
            return false;
        }
        self.reverse.insert(key, deadline);
        self.deadlines.entry(deadline).or_default().push_back(key);
        true
    }

    pub fn cancel(&mut self, key: WaitKey) -> bool {
        let Some(deadline) = self.reverse.remove(&key) else {
            return false;
        };
        let queue = self
            .deadlines
            .get_mut(&deadline)
            .expect("reverse timer entry has deadline bucket");
        let position = queue
            .iter()
            .position(|candidate| *candidate == key)
            .expect("reverse timer entry is present in deadline bucket");
        queue.remove(position);
        if queue.is_empty() {
            self.deadlines.remove(&deadline);
        }
        true
    }

    pub fn pop_due(&mut self, now: Instant) -> Option<WaitKey> {
        let deadline = *self.deadlines.first_key_value()?.0;
        if deadline > now {
            return None;
        }
        let queue = self.deadlines.get_mut(&deadline)?;
        let key = queue.pop_front().expect("non-empty timer bucket");
        if queue.is_empty() {
            self.deadlines.remove(&deadline);
        }
        self.reverse.remove(&key);
        Some(key)
    }

    pub fn next_deadline(&mut self) -> Option<Instant> {
        self.deadlines
            .first_key_value()
            .map(|(&deadline, _)| deadline)
    }

    pub fn is_empty(&self) -> bool {
        self.reverse.is_empty()
    }

    pub fn scheduled_len(&self) -> usize {
        self.reverse.len()
    }
}
