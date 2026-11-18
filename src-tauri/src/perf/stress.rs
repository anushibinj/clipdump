//! The stress harness of Phase 14: thousands of mixed clipboard entries through
//! the real pipeline into the real store, with the invariants of PLAN.md Section 7
//! checked while it runs and after it.
//!
//! What is real: the ingest pool (decode, hash, thumbnail, PNG), the store and its
//! retention, the encrypted persistence and its writer thread, the OCR worker (its
//! engine is a mock that answers with known words), the `history:changed`
//! debouncer. What is not: the clipboard watcher (the harness plays it, sending
//! `IngestMsg`s into the same bounded channel) and the window.
//!
//! The same code runs on Windows (key protector DPAPI there, `GetProcessMemoryInfo`
//! for memory, `GetDiskFreeSpaceExW` for free space). Run it:
//!
//! ```text
//! cargo test --profile perf --lib stress_ -- --ignored --nocapture --test-threads=1
//! ```
//!
//! or `node scripts/stress.mjs` (builds, runs the three scenarios, keeps the
//! reports). Each test is one scenario; all take their size from the environment
//! (`CLIPDUMP_STRESS_*`, listed in [`Params::from_env`]), so a quick run is
//! `CLIPDUMP_STRESS_TOTAL=500`. Every run uses a temp folder it deletes at the
//! end, and stops by itself (and says so) when free space on that disk falls below
//! `CLIPDUMP_STRESS_MIN_FREE_MB` (default 2048).
//!
//! What it asserts (a failure is a panic after the report is printed):
//!
//! * no entry is lost: every entry produced reaches the store (new or bumped), so
//!   no ingest worker panicked or dropped one, and the writer reported no error;
//! * the history never holds more than `max_items` unpinned items, at any sample;
//! * pinned items survive, pinned, with their files, to the end;
//! * the estimated disk use never exceeds the cap (when pins alone do not), and
//!   the folder is within a bounded slack of it;
//! * memory: the peak is within an allowance of the baseline, and the last third
//!   of the run does not grow over the middle third (no leak);
//! * a search every 50 ms stays fast while ingest runs (p95 bound below);
//! * after the run the blob and thumbnail files on disk are exactly those the
//!   store references;
//! * a graceful shutdown and a reopen give the identical state, and a search for
//!   an OCR word finds the screenshots that got that text.

// All of this file is test code (declared `#[cfg(test)] mod` in `perf/mod.rs`, never
// part of the app). The log audit of `commands/bounds_tests.rs` treats everything
// after the first `#[cfg(test)]` in a file as test code, so say so here.
#[cfg(test)]
const _: () = ();
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::procmem;
use super::summary::percentile;
use crate::ingest::changed::HistoryChanged;
use crate::ingest::image::{Rgba, encode_png, tests as pics};
use crate::ingest::{IngestPool, IngestedItem, ItemSink};
use crate::model::{ClipItem, ClipKind, OcrStatus};
use crate::ocr::{OcrConfig, OcrEngine, OcrQueueHandle, OcrWorker};
use crate::persist::Persist;
use crate::persist::blobs::hash_hex;
use crate::platform::{Captured, CapturedEvent, ImageData, ImageFormat, IngestMsg};
use crate::search::{CachedPathChecker, MatchSource, SearchRequest, TypeFilter};
use crate::settings::Settings;
use crate::store::{Inserted, Store, system_clock};

const MB: u64 = 1024 * 1024;

/// The word the mock OCR engine puts in every text, so the harness can search for it.
const OCR_WORD: &str = "stressocr";

/// p95 of the concurrent search while ingest runs must stay under this. The
/// budget for a search alone is 1 ms; this is "a few ms" with room for a loaded
/// 4 core CI machine (see `docs/perf.md` for the figures actually seen).
const SEARCH_P95_MAX_MS: f64 = 5.0;

/// Peak memory above the baseline: the blob cache budget (64 MB), thumbnails and
/// index of 1,000 items (about 25 MB), two ingest workers each holding a 4K
/// picture three ways (decoded, raw, re-encoded: about 170 MB each), the harness's
/// own queue of raw pictures (about 4 x 33 MB) and the writer's backlog.
const RAM_PEAK_ALLOWANCE_MB: u64 = 900;

/// Growth of resident memory from the middle third of the run to the end that is
/// still "not a leak" (allocator noise, page cache of the cipher buffers).
const RAM_GROWTH_ALLOWANCE_MB: u64 = 150;

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Params {
    label: &'static str,
    /// Entries to push through.
    total: usize,
    /// Entries per second, 0 for as fast as the pipeline accepts them.
    rate: u32,
    /// Percent of the entries that are screenshots.
    image_pct: u32,
    width: u32,
    height: u32,
    /// Disk cap in MB, 0 for the default (2 GB).
    disk_cap_mb: u64,
    /// Stop when free space on the data disk falls below this.
    min_free_mb: u64,
    /// Capacity of the harness's stand-in for the watcher channel. The app's is
    /// 64; with 4K pictures that would be 2 GB of raw bytes waiting, so the harness
    /// holds less to keep its own memory out of the figures it reports.
    queue: usize,
    /// Parent folder for the temp data folder (default: the system temp dir).
    dir: Option<PathBuf>,
    /// Write the samples as CSV here.
    csv: Option<PathBuf>,
    /// How long to wait for the OCR backlog after the last entry.
    ocr_drain_secs: u64,
    /// Check memory and search latency too. Off for the smoke test of the normal
    /// suite, which shares its process (and CPUs) with hundreds of other tests.
    strict: bool,
    /// Run with `--perf` logging on and print its p50 and p95 table. Off in the
    /// smoke test (the switch is process wide and other tests share the process).
    perf_log: bool,
    /// Threads that play the watcher. One generates 4K pictures at about 6 a
    /// second, which at full speed is slower than ingest takes them, so full speed
    /// runs use two.
    producers: usize,
}

fn env_num(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

impl Params {
    /// Defaults of one scenario, overridden by `CLIPDUMP_STRESS_TOTAL`, `_RATE`,
    /// `_IMAGE_PCT`, `_WIDTH`, `_HEIGHT`, `_DISK_CAP_MB`, `_MIN_FREE_MB`, `_QUEUE`,
    /// `_DIR`, `_CSV`, `_OCR_DRAIN_SECS`.
    fn from_env(label: &'static str, total: usize, rate: u32, disk_cap_mb: u64) -> Self {
        let n = |k: &str, d: u64| env_num(&format!("CLIPDUMP_STRESS_{k}")).unwrap_or(d);
        Self {
            label,
            total: n("TOTAL", total as u64) as usize,
            rate: n("RATE", u64::from(rate)) as u32,
            image_pct: n("IMAGE_PCT", 12).min(100) as u32,
            width: n("WIDTH", 3840) as u32,
            height: n("HEIGHT", 2160) as u32,
            disk_cap_mb: n("DISK_CAP_MB", disk_cap_mb),
            min_free_mb: n("MIN_FREE_MB", 2048),
            queue: n("QUEUE", 4) as usize,
            dir: std::env::var_os("CLIPDUMP_STRESS_DIR").map(PathBuf::from),
            csv: std::env::var_os("CLIPDUMP_STRESS_CSV").map(PathBuf::from),
            ocr_drain_secs: n("OCR_DRAIN_SECS", 60),
            strict: true,
            perf_log: true,
            producers: n("PRODUCERS", if rate == 0 { 2 } else { 1 }) as usize,
        }
    }

    fn disk_cap(&self) -> u64 {
        if self.disk_cap_mb == 0 {
            Settings::default().disk_cap_bytes
        } else {
            self.disk_cap_mb * MB
        }
    }
}

// ---------------------------------------------------------------------------
// A small deterministic generator (no rand crate: the runs must be repeatable)
// ---------------------------------------------------------------------------

pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub(super) fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    pub(super) fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below((hi - lo + 1) as u64) as usize
    }
}

pub(super) const WORDS: &[&str] = &[
    "meeting",
    "notes",
    "invoice",
    "budget",
    "quarterly",
    "report",
    "design",
    "review",
    "deploy",
    "release",
    "server",
    "client",
    "network",
    "backup",
    "restore",
    "schedule",
    "project",
    "draft",
    "final",
    "customer",
    "support",
    "ticket",
    "error",
    "warning",
    "build",
    "test",
    "branch",
    "merge",
    "commit",
    "document",
    "slides",
    "travel",
    "flight",
    "hotel",
    "receipt",
    "payment",
    "account",
    "password",
    "profile",
    "settings",
    "update",
    "install",
    "version",
    "feature",
    "bug",
    "fix",
    "patch",
    "memory",
    "latency",
    "cache",
    "queue",
    "thread",
    "index",
    "search",
    "query",
    "result",
    "screenshot",
    "clipboard",
    "history",
    "pinned",
    "recent",
    "archive",
];

pub(super) fn sentence(rng: &mut Rng, words: usize) -> String {
    let mut s = String::new();
    for k in 0..words {
        if k > 0 {
            s.push(' ');
        }
        s.push_str(WORDS[rng.below(WORDS.len() as u64) as usize]);
    }
    s
}

/// Text of about `bytes` bytes: sentences, one per line (every third line
/// CRLF, like text copied from a Windows app).
pub(super) fn paragraphs(rng: &mut Rng, i: usize, bytes: usize) -> String {
    let mut t = format!("entry {i}: ");
    let mut line = 0;
    while t.len() < bytes {
        let n = rng.range(6, 18);
        t.push_str(&sentence(rng, n));
        t.push_str(if line % 3 == 2 { "\r\n" } else { "\n" });
        line += 1;
    }
    t
}

pub(super) fn code(rng: &mut Rng, i: usize) -> String {
    let mut t = format!("// entry {i}\n");
    for k in 0..rng.range(5, 80) {
        let w = WORDS[rng.below(WORDS.len() as u64) as usize];
        t.push_str(&format!(
            "fn {w}_{k}(x: u32, y: &str) -> Result<u32, Error> {{ let v = x * {k} + y.len() as u32; Ok(v) }}\n"
        ));
    }
    t
}

pub(super) fn url(rng: &mut Rng, i: usize) -> String {
    let (a, b) = (
        WORDS[rng.below(WORDS.len() as u64) as usize],
        WORDS[rng.below(WORDS.len() as u64) as usize],
    );
    format!(
        "https://{a}.example.com/{b}/page-{i}?id={}&ref={a}",
        rng.below(1_000_000)
    )
}

// ---------------------------------------------------------------------------
// Pictures
// ---------------------------------------------------------------------------

/// A screenshot like picture: flat window areas, a title gradient, lines of
/// text like marks and one region of photo like noise (which does not compress,
/// so PNGs come out between a few hundred KB and a few MB, like real ones).
pub(super) struct Pictures {
    base: Rgba,
    noise: (u32, u32, u32, u32),
}

impl Pictures {
    pub(super) fn new(w: u32, h: u32) -> Self {
        let mut px = vec![0u8; (w as usize) * (h as usize) * 4];
        let mut put = |x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 4]| {
            for y in y0.min(h)..y1.min(h) {
                let row = (y as usize) * (w as usize) * 4;
                for x in x0.min(w)..x1.min(w) {
                    let o = row + (x as usize) * 4;
                    px[o..o + 4].copy_from_slice(&c);
                }
            }
        };
        put(0, 0, w, h, [240, 240, 243, 255]);
        put(0, 0, w, h / 36, [38, 74, 140, 255]);
        put(0, h / 36, w / 12, h, [226, 228, 234, 255]);
        let mut rng = Rng(0x5EED_1234_ABCD_9876);
        // Lines of "text": short dark bars of word like lengths.
        let (left, line_h) = (w / 10, (h / 90).max(2));
        let mut y = h / 18;
        while y + line_h < h {
            let mut x = left;
            while x < w - w / 8 {
                let len = rng.range(2, 14) as u32 * (w / 400).max(1);
                put(x, y, x + len, y + line_h / 2, [30, 30, 36, 255]);
                x += len + (w / 300).max(2);
            }
            y += line_h * 2;
        }
        let noise = (w / 2, h / 6, (w / 3).min(w - w / 2), (h / 3).min(h - h / 6));
        let mut pics = Self {
            base: Rgba {
                width: w,
                height: h,
                pixels: px,
            },
            noise,
        };
        pics.fill_noise(1);
        pics
    }

    fn fill_noise(&mut self, seed: u64) {
        let (x0, y0, nw, nh) = self.noise;
        let w = self.base.width as usize;
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        for y in y0..y0 + nh {
            for x in x0..x0 + nw {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                let o = (y as usize * w + x as usize) * 4;
                let v = s.to_le_bytes();
                self.base.pixels[o..o + 4].copy_from_slice(&[v[0], v[1], v[2], 255]);
            }
        }
    }

    /// Picture number `i`: the base, with the noise region flattened for every
    /// other one (so sizes vary) and a 32 x 32 stamp whose colour encodes `i`
    /// (so every picture hashes differently).
    pub(super) fn make(&self, i: u32) -> Rgba {
        let mut p = self.base.clone();
        let w = p.width as usize;
        if i % 2 == 1 {
            let (x0, y0, nw, nh) = self.noise;
            for y in y0..y0 + nh {
                let o = (y as usize * w + x0 as usize) * 4;
                for x in 0..nw as usize {
                    let g = (x * 255 / nw as usize) as u8;
                    p.pixels[o + x * 4..o + x * 4 + 4].copy_from_slice(&[g, 120, 255 - g, 255]);
                }
            }
        }
        let (sx, sy) = (
            (i as usize * 37) % (w - 32),
            (i as usize * 91) % (p.height as usize - 32),
        );
        let c = [
            (i & 0xFF) as u8,
            ((i >> 8) & 0xFF) as u8,
            ((i >> 16) & 0xFF) as u8,
            255,
        ];
        for y in sy..sy + 32 {
            for x in sx..sx + 32 {
                let o = (y * w + x) * 4;
                p.pixels[o..o + 4].copy_from_slice(&c);
            }
        }
        p
    }

    /// The clipboard bytes of picture `i` in one of the three formats the Windows
    /// watcher hands over: the registered PNG format, `CF_DIBV5` (what Win+Shift+S
    /// uses) and a plain 24 bit `CF_DIB`.
    pub(super) fn image_data(&self, i: u32) -> ImageData {
        let format = match i % 5 {
            0 | 1 => ImageFormat::Png,
            2 | 3 => ImageFormat::Dibv5,
            _ => ImageFormat::Dib,
        };
        self.image_data_in(i, format)
    }

    /// Picture `i` in a given format.
    pub(super) fn image_data_in(&self, i: u32, format: ImageFormat) -> ImageData {
        let pic = self.make(i);
        let bytes = match format {
            ImageFormat::Png => encode_png(&pic).expect("png"),
            ImageFormat::Dibv5 => pics::dibv5_bitfields(&pic, false),
            ImageFormat::Dib => pics::dib24(&pic, false),
        };
        ImageData { format, bytes }
    }
}

// ---------------------------------------------------------------------------
// Entries
// ---------------------------------------------------------------------------

/// Real files for the file list entries, so `stat` and the image thumbnail code
/// run for real; the rest of each list is made up paths.
pub(super) struct FileFixtures {
    _dir: tempfile::TempDir,
    real: Vec<PathBuf>,
}

impl FileFixtures {
    pub(super) fn new() -> Self {
        let dir = tempfile::tempdir().expect("fixture dir");
        let mut real = Vec::new();
        for k in 0..4 {
            let p = dir.path().join(format!("note{k}.txt"));
            std::fs::write(&p, format!("note {k}\n")).unwrap();
            real.push(p);
        }
        let small = Pictures::new(160, 100).make(7);
        for k in 0..2 {
            let p = dir.path().join(format!("shot{k}.png"));
            std::fs::write(&p, encode_png(&small).unwrap()).unwrap();
            real.push(p);
        }
        Self { _dir: dir, real }
    }

    pub(super) fn list(&self, rng: &mut Rng, i: usize) -> Vec<PathBuf> {
        let n = rng.range(1, 200);
        let mut v = Vec::with_capacity(n);
        let real = rng.range(0, self.real.len().min(n));
        for k in 0..real {
            v.push(self.real[(k + i) % self.real.len()].clone());
        }
        for k in real..n {
            v.push(PathBuf::from(format!(
                "/stress/project{}/dir{}/{}-{k}.{}",
                i % 17,
                i,
                WORDS[rng.below(WORDS.len() as u64) as usize],
                ["txt", "rs", "png", "pdf", "mp4"][k % 5]
            )));
        }
        v.sort();
        v.dedup();
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Short,
    Url,
    Code,
    Paragraphs,
    Huge,
    Files,
    Image,
    Duplicate,
}

struct Generator {
    rng: Rng,
    pics: Pictures,
    files: FileFixtures,
    image_pct: u32,
    /// The last few short texts, for the repeated copies.
    recent: Vec<String>,
}

impl Generator {
    fn new(p: &Params, k: usize) -> Self {
        Self {
            rng: Rng(0xC11D_0001 + 7919 * k as u64),
            pics: Pictures::new(p.width, p.height),
            files: FileFixtures::new(),
            image_pct: p.image_pct,
            recent: Vec::new(),
        }
    }

    fn pick(&mut self, i: usize) -> Kind {
        if i > 8 && i.is_multiple_of(53) {
            return Kind::Duplicate;
        }
        if self.rng.below(100) < u64::from(self.image_pct) {
            return Kind::Image;
        }
        match self.rng.below(100) {
            0..=39 => Kind::Short,
            40..=57 => Kind::Url,
            58..=74 => Kind::Code,
            75..=91 => Kind::Paragraphs,
            92..=93 => Kind::Huge,
            _ => Kind::Files,
        }
    }

    fn entry(&mut self, i: usize) -> (Kind, IngestMsg) {
        let kind = self.pick(i);
        let captured = match kind {
            Kind::Short => {
                let n = self.rng.range(3, 12);
                let t = format!("{i} {}", sentence(&mut self.rng, n));
                self.recent.push(t.clone());
                if self.recent.len() > 8 {
                    self.recent.remove(0);
                }
                Captured::Text(t)
            }
            Kind::Url => Captured::Text(url(&mut self.rng, i)),
            Kind::Code => Captured::Text(code(&mut self.rng, i)),
            Kind::Paragraphs => {
                let b = self.rng.range(1_000, 60_000);
                Captured::Text(paragraphs(&mut self.rng, i, b))
            }
            Kind::Huge => {
                let b = self.rng.range(100_000, 1_000_000);
                Captured::Text(paragraphs(&mut self.rng, i, b))
            }
            Kind::Files => Captured::Files(self.files.list(&mut self.rng, i)),
            Kind::Image => Captured::Image(self.pics.image_data(i as u32)),
            Kind::Duplicate => Captured::Text(
                self.recent
                    .first()
                    .cloned()
                    .unwrap_or_else(|| format!("duplicate seed {i}")),
            ),
        };
        (
            kind,
            IngestMsg::Captured(CapturedEvent {
                captured,
                source_app: Some("stress.exe".into()),
            }),
        )
    }
}

// ---------------------------------------------------------------------------
// Pieces around the store
// ---------------------------------------------------------------------------

/// Counts what the ingest pool hands to the store.
struct CountingSink {
    store: Arc<Store>,
    new: AtomicU64,
    bumped: AtomicU64,
    /// Longest `Store::insert` seen, in microseconds (the write lock is held
    /// inside it).
    insert_max_us: AtomicU64,
    /// Every `Store::insert`, in microseconds, for its distribution.
    insert_us: Mutex<Vec<u64>>,
}

impl ItemSink for CountingSink {
    fn submit(&self, item: IngestedItem) {
        let t = Instant::now();
        match self.store.insert(item) {
            Inserted::New(_) => self.new.fetch_add(1, Ordering::Relaxed),
            Inserted::Bumped(_) => self.bumped.fetch_add(1, Ordering::Relaxed),
        };
        let us = t.elapsed().as_micros() as u64;
        self.insert_max_us.fetch_max(us, Ordering::Relaxed);
        self.insert_us.lock().unwrap().push(us);
    }

    fn bump(&self, hash: u128) {
        ItemSink::bump(&*self.store, hash);
    }
}

/// An OCR engine that answers at once with known words plus a checksum of the
/// picture, so the harness can search for them.
struct MockOcr;

impl OcrEngine for MockOcr {
    fn recognize(&self, rgba: &[u8], w: u32, h: u32) -> anyhow::Result<String> {
        let sum: u64 = rgba.iter().step_by(4099).map(|&b| u64::from(b)).sum();
        Ok(format!("{OCR_WORD} receipt total {sum:x}\n{w}x{h} invoice"))
    }

    fn available(&self) -> bool {
        true
    }
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // the statvfs field types differ per unix
fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL terminated path and `st` a writable statvfs.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(windows)]
fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut avail = 0u64;
    // SAFETY: `wide` is NUL terminated and outlives the call; `avail` is writable.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut avail), None, None) }.ok()?;
    Some(avail)
}

#[cfg(not(any(unix, windows)))]
fn free_bytes(_: &Path) -> Option<u64> {
    None
}

/// Bytes of every file under `dir`, and of the database files alone.
#[derive(Clone, Copy, Debug, Default)]
struct DiskUse {
    total: u64,
    blobs: u64,
    thumbs: u64,
    db: u64,
}

fn walk(dir: &Path, out: &mut DiskUse, bucket: u8) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if meta.is_dir() {
            let b = match name.as_str() {
                "blobs" => 1,
                "thumbs" => 2,
                _ => bucket,
            };
            walk(&e.path(), out, b);
        } else {
            out.total += meta.len();
            match bucket {
                1 => out.blobs += meta.len(),
                2 => out.thumbs += meta.len(),
                _ if name.starts_with("history.db") => out.db += meta.len(),
                _ => {}
            }
        }
    }
}

fn disk_use(dir: &Path) -> DiskUse {
    let mut d = DiskUse::default();
    walk(dir, &mut d, 0);
    d
}

/// File stems under `dir/<sub>/<xx>/`.
fn stems(dir: &Path, sub: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(top) = std::fs::read_dir(dir.join(sub)) else {
        return out;
    };
    for shard in top.flatten() {
        if let Ok(files) = std::fs::read_dir(shard.path()) {
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                out.insert(name.strip_suffix(".bin").unwrap_or(&name).to_owned());
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct Sample {
    t_ms: u64,
    rss: u64,
    items: usize,
    unpinned: usize,
    cache_bytes: usize,
    disk_est: u64,
    disk_actual: u64,
    persist_backlog: usize,
    ingest_queue: usize,
    ocr_backlog: usize,
    free: u64,
}

/// A search that took more than 20 ms, and how long the same query took when
/// asked again at once.
#[derive(Clone, Copy, Debug)]
struct SlowSearch {
    at_ms: u64,
    us: u64,
    again_us: u64,
    query: &'static str,
}

const QUERIES: &[&str] = &[
    "",
    "m",
    "mee",
    "meeting notes",
    "https",
    "fn ",
    "img:",
    "file:",
    "zzzzqq",
    "invoice",
    "Budget",
];

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

struct Report {
    label: &'static str,
    params: Params,
    produced: u64,
    new: u64,
    bumped: u64,
    aborted: Option<String>,
    elapsed: Duration,
    ingest_elapsed: Duration,
    baseline_rss: u64,
    peak_rss: u64,
    steady_rss: u64,
    rss_mid: u64,
    rss_end: u64,
    max_items: usize,
    max_unpinned: usize,
    max_cache: usize,
    cache_budget: usize,
    max_disk_est: u64,
    disk_cap: u64,
    disk_final: DiskUse,
    max_persist_backlog: usize,
    max_ingest_queue: usize,
    max_ocr_backlog: usize,
    ocr_done: usize,
    ocr_left_at_stop: usize,
    min_free: u64,
    search_us: Vec<u64>,
    insert_max_us: u64,
    insert_p50_us: u64,
    slow_searches: Vec<SlowSearch>,
    insert_p99_us: u64,
    steady_trimmed_rss: u64,
    perf_table: String,
    final_items: usize,
    final_pinned: usize,
    final_images: usize,
    shutdown_ms: u128,
    reopen_ms: u128,
    samples: Vec<Sample>,
    problems: Vec<String>,
}

fn ms(us: u64) -> f64 {
    us as f64 / 1000.0
}

impl Report {
    fn print(&self) {
        let mut s = self.search_us.clone();
        s.sort_unstable();
        let (p50, p95, p99) = (percentile(&s, 50), percentile(&s, 95), percentile(&s, 99));
        let max = s.last().copied().unwrap_or(0);
        let mb = |b: u64| b as f64 / MB as f64;
        let p = &self.params;
        eprintln!("\n=== STRESS REPORT: {} ===", self.label);
        eprintln!(
            "run: {} entries ({}), pictures {}x{} at {}%, {}, disk cap {}",
            p.total,
            if p.rate == 0 {
                "full speed".to_owned()
            } else {
                format!("{} per second", p.rate)
            },
            p.width,
            p.height,
            p.image_pct,
            if cfg!(debug_assertions) {
                "debug build"
            } else {
                "optimized build"
            },
            if p.disk_cap_mb == 0 {
                "2 GB (default)".to_owned()
            } else {
                format!("{} MB", p.disk_cap_mb)
            },
        );
        eprintln!(
            "entries: produced {}, stored new {}, bumped (duplicates) {}, aborted: {}",
            self.produced,
            self.new,
            self.bumped,
            self.aborted.as_deref().unwrap_or("no")
        );
        eprintln!(
            "time: ingest of all entries {:.1} s, whole run {:.1} s",
            self.ingest_elapsed.as_secs_f64(),
            self.elapsed.as_secs_f64()
        );
        eprintln!(
            "store: max items {} (max unpinned {}, limit {}), final {} items, {} pinned, {} images",
            self.max_items,
            self.max_unpinned,
            Settings::default().max_items,
            self.final_items,
            self.final_pinned,
            self.final_images
        );
        eprintln!(
            "memory (RSS): baseline {:.0} MB, peak {:.0} MB (+{:.0}), middle third {:.0} MB, end {:.0} MB (+{:.0} over middle), after settling {:.0} MB",
            mb(self.baseline_rss),
            mb(self.peak_rss),
            mb(self.peak_rss.saturating_sub(self.baseline_rss)),
            mb(self.rss_mid),
            mb(self.rss_end),
            mb(self.rss_end).max(mb(self.rss_mid)) - mb(self.rss_mid),
            mb(self.steady_rss),
        );
        if self.steady_trimmed_rss > 0 {
            eprintln!(
                "memory (RSS) after the allocator is told to give its free memory back (malloc_trim): {:.0} MB",
                mb(self.steady_trimmed_rss)
            );
        }
        eprintln!(
            "cache: max {:.1} MB of budget {:.0} MB",
            self.max_cache as f64 / MB as f64,
            self.cache_budget as f64 / MB as f64
        );
        eprintln!(
            "disk: max estimated {:.0} MB of cap {:.0} MB; folder at the end {:.0} MB (blobs {:.0}, thumbs {:.0}, history.db and wal {:.0})",
            mb(self.max_disk_est),
            mb(self.disk_cap),
            mb(self.disk_final.total),
            mb(self.disk_final.blobs),
            mb(self.disk_final.thumbs),
            mb(self.disk_final.db),
        );
        eprintln!(
            "free disk: lowest {:.0} MB (stop below {} MB)",
            mb(self.min_free),
            p.min_free_mb
        );
        eprintln!(
            "backlog: persist writer queue max {}, ingest channel max {} of {}, OCR queue max {} (done {}, left when stopped {})",
            self.max_persist_backlog,
            self.max_ingest_queue,
            p.queue,
            self.max_ocr_backlog,
            self.ocr_done,
            self.ocr_left_at_stop
        );
        eprintln!(
            "search every 50 ms during the run: {} searches, p50 {:.3} ms, p95 {:.3} ms, p99 {:.3} ms, max {:.3} ms (limit p95 {SEARCH_P95_MAX_MS} ms); Store::insert p50 {:.3} ms, p99 {:.3} ms, max {:.3} ms",
            s.len(),
            ms(p50),
            ms(p95),
            ms(p99),
            ms(max),
            ms(self.insert_p50_us),
            ms(self.insert_p99_us),
            ms(self.insert_max_us),
        );
        for s in self.slow_searches.iter().take(8) {
            eprintln!(
                "slow search at {:.1} s: {:.1} ms for {:?}, the same query again at once {:.3} ms",
                s.at_ms as f64 / 1000.0,
                ms(s.us),
                s.query,
                ms(s.again_us)
            );
        }
        if self.slow_searches.len() > 8 {
            eprintln!(
                "... and {} more slow searches",
                self.slow_searches.len() - 8
            );
        }
        if !self.perf_table.is_empty() {
            eprintln!(
                "perf log of this run (what --perf records, same code):\n{}",
                self.perf_table
            );
        }
        eprintln!(
            "graceful shutdown {} ms, reopen {} ms",
            self.shutdown_ms, self.reopen_ms
        );
        if self.problems.is_empty() {
            eprintln!("invariants: all held");
        } else {
            for pr in &self.problems {
                eprintln!("INVARIANT FAILED: {pr}");
            }
        }
        eprintln!("=== END {} ===\n", self.label);
    }

    fn write_csv(&self, path: &Path) {
        let mut out = String::from(
            "t_ms,rss,items,unpinned,cache_bytes,disk_est,disk_actual,persist_backlog,ingest_queue,ocr_backlog,free\n",
        );
        for s in &self.samples {
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{}\n",
                s.t_ms,
                s.rss,
                s.items,
                s.unpinned,
                s.cache_bytes,
                s.disk_est,
                s.disk_actual,
                s.persist_backlog,
                s.ingest_queue,
                s.ocr_backlog,
                s.free
            ));
        }
        let _ = std::fs::write(path, out);
    }
}

pub(super) fn open_store(
    dir: &Path,
    settings: &Arc<std::sync::RwLock<Settings>>,
    ocr: &OcrQueueHandle,
) -> Arc<Store> {
    let persist = Arc::new(Persist::open(dir).expect("open persist"));
    let changed = HistoryChanged::with_interval(Duration::from_millis(50), || {}).expect("changed");
    Arc::new(
        Store::open(
            persist,
            Arc::clone(settings),
            Arc::new(ocr.clone()),
            Some(changed),
            system_clock(),
        )
        .expect("open store"),
    )
}

pub(super) fn snapshot(store: &Store) -> Vec<ClipItem> {
    store
        .read()
        .iter_newest_first()
        .map(|i| (**i).clone())
        .collect()
}

fn run(p: &Params) -> Report {
    let started = Instant::now();
    let perf_since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let perf_path = p
        .perf_log
        .then(|| super::enable_for_harness(&std::env::temp_dir().join("clipdump-stress-perf")));
    let mut problems = Vec::new();
    let parent = p.dir.clone().unwrap_or_else(std::env::temp_dir);
    let tmp = tempfile::Builder::new()
        .prefix("clipdump-stress-")
        .tempdir_in(&parent)
        .expect("temp data folder");
    let dir = tmp.path().join("clipdump");
    let min_free = p.min_free_mb * MB;

    let settings = Arc::new(std::sync::RwLock::new(Settings {
        disk_cap_bytes: p.disk_cap(),
        ..Settings::default()
    }));
    let max_items = settings.read().unwrap().max_items;
    let cache_budget = settings.read().unwrap().ram_cache_bytes;
    let ocr_queue = OcrQueueHandle::new(Arc::clone(&settings));
    let store = open_store(&dir, &settings, &ocr_queue);
    let ocr_worker = OcrWorker::start(
        ocr_queue.clone(),
        Arc::downgrade(&store),
        Box::new(|| Box::new(MockOcr)),
        OcrConfig::default(),
    )
    .expect("ocr worker");

    let sink = Arc::new(CountingSink {
        store: Arc::clone(&store),
        new: AtomicU64::new(0),
        bumped: AtomicU64::new(0),
        insert_max_us: AtomicU64::new(0),
        insert_us: Mutex::default(),
    });
    let (tx, rx) = crossbeam_channel::bounded::<IngestMsg>(p.queue);
    let pool = IngestPool::start(rx.clone(), sink.clone()).expect("ingest pool");

    // The generator holds the base picture (33 MB at 4K): build it before the
    // baseline, so the baseline contains the harness's own fixed memory.
    let generators: Vec<Generator> = (0..p.producers.max(1))
        .map(|k| Generator::new(p, k))
        .collect();
    let baseline = procmem::rss().map_or(0, |r| r.now);

    let abort: Arc<Mutex<Option<String>>> = Arc::default();
    let produced = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let t0 = Instant::now();

    // -- sampler ---------------------------------------------------------------
    let samples: Arc<Mutex<Vec<Sample>>> = Arc::default();
    let sampler = {
        let (store, rx, ocr, abort, stop, samples, dir) = (
            Arc::clone(&store),
            rx.clone(),
            ocr_queue.clone(),
            Arc::clone(&abort),
            Arc::clone(&stop),
            Arc::clone(&samples),
            dir.clone(),
        );
        thread::spawn(move || {
            let mut n = 0u64;
            let mut disk_actual = 0;
            while !stop.load(Ordering::Relaxed) {
                let st = store.stats();
                let (unpinned, items) = {
                    let g = store.read();
                    (g.unpinned_count(), g.len())
                };
                if n.is_multiple_of(4) {
                    disk_actual = disk_use(&dir).total;
                }
                let free = free_bytes(&dir).unwrap_or(u64::MAX);
                if free < min_free {
                    let mut a = abort.lock().unwrap();
                    a.get_or_insert_with(|| format!("free disk space fell to {} MB", free / MB));
                }
                samples.lock().unwrap().push(Sample {
                    t_ms: t0.elapsed().as_millis() as u64,
                    rss: procmem::rss().map_or(0, |r| r.now),
                    items,
                    unpinned,
                    cache_bytes: st.cache_bytes,
                    disk_est: st.disk_bytes,
                    disk_actual,
                    persist_backlog: store.persist().backlog(),
                    ingest_queue: rx.len(),
                    ocr_backlog: ocr.backlog(),
                    free,
                });
                n += 1;
                thread::sleep(Duration::from_millis(250));
            }
        })
    };

    // -- concurrent search every 50 ms -------------------------------------------
    let searcher = {
        let (store, stop) = (Arc::clone(&store), Arc::clone(&stop));
        thread::spawn(move || {
            let paths = CachedPathChecker::default();
            let mut lat = Vec::new();
            let mut slow = Vec::new();
            let mut q = 0;
            let run = |query: &str| {
                let t = Instant::now();
                // The function behind the `search` command, so `--perf` times it too.
                let found =
                    crate::commands::search_items(&store, &paths, query, TypeFilter::All, Some(50));
                std::hint::black_box(found);
                t.elapsed().as_micros() as u64
            };
            while !stop.load(Ordering::Relaxed) {
                let query = QUERIES[q % QUERIES.len()];
                q += 1;
                let us = run(query);
                lat.push(us);
                if us > 20_000 {
                    // A stall: ask again at once. If the repeat is fast, the query
                    // itself is not slow, something else held the search up.
                    let again = run(query);
                    slow.push(SlowSearch {
                        at_ms: t0.elapsed().as_millis() as u64,
                        us,
                        again_us: again,
                        query,
                    });
                }
                thread::sleep(Duration::from_millis(50));
            }
            (lat, slow)
        })
    };

    // -- producers (play the clipboard watcher) ----------------------------------
    // Producer k sends the entries i with i % producers == k; all share the channel.
    let n_producers = generators.len();
    let producers: Vec<_> = generators
        .into_iter()
        .enumerate()
        .map(|(k, mut generator)| {
            let (produced, abort, tx) = (Arc::clone(&produced), Arc::clone(&abort), tx.clone());
            let (total, rate) = (p.total, p.rate);
            thread::spawn(move || {
                let begin = Instant::now();
                for i in (k..total).step_by(n_producers) {
                    if abort.lock().unwrap().is_some() {
                        break;
                    }
                    if rate > 0 {
                        let due = begin + Duration::from_secs_f64(i as f64 / f64::from(rate));
                        if let Some(wait) = due.checked_duration_since(Instant::now()) {
                            thread::sleep(wait);
                        }
                    }
                    let (_, msg) = generator.entry(i);
                    if tx.send(msg).is_err() {
                        break;
                    }
                    produced.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    // Only the producers' clones keep the channel open now: the pool ends when they
    // are done and the queue is drained.
    drop(tx);

    // -- pins: a few early items, which must survive everything ------------------
    let pin_after = (p.total / 20).clamp(20, 400) as u64;
    while produced.load(Ordering::Relaxed) < pin_after && !producers.iter().all(|h| h.is_finished())
    {
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(300));
    let mut pinned: Vec<(u64, ClipKind, Option<String>)> = Vec::new();
    {
        let want = [
            ClipKind::Text,
            ClipKind::Text,
            ClipKind::Image,
            ClipKind::Files,
        ];
        let candidates: Vec<(u64, ClipKind, Option<String>)> = store
            .read()
            .iter_oldest_first()
            .map(|i| (i.id, i.kind, i.blob_hash.as_deref().map(str::to_owned)))
            .collect();
        for k in want {
            if let Some(c) = candidates
                .iter()
                .find(|c| c.1 == k && !pinned.iter().any(|p| p.0 == c.0))
            {
                pinned.push(c.clone());
            }
        }
        for (id, ..) in &pinned {
            store.toggle_pin(*id);
        }
    }

    for h in producers {
        h.join().expect("producer");
    }
    pool.join();
    let ingest_elapsed = t0.elapsed();

    // -- the OCR backlog: let it drain (bounded), then stop like the shutdown does --
    let mut max_ocr = 0;
    let deadline = Instant::now() + Duration::from_secs(p.ocr_drain_secs);
    while !ocr_queue.is_idle() && Instant::now() < deadline {
        max_ocr = max_ocr.max(ocr_queue.backlog());
        thread::sleep(Duration::from_millis(100));
    }
    let ocr_left_at_stop = ocr_queue.backlog();
    ocr_worker.stop();
    drop(ocr_worker);

    // Settle: let the writer drain, then sample the memory the process keeps.
    store.persist().flush().expect("flush");
    thread::sleep(Duration::from_millis(500));
    let steady_rss = procmem::rss().map_or(0, |r| r.now);
    // How much of that is the allocator keeping free memory for reuse (glibc does,
    // the Windows heap returns big blocks at once): ask it to give it back.
    #[cfg(target_os = "linux")]
    let steady_trimmed_rss = {
        // SAFETY: malloc_trim has no preconditions.
        unsafe { libc::malloc_trim(0) };
        procmem::rss().map_or(0, |r| r.now)
    };
    #[cfg(not(target_os = "linux"))]
    let steady_trimmed_rss = 0;
    let aborted = abort.lock().unwrap().clone();
    let perf_table = match perf_path.flatten() {
        Some(path) => {
            super::sync();
            super::disable_for_harness();
            let samples: Vec<_> = super::summary::read_log(&path)
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.at_ms >= perf_since)
                .collect();
            super::summary::render_markdown(&super::summary::summarize(&samples))
        }
        None => String::new(),
    };

    stop.store(true, Ordering::Relaxed);
    sampler.join().unwrap();
    let (search_us, slow_searches) = searcher.join().unwrap();
    let samples = Arc::try_unwrap(samples).unwrap().into_inner().unwrap();

    // -- invariants ----------------------------------------------------------------
    let produced = produced.load(Ordering::Relaxed);
    let (new, bumped) = (
        sink.new.load(Ordering::Relaxed),
        sink.bumped.load(Ordering::Relaxed),
    );
    if new + bumped != produced {
        problems.push(format!(
            "entries lost: produced {produced}, reached the store {} (a worker panicked or dropped one)",
            new + bumped
        ));
    }
    if store.persist().stats().errors != 0 {
        problems.push(format!(
            "the writer reported {} failed operations",
            store.persist().stats().errors
        ));
    }
    let max_unpinned = samples.iter().map(|s| s.unpinned).max().unwrap_or(0);
    if max_unpinned > max_items {
        problems.push(format!(
            "{max_unpinned} unpinned items at once, the limit is {max_items}"
        ));
    }
    if store.read().unpinned_count() > max_items {
        problems.push("more unpinned items than the limit at the end".into());
    }
    for (id, kind, _) in &pinned {
        match store.get(*id) {
            Some(it) if it.pinned => {}
            _ => problems.push(format!("pinned {kind:?} item {id} was lost or unpinned")),
        }
    }
    let disk_cap = p.disk_cap();
    let max_disk_est = samples.iter().map(|s| s.disk_est).max().unwrap_or(0);
    let pinned_cost: u64 = {
        let g = store.read();
        g.iter_newest_first()
            .filter(|i| i.pinned)
            .map(|i| i.byte_size)
            .sum()
    };
    // Insert and eviction happen under one write lock, so a sample never sees the
    // estimate over the cap, unless the pins alone cost more than the cap (then
    // nothing unpinned is evicted for it, by design).
    if pinned_cost < disk_cap && max_disk_est > disk_cap {
        problems.push(format!(
            "estimated disk use reached {} MB, cap {} MB",
            max_disk_est / MB,
            disk_cap / MB
        ));
    }
    let disk_final = disk_use(&dir);
    // The folder also holds SQLite and its WAL, which the estimate ignores.
    if pinned_cost < disk_cap && disk_final.blobs + disk_final.thumbs > disk_cap + 4 * MB {
        problems.push(format!(
            "blobs and thumbnails take {} MB, cap {} MB",
            (disk_final.blobs + disk_final.thumbs) / MB,
            disk_cap / MB
        ));
    }
    let peak_rss = samples
        .iter()
        .map(|s| s.rss)
        .max()
        .unwrap_or(0)
        .max(steady_rss);
    if p.strict && baseline > 0 && peak_rss > baseline + RAM_PEAK_ALLOWANCE_MB * MB {
        problems.push(format!(
            "memory peaked at {} MB, baseline {} MB, allowance {RAM_PEAK_ALLOWANCE_MB} MB",
            peak_rss / MB,
            baseline / MB
        ));
    }
    // Memory of the middle third of the ingest period against its last third.
    let ingest_ms = ingest_elapsed.as_millis() as u64;
    let third = |lo: u64, hi: u64| -> u64 {
        samples
            .iter()
            .filter(|s| s.t_ms >= lo && s.t_ms < hi)
            .map(|s| s.rss)
            .max()
            .unwrap_or(0)
    };
    let rss_mid = third(ingest_ms / 3, ingest_ms * 2 / 3);
    let rss_end = third(ingest_ms * 2 / 3, ingest_ms + 1);
    if p.strict && rss_mid > 0 && rss_end > rss_mid + RAM_GROWTH_ALLOWANCE_MB * MB {
        problems.push(format!(
            "memory grows: {} MB in the middle third, {} MB in the last",
            rss_mid / MB,
            rss_end / MB
        ));
    }
    let mut s_sorted = search_us.clone();
    s_sorted.sort_unstable();
    let p95_ms = ms(percentile(&s_sorted, 95));
    if p.strict && p95_ms > SEARCH_P95_MAX_MS {
        problems.push(format!(
            "search p95 {p95_ms:.2} ms under ingest, limit {SEARCH_P95_MAX_MS} ms"
        ));
    }

    // Files on disk are exactly the files the store references.
    store.persist().flush().expect("flush");
    let before = snapshot(&store);
    let want_blobs: BTreeSet<String> = before
        .iter()
        .filter_map(|i| i.blob_hash.as_deref().map(str::to_owned))
        .collect();
    let want_thumbs: BTreeSet<String> = before
        .iter()
        .filter(|i| i.thumb.is_some())
        .map(|i| hash_hex(&i.hash.to_be_bytes()))
        .collect();
    let (have_blobs, have_thumbs) = (stems(&dir, "blobs"), stems(&dir, "thumbs"));
    if have_blobs != want_blobs {
        problems.push(format!(
            "blob files differ from the store: {} on disk, {} referenced ({} only on disk, {} only in the store)",
            have_blobs.len(),
            want_blobs.len(),
            have_blobs.difference(&want_blobs).count(),
            want_blobs.difference(&have_blobs).count()
        ));
    }
    if have_thumbs != want_thumbs {
        problems.push(format!(
            "thumbnail files differ from the store: {} on disk, {} referenced",
            have_thumbs.len(),
            want_thumbs.len()
        ));
    }
    for (id, kind, blob) in &pinned {
        if let Some(name) = blob
            && !have_blobs.contains(name)
        {
            problems.push(format!("the pinned {kind:?} item {id} lost its file"));
        }
    }

    // Graceful shutdown, reopen, same state; OCR words are still searchable.
    let ocr_done = before
        .iter()
        .filter(|i| i.kind == ClipKind::Image && i.ocr_status == OcrStatus::Done)
        .count();
    let max_cache = samples.iter().map(|s| s.cache_bytes).max().unwrap_or(0);
    let final_images = before.iter().filter(|i| i.kind == ClipKind::Image).count();
    let insert_max_us = sink.insert_max_us.load(Ordering::Relaxed);
    let mut ins = std::mem::take(&mut *sink.insert_us.lock().unwrap());
    ins.sort_unstable();
    let (insert_p50_us, insert_p99_us) = (percentile(&ins, 50), percentile(&ins, 99));
    let t = Instant::now();
    store.persist().shutdown().expect("shutdown");
    drop(sink);
    drop(store);
    let shutdown_ms = t.elapsed().as_millis();
    let t = Instant::now();
    let reopened = open_store(&dir, &settings, &OcrQueueHandle::new(Arc::clone(&settings)));
    let reopen_ms = t.elapsed().as_millis();
    let after = snapshot(&reopened);
    if before != after {
        let first = before
            .iter()
            .zip(after.iter())
            .position(|(a, b)| a != b)
            .map_or_else(
                || "lengths differ".to_owned(),
                |k| format!("first difference at position {k}"),
            );
        problems.push(format!(
            "state after reopen differs: {} items before, {} after, {first}",
            before.len(),
            after.len()
        ));
    }
    let mut req = SearchRequest::new(OCR_WORD, TypeFilter::All, reopened.now_ms());
    req.limit = 200;
    let found = reopened.search(&req, &CachedPathChecker::default());
    let ocr_hits = found
        .iter()
        .filter(|r| r.match_source == MatchSource::Ocr)
        .count();
    if ocr_done <= 200 && ocr_hits != ocr_done {
        problems.push(format!(
            "a search for the OCR word finds {ocr_hits} screenshots, {ocr_done} have OCR text"
        ));
    }
    drop(reopened);

    let report = Report {
        label: p.label,
        params: p.clone(),
        produced,
        new,
        bumped,
        aborted: aborted.clone(),
        elapsed: started.elapsed(),
        ingest_elapsed,
        baseline_rss: baseline,
        peak_rss,
        steady_rss,
        rss_mid,
        rss_end,
        max_items: samples.iter().map(|s| s.items).max().unwrap_or(0),
        max_unpinned,
        max_cache,
        cache_budget,
        max_disk_est,
        disk_cap,
        disk_final,
        max_persist_backlog: samples.iter().map(|s| s.persist_backlog).max().unwrap_or(0),
        max_ingest_queue: samples.iter().map(|s| s.ingest_queue).max().unwrap_or(0),
        max_ocr_backlog: max_ocr.max(samples.iter().map(|s| s.ocr_backlog).max().unwrap_or(0)),
        ocr_done,
        ocr_left_at_stop,
        min_free: samples.iter().map(|s| s.free).min().unwrap_or(0),
        search_us,
        insert_max_us,
        insert_p50_us,
        slow_searches,
        insert_p99_us,
        steady_trimmed_rss,
        perf_table,
        final_items: before.len(),
        final_pinned: before.iter().filter(|i| i.pinned).count(),
        final_images,
        shutdown_ms,
        reopen_ms,
        samples,
        problems,
    };
    // `tmp` (the data folder) is deleted when it drops, after the report.
    report
}

fn finish(mut r: Report) {
    if let Some(why) = &r.aborted {
        r.problems.push(format!("run stopped early: {why}"));
    }
    r.print();
    if let Some(csv) = &r.params.csv {
        r.write_csv(csv);
    }
    assert!(
        r.problems.is_empty(),
        "stress invariants failed: {:?}",
        r.problems
    );
}

// ---------------------------------------------------------------------------
// The scenarios (all `#[ignore]`: they take minutes and gigabytes of disk I/O)
// ---------------------------------------------------------------------------

/// A fast copying user: 50 entries a second, default settings (2 GB cap, 1,000 items).
#[test]
#[ignore = "stress run, see docs/perf.md"]
fn stress_burst_default_settings() {
    finish(run(&Params::from_env(
        "burst at 50 per second, default settings",
        10_000,
        50,
        0,
    )));
}

/// As fast as the pipeline takes entries, default settings.
#[test]
#[ignore = "stress run, see docs/perf.md"]
fn stress_full_speed_default_settings() {
    finish(run(&Params::from_env(
        "full speed, default settings",
        10_000,
        0,
        0,
    )));
}

/// Full speed with a small disk cap, so the cap evicts all the time.
#[test]
#[ignore = "stress run, see docs/perf.md"]
fn stress_full_speed_capped_256mb() {
    finish(run(&Params::from_env(
        "full speed, 256 MB disk cap",
        10_000,
        0,
        256,
    )));
}

/// A short run of the same code for the normal test suite: a few hundred small
/// pictures and texts, so a broken harness is noticed without a manual run.
#[test]
fn stress_harness_smoke() {
    let mut p = Params::from_env("smoke", 300, 0, 0);
    p.strict = false;
    p.perf_log = false;
    p.width = 320;
    p.height = 200;
    p.min_free_mb = 64;
    p.image_pct = 20;
    p.ocr_drain_secs = 30;
    // The normal suite must not depend on the environment of a manual run.
    (p.total, p.disk_cap_mb, p.dir) = (300, 0, None);
    p.csv = None;
    let r = run(&p);
    r.print();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.produced, 300);
    assert!(
        r.final_images > 0 && r.ocr_done > 0,
        "pictures and OCR were part of it"
    );
}
