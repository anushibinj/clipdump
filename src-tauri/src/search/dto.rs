//! The result rows sent to the webview. Never full content: the preview is one
//! line of at most 300 chars and the snippet about 100.

use serde::Serialize;

use super::paths::{PathChecker, count_missing};
use super::{Query, Ranked, Scratch, SearchRequest, parse_query, with_scratch};
use crate::model::{ClipItem, ClipKind};
use nucleo_matcher::Utf32Str;

/// Characters in a snippet at most.
pub const SNIPPET_CHARS: usize = 96;
/// How many characters before the first match a snippet starts.
const SNIPPET_LEAD: usize = 24;

/// Where the matched words were found. One value per item kind: text items
/// match their text, images their OCR text, file items their paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchSource {
    Text,
    Ocr,
    File,
}

/// One row of the result list (serialized as camelCase JSON).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultDto {
    pub id: u64,
    /// `"text"`, `"image"` or `"files"`.
    pub kind: ClipKind,
    /// One line, 300 chars at most.
    pub preview: String,
    /// A short window of the matched text, set when the match is not visible in
    /// the preview: always for OCR matches (the subtitle under the image), and
    /// for text or file matches that sit beyond the preview. The same text with
    /// whitespace collapsed; no ellipsis is added (the UI can add one).
    pub snippet: Option<String>,
    /// Sorted, unique **character** (code point) positions to highlight, in the
    /// `snippet` when there is one, otherwise in the `preview`. Empty for the
    /// empty query.
    pub match_indices: Vec<u32>,
    pub match_source: MatchSource,
    /// Unix ms.
    pub created_at: i64,
    pub use_count: u32,
    pub pinned: bool,
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
    /// Number of paths of a file item, else 0.
    pub file_count: u32,
    /// How many paths of a file item no longer exist (a lower bound for lists
    /// longer than `MISSING_CHECK_MAX`), else 0.
    pub missing_files: u32,
    /// Whether `clip://thumb/<id>` has a picture.
    pub has_thumb: bool,
}

/// Build the DTOs of ranked results. Takes no store lock: `ranked` holds its own
/// snapshots. The only I/O is the existence check of file items, which goes
/// through `paths` (cached) for these few items only.
pub fn render(
    ranked: &[Ranked],
    req: &SearchRequest<'_>,
    paths: &dyn PathChecker,
) -> Vec<ResultDto> {
    let parsed = parse_query(req.query, req.filter);
    let query = Query::parse(parsed.text);
    with_scratch(|sc| {
        ranked
            .iter()
            .map(|r| to_dto(r.item(), query.as_ref(), paths, sc))
            .collect()
    })
}

fn to_dto(
    item: &ClipItem,
    query: Option<&Query>,
    paths: &dyn PathChecker,
    sc: &mut Scratch,
) -> ResultDto {
    let (snippet, match_indices) = match query {
        Some(q) => highlight(item, q, sc),
        None => (None, Vec::new()),
    };
    let (file_count, missing_files) = match (&item.files, item.kind) {
        (Some(files), ClipKind::Files) => (files.len() as u32, count_missing(files, paths)),
        _ => (0, 0),
    };
    ResultDto {
        id: item.id,
        kind: item.kind,
        preview: item.preview.to_string(),
        snippet,
        match_indices,
        match_source: match item.kind {
            ClipKind::Text => MatchSource::Text,
            ClipKind::Image => MatchSource::Ocr,
            ClipKind::Files => MatchSource::File,
        },
        created_at: item.created_at,
        use_count: item.use_count,
        pinned: item.pinned,
        width: item.width,
        height: item.height,
        byte_size: item.byte_size,
        file_count,
        missing_files,
        has_thumb: item.thumb.is_some(),
    }
}

/// The snippet (if any) and the highlight positions, computed against the string
/// the UI will show: the preview for text and file items, the snippet otherwise.
fn highlight(item: &ClipItem, q: &Query, sc: &mut Scratch) -> (Option<String>, Vec<u32>) {
    // Text and files: highlight in the preview when the preview shows the match.
    if item.kind != ClipKind::Image {
        let idx = indices_in(q, &item.preview, sc);
        if !idx.is_empty() {
            return (None, idx);
        }
    }
    // OCR text, or a match that lies beyond the preview: show a window of the
    // searched text around the first match and highlight inside that window.
    let text: &str = &item.search_text;
    let Some(pos) = q.first_hit(item).or_else(|| first_match(q, text, sc)) else {
        return (None, Vec::new());
    };
    let snippet = snippet_around(text, pos);
    let idx = indices_in(q, &snippet, sc);
    (Some(snippet), idx)
}

/// Character positions of every positive word of `q` in `hay`, sorted and
/// unique. A word that does not match `hay` simply adds nothing, so a partial
/// match still highlights what it can. Positions are always below the length of
/// `hay` in characters (the final `retain` is a guard, not a fix up).
fn indices_in(q: &Query, hay: &str, sc: &mut Scratch) -> Vec<u32> {
    let Scratch {
        matcher,
        chars,
        indices,
    } = sc;
    indices.clear();
    let hay = Utf32Str::new(hay, chars);
    for t in q.terms.iter().filter(|t| !t.atom.negative) {
        let before = indices.len();
        // One contiguous run reads better than scattered letters, so try the
        // substring form first. nucleo leaves `indices` untouched on a miss, the
        // truncates are for safety.
        let found = t
            .exact
            .as_ref()
            .is_some_and(|twin| twin.indices(hay, matcher, indices).is_some());
        if !found {
            indices.truncate(before);
            if t.atom.indices(hay, matcher, indices).is_none() {
                indices.truncate(before);
            }
        }
    }
    indices.sort_unstable();
    indices.dedup();
    let len = hay.len() as u32;
    indices.retain(|&i| i < len);
    indices.clone()
}

/// Character position where the first positive word matches in `text`.
fn first_match(q: &Query, text: &str, sc: &mut Scratch) -> Option<usize> {
    let t = q.terms.iter().find(|t| !t.atom.negative)?;
    let Scratch {
        matcher,
        chars,
        indices,
    } = sc;
    indices.clear();
    let hay = Utf32Str::new(text, chars);
    let found = t
        .exact
        .as_ref()
        .is_some_and(|twin| twin.indices(hay, matcher, indices).is_some());
    if !found {
        indices.clear();
        t.atom.indices(hay, matcher, indices)?;
    }
    indices.iter().min().map(|&i| i as usize)
}

/// A window of `text` that starts [`SNIPPET_LEAD`] chars before character `pos`
/// (not in the middle of a word when avoidable), is at most [`SNIPPET_CHARS`]
/// long, and has every whitespace run collapsed to one space.
fn snippet_around(text: &str, pos: usize) -> String {
    let start = pos.saturating_sub(SNIPPET_LEAD);
    // One char before the window tells whether it starts at a word start.
    let from = start.saturating_sub(1);
    let mut window: Vec<char> = text
        .chars()
        .skip(from)
        .take(SNIPPET_CHARS + 1 + 16)
        .collect();
    if start > 0 && !window.is_empty() {
        let prev_is_space = window.remove(0).is_whitespace();
        if !prev_is_space && window.first().is_some_and(|c| !c.is_whitespace()) {
            // Starts inside a word: skip the rest of it, but never past the match.
            let limit = pos - start;
            let skip = window
                .iter()
                .take(limit)
                .position(|c| c.is_whitespace())
                .map(|p| p + 1);
            if let Some(skip) = skip {
                window.drain(..skip);
            }
        }
    }
    let mut out = String::with_capacity(SNIPPET_CHARS);
    let mut n = 0;
    let mut pending_space = false;
    for c in window {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if n >= SNIPPET_CHARS {
            break;
        }
        if pending_space {
            if n + 2 > SNIPPET_CHARS {
                break;
            }
            out.push(' ');
            n += 1;
            pending_space = false;
        }
        out.push(c);
        n += 1;
    }
    out
}
