//! Commands of the task assistant. Thin wrappers over `taskai.rs` (the prompts and the
//! checks of what comes back); this file only gathers what a prompt needs (the task list,
//! recent clipboard items) and stores what is worth keeping (an estimate, a suggested step).
//!
//! Each command is one round trip to the user's own LLM server and may take a few seconds,
//! so they all run off the UI thread (`#[tauri::command(async)]`). Nothing here applies a
//! suggestion to the person's tasks except the two annotations that never count as touching
//! a task (the estimate and the suggested next step); the rest comes back for the person to accept.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::tasks::{system_now_ms, tasks};
use crate::llm::{HttpPost, LlmConfig, UreqHttp};
use crate::model::{ClipItem, ClipKind};
use crate::state::AppState;
use crate::store::Store;
use crate::taskai::{self, ClipLine, DigestKind, Energy};
use crate::tasks::{Status, Task, TaskPatch, TaskStore};

const NOT_SET_UP: &str = "set up an LLM server in Settings first";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Clips older than this are never offered to the model for a resume draft or for links.
const CLIP_WINDOW_MS: i64 = 7 * DAY_MS;
/// Clips are read from the history at most this many per request.
const CLIPS_PER_REQUEST: usize = 40;
/// Open tasks estimated by one "estimate the backlog" request, newest first.
pub const BACKLOG_ESTIMATES: usize = 30;
/// Longest source text echoed back to the webview after an extract.
const SOURCE_ECHO_CHARS: usize = 2_000;

fn config(app: &AppHandle) -> Result<LlmConfig, String> {
    LlmConfig::from_settings(
        &app.state::<AppState>()
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner()),
    )
    .ok_or_else(|| NOT_SET_UP.to_owned())
}

// ---------------------------------------------------------------------------
// What goes to the webview
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickDto {
    pub id: u64,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextActionDto {
    pub step: String,
    pub subtasks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestDto {
    pub summary: String,
    pub order: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkDto {
    pub task_id: u64,
    pub clip_id: u64,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedDto {
    pub title: String,
    pub important: bool,
    pub duplicate_of: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractDto {
    /// The start of the text the tasks were taken from, so the person sees what was read.
    pub source: String,
    pub tasks: Vec<ExtractedDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerdictDto {
    pub id: u64,
    /// `"do"`, `"defer"` or `"drop"`.
    pub verdict: &'static str,
    pub reason: String,
}

/// A linked clipboard item, enough to list it and copy it back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipRefDto {
    pub id: u64,
    pub kind: ClipKind,
    pub preview: String,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------
// Reading the history for a prompt
// ---------------------------------------------------------------------------

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// One history item as a line of text for the model: the text itself, an image's OCR text,
/// or a file list's names.
fn clip_line_of(item: &ClipItem) -> ClipLine {
    let (kind, text) = match item.kind {
        ClipKind::Text => ("text", item.search_text.to_string()),
        ClipKind::Image => (
            "image",
            if item.search_text.is_empty() {
                "(a picture with no readable text)".to_owned()
            } else {
                item.search_text.to_string()
            },
        ),
        ClipKind::Files => (
            "files",
            item.files
                .as_deref()
                .map(|files| {
                    files
                        .iter()
                        .map(|f| file_name(&f.path))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
        ),
    };
    ClipLine {
        id: item.id,
        at: item.last_used_at,
        kind,
        text,
        app: item.source_app.as_deref().map(str::to_owned),
    }
}

/// The clips used (copied again, or for the first time) at or after `since`, newest first, at
/// most [`CLIPS_PER_REQUEST`] and none older than [`CLIP_WINDOW_MS`].
pub fn recent_clip_lines(store: &Store, since: i64, now: i64) -> Vec<ClipLine> {
    let since = since.max(now - CLIP_WINDOW_MS);
    store
        .read()
        .iter_newest_first()
        .take_while(|item| item.last_used_at >= since)
        .take(CLIPS_PER_REQUEST)
        .map(|item| clip_line_of(item))
        .collect()
}

/// The history items among `ids`, in the order asked for. An id that left the history is skipped.
pub fn clip_refs(store: &Store, ids: &[u64]) -> Vec<ClipRefDto> {
    let items = store.read();
    ids.iter()
        .filter_map(|id| items.get(*id))
        .map(|item| ClipRefDto {
            id: item.id,
            kind: item.kind,
            preview: item.preview.to_string(),
            created_at: item.created_at,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The commands' logic
// ---------------------------------------------------------------------------

fn task_or_err(store: &TaskStore, id: u64) -> Result<Task, String> {
    store.get(id).ok_or_else(|| "task not found".to_owned())
}

/// Estimate task `id` and keep the answer on it. The estimate does not count as touching the task.
pub fn estimate_into(
    store: &TaskStore,
    cfg: &LlmConfig,
    http: &dyn HttpPost,
    id: u64,
    now: i64,
) -> Result<Task, String> {
    let task = task_or_err(store, id)?;
    let e = taskai::estimate(cfg, http, &task)?;
    store
        .update(
            id,
            TaskPatch {
                size: Some(Some(e.size)),
                kind: Some(Some(e.kind)),
                ..Default::default()
            },
            now,
        )
        .map_err(|e| e.to_string())
}

/// Open tasks with no estimate yet, newest first, at most [`BACKLOG_ESTIMATES`].
pub fn tasks_without_estimate(store: &TaskStore) -> Vec<u64> {
    let mut list: Vec<Task> = store
        .list()
        .into_iter()
        .filter(|t| t.status != Status::Done && t.size.is_none())
        .collect();
    list.sort_by_key(|t| std::cmp::Reverse((t.created_at, t.id)));
    list.into_iter()
        .take(BACKLOG_ESTIMATES)
        .map(|t| t.id)
        .collect()
}

pub fn pick(
    store: &TaskStore,
    cfg: &LlmConfig,
    http: &dyn HttpPost,
    now: i64,
    minutes: Option<u32>,
    energy: Energy,
) -> Result<Option<PickDto>, String> {
    Ok(
        taskai::pick_next(cfg, http, &store.list(), now, minutes, energy)?.map(|p| PickDto {
            id: p.id,
            reason: p.reason,
        }),
    )
}

/// Suggest a next step for task `id`. The step is kept on the task (it shows under the title);
/// the subtasks are only returned, the person decides whether they become tasks.
pub fn next_step(
    store: &TaskStore,
    cfg: &LlmConfig,
    http: &dyn HttpPost,
    id: u64,
    now: i64,
) -> Result<NextActionDto, String> {
    let task = task_or_err(store, id)?;
    let a = taskai::next_action(cfg, http, &task)?;
    store
        .update(
            id,
            TaskPatch {
                next_step: Some(a.step.clone()),
                ..Default::default()
            },
            now,
        )
        .map_err(|e| e.to_string())?;
    Ok(NextActionDto {
        step: a.step,
        subtasks: a.subtasks,
    })
}

/// A draft of "where I left off" for task `id` from what was copied since it was last touched.
pub fn resume_draft(
    store: &TaskStore,
    history: &Store,
    cfg: &LlmConfig,
    http: &dyn HttpPost,
    id: u64,
    now: i64,
) -> Result<String, String> {
    let task = task_or_err(store, id)?;
    // A little before the last touch: the clip that started the work is often copied first.
    let clips = recent_clip_lines(history, task.touched_at - 30 * 60_000, now);
    taskai::resume_note(cfg, http, &task, &clips, now)
}

pub fn digest(
    store: &TaskStore,
    cfg: &LlmConfig,
    http: &dyn HttpPost,
    kind: DigestKind,
    now: i64,
) -> Result<DigestDto, String> {
    let d = taskai::digest(cfg, http, &store.list(), kind, now)?;
    Ok(DigestDto {
        summary: d.summary,
        order: d.order,
    })
}

/// Which of the last few days' clips belong to which open task.
pub fn links(
    store: &TaskStore,
    history: &Store,
    cfg: &LlmConfig,
    http: &dyn HttpPost,
    now: i64,
) -> Result<Vec<LinkDto>, String> {
    let clips = recent_clip_lines(history, now - 3 * DAY_MS, now);
    Ok(
        taskai::suggest_links(cfg, http, &store.list(), &clips, now)?
            .into_iter()
            .map(|l| LinkDto {
                task_id: l.task_id,
                clip_id: l.clip_id,
                reason: l.reason,
            })
            .collect(),
    )
}

/// The newest text item's full text, for "use what I just copied".
fn latest_text(history: &Store) -> Result<String, String> {
    let id = history
        .read()
        .iter_newest_first()
        .find(|item| item.kind == ClipKind::Text)
        .map(|item| item.id)
        .ok_or_else(|| "there is no copied text to read".to_owned())?;
    let bytes = history
        .get_full_text(id)
        .map_err(|_| "the copied text could not be read".to_owned())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Action items in `text`, or, with no text, in the newest copied text.
pub fn extract(
    store: &TaskStore,
    history: &Store,
    cfg: &LlmConfig,
    http: &dyn HttpPost,
    text: Option<String>,
) -> Result<ExtractDto, String> {
    let text = match text {
        Some(t) => t,
        None => latest_text(history)?,
    };
    let found = taskai::extract_tasks(cfg, http, &text, &store.list())?;
    Ok(ExtractDto {
        source: crate::llm::clip_to(text.trim(), SOURCE_ECHO_CHARS).to_owned(),
        tasks: found
            .into_iter()
            .map(|e| ExtractedDto {
                title: e.title,
                important: e.important,
                duplicate_of: e.duplicate_of,
            })
            .collect(),
    })
}

/// A verdict for each open task in `ids` (a done or unknown id is ignored).
pub fn review(
    store: &TaskStore,
    cfg: &LlmConfig,
    http: &dyn HttpPost,
    ids: &[u64],
    now: i64,
) -> Result<Vec<VerdictDto>, String> {
    let quiet: Vec<Task> = ids
        .iter()
        .filter_map(|id| store.get(*id))
        .filter(|t| t.status != Status::Done)
        .collect();
    let refs: Vec<&Task> = quiet.iter().collect();
    Ok(taskai::review_stale(cfg, http, &refs, now)?
        .into_iter()
        .map(|v| VerdictDto {
            id: v.id,
            verdict: match v.verdict {
                taskai::Verdict::Do => "do",
                taskai::Verdict::Defer => "defer",
                taskai::Verdict::Drop => "drop",
            },
            reason: v.reason,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Background estimates
// ---------------------------------------------------------------------------

/// Ask the background worker to estimate task `id` (when an LLM server is set up). Never
/// blocks and never fails: no server, or no worker, means no estimate, which is fine.
pub fn queue_estimate(app: &AppHandle, id: u64) {
    let state = app.state::<AppState>();
    if config(app).is_err() {
        return;
    }
    if let Some(tx) = state.task_estimates.get() {
        let _ = tx.send(id);
    }
}

/// Start the worker that estimates new tasks one at a time, so adding a task never waits for
/// the server and a burst of tasks never hammers it. Each finished estimate emits
/// `tasks:updated`. A failure is dropped: the task simply stays without an estimate until
/// someone asks again.
pub fn start_estimator(app: &AppHandle) {
    use tauri::Emitter;
    let (tx, rx) = std::sync::mpsc::channel::<u64>();
    if app.state::<AppState>().task_estimates.set(tx).is_err() {
        return;
    }
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("clipdump-task-estimates".to_owned())
        .spawn(move || {
            while let Ok(id) = rx.recv() {
                let Ok(cfg) = config(&app) else { continue };
                let Ok(store) = tasks(&app) else { continue };
                // An estimate already there (set by hand, or by an earlier run) is kept.
                if store.get(id).is_none_or(|t| t.size.is_some()) {
                    continue;
                }
                if estimate_into(&store, &cfg, &UreqHttp, id, system_now_ms()).is_ok() {
                    let _ = app.emit_to(crate::window::LABEL, crate::events::TASKS_UPDATED, ());
                }
            }
        });
}

// ---------------------------------------------------------------------------
// The commands
// ---------------------------------------------------------------------------

/// Estimate (or estimate again) task `id` now. Resolves to the task with its size and kind.
#[tauri::command(async)]
pub fn estimate_task(app: AppHandle, id: u64) -> Result<Task, String> {
    estimate_into(
        tasks(&app)?.as_ref(),
        &config(&app)?,
        &UreqHttp,
        id,
        system_now_ms(),
    )
}

/// Queue an estimate for every open task that has none (newest first, 30 at most). Returns how
/// many were queued; `tasks:updated` fires as each one finishes.
#[tauri::command(async)]
pub fn estimate_backlog(app: AppHandle) -> Result<u32, String> {
    config(&app)?;
    let ids = tasks_without_estimate(tasks(&app)?.as_ref());
    for id in &ids {
        queue_estimate(&app, *id);
    }
    Ok(ids.len() as u32)
}

/// "What should I do now?": the task to do next, given the minutes free and the energy.
#[tauri::command(async)]
pub fn pick_next_task(
    app: AppHandle,
    minutes: Option<u32>,
    energy: Energy,
) -> Result<Option<PickDto>, String> {
    pick(
        tasks(&app)?.as_ref(),
        &config(&app)?,
        &UreqHttp,
        system_now_ms(),
        minutes,
        energy,
    )
}

/// A concrete first step for task `id` (kept on the task) and, for a big one, subtasks.
#[tauri::command(async)]
pub fn suggest_next_step(app: AppHandle, id: u64) -> Result<NextActionDto, String> {
    next_step(
        tasks(&app)?.as_ref(),
        &config(&app)?,
        &UreqHttp,
        id,
        system_now_ms(),
    )
}

/// A draft of where task `id` was left off, from what was copied since.
#[tauri::command(async)]
pub fn draft_resume_note(app: AppHandle, id: u64) -> Result<String, String> {
    let history = super::store(&app)?;
    resume_draft(
        tasks(&app)?.as_ref(),
        &history,
        &config(&app)?,
        &UreqHttp,
        id,
        system_now_ms(),
    )
}

/// The morning briefing or the week's recap.
#[tauri::command(async)]
pub fn task_digest(app: AppHandle, kind: DigestKind) -> Result<DigestDto, String> {
    digest(
        tasks(&app)?.as_ref(),
        &config(&app)?,
        &UreqHttp,
        kind,
        system_now_ms(),
    )
}

/// Which recent clipboard items belong to which open task.
#[tauri::command(async)]
pub fn suggest_task_links(app: AppHandle) -> Result<Vec<LinkDto>, String> {
    let history = super::store(&app)?;
    links(
        tasks(&app)?.as_ref(),
        &history,
        &config(&app)?,
        &UreqHttp,
        system_now_ms(),
    )
}

/// Action items in `text`; without `text`, in the newest copied text.
#[tauri::command(async)]
pub fn extract_tasks(app: AppHandle, text: Option<String>) -> Result<ExtractDto, String> {
    let history = super::store(&app)?;
    extract(
        tasks(&app)?.as_ref(),
        &history,
        &config(&app)?,
        &UreqHttp,
        text,
    )
}

/// What to do about each of the quiet tasks in `ids`: do it, defer it or drop it.
#[tauri::command(async)]
pub fn review_stale_tasks(app: AppHandle, ids: Vec<u64>) -> Result<Vec<VerdictDto>, String> {
    review(
        tasks(&app)?.as_ref(),
        &config(&app)?,
        &UreqHttp,
        &ids,
        system_now_ms(),
    )
}

/// The clipboard items linked to a task, for the notes sidebar. No LLM involved.
#[tauri::command(async)]
pub fn get_clips(app: AppHandle, ids: Vec<u64>) -> Result<Vec<ClipRefDto>, String> {
    Ok(clip_refs(super::store(&app)?.as_ref(), &ids))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::*;
    use crate::persist::crypto::Crypto;
    use crate::store::tests::{Fx, fake_files};

    /// A server that answers every request with the same reply and keeps what it was sent.
    struct Fake {
        reply: String,
        seen: RefCell<Vec<Value>>,
    }

    impl Fake {
        fn saying(content: &str) -> Self {
            Self {
                reply: json!({ "choices": [{ "message": { "content": content } }] }).to_string(),
                seen: RefCell::default(),
            }
        }
        fn user(&self) -> String {
            self.seen.borrow()[0]["messages"][1]["content"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    }

    impl HttpPost for Fake {
        fn post_json(&self, _: &str, _: Option<&str>, body: &Value) -> Result<String, String> {
            self.seen.borrow_mut().push(body.clone());
            Ok(self.reply.clone())
        }
    }

    fn cfg() -> LlmConfig {
        LlmConfig {
            base_url: "http://localhost:11434/v1".into(),
            model: "m".into(),
            api_key: String::new(),
        }
    }

    fn store(dir: &std::path::Path) -> TaskStore {
        TaskStore::open(dir, Arc::new(Crypto::from_key(&[7; 32])), 0).0
    }

    #[test]
    fn an_estimate_is_kept_on_the_task_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let t = s.add("email the landlord", false, false, 100).unwrap();
        let http = Fake::saying(r#"{"size":"quick","kind":"quick"}"#);
        let got = estimate_into(&s, &cfg(), &http, t.id, 9_999).unwrap();
        assert_eq!(got.size, Some(crate::tasks::Size::Quick));
        assert_eq!(got.kind, Some(crate::tasks::Kind::Quick));
        assert_eq!(got.touched_at, 100);
        assert_eq!(s.get(t.id).unwrap(), got);
        assert_eq!(
            estimate_into(&s, &cfg(), &http, 99, 1).unwrap_err(),
            "task not found"
        );
    }

    #[test]
    fn the_backlog_is_open_tasks_with_no_estimate_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let a = s.add("a", false, false, 1).unwrap();
        let b = s.add("b", false, true, 2).unwrap();
        let done = s.add("done", false, false, 3).unwrap();
        s.update(
            done.id,
            TaskPatch {
                status: Some(Status::Done),
                ..Default::default()
            },
            4,
        )
        .unwrap();
        let sized = s.add("sized", false, false, 5).unwrap();
        s.update(
            sized.id,
            TaskPatch {
                size: Some(Some(crate::tasks::Size::Long)),
                ..Default::default()
            },
            6,
        )
        .unwrap();
        assert_eq!(tasks_without_estimate(&s), vec![b.id, a.id]);
        for i in 0..40 {
            s.add(&format!("t{i}"), false, false, 10 + i).unwrap();
        }
        assert_eq!(tasks_without_estimate(&s).len(), BACKLOG_ESTIMATES);
    }

    #[test]
    fn the_suggested_step_is_kept_and_the_subtasks_are_only_returned() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let t = s.add("taxes", false, false, 10).unwrap();
        let http = Fake::saying(
            r#"{"step":"Find last year's return","subtasks":["Gather receipts","File"]}"#,
        );
        let got = next_step(&s, &cfg(), &http, t.id, 500).unwrap();
        assert_eq!(got.subtasks, vec!["Gather receipts", "File"]);
        let stored = s.get(t.id).unwrap();
        assert_eq!(stored.next_step, "Find last year's return");
        assert_eq!(
            stored.touched_at, 10,
            "a suggestion is not the person touching the task"
        );
        assert_eq!(
            s.list().len(),
            1,
            "subtasks become tasks only when accepted"
        );
    }

    #[test]
    fn clips_become_lines_for_text_images_and_files() {
        let fx = Fx::new();
        fx.text("hello from the clipboard");
        fx.advance(10);
        fx.image(1, 100);
        fx.advance(10);
        fx.store.insert(fake_files(1, fx.now()));
        let lines = recent_clip_lines(&fx.store, 0, fx.now());
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].kind, "files");
        assert_eq!(lines[0].text, "file0.txt, file1.txt, file2.txt");
        assert_eq!(lines[1].kind, "image");
        assert_eq!(lines[1].text, "(a picture with no readable text)");
        assert_eq!(lines[2].kind, "text");
        assert_eq!(lines[2].app.as_deref(), Some("notepad.exe"));
        assert!(lines[2].text.contains("hello from the clipboard"));
    }

    #[test]
    fn only_clips_since_the_start_and_inside_the_week_are_offered() {
        let fx = Fx::new();
        fx.advance(20 * DAY_MS);
        fx.text("ancient");
        fx.advance(15 * DAY_MS);
        fx.text("last week");
        fx.advance(5 * DAY_MS);
        let cut = fx.now();
        fx.advance(1000);
        fx.text("fresh");
        let texts = |since: i64| {
            recent_clip_lines(&fx.store, since, fx.now())
                .into_iter()
                .map(|l| l.text)
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(cut), vec!["fresh"]);
        assert_eq!(
            texts(0),
            vec!["fresh", "last week"],
            "never older than a week"
        );
    }

    #[test]
    fn a_resume_draft_reads_what_was_copied_since_the_task_was_touched() {
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        fx.advance(DAY_MS);
        fx.text("copied before the task");
        fx.advance(DAY_MS);
        let t = s.add("fix login", false, true, fx.now()).unwrap();
        fx.advance(60_000);
        fx.text("401 Unauthorized at /login");
        let http = Fake::saying(r#"{"note":"Was reproducing the 401."}"#);
        let note = resume_draft(&s, &fx.store, &cfg(), &http, t.id, fx.now()).unwrap();
        assert_eq!(note, "Was reproducing the 401.");
        let user = http.user();
        assert!(user.contains("401 Unauthorized"));
        assert!(!user.contains("copied before the task"));
        // The draft is only returned: the task keeps its (empty) note.
        assert_eq!(s.get(t.id).unwrap().note, "");
    }

    #[test]
    fn a_resume_draft_with_no_new_clips_says_so_without_asking_the_server() {
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let t = s.add("idle", false, true, fx.now() + DAY_MS).unwrap();
        let http = Fake::saying("unused");
        let e =
            resume_draft(&s, &fx.store, &cfg(), &http, t.id, fx.now() + 2 * DAY_MS).unwrap_err();
        assert!(e.contains("nothing was copied"));
        assert!(http.seen.borrow().is_empty());
    }

    #[test]
    fn links_use_recent_clips_and_return_plain_pairs() {
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let t = s.add("renew passport", false, false, 0).unwrap();
        let clip = fx.text("passport office opening hours");
        let http = Fake::saying(&format!(
            r#"{{"links":[{{"task":{},"clip":{},"reason":"hours"}}]}}"#,
            t.id, clip
        ));
        let got = links(&s, &fx.store, &cfg(), &http, fx.now()).unwrap();
        assert_eq!(
            got,
            vec![LinkDto {
                task_id: t.id,
                clip_id: clip,
                reason: "hours".into()
            }]
        );
    }

    #[test]
    fn extract_reads_the_given_text_else_the_newest_copied_text() {
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        s.add("send the signed lease to dana", false, false, 0)
            .unwrap();
        fx.text("Hi, please send the signed lease to Dana and book the venue by Friday.");
        fx.advance(10);
        fx.image(1, 100);
        let http = Fake::saying(
            r#"{"tasks":[{"title":"Send the signed lease to Dana"},{"title":"Book the venue","important":true}]}"#,
        );
        let got = extract(&s, &fx.store, &cfg(), &http, None).unwrap();
        // The newest item is a picture; the newest *text* is read.
        assert!(got.source.starts_with("Hi, please send the signed lease"));
        assert_eq!(got.tasks.len(), 2);
        assert_eq!(got.tasks[0].duplicate_of, Some(1));
        assert!(got.tasks[1].important && got.tasks[1].duplicate_of.is_none());

        let http = Fake::saying(r#"{"tasks":[]}"#);
        let got = extract(
            &s,
            &fx.store,
            &cfg(),
            &http,
            Some("  call the plumber about the leak  ".into()),
        )
        .unwrap();
        assert_eq!(got.source, "call the plumber about the leak");
        assert!(http.user().contains("call the plumber"));
    }

    #[test]
    fn extract_without_any_copied_text_is_an_error_sentence() {
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        fx.image(1, 100);
        let http = Fake::saying("unused");
        let e = extract(&s, &fx.store, &cfg(), &http, None).unwrap_err();
        assert_eq!(e, "there is no copied text to read");
    }

    #[test]
    fn the_review_covers_open_tasks_only_and_names_the_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let a = s.add("old idea", false, false, 0).unwrap();
        let done = s.add("finished", false, false, 0).unwrap();
        s.update(
            done.id,
            TaskPatch {
                status: Some(Status::Done),
                ..Default::default()
            },
            1,
        )
        .unwrap();
        let http = Fake::saying(&format!(
            r#"{{"verdicts":[{{"id":{},"verdict":"drop","reason":"no longer relevant"}},{{"id":{},"verdict":"do","reason":"x"}}]}}"#,
            a.id, done.id
        ));
        let got = review(&s, &cfg(), &http, &[a.id, done.id, 77], 9 * DAY_MS).unwrap();
        assert_eq!(
            got,
            vec![VerdictDto {
                id: a.id,
                verdict: "drop",
                reason: "no longer relevant".into()
            }]
        );
        assert!(!http.user().contains("finished"));
    }

    #[test]
    fn linked_clips_come_back_in_order_and_gone_ones_are_skipped() {
        let fx = Fx::new();
        let a = fx.text("first clip");
        fx.advance(5);
        let b = fx.text("second clip");
        let got = clip_refs(&fx.store, &[b, 4242, a]);
        assert_eq!(got.iter().map(|c| c.id).collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(got[0].preview, "second clip");
        assert_eq!(got[0].kind, ClipKind::Text);
    }

    #[test]
    fn the_wire_format_is_camel_case() {
        let v = serde_json::to_value(LinkDto {
            task_id: 1,
            clip_id: 2,
            reason: "r".into(),
        })
        .unwrap();
        assert_eq!(v, json!({ "taskId": 1, "clipId": 2, "reason": "r" }));
        let v = serde_json::to_value(ExtractedDto {
            title: "t".into(),
            important: false,
            duplicate_of: Some(3),
        })
        .unwrap();
        assert_eq!(v["duplicateOf"], 3);
        let e: Energy = serde_json::from_str("\"low\"").unwrap();
        assert_eq!(e, Energy::Low);
        let k: DigestKind = serde_json::from_str("\"week\"").unwrap();
        assert_eq!(k, DigestKind::Week);
    }
}
