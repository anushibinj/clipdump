//! Commands of the Notes view. Thin wrappers over [`NoteStore`]; the rules (limits, tag
//! cleaning, ids) live in `notes.rs` and are tested there.

use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::commands::tasks::system_now_ms;
use crate::notes::{Note, NotePatch, NoteStore};
use crate::state::AppState;

/// The note store, or a plain error when it could not start (it needs the history's key).
fn notes(app: &AppHandle) -> Result<Arc<NoteStore>, String> {
    app.state::<AppState>()
        .notes
        .get()
        .map(Arc::clone)
        .ok_or_else(|| "the notes are not available".to_owned())
}

#[tauri::command(async)]
pub fn list_notes(app: AppHandle) -> Result<Vec<Note>, String> {
    Ok(notes(&app)?.list())
}

/// Add a note; every field may be empty. Returns it with its id.
#[tauri::command(async)]
pub fn add_note(
    app: AppHandle,
    title: String,
    body: String,
    tags: Vec<String>,
) -> Result<Note, String> {
    notes(&app)?
        .add(&title, &body, &tags, system_now_ms())
        .map_err(|e| e.to_string())
}

/// Change the fields in `patch` (title, body, pinned, tags). Returns the note as stored.
#[tauri::command(async)]
pub fn update_note(app: AppHandle, id: u64, patch: NotePatch) -> Result<Note, String> {
    notes(&app)?
        .update(id, patch, system_now_ms())
        .map_err(|e| e.to_string())
}

/// Delete a note. Resolves to whether it existed.
#[tauri::command(async)]
pub fn delete_note(app: AppHandle, id: u64) -> Result<bool, String> {
    notes(&app)?.delete(id).map_err(|e| e.to_string())
}
