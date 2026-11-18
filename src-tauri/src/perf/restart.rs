//! The restart test of Phase 14 and the cold start measurement.
//!
//! `restart_keeps_items_pins_and_ocr_search` is an automated version of "reboot
//! the PC, open the bar, confirm all items, pins and OCR search still work":
//! it fills a history through the real pipeline (ingest pool, store, persistence,
//! the OCR worker with an engine that answers with known words), pins a few
//! items, and then checks that the whole state, and the answers to a set of
//! searches including one for an OCR word, are identical
//!
//! * after a graceful shutdown (the writer flushed and checkpointed) and a reopen,
//! * after a kill: the data folder copied while the store is still running, WAL
//!   and all (what a power cut or a task manager kill leaves), after a flush,
//!   and then opened.
//!
//! `cold_start_1000_items_300_screenshots` (ignored, run by hand with the perf
//! profile) measures the "load 1,000 items from disk and ready to search" budget
//! on a history with 300 screenshots that carry real thumbnails and OCR text; the
//! same population can be written to a folder (`CLIPDUMP_RESTART_KEEP_DIR`) for
//! the real app's `--perf` startup line.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::stress::{
    FileFixtures, Pictures, Rng, code, open_store, paragraphs, sentence, snapshot, url,
};
use crate::ingest::IngestPool;
use crate::model::{ClipKind, OcrStatus};
use crate::ocr::{OcrConfig, OcrEngine, OcrQueueHandle, OcrWorker};
use crate::platform::{Captured, CapturedEvent, IngestMsg};
use crate::search::{CachedPathChecker, ResultDto, SearchRequest, TypeFilter};
use crate::settings::Settings;
use crate::store::Store;

/// The word every mock OCR text contains.
const OCR_WORD: &str = "restartocr";

/// An engine that returns `chars` characters of words plus the word above and a
/// token that is different for every picture (a checksum of its pixels).
struct WordOcr {
    chars: usize,
}

impl OcrEngine for WordOcr {
    fn recognize(&self, rgba: &[u8], w: u32, h: u32) -> anyhow::Result<String> {
        let sum: u64 = rgba.iter().step_by(4099).map(|&b| u64::from(b)).sum();
        let mut rng = Rng(sum | 1);
        let mut t = format!("{OCR_WORD} snap{sum:x} {w}x{h}\n");
        while t.len() < self.chars {
            let n = rng.range(5, 12);
            t.push_str(&sentence(&mut rng, n));
            t.push('\n');
        }
        Ok(t)
    }

    fn available(&self) -> bool {
        true
    }
}

fn text_msg(t: String) -> IngestMsg {
    IngestMsg::Captured(CapturedEvent {
        captured: Captured::Text(t),
        source_app: Some("restart.exe".into()),
    })
}

/// What the population is made of.
struct Mix {
    texts: usize,
    big_texts: usize,
    images: usize,
    files: usize,
    width: u32,
    height: u32,
    ocr_chars: usize,
}

/// A running store with its OCR worker, filled by `fill`.
struct Live {
    store: Arc<Store>,
    ocr_queue: OcrQueueHandle,
    worker: OcrWorker,
}

/// Send the mix through the ingest pool, wait for the pool and the OCR worker to
/// finish, and flush.
fn fill(dir: &Path, settings: &Arc<std::sync::RwLock<Settings>>, mix: &Mix) -> Live {
    let ocr_queue = OcrQueueHandle::new(Arc::clone(settings));
    let store = open_store(dir, settings, &ocr_queue);
    let chars = mix.ocr_chars;
    let worker = OcrWorker::start(
        ocr_queue.clone(),
        Arc::downgrade(&store),
        Box::new(move || Box::new(WordOcr { chars })),
        OcrConfig::default(),
    )
    .expect("ocr worker");
    let (tx, rx) = crossbeam_channel::bounded(8);
    let pool = IngestPool::start(rx, store.clone()).expect("pool");

    let pics = Pictures::new(mix.width, mix.height);
    let files = FileFixtures::new();
    let mut rng = Rng(0xFEED_0007);
    let mut order: Vec<u8> = Vec::new();
    order.extend(std::iter::repeat_n(0, mix.texts));
    order.extend(std::iter::repeat_n(1, mix.big_texts));
    order.extend(std::iter::repeat_n(2, mix.images));
    order.extend(std::iter::repeat_n(3, mix.files));
    // A fixed shuffle, so kinds interleave like real use.
    for k in (1..order.len()).rev() {
        order.swap(k, rng.below(k as u64 + 1) as usize);
    }
    for (i, kind) in order.into_iter().enumerate() {
        let msg = match kind {
            0 => match i % 3 {
                0 => text_msg(format!("{i} budget {}", sentence(&mut rng, 8))),
                1 => text_msg(url(&mut rng, i)),
                _ => text_msg(code(&mut rng, i)),
            },
            1 => {
                let b = rng.range(6_000, 150_000);
                text_msg(paragraphs(&mut rng, i, b))
            }
            2 => IngestMsg::Captured(CapturedEvent {
                captured: Captured::Image(pics.image_data(i as u32)),
                source_app: Some("restart.exe".into()),
            }),
            _ => IngestMsg::Captured(CapturedEvent {
                captured: Captured::Files(files.list(&mut rng, i)),
                source_app: Some("restart.exe".into()),
            }),
        };
        tx.send(msg).expect("send");
    }
    drop(tx);
    pool.join();
    let deadline = Instant::now() + Duration::from_secs(120);
    while !ocr_queue.is_idle() {
        assert!(Instant::now() < deadline, "OCR did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
    store.persist().flush().expect("flush");
    Live {
        store,
        ocr_queue,
        worker,
    }
}

/// The same searches before and after a restart, at a fixed "now" so ranking
/// cannot differ because time passed.
fn answers(store: &Store, now: i64) -> Vec<(&'static str, Vec<ResultDto>)> {
    const QUERIES: [&str; 8] = [
        "", "budget", "https", "fn ", OCR_WORD, "snap", "file:", "img:",
    ];
    let paths = CachedPathChecker::default();
    QUERIES
        .into_iter()
        .map(|q| {
            let mut req = SearchRequest::new(q, TypeFilter::All, now);
            req.limit = 200;
            (q, store.search(&req, &paths))
        })
        .collect()
}

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

fn assert_same_state(label: &str, live: &Store, reopened: &Store, now: i64) {
    let (a, b) = (snapshot(live), snapshot(reopened));
    assert_eq!(a.len(), b.len(), "{label}: item count");
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x, y, "{label}: item {} differs after the restart", x.id);
    }
    for ((q, before), (_, after)) in answers(live, now).into_iter().zip(answers(reopened, now)) {
        assert_eq!(before, after, "{label}: search {q:?} answers differently");
    }
}

#[test]
fn restart_keeps_items_pins_and_ocr_search() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("clipdump");
    let settings = Arc::new(std::sync::RwLock::new(Settings::default()));
    let mix = Mix {
        texts: 40,
        big_texts: 4,
        images: 14,
        files: 8,
        width: 480,
        height: 300,
        ocr_chars: 600,
    };
    let live = fill(&dir, &settings, &mix);
    let store = &live.store;
    assert_eq!(store.len(), 66, "everything arrived");

    // Pins: the oldest text, image and file list.
    let mut pinned = Vec::new();
    for k in [ClipKind::Text, ClipKind::Image, ClipKind::Files] {
        let id = store
            .read()
            .iter_oldest_first()
            .find(|i| i.kind == k)
            .unwrap()
            .id;
        assert_eq!(store.toggle_pin(id), Some(true));
        pinned.push(id);
    }
    // Read it a few times, so use counts and recency are part of the state.
    let some: Vec<u64> = store
        .read()
        .iter_newest_first()
        .take(5)
        .map(|i| i.id)
        .collect();
    for id in &some {
        assert!(store.mark_used(*id));
    }
    store.persist().flush().unwrap();

    let ocr_done = snapshot(store)
        .iter()
        .filter(|i| i.kind == ClipKind::Image && i.ocr_status == OcrStatus::Done)
        .count();
    assert_eq!(ocr_done, 14, "every screenshot has its OCR text");
    let now = store.now_ms();
    let hits = answers(store, now);
    let word_hits = &hits.iter().find(|(q, _)| *q == OCR_WORD).unwrap().1;
    assert_eq!(word_hits.len(), 14, "the OCR word finds every screenshot");
    assert!(word_hits.iter().all(|r| r.kind == ClipKind::Image));

    // 1. A kill: the folder as it is on disk while the app is still running.
    let crash = tmp.path().join("crash");
    copy_tree(&dir, &crash);
    {
        let q = OcrQueueHandle::new(Arc::clone(&settings));
        let after_kill = open_store(&crash, &settings, &q);
        assert_same_state("after a kill", store, &after_kill, now);
        for id in &pinned {
            assert!(after_kill.get(*id).unwrap().pinned, "pin {id} after a kill");
        }
        assert_eq!(after_kill.stats().ocr_pending, 0);
    }

    // 2. A graceful shutdown (the order of `shutdown.rs`: workers, then the flush).
    let Live {
        store,
        ocr_queue,
        worker,
    } = live;
    drop(ocr_queue);
    worker.stop();
    drop(worker);
    let before = snapshot(&store);
    let live_answers = answers(&store, now);
    store.persist().shutdown().unwrap();
    drop(store);

    let q = OcrQueueHandle::new(Arc::clone(&settings));
    let reopened = open_store(&dir, &settings, &q);
    assert_eq!(
        snapshot(&reopened),
        before,
        "state after a graceful restart"
    );
    assert_eq!(
        answers(&reopened, now),
        live_answers,
        "searches after a restart"
    );
    for id in &pinned {
        assert!(
            reopened.get(*id).unwrap().pinned,
            "pin {id} after a restart"
        );
    }
    let st = reopened.stats();
    assert_eq!((st.items, st.pinned, st.ocr_pending), (66, 3, 0));
    // The blob of a screenshot is readable again, so a copy after a restart works.
    let img = reopened
        .read()
        .iter_newest_first()
        .find(|i| i.kind == ClipKind::Image)
        .unwrap()
        .id;
    assert!(reopened.get_blob(img).unwrap().starts_with(b"\x89PNG"));
    drop(reopened);

    // 3. And a second restart changes nothing.
    let q = OcrQueueHandle::new(Arc::clone(&settings));
    let again = open_store(&dir, &settings, &q);
    assert_eq!(snapshot(&again), before, "state after a second restart");
}

fn median(v: &mut [Duration]) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// Try to empty the page cache of the machine (Linux, needs root), so the next
/// open reads from the disk. Says whether it worked.
fn drop_page_cache() -> bool {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("sync").status();
        fs::write("/proc/sys/vm/drop_caches", "3").is_ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// `Persist::open` plus `Store::open` of `dir`: what `start_ingest` times as
/// `startup_ready`, and the load inside it (`startup_load`).
fn timed_open(dir: &Path, settings: &Arc<std::sync::RwLock<Settings>>) -> (Duration, Duration) {
    let t = Instant::now();
    let q = OcrQueueHandle::new(Arc::clone(settings));
    let store = open_store(dir, settings, &q);
    (t.elapsed(), store.load_time())
}

/// 1,000 items, 300 of them 1080p screenshots with real thumbnails and about
/// 1,500 characters of OCR text each; 600 texts (a few big); 100 file lists.
/// Reports the open time cold (page cache dropped, if allowed) and warm.
///
/// ```text
/// cargo test --profile perf --lib cold_start_1000 -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement, see docs/perf.md"]
fn cold_start_1000_items_300_screenshots() {
    let keep = std::env::var_os("CLIPDUMP_RESTART_KEEP_DIR").map(std::path::PathBuf::from);
    let tmp = tempfile::tempdir().unwrap();
    let dir = keep
        .as_ref()
        .map_or_else(|| tmp.path().join("clipdump"), |k| k.join("clipdump"));
    let settings = Arc::new(std::sync::RwLock::new(Settings::default()));
    let mix = Mix {
        texts: 560,
        big_texts: 40,
        images: 300,
        files: 100,
        width: 1920,
        height: 1080,
        ocr_chars: 1500,
    };
    let t = Instant::now();
    let live = fill(&dir, &settings, &mix);
    let built = t.elapsed();
    assert_eq!(live.store.len(), 1000);
    let st = live.store.stats();
    live.worker.stop();
    live.store.persist().shutdown().unwrap();
    drop(live);
    let on_disk = {
        let mut total = 0u64;
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let m = e.metadata().unwrap();
                if m.is_dir() {
                    stack.push(e.path());
                } else {
                    total += m.len();
                }
            }
        }
        total
    };

    let dropped = drop_page_cache();
    let (cold_ready, cold_load) = timed_open(&dir, &settings);
    let mut ready = Vec::new();
    let mut load = Vec::new();
    for _ in 0..7 {
        let (r, l) = timed_open(&dir, &settings);
        ready.push(r);
        load.push(l);
    }
    eprintln!("\n=== COLD START REPORT ===");
    eprintln!(
        "history: 1000 items, {} screenshots ({}x{} with thumbnails and about {} chars of OCR text each), {} files, {} MB on disk, built in {:.1} s ({} build)",
        mix.images,
        mix.width,
        mix.height,
        mix.ocr_chars,
        mix.files,
        on_disk / (1024 * 1024),
        built.as_secs_f64(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "optimized"
        },
    );
    eprintln!(
        "pinned {}, disk estimate {} MB",
        st.pinned,
        st.disk_bytes / (1024 * 1024)
    );
    eprintln!(
        "first open after dropping the page cache ({}): ready {:.1} ms, Store::open {:.1} ms",
        if dropped {
            "dropped"
        } else {
            "could NOT be dropped, so this is warm"
        },
        cold_ready.as_secs_f64() * 1e3,
        cold_load.as_secs_f64() * 1e3,
    );
    let (r, l) = (median(&mut ready.clone()), median(&mut load.clone()));
    eprintln!(
        "7 warm opens: ready (Persist + Store) median {:.1} ms (min {:.1}, max {:.1}); Store::open median {:.1} ms (min {:.1}, max {:.1})",
        r.as_secs_f64() * 1e3,
        ready.iter().min().unwrap().as_secs_f64() * 1e3,
        ready.iter().max().unwrap().as_secs_f64() * 1e3,
        l.as_secs_f64() * 1e3,
        load.iter().min().unwrap().as_secs_f64() * 1e3,
        load.iter().max().unwrap().as_secs_f64() * 1e3,
    );
    eprintln!("=== END COLD START ===\n");
    if let Some(k) = keep {
        eprintln!("history kept in {}", k.join("clipdump").display());
    }
}

/// The parts of "Ctrl+Enter to clipboard set" that are plain CPU and disk work and
/// so can be measured on any platform: loading the stored PNG of a 4K screenshot
/// (cache miss: file read and decrypt), decoding it, and building the `CF_DIBV5`
/// bytes (what `platform/windows/write.rs` does before it opens the clipboard); and
/// converting a text to the UTF-16 of `CF_UNICODETEXT`. The Win32 calls
/// (`OpenClipboard`, `GlobalAlloc`, `SetClipboardData`) are not in these figures;
/// they are measured on Windows by the `copy_*` events of `--perf`.
///
/// ```text
/// cargo test --profile perf --lib copy_image_cpu_parts -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement, see docs/perf.md"]
fn copy_image_cpu_parts_4k_cache_miss() {
    use crate::ingest::image::decode_png;
    use crate::model::Payload;
    use crate::platform::clipfmt::{dibv5_len, fill_dibv5, unicode_text_units, write_units_le};

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("clipdump");
    let settings = Arc::new(std::sync::RwLock::new(Settings::default()));
    let mix = Mix {
        texts: 0,
        big_texts: 0,
        images: 8,
        files: 0,
        width: 3840,
        height: 2160,
        ocr_chars: 100,
    };
    let live = fill(&dir, &settings, &mix);
    live.worker.stop();
    live.store.persist().shutdown().unwrap();
    drop(live);
    // A fresh store: its blob cache is empty, so every first read is a miss.
    let q = OcrQueueHandle::new(Arc::clone(&settings));
    let store = open_store(&dir, &settings, &q);
    let ids: Vec<u64> = store.read().iter_newest_first().map(|i| i.id).collect();
    let dropped = drop_page_cache();
    let (mut load, mut decode, mut fill, mut total, mut png_kb) =
        (vec![], vec![], vec![], vec![], vec![]);
    for id in ids {
        let t0 = Instant::now();
        let Payload::Image { png, .. } = store.payload(id).unwrap() else {
            panic!("not an image")
        };
        let t_load = t0.elapsed();
        let t1 = Instant::now();
        let rgba = decode_png(&png).unwrap();
        let t_decode = t1.elapsed();
        let t2 = Instant::now();
        let len = dibv5_len(rgba.width, rgba.height).unwrap();
        let mut out = vec![0u8; len];
        fill_dibv5(&rgba.pixels, rgba.width, rgba.height, &mut out).unwrap();
        std::hint::black_box(&out);
        let t_fill = t2.elapsed();
        load.push(t_load);
        decode.push(t_decode);
        fill.push(t_fill);
        total.push(t0.elapsed());
        png_kb.push(png.len() / 1024);
    }
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    eprintln!("\n=== COPY CPU PARTS ===");
    eprintln!(
        "{} screenshots 3840x2160 (PNG {}..{} KB), page cache {}:",
        total.len(),
        png_kb.iter().min().unwrap(),
        png_kb.iter().max().unwrap(),
        if dropped {
            "dropped before the first"
        } else {
            "warm"
        }
    );
    for (name, v) in [
        ("payload (cache miss: read file, decrypt)", &mut load),
        ("decode PNG to RGBA", &mut decode),
        ("build CF_DIBV5 bytes", &mut fill),
        ("sum of the three", &mut total),
    ] {
        eprintln!(
            "  {name}: median {:.1} ms, max {:.1} ms",
            ms(median(v)),
            ms(*v.iter().max().unwrap())
        );
    }
    // Text: UTF-16 conversion of 10 KB and 1 MB.
    let mut rng = Rng(7);
    for (label, bytes) in [("10 KB", 10_000usize), ("1 MB", 1_000_000)] {
        let text = paragraphs(&mut rng, 0, bytes);
        let t = Instant::now();
        let units = unicode_text_units(&text);
        let mut out = vec![0u8; units.len() * 2];
        write_units_le(&units, &mut out).unwrap();
        std::hint::black_box(&out);
        eprintln!(
            "  text {label}: UTF-16 conversion {:.3} ms",
            ms(t.elapsed())
        );
    }
    eprintln!("=== END COPY CPU PARTS ===\n");
}
