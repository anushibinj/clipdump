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

// -- missing files: checker, cache, TTL ------------------------------------------

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

/// A checker over a fixed set of existing paths that counts its calls.
struct FakeFs {
    present: Mutex<HashSet<String>>,
    calls: Arc<AtomicUsize>,
}

impl FakeFs {
    fn new(present: &[&str]) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let fs = Self {
            present: Mutex::new(present.iter().map(|p| (*p).to_string()).collect()),
            calls: Arc::clone(&calls),
        };
        (fs, calls)
    }
}

impl PathChecker for FakeFs {
    fn exists(&self, path: &str) -> bool {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        self.present.lock().contains(path)
    }
}

use parking_lot::Mutex;

/// A cached checker whose clock is a counter of milliseconds.
fn cached(present: &[&str]) -> (CachedPathChecker, Arc<AtomicUsize>, Arc<AtomicU64>) {
    let (fs, calls) = FakeFs::new(present);
    let ms = Arc::new(AtomicU64::new(0));
    let base = Instant::now();
    let clock_ms = Arc::clone(&ms);
    let c = CachedPathChecker::with_clock(
        Box::new(fs),
        Duration::from_millis(2000),
        Box::new(move || base + Duration::from_millis(clock_ms.load(AtomicOrdering::SeqCst))),
    );
    (c, calls, ms)
}

#[test]
fn count_missing_counts_paths_that_do_not_exist() {
    let (fs, _) = FakeFs::new(&["/a", "/c"]);
    let list: Vec<FileRef> = ["/a", "/b", "/c", "/d"]
        .iter()
        .map(|p| FileRef {
            path: (*p).into(),
            size: 1,
            is_dir: false,
        })
        .collect();
    assert_eq!(count_missing(&list, &fs), 2);
    assert_eq!(count_missing(&[], &fs), 0);
}

#[test]
fn count_missing_stops_at_the_per_item_cap() {
    let (fs, calls) = FakeFs::new(&[]);
    let list: Vec<FileRef> = (0..MISSING_CHECK_MAX + 50)
        .map(|i| FileRef {
            path: format!("/gone/{i}"),
            size: 1,
            is_dir: false,
        })
        .collect();
    assert_eq!(count_missing(&list, &fs) as usize, MISSING_CHECK_MAX);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), MISSING_CHECK_MAX);
}

#[test]
fn the_cache_answers_within_the_ttl_and_rechecks_after_it() {
    let (c, calls, ms) = cached(&["/here"]);
    assert!(c.exists("/here"));
    assert!(!c.exists("/gone"));
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    // Within the TTL: no new disk access.
    ms.store(1999, AtomicOrdering::SeqCst);
    assert!(c.exists("/here"));
    assert!(!c.exists("/gone"));
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    // At the TTL the entry is stale.
    ms.store(2000, AtomicOrdering::SeqCst);
    assert!(c.exists("/here"));
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    assert_eq!(c.len(), 2);
}

#[test]
fn a_stale_answer_is_replaced_when_the_file_changes() {
    let (fs, _) = FakeFs::new(&[]);
    let present = Arc::new(fs);
    struct Shared(Arc<FakeFs>);
    impl PathChecker for Shared {
        fn exists(&self, p: &str) -> bool {
            self.0.exists(p)
        }
    }
    let ms = Arc::new(AtomicU64::new(0));
    let base = Instant::now();
    let m = Arc::clone(&ms);
    let c = CachedPathChecker::with_clock(
        Box::new(Shared(Arc::clone(&present))),
        Duration::from_millis(2000),
        Box::new(move || base + Duration::from_millis(m.load(AtomicOrdering::SeqCst))),
    );
    assert!(!c.exists("/new"));
    present.present.lock().insert("/new".into());
    assert!(!c.exists("/new"), "still the cached answer inside the TTL");
    ms.store(2500, AtomicOrdering::SeqCst);
    assert!(c.exists("/new"));
}

#[test]
fn the_real_checker_sees_real_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, b"x").unwrap();
    assert!(FsPathChecker.exists(file.to_str().unwrap()));
    assert!(FsPathChecker.exists(dir.path().to_str().unwrap()));
    assert!(!FsPathChecker.exists(dir.path().join("nope").to_str().unwrap()));
}

// -- result DTOs -------------------------------------------------------------

/// Run the whole search with an injected checker.
fn run(items: &[Arc<ClipItem>], query: &str, paths: &dyn PathChecker) -> Vec<ResultDto> {
    search(
        items.iter(),
        &SearchRequest::new(query, TypeFilter::All, NOW),
        paths,
    )
}

#[test]
fn a_text_result_carries_metadata_but_no_full_content() {
    let long = "needle ".to_string() + &"filler ".repeat(500);
    let item = with(text(7, &long, 5 * DAY), |i| {
        i.use_count = 3;
        i.pinned = true;
        // The store keeps a 300 char preview.
        i.preview = Arc::from(&long[..300]);
    });
    let (fs, _) = FakeFs::new(&[]);
    let r = run(&[item], "needle", &fs);
    let d = &r[0];
    assert_eq!(d.id, 7);
    assert_eq!(d.kind, ClipKind::Text);
    assert_eq!(d.match_source, MatchSource::Text);
    assert_eq!((d.use_count, d.pinned), (3, true));
    assert_eq!(d.created_at, NOW - 5 * DAY);
    assert_eq!((d.file_count, d.missing_files, d.has_thumb), (0, 0, false));
    assert_eq!(d.preview.chars().count(), 300);
    assert_eq!(d.snippet, None, "the preview shows the match");
    assert_eq!(d.match_indices, [0, 1, 2, 3, 4, 5]);
}

#[test]
fn the_dto_serializes_as_camel_case_json() {
    let (fs, _) = FakeFs::new(&[]);
    let r = run(&[image(1, "invoice total", DAY)], "invoice", &fs);
    let v = serde_json::to_value(&r[0]).unwrap();
    let obj = v.as_object().unwrap();
    for key in [
        "id",
        "kind",
        "preview",
        "snippet",
        "matchIndices",
        "matchSource",
        "createdAt",
        "useCount",
        "pinned",
        "width",
        "height",
        "byteSize",
        "fileCount",
        "missingFiles",
        "hasThumb",
    ] {
        assert!(obj.contains_key(key), "missing {key}");
    }
    assert_eq!(obj.len(), 15, "no stray fields: {obj:?}");
    assert_eq!(v["kind"], "image");
    assert_eq!(v["matchSource"], "ocr");
    assert_eq!(v["hasThumb"], true);
}

#[test]
fn an_ocr_match_gets_a_snippet_around_it_and_indices_into_the_snippet() {
    let ocr = format!(
        "{} the secret passphrase is here {}",
        "lorem ".repeat(60),
        "ipsum ".repeat(60)
    );
    let (fs, _) = FakeFs::new(&[]);
    let r = run(&[image(1, &ocr, 1)], "passphrase", &fs);
    let d = &r[0];
    assert_eq!(d.match_source, MatchSource::Ocr);
    let snip = d.snippet.as_deref().expect("OCR matches carry a snippet");
    assert!(snip.chars().count() <= SNIPPET_CHARS);
    assert!(snip.contains("passphrase"));
    assert!(!snip.contains("  "), "whitespace is collapsed");
    // The indices point at "passphrase" inside the snippet.
    let hl: String = d
        .match_indices
        .iter()
        .map(|&i| snip.chars().nth(i as usize).unwrap())
        .collect();
    assert_eq!(hl, "passphrase");
    assert_eq!(
        d.preview, "Image 1920x1080",
        "the preview is not the OCR text"
    );
}

#[test]
fn file_results_count_files_and_missing_ones() {
    let item = files(
        2,
        &["/docs/report.pdf", "/docs/old.txt", "/docs/report2.pdf"],
        1,
    );
    let (fs, calls) = FakeFs::new(&["/docs/report.pdf"]);
    let r = run(&[item], "report", &fs);
    let d = &r[0];
    assert_eq!(d.kind, ClipKind::Files);
    assert_eq!(d.match_source, MatchSource::File);
    assert_eq!(d.file_count, 3);
    assert_eq!(d.missing_files, 2);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 3);
}

#[test]
fn existence_is_checked_only_for_returned_file_items() {
    let (fs, calls) = FakeFs::new(&[]);
    let items = [
        files(1, &["/a/matching.txt"], 1),
        files(2, &["/a/other.txt"], 2),
        text(3, "matching text", 3),
    ];
    let r = run(&items, "matching", &fs);
    assert_eq!(r.len(), 2);
    assert_eq!(
        calls.load(AtomicOrdering::SeqCst),
        1,
        "item 2 did not match"
    );
    // A text only search never touches the checker.
    calls.store(0, AtomicOrdering::SeqCst);
    let _ = run(&[text(3, "x", 1)], "x", &fs);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}
