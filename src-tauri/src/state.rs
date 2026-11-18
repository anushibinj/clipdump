//! Shared application state, registered with `app.manage(...)`.
//!
//! Everything the hot path touches is a plain atomic so reading it never blocks
//! and never allocates.

use std::sync::atomic::AtomicBool;

#[derive(Default)]
pub struct AppState {
    /// Clipboard capture paused (tray toggle). Consumed by the watcher in Phase 4.
    pub paused: AtomicBool,
    /// "Pinned bar" mode: the bar stays visible and does not hide on blur.
    /// Setting stub until Phase 12 wires it to settings.json. Default off.
    pub pinned_bar: AtomicBool,
}
