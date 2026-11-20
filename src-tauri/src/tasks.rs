//! The task list behind the Tasks view.
//!
//! Tasks are a handful of short lines, not clipboard content, so they live in
//! their own encrypted file (`tasks.bin` next to `history.db`) instead of the
//! history database:
//!
//! * "Clear history" and the history recovery (`persist::recovery`) never touch
//!   them, and retention never evicts them.
//! * The whole list is rewritten on every change (a few KB at most): write a temp
//!   file, rename over the old one. A crash leaves the old or the new list, never
//!   half of one. This is not the hot path (no hotkey, typing or copy goes
//!   through here), so there is no writer thread to wait for.
//!
//! The store knows nothing about Tauri or about the cipher: it takes a [`Sealer`],
//! so the tests run with a fake one. Errors are fixed sentences ([`TaskError`])
//! that never contain a task's text, because they reach the webview.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Deserializer, Serialize};

/// File name inside the data directory.
pub const FILE: &str = "tasks.bin";
/// Most tasks kept, finished ones included. The oldest finished task makes room.
pub const MAX_TASKS: usize = 500;
/// Longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 300;
/// Longest note ("where I left off"), in characters.
pub const MAX_NOTE_CHARS: usize = 1000;
/// Longest suggested next step, in characters.
pub const MAX_STEP_CHARS: usize = 300;
/// Clipboard items one task can be linked to.
pub const MAX_LINKS: usize = 20;
/// Finished tasks older than this are dropped when the list is loaded (30 days).
pub const DONE_KEEP_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Encrypts and decrypts the file. Implemented for the history's cipher in
/// `commands::tasks`.
pub trait Sealer: Send + Sync {
    fn seal(&self, plain: &[u8]) -> anyhow::Result<Vec<u8>>;
    fn open(&self, sealed: &[u8]) -> anyhow::Result<Vec<u8>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Waiting.
    Todo,
    /// Being worked on right now. Any number of tasks can be in flight.
    Doing,
    Done,
}

/// How long a task takes, as estimated by the LLM (or set by hand): what the
/// "quick wins" filter looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    /// About 15 minutes or less.
    Quick,
    /// Up to about an hour.
    Short,
    /// More than an hour.
    Long,
}

/// What kind of work a task is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Small, mechanical, can be done between other things.
    Quick,
    /// Needs a block of focus.
    Deep,
    /// Blocked on someone or something else.
    Waiting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: u64,
    pub title: String,
    /// Where the work was left off, so picking it up again takes no thinking.
    pub note: String,
    pub status: Status,
    pub important: bool,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms of the last change of any kind.
    pub touched_at: i64,
    /// Unix ms, set while the task is done.
    pub done_at: Option<i64>,
    /// Estimated effort. Set by the LLM after the task is added; none until then.
    /// The fields below all default, so a `tasks.bin` from before they existed still loads.
    #[serde(default)]
    pub size: Option<Size>,
    #[serde(default)]
    pub kind: Option<Kind>,
    /// A concrete first step suggested by the LLM. Empty when there is none.
    #[serde(default)]
    pub next_step: String,
    /// Clipboard items (history ids) that belong to this task. An id whose item has
    /// since left the history is simply not shown.
    #[serde(default)]
    pub clip_ids: Vec<u64>,
    /// Unix ms. Until then the task is not nagged about as stale.
    #[serde(default)]
    pub deferred_until: Option<i64>,
}

/// A JSON field that can be absent (leave alone), null (clear) or a value (set).
fn set_or_clear<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// The fields `update` can change; a missing field stays as it is.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPatch {
    pub title: Option<String>,
    pub note: Option<String>,
    pub status: Option<Status>,
    pub important: Option<bool>,
    /// `null` clears the estimate.
    #[serde(default, deserialize_with = "set_or_clear")]
    pub size: Option<Option<Size>>,
    #[serde(default, deserialize_with = "set_or_clear")]
    pub kind: Option<Option<Kind>>,
    /// An empty string clears the suggestion.
    pub next_step: Option<String>,
    /// Replaces the linked items.
    pub clip_ids: Option<Vec<u64>>,
    /// `null` clears the deferral.
    #[serde(default, deserialize_with = "set_or_clear")]
    pub deferred_until: Option<Option<i64>>,
}

impl TaskPatch {
    /// Whether the patch changes something the person did or decided (a title, a note, a
    /// status, importance, a link, a deferral). Estimates and suggestions come from the LLM,
    /// and must not make an untouched task look touched: that would hide it from the
    /// "gone quiet" nudge.
    fn is_a_touch(&self) -> bool {
        self.title.is_some()
            || self.note.is_some()
            || self.status.is_some()
            || self.important.is_some()
            || self.clip_ids.is_some()
            || self.deferred_until.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskError {
    EmptyTitle,
    TitleTooLong,
    NoteTooLong,
    StepTooLong,
    TooManyLinks,
    NotFound,
    Full,
    Save,
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::EmptyTitle => "the task needs a title",
            Self::TitleTooLong => "the title is too long",
            Self::NoteTooLong => "the note is too long",
            Self::StepTooLong => "the suggested step is too long",
            Self::TooManyLinks => "too many linked clipboard items",
            Self::NotFound => "task not found",
            Self::Full => "too many tasks, delete or finish some first",
            Self::Save => "could not save the tasks",
        })
    }
}

impl std::error::Error for TaskError {}

/// What is on disk: the list and the next id (kept, so the id of a deleted task
/// is not handed out again while the webview may still show it).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    next_id: u64,
    tasks: Vec<Task>,
}

#[derive(Clone)]
struct State {
    next_id: u64,
    tasks: Vec<Task>,
}

pub struct TaskStore {
    path: PathBuf,
    sealer: Arc<dyn Sealer>,
    state: Mutex<State>,
}

/// Where an unreadable `tasks.bin` goes, so a bad file is kept for the user instead of
/// being overwritten by the next save.
fn aside_path(dir: &Path, now_ms: i64) -> PathBuf {
    dir.join(format!("tasks.unreadable-{now_ms}.bin"))
}

fn read_file(path: &Path, sealer: &dyn Sealer) -> anyhow::Result<Option<File>> {
    let sealed = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let json = sealer.open(&sealed)?;
    Ok(Some(serde_json::from_slice(&json)?))
}

fn trimmed(s: &str, max: usize, too_long: TaskError) -> Result<String, TaskError> {
    let s = s.trim();
    if s.chars().count() > max {
        return Err(too_long);
    }
    Ok(s.to_owned())
}

impl TaskStore {
    /// Load `dir/tasks.bin`. A missing file is an empty list. A file that cannot
    /// be read (damaged, or sealed with another key) is renamed to
    /// `tasks.unreadable-<ms>.bin` and the list starts empty; the second value is
    /// where it went, for a log line.
    pub fn open(dir: &Path, sealer: Arc<dyn Sealer>, now_ms: i64) -> (Self, Option<PathBuf>) {
        let path = dir.join(FILE);
        let mut moved = None;
        let mut state = State {
            next_id: 1,
            tasks: Vec::new(),
        };
        match read_file(&path, sealer.as_ref()) {
            Ok(Some(file)) => {
                state.next_id = file
                    .next_id
                    .max(file.tasks.iter().map(|t| t.id).max().map_or(1, |m| m + 1));
                state.tasks = file.tasks;
                state
                    .tasks
                    .retain(|t| t.done_at.is_none_or(|at| now_ms - at <= DONE_KEEP_MS));
            }
            Ok(None) => {}
            Err(_) => {
                let to = aside_path(dir, now_ms);
                if fs::rename(&path, &to).is_ok() {
                    moved = Some(to);
                }
            }
        }
        let store = Self {
            path,
            sealer,
            state: Mutex::new(state),
        };
        (store, moved)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every task, in the order they were added.
    pub fn list(&self) -> Vec<Task> {
        self.lock().tasks.clone()
    }

    /// Task `id`, if it exists.
    pub fn get(&self, id: u64) -> Option<Task> {
        self.lock().tasks.iter().find(|t| t.id == id).cloned()
    }

    /// Write `next` to disk; only when that worked does it replace the list in memory.
    fn commit(&self, state: &mut State, next: State) -> Result<(), TaskError> {
        self.write(&next)?;
        *state = next;
        Ok(())
    }

    fn write(&self, state: &State) -> Result<(), TaskError> {
        let file = File {
            next_id: state.next_id,
            tasks: state.tasks.clone(),
        };
        let json = serde_json::to_vec(&file).map_err(|_| TaskError::Save)?;
        let sealed = self.sealer.seal(&json).map_err(|_| TaskError::Save)?;
        let tmp = self.path.with_extension("bin.tmp");
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&sealed)?;
            f.sync_all()?;
            fs::rename(&tmp, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
            return Err(TaskError::Save);
        }
        Ok(())
    }

    /// Add a task, waiting (`start` false) or already in flight (`start` true).
    pub fn add(
        &self,
        title: &str,
        important: bool,
        start: bool,
        now_ms: i64,
    ) -> Result<Task, TaskError> {
        let title = trimmed(title, MAX_TITLE_CHARS, TaskError::TitleTooLong)?;
        if title.is_empty() {
            return Err(TaskError::EmptyTitle);
        }
        let mut guard = self.lock();
        let mut next = guard.clone();
        if next.tasks.len() >= MAX_TASKS {
            // The oldest finished task makes room; with none, the list is full.
            let oldest_done = next
                .tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.status == Status::Done)
                .min_by_key(|(_, t)| t.done_at.unwrap_or(t.touched_at))
                .map(|(i, _)| i)
                .ok_or(TaskError::Full)?;
            next.tasks.remove(oldest_done);
        }
        let task = Task {
            id: next.next_id,
            title,
            note: String::new(),
            status: if start { Status::Doing } else { Status::Todo },
            important,
            created_at: now_ms,
            touched_at: now_ms,
            done_at: None,
            size: None,
            kind: None,
            next_step: String::new(),
            clip_ids: Vec::new(),
            deferred_until: None,
        };
        next.next_id += 1;
        next.tasks.push(task.clone());
        self.commit(&mut guard, next)?;
        Ok(task)
    }

    /// Apply `patch` to task `id`. Every change counts as touching the task.
    pub fn update(&self, id: u64, patch: TaskPatch, now_ms: i64) -> Result<Task, TaskError> {
        let title = patch
            .title
            .as_deref()
            .map(|t| trimmed(t, MAX_TITLE_CHARS, TaskError::TitleTooLong))
            .transpose()?;
        if title.as_deref() == Some("") {
            return Err(TaskError::EmptyTitle);
        }
        let note = patch
            .note
            .as_deref()
            .map(|n| trimmed(n, MAX_NOTE_CHARS, TaskError::NoteTooLong))
            .transpose()?;
        let step = patch
            .next_step
            .as_deref()
            .map(|s| trimmed(s, MAX_STEP_CHARS, TaskError::StepTooLong))
            .transpose()?;
        let links = match &patch.clip_ids {
            None => None,
            Some(ids) => {
                let mut kept: Vec<u64> = Vec::new();
                for id in ids {
                    if !kept.contains(id) {
                        kept.push(*id);
                    }
                }
                if kept.len() > MAX_LINKS {
                    return Err(TaskError::TooManyLinks);
                }
                Some(kept)
            }
        };
        let touch = patch.is_a_touch();

        let mut guard = self.lock();
        let mut next = guard.clone();
        let task = next
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(TaskError::NotFound)?;
        if let Some(title) = title {
            task.title = title;
        }
        if let Some(note) = note {
            task.note = note;
        }
        if let Some(important) = patch.important {
            task.important = important;
        }
        if let Some(status) = patch.status {
            task.status = status;
            task.done_at = (status == Status::Done).then_some(now_ms);
        }
        if let Some(size) = patch.size {
            task.size = size;
        }
        if let Some(kind) = patch.kind {
            task.kind = kind;
        }
        if let Some(step) = step {
            task.next_step = step;
        }
        if let Some(links) = links {
            task.clip_ids = links;
        }
        if let Some(until) = patch.deferred_until {
            task.deferred_until = until;
        }
        if touch {
            task.touched_at = now_ms;
        }
        let updated = task.clone();
        self.commit(&mut guard, next)?;
        Ok(updated)
    }

    /// Delete task `id`. Returns whether it existed.
    pub fn delete(&self, id: u64) -> Result<bool, TaskError> {
        let mut guard = self.lock();
        let mut next = guard.clone();
        let before = next.tasks.len();
        next.tasks.retain(|t| t.id != id);
        if next.tasks.len() == before {
            return Ok(false);
        }
        self.commit(&mut guard, next)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reverses the bytes and checks a marker: enough to see that the file is not
    /// plain JSON and that another "key" cannot read it.
    struct Fake(u8);

    impl Sealer for Fake {
        fn seal(&self, plain: &[u8]) -> anyhow::Result<Vec<u8>> {
            let mut out = vec![self.0];
            out.extend(plain.iter().rev());
            Ok(out)
        }
        fn open(&self, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
            match sealed.split_first() {
                Some((k, rest)) if *k == self.0 => Ok(rest.iter().rev().copied().collect()),
                _ => anyhow::bail!("wrong key"),
            }
        }
    }

    fn open(dir: &Path, key: u8, now: i64) -> (TaskStore, Option<PathBuf>) {
        TaskStore::open(dir, Arc::new(Fake(key)), now)
    }

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn add_list_and_survive_a_restart() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 100);
        let a = s.add("  write the report ", true, false, 100).unwrap();
        let b = s.add("call back", false, true, 200).unwrap();
        assert_eq!((a.id, b.id), (1, 2));
        assert_eq!(a.title, "write the report");
        assert_eq!(a.status, Status::Todo);
        assert_eq!(b.status, Status::Doing);
        assert!(a.important && !b.important);

        let (again, moved) = open(d.path(), 1, 300);
        assert!(moved.is_none());
        assert_eq!(again.list(), s.list());
    }

    #[test]
    fn file_is_not_plain_text() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        s.add("secret plan", false, false, 0).unwrap();
        let raw = fs::read(d.path().join(FILE)).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("secret plan"));
    }

    #[test]
    fn title_rules() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        assert_eq!(s.add("   ", false, false, 0), Err(TaskError::EmptyTitle));
        let long = "x".repeat(MAX_TITLE_CHARS + 1);
        assert_eq!(s.add(&long, false, false, 0), Err(TaskError::TitleTooLong));
        // Characters, not bytes.
        let wide = "é".repeat(MAX_TITLE_CHARS);
        assert!(s.add(&wide, false, false, 0).is_ok());
        assert!(s.list().len() == 1);
    }

    #[test]
    fn update_changes_only_what_is_sent_and_touches() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        let t = s.add("a", false, false, 10).unwrap();
        let t2 = s
            .update(
                t.id,
                TaskPatch {
                    note: Some(" left off at step 3 ".into()),
                    ..Default::default()
                },
                50,
            )
            .unwrap();
        assert_eq!(t2.note, "left off at step 3");
        assert_eq!(t2.title, "a");
        assert_eq!(t2.created_at, 10);
        assert_eq!(t2.touched_at, 50);
        assert_eq!(t2.status, Status::Todo);
    }

    #[test]
    fn done_sets_and_clears_done_at() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        let t = s.add("a", false, true, 0).unwrap();
        let done = s
            .update(
                t.id,
                TaskPatch {
                    status: Some(Status::Done),
                    ..Default::default()
                },
                70,
            )
            .unwrap();
        assert_eq!(done.done_at, Some(70));
        let back = s
            .update(
                t.id,
                TaskPatch {
                    status: Some(Status::Todo),
                    ..Default::default()
                },
                80,
            )
            .unwrap();
        assert_eq!(back.done_at, None);
    }

    #[test]
    fn update_rejects_bad_input_and_unknown_ids() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        let t = s.add("a", false, false, 0).unwrap();
        let empty = TaskPatch {
            title: Some(" ".into()),
            ..Default::default()
        };
        assert_eq!(s.update(t.id, empty, 1), Err(TaskError::EmptyTitle));
        let note = TaskPatch {
            note: Some("n".repeat(MAX_NOTE_CHARS + 1)),
            ..Default::default()
        };
        assert_eq!(s.update(t.id, note, 1), Err(TaskError::NoteTooLong));
        assert_eq!(
            s.update(99, TaskPatch::default(), 1),
            Err(TaskError::NotFound)
        );
        // Nothing above changed the task.
        assert_eq!(s.list()[0].touched_at, 0);
    }

    #[test]
    fn delete_and_ids_are_not_reused() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        let a = s.add("a", false, false, 0).unwrap();
        let b = s.add("b", false, false, 0).unwrap();
        assert_eq!(s.delete(b.id), Ok(true));
        assert_eq!(s.delete(b.id), Ok(false));
        let (again, _) = open(d.path(), 1, 0);
        let c = again.add("c", false, false, 0).unwrap();
        assert!(c.id > b.id && c.id != a.id);
    }

    #[test]
    fn unreadable_file_is_moved_aside_not_overwritten() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        s.add("keep me", false, false, 0).unwrap();
        // Another key (history recovered with a new data key): cannot be read.
        let (other, moved) = open(d.path(), 2, 555);
        assert!(other.list().is_empty());
        let moved = moved.expect("moved aside");
        assert!(moved.ends_with("tasks.unreadable-555.bin"));
        assert!(moved.exists());
        assert!(!d.path().join(FILE).exists());
        // And the new list saves normally.
        other.add("fresh", false, false, 600).unwrap();
        assert_eq!(open(d.path(), 2, 700).0.list().len(), 1);
    }

    #[test]
    fn corrupt_json_is_moved_aside() {
        let d = dir();
        fs::write(d.path().join(FILE), Fake(1).seal(b"{not json").unwrap()).unwrap();
        let (s, moved) = open(d.path(), 1, 9);
        assert!(s.list().is_empty());
        assert!(moved.is_some());
    }

    #[test]
    fn old_finished_tasks_are_dropped_on_load() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        let old = s.add("old", false, true, 0).unwrap();
        let fresh = s.add("fresh", false, true, 0).unwrap();
        s.add("still open", false, true, 0).unwrap();
        s.update(
            old.id,
            TaskPatch {
                status: Some(Status::Done),
                ..Default::default()
            },
            1_000,
        )
        .unwrap();
        s.update(
            fresh.id,
            TaskPatch {
                status: Some(Status::Done),
                ..Default::default()
            },
            DONE_KEEP_MS,
        )
        .unwrap();
        let (again, _) = open(d.path(), 1, DONE_KEEP_MS + 2_000);
        let titles: Vec<_> = again.list().into_iter().map(|t| t.title).collect();
        assert_eq!(titles, vec!["fresh", "still open"]);
    }

    #[test]
    fn full_list_drops_the_oldest_done_or_refuses() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        for i in 0..MAX_TASKS {
            s.add(&format!("t{i}"), false, false, 0).unwrap();
        }
        assert_eq!(s.add("one more", false, false, 0), Err(TaskError::Full));
        let list = s.list();
        s.update(
            list[7].id,
            TaskPatch {
                status: Some(Status::Done),
                ..Default::default()
            },
            5,
        )
        .unwrap();
        s.add("one more", false, false, 6).unwrap();
        let list = s.list();
        assert_eq!(list.len(), MAX_TASKS);
        assert!(list.iter().all(|t| t.title != "t7"));
        assert!(list.iter().any(|t| t.title == "one more"));
    }

    #[test]
    fn failed_save_changes_nothing_in_memory() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        s.add("a", false, false, 0).unwrap();
        // A directory where the temp file must go makes the write fail.
        fs::create_dir(d.path().join("tasks.bin.tmp")).unwrap();
        assert_eq!(s.add("b", false, false, 0), Err(TaskError::Save));
        assert_eq!(s.list().len(), 1);
    }

    #[test]
    fn wire_format_is_camel_case_with_lowercase_status() {
        let t = Task {
            id: 1,
            title: "a".into(),
            note: String::new(),
            status: Status::Doing,
            important: true,
            created_at: 1,
            touched_at: 2,
            done_at: None,
            size: Some(Size::Quick),
            kind: None,
            next_step: String::new(),
            clip_ids: vec![4],
            deferred_until: None,
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["size"], "quick");
        assert!(v["kind"].is_null());
        assert_eq!(v["clipIds"], serde_json::json!([4]));
        assert_eq!(v["nextStep"], "");
        assert_eq!(v["status"], "doing");
        assert_eq!(v["createdAt"], 1);
        assert_eq!(v["touchedAt"], 2);
        assert!(v["doneAt"].is_null());
        let p: TaskPatch = serde_json::from_str(r#"{"status":"done","important":false}"#).unwrap();
        assert_eq!(p.status, Some(Status::Done));
        assert_eq!(p.important, Some(false));
        assert!(p.title.is_none());
    }

    #[test]
    fn estimates_and_suggestions_do_not_touch_the_task() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        let t = s.add("a", false, true, 10).unwrap();
        let t2 = s
            .update(
                t.id,
                TaskPatch {
                    size: Some(Some(Size::Quick)),
                    kind: Some(Some(Kind::Deep)),
                    next_step: Some("  open the file ".into()),
                    ..Default::default()
                },
                9_000,
            )
            .unwrap();
        assert_eq!(t2.size, Some(Size::Quick));
        assert_eq!(t2.kind, Some(Kind::Deep));
        assert_eq!(t2.next_step, "open the file");
        // The LLM filled these in; the person did nothing, so it is still untouched.
        assert_eq!(t2.touched_at, 10);
        // A link, a deferral and a note are the person's doing.
        for patch in [
            TaskPatch {
                clip_ids: Some(vec![3]),
                ..Default::default()
            },
            TaskPatch {
                deferred_until: Some(Some(5)),
                ..Default::default()
            },
        ] {
            let before = s.list()[0].touched_at;
            let after = s.update(t.id, patch, before + 100).unwrap().touched_at;
            assert_eq!(after, before + 100);
        }
    }

    #[test]
    fn null_clears_and_absent_leaves_alone() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        let t = s.add("a", false, false, 0).unwrap();
        let set: TaskPatch =
            serde_json::from_str(r#"{"size":"long","kind":"waiting","deferredUntil":50}"#).unwrap();
        let t = s.update(t.id, set, 1).unwrap();
        assert_eq!(
            (t.size, t.kind, t.deferred_until),
            (Some(Size::Long), Some(Kind::Waiting), Some(50))
        );
        let leave: TaskPatch = serde_json::from_str(r#"{"important":true}"#).unwrap();
        let t = s.update(t.id, leave, 2).unwrap();
        assert_eq!(t.size, Some(Size::Long));
        let clear: TaskPatch =
            serde_json::from_str(r#"{"size":null,"deferredUntil":null}"#).unwrap();
        let t = s.update(t.id, clear, 3).unwrap();
        assert_eq!(
            (t.size, t.kind, t.deferred_until),
            (None, Some(Kind::Waiting), None)
        );
    }

    #[test]
    fn links_are_deduplicated_and_capped_and_the_step_is_bounded() {
        let d = dir();
        let (s, _) = open(d.path(), 1, 0);
        let t = s.add("a", false, false, 0).unwrap();
        let t = s
            .update(
                t.id,
                TaskPatch {
                    clip_ids: Some(vec![5, 6, 5]),
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        assert_eq!(t.clip_ids, vec![5, 6]);
        let many = TaskPatch {
            clip_ids: Some((0..=MAX_LINKS as u64).collect()),
            ..Default::default()
        };
        assert_eq!(s.update(t.id, many, 2), Err(TaskError::TooManyLinks));
        let long = TaskPatch {
            next_step: Some("s".repeat(MAX_STEP_CHARS + 1)),
            ..Default::default()
        };
        assert_eq!(s.update(t.id, long, 2), Err(TaskError::StepTooLong));
        assert_eq!(s.list()[0].clip_ids, vec![5, 6]);
    }

    #[test]
    fn a_list_saved_before_the_ai_fields_still_loads() {
        let d = dir();
        let old = r#"{"nextId":3,"tasks":[{"id":2,"title":"old","note":"","status":"todo",
            "important":false,"createdAt":1,"touchedAt":1,"doneAt":null}]}"#;
        fs::write(d.path().join(FILE), Fake(1).seal(old.as_bytes()).unwrap()).unwrap();
        let (s, moved) = open(d.path(), 1, 5);
        assert!(moved.is_none());
        let t = &s.list()[0];
        assert_eq!(t.title, "old");
        assert!(t.size.is_none() && t.next_step.is_empty() && t.clip_ids.is_empty());
    }
}
