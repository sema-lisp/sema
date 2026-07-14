use std::collections::{BTreeMap, HashSet, VecDeque};
use std::time::Instant;

use super::WaitKey;

#[derive(Default)]
pub struct TimerQueue {
    deadlines: BTreeMap<Instant, VecDeque<WaitKey>>,
    active: HashSet<WaitKey>,
}

impl TimerQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, deadline: Instant, key: WaitKey) -> bool {
        if !self.active.insert(key) {
            return false;
        }
        self.deadlines.entry(deadline).or_default().push_back(key);
        true
    }

    pub fn cancel(&mut self, key: WaitKey) -> bool {
        self.active.remove(&key)
    }

    pub fn pop_due(&mut self, now: Instant) -> Option<WaitKey> {
        loop {
            let deadline = self.deadlines.first_key_value()?.0;
            if *deadline > now {
                return None;
            }
            let deadline = *deadline;
            let queue = self.deadlines.get_mut(&deadline)?;
            let key = queue.pop_front();
            if queue.is_empty() {
                self.deadlines.remove(&deadline);
            }
            if key.is_some_and(|key| self.active.remove(&key)) {
                return key;
            }
        }
    }

    pub fn next_deadline(&mut self) -> Option<Instant> {
        loop {
            let (&deadline, queue) = self.deadlines.first_key_value()?;
            if queue.iter().any(|key| self.active.contains(key)) {
                return Some(deadline);
            }
            self.deadlines.remove(&deadline);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}
