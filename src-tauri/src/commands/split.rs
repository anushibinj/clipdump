//! `split_item` and `copy_text`: the plain functions behind the two commands
//! (the commands themselves are in `commands.rs`).

use crate::hash::hash_text;
use crate::llm::{self, HttpPost, LlmConfig, Part};
use crate::model::{ClipKind, Payload};
use crate::platform::ClipboardWriter;
use crate::platform::self_write::SelfWriteGuard;
use crate::splitter::Status;
use crate::store::Store;
use serde::Serialize;

/// Where a split stands, as the webview sees it (`status` is the tag).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum SplitStatusDto {
    /// Never asked for.
    None,
    Queued,
    Running,
    Done {
        parts: Vec<Part>,
    },
    Error {
        message: String,
    },
}

impl From<Option<Status>> for SplitStatusDto {
    fn from(s: Option<Status>) -> Self {
        match s {
            None => Self::None,
            Some(Status::Queued) => Self::Queued,
            Some(Status::Running) => Self::Running,
            Some(Status::Done(parts)) => Self::Done { parts },
            Some(Status::Failed(message)) => Self::Error { message },
        }
    }
}

/// Check that item `id` can be split (Split is set up, it is text) and return
/// its content hash, the key of its result.
pub fn splittable_hash(store: &Store, cfg: Option<&LlmConfig>, id: u64) -> Result<u128, String> {
    cfg.ok_or("set up an LLM server in Settings first")?;
    let item = store.get(id).ok_or("item not found")?;
    if item.kind != ClipKind::Text {
        return Err("only text items can be split".to_owned());
    }
    Ok(item.hash)
}

/// The splits kept in the encrypted history database (`persist`): JSON in, JSON
/// out, encrypted by the writer thread.
pub struct SplitStore(pub std::sync::Arc<crate::persist::Persist>);

impl crate::splitter::Persistence for SplitStore {
    fn load(&self, hash: u128) -> Option<Vec<Part>> {
        let json = self.0.load_split(&hash.to_be_bytes()).ok().flatten()?;
        serde_json::from_slice(&json).ok()
    }

    fn save(&self, hash: u128, parts: &[Part]) {
        let Ok(json) = serde_json::to_vec(parts) else {
            return;
        };
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let _ = self.0.submit(crate::persist::WriteOp::SetSplit {
            hash: hash.to_be_bytes(),
            json,
            at,
        });
    }
}

/// The most text `copy_text` accepts. A piece is a number or a token, never a book;
/// the cap only stops a runaway caller.
pub const MAX_SNIPPET_BYTES: usize = 1024 * 1024;

/// Split the text item `id` with `cfg`. Errors are plain sentences: Split not
/// set up, an item that is not text, or what `llm::split` reports.
pub fn split_text_item(
    store: &Store,
    cfg: Option<&LlmConfig>,
    http: &dyn HttpPost,
    id: u64,
) -> Result<Vec<Part>, String> {
    let cfg = cfg.ok_or("set up an LLM server in Settings first")?;
    let item = store.get(id).ok_or("item not found")?;
    if item.kind != ClipKind::Text {
        return Err("only text items can be split".to_owned());
    }
    let text = match store
        .payload(id)
        .map_err(|_| "could not load the item".to_owned())?
    {
        Payload::Text(t) => t,
        _ => return Err("only text items can be split".to_owned()),
    };
    llm::split(cfg, &text, http)
}

/// Write `text` to the clipboard with writer `W`, telling the self-write guard
/// first so the watcher does not capture it as a new history item.
pub fn copy_snippet<W: ClipboardWriter>(
    guard: &SelfWriteGuard,
    clock: fn() -> u64,
    text: String,
) -> Result<(), String> {
    if text.is_empty() {
        return Err("nothing to copy".to_owned());
    }
    if text.len() > MAX_SNIPPET_BYTES {
        return Err("that text is too long to copy this way".to_owned());
    }
    let hash = hash_text(&text);
    let note = || guard.note_write_as(ClipKind::Text, hash, hash, clock());
    W::set(&Payload::Text(text), &note).map_err(|e| e.to_string())?;
    note();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::{Value, json};

    use super::*;
    use crate::store::Inserted;
    use crate::store::tests::{Fx, fake_files};

    struct Canned(String, RefCell<usize>);

    impl HttpPost for Canned {
        fn post_json(&self, _: &str, _: Option<&str>, _: &Value) -> Result<String, String> {
            *self.1.borrow_mut() += 1;
            Ok(self.0.clone())
        }
    }

    fn answering(items: Value) -> Canned {
        let content = json!({ "items": items }).to_string();
        Canned(
            json!({ "choices": [{ "message": { "content": content } }] }).to_string(),
            RefCell::new(0),
        )
    }

    fn cfg() -> LlmConfig {
        LlmConfig {
            base_url: "http://h/v1".into(),
            model: "m".into(),
            api_key: String::new(),
        }
    }

    #[test]
    fn a_text_item_is_split_into_exact_pieces() {
        let fx = Fx::new();
        let id = fx.text("mail ann@example.com or call 555-0100");
        let http = answering(json!([
            { "label": "Email", "value": "ann@example.com" },
            { "label": "Phone", "value": "555-0100" },
            { "label": "Made up", "value": "bob@example.com" },
        ]));
        let parts = split_text_item(&fx.store, Some(&cfg()), &http, id).unwrap();
        let values: Vec<_> = parts.iter().map(|p| p.value.as_str()).collect();
        assert_eq!(values, ["ann@example.com", "555-0100"]);
    }

    #[test]
    fn without_a_server_nothing_is_sent() {
        let fx = Fx::new();
        let id = fx.text("hello");
        let http = answering(json!([]));
        let e = split_text_item(&fx.store, None, &http, id).unwrap_err();
        assert!(e.contains("Settings"));
        assert_eq!(*http.1.borrow(), 0);
    }

    #[test]
    fn only_text_items_and_known_ids_can_be_split() {
        let fx = Fx::new();
        let Inserted::New(files) = fx.store.insert(fake_files(1, fx.now())) else {
            panic!("a new item");
        };
        let http = answering(json!([]));
        assert!(
            split_text_item(&fx.store, Some(&cfg()), &http, files)
                .unwrap_err()
                .contains("only text")
        );
        assert!(
            split_text_item(&fx.store, Some(&cfg()), &http, 999_999)
                .unwrap_err()
                .contains("not found")
        );
        assert_eq!(*http.1.borrow(), 0);
    }

    #[test]
    fn the_status_json_is_tagged_by_status() {
        let v = |s: Option<Status>| serde_json::to_value(SplitStatusDto::from(s)).unwrap();
        assert_eq!(v(None), json!({ "status": "none" }));
        assert_eq!(v(Some(Status::Queued)), json!({ "status": "queued" }));
        assert_eq!(v(Some(Status::Running)), json!({ "status": "running" }));
        assert_eq!(
            v(Some(Status::Done(vec![Part {
                label: "A".into(),
                value: "b".into()
            }]))),
            json!({ "status": "done", "parts": [{ "label": "A", "value": "b" }] })
        );
        assert_eq!(
            v(Some(Status::Failed("no".into()))),
            json!({ "status": "error", "message": "no" })
        );
    }

    #[test]
    fn only_text_with_a_server_gets_a_key() {
        let fx = Fx::new();
        let text = fx.text("some text to split");
        let hash = fx.store.get(text).unwrap().hash;
        assert_eq!(splittable_hash(&fx.store, Some(&cfg()), text), Ok(hash));
        assert!(
            splittable_hash(&fx.store, None, text)
                .unwrap_err()
                .contains("Settings")
        );
        assert!(splittable_hash(&fx.store, Some(&cfg()), 999_999).is_err());
        let Inserted::New(files) = fx.store.insert(fake_files(1, fx.now())) else {
            panic!("a new item");
        };
        assert!(
            splittable_hash(&fx.store, Some(&cfg()), files)
                .unwrap_err()
                .contains("only text")
        );
    }

    struct Recording;

    thread_local! {
        static WROTE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    impl ClipboardWriter for Recording {
        fn set(payload: &Payload, armed: &dyn Fn()) -> anyhow::Result<()> {
            armed();
            if let Payload::Text(t) = payload {
                WROTE.with(|w| w.borrow_mut().push(t.clone()));
            }
            Ok(())
        }
    }

    #[test]
    fn a_piece_is_written_and_its_echo_is_swallowed() {
        let guard = SelfWriteGuard::default();
        copy_snippet::<Recording>(&guard, || 1_000, "Tr0ub4dor&3".to_owned()).unwrap();
        WROTE.with(|w| assert_eq!(w.borrow().last().unwrap(), "Tr0ub4dor&3"));
        assert!(
            guard.text_echo(1_100, hash_text("Tr0ub4dor&3")).is_some(),
            "the watcher must ignore the update our own write causes"
        );
    }

    #[test]
    fn empty_and_huge_pieces_are_refused() {
        let guard = SelfWriteGuard::default();
        assert!(copy_snippet::<Recording>(&guard, || 0, String::new()).is_err());
        assert!(
            copy_snippet::<Recording>(&guard, || 0, "x".repeat(MAX_SNIPPET_BYTES + 1)).is_err()
        );
    }
}
