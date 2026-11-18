//! Shared data types.
//!
//! Phase 3 only needs the small value types the persistence layer stores. The
//! full in-memory `ClipItem` (Section 6, Phase 6) is added later and builds on
//! these, so their numeric codes are part of the on-disk format and must not
//! change.

use serde::{Deserialize, Serialize};

/// What an item holds. The discriminant is stored in `items.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
