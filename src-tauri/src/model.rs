//! Shared data types.
//!
//! The small value types the persistence layer stores, plus [`ClipItem`], the
//! in-memory form of one history entry (Section 6, Phase 6). The numeric codes
//! of [`ClipKind`] and [`OcrStatus`] are part of the on-disk format and must
//! not change.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// What an item holds. The discriminant is stored in `items.kind`. Serialized
/// to the webview as `"text"`, `"image"` or `"files"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum ClipKind {
    Text = 0,
    Image = 1,
    Files = 2,
}

impl ClipKind {
    pub fn code(self) -> i64 {
        self as i64
    }

    pub fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Text),
            1 => Some(Self::Image),
            2 => Some(Self::Files),
            _ => None,
        }
    }
}

/// Progress of OCR for an image item. Stored in `items.ocr_status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum OcrStatus {
    Pending = 0,
    Done = 1,
    Failed = 2,
    /// No OCR engine or language pack on this machine.
    Unavailable = 3,
}

impl OcrStatus {
    pub fn code(self) -> i64 {
        self as i64
    }

    pub fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Pending),
            1 => Some(Self::Done),
            2 => Some(Self::Failed),
            3 => Some(Self::Unavailable),
            _ => None,
        }
    }
}

/// One entry of a copied file list. Serialized as JSON into `files_enc`.
/// Paths are kept as `String` (lossy for the rare non UTF-8 name) so the JSON
/// stays portable between platforms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
}

/// One history entry as the store holds it in RAM.
///
/// Everything here is cheap to clone (`Arc`s and small scalars), so readers can
/// take a snapshot under the read lock and use it after releasing it. Large
/// payloads are deliberately absent: PNG blobs and the full text of big items
/// live on disk and come back through the store's byte budgeted cache.
///
/// Items are immutable once shared; the store updates one by cloning the (cheap)
/// struct, so a snapshot a reader holds never changes under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipItem {
    /// Store assigned, never reused while the item exists. Also the SQLite id.
    pub id: u64,
    /// Dedup key: xxh3-128 of the normalized content (see `hash.rs`).
    pub hash: u128,
    pub kind: ClipKind,
    /// Unix ms. Retention age is measured from this, so copying an old item
    /// again does not extend its life (pinning does).
    pub created_at: i64,
    /// Unix ms. Orders the history (newest first); strictly increasing across
    /// the store, see `Items::stamp`.
    pub last_used_at: i64,
    pub use_count: u32,
    pub pinned: bool,
    /// One line, 300 chars at most.
    pub preview: Arc<str>,
    /// What fuzzy search matches: the first 10,000 chars of text, the OCR text
    /// of an image (empty until OCR finishes), or the paths of a file list.
    pub search_text: Arc<str>,
    /// Full text when it is at most 64 KB. Larger texts load on demand.
    pub text_in_ram: Option<Arc<str>>,
    /// WebP thumbnail, 96 px tall at most.
    pub thumb: Option<Arc<[u8]>>,
    pub files: Option<Arc<[FileRef]>>,
    /// Pixel size; 0 for text and file items.
    pub width: u32,
    pub height: u32,
    /// Text: UTF-8 bytes. Image: PNG bytes. Files: sum of file sizes.
    pub byte_size: u64,
    pub ocr_status: OcrStatus,
    pub source_app: Option<Arc<str>>,
    /// File name of the encrypted PNG blob (image items). The plan's struct
    /// omits it; it is the same as the hex item hash today, but carrying the
    /// stored value means the store never has to assume that.
    pub blob_hash: Option<Arc<str>>,
}

/// What the app asks the OS clipboard to hold when copying an item back
/// (Phase 11). Minimal on purpose: Phase 11 may extend it, the variants map 1:1
/// to [`ClipKind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    /// Plain text, written as `CF_UNICODETEXT`.
    Text(String),
    /// Lossless PNG bytes plus the pixel size, so the writer can build
    /// `CF_DIBV5` and the registered `PNG` format without probing the PNG.
    Image {
        png: std::sync::Arc<[u8]>,
        width: u32,
        height: u32,
    },
    /// Existing paths, written as `CF_HDROP`. Missing paths are skipped by the
    /// writer.
    Files(Vec<std::path::PathBuf>),
}

impl Payload {
    pub fn kind(&self) -> ClipKind {
        match self {
            Self::Text(_) => ClipKind::Text,
            Self::Image { .. } => ClipKind::Image,
            Self::Files(_) => ClipKind::Files,
        }
    }
}
