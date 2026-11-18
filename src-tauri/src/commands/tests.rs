//! Tests of the command logic. They call the plain functions with a real store
//! over a temp directory (no Tauri runtime).

use super::*;
use crate::model::ClipKind;
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
