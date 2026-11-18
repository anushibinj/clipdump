//! `--perf`: timing of the things Section 4 of PLAN.md puts a budget on.
//!
//! Start the app with `--perf` (or the environment variable `CLIPDUMP_PERF=1`) and
//! every measured event adds one line to `perf.log` in the data folder and to
//! stderr:
//!
//! ```text
//! perf t=1700000000123 ev=search_core us=412 n=37
//! ```
//!
//! `t` is unix milliseconds, `ev` the event name, `us` the duration in
//! microseconds and `n` an event specific count (results, bytes, items, pixels).
//! Never any clipboard content, not even a query: only sizes and counts.
//! [`summary`] turns such a log into the p50 and p95 table of `docs/perf.md`;
//! `clipdump --perf-summary <perf.log>` prints it, and a run with `--perf` writes
//! `perf-summary.md` next to the log when it quits.
//!
//! # Zero cost when off
//!
//! The speed rule of the project: nothing on the capture or keystroke paths may
//! pay for this when `--perf` is off. A call site does
//!
//! ```ignore
//! let t = perf::start();            // one relaxed atomic load; None when off
//! ... the measured work ...
//! perf::finish(t, Event::SearchCore, n);   // does nothing for None
//! ```
//!
//! so with the flag off the cost is a load and a branch, and `Instant::now` is
//! never called. With the flag on, `finish` pushes a small `Copy` struct into a
//! bounded channel (never blocks, drops the sample when full) and a background
//! thread formats and writes it, so even formatting and file I/O stay off the
//! measured threads.
//!
//! # Layout
//!
//! * [`Sample`], [`Event`]: the data, with `format_line` and `parse_line`.
//! * [`Logger`]: the background writer over a size bounded file ([`BoundedFile`]).
//! * [`HotkeyStamp`]: the pure state of "hotkey pressed, waiting for the paint ack".
//! * this module's free functions: the process wide switch and logger the app uses.
//!
//! Everything but the free functions is plain data and instances, so the tests do
//! not touch global state.

#[cfg_attr(not(test), allow(dead_code))]
pub mod procmem;
pub mod summary;

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, TrySendError};

/// What was measured. The name is the `ev=` value in the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// Hotkey handler to `show()` + focus + `bar:shown` emitted (the backend share).
    HotkeyToShown,
    /// Hotkey handler to the frontend's ack after two animation frames, which is
    /// the closest the app can get to "the bar is visible with a focused input".
    HotkeyToPainted,
    /// The `search` command handler, from entering the command to the result
    /// being ready (state lookup, scan, DTOs; not the IPC serialization).
    SearchIpc,
    /// What the webview saw: `invoke("search")` to its promise resolving.
    SearchRtt,
    /// `Store::search` alone (lock, scan, rank, DTOs).
    SearchCore,
    /// Capture dequeued to the item being in the store, per kind (decode, hash,
    /// thumbnail and PNG for images; no OCR). `n` is the stored byte size.
    IngestText,
    IngestImage,
    IngestFiles,
    /// OCR of one image: PNG decode plus the engine. `n` is the pixel count.
    Ocr,
    /// `Store::open`: read, decrypt, index, first retention. `n` is the item count.
    StartupLoad,
    /// Opening the history to the store being ready (preflight, integrity check,
    /// key, load): the "open to ready" figure. `n` is the item count.
    StartupReady,
    /// `copy_item`: loading the payload (RAM, cache or disk plus decrypt). `n` is bytes.
    CopyPayload,
    /// `copy_item`: from the command start to the clipboard being set, per kind.
    CopyText,
    CopyImage,
    CopyFiles,
    /// One retention pass. `n` is how many items it evicted.
    Retention,
}

impl Event {
    pub const ALL: [Event; 16] = [
        Event::HotkeyToShown,
        Event::HotkeyToPainted,
        Event::SearchIpc,
        Event::SearchRtt,
        Event::SearchCore,
        Event::IngestText,
        Event::IngestImage,
        Event::IngestFiles,
        Event::Ocr,
        Event::StartupLoad,
        Event::StartupReady,
        Event::CopyPayload,
        Event::CopyText,
        Event::CopyImage,
        Event::CopyFiles,
        Event::Retention,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Event::HotkeyToShown => "hotkey_to_shown",
            Event::HotkeyToPainted => "hotkey_to_painted",
            Event::SearchIpc => "search_ipc",
            Event::SearchRtt => "search_rtt",
            Event::SearchCore => "search_core",
            Event::IngestText => "ingest_text",
            Event::IngestImage => "ingest_image",
            Event::IngestFiles => "ingest_files",
            Event::Ocr => "ocr",
            Event::StartupLoad => "startup_load",
            Event::StartupReady => "startup_ready",
            Event::CopyPayload => "copy_payload",
            Event::CopyText => "copy_text",
            Event::CopyImage => "copy_image",
            Event::CopyFiles => "copy_files",
            Event::Retention => "retention",
        }
    }

    pub fn from_name(name: &str) -> Option<Event> {
        Event::ALL.into_iter().find(|e| e.name() == name)
    }

    /// The Section 4 budget this event is judged against, in milliseconds.
    /// `None` for events that only explain another one. The p95 is compared with
    /// it (a budget is a worst case target, not a mean).
    pub fn budget_ms(self) -> Option<f64> {
        match self {
            Event::HotkeyToPainted => Some(50.0),
            Event::SearchRtt => Some(16.0),
            Event::SearchCore => Some(1.0),
            Event::CopyText => Some(20.0),
            Event::CopyImage => Some(150.0),
            Event::StartupLoad => Some(300.0),
            Event::IngestImage => Some(300.0),
            Event::Ocr => Some(1000.0),
            _ => None,
        }
    }

    /// The ingest event of an item kind.
    pub fn ingest(kind: crate::model::ClipKind) -> Event {
        match kind {
            crate::model::ClipKind::Text => Event::IngestText,
            crate::model::ClipKind::Image => Event::IngestImage,
            crate::model::ClipKind::Files => Event::IngestFiles,
        }
    }

    /// The copy event of an item kind.
    pub fn copy(kind: crate::model::ClipKind) -> Event {
        match kind {
            crate::model::ClipKind::Text => Event::CopyText,
            crate::model::ClipKind::Image => Event::CopyImage,
            crate::model::ClipKind::Files => Event::CopyFiles,
        }
    }
}

/// One measurement. `Copy` and allocation free so recording it is a channel push.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub at_ms: u64,
    pub event: Event,
    pub micros: u64,
    pub n: u64,
}

impl Sample {
    /// A sample taken now.
    pub fn now(event: Event, d: Duration, n: u64) -> Self {
        Self {
            at_ms: unix_ms(),
            event,
            micros: u64::try_from(d.as_micros()).unwrap_or(u64::MAX),
            n,
        }
    }

    /// The log line, without a newline.
    pub fn format_line(&self) -> String {
        format!(
            "perf t={} ev={} us={} n={}",
            self.at_ms,
            self.event.name(),
            self.micros,
            self.n
        )
    }

    /// Read a line written by [`Sample::format_line`], also when it carries the
    /// `clipdump: ` prefix of the stderr copy. Anything else is `None`.
    pub fn parse_line(line: &str) -> Option<Sample> {
        let rest = line
            .trim()
            .strip_prefix("clipdump: ")
            .unwrap_or(line.trim());
        let mut it = rest.split_ascii_whitespace();
        if it.next()? != "perf" {
            return None;
        }
        let (mut at_ms, mut event, mut micros, mut n) = (None, None, None, None);
        for field in it {
            let (k, v) = field.split_once('=')?;
            match k {
                "t" => at_ms = v.parse().ok(),
                "ev" => event = Event::from_name(v),
                "us" => micros = v.parse().ok(),
                "n" => n = v.parse().ok(),
                _ => {}
            }
        }
        Some(Sample {
            at_ms: at_ms?,
            event: event?,
            micros: micros?,
            n: n?,
        })
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

// ---------------------------------------------------------------------------
// The size bounded file
// ---------------------------------------------------------------------------

/// A log file that cannot grow without limit: when the next line would take it
/// past `cap` bytes, the file is renamed to `<name>.1` (replacing an older one)
/// and a fresh file is started. So at most `2 * cap` bytes of history exist, and
/// the summary reads both.
pub struct BoundedFile {
    path: PathBuf,
    cap: u64,
    out: BufWriter<File>,
    written: u64,
}

impl BoundedFile {
    pub fn open(path: &Path, cap: u64) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata().map_or(0, |m| m.len());
        Ok(Self {
            path: path.to_owned(),
            cap,
            out: BufWriter::new(file),
            written,
        })
    }

    /// The older half of the log, `<name>.1`.
    pub fn rotated_path(path: &Path) -> PathBuf {
        let mut name = path.file_name().unwrap_or_default().to_owned();
        name.push(".1");
        path.with_file_name(name)
    }

    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let len = line.len() as u64 + 1;
        if self.written > 0 && self.written + len > self.cap {
            self.out.flush()?;
            let _ = fs::rename(&self.path, Self::rotated_path(&self.path));
            let file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.path)?;
            self.out = BufWriter::new(file);
            self.written = 0;
        }
        self.out.write_all(line.as_bytes())?;
        self.out.write_all(b"\n")?;
        self.written += len;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

// ---------------------------------------------------------------------------
// The background writer
// ---------------------------------------------------------------------------

/// Samples waiting for the writer thread. A burst past this is dropped (and
/// counted), never waited for.
const QUEUE: usize = 8192;

/// Default size of one half of the log file.
pub const LOG_CAP_BYTES: u64 = 2 * 1024 * 1024;

pub const LOG_FILE: &str = "perf.log";
pub const SUMMARY_FILE: &str = "perf-summary.md";

enum Msg {
    Sample(Sample),
    Stop,
}

/// Formats samples and writes them to the file (and optionally stderr) on its own
/// thread. [`Logger::record`] never blocks and never touches the disk.
pub struct Logger {
    tx: Sender<Msg>,
    handle: Mutex<Option<JoinHandle<()>>>,
    dropped: AtomicU64,
}

impl Logger {
    pub fn start(path: &Path, cap: u64, echo_stderr: bool) -> io::Result<Self> {
        let file = BoundedFile::open(path, cap)?;
        let (tx, rx) = crossbeam_channel::bounded(QUEUE);
        let handle = std::thread::Builder::new()
            .name("clipdump-perf".into())
            .spawn(move || write_loop(&rx, file, echo_stderr))?;
        Ok(Self {
            tx,
            handle: Mutex::new(Some(handle)),
            dropped: AtomicU64::new(0),
        })
    }

    pub fn record(&self, s: Sample) {
        if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) =
            self.tx.try_send(Msg::Sample(s))
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Samples that did not fit the queue (or came after `stop`).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Write out what is queued and end the thread. Idempotent.
    pub fn stop(&self) {
        let handle = self.handle.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(h) = handle {
            // Blocking send: the queue drains, so this returns promptly.
            let _ = self.tx.send(Msg::Stop);
            let _ = h.join();
        }
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        self.stop();
    }
}

fn write_loop(rx: &Receiver<Msg>, mut file: BoundedFile, echo: bool) {
    while let Ok(msg) = rx.recv() {
        let Msg::Sample(s) = msg else { break };
        let line = s.format_line();
        if echo {
            eprintln!("clipdump: {line}");
        }
        if let Err(e) = file.write_line(&line) {
            eprintln!("clipdump: perf log not written: {e}");
        }
        // Flush when the burst is over, so the file is current whenever the
        // queue is idle (a summary or a human can read it at any time).
        if rx.is_empty() {
            let _ = file.flush();
        }
    }
    let _ = file.flush();
}

// ---------------------------------------------------------------------------
// Hotkey to paint
// ---------------------------------------------------------------------------

/// "The hotkey was pressed at this moment, no ack yet", as one atomic. Time is
/// passed in as nanoseconds on any monotonic scale, so the logic is testable
/// without a clock. 0 means no press is waiting.
#[derive(Default)]
pub struct HotkeyStamp(AtomicU64);

impl HotkeyStamp {
    pub fn mark(&self, now_ns: u64) {
        // +1 keeps a press at exactly 0 ns distinct from "none".
        self.0.store(now_ns.saturating_add(1), Ordering::Relaxed);
    }

    /// Nanoseconds since the press, leaving the stamp in place (several backend
    /// events measure from the same press).
    pub fn peek(&self, now_ns: u64) -> Option<u64> {
        match self.0.load(Ordering::Relaxed) {
            0 => None,
            s => Some(now_ns.saturating_sub(s - 1)),
        }
    }

    /// Like [`HotkeyStamp::peek`] but consumes the stamp: one ack per press.
    pub fn take(&self, now_ns: u64) -> Option<u64> {
        match self.0.swap(0, Ordering::Relaxed) {
            0 => None,
            s => Some(now_ns.saturating_sub(s - 1)),
        }
    }
}

// ---------------------------------------------------------------------------
// The process wide switch (what the app calls)
// ---------------------------------------------------------------------------

static ENABLED: AtomicBool = AtomicBool::new(false);
static LOGGER: OnceLock<Logger> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static HOTKEY: HotkeyStamp = HotkeyStamp(AtomicU64::new(0));
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Whether `--perf` is on. One relaxed load.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Does the command line or the environment ask for perf logging? `env` is the
/// value of `CLIPDUMP_PERF`; anything but empty, `0` and `false` turns it on.
pub fn wants_perf<I, S>(args: I, env: Option<&str>) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let flag = args.into_iter().any(|a| a.as_ref() == "--perf");
    let env = env.is_some_and(|v| !matches!(v.trim(), "" | "0" | "false"));
    flag || env
}

/// The path after `--perf-summary`, if that flag is present.
pub fn summary_arg<I, S>(args: I) -> Option<Option<PathBuf>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut it = args.into_iter();
    it.by_ref().find(|a| a.as_ref() == "--perf-summary")?;
    Some(it.next().map(|p| PathBuf::from(p.as_ref())))
}

/// Read the process arguments and the environment once, at the start of `run`.
pub fn init_from_process() {
    let env = std::env::var("CLIPDUMP_PERF").ok();
    if wants_perf(std::env::args(), env.as_deref()) {
        EPOCH.get_or_init(Instant::now);
        ENABLED.store(true, Ordering::Relaxed);
    }
}

/// Start the log file writer in `dir` (the data folder). A no-op when perf is
/// off; a failure only turns the log off, it is never fatal.
pub fn start_logger(dir: &Path) {
    if !enabled() {
        return;
    }
    let path = dir.join(LOG_FILE);
    match Logger::start(&path, LOG_CAP_BYTES, true) {
        Ok(l) => {
            let _ = LOGGER.set(l);
            let _ = LOG_PATH.set(path.clone());
            eprintln!("clipdump: perf logging to {}", path.display());
        }
        Err(e) => {
            eprintln!("clipdump: perf log not started: {e}");
            ENABLED.store(false, Ordering::Relaxed);
        }
    }
}

/// Start a measurement: `Some(now)` with `--perf`, else `None` and no clock read.
#[inline]
pub fn start() -> Option<Instant> {
    enabled().then(Instant::now)
}

/// End a measurement started with [`start`]; `n` is the event's count.
#[inline]
pub fn finish(t: Option<Instant>, event: Event, n: u64) {
    if let Some(t) = t {
        record(event, t.elapsed(), n);
    }
}

/// Record a duration measured by the caller (who checked [`enabled`]).
pub fn record(event: Event, d: Duration, n: u64) {
    if let Some(l) = LOGGER.get() {
        l.record(Sample::now(event, d, n));
    }
}

fn now_ns() -> u64 {
    let epoch = EPOCH.get_or_init(Instant::now);
    u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// The hotkey handler calls this first thing.
#[inline]
pub fn mark_hotkey() {
    if enabled() {
        HOTKEY.mark(now_ns());
    }
}

/// Log the time since the hotkey press as `event`, without consuming the stamp.
/// Called when `show()` is done.
pub fn hotkey_elapsed(event: Event) {
    if enabled()
        && let Some(ns) = HOTKEY.peek(now_ns())
    {
        record(event, Duration::from_nanos(ns), 0);
    }
}

/// What the webview reports back (command `perf_ack`).
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AckKind {
    /// Two animation frames after `bar:shown`.
    BarPainted,
    /// A `search` round trip as the webview measured it, in `ms`.
    SearchRtt,
}

/// The event and duration an ack stands for, or `None` when there is nothing to
/// log (no hotkey press waiting, no or a nonsense duration). Pure, so the
/// command is a thin wrapper.
pub fn ack_event(
    kind: AckKind,
    ms: Option<f64>,
    hotkey_ns: impl FnOnce() -> Option<u64>,
) -> Option<(Event, Duration)> {
    match kind {
        AckKind::BarPainted => {
            hotkey_ns().map(|ns| (Event::HotkeyToPainted, Duration::from_nanos(ns)))
        }
        AckKind::SearchRtt => {
            let ms = ms.filter(|m| m.is_finite() && *m >= 0.0)?;
            Some((Event::SearchRtt, Duration::from_secs_f64(ms / 1000.0)))
        }
    }
}

/// Handle a `perf_ack` from the webview. Does nothing unless `--perf` is on.
pub fn ack(kind: AckKind, ms: Option<f64>) {
    if !enabled() {
        return;
    }
    if let Some((event, d)) = ack_event(kind, ms, || HOTKEY.take(now_ns())) {
        record(event, d, 0);
    }
}

/// Whether the webview should send acks (`get_stats`, field `perf`).
pub fn webview_acks() -> bool {
    enabled()
}

/// At exit: write what is queued, then the summary next to the log. A no-op
/// without `--perf`.
pub fn on_exit() {
    if !enabled() {
        return;
    }
    if let Some(l) = LOGGER.get() {
        l.stop();
        if l.dropped() > 0 {
            eprintln!(
                "clipdump: perf: {} samples did not fit the queue and were dropped",
                l.dropped()
            );
        }
    }
    if let Some(path) = LOG_PATH.get() {
        let out = path.with_file_name(SUMMARY_FILE);
        match summary::summarize_log(path) {
            Ok(md) => {
                if let Err(e) = fs::write(&out, md) {
                    eprintln!("clipdump: perf summary not written: {e}");
                } else {
                    eprintln!("clipdump: perf summary at {}", out.display());
                }
            }
            Err(e) => eprintln!("clipdump: perf summary not written: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(event: Event, micros: u64, n: u64) -> Sample {
        Sample {
            at_ms: 1_700_000_000_123,
            event,
            micros,
            n,
        }
    }

    #[test]
    fn every_event_name_round_trips_and_is_unique() {
        let mut names = std::collections::HashSet::new();
        for e in Event::ALL {
            assert_eq!(Event::from_name(e.name()), Some(e));
            assert!(names.insert(e.name()), "duplicate name {}", e.name());
        }
        assert_eq!(Event::from_name("nope"), None);
    }

    #[test]
    fn a_line_round_trips_with_and_without_the_stderr_prefix() {
        let s = sample(Event::SearchCore, 412, 37);
        let line = s.format_line();
        assert_eq!(line, "perf t=1700000000123 ev=search_core us=412 n=37");
        assert_eq!(Sample::parse_line(&line), Some(s));
        assert_eq!(Sample::parse_line(&format!("clipdump: {line}\n")), Some(s));
    }

    #[test]
    fn other_lines_are_not_samples() {
        for bad in [
            "",
            "clipdump: ingested text, 5 bytes in 1 ms",
            "perf t=1 ev=unknown_event us=1 n=1",
            "perf t=x ev=ocr us=1 n=1",
            "perf t=1 ev=ocr us=1",
            "perf ev=ocr us=1 n=1",
        ] {
            assert_eq!(Sample::parse_line(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_line_never_carries_more_than_numbers_and_the_event_name() {
        let line = sample(Event::IngestImage, 9, 1).format_line();
        assert!(
            line.chars()
                .all(|c| c.is_ascii_alphanumeric() || " =_".contains(c))
        );
    }

    #[test]
    fn perf_is_wanted_by_the_flag_or_the_environment() {
        assert!(wants_perf(["clipdump", "--perf"], None));
        assert!(!wants_perf(["clipdump"], None));
        assert!(!wants_perf(["clipdump", "--perf-summary", "x"], None));
        assert!(wants_perf(["clipdump"], Some("1")));
        assert!(wants_perf(["clipdump"], Some("yes")));
        for off in ["", "0", "false", " 0 "] {
            assert!(!wants_perf(["clipdump"], Some(off)), "{off:?}");
        }
    }

    #[test]
    fn the_summary_flag_takes_the_next_argument() {
        assert_eq!(
            summary_arg(["c", "--perf-summary", "a.log"]),
            Some(Some("a.log".into()))
        );
        assert_eq!(summary_arg(["c", "--perf-summary"]), Some(None));
        assert_eq!(summary_arg(["c", "--perf"]), None);
    }

    #[test]
    fn the_hotkey_stamp_is_peeked_many_times_and_taken_once() {
        let h = HotkeyStamp::default();
        assert_eq!(h.peek(5), None);
        assert_eq!(h.take(5), None);
        h.mark(100);
        assert_eq!(h.peek(150), Some(50));
        assert_eq!(h.peek(170), Some(70), "peek leaves the stamp");
        assert_eq!(h.take(200), Some(100));
        assert_eq!(h.take(300), None, "one ack per press");
        // A press at time zero is still a press.
        h.mark(0);
        assert_eq!(h.take(40), Some(40));
    }

    #[test]
    fn acks_become_events_only_when_there_is_something_to_log() {
        let ns = || Some(12_000_000);
        assert_eq!(
            ack_event(AckKind::BarPainted, None, ns),
            Some((Event::HotkeyToPainted, Duration::from_millis(12)))
        );
        assert_eq!(
            ack_event(AckKind::BarPainted, None, || None),
            None,
            "no press waiting"
        );
        assert_eq!(
            ack_event(AckKind::SearchRtt, Some(2.5), || None),
            Some((Event::SearchRtt, Duration::from_micros(2500)))
        );
        for bad in [None, Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
            assert_eq!(ack_event(AckKind::SearchRtt, bad, ns), None, "{bad:?}");
        }
    }

    #[test]
    fn the_ack_kinds_use_the_wire_names_of_the_frontend() {
        let k: AckKind = serde_json::from_str("\"barPainted\"").unwrap();
        assert_eq!(k, AckKind::BarPainted);
        let k: AckKind = serde_json::from_str("\"searchRtt\"").unwrap();
        assert_eq!(k, AckKind::SearchRtt);
        assert!(serde_json::from_str::<AckKind>("\"other\"").is_err());
    }

    #[test]
    fn the_bounded_file_rotates_and_never_exceeds_twice_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf.log");
        let mut f = BoundedFile::open(&path, 100).unwrap();
        for i in 0..50 {
            f.write_line(&format!("line number {i:04}")).unwrap();
        }
        f.flush().unwrap();
        let cur = fs::metadata(&path).unwrap().len();
        let old = fs::metadata(BoundedFile::rotated_path(&path))
            .unwrap()
            .len();
        assert!(cur <= 100 && old <= 100, "cur {cur}, old {old}");
        // The newest line is in the current file, nothing was cut in half.
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("line number 0049\n"));
        assert!(text.lines().all(|l| l.starts_with("line number ")));
    }

    #[test]
    fn a_line_longer_than_the_cap_is_still_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf.log");
        let mut f = BoundedFile::open(&path, 10).unwrap();
        f.write_line("a line well past ten bytes").unwrap();
        f.flush().unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "a line well past ten bytes\n"
        );
    }

    #[test]
    fn the_logger_writes_every_sample_off_the_calling_thread_and_stop_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf.log");
        let l = Logger::start(&path, LOG_CAP_BYTES, false).unwrap();
        for i in 0..100 {
            l.record(sample(Event::SearchCore, i, i));
        }
        l.stop();
        l.stop();
        let got: Vec<Sample> = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter_map(Sample::parse_line)
            .collect();
        assert_eq!(got.len(), 100);
        assert_eq!(got[99], sample(Event::SearchCore, 99, 99));
        assert_eq!(l.dropped(), 0);
        // After stop a record is dropped and counted, not a panic or a hang.
        l.record(sample(Event::Ocr, 1, 1));
        assert_eq!(l.dropped(), 1);
    }

    #[test]
    fn recording_never_blocks_when_the_queue_is_full() {
        let dir = tempfile::tempdir().unwrap();
        let l = Logger::start(&dir.path().join("p.log"), LOG_CAP_BYTES, false).unwrap();
        let t = Instant::now();
        for i in 0..(QUEUE as u64 * 4) {
            l.record(sample(Event::SearchCore, i, 0));
        }
        // Any wait on the writer would take far longer than this.
        assert!(t.elapsed() < Duration::from_secs(5));
        l.stop();
    }

    #[test]
    fn with_perf_off_start_reads_no_clock_and_finish_does_nothing() {
        // The global flag is off in unit tests (nothing here turns it on).
        assert!(!enabled());
        assert!(start().is_none());
        finish(None, Event::SearchCore, 1);
        mark_hotkey();
        ack(AckKind::SearchRtt, Some(1.0));
        assert!(!webview_acks());
    }
}
