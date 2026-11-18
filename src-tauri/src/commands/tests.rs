//! Tests of the command logic. They call the plain functions with a real store
//! over a temp directory (no Tauri runtime).

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicIsize, Ordering};

use super::*;
use crate::commands::preview::{
    PREVIEW_FILES_MAX, PREVIEW_TEXT_MAX_BYTES, file_name, truncate_utf8,
};
use crate::model::Payload;
use crate::model::{ClipKind, OcrStatus};
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
}

/// Each test runs on its own thread, so the thread locals are per test.
struct FakeWriter;
struct FakePaster;

impl ClipboardWriter for FakeWriter {
    fn set(payload: &Payload) -> anyhow::Result<()> {
        if FAIL_WRITE.get() {
            anyhow::bail!("clipboard is locked");
        }
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
    guard: SelfWriteGuard,
    prev: AtomicIsize,
}

impl CopyFx {
    fn new(prev: isize) -> Self {
        Self {
            guard: SelfWriteGuard::default(),
            prev: AtomicIsize::new(prev),
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
            now_ms: 1_000,
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
    let hash = fx.store.get(a).unwrap().hash;
    assert_eq!(cfx.guard.text_echo(1_100, hash), Some(hash));
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
    // A second paste has no window to go to.
    assert_eq!(cfx.copy(&fx, &all_exist(), id, true).0, Ok(()));
    assert_eq!(pasted(), vec![Some(1234), None]);
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

#[test]
fn the_platform_writer_is_a_stub_that_fails_until_phase_11() {
    let fx = Fx::new();
    let id = fx.text("keep");
    let cfx = CopyFx::new(0);
    let env = CopyEnv {
        store: &fx.store,
        paths: &all_exist(),
        self_write: &cfx.guard,
        prev_foreground: &cfx.prev,
        now_ms: 0,
    };
    let r = copy_item_with::<crate::platform::Writer, crate::platform::PasterImpl>(
        &env,
        id,
        false,
        || panic!("must not hide"),
    );
    assert!(r.is_err());
    assert_eq!(fx.store.get(id).unwrap().use_count, 0);
}
