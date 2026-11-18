//! Search benchmark (Phase 8). Budget (Section 4): under 1 ms for 1,000 items.
//!
//! Run from `src-tauri`:
//!
//! ```text
//! cargo bench --bench search_bench
//! cargo bench --bench search_bench -- 1000     # only the 1,000 item groups
//! ```
//!
//! The histories are synthetic and deterministic (a seeded generator, no I/O):
//! short text, URLs, code, paragraphs, images with long OCR text, images still
//! waiting for OCR, and file lists, a few percent of them non ASCII. For each
//! size and query two things are timed:
//!
//! * `rank`: the scan under the store's read lock, which is the "search itself"
//!   of the budget.
//! * `full`: `rank` plus building the (at most 50) result DTOs: highlight
//!   indices, snippets and the file existence check (a stub that never touches
//!   the disk).
//!
//! The items sit in a `Vec`; the store walks a `BTreeMap`, which costs a few
//! microseconds more per 1,000 items (see the store module docs).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use clipdump_lib::model::{ClipItem, ClipKind, FileRef, OcrStatus, fold_for_search};
use clipdump_lib::search::{PathChecker, SearchRequest, TypeFilter, rank, search};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

const SEED: u64 = 0x5EA2_C4D0_0000_0001;
const NOW: i64 = 1_800_000_000_000;

/// Every path exists; keeps the benchmark off the disk.
struct AllExist;

impl PathChecker for AllExist {
    fn exists(&self, _path: &str) -> bool {
        true
    }
}

/// splitmix64: tiny, fast, good enough, and identical on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..hi`.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next() % (hi - lo) as u64) as usize
    }

    fn pick<'a>(&mut self, list: &[&'a str]) -> &'a str {
        list[self.range(0, list.len())]
    }
}

const WORDS: &[&str] = &[
    "the",
    "report",
    "invoice",
    "total",
    "meeting",
    "notes",
    "kubernetes",
    "cluster",
    "deploy",
    "release",
    "config",
    "parser",
    "token",
    "request",
    "response",
    "server",
    "client",
    "database",
    "query",
    "index",
    "search",
    "result",
    "error",
    "warning",
    "build",
    "test",
    "commit",
    "branch",
    "merge",
    "review",
    "design",
    "layout",
    "button",
    "window",
    "screen",
    "image",
    "file",
    "folder",
    "password",
    "account",
    "payment",
    "order",
    "customer",
    "address",
    "phone",
    "email",
    "schedule",
    "project",
    "budget",
    "summary",
    "quarter",
    "revenue",
    "forecast",
    "meeting",
    "agenda",
    "action",
    "item",
    "owner",
    "deadline",
    "priority",
    "status",
    "update",
    "version",
    "feature",
    "bug",
    "fix",
    "patch",
    "memory",
    "thread",
    "async",
    "buffer",
    "stream",
    "socket",
    "network",
    "latency",
    "throughput",
    "cache",
    "queue",
    "worker",
    "timer",
    "event",
    "handler",
    "service",
    "module",
    "package",
    "library",
    "import",
    "export",
    "function",
    "method",
    "class",
    "struct",
    "trait",
    "enum",
    "value",
    "string",
    "number",
    "array",
    "object",
    "logging",
    "metrics",
    "alert",
    "dashboard",
    "README",
    "Readme",
    "TODO",
    "FIXME",
    "Clipboard",
    "Windows",
    "Linux",
    "macOS",
];

/// A few non ASCII words so the `Vec<char>` path is exercised.
const UNICODE_WORDS: &[&str] = &[
    "caf\u{e9}",
    "na\u{ef}ve",
    "\u{fc}ber",
    "se\u{f1}or",
    "\u{4f60}\u{597d}",
    "\u{65e5}\u{672c}\u{8a9e}",
    "\u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}",
    "\u{1f600}smile",
    "r\u{e9}sum\u{e9}",
];

const HOSTS: &[&str] = &[
    "github.com",
    "docs.rs",
    "example.org",
    "news.ycombinator.com",
    "stackoverflow.com",
];

fn words(r: &mut Rng, n: usize, unicode: bool) -> String {
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            s.push(' ');
        }
        if unicode && r.range(0, 8) == 0 {
            s.push_str(r.pick(UNICODE_WORDS));
        } else {
            s.push_str(r.pick(WORDS));
        }
    }
    s
}

fn code(r: &mut Rng) -> String {
    let mut s = String::new();
    for _ in 0..r.range(5, 25) {
        let (a, b) = (r.pick(WORDS), r.pick(WORDS));
        s.push_str(&format!(
            "    let {a}_{b} = self.{a}.{b}(&ctx)?; // {}\n",
            r.pick(WORDS)
        ));
    }
    format!(
        "fn {}_{}(ctx: &Context) -> Result<()> {{\n{s}}}\n",
        r.pick(WORDS),
        r.pick(WORDS)
    )
}

/// Capped like the real search text (10,000 chars).
fn cap(mut s: String) -> String {
    if let Some((i, _)) = s.char_indices().nth(10_000) {
        s.truncate(i);
    }
    s
}

/// One line of at most 300 chars with whitespace collapsed, like the ingest preview.
fn preview(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn item(
    id: u64,
    kind: ClipKind,
    last_used_at: i64,
    use_count: u32,
    pinned: bool,
    preview: String,
    search_text: String,
    files: Option<Vec<FileRef>>,
) -> Arc<ClipItem> {
    let is_image = kind == ClipKind::Image;
    let search_text: Arc<str> = Arc::from(search_text);
    Arc::new(ClipItem {
        id,
        hash: u128::from(id),
        kind,
        created_at: last_used_at,
        last_used_at,
        use_count,
        pinned,
        byte_size: search_text.len() as u64,
        preview: Arc::from(preview),
        search_lower: fold_for_search(&search_text),
        search_text,
        text_in_ram: None,
        thumb: is_image.then(|| Arc::from(vec![0u8; 8])),
        files: files.map(Arc::from),
        width: if is_image { 1920 } else { 0 },
        height: if is_image { 1080 } else { 0 },
        ocr_status: OcrStatus::Done,
        source_app: None,
        blob_hash: None,
    })
}

/// `n` items, newest first, like the store iterates.
fn history(n: usize) -> Vec<Arc<ClipItem>> {
    let mut r = Rng(SEED);
    let mut t = NOW;
    (0..n as u64)
        .map(|id| {
            // 1 to 90 minutes between copies: a 1,000 item history spans weeks.
            t -= r.range(60_000, 5_400_000) as i64;
            let uses = if r.range(0, 5) == 0 {
                r.range(1, 40) as u32
            } else {
                0
            };
            let pinned = r.range(0, 100) == 0;
            let unicode = r.range(0, 20) == 0;
            let roll = r.range(0, 100);
            let (kind, text, files) = match roll {
                0..=37 => {
                    let n = r.range(2, 9);
                    (ClipKind::Text, words(&mut r, n, unicode), None)
                }
                38..=52 => {
                    let host = r.pick(HOSTS);
                    let url = format!(
                        "https://{host}/{}/{}-{}?id={}",
                        r.pick(WORDS),
                        r.pick(WORDS),
                        r.pick(WORDS),
                        r.range(0, 99_999)
                    );
                    (ClipKind::Text, url, None)
                }
                53..=67 => (ClipKind::Text, code(&mut r), None),
                68..=79 => {
                    let n = r.range(60, 400);
                    (ClipKind::Text, words(&mut r, n, unicode), None)
                }
                80..=89 => {
                    // OCR text keeps line breaks like the recognizer returns them.
                    let lines = r.range(30, 160);
                    let mut s = String::new();
                    for _ in 0..lines {
                        let n = r.range(3, 14);
                        s.push_str(&words(&mut r, n, unicode));
                        s.push('\n');
                    }
                    (ClipKind::Image, s, None)
                }
                90..=96 => (ClipKind::Image, String::new(), None), // OCR pending
                _ => {
                    let list: Vec<FileRef> = (0..r.range(1, 6))
                        .map(|_| FileRef {
                            path: format!(
                                "C:\\Users\\me\\{}\\{}_{}.{}",
                                r.pick(WORDS),
                                r.pick(WORDS),
                                r.pick(WORDS),
                                r.pick(&["pdf", "txt", "png", "docx"])
                            ),
                            size: 1024,
                            is_dir: false,
                        })
                        .collect();
                    let joined = list
                        .iter()
                        .map(|f| f.path.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    (ClipKind::Files, joined, Some(list))
                }
            };
            let text = cap(text);
            let pv = match kind {
                ClipKind::Image => "Image 1920x1080".to_string(),
                ClipKind::Files => files
                    .as_ref()
                    .map(|l| {
                        l.iter()
                            .map(|f| f.path.rsplit('\\').next().unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
                ClipKind::Text => preview(&text),
            };
            item(id, kind, t, uses, pinned, pv, text, files)
        })
        .collect()
}

/// (label, query). The first group is what a typical keystroke looks like.
const QUERIES: &[(&str, &str)] = &[
    ("empty", ""),
    ("1char", "e"),
    ("common_word", "report"),
    ("rare_word", "kubernetes"),
    ("two_words", "invoice total"),
    ("code_fuzzy", "fn parse"),
    ("typo_fuzzy", "kubrnts"),
    ("smart_case", "README"),
    ("url", "https github"),
    ("unicode", "caf\u{e9}"),
    ("images_only", "img:invoice"),
    ("no_match", "zzqxj"),
];

fn bench_search(c: &mut Criterion) {
    for n in [1_000usize, 10_000] {
        let items = history(n);
        let mut group = c.benchmark_group(format!("search_{n}"));
        for (label, query) in QUERIES {
            let req = SearchRequest::new(query, TypeFilter::All, NOW);
            group.bench_with_input(BenchmarkId::new("rank", label), &req, |b, req| {
                b.iter(|| black_box(rank(black_box(items.iter()), req)));
            });
            group.bench_with_input(BenchmarkId::new("full", label), &req, |b, req| {
                b.iter(|| black_box(search(black_box(items.iter()), req, &AllExist)));
            });
        }
        group.finish();
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(50);
    targets = bench_search
}
criterion_main!(benches);
