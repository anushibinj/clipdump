//! Search tests. Items are built in memory; nothing here touches disk.

use super::*;
use crate::model::{FileRef, OcrStatus};

const DAY: i64 = 86_400_000;
/// The "now" most tests use.
const NOW: i64 = 1_000 * DAY;

/// A text item. `age_ms` is how long ago it was last used.
fn text(id: u64, body: &str, age_ms: i64) -> Arc<ClipItem> {
    Arc::new(ClipItem {
        id,
        hash: id as u128,
        kind: ClipKind::Text,
        created_at: NOW - age_ms,
        last_used_at: NOW - age_ms,
        use_count: 0,
        pinned: false,
        preview: Arc::from(body.split_whitespace().collect::<Vec<_>>().join(" ")),
        search_text: Arc::from(body),
        text_in_ram: Some(Arc::from(body)),
        thumb: None,
        files: None,
        width: 0,
        height: 0,
        byte_size: body.len() as u64,
        ocr_status: OcrStatus::Done,
        source_app: None,
        blob_hash: None,
    })
}

fn with(item: Arc<ClipItem>, f: impl FnOnce(&mut ClipItem)) -> Arc<ClipItem> {
    let mut item = item;
    f(Arc::make_mut(&mut item));
    item
}

fn pinned(item: Arc<ClipItem>) -> Arc<ClipItem> {
    with(item, |i| i.pinned = true)
}

fn image(id: u64, ocr: &str, age_ms: i64) -> Arc<ClipItem> {
    with(text(id, ocr, age_ms), |i| {
        i.kind = ClipKind::Image;
        i.preview = Arc::from("Image 1920x1080");
        i.text_in_ram = None;
        i.width = 1920;
        i.height = 1080;
        i.byte_size = 1_200_000;
        i.thumb = Some(Arc::from(vec![1u8, 2, 3]));
    })
}

fn files(id: u64, paths: &[&str], age_ms: i64) -> Arc<ClipItem> {
    let list: Vec<FileRef> = paths
        .iter()
        .map(|p| FileRef {
            path: (*p).to_string(),
            size: 10,
            is_dir: false,
        })
        .collect();
    let names: Vec<&str> = paths
        .iter()
        .map(|p| p.rsplit(['/', '\\']).next().unwrap())
        .collect();
    with(text(id, &paths.join("\n"), age_ms), |i| {
        i.kind = ClipKind::Files;
        i.preview = Arc::from(names.join(", "));
        i.text_in_ram = None;
        i.byte_size = 10 * list.len() as u64;
        i.files = Some(Arc::from(list));
    })
}

/// Rank `items` (given newest first, as the store iterates them).
fn ranked(items: &[Arc<ClipItem>], query: &str, filter: TypeFilter) -> Vec<Ranked> {
    rank(items.iter(), &SearchRequest::new(query, filter, NOW))
}

fn ids(r: &[Ranked]) -> Vec<u64> {
    r.iter().map(|r| r.item().id).collect()
}

#[test]
fn blank_queries_have_no_terms() {
    assert!(Query::parse("").is_none());
    assert!(Query::parse("   \t ").is_none());
    // A lone operator has no needle either.
    assert!(Query::parse("!").is_none());
}

#[test]
fn plain_words_become_fuzzy_terms_with_an_exact_twin() {
    let q = Query::parse("foo bar").unwrap();
    assert_eq!(q.terms.len(), 2);
    assert_eq!(q.positives, 2);
    for t in &q.terms {
        assert_eq!(t.atom.kind, AtomKind::Fuzzy);
        assert_eq!(t.exact.as_ref().unwrap().kind, AtomKind::Substring);
    }
}

#[test]
fn operators_are_parsed_and_have_no_twin() {
    let q = Query::parse("^foo bar$ 'baz !qux").unwrap();
    let kinds: Vec<_> = q.terms.iter().map(|t| t.atom.kind).collect();
    assert_eq!(
        kinds,
        [
            AtomKind::Prefix,
            AtomKind::Postfix,
            AtomKind::Substring,
            AtomKind::Substring
        ]
    );
    assert!(q.terms.iter().all(|t| t.exact.is_none()));
    assert_eq!(q.positives, 3, "the negated word does not have to match");
}

#[test]
fn the_matcher_is_reused_on_a_thread() {
    // The scratch buffers keep their allocation between calls.
    with_scratch(|s| s.chars.extend(['a'; 64]));
    with_scratch(|s| assert!(s.chars.capacity() >= 64));
}

#[test]
fn tabs_and_newlines_separate_words() {
    let q = Query::parse("foo\tbar\nbaz").unwrap();
    assert_eq!(q.terms.len(), 3);
}

// -- filters -------------------------------------------------------------

#[test]
fn prefixes_are_stripped_and_set_the_filter() {
    let p = parse_query("img: cat photo", TypeFilter::All);
    assert_eq!(p.text, "cat photo");
    assert_eq!(p.filter, TypeFilter::Images);
    assert_eq!(
        parse_query("file:report", TypeFilter::All).filter,
        TypeFilter::Files
    );
    assert_eq!(parse_query("file:report", TypeFilter::All).text, "report");
    assert_eq!(parse_query("txt:  hi", TypeFilter::All).text, "hi");
    assert_eq!(
        parse_query("txt:hi", TypeFilter::All).filter,
        TypeFilter::Text
    );
}

#[test]
fn a_typed_prefix_overrides_the_ui_filter() {
    for ui in [
        TypeFilter::All,
        TypeFilter::Text,
        TypeFilter::Images,
        TypeFilter::Files,
    ] {
        assert_eq!(parse_query("img:x", ui).filter, TypeFilter::Images);
        assert_eq!(parse_query("txt:x", ui).filter, TypeFilter::Text);
    }
}

#[test]
fn without_a_prefix_the_ui_filter_applies() {
    let p = parse_query("  hello img:", TypeFilter::Files);
    assert_eq!(p.filter, TypeFilter::Files);
    assert_eq!(p.text, "hello img:", "only a leading prefix counts");
}

#[test]
fn prefixes_are_case_insensitive_and_alone_is_valid() {
    let p = parse_query("  IMG:", TypeFilter::Text);
    assert_eq!(p.filter, TypeFilter::Images);
    assert_eq!(p.text, "");
    assert_eq!(
        parse_query("Txt:a", TypeFilter::All).filter,
        TypeFilter::Text
    );
}

#[test]
fn similar_words_are_not_prefixes() {
    assert_eq!(
        parse_query("image:x", TypeFilter::All).filter,
        TypeFilter::All
    );
    assert_eq!(parse_query("im", TypeFilter::All).text, "im");
    // Multibyte text right at the cut point must not panic.
    assert_eq!(parse_query("ééé", TypeFilter::All).text, "ééé");
}

#[test]
fn type_filter_accepts_the_right_kinds() {
    use crate::model::ClipKind::*;
    assert!(TypeFilter::All.accepts(Text) && TypeFilter::All.accepts(Image));
    assert!(TypeFilter::Text.accepts(Text) && !TypeFilter::Text.accepts(Image));
    assert!(TypeFilter::Images.accepts(Image) && !TypeFilter::Images.accepts(Files));
    assert!(TypeFilter::Files.accepts(Files) && !TypeFilter::Files.accepts(Text));
}

#[test]
fn type_filter_deserializes_from_the_ui_names() {
    let f: TypeFilter = serde_json::from_str("\"images\"").unwrap();
    assert_eq!(f, TypeFilter::Images);
    assert_eq!(
        serde_json::from_str::<TypeFilter>("\"all\"").unwrap(),
        TypeFilter::All
    );
}

// -- empty query ----------------------------------------------------------

#[test]
fn empty_query_lists_pinned_first_then_most_recent() {
    // Newest first, as the store iterates.
    let items = [
        text(1, "newest", 10),
        text(2, "second", 20),
        pinned(text(3, "old pinned", 5 * DAY)),
        text(4, "third", 30),
        pinned(text(5, "older pinned", 9 * DAY)),
    ];
    assert_eq!(ids(&ranked(&items, "", TypeFilter::All)), [3, 5, 1, 2, 4]);
    // Whitespace is an empty query too.
    assert_eq!(
        ids(&ranked(&items, "  \t", TypeFilter::All)),
        [3, 5, 1, 2, 4]
    );
}

#[test]
fn empty_query_respects_the_limit_and_filters() {
    let items = [
        text(1, "a", 1),
        image(2, "", 2),
        text(3, "b", 3),
        pinned(files(4, &["/x/a.txt"], 4)),
    ];
    let mut req = SearchRequest::new("", TypeFilter::All, NOW);
    req.limit = 2;
    assert_eq!(ids(&rank(items.iter(), &req)), [4, 1]);
    assert_eq!(ids(&ranked(&items, "", TypeFilter::Images)), [2]);
    assert_eq!(ids(&ranked(&items, "img:", TypeFilter::All)), [2]);
    assert_eq!(ids(&ranked(&items, "file:", TypeFilter::Text)), [4]);
    req.limit = 0;
    assert!(rank(items.iter(), &req).is_empty());
}

#[test]
fn pinned_items_alone_can_fill_the_limit() {
    let items: Vec<_> = (1..=10).map(|i| pinned(text(i, "p", i as i64))).collect();
    let mut req = SearchRequest::new("", TypeFilter::All, NOW);
    req.limit = 3;
    assert_eq!(ids(&rank(items.iter(), &req)), [1, 2, 3]);
}

// -- fuzzy ranking (sanity here, the full set is in "ranking tests") ---------

#[test]
fn a_fuzzy_query_finds_and_an_unrelated_one_does_not() {
    let items = [text(1, "the quick brown fox", 1), text(2, "lorem ipsum", 2)];
    assert_eq!(ids(&ranked(&items, "qbf", TypeFilter::All)), [1]);
    assert!(ranked(&items, "zzz", TypeFilter::All).is_empty());
}

#[test]
fn exact_substring_beats_a_better_scoring_fuzzy_match() {
    // Item 1 is a perfect word start fuzzy match and much more recent; item 2
    // contains the query as a plain substring, mid word.
    let items = [
        text(1, "Config Parser Class", 1),
        text(2, "xxcfgparxx", 50 * DAY),
    ];
    let r = ranked(&items, "cfgpar", TypeFilter::All);
    assert_eq!(ids(&r), [2, 1], "both match, the exact one first");
    assert!(r[0].is_exact() && !r[1].is_exact());
    // Without a contiguous match the recent word start match is first anyway.
    let r = ranked(&items, "cpc", TypeFilter::All);
    assert_eq!(ids(&r), [1]);
}
