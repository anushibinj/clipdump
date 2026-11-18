//! Shared application state, registered with `app.manage(...)`.
//!
//! Everything the hot path touches is a plain atomic so reading it never blocks
//! and never allocates.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct AppState {
    /// Clipboard capture paused (tray toggle). Consumed by the watcher in Phase 4.
    pub paused: AtomicBool,
    /// "Pinned bar" mode: the bar stays visible and does not hide on blur.
    /// Setting stub until Phase 12 wires it to settings.json. Default off.
    pub pinned_bar: AtomicBool,
    /// Raw handle (HWND on Windows, 0 elsewhere) of the window that had focus
    /// before the bar opened. Used by auto-paste in Phase 11.
    pub prev_foreground: AtomicIsize,
}

/// Is pinned-bar mode on? Relaxed is enough: it is an independent flag.
pub fn pinned_bar(app: &AppHandle) -> bool {
    app.state::<AppState>().pinned_bar.load(Ordering::Relaxed)
}
