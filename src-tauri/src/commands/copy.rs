//! `copy_item`: put an item back on the clipboard, optionally paste it.
//!
//! The pure parts are real: load the payload (RAM, cache or disk), tell the
//! self-write guard before touching the clipboard, `mark_used`, hide the bar and
//! take the window to paste into. The clipboard write and the paste are the
//! platform traits (`platform::Writer`, `platform::PasterImpl`), which are
//! Phase 11 stubs that return an error, so today a copy fails with a plain
//! message and nothing else changes.

use std::sync::atomic::{AtomicIsize, Ordering};

use crate::model::Payload;
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
    /// Monotonic milliseconds for the self-write guard (`self_write::mono_ms`).
    pub now_ms: u64,
}

/// Drop paths that no longer exist from a file payload. Errors when none is left
/// (Phase 11: "if all are missing, show an error toast instead").
fn keep_existing(payload: Payload, paths: &dyn PathChecker) -> Result<Payload, String> {
    match payload {
        Payload::Files(list) => {
            let kept: Vec<_> = list
                .into_iter()
                .filter(|p| paths.exists(&p.to_string_lossy()))
                .collect();
            if kept.is_empty() {
                return Err("none of the files exist any more".to_owned());
            }
            Ok(Payload::Files(kept))
        }
        other => Ok(other),
    }
}

/// Copy item `id` to the clipboard with writer `W`; when `paste` is set, also
/// paste into the previously focused window with `P`. `hide` hides the bar and
/// runs once the clipboard is set (before the paste, so the target gets focus).
///
/// Order, and why:
/// 1. build the payload (fails fast on an unknown id or a missing blob, before
///    anything is touched);
/// 2. `note_write` before the write, so the watcher can never see the echo
///    before the guard knows about it;
/// 3. write; on failure return, leaving the bar open and the history untouched;
/// 4. `mark_used` (moves the item to the top), hide, then paste.
pub fn copy_item_with<W: ClipboardWriter, P: Paster>(
    env: &CopyEnv<'_>,
    id: u64,
    paste: bool,
    hide: impl FnOnce(),
) -> Result<(), String> {
    let item = env.store.get(id).ok_or("item not found")?;
    let payload = env
        .store
        .payload(id)
        .map_err(|_| "could not load the item".to_owned())
        .and_then(|p| keep_existing(p, env.paths))?;

    env.self_write.note_write(item.kind, item.hash, env.now_ms);
    W::set(&payload).map_err(|e| e.to_string())?;

    env.store.mark_used(id);
    hide();

    if paste {
        // Take it: the next bar opening records a fresh one, and a stale handle
        // must never receive a Ctrl+V.
        let raw = env.prev_foreground.swap(0, Ordering::Relaxed);
        let target = (raw != 0).then_some(raw);
        P::paste_into(target).map_err(|e| format!("copied, but auto-paste failed: {e}"))?;
    }
    Ok(())
}
