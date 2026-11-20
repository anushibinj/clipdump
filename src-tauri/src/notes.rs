//! The notes behind the Notes view.
//!
//! Same storage idea as the task list (`tasks.rs`): a small list rewritten in full on every
//! change, sealed with the history's cipher, in its own file (`notes.bin`), so "Clear history"
//! and the history recovery never touch it. Notes are flat: no folders or categories, only an
//! optional title, a pin and free tags.
//!
//! Errors are fixed sentences ([`NoteError`]) that never contain a note's text, because they
//! reach the webview.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::tasks::Sealer;

/// File name inside the data directory.
pub const FILE: &str = "notes.bin";
/// Most notes kept.
pub const MAX_NOTES: usize = 1000;
/// Longest optional title, in characters.
pub const MAX_TITLE_CHARS: usize = 120;
/// Longest body, in characters.
pub const MAX_BODY_CHARS: usize = 20_000;
/// Most tags on one note.
pub const MAX_TAGS: usize = 10;
/// Longest tag, in characters.
pub const MAX_TAG_CHARS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: u64,
    /// Optional: empty when the note has none.
    pub title: String,
    pub body: String,
    pub pinned: bool,
    /// Lowercase, no spaces, no duplicates, in the order they were given.
    pub tags: Vec<String>,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms of the last change of any kind.
    pub updated_at: i64,
}

/// The fields `update` can change; a missing field stays as it is.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub pinned: Option<bool>,
    pub tags: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteError {
    TitleTooLong,
    BodyTooLong,
    TooManyTags,
    TagTooLong,
    NotFound,
    Full,
    Save,
}

impl std::fmt::Display for NoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TitleTooLong => "the title is too long",
            Self::BodyTooLong => "the note is too long",
            Self::TooManyTags => "too many tags on this note",
            Self::TagTooLong => "a tag is too long",
            Self::NotFound => "note not found",
            Self::Full => "too many notes, delete some first",
            Self::Save => "could not save the notes",
        })
    }
}

impl std::error::Error for NoteError {}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    next_id: u64,
    notes: Vec<Note>,
}

#[derive(Clone)]
struct State {
    next_id: u64,
    notes: Vec<Note>,
}

pub struct NoteStore {
    path: PathBuf,
    sealer: Arc<dyn Sealer>,
    state: Mutex<State>,
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

fn title_of(s: &str) -> Result<String, NoteError> {
    let s = s.trim();
    if s.chars().count() > MAX_TITLE_CHARS {
        return Err(NoteError::TitleTooLong);
    }
    Ok(s.to_owned())
}

fn body_of(s: &str) -> Result<String, NoteError> {
    // Not trimmed at the start: leading indentation is the user's. Trailing blanks go.
    let s = s.trim_end();
    if s.chars().count() > MAX_BODY_CHARS {
        return Err(NoteError::BodyTooLong);
    }
    Ok(s.to_owned())
}

/// Clean a tag list: strip a leading `#`, lowercase, spaces become dashes, drop blanks and
/// duplicates.
fn tags_of(raw: &[String]) -> Result<Vec<String>, NoteError> {
    let mut out: Vec<String> = Vec::new();
    for t in raw {
        let t = t
            .trim()
            .trim_start_matches('#')
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
            .to_lowercase();
        if t.is_empty() || out.contains(&t) {
            continue;
        }
        if t.chars().count() > MAX_TAG_CHARS {
            return Err(NoteError::TagTooLong);
        }
        out.push(t);
    }
    if out.len() > MAX_TAGS {
        return Err(NoteError::TooManyTags);
    }
    Ok(out)
}

impl NoteStore {
    /// Load `dir/notes.bin`. A missing file is an empty list. An unreadable one (damaged, or
    /// sealed with another key) is renamed to `notes.unreadable-<ms>.bin` and the list starts
    /// empty; the second value is where it went, for a log line.
    pub fn open(dir: &Path, sealer: Arc<dyn Sealer>, now_ms: i64) -> (Self, Option<PathBuf>) {
        let path = dir.join(FILE);
        let mut moved = None;
        let mut state = State {
            next_id: 1,
            notes: Vec::new(),
        };
        match read_file(&path, sealer.as_ref()) {
            Ok(Some(file)) => {
                state.next_id = file
                    .next_id
                    .max(file.notes.iter().map(|n| n.id).max().map_or(1, |m| m + 1));
                state.notes = file.notes;
            }
            Ok(None) => {}
            Err(_) => {
                let to = dir.join(format!("notes.unreadable-{now_ms}.bin"));
                if fs::rename(&path, &to).is_ok() {
                    moved = Some(to);
                }
            }
        }
        (
            Self {
                path,
                sealer,
                state: Mutex::new(state),
            },
            moved,
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every note, in the order they were added.
    pub fn list(&self) -> Vec<Note> {
        self.lock().notes.clone()
    }

    fn commit(&self, state: &mut State, next: State) -> Result<(), NoteError> {
        self.write(&next)?;
        *state = next;
        Ok(())
    }

    fn write(&self, state: &State) -> Result<(), NoteError> {
        let file = File {
            next_id: state.next_id,
            notes: state.notes.clone(),
        };
        let json = serde_json::to_vec(&file).map_err(|_| NoteError::Save)?;
        let sealed = self.sealer.seal(&json).map_err(|_| NoteError::Save)?;
        let tmp = self.path.with_extension("bin.tmp");
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&sealed)?;
            f.sync_all()?;
            fs::rename(&tmp, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
            return Err(NoteError::Save);
        }
        Ok(())
    }

    /// Add a note. Title, body and tags may all be empty (a blank note to type into).
    pub fn add(
        &self,
        title: &str,
        body: &str,
        tags: &[String],
        now_ms: i64,
    ) -> Result<Note, NoteError> {
        let (title, body, tags) = (title_of(title)?, body_of(body)?, tags_of(tags)?);
        let mut guard = self.lock();
        let mut next = guard.clone();
        if next.notes.len() >= MAX_NOTES {
            return Err(NoteError::Full);
        }
        let note = Note {
            id: next.next_id,
            title,
            body,
            pinned: false,
            tags,
            created_at: now_ms,
            updated_at: now_ms,
        };
        next.next_id += 1;
        next.notes.push(note.clone());
        self.commit(&mut guard, next)?;
        Ok(note)
    }

    /// Apply `patch` to note `id`. Every change counts as an update.
    pub fn update(&self, id: u64, patch: NotePatch, now_ms: i64) -> Result<Note, NoteError> {
        let title = patch.title.as_deref().map(title_of).transpose()?;
        let body = patch.body.as_deref().map(body_of).transpose()?;
        let tags = patch.tags.as_deref().map(tags_of).transpose()?;
        let mut guard = self.lock();
        let mut next = guard.clone();
        let note = next
            .notes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or(NoteError::NotFound)?;
        if let Some(title) = title {
            note.title = title;
        }
        if let Some(body) = body {
            note.body = body;
        }
        if let Some(tags) = tags {
            note.tags = tags;
        }
        if let Some(pinned) = patch.pinned {
            note.pinned = pinned;
        }
        note.updated_at = now_ms;
        let updated = note.clone();
        self.commit(&mut guard, next)?;
        Ok(updated)
    }

    /// Delete note `id`. Returns whether it existed.
    pub fn delete(&self, id: u64) -> Result<bool, NoteError> {
        let mut guard = self.lock();
        let mut next = guard.clone();
        let before = next.notes.len();
        next.notes.retain(|n| n.id != id);
        if next.notes.len() == before {
            return Ok(false);
        }
        self.commit(&mut guard, next)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn open(dir: &Path, key: u8) -> (NoteStore, Option<PathBuf>) {
        NoteStore::open(dir, Arc::new(Fake(key)), 5)
    }

    fn tags(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn add_list_and_survive_a_restart() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        let a = s.add("  Wifi ", "pass: hunter2  ", &tags(&["Home"]), 10).unwrap();
        let b = s.add("", "untitled", &[], 20).unwrap();
        assert_eq!((a.id, b.id), (1, 2));
        assert_eq!(a.title, "Wifi");
        assert_eq!(a.body, "pass: hunter2");
        assert_eq!(b.title, "");
        assert!(!a.pinned);
        assert_eq!(open(d.path(), 1).0.list(), s.list());
    }

    #[test]
    fn blank_note_is_allowed() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        assert!(s.add("", "", &[], 0).is_ok());
    }

    #[test]
    fn file_is_not_plain_text() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        s.add("", "secret plan", &[], 0).unwrap();
        let raw = fs::read(d.path().join(FILE)).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("secret plan"));
    }

    #[test]
    fn tags_are_cleaned() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        let n = s
            .add("", "x", &tags(&["#Work", "work", " big idea ", ""]), 0)
            .unwrap();
        assert_eq!(n.tags, tags(&["work", "big-idea"]));
        let long = "x".repeat(MAX_TAG_CHARS + 1);
        assert_eq!(s.add("", "", &[long], 0), Err(NoteError::TagTooLong));
        let many: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        assert_eq!(s.add("", "", &many, 0), Err(NoteError::TooManyTags));
    }

    #[test]
    fn limits() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        let t = "x".repeat(MAX_TITLE_CHARS + 1);
        assert_eq!(s.add(&t, "", &[], 0), Err(NoteError::TitleTooLong));
        let b = "x".repeat(MAX_BODY_CHARS + 1);
        assert_eq!(s.add("", &b, &[], 0), Err(NoteError::BodyTooLong));
        // Characters, not bytes.
        assert!(s.add("", &"é".repeat(MAX_BODY_CHARS), &[], 0).is_ok());
    }

    #[test]
    fn update_changes_only_what_is_sent_and_touches() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        let n = s.add("t", "b", &tags(&["a"]), 10).unwrap();
        let p = s
            .update(
                n.id,
                NotePatch {
                    pinned: Some(true),
                    ..Default::default()
                },
                50,
            )
            .unwrap();
        assert!(p.pinned);
        assert_eq!((p.title.as_str(), p.body.as_str()), ("t", "b"));
        assert_eq!(p.tags, tags(&["a"]));
        assert_eq!((p.created_at, p.updated_at), (10, 50));
        let cleared = s
            .update(
                n.id,
                NotePatch {
                    title: Some(String::new()),
                    tags: Some(vec![]),
                    ..Default::default()
                },
                60,
            )
            .unwrap();
        assert!(cleared.title.is_empty() && cleared.tags.is_empty());
        assert_eq!(
            s.update(99, NotePatch::default(), 0),
            Err(NoteError::NotFound)
        );
    }

    #[test]
    fn delete_and_ids_are_not_reused() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        let a = s.add("", "a", &[], 0).unwrap();
        assert_eq!(s.delete(a.id), Ok(true));
        assert_eq!(s.delete(a.id), Ok(false));
        let (again, _) = open(d.path(), 1);
        assert_eq!(again.add("", "b", &[], 0).unwrap().id, 2);
    }

    #[test]
    fn unreadable_file_is_moved_aside() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        s.add("", "keep me", &[], 0).unwrap();
        let (fresh, moved) = open(d.path(), 2);
        assert!(fresh.list().is_empty());
        let moved = moved.unwrap();
        assert!(moved.exists());
        assert!(!d.path().join(FILE).exists());
    }
}
