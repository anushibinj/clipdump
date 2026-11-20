//! The queue behind "Split": a few worker threads that run the LLM calls one
//! after another, so splitting never slows the app and never hammers the server.
//!
//! * At most [`WORKERS`] calls run at the same time; the rest wait in a queue.
//! * The queue is first in, first out: texts are split in the order they were
//!   copied. A job is queued the moment a text is captured (see
//!   `Store::set_new_text_hook`), so by the time the user opens the bar the result
//!   is usually there already.
//! * Results are kept by content hash (the same text copied twice shares one
//!   result) in memory and, through [`Persistence`], in the encrypted history
//!   database, so they survive a restart. Only successes are persisted.
//! * A failure is kept in memory and is not retried by itself; only `force` runs
//!   it again. That is what stops a wrong API key from being retried on every look.
//!
//! The job itself (`run`) and the "something changed" hook are closures, so the
//! queue is tested without a network or a window.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::llm::Part;

/// Calls to the LLM server that run at once.
pub const WORKERS: usize = 2;
/// Jobs waiting at most. A request past it is refused (nothing already waiting
/// is dropped: first come, first served) and can simply be made again later.
pub const MAX_QUEUED: usize = 200;
/// Finished results kept (oldest dropped first).
pub const KEEP_FINISHED: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Done(Vec<Part>),
    Failed(String),
}

/// Where finished splits are kept between runs.
pub trait Persistence: Send + Sync {
    fn load(&self, hash: u128) -> Option<Vec<Part>>;
    fn save(&self, hash: u128, parts: &[Part]);
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
    persistence: Option<Arc<dyn Persistence>>,
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
        Self::start_with(workers, run, changed, None)
    }

    /// [`Splitter::start`] with finished splits kept in `persistence`.
    pub fn start_with(
        workers: usize,
        run: impl Fn(u64) -> Result<Vec<Part>, String> + Send + Sync + 'static,
        changed: impl Fn() + Send + Sync + 'static,
        persistence: Option<Arc<dyn Persistence>>,
    ) -> Self {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
            run: Box::new(run),
            changed: Box::new(changed),
            persistence,
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
    /// return where it stands. Already queued, running, done (in memory or on
    /// disk) or failed: nothing new happens, unless `force` is set and it is done
    /// or failed, which runs it again. Otherwise the job goes to the back of the
    /// queue, or is refused as a `Failed` that is not stored when the queue is full.
    pub fn request(&self, id: u64, hash: u128, force: bool) -> Status {
        if !force && let Some(known) = self.status(hash) {
            return known;
        }
        let mut g = self.shared.lock();
        match g.entries.get(&hash) {
            Some(s @ (Status::Queued | Status::Running)) => return s.clone(),
            Some(s) if !force => return s.clone(),
            _ => {}
        }
        if g.queue.len() >= MAX_QUEUED {
            return Status::Failed("the split queue is full, try again in a moment".to_owned());
        }
        g.finished.retain(|h| *h != hash);
        g.queue.push_back(Job { id, hash });
        g.entries.insert(hash, Status::Queued);
        drop(g);
        self.shared.cv.notify_one();
        Status::Queued
    }

    /// Where the text with content hash `hash` stands, `None` if never asked for.
    /// A result that is only on disk is loaded (and then kept in memory).
    pub fn status(&self, hash: u128) -> Option<Status> {
        if let Some(s) = self.shared.lock().entries.get(&hash) {
            return Some(s.clone());
        }
        // The disk read happens without the lock.
        let parts = self.shared.persistence.as_ref()?.load(hash)?;
        let mut g = self.shared.lock();
        if !g.entries.contains_key(&hash) {
            g.entries.insert(hash, Status::Done(parts));
            g.finished.push_back(hash);
            trim(&mut g);
        }
        g.entries.get(&hash).cloned()
    }
}

/// Forget the oldest finished results past [`KEEP_FINISHED`] (they stay on disk).
fn trim(g: &mut Inner) {
    while g.finished.len() > KEEP_FINISHED {
        if let Some(old) = g.finished.pop_front() {
            g.entries.remove(&old);
        }
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
        // Written down before the entry says "done", so a restart right after
        // the UI showed the pieces still finds them.
        if let (Ok(parts), Some(p)) = (&outcome, &shared.persistence) {
            p.save(job.hash, parts);
        }
        {
            let mut g = shared.lock();
            g.entries.insert(
                job.hash,
                match outcome {
                    Ok(parts) => Status::Done(parts),
                    Err(message) => Status::Failed(message),
                },
            );
            g.finished.push_back(job.hash);
            trim(&mut g);
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
    fn requests_are_served_first_come_first_served() {
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
        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3, 4]);
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
    fn a_full_queue_refuses_the_newest_and_keeps_what_waits() {
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
        for h in 1..=MAX_QUEUED as u128 {
            assert_eq!(s.request(h as u64, h, false), Status::Queued);
        }
        let over = MAX_QUEUED as u128 + 1;
        assert!(
            matches!(s.request(over as u64, over, false), Status::Failed(m) if m.contains("full"))
        );
        assert_eq!(
            s.status(over),
            None,
            "a refusal is not stored, asking again later works"
        );
        assert_eq!(
            s.status(1),
            Some(Status::Queued),
            "the first in line is untouched"
        );
        drop(go_tx);
    }

    #[derive(Default)]
    struct Mem(Mutex<HashMap<u128, Vec<Part>>>);

    impl Persistence for Mem {
        fn load(&self, hash: u128) -> Option<Vec<Part>> {
            self.0.lock().unwrap().get(&hash).cloned()
        }
        fn save(&self, hash: u128, parts: &[Part]) {
            self.0.lock().unwrap().insert(hash, parts.to_vec());
        }
    }

    #[test]
    fn results_survive_a_restart_through_the_persistence() {
        let disk = Arc::new(Mem::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let first = Splitter::start_with(
            1,
            move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(vec![part("kept")])
            },
            || {},
            Some(disk.clone()),
        );
        first.request(1, 5, false);
        wait_until("done", || done(&first, 5));
        assert_eq!(disk.load(5), Some(vec![part("kept")]), "written to disk");
        drop(first);

        // A new run: nothing in memory, the answer comes from disk, no call.
        let c = Arc::clone(&calls);
        let second = Splitter::start_with(
            1,
            move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            },
            || {},
            Some(disk.clone()),
        );
        assert_eq!(second.status(5), Some(Status::Done(vec![part("kept")])));
        assert_eq!(
            second.request(1, 5, false),
            Status::Done(vec![part("kept")])
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Re-split replaces what is on disk.
        second.request(1, 5, true);
        wait_until("re-run", || {
            calls.load(Ordering::SeqCst) == 2
                && matches!(second.status(5), Some(Status::Done(p)) if p.is_empty())
        });
        assert_eq!(disk.load(5), Some(Vec::new()));
    }

    #[test]
    fn failures_are_not_written_to_disk() {
        let disk = Arc::new(Mem::default());
        let s = Splitter::start_with(1, |_| Err("no".into()), || {}, Some(disk.clone()));
        s.request(1, 8, false);
        wait_until("failed", || done(&s, 8));
        assert_eq!(disk.load(8), None);
    }
}
