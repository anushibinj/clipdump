//! PLACEHOLDER sink, to be replaced by the Phase 6 store.
//!
//! The real store (Phase 6) keeps the in-memory index, runs retention and owns
//! the persist ops. Until it exists, this sink does the minimum that makes
//! Phase 5 observable end to end on Windows: dedup by hash, assign ids, and
//! write each item through the existing [`Persist`] writer (encrypted rows,
//! blob and thumbnail files). It keeps no items in RAM beyond a hash to id map,
//! never evicts anything and never reads items back for the UI.
//!
//! Phase 6 swaps it by implementing [`ItemSink`] on the store and passing that
//! to [`super::IngestPool::start`]; nothing else in the pipeline changes. The
//! [`OcrQueue`] and [`HistoryChanged`] hooks move with it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

use super::changed::HistoryChanged;
use super::{IngestedItem, ItemSink, OcrJob, OcrQueue};
use crate::model::ClipKind;
use crate::persist::blobs::hash_hex;
use crate::persist::{NewItem, Persist, WriteOp};

pub struct PlaceholderPersistSink {
    persist: Arc<Persist>,
    /// Phase 7 plugs the OCR worker in here. Called once per new image item.
    ocr: Arc<dyn OcrQueue>,
    /// Fired after every stored or bumped item (debounced).
    changed: Option<HistoryChanged>,
    next_id: AtomicI64,
    /// content hash -> item id. Seeded from the database so dedup survives a
    /// restart.
    known: Mutex<HashMap<u128, i64>>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl PlaceholderPersistSink {
    pub fn new(
        persist: Arc<Persist>,
        ocr: Arc<dyn OcrQueue>,
        changed: Option<HistoryChanged>,
    ) -> Result<Self> {
        let known = persist
            .load_all_meta(0)?
            .into_iter()
            .map(|m| (u128::from_be_bytes(m.hash), m.id))
            .collect();
        Ok(Self {
            next_id: AtomicI64::new(persist.max_id()? + 1),
            persist,
            ocr,
            changed,
            known: Mutex::new(known),
        })
    }

    fn notify(&self) {
        if let Some(c) = &self.changed {
            c.notify();
        }
    }

    fn write(&self, op: WriteOp) {
        if let Err(e) = self.persist.submit(op) {
            eprintln!("clipdump: could not queue a write: {e:#}");
        }
    }
}

impl ItemSink for PlaceholderPersistSink {
    fn submit(&self, item: IngestedItem) {
        // The lock is held until the insert is queued, so a concurrent duplicate
        // (two workers, same picture as PNG and DIB) can never queue its
        // `UpdateUsed` ahead of the `Insert` it refers to. Queueing is a channel
        // push, so the lock is held for microseconds.
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = known.get(&item.hash) {
            // Same content copied again: move it to the top.
            self.write(WriteOp::UpdateUsed { id, at: now_ms() });
            drop(known);
            self.notify();
            return;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        known.insert(item.hash, id);

        let hash = item.hash.to_be_bytes();
        let is_image = item.kind == ClipKind::Image;
        let (item_hash, width, height) = (item.hash, item.width, item.height);
        let new = NewItem {
            id,
            hash,
            kind: item.kind,
            created_at: item.created_at,
            last_used_at: item.created_at,
            use_count: 0,
            pinned: false,
            preview: item.preview,
            text: item.text,
            ocr: None,
            ocr_status: item.ocr_status,
            files: item.files,
            // The blob is named after the item hash, like the thumbnail.
            blob_hash: is_image.then(|| hash_hex(&hash)),
            width: is_image.then_some(width),
            height: is_image.then_some(height),
            byte_size: item.byte_size,
            source_app: item.source_app,
        };
        self.write(WriteOp::Insert {
            item: Box::new(new),
            blob: item.png,
            thumb: item.thumb,
        });
        drop(known);
        if is_image {
            self.ocr.enqueue(OcrJob {
                item_id: id,
                hash: item_hash,
                width,
                height,
            });
        }
        self.notify();
    }

    fn bump(&self, hash: u128) {
        let known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = known.get(&hash) {
            // Same ordering rule as in `submit`.
            self.write(WriteOp::UpdateUsed { id, at: now_ms() });
            drop(known);
            self.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::image::tests::{dib24, pic_gradient, png_of};
    use crate::ingest::{IngestPool, files};
    use crate::model::OcrStatus;
    use crate::persist::tests::open;
    use crate::platform::{Captured, CapturedEvent, ImageData, ImageFormat, IngestMsg};
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct RecordingOcr(Mutex<Vec<OcrJob>>);
    impl OcrQueue for RecordingOcr {
        fn enqueue(&self, job: OcrJob) {
            self.0.lock().unwrap().push(job);
        }
    }

    fn ev(c: Captured, app: &str) -> IngestMsg {
        IngestMsg::Captured(CapturedEvent {
            captured: c,
            source_app: Some(app.into()),
        })
    }

    fn run(persist: &Arc<Persist>, ocr: &Arc<RecordingOcr>, msgs: Vec<IngestMsg>) -> usize {
        let events = Arc::new(AtomicUsize::new(0));
        let e2 = Arc::clone(&events);
        let changed = HistoryChanged::new(move || {
            e2.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        let sink = Arc::new(
            PlaceholderPersistSink::new(Arc::clone(persist), ocr.clone(), Some(changed)).unwrap(),
        );
        let (tx, rx) = crossbeam_channel::bounded(32);
        let pool = IngestPool::start(rx, sink).unwrap();
        for m in msgs {
            tx.send(m).unwrap();
        }
        drop(tx);
        pool.join();
        persist.flush().unwrap();
        events.load(Ordering::SeqCst)
    }

    #[test]
    fn pool_to_persist_end_to_end_with_dedup_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Arc::new(open(dir.path()));
        let ocr = Arc::new(RecordingOcr::default());

        let pic = pic_gradient(300, 200);
        let data_dir = tempfile::tempdir().unwrap();
        let file = data_dir.path().join("note.txt");
        std::fs::write(&file, b"abc").unwrap();

        let events = run(
            &persist,
            &ocr,
            vec![
                ev(Captured::Text("hello\r\nworld".into()), "notepad.exe"),
                // Same text with LF endings: dedups.
                ev(Captured::Text("hello\nworld".into()), "code.exe"),
                ev(
                    Captured::Image(ImageData {
                        format: ImageFormat::Png,
                        bytes: png_of(&pic),
                    }),
                    "snip.exe",
                ),
                // Same picture as a DIB: dedups across formats.
                ev(
                    Captured::Image(ImageData {
                        format: ImageFormat::Dib,
                        bytes: dib24(&pic, false),
                    }),
                    "paint.exe",
                ),
                ev(Captured::Files(vec![file.clone()]), "explorer.exe"),
            ],
        );
        assert!(events >= 1, "history:changed fired");

        let rows = persist.load_all_meta(1 << 20).unwrap();
        assert_eq!(rows.len(), 3, "text, image and files, duplicates merged");
        let c = persist.crypto();
        let by_kind = |k: ClipKind| rows.iter().find(|r| r.kind == k).unwrap();

        let t = by_kind(ClipKind::Text);
        assert_eq!(c.decrypt(&t.preview_enc).unwrap(), b"hello world");
        assert_eq!(
            persist.load_text(t.id).unwrap().as_deref(),
            Some("hello\nworld")
        );
        assert_eq!(t.use_count, 1, "the LF copy bumped the CRLF item");

        let i = by_kind(ClipKind::Image);
        assert_eq!(i.ocr_status, OcrStatus::Pending);
        assert_eq!((i.width, i.height), (Some(300), Some(200)));
        assert_eq!(i.use_count, 1);
        let png = persist.read_blob(i.blob_hash.as_ref().unwrap()).unwrap();
        assert_eq!(crate::ingest::image::decode_png(&png).unwrap(), pic);
        let thumb = persist.read_thumb(&i.hash).unwrap();
        let th = crate::ingest::image::decode_any(&thumb).unwrap();
        assert_eq!((th.width, th.height), (144, 96));
        assert_eq!(i.source_app.as_deref(), Some("snip.exe"));

        let f = by_kind(ClipKind::Files);
        let refs: Vec<crate::model::FileRef> =
            serde_json::from_slice(&c.decrypt(f.files_enc.as_ref().unwrap()).unwrap()).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].size, 3);
        assert_eq!(f.ocr_status, OcrStatus::Done);
        assert_eq!(f.byte_size, 3);

        // OCR hook: exactly one job, for the one image item.
        {
            let jobs = ocr.0.lock().unwrap();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].item_id, i.id);
            assert_eq!(jobs[0].hash, u128::from_be_bytes(i.hash));
            assert_eq!((jobs[0].width, jobs[0].height), (300, 200));
        }

        // "Restart": a new sink over the same database still dedups and does
        // not reuse ids. Also exercises Bump.
        let known_hash = files::build(std::slice::from_ref(&file), None, 0)
            .unwrap()
            .hash;
        run(
            &persist,
            &ocr,
            vec![
                ev(Captured::Text("hello world".into()), "x.exe"),
                IngestMsg::Bump { hash: known_hash },
                IngestMsg::Bump { hash: 0xdead },
            ],
        );
        let rows = persist.load_all_meta(1 << 20).unwrap();
        assert_eq!(
            rows.len(),
            4,
            "new text added, bump of unknown hash ignored"
        );
        let max = rows.iter().map(|r| r.id).max().unwrap();
        assert_eq!(max, 4);
        let f = rows.iter().find(|r| r.kind == ClipKind::Files).unwrap();
        assert_eq!(f.use_count, 1, "bump moved the file item");
        assert_eq!(ocr.0.lock().unwrap().len(), 1, "no new OCR job");
    }
}
