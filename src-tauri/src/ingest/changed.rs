//! The debounced `history:changed` event.
//!
//! The UI re-runs its search whenever this fires, so a burst of changes (a paste
//! of ten files, an OCR result landing next to a new screenshot) must not turn
//! into ten searches. The rule is "at most one event per 50 ms" with both edges
//! covered:
//!
//! * the first change after a quiet period emits immediately (no added latency);
//! * changes inside the 50 ms window are merged into one trailing event at the
//!   end of the window, so the UI never misses the final state.
//!
//! [`Debouncer`] is the pure decision logic (time is passed in, so it is tested
//! without sleeping). [`HistoryChanged`] adds a timer thread and the callback;
//! [`HistoryChanged::for_app`] is the thin Tauri wrapper. Phase 6's store and
//! Phase 7's OCR both call [`HistoryChanged::notify`].

use std::io;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

/// Minimum time between two events.
pub const MIN_INTERVAL: Duration = Duration::from_millis(50);

/// What the caller must do after [`Debouncer::on_change`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Emit the event now.
    EmitNow,
    /// Too soon: emit once at this time, by calling [`Debouncer::on_timer`].
    WaitUntil(Instant),
    /// A trailing event is already scheduled; this change is merged into it.
    AlreadyScheduled,
}

/// Pure leading plus trailing debounce state. No clock, no threads.
#[derive(Debug)]
pub struct Debouncer {
    interval: Duration,
    last_emit: Option<Instant>,
    pending: bool,
}

impl Debouncer {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_emit: None,
            pending: false,
        }
    }

    /// Something changed at `now`.
    pub fn on_change(&mut self, now: Instant) -> Decision {
        if self.pending {
            return Decision::AlreadyScheduled;
        }
        match self.last_emit {
            Some(t) if now < t + self.interval => {
                self.pending = true;
                Decision::WaitUntil(t + self.interval)
            }
            _ => {
                self.last_emit = Some(now);
                Decision::EmitNow
            }
        }
    }

    /// The scheduled time arrived. Returns true if the trailing event must be
    /// emitted (false if nothing was pending).
    pub fn on_timer(&mut self, now: Instant) -> bool {
        if !self.pending {
            return false;
        }
        self.pending = false;
        self.last_emit = Some(now);
        true
    }
}

struct Inner {
    state: Mutex<Debouncer>,
    emit: Box<dyn Fn() + Send + Sync>,
    timer_tx: Sender<Instant>,
}

/// Cheap to clone; every clone shares one debounce state. The timer thread
/// stops when the last clone is dropped.
#[derive(Clone)]
pub struct HistoryChanged {
    inner: Arc<Inner>,
}

impl HistoryChanged {
    /// `emit` is called on the notifying thread (leading edge) or on the timer
    /// thread (trailing edge), never while the internal lock is held.
    pub fn new(emit: impl Fn() + Send + Sync + 'static) -> io::Result<Self> {
        Self::with_interval(MIN_INTERVAL, emit)
    }

    pub fn with_interval(
        interval: Duration,
        emit: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let (timer_tx, timer_rx) = crossbeam_channel::unbounded();
        let inner = Arc::new(Inner {
            state: Mutex::new(Debouncer::new(interval)),
            emit: Box::new(emit),
            timer_tx,
        });
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("history-changed".into())
            .spawn(move || timer_loop(timer_rx, weak))?;
        Ok(Self { inner })
    }

    /// The history changed. Cheap: one mutex lock, no allocation.
    pub fn notify(&self) {
        let decision = lock(&self.inner.state).on_change(Instant::now());
        match decision {
            Decision::EmitNow => (self.inner.emit)(),
            Decision::WaitUntil(t) => {
                let _ = self.inner.timer_tx.send(t);
            }
            Decision::AlreadyScheduled => {}
        }
    }

    /// Emits the Tauri event `history:changed` (no payload) to all windows.
    pub fn for_app(app: tauri::AppHandle) -> io::Result<Self> {
        use tauri::Emitter;
        Self::new(move || {
            if let Err(e) = app.emit(crate::events::HISTORY_CHANGED, ()) {
                eprintln!("clipdump: could not emit history:changed: {e}");
            }
        })
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A panic in `emit` must not wedge the history events forever.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn timer_loop(rx: Receiver<Instant>, inner: Weak<Inner>) {
    // `recv` fails once every `HistoryChanged` clone (and so the sender) is gone.
    while let Ok(deadline) = rx.recv() {
        let now = Instant::now();
        if deadline > now {
            std::thread::sleep(deadline - now);
        }
        let Some(inner) = inner.upgrade() else { return };
        let fire = lock(&inner.state).on_timer(Instant::now());
        if fire {
            (inner.emit)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const I: Duration = Duration::from_millis(50);

    fn ms(base: Instant, n: u64) -> Instant {
        base + Duration::from_millis(n)
    }

    #[test]
    fn first_change_emits_immediately() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(I);
        assert_eq!(d.on_change(t0), Decision::EmitNow);
    }

    #[test]
    fn changes_inside_the_window_merge_into_one_trailing_event() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(I);
        assert_eq!(d.on_change(t0), Decision::EmitNow);
        assert_eq!(d.on_change(ms(t0, 10)), Decision::WaitUntil(ms(t0, 50)));
        assert_eq!(d.on_change(ms(t0, 20)), Decision::AlreadyScheduled);
        assert_eq!(d.on_change(ms(t0, 49)), Decision::AlreadyScheduled);
        assert!(d.on_timer(ms(t0, 50)));
        assert!(!d.on_timer(ms(t0, 51)), "only one trailing event");
    }

    #[test]
    fn spacing_between_events_is_at_least_the_interval() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(I);
        let mut emits = vec![];
        // A change every 7 ms for 400 ms, firing timers on schedule.
        let mut timer: Option<Instant> = None;
        for step in 0..=57u64 {
            let now = ms(t0, step * 7);
            if let Some(t) = timer.filter(|&t| now >= t) {
                assert!(d.on_timer(t));
                emits.push(t);
                timer = None;
            }
            match d.on_change(now) {
                Decision::EmitNow => emits.push(now),
                Decision::WaitUntil(t) => timer = Some(t),
                Decision::AlreadyScheduled => {}
            }
        }
        assert!(emits.len() >= 7);
        for w in emits.windows(2) {
            assert!(w[1] - w[0] >= I, "{:?}", w[1] - w[0]);
        }
    }

    #[test]
    fn a_change_after_a_quiet_period_emits_immediately_again() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(I);
        assert_eq!(d.on_change(t0), Decision::EmitNow);
        assert_eq!(
            d.on_change(ms(t0, 50)),
            Decision::EmitNow,
            "window is exclusive"
        );
        assert_eq!(d.on_change(ms(t0, 500)), Decision::EmitNow);
    }

    #[test]
    fn late_timer_restarts_the_window_from_when_it_fired() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(I);
        d.on_change(t0);
        d.on_change(ms(t0, 5));
        assert!(d.on_timer(ms(t0, 80)));
        // 30 ms after the trailing emit is still inside the new window.
        assert_eq!(d.on_change(ms(t0, 110)), Decision::WaitUntil(ms(t0, 130)));
    }

    #[test]
    fn timer_with_nothing_pending_does_nothing() {
        let mut d = Debouncer::new(I);
        assert!(!d.on_timer(Instant::now()));
    }

    #[test]
    fn real_time_burst_gives_leading_plus_one_trailing_event() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let h = HistoryChanged::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        for _ in 0..20 {
            h.notify();
        }
        assert_eq!(count.load(Ordering::SeqCst), 1, "leading edge only so far");
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(count.load(Ordering::SeqCst), 2, "one merged trailing event");
        // Quiet now: next change is immediate again.
        h.notify();
        assert_eq!(count.load(Ordering::SeqCst), 3);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::SeqCst), 3, "no spurious events");
    }

    #[test]
    fn timer_thread_exits_when_the_last_handle_is_dropped() {
        let h = HistoryChanged::new(|| {}).unwrap();
        let h2 = h.clone();
        drop(h);
        h2.notify();
        drop(h2);
        // Nothing to assert beyond "does not hang or panic"; the thread ends
        // on its own once the channel disconnects.
        std::thread::sleep(Duration::from_millis(20));
    }
}
