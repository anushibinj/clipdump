//! Shared application state, registered with `app.manage(...)`.
//!
//! Everything the hot path touches is a plain atomic so reading it never blocks
//! and never allocates.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use tauri::{AppHandle, Manager};

use crate::pause::Pause;
use crate::platform::CaptureContext;
use crate::platform::self_write::SelfWriteGuard;
use crate::search::CachedPathChecker;
use crate::settings::Settings;
use crate::settings::saver::SettingsSaver;
use crate::shutdown::Shutdown;

#[derive(Default)]
pub struct AppState {
    /// Clipboard capture paused: the flag the watcher thread reads, kept in step
    /// with the tray check item and the Settings switch (see `pause.rs`).
    pub pause: Pause,
    /// Runtime settings, loaded from `settings.json` at startup and read from
    /// here (memory) on every hot path.
    pub settings: Arc<RwLock<Settings>>,
    /// Writes `settings.json` on a background thread. Empty until `setup` has
    /// found the config folder (and in tests).
    pub saver: OnceLock<SettingsSaver>,
    /// Why the configured hotkey is not active (another app owns it), shown in
    /// the settings view. `None` when it is registered.
    pub hotkey_error: Mutex<Option<String>>,
    /// Set when the history had to be moved aside at startup: what happened and
    /// where the old files are. Shown by the bar until dismissed (`StatsDto`).
    pub recovery: Mutex<Option<String>>,
    /// The history folder (`%LOCALAPPDATA%\clipdump`), for "Open data folder".
    pub data_dir: OnceLock<PathBuf>,
    /// Records the app's own clipboard writes so the watcher can ignore the echo.
    /// `copy_item` calls `note_write_as` on it around every clipboard write.
    pub self_write: Arc<SelfWriteGuard>,
    /// "Pinned bar" mode: the bar stays visible and does not hide on blur or
    /// after a copy. Mirrors `Settings::pinned_bar` as an atomic so the blur
    /// handler and the hotkey never take the settings lock. Default off.
    pub pinned_bar: AtomicBool,
    /// Raw handle (HWND on Windows, 0 elsewhere) of the window that had focus
    /// before the bar opened. `copy_item` takes it for auto-paste.
    pub prev_foreground: AtomicIsize,
    /// The once only gate of the graceful shutdown (`shutdown.rs`).
    pub shutdown: Shutdown,
    /// The physical size last applied to the bar, width in the high 32 bits and
    /// height in the low 32 (0 before the first show). `window.rs` only sets the
    /// size when the target monitor's scale makes it differ.
    pub bar_size: AtomicU64,
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
            paused: self.pause.flag(),
            settings: Arc::clone(&self.settings),
            self_write: Arc::clone(&self.self_write),
        }
    }
}
