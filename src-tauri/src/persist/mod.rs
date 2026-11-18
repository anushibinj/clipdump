//! Encrypted, crash safe persistence for clipboard history (Section 3 of PLAN.md).
//!
//! Layout on disk, under the data directory:
//!
//! ```text
//! history.db            SQLite (WAL), text columns encrypted
//! blobs/ab/<hash>.bin   encrypted PNG
//! thumbs/ab/<hash>.bin  encrypted WebP thumbnail
//! key.bin               32 byte data key, protected by the platform keystore
//! ```
//!
//! # Threading
//!
//! * **Writes** go through [`Persist::submit`], which only pushes a
//!   [`WriteOp`] on an unbounded channel: it never blocks and never touches
//!   disk, so the hotkey and clipboard paths stay fast. A single writer thread
//!   applies ops in order, grouped into one SQLite transaction per batch
//!   (every [`BATCH_WINDOW`] or [`MAX_BATCH_OPS`] ops, whichever comes first).
//!   Encryption and blob file writes also happen on that thread.
//! * **Reads** run on the caller's thread over a separate read only
//!   connection (WAL lets readers and the writer work at the same time). They
//!   see committed data only: call [`Persist::flush`] first if you need to read
//!   your own writes.
//! * [`Persist::flush`] waits until everything submitted so far is committed;
//!   [`Persist::shutdown`] additionally drains the queue, checkpoints the WAL
//!   and stops the thread. Call it on quit and on `WM_ENDSESSION`.

pub mod blobs;
pub mod crypto;
pub mod db;
pub mod integrity;

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use rusqlite::Connection;

use crate::model::{ClipKind, FileRef, OcrStatus};
use crate::platform::KeyProtector;
use blobs::{Blobs, FileKind, hash_hex};
use crypto::Crypto;
use db::{ItemRow, MetaRow, Removed};
use integrity::IntegrityReport;

/// How long the writer waits for more ops after the first one before it
/// commits. Speed trade: a longer window means fewer commits (cheaper), a
/// shorter one means data hits disk sooner. 50 ms is invisible to the user.
pub const BATCH_WINDOW: Duration = Duration::from_millis(50);
/// Upper bound on ops per transaction, so a burst cannot delay a commit.
pub const MAX_BATCH_OPS: usize = 32;

/// A new item in plaintext, as the store builds it. The writer thread encrypts
/// it. Field meanings match the `items` table in Section 3.
#[derive(Clone, Debug)]
pub struct NewItem {
    pub id: i64,
    /// xxh3-128 of the normalized content, big endian.
    pub hash: [u8; 16],
    pub kind: ClipKind,
    pub created_at: i64,
    pub last_used_at: i64,
    pub use_count: u32,
    pub pinned: bool,
    pub preview: String,
    pub text: Option<String>,
    pub ocr: Option<String>,
    pub ocr_status: OcrStatus,
    pub files: Option<Vec<FileRef>>,
    /// Name of the blob file (image items). Must be at least 2 characters of
    /// `[A-Za-z0-9_-]`; a hex hash is the intended value.
    pub blob_hash: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub byte_size: u64,
    pub source_app: Option<String>,
}

/// One unit of write work. Applied in submission order.
pub enum WriteOp {
    /// Insert a row, plus its encrypted PNG blob and WebP thumbnail files.
    /// The files are written first, so a committed row never points at a file
    /// that was not written. The thumbnail is named after the item hash.
    Insert {
        item: Box<NewItem>,
        blob: Option<Vec<u8>>,
        thumb: Option<Vec<u8>>,
    },
    UpdateUsed {
        id: i64,
        at: i64,
    },
    SetPinned {
        id: i64,
        pinned: bool,
    },
    /// Delete one item (pinned or not) and its files.
    Delete {
        id: i64,
    },
    /// "Clear history": every unpinned item and its files.
    DeleteUnpinned,
    /// Retention: unpinned items created before `cutoff` (unix ms).
    DeleteOlderThan {
        cutoff: i64,
    },
    /// OCR result. `None` text with a failed or unavailable status clears it.
    SetOcr {
        id: i64,
        text: Option<String>,
        status: OcrStatus,
    },
    /// Commit everything before this op, then report whether it all succeeded.
    Flush(Sender<bool>),
    /// Like `Flush`, then drain, checkpoint and exit the writer thread.
    Shutdown(Sender<bool>),
}

/// Counters for tests and the `--perf` overlay (Phase 14).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriterStats {
    /// Transactions committed (batches that contained at least one op).
    pub batches: u64,
    /// Data ops applied (control ops excluded).
    pub ops: u64,
    /// Ops that failed; details go to stderr.
    pub errors: u64,
}

#[derive(Default)]
struct StatsInner {
    batches: AtomicU64,
    ops: AtomicU64,
    errors: AtomicU64,
}

/// The persistence facade owned by the store.
pub struct Persist {
    tx: Sender<WriteOp>,
    writer: Mutex<Option<JoinHandle<()>>>,
    reader: Mutex<Connection>,
    crypto: Arc<Crypto>,
    blobs: Arc<Blobs>,
    stats: Arc<StatsInner>,
    closed: AtomicBool,
    integrity: IntegrityReport,
    protector_name: &'static str,
}

impl Persist {
    /// Open (or create) the history in `dir` with this platform's key
    /// protector: DPAPI on Windows, the NOT SECURE stub elsewhere.
    pub fn open(dir: &Path) -> Result<Self> {
        Self::open_with(dir, crate::platform::default_key_protector().as_ref())
    }

    /// [`Persist::open`] with an explicit key protector (used by tests).
    pub fn open_with(dir: &Path, protector: &dyn KeyProtector) -> Result<Self> {
        fs::create_dir_all(dir)
            .with_context(|| format!("cannot create data directory {}", dir.display()))?;
        let crypto = Arc::new(Crypto::load_or_create(dir, protector)?);
        let blobs = Arc::new(Blobs::new(dir, crypto.clone()));
        let db_path = dir.join(db::DB_FILE);
        let writer_conn = db::open_writer(&db_path)?;
        // Before the writer thread exists, so nothing races with the cleanup.
        let integrity = integrity::run(&writer_conn, &blobs)?;
        if !integrity.is_clean() {
            eprintln!("clipdump: persist: startup repair: {integrity:?}");
        }
        // The writer connection creates the file, so the reader can open it.
        let reader = db::open_reader(&db_path)?;

        let (tx, rx) = crossbeam_channel::unbounded();
        let stats = Arc::new(StatsInner::default());
        let writer = Writer {
            conn: writer_conn,
            crypto: crypto.clone(),
            blobs: blobs.clone(),
            stats: stats.clone(),
        };
        let handle = std::thread::Builder::new()
            .name("clipdump-persist".into())
            .spawn(move || writer.run(rx))
            .context("cannot start persist writer thread")?;
        Ok(Self {
            tx,
            writer: Mutex::new(Some(handle)),
            reader: Mutex::new(reader),
            crypto,
            blobs,
            stats,
            closed: AtomicBool::new(false),
            integrity,
            protector_name: protector.name(),
        })
    }

    /// What the startup integrity check had to repair.
    pub fn integrity_report(&self) -> &IntegrityReport {
        &self.integrity
    }

    /// Which key protector guards `key.bin` (for example `"dpapi"`). The
    /// settings screen can warn when this is the insecure stub.
    pub fn key_protector_name(&self) -> &'static str {
        self.protector_name
    }

    /// Queue a write. Never blocks and never does I/O. Fails only after
    /// [`Persist::shutdown`].
    pub fn submit(&self, op: WriteOp) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            bail!("persistence is shut down");
        }
        self.tx
            .send(op)
            .map_err(|_| anyhow!("persist writer thread is gone"))
    }

    /// Block until everything submitted before this call is committed to
    /// SQLite (and its blob files written). Errors if any op in the final
    /// batch failed. Not for the hot path: use on quit, in tests, or before a
    /// read that must see recent writes.
    pub fn flush(&self) -> Result<()> {
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        self.submit(WriteOp::Flush(ack_tx))?;
        match ack_rx.recv() {
            Ok(true) => Ok(()),
            Ok(false) => bail!("some persist operations failed, see the log"),
            Err(_) => bail!("persist writer thread exited before flushing"),
        }
    }

    /// Drain the queue, commit, checkpoint the WAL and stop the writer thread.
    /// Idempotent. Also runs on drop as a best effort.
    pub fn shutdown(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        // `submit` refuses after `closed`, so send directly.
        let sent = self.tx.send(WriteOp::Shutdown(ack_tx)).is_ok();
        let ok = sent && ack_rx.recv().unwrap_or(false);
        if let Some(handle) = self.writer.lock().unwrap().take() {
            let _ = handle.join();
        }
        if ok {
            Ok(())
        } else {
            bail!("persist shutdown was not clean, see the log")
        }
    }

    /// Snapshot of the writer counters.
    pub fn stats(&self) -> WriterStats {
        WriterStats {
            batches: self.stats.batches.load(Ordering::Relaxed),
            ops: self.stats.ops.load(Ordering::Relaxed),
            errors: self.stats.errors.load(Ordering::Relaxed),
        }
    }

    /// The data cipher, so the store can decrypt the raw rows from
    /// [`Persist::load_all_meta`] (in parallel at startup, Phase 6).
    pub fn crypto(&self) -> &Arc<Crypto> {
        &self.crypto
    }

    /// All committed items, newest use first, columns still encrypted. See
    /// [`db::MetaRow`] for the `small_text_limit` rule.
    pub fn load_all_meta(&self, small_text_limit: usize) -> Result<Vec<MetaRow>> {
        db::load_all_meta(&self.reader.lock().unwrap(), small_text_limit)
    }

    /// Decrypted full text of an item, if it has any.
    pub fn load_text(&self, id: i64) -> Result<Option<String>> {
        let enc = db::load_text(&self.reader.lock().unwrap(), id)?;
        enc.map(|e| String::from_utf8(self.crypto.decrypt(&e)?).context("stored text is not UTF-8"))
            .transpose()
    }

    /// Largest committed item id (0 when empty). Seeds the store's id counter.
    pub fn max_id(&self) -> Result<i64> {
        db::max_id(&self.reader.lock().unwrap())
    }

    /// Decrypted PNG bytes of an image blob.
    pub fn read_blob(&self, blob_hash: &str) -> Result<Vec<u8>> {
        self.blobs.read(FileKind::Blob, blob_hash)
    }

    /// Decrypted WebP thumbnail of an item, named after its hash.
    pub fn read_thumb(&self, hash: &[u8; 16]) -> Result<Vec<u8>> {
        self.blobs.read(FileKind::Thumb, &hash_hex(hash))
    }
}

impl Drop for Persist {
    fn drop(&mut self) {
        if let Err(e) = self.shutdown() {
            eprintln!("clipdump: persist: {e:#}");
        }
    }
}

// ---------------------------------------------------------------------------
// Writer thread
// ---------------------------------------------------------------------------

struct Writer {
    conn: Connection,
    crypto: Arc<Crypto>,
    blobs: Arc<Blobs>,
    stats: Arc<StatsInner>,
}

/// Ops collected for one transaction.
#[derive(Default)]
struct Batch {
    ops: Vec<WriteOp>,
    acks: Vec<Sender<bool>>,
    shutdown: bool,
    /// All senders are gone and the queue is empty.
    disconnected: bool,
}

impl Batch {
    /// Returns true if `op` ends the batch (a control op).
    fn push(&mut self, op: WriteOp) -> bool {
        match op {
            WriteOp::Flush(ack) => {
                self.acks.push(ack);
                true
            }
            WriteOp::Shutdown(ack) => {
                self.acks.push(ack);
                self.shutdown = true;
                true
            }
            data => {
                self.ops.push(data);
                false
            }
        }
    }

    fn full(&self) -> bool {
        self.ops.len() >= MAX_BATCH_OPS
    }
}

impl Writer {
    fn run(mut self, rx: Receiver<WriteOp>) {
        // Idle cost: `recv` blocks, so the thread uses no CPU between ops.
        'outer: while let Ok(first) = rx.recv() {
            let mut batch = Batch::default();
            let mut barrier = batch.push(first);
            let deadline = Instant::now() + BATCH_WINDOW;
            while !barrier && !batch.full() {
                match rx.recv_deadline(deadline) {
                    Ok(op) => barrier = batch.push(op),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        batch.disconnected = true;
                        break;
                    }
                }
            }
            let mut ok = self.apply(&mut batch);
            let shutdown = batch.shutdown;
            let disconnected = batch.disconnected;
            if shutdown {
                // Anything queued behind the Shutdown op (a racing submit) is
                // still written.
                let mut rest = Batch::default();
                while let Ok(op) = rx.try_recv() {
                    rest.push(op);
                    if rest.full() {
                        ok &= self.apply(&mut rest);
                        rest = Batch::default();
                    }
                }
                ok &= self.apply(&mut rest);
            }
            for ack in batch.acks.drain(..) {
                let _ = ack.send(ok);
            }
            if shutdown || disconnected {
                break 'outer;
            }
        }
        // Fold the WAL into the main file so a cold copy of history.db is
        // complete and the -wal file does not grow across runs.
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }

    /// Apply `batch` in one transaction. Returns false if any op failed.
    /// Control acks are *not* sent here; the caller does that after.
    fn apply(&mut self, batch: &mut Batch) -> bool {
        if batch.ops.is_empty() {
            return true;
        }
        let mut errors = 0u64;
        let ops = std::mem::take(&mut batch.ops);
        let n_ops = ops.len() as u64;
        // Files to delete once the transaction is committed: a crash in between
        // leaves an orphan file (cleaned at startup), never a row without its file.
        let mut doomed: Vec<(FileKind, String)> = Vec::new();

        let tx = match self.conn.transaction() {
            Ok(tx) => tx,
            Err(e) => {
                eprintln!("clipdump: persist: cannot begin transaction: {e}");
                self.stats.errors.fetch_add(n_ops, Ordering::Relaxed);
                return false;
            }
        };
        for op in ops {
            let result: Result<()> = match op {
                WriteOp::Insert { item, blob, thumb } => {
                    // A re-insert of something deleted earlier in this batch
                    // must keep its files.
                    let thumb_name = hash_hex(&item.hash);
                    doomed.retain(|(k, n)| {
                        !(*k == FileKind::Thumb && *n == thumb_name
                            || *k == FileKind::Blob && Some(n) == item.blob_hash.as_ref())
                    });
                    insert(&tx, &self.crypto, &self.blobs, &item, blob, thumb)
                }
                WriteOp::UpdateUsed { id, at } => db::update_used(&tx, id, at).map(|_| ()),
                WriteOp::SetPinned { id, pinned } => db::set_pinned(&tx, id, pinned).map(|_| ()),
                WriteOp::Delete { id } => db::delete_item(&tx, id).map(|r| {
                    if let Some(r) = r {
                        doom(&mut doomed, &r);
                    }
                }),
                WriteOp::DeleteUnpinned => db::delete_unpinned(&tx).map(|v| {
                    v.iter().for_each(|r| doom(&mut doomed, r));
                }),
                WriteOp::DeleteOlderThan { cutoff } => db::delete_older_than(&tx, cutoff)
                    .map(|v| v.iter().for_each(|r| doom(&mut doomed, r))),
                WriteOp::SetOcr { id, text, status } => {
                    set_ocr(&tx, &self.crypto, id, text, status)
                }
                // `Batch::push` keeps control ops out of `ops`.
                WriteOp::Flush(_) | WriteOp::Shutdown(_) => Ok(()),
            };
            if let Err(e) = result {
                errors += 1;
                eprintln!("clipdump: persist: write failed: {e:#}");
            }
        }
        if let Err(e) = tx.commit() {
            eprintln!("clipdump: persist: commit failed: {e}");
            errors += 1;
        } else {
            for (kind, name) in doomed {
                if let Err(e) = self.blobs.delete(kind, &name) {
                    // Not fatal: the startup integrity pass removes orphans.
                    eprintln!("clipdump: persist: {e:#}");
                }
            }
        }
        self.stats.batches.fetch_add(1, Ordering::Relaxed);
        self.stats.ops.fetch_add(n_ops, Ordering::Relaxed);
        self.stats.errors.fetch_add(errors, Ordering::Relaxed);
        errors == 0
    }
}

fn doom(doomed: &mut Vec<(FileKind, String)>, removed: &Removed) {
    if let Some(name) = &removed.blob_hash {
        doomed.push((FileKind::Blob, name.clone()));
    }
    doomed.push((FileKind::Thumb, hash_hex(&removed.hash)));
}

fn insert(
    conn: &Connection,
    crypto: &Crypto,
    blobs: &Blobs,
    item: &NewItem,
    blob: Option<Vec<u8>>,
    thumb: Option<Vec<u8>>,
) -> Result<()> {
    // Files first. If the blob cannot be written the row must not exist, or it
    // would reference a missing file.
    if let (Some(name), Some(bytes)) = (&item.blob_hash, &blob) {
        blobs.write(FileKind::Blob, name, bytes)?;
    }
    if let Some(bytes) = &thumb {
        // A missing thumbnail only costs a preview, so do not fail the insert.
        if let Err(e) = blobs.write(FileKind::Thumb, &hash_hex(&item.hash), bytes) {
            eprintln!("clipdump: persist: thumbnail not saved: {e:#}");
        }
    }
    let row = encrypt_item(crypto, item)?;
    db::insert_item(conn, &row)
}

fn encrypt_item(crypto: &Crypto, item: &NewItem) -> Result<ItemRow> {
    let enc_str = |s: &Option<String>| -> Result<Option<Vec<u8>>> {
        s.as_ref().map(|s| crypto.encrypt(s.as_bytes())).transpose()
    };
    let files_enc = item
        .files
        .as_ref()
        .map(|f| crypto.encrypt(&serde_json::to_vec(f)?))
        .transpose()?;
    Ok(ItemRow {
        id: item.id,
        hash: item.hash,
        kind: item.kind,
        created_at: item.created_at,
        last_used_at: item.last_used_at,
        use_count: item.use_count,
        pinned: item.pinned,
        preview_enc: crypto.encrypt(item.preview.as_bytes())?,
        text_enc: enc_str(&item.text)?,
        ocr_enc: enc_str(&item.ocr)?,
        ocr_status: item.ocr_status,
        files_enc,
        blob_hash: item.blob_hash.clone(),
        width: item.width,
        height: item.height,
        byte_size: item.byte_size,
        source_app: item.source_app.clone(),
    })
}

fn set_ocr(
    conn: &Connection,
    crypto: &Crypto,
    id: i64,
    text: Option<String>,
    status: OcrStatus,
) -> Result<()> {
    let enc = text.map(|t| crypto.encrypt(t.as_bytes())).transpose()?;
    db::set_ocr(conn, id, enc.as_deref(), status).map(|_| ())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::platform::insecure::InsecureKeyProtector;

    pub fn open(dir: &Path) -> Persist {
        Persist::open_with(dir, &InsecureKeyProtector).unwrap()
    }

    pub fn text_item(id: i64, text: &str) -> NewItem {
        let mut hash = [0u8; 16];
        hash[8..].copy_from_slice(&(id as u64).to_be_bytes());
        NewItem {
            id,
            hash,
            kind: ClipKind::Text,
            created_at: 1_000 + id,
            last_used_at: 1_000 + id,
            use_count: 0,
            pinned: false,
            preview: text.chars().take(300).collect(),
            text: Some(text.to_string()),
            ocr: None,
            ocr_status: OcrStatus::Pending,
            files: None,
            blob_hash: None,
            width: None,
            height: None,
            byte_size: text.len() as u64,
            source_app: None,
        }
    }

    pub fn image_item(id: i64) -> NewItem {
        let mut it = text_item(id, "");
        it.kind = ClipKind::Image;
        it.text = None;
        it.preview = format!("image {id}");
        it.blob_hash = Some(hash_hex(&it.hash));
        it.width = Some(64);
        it.height = Some(48);
        it
    }

    pub fn insert_op(item: NewItem) -> WriteOp {
        let has_blob = item.blob_hash.is_some();
        WriteOp::Insert {
            item: Box::new(item),
            blob: has_blob.then(|| vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4]),
            thumb: has_blob.then(|| b"RIFFthumb".to_vec()),
        }
    }

    fn ids(p: &Persist) -> Vec<i64> {
        let mut v: Vec<i64> = p.load_all_meta(0).unwrap().iter().map(|m| m.id).collect();
        v.sort();
        v
    }

    #[test]
    fn insert_flush_read_back_decrypts() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        let mut it = text_item(1, "hello world");
        it.ocr = Some("ocr words".into());
        it.files = Some(vec![FileRef {
            path: "C:\\a.txt".into(),
            size: 3,
            is_dir: false,
        }]);
        p.submit(insert_op(it)).unwrap();
        p.flush().unwrap();
        let meta = p.load_all_meta(1 << 20).unwrap();
        assert_eq!(meta.len(), 1);
        let m = &meta[0];
        let c = p.crypto();
        assert_eq!(c.decrypt(&m.preview_enc).unwrap(), b"hello world");
        assert_eq!(
            c.decrypt(m.text_enc.as_ref().unwrap()).unwrap(),
            b"hello world"
        );
        assert_eq!(
            c.decrypt(m.ocr_enc.as_ref().unwrap()).unwrap(),
            b"ocr words"
        );
        let files: Vec<FileRef> =
            serde_json::from_slice(&c.decrypt(m.files_enc.as_ref().unwrap()).unwrap()).unwrap();
        assert_eq!(files[0].path, "C:\\a.txt");
        assert_eq!(p.load_text(1).unwrap().as_deref(), Some("hello world"));
        assert_eq!(p.load_text(2).unwrap(), None);
        assert_eq!(p.max_id().unwrap(), 1);
    }

    #[test]
    fn image_insert_writes_blob_and_thumb_and_delete_removes_them() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        let it = image_item(7);
        let name = it.blob_hash.clone().unwrap();
        let hash = it.hash;
        p.submit(insert_op(it)).unwrap();
        p.flush().unwrap();
        assert_eq!(
            p.read_blob(&name).unwrap(),
            vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4]
        );
        assert_eq!(p.read_thumb(&hash).unwrap(), b"RIFFthumb");
        p.submit(WriteOp::Delete { id: 7 }).unwrap();
        p.flush().unwrap();
        assert!(ids(&p).is_empty());
        assert!(p.read_blob(&name).is_err());
        assert!(p.read_thumb(&hash).is_err());
    }

    #[test]
    fn update_pin_ocr_and_retention_ops_apply_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        for id in 1..=4 {
            p.submit(insert_op(text_item(id, &format!("item {id}"))))
                .unwrap();
        }
        p.submit(WriteOp::SetPinned {
            id: 1,
            pinned: true,
        })
        .unwrap();
        p.submit(WriteOp::UpdateUsed { id: 2, at: 99_999 }).unwrap();
        p.submit(WriteOp::SetOcr {
            id: 3,
            text: Some("scan".into()),
            status: OcrStatus::Done,
        })
        .unwrap();
        // created_at = 1000 + id: removes 2 (unpinned, older than 1003), keeps 1 (pinned).
        p.submit(WriteOp::DeleteOlderThan { cutoff: 1_003 })
            .unwrap();
        p.flush().unwrap();
        assert_eq!(ids(&p), vec![1, 3, 4]);
        let meta = p.load_all_meta(0).unwrap();
        let m3 = meta.iter().find(|m| m.id == 3).unwrap();
        assert_eq!(m3.ocr_status, OcrStatus::Done);
        assert_eq!(
            p.crypto().decrypt(m3.ocr_enc.as_ref().unwrap()).unwrap(),
            b"scan"
        );
        assert!(meta.iter().find(|m| m.id == 1).unwrap().pinned);
        // Clear history keeps the pinned item.
        p.submit(WriteOp::DeleteUnpinned).unwrap();
        p.flush().unwrap();
        assert_eq!(ids(&p), vec![1]);
    }

    #[test]
    fn data_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let p = open(dir.path());
            p.submit(insert_op(text_item(1, "persisted"))).unwrap();
            p.submit(insert_op(image_item(2))).unwrap();
            p.shutdown().unwrap();
        }
        let p = open(dir.path());
        assert_eq!(ids(&p), vec![1, 2]);
        assert_eq!(p.load_text(1).unwrap().as_deref(), Some("persisted"));
        assert_eq!(p.max_id().unwrap(), 2);
    }

    #[test]
    fn writer_batches_ops_into_few_transactions() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        let n = 200;
        for id in 1..=n {
            p.submit(insert_op(text_item(id, "x"))).unwrap();
        }
        p.flush().unwrap();
        let s = p.stats();
        assert_eq!(s.ops, n as u64);
        assert_eq!(s.errors, 0);
        // The cap guarantees at least ceil(200 / 32) transactions...
        assert!(
            s.batches >= (n as u64).div_ceil(MAX_BATCH_OPS as u64),
            "{s:?}"
        );
        // ...and batching means far fewer than one per op.
        assert!(s.batches <= n as u64 / 2, "{s:?}");
        assert_eq!(ids(&p).len(), n as usize);
    }

    #[test]
    fn ops_beyond_the_cap_start_a_new_batch() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        for id in 1..=64 {
            p.submit(insert_op(text_item(id, "x"))).unwrap();
        }
        p.flush().unwrap();
        assert!(
            p.stats().batches >= 2,
            "64 ops cannot fit in one batch of {MAX_BATCH_OPS}"
        );
    }

    #[test]
    fn single_op_is_committed_by_the_timer_without_flush() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        p.submit(insert_op(text_item(1, "timed"))).unwrap();
        let start = Instant::now();
        while ids(&p).is_empty() {
            assert!(start.elapsed() < Duration::from_secs(5), "never committed");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Committed after roughly one window, far below the 5 s guard.
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(p.stats().batches, 1);
    }

    #[test]
    fn flush_and_shutdown_drain_everything_queued() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        for id in 1..=100 {
            p.submit(insert_op(text_item(id, "queued"))).unwrap();
        }
        // No flush: shutdown alone must drain and commit.
        p.shutdown().unwrap();
        assert_eq!(ids(&p).len(), 100);
        assert!(
            p.submit(insert_op(text_item(101, "late"))).is_err(),
            "closed after shutdown"
        );
        assert!(p.shutdown().is_ok(), "idempotent");
    }

    #[test]
    fn drop_flushes_like_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        {
            let p = open(dir.path());
            for id in 1..=10 {
                p.submit(insert_op(text_item(id, "dropped"))).unwrap();
            }
        }
        assert_eq!(ids(&open(dir.path())).len(), 10);
    }

    #[test]
    fn a_failing_op_does_not_poison_its_batch() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        p.submit(insert_op(text_item(1, "ok"))).unwrap();
        p.submit(insert_op(text_item(1, "duplicate id"))).unwrap();
        p.submit(insert_op(text_item(2, "also ok"))).unwrap();
        assert!(p.flush().is_err(), "flush reports the failed op");
        assert_eq!(ids(&p), vec![1, 2]);
        assert_eq!(p.stats().errors, 1);
        p.flush().unwrap(); // a clean batch reports success again
    }

    #[test]
    fn delete_then_reinsert_in_one_batch_keeps_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        p.submit(insert_op(image_item(1))).unwrap();
        p.flush().unwrap();
        // Same content copied again right after the delete.
        let mut again = image_item(1);
        again.id = 2;
        let name = again.blob_hash.clone().unwrap();
        let hash = again.hash;
        p.submit(WriteOp::Delete { id: 1 }).unwrap();
        p.submit(insert_op(again)).unwrap();
        p.flush().unwrap();
        assert_eq!(ids(&p), vec![2]);
        assert!(p.read_blob(&name).is_ok());
        assert!(p.read_thumb(&hash).is_ok());
    }

    #[test]
    fn reads_do_not_wait_for_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        p.submit(insert_op(text_item(1, "a"))).unwrap();
        p.flush().unwrap();
        // Queue work, then read immediately from another connection.
        for id in 2..=30 {
            p.submit(insert_op(text_item(id, "b"))).unwrap();
        }
        assert!(!p.load_all_meta(0).unwrap().is_empty());
    }

    /// Every file under `dir`, with its raw bytes.
    fn all_files(dir: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(d).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let bytes = fs::read(&path).unwrap();
                    out.push((path, bytes));
                }
            }
        }
        out
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn nothing_on_disk_contains_plaintext() {
        const MARKER: &str = "ZXQ_PLAINTEXT_MARKER_9f3a7c";
        let dir = tempfile::tempdir().unwrap();

        // Positive control: the scan below really finds plaintext when it is there.
        let control = tempfile::tempdir().unwrap();
        let conn = Connection::open(control.path().join("control.db")).unwrap();
        conn.execute_batch("CREATE TABLE t(x TEXT)").unwrap();
        conn.execute("INSERT INTO t VALUES (?1)", [MARKER]).unwrap();
        drop(conn);
        assert!(
            all_files(control.path())
                .iter()
                .any(|(_, b)| contains(b, MARKER.as_bytes())),
            "control must be detectable"
        );

        let p = open(dir.path());
        // Text with the marker in preview and body, a large text, an image whose
        // OCR text, blob and thumbnail all carry the marker, and a file list.
        let mut text = text_item(1, &format!("body {MARKER} body"));
        text.preview = format!("preview {MARKER}");
        let mut big = text_item(2, &MARKER.repeat(20_000)); // over the 64 KB RAM limit
        big.preview = MARKER.into();
        let mut img = image_item(3);
        img.ocr = Some(format!("ocr {MARKER}"));
        let mut files = text_item(4, "");
        files.kind = ClipKind::Files;
        files.text = None;
        files.files = Some(vec![FileRef {
            path: format!("C:\\Users\\{MARKER}\\secret.txt"),
            size: 1,
            is_dir: false,
        }]);
        p.submit(insert_op(text)).unwrap();
        p.submit(insert_op(big)).unwrap();
        let img_hash = img.hash;
        let blob_name = img.blob_hash.clone().unwrap();
        p.submit(WriteOp::Insert {
            item: Box::new(img),
            blob: Some(format!("PNG {MARKER}").into_bytes().repeat(50)),
            thumb: Some(format!("WEBP {MARKER}").into_bytes()),
        })
        .unwrap();
        p.submit(insert_op(files)).unwrap();
        p.flush().unwrap();

        // Sanity: the data is really there and decrypts to the marker.
        assert!(p.load_text(1).unwrap().unwrap().contains(MARKER));
        assert!(contains(
            &p.read_blob(&blob_name).unwrap(),
            MARKER.as_bytes()
        ));
        assert!(contains(
            &p.read_thumb(&img_hash).unwrap(),
            MARKER.as_bytes()
        ));

        let check = |when: &str| {
            let files = all_files(dir.path());
            // history.db, blob and thumb files must all be among the scanned files.
            assert!(
                files.iter().any(|(p, _)| p.ends_with("history.db")),
                "{when}"
            );
            assert!(
                files
                    .iter()
                    .any(|(p, _)| p.starts_with(dir.path().join("blobs"))),
                "{when}"
            );
            assert!(
                files
                    .iter()
                    .any(|(p, _)| p.starts_with(dir.path().join("thumbs"))),
                "{when}"
            );
            for (path, bytes) in files {
                assert!(
                    !contains(&bytes, MARKER.as_bytes()),
                    "plaintext marker found in {} {when}",
                    path.display()
                );
            }
        };
        // 1. After commit, before any checkpoint: data lives in history.db-wal.
        assert!(
            dir.path().join("history.db-wal").metadata().unwrap().len() > 0,
            "this step must really scan a non empty WAL"
        );
        check("(before checkpoint, WAL present)");
        // 2. After shutdown: WAL folded into history.db.
        p.shutdown().unwrap();
        check("(after shutdown and checkpoint)");
        // 3. After a reopen as well.
        drop(p);
        let p = open(dir.path());
        assert!(p.load_text(1).unwrap().unwrap().contains(MARKER));
        drop(p);
        check("(after reopen)");
    }

    // -- crash resilience (Phase 13) --------------------------------------------

    /// Copy a data directory file by file, the way it is on disk right now: a
    /// stand in for the state a killed process leaves behind (WAL not
    /// checkpointed, writer thread gone without a word).
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let target = to.join(e.file_name());
            if e.path().is_dir() {
                copy_tree(&e.path(), &target);
            } else {
                fs::copy(e.path(), &target).unwrap();
            }
        }
    }

    /// `PRAGMA integrity_check` of the database file in `dir`.
    fn integrity_check(dir: &Path) -> String {
        let conn = Connection::open(dir.join(db::DB_FILE)).unwrap();
        conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap()
    }

    /// Fill `p` with 40 texts and 10 images, flush, and return the ids.
    fn fill_and_flush(p: &Persist) -> Vec<i64> {
        for id in 1..=40 {
            p.submit(insert_op(text_item(id, &format!("text number {id}"))))
                .unwrap();
        }
        for id in 41..=50 {
            p.submit(insert_op(image_item(id))).unwrap();
        }
        p.submit(WriteOp::SetPinned {
            id: 3,
            pinned: true,
        })
        .unwrap();
        p.flush().unwrap();
        (1..=50).collect()
    }

    #[test]
    fn a_killed_process_after_flush_loses_nothing_and_corrupts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        let flushed = fill_and_flush(&p);
        // Ops that are still queued or in the next batch when the "crash" hits:
        // they may or may not be on disk, but must never damage what is.
        for id in 51..=60 {
            p.submit(insert_op(text_item(id, &format!("late {id}"))))
                .unwrap();
        }
        let crash = tempfile::tempdir().unwrap();
        assert!(
            dir.path().join("history.db-wal").metadata().unwrap().len() > 0,
            "the snapshot must contain an un-checkpointed WAL"
        );
        copy_tree(dir.path(), crash.path());

        assert_eq!(integrity_check(crash.path()), "ok", "no corruption");
        let q = open(crash.path());
        let got = ids(&q);
        for id in &flushed {
            assert!(got.contains(id), "flushed item {id} was lost");
        }
        assert!(
            got.iter().all(|id| (1..=60).contains(id)),
            "no foreign rows: {got:?}"
        );
        for id in 1..=40 {
            assert_eq!(
                q.load_text(id).unwrap().as_deref(),
                Some(format!("text number {id}").as_str())
            );
        }
        // Every image row has its blob and thumbnail (the startup repair would
        // have dropped it otherwise; nothing may have been dropped).
        assert!(q.integrity_report().rows_dropped.is_empty());
        for id in 41..=50 {
            let it = image_item(id);
            assert_eq!(
                q.read_blob(it.blob_hash.as_ref().unwrap()).unwrap().len(),
                8
            );
            assert_eq!(q.read_thumb(&it.hash).unwrap(), b"RIFFthumb");
        }
        assert!(
            q.load_all_meta(0)
                .unwrap()
                .iter()
                .any(|m| m.id == 3 && m.pinned)
        );
        drop(p);
    }

    #[test]
    fn dropping_the_handles_without_a_shutdown_after_flush_keeps_the_data() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        let flushed = fill_and_flush(&p);
        // Leak the handle: no Drop, no Shutdown op, no checkpoint, the writer
        // thread just stays where it is, like a process that died.
        std::mem::forget(p);
        let q = open(dir.path());
        assert_eq!(ids(&q), flushed);
        assert_eq!(integrity_check(dir.path()), "ok");
    }

    #[test]
    fn a_torn_wal_tail_is_survived() {
        // A crash in the middle of appending to the WAL leaves a partial frame at
        // its end. SQLite ignores it: everything before it is still there.
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        let flushed = fill_and_flush(&p);
        let crash = tempfile::tempdir().unwrap();
        copy_tree(dir.path(), crash.path());
        let wal = crash.path().join("history.db-wal");
        let len = fs::metadata(&wal).unwrap().len();
        // Append half a frame of garbage.
        let mut f = fs::OpenOptions::new().append(true).open(&wal).unwrap();
        std::io::Write::write_all(&mut f, &[0xAB; 2_000]).unwrap();
        drop(f);
        assert!(fs::metadata(&wal).unwrap().len() > len);
        let q = open(crash.path());
        assert_eq!(ids(&q), flushed);
        assert_eq!(integrity_check(crash.path()), "ok");
        drop(p);
    }

    #[test]
    fn shutdown_then_reopen_is_complete_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(dir.path());
        let flushed = fill_and_flush(&p);
        p.submit(insert_op(text_item(99, "last one"))).unwrap();
        // No flush: shutdown itself must commit what is queued.
        p.shutdown().unwrap();
        p.shutdown().unwrap();
        assert!(p.submit(insert_op(text_item(100, "too late"))).is_err());
        drop(p);
        let q = open(dir.path());
        let mut want = flushed;
        want.push(99);
        assert_eq!(ids(&q), want);
        assert_eq!(
            fs::metadata(dir.path().join("history.db-wal")).map_or(0, |m| m.len()),
            0,
            "the WAL was folded into the database"
        );
    }
}
