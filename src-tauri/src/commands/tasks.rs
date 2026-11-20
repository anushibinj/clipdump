//! Commands of the Tasks view. Thin wrappers over [`TaskStore`]; the rules
//! (limits, ids, what a change touches) live in `tasks.rs` and are tested there.

use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::persist::crypto::Crypto;
use crate::state::AppState;
use crate::tasks::{Sealer, Task, TaskPatch, TaskStore};

/// The history's cipher also seals the task list, so one key (itself protected by the
/// OS keystore) covers everything on disk.
impl Sealer for Crypto {
    fn seal(&self, plain: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.encrypt(plain)
    }
    fn open(&self, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.decrypt(sealed)
    }
}

/// Unix ms from the system clock.
pub fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The task store, or a plain error when it could not start (it needs the history's key).
fn tasks(app: &AppHandle) -> Result<Arc<TaskStore>, String> {
    app.state::<AppState>()
        .tasks
        .get()
        .map(Arc::clone)
        .ok_or_else(|| "the tasks are not available".to_owned())
}

pub fn add_task_to(
    store: &TaskStore,
    title: &str,
    important: bool,
    start: bool,
    now_ms: i64,
) -> Result<Task, String> {
    store
        .add(title, important, start, now_ms)
        .map_err(|e| e.to_string())
}

pub fn update_task_in(
    store: &TaskStore,
    id: u64,
    patch: TaskPatch,
    now_ms: i64,
) -> Result<Task, String> {
    store.update(id, patch, now_ms).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn list_tasks(app: AppHandle) -> Result<Vec<Task>, String> {
    Ok(tasks(&app)?.list())
}

/// Add a task; `start` puts it in flight at once. Returns it with its id.
#[tauri::command(async)]
pub fn add_task(
    app: AppHandle,
    title: String,
    important: bool,
    start: bool,
) -> Result<Task, String> {
    add_task_to(
        tasks(&app)?.as_ref(),
        &title,
        important,
        start,
        system_now_ms(),
    )
}

/// Change the fields in `patch` (title, note, status, important). Returns the task as stored.
#[tauri::command(async)]
pub fn update_task(app: AppHandle, id: u64, patch: TaskPatch) -> Result<Task, String> {
    update_task_in(tasks(&app)?.as_ref(), id, patch, system_now_ms())
}

/// Delete a task. Resolves to whether it existed.
#[tauri::command(async)]
pub fn delete_task(app: AppHandle, id: u64) -> Result<bool, String> {
    tasks(&app)?.delete(id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &std::path::Path, key: u8) -> TaskStore {
        let crypto = Arc::new(Crypto::from_key(&[key; 32]));
        TaskStore::open(dir, crypto, 0).0
    }

    #[test]
    fn tasks_are_sealed_with_the_history_cipher_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path(), 7);
        let t = add_task_to(&s, "renew the passport", true, false, 10).unwrap();
        let raw = std::fs::read(dir.path().join(crate::tasks::FILE)).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("passport"));
        assert_eq!(store(dir.path(), 7).list(), vec![t]);
    }

    #[test]
    fn errors_are_plain_sentences_without_task_text() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path(), 7);
        assert_eq!(
            add_task_to(&s, "  ", false, false, 0).unwrap_err(),
            "the task needs a title"
        );
        let secret = TaskPatch {
            title: Some("x".repeat(400)),
            ..Default::default()
        };
        let err = update_task_in(&s, 1, secret, 0).unwrap_err();
        assert!(!err.contains("xxx"));
    }
}
