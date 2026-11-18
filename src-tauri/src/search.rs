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

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

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
}

#[cfg(test)]
mod tests;
