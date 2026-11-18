//! Tests of the command logic. They call the plain functions with a real store
//! over a temp directory (no Tauri runtime).

use std::cell::{Cell, RefCell};
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

struct SettingsFx {
    fx: Fx,
    pinned: AtomicBool,
    paused: AtomicBool,
    ocr: OcrQueueHandle,
}

impl SettingsFx {
    fn new() -> Self {
        let fx = Fx::new();
        let ocr = OcrQueueHandle::new(Arc::clone(&fx.settings));
        Self {
            fx,
            pinned: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            ocr,
        }
    }

    fn env(&self) -> SettingsEnv<'_> {
        SettingsEnv {
            settings: &self.fx.settings,
            pinned_bar: &self.pinned,
            paused: &self.paused,
            store: &self.fx.store,
            ocr: Some(&self.ocr),
        }
    }
}

#[test]
fn get_settings_returns_the_defaults_with_the_runtime_flags() {
    let sfx = SettingsFx::new();
    let dto = sfx.env().current();
    assert_eq!(dto.max_items, 1_000);
    assert_eq!(dto.max_age_days, 30);
    assert!(dto.ocr_enabled);
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
        "pinnedBar",
        "paused",
    ] {
        assert!(v.get(key).is_some(), "{key}");
    }
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
    assert!(sfx.paused.load(Ordering::Relaxed));
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

    let st = stats_of(&fx.store, &fx.settings, Some(&sfx.ocr));
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

    let none = stats_of(&fx.store, &fx.settings, None);
    assert_eq!(none.ocr_engine, "unavailable");
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
    ] {
        assert!(v.get(key).is_some(), "{key}");
    }
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

    assert_eq!(from_build.len(), 10);
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
