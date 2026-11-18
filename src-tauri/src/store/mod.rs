//! The in-memory store: the fast index and cache on top of the encrypted
//! persistence layer (Section 3 of PLAN.md).
//!
//! RAM holds every item's metadata, search text and thumbnail; images and large
//! texts stay on disk and come back through [`cache::BlobCache`]. Every change
//! is queued to [`Persist`] (a channel push, never disk I/O on the caller's
//! thread), so the store never waits for SQLite.
//!
//! # Data structure (and why)
//!
//! [`Items`] is three structures kept in lockstep under one
//! `parking_lot::RwLock`:
//!
//! ```text
//! by_seq:      BTreeMap<u64 seq, Arc<ClipItem>>   order: ascending seq = oldest first
//! seq_of:      HashMap<u64 id,   u64 seq>         id lookup
//! id_of_hash:  HashMap<u128 hash, u64 id>         O(1) dedup
//! ```
//!
//! Every item has a *sequence number* that grows on each insert and each bump to
//! the top, so "newest first" is just the BTreeMap read backwards. The item's
//! position is never stored as an index, so the classic bugs of a
//! `HashMap<id, usize>` over a `Vec` (every insert at the front and every
//! removal shifting every later position, so the whole map needs patching) cannot
//! happen: insert, bump, remove and evict-oldest are each one O(log n) BTreeMap
//! operation plus O(1) hash map updates, with n at most about 1,000 (so a few
//! dozen nanoseconds). Dedup is `id_of_hash` then `seq_of`, two hash lookups.
//!
//! Tradeoff: iterating a BTreeMap is a little slower than a contiguous `Vec`
//! (about 2 to 5 microseconds for 1,000 items, still far under the 1 ms search
//! budget, which is dominated by fuzzy matching). In return there is no index to
//! rebuild, and moving an item to the top costs the same wherever it was.
//!
//! The search path takes only [`Store::read`] (a shared lock) and iterates
//! [`Items::iter_newest_first`]. Writers (the two ingest threads, UI commands,
//! the retention timer) hold the write lock only for in-memory work plus
//! channel pushes: no disk, no decryption.
//!
//! Items are `Arc<ClipItem>` and changed copy on write (`Arc::make_mut`), so a
//! snapshot handed out by [`Store::get`] never changes under its reader.

pub mod cache;
pub mod retention;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use parking_lot::{RwLock, RwLockReadGuard};
use rayon::prelude::*;

use crate::ingest::changed::HistoryChanged;
use crate::ingest::text::search_slice;
use crate::ingest::{IngestedItem, ItemSink, OcrJob, OcrQueue, files};
use crate::model::{ClipItem, ClipKind, FileRef, OcrStatus, fold_for_search};
use crate::persist::blobs::hash_hex;
use crate::persist::crypto::{Crypto, OVERHEAD};
use crate::persist::db::MetaRow;
use crate::persist::{NewItem, Persist, WriteOp};
use crate::search::{self, PathChecker, ResultDto, SearchRequest};
use crate::settings::Settings;
use retention::Limits;

/// Texts up to this many UTF-8 bytes keep their full text in RAM; bigger ones
/// keep only the search slice and load the rest from SQLite on demand.
pub const TEXT_IN_RAM_MAX: usize = 64 * 1024;

/// Source of "now" in unix milliseconds. Injected so tests can move time.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The wall clock.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    })
}

/// Estimated bytes an item takes on disk, used for the disk cap. This counts
/// the encrypted payloads (blob, thumbnail, text) including their 40 byte
/// envelopes and ignores SQLite page overhead and the WAL, so the real folder is
/// a little bigger. Speed trade: an exact figure would need a `stat` per file.
fn disk_cost(item: &ClipItem) -> u64 {
    let o = OVERHEAD as u64;
    let thumb = item.thumb.as_ref().map_or(0, |t| t.len() as u64 + o);
    match item.kind {
        ClipKind::Text => item.byte_size + o + item.preview.len() as u64 + o,
        ClipKind::Image => item.byte_size + o + thumb + item.preview.len() as u64 + o,
        ClipKind::Files => thumb + item.preview.len() as u64 + o,
    }
}

/// All items plus their indexes. See the module docs for the layout.
#[derive(Default)]
pub struct Items {
    by_seq: BTreeMap<u64, Arc<ClipItem>>,
    seq_of: HashMap<u64, u64>,
    id_of_hash: HashMap<u128, u64>,
    next_seq: u64,
    /// Last `last_used_at` handed out, see [`Items::stamp`].
    last_stamp: i64,
    unpinned: usize,
    disk_bytes: u64,
}

impl Items {
    pub fn len(&self) -> usize {
        self.by_seq.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_seq.is_empty()
    }

    pub fn unpinned_count(&self) -> usize {
        self.unpinned
    }

    pub fn pinned_count(&self) -> usize {
        self.by_seq.len() - self.unpinned
    }

    /// Estimated payload bytes on disk, see [`disk_cost`].
    pub fn disk_bytes(&self) -> u64 {
        self.disk_bytes
    }

    pub fn get(&self, id: u64) -> Option<&Arc<ClipItem>> {
        self.by_seq.get(self.seq_of.get(&id)?)
    }

    pub fn id_for_hash(&self, hash: u128) -> Option<u64> {
        self.id_of_hash.get(&hash).copied()
    }

    /// Newest (most recently used) first. This is the order the empty-query
    /// list and the search tie-breaks use.
    pub fn iter_newest_first(&self) -> impl DoubleEndedIterator<Item = &Arc<ClipItem>> {
        self.by_seq.values().rev()
    }

    /// Oldest first. Retention walks this way.
    pub fn iter_oldest_first(&self) -> impl DoubleEndedIterator<Item = &Arc<ClipItem>> {
        self.by_seq.values()
    }

    /// A `last_used_at` that is at least `at` and strictly greater than every
    /// one handed out before. Strictly increasing stamps make the persisted
    /// order (`ORDER BY last_used_at DESC`) identical to the in-memory order
    /// after a restart, even when two copies land in the same millisecond. The
    /// price is that a burst can run ahead of the wall clock by a few ms.
    fn stamp(&mut self, at: i64) -> i64 {
        let s = at.max(self.last_stamp + 1);
        self.last_stamp = s;
        s
    }

    fn take_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Add an item at the top. The caller guarantees its id and hash are new.
    fn add(&mut self, item: ClipItem) {
        let seq = self.take_seq();
        self.seq_of.insert(item.id, seq);
        self.id_of_hash.insert(item.hash, item.id);
        if !item.pinned {
            self.unpinned += 1;
        }
        self.disk_bytes += disk_cost(&item);
        self.last_stamp = self.last_stamp.max(item.last_used_at);
        self.by_seq.insert(seq, Arc::new(item));
    }

    fn remove(&mut self, id: u64) -> Option<Arc<ClipItem>> {
        let seq = self.seq_of.remove(&id)?;
        let item = self.by_seq.remove(&seq)?;
        self.id_of_hash.remove(&item.hash);
        if !item.pinned {
            self.unpinned -= 1;
        }
        self.disk_bytes -= disk_cost(&item);
        Some(item)
    }

    /// Move an item to the top and count a use. False if the id is unknown.
    fn bump(&mut self, id: u64, at: i64) -> bool {
        let Some(old_seq) = self.seq_of.get(&id).copied() else {
            return false;
        };
        let Some(mut item) = self.by_seq.remove(&old_seq) else {
            return false;
        };
        let new_seq = self.take_seq();
        let it = Arc::make_mut(&mut item);
        it.last_used_at = at;
        it.use_count = it.use_count.saturating_add(1);
        self.seq_of.insert(id, new_seq);
        self.by_seq.insert(new_seq, item);
        true
    }

    /// Change an item in place (copy on write if a reader holds it). The
    /// closure must not change the id, hash, pinned flag or anything
    /// [`disk_cost`] reads: use the dedicated methods for those.
    fn update(&mut self, id: u64, f: impl FnOnce(&mut ClipItem)) -> bool {
        let Some(seq) = self.seq_of.get(&id) else {
            return false;
        };
        match self.by_seq.get_mut(seq) {
            Some(item) => {
                f(Arc::make_mut(item));
                true
            }
            None => false,
        }
    }

    /// Set the pinned flag. Returns the previous value, `None` if unknown.
    fn set_pinned(&mut self, id: u64, pinned: bool) -> Option<bool> {
        let seq = self.seq_of.get(&id)?;
        let item = Arc::make_mut(self.by_seq.get_mut(seq)?);
        let was = item.pinned;
        item.pinned = pinned;
        match (was, pinned) {
            (true, false) => self.unpinned += 1,
            (false, true) => self.unpinned -= 1,
            _ => {}
        }
        Some(was)
    }
}

/// What [`Store::insert`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inserted {
    /// A new item with this id.
    New(u64),
    /// The content was already stored: that item moved to the top.
    Bumped(u64),
}

/// Numbers for the stats screen (`get_stats`, Phase 9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub items: usize,
    pub pinned: usize,
    /// Estimated payload bytes on disk (see [`disk_cost`]).
    pub disk_bytes: u64,
    /// Bytes currently in the blob and full text cache.
    pub cache_bytes: usize,
    /// Image items whose OCR has not run yet (queued, or OCR is turned off).
    pub ocr_pending: usize,
    /// Image items skipped because no OCR language is installed. The settings
    /// screen shows `ocr::OCR_UNAVAILABLE_HINT` when this is above zero.
    pub ocr_unavailable: usize,
}

pub struct Store {
    items: RwLock<Items>,
    persist: Arc<Persist>,
    cache: cache::BlobCache,
    next_id: AtomicU64,
    /// The OCR queue (`ocr::OcrQueueHandle`). Called once per new image item.
    ocr: Arc<dyn OcrQueue>,
    /// Fired after every change (debounced). `None` in tests that do not care.
    changed: Option<HistoryChanged>,
    /// Shared with `AppState`, so a settings change is seen on the next insert.
    settings: Arc<std::sync::RwLock<Settings>>,
    clock: Clock,
    load_time: Duration,
}

impl Store {
    /// Load the history and build the indexes.
    ///
    /// The startup integrity check (orphan rows and files) already ran inside
    /// [`Persist::open`], so what is read here is consistent. Rows are decrypted
    /// in parallel on a short lived rayon pool (dropped before this returns, so
    /// no idle worker threads stay behind). After loading, retention runs once:
    /// that applies the age limit and any lowered count or disk limit.
    pub fn open(
        persist: Arc<Persist>,
        settings: Arc<std::sync::RwLock<Settings>>,
        ocr: Arc<dyn OcrQueue>,
        changed: Option<HistoryChanged>,
        clock: Clock,
    ) -> Result<Self> {
        let started = Instant::now();
        // The limit applies to encrypted bytes, hence the envelope overhead.
        let rows = persist
            .load_all_meta(TEXT_IN_RAM_MAX + OVERHEAD)
            .context("cannot read the history")?;
        let decoded = decode_rows(&persist, rows);

        let mut items = Items::default();
        // Rows come newest first; add oldest first so seq grows towards the top.
        let mut good = Vec::with_capacity(decoded.len());
        for (id, r) in decoded {
            match r {
                Ok(item) => good.push(item),
                Err(e) => {
                    // An unreadable row (wrong key, corruption) would also block
                    // a re-copy of the same content (UNIQUE hash). Drop it.
                    eprintln!("clipdump: store: dropping unreadable item {id}: {e:#}");
                    let _ = persist.submit(WriteOp::Delete { id });
                }
            }
        }
        for item in good.into_iter().rev() {
            items.add(item);
        }

        let (cache_budget, next_id) = {
            let s = settings.read().unwrap_or_else(|e| e.into_inner());
            (s.ram_cache_bytes, persist.max_id()?.max(0) as u64 + 1)
        };
        let store = Self {
            items: RwLock::new(items),
            persist,
            cache: cache::BlobCache::new(cache_budget),
            next_id: AtomicU64::new(next_id),
            ocr,
            changed,
            settings,
            clock,
            load_time: Duration::ZERO,
        };
        store.run_retention();
        Ok(Self {
            load_time: started.elapsed(),
            ..store
        })
    }

    /// How long [`Store::open`] took (read, decrypt, index, first retention).
    pub fn load_time(&self) -> Duration {
        self.load_time
    }

    /// Shared read access for search. Hold it only while scanning.
    pub fn read(&self) -> RwLockReadGuard<'_, Items> {
        self.items.read()
    }

    pub fn len(&self) -> usize {
        self.items.read().len()
    }

    pub fn stats(&self) -> StoreStats {
        let g = self.items.read();
        // One pass over at most a few thousand items; stats are read rarely
        // (the settings screen), never on the search path.
        let (mut ocr_pending, mut ocr_unavailable) = (0, 0);
        for i in g.iter_newest_first().filter(|i| i.kind == ClipKind::Image) {
            match i.ocr_status {
                OcrStatus::Pending => ocr_pending += 1,
                OcrStatus::Unavailable => ocr_unavailable += 1,
                OcrStatus::Done | OcrStatus::Failed => {}
            }
        }
        StoreStats {
            items: g.len(),
            pinned: g.pinned_count(),
            disk_bytes: g.disk_bytes(),
            cache_bytes: self.cache.bytes(),
            ocr_pending,
            ocr_unavailable,
        }
    }

    /// Bytes held by the blob and full text cache.
    pub fn cache_bytes(&self) -> usize {
        self.cache.bytes()
    }

    /// Re-read the settings that have a live effect: the cache budget, and run
    /// retention once so lowered limits apply immediately (Phase 12 calls this).
    pub fn apply_settings(&self) {
        let budget = self
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .ram_cache_bytes;
        self.cache.set_budget(budget);
        self.run_retention();
    }

    // -- internals shared with retention.rs ---------------------------------

    fn limits(&self) -> Limits {
        Limits::from_settings(&self.settings.read().unwrap_or_else(|e| e.into_inner()))
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// "Now" in unix ms from the store's clock (the search recency boost uses
    /// it, so tests that move the clock move the ranking with it).
    pub fn now_ms(&self) -> i64 {
        self.now()
    }

    /// Queue a persist op. A failure (only after shutdown) is logged, never
    /// fatal: the in-memory state stays usable.
    fn queue(&self, op: WriteOp) {
        if let Err(e) = self.persist.submit(op) {
            eprintln!("clipdump: could not queue a write: {e:#}");
        }
    }

    fn notify(&self) {
        if let Some(c) = &self.changed {
            c.notify();
        }
    }

    // -- operations ----------------------------------------------------------

    /// Store a finished item, or move the existing one with the same hash to the
    /// top and count a use. New items are queued for persistence and, if they
    /// are images, for OCR. Then retention runs (item count and disk cap).
    pub fn insert(&self, item: IngestedItem) -> Inserted {
        let limits = self.limits();
        let mut items = self.items.write();
        // The write lock is held until the persist op is queued, so a concurrent
        // duplicate (two workers, same picture as PNG and DIB) can never queue
        // its `UpdateUsed` ahead of the `Insert` it refers to. Queueing is a
        // channel push, so the lock is held for microseconds.
        if let Some(id) = items.id_for_hash(item.hash) {
            let at = items.stamp(self.now());
            items.bump(id, at);
            self.queue(WriteOp::UpdateUsed { id: id as i64, at });
            drop(items);
            self.notify();
            return Inserted::Bumped(id);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let last_used_at = items.stamp(item.created_at);
        let ocr_job = (item.kind == ClipKind::Image).then_some(OcrJob {
            item_id: id as i64,
            hash: item.hash,
            width: item.width,
            height: item.height,
        });
        let (clip, op) = build_new(id, last_used_at, item);
        items.add(clip);
        self.queue(op);
        self.enforce_limits(&mut items, &limits);
        drop(items);
        if let Some(job) = ocr_job {
            self.ocr.enqueue(job);
        }
        self.notify();
        Inserted::New(id)
    }

    /// Search the history (Phase 8). The read lock is held only while ranking
    /// (a scan of the in-memory text, no disk, no decryption); the result rows,
    /// including the file existence checks, are built after it is released, so a
    /// slow disk can never block an ingest worker.
    pub fn search(&self, req: &SearchRequest<'_>, paths: &dyn PathChecker) -> Vec<ResultDto> {
        let ranked = {
            let items = self.items.read();
            search::rank(items.iter_newest_first(), req)
        };
        search::render(&ranked, req, paths)
    }

    /// A snapshot of one item.
    pub fn get(&self, id: u64) -> Option<Arc<ClipItem>> {
        self.items.read().get(id).cloned()
    }

    /// Delete one item (pinned or not; an explicit delete always wins). The
    /// writer thread removes its blob and thumbnail files after the row is gone.
    pub fn remove(&self, id: u64) -> bool {
        let mut items = self.items.write();
        if items.remove(id).is_none() {
            return false;
        }
        // Queued while the lock is held: if the same content is copied again
        // right after, its `Insert` must come after this `Delete` (the hash
        // column is UNIQUE).
        self.queue(WriteOp::Delete { id: id as i64 });
        self.cache.remove(id);
        drop(items);
        self.notify();
        true
    }

    /// Flip the pinned flag; returns the new state, `None` if the id is unknown.
    /// Unpinning can push the unpinned count over the limit, so retention runs.
    pub fn toggle_pin(&self, id: u64) -> Option<bool> {
        let limits = self.limits();
        let mut items = self.items.write();
        let now_pinned = !items.get(id)?.pinned;
        items.set_pinned(id, now_pinned)?;
        self.queue(WriteOp::SetPinned {
            id: id as i64,
            pinned: now_pinned,
        });
        if !now_pinned {
            self.enforce_limits(&mut items, &limits);
        }
        drop(items);
        self.notify();
        Some(now_pinned)
    }

    /// "Clear history": delete every unpinned item. Returns how many.
    pub fn clear_unpinned(&self) -> usize {
        let mut items = self.items.write();
        let ids: Vec<u64> = items
            .iter_oldest_first()
            .filter(|i| !i.pinned)
            .map(|i| i.id)
            .collect();
        for &id in &ids {
            items.remove(id);
            self.cache.remove(id);
        }
        // One op for all of them (a single DELETE ... WHERE pinned = 0).
        if !ids.is_empty() {
            self.queue(WriteOp::DeleteUnpinned);
        }
        drop(items);
        if !ids.is_empty() {
            self.notify();
        }
        ids.len()
    }

    /// The item was copied back (Phase 11): move it to the top and count a use.
    pub fn mark_used(&self, id: u64) -> bool {
        let mut items = self.items.write();
        let at = items.stamp(self.now());
        if !items.bump(id, at) {
            return false;
        }
        self.queue(WriteOp::UpdateUsed { id: id as i64, at });
        drop(items);
        self.notify();
        true
    }

    /// The app wrote this hash to the clipboard itself (self-write echo): move
    /// that item to the top. Unknown hashes are ignored.
    fn bump_hash(&self, hash: u128) {
        let mut items = self.items.write();
        let Some(id) = items.id_for_hash(hash) else {
            return;
        };
        let at = items.stamp(self.now());
        items.bump(id, at);
        self.queue(WriteOp::UpdateUsed { id: id as i64, at });
        drop(items);
        self.notify();
    }

    /// Record an OCR result (Phase 7). `text` is the full recognized text, or
    /// `None` on failure. RAM keeps only the first 10,000 chars as the item's
    /// search text; SQLite gets all of it.
    pub fn set_ocr_text(&self, id: u64, text: Option<String>, status: OcrStatus) -> bool {
        let search: Arc<str> = text
            .as_deref()
            .map_or_else(|| Arc::from(""), |t| Arc::from(search_slice(t)));
        let mut items = self.items.write();
        let found = items.update(id, |it| {
            it.set_search_text(search);
            it.ocr_status = status;
        });
        if !found {
            return false;
        }
        self.queue(WriteOp::SetOcr {
            id: id as i64,
            text,
            status,
        });
        drop(items);
        self.notify();
        true
    }

    // -- payloads that live on disk -------------------------------------------

    /// The full text of a text item as UTF-8 bytes (validated when stored).
    /// Small texts come from RAM; big ones from the cache, else SQLite.
    ///
    /// A text that was inserted less than one writer batch ago (about 50 ms)
    /// may not be readable from SQLite yet; the error then says so and the
    /// caller can retry. Not reachable by a human, who needs longer than that to
    /// open the bar.
    pub fn get_full_text(&self, id: u64) -> Result<Arc<[u8]>> {
        let item = self.get(id).context("no such item")?;
        if item.kind != ClipKind::Text {
            anyhow::bail!("item {id} is not a text");
        }
        if let Some(text) = &item.text_in_ram {
            return Ok(Arc::from(text.as_bytes()));
        }
        self.cache.get_or_load(id, || {
            let text = self
                .persist
                .load_text(id as i64)?
                .context("the text is not stored (yet)")?;
            Ok(text.into_bytes())
        })
    }

    /// The PNG bytes of an image item: from the cache, else read from disk and
    /// decrypted. A blob larger than the cache budget is returned but not
    /// cached. Same 50 ms write delay caveat as [`Store::get_full_text`].
    pub fn get_blob(&self, id: u64) -> Result<Arc<[u8]>> {
        let item = self.get(id).context("no such item")?;
        let name = item
            .blob_hash
            .as_deref()
            .with_context(|| format!("item {id} has no image"))?
            .to_owned();
        self.cache.get_or_load(id, || self.persist.read_blob(&name))
    }

    /// The persistence handle (shutdown flushes it, tests read through it).
    pub fn persist(&self) -> &Arc<Persist> {
        &self.persist
    }
}

impl ItemSink for Store {
    fn submit(&self, item: IngestedItem) {
        self.insert(item);
    }

    fn bump(&self, hash: u128) {
        self.bump_hash(hash);
    }
}

// ---------------------------------------------------------------------------
// Building items
// ---------------------------------------------------------------------------

/// The in-memory item and the persist op for a new ingested item. Large fields
/// are moved to the op; RAM gets its own copies of only what it keeps (a text
/// of at most 64 KB, a roughly 10 KB thumbnail, the file list).
fn build_new(id: u64, last_used_at: i64, it: IngestedItem) -> (ClipItem, WriteOp) {
    let IngestedItem {
        kind,
        hash,
        preview,
        search_text,
        text,
        png,
        thumb,
        files,
        width,
        height,
        byte_size,
        source_app,
        created_at,
        ocr_status,
    } = it;
    let is_image = kind == ClipKind::Image;
    let hash_bytes = hash.to_be_bytes();
    // The blob is named after the item hash, like the thumbnail.
    let blob_hash = is_image.then(|| hash_hex(&hash_bytes));

    let text_in_ram: Option<Arc<str>> = text
        .as_deref()
        .filter(|t| t.len() <= TEXT_IN_RAM_MAX)
        .map(Arc::from);
    // When the whole text is within the search slice (the common case for short
    // copies) share one allocation instead of keeping two copies.
    let search: Arc<str> = match &text_in_ram {
        Some(full) if **full == *search_text => Arc::clone(full),
        _ => Arc::from(search_text),
    };

    let clip = ClipItem {
        id,
        hash,
        kind,
        created_at,
        last_used_at,
        use_count: 0,
        pinned: false,
        preview: Arc::from(preview.as_str()),
        search_lower: fold_for_search(&search),
        search_text: search,
        text_in_ram,
        thumb: thumb.as_deref().map(Arc::from),
        files: files.as_deref().map(Arc::from),
        width,
        height,
        byte_size,
        ocr_status,
        source_app: source_app.as_deref().map(Arc::from),
        blob_hash: blob_hash.as_deref().map(Arc::from),
    };
    let new = NewItem {
        id: id as i64,
        hash: hash_bytes,
        kind,
        created_at,
        last_used_at,
        use_count: 0,
        pinned: false,
        preview,
        text,
        ocr: None,
        ocr_status,
        files,
        blob_hash,
        width: is_image.then_some(width),
        height: is_image.then_some(height),
        byte_size,
        source_app,
    };
    let op = WriteOp::Insert {
        item: Box::new(new),
        blob: png,
        thumb,
    };
    (clip, op)
}

/// Decrypt one stored row into an item.
fn item_from_row(persist: &Persist, row: MetaRow) -> Result<ClipItem> {
    let crypto: &Crypto = persist.crypto();
    let dec = |b: &[u8]| -> Result<String> {
        String::from_utf8(crypto.decrypt(b)?).context("stored text is not UTF-8")
    };
    let id = u64::try_from(row.id).context("negative item id")?;
    let preview = dec(&row.preview_enc)?;

    let mut text_in_ram = None;
    let mut files_list: Option<Vec<FileRef>> = None;
    let search_text: Arc<str> = match row.kind {
        ClipKind::Text => {
            let full = match &row.text_enc {
                Some(enc) => {
                    // Small text: it came with the row.
                    let t = dec(enc)?;
                    text_in_ram = Some(Arc::<str>::from(t.as_str()));
                    Some(t)
                }
                // Big text: only its first 10,000 chars are needed. This is the
                // one place the startup reads a multi megabyte value, and big
                // texts are rare. The rest is dropped right away.
                None if row.has_text => persist.load_text(row.id)?,
                None => None,
            };
            match (&text_in_ram, &full) {
                (Some(arc), Some(t)) if search_slice(t).len() == t.len() => Arc::clone(arc),
                (_, Some(t)) => Arc::from(search_slice(t)),
                _ => Arc::from(preview.as_str()),
            }
        }
        ClipKind::Image => match &row.ocr_enc {
            Some(enc) => Arc::from(search_slice(&dec(enc)?)),
            None => Arc::from(""),
        },
        ClipKind::Files => {
            let list: Vec<FileRef> = match &row.files_enc {
                Some(enc) => serde_json::from_slice(&crypto.decrypt(enc)?)
                    .context("stored file list is not valid")?,
                None => Vec::new(),
            };
            let s = Arc::from(files::search_text(&list));
            files_list = Some(list);
            s
        }
    };

    // Images and image files have a thumbnail named after the hash. A missing
    // or unreadable one only costs the preview picture.
    let thumb = match row.kind {
        ClipKind::Text => None,
        _ => persist.read_thumb(&row.hash).ok().map(Arc::<[u8]>::from),
    };

    Ok(ClipItem {
        id,
        hash: u128::from_be_bytes(row.hash),
        kind: row.kind,
        created_at: row.created_at,
        last_used_at: row.last_used_at,
        use_count: row.use_count,
        pinned: row.pinned,
        preview: Arc::from(preview),
        search_lower: fold_for_search(&search_text),
        search_text,
        text_in_ram,
        thumb,
        files: files_list.map(Arc::from),
        width: row.width.unwrap_or(0),
        height: row.height.unwrap_or(0),
        byte_size: row.byte_size,
        ocr_status: row.ocr_status,
        source_app: row.source_app.as_deref().map(Arc::from),
        blob_hash: row.blob_hash.as_deref().map(Arc::from),
    })
}

/// Decrypt all rows, in parallel when a pool can be built (rows keep their
/// order). Decryption of the preview, the text, OCR text and the thumbnail
/// file read are independent per row, so this scales with cores.
fn decode_rows(persist: &Persist, rows: Vec<MetaRow>) -> Vec<(i64, Result<ClipItem>)> {
    let one = |r: MetaRow| {
        let id = r.id;
        (id, item_from_row(persist, r))
    };
    let threads = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .min(8);
    // A dedicated pool, dropped on return: the global rayon pool would leave one
    // idle thread per core alive for the life of the app.
    match rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("clipdump-load-{i}"))
        .build()
    {
        Ok(pool) => pool.install(|| rows.into_par_iter().map(one).collect()),
        Err(_) => rows.into_iter().map(one).collect(),
    }
}
