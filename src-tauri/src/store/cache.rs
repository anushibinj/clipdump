//! Byte budgeted LRU cache for the payloads that live on disk: full PNG blobs
//! and the full text of items too big to keep in RAM (Section 3, "RAM bounded
//! LRU"). Keys are item ids; an id is either an image or a text, never both, so
//! one cache serves both.
//!
//! Rules:
//!
//! * The budget counts payload bytes, not entries. Inserting evicts least
//!   recently used entries until the total is back under the budget.
//! * A payload bigger than the whole budget is handed to the caller but never
//!   cached (it would evict everything and then itself).
//! * Cached payloads are `Arc<[u8]>`, so a hit is a refcount bump and the caller
//!   can keep using it after the entry is evicted.
//!
//! Speed trade: one `Mutex` (the LRU needs `&mut` even for a hit, because a hit
//! reorders it). The search path never touches the cache, so contention is only
//! between copy/preview requests, which are human paced. Disk reads and
//! decryption happen *outside* the lock; if two threads miss on the same id at
//! once, both load and the second insert simply replaces the first.

use std::sync::Arc;

use anyhow::Result;
use lru::LruCache;
use parking_lot::Mutex;

struct Inner {
    // Unbounded by count: the byte budget below is the real limit.
    lru: LruCache<u64, Arc<[u8]>>,
    bytes: usize,
    budget: usize,
}

impl Inner {
    /// Drop least recently used entries until the total fits the budget.
    fn shrink(&mut self) {
        while self.bytes > self.budget {
            match self.lru.pop_lru() {
                Some((_, v)) => self.bytes -= v.len(),
                None => break,
            }
        }
    }
}

pub struct BlobCache {
    inner: Mutex<Inner>,
}

impl BlobCache {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                lru: LruCache::unbounded(),
                bytes: 0,
                budget: budget_bytes,
            }),
        }
    }

    /// A cached payload, marking it most recently used.
    pub fn get(&self, id: u64) -> Option<Arc<[u8]>> {
        self.inner.lock().lru.get(&id).cloned()
    }

    /// Cache `data` for `id`. Returns false (and caches nothing) if it is
    /// larger than the whole budget. Replaces an older entry for the same id.
    pub fn insert(&self, id: u64, data: Arc<[u8]>) -> bool {
        let mut g = self.inner.lock();
        // A stale entry for this id must go either way.
        if let Some(old) = g.lru.pop(&id) {
            g.bytes -= old.len();
        }
        if data.len() > g.budget {
            return false;
        }
        g.bytes += data.len();
        g.lru.put(id, data);
        g.shrink();
        true
    }

    /// The cached payload for `id`, or `load()` (disk read plus decrypt, run
    /// without holding the lock) cached under `id`. A payload over the budget
    /// is returned uncached.
    pub fn get_or_load(
        &self,
        id: u64,
        load: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Arc<[u8]>> {
        if let Some(hit) = self.get(id) {
            return Ok(hit);
        }
        let data: Arc<[u8]> = load()?.into();
        self.insert(id, Arc::clone(&data));
        Ok(data)
    }

    /// Forget `id` (the item was deleted).
    pub fn remove(&self, id: u64) {
        let mut g = self.inner.lock();
        if let Some(old) = g.lru.pop(&id) {
            g.bytes -= old.len();
        }
    }

    /// Change the budget (a setting) and evict down to it right away. A budget
    /// of 0 empties the cache and disables it.
    pub fn set_budget(&self, budget_bytes: usize) {
        let mut g = self.inner.lock();
        g.budget = budget_bytes;
        g.shrink();
    }

    /// Payload bytes currently held (for the stats screen).
    pub fn bytes(&self) -> usize {
        self.inner.lock().bytes
    }

    pub fn budget(&self) -> usize {
        self.inner.lock().budget
    }

    pub fn len(&self) -> usize {
        self.inner.lock().lru.len()
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock();
        g.lru.clear();
        g.bytes = 0;
    }

    /// Ids in the cache, most recently used first. For tests.
    #[cfg(test)]
    fn ids_mru_first(&self) -> Vec<u64> {
        self.inner.lock().lru.iter().map(|(k, _)| *k).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn blob(n: usize, fill: u8) -> Arc<[u8]> {
        vec![fill; n].into()
    }

    #[test]
    fn evicts_least_recently_used_until_under_budget() {
        let c = BlobCache::new(100);
        assert!(c.insert(1, blob(40, 1)));
        assert!(c.insert(2, blob(40, 2)));
        assert_eq!(c.bytes(), 80);
        // Touch 1 so that 2 is now the LRU entry.
        assert!(c.get(1).is_some());
        assert!(c.insert(3, blob(40, 3)));
        assert_eq!(c.bytes(), 80, "120 would be over: one entry was evicted");
        assert!(c.get(2).is_none(), "the least recently used one went");
        assert_eq!(c.ids_mru_first(), vec![3, 1]);
        assert!(c.bytes() <= c.budget());
    }

    #[test]
    fn one_big_insert_evicts_several_small_ones() {
        let c = BlobCache::new(100);
        for id in 1..=4 {
            c.insert(id, blob(25, 0));
        }
        assert_eq!(c.bytes(), 100);
        assert!(c.insert(9, blob(90, 9)));
        assert_eq!(c.len(), 1);
        assert_eq!(c.bytes(), 90);
    }

    #[test]
    fn payload_over_budget_is_returned_but_not_cached() {
        let c = BlobCache::new(100);
        c.insert(1, blob(50, 1));
        let loads = Cell::new(0);
        let got = c
            .get_or_load(2, || {
                loads.set(loads.get() + 1);
                Ok(vec![7; 101])
            })
            .unwrap();
        assert_eq!(got.len(), 101);
        assert!(c.get(2).is_none());
        assert_eq!(c.bytes(), 50, "the existing entry was not evicted for it");
        // Not cached, so the next call loads again.
        c.get_or_load(2, || {
            loads.set(loads.get() + 1);
            Ok(vec![7; 101])
        })
        .unwrap();
        assert_eq!(loads.get(), 2);
    }

    #[test]
    fn get_or_load_hits_after_the_first_load() {
        let c = BlobCache::new(1000);
        let loads = Cell::new(0);
        for _ in 0..3 {
            let v = c
                .get_or_load(5, || {
                    loads.set(loads.get() + 1);
                    Ok(vec![1, 2, 3])
                })
                .unwrap();
            assert_eq!(&*v, &[1, 2, 3]);
        }
        assert_eq!(loads.get(), 1);
        assert_eq!(c.bytes(), 3);
        assert!(c.get_or_load(6, || anyhow::bail!("disk gone")).is_err());
        assert_eq!(c.len(), 1, "a failed load caches nothing");
    }

    #[test]
    fn replace_remove_clear_and_budget_changes_keep_the_byte_count_exact() {
        let c = BlobCache::new(100);
        c.insert(1, blob(30, 1));
        c.insert(1, blob(10, 2));
        assert_eq!(c.bytes(), 10, "replacing does not double count");
        c.insert(2, blob(60, 3));
        c.insert(3, blob(20, 3));
        assert_eq!(c.bytes(), 90);
        c.remove(2);
        assert_eq!(c.bytes(), 30);
        c.remove(2);
        assert_eq!(c.bytes(), 30, "removing twice is harmless");
        c.set_budget(15);
        assert!(c.bytes() <= 15);
        assert!(!c.insert(4, blob(16, 4)), "over the new budget");
        c.set_budget(0);
        assert_eq!((c.bytes(), c.len()), (0, 0));
        assert!(!c.insert(5, blob(1, 5)), "budget 0 disables the cache");
        c.set_budget(10);
        c.insert(6, blob(5, 6));
        c.clear();
        assert_eq!((c.bytes(), c.len()), (0, 0));
    }

    #[test]
    fn replacing_an_entry_with_one_over_budget_drops_the_stale_one() {
        let c = BlobCache::new(50);
        c.insert(1, blob(10, 1));
        assert!(!c.insert(1, blob(60, 2)));
        assert!(
            c.get(1).is_none(),
            "never serve the old payload for a new one"
        );
        assert_eq!(c.bytes(), 0);
    }
}
