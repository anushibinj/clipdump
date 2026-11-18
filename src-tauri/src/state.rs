//! Shared application state, registered with `app.manage(...)`.
//!
//! Everything the hot path touches is a plain atomic so reading it never blocks
//! and never allocates.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, RwLock};

use tauri::{AppHandle, Manager};

use crate::platform::CaptureContext;
use crate::platform::self_write::SelfWriteGuard;
use crate::search::CachedPathChecker;
use crate::settings::Settings;

#[derive(Default)]
pub struct AppState {
    /// Clipboard capture paused (tray toggle). Read by the watcher thread.
    pub paused: Arc<AtomicBool>,
    /// Runtime settings. Minimal until Phase 12 loads and saves them.
    pub settings: Arc<RwLock<Settings>>,
    /// Records the app's own clipboard writes so the watcher can ignore the echo.
    /// Phase 11's copy path calls `note_write` on it.
    pub self_write: Arc<SelfWriteGuard>,
    /// "Pinned bar" mode: the bar stays visible and does not hide on blur.
    /// Setting stub until Phase 12 wires it to settings.json. Default off.
    pub pinned_bar: AtomicBool,
    /// Raw handle (HWND on Windows, 0 elsewhere) of the window that had focus
    /// before the bar opened. Used by auto-paste in Phase 11.
    pub prev_foreground: AtomicIsize,
    /// File existence checks for search rows and previews, cached for 2 seconds
    /// so typing never repeats a `stat` (see `search::paths`).
    pub paths: Arc<CachedPathChecker>,
}

/// Is pinned-bar mode on? Relaxed is enough: it is an independent flag.
pub fn pinned_bar(app: &AppHandle) -> bool {
    app.state::<AppState>().pinned_bar.load(Ordering::Relaxed)
}

impl AppState {
    /// The slice of state the clipboard watcher thread needs.
    pub fn capture_context(&self) -> CaptureContext {
        CaptureContext {
            paused: Arc::clone(&self.paused),
            settings: Arc::clone(&self.settings),
            self_write: Arc::clone(&self.self_write),
        }
    }
}
