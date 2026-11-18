//! Store tests: the test fixture, the operations, the pool to store to disk path,
//! the text in RAM rule, restart persistence and the cold start timing report.
//! Retention and cache have their own tests next to their code; they use
//! [`Fx`] from here.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::Duration;

use super::*;
use crate::ingest::image::tests::{dib24, pic_gradient, png_of};
use crate::ingest::{IngestPool, process, text};
use crate::persist::tests::open;
use crate::platform::{Captured, CapturedEvent, ImageData, ImageFormat, IngestMsg};

/// A fixed "now" far from zero, in unix ms.
pub const BASE: i64 = 1_700_000_000_000;
pub const DAY: i64 = 24 * 60 * 60 * 1000;

/// Records every OCR job the store hands out.
#[derive(Default)]
pub struct RecordingOcr(pub Mutex<Vec<OcrJob>>);

impl OcrQueue for RecordingOcr {
    fn enqueue(&self, job: OcrJob) {
        self.0.lock().unwrap().push(job);
    }
}

/// A store over a temp directory with a movable clock.
pub struct Fx {
    pub dir: tempfile::TempDir,
    pub store: Arc<Store>,
    pub settings: Arc<std::sync::RwLock<Settings>>,
    pub clock: Arc<AtomicI64>,
    pub ocr: Arc<RecordingOcr>,
    pub events: Arc<AtomicUsize>,
}

fn open_store(
    dir: &Path,
    settings: &Arc<std::sync::RwLock<Settings>>,
    clock: &Arc<AtomicI64>,
    ocr: &Arc<RecordingOcr>,
    events: &Arc<AtomicUsize>,
) -> Arc<Store> {
    let persist = Arc::new(open(dir));
    let e = Arc::clone(events);
    // Interval zero: every change emits at once, so tests can count events
    // without waiting out the 50 ms debounce.
    let changed = HistoryChanged::with_interval(Duration::ZERO, move || {
        e.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    let c = Arc::clone(clock);
    Arc::new(
        Store::open(
            persist,
            Arc::clone(settings),
            ocr.clone(),
            Some(changed),
            Arc::new(move || c.load(Ordering::SeqCst)),
        )
        .unwrap(),
    )
}

impl Fx {
    pub fn new() -> Self {
        Self::with(Settings::default())
    }

    pub fn with(settings: Settings) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let settings = Arc::new(std::sync::RwLock::new(settings));
        let clock = Arc::new(AtomicI64::new(BASE));
        let ocr = Arc::new(RecordingOcr::default());
        let events = Arc::new(AtomicUsize::new(0));
        let store = open_store(dir.path(), &settings, &clock, &ocr, &events);
        Self {
            dir,
            store,
            settings,
            clock,
            ocr,
            events,
        }
    }

    /// Flush, close and open again over the same directory: a restart.
    pub fn reopen(self) -> Self {
        let Fx {
            dir,
            store,
            settings,
            clock,
            ocr,
            events,
        } = self;
        store.persist().flush().unwrap();
        // The store holds the only `Persist` handle; dropping it shuts the
        // writer down and checkpoints the WAL.
        drop(store);
        let store = open_store(dir.path(), &settings, &clock, &ocr, &events);
        Self {
            dir,
            store,
            settings,
            clock,
            ocr,
            events,
        }
    }

    pub fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }

    pub fn advance(&self, ms: i64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }

    pub fn flush(&self) {
        self.store.persist().flush().unwrap();
    }

    /// Ids newest first.
    pub fn ids(&self) -> Vec<u64> {
        self.store
            .read()
            .iter_newest_first()
            .map(|i| i.id)
            .collect()
    }

    /// Everything, newest first, for whole state comparisons.
    pub fn snapshot(&self) -> Vec<ClipItem> {
        self.store
            .read()
            .iter_newest_first()
            .map(|i| (**i).clone())
            .collect()
    }

    /// Row ids in SQLite (after a flush), sorted.
    pub fn db_ids(&self) -> Vec<i64> {
        self.flush();
        let mut v: Vec<i64> = self
            .store
            .persist()
            .load_all_meta(0)
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect();
        v.sort_unstable();
        v
    }

    /// Insert a text created "now".
    pub fn text(&self, t: &str) -> u64 {
        self.text_at(t, self.now())
    }

    pub fn text_at(&self, t: &str, created_at: i64) -> u64 {
        let it = text::build(t, Some("notepad.exe".into()), created_at).unwrap();
        match self.store.insert(it) {
            Inserted::New(id) | Inserted::Bumped(id) => id,
        }
    }

    pub fn image(&self, n: u32, png_len: usize) -> u64 {
        self.image_at(n, png_len, self.now())
    }

    pub fn image_at(&self, n: u32, png_len: usize, created_at: i64) -> u64 {
        match self.store.insert(fake_image(n, png_len, created_at)) {
            Inserted::New(id) | Inserted::Bumped(id) => id,
        }
    }
}

/// An image item without decoding anything: the blob is `png_len` filler bytes.
pub fn fake_image(n: u32, png_len: usize, created_at: i64) -> IngestedItem {
    IngestedItem {
        kind: ClipKind::Image,
        hash: 0xA000_0000_0000_0000_0000_0000_0000_0000u128 | u128::from(n),
        preview: format!("image {n}"),
        search_text: String::new(),
        text: None,
        png: Some(vec![(n % 251) as u8; png_len]),
        thumb: Some(vec![(n % 241) as u8; 300]),
        files: None,
        width: 640,
        height: 480,
        byte_size: png_len as u64,
        source_app: Some("snip.exe".into()),
        created_at,
        ocr_status: OcrStatus::Pending,
    }
}

pub fn fake_files(n: u32, created_at: i64) -> IngestedItem {
    let files: Vec<FileRef> = (0..3u32)
        .map(|i| FileRef {
            path: format!("C:\\docs\\project{n}\\file{i}.txt"),
            size: 100 + u64::from(i),
            is_dir: false,
        })
        .collect();
    IngestedItem {
        kind: ClipKind::Files,
        hash: 0xB000_0000_0000_0000_0000_0000_0000_0000u128 | u128::from(n),
        preview: format!("file0.txt, file1.txt, file2.txt ({n})"),
        search_text: files::search_text(&files),
        text: None,
        png: None,
        thumb: n.is_multiple_of(2).then(|| vec![7u8; 200]),
        byte_size: files.iter().map(|f| f.size).sum(),
        files: Some(files),
        width: 0,
        height: 0,
        source_app: Some("explorer.exe".into()),
        created_at,
        ocr_status: OcrStatus::Done,
    }
}

fn ev(c: Captured, app: &str) -> IngestMsg {
    IngestMsg::Captured(CapturedEvent {
        captured: c,
        source_app: Some(app.into()),
    })
}

fn run_pool(store: &Arc<Store>, msgs: Vec<IngestMsg>) {
    let (tx, rx) = crossbeam_channel::bounded(32);
    let pool = IngestPool::start(rx, store.clone()).unwrap();
    for m in msgs {
        tx.send(m).unwrap();
    }
    drop(tx);
    pool.join();
}

// ---------------------------------------------------------------------------
// Pool to store to disk
// ---------------------------------------------------------------------------

#[test]
fn pool_to_store_end_to_end_with_dedup_and_restart() {
    let fx = Fx::new();
    let pic = pic_gradient(300, 200);
    let data_dir = tempfile::tempdir().unwrap();
    let file = data_dir.path().join("note.txt");
    std::fs::write(&file, b"abc").unwrap();

    run_pool(
        &fx.store,
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
    fx.flush();
    assert!(
        fx.events.load(Ordering::SeqCst) >= 1,
        "history:changed fired"
    );
    assert_eq!(
        fx.store.len(),
        3,
        "text, image and files, duplicates merged"
    );

    let items = fx.snapshot();
    let by_kind = |k: ClipKind| items.iter().find(|i| i.kind == k).unwrap().clone();
    let t = by_kind(ClipKind::Text);
    assert_eq!(&*t.preview, "hello world");
    assert_eq!(t.text_in_ram.as_deref(), Some("hello\nworld"));
    assert_eq!(&*t.search_text, "hello\nworld");
    assert_eq!(t.use_count, 1, "the LF copy bumped the CRLF item");
    // Two workers: whichever copy got there first created the item.
    assert!(matches!(
        t.source_app.as_deref(),
        Some("notepad.exe" | "code.exe")
    ));

    let i = by_kind(ClipKind::Image);
    assert_eq!(i.ocr_status, OcrStatus::Pending);
    assert_eq!((i.width, i.height), (300, 200));
    assert_eq!(i.use_count, 1, "the DIB copy bumped the PNG item");
    let png = fx.store.get_blob(i.id).unwrap();
    assert_eq!(crate::ingest::image::decode_png(&png).unwrap(), pic);
    let th = crate::ingest::image::decode_any(i.thumb.as_ref().unwrap()).unwrap();
    assert_eq!((th.width, th.height), (144, 96));

    let f = by_kind(ClipKind::Files);
    assert_eq!(f.files.as_ref().unwrap().len(), 1);
    assert_eq!(f.byte_size, 3);
    assert!(f.search_text.contains("note.txt"));

    // OCR hook: exactly one job, for the one image item.
    {
        let jobs = fx.ocr.0.lock().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].item_id, i.id as i64);
        assert_eq!(jobs[0].hash, i.hash);
        assert_eq!((jobs[0].width, jobs[0].height), (300, 200));
    }
    // Restart: dedup still works, ids are not reused, Bump moves to the top.
    let fx = fx.reopen();
    let known_hash = fx
        .snapshot()
        .iter()
        .find(|i| i.kind == ClipKind::Files)
        .unwrap()
        .hash;
    // One pool run per step: two workers would make the order of these
    // independent messages arbitrary.
    run_pool(
        &fx.store,
        vec![ev(Captured::Text("hello world".into()), "x.exe")],
    );
    run_pool(&fx.store, vec![IngestMsg::Bump { hash: 0xdead }]);
    run_pool(&fx.store, vec![IngestMsg::Bump { hash: known_hash }]);
    assert_eq!(
        fx.store.len(),
        4,
        "new text added, bump of an unknown hash ignored"
    );
    let max = fx.ids().into_iter().max().unwrap();
    assert_eq!(max, 4, "ids continue after the restart");
    let top = fx.snapshot();
    assert_eq!(
        top[0].kind,
        ClipKind::Files,
        "the bump moved the file item up"
    );
    assert_eq!(top[0].use_count, 0, "an echo does not count a use");
    assert_eq!(fx.ocr.0.lock().unwrap().len(), 1, "no new OCR job");
}

#[test]
fn same_picture_from_png_and_dib_is_one_item() {
    let fx = Fx::new();
    let pic = pic_gradient(120, 90);
    let png = process(
        CapturedEvent {
            captured: Captured::Image(ImageData {
                format: ImageFormat::Png,
                bytes: png_of(&pic),
            }),
            source_app: None,
        },
        fx.now(),
    )
    .unwrap()
    .unwrap();
    let dib = process(
        CapturedEvent {
            captured: Captured::Image(ImageData {
                format: ImageFormat::Dib,
                bytes: dib24(&pic, true),
            }),
            source_app: None,
        },
        fx.now(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(png.hash, dib.hash, "pixel hash is format independent");
    let Inserted::New(id) = fx.store.insert(png) else {
        panic!("first copy is new")
    };
    fx.text("something newer");
    assert_eq!(fx.store.insert(dib), Inserted::Bumped(id));
    assert_eq!(fx.store.len(), 2);
    assert_eq!(fx.ids()[0], id, "moved to the top");
    assert_eq!(fx.store.get(id).unwrap().use_count, 1);
    assert_eq!(fx.ocr.0.lock().unwrap().len(), 1, "OCR queued once");
    assert_eq!(fx.db_ids().len(), 2);
}

#[test]
fn concurrent_duplicates_make_one_item_and_no_failed_writes() {
    let fx = Fx::new();
    std::thread::scope(|s| {
        for t in 0..4u32 {
            let store = &fx.store;
            s.spawn(move || {
                for n in 0..50u32 {
                    // Four threads, 50 items each, all hashes shared.
                    let it =
                        text::build(&format!("shared {n}"), None, BASE + i64::from(t)).unwrap();
                    store.insert(it);
                }
            });
        }
    });
    fx.flush();
    assert_eq!(fx.store.len(), 50);
    let total_uses: u32 = fx.snapshot().iter().map(|i| i.use_count).sum();
    assert_eq!(total_uses, 150, "200 inserts, 50 of them new");
    assert_eq!(
        fx.store.persist().stats().errors,
        0,
        "no op ran before its insert"
    );
    let reloaded = fx.reopen();
    assert_eq!(reloaded.store.len(), 50);
    let uses: u32 = reloaded.snapshot().iter().map(|i| i.use_count).sum();
    assert_eq!(uses, 150, "use counts persisted exactly");
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

#[test]
fn dedup_moves_to_top_and_updates_last_used_at() {
    let fx = Fx::new();
    let a = fx.text("alpha");
    let b = fx.text("beta");
    let c = fx.text("gamma");
    assert_eq!(fx.ids(), vec![c, b, a]);
    let before = fx.store.get(a).unwrap().last_used_at;
    fx.advance(10);
    assert_eq!(
        fx.store
            .insert(text::build("alpha", None, fx.now()).unwrap()),
        Inserted::Bumped(a)
    );
    assert_eq!(fx.ids(), vec![a, c, b]);
    let it = fx.store.get(a).unwrap();
    assert_eq!(it.use_count, 1);
    assert!(it.last_used_at > before);
    assert_eq!(it.created_at, BASE, "created_at is not touched");
    assert_eq!(fx.store.len(), 3);
}

#[test]
fn last_used_at_is_strictly_increasing_even_within_one_millisecond() {
    let fx = Fx::new();
    // The clock never moves: every stamp must still be unique and ordered.
    let ids: Vec<u64> = (0..20).map(|n| fx.text(&format!("t{n}"))).collect();
    for &id in &ids[..10] {
        assert!(fx.store.mark_used(id));
    }
    let snap = fx.snapshot();
    assert!(
        snap.windows(2)
            .all(|w| w[0].last_used_at > w[1].last_used_at)
    );
}

#[test]
fn get_remove_deletes_files_and_row() {
    let fx = Fx::new();
    let img = fx.image(1, 1000);
    let t = fx.text("keep me");
    fx.flush();
    let blob_name = fx.store.get(img).unwrap().blob_hash.clone().unwrap();
    let hash = fx.store.get(img).unwrap().hash.to_be_bytes();
    assert!(fx.store.persist().read_blob(&blob_name).is_ok());
    assert!(fx.store.persist().read_thumb(&hash).is_ok());

    assert!(fx.store.remove(img));
    assert!(!fx.store.remove(img), "already gone");
    assert!(fx.store.get(img).is_none());
    assert_eq!(fx.ids(), vec![t]);
    fx.flush();
    assert!(
        fx.store.persist().read_blob(&blob_name).is_err(),
        "blob file deleted"
    );
    assert!(
        fx.store.persist().read_thumb(&hash).is_err(),
        "thumb file deleted"
    );
    assert_eq!(fx.db_ids(), vec![t as i64]);
    // The same content can be stored again afterwards.
    let again = fx.image(1, 1000);
    assert_ne!(again, img);
    fx.flush();
    assert!(fx.store.persist().read_blob(&blob_name).is_ok());
    assert_eq!(fx.store.persist().stats().errors, 0);
}

#[test]
fn remove_deletes_a_pinned_item_too() {
    let fx = Fx::new();
    let a = fx.text("pinned");
    fx.store.toggle_pin(a);
    assert!(fx.store.remove(a), "an explicit delete wins over a pin");
    assert_eq!(fx.store.len(), 0);
    assert_eq!(fx.store.read().unpinned_count(), 0);
}

#[test]
fn toggle_pin_flips_counts_and_persists() {
    let fx = Fx::new();
    let a = fx.text("a");
    let b = fx.text("b");
    assert_eq!(fx.store.toggle_pin(a), Some(true));
    assert_eq!(fx.store.read().pinned_count(), 1);
    assert_eq!(fx.store.read().unpinned_count(), 1);
    assert_eq!(fx.store.toggle_pin(999), None);
    // Pinning does not reorder.
    assert_eq!(fx.ids(), vec![b, a]);
    let fx = fx.reopen();
    assert!(fx.store.get(a).unwrap().pinned);
    assert!(!fx.store.get(b).unwrap().pinned);
    assert_eq!(fx.store.toggle_pin(a), Some(false));
    assert_eq!(fx.store.stats().pinned, 0);
    let fx = fx.reopen();
    assert!(!fx.store.get(a).unwrap().pinned);
}

#[test]
fn mark_used_moves_to_top_counts_and_persists() {
    let fx = Fx::new();
    let a = fx.text("a");
    let b = fx.text("b");
    assert!(fx.store.mark_used(a));
    assert!(fx.store.mark_used(a));
    assert!(!fx.store.mark_used(77));
    assert_eq!(fx.ids(), vec![a, b]);
    assert_eq!(fx.store.get(a).unwrap().use_count, 2);
    let fx = fx.reopen();
    assert_eq!(fx.ids(), vec![a, b]);
    assert_eq!(fx.store.get(a).unwrap().use_count, 2);
}

#[test]
fn bump_by_hash_moves_the_item_and_ignores_unknown_hashes() {
    let fx = Fx::new();
    let a = fx.text("a");
    let _b = fx.text("b");
    let hash = fx.store.get(a).unwrap().hash;
    ItemSink::bump(&*fx.store, hash);
    ItemSink::bump(&*fx.store, 12345);
    assert_eq!(fx.ids()[0], a);
    assert_eq!(
        fx.store.get(a).unwrap().use_count,
        0,
        "an echo moves the item but does not count a second use"
    );
    assert_eq!(fx.store.len(), 2);
    // The move is persisted like any other use.
    let fx = fx.reopen();
    assert_eq!(fx.ids()[0], a);
}

#[test]
fn copy_then_echo_counts_one_use() {
    let fx = Fx::new();
    let a = fx.text("a");
    let _b = fx.text("b");
    let hash = fx.store.get(a).unwrap().hash;
    assert!(fx.store.mark_used(a)); // copy_item
    ItemSink::bump(&*fx.store, hash); // the suppressed echo of that write
    assert_eq!(fx.ids()[0], a);
    assert_eq!(fx.store.get(a).unwrap().use_count, 1);
    assert_eq!(fx.store.len(), 2, "no duplicate item");
}

#[test]
fn set_ocr_text_updates_search_text_and_persists() {
    let fx = Fx::new();
    let img = fx.image(3, 500);
    assert_eq!(fx.store.get(img).unwrap().search_text.len(), 0);
    assert!(
        fx.store
            .set_ocr_text(img, Some("invoice total 42".into()), OcrStatus::Done)
    );
    let it = fx.store.get(img).unwrap();
    assert_eq!(&*it.search_text, "invoice total 42");
    assert_eq!(it.ocr_status, OcrStatus::Done);
    assert!(!fx.store.set_ocr_text(404, None, OcrStatus::Failed));

    // A long OCR text keeps only 10,000 chars in RAM, all of it on disk.
    let img2 = fx.image(4, 500);
    let long: String = "word ".repeat(5_000);
    assert!(
        fx.store
            .set_ocr_text(img2, Some(long.clone()), OcrStatus::Done)
    );
    assert_eq!(
        fx.store.get(img2).unwrap().search_text.chars().count(),
        10_000
    );
    // A failure clears nothing it should not and records the status.
    let img3 = fx.image(5, 500);
    assert!(fx.store.set_ocr_text(img3, None, OcrStatus::Unavailable));
    assert_eq!(
        fx.store.get(img3).unwrap().ocr_status,
        OcrStatus::Unavailable
    );

    let fx = fx.reopen();
    assert_eq!(&*fx.store.get(img).unwrap().search_text, "invoice total 42");
    assert_eq!(
        fx.store.get(img2).unwrap().search_text.chars().count(),
        10_000
    );
    assert_eq!(
        fx.store.get(img3).unwrap().ocr_status,
        OcrStatus::Unavailable
    );
    let meta = fx.store.persist().load_all_meta(0).unwrap();
    let row = meta.iter().find(|m| m.id == img2 as i64).unwrap();
    let full = fx
        .store
        .persist()
        .crypto()
        .decrypt(row.ocr_enc.as_ref().unwrap())
        .unwrap();
    assert_eq!(full, long.as_bytes(), "the database has the whole OCR text");
}

#[test]
fn set_ocr_text_does_not_disturb_a_reader_snapshot() {
    let fx = Fx::new();
    let img = fx.image(1, 100);
    let before = fx.store.get(img).unwrap();
    fx.store
        .set_ocr_text(img, Some("new".into()), OcrStatus::Done);
    assert_eq!(before.search_text.len(), 0, "the old snapshot is unchanged");
    assert_eq!(&*fx.store.get(img).unwrap().search_text, "new");
}

#[test]
fn clear_unpinned_keeps_pins_and_queues_one_delete() {
    let fx = Fx::new();
    let keep = fx.text("keep");
    let img = fx.image(2, 400);
    let gone = fx.text("gone");
    fx.store.toggle_pin(keep);
    fx.flush();
    let blob_name = fx.store.get(img).unwrap().blob_hash.clone().unwrap();
    assert_eq!(fx.store.clear_unpinned(), 2);
    assert_eq!(fx.store.clear_unpinned(), 0);
    assert_eq!(fx.ids(), vec![keep]);
    assert!(fx.store.get(gone).is_none());
    assert_eq!(fx.db_ids(), vec![keep as i64]);
    assert!(fx.store.persist().read_blob(&blob_name).is_err());
    assert_eq!(fx.store.stats().items, 1);
}

#[test]
fn history_changed_fires_for_every_kind_of_change() {
    let fx = Fx::new();
    let count = || fx.events.load(Ordering::SeqCst);
    let id = fx.text("a");
    let mut last = count();
    assert!(last >= 1, "insert");
    let mut expect_event = |what: &str| {
        assert!(count() > last, "no history:changed after {what}");
        last = count();
    };
    fx.store.toggle_pin(id);
    expect_event("pin");
    fx.store.mark_used(id);
    expect_event("mark_used");
    fx.store.set_ocr_text(id, Some("x".into()), OcrStatus::Done);
    expect_event("OCR");
    fx.text("a");
    expect_event("a duplicate copy");
    fx.store.remove(id);
    expect_event("remove");
    fx.text("b");
    fx.store.clear_unpinned();
    expect_event("clear");
    // Nothing changed: nothing is emitted.
    let before = count();
    fx.store.clear_unpinned();
    fx.store.remove(12345);
    fx.store.mark_used(12345);
    assert_eq!(count(), before);
}

// ---------------------------------------------------------------------------
// Text in RAM rule
// ---------------------------------------------------------------------------

#[test]
fn text_up_to_64kb_stays_in_ram_bigger_loads_on_demand() {
    let fx = Fx::new();
    let small = "s".repeat(TEXT_IN_RAM_MAX);
    let big: String = ["line of text with words\n"; 4000].concat(); // 96,000 bytes
    assert!(big.len() > TEXT_IN_RAM_MAX);
    let s = fx.text(&small);
    let b = fx.text(&big);

    let si = fx.store.get(s).unwrap();
    assert_eq!(
        si.text_in_ram.as_deref().map(str::len),
        Some(TEXT_IN_RAM_MAX)
    );
    assert_eq!(si.search_text.chars().count(), 10_000);
    let bi = fx.store.get(b).unwrap();
    assert!(bi.text_in_ram.is_none(), "over 64 KB is not kept in RAM");
    assert_eq!(bi.search_text.chars().count(), 10_000, "search slice only");
    assert_eq!(bi.byte_size, big.len() as u64);

    fx.flush();
    // Small: served from RAM, nothing is cached.
    assert_eq!(&*fx.store.get_full_text(s).unwrap(), small.as_bytes());
    assert_eq!(fx.store.cache_bytes(), 0);
    // Big: loaded from SQLite, decrypted, cached.
    assert_eq!(&*fx.store.get_full_text(b).unwrap(), big.as_bytes());
    assert_eq!(fx.store.cache_bytes(), big.len());
    assert_eq!(&*fx.store.get_full_text(b).unwrap(), big.as_bytes());
    assert_eq!(
        fx.store.cache_bytes(),
        big.len(),
        "second read is a cache hit"
    );
    assert!(fx.store.get_full_text(9999).is_err());
    let img = fx.image(1, 10);
    assert!(fx.store.get_full_text(img).is_err(), "not a text item");

    // Same after a restart: the big text is not loaded into RAM, only its slice.
    let fx = fx.reopen();
    let bi = fx.store.get(b).unwrap();
    assert!(bi.text_in_ram.is_none());
    assert_eq!(bi.search_text.chars().count(), 10_000);
    assert_eq!(fx.store.cache_bytes(), 0, "startup does not fill the cache");
    assert_eq!(&*fx.store.get_full_text(b).unwrap(), big.as_bytes());
    assert!(fx.store.get(s).unwrap().text_in_ram.is_some());
}

#[test]
fn short_text_shares_one_allocation_for_search_and_full_text() {
    let fx = Fx::new();
    let id = fx.text("short text");
    let it = fx.store.get(id).unwrap();
    assert!(Arc::ptr_eq(
        it.text_in_ram.as_ref().unwrap(),
        &it.search_text
    ));
    let fx = fx.reopen();
    let it = fx.store.get(id).unwrap();
    assert!(Arc::ptr_eq(
        it.text_in_ram.as_ref().unwrap(),
        &it.search_text
    ));
}

// ---------------------------------------------------------------------------
// Blobs and the cache through the store
// ---------------------------------------------------------------------------

#[test]
fn get_blob_loads_from_disk_then_hits_the_cache() {
    let fx = Fx::new();
    let a = fx.image(1, 1_000);
    fx.flush();
    assert_eq!(fx.store.cache_bytes(), 0);
    let blob = fx.store.get_blob(a).unwrap();
    assert_eq!(&*blob, &vec![1u8; 1_000][..]);
    assert_eq!(fx.store.cache_bytes(), 1_000);
    let again = fx.store.get_blob(a).unwrap();
    assert!(Arc::ptr_eq(&blob, &again), "second read is the cached Arc");
    // Removing the item drops its cache entry.
    fx.store.remove(a);
    assert_eq!(fx.store.cache_bytes(), 0);
    assert!(fx.store.get_blob(a).is_err());
    let t = fx.text("not an image");
    assert!(fx.store.get_blob(t).is_err());
}

#[test]
fn cache_stays_under_budget_and_oversized_blobs_are_not_cached() {
    let fx = Fx::with(Settings {
        ram_cache_bytes: 2_500,
        ..Settings::default()
    });
    let ids: Vec<u64> = (0..4).map(|n| fx.image(n, 1_000)).collect();
    let huge = fx.image(10, 3_000);
    fx.flush();
    for &id in &ids {
        fx.store.get_blob(id).unwrap();
        assert!(fx.store.cache_bytes() <= 2_500);
    }
    assert_eq!(fx.store.cache_bytes(), 2_000, "two 1,000 byte blobs fit");
    // The oldest two were evicted, the newest two are cached.
    let big = fx.store.get_blob(huge).unwrap();
    assert_eq!(big.len(), 3_000, "returned although it is over the budget");
    assert_eq!(
        fx.store.cache_bytes(),
        2_000,
        "and not cached, nothing evicted"
    );
    // Lowering the budget (a setting) evicts right away.
    fx.settings.write().unwrap().ram_cache_bytes = 1_000;
    fx.store.apply_settings();
    assert!(fx.store.cache_bytes() <= 1_000);
}

// ---------------------------------------------------------------------------
// Restart
// ---------------------------------------------------------------------------

#[test]
fn reload_after_restart_gives_identical_state() {
    let fx = Fx::new();
    let t1 = fx.text("first text, with some words");
    let big_text: String = (0..3000)
        .map(|n| format!("line {n} of a long log\n"))
        .collect();
    let t_big = fx.text(&big_text);
    let img_ocr = fx.image(1, 5_000);
    let img_plain = fx.image(2, 4_000);
    let (Inserted::New(files_a) | Inserted::Bumped(files_a)) =
        fx.store.insert(fake_files(1, fx.now()));
    let (Inserted::New(files_b) | Inserted::Bumped(files_b)) =
        fx.store.insert(fake_files(2, fx.now()));
    let removed = fx.text("this one is deleted");
    fx.store
        .set_ocr_text(img_ocr, Some("scanned words here".into()), OcrStatus::Done);
    fx.store.toggle_pin(t1);
    fx.store.toggle_pin(img_plain);
    fx.advance(5);
    fx.store.mark_used(t_big);
    fx.store.mark_used(t_big);
    fx.store.mark_used(files_a);
    fx.text("first text, with some words"); // dedup bump of t1
    fx.store.remove(removed);
    let after_edits = fx.text("last one");
    fx.store.toggle_pin(files_b);
    fx.store.toggle_pin(files_b); // pinned then unpinned again

    let before = fx.snapshot();
    let before_stats = fx.store.stats();
    assert_eq!(before.len(), 7);

    let fx = fx.reopen();
    assert_eq!(
        fx.snapshot(),
        before,
        "items, order, pins, counts, OCR, thumbs"
    );
    let after_stats = fx.store.stats();
    assert_eq!(after_stats.items, before_stats.items);
    assert_eq!(after_stats.pinned, before_stats.pinned);
    assert_eq!(after_stats.disk_bytes, before_stats.disk_bytes);
    assert_eq!(fx.ids()[0], after_edits);

    // And again, to be sure that loading does not change what gets saved.
    let fx = fx.reopen();
    assert_eq!(fx.snapshot(), before);
    // New ids continue after the highest used one.
    let n = fx.text("after restart");
    assert_eq!(n, after_edits + 1);
}

#[test]
fn unreadable_rows_are_dropped_at_startup_and_do_not_block_recopy() {
    let fx = Fx::new();
    fx.text("fine");
    fx.flush();
    // A second store over the same data with a different key cannot decrypt.
    drop(fx.store);
    let dir = fx.dir;
    std::fs::remove_file(dir.path().join("key.bin")).unwrap();
    let settings = fx.settings;
    let clock = fx.clock;
    let ocr = fx.ocr;
    let events = fx.events;
    let store = open_store(dir.path(), &settings, &clock, &ocr, &events);
    assert_eq!(store.len(), 0, "undecryptable item dropped, no crash");
    store.persist().flush().unwrap();
    assert!(
        store.persist().load_all_meta(0).unwrap().is_empty(),
        "and its row removed"
    );
}

// ---------------------------------------------------------------------------
// Cold start report
// ---------------------------------------------------------------------------

/// Fill a store with 1,000 realistic items: mostly short and medium texts, some
/// large logs, screenshots with thumbnails and OCR text, and file lists.
fn fill_realistic(fx: &Fx) {
    let words = [
        "invoice", "meeting", "report", "function", "const", "return", "http", "error", "total",
        "customer", "release", "branch", "merge", "deploy", "config", "value",
    ];
    let sentence = |seed: usize, n: usize| -> String {
        (0..n)
            .map(|k| words[(seed * 7 + k * 3) % words.len()])
            .collect::<Vec<_>>()
            .join(" ")
    };
    for n in 0..1000usize {
        fx.advance(1);
        match n % 10 {
            // 6 in 10: text, mostly short, a few big.
            0..=5 => {
                let len = match n % 60 {
                    0 => 150_000,
                    1..=5 => 6_000,
                    _ => 120,
                };
                let mut t = format!("{n} ");
                let mut k = 0;
                while t.len() < len {
                    t.push_str(&sentence(n + k, 12));
                    t.push('\n');
                    k += 1;
                }
                fx.text(&t);
            }
            // 3 in 10: screenshots; blob content is not read at startup.
            6..=8 => {
                let id = fx.image(n as u32, 30_000);
                if n % 3 != 0 {
                    let ocr = (0..40)
                        .map(|k| sentence(n + k, 10))
                        .collect::<Vec<_>>()
                        .join("\n");
                    fx.store.set_ocr_text(id, Some(ocr), OcrStatus::Done);
                }
            }
            _ => {
                fx.store.insert(fake_files(n as u32, fx.now()));
            }
        }
    }
}

#[test]
fn cold_start_with_1000_items_report() {
    let fx = Fx::with(Settings {
        max_items: 0,
        ..Settings::default()
    });
    fill_realistic(&fx);
    assert_eq!(fx.store.len(), 1000);
    let before = fx.snapshot();
    let fx = fx.reopen();
    // `reopen` has just done a cold open; time two more with a warm file cache.
    let mut times = Vec::new();
    let mut fx = fx;
    for _ in 0..3 {
        fx = fx.reopen();
        times.push(fx.store.load_time());
    }
    assert_eq!(fx.store.len(), 1000);
    assert_eq!(fx.snapshot(), before, "same 1,000 items after loading");
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    eprintln!(
        "cold start, 1,000 items ({build} build, Linux, NOT a Windows number): \
         Store::open took {times:?}"
    );
    // Not a performance gate (CI machines vary); this only catches a hang or an
    // accidentally quadratic load. The 300 ms budget is checked on Windows.
    assert!(times.iter().all(|t| t.as_secs() < 30));
}

// -- search through the store (Phase 8) ------------------------------------------

#[test]
fn store_search_finds_text_ocr_and_files_and_survives_a_restart() {
    use crate::search::{MatchSource, PathChecker, SearchRequest, TypeFilter};

    struct Gone;
    impl PathChecker for Gone {
        fn exists(&self, _: &str) -> bool {
            false
        }
    }
    let run = |fx: &Fx, q: &str| {
        fx.store
            .search(&SearchRequest::new(q, TypeFilter::All, fx.now()), &Gone)
    };

    let fx = Fx::new();
    let txt = fx.text("quarterly budget meeting notes");
    let img = fx.image(1, 100);
    fx.store
        .set_ocr_text(img, Some("invoice total 42 euro".into()), OcrStatus::Done);
    let files = match fx.store.insert(fake_files(1, fx.now())) {
        Inserted::New(id) | Inserted::Bumped(id) => id,
    };
    let fx = fx.reopen();

    let r = run(&fx, "budget");
    assert_eq!(r.iter().map(|d| d.id).collect::<Vec<_>>(), [txt]);
    assert_eq!(r[0].match_source, MatchSource::Text);

    // The lowercase twin is rebuilt from the stored OCR text at load, and a
    // capitalized query still finds it (smart case: "Invoice" is case sensitive,
    // so it must not).
    assert_eq!(
        &*fx.store.get(img).unwrap().search_lower,
        "invoice total 42 euro"
    );
    assert!(run(&fx, "Invoice").is_empty());

    let r = run(&fx, "invoice");
    assert_eq!(r.iter().map(|d| d.id).collect::<Vec<_>>(), [img]);
    assert_eq!(r[0].match_source, MatchSource::Ocr);
    assert!(r[0].snippet.as_deref().unwrap().contains("invoice"));
    assert!(r[0].has_thumb);

    let r = run(&fx, "file1");
    assert_eq!(r.iter().map(|d| d.id).collect::<Vec<_>>(), [files]);
    assert_eq!((r[0].file_count, r[0].missing_files), (3, 3));

    // Empty query: all three, newest first.
    assert_eq!(run(&fx, "").len(), 3);
    // Searching holds no lock afterwards: a write goes through.
    fx.text("another one");
    assert_eq!(run(&fx, "another").len(), 1);
}
