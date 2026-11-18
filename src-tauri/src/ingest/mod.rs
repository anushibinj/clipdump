//! Ingest pipeline: turns raw clipboard captures into finished items.
//!
//! A small pool of worker threads (below normal priority) reads [`IngestMsg`]s
//! from the capture channel and hands finished items to an [`ItemSink`].
//!
//! Item flow: `IngestMsg` -> (worker thread) -> [`IngestedItem`] -> [`ItemSink`].
//! Everything that is slow (image decode, PNG and WebP encode, file stat calls)
//! happens on the worker threads, never on the capture thread.

pub mod changed;
pub mod files;
pub mod image;
pub mod text;

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossbeam_channel::Receiver;

use crate::model::{ClipKind, FileRef, OcrStatus};
use crate::platform::{Captured, CapturedEvent, IngestMsg};

/// A fully processed clipboard item, everything the store needs to build its
/// in-memory `ClipItem` and the persist `NewItem`. Plain owned data
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
    /// Pixel size of the image; 0 for text and file items.
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

/// Destination of finished items. The store (`store::Store`) implements it.
/// Called from ingest worker threads, so
/// implementations must be cheap and thread safe.
pub trait ItemSink: Send + Sync {
    /// A new or repeated item. If `item.hash` is already known the sink moves
    /// the existing item to the top instead of creating a second one.
    fn submit(&self, item: IngestedItem);
    /// The app wrote this hash to the clipboard itself: move that item to the
    /// top (self-write echo, see `platform::self_write`).
    fn bump(&self, hash: u128);
}

/// Work for the OCR worker (`ocr::OcrWorker`). Deliberately carries
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

/// The store calls it after a new image item is stored (`ocr::OcrQueueHandle`
/// is the real queue). Must be cheap and never block.
pub trait OcrQueue: Send + Sync {
    fn enqueue(&self, job: OcrJob);
}

/// Shared handle type used by the pool and the sinks.
pub type SharedSink = Arc<dyn ItemSink>;

/// Worker threads. Two is enough: image work is the only heavy part, and a
/// second thread keeps text and file items from queueing behind a 4K screenshot.
pub const WORKERS: usize = 2;

/// Unix milliseconds now.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Turn one capture into a finished item. `Ok(None)` means there is nothing to
/// keep (empty text, empty file list). All the slow work happens in here, so
/// only call it from an ingest worker.
pub fn process(ev: CapturedEvent, created_at: i64) -> Result<Option<IngestedItem>> {
    let CapturedEvent {
        captured,
        source_app,
    } = ev;
    Ok(match captured {
        Captured::Text(t) => text::build(&t, source_app, created_at),
        Captured::Image(data) => Some(image::build(data, source_app, created_at)?),
        Captured::Files(paths) => files::build(&paths, source_app, created_at),
    })
}

/// One line per item for the log. Never includes clipboard content.
fn describe(item: &IngestedItem) -> String {
    let src = item.source_app.as_deref().unwrap_or("unknown app");
    match item.kind {
        ClipKind::Text => format!("text, {} bytes, from {src}", item.byte_size),
        ClipKind::Image => format!(
            "image {}x{}, {} bytes png, from {src}",
            item.width, item.height, item.byte_size
        ),
        ClipKind::Files => format!(
            "files, {} paths, from {src}",
            item.files.as_ref().map_or(0, Vec::len)
        ),
    }
}

fn handle(msg: IngestMsg, sink: &dyn ItemSink) {
    match msg {
        IngestMsg::Bump { hash } => sink.bump(hash),
        IngestMsg::Captured(ev) => {
            // Stamp on dequeue: the channel is FIFO, so this keeps copy order
            // even when a fast text item overtakes a slow screenshot.
            let created_at = now_ms();
            let started = Instant::now();
            match process(ev, created_at) {
                Ok(Some(item)) => {
                    eprintln!(
                        "clipdump: ingested {} in {} ms",
                        describe(&item),
                        started.elapsed().as_millis()
                    );
                    sink.submit(item);
                }
                Ok(None) => {}
                // Errors never include clipboard bytes.
                Err(e) => eprintln!("clipdump: ingest skipped an item: {e:#}"),
            }
        }
    }
}

/// The ingest worker pool. Workers end when every sender of the capture channel
/// is gone and the queue is drained.
pub struct IngestPool {
    workers: Vec<JoinHandle<()>>,
}

impl IngestPool {
    /// Start [`WORKERS`] threads consuming `rx`. If a thread cannot be
    /// spawned, the ones already started keep running and the error is returned
    /// only when none could start.
    pub fn start(rx: Receiver<IngestMsg>, sink: SharedSink) -> io::Result<Self> {
        let mut workers = Vec::with_capacity(WORKERS);
        let mut last_err = None;
        for n in 0..WORKERS {
            let rx = rx.clone();
            let sink = Arc::clone(&sink);
            let spawned = std::thread::Builder::new()
                .name(format!("ingest-{n}"))
                .spawn(move || {
                    // Ingest must never compete with typing or the hotkey.
                    if let Err(e) = crate::platform::lower_current_thread_priority() {
                        eprintln!("clipdump: could not lower ingest thread priority: {e}");
                    }
                    for msg in rx {
                        // A bug in one decoder must not take the worker down.
                        // (With `panic = "abort"` in release this cannot
                        // trigger; it protects dev builds and tests.)
                        let r = catch_unwind(AssertUnwindSafe(|| handle(msg, &*sink)));
                        if r.is_err() {
                            eprintln!("clipdump: ingest worker recovered from a panic");
                        }
                    }
                });
            match spawned {
                Ok(h) => workers.push(h),
                Err(e) => last_err = Some(e),
            }
        }
        match (workers.is_empty(), last_err) {
            (true, Some(e)) => Err(e),
            _ => Ok(Self { workers }),
        }
    }

    /// Wait for the workers to finish. They finish once all senders are
    /// dropped, so drop those first. Used by tests and by shutdown.
    pub fn join(self) {
        for w in self.workers {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::platform::{ImageData, ImageFormat};
    use std::sync::Mutex;

    /// Collects everything it is given.
    #[derive(Default)]
    pub struct CollectSink {
        pub items: Mutex<Vec<IngestedItem>>,
        pub bumps: Mutex<Vec<u128>>,
    }

    impl ItemSink for CollectSink {
        fn submit(&self, item: IngestedItem) {
            self.items.lock().unwrap().push(item);
        }
        fn bump(&self, hash: u128) {
            self.bumps.lock().unwrap().push(hash);
        }
    }

    fn text_ev(t: &str) -> IngestMsg {
        IngestMsg::Captured(CapturedEvent {
            captured: Captured::Text(t.into()),
            source_app: Some("notepad.exe".into()),
        })
    }

    #[test]
    fn process_handles_all_three_kinds_and_empties() {
        let t = process(
            CapturedEvent {
                captured: Captured::Text("hi".into()),
                source_app: Some("a.exe".into()),
            },
            9,
        )
        .unwrap()
        .unwrap();
        assert_eq!((t.kind, t.created_at), (ClipKind::Text, 9));
        assert_eq!(t.source_app.as_deref(), Some("a.exe"));

        let p = image::tests::pic_gradient(30, 20);
        let i = process(
            CapturedEvent {
                captured: Captured::Image(ImageData {
                    format: ImageFormat::Dib,
                    bytes: image::tests::dib24(&p, true),
                }),
                source_app: None,
            },
            9,
        )
        .unwrap()
        .unwrap();
        assert_eq!((i.kind, i.width, i.height), (ClipKind::Image, 30, 20));

        let f = process(
            CapturedEvent {
                captured: Captured::Files(vec!["/nope/x.txt".into()]),
                source_app: None,
            },
            9,
        )
        .unwrap()
        .unwrap();
        assert_eq!(f.kind, ClipKind::Files);

        for empty in [Captured::Text(String::new()), Captured::Files(vec![])] {
            let ev = CapturedEvent {
                captured: empty,
                source_app: None,
            };
            assert!(process(ev, 0).unwrap().is_none());
        }
        let bad = CapturedEvent {
            captured: Captured::Image(ImageData {
                format: ImageFormat::Png,
                bytes: vec![1, 2, 3],
            }),
            source_app: None,
        };
        assert!(process(bad, 0).is_err());
    }

    #[test]
    fn describe_never_leaks_content() {
        let it = text::build("super secret", Some("notepad.exe".into()), 0).unwrap();
        let d = describe(&it);
        assert!(d.contains("12 bytes") && d.contains("notepad.exe"));
        assert!(!d.contains("secret"));
    }

    #[test]
    fn pool_processes_everything_bumps_and_survives_bad_items() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let sink = Arc::new(CollectSink::default());
        let pool = IngestPool::start(rx, sink.clone()).unwrap();

        tx.send(text_ev("one")).unwrap();
        tx.send(IngestMsg::Bump { hash: 77 }).unwrap();
        // A broken image in the middle must not stop later items.
        tx.send(IngestMsg::Captured(CapturedEvent {
            captured: Captured::Image(ImageData {
                format: ImageFormat::Dibv5,
                bytes: vec![0; 7],
            }),
            source_app: None,
        }))
        .unwrap();
        let p = image::tests::pic_gradient(64, 48);
        tx.send(IngestMsg::Captured(CapturedEvent {
            captured: Captured::Image(ImageData {
                format: ImageFormat::Dibv5,
                bytes: image::tests::dibv5_bitfields(&p, false),
            }),
            source_app: Some("snip.exe".into()),
        }))
        .unwrap();
        tx.send(text_ev("two")).unwrap();
        drop(tx);
        pool.join();

        let mut items = sink.items.lock().unwrap().clone();
        items.sort_by_key(|i| i.kind.code());
        assert_eq!(items.len(), 3, "bad image dropped, the rest delivered");
        assert_eq!(items.iter().filter(|i| i.kind == ClipKind::Text).count(), 2);
        let img = items.iter().find(|i| i.kind == ClipKind::Image).unwrap();
        assert_eq!(img.source_app.as_deref(), Some("snip.exe"));
        assert_eq!(*sink.bumps.lock().unwrap(), vec![77]);
    }

    #[test]
    fn pool_stops_when_senders_drop() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let pool = IngestPool::start(rx, Arc::new(CollectSink::default())).unwrap();
        drop(tx);
        pool.join();
    }

    #[test]
    fn same_text_with_crlf_and_lf_is_the_same_hash_through_the_pool() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let sink = Arc::new(CollectSink::default());
        let pool = IngestPool::start(rx, sink.clone()).unwrap();
        tx.send(text_ev("a\r\nb")).unwrap();
        tx.send(text_ev("a\nb")).unwrap();
        drop(tx);
        pool.join();
        let items = sink.items.lock().unwrap();
        assert_eq!(items[0].hash, items[1].hash);
    }
}
