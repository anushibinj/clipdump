//! `get_preview`: what the Tab pane shows. Everything is capped in Rust, so the
//! webview never receives megabytes (Section 3: very long text is truncated
//! here, not in the UI).

use serde::Serialize;

use crate::model::{ClipKind, OcrStatus};
use crate::search::PathChecker;
use crate::store::Store;

/// Most UTF-8 bytes of text (or OCR text) a preview carries: 200 KB.
pub const PREVIEW_TEXT_MAX_BYTES: usize = 200 * 1024;
/// Most bytes the same text may take once escaped as a JSON string (control
/// characters become six bytes, newlines and quotes two), 256 KB. Whichever of the
/// two limits is hit first cuts the text, so the serialized `PreviewDto` stays
/// under 300 KB whatever the text holds, not just for plain prose.
pub const PREVIEW_TEXT_MAX_JSON_BYTES: usize = 256 * 1024;
/// Most files a preview lists. A copied folder tree can hold tens of thousands.
pub const PREVIEW_FILES_MAX: usize = 500;
/// Most bytes of one displayed path and of one displayed name. Windows allows
/// paths of 32,767 characters; nobody needs to read past this.
pub const PREVIEW_PATH_MAX_BYTES: usize = 2048;
pub const PREVIEW_NAME_MAX_BYTES: usize = 512;
/// The listed paths and names together take at most this many bytes (the file
/// list is cut short when they would exceed it), so the serialized files preview
/// stays under 300 KB even for 500 very long paths.
pub const PREVIEW_FILES_MAX_BYTES: usize = 200 * 1024;

/// One row of a file list preview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub path: String,
    /// The last path component, for display.
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    /// Whether the path exists right now (so the pane can show "missing").
    pub exists: bool,
}

/// The preview of one item. Tagged by `kind` (`"text"`, `"image"`, `"files"`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum PreviewDto {
    Text {
        id: u64,
        /// At most [`PREVIEW_TEXT_MAX_BYTES`] bytes, cut on a character boundary.
        text: String,
        truncated: bool,
        /// Characters (code points) of the whole text, not of `text`.
        total_chars: usize,
        byte_size: u64,
    },
    Image {
        id: u64,
        width: u32,
        height: u32,
        /// PNG size in bytes.
        byte_size: u64,
        /// The recognized text RAM keeps (the first 10,000 chars; the rest is
        /// only searched in the database). Empty until OCR has finished.
        ocr_text: String,
        ocr_truncated: bool,
        ocr_status: OcrStatus,
    },
    Files {
        id: u64,
        /// The first [`PREVIEW_FILES_MAX`] entries.
        files: Vec<FilePreview>,
        total_files: usize,
        truncated: bool,
        /// Sum of the sizes of all files (not just the listed ones).
        total_size: u64,
    },
}

/// `s` cut to at most `max` bytes on a char boundary, and whether it was cut.
pub fn truncate_utf8(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    (&s[..cut], true)
}

/// Bytes `c` takes inside a JSON string (what `serde_json` writes).
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// `s` cut for a preview: at most [`PREVIEW_TEXT_MAX_BYTES`] UTF-8 bytes and at
/// most [`PREVIEW_TEXT_MAX_JSON_BYTES`] once JSON escaped, on a char boundary.
/// Walks at most the kept part (about 200 KB), never the whole of a huge text.
pub fn truncate_preview(s: &str) -> (&str, bool) {
    let (cut, truncated) = truncate_utf8(s, PREVIEW_TEXT_MAX_BYTES);
    let mut json = 0;
    for (i, c) in cut.char_indices() {
        json += json_len(c);
        if json > PREVIEW_TEXT_MAX_JSON_BYTES {
            return (&cut[..i], true);
        }
    }
    (cut, truncated)
}

/// `s` cut to `max` bytes on a char boundary with a trailing ellipsis when cut.
fn shorten(s: &str, max: usize) -> String {
    let (kept, cut) = truncate_utf8(s, max);
    if cut {
        format!("{kept}\u{2026}")
    } else {
        kept.to_owned()
    }
}

/// The last component of a path written with `\` or `/`.
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['\\', '/']);
    let name = trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed);
    if name.is_empty() { path } else { name }
}

/// Build the preview of item `id`. Text loads from RAM or (large texts) from
/// the cache or SQLite, which is fine: it is one user action, not a keystroke.
pub fn preview_item(store: &Store, paths: &dyn PathChecker, id: u64) -> Result<PreviewDto, String> {
    let item = store.get(id).ok_or("item not found")?;
    match item.kind {
        ClipKind::Text => {
            let bytes = store
                .get_full_text(id)
                .map_err(|_| "could not load the text")?;
            let text = std::str::from_utf8(&bytes).map_err(|_| "could not load the text")?;
            let (shown, truncated) = truncate_preview(text);
            Ok(PreviewDto::Text {
                id,
                text: shown.to_owned(),
                truncated,
                total_chars: text.chars().count(),
                byte_size: item.byte_size,
            })
        }
        ClipKind::Image => {
            let (ocr, ocr_truncated) = truncate_preview(&item.search_text);
            Ok(PreviewDto::Image {
                id,
                width: item.width,
                height: item.height,
                byte_size: item.byte_size,
                ocr_text: ocr.to_owned(),
                ocr_truncated,
                ocr_status: item.ocr_status,
            })
        }
        ClipKind::Files => {
            let all = item.files.as_deref().unwrap_or(&[]);
            let mut budget = PREVIEW_FILES_MAX_BYTES;
            let mut files = Vec::new();
            for f in all.iter().take(PREVIEW_FILES_MAX) {
                let path = shorten(&f.path, PREVIEW_PATH_MAX_BYTES);
                let name = shorten(file_name(&f.path), PREVIEW_NAME_MAX_BYTES);
                let cost = path.len() + name.len();
                if cost > budget {
                    break;
                }
                budget -= cost;
                files.push(FilePreview {
                    path,
                    name,
                    size: f.size,
                    is_dir: f.is_dir,
                    exists: paths.exists(&f.path),
                });
            }
            let listed = files.len();
            Ok(PreviewDto::Files {
                id,
                files,
                total_files: all.len(),
                truncated: all.len() > listed,
                total_size: all.iter().map(|f| f.size).sum(),
            })
        }
    }
}
