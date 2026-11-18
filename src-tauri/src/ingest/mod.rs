//! Ingest pipeline: turns raw clipboard captures into finished items.
//!
//! Transitional state: the types below (Phase 5 steps 2 to 6) are in place, the
//! thread pool replaces the placeholder consumer at the end of Phase 5. Nothing
//! outside this module may rely on the placeholder.
//!
//! Item flow: `IngestMsg` -> (worker thread) -> [`IngestedItem`] -> [`ItemSink`].
//! Everything that is slow (image decode, PNG and WebP encode, file stat calls)
//! happens on the worker threads, never on the capture thread.

pub mod image;
pub mod text;

use std::sync::Arc;

use crate::model::{ClipKind, FileRef, OcrStatus};

/// A fully processed clipboard item, everything the store needs to build its
/// in-memory `ClipItem` (Phase 6) and the persist `NewItem`. Plain owned data
/// so it can cross threads; large payloads are moved, never copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestedItem {
    pub kind: ClipKind,
    /// Dedup key. Text: `hash::hash_text`. Image: RGBA pixels plus dimensions.
    /// Files: sorted path list.
    pub hash: u128,
    /// Short single line description (300 chars at most, whitespace collapsed).
    pub preview: String,
    /// What fuzzy search matches against: first 10,000 chars of the text, the
    /// file names, or empty for images until OCR fills it in.
    pub search_text: String,
    /// Full text with `\n` line endings (text items only).
    pub text: Option<String>,
    /// Lossless PNG of the picture (image items only).
    pub png: Option<Vec<u8>>,
    /// WebP thumbnail, max 96 px tall (images and image files).
    pub thumb: Option<Vec<u8>>,
    /// Copied paths (file items only).
    pub files: Option<Vec<FileRef>>,
    /// Pixel size of the image, or of the first image file's thumbnail source;
    /// 0 when not applicable.
    pub width: u32,
    pub height: u32,
    /// Text: UTF-8 bytes. Image: PNG bytes. Files: sum of file sizes.
    pub byte_size: u64,
    pub source_app: Option<String>,
    /// Unix milliseconds when ingest finished building the item.
    pub created_at: i64,
    /// `Pending` for images (OCR queued by the sink), `Done` for the kinds that
    /// never get OCR so a startup re-queue of pending items skips them.
    pub ocr_status: OcrStatus,
}

/// Destination of finished items. Phase 6's store implements it; until then
/// `sink::PlaceholderPersistSink` does. Called from ingest worker threads, so
/// implementations must be cheap and thread safe.
pub trait ItemSink: Send + Sync {
    /// A new or repeated item. If `item.hash` is already known the sink moves
    /// the existing item to the top instead of creating a second one.
    fn submit(&self, item: IngestedItem);
    /// The app wrote this hash to the clipboard itself: move that item to the
    /// top (self-write echo, see `platform::self_write`).
    fn bump(&self, hash: u128);
}

/// Work for the OCR worker (Phase 7 implements the queue). Deliberately carries
/// no pixels: queued jobs must not pin 4K bitmaps in RAM, the worker reloads the
/// PNG blob by `hash`. Speed trade: one extra PNG decode on a background thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OcrJob {
    /// Store id of the new image item.
    pub item_id: i64,
    pub hash: u128,
    pub width: u32,
    pub height: u32,
}

/// Hook for Phase 7. Sinks call it after a new image item is stored.
pub trait OcrQueue: Send + Sync {
    fn enqueue(&self, job: OcrJob);
}

/// OCR hook until Phase 7: accepts and drops the job.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopOcrQueue;

impl OcrQueue for NoopOcrQueue {
    fn enqueue(&self, _job: OcrJob) {}
}

/// Shared handle type used by the pool and the sinks.
#[allow(dead_code)] // used by the pool in a later commit of this phase
pub type SharedSink = Arc<dyn ItemSink>;

use std::thread::JoinHandle;

use crossbeam_channel::Receiver;

use crate::platform::{Captured, IngestMsg};

/// One line of description per message. Never includes clipboard content, only
/// the kind and the size.
fn describe(msg: &IngestMsg) -> String {
    match msg {
        IngestMsg::Captured(ev) => {
            let src = ev.source_app.as_deref().unwrap_or("unknown app");
            match &ev.captured {
                Captured::Text(t) => format!("text, {} bytes, from {src}", t.len()),
                Captured::Image(i) => {
                    format!("image {:?}, {} bytes, from {src}", i.format, i.bytes.len())
                }
                Captured::Files(f) => format!("files, {} paths, from {src}", f.len()),
            }
        }
        IngestMsg::Bump { hash } => format!("bump existing item {hash:032x}"),
    }
}

/// PLACEHOLDER consumer (replaced in Phase 5): logs each message and drops it.
/// Ends when every sender is gone.
pub fn spawn_placeholder_consumer(rx: Receiver<IngestMsg>) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("ingest-placeholder".into())
        .spawn(move || {
            for msg in rx {
                eprintln!("clipdump: captured {}", describe(&msg));
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{CapturedEvent, ImageData, ImageFormat};

    #[test]
    fn describe_never_leaks_content() {
        let text = IngestMsg::Captured(CapturedEvent {
            captured: Captured::Text("super secret".into()),
            source_app: Some("notepad.exe".into()),
        });
        let d = describe(&text);
        assert!(d.contains("12 bytes") && d.contains("notepad.exe"));
        assert!(!d.contains("secret"));

        let img = IngestMsg::Captured(CapturedEvent {
            captured: Captured::Image(ImageData {
                format: ImageFormat::Dib,
                bytes: vec![0; 5],
            }),
            source_app: None,
        });
        assert!(describe(&img).contains("Dib, 5 bytes"));
    }

    #[test]
    fn consumer_drains_and_stops_when_senders_drop() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let h = spawn_placeholder_consumer(rx).unwrap();
        tx.send(IngestMsg::Bump { hash: 1 }).unwrap();
        drop(tx);
        h.join().unwrap();
    }
}
