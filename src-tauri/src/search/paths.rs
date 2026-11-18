//! Existence checks for file items: the "missing" badge (Section 1: files are
//! references, so a moved or deleted file must show).
//!
//! `stat` on a path is a disk (or network) access, so the rules are:
//!
//! * Never per keystroke for every item: search calls this only for the file
//!   items among the (at most 50) results it returns.
//! * Results are cached for a short time ([`MISSING_TTL`]), so typing a query
//!   re-checks a path at most once per TTL, not once per keystroke.
//! * At most [`MISSING_CHECK_MAX`] paths per item are checked. A copied folder
//!   tree can hold thousands of files; the badge only needs "some are gone".
//! * The check sits behind [`PathChecker`], so tests never touch the disk.
//!
//! Known limit: a path on a disconnected network share can make the real `stat`
//! block for seconds on Windows. Nothing here can time a `stat` out; it is listed
//! in the Phase 8 manual checks.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::model::FileRef;

/// How long a cached answer stays valid.
pub const MISSING_TTL: Duration = Duration::from_secs(2);
/// Most paths of one item that are checked. `missing_files` is a lower bound for
/// an item with more files than this.
pub const MISSING_CHECK_MAX: usize = 128;
/// Cache size bound. Reaching it drops expired entries first, then everything.
const CACHE_MAX_ENTRIES: usize = 8192;

/// Does a path still exist?
pub trait PathChecker: Send + Sync {
    fn exists(&self, path: &str) -> bool;
}

/// The real file system. Follows symlinks: a link to a deleted target is
/// missing, which is what a paste would hit.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsPathChecker;

impl PathChecker for FsPathChecker {
    fn exists(&self, path: &str) -> bool {
        std::fs::metadata(path).is_ok()
    }
}

/// How many of `files` are gone, looking at the first [`MISSING_CHECK_MAX`].
pub fn count_missing(files: &[FileRef], checker: &dyn PathChecker) -> u32 {
    files
        .iter()
        .take(MISSING_CHECK_MAX)
        .filter(|f| !checker.exists(&f.path))
        .count() as u32
}

/// Wraps a checker with a short lived cache. Share one instance for the app
/// (it is `Sync`); the lock is held only for map access, never during a `stat`.
pub struct CachedPathChecker {
    inner: Box<dyn PathChecker>,
    ttl: Duration,
    /// Source of "now"; injected so tests can move time without sleeping.
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
    entries: Mutex<HashMap<String, (Instant, bool)>>,
}

impl Default for CachedPathChecker {
    fn default() -> Self {
        Self::for_fs()
    }
}

impl CachedPathChecker {
    /// The app's checker: the real file system, [`MISSING_TTL`], wall clock.
    pub fn for_fs() -> Self {
        Self::new(Box::new(FsPathChecker), MISSING_TTL)
    }

    pub fn new(inner: Box<dyn PathChecker>, ttl: Duration) -> Self {
        Self::with_clock(inner, ttl, Box::new(Instant::now))
    }

    pub fn with_clock(
        inner: Box<dyn PathChecker>,
        ttl: Duration,
        clock: Box<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self {
            inner,
            ttl,
            clock,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Entries currently cached (tests, stats).
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl PathChecker for CachedPathChecker {
    fn exists(&self, path: &str) -> bool {
        let now = (self.clock)();
        if let Some(&(at, ok)) = self.entries.lock().get(path)
            && now.saturating_duration_since(at) < self.ttl
        {
            return ok;
        }
        // The slow part runs without the lock. Two threads may both check the
        // same path once; harmless, the answers agree.
        let ok = self.inner.exists(path);
        let mut map = self.entries.lock();
        if map.len() >= CACHE_MAX_ENTRIES {
            map.retain(|_, (at, _)| now.saturating_duration_since(*at) < self.ttl);
            if map.len() >= CACHE_MAX_ENTRIES {
                map.clear();
            }
        }
        map.insert(path.to_owned(), (now, ok));
        ok
    }
}
