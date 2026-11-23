//! The "Ask" command. A thin wrapper over `ask.rs` (the prompts and the checks of what comes
//! back): this file only gathers what can be searched, the notes, the tasks and the clipboard
//! history, into [`Candidate`]s, and runs the question off the UI thread (it is two round
//! trips to the user's own LLM server and may take several seconds).
//!
//! Nothing here changes a note, a task or a clip, and nothing leaves the machine except the
//! question and the few excerpts `ask::retrieve` picks.

use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Manager};

use super::tasks::{system_now_ms, tasks};
use crate::ask::semantic::{self, EmbedCache};
use crate::ask::{self, Answer, Candidate, Options, Source};
use crate::llm::{LlmConfig, UreqHttp};
use crate::model::{ClipItem, ClipKind};
use crate::notes::Note;
use crate::state::AppState;
use crate::store::Store;
use crate::tasks::{Status, Task};

const NOT_SET_UP: &str = "set up an LLM server in Settings first";

fn config(app: &AppHandle) -> Result<LlmConfig, String> {
    LlmConfig::from_settings(
        &app.state::<AppState>()
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner()),
    )
    .ok_or_else(|| NOT_SET_UP.to_owned())
}

/// The vectors of the items seen so far. In memory for the life of the app, so only the first
/// question after a start (or a new model) embeds the history.
fn embed_cache() -> &'static Mutex<EmbedCache> {
    static CACHE: OnceLock<Mutex<EmbedCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(EmbedCache::default()))
}

/// The embedding model, when semantic search is on.
fn embedding_model(app: &AppHandle) -> Option<String> {
    let model = app
        .state::<AppState>()
        .settings
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .llm_embedding_model
        .clone();
    (!model.is_empty()).then_some(model)
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A note as a searchable item: its title, and its body with the tags.
pub fn note_candidate(n: &Note) -> Candidate {
    let mut text = n.body.clone();
    if !n.tags.is_empty() {
        text.push_str("\nTags: ");
        text.push_str(&n.tags.join(", "));
    }
    Candidate::new(
        Source::Note,
        n.id,
        n.created_at,
        n.updated_at,
        &n.title,
        &text,
    )
}

/// A task as a searchable item: where it stands, where it was left off, its next step.
pub fn task_candidate(t: &Task) -> Candidate {
    let mut text = String::from(match t.status {
        Status::Todo => "Status: waiting",
        Status::Doing => "Status: in flight",
        Status::Done => "Status: done",
    });
    if t.important {
        text.push_str(", important");
    }
    if !t.note.is_empty() {
        text.push_str("\nLeft off: ");
        text.push_str(&t.note);
    }
    if !t.next_step.is_empty() {
        text.push_str("\nNext step: ");
        text.push_str(&t.next_step);
    }
    // A done task was last meaningfully touched when it was finished.
    let touched = t.done_at.unwrap_or(t.touched_at).max(t.touched_at);
    Candidate::new(Source::Task, t.id, t.created_at, touched, &t.title, &text)
}

/// A clipboard item as a searchable item: its text, an image's OCR text, or a file list's names;
/// the program it was copied from is the title. An image without readable text, or an empty
/// file list, has nothing to find and is left out.
pub fn clip_candidate(item: &ClipItem) -> Option<Candidate> {
    let title = item.source_app.as_deref().unwrap_or("");
    let (text, lower) = match item.kind {
        ClipKind::Text | ClipKind::Image => {
            if item.search_text.trim().is_empty() {
                return None;
            }
            (item.search_text.clone(), item.search_lower.clone())
        }
        ClipKind::Files => {
            let names = item
                .files
                .as_deref()?
                .iter()
                .map(|f| file_name(&f.path))
                .collect::<Vec<_>>()
                .join(", ");
            if names.is_empty() {
                return None;
            }
            let lower = names.to_lowercase();
            (names.into(), lower.into())
        }
    };
    Some(Candidate::with_lower(
        Source::Clip,
        item.id,
        item.created_at,
        item.last_used_at,
        title,
        text,
        lower,
    ))
}

/// Everything searchable right now.
pub fn candidates(notes: &[Note], tasks: &[Task], history: &Store) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    out.extend(notes.iter().map(note_candidate));
    out.extend(tasks.iter().map(task_candidate));
    out.extend(
        history
            .read()
            .iter_newest_first()
            .filter_map(|item| clip_candidate(item)),
    );
    out
}

/// Answer `question` from the notes, the tasks and the clipboard history.
#[tauri::command(async)]
pub fn ask_everything(app: AppHandle, question: String) -> Result<Answer, String> {
    let cfg = config(&app)?;
    let history = super::store(&app)?;
    let notes = app
        .state::<AppState>()
        .notes
        .get()
        .map(|n| n.list())
        .unwrap_or_default();
    // The task list is optional too: a question can still be answered from the rest.
    let task_list = tasks(&app).map(|t| t.list()).unwrap_or_default();
    let all = candidates(&notes, &task_list, &history);
    // Ranking by meaning is a bonus: a server that cannot embed only means words alone.
    let semantic = embedding_model(&app)
        .and_then(|model| {
            let mut cache = embed_cache().lock().unwrap_or_else(|e| e.into_inner());
            semantic::rank(&cfg, &model, &UreqHttp, &mut cache, &question, &all).ok()
        })
        .unwrap_or_default();
    ask::ask_with(
        &cfg,
        &UreqHttp,
        &question,
        system_now_ms(),
        &all,
        &Options { semantic },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::{Fx, fake_files};

    fn note() -> Note {
        Note {
            id: 3,
            title: "1:1".into(),
            body: "Priya wants the report".into(),
            pinned: false,
            tags: vec!["work".into(), "boss".into()],
            created_at: 10,
            updated_at: 20,
        }
    }

    fn task(status: Status) -> Task {
        Task {
            id: 4,
            title: "Send report".into(),
            note: "waiting for numbers".into(),
            status,
            important: true,
            created_at: 100,
            touched_at: 200,
            done_at: (status == Status::Done).then_some(500),
            size: None,
            kind: None,
            next_step: "Ask finance".into(),
            clip_ids: Vec::new(),
            deferred_until: None,
        }
    }

    #[test]
    fn a_note_is_searchable_by_title_body_and_tags() {
        let c = note_candidate(&note());
        assert_eq!(
            (c.source, c.id, c.created_at, c.touched_at),
            (Source::Note, 3, 10, 20)
        );
        assert_eq!(c.title, "1:1");
        assert_eq!(&*c.text, "Priya wants the report\nTags: work, boss");
        assert!(c.lower.contains("tags: work, boss"));
    }

    #[test]
    fn a_task_carries_its_state_and_a_done_task_is_dated_by_its_finish() {
        let c = task_candidate(&task(Status::Doing));
        assert_eq!(
            &*c.text,
            "Status: in flight, important\nLeft off: waiting for numbers\nNext step: Ask finance"
        );
        assert_eq!((c.created_at, c.touched_at), (100, 200));
        assert_eq!(task_candidate(&task(Status::Done)).touched_at, 500);
        assert!(
            task_candidate(&task(Status::Todo))
                .text
                .starts_with("Status: waiting")
        );
    }

    #[test]
    fn the_history_becomes_candidates_without_empty_ones() {
        let fx = Fx::new();
        fx.text("Wi-Fi password is hunter2");
        fx.image(1, 100); // OCR pending: no text, nothing to find
        fx.store.insert(fake_files(1, fx.now()));
        fx.flush();
        let all = candidates(&[note()], &[task(Status::Todo)], &fx.store);
        let clips: Vec<_> = all.iter().filter(|c| c.source == Source::Clip).collect();
        assert_eq!(all.len(), 2 + clips.len());
        assert!(
            clips
                .iter()
                .any(|c| c.text.contains("hunter2") && c.lower.contains("wi-fi"))
        );
        assert!(
            clips
                .iter()
                .all(|c| !c.text.contains('/') && !c.text.contains('\\')),
            "a file list is sent as names, never as paths"
        );
    }
}
