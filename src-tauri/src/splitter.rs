//! The queue behind "Split": a few worker threads that run the LLM calls one
//! after another, so splitting never slows the app and never hammers the server.
//!
//! * At most [`WORKERS`] calls run at the same time; the rest wait in a queue.
//! * The queue is last in, first out: what the user asked for most recently is
//!   what they are looking at, so it goes first (arrowing quickly through the
//!   list must not make the current item wait behind ten skipped ones).
//! * Results are kept in memory only, by content hash (the same text copied twice
//!   shares one result), never on disk: the pieces are often passwords and tokens
//!   and the history is encrypted at rest, a plain cache next to it would not be.
//! * A failure is kept too and is not retried by itself; only `force` runs it
//!   again. That is what stops a wrong API key from being retried on every look.
//!
//! The job itself (`run`) and the "something changed" hook are closures, so the
//! queue is tested without a network or a window.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::llm::Part;

/// Calls to the LLM server that run at once.
pub const WORKERS: usize = 2;
/// Jobs waiting at most; past it the oldest waiting job is dropped.
pub const MAX_QUEUED: usize = 50;
/// Finished results kept (oldest dropped first).
pub const KEEP_FINISHED: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Done(Vec<Part>),
    Failed(String),
}

struct Job {
    id: u64,
    hash: u128,
}

#[derive(Default)]
struct Inner {
    /// Waiting jobs, the next one to run at the front.
    queue: VecDeque<Job>,
    entries: HashMap<u128, Status>,
    /// Hashes of finished entries, oldest first.
    finished: VecDeque<u128>,
    closed: bool,
}

type Run = dyn Fn(u64) -> Result<Vec<Part>, String> + Send + Sync;

struct Shared {
    inner: Mutex<Inner>,
    cv: Condvar,
    run: Box<Run>,
    changed: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Splitter {
    shared: Arc<Shared>,
}

impl Splitter {
    /// Start `workers` threads. `run(id)` does one split; `changed()` is called
    /// (from a worker, no lock held) whenever a job starts or finishes.
    pub fn start(
        workers: usize,
        run: impl Fn(u64) -> Result<Vec<Part>, String> + Send + Sync + 'static,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
            run: Box::new(run),
            changed: Box::new(changed),
        });
        for n in 0..workers.max(1) {
            let worker = Arc::clone(&shared);
            let _ = std::thread::Builder::new()
                .name(format!("clipdump-split-{n}"))
                .spawn(move || work(&worker));
        }
        Self { shared }
    }

    /// Ask for the text with content hash `hash` (item `id`) to be split, and
    /// return where it stands. Already queued, running, done or failed: nothing
    /// new happens, unless `force` is set and it is done or failed, which runs it
    /// again. Otherwise the job goes to the front of the queue.
    pub fn request(&self, id: u64, hash: u128, force: bool) -> Status {
        let mut g = self.shared.lock();
        match g.entries.get(&hash) {
            Some(s @ (Status::Queued | Status::Running)) => return s.clone(),
            Some(s) if !force => return s.clone(),
            _ => {}
        }
        g.finished.retain(|h| *h != hash);
        if g.queue.len() >= MAX_QUEUED
            && let Some(dropped) = g.queue.pop_back()
        {
            g.entries.remove(&dropped.hash);
        }
        g.queue.push_front(Job { id, hash });
        g.entries.insert(hash, Status::Queued);
        drop(g);
        self.shared.cv.notify_one();
        Status::Queued
    }

    /// Where the text with content hash `hash` stands, `None` if never asked for.
    pub fn status(&self, hash: u128) -> Option<Status> {
        self.shared.lock().entries.get(&hash).cloned()
    }
}

impl Drop for Splitter {
    /// Tell the workers to stop. They are not joined: one may be inside a slow
    /// HTTP call, and a quit must not wait for it.
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.cv.notify_all();
    }
}

fn work(shared: &Shared) {
    loop {
        let job = {
            let mut g = shared.lock();
            loop {
                if g.closed {
                    return;
                }
                if let Some(job) = g.queue.pop_front() {
                    g.entries.insert(job.hash, Status::Running);
                    break job;
                }
                g = shared.cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
        };
        (shared.changed)();
        let outcome = catch_unwind(AssertUnwindSafe(|| (shared.run)(job.id)))
            .unwrap_or_else(|_| Err("the split stopped unexpectedly".to_owned()));
        {
            let mut g = shared.lock();
            // Dropped by a full queue or replaced meanwhile: only a job still
            // marked running owns its entry.
            if matches!(g.entries.get(&job.hash), Some(Status::Running)) {
                g.entries.insert(
                    job.hash,
                    match outcome {
                        Ok(parts) => Status::Done(parts),
                        Err(message) => Status::Failed(message),
                    },
                );
                g.finished.push_back(job.hash);
                while g.finished.len() > KEEP_FINISHED {
                    if let Some(old) = g.finished.pop_front() {
                        g.entries.remove(&old);
                    }
                }
            }
        }
        (shared.changed)();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    fn part(v: &str) -> Part {
        Part {
            label: "L".into(),
            value: v.into(),
        }
    }

    fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn done(s: &Splitter, hash: u128) -> bool {
        matches!(s.status(hash), Some(Status::Done(_) | Status::Failed(_)))
    }

    #[test]
    fn a_result_is_kept_and_the_same_text_is_not_asked_twice() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let s = Splitter::start(
            2,
            move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(vec![part("x")])
            },
            || {},
        );
        assert_eq!(s.status(1), None);
        assert_eq!(s.request(10, 1, false), Status::Queued);
        wait_until("done", || done(&s, 1));
        assert_eq!(s.status(1), Some(Status::Done(vec![part("x")])));
        // Another look, and another item with the same content: no new call.
        assert!(matches!(s.request(10, 1, false), Status::Done(_)));
        assert!(matches!(s.request(11, 1, false), Status::Done(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // The re-split button.
        s.request(10, 1, true);
        wait_until("second run", || {
            calls.load(Ordering::SeqCst) == 2 && done(&s, 1)
        });
    }

    #[test]
    fn never_more_than_two_calls_at_once() {
        let now = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let (n, m) = (Arc::clone(&now), Arc::clone(&max));
        let s = Splitter::start(
            WORKERS,
            move |_| {
                let cur = n.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(cur, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(60));
                n.fetch_sub(1, Ordering::SeqCst);
                Ok(Vec::new())
            },
            || {},
        );
        for h in 1..=8u128 {
            s.request(h as u64, h, false);
        }
        wait_until("all done", || (1..=8).all(|h| done(&s, h)));
        assert_eq!(
            max.load(Ordering::SeqCst),
            2,
            "two at a time, not one, not more"
        );
    }

    #[test]
    fn the_newest_request_goes_first() {
        let (started_tx, started_rx) = mpsc::channel::<u64>();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let go_rx = Mutex::new(go_rx);
        let order = Arc::new(Mutex::new(Vec::new()));
        let o = Arc::clone(&order);
        let s = Splitter::start(
            1,
            move |id| {
                started_tx.send(id).unwrap();
                go_rx.lock().unwrap().recv().unwrap();
                o.lock().unwrap().push(id);
                Ok(Vec::new())
            },
            || {},
        );
        s.request(1, 1, false);
        assert_eq!(
            started_rx.recv().unwrap(),
            1,
            "the first one is running and blocked"
        );
        s.request(2, 2, false);
        s.request(3, 3, false);
        s.request(4, 4, false);
        for _ in 0..4 {
            go_tx.send(()).unwrap();
        }
        wait_until("all done", || (1..=4).all(|h| done(&s, h)));
        assert_eq!(*order.lock().unwrap(), vec![1, 4, 3, 2]);
    }

    #[test]
    fn a_failure_is_kept_and_only_force_runs_it_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let s = Splitter::start(
            1,
            move |_| {
                if c.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err("the LLM server refused the API key".into())
                } else {
                    Ok(vec![part("ok")])
                }
            },
            || {},
        );
        s.request(1, 7, false);
        wait_until("failed", || done(&s, 7));
        assert!(matches!(s.request(1, 7, false), Status::Failed(m) if m.contains("API key")));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry by itself");
        s.request(1, 7, true);
        wait_until("retried", || matches!(s.status(7), Some(Status::Done(_))));
    }

    #[test]
    fn a_panicking_job_fails_and_the_worker_goes_on() {
        let s = Splitter::start(
            1,
            |id| {
                if id == 1 {
                    panic!("boom");
                }
                Ok(Vec::new())
            },
            || {},
        );
        s.request(1, 1, false);
        wait_until("failed", || done(&s, 1));
        assert!(matches!(s.status(1), Some(Status::Failed(_))));
        s.request(2, 2, false);
        wait_until("next job", || matches!(s.status(2), Some(Status::Done(_))));
    }

    #[test]
    fn the_hook_fires_when_a_job_starts_and_when_it_ends() {
        let changes = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&changes);
        let s = Splitter::start(
            1,
            |_| Ok(Vec::new()),
            move || {
                c.fetch_add(1, Ordering::SeqCst);
            },
        );
        s.request(1, 1, false);
        wait_until("two changes", || changes.load(Ordering::SeqCst) == 2);
    }

    #[test]
    fn a_full_queue_drops_its_oldest_waiting_job() {
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let go_rx = Mutex::new(go_rx);
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let s = Splitter::start(
            1,
            move |_| {
                started_tx.send(()).unwrap();
                let _ = go_rx.lock().unwrap().recv();
                Ok(Vec::new())
            },
            || {},
        );
        s.request(0, 0, false);
        started_rx.recv().unwrap(); // job 0 holds the only worker
        for h in 1..=(MAX_QUEUED as u128 + 1) {
            s.request(h as u64, h, false);
        }
        assert_eq!(s.status(1), None, "the oldest waiting job was dropped");
        assert_eq!(s.status(2), Some(Status::Queued));
        assert_eq!(s.status(MAX_QUEUED as u128 + 1), Some(Status::Queued));
        drop(go_tx); // lets blocked and later jobs fail fast on recv
    }
}
