//! OCR: text recognition for image items, on one background thread.
//!
//! ```text
//! Store::insert (new image) --OcrQueue::enqueue--> OcrQueueHandle  (deduped queue)
//!                                                        |
//!                                  OcrWorker thread (idle priority, owns the engine)
//!                                                        |
//!   Store::get_blob (PNG, retried) -> decode -> OcrEngine::recognize -> Store::set_ocr_text
//! ```
//!
//! # Rules this module keeps
//!
//! * OCR never runs on a hot path. Enqueueing is a short mutex push; every
//!   decode and recognition happens on the worker, which runs at the lowest
//!   thread priority ([`crate::platform::lower_current_thread_priority_idle`]).
//! * A job carries no pixels (see `OcrJob`). The worker reloads the PNG blob
//!   through the store, so a backlog of 4K screenshots costs no RAM.
//! * The queue is deduplicated by item id (queued or running), so a startup
//!   re-queue and a live insert can never OCR one item twice at the same time.
//! * Jobs for items that were deleted meanwhile are skipped.
//! * The engine is created on the worker thread (WinRT wants the apartment of
//!   the thread that uses it) from an [`EngineFactory`]. Tests pass a mock.
//! * Dropping the [`OcrWorker`] stops it: queued jobs are discarded (their items
//!   stay `Pending` and are re-queued at the next start), the job in flight
//!   finishes first.
//!
//! The engine itself is behind [`OcrEngine`]. Windows implements it in
//! `platform/windows/ocr.rs` (Windows.Media.Ocr); other platforms get an
//! engine that reports itself unavailable.
//!
//! Logging: one line per job with the result length and the elapsed time, never
//! the recognized text.

pub mod prepare;
#[cfg(test)]
mod tests;

use std::collections::{HashSet, VecDeque};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use parking_lot::{Condvar, Mutex};

use crate::ingest::{OcrJob, OcrQueue};
use crate::model::{ClipKind, OcrStatus};
use crate::settings::Settings;
use crate::store::Store;

/// What the settings screen shows when no OCR language is installed (Phase 10
/// reads it; it lives here so the wording exists in one place).
pub const OCR_UNAVAILABLE_HINT: &str = "No OCR language is installed, so text in screenshots cannot be searched. \
     Add a language pack in Windows: Settings > Time & language > Language & region > Add a language. \
     Then restart Clipdump.";

/// A text recognition engine. One instance lives on the worker thread, so it
/// needs no `Send` or `Sync`.
pub trait OcrEngine {
    /// Recognize the text in a picture of `w` x `h` pixels, `rgba` being
    /// `w * h * 4` bytes, top row first. Lines are joined with `\n`. An empty
    /// string is a valid result (a picture without text).
    fn recognize(&self, rgba: &[u8], w: u32, h: u32) -> Result<String>;

    /// False when this machine has no OCR language (or no engine at all).
    fn available(&self) -> bool;
}

/// Builds the engine on the worker thread (see the module docs).
pub type EngineFactory = Box<dyn FnOnce() -> Box<dyn OcrEngine> + Send>;

/// What the worker knows about its engine, for the settings screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineState {
    /// The worker has not looked yet (it starts a moment after the app).
    Unknown,
    Available,
    /// No OCR language: show [`OCR_UNAVAILABLE_HINT`].
    Unavailable,
}

impl EngineState {
    fn code(self) -> u8 {
        self as u8
    }

    fn from_code(c: u8) -> Self {
        match c {
            1 => Self::Available,
            2 => Self::Unavailable,
            _ => Self::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// Waiting for a blob that is not on disk yet
// ---------------------------------------------------------------------------

/// How long and how often to retry reading a blob that is not readable yet.
///
/// Why this exists: `Store::insert` queues the PNG for the persist writer, which
/// commits in batches (every 50 ms). The OCR job is enqueued right after the
/// insert, so the worker can ask for the blob a few milliseconds before the
/// writer has put it on disk (the caveat documented on `Store::get_blob`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Stop retrying after sleeping this long in total.
    pub total: Duration,
    /// First sleep; doubles each time up to `max_step`.
    pub first_step: Duration,
    pub max_step: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            total: Duration::from_secs(2),
            first_step: Duration::from_millis(10),
            max_step: Duration::from_millis(250),
        }
    }
}

/// Result of one attempt to load something.
pub enum Attempt<T> {
    Ready(T),
    /// Not there yet (or a read error): try again later.
    Retry(anyhow::Error),
    /// It never will be (the item was deleted): stop at once.
    Gone,
}

/// Final result of [`retry_load`].
#[derive(Debug)]
pub enum Loaded<T> {
    Ready(T),
    Gone,
    /// `sleep` asked to stop (the worker is shutting down).
    Shutdown,
    /// Still failing after the whole budget; carries the last error.
    Failed(anyhow::Error),
}

/// Call `attempt(n)` (n counts from 0) until it is ready, gone or the budget in
/// `policy` is used up. The first retry (n = 1) follows the first failure
/// without sleeping, so the caller can use it to force the writer to commit
/// (`Persist::flush`); later retries back off. `sleep` returns false to abort.
///
/// The budget is the sum of the sleeps, not wall time, so tests can drive it
/// with a fake `sleep`; the attempts themselves add only a few milliseconds.
pub fn retry_load<T>(
    policy: &RetryPolicy,
    mut attempt: impl FnMut(u32) -> Attempt<T>,
    mut sleep: impl FnMut(Duration) -> bool,
) -> Loaded<T> {
    let mut waited = Duration::ZERO;
    let mut step = policy.first_step;
    let mut n = 0u32;
    loop {
        let err = match attempt(n) {
            Attempt::Ready(v) => return Loaded::Ready(v),
            Attempt::Gone => return Loaded::Gone,
            Attempt::Retry(e) => e,
        };
        n += 1;
        if n == 1 {
            continue;
        }
        if waited >= policy.total {
            return Loaded::Failed(err);
        }
        let d = step.min(policy.total - waited);
        if !sleep(d) {
            return Loaded::Shutdown;
        }
        waited += d;
        step = (step * 2).min(policy.max_step);
    }
}

// ---------------------------------------------------------------------------
// The queue
// ---------------------------------------------------------------------------

#[derive(Default)]
struct State {
    queue: VecDeque<OcrJob>,
    /// Item ids in `queue`, for O(1) dedup.
    queued: HashSet<i64>,
    /// The item the worker is on right now.
    in_flight: Option<i64>,
    closed: bool,
    /// Scan the store for pending (and now possible) items.
    rescan: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    settings: Arc<RwLock<Settings>>,
    /// `ocr_enabled` as of the last [`OcrQueueHandle::apply_settings`], to see
    /// the off to on edge.
    enabled_seen: AtomicBool,
    engine: AtomicU8,
}

impl Shared {
    fn enabled(&self) -> bool {
        self.settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .ocr_enabled
    }

    fn set_engine(&self, available: bool) {
        let s = if available {
            EngineState::Available
        } else {
            EngineState::Unavailable
        };
        self.engine.store(s.code(), Ordering::Relaxed);
    }

    /// Block until there is something to do.
    fn next(&self) -> Step {
        let mut st = self.state.lock();
        loop {
            if st.closed {
                return Step::Stop;
            }
            if st.rescan {
                st.rescan = false;
                return Step::Rescan;
            }
            if let Some(job) = st.queue.pop_front() {
                st.queued.remove(&job.item_id);
                st.in_flight = Some(job.item_id);
                return Step::Job(job);
            }
            self.cv.wait(&mut st);
        }
    }

    fn finish(&self) {
        self.state.lock().in_flight = None;
    }

    fn closed(&self) -> bool {
        self.state.lock().closed
    }

    /// Sleep for `d`, waking early only to stop. False means "shutting down".
    fn sleep(&self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        let mut st = self.state.lock();
        loop {
            if st.closed {
                return false;
            }
            if Instant::now() >= deadline {
                return true;
            }
            // Other wake-ups (a new job) are ignored until the deadline.
            self.cv.wait_until(&mut st, deadline);
        }
    }
}

enum Step {
    Stop,
    Rescan,
    Job(OcrJob),
}

/// The OCR queue the store hands new image items to (an [`OcrQueue`]). Cheap to
/// clone; all clones share one queue. It can exist before the worker: jobs
/// simply wait.
#[derive(Clone)]
pub struct OcrQueueHandle {
    shared: Arc<Shared>,
}

impl OcrQueueHandle {
    /// A queue that follows `settings.ocr_enabled`.
    pub fn new(settings: Arc<RwLock<Settings>>) -> Self {
        let enabled = settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .ocr_enabled;
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                cv: Condvar::new(),
                settings,
                enabled_seen: AtomicBool::new(enabled),
                engine: AtomicU8::new(EngineState::Unknown.code()),
            }),
        }
    }

    /// Call after `ocr_enabled` may have changed (Phase 12 calls it when the
    /// settings are saved). Turning OCR off drops the waiting jobs, their items
    /// stay `Pending`; turning it on makes the worker scan the store and queue
    /// every pending image (and the unavailable ones if an OCR language has
    /// appeared since).
    pub fn apply_settings(&self) {
        let enabled = self.shared.enabled();
        let was = self.shared.enabled_seen.swap(enabled, Ordering::AcqRel);
        let mut st = self.shared.state.lock();
        if !enabled {
            st.queue.clear();
            st.queued.clear();
        } else if !was {
            st.rescan = true;
        }
        drop(st);
        self.shared.cv.notify_all();
    }

    /// Ask the worker to scan the store for pending images (startup).
    fn request_rescan(&self) {
        self.shared.state.lock().rescan = true;
        self.shared.cv.notify_all();
    }

    /// Jobs waiting plus the one running.
    pub fn backlog(&self) -> usize {
        let st = self.shared.state.lock();
        st.queue.len() + usize::from(st.in_flight.is_some())
    }

    /// Nothing queued, nothing running, no scan requested.
    pub fn is_idle(&self) -> bool {
        let st = self.shared.state.lock();
        st.queue.is_empty() && st.in_flight.is_none() && !st.rescan
    }

    /// Whether an OCR language is installed, as far as the worker has seen.
    pub fn engine_state(&self) -> EngineState {
        EngineState::from_code(self.shared.engine.load(Ordering::Relaxed))
    }

    fn close(&self) {
        let mut st = self.shared.state.lock();
        st.closed = true;
        st.queue.clear();
        st.queued.clear();
        drop(st);
        self.shared.cv.notify_all();
    }
}

impl OcrQueue for OcrQueueHandle {
    /// Queue a new image item. Ignored while OCR is off, when the id is already
    /// queued or running, and after shutdown.
    fn enqueue(&self, job: OcrJob) {
        if !self.shared.enabled() {
            return;
        }
        let mut st = self.shared.state.lock();
        if st.closed || st.in_flight == Some(job.item_id) || !st.queued.insert(job.item_id) {
            return;
        }
        st.queue.push_back(job);
        drop(st);
        self.shared.cv.notify_one();
    }
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

/// Tunables; tests shorten the retry budget.
#[derive(Clone, Copy, Debug, Default)]
pub struct OcrConfig {
    pub retry: RetryPolicy,
}

/// The OCR thread. Dropping it stops the thread (see the module docs).
pub struct OcrWorker {
    queue: OcrQueueHandle,
    join: Option<JoinHandle<()>>,
}

impl OcrWorker {
    /// Start the thread. It builds the engine, scans `store` for pending items
    /// (startup re-queue) and then serves `queue`. It holds the store only
    /// weakly: if the store is gone the thread ends.
    pub fn start(
        queue: OcrQueueHandle,
        store: Weak<Store>,
        factory: EngineFactory,
        config: OcrConfig,
    ) -> io::Result<Self> {
        queue.request_rescan();
        let shared = Arc::clone(&queue.shared);
        let join = std::thread::Builder::new()
            .name("clipdump-ocr".into())
            .spawn(move || run(&shared, &store, factory, config))?;
        Ok(Self {
            queue,
            join: Some(join),
        })
    }

    /// The queue this worker serves.
    pub fn queue(&self) -> &OcrQueueHandle {
        &self.queue
    }

    /// See [`OcrQueueHandle::apply_settings`].
    pub fn apply_settings(&self) {
        self.queue.apply_settings();
    }
}

impl Drop for OcrWorker {
    fn drop(&mut self) {
        self.queue.close();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn run(shared: &Arc<Shared>, store: &Weak<Store>, factory: EngineFactory, cfg: OcrConfig) {
    // Lowest priority: OCR may take a second of CPU and must never be felt.
    if let Err(e) = crate::platform::lower_current_thread_priority_idle() {
        eprintln!("clipdump: could not lower the OCR thread priority: {e}");
    }
    let engine = factory();
    let available = engine.available();
    shared.set_engine(available);
    if !available {
        eprintln!("clipdump: ocr: no OCR language installed, screenshots will not be searchable");
    }
    loop {
        match shared.next() {
            Step::Stop => break,
            Step::Rescan => {
                let Some(store) = store.upgrade() else { break };
                rescan(shared, &store, &*engine);
            }
            Step::Job(job) => {
                let Some(store) = store.upgrade() else { break };
                if shared.enabled() {
                    process_job(shared, &store, &*engine, &job, &cfg);
                }
                shared.finish();
            }
        }
    }
}

/// Queue every image whose OCR is not settled: `Pending` (the app closed before
/// it ran, or OCR was off) and, if an engine is available now, `Unavailable`
/// (a language was installed since). Newest first, so the screenshot the user
/// just took is searchable before an old backlog is.
fn rescan(shared: &Arc<Shared>, store: &Store, engine: &dyn OcrEngine) {
    if !shared.enabled() {
        return;
    }
    let available = engine.available();
    shared.set_engine(available);
    let jobs: Vec<OcrJob> = store
        .read()
        .iter_newest_first()
        .filter(|i| i.kind == ClipKind::Image)
        .filter(|i| match i.ocr_status {
            OcrStatus::Pending => true,
            OcrStatus::Unavailable => available,
            OcrStatus::Done | OcrStatus::Failed => false,
        })
        .map(|i| OcrJob {
            item_id: i.id as i64,
            hash: i.hash,
            width: i.width,
            height: i.height,
        })
        .collect();
    if !jobs.is_empty() {
        eprintln!(
            "clipdump: ocr: queued {} images from the history",
            jobs.len()
        );
    }
    let handle = OcrQueueHandle {
        shared: Arc::clone(shared),
    };
    for job in jobs {
        handle.enqueue(job);
    }
}

/// Run one job and record the outcome in the store. Every path ends with the
/// item in a settled status, or untouched when it vanished or on shutdown.
fn process_job(
    shared: &Arc<Shared>,
    store: &Store,
    engine: &dyn OcrEngine,
    job: &OcrJob,
    cfg: &OcrConfig,
) {
    let id = job.item_id as u64;
    let Some(item) = store.get(id) else { return };
    if item.kind != ClipKind::Image || item.ocr_status == OcrStatus::Done {
        return;
    }
    let available = engine.available();
    shared.set_engine(available);
    if !available {
        store.set_ocr_text(id, None, OcrStatus::Unavailable);
        return;
    }

    let started = Instant::now();
    let png = match load_png(shared, store, id, &cfg.retry) {
        Loaded::Ready(p) => p,
        Loaded::Gone | Loaded::Shutdown => return,
        Loaded::Failed(e) => {
            eprintln!("clipdump: ocr: item {id}: image not readable: {e:#}");
            store.set_ocr_text(id, None, OcrStatus::Failed);
            return;
        }
    };
    if shared.closed() {
        return;
    }
    let result = recognize_png(engine, &png);
    drop(png);
    match result {
        Ok(text) => {
            eprintln!(
                "clipdump: ocr: item {id}: {} chars in {} ms",
                text.chars().count(),
                started.elapsed().as_millis()
            );
            // An empty text is a finished OCR (a picture without words).
            store.set_ocr_text(id, Some(text), OcrStatus::Done);
        }
        Err(e) => {
            eprintln!("clipdump: ocr: item {id}: failed: {e:#}");
            store.set_ocr_text(id, None, OcrStatus::Failed);
        }
    }
}

/// The PNG of image item `id`, waiting for the persist writer if needed.
fn load_png(shared: &Shared, store: &Store, id: u64, policy: &RetryPolicy) -> Loaded<Arc<[u8]>> {
    retry_load(
        policy,
        |n| {
            if n == 1 {
                // First retry: make the writer commit what is queued. It
                // returns after at most one batch (about 50 ms). The result is
                // ignored: a failed op elsewhere is not our problem.
                let _ = store.persist().flush();
            }
            match store.get_blob(id) {
                Ok(b) => Attempt::Ready(b),
                Err(_) if store.get(id).is_none() => Attempt::Gone,
                Err(e) => Attempt::Retry(e),
            }
        },
        |d| shared.sleep(d),
    )
}

/// Decode the PNG and run the engine. A panic inside the engine becomes an
/// error, so one bad picture cannot take the worker down (in a release build
/// `panic = "abort"` makes this a no-op, as for the ingest workers).
fn recognize_png(engine: &dyn OcrEngine, png: &[u8]) -> Result<String> {
    let img = crate::ingest::image::decode_png(png).context("cannot decode the stored PNG")?;
    catch_unwind(AssertUnwindSafe(|| {
        engine.recognize(&img.pixels, img.width, img.height)
    }))
    .unwrap_or_else(|_| Err(anyhow::anyhow!("the OCR engine panicked")))
}
