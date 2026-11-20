//! The IPC surface: every `#[tauri::command]` the webview may call (Phase 9).
//!
//! Layout: each command is a thin wrapper that fetches the managed state and
//! calls a plain function taking `&Store` and friends. The plain functions hold
//! all the logic and are what the tests cover, so no Tauri runtime is needed.
//!
//! Rules:
//!
//! * Commands are declared `async` in the Tauri sense (`#[tauri::command(async)]`
//!   on a plain `fn`): they run on the runtime's thread pool, not on the main
//!   thread, so a slow disk read can never freeze the window. Search itself is
//!   an in-memory scan and never waits for the disk.
//! * Never send big payloads: previews are truncated here (see [`preview`]),
//!   images and thumbnails travel through the `clip` URI scheme, not through IPC.
//! * Errors are `Result<_, String>` with plain fixed messages. They never
//!   contain clipboard content (the string reaches the webview and may be logged).
//! * Wire names are camelCase; enum values are lowercase words. The TypeScript
//!   mirror is `src/lib/ipc.ts`.

use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::autostart::SystemAutostart;
use crate::hotkey::PluginRegistrar;
use crate::ocr::OcrWorker;
use crate::perf::{self, AckKind, Event};
use crate::platform;
use crate::platform::self_write::mono_ms;
use crate::search::{FsPathChecker, PathChecker, ResultDto, SearchRequest, TypeFilter};
use crate::state::AppState;
use crate::store::Store;
use crate::window;

#[cfg(test)]
mod bounds_tests;
pub mod config;
pub mod copy;
pub mod folder;
pub mod preview;
pub mod split;
pub mod tasks;
#[cfg(test)]
mod tests;

pub use config::{SettingsDto, SettingsEnv, StatsDto, stats_of};
pub use copy::{CopyEnv, copy_item_with};
pub use preview::{PreviewDto, preview_item};

/// The history store, or a plain error when it failed to open at startup (the
/// bar and tray still run in that case, see `lib::start_capture`).
fn store(app: &AppHandle) -> Result<Arc<Store>, String> {
    app.try_state::<Arc<Store>>()
        .map(|s| Arc::clone(s.inner()))
        .ok_or_else(|| "the history is not available".to_owned())
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

/// Rank the history for `query` (typed prefixes `img:`, `file:`, `txt:` win over
/// `filter`). `limit` defaults to 50 and is capped by the search module.
pub fn search_items(
    store: &Store,
    paths: &dyn PathChecker,
    query: &str,
    filter: TypeFilter,
    limit: Option<usize>,
) -> Vec<ResultDto> {
    let mut req = SearchRequest::new(query, filter, store.now_ms());
    if let Some(limit) = limit {
        req.limit = limit;
    }
    let t = perf::start();
    let found = store.search(&req, paths);
    perf::finish(t, Event::SearchCore, found.len() as u64);
    found
}

#[tauri::command(async)]
pub fn search(
    app: AppHandle,
    query: String,
    filter: Option<TypeFilter>,
    limit: Option<usize>,
) -> Result<Vec<ResultDto>, String> {
    let t = perf::start();
    let store = store(&app)?;
    let state = app.state::<AppState>();
    let found = search_items(
        &store,
        state.paths.as_ref(),
        &query,
        filter.unwrap_or_default(),
        limit,
    );
    perf::finish(t, Event::SearchIpc, found.len() as u64);
    Ok(found)
}

// ---------------------------------------------------------------------------
// get_preview
// ---------------------------------------------------------------------------

/// The Tab pane content of one item, capped in size (see [`preview`]).
#[tauri::command(async)]
pub fn get_preview(app: AppHandle, id: u64) -> Result<PreviewDto, String> {
    let store = store(&app)?;
    let state = app.state::<AppState>();
    preview_item(&store, state.paths.as_ref(), id)
}

// ---------------------------------------------------------------------------
// copy_item
// ---------------------------------------------------------------------------

/// Copy an item back to the clipboard and hide the bar; with `paste`, also paste
/// into the window that was focused before the bar opened.
///
/// A failed write returns its error and changes nothing. A write that worked
/// but a paste that did not returns "copied, but auto-paste failed: ...".
#[tauri::command(async)]
pub fn copy_item(app: AppHandle, id: u64, paste: bool) -> Result<(), String> {
    let store = store(&app)?;
    let state = app.state::<AppState>();
    let env = CopyEnv {
        store: &store,
        paths: &FsPathChecker,
        self_write: &state.self_write,
        prev_foreground: &state.prev_foreground,
        own_window: platform::own_window_handle(&app),
        clock: mono_ms,
    };
    // Pinned-bar mode: the bar is a permanent strip, a copy does not hide it
    // (Esc does). With Paste on Enter the paste still moves the focus back to
    // the window the hotkey was pressed over.
    copy_item_with::<platform::Writer, platform::PasterImpl>(&env, id, paste, || {
        if !crate::state::pinned_bar(&app) {
            window::hide(&app);
        }
    })
}

// ---------------------------------------------------------------------------
// delete_item, toggle_pin, clear_history
// ---------------------------------------------------------------------------

/// Delete one item, pinned or not (an explicit delete always wins). Returns
/// whether it existed: deleting an item that retention just evicted is not an
/// error. The store removes the blob and thumbnail files.
pub fn delete_one(store: &Store, id: u64) -> bool {
    store.remove(id)
}

/// Flip the pin of `id`; returns the new state.
pub fn toggle_pin_of(store: &Store, id: u64) -> Result<bool, String> {
    store
        .toggle_pin(id)
        .ok_or_else(|| "item not found".to_owned())
}

/// "Clear history": delete every unpinned item. Returns how many were removed.
pub fn clear_unpinned(store: &Store) -> usize {
    store.clear_unpinned()
}

#[tauri::command(async)]
pub fn delete_item(app: AppHandle, id: u64) -> Result<bool, String> {
    Ok(delete_one(store(&app)?.as_ref(), id))
}

#[tauri::command(async)]
pub fn toggle_pin(app: AppHandle, id: u64) -> Result<bool, String> {
    toggle_pin_of(store(&app)?.as_ref(), id)
}

#[tauri::command(async)]
pub fn clear_history(app: AppHandle) -> Result<usize, String> {
    Ok(clear_unpinned(store(&app)?.as_ref()))
}

// ---------------------------------------------------------------------------
// get_settings, set_settings, get_stats, hide_bar
// ---------------------------------------------------------------------------

/// Run `f` with the settings environment of the running app.
fn with_settings_env<T>(
    app: &AppHandle,
    f: impl FnOnce(&SettingsEnv<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let store = store(app)?;
    let state = app.state::<AppState>();
    let ocr = app.try_state::<OcrWorker>();
    let registrar = PluginRegistrar(app);
    let autostart = SystemAutostart(app);
    let env = SettingsEnv {
        settings: &state.settings,
        pinned_bar: &state.pinned_bar,
        pause: &state.pause,
        store: &store,
        ocr: ocr.as_ref().map(|w| w.queue()),
        registrar: &registrar,
        autostart: &autostart,
        saver: state.saver.get(),
        hotkey_error: &state.hotkey_error,
    };
    f(&env)
}

#[tauri::command(async)]
pub fn get_settings(app: AppHandle) -> Result<SettingsDto, String> {
    with_settings_env(&app, |env| Ok(env.current()))
}

/// Validate and apply the settings now, and save them in the background.
/// Returns the settings as stored, which can differ from the input (a tidied
/// ignore list, a canonical hotkey). A hotkey another app owns, or a login item
/// the OS refuses, returns a plain error and applies nothing.
#[tauri::command(async)]
pub fn set_settings(app: AppHandle, settings: SettingsDto) -> Result<SettingsDto, String> {
    with_settings_env(&app, |env| env.apply(settings))
}

#[tauri::command(async)]
pub fn get_stats(app: AppHandle) -> Result<StatsDto, String> {
    let store = store(&app)?;
    let state = app.state::<AppState>();
    let ocr = app.try_state::<OcrWorker>();
    let hotkey_error = state
        .hotkey_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let mut stats = stats_of(
        &store,
        &state.settings,
        ocr.as_ref().map(|w| w.queue()),
        hotkey_error,
    );
    stats.recovery = state
        .recovery
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    stats.perf = perf::webview_acks();
    Ok(stats)
}

/// A timing the webview measured or a paint it saw, for the `--perf` log. Does
/// nothing (and the webview does not even call it) unless the app runs with
/// `--perf`. `ms` is only read for `searchRtt`.
#[tauri::command(async)]
pub fn perf_ack(kind: AckKind, ms: Option<f64>) {
    perf::ack(kind, ms);
}

/// Hide the search bar (Esc). Idempotent.
#[tauri::command(async)]
pub fn hide_bar(app: AppHandle) {
    window::hide(&app);
}

/// The user pressed the bar's drag handle: hand the mouse to the OS window drag
/// and remember where the bar ends up (see `window::remember_position`).
#[tauri::command(async)]
pub fn start_drag(app: AppHandle) {
    window::start_user_drag(&app);
}

/// Ask for the text item `id` to be split into copyable pieces by the user's own
/// LLM server (see `llm.rs`) and return where it stands. The call itself runs in
/// the split queue (`splitter.rs`, two at a time); the webview hears
/// `split:updated` when something moves. `force` runs it again even if it is done
/// or failed (the re-split button).
#[tauri::command(async)]
pub fn request_split(
    app: AppHandle,
    id: u64,
    force: bool,
) -> Result<split::SplitStatusDto, String> {
    let store = store(&app)?;
    let state = app.state::<AppState>();
    let cfg = crate::llm::LlmConfig::from_settings(
        &state.settings.read().unwrap_or_else(|e| e.into_inner()),
    );
    let hash = split::splittable_hash(&store, cfg.as_ref(), id)?;
    let splitter = state.splitter.get().ok_or("splitting is not available")?;
    Ok(Some(splitter.request(id, hash, force)).into())
}

/// Where the split of item `id` stands, without starting one.
#[tauri::command(async)]
pub fn get_split(app: AppHandle, id: u64) -> Result<split::SplitStatusDto, String> {
    let store = store(&app)?;
    let state = app.state::<AppState>();
    let item = store.get(id).ok_or("item not found")?;
    Ok(state
        .splitter
        .get()
        .and_then(|s| s.status(item.hash))
        .into())
}

/// Put one piece of a split text on the clipboard. The bar stays open (the next
/// piece is usually wanted too) and the item is not added to the history: the
/// self-write guard swallows the echo like it does for `copy_item`.
#[tauri::command(async)]
pub fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    split::copy_snippet::<platform::Writer>(&state.self_write, mono_ms, text)
}

/// Open the history folder in the file manager. The path is the fixed data
/// directory recorded at startup, never something the webview sends.
#[tauri::command(async)]
pub fn open_data_folder(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let dir = state
        .data_dir
        .get()
        .ok_or_else(|| "the data folder is not known".to_owned())?;
    folder::open_dir(folder::file_manager(), dir)
}
