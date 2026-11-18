//! `copy_item`: put an item back on the clipboard, optionally paste it.
//!
//! The pure parts: load the payload (RAM, cache or disk), tell the self-write
//! guard around the write, `mark_used`, hide the bar and take the window to
//! paste into. The clipboard write and the paste are the platform traits
//! (`platform::Writer`, `platform::PasterImpl`).

use std::sync::atomic::{AtomicIsize, Ordering};

use crate::hash::hash_path_bufs;
use crate::model::Payload;
use crate::perf::{self, Event};
use crate::platform::clipfmt::path_is_listable;
use crate::platform::self_write::SelfWriteGuard;
use crate::platform::{ClipboardWriter, Paster};
use crate::search::PathChecker;
use crate::store::Store;

/// What `copy_item` reads and updates. Borrowed so tests can build it without a
/// Tauri runtime.
pub struct CopyEnv<'a> {
    pub store: &'a Store,
    /// Real existence checks (not the 2 second cache): a copy is one user
    /// action, and a stale "exists" would put a dead path on the clipboard.
    pub paths: &'a dyn PathChecker,
    pub self_write: &'a SelfWriteGuard,
    /// Raw handle of the window to paste into (0 = none), see `AppState`.
    pub prev_foreground: &'a AtomicIsize,
    /// Raw handle of the bar itself. Never a paste target: Ctrl+V into the
    /// search box would paste into the query.
    pub own_window: Option<isize>,
    /// Monotonic milliseconds for the self-write guard (`self_write::mono_ms`).
    /// A function, not a number: the guard is stamped twice, before and after
    /// the write, and the write can take a while.
    pub clock: fn() -> u64,
}

/// Drop paths that no longer exist (and paths that cannot go into a file list)
/// from a file payload. Errors when none is left (Phase 11: "if all are
/// missing, show an error toast instead").
fn keep_existing(payload: Payload, paths: &dyn PathChecker) -> Result<Payload, String> {
    match payload {
        Payload::Files(list) => {
            let kept: Vec<_> = list
                .into_iter()
                .filter(|p| {
                    let s = p.to_string_lossy();
                    path_is_listable(&s) && paths.exists(&s)
                })
                .collect();
            if kept.is_empty() {
                return Err("none of the files exist any more".to_owned());
            }
            Ok(Payload::Files(kept))
        }
        other => Ok(other),
    }
}

/// The hash the watcher will compute from what `payload` puts on the clipboard.
/// Text and images: the item hash (the written text hashes to the stored text's
/// hash because CRLF folds back to LF, a picture hashes by its pixels). Files:
/// the hash of the list actually written, which is shorter than the item's when
/// paths are missing.
fn written_hash(payload: &Payload, item_hash: u128) -> u128 {
    match payload {
        Payload::Files(list) => hash_path_bufs(list),
        _ => item_hash,
    }
}

/// Copy item `id` to the clipboard with writer `W`; when `paste` is set, also
/// paste into the previously focused window with `P`. `hide` hides the bar and
/// runs once the clipboard is set (before the paste, so the target gets focus).
///
/// Order, and why:
/// 1. build the payload (fails fast on an unknown id or a missing blob, before
///    anything is touched);
/// 2. the writer builds its bytes, then calls back to arm the self-write guard
///    right before it opens the clipboard: `WM_CLIPBOARDUPDATE` can reach the
///    watcher thread before the write call returns, so the guard must know
///    first, but not earlier than needed (a 4K image takes tens of
///    milliseconds to build, and the image window is only 150 ms);
/// 3. the guard is stamped again once the write succeeded, so its window counts
///    from the end of the write;
/// 4. on failure return, leaving the bar open and the history untouched;
/// 5. `mark_used` (moves the item to the top and counts the use; the echo of
///    this write only moves it, see `Store::bump_hash`), hide, then paste.
///
/// Paste happens only when asked for AND a window was recorded AND it is not the
/// bar itself; otherwise the item is just copied.
pub fn copy_item_with<W: ClipboardWriter, P: Paster>(
    env: &CopyEnv<'_>,
    id: u64,
    paste: bool,
    hide: impl FnOnce(),
) -> Result<(), String> {
    let t_copy = perf::start();
    let item = env.store.get(id).ok_or("item not found")?;
    let t_payload = perf::start();
    let payload = env
        .store
        .payload(id)
        .map_err(|_| "could not load the item".to_owned())
        .and_then(|p| keep_existing(p, env.paths))?;
    perf::finish(t_payload, Event::CopyPayload, item.byte_size);

    let content_hash = written_hash(&payload, item.hash);
    let note = || {
        env.self_write
            .note_write_as(item.kind, item.hash, content_hash, (env.clock)());
    };
    W::set(&payload, &note).map_err(|e| e.to_string())?;
    note();
    // The "Ctrl+Enter to clipboard set" figure: command start to the write done.
    perf::finish(t_copy, Event::copy(item.kind), item.byte_size);

    env.store.mark_used(id);
    hide();

    if paste {
        // Take it: the next bar opening records a fresh one, and a stale handle
        // must never receive a Ctrl+V.
        let raw = env.prev_foreground.swap(0, Ordering::Relaxed);
        if raw != 0 && Some(raw) != env.own_window {
            P::paste_into(Some(raw)).map_err(|e| format!("copied, but auto-paste failed: {e}"))?;
        }
    }
    Ok(())
}
