//! Background, coalescing writer of `settings.json`.
//!
//! `request` only stores the newest snapshot and wakes the writer thread; it
//! never touches the disk, so `set_settings` stays instant. The thread waits
//! until no request has arrived for `debounce` (a trailing debounce, so a
//! slider or a burst of switch clicks becomes one write), but never longer than
//! `max_wait` after the first unwritten request, so a steady stream of changes
//! still reaches the disk. `flush` writes now and waits; dropping the saver
//! flushes and joins the thread, so a normal quit loses nothing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{Settings, file};

/// Quiet time after the last change before the file is written.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// Longest a change waits while changes keep coming.
pub const MAX_WAIT: Duration = Duration::from_secs(2);

#[derive(Default)]
struct State {
    pending: Option<Settings>,
    /// When the oldest unwritten request arrived, and the newest one.
    first_at: Option<Instant>,
    last_at: Option<Instant>,
    /// Counts requests and finished writes, so `flush` knows when its own
    /// request has been written, also when a write was already running.
    requested: u64,
    written: u64,
    flush: bool,
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    dir: PathBuf,
    debounce: Duration,
    max_wait: Duration,
    writes: AtomicUsize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct SettingsSaver {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl SettingsSaver {
    /// Writer for `dir/settings.json` with the standard timings.
    pub fn start(dir: PathBuf) -> std::io::Result<Self> {
        Self::start_with(dir, DEBOUNCE, MAX_WAIT)
    }

    pub fn start_with(
        dir: PathBuf,
        debounce: Duration,
        max_wait: Duration,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
            dir,
            debounce,
            max_wait,
            writes: AtomicUsize::new(0),
        });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("settings-saver".into())
            .spawn(move || run(&worker))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Ask for `settings` to be written soon. Replaces any request that has not
    /// been written yet. Never blocks on the disk.
    pub fn request(&self, settings: Settings) {
        let now = Instant::now();
        let mut st = self.shared.lock();
        st.pending = Some(settings);
        st.first_at.get_or_insert(now);
        st.last_at = Some(now);
        st.requested += 1;
        drop(st);
        self.shared.cv.notify_all();
    }

    /// Write whatever is pending now and wait until it is on disk.
    pub fn flush(&self) {
        let mut st = self.shared.lock();
        if st.written == st.requested {
            return;
        }
        st.flush = true;
        self.shared.cv.notify_all();
        let mut st = self
            .shared
            .cv
            .wait_while(st, |s| s.written < s.requested && !s.closed)
            .unwrap_or_else(|e| e.into_inner());
        st.flush = false;
    }

    /// How many times the file was written (tests use it to check coalescing).
    #[cfg(test)]
    pub fn write_count(&self) -> usize {
        self.shared.writes.load(Ordering::SeqCst)
    }
}

impl Drop for SettingsSaver {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(shared: &Shared) {
    let mut st = shared.lock();
    loop {
        let Some(_) = st.pending else {
            if st.closed {
                return;
            }
            st = shared.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            continue;
        };
        let now = Instant::now();
        let due = if st.closed || st.flush {
            now
        } else {
            let quiet = st.last_at.unwrap_or(now) + shared.debounce;
            let cap = st.first_at.unwrap_or(now) + shared.max_wait;
            quiet.min(cap)
        };
        if due > now {
            st = shared
                .cv
                .wait_timeout(st, due - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            continue;
        }
        let Some(snapshot) = st.pending.take() else {
            continue;
        };
        let upto = st.requested;
        st.first_at = None;
        st.last_at = None;
        drop(st);
        if let Err(e) = file::write_atomic(&shared.dir, &snapshot) {
            // Not fatal: the next change tries again, and the settings in memory
            // are what the running app uses.
            eprintln!("clipdump: could not save settings: {e}");
        }
        shared.writes.fetch_add(1, Ordering::SeqCst);
        st = shared.lock();
        st.written = upto;
        shared.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOW: Duration = Duration::from_millis(400);

    fn with_items(n: usize) -> Settings {
        Settings {
            max_items: n,
            ..Settings::default()
        }
    }

    fn on_disk(dir: &std::path::Path) -> Settings {
        file::load(dir).settings
    }

    #[test]
    fn a_request_reaches_the_disk_after_the_debounce_without_a_flush() {
        let t = tempfile::tempdir().unwrap();
        let s = SettingsSaver::start_with(t.path().into(), Duration::from_millis(30), MAX_WAIT)
            .unwrap();
        s.request(with_items(5));
        let deadline = Instant::now() + Duration::from_secs(5);
        while s.write_count() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(on_disk(t.path()).max_items, 5);
    }

    #[test]
    fn a_burst_of_changes_is_one_write_with_the_last_value() {
        let t = tempfile::tempdir().unwrap();
        let s = SettingsSaver::start_with(t.path().into(), SLOW, Duration::from_secs(30)).unwrap();
        for n in 1..=200 {
            s.request(with_items(n));
        }
        assert_eq!(s.write_count(), 0, "nothing is written while changes come");
        s.flush();
        assert_eq!(s.write_count(), 1, "one write for 200 requests");
        assert_eq!(on_disk(t.path()).max_items, 200);
    }

    #[test]
    fn request_returns_at_once_even_when_the_write_is_slow_to_come() {
        let t = tempfile::tempdir().unwrap();
        let s = SettingsSaver::start_with(
            t.path().into(),
            Duration::from_secs(30),
            Duration::from_secs(60),
        )
        .unwrap();
        let t0 = Instant::now();
        for n in 0..1000 {
            s.request(with_items(n));
        }
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "{:?}",
            t0.elapsed()
        );
        assert_eq!(s.write_count(), 0);
    }

    #[test]
    fn changes_that_never_stop_still_get_written_at_the_cap() {
        let t = tempfile::tempdir().unwrap();
        let s = SettingsSaver::start_with(
            t.path().into(),
            Duration::from_millis(200),
            Duration::from_millis(150),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut n = 0;
        while s.write_count() == 0 && Instant::now() < deadline {
            n += 1;
            s.request(with_items(n));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            s.write_count() >= 1,
            "the cap forced a write during the stream"
        );
    }

    #[test]
    fn two_separate_changes_make_two_writes() {
        let t = tempfile::tempdir().unwrap();
        let s = SettingsSaver::start_with(t.path().into(), SLOW, MAX_WAIT).unwrap();
        s.request(with_items(1));
        s.flush();
        s.request(with_items(2));
        s.flush();
        assert_eq!(s.write_count(), 2);
        assert_eq!(on_disk(t.path()).max_items, 2);
    }

    #[test]
    fn flush_with_nothing_pending_returns_at_once() {
        let t = tempfile::tempdir().unwrap();
        let s = SettingsSaver::start_with(t.path().into(), SLOW, MAX_WAIT).unwrap();
        let t0 = Instant::now();
        s.flush();
        assert!(t0.elapsed() < Duration::from_millis(100));
        assert_eq!(s.write_count(), 0);
        assert!(
            !t.path().join(file::FILE_NAME).exists(),
            "no file without a change"
        );
    }

    #[test]
    fn dropping_the_saver_writes_what_is_pending() {
        let t = tempfile::tempdir().unwrap();
        {
            let s = SettingsSaver::start_with(
                t.path().into(),
                Duration::from_secs(60),
                Duration::from_secs(60),
            )
            .unwrap();
            s.request(with_items(77));
        }
        assert_eq!(on_disk(t.path()).max_items, 77, "a quit loses nothing");
    }

    #[test]
    fn a_failing_write_does_not_hang_or_panic() {
        let t = tempfile::tempdir().unwrap();
        // The "folder" is a file, so every write fails.
        let blocker = t.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let s = SettingsSaver::start_with(blocker.join("sub"), Duration::from_millis(10), MAX_WAIT)
            .unwrap();
        s.request(with_items(1));
        s.flush();
        s.request(with_items(2));
        s.flush();
        assert_eq!(s.write_count(), 2, "it tried each time and kept running");
    }

    #[test]
    fn requests_from_many_threads_end_with_a_valid_file() {
        let t = tempfile::tempdir().unwrap();
        let s = Arc::new(
            SettingsSaver::start_with(t.path().into(), Duration::from_millis(5), MAX_WAIT).unwrap(),
        );
        let handles: Vec<_> = (0..4)
            .map(|k| {
                let s = Arc::clone(&s);
                std::thread::spawn(move || {
                    for n in 0..100 {
                        s.request(with_items(k * 1000 + n + 1));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        s.flush();
        let loaded = file::load(t.path());
        assert_eq!(loaded.source, file::Source::Loaded);
        assert!(loaded.issues.is_empty());
        assert!(loaded.settings.max_items >= 1);
    }
}
