use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use super::WaitKey;

#[derive(Default)]
pub struct TimerQueue {
    deadlines: BTreeMap<(Instant, u64), WaitKey>,
    reverse: HashMap<WaitKey, (Instant, u64)>,
    next_sequence: u64,
}

impl TimerQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, deadline: Instant, key: WaitKey) -> bool {
        if self.reverse.contains_key(&key) {
            return false;
        }
        let timer_key = (deadline, self.next_sequence);
        let Some(next_sequence) = self.next_sequence.checked_add(1) else {
            return false;
        };
        self.next_sequence = next_sequence;
        self.reverse.insert(key, timer_key);
        self.deadlines.insert(timer_key, key);
        true
    }

    pub fn cancel(&mut self, key: WaitKey) -> bool {
        let Some(timer_key) = self.reverse.remove(&key) else {
            return false;
        };
        let removed = self.deadlines.remove(&timer_key);
        debug_assert_eq!(removed, Some(key));
        true
    }

    pub fn pop_due(&mut self, now: Instant) -> Option<WaitKey> {
        let timer_key = *self.deadlines.first_key_value()?.0;
        let deadline = timer_key.0;
        if deadline > now {
            return None;
        }
        let key = self.deadlines.remove(&timer_key)?;
        self.reverse.remove(&key);
        Some(key)
    }

    pub fn next_deadline(&mut self) -> Option<Instant> {
        self.deadlines
            .first_key_value()
            .map(|(&(deadline, _), _)| deadline)
    }

    pub fn is_empty(&self) -> bool {
        self.reverse.is_empty()
    }

    pub fn scheduled_len(&self) -> usize {
        self.reverse.len()
    }
}
