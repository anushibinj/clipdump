//! Text items: line ending normalization, hash, preview and search text.
//!
//! All of it is a single pass over the string or less, so a 5 MB paste costs a
//! few milliseconds on a worker thread.

use std::borrow::Cow;

use super::IngestedItem;
use crate::hash::hash_text;
use crate::model::{ClipKind, OcrStatus};

/// Characters kept in the one line preview.
pub const PREVIEW_CHARS: usize = 300;
/// Characters of text that fuzzy search looks at (Section 3: RAM holds the first
/// 10,000 chars of every text item).
pub const SEARCH_CHARS: usize = 10_000;

/// `\r\n` to `\n`. A lone `\r` is kept, exactly like `hash_text`, so the stored
/// text hashes to the same value as the clipboard original. Borrows when there
/// is nothing to change, which is the common case for text copied on Linux and
/// macOS and for single line text everywhere.
pub fn normalize_newlines(text: &str) -> Cow<'_, str> {
    if !text.contains("\r\n") {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n"))
}

/// First [`PREVIEW_CHARS`] characters with every whitespace run (newlines and
/// tabs included) collapsed to one space and no leading or trailing space.
/// Stops reading as soon as the preview is full, so it never walks a huge text.
pub fn preview(text: &str) -> String {
    let mut out = String::with_capacity(PREVIEW_CHARS.min(text.len()));
    let mut chars = 0;
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            // A space needs a following character, or it would dangle at the end.
            if chars + 2 > PREVIEW_CHARS {
                break;
            }
            out.push(' ');
            chars += 1;
            pending_space = false;
        }
        out.push(c);
        chars += 1;
        if chars >= PREVIEW_CHARS {
            break;
        }
    }
    out
}

/// The part of `text` that search indexes: its first [`SEARCH_CHARS`] chars.
pub fn search_slice(text: &str) -> &str {
    match text.char_indices().nth(SEARCH_CHARS) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// Build the finished item. `None` for empty text (nothing worth keeping).
pub fn build(text: &str, source_app: Option<String>, created_at: i64) -> Option<IngestedItem> {
    if text.is_empty() {
        return None;
    }
    let normalized = normalize_newlines(text);
    // `hash_text` folds CRLF itself, so hashing the normalized text gives the
    // same value the watcher computes from the raw clipboard string.
    let hash = hash_text(&normalized);
    Some(IngestedItem {
        kind: ClipKind::Text,
        hash,
        preview: preview(&normalized),
        search_text: search_slice(&normalized).to_string(),
        byte_size: normalized.len() as u64,
        text: Some(normalized.into_owned()),
        png: None,
        thumb: None,
        files: None,
        width: 0,
        height: 0,
        source_app,
        created_at,
        ocr_status: OcrStatus::Done,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_becomes_lf_and_lone_cr_stays() {
        assert_eq!(normalize_newlines("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(normalize_newlines("a\rb"), "a\rb");
        assert!(matches!(normalize_newlines("plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn preview_collapses_whitespace_and_trims() {
        assert_eq!(preview("  hello \r\n\t world  \n\n x "), "hello world x");
        assert_eq!(preview(""), "");
        assert_eq!(preview(" \n\t "), "");
    }

    #[test]
    fn preview_is_at_most_300_chars_counted_in_chars_not_bytes() {
        let long = "a".repeat(1000);
        assert_eq!(preview(&long).chars().count(), 300);
        let wide = "é ".repeat(500);
        let p = preview(&wide);
        // 300 chars would end on a space, which is dropped.
        assert_eq!(p.chars().count(), 299);
        assert!(!p.ends_with(' '));
        let emoji = "😀".repeat(400);
        assert_eq!(preview(&emoji).chars().count(), 300);
    }

    #[test]
    fn preview_never_ends_or_starts_with_space() {
        // The space that would land exactly at the limit must not be emitted
        // dangling.
        let t = format!("{} b", "a".repeat(299));
        let p = preview(&t);
        assert!(!p.ends_with(' '), "{p:?}");
        assert!(p.chars().count() <= 300);
    }

    #[test]
    fn search_slice_caps_at_10000_chars() {
        let t = "é".repeat(SEARCH_CHARS + 5);
        assert_eq!(search_slice(&t).chars().count(), SEARCH_CHARS);
        assert_eq!(search_slice("short"), "short");
        let exact = "x".repeat(SEARCH_CHARS);
        assert_eq!(search_slice(&exact).len(), SEARCH_CHARS);
    }

    #[test]
    fn build_uses_hash_text_and_normalizes() {
        let a = build("one\r\ntwo", Some("notepad.exe".into()), 5).unwrap();
        let b = build("one\ntwo", None, 6).unwrap();
        assert_eq!(a.hash, b.hash, "CRLF and LF must dedup");
        assert_eq!(a.hash, hash_text("one\r\ntwo"));
        assert_eq!(a.hash, hash_text("one\ntwo"));
        assert_eq!(a.text.as_deref(), Some("one\ntwo"));
        assert_eq!(a.preview, "one two");
        assert_eq!(a.search_text, "one\ntwo");
        assert_eq!(a.byte_size, 7);
        assert_eq!(a.source_app.as_deref(), Some("notepad.exe"));
        assert_eq!(a.kind, ClipKind::Text);
        assert_eq!(a.ocr_status, OcrStatus::Done);
        assert_ne!(a.hash, build("other", None, 0).unwrap().hash);
    }

    #[test]
    fn empty_text_is_dropped_whitespace_only_is_kept() {
        assert!(build("", None, 0).is_none());
        let ws = build("   ", None, 0).unwrap();
        assert_eq!(ws.preview, "");
    }

    #[test]
    fn large_text_keeps_full_text_but_capped_search_text() {
        let big = "word ".repeat(100_000);
        let it = build(&big, None, 0).unwrap();
        assert_eq!(it.text.as_ref().unwrap().len(), big.len());
        assert_eq!(it.search_text.chars().count(), SEARCH_CHARS);
    }
}
