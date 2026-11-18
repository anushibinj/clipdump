//! Search tests. Items are built in memory; nothing here touches disk.

use super::*;
use crate::model::{FileRef, OcrStatus, fold_for_search};

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
        search_lower: fold_for_search(&Arc::from(body)),
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

// -- ranking tests (step 7) --------------------------------------------------------

/// A checker for tests that do not care about files.
struct NoFiles;

impl PathChecker for NoFiles {
    fn exists(&self, _: &str) -> bool {
        true
    }
}

fn dtos(items: &[Arc<ClipItem>], query: &str, filter: TypeFilter) -> Vec<ResultDto> {
    search(
        items.iter(),
        &SearchRequest::new(query, filter, NOW),
        &NoFiles,
    )
}

#[test]
fn every_word_must_match_and_negated_words_exclude() {
    let items = [
        text(1, "alpha beta gamma", 1),
        text(2, "alpha only", 2),
        text(3, "beta only", 3),
    ];
    assert_eq!(ids(&ranked(&items, "alpha beta", TypeFilter::All)), [1]);
    assert_eq!(ids(&ranked(&items, "alpha !beta", TypeFilter::All)), [2]);
    // `^` anchors at the start of the text, `$` at its end.
    assert_eq!(ids(&ranked(&items, "^beta", TypeFilter::All)), [3]);
    assert_eq!(ids(&ranked(&items, "only$", TypeFilter::All)), [2, 3]);
}

#[test]
fn exact_beats_fuzzy_even_when_the_fuzzy_item_is_newer_and_used_more() {
    let fuzzy = with(text(1, "Quick Brown Fox", 1), |i| i.use_count = 500);
    let exact = text(2, "a qbf b", 80 * DAY);
    // Both match "qbf": the first fuzzily (word starts), the second contiguously.
    let r = ranked(&[fuzzy, exact], "qbf", TypeFilter::All);
    assert_eq!(ids(&r), [2, 1]);
    assert!(r[0].is_exact() && !r[1].is_exact());
}

#[test]
fn among_exact_matches_a_word_start_scores_higher() {
    let items = [text(1, "xxfooxx", 5), text(2, "foo bar", 5)];
    let r = ranked(&items, "foo", TypeFilter::All);
    assert_eq!(ids(&r), [2, 1]);
    assert!(r.iter().all(Ranked::is_exact));
}

#[test]
fn all_words_contiguous_is_exact_but_one_scattered_word_is_not() {
    let items = [text(1, "foo bar", 1), text(2, "fxoxo bar", 2)];
    let r = ranked(&items, "foo bar", TypeFilter::All);
    assert_eq!(ids(&r), [1, 2]);
    assert!(r[0].is_exact());
    assert!(!r[1].is_exact(), "'foo' only matches fuzzily in item 2");
}

#[test]
fn newer_items_rank_higher_when_the_match_is_equal() {
    // Listed oldest first on purpose: order comes from the boost, not the input.
    let items = [
        text(1, "meeting notes", 30 * DAY),
        text(2, "meeting notes", DAY),
    ];
    assert_eq!(ids(&ranked(&items, "meeting", TypeFilter::All)), [2, 1]);
}

#[test]
fn the_recency_boost_decays_with_age() {
    let at = |age: i64| boost(&text(1, "x", age), NOW);
    assert!((at(0) - RECENCY_BOOST_MAX).abs() < 1e-9);
    assert!((at(RECENCY_HALF_LIFE_MS as i64) - RECENCY_BOOST_MAX / 2.0).abs() < 1e-9);
    let mut last = f64::MAX;
    for days in [0, 1, 2, 5, 10, 30, 90, 365] {
        let b = at(days * DAY);
        assert!(b < last, "monotonic decay, day {days}");
        assert!(b > 0.0);
        last = b;
    }
    assert!(at(365 * DAY) < 1e-6, "a year old item gets nothing");
}

#[test]
fn use_count_boosts_with_a_logarithm() {
    let used = |n: u32| boost(&with(text(1, "x", 100 * DAY), |i| i.use_count = n), NOW);
    let base = used(0);
    assert!((used(1) - base - USE_COUNT_WEIGHT * 2f64.ln()).abs() < 1e-9);
    assert!((used(9) - base - USE_COUNT_WEIGHT * 10f64.ln()).abs() < 1e-9);
    assert!(used(100) > used(10) && used(10) > used(1));
    // Diminishing returns: the second hundred uses add less than the first ten.
    assert!(used(200) - used(100) < used(10) - used(0));
}

#[test]
fn use_count_orders_otherwise_equal_items() {
    let items = [
        text(1, "release notes", DAY),
        with(text(2, "release notes", DAY), |i| i.use_count = 12),
    ];
    assert_eq!(ids(&ranked(&items, "release", TypeFilter::All)), [2, 1]);
}

#[test]
fn a_stale_now_changes_the_boost_but_never_breaks_ranking() {
    // A is fresh and never used; B is a month old but used a lot.
    let a = text(1, "deploy script", 0);
    let b = with(text(2, "deploy script", 30 * DAY), |i| i.use_count = 9);
    let items = [a, b];
    let rank_at = |now: i64| {
        rank(
            items.iter(),
            &SearchRequest::new("deploy", TypeFilter::All, now),
        )
    };

    // Now: the fresh item's recency (24) beats ~18 points of use count.
    assert_eq!(ids(&rank_at(NOW)), [1, 2]);
    // Two months later nothing is recent; use count decides.
    let later = rank_at(NOW + 60 * DAY);
    assert_eq!(ids(&later), [2, 1]);
    assert!(later.iter().all(|r| r.score().is_finite()));
    // The scores of the same item fall as `now` moves away from it.
    assert!(rank_at(NOW)[0].score() > rank_at(NOW + 60 * DAY)[1].score());
    // A clock behind the item (the store stamps can run ahead of it) is clamped
    // to the maximum boost, not extrapolated.
    let behind = rank_at(NOW - 10 * DAY);
    let at_zero = rank_at(NOW);
    assert!((behind[0].score() - at_zero[0].score()).abs() < 1e-9);
    // Extreme values do not overflow or produce NaN.
    for now in [i64::MIN / 4, 0, i64::MAX / 4] {
        let r = rank_at(now);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|r| r.score().is_finite()));
    }
}

#[test]
fn smart_case_lowercase_ignores_case_and_uppercase_respects_it() {
    let items = [
        text(1, "see the README file", 1),
        text(2, "see the readme file", 2),
        text(3, "Readme.md", 3),
    ];
    assert_eq!(ids(&ranked(&items, "readme", TypeFilter::All)).len(), 3);
    assert_eq!(ids(&ranked(&items, "README", TypeFilter::All)), [1]);
    assert_eq!(ids(&ranked(&items, "Readme", TypeFilter::All)), [3]);
}

#[test]
fn smart_normalization_folds_accents_only_for_plain_queries() {
    let items = [
        text(1, "visit the caf\u{e9} today", 1),
        text(2, "visit the cafe", 2),
    ];
    // A plain query finds both.
    let mut found = ids(&ranked(&items, "cafe", TypeFilter::All));
    found.sort_unstable();
    assert_eq!(found, [1, 2]);
    // An accented query asks for the accent.
    assert_eq!(ids(&ranked(&items, "caf\u{e9}", TypeFilter::All)), [1]);
}

#[test]
fn type_filter_and_prefixes_restrict_the_results() {
    let items = [
        text(1, "invoice notes", 1),
        image(2, "invoice total 42", 2),
        files(3, &["/billing/invoice.pdf"], 3),
    ];
    assert_eq!(ids(&ranked(&items, "invoice", TypeFilter::All)).len(), 3);
    assert_eq!(ids(&ranked(&items, "invoice", TypeFilter::Text)), [1]);
    assert_eq!(ids(&ranked(&items, "invoice", TypeFilter::Images)), [2]);
    assert_eq!(ids(&ranked(&items, "invoice", TypeFilter::Files)), [3]);
    // Typed prefixes are stripped from the query and win over the UI filter.
    assert_eq!(ids(&ranked(&items, "img:invoice", TypeFilter::All)), [2]);
    assert_eq!(ids(&ranked(&items, "img: invoice", TypeFilter::Files)), [2]);
    assert_eq!(ids(&ranked(&items, "file:invoice", TypeFilter::Text)), [3]);
    assert_eq!(ids(&ranked(&items, "txt:invoice", TypeFilter::Images)), [1]);
    // The prefix word itself is not part of the search.
    assert!(ranked(&items, "txt:total", TypeFilter::All).is_empty());
}

#[test]
fn images_match_through_their_ocr_text_and_pending_ones_do_not() {
    let items = [image(1, "", 1), image(2, "meeting agenda", 2)];
    let r = dtos(&items, "agenda", TypeFilter::All);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].id, 2);
    assert_eq!(r[0].match_source, MatchSource::Ocr);
    assert!(r[0].snippet.as_deref().unwrap().contains("agenda"));
    // Pending images still show in the empty query.
    assert_eq!(dtos(&items, "", TypeFilter::All).len(), 2);
}

#[test]
fn match_source_follows_the_kind() {
    let items = [
        text(1, "budget text", 1),
        image(2, "budget scan", 2),
        files(3, &["/x/budget.xlsx"], 3),
    ];
    let r = dtos(&items, "budget", TypeFilter::All);
    let src = |id| r.iter().find(|d| d.id == id).unwrap().match_source;
    assert_eq!(src(1), MatchSource::Text);
    assert_eq!(src(2), MatchSource::Ocr);
    assert_eq!(src(3), MatchSource::File);
}

#[test]
fn indices_are_character_positions_not_bytes() {
    // "caf\u{e9} na\u{ef}ve" has 2 byte chars before the match.
    let items = [text(1, "caf\u{e9} na\u{ef}ve r\u{e9}sum\u{e9} target", 1)];
    let d = &dtos(&items, "target", TypeFilter::All)[0];
    let preview: Vec<char> = d.preview.chars().collect();
    let hl: String = d
        .match_indices
        .iter()
        .map(|&i| preview[i as usize])
        .collect();
    assert_eq!(hl, "target");
    // Byte offsets would be 3 higher (three 2 byte chars before the match).
    assert_eq!(d.match_indices.first(), Some(&18));
    assert_eq!(d.match_indices.len(), 6);
}

#[test]
fn indices_count_code_points_not_grapheme_clusters() {
    // 'a' + combining acute is one grapheme but two chars; a CJK prefix is
    // three bytes per char. Highlighting is by char, so "bc" is at 2 and 3.
    let items = [
        text(1, "a\u{301}bc", 1),
        text(2, "\u{4f60}\u{597d} hello", 2),
    ];
    let r = dtos(&items, "bc", TypeFilter::All);
    assert_eq!(r[0].match_indices, [2, 3]);
    let r = dtos(&items, "hello", TypeFilter::All);
    assert_eq!(r[0].match_indices, [3, 4, 5, 6, 7]);
}

#[test]
fn ocr_snippet_indices_are_character_positions_in_the_snippet() {
    let ocr = format!(
        "{}caf\u{e9} \u{4f60}\u{597d} the zebra crossing is ahead {}",
        "bonjour ".repeat(30),
        "au revoir ".repeat(30)
    );
    let d = &dtos(&[image(1, &ocr, 1)], "zebra", TypeFilter::All)[0];
    let snip: Vec<char> = d.snippet.as_deref().unwrap().chars().collect();
    let hl: String = d.match_indices.iter().map(|&i| snip[i as usize]).collect();
    assert_eq!(hl, "zebra");
}

#[test]
fn a_text_match_beyond_the_preview_gets_a_snippet() {
    let long = format!("{} needle in the haystack", "padding ".repeat(80));
    let item = with(text(1, &long, 1), |i| {
        i.preview = Arc::from(&long[..300]);
    });
    let d = &dtos(&[item], "needle", TypeFilter::All)[0];
    assert_eq!(d.match_source, MatchSource::Text);
    let snip = d
        .snippet
        .as_deref()
        .expect("the preview does not show the match");
    assert!(snip.contains("needle"));
    assert!(snip.chars().count() <= SNIPPET_CHARS);
    let chars: Vec<char> = snip.chars().collect();
    let hl: String = d.match_indices.iter().map(|&i| chars[i as usize]).collect();
    assert_eq!(hl, "needle");
}

#[test]
fn every_index_lies_inside_the_string_it_refers_to() {
    let items = [
        text(1, "caf\u{e9} \u{1f600} na\u{ef}ve a\u{301}bc", 1),
        image(
            2,
            &format!("{}tail \u{4f60}\u{597d} end", "word ".repeat(100)),
            2,
        ),
        files(3, &["C:\\Users\\me\\r\u{e9}sum\u{e9}.pdf"], 3),
        text(4, &"abc ".repeat(200), 4),
    ];
    for q in [
        "a",
        "c",
        "caf\u{e9}",
        "abc",
        "tail",
        "end",
        "r\u{e9}sum\u{e9}",
        "w d",
        "x",
        "\u{4f60}",
    ] {
        for d in dtos(&items, q, TypeFilter::All) {
            let shown = d.snippet.as_deref().unwrap_or(&d.preview);
            let len = shown.chars().count() as u32;
            assert!(
                d.match_indices.iter().all(|&i| i < len),
                "{q:?} on {}: {:?} vs {len}",
                d.id,
                d.match_indices
            );
            let mut sorted = d.match_indices.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted, d.match_indices, "sorted and unique");
        }
    }
}

#[test]
fn the_empty_query_has_no_indices_and_no_snippet() {
    let items = [text(1, "hello", 1), image(2, "ocr words", 2)];
    for d in dtos(&items, "", TypeFilter::All) {
        assert!(d.match_indices.is_empty());
        assert_eq!(d.snippet, None);
    }
}

#[test]
fn at_most_the_limit_is_returned_and_the_best_come_first() {
    let items: Vec<_> = (0..120)
        .map(|i| text(i + 1, "common phrase", (i as i64) * 1000))
        .collect();
    assert_eq!(
        ranked(&items, "common", TypeFilter::All).len(),
        DEFAULT_LIMIT
    );
    assert_eq!(DEFAULT_LIMIT, 50);
    let mut req = SearchRequest::new("common", TypeFilter::All, NOW);
    req.limit = 7;
    assert_eq!(ids(&rank(items.iter(), &req)), [1, 2, 3, 4, 5, 6, 7]);
    req.limit = 0;
    assert!(rank(items.iter(), &req).is_empty());
    req.limit = 1_000_000;
    assert_eq!(rank(items.iter(), &req).len(), MAX_LIMIT.min(120));
    // The empty query obeys the limit too.
    assert_eq!(ranked(&items, "", TypeFilter::All).len(), DEFAULT_LIMIT);
}

#[test]
fn the_limit_keeps_the_best_matches_not_the_first_ones() {
    // 60 weak (fuzzy) matches, newest first, then one exact match, oldest.
    let mut items: Vec<_> = (0..60)
        .map(|i| text(i + 1, "quick brown fox", i as i64))
        .collect();
    items.push(text(99, "...qbf...", 30 * DAY));
    let r = ranked(&items, "qbf", TypeFilter::All);
    assert_eq!(r.len(), DEFAULT_LIMIT);
    assert_eq!(r[0].item().id, 99, "the exact match survives the cut");
}

#[test]
fn ordering_is_stable_and_ties_put_the_newer_item_first() {
    // Identical text and age: only the input (newest first) order differs.
    let items: Vec<_> = (1..=100).map(|i| text(i, "same words here", DAY)).collect();
    let first = ids(&ranked(&items, "same", TypeFilter::All));
    assert_eq!(first, (1..=50).collect::<Vec<u64>>());
    for _ in 0..20 {
        assert_eq!(ids(&ranked(&items, "same", TypeFilter::All)), first);
    }
    // A limit that cuts through a tie keeps the earlier (newer) items.
    let mut req = SearchRequest::new("same", TypeFilter::All, NOW);
    req.limit = 10;
    assert_eq!(
        ids(&rank(items.iter(), &req)),
        (1..=10).collect::<Vec<u64>>()
    );
}

#[test]
fn degenerate_inputs_do_not_panic() {
    let long = "a".repeat(10_000);
    let items = [
        text(1, &long, 1),
        text(2, "", 2),
        text(3, &"\u{4f60}".repeat(10_000), 3),
    ];
    for q in [
        "aaaaab",
        "a",
        "!a",
        "'",
        "^$",
        "\\",
        "\u{4f60}\u{4f60}",
        "a b c d e f g h",
        "\u{0}",
    ] {
        let _ = dtos(&items, q, TypeFilter::All);
    }
    // A query longer than every text.
    assert!(ranked(&items, &"z".repeat(20_000), TypeFilter::All).is_empty());
}

// -- size gate of the fuzzy stage -------------------------------------------------

#[test]
fn scattered_letter_matches_are_limited_to_short_texts() {
    let short = "quick brown fox";
    // Same scattered letters, but inside a text over the gate.
    let long = format!("{short} {}", "lorem ipsum dolor sit amet ".repeat(30));
    assert!(long.len() > FUZZY_MAX_BYTES);
    let items = [text(1, short, 1), text(2, &long, 2)];
    assert_eq!(ids(&ranked(&items, "qbf", TypeFilter::All)), [1]);
    // A contiguous word is found in the long text too.
    assert_eq!(ids(&ranked(&items, "ipsum", TypeFilter::All)), [2]);
    // And so is a contiguous word right at the end of a long text.
    let tail = format!("{} needle", "padding ".repeat(100));
    assert_eq!(
        ids(&ranked(&[text(3, &tail, 3)], "needle", TypeFilter::All)),
        [3]
    );
}

#[test]
fn accent_folding_only_applies_to_short_texts() {
    let long = format!("caf\u{e9} {}", "word ".repeat(100));
    assert!(long.len() > FUZZY_MAX_BYTES);
    let items = [text(1, "caf\u{e9}", 1), text(2, &long, 2)];
    let mut found = ids(&ranked(&items, "cafe", TypeFilter::All));
    found.sort_unstable();
    assert_eq!(found, [1], "the long text needs the exact spelling");
    let mut found = ids(&ranked(&items, "caf\u{e9}", TypeFilter::All));
    found.sort_unstable();
    assert_eq!(found, [1, 2]);
}

#[test]
fn the_lowercase_twin_follows_the_search_text() {
    let mut i = (*text(1, "Hello WORLD", 1)).clone();
    assert_eq!(&*i.search_lower, "hello world");
    i.set_search_text(Arc::from("Changed TEXT"));
    assert_eq!(&*i.search_lower, "changed text");
    // Already lowercase ASCII shares the allocation instead of copying.
    let lower: Arc<str> = Arc::from("already lower");
    assert!(Arc::ptr_eq(&fold_for_search(&lower), &lower));
    // Non ASCII goes through Unicode lowercasing.
    assert_eq!(&*fold_for_search(&Arc::from("\u{c9}COLE")), "\u{e9}cole");
}

#[test]
fn the_byte_search_and_nucleo_agree_on_what_matches() {
    // Plain words use the byte search, anchored ones use nucleo; a mixed query
    // must apply both.
    let items = [text(1, "alpha beta", 1), text(2, "beta alpha", 2)];
    assert_eq!(ids(&ranked(&items, "^alpha beta", TypeFilter::All)), [1]);
    assert_eq!(ids(&ranked(&items, "alpha beta$", TypeFilter::All)), [1]);
    assert_eq!(
        ids(&ranked(&items, "'alpha !gamma", TypeFilter::All)),
        [1, 2]
    );
    assert_eq!(ids(&ranked(&items, "'alp", TypeFilter::All)), [1, 2]);
    assert!(
        ranked(&items, "'alq", TypeFilter::All).is_empty(),
        "a quoted word has no fuzzy fallback"
    );
}
