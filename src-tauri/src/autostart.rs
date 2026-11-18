//! Start at login. Enabled by default on first run only: a marker file records
//! that we already made that choice, so a user who later turns it off (settings
//! UI in Phase 10, or the OS startup manager) is never overridden.
//!
//! This runs once in `setup`, never on the hotkey path. A tiny marker file is
//! the simplest durable flag until settings.json exists (Phase 12), which
//! should absorb it.

use std::fs;

use tauri::{App, Manager};
use tauri_plugin_autostart::ManagerExt;

const MARKER: &str = "autostart-initialized";

pub fn enable_on_first_run(app: &App) {
    // Dev builds must not register a debug executable as a login item.
    // Set CLIPDUMP_AUTOSTART=1 to exercise this path from a dev build.
    if cfg!(debug_assertions) && std::env::var_os("CLIPDUMP_AUTOSTART").is_none() {
        return;
    }
    let Ok(dir) = app.path().app_config_dir() else {
        return;
    };
    let marker = dir.join(MARKER);
    if marker.exists() {
        return;
    }
    match app.autolaunch().enable() {
        Ok(()) => {
            // Only record success, so a failure is retried on the next launch.
            if fs::create_dir_all(&dir).is_ok() {
                let _ = fs::write(&marker, b"");
            }
        }
        Err(e) => eprintln!("clipdump: could not enable autostart: {e}"),
    }
}
