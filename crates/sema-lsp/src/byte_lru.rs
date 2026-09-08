//! Byte-bounded least-recently-used storage for re-creatable LSP data.
//!
//! This cache owns only closed-document data. Open documents belong to the
//! document store and must not be inserted here: a request may hold an open
//! document's parsed result across any cache operation.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::mem;
use std::num::NonZeroUsize;

/// The result of inserting an entry into a [`ByteLruCache`].
#[derive(Debug)]
pub(crate) enum InsertResult<K, V> {
    /// The entry was stored. Entries in `evicted` were the least recently used
    /// entries needed to return the cache to its byte budget.
    Cached { evicted: Vec<(K, V)> },
    /// The entry alone exceeds the cache budget, so the cache was not changed.
    Rejected { key: K, value: V },
}

/// A true LRU cache with a fixed byte budget.
///
/// Each entry has a non-zero caller-supplied size. The size must account for
/// every allocation retained by the value and its cache bookkeeping. The cache
/// stores only values that fit by themselves, and evicts the least recently
/// used entries until its total retained size is within `capacity_bytes`.
///
/// The key bounds make the recency index efficient without keeping duplicate
/// keys in a queue. This is intended for canonical document paths, which meet
/// these bounds naturally.
pub(crate) struct ByteLruCache<K, V> {
    capacity_bytes: usize,
    used_bytes: usize,
    entries: HashMap<K, Entry<V>>,
    by_recency: BTreeMap<u64, K>,
    next_access: u64,
}

struct Entry<V> {
    value: V,
    bytes: usize,
    access: u64,
}

impl<K, V> ByteLruCache<K, V>
where
    K: Clone + Eq + Hash + Ord,
{
    /// Creates an empty cache with the given byte budget.
    pub(crate) fn new(capacity_bytes: usize) -> Self {
        Self {
            capacity_bytes,
            used_bytes: 0,
            entries: HashMap::new(),
            by_recency: BTreeMap::new(),
            next_access: 0,
        }
    }

    /// Returns the total caller-supplied size of cached values.
    #[cfg(test)]
    pub(crate) fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    /// Returns the number of cached values.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the value for `key` and marks it as most recently used.
    pub(crate) fn get(&mut self, key: &K) -> Option<&V> {
        self.touch(key)?;
        self.entries.get(key).map(|entry| &entry.value)
    }

    /// Updates metadata without changing the value's accounted memory size.
    pub(crate) fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.touch(key)?;
        self.entries.get_mut(key).map(|entry| &mut entry.value)
    }

    /// Borrows an entry without changing recency. Use this only for complete
    /// cache scans and other bookkeeping, not to serve an interactive hit.
    pub(crate) fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|entry| &entry.value)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(key, entry)| (key, &entry.value))
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        let removed: Vec<K> = self
            .entries
            .iter()
            .filter(|(key, entry)| !keep(key, &entry.value))
            .map(|(key, _)| key.clone())
            .collect();
        for key in removed {
            self.remove(&key);
        }
    }

    /// Stores `value` under `key` and returns any entries evicted to fit it.
    ///
    /// A value larger than the whole budget is returned unchanged. Existing
    /// cache entries, including an existing value for the same key, remain
    /// available in that case.
    pub(crate) fn insert(&mut self, key: K, value: V, bytes: NonZeroUsize) -> InsertResult<K, V> {
        let bytes = bytes.get();
        if bytes > self.capacity_bytes {
            return InsertResult::Rejected { key, value };
        }

        self.remove(&key);

        let mut evicted = Vec::new();
        let available_before_insert = self.capacity_bytes - bytes;
        while self.used_bytes > available_before_insert {
            if let Some(entry) = self.remove_least_recently_used() {
                evicted.push(entry);
            } else {
                break;
            }
        }
        debug_assert!(self.used_bytes <= available_before_insert);

        let access = self.next_access();
        self.used_bytes += bytes;
        self.by_recency.insert(access, key.clone());
        self.entries.insert(
            key,
            Entry {
                value,
                bytes,
                access,
            },
        );

        InsertResult::Cached { evicted }
    }

    /// Removes `key` from the cache and returns its value, if present.
    pub(crate) fn remove(&mut self, key: &K) -> Option<V> {
        let entry = self.entries.remove(key)?;
        self.used_bytes -= entry.bytes;
        self.by_recency.remove(&entry.access);
        Some(entry.value)
    }

    fn touch(&mut self, key: &K) -> Option<()> {
        let access = self.next_access();
        let old_access = self.entries.get(key)?.access;
        self.by_recency.remove(&old_access);
        self.by_recency.insert(access, key.clone());
        if let Some(entry) = self.entries.get_mut(key) {
            entry.access = access;
            Some(())
        } else {
            None
        }
    }

    fn remove_least_recently_used(&mut self) -> Option<(K, V)> {
        let (_, key) = self.by_recency.pop_first()?;
        self.remove(&key).map(|value| (key, value))
    }

    fn next_access(&mut self) -> u64 {
        if self.next_access == u64::MAX {
            self.compact_recency();
        }
        self.next_access += 1;
        self.next_access
    }

    fn compact_recency(&mut self) {
        let by_recency = mem::take(&mut self.by_recency);
        self.next_access = 0;
        for (_, key) in by_recency {
            self.next_access += 1;
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.access = self.next_access;
                self.by_recency.insert(self.next_access, key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteLruCache, InsertResult};
    use std::num::NonZeroUsize;

    fn bytes(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).expect("test sizes are non-zero")
    }

    #[test]
    fn evicts_the_least_recently_used_entry() {
        let mut cache = ByteLruCache::new(10);
        assert!(matches!(
            cache.insert("first", 1, bytes(4)),
            InsertResult::Cached { evicted } if evicted.is_empty()
        ));
        assert!(matches!(
            cache.insert("second", 2, bytes(4)),
            InsertResult::Cached { evicted } if evicted.is_empty()
        ));

        assert_eq!(cache.get(&"first"), Some(&1));

        let result = cache.insert("third", 3, bytes(4));
        assert!(matches!(
            result,
            InsertResult::Cached { evicted } if evicted == vec![("second", 2)]
        ));
        assert_eq!(cache.get(&"first"), Some(&1));
        assert_eq!(cache.get(&"third"), Some(&3));
        assert_eq!(cache.get(&"second"), None);
        assert_eq!(cache.used_bytes(), 8);
    }

    #[test]
    fn evicts_until_the_byte_budget_is_met() {
        let mut cache = ByteLruCache::new(10);
        let _ = cache.insert("one", 1, bytes(3));
        let _ = cache.insert("two", 2, bytes(3));
        let _ = cache.insert("three", 3, bytes(3));

        let result = cache.insert("four", 4, bytes(8));
        assert!(matches!(
            result,
            InsertResult::Cached { evicted }
                if evicted == vec![("one", 1), ("two", 2), ("three", 3)]
        ));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.used_bytes(), 8);
        assert_eq!(cache.get(&"four"), Some(&4));
    }

    #[test]
    fn rejects_an_oversized_value_without_disturbing_the_cache() {
        let mut cache = ByteLruCache::new(5);
        let _ = cache.insert("kept", 1, bytes(5));

        let result = cache.insert("too-large", 2, bytes(6));
        assert!(matches!(
            result,
            InsertResult::Rejected {
                key: "too-large",
                value: 2
            }
        ));
        assert_eq!(cache.get(&"kept"), Some(&1));
        assert_eq!(cache.used_bytes(), 5);
    }

    #[test]
    fn replacing_a_value_updates_its_byte_accounting_and_recency() {
        let mut cache = ByteLruCache::new(10);
        let _ = cache.insert("first", 1, bytes(4));
        let _ = cache.insert("second", 2, bytes(4));

        let result = cache.insert("first", 3, bytes(7));
        assert!(matches!(
            result,
            InsertResult::Cached { evicted } if evicted == vec![("second", 2)]
        ));
        assert_eq!(cache.get(&"first"), Some(&3));
        assert_eq!(cache.used_bytes(), 7);
    }
}
