//! Graceful shutdown (Phase 13): one `run`, ordered, idempotent and time bounded.
//!
//! What it must achieve: nothing the user copied is lost when the app quits from
//! the tray, when Windows logs off, shuts down or restarts, or when an installer
//! asks the app to close. The persist writer already commits within 50 ms of an
//! item arriving, so the realistic loss without this is the last batch (and the
//! WAL checkpoint, and a settings change still in its debounce); with it, none.
//!
//! # Order
//!
//! 1. **Capture watcher stops**: no new clipboard update can start an item.
//! 2. **Ingest drains**: the workers finish the items already queued (the
//!    watcher's senders are gone, so they end by themselves) and hand them to the
//!    store, which queues the writes.
//! 3. **OCR and retention stop**: no background job can write after the flush.
//! 4. **Store and persist flush**: everything queued is committed, the WAL is
//!    checkpointed, the writer thread ends (`Persist::shutdown`).
//! 5. **Settings flush**: a change still waiting for its debounce is written.
//!
//! The order is the one in `Stage::ORDER` and is tested with a fake [`Stages`].
//!
//! # Bounds
//!
//! Windows gives an app about five seconds at logoff or shutdown. Every stage runs
//! on its own short lived thread and gets a share of the total budget; a stage that
//! does not finish in time is reported as late and the next one starts anyway (the
//! stuck thread is abandoned, the process is about to exit). So the settings flush
//! is never skipped because the database was slow, and `run` always returns within
//! about the budget. Idle cost is zero: this code only runs once, at exit.
//!
//! # Where it is called
//!
//! * `RunEvent::Exit` (`lib.rs`): tray Quit, `app.exit(0)` and the last window
//!   closing all end in it.
//! * Windows logoff, shutdown, restart and the Restart Manager: **tao's hidden
//!   top level event window already handles `WM_ENDSESSION`** (`wParam` true): it
//!   fires the loop destroyed event, which Tauri turns into `RunEvent::Exit`, runs
//!   our callback synchronously inside the window procedure, and then calls
//!   `std::process::exit(0)`. So the same single `run` covers session end, with no
//!   second window of our own. (The clipboard watcher's window is message only and
//!   never receives these broadcasts; tao's window is a plain top level popup with
//!   `WS_EX_TOOLWINDOW`, which does.) `WM_QUERYENDSESSION` is left to
//!   `DefWindowProc`, which answers "yes": we never veto, and a cancelled session
//!   end (`wParam` false) needs no undo because nothing was stopped before the
//!   real `WM_ENDSESSION`. `ExitRequested` only does the early, harmless flush.
//!   `platform::prefer_late_shutdown` lowers the process shutdown level so Windows
//!   terminates this process after the ordinary apps.
//! * An abrupt kill (task manager, power cut) skips all of it; then the 50 ms
//!   batching of the writer and SQLite's WAL are what protect the data.

use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Total time `run` may take at exit. Windows allows about five seconds; this
/// stays well inside, and the normal case takes a few milliseconds.
pub const EXIT_BUDGET: Duration = Duration::from_millis(3500);

/// The five steps, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    StopCapture,
    DrainIngest,
    StopWorkers,
    FlushStore,
    FlushSettings,
}

impl Stage {
    pub const ORDER: [Stage; 5] = [
        Stage::StopCapture,
        Stage::DrainIngest,
        Stage::StopWorkers,
        Stage::FlushStore,
        Stage::FlushSettings,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Stage::StopCapture => "stop capture",
            Stage::DrainIngest => "drain ingest",
            Stage::StopWorkers => "stop OCR and retention",
            Stage::FlushStore => "flush history",
            Stage::FlushSettings => "flush settings",
        }
    }

    /// Share of the total budget, in tenths. The database gets the most (it may
    /// have a batch and a checkpoint to write), the others are normally instant.
    fn tenths(self) -> u32 {
        match self {
            Stage::StopCapture => 1,
            Stage::DrainIngest => 2,
            Stage::StopWorkers => 2,
            Stage::FlushStore => 4,
            Stage::FlushSettings => 1,
        }
    }

    fn run(self, s: &dyn Stages) {
        match self {
            Stage::StopCapture => s.stop_capture(),
            Stage::DrainIngest => s.drain_ingest(),
            Stage::StopWorkers => s.stop_workers(),
            Stage::FlushStore => s.flush_store(),
            Stage::FlushSettings => s.flush_settings(),
        }
    }
}

/// What the five steps do. The app implements it with the real watcher, pool,
/// workers, database and settings saver; tests with a recorder.
pub trait Stages: Send + Sync + 'static {
    /// Stop the clipboard watcher (drops it, ending its thread and its sender).
    fn stop_capture(&self);
    /// Wait until the ingest workers have finished what is queued.
    fn drain_ingest(&self);
    /// Stop the OCR worker and the retention timer.
    fn stop_workers(&self);
    /// Commit the history, checkpoint it and stop the writer.
    fn flush_store(&self);
    /// Write `settings.json` if a change is pending.
    fn flush_settings(&self);
    /// The harmless early flush (history and settings, nothing stopped).
    fn flush_pending(&self);
}

/// How a call to [`Shutdown::run`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every stage finished in its budget.
    Completed,
    /// These stages did not finish in time (the others did).
    Late(Vec<Stage>),
    /// An earlier call had already finished the work.
    AlreadyDone,
    /// Another call is still running and did not finish in this call's budget.
    StillRunning,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Running,
    Done,
}

/// The once only gate around the stages. Lives in `AppState`.
pub struct Shutdown {
    phase: Mutex<Phase>,
    changed: Condvar,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self {
            phase: Mutex::new(Phase::Idle),
            changed: Condvar::new(),
        }
    }
}

impl Shutdown {
    /// Has a shutdown started (or finished)?
    pub fn started(&self) -> bool {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) != Phase::Idle
    }

    /// Run the five stages in order within `total`. Idempotent: the first call
    /// does the work, a later or concurrent one waits (at most `total`) for it and
    /// does nothing itself.
    pub fn run(&self, stages: &Arc<dyn Stages>, total: Duration) -> Outcome {
        {
            let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
            match *phase {
                Phase::Done => return Outcome::AlreadyDone,
                Phase::Running => {
                    let (phase, _) = self
                        .changed
                        .wait_timeout_while(phase, total, |p| *p == Phase::Running)
                        .unwrap_or_else(|e| e.into_inner());
                    return if *phase == Phase::Done {
                        Outcome::AlreadyDone
                    } else {
                        Outcome::StillRunning
                    };
                }
                Phase::Idle => *phase = Phase::Running,
            }
        }
        let start = Instant::now();
        let mut late = Vec::new();
        for stage in Stage::ORDER {
            // Its share, but never more than what is left; a small floor so a
            // stage after an overrun still gets a moment (settings are tiny).
            let share = total / 10 * stage.tenths();
            let left = total.saturating_sub(start.elapsed());
            let wait = share.min(left).max(Duration::from_millis(20));
            if !run_stage(stage, stages, wait) {
                eprintln!(
                    "clipdump: shutdown: \"{}\" did not finish in {} ms",
                    stage.name(),
                    wait.as_millis()
                );
                late.push(stage);
            }
        }
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Done;
        self.changed.notify_all();
        if late.is_empty() {
            Outcome::Completed
        } else {
            Outcome::Late(late)
        }
    }

    /// The early, harmless flush (history and settings, nothing is stopped), for
    /// `ExitRequested`. Skipped once the real shutdown has started. Returns
    /// whether it finished in `wait`.
    pub fn flush_early(&self, stages: &Arc<dyn Stages>, wait: Duration) -> bool {
        if self.started() {
            return true;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let st = Arc::clone(stages);
        let spawned = thread::Builder::new()
            .name("clipdump-flush".into())
            .spawn(move || {
                st.flush_pending();
                let _ = tx.send(());
            });
        spawned.is_ok() && rx.recv_timeout(wait).is_ok()
    }
}

/// Run one stage on its own thread and wait up to `wait`. True when it finished
/// (a panic counts: it is over), false when it is still running.
fn run_stage(stage: Stage, stages: &Arc<dyn Stages>, wait: Duration) -> bool {
    let (tx, rx) = crossbeam_channel::bounded::<()>(1);
    let st = Arc::clone(stages);
    let spawned = thread::Builder::new()
        .name("clipdump-shutdown".into())
        .spawn(move || {
            stage.run(&*st);
            let _ = tx.send(());
        });
    match spawned {
        Ok(_) => !matches!(
            rx.recv_timeout(wait),
            Err(crossbeam_channel::RecvTimeoutError::Timeout)
        ),
        // No thread to spare at exit: do it here, unbounded, rather than skip it.
        Err(_) => {
            stage.run(&**stages);
            true
        }
    }
}

// ---------------------------------------------------------------------------
// The real stages
// ---------------------------------------------------------------------------

/// A value the shutdown must be able to take out of managed state and drop
/// (Tauri's `manage` cannot remove what it holds).
pub struct Slot<T>(Mutex<Option<T>>);

impl<T> Slot<T> {
    pub fn new(value: T) -> Self {
        Self(Mutex::new(Some(value)))
    }

    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// The app's real stages, over the managed state.
pub struct AppStages(pub tauri::AppHandle);

impl Stages for AppStages {
    fn stop_capture(&self) {
        use tauri::Manager;
        if let Some(slot) = self.0.try_state::<Slot<crate::platform::Watcher>>() {
            // Dropping the watcher ends its thread and with it the last sender of
            // the ingest queue.
            drop(slot.take());
        }
    }

    fn drain_ingest(&self) {
        use tauri::Manager;
        if let Some(slot) = self.0.try_state::<Slot<crate::ingest::IngestPool>>()
            && let Some(pool) = slot.take()
        {
            pool.join();
        }
    }

    fn stop_workers(&self) {
        use tauri::Manager;
        if let Some(ocr) = self.0.try_state::<crate::ocr::OcrWorker>() {
            ocr.stop();
        }
        if let Some(timer) = self
            .0
            .try_state::<crate::store::retention::RetentionTimer>()
        {
            timer.stop();
        }
    }

    fn flush_store(&self) {
        use tauri::Manager;
        if let Some(persist) = self.0.try_state::<Arc<crate::persist::Persist>>()
            && let Err(e) = persist.shutdown()
        {
            eprintln!("clipdump: shutdown: {e:#}");
        }
    }

    fn flush_settings(&self) {
        use tauri::Manager;
        if let Some(saver) = self.0.state::<crate::state::AppState>().saver.get() {
            saver.flush();
        }
    }

    fn flush_pending(&self) {
        use tauri::Manager;
        if let Some(persist) = self.0.try_state::<Arc<crate::persist::Persist>>()
            && let Err(e) = persist.flush()
        {
            eprintln!("clipdump: flush: {e:#}");
        }
        self.flush_settings();
    }
}

/// `RunEvent::Exit`: the full shutdown, with the exit budget.
pub fn run(app: &tauri::AppHandle) {
    use tauri::Manager;
    let stages: Arc<dyn Stages> = Arc::new(AppStages(app.clone()));
    let started = Instant::now();
    let outcome = app
        .state::<crate::state::AppState>()
        .shutdown
        .run(&stages, EXIT_BUDGET);
    eprintln!(
        "clipdump: shutdown {outcome:?} in {} ms",
        started.elapsed().as_millis()
    );
}

/// `RunEvent::ExitRequested`: flush what is queued, stop nothing (the exit can
/// still be prevented by someone; `Exit` does the real work).
pub fn flush_early(app: &tauri::AppHandle) {
    use tauri::Manager;
    let stages: Arc<dyn Stages> = Arc::new(AppStages(app.clone()));
    app.state::<crate::state::AppState>()
        .shutdown
        .flush_early(&stages, Duration::from_millis(1000));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Records the order, can block or panic one stage.
    #[derive(Default)]
    struct Fake {
        log: Mutex<Vec<&'static str>>,
        /// This stage waits for `release` (or ten seconds).
        block: Option<Stage>,
        panic_in: Option<Stage>,
        release: (Mutex<bool>, Condvar),
        runs: AtomicUsize,
    }

    impl Fake {
        fn note(&self, stage: Stage, what: &'static str) {
            self.log.lock().unwrap().push(what);
            self.runs.fetch_add(1, Ordering::SeqCst);
            if self.panic_in == Some(stage) {
                panic!("boom in {what}");
            }
            if self.block == Some(stage) {
                let (m, cv) = &self.release;
                let g = m.lock().unwrap();
                let _ = cv.wait_timeout_while(g, Duration::from_secs(10), |r| !*r);
            }
        }

        fn release(&self) {
            *self.release.0.lock().unwrap() = true;
            self.release.1.notify_all();
        }

        fn order(&self) -> Vec<&'static str> {
            self.log.lock().unwrap().clone()
        }
    }

    impl Stages for Fake {
        fn stop_capture(&self) {
            self.note(Stage::StopCapture, "capture");
        }
        fn drain_ingest(&self) {
            self.note(Stage::DrainIngest, "ingest");
        }
        fn stop_workers(&self) {
            self.note(Stage::StopWorkers, "workers");
        }
        fn flush_store(&self) {
            self.note(Stage::FlushStore, "store");
        }
        fn flush_settings(&self) {
            self.note(Stage::FlushSettings, "settings");
        }
        fn flush_pending(&self) {
            self.log.lock().unwrap().push("pending");
        }
    }

    fn arc(f: Fake) -> (Arc<Fake>, Arc<dyn Stages>) {
        let f = Arc::new(f);
        let dynf: Arc<dyn Stages> = f.clone();
        (f, dynf)
    }

    const ALL: [&str; 5] = ["capture", "ingest", "workers", "store", "settings"];

    #[test]
    fn the_stages_run_in_the_documented_order() {
        let (fake, stages) = arc(Fake::default());
        let out = Shutdown::default().run(&stages, Duration::from_secs(2));
        assert_eq!(out, Outcome::Completed);
        assert_eq!(fake.order(), ALL);
        assert_eq!(
            Stage::ORDER.map(Stage::name),
            [
                "stop capture",
                "drain ingest",
                "stop OCR and retention",
                "flush history",
                "flush settings"
            ]
        );
    }

    #[test]
    fn it_is_idempotent() {
        let (fake, stages) = arc(Fake::default());
        let s = Shutdown::default();
        assert_eq!(s.run(&stages, Duration::from_secs(2)), Outcome::Completed);
        assert_eq!(s.run(&stages, Duration::from_secs(2)), Outcome::AlreadyDone);
        assert_eq!(s.run(&stages, Duration::from_secs(2)), Outcome::AlreadyDone);
        assert_eq!(fake.runs.load(Ordering::SeqCst), 5, "each stage ran once");
        assert!(s.started());
    }

    #[test]
    fn concurrent_callers_run_the_stages_once() {
        let (fake, stages) = arc(Fake {
            block: Some(Stage::DrainIngest),
            ..Fake::default()
        });
        let s = Arc::new(Shutdown::default());
        let first = {
            let (s, st) = (Arc::clone(&s), Arc::clone(&stages));
            thread::spawn(move || s.run(&st, Duration::from_secs(5)))
        };
        // Wait until the first call is inside the blocked stage.
        while fake.runs.load(Ordering::SeqCst) < 2 {
            thread::sleep(Duration::from_millis(2));
        }
        // A second caller with a short budget gives up without doing any work.
        assert_eq!(
            s.run(&stages, Duration::from_millis(30)),
            Outcome::StillRunning
        );
        fake.release();
        assert_eq!(first.join().unwrap(), Outcome::Completed);
        assert_eq!(s.run(&stages, Duration::from_secs(1)), Outcome::AlreadyDone);
        assert_eq!(fake.order(), ALL);
    }

    #[test]
    fn a_stuck_stage_is_abandoned_and_the_rest_still_run_within_the_budget() {
        let (fake, stages) = arc(Fake {
            block: Some(Stage::FlushStore),
            ..Fake::default()
        });
        let started = Instant::now();
        let out = Shutdown::default().run(&stages, Duration::from_millis(500));
        let took = started.elapsed();
        assert_eq!(out, Outcome::Late(vec![Stage::FlushStore]));
        // Settings are flushed even though the database stage hung.
        assert_eq!(fake.order(), ALL);
        assert!(took < Duration::from_millis(900), "took {took:?}");
        assert!(took >= Duration::from_millis(200), "waited for its share");
        fake.release();
    }

    #[test]
    fn a_panicking_stage_does_not_stop_the_others() {
        let (fake, stages) = arc(Fake {
            panic_in: Some(Stage::StopWorkers),
            ..Fake::default()
        });
        let out = Shutdown::default().run(&stages, Duration::from_secs(2));
        assert_eq!(out, Outcome::Completed, "a panic is over, not late");
        assert_eq!(fake.order(), ALL);
    }

    #[test]
    fn the_early_flush_touches_nothing_and_stops_once_shutdown_began() {
        let (fake, stages) = arc(Fake::default());
        let s = Shutdown::default();
        assert!(s.flush_early(&stages, Duration::from_secs(1)));
        assert_eq!(fake.order(), ["pending"], "no stage ran");
        s.run(&stages, Duration::from_secs(2));
        assert!(s.flush_early(&stages, Duration::from_secs(1)));
        assert_eq!(fake.order().iter().filter(|w| **w == "pending").count(), 1);
    }

    #[test]
    fn the_shares_add_up_to_the_budget() {
        let sum: u32 = Stage::ORDER.iter().map(|s| s.tenths()).sum();
        assert_eq!(sum, 10);
        assert!(EXIT_BUDGET < Duration::from_secs(5));
    }

    // -- the real parts behind the same stage order -------------------------------

    use crate::ingest::IngestPool;
    use crate::ocr::{OcrConfig, OcrEngine, OcrQueueHandle, OcrWorker};
    use crate::platform::{Captured, CapturedEvent, IngestMsg};
    use crate::store::Store;
    use crate::store::retention::RetentionTimer;
    use crate::store::tests::Fx;

    struct Never;
    impl OcrEngine for Never {
        fn recognize(&self, _: &[u8], _: u32, _: u32) -> anyhow::Result<String> {
            Ok(String::new())
        }
        fn available(&self) -> bool {
            true
        }
    }

    /// The pieces `AppStages` takes out of managed state, held directly.
    struct Real {
        tx: Slot<crossbeam_channel::Sender<IngestMsg>>,
        pool: Slot<IngestPool>,
        ocr: OcrWorker,
        timer: RetentionTimer,
        store: Arc<Store>,
        saved: AtomicUsize,
    }

    impl Stages for Real {
        fn stop_capture(&self) {
            drop(self.tx.take());
        }
        fn drain_ingest(&self) {
            if let Some(p) = self.pool.take() {
                p.join();
            }
        }
        fn stop_workers(&self) {
            self.ocr.stop();
            self.timer.stop();
        }
        fn flush_store(&self) {
            self.store.persist().shutdown().unwrap();
        }
        fn flush_settings(&self) {
            self.saved.fetch_add(1, Ordering::SeqCst);
        }
        fn flush_pending(&self) {
            self.store.persist().flush().unwrap();
        }
    }

    #[test]
    fn items_still_queued_at_quit_are_in_the_database_afterwards() {
        let fx = Fx::new();
        let (tx, rx) = crossbeam_channel::bounded(256);
        let pool = IngestPool::start(rx, fx.store.clone()).unwrap();
        let queue = OcrQueueHandle::new(Arc::clone(&fx.settings));
        let ocr = OcrWorker::start(
            queue,
            Arc::downgrade(&fx.store),
            Box::new(|| Box::new(Never)),
            OcrConfig::default(),
        )
        .unwrap();
        let timer = RetentionTimer::start(&fx.store).unwrap();
        // The watcher's last act before quit: a burst of fresh copies.
        for n in 0..120 {
            tx.send(IngestMsg::Captured(CapturedEvent {
                captured: Captured::Text(format!("copied just before quit {n}")),
                source_app: None,
            }))
            .unwrap();
        }
        let real = Arc::new(Real {
            tx: Slot::new(tx),
            pool: Slot::new(pool),
            ocr,
            timer,
            store: fx.store.clone(),
            saved: AtomicUsize::new(0),
        });
        let stages: Arc<dyn Stages> = real.clone();
        let sd = Shutdown::default();
        assert_eq!(sd.run(&stages, Duration::from_secs(5)), Outcome::Completed);
        assert_eq!(
            sd.run(&stages, Duration::from_secs(5)),
            Outcome::AlreadyDone
        );
        assert_eq!(
            real.saved.load(Ordering::SeqCst),
            1,
            "settings flushed once"
        );
        assert_eq!(fx.store.len(), 120, "the ingest queue was drained first");
        // The writer is stopped: nothing more can be queued, and everything
        // that was queued is in the file, with no explicit flush by the test.
        assert!(fx.store.persist().flush().is_err());
        let Fx { dir, store, .. } = fx;
        drop(real);
        drop(stages);
        drop(store);
        let reopened = crate::persist::tests::open(dir.path());
        assert_eq!(reopened.load_all_meta(0).unwrap().len(), 120);
    }
}
