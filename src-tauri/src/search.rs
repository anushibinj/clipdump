//! Search engine (Phase 8): fuzzy search over the in-memory history.
//!
//! Search takes only the store's read lock, scans every item once and never
//! sends full content to the webview. This file grows over the Phase 8 steps:
//! the matcher and query plan, filters, ranking, then the result DTOs.
//!
//! # Speed rules
//!
//! * One `nucleo_matcher::Matcher` per thread (`thread_local!`): creating one
//!   allocates about 135 KB, which would dwarf the search itself.
//! * The per item loop allocates nothing: the haystack is the item's own
//!   `search_text` (already capped at 10,000 chars), a `Vec<char>` scratch buffer
//!   is reused for the rare non ASCII text, and hits are 24 byte structs.
//! * Smart case and Latin normalization come from nucleo itself
//!   (`CaseMatching::Smart`, `Normalization::Smart`).

// Temporary: the query plan is wired into ranking in the next commits.
#![allow(dead_code)]

use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::sync::Arc;

use serde::Deserialize;

use crate::model::{ClipItem, ClipKind};
use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Per thread reusable matching state.
struct Scratch {
    matcher: Matcher,
    /// Code points of the current haystack when it is not pure ASCII.
    chars: Vec<char>,
    /// Match positions, filled only for the few results that get rendered.
    indices: Vec<u32>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch {
        matcher: Matcher::new(Config::DEFAULT),
        chars: Vec::new(),
        indices: Vec::new(),
    });
}

/// Run `f` with this thread's scratch state.
fn with_scratch<R>(f: impl FnOnce(&mut Scratch) -> R) -> R {
    SCRATCH.with(|s| f(&mut s.borrow_mut()))
}

/// One word of the query, ready to match.
struct Term {
    /// The word as nucleo parsed it (fuzzy, or `'x` `^x` `x$` `!x` syntax).
    atom: Atom,
    /// For a plain fuzzy word: the same word as a contiguous (substring) match.
    /// An item that matches this way counts as an exact match and ranks above
    /// fuzzy-only items.
    exact: Option<Atom>,
}

/// A parsed, non empty query.
struct Query {
    terms: Vec<Term>,
    /// Words that must match (everything except `!negated`).
    positives: usize,
}

impl Query {
    /// `None` when the text has no word (empty or only whitespace).
    fn parse(text: &str) -> Option<Self> {
        // nucleo splits words on a plain space only. A pasted tab or newline
        // would glue words together, so fold other whitespace to single spaces
        // (the borrowed fast path is what a typed query takes).
        let folded: Cow<'_, str> = if text.chars().any(|c| c.is_whitespace() && c != ' ') {
            Cow::Owned(text.split_whitespace().collect::<Vec<_>>().join(" "))
        } else {
            Cow::Borrowed(text)
        };
        let pattern = Pattern::parse(&folded, CaseMatching::Smart, Normalization::Smart);
        if pattern.atoms.is_empty() {
            return None;
        }
        let mut positives = 0;
        let terms = pattern
            .atoms
            .into_iter()
            .map(|atom| {
                let exact = (!atom.negative && atom.kind == AtomKind::Fuzzy).then(|| {
                    let mut twin = atom.clone();
                    twin.kind = AtomKind::Substring;
                    twin
                });
                if !atom.negative {
                    positives += 1;
                }
                Term { atom, exact }
            })
            .collect();
        Some(Self { terms, positives })
    }

    /// Score `hay`: the summed nucleo score of every word and whether all
    /// positive words matched contiguously. `None` when the item does not match
    /// (a word is missing, or a `!negated` word is present).
    ///
    /// A plain word is tried as a substring first: that is memchr accelerated, and
    /// when it hits there is no need for the much costlier fuzzy pass. nucleo
    /// scores a contiguous placement with the same function, so the two scores
    /// are comparable.
    fn score(&self, hay: Utf32Str<'_>, m: &mut Matcher) -> Option<(u32, bool)> {
        let mut total = 0u32;
        let mut exact = self.positives > 0;
        for t in &self.terms {
            if let Some(twin) = &t.exact {
                if let Some(s) = twin.score(hay, m) {
                    total += u32::from(s);
                    continue;
                }
                exact = false;
            }
            // `!negated` words score 0 when absent and `None` when present.
            total += u32::from(t.atom.score(hay, m)?);
        }
        Some((total, exact))
    }
}

/// Which kinds of items a search returns. The UI sends this (`Alt+1` to `Alt+4`);
/// a typed prefix in the query overrides it, see [`parse_query`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypeFilter {
    #[default]
    All,
    Text,
    Images,
    Files,
}

impl TypeFilter {
    pub fn accepts(self, kind: ClipKind) -> bool {
        match self {
            Self::All => true,
            Self::Text => kind == ClipKind::Text,
            Self::Images => kind == ClipKind::Image,
            Self::Files => kind == ClipKind::Files,
        }
    }
}

/// Typed prefixes, matched case insensitively at the start of the query.
const PREFIXES: [(&str, TypeFilter); 3] = [
    ("img:", TypeFilter::Images),
    ("file:", TypeFilter::Files),
    ("txt:", TypeFilter::Text),
];

/// A query split into the text to match and the effective type filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedQuery<'a> {
    /// What is left after the prefix: leading spaces and the prefix are removed.
    pub text: &'a str,
    pub filter: TypeFilter,
}

/// Strip a leading `img:`, `file:` or `txt:` from `raw` and return the filter it
/// names; without a prefix the UI filter `ui` is used. Only one prefix is
/// stripped. `img:` alone (nothing after it) is valid: it lists the images.
/// Allocation free.
///
/// A query that really starts with one of these words (`file:///tmp/x`) is read
/// as a prefix, and the filter lasts until the user edits the query. That is the
/// price of typed filters, which the plan asks for.
pub fn parse_query(raw: &str, ui: TypeFilter) -> ParsedQuery<'_> {
    let trimmed = raw.trim_start();
    for (prefix, filter) in PREFIXES {
        if let Some(head) = trimmed.get(..prefix.len())
            && head.eq_ignore_ascii_case(prefix)
        {
            return ParsedQuery {
                text: trimmed[prefix.len()..].trim_start(),
                filter,
            };
        }
    }
    ParsedQuery {
        text: trimmed,
        filter: ui,
    }
}

/// Most results a search returns unless the caller asks for fewer.
pub const DEFAULT_LIMIT: usize = 50;
/// Upper bound on a caller supplied limit. The UI shows about 8 rows and asks
/// for 50; the cap only stops a bad argument from turning one keystroke into a
/// scan that builds thousands of DTOs.
pub const MAX_LIMIT: usize = 200;

// -- ranking constants (all tuning lives here) --------------------------------
//
// nucleo scores a match in "points": about 16 per matched char, plus bonuses for
// word starts, camelCase and consecutive chars, minus gap penalties. A good
// 5 char match scores roughly 120 to 160; a scattered one 60 to 90. The boosts
// below are small next to that, so they reorder results of similar quality but
// never turn a bad match into the top hit.

/// Recency boost for an item used right now, in nucleo points (about one and a
/// half matched characters).
pub const RECENCY_BOOST_MAX: f64 = 24.0;
/// The recency boost halves every this many ms (3 days): something copied this
/// morning clearly beats last week's copy, while a month old item keeps almost
/// nothing. A decaying curve (not a bucket) so order stays smooth over time.
pub const RECENCY_HALF_LIFE_MS: f64 = 3.0 * 86_400_000.0;
/// Weight of `ln(1 + use_count)`. The plan ranks by that logarithm; the weight
/// puts it on the nucleo point scale: used 10 times is worth about 19 points,
/// 100 times about 37.
pub const USE_COUNT_WEIGHT: f64 = 8.0;
// Exact (contiguous, case insensitive per smart case) matches are not a score
// bonus: they are a separate tier that always sorts above fuzzy-only matches, see
// `Hit::cmp_rank`. That guarantees "exact beats fuzzy" whatever the boosts are.

/// Everything a search needs besides the items.
#[derive(Clone, Copy, Debug)]
pub struct SearchRequest<'a> {
    /// What the user typed, prefix included.
    pub query: &'a str,
    /// The type filter chosen in the UI; a typed prefix overrides it.
    pub filter: TypeFilter,
    /// How many results at most (0 gives none, capped at [`MAX_LIMIT`]).
    pub limit: usize,
    /// "Now" in unix ms, for the recency boost. Injected so tests can move time;
    /// the IPC command passes the store clock.
    pub now_ms: i64,
}

impl<'a> SearchRequest<'a> {
    pub fn new(query: &'a str, filter: TypeFilter, now_ms: i64) -> Self {
        Self {
            query,
            filter,
            limit: DEFAULT_LIMIT,
            now_ms,
        }
    }
}

/// One selected result: a snapshot of the item (a cheap `Arc` clone, so the store
/// lock can be released before the DTOs are built) and why it ranked here.
#[derive(Clone, Debug)]
pub struct Ranked {
    item: Arc<ClipItem>,
    /// Every word of the query occurs as a contiguous substring.
    exact: bool,
    /// Fuzzy score plus boosts; 0 for the empty query.
    score: f64,
}

impl Ranked {
    pub fn item(&self) -> &Arc<ClipItem> {
        &self.item
    }

    pub fn is_exact(&self) -> bool {
        self.exact
    }

    pub fn score(&self) -> f64 {
        self.score
    }
}

/// Pick and order the results. Call it with the store's read guard held (the
/// iterator borrows the items) and release the guard before rendering.
///
/// `items` must be newest first: with equal rank, the earlier item wins, so ties
/// always put the newer item on top.
pub fn rank<'a>(
    items: impl Iterator<Item = &'a Arc<ClipItem>>,
    req: &SearchRequest<'_>,
) -> Vec<Ranked> {
    let limit = req.limit.min(MAX_LIMIT);
    if limit == 0 {
        return Vec::new();
    }
    let parsed = parse_query(req.query, req.filter);
    match Query::parse(parsed.text) {
        None => rank_recent(items, parsed.filter, limit),
        Some(q) => with_scratch(|sc| rank_matches(items, &q, parsed.filter, limit, req.now_ms, sc)),
    }
}

/// A matching item during selection. Small and `Copy`-like on purpose: a
/// search pushes one per matching item.
struct Hit<'a> {
    item: &'a Arc<ClipItem>,
    /// Position in the newest first scan: the final, unique tie breaker.
    order: u32,
    exact: bool,
    score: f64,
}

impl Hit<'_> {
    /// `Less` means `self` ranks first: exact tier, then score (highest), then
    /// newest. `order` is unique, so this is a total order and an unstable sort
    /// still gives a stable, reproducible result.
    fn cmp_rank(&self, other: &Self) -> Ordering {
        other
            .exact
            .cmp(&self.exact)
            .then_with(|| other.score.total_cmp(&self.score))
            .then_with(|| self.order.cmp(&other.order))
    }
}

/// The recency and use count boost of one item.
fn boost(item: &ClipItem, now_ms: i64) -> f64 {
    // A clock that ran backwards (or the store's stamps running a few ms ahead
    // of it) must not give more than the maximum.
    let age = (now_ms - item.last_used_at).max(0) as f64;
    let recency = RECENCY_BOOST_MAX * (-age / RECENCY_HALF_LIFE_MS).exp2();
    let uses = if item.use_count == 0 {
        0.0
    } else {
        USE_COUNT_WEIGHT * f64::from(item.use_count).ln_1p()
    };
    recency + uses
}

/// A non empty query: score every item that passes the type filter, keep the
/// best `limit`.
fn rank_matches<'a>(
    items: impl Iterator<Item = &'a Arc<ClipItem>>,
    q: &Query,
    filter: TypeFilter,
    limit: usize,
    now_ms: i64,
    sc: &mut Scratch,
) -> Vec<Ranked> {
    let mut hits: Vec<Hit<'a>> = Vec::with_capacity(64);
    for (order, item) in items.enumerate() {
        if !filter.accepts(item.kind) {
            continue;
        }
        let text: &str = &item.search_text;
        // An image whose OCR has not run has no text. (A query of only negated
        // words matches everything, so only skip when a word must match.)
        if text.is_empty() && q.positives > 0 {
            continue;
        }
        let hay = Utf32Str::new(text, &mut sc.chars);
        let Some((fuzzy, exact)) = q.score(hay, &mut sc.matcher) else {
            continue;
        };
        hits.push(Hit {
            item,
            order: order as u32,
            exact,
            score: f64::from(fuzzy) + boost(item, now_ms),
        });
    }
    if hits.len() > limit {
        // O(n) partition, then sort only the winners.
        hits.select_nth_unstable_by(limit, Hit::cmp_rank);
        hits.truncate(limit);
    }
    hits.sort_unstable_by(Hit::cmp_rank);
    hits.into_iter()
        .map(|h| Ranked {
            item: Arc::clone(h.item),
            exact: h.exact,
            score: h.score,
        })
        .collect()
}

/// The empty query: pinned items first, then the rest, each group newest first.
/// One pass over the items that stops early once the pinned group fills the
/// limit.
fn rank_recent<'a>(
    items: impl Iterator<Item = &'a Arc<ClipItem>>,
    filter: TypeFilter,
    limit: usize,
) -> Vec<Ranked> {
    let mut pinned: Vec<&Arc<ClipItem>> = Vec::new();
    let mut recent: Vec<&Arc<ClipItem>> = Vec::with_capacity(limit);
    for item in items.filter(|i| filter.accepts(i.kind)) {
        if item.pinned {
            pinned.push(item);
            if pinned.len() == limit {
                break;
            }
        } else if recent.len() < limit {
            recent.push(item);
        }
    }
    pinned
        .into_iter()
        .chain(recent)
        .take(limit)
        .map(|item| Ranked {
            item: Arc::clone(item),
            exact: false,
            score: 0.0,
        })
        .collect()
}

#[cfg(test)]
mod tests;
