//! Names of the events the backend emits to the webview. The frontend mirrors
//! them in `src/lib/ipc.ts` (`EVENTS`); keep both lists in step.

/// The bar became visible and should focus and select its input.
pub const BAR_SHOWN: &str = "bar:shown";
/// The bar was hidden.
pub const BAR_HIDDEN: &str = "bar:hidden";
/// The history changed (new item, OCR result, delete, pin). Debounced to at most
/// one per 50 ms, no payload: the UI re-runs its search.
pub const HISTORY_CHANGED: &str = "history:changed";
/// Ask the bar to open its settings view (tray "Settings").
pub const SETTINGS_OPEN: &str = "settings:open";
/// Settings changed behind the webview's back (the tray toggled "Pause
/// capture"). No payload: the UI calls `get_settings` again.
pub const SETTINGS_CHANGED: &str = "settings:changed";
/// The history was damaged or its key unreadable at startup and was moved aside
/// (`persist::recovery`). No payload: the UI calls `get_stats`, whose `recovery`
/// field holds the sentence (so a webview that loaded after the event still
/// learns about it).
pub const HISTORY_RECOVERED: &str = "history:recovered";
