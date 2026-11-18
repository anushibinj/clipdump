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
//!   not enough, the oldest unpinned items of any kind. Pins always win: if the
//!   pinned items alone are already over the cap, no unpinned item is evicted for
//!   it (that would only empty the history without ever meeting the cap, and
//!   every new copy would vanish at once).
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
    let pinned_bytes: u64 = items
        .iter_oldest_first()
        .filter(|i| i.pinned)
        .map(|i| super::disk_cost(i))
        .sum();
    if pinned_bytes >= cap {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::{BASE, DAY, Fx, fake_image};
    use std::time::Instant;

    fn settings(max_items: usize, max_age_days: u32, disk_cap_bytes: u64) -> Settings {
        Settings {
            max_items,
            max_age_days,
            disk_cap_bytes,
            ..Settings::default()
        }
    }

    // -- count ----------------------------------------------------------------

    #[test]
    fn default_limit_is_1000_unpinned_items_oldest_evicted_first() {
        let fx = Fx::new();
        let ids: Vec<u64> = (0..1_050)
            .map(|n| fx.text(&format!("item number {n}")))
            .collect();
        assert_eq!(fx.store.len(), 1_000, "capped after every insert");
        assert_eq!(fx.store.read().unpinned_count(), 1_000);
        // The 50 oldest are gone, the 1,000 newest remain, in order.
        let mut expected = ids[50..].to_vec();
        expected.reverse();
        assert_eq!(fx.ids(), expected);
        assert!(fx.store.get(ids[49]).is_none());
        // And the database agrees, so a restart does not bring them back.
        let db: Vec<i64> = ids[50..].iter().map(|&i| i as i64).collect();
        assert_eq!(fx.db_ids(), db);
        assert_eq!(fx.store.persist().stats().errors, 0);
    }

    #[test]
    fn pins_survive_the_count_limit_and_do_not_count_against_it() {
        let fx = Fx::with(settings(5, 0, 0));
        let a = fx.text("pinned a");
        let b = fx.text("pinned b");
        let c = fx.text("old unpinned");
        fx.store.toggle_pin(a);
        fx.store.toggle_pin(b);
        for n in 0..20 {
            fx.text(&format!("filler {n}"));
        }
        assert!(fx.store.get(a).unwrap().pinned && fx.store.get(b).unwrap().pinned);
        assert!(
            fx.store.get(c).is_none(),
            "the unpinned old one was evicted"
        );
        assert_eq!(fx.store.len(), 2 + 5, "two pins plus five unpinned");
        assert_eq!(fx.store.read().unpinned_count(), 5);
        assert_eq!(fx.db_ids().len(), 7);
    }

    #[test]
    fn unpinning_over_the_limit_evicts_the_oldest_unpinned() {
        let fx = Fx::with(settings(2, 0, 0));
        let a = fx.text("a");
        fx.store.toggle_pin(a);
        let b = fx.text("b");
        let c = fx.text("c");
        assert_eq!(fx.store.len(), 3);
        // Unpinning `a` makes three unpinned items; `a` is the oldest.
        assert_eq!(fx.store.toggle_pin(a), Some(false));
        assert!(fx.store.get(a).is_none());
        assert_eq!(fx.ids(), vec![c, b]);
    }

    #[test]
    fn zero_means_no_limit() {
        let fx = Fx::with(settings(0, 0, 0));
        for n in 0..1_200 {
            fx.text_at(&format!("t{n}"), BASE - 400 * DAY);
        }
        fx.advance(500 * DAY);
        assert_eq!(fx.store.run_retention(), 0);
        assert_eq!(fx.store.len(), 1_200);
    }

    // -- age ------------------------------------------------------------------

    #[test]
    fn items_older_than_30_days_expire_boundary_is_exclusive() {
        let fx = Fx::new();
        let old = fx.text_at("old", BASE);
        fx.advance(29 * DAY);
        let mid = fx.text("created 29 days in");
        assert_eq!(fx.store.run_retention(), 0, "29 days old is kept");
        fx.advance(DAY);
        // `old` is exactly 30 days old: not older than 30 days yet.
        assert_eq!(fx.store.run_retention(), 0);
        assert!(fx.store.get(old).is_some());
        fx.advance(1);
        assert_eq!(fx.store.run_retention(), 1, "30 days and 1 ms: expired");
        assert!(fx.store.get(old).is_none());
        assert!(fx.store.get(mid).is_some());
        assert_eq!(fx.db_ids(), vec![mid as i64]);
        assert_eq!(fx.store.persist().stats().errors, 0);
    }

    #[test]
    fn age_is_measured_from_created_at_so_a_recent_copy_does_not_extend_it() {
        let fx = Fx::new();
        let a = fx.text_at("a", BASE);
        let b = fx.text_at("b", BASE);
        fx.advance(31 * DAY);
        // `a` is copied again today: its last_used_at is new, its created_at is not.
        assert!(fx.store.mark_used(a));
        fx.store.run_retention();
        assert!(fx.store.get(a).is_none());
        assert!(fx.store.get(b).is_none());
    }

    #[test]
    fn expiry_removes_blob_and_thumb_files() {
        let fx = Fx::new();
        let img = fx.image(1, 2_000);
        fx.flush();
        let it = fx.store.get(img).unwrap();
        let name = it.blob_hash.clone().unwrap();
        let hash = it.hash.to_be_bytes();
        fx.advance(31 * DAY);
        fx.store.run_retention();
        fx.flush();
        assert!(fx.store.persist().read_blob(&name).is_err());
        assert!(fx.store.persist().read_thumb(&hash).is_err());
    }

    #[test]
    fn pins_survive_age_and_clear_all_including_after_a_restart() {
        let fx = Fx::new();
        let pinned_old = fx.text_at("keep forever", BASE);
        let pinned_img = fx.image_at(1, 1_000, BASE);
        let plain_old = fx.text_at("expires", BASE);
        let plain_new = fx.text_at("recent", BASE + 40 * DAY);
        fx.store.toggle_pin(pinned_old);
        fx.store.toggle_pin(pinned_img);
        fx.advance(45 * DAY);
        fx.store.run_retention();
        assert!(
            fx.store.get(plain_old).is_none(),
            "45 days old, unpinned: gone"
        );
        assert!(fx.store.get(plain_new).is_some(), "5 days old");
        assert!(
            fx.store.get(pinned_old).is_some(),
            "pinned, 45 days old: kept"
        );
        fx.advance(100 * DAY);
        fx.store.run_retention();
        assert_eq!(fx.store.len(), 2, "only the two pins are left");
        assert_eq!(fx.store.clear_unpinned(), 0);

        let fx = fx.reopen();
        assert_eq!(fx.store.len(), 2);
        assert!(fx.store.get(pinned_img).unwrap().pinned);
        fx.text("new after restart");
        assert_eq!(fx.store.clear_unpinned(), 1);
        assert_eq!(fx.store.len(), 2, "clear all keeps pinned");
        let fx = fx.reopen();
        assert_eq!(fx.store.len(), 2);
        assert_eq!(fx.db_ids().len(), 2);
    }

    #[test]
    fn startup_applies_the_age_limit_and_lowered_limits() {
        let fx = Fx::new();
        for n in 0..10 {
            fx.text_at(&format!("old {n}"), BASE);
            fx.advance(1);
        }
        let recent: Vec<u64> = (0..6)
            .map(|n| fx.text_at(&format!("recent {n}"), BASE + 29 * DAY))
            .collect();
        fx.advance(31 * DAY);
        let fx = fx.reopen();
        assert_eq!(fx.store.len(), 6, "the ten old ones expired at startup");
        assert_eq!(fx.db_ids().len(), 6, "and were deleted from the database");
        // The user lowers the item limit between runs.
        fx.settings.write().unwrap().max_items = 4;
        let fx = fx.reopen();
        assert_eq!(fx.store.len(), 4);
        let mut newest: Vec<u64> = recent[2..].to_vec();
        newest.reverse();
        assert_eq!(fx.ids(), newest, "the four newest remain");
    }

    #[test]
    fn apply_settings_runs_retention_immediately() {
        let fx = Fx::new();
        for n in 0..10 {
            fx.text(&format!("t{n}"));
        }
        fx.settings.write().unwrap().max_items = 3;
        fx.store.apply_settings();
        assert_eq!(fx.store.len(), 3);
        fx.settings.write().unwrap().max_age_days = 1;
        fx.advance(2 * DAY);
        fx.store.apply_settings();
        assert_eq!(fx.store.len(), 0);
    }

    // -- disk cap -------------------------------------------------------------

    const MB: usize = 1_000_000;

    #[test]
    fn disk_cap_evicts_the_oldest_unpinned_blobs_until_under_it() {
        let fx = Fx::with(settings(0, 0, MB as u64));
        let a = fx.image(1, 400_000);
        let b = fx.image(2, 400_000);
        assert!(fx.store.stats().disk_bytes <= MB as u64, "two fit");
        let c = fx.image(3, 400_000);
        assert!(fx.store.get(a).is_none(), "the oldest image went");
        assert_eq!(fx.ids(), vec![c, b]);
        assert!(fx.store.stats().disk_bytes <= MB as u64);
        fx.flush();
        // Its files are deleted from disk, the survivors' are not.
        assert!(fx.store.get_blob(b).is_ok());
        assert!(fx.store.persist().read_blob(&fake_blob_name(1)).is_err());
        assert_eq!(fx.db_ids(), vec![b as i64, c as i64]);
    }

    /// Blob name of `fake_image(n, ..)`: the hex of its hash.
    fn fake_blob_name(n: u32) -> String {
        let h = fake_image(n, 1, 0).hash.to_be_bytes();
        crate::persist::blobs::hash_hex(&h)
    }

    #[test]
    fn disk_cap_spares_pinned_images() {
        let fx = Fx::with(settings(0, 0, MB as u64));
        let pinned = fx.image(1, 400_000);
        fx.store.toggle_pin(pinned);
        for n in 2..=6 {
            fx.image(n, 400_000);
        }
        assert!(fx.store.get(pinned).is_some());
        assert_eq!(fx.store.len(), 2, "the pin and the newest image");
        assert!(fx.store.stats().disk_bytes <= MB as u64);
    }

    #[test]
    fn disk_cap_evicts_images_before_text_and_falls_back_to_text() {
        let fx = Fx::with(settings(0, 0, 100_000));
        let t_old = fx.text(&format!("old text {}", "x".repeat(5_000)));
        let img = fx.image(1, 60_000);
        let t_new = fx.text(&format!("new text {}", "y".repeat(5_000)));
        assert_eq!(fx.store.len(), 3, "about 70 KB: under the cap");
        let img2 = fx.image(2, 60_000);
        // 130 KB: the old image goes, the older text stays.
        assert!(fx.store.get(img).is_none());
        assert!(fx.store.get(t_old).is_some());
        assert_eq!(fx.ids(), vec![img2, t_new, t_old]);
        // Now only text can be evicted: a 200 KB text is over the cap by itself,
        // and everything older goes first, then (last resort) itself.
        let big = fx.text(&"z".repeat(200_000));
        assert!(fx.store.stats().disk_bytes <= 100_000);
        assert!(fx.store.get(big).is_none());
        assert_eq!(fx.store.len(), 0);
    }

    #[test]
    fn disk_cap_the_pins_alone_exceed_is_left_alone() {
        let fx = Fx::with(settings(0, 0, 0));
        let pin = fx.image(1, 150_000);
        fx.store.toggle_pin(pin);
        // The user now sets a cap that the pinned image alone is over.
        fx.settings.write().unwrap().disk_cap_bytes = 100_000;
        fx.store.apply_settings();
        let t = fx.text("small and unpinned");
        assert!(fx.store.get(pin).is_some());
        assert!(
            fx.store.get(t).is_some(),
            "evicting it could never meet the cap, so it stays"
        );
        assert!(fx.store.stats().disk_bytes > 100_000);
    }

    #[test]
    fn disk_accounting_matches_the_items_after_every_kind_of_change() {
        let fx = Fx::new();
        let sum = |fx: &Fx| -> u64 { fx.snapshot().iter().map(super::super::disk_cost).sum() };
        let a = fx.image(1, 10_000);
        let b = fx.text("some text");
        fx.store
            .set_ocr_text(a, Some("ocr".into()), crate::model::OcrStatus::Done);
        fx.store.toggle_pin(b);
        fx.store.mark_used(a);
        assert_eq!(fx.store.stats().disk_bytes, sum(&fx));
        fx.store.remove(a);
        assert_eq!(fx.store.stats().disk_bytes, sum(&fx));
        fx.store.clear_unpinned();
        fx.store.toggle_pin(b);
        fx.store.clear_unpinned();
        assert_eq!(fx.store.stats().disk_bytes, 0);
        assert_eq!(fx.store.len(), 0);
    }

    // -- timer ----------------------------------------------------------------

    #[test]
    fn timer_runs_retention_and_stops_cleanly() {
        let fx = Fx::new();
        fx.text("old one");
        fx.text("another");
        fx.advance(31 * DAY);
        let timer = RetentionTimer::start_with(&fx.store, Duration::from_millis(5)).unwrap();
        // Poll with a generous deadline instead of sleeping a fixed time.
        let deadline = Instant::now() + Duration::from_secs(30);
        while fx.store.len() > 0 {
            assert!(Instant::now() < deadline, "the timer never ran retention");
            std::thread::sleep(Duration::from_millis(2));
        }
        timer.stop();
        timer.stop();
        assert_eq!(fx.db_ids().len(), 0);
    }

    #[test]
    fn timer_with_a_long_interval_stops_without_waiting_it_out() {
        let fx = Fx::new();
        let timer = RetentionTimer::start(&fx.store).unwrap();
        let started = Instant::now();
        drop(timer);
        // The interval is 15 minutes; any sane bound proves stop did not wait.
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn timer_does_not_keep_the_store_alive() {
        let fx = Fx::new();
        let weak = Arc::downgrade(&fx.store);
        let timer = RetentionTimer::start_with(&fx.store, Duration::from_secs(3600)).unwrap();
        assert_eq!(Arc::strong_count(&fx.store), 1);
        drop(timer);
        assert!(weak.upgrade().is_some());
    }

    // -- pure victim selection --------------------------------------------------

    #[test]
    fn limits_come_from_settings() {
        let l = Limits::from_settings(&settings(7, 8, 9));
        assert_eq!(
            l,
            Limits {
                max_items: 7,
                max_age_days: 8,
                disk_cap_bytes: 9
            }
        );
    }
}
