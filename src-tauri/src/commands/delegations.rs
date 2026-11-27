//! Commands of the Delegations view. Thin wrappers over [`DelegationStore`]; the checks live in
//! `delegations.rs` and are tested there.

use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::delegations::DelegationStore;
use crate::state::AppState;

/// The store, or a plain error when it could not start (it needs the history's key).
fn store(app: &AppHandle) -> Result<Arc<DelegationStore>, String> {
    app.state::<AppState>()
        .delegations
        .get()
        .map(Arc::clone)
        .ok_or_else(|| "the delegations are not available".to_owned())
}

/// The saved document as JSON text, empty when nothing was saved yet.
#[tauri::command(async)]
pub fn get_delegations(app: AppHandle) -> Result<String, String> {
    Ok(store(&app)?.get())
}

/// Replace the saved document.
#[tauri::command(async)]
pub fn set_delegations(app: AppHandle, doc: String) -> Result<(), String> {
    store(&app)?.set(&doc).map_err(|e| e.to_string())
}
