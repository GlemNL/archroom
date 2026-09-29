//! A small most-recently-used cache for decoded images (plan §6.3: "a RAM
//! LRU of 2–3 decoded images"). Values are `Arc`s so a render in flight
//! keeps its image alive after eviction.

use std::sync::Arc;

#[derive(Debug)]
pub struct Lru<K, V> {
    capacity: usize,
    /// Most recently used last.
    entries: Vec<(K, Arc<V>)>,
}

impl<K: PartialEq, V> Lru<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Vec::new(),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<Arc<V>> {
        let at = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(at);
        let value = Arc::clone(&entry.1);
        self.entries.push(entry);
        Some(value)
    }

    pub fn insert(&mut self, key: K, value: V) -> Arc<V> {
        self.entries.retain(|(k, _)| *k != key);
        let value = Arc::new(value);
        self.entries.push((key, Arc::clone(&value)));
        if self.entries.len() > self.capacity {
            self.entries.remove(0);
        }
        value
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_least_recently_used() {
        let mut c = Lru::new(2);
        c.insert(1, "a");
        c.insert(2, "b");
        assert!(c.get(&1).is_some()); // 2 is now the oldest
        c.insert(3, "c");
        assert!(c.get(&2).is_none());
        assert!(c.get(&1).is_some() && c.get(&3).is_some());
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn reinserting_a_key_replaces_it() {
        let mut c = Lru::new(2);
        c.insert(1, "a");
        c.insert(1, "b");
        assert_eq!(c.len(), 1);
        assert_eq!(*c.get(&1).unwrap(), "b");
    }
}
