//! Retention: what the history keeps and what it evicts (Section 1, "History
//! rules").
//!
//! Three independent limits, all of which spare pinned items:
//!
//! * **Count**: at most `max_items` unpinned items (default 1,000). Checked
//!   after every insert and after an unpin; evicts the oldest first.
//! * **Age**: unpinned items whose `created_at` is older than `max_age_days`
//!   (default 30). Checked at startup and every 15 minutes by
//!   [`RetentionTimer`]. Age counts from `created_at`, not `last_used_at`, so
//!   copying an old item again does not keep it alive (pinning does).
//! * **Disk cap**: the estimated payload bytes (`Items::disk_bytes`) at most
//!   `disk_cap_bytes` (default 2 GB). Evicts the oldest unpinned items that own
//!   an image blob first (they are what fills the disk), then, only if that is
//!   not enough, the oldest unpinned items of any kind. If everything left is
//!   pinned the cap is simply exceeded: pins always win.
//!
//! A limit of 0 means "no limit". The victim selection functions are pure (they
//! only read [`Items`]), so they are tested without a database.

use std::io;
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{RecvTimeoutError, Sender};
use parking_lot::Mutex;

use super::{Items, Store};
use crate::model::ClipKind;
use crate::persist::WriteOp;
use crate::settings::Settings;

const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// How often the timer thread runs the age and disk checks.
pub const INTERVAL: Duration = Duration::from_secs(15 * 60);

/// The limits as plain numbers. 0 disables a limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_items: usize,
    pub max_age_days: u32,
    pub disk_cap_bytes: u64,
}

impl Limits {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            max_items: s.max_items,
            max_age_days: s.max_age_days,
            disk_cap_bytes: s.disk_cap_bytes,
        }
    }
}

/// Oldest unpinned items beyond `max_items` unpinned ones.
pub fn count_victims(items: &Items, max_items: usize) -> Vec<u64> {
    if max_items == 0 || items.unpinned_count() <= max_items {
        return Vec::new();
    }
    let excess = items.unpinned_count() - max_items;
    items
        .iter_oldest_first()
        .filter(|i| !i.pinned)
        .take(excess)
        .map(|i| i.id)
        .collect()
}

/// Unpinned items created before `cutoff` (unix ms).
pub fn expired_victims(items: &Items, cutoff: i64) -> Vec<u64> {
    items
        .iter_oldest_first()
        .filter(|i| !i.pinned && i.created_at < cutoff)
        .map(|i| i.id)
        .collect()
}

/// Oldest unpinned items to drop so the estimated disk use is at most `cap`.
/// Image items first (they own the big blobs), then everything else unpinned.
pub fn disk_victims(items: &Items, cap: u64) -> Vec<u64> {
    if cap == 0 || items.disk_bytes() <= cap {
        return Vec::new();
    }
    let mut over = items.disk_bytes() - cap;
    let mut victims = Vec::new();
    for images_pass in [true, false] {
        for it in items.iter_oldest_first() {
            if over == 0 {
                return victims;
            }
            if it.pinned || (it.kind == ClipKind::Image) != images_pass {
                continue;
            }
            victims.push(it.id);
            over = over.saturating_sub(super::disk_cost(it));
        }
    }
    victims
}

impl Store {
    /// Remove `ids` from RAM and queue their deletion (the writer thread then
    /// deletes the blob and thumbnail files). Runs under the caller's write
    /// lock.
    fn evict_ids(&self, items: &mut Items, ids: &[u64]) -> usize {
        for &id in ids {
            if items.remove(id).is_some() {
                self.queue(WriteOp::Delete { id: id as i64 });
                self.cache.remove(id);
            }
        }
        ids.len()
    }

    /// Enforce the count and disk limits. The caller holds the write lock and
    /// calls this right after anything that can break them (an insert, an
    /// unpin). Returns how many items were evicted. Cheap when nothing is over:
    /// two integer comparisons.
    pub(super) fn enforce_limits(&self, items: &mut Items, limits: &Limits) -> usize {
        let by_count = count_victims(items, limits.max_items);
        let mut n = self.evict_ids(items, &by_count);
        let by_disk = disk_victims(items, limits.disk_cap_bytes);
        n += self.evict_ids(items, &by_disk);
        n
    }

    /// Evict unpinned items created more than `max_age_days` days ago (0 keeps
    /// everything). Returns how many were evicted.
    pub fn evict_older_than(&self, max_age_days: u32) -> usize {
        if max_age_days == 0 {
            return 0;
        }
        let cutoff = self.now() - i64::from(max_age_days) * MS_PER_DAY;
        let n = {
            let mut items = self.items.write();
            let ids = expired_victims(&items, cutoff);
            if ids.is_empty() {
                return 0;
            }
            for &id in &ids {
                items.remove(id);
                self.cache.remove(id);
            }
            // One SQL statement deletes the same set (unpinned, created_at
            // below the cutoff) and the writer removes the files.
            self.queue(WriteOp::DeleteOlderThan { cutoff });
            ids.len()
        };
        self.notify();
        n
    }

    /// One full retention pass with the current settings: age, then count, then
    /// disk. Runs at startup, every 15 minutes, and when settings change.
    /// Returns how many items were evicted.
    pub fn run_retention(&self) -> usize {
        let limits = self.limits();
        let mut n = self.evict_older_than(limits.max_age_days);
        let by_limits = {
            let mut items = self.items.write();
            self.enforce_limits(&mut items, &limits)
        };
        if by_limits > 0 {
            self.notify();
        }
        n += by_limits;
        n
    }
}

/// The retention timer: one thread that sleeps, runs [`Store::run_retention`]
/// once per interval, and stops promptly (it waits on a channel, not a plain
/// sleep, so `stop` does not have to wait out the interval).
pub struct RetentionTimer {
    stop: Mutex<Option<Sender<()>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl RetentionTimer {
    /// Start with the default 15 minute interval.
    pub fn start(store: &Arc<Store>) -> io::Result<Self> {
        Self::start_with(store, INTERVAL)
    }

    /// Start with a custom interval (tests use a very short one). The thread
    /// holds only a weak reference, so it never keeps the store alive.
    pub fn start_with(store: &Arc<Store>, interval: Duration) -> io::Result<Self> {
        let (tx, rx) = crossbeam_channel::bounded::<()>(1);
        let weak: Weak<Store> = Arc::downgrade(store);
        let handle = std::thread::Builder::new()
            .name("clipdump-retention".into())
            .spawn(move || {
                // `recv_timeout` is the sleep: a message or a closed channel
                // means stop, a timeout means run.
                while let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(interval) {
                    match weak.upgrade() {
                        Some(store) => {
                            store.run_retention();
                        }
                        None => break,
                    }
                }
            })?;
        Ok(Self {
            stop: Mutex::new(Some(tx)),
            handle: Mutex::new(Some(handle)),
        })
    }

    /// Stop the thread and wait for it. Idempotent.
    pub fn stop(&self) {
        // Dropping the sender wakes the thread (disconnected = stop).
        drop(self.stop.lock().take());
        if let Some(h) = self.handle.lock().take() {
            let _ = h.join();
        }
    }
}

impl Drop for RetentionTimer {
    fn drop(&mut self) {
        self.stop();
    }
}
