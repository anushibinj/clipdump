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

use crate::platform;
use crate::platform::self_write::mono_ms;
use crate::search::{FsPathChecker, PathChecker, ResultDto, SearchRequest, TypeFilter};
use crate::state::AppState;
use crate::store::Store;
use crate::window;

pub mod copy;
pub mod preview;
#[cfg(test)]
mod tests;

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
    store.search(&req, paths)
}

#[tauri::command(async)]
pub fn search(
    app: AppHandle,
    query: String,
    filter: Option<TypeFilter>,
    limit: Option<usize>,
) -> Result<Vec<ResultDto>, String> {
    let store = store(&app)?;
    let state = app.state::<AppState>();
    Ok(search_items(
        &store,
        state.paths.as_ref(),
        &query,
        filter.unwrap_or_default(),
        limit,
    ))
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
/// The clipboard write and the paste are Phase 11 stubs for now: this returns
/// their error and changes nothing.
#[tauri::command(async)]
pub fn copy_item(app: AppHandle, id: u64, paste: bool) -> Result<(), String> {
    let store = store(&app)?;
    let state = app.state::<AppState>();
    let env = CopyEnv {
        store: &store,
        paths: &FsPathChecker,
        self_write: &state.self_write,
        prev_foreground: &state.prev_foreground,
        now_ms: mono_ms(),
    };
    copy_item_with::<platform::Writer, platform::PasterImpl>(&env, id, paste, || window::hide(&app))
}
