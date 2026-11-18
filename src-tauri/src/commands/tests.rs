//! Tests of the command logic. They call the plain functions with a real store
//! over a temp directory (no Tauri runtime).

use super::*;
use crate::commands::preview::{
    PREVIEW_FILES_MAX, PREVIEW_TEXT_MAX_BYTES, file_name, truncate_utf8,
};
use crate::model::{ClipKind, OcrStatus};
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
