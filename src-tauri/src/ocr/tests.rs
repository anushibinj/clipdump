//! Worker tests with a mock engine: results, failures, skipping, dedup, the
//! blob retry, the startup re-queue, the settings switch and shutdown. Every
//! test runs the real store and persistence over a temp directory.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use super::*;
use crate::ingest::changed::HistoryChanged;
use crate::ingest::image::tests::{pic_gradient, png_of};
use crate::persist::tests::open;
use crate::platform::{ImageData, ImageFormat};
use crate::store::{Inserted, system_clock};

// ---------------------------------------------------------------------------
// retry_load, the pure part
// ---------------------------------------------------------------------------

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn retry_succeeds_after_a_few_failures_with_growing_sleeps() {
    let mut sleeps = Vec::new();
    let r = retry_load(
        &RetryPolicy::default(),
        |n| {
            if n < 5 {
                Attempt::Retry(anyhow::anyhow!("not yet"))
            } else {
                Attempt::Ready(n)
            }
        },
        |d| {
            sleeps.push(d);
            true
        },
    );
    assert!(matches!(r, Loaded::Ready(5)));
    // Attempt 1 follows attempt 0 at once (no sleep); attempts 2 to 5 follow
    // sleeps of 10, 20, 40 and 80 ms.
    assert_eq!(sleeps, [ms(10), ms(20), ms(40), ms(80)]);
}

#[test]
fn retry_gives_up_after_the_budget_with_the_last_error() {
    let mut slept = Duration::ZERO;
    let mut longest = Duration::ZERO;
    let mut tries = 0;
    let r: Loaded<()> = retry_load(
        &RetryPolicy::default(),
        |n| {
            tries += 1;
            Attempt::Retry(anyhow::anyhow!("attempt {n}"))
        },
        |d| {
            slept += d;
            longest = longest.max(d);
            true
        },
    );
    let Loaded::Failed(e) = r else {
        panic!("expected a failure")
    };
    assert_eq!(slept, Duration::from_secs(2), "the whole budget, no more");
    assert_eq!(longest, ms(250), "the backoff is capped");
    assert_eq!(
        e.to_string(),
        format!("attempt {}", tries - 1),
        "last error"
    );
}

#[test]
fn retry_stops_at_once_when_gone_or_on_shutdown() {
    let r: Loaded<()> = retry_load(&RetryPolicy::default(), |_| Attempt::Gone, |_| panic!());
    assert!(matches!(r, Loaded::Gone));
    let r: Loaded<()> = retry_load(
        &RetryPolicy::default(),
        |_| Attempt::Retry(anyhow::anyhow!("x")),
        |_| false,
    );
    assert!(matches!(r, Loaded::Shutdown));
    // A hit on the first try never sleeps.
    let r = retry_load(&RetryPolicy::default(), |_| Attempt::Ready(1), |_| panic!());
    assert!(matches!(r, Loaded::Ready(1)));
}

// ---------------------------------------------------------------------------
// The mock engine and the fixture
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reply {
    /// `"text WxH"`, so a test can tell the pictures apart.
    Text,
    Empty,
    Error,
    Panic,
}

struct Ctl {
    available: AtomicBool,
    /// While true, `recognize` blocks after recording the call.
    hold: AtomicBool,
    calls: Mutex<Vec<(u32, u32)>>,
    reply: Mutex<Reply>,
}

impl Ctl {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            available: AtomicBool::new(true),
            hold: AtomicBool::new(false),
            calls: Mutex::new(Vec::new()),
            reply: Mutex::new(Reply::Text),
        })
    }

    fn calls(&self) -> Vec<(u32, u32)> {
        self.calls.lock().clone()
    }
}

struct MockEngine(Arc<Ctl>);

impl OcrEngine for MockEngine {
    fn recognize(&self, rgba: &[u8], w: u32, h: u32) -> Result<String> {
        assert_eq!(rgba.len(), (w * h * 4) as usize, "a full RGBA buffer");
        self.0.calls.lock().push((w, h));
        while self.0.hold.load(Ordering::SeqCst) {
            std::thread::sleep(ms(2));
        }
        match *self.0.reply.lock() {
            Reply::Text => Ok(format!("text {w}x{h}")),
            Reply::Empty => Ok(String::new()),
            Reply::Error => Err(anyhow::anyhow!("engine broke")),
            Reply::Panic => panic!("engine panicked on purpose"),
        }
    }

    fn available(&self) -> bool {
        self.0.available.load(Ordering::SeqCst)
    }
}

struct Fx {
    dir: tempfile::TempDir,
    settings: Arc<RwLock<Settings>>,
    ctl: Arc<Ctl>,
    queue: OcrQueueHandle,
    store: Arc<Store>,
    worker: Option<OcrWorker>,
    events: Arc<AtomicUsize>,
}

fn short_retry() -> OcrConfig {
    OcrConfig {
        retry: RetryPolicy {
            total: ms(300),
            first_step: ms(10),
            max_step: ms(50),
        },
    }
}

impl Fx {
    /// A store, a queue and a running worker with an available engine.
    fn new() -> Self {
        let mut fx = Self::bare();
        fx.start_worker(OcrConfig::default());
        fx
    }

    /// Store and queue, no worker: queued jobs just wait.
    fn bare() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self::open_in(
            dir,
            Arc::new(RwLock::new(Settings::default())),
            Ctl::new(),
            Arc::new(AtomicUsize::new(0)),
        )
    }

    fn open_in(
        dir: tempfile::TempDir,
        settings: Arc<RwLock<Settings>>,
        ctl: Arc<Ctl>,
        events: Arc<AtomicUsize>,
    ) -> Self {
        let queue = OcrQueueHandle::new(Arc::clone(&settings));
        let e = Arc::clone(&events);
        let changed = HistoryChanged::with_interval(Duration::ZERO, move || {
            e.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        let store = Arc::new(
            Store::open(
                Arc::new(open(dir.path())),
                Arc::clone(&settings),
                Arc::new(queue.clone()),
                Some(changed),
                system_clock(),
            )
            .unwrap(),
        );
        Self {
            dir,
            settings,
            ctl,
            queue,
            store,
            worker: None,
            events,
        }
    }

    fn start_worker(&mut self, cfg: OcrConfig) {
        let ctl = Arc::clone(&self.ctl);
        let factory: EngineFactory = Box::new(move || Box::new(MockEngine(ctl)));
        self.worker = Some(
            OcrWorker::start(
                self.queue.clone(),
                Arc::downgrade(&self.store),
                factory,
                cfg,
            )
            .unwrap(),
        );
    }

    /// Stop the worker, flush, drop the store and open the directory again.
    fn reopen(mut self, start_worker: bool) -> Self {
        self.worker = None;
        self.store.persist().flush().unwrap();
        let Fx {
            dir,
            settings,
            ctl,
            store,
            events,
            ..
        } = self;
        drop(store);
        let mut fx = Self::open_in(dir, settings, ctl, events);
        if start_worker {
            fx.start_worker(OcrConfig::default());
        }
        fx
    }

    /// Insert the n-th test picture (a real PNG, `40 + n` by `30 + n` pixels).
    fn image(&self, n: u32) -> u64 {
        let pic = pic_gradient(40 + n, 30 + n);
        let data = ImageData {
            format: ImageFormat::Png,
            bytes: png_of(&pic),
        };
        // Created "now": older items would be evicted by the age limit at reopen.
        let now = (system_clock())();
        let item = crate::ingest::image::build(data, None, now).unwrap();
        match self.store.insert(item) {
            Inserted::New(id) | Inserted::Bumped(id) => id,
        }
    }

    fn status(&self, id: u64) -> OcrStatus {
        self.store.get(id).unwrap().ocr_status
    }

    fn text(&self, id: u64) -> String {
        self.store.get(id).unwrap().search_text.to_string()
    }

    fn wait_idle(&self) {
        wait_for("the queue to go idle", || self.queue.is_idle());
    }

    fn wait_calls(&self, n: usize) {
        wait_for("the engine to be called", || self.ctl.calls().len() >= n);
    }
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(ms(2));
    }
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

#[test]
fn a_result_updates_search_text_persists_and_notifies() {
    let fx = Fx::new();
    let id = fx.image(1);
    let before = fx.events.load(Ordering::SeqCst);
    assert!(before >= 1, "the insert notified");
    fx.wait_idle();
    assert_eq!(fx.status(id), OcrStatus::Done);
    assert_eq!(fx.text(id), "text 41x31");
    assert!(
        fx.events.load(Ordering::SeqCst) > before,
        "history:changed fired for the OCR result"
    );

    // After a restart (no worker, so nothing can touch it) the text is there.
    let fx = fx.reopen(false);
    assert_eq!(fx.status(id), OcrStatus::Done);
    assert_eq!(fx.text(id), "text 41x31");
    let meta = fx.store.persist().load_all_meta(0).unwrap();
    let row = meta.iter().find(|m| m.id == id as i64).unwrap();
    let plain = fx
        .store
        .persist()
        .crypto()
        .decrypt(row.ocr_enc.as_ref().unwrap())
        .unwrap();
    assert_eq!(plain, b"text 41x31");
}

#[test]
fn an_empty_result_is_done_not_failed() {
    let fx = Fx::new();
    *fx.ctl.reply.lock() = Reply::Empty;
    let id = fx.image(1);
    fx.wait_idle();
    assert_eq!(fx.status(id), OcrStatus::Done);
    assert_eq!(fx.text(id), "");

    let fx = fx.reopen(false);
    assert_eq!(fx.status(id), OcrStatus::Done);
    let row = fx
        .store
        .persist()
        .load_all_meta(0)
        .unwrap()
        .into_iter()
        .find(|m| m.id == id as i64)
        .unwrap();
    assert!(row.ocr_enc.is_some(), "an empty text is stored, not null");
}

#[test]
fn an_engine_error_marks_the_item_failed_and_the_worker_goes_on() {
    let fx = Fx::new();
    *fx.ctl.reply.lock() = Reply::Error;
    let bad = fx.image(1);
    fx.wait_idle();
    assert_eq!(fx.status(bad), OcrStatus::Failed);
    assert_eq!(fx.text(bad), "");

    *fx.ctl.reply.lock() = Reply::Text;
    let good = fx.image(2);
    fx.wait_idle();
    assert_eq!(fx.status(good), OcrStatus::Done);

    // Failed is final: it survives a restart and is not retried.
    let before = fx.ctl.calls().len();
    let fx = fx.reopen(true);
    fx.wait_idle();
    assert_eq!(fx.status(bad), OcrStatus::Failed);
    assert_eq!(fx.ctl.calls().len(), before, "no re-run of a failed item");
}

#[test]
fn an_engine_panic_marks_the_item_failed_and_the_worker_survives() {
    let fx = Fx::new();
    *fx.ctl.reply.lock() = Reply::Panic;
    let bad = fx.image(1);
    fx.wait_idle();
    assert_eq!(fx.status(bad), OcrStatus::Failed);
    *fx.ctl.reply.lock() = Reply::Text;
    let good = fx.image(2);
    fx.wait_idle();
    assert_eq!(fx.status(good), OcrStatus::Done);
}

#[test]
fn an_unavailable_engine_marks_items_unavailable_and_is_never_called() {
    let mut fx = Fx::bare();
    fx.ctl.available.store(false, Ordering::SeqCst);
    fx.start_worker(OcrConfig::default());
    wait_for("the engine state", || {
        fx.queue.engine_state() != EngineState::Unknown
    });
    assert_eq!(fx.queue.engine_state(), EngineState::Unavailable);
    let a = fx.image(1);
    let b = fx.image(2);
    fx.wait_idle();
    assert_eq!(fx.status(a), OcrStatus::Unavailable);
    assert_eq!(fx.status(b), OcrStatus::Unavailable);
    assert!(fx.ctl.calls().is_empty());
    let stats = fx.store.stats();
    assert_eq!(stats.ocr_unavailable, 2);
    assert_eq!(stats.ocr_pending, 0);

    // The status survives a restart ...
    let fx = fx.reopen(true);
    fx.wait_idle();
    assert_eq!(fx.status(a), OcrStatus::Unavailable);
    assert_eq!(fx.store.stats().ocr_unavailable, 2);
    assert!(fx.ctl.calls().is_empty());

    // ... and once a language exists, the next start recognizes them.
    fx.ctl.available.store(true, Ordering::SeqCst);
    let fx = fx.reopen(true);
    fx.wait_idle();
    assert_eq!(fx.status(a), OcrStatus::Done);
    assert_eq!(fx.status(b), OcrStatus::Done);
    assert_eq!(fx.queue.engine_state(), EngineState::Available);
    assert_eq!(fx.store.stats().ocr_unavailable, 0);
}

// ---------------------------------------------------------------------------
// Skipping and dedup
// ---------------------------------------------------------------------------

#[test]
fn a_deleted_item_is_skipped() {
    let fx = Fx::new();
    fx.ctl.hold.store(true, Ordering::SeqCst);
    let a = fx.image(1);
    fx.wait_calls(1); // the worker is inside the engine, busy with `a`
    let b = fx.image(2);
    let c = fx.image(3);
    assert!(fx.store.remove(b));
    fx.ctl.hold.store(false, Ordering::SeqCst);
    fx.wait_idle();
    assert_eq!(
        fx.ctl.calls(),
        [(41, 31), (43, 33)],
        "b was never recognized"
    );
    assert_eq!(fx.status(a), OcrStatus::Done);
    assert_eq!(fx.status(c), OcrStatus::Done);
    assert!(fx.store.get(b).is_none());
}

#[test]
fn an_item_deleted_while_it_is_recognized_does_not_come_back() {
    let fx = Fx::new();
    fx.ctl.hold.store(true, Ordering::SeqCst);
    let a = fx.image(1);
    fx.wait_calls(1);
    assert!(fx.store.remove(a));
    fx.ctl.hold.store(false, Ordering::SeqCst);
    fx.wait_idle();
    assert!(fx.store.get(a).is_none());
    assert_eq!(fx.store.len(), 0);
}

#[test]
fn duplicate_jobs_are_collapsed() {
    let fx = Fx::new();
    fx.ctl.hold.store(true, Ordering::SeqCst);
    let a = fx.image(1);
    fx.wait_calls(1);
    let b = fx.image(2);
    let job = |id: u64, n: u32| OcrJob {
        item_id: id as i64,
        hash: 0,
        width: 40 + n,
        height: 30 + n,
    };
    // `a` is running, `b` is queued: repeats of either change nothing.
    for _ in 0..3 {
        fx.queue.enqueue(job(a, 1));
        fx.queue.enqueue(job(b, 2));
    }
    assert_eq!(fx.queue.backlog(), 2, "one running, one waiting");
    fx.ctl.hold.store(false, Ordering::SeqCst);
    fx.wait_idle();
    assert_eq!(fx.ctl.calls(), [(41, 31), (42, 32)], "each image once");
    assert_eq!(fx.status(a), OcrStatus::Done);
    assert_eq!(fx.status(b), OcrStatus::Done);
}

#[test]
fn a_done_item_is_not_recognized_again() {
    let fx = Fx::new();
    let a = fx.image(1);
    fx.wait_idle();
    fx.queue.enqueue(OcrJob {
        item_id: a as i64,
        hash: 0,
        width: 41,
        height: 31,
    });
    fx.wait_idle();
    assert_eq!(fx.ctl.calls().len(), 1);
}

// ---------------------------------------------------------------------------
// Waiting for the blob
// ---------------------------------------------------------------------------

/// The path of an image's blob file, and the encrypted bytes in it.
fn blob_file(fx: &Fx, id: u64) -> std::path::PathBuf {
    let name = fx.store.get(id).unwrap().blob_hash.clone().unwrap();
    fx.dir
        .path()
        .join("blobs")
        .join(&name[..2])
        .join(format!("{name}.bin"))
}

#[test]
fn a_blob_that_appears_late_is_waited_for() {
    // No worker yet: insert, let the writer commit, then take the file away.
    let mut fx = Fx::bare();
    let id = fx.image(1);
    fx.store.persist().flush().unwrap();
    let path = blob_file(&fx, id);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();

    let started = Instant::now();
    fx.start_worker(OcrConfig::default());
    let restore = {
        let path = path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(ms(250));
            std::fs::write(path, bytes).unwrap();
        })
    };
    fx.wait_idle();
    restore.join().unwrap();
    assert_eq!(fx.status(id), OcrStatus::Done);
    assert!(
        started.elapsed() >= ms(240),
        "it really waited for the file ({:?})",
        started.elapsed()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_blob_that_never_appears_fails_after_the_retry_budget() {
    let mut fx = Fx::bare();
    let id = fx.image(1);
    fx.store.persist().flush().unwrap();
    std::fs::remove_file(blob_file(&fx, id)).unwrap();

    let started = Instant::now();
    fx.start_worker(short_retry());
    fx.wait_idle();
    let took = started.elapsed();
    assert_eq!(fx.status(id), OcrStatus::Failed);
    assert!(fx.ctl.calls().is_empty(), "the engine never saw a picture");
    assert!(
        took >= ms(290),
        "waited about the 300 ms budget, took {took:?}"
    );
    assert!(
        took < Duration::from_secs(5),
        "and then gave up, took {took:?}"
    );
}

#[test]
fn a_job_right_after_the_insert_finds_the_blob() {
    // The insert queues the PNG for the writer and enqueues the job at once;
    // the worker must cope with the writer being a few ms behind. Repeat to
    // make the race likely.
    let fx = Fx::new();
    let ids: Vec<u64> = (1..=12).map(|n| fx.image(n)).collect();
    fx.wait_idle();
    for id in ids {
        assert_eq!(fx.status(id), OcrStatus::Done, "item {id}");
    }
}

// ---------------------------------------------------------------------------
// Startup re-queue
// ---------------------------------------------------------------------------

#[test]
fn startup_requeues_pending_images_newest_first() {
    // First run without a worker (the app was closed before OCR ran).
    let fx = Fx::bare();
    let old = fx.image(1);
    let mid = fx.image(2);
    let new = fx.image(3);
    let done = fx.image(4);
    let failed = fx.image(5);
    let text = fx
        .store
        .insert(crate::ingest::text::build("hello", None, 1).unwrap());
    fx.store
        .set_ocr_text(done, Some("kept".into()), OcrStatus::Done);
    fx.store.set_ocr_text(failed, None, OcrStatus::Failed);
    assert!(matches!(text, Inserted::New(_)));
    for id in [old, mid, new] {
        assert_eq!(fx.status(id), OcrStatus::Pending);
    }

    let fx = fx.reopen(true);
    fx.wait_idle();
    for id in [old, mid, new] {
        assert_eq!(fx.status(id), OcrStatus::Done, "item {id}");
    }
    assert_eq!(fx.text(done), "kept", "a finished item is left alone");
    assert_eq!(fx.status(failed), OcrStatus::Failed);
    // Newest first: items 3, 2, 1 (4 and 5 are settled).
    assert_eq!(fx.ctl.calls(), [(43, 33), (42, 32), (41, 31)]);
}

// ---------------------------------------------------------------------------
// The setting
// ---------------------------------------------------------------------------

#[test]
fn with_ocr_off_new_images_stay_pending_and_turning_it_on_queues_them() {
    let fx = Fx::new();
    fx.settings.write().unwrap().ocr_enabled = false;
    fx.queue.apply_settings();
    let a = fx.image(1);
    let b = fx.image(2);
    fx.wait_idle();
    assert!(fx.ctl.calls().is_empty());
    assert_eq!(fx.status(a), OcrStatus::Pending);
    assert_eq!(fx.status(b), OcrStatus::Pending);
    assert_eq!(fx.store.stats().ocr_pending, 2);

    fx.settings.write().unwrap().ocr_enabled = true;
    fx.worker.as_ref().unwrap().apply_settings();
    fx.wait_idle();
    assert_eq!(fx.status(a), OcrStatus::Done);
    assert_eq!(fx.status(b), OcrStatus::Done);
    assert_eq!(fx.ctl.calls().len(), 2);

    // Applying the same value again queues nothing new.
    fx.worker.as_ref().unwrap().apply_settings();
    fx.wait_idle();
    assert_eq!(fx.ctl.calls().len(), 2);
}

#[test]
fn turning_ocr_off_drops_the_waiting_jobs() {
    let fx = Fx::new();
    fx.ctl.hold.store(true, Ordering::SeqCst);
    let a = fx.image(1);
    fx.wait_calls(1);
    let b = fx.image(2);
    let c = fx.image(3);
    fx.settings.write().unwrap().ocr_enabled = false;
    fx.queue.apply_settings();
    fx.ctl.hold.store(false, Ordering::SeqCst);
    fx.wait_idle();
    assert_eq!(fx.status(a), OcrStatus::Done, "the running job finishes");
    assert_eq!(fx.status(b), OcrStatus::Pending);
    assert_eq!(fx.status(c), OcrStatus::Pending);
    assert_eq!(fx.ctl.calls().len(), 1);

    fx.settings.write().unwrap().ocr_enabled = true;
    fx.queue.apply_settings();
    fx.wait_idle();
    assert_eq!(fx.status(b), OcrStatus::Done);
    assert_eq!(fx.status(c), OcrStatus::Done);
}

#[test]
fn starting_with_ocr_off_queues_nothing_until_it_is_turned_on() {
    let fx = Fx::bare();
    let a = fx.image(1);
    fx.settings.write().unwrap().ocr_enabled = false;
    let mut fx = fx.reopen(false);
    fx.queue.apply_settings();
    fx.start_worker(OcrConfig::default());
    fx.wait_idle();
    assert_eq!(fx.status(a), OcrStatus::Pending);
    assert!(fx.ctl.calls().is_empty());
    fx.settings.write().unwrap().ocr_enabled = true;
    fx.queue.apply_settings();
    fx.wait_idle();
    assert_eq!(fx.status(a), OcrStatus::Done);
}

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------

#[test]
fn dropping_the_worker_with_a_full_queue_is_clean_and_loses_nothing() {
    let mut fx = Fx::new();
    fx.ctl.hold.store(true, Ordering::SeqCst);
    let ids: Vec<u64> = (1..=5).map(|n| fx.image(n)).collect();
    fx.wait_calls(1);
    assert_eq!(fx.queue.backlog(), 5);

    // The job in flight is released a little later; drop waits for it only.
    let ctl = Arc::clone(&fx.ctl);
    let release = std::thread::spawn(move || {
        std::thread::sleep(ms(100));
        ctl.hold.store(false, Ordering::SeqCst);
    });
    let started = Instant::now();
    drop(fx.worker.take());
    release.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(fx.ctl.calls().len(), 1, "the queued jobs were dropped");
    assert_eq!(fx.queue.backlog(), 0);

    // Late enqueues are ignored, and nothing was lost: a restart finishes them.
    fx.queue.enqueue(OcrJob {
        item_id: ids[1] as i64,
        hash: 0,
        width: 42,
        height: 32,
    });
    assert_eq!(fx.queue.backlog(), 0);
    let pending = ids
        .iter()
        .filter(|&&id| fx.status(id) == OcrStatus::Pending)
        .count();
    assert_eq!(pending, 4);
    let fx = fx.reopen(true);
    fx.wait_idle();
    for id in ids {
        assert_eq!(fx.status(id), OcrStatus::Done, "item {id}");
    }
}

#[test]
fn dropping_an_idle_worker_and_one_that_never_started_work_returns_at_once() {
    let started = Instant::now();
    let fx = Fx::new();
    drop(fx);
    let fx = Fx::bare();
    drop(fx);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn the_worker_ends_when_the_store_is_gone() {
    let fx = Fx::new();
    let Fx {
        store,
        worker,
        queue,
        ..
    } = fx;
    drop(store);
    // A job for a store that no longer exists ends the thread; drop must not hang.
    queue.enqueue(OcrJob {
        item_id: 1,
        hash: 0,
        width: 1,
        height: 1,
    });
    drop(worker);
}
