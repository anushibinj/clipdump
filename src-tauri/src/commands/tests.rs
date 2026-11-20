//! Tests of the command logic. They call the plain functions with a real store
//! over a temp directory (no Tauri runtime).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use super::*;
use crate::commands::preview::{
    PREVIEW_FILES_MAX, PREVIEW_TEXT_MAX_BYTES, file_name, truncate_utf8,
};
use crate::ingest::{OcrJob, OcrQueue};
use crate::model::Payload;
use crate::model::{ClipKind, OcrStatus};
use crate::ocr::{OCR_UNAVAILABLE_HINT, OcrQueueHandle};
use crate::platform::self_write::SelfWriteGuard;
use crate::platform::{ClipboardWriter, Paster};
use crate::search::PathChecker;
use crate::store::tests::{Fx, fake_files};

/// Existence answers from a fixed list of missing paths.
struct Missing(Vec<&'static str>);

impl PathChecker for Missing {
    fn exists(&self, path: &str) -> bool {
        !self.0.contains(&path)
    }
}

fn all_exist() -> Missing {
    Missing(Vec::new())
}

// -- search ------------------------------------------------------------------

#[test]
fn search_maps_query_filter_and_limit() {
    let fx = Fx::new();
    fx.text("alpha note");
    fx.advance(10);
    fx.text("beta note");
    fx.advance(10);
    fx.image(1, 100);
    let paths = all_exist();

    let all = search_items(&fx.store, &paths, "", TypeFilter::All, None);
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].kind, ClipKind::Image, "newest first");

    let texts = search_items(&fx.store, &paths, "", TypeFilter::Text, None);
    assert_eq!(texts.len(), 2);
    assert!(texts.iter().all(|r| r.kind == ClipKind::Text));

    let hit = search_items(&fx.store, &paths, "alpha", TypeFilter::All, None);
    assert_eq!(hit.len(), 1);
    assert!(!hit[0].match_indices.is_empty());

    // A typed prefix beats the UI filter.
    let by_prefix = search_items(&fx.store, &paths, "img:", TypeFilter::Text, None);
    assert_eq!(by_prefix.len(), 1);
    assert_eq!(by_prefix[0].kind, ClipKind::Image);

    assert_eq!(
        search_items(&fx.store, &paths, "", TypeFilter::All, Some(2)).len(),
        2
    );
    assert!(search_items(&fx.store, &paths, "", TypeFilter::All, Some(0)).is_empty());
}

#[test]
fn search_reports_missing_files_through_the_checker() {
    let fx = Fx::new();
    fx.store.insert(fake_files(1, fx.now()));
    let gone = Missing(vec!["C:\\docs\\project1\\file1.txt"]);
    let rows = search_items(&fx.store, &gone, "", TypeFilter::Files, None);
    assert_eq!(rows[0].file_count, 3);
    assert_eq!(rows[0].missing_files, 1);
}

#[test]
fn the_result_json_uses_camel_case_and_lowercase_kinds() {
    let fx = Fx::new();
    fx.text("hello");
    let rows = search_items(&fx.store, &all_exist(), "", TypeFilter::All, None);
    let v = serde_json::to_value(&rows).unwrap();
    assert_eq!(v[0]["kind"], "text");
    assert!(v[0].get("matchIndices").is_some());
    assert!(v[0].get("hasThumb").is_some());
}

#[test]
fn the_filter_arrives_as_a_lowercase_word() {
    let f: TypeFilter = serde_json::from_str("\"images\"").unwrap();
    assert_eq!(f, TypeFilter::Images);
}

// -- preview -----------------------------------------------------------------

#[test]
fn a_short_text_previews_whole() {
    let fx = Fx::new();
    let id = fx.text("hello\nworld");
    match preview_item(&fx.store, &all_exist(), id).unwrap() {
        PreviewDto::Text {
            text,
            truncated,
            total_chars,
            ..
        } => {
            assert_eq!(text, "hello\nworld");
            assert!(!truncated);
            assert_eq!(total_chars, 11);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_long_text_is_cut_on_a_char_boundary_and_counted() {
    let fx = Fx::new();
    // 3 bytes per char: 204,800 is not a multiple of 3, so the cut must back up.
    let chars = 100_000;
    let id = fx.text(&"€".repeat(chars));
    fx.flush(); // 300 KB is over the 64 KB RAM limit: it is read back from disk
    match preview_item(&fx.store, &all_exist(), id).unwrap() {
        PreviewDto::Text {
            text,
            truncated,
            total_chars,
            byte_size,
            ..
        } => {
            assert!(truncated);
            assert_eq!(total_chars, chars);
            assert_eq!(byte_size, 3 * chars as u64);
            assert!(text.len() <= PREVIEW_TEXT_MAX_BYTES);
            assert_eq!(text.len(), PREVIEW_TEXT_MAX_BYTES - 2);
            assert!(text.chars().all(|c| c == '€'));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_huge_text_loads_from_disk_and_stays_capped() {
    let fx = Fx::new();
    // Over the 64 KB RAM limit and about 4 MB: comes back through the cache.
    let big = "lorem ipsum dolor sit amet\n".repeat(150_000);
    let id = fx.text(&big);
    fx.flush();
    let dto = preview_item(&fx.store, &all_exist(), id).unwrap();
    let PreviewDto::Text {
        text,
        truncated,
        total_chars,
        ..
    } = &dto
    else {
        panic!("{dto:?}")
    };
    assert!(*truncated);
    assert_eq!(*total_chars, big.chars().count());
    assert_eq!(text.len(), PREVIEW_TEXT_MAX_BYTES);
    // And the JSON the webview gets is about the cap (newlines double when
    // escaped), not megabytes.
    assert!(serde_json::to_string(&dto).unwrap().len() < 2 * PREVIEW_TEXT_MAX_BYTES + 1024);
}

#[test]
fn an_image_previews_dimensions_and_ocr_text() {
    let fx = Fx::new();
    let id = fx.image(1, 5000);
    // No OCR yet.
    match preview_item(&fx.store, &all_exist(), id).unwrap() {
        PreviewDto::Image {
            width,
            height,
            byte_size,
            ocr_text,
            ocr_truncated,
            ocr_status,
            ..
        } => {
            assert_eq!((width, height, byte_size), (640, 480, 5000));
            assert_eq!(ocr_text, "");
            assert!(!ocr_truncated);
            assert_eq!(ocr_status, OcrStatus::Pending);
        }
        other => panic!("{other:?}"),
    }
    // OCR done.
    fx.store
        .set_ocr_text(id, Some("invoice 42\ntotal".into()), OcrStatus::Done);
    match preview_item(&fx.store, &all_exist(), id).unwrap() {
        PreviewDto::Image {
            ocr_text,
            ocr_status,
            ..
        } => {
            assert_eq!(ocr_text, "invoice 42\ntotal");
            assert_eq!(ocr_status, OcrStatus::Done);
        }
        other => panic!("{other:?}"),
    }
    let v = serde_json::to_value(preview_item(&fx.store, &all_exist(), id).unwrap()).unwrap();
    assert_eq!(v["kind"], "image");
    assert_eq!(v["ocrStatus"], "done");
    assert!(v.get("ocrTruncated").is_some());
}

#[test]
fn files_preview_lists_sizes_names_and_missing_paths() {
    let fx = Fx::new();
    let id = match fx.store.insert(fake_files(1, fx.now())) {
        crate::store::Inserted::New(id) | crate::store::Inserted::Bumped(id) => id,
    };
    let checker = Missing(vec!["C:\\docs\\project1\\file2.txt"]);
    match preview_item(&fx.store, &checker, id).unwrap() {
        PreviewDto::Files {
            files,
            total_files,
            truncated,
            total_size,
            ..
        } => {
            assert_eq!(total_files, 3);
            assert!(!truncated);
            assert_eq!(total_size, 100 + 101 + 102);
            assert_eq!(files[0].name, "file0.txt");
            assert_eq!(files[0].size, 100);
            assert!(files[0].exists && files[1].exists);
            assert!(!files[2].exists);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_long_file_list_is_capped() {
    let fx = Fx::new();
    let mut item = fake_files(9, fx.now());
    let files: Vec<_> = (0..PREVIEW_FILES_MAX + 25)
        .map(|i| crate::model::FileRef {
            path: format!("/data/f{i}.bin"),
            size: 1,
            is_dir: false,
        })
        .collect();
    item.byte_size = files.len() as u64;
    item.files = Some(files);
    let id = match fx.store.insert(item) {
        crate::store::Inserted::New(id) | crate::store::Inserted::Bumped(id) => id,
    };
    match preview_item(&fx.store, &all_exist(), id).unwrap() {
        PreviewDto::Files {
            files,
            total_files,
            truncated,
            total_size,
            ..
        } => {
            assert_eq!(files.len(), PREVIEW_FILES_MAX);
            assert_eq!(total_files, PREVIEW_FILES_MAX + 25);
            assert!(truncated);
            assert_eq!(total_size, (PREVIEW_FILES_MAX + 25) as u64);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unknown_id_is_a_plain_error() {
    let fx = Fx::new();
    assert_eq!(
        preview_item(&fx.store, &all_exist(), 999).unwrap_err(),
        "item not found"
    );
}

#[test]
fn truncate_utf8_and_file_name_edges() {
    assert_eq!(truncate_utf8("abc", 3), ("abc", false));
    assert_eq!(truncate_utf8("abcd", 3), ("abc", true));
    assert_eq!(
        truncate_utf8("a€", 2),
        ("a", true),
        "does not split the euro sign"
    );
    assert_eq!(truncate_utf8("€", 0), ("", true));
    assert_eq!(file_name("C:\\a\\b.txt"), "b.txt");
    assert_eq!(file_name("/a/b/"), "b");
    assert_eq!(file_name("C:\\"), "C:");
    assert_eq!(file_name("plain"), "plain");
    assert_eq!(file_name("/"), "/");
}

// -- copy_item ---------------------------------------------------------------

thread_local! {
    static WRITTEN: RefCell<Vec<Payload>> = const { RefCell::new(Vec::new()) };
    static PASTED: RefCell<Vec<Option<isize>>> = const { RefCell::new(Vec::new()) };
    static FAIL_WRITE: Cell<bool> = const { Cell::new(false) };
    static FAIL_PASTE: Cell<bool> = const { Cell::new(false) };
    /// Runs inside `FakeWriter::set` right after it armed the guard: where the
    /// real echo could arrive before `set` returns.
    static DURING_WRITE: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    /// Fake monotonic clock: every reading is 100 ms after the previous one.
    static CLOCK: Cell<u64> = const { Cell::new(900) };
}

fn tick() -> u64 {
    CLOCK.with(|c| {
        c.set(c.get() + 100);
        c.get()
    })
}

/// Each test runs on its own thread, so the thread locals are per test.
struct FakeWriter;
struct FakePaster;

impl ClipboardWriter for FakeWriter {
    fn set(payload: &Payload, armed: &dyn Fn()) -> anyhow::Result<()> {
        if FAIL_WRITE.get() {
            anyhow::bail!("clipboard is locked");
        }
        armed();
        DURING_WRITE.with(|d| {
            if let Some(f) = &*d.borrow() {
                f();
            }
        });
        WRITTEN.with(|w| w.borrow_mut().push(payload.clone()));
        Ok(())
    }
}

impl Paster for FakePaster {
    fn paste_into(target: Option<isize>) -> anyhow::Result<()> {
        PASTED.with(|p| p.borrow_mut().push(target));
        if FAIL_PASTE.get() {
            anyhow::bail!("no window");
        }
        Ok(())
    }
}

struct CopyFx {
    guard: Arc<SelfWriteGuard>,
    prev: AtomicIsize,
    own: Option<isize>,
}

impl CopyFx {
    fn new(prev: isize) -> Self {
        Self {
            guard: Arc::new(SelfWriteGuard::default()),
            prev: AtomicIsize::new(prev),
            own: None,
        }
    }

    fn copy(
        &self,
        fx: &Fx,
        paths: &dyn PathChecker,
        id: u64,
        paste: bool,
    ) -> (Result<(), String>, bool) {
        let env = CopyEnv {
            store: &fx.store,
            paths,
            self_write: &self.guard,
            prev_foreground: &self.prev,
            own_window: self.own,
            clock: tick,
        };
        let hidden = Cell::new(false);
        let r = copy_item_with::<FakeWriter, FakePaster>(&env, id, paste, || hidden.set(true));
        (r, hidden.get())
    }
}

fn written() -> Vec<Payload> {
    WRITTEN.with(|w| w.borrow().clone())
}

fn pasted() -> Vec<Option<isize>> {
    PASTED.with(|p| p.borrow().clone())
}

#[test]
fn copying_text_writes_marks_used_hides_and_notes_the_write() {
    let fx = Fx::new();
    let a = fx.text("first");
    fx.advance(10);
    let b = fx.text("second");
    assert_eq!(fx.ids(), vec![b, a]);
    let cfx = CopyFx::new(77);

    let (r, hidden) = cfx.copy(&fx, &all_exist(), a, false);
    assert_eq!(r, Ok(()));
    assert!(hidden);
    assert_eq!(written(), vec![Payload::Text("first".into())]);
    assert_eq!(fx.ids(), vec![a, b], "the copied item moved to the top");
    assert_eq!(fx.store.get(a).unwrap().use_count, 1);
    // The guard knows the hash, so the watcher treats the echo as a bump.
    // Stamped twice (before and after the write): 1,000 and 1,100.
    let hash = fx.store.get(a).unwrap().hash;
    assert_eq!(cfx.guard.text_echo(1_200, hash), Some(hash));
    assert_eq!(
        cfx.guard.text_echo(1_600, hash),
        None,
        "500 ms from the second stamp"
    );
    // No paste was asked for: the saved window is kept.
    assert!(pasted().is_empty());
    assert_eq!(cfx.prev.load(Ordering::Relaxed), 77);
}

#[test]
fn copying_an_image_builds_a_png_payload_and_arms_the_image_guard() {
    let fx = Fx::new();
    let id = fx.image(3, 2048);
    fx.flush();
    let cfx = CopyFx::new(0);
    let (r, _) = cfx.copy(&fx, &all_exist(), id, false);
    assert_eq!(r, Ok(()));
    match &written()[0] {
        Payload::Image { png, width, height } => {
            assert_eq!(png.len(), 2048);
            assert_eq!((*width, *height), (640, 480));
        }
        other => panic!("{other:?}"),
    }
    assert!(cfx.guard.image_echo(1_050).is_some());
}

#[test]
fn the_guard_is_armed_before_the_write_returns_and_refreshed_after() {
    let fx = Fx::new();
    let id = fx.image(4, 100);
    fx.flush();
    let cfx = CopyFx::new(0);
    // An echo that arrives while `set` is still running must already be known.
    let during = Rc::new(Cell::new(None));
    let (g, d) = (Arc::clone(&cfx.guard), Rc::clone(&during));
    DURING_WRITE.set(Some(Box::new(move || d.set(g.image_echo(1_001)))));
    let hash = fx.store.get(id).unwrap().hash;
    assert_eq!(cfx.copy(&fx, &all_exist(), id, false).0, Ok(()));
    assert_eq!(
        during.get(),
        Some(hash),
        "armed at 1,000 before the clipboard opens"
    );
    // Refreshed when the write was done (the clock read 1,100): the 150 ms
    // window runs from there, not from the start of the write.
    assert_eq!(cfx.guard.image_echo(1_249), Some(hash));
    assert_eq!(cfx.guard.image_echo(1_250), None);
}

#[test]
fn a_failed_write_does_not_arm_the_guard() {
    let fx = Fx::new();
    let id = fx.text("never written");
    FAIL_WRITE.set(true);
    let cfx = CopyFx::new(0);
    assert!(cfx.copy(&fx, &all_exist(), id, false).0.is_err());
    let hash = fx.store.get(id).unwrap().hash;
    assert_eq!(cfx.guard.text_echo(1_000, hash), None);
}

#[test]
fn the_written_text_echo_is_recognised_and_only_moves_the_item() {
    use crate::hash::hash_text;
    use crate::ingest::ItemSink;
    use crate::platform::clipfmt::unicode_text_units;

    let fx = Fx::new();
    let a = fx.text("line one\nline two");
    fx.advance(10);
    let b = fx.text("other");
    let cfx = CopyFx::new(0);
    assert_eq!(cfx.copy(&fx, &all_exist(), a, false).0, Ok(()));

    // What the watcher does with the clipboard update that follows: read the
    // text back (CRLF, as the writer builds it), hash it, ask the guard.
    let units = unicode_text_units("line one\nline two");
    let read_back = String::from_utf16_lossy(&units[..units.len() - 1]);
    assert!(read_back.contains("\r\n"));
    let bump = cfx.guard.text_echo(1_100, hash_text(&read_back));
    assert_eq!(bump, Some(fx.store.get(a).unwrap().hash));
    ItemSink::bump(&*fx.store, bump.unwrap());

    assert_eq!(fx.ids(), vec![a, b], "still the top item, no duplicate");
    assert_eq!(fx.store.len(), 2);
    assert_eq!(fx.store.get(a).unwrap().use_count, 1, "one copy, one use");
}

#[test]
fn the_files_echo_hash_is_what_ingest_would_compute() {
    use crate::hash::hash_path_bufs;
    use crate::ingest::files;

    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b \u{1F600}.txt");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();
    let list = vec![a, b];
    assert_eq!(
        files::build(&list, None, 0).unwrap().hash,
        hash_path_bufs(&list)
    );
}

#[test]
fn a_files_copy_with_missing_paths_echoes_as_the_shorter_list() {
    use crate::hash::hash_path_bufs;
    use crate::ingest::ItemSink;

    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = ["one.txt", "two.txt", "three.txt"]
        .iter()
        .map(|n| dir.path().join(n))
        .collect();
    paths[0..2]
        .iter()
        .for_each(|p| std::fs::write(p, b"x").unwrap());
    let fx = Fx::new();
    let ingested = crate::ingest::files::build(&paths, None, fx.now()).unwrap();
    let item_hash = ingested.hash;
    let id = match fx.store.insert(ingested) {
        crate::store::Inserted::New(id) | crate::store::Inserted::Bumped(id) => id,
    };
    let cfx = CopyFx::new(0);
    // Real existence checks: three.txt was never created.
    assert_eq!(
        cfx.copy(&fx, &crate::search::FsPathChecker, id, false).0,
        Ok(())
    );
    match &written()[0] {
        Payload::Files(list) => assert_eq!(list, &paths[0..2]),
        other => panic!("{other:?}"),
    }
    // The watcher reads back the two files: not the item's hash, but the guard
    // knows, and says which item to bump.
    let echoed = hash_path_bufs(&paths[0..2]);
    assert_ne!(echoed, item_hash);
    assert_eq!(cfx.guard.files_echo(1_100, echoed), Some(item_hash));
    ItemSink::bump(&*fx.store, item_hash);
    assert_eq!(fx.store.len(), 1, "no second item for the shorter list");
}

#[test]
fn copying_files_skips_missing_paths_and_fails_when_all_are_missing() {
    let fx = Fx::new();
    let id = match fx.store.insert(fake_files(1, fx.now())) {
        crate::store::Inserted::New(id) | crate::store::Inserted::Bumped(id) => id,
    };
    let cfx = CopyFx::new(0);
    let one_gone = Missing(vec!["C:\\docs\\project1\\file1.txt"]);
    assert_eq!(cfx.copy(&fx, &one_gone, id, false).0, Ok(()));
    match &written()[0] {
        Payload::Files(list) => assert_eq!(list.len(), 2),
        other => panic!("{other:?}"),
    }

    struct NoneExist;
    impl PathChecker for NoneExist {
        fn exists(&self, _: &str) -> bool {
            false
        }
    }
    let (r, hidden) = cfx.copy(&fx, &NoneExist, id, false);
    assert_eq!(r, Err("none of the files exist any more".to_owned()));
    assert!(!hidden, "the bar stays open on an error");
    assert_eq!(written().len(), 1, "nothing else was written");
}

#[test]
fn a_failed_write_changes_nothing() {
    let fx = Fx::new();
    let a = fx.text("one");
    fx.advance(10);
    let b = fx.text("two");
    FAIL_WRITE.set(true);
    let cfx = CopyFx::new(5);
    let (r, hidden) = cfx.copy(&fx, &all_exist(), a, true);
    assert_eq!(r, Err("clipboard is locked".to_owned()));
    assert!(!hidden);
    assert_eq!(fx.ids(), vec![b, a], "order unchanged");
    assert_eq!(fx.store.get(a).unwrap().use_count, 0);
    assert!(pasted().is_empty(), "no paste after a failed copy");
    assert_eq!(cfx.prev.load(Ordering::Relaxed), 5);
}

#[test]
fn paste_takes_the_saved_window_once() {
    let fx = Fx::new();
    let id = fx.text("paste me");
    let cfx = CopyFx::new(1234);
    assert_eq!(cfx.copy(&fx, &all_exist(), id, true).0, Ok(()));
    assert_eq!(pasted(), vec![Some(1234)]);
    assert_eq!(cfx.prev.load(Ordering::Relaxed), 0, "taken");
    // A second paste has no window to go to: the item is copied, nothing is sent.
    assert_eq!(cfx.copy(&fx, &all_exist(), id, true).0, Ok(()));
    assert_eq!(pasted(), vec![Some(1234)]);
    assert_eq!(written().len(), 2, "but it was copied again");
}

#[test]
fn the_bar_itself_is_never_a_paste_target() {
    let fx = Fx::new();
    let id = fx.text("x");
    let mut cfx = CopyFx::new(555);
    cfx.own = Some(555);
    let (r, hidden) = cfx.copy(&fx, &all_exist(), id, true);
    assert_eq!(r, Ok(()), "plain copy, no error");
    assert!(hidden);
    assert!(pasted().is_empty());
    assert_eq!(written().len(), 1);
}

#[test]
fn no_paste_when_not_asked_for() {
    let fx = Fx::new();
    let id = fx.text("x");
    let cfx = CopyFx::new(42);
    assert_eq!(cfx.copy(&fx, &all_exist(), id, false).0, Ok(()));
    assert!(pasted().is_empty());
}

#[test]
fn a_failed_paste_reports_that_the_copy_worked() {
    let fx = Fx::new();
    let id = fx.text("x");
    FAIL_PASTE.set(true);
    let cfx = CopyFx::new(9);
    let (r, hidden) = cfx.copy(&fx, &all_exist(), id, true);
    assert_eq!(
        r,
        Err("copied, but auto-paste failed: no window".to_owned())
    );
    assert!(hidden);
    assert_eq!(written().len(), 1);
}

#[test]
fn copying_an_unknown_id_is_a_plain_error() {
    let fx = Fx::new();
    let cfx = CopyFx::new(0);
    let (r, hidden) = cfx.copy(&fx, &all_exist(), 404, false);
    assert_eq!(r, Err("item not found".to_owned()));
    assert!(!hidden);
    assert!(written().is_empty());
}

/// Linux and macOS have no clipboard backend: the copy fails with the stub's
/// message and changes nothing (the bar stays open and shows it).
#[cfg(not(windows))]
#[test]
fn without_a_clipboard_backend_copy_fails_and_changes_nothing() {
    let fx = Fx::new();
    let id = fx.text("keep");
    let cfx = CopyFx::new(0);
    let env = CopyEnv {
        store: &fx.store,
        paths: &all_exist(),
        self_write: &cfx.guard,
        prev_foreground: &cfx.prev,
        own_window: None,
        clock: || 0,
    };
    let r = copy_item_with::<crate::platform::Writer, crate::platform::PasterImpl>(
        &env,
        id,
        false,
        || panic!("must not hide"),
    );
    assert_eq!(
        r,
        Err("clipboard writing is not supported on this platform".to_owned())
    );
    assert_eq!(fx.store.get(id).unwrap().use_count, 0);
}

// -- delete, pin, clear ------------------------------------------------------

#[test]
fn delete_removes_one_item_even_when_pinned() {
    let fx = Fx::new();
    let a = fx.text("keep");
    let b = fx.text("pinned but deleted");
    assert_eq!(toggle_pin_of(&fx.store, b), Ok(true));
    assert!(delete_one(&fx.store, b));
    assert_eq!(fx.ids(), vec![a]);
    assert!(!delete_one(&fx.store, b), "already gone is not an error");
    assert_eq!(
        fx.db_ids(),
        vec![a as i64],
        "the row is gone from SQLite too"
    );
}

#[test]
fn delete_removes_the_blob_files_of_an_image() {
    let fx = Fx::new();
    let id = fx.image(1, 1000);
    fx.flush();
    assert!(fx.store.get_blob(id).is_ok());
    let files = |sub: &str| {
        walk(&fx.dir.path().join(sub))
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "bin"))
            .count()
    };
    assert_eq!((files("blobs"), files("thumbs")), (1, 1));
    assert!(delete_one(&fx.store, id));
    fx.flush();
    assert!(fx.store.get_blob(id).is_err());
    assert_eq!(files("blobs"), 0);
    assert_eq!(files("thumbs"), 0);
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn toggle_pin_flips_and_reports_unknown_ids() {
    let fx = Fx::new();
    let a = fx.text("a");
    assert_eq!(toggle_pin_of(&fx.store, a), Ok(true));
    assert!(fx.store.get(a).unwrap().pinned);
    assert_eq!(toggle_pin_of(&fx.store, a), Ok(false));
    assert!(!fx.store.get(a).unwrap().pinned);
    assert_eq!(
        toggle_pin_of(&fx.store, 99),
        Err("item not found".to_owned())
    );
}

#[test]
fn clear_history_keeps_pinned_items() {
    let fx = Fx::new();
    let a = fx.text("a");
    let b = fx.text("b");
    let img = fx.image(1, 500);
    fx.text("c");
    toggle_pin_of(&fx.store, a).unwrap();
    toggle_pin_of(&fx.store, img).unwrap();
    assert_eq!(clear_unpinned(&fx.store), 2);
    let mut left = fx.ids();
    left.sort_unstable();
    assert_eq!(left, vec![a, img]);
    assert!(fx.store.get(b).is_none());
    assert_eq!(fx.db_ids(), vec![a as i64, img as i64]);
    assert_eq!(clear_unpinned(&fx.store), 0, "nothing left to clear");
}

// -- settings and stats ------------------------------------------------------

/// A global hotkey registry: chords other apps own, and the ones we hold.
#[derive(Default)]
struct FakeReg {
    taken: RefCell<std::collections::HashSet<String>>,
    active: RefCell<Vec<String>>,
}

impl crate::hotkey::Registrar for FakeReg {
    fn register(&self, hk: &crate::hotkey::Hotkey) -> Result<(), String> {
        let name = hk.to_string();
        if self.taken.borrow().contains(&name) || self.active.borrow().contains(&name) {
            return Err("already registered".to_owned());
        }
        self.active.borrow_mut().push(name);
        Ok(())
    }
    fn unregister(&self, hk: &crate::hotkey::Hotkey) -> Result<(), String> {
        self.active.borrow_mut().retain(|c| *c != hk.to_string());
        Ok(())
    }
}

/// The OS login item.
struct FakeOs {
    on: Cell<bool>,
    fail: Cell<bool>,
}

impl crate::autostart::Autostart for FakeOs {
    fn set(&self, enabled: bool) -> Result<(), String> {
        if self.fail.get() {
            return Err("denied".to_owned());
        }
        self.on.set(enabled);
        Ok(())
    }
    fn current(&self) -> Option<bool> {
        Some(self.on.get())
    }
}

#[derive(Default)]
struct TrayCheck(Arc<std::sync::Mutex<Vec<bool>>>);

impl crate::pause::PauseIndicator for TrayCheck {
    fn show(&self, paused: bool) {
        self.0.lock().unwrap().push(paused);
    }
}

struct SettingsFx {
    fx: Fx,
    pinned: AtomicBool,
    pause: crate::pause::Pause,
    tray: Arc<std::sync::Mutex<Vec<bool>>>,
    ocr: OcrQueueHandle,
    reg: FakeReg,
    os: FakeOs,
    hotkey_error: std::sync::Mutex<Option<String>>,
    /// A real saver over a temp dir, to look at what reaches settings.json.
    dir: tempfile::TempDir,
    saver: crate::settings::saver::SettingsSaver,
}

impl SettingsFx {
    fn new() -> Self {
        let fx = Fx::new();
        let ocr = OcrQueueHandle::new(Arc::clone(&fx.settings));
        let pause = crate::pause::Pause::default();
        let tray = TrayCheck::default();
        let log = Arc::clone(&tray.0);
        pause.set_indicator(Box::new(tray));
        let reg = FakeReg::default();
        reg.active.borrow_mut().push("Ctrl+Shift+Space".to_owned());
        let dir = tempfile::tempdir().unwrap();
        let saver = crate::settings::saver::SettingsSaver::start_with(
            dir.path().to_owned(),
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(60),
        )
        .unwrap();
        Self {
            fx,
            pinned: AtomicBool::new(false),
            pause,
            tray: log,
            ocr,
            reg,
            os: FakeOs {
                on: Cell::new(true),
                fail: Cell::new(false),
            },
            hotkey_error: std::sync::Mutex::new(None),
            dir,
            saver,
        }
    }

    fn env(&self) -> SettingsEnv<'_> {
        SettingsEnv {
            settings: &self.fx.settings,
            pinned_bar: &self.pinned,
            pause: &self.pause,
            store: &self.fx.store,
            ocr: Some(&self.ocr),
            registrar: &self.reg,
            autostart: &self.os,
            saver: Some(&self.saver),
            hotkey_error: &self.hotkey_error,
        }
    }

    /// What settings.json holds once the saver has written.
    fn on_disk(&self) -> Option<crate::settings::Settings> {
        self.saver.flush();
        self.dir
            .path()
            .join(crate::settings::file::FILE_NAME)
            .exists()
            .then(|| crate::settings::file::load(self.dir.path()).settings)
    }
}

#[test]
fn get_settings_returns_the_defaults_with_the_runtime_flags() {
    let sfx = SettingsFx::new();
    let dto = sfx.env().current();
    assert_eq!(dto.max_items, 1_000);
    assert_eq!(dto.max_age_days, 30);
    assert!(dto.ocr_enabled);
    assert!(!dto.auto_paste_on_enter, "auto paste is off by default");
    assert!(!dto.pinned_bar && !dto.paused);
    let v = serde_json::to_value(&dto).unwrap();
    for key in [
        "maxTextBytes",
        "maxImageBytes",
        "ignoredApps",
        "maxItems",
        "maxAgeDays",
        "diskCapBytes",
        "ramCacheBytes",
        "ocrEnabled",
        "autoPasteOnEnter",
        "pinnedBar",
        "paused",
        "hotkey",
        "startAtLogin",
    ] {
        assert!(v.get(key).is_some(), "{key}");
    }
    assert_eq!(dto.hotkey, "Ctrl+Shift+Space");
    assert!(dto.start_at_login, "on by default");
}

#[test]
fn set_settings_applies_values_flags_and_retention_at_once() {
    let sfx = SettingsFx::new();
    for i in 0..5 {
        sfx.fx.text(&format!("item {i}"));
        sfx.fx.advance(10);
    }
    let pinned = sfx.fx.text("pinned one");
    toggle_pin_of(&sfx.fx.store, pinned).unwrap();

    let env = sfx.env();
    let mut dto = env.current();
    dto.max_items = 2;
    dto.pinned_bar = true;
    dto.paused = true;
    dto.max_text_bytes = 4096;
    let stored = env.apply(dto.clone()).unwrap();
    assert_eq!(stored, dto);
    assert_eq!(env.current(), dto, "get_settings reads it back");

    let s = sfx.fx.settings.read().unwrap();
    assert_eq!((s.max_items, s.max_text_bytes), (2, 4096));
    drop(s);
    assert!(sfx.pinned.load(Ordering::Relaxed));
    assert!(sfx.pause.is_paused());
    assert_eq!(
        *sfx.tray.lock().unwrap(),
        vec![false, true],
        "the tray check follows"
    );
    // Two unpinned kept (the newest) plus the pinned one.
    assert_eq!(sfx.fx.store.len(), 3);
    assert!(sfx.fx.store.get(pinned).is_some());
}

#[test]
fn set_settings_shrinks_the_ram_cache_and_stops_ocr() {
    let sfx = SettingsFx::new();
    let id = sfx.fx.image(1, 4000);
    sfx.fx.flush();
    sfx.fx.store.get_blob(id).unwrap();
    assert_eq!(sfx.fx.store.cache_bytes(), 4000);
    sfx.ocr.enqueue(OcrJob {
        item_id: id as i64,
        hash: 1,
        width: 640,
        height: 480,
    });
    assert_eq!(sfx.ocr.backlog(), 1);

    let env = sfx.env();
    let mut dto = env.current();
    dto.ram_cache_bytes = 0;
    dto.ocr_enabled = false;
    env.apply(dto).unwrap();
    assert_eq!(sfx.fx.store.cache_bytes(), 0, "budget 0 empties the cache");
    assert_eq!(sfx.ocr.backlog(), 0, "waiting OCR jobs are dropped");
}

#[test]
fn auto_paste_on_enter_applies_and_is_what_get_settings_returns() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    let mut dto = env.current();
    assert!(!dto.auto_paste_on_enter);
    dto.auto_paste_on_enter = true;
    let stored = env.apply(dto).unwrap();
    assert!(stored.auto_paste_on_enter);
    assert!(env.current().auto_paste_on_enter);
    assert!(
        sfx.fx.settings.read().unwrap().auto_paste_on_enter,
        "the shared Settings carries it"
    );
    // The webview sends the whole object: a change to another field keeps it.
    let mut other = env.current();
    other.max_items = 500;
    assert!(env.apply(other).unwrap().auto_paste_on_enter);
    // And it switches off again.
    let mut off = env.current();
    off.auto_paste_on_enter = false;
    assert!(!env.apply(off).unwrap().auto_paste_on_enter);
    let json = serde_json::to_value(env.current()).unwrap();
    assert_eq!(json["autoPasteOnEnter"], false);
}

#[test]
fn set_settings_refuses_limits_that_would_stop_capture() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    let before = env.current();
    let mut bad = before.clone();
    bad.max_text_bytes = 0;
    assert!(env.apply(bad).is_err());
    let mut bad = before.clone();
    bad.max_image_bytes = 10;
    assert!(env.apply(bad).is_err());
    assert_eq!(env.current(), before, "a refused change applies nothing");
}

#[test]
fn the_ignore_list_is_trimmed_and_deduplicated() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    let mut dto = env.current();
    dto.ignored_apps = vec![
        " KeePass.exe ".into(),
        "keepass.EXE".into(),
        "".into(),
        "   ".into(),
        "1password.exe".into(),
    ];
    let stored = env.apply(dto).unwrap();
    assert_eq!(stored.ignored_apps, vec!["KeePass.exe", "1password.exe"]);
    assert_eq!(
        sfx.fx.settings.read().unwrap().ignored_apps,
        vec!["KeePass.exe", "1password.exe"]
    );

    let mut too_long = env.current();
    too_long.ignored_apps = vec!["x".repeat(300)];
    assert!(env.apply(too_long).is_err());
    let mut too_many = env.current();
    too_many.ignored_apps = (0..501).map(|i| format!("app{i}.exe")).collect();
    assert!(env.apply(too_many).is_err());
}

#[test]
fn the_llm_key_is_write_only_and_the_url_is_checked() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    let mut dto = env.current();
    assert!(!dto.llm_api_key_set);
    dto.llm_base_url = " http://localhost:11434/v1 ".into();
    dto.llm_model = " llama3.2 ".into();
    dto.llm_api_key = Some(" sk-secret ".into());
    let stored = env.apply(dto).unwrap();
    assert_eq!(stored.llm_base_url, "http://localhost:11434/v1");
    assert_eq!(stored.llm_model, "llama3.2");
    assert!(stored.llm_api_key_set);
    assert_eq!(
        stored.llm_api_key, None,
        "the key never goes back to the webview"
    );
    assert_eq!(sfx.fx.settings.read().unwrap().llm_api_key, "sk-secret");
    let json = serde_json::to_string(&env.current()).unwrap();
    assert!(!json.contains("sk-secret"));

    // The webview sends the whole object back with a null key: the key stays.
    let mut other = env.current();
    other.max_items = 500;
    assert!(env.apply(other).unwrap().llm_api_key_set);
    // An empty string removes it.
    let mut clear = env.current();
    clear.llm_api_key = Some(String::new());
    assert!(!env.apply(clear).unwrap().llm_api_key_set);

    let mut bad = env.current();
    bad.llm_base_url = "localhost:11434".into();
    assert!(env.apply(bad).is_err());
    let mut empty = env.current();
    empty.llm_base_url = String::new();
    assert!(env.apply(empty).is_ok(), "clearing the URL turns Split off");
}

#[test]
fn settings_json_must_be_complete_and_camel_case() {
    let sfx = SettingsFx::new();
    let mut v = serde_json::to_value(sfx.env().current()).unwrap();
    assert!(serde_json::from_value::<SettingsDto>(v.clone()).is_ok());
    v.as_object_mut().unwrap().remove("maxItems");
    assert!(serde_json::from_value::<SettingsDto>(v).is_err());
}

#[test]
fn stats_count_items_cache_ocr_and_name_the_key_protector() {
    let sfx = SettingsFx::new();
    let fx = &sfx.fx;
    fx.text("a");
    let img = fx.image(1, 3000);
    let img2 = fx.image(2, 100);
    let pinned = fx.text("b");
    toggle_pin_of(&fx.store, pinned).unwrap();
    fx.store.set_ocr_text(img2, None, OcrStatus::Unavailable);
    fx.flush();
    fx.store.get_blob(img).unwrap();

    let st = stats_of(&fx.store, &fx.settings, Some(&sfx.ocr), None);
    assert_eq!(st.items, 4);
    assert_eq!(st.pinned, 1);
    assert!(st.disk_bytes > 3000);
    assert_eq!(st.cache_bytes, 3000);
    assert_eq!(st.cache_budget_bytes, 64 * 1024 * 1024);
    assert_eq!(st.ocr_pending, 1);
    assert_eq!(st.ocr_unavailable, 1);
    assert_eq!(st.ocr_backlog, 0);
    assert_eq!(st.ocr_engine, "unknown", "no worker has looked yet");
    assert_eq!(st.ocr_hint, OCR_UNAVAILABLE_HINT);
    assert_eq!(st.key_protector, fx.store.persist().key_protector_name());
    assert!(!st.key_protector.is_empty());

    let none = stats_of(&fx.store, &fx.settings, None, Some("taken".to_owned()));
    assert_eq!(none.ocr_engine, "unavailable");
    assert_eq!(none.hotkey_error.as_deref(), Some("taken"));
    assert_eq!(st.hotkey_error, None);
    let v = serde_json::to_value(&st).unwrap();
    for key in [
        "diskBytes",
        "cacheBytes",
        "cacheBudgetBytes",
        "ocrPending",
        "ocrUnavailable",
        "ocrBacklog",
        "ocrEngine",
        "ocrHint",
        "keyProtector",
        "hotkeyError",
        "recovery",
        "perf",
    ] {
        assert!(v.get(key).is_some(), "{key}");
    }
    // The command fills `recovery` from the app state; `stats_of` leaves it empty.
    assert_eq!(st.recovery, None);
}

// -- Phase 12: persistence, hotkey, login item, pause, retention ---------------

#[test]
fn a_change_reaches_settings_json_and_unchanged_saves_do_not_write() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    assert!(sfx.on_disk().is_none(), "nothing written before a change");
    let mut dto = env.current();
    dto.max_items = 123;
    dto.pinned_bar = true;
    env.apply(dto).unwrap();
    let disk = sfx.on_disk().expect("written");
    assert_eq!((disk.max_items, disk.pinned_bar), (123, true));
    assert_eq!(sfx.saver.write_count(), 1);
    // Same values again: no new write.
    env.apply(env.current()).unwrap();
    sfx.saver.flush();
    assert_eq!(sfx.saver.write_count(), 1);
}

#[test]
fn many_quick_changes_make_one_write() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    for n in 1..=50 {
        let mut dto = env.current();
        dto.max_items = n;
        env.apply(dto).unwrap();
    }
    assert_eq!(sfx.saver.write_count(), 0, "debounced");
    assert_eq!(sfx.on_disk().unwrap().max_items, 50);
    assert_eq!(sfx.saver.write_count(), 1);
}

#[test]
fn pause_is_applied_but_never_written_to_disk() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    let mut dto = env.current();
    dto.paused = true;
    env.apply(dto).unwrap();
    assert!(sfx.pause.is_paused());
    assert!(
        sfx.on_disk().is_none(),
        "a pause alone does not touch the file"
    );
    let mut dto = env.current();
    dto.max_items = 9;
    env.apply(dto).unwrap();
    sfx.on_disk().unwrap();
    let text =
        std::fs::read_to_string(sfx.dir.path().join(crate::settings::file::FILE_NAME)).unwrap();
    assert!(!text.contains("paused"));
}

#[test]
fn the_tray_toggle_and_the_settings_switch_stay_consistent() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    assert!(sfx.pause.toggle(), "tray: pause");
    assert!(env.current().paused, "get_settings sees the tray's change");
    let mut dto = env.current();
    dto.paused = false;
    env.apply(dto).unwrap();
    assert!(!sfx.pause.is_paused());
    assert_eq!(*sfx.tray.lock().unwrap(), vec![false, true, false]);
}

#[test]
fn changing_the_hotkey_moves_the_registration_and_is_saved_canonical() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    let mut dto = env.current();
    dto.hotkey = "alt + ctrl + k".to_owned();
    let stored = env.apply(dto).unwrap();
    assert_eq!(stored.hotkey, "Ctrl+Alt+K");
    assert_eq!(*sfx.reg.active.borrow(), vec!["Ctrl+Alt+K".to_owned()]);
    assert_eq!(sfx.on_disk().unwrap().hotkey, "Ctrl+Alt+K");
}

#[test]
fn a_hotkey_another_app_owns_keeps_the_old_one_and_is_not_saved() {
    let sfx = SettingsFx::new();
    sfx.reg.taken.borrow_mut().insert("Super+L".to_owned());
    let env = sfx.env();
    let before = env.current();
    let mut dto = before.clone();
    dto.hotkey = "Win+L".to_owned();
    dto.max_items = 77; // the same save carries another change
    let err = env.apply(dto).unwrap_err();
    assert!(err.contains("taken by another app"), "{err}");
    assert_eq!(
        *sfx.reg.active.borrow(),
        vec!["Ctrl+Shift+Space".to_owned()]
    );
    assert_eq!(env.current(), before, "nothing was applied");
    assert!(sfx.on_disk().is_none(), "the bad value is never persisted");
}

#[test]
fn an_invalid_chord_is_refused_before_anything_changes() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    for bad in ["", "Space", "Ctrl+Shift", "Ctrl+Banana", "Ctrl+A+B"] {
        let mut dto = env.current();
        dto.hotkey = bad.to_owned();
        assert!(env.apply(dto).is_err(), "{bad:?}");
    }
    assert_eq!(
        *sfx.reg.active.borrow(),
        vec!["Ctrl+Shift+Space".to_owned()]
    );
}

#[test]
fn a_successful_hotkey_change_clears_the_startup_note() {
    let sfx = SettingsFx::new();
    *sfx.hotkey_error.lock().unwrap() = Some("taken".to_owned());
    let env = sfx.env();
    let mut dto = env.current();
    dto.hotkey = "Ctrl+Alt+J".to_owned();
    env.apply(dto).unwrap();
    assert_eq!(*sfx.hotkey_error.lock().unwrap(), None);
}

#[test]
fn any_save_retries_a_hotkey_that_was_taken_at_startup() {
    let sfx = SettingsFx::new();
    // At startup "Ctrl+Shift+Space" was owned by another app: nothing is active.
    sfx.reg.active.borrow_mut().clear();
    *sfx.hotkey_error.lock().unwrap() = Some("taken".to_owned());
    let env = sfx.env();
    sfx.reg
        .taken
        .borrow_mut()
        .insert("Ctrl+Shift+Space".to_owned());
    let mut dto = env.current();
    dto.max_items = 5;
    env.apply(dto).expect("the unrelated save still works");
    assert!(sfx.hotkey_error.lock().unwrap().is_some(), "still taken");
    // The other app lets go.
    sfx.reg.taken.borrow_mut().clear();
    let mut dto = env.current();
    dto.max_items = 6;
    env.apply(dto).unwrap();
    assert_eq!(*sfx.hotkey_error.lock().unwrap(), None);
    assert_eq!(
        *sfx.reg.active.borrow(),
        vec!["Ctrl+Shift+Space".to_owned()]
    );
}

#[test]
fn start_at_login_goes_to_the_os_and_a_refusal_changes_nothing() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    let mut dto = env.current();
    dto.start_at_login = false;
    assert!(!env.apply(dto).unwrap().start_at_login);
    assert!(!sfx.os.on.get());
    assert!(!sfx.on_disk().unwrap().start_at_login);

    sfx.os.fail.set(true);
    let before = env.current();
    let mut dto = before.clone();
    dto.start_at_login = true;
    dto.hotkey = "Ctrl+Alt+K".to_owned();
    let err = env.apply(dto).unwrap_err();
    assert_eq!(err, "could not change the start at login setting");
    assert_eq!(env.current(), before);
    assert_eq!(
        *sfx.reg.active.borrow(),
        vec!["Ctrl+Shift+Space".to_owned()],
        "the hotkey moved back"
    );
    assert!(!sfx.pinned.load(Ordering::Relaxed));
}

#[test]
fn the_login_item_is_only_touched_when_the_setting_changes() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    sfx.os.fail.set(true); // would fail if called
    let mut dto = env.current();
    dto.max_items = 3;
    assert!(env.apply(dto).is_ok());
}

#[test]
fn pinned_bar_flag_follows_the_setting_both_ways() {
    let sfx = SettingsFx::new();
    let env = sfx.env();
    let mut dto = env.current();
    dto.pinned_bar = true;
    env.apply(dto).unwrap();
    assert!(sfx.pinned.load(Ordering::Relaxed));
    let mut dto = env.current();
    dto.pinned_bar = false;
    env.apply(dto).unwrap();
    assert!(!sfx.pinned.load(Ordering::Relaxed));
}

#[test]
fn lowering_max_items_evicts_at_once_pins_survive_and_it_persists() {
    let sfx = SettingsFx::new();
    let mut unpinned = Vec::new();
    for i in 0..8 {
        unpinned.push(sfx.fx.text(&format!("item {i}")));
        sfx.fx.advance(10);
    }
    let pinned = sfx.fx.text("keep me");
    toggle_pin_of(&sfx.fx.store, pinned).unwrap();
    sfx.fx.advance(10);
    let env = sfx.env();
    let mut dto = env.current();
    dto.max_items = 3;
    env.apply(dto).unwrap();
    assert_eq!(sfx.fx.store.len(), 4, "3 unpinned newest plus the pin");
    assert!(sfx.fx.store.get(pinned).is_some());
    assert!(sfx.fx.store.get(unpinned[0]).is_none(), "oldest gone");
    assert!(sfx.fx.store.get(unpinned[7]).is_some());
    assert_eq!(sfx.on_disk().unwrap().max_items, 3);
    // A max of 0 means no limit and evicts nothing more.
    let mut dto = env.current();
    dto.max_items = 0;
    env.apply(dto).unwrap();
    assert_eq!(sfx.fx.store.len(), 4);
}

#[test]
fn lowering_the_age_limit_evicts_old_unpinned_items_but_not_pins() {
    let sfx = SettingsFx::new();
    let old = sfx.fx.text("old");
    let old_pin = sfx.fx.text("old pinned");
    toggle_pin_of(&sfx.fx.store, old_pin).unwrap();
    sfx.fx.advance(3 * 24 * 3600 * 1000);
    let fresh = sfx.fx.text("fresh");
    let env = sfx.env();
    let mut dto = env.current();
    dto.max_age_days = 1;
    env.apply(dto).unwrap();
    assert!(sfx.fx.store.get(old).is_none());
    assert!(sfx.fx.store.get(old_pin).is_some());
    assert!(sfx.fx.store.get(fresh).is_some());
}

// -- capabilities --------------------------------------------------------------

/// The names in `build.rs` `COMMANDS`, the `generate_handler!` list in `lib.rs`
/// and the `allow-...` permissions of the capability must be the same set: a
/// command missing from the capability is refused at runtime, and one missing
/// from `build.rs` is allowed to everyone.
#[test]
fn build_rs_handler_and_capability_list_the_same_commands() {
    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }
    let build = include_str!("../../build.rs");
    let list = build
        .split("COMMANDS: &[&str] = &[")
        .nth(1)
        .and_then(|s| s.split("];").next())
        .unwrap();
    let from_build = sorted(
        list.split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect(),
    );

    let lib = include_str!("../lib.rs");
    let handler = lib
        .split("generate_handler![")
        .nth(1)
        .and_then(|s| s.split(']').next())
        .unwrap();
    let from_lib = sorted(
        handler
            .split(',')
            .map(|c| c.trim().trim_start_matches("commands::").to_owned())
            .filter(|c| !c.is_empty())
            .collect(),
    );

    let cap: serde_json::Value =
        serde_json::from_str(include_str!("../../capabilities/default.json")).unwrap();
    let from_cap = sorted(
        cap["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p.as_str()?.strip_prefix("allow-"))
            .map(|p| p.replace('-', "_"))
            .collect(),
    );

    assert_eq!(from_build.len(), 16);
    assert_eq!(from_build, from_lib, "build.rs vs generate_handler!");
    assert_eq!(
        from_build, from_cap,
        "build.rs vs capabilities/default.json"
    );
    // Only event listening and the app commands: no plugin permission.
    let others: Vec<_> = cap["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.as_str())
        .filter(|p| !p.starts_with("allow-"))
        .collect();
    assert_eq!(
        others,
        ["core:event:allow-listen", "core:event:allow-unlisten"]
    );
}
