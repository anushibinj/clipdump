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

    fn touch(&self, id: i64) {
        self.write(WriteOp::UpdateUsed { id, at: now_ms() });
        self.notify();
    }
}

impl ItemSink for PlaceholderPersistSink {
    fn submit(&self, item: IngestedItem) {
        let id = {
            let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(&id) = known.get(&item.hash) {
                drop(known);
                // Same content copied again: move it to the top.
                self.touch(id);
                return;
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            known.insert(item.hash, id);
            id
        };

        let hash = item.hash.to_be_bytes();
        let is_image = item.kind == ClipKind::Image;
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
            width: is_image.then_some(item.width),
            height: is_image.then_some(item.height),
            byte_size: item.byte_size,
            source_app: item.source_app,
        };
        self.write(WriteOp::Insert {
            item: Box::new(new),
            blob: item.png,
            thumb: item.thumb,
        });
        if is_image {
            self.ocr.enqueue(OcrJob {
                item_id: id,
                hash: item.hash,
                width: item.width,
                height: item.height,
            });
        }
        self.notify();
    }

    fn bump(&self, hash: u128) {
        let id = self
            .known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&hash)
            .copied();
        if let Some(id) = id {
            self.touch(id);
        }
    }
}
