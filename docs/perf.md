# Performance verification (Phase 14)

## Status, in plain words

- **No Windows budget has been measured.** The container is Linux (Phase 0, `docs/environment.md`):
  there is no WebView2, no Windows clipboard, no Windows OCR, no DPAPI, no NSIS, no Windows file
  system or Defender. Every row of PLAN.md Section 4 below is **PENDING on Windows**, each with the
  exact command or step that produces its number.
- **PLAN.md Section 7, "`docs/perf.md` shows all budgets met", is NOT satisfied** and stays unchecked
  until the Windows column of the table is filled in with real numbers and every one is under its
  target. Nothing in this file says a Windows budget is met.
- What this phase did deliver: the measurement tooling (a `--perf` mode in the app, a p50 and p95
  summary, a stress harness, a restart test, a cold start measurement), relative figures from Linux
  for everything that can honestly be measured there (labelled as such, never a verification), five
  speed-ups found on the way, and a Windows procedure with one command per row.
- Over budget on Linux after the optimizations: nothing that is clearly a code problem. Honest
  exceptions are listed under "Findings and risks" (a slow first show of a never shown window, search
  p95 of about 2 ms while two ingest threads and everything else saturate the 4 CPUs, 4K image ingest
  p95 of up to 255 ms under that same saturation). The Windows run decides whether they matter.

## Environment and method

| | |
|---|---|
| Machine | Linux 6.18 microVM (Firecracker), 4 vCPU Intel Xeon at 2.10 GHz (AVX2 and AVX-512 present), 16 GB RAM, virtual block disk (not an NVMe) |
| Toolchain | rustc 1.97.0, Node 22.23.5, pnpm 10.28.0, Chromium (preinstalled Playwright), WebKitGTK 4.1 for the app, Xvfb with openbox |
| Rust build | the `perf` Cargo profile (`src-tauri/Cargo.toml`): release optimization (`opt-level 3`), **no LTO**, 16 codegen units, no debug info. The package under test and the crates on the measured paths (image and PNG, SQLite, the cipher, the matcher, hashing, rayon, channels) are optimized; the windowing stack (Tauri, GTK) is not, because nothing measured runs inside it. |
| Shipping profile | `[profile.release]` is fat LTO with one codegen unit. This container cannot afford to build that repeatedly (disk), so no figure here comes from it. LTO only helps, so shipping numbers should be equal or better; that is a reasoned expectation, not a measurement. |
| Noise | The VM is shared: the same measurement repeated minutes apart differed by up to 25 percent. Only paired comparisons (before and after in one run) and orders of magnitude mean anything. |
| What "relative" means | A Linux figure shows where the time goes and that nothing is absurd. It says nothing about Windows: different OS, disk, allocator, webview and clipboard. |

How each figure was taken:

- **Timing log**: `clipdump --perf` (or `CLIPDUMP_PERF=1`) writes one line per event to `perf.log` in the
  data folder and to stderr (`perf t=<unix ms> ev=<name> us=<microseconds> n=<count>`; never clipboard
  content, not even a query) and, at quit, a table in `perf-summary.md`. `clipdump --perf-summary
  <perf.log>` prints the table for an existing log. With the flag off the cost is one relaxed atomic
  load per call site and no clock read, no thread, no extra IPC (`perf::start` returns `None`).
  Events: `hotkey_to_shown`, `hotkey_to_painted`, `search_ipc`, `search_rtt`, `search_core`,
  `ingest_text`, `ingest_image`, `ingest_files`, `ocr`, `startup_load`, `startup_ready`,
  `copy_payload`, `copy_text`, `copy_image`, `copy_files`, `retention`. Samples go through a bounded
  channel to a background thread, so formatting and file I/O are off the measured threads.
- **The webview ack**: after `bar:shown` the page waits two animation frames and calls `perf_ack`;
  the backend logs hotkey handler to that ack as `hotkey_to_painted`. The same command carries the
  webview's own `search` round trip (`search_rtt`). Both are sent only when `get_stats().perf` says
  the app runs with `--perf`.
- **Stress and restart**: `src-tauri/src/perf/stress.rs`, `restart.rs`, `ingest.rs`, run through
  `node scripts/stress.mjs` (or `cargo test --profile perf --lib <name> -- --ignored --nocapture`).
  They use the real ingest pool, store, retention, encrypted persistence and OCR worker (mock engine);
  the harness only plays the clipboard watcher.
- **The real app** ran under Xvfb and openbox (`cargo build --profile perf --features
  tauri/custom-protocol --bin clipdump`, so the UI is embedded), a 1,000 item history on disk, the
  hotkey sent with `xdotool`.
- **Keystroke**: `node scripts/ui-check.mjs perf --prod` (Chromium, production build of the UI, a
  JavaScript fake backend, 1,000 fake items).

## The table

Legend: "Linux" columns are relative figures (see above), not verifications. Every "Windows" cell is
PENDING; the command is what to run on Windows, with a build from `pnpm tauri build` (or `pnpm tauri
dev --release`), details in `docs/manual-checks.md`, Phase 14.

### PLAN.md Section 4

| Metric | Target | Linux relative figure (how measured) | Windows status | Notes |
|---|---|---|---|---|
| Hotkey press to bar visible with focused input | < 50 ms | Real app, Xvfb, WebKitGTK, software GL, 1,000 items: `hotkey_to_painted` (hotkey handler to two frames after `bar:shown`) **median 20.3 ms, p95 23.4 ms** over 25 presses with the experimental pre-warm on; backend share (`hotkey_to_shown`) median 0.65 ms, p95 0.84 ms. **Without the pre-warm the very first press took 326 and 375 ms** in two runs (62 ms in an earlier one), later presses 20 ms. | **PENDING.** `clipdump.exe --perf`, press the hotkey 30 times with Esc between, read `hotkey_to_painted` p95 in `perf-summary.md`; note the first press separately. | The first show of a never shown window is a real finding (below). `CLIPDUMP_PREWARM=1` is the experiment that removed it on Linux (first press 21.6 and 23.4 ms); it is off by default until Windows says it helps and does not take focus. |
| Keystroke to results rendered (1,000 items incl. OCR text) | < 16 ms | `ui-check.mjs perf --prod`, Chromium, fake backend: input event to new rows in the DOM **p50 2.4 ms, p95 6.3 ms** (IPC 0 ms); with a 2 ms IPC p50 3.8, p95 7.3; with a 4x CPU slowdown p50 9.6, p95 27.8. Until the next animation frame: p50 9.5, p95 21 (includes waiting for the 60 Hz boundary, not app work). Real app, WebKitGTK: `search_rtt` p50 3 ms, p95 5 to 7 ms. | **PENDING.** DevTools Performance recording while typing (steps in `manual-checks.md`, Phase 10, "Real keystroke latency"); plus `search_rtt` in `perf-summary.md` for the IPC part. | The mock backend is JavaScript, so this measures React and the DOM, not Rust. A 4x slower CPU is over one frame at p95: a slow PC is the thing to watch. |
| Search itself in Rust (1,000 items) | < 1 ms | `search_timing_1000_items` (perf profile, the 1,000 item history of the cold start test: 300 screenshots with 1,500 characters of OCR text, 600 texts, 100 file lists), 400 searches per query, **p95 under 0.7 ms for every query** (empty 0.017, `m` 0.195, `mee` 0.682, `meeting` 0.364, `snap` 0.623, `fn ` 0.372, no match 0.292, `img:` 0.010 ms); worst single search 2.9 ms. Empty query with a real `stat` of every file row, cache empty: p50 0.30 ms, p95 0.51 ms. In the real app (`search_core`): p50 0.40, p95 0.79 ms (26 samples). Under heavy ingest (stress) p50 0.9, p95 about 2 ms, see below. | **PENDING.** `cd src-tauri; cargo bench --bench search_bench` (read the `search_1000/full` lines) and `search_core` p95 from a `--perf` session. | On Windows the file rows of the first 50 results are `stat`ed (up to 128 paths per item, cached 2 s): measure `search_core` with about 100 file lists of 200 paths in the history; a slow `stat` (network share, Defender) is the thing that can push it over. |
| Ctrl+Enter to clipboard set (text) | < 20 ms | The clipboard write is not measurable here (the Linux writer is a stub that fails). Measurable part: the text payload comes from RAM up to 64 KB (else the cache, else SQLite) and the UTF-16 conversion of `CF_UNICODETEXT` takes 0.05 ms for 10 KB and 4.8 to 7.5 ms for 1 MB. | **PENDING.** `--perf` session: `copy_text` p95 (copy a text of 1 KB and one of 1 MB). | `copy_text` covers the whole command from its start to the clipboard being set. |
| Ctrl+Enter to clipboard set (image, cache miss, 4K screenshot) | < 150 ms | The CPU and disk parts of the Windows writer, measured here (`copy_image_cpu_parts_4k_cache_miss`, 8 pictures of 3840x2160, PNG 0.8 to 4.0 MB, page cache dropped so every read is cold): payload load (file read and decrypt) median 39 ms, PNG decode 41 ms (30 in other runs), build `CF_DIBV5` bytes 12 ms (**was 59 ms**, see optimizations); **sum median 93 ms, max 112 ms** (was median 130, max 163). The Win32 calls (`GlobalAlloc` of 33 MB, two `SetClipboardData`) are not in this number. | **PENDING.** Restart the app (empty blob cache), then Ctrl+Enter a 4K screenshot with `--perf`: `copy_image` p95 (and `copy_payload` for its disk part). | About 55 ms are left for the Win32 part. The cold disk read here (about 30 ms) is a virtual disk; NVMe should be much faster, a hard disk slower. |
| Cold start: load 1,000 items from disk and ready to search | < 300 ms | `cold_start_1000_items_300_screenshots` (1,000 items, 300 screenshots of 1920x1080 with real thumbnails and about 1,500 characters of OCR text each, 219 MB on disk): **first open after dropping the page cache: ready 116.6 ms, `Store::open` 40.1 ms**; seven warm opens: ready median 36.4 ms (34.6 to 41.4), `Store::open` median 30.2 ms. The real app logs it too: `startup_load` 15.5 to 18.7 ms, `startup_ready` 23.6 to 29.7 ms for the same kind of history (warm file cache). | **PENDING.** Restart the PC, sign in, start `clipdump.exe --perf` once with a 1,000 item history: `startup_load` is the figure. `node scripts/stress.mjs cold` with `CLIPDUMP_RESTART_KEEP_DIR` builds such a history (steps in `manual-checks.md`). | Cold on Windows also pays for Defender scanning each thumbnail file the first time and the DPAPI unprotect of the key. |
| Clipboard event to item visible in history (image, excluding OCR) | < 300 ms | Idle machine, `ingest_image_4k_stages`, 3840x2160, medians of 7: `image::build` **PNG on the clipboard 73 ms** (was 145), **`CF_DIBV5` 219 ms** (was 409), **`CF_DIB` 24 bit 209 ms** (was 399); `Store::insert` p50 0.006 to 0.015 ms. Under saturation (stress, two ingest threads plus a producer, OCR, writer, searcher on 4 vCPUs) `ingest_image` p50 183 to 198 ms, p95 223 to 254 ms, max 351 ms. | **PENDING.** `--perf` session: take 10 Win+Shift+S screenshots at the biggest monitor's resolution, read `ingest_image` p95. | Excludes the watcher's own read of the clipboard (Windows only) and the `history:changed` round trip to the webview. Whether Win+Shift+S offers the registered PNG format (the 73 ms path) or only a DIB (the 209 to 219 ms path) depends on the Windows version and the capture tool: check `ingest_image` for both. |
| OCR of a 1080p screenshot (background) | < 1 s | Not measurable: Windows.Media.Ocr does not exist here. The harness uses a mock engine; the `ocr` event then measures the PNG decode of a 4K picture plus the mock: p50 32 to 38 ms, p95 57 to 67 ms. | **PENDING.** `--perf` session: screenshots of a 1080p window full of text, `ocr` p95 (`n` is the pixel count, 2,073,600 for 1080p). | Runs at idle thread priority, so under load it is slower by design; the budget is judged on an otherwise idle machine. |
| Idle RAM (whole process incl. WebView2) | < 120 MB | Not comparable (WebKitGTK, not WebView2; debug build). The Rust side only: the main process of the debug build with an empty history had 38 MB anonymous memory (`RssAnon`) and 173 MB `VmRSS` including mapped libraries. After the 10,000 entry stress run the process keeps 290 to 410 MB resident once the allocator is asked to give free memory back (glibc keeps freed memory otherwise: 446 to 521 MB), of which the blob cache is at most 64 MB. | **PENDING.** Task Manager (private working set of Clipdump plus its `msedgewebview2` children) or the PowerShell line in `manual-checks.md`, Phase 14, "Idle RAM and CPU", measured after a fresh start. | **Risk, not a measurement:** the WebView2 processes alone may take much of this budget on some machines. The Rust side adds up to 64 MB of blob cache (setting `ramCacheBytes`), which the OCR worker fills when it reads screenshots, see findings. |
| Idle CPU | about 0% | Debug build, three processes (app and WebKit), Xvfb, 30 s idle after 15 s: **7 ticks, 0.07 s of CPU, 0.23 percent of one core**. All Rust threads block on channels or condition variables (writer, ingest, OCR, retention, perf logger, history debouncer); there is no polling. | **PENDING.** Task Manager, CPU column of Clipdump and all its children stays 0 for a minute (steps in `manual-checks.md`). | Event driven by construction (`AddClipboardFormatListener`, no timer except the 15 minute retention sleep). |
| Installer size | < 12 MB | Not built (Phase 15). The perf profile executable is 17.6 MB stripped with unoptimized dependencies; it says nothing about the installer. | **PENDING (Phase 15).** `pnpm tauri build`, size of the NSIS installer. | |

### PLAN.md Section 7 rows that performance and the restart test touch

| Row | Linux evidence (relative, not a verification) | Windows status |
|---|---|---|
| Starts at login, idle CPU about 0% | See idle CPU above. | **PENDING** (autostart and tray are Windows features, `manual-checks.md` Phases 2 and 12). |
| Hotkey shows bar in under 50 ms with input focused | See the first table row. | **PENDING** |
| Retention: never more than 1,000 unpinned items | Stress, three runs of 10,000 entries: **at most 1,000 unpinned items at every 250 ms sample and at the end**, plus the pinned ones (1,004 in total). Age limit: unit tests (`store/retention.rs`), not part of the stress run (all entries are new). | **PENDING** (`node scripts/stress.mjs`, same code; the age limit needs a real 30 day old history or the unit tests). |
| Pinned items survive retention and clear | Stress: four pins (two texts, an image, a file list) set at 5 percent progress were present, pinned and with their files at the end of every run. Unit tests cover "clear history". | **PENDING** (stress on Windows) |
| Delete and clear remove blob files | After every stress run the blob and thumbnail files on disk are exactly the files the store references (set equality, checked after a flush); evictions during the run deleted theirs. | **PENDING** (stress on Windows) |
| RAM stays under budget with many large screenshots; blobs live on disk | Stress: resident memory is flat across the run (last third at most 5 MB above the middle third, limit 150 MB), the blob cache never exceeds its 64 MB budget, blobs and thumbnails live on disk only (243 to 280 MB of files at the end). The peak is transient: **+583 to +664 MB** over the harness baseline while two 4K pictures are decoded, converted and encoded at once plus the queue of raw pictures the harness holds (limit 900 MB). | **PENDING** (stress on Windows; Task Manager) |
| History, pins and OCR text survive a PC restart; data on disk is encrypted | `restart_keeps_items_pins_and_ocr_search` passes: whole state (every `ClipItem` field), eight search answers including one for an OCR word, and the pins, identical after a graceful shutdown and reopen, after a copy of the data folder taken while the app ran (a kill: WAL and all) and after a second restart. Encryption: Phase 3 tests. | **PENDING.** Real reboot procedure in `manual-checks.md`, Phase 14, "Restart test". |
| `cargo test` and `pnpm tsc --noEmit` pass | Yes, in this container (see the hand-off). | Run on Windows by the user. |
| **`docs/perf.md` shows all budgets met** | **NOT MET.** Windows column empty. | **PENDING** |

## Results in detail

### Stress runs (Linux, perf profile)

`node scripts/stress.mjs` (three scenarios, 10,000 entries each). Each entry is drawn from a seeded
generator: 12 percent are 3840x2160 screenshots (PNG 40 percent, `CF_DIBV5` 40, 24 bit `CF_DIB` 20; each
one a different picture so hashes differ; PNGs 0.8 to 4.0 MB), and of the rest 40 percent short texts, 18
URLs, 17 code, 17 paragraphs of 1 to 60 KB, 2 big texts of 100 KB to 1 MB and 6 file lists of 1 to 200
paths (a few of them real files, some images, so `stat` and file thumbnails run); every 53rd entry is a
repeat of a recent text. All of it goes through the real `IngestPool`, `Store`, retention, encrypted
persistence and an `OcrWorker` with a mock engine, with the default settings (1,000 items, 64 MB cache)
and a temp data folder. One run is 9,812 new items and 188 duplicates that bump an existing item. Four items are
pinned early. A search runs every 50 ms the whole time (`commands::search_items`, the function behind
the IPC command). The harness samples memory, item counts, cache, disk, the writer's queue, the ingest
channel and the OCR queue every 250 ms. It stops by itself if free disk space falls below 2 GB
(lowest seen: 3.5 GB). All figures are from one run each, 2026-11-18.

| | Burst, 50 entries per second, default settings | Full speed, default settings | Full speed, 256 MB disk cap |
|---|---:|---:|---:|
| Entries pushed through | 10,000 (new 9,812, duplicates 188) | 10,000 | 10,000 |
| Duration of the ingest | 200 s | 113 s (about 89 entries per second sustained) | 115 s |
| Max items / max unpinned (limit 1,000) | 1,004 / 1,000 | 1,004 / 1,000 | 1,004 / 1,000 |
| Items at the end (pinned, images) | 1,004 (4, 111) | 1,004 (4, 118) | 1,003 (4, 96) |
| Peak resident memory (baseline) | 625 MB (41) | 720 MB (73) | 737 MB (73) |
| Last third against middle third | +4 MB | +5 MB | +0 MB |
| Resident memory after settling, and after `malloc_trim` | 446 MB, 290 MB | 477 MB, 317 MB | 521 MB, 406 MB |
| Blob cache max (budget 64 MB) | 64.0 MB | 64.0 MB | 63.9 MB |
| Estimated disk use max (cap) | 381 MB (2,048) | 373 MB (2,048) | **256 MB (256)** |
| Folder at the end: blobs, thumbs, database and WAL | 280, 1, 25 MB | 242, 1, 27 MB | 243, 1, 27 MB |
| Lowest free disk | 3,504 MB | 3,515 MB | 3,620 MB |
| Writer queue max / ingest channel max (of 4) / OCR queue max | 27 / 4 / 2 | 85 / 4 / 5 | 92 / 4 / 5 |
| Search every 50 ms while ingesting: p50 / p95 / p99 / max | 0.92 / 2.02 / 3.18 / 6.7 ms | 0.91 / 2.35 / 5.75 / 22.2 ms | 0.97 / 3.11 / 5.87 / 9.7 ms |
| `Store::insert` p50 / p99 / max (includes waiting for the lock) | 0.015 / 0.14 / 5.6 ms | 0.006 / 0.11 / 8.0 ms | 0.006 / 0.17 / 8.0 ms |
| `ingest_image` p50 / p95 / max (n = 1,191, 1,170, 1,170) | 183 / 223 / 259 ms | 193 / 249 / 326 ms | 198 / 254 / 351 ms |
| `ingest_text` p95, `ingest_files` p95 | 0.16, 0.90 ms | 0.12, 0.80 ms | 0.13, 0.96 ms |
| `ocr` (mock engine, 4K decode) p50 / p95 | 35 / 57 ms | 32 / 60 ms | 35 / 67 ms |
| Graceful shutdown, reopen | 6 ms, 66 ms | 11 ms, 78 ms | 12 ms, 65 ms |
| Invariants | all held | all held | all held |

Invariants checked by the harness (a failure is a panic): every entry reached the store (none lost to a
panicking worker), the writer reported no failed operation, never more than 1,000 unpinned items,
pins survive with their files, the estimated disk use never exceeds the cap (and the files are
within 4 MB of it), memory is bounded and does not grow, search p95 stays under 5 ms, the files on disk
are exactly the files the store references, and a graceful shutdown plus reopen gives an identical
state and the OCR word is found on exactly the screenshots that have OCR text.

Reading the numbers:

- The default 2 GB disk cap is never reached with the default 1,000 item limit unless nearly every
  item is a multi-megabyte screenshot, and this container does not have the 2 GB to spare for that
  run, so the cap was exercised with a 256 MB cap: the oldest screenshots are evicted first, the
  estimate sits at the cap, and the folder stays within 15 MB of it (the rest is SQLite and the
  WAL, which the cap does not count).
- Full speed is not "fast enough to be a problem": the ingest keeps up with about 89 entries per
  second including 11 screenshots per second, the 4-slot channel is full much of the time (the
  producers wait, nothing is dropped here; the real watcher drops when its 64 slot channel is full).
- The memory peak is transient work, not retained: two 4K pictures in flight per worker (raw bytes,
  decoded RGBA, RGB for the PNG, the PNG, the thumbnail), the 64 MB cache, the writer's queue, the
  harness's own queued raw pictures (4 x 33 MB) and its two generators (66 MB). After the run
  memory falls to 290 to 410 MB once glibc is told to release its free lists; Windows' heap behaves
  differently, so only the shape (flat, bounded) transfers.
- The slow searches (22 ms once in the second run; the same query asked again at once took 0.7 ms)
  are scheduler stalls on a fully loaded 4 vCPU VM, not a slow query, but they show what a keystroke
  can meet when ingest saturates every core. On Windows the ingest threads run at below normal
  priority; this container's stub does not lower them.

### First show against later shows (the hidden window)

What happens before the first press (checked in the code and in the run): the window `main` is created
at startup from `tauri.conf.json` (`visible: false`, never destroyed); `setup` runs before the page
loads; the page loads while hidden; React mounts; the font is requested (`document.fonts.load`);
the first empty search is sent by `useSearch` on mount and answered; the first rows render; the
first rows' thumbnails are requested; `get_stats` and `get_settings` are loaded. `window::show()`
waits on nothing slow: one `GetForegroundWindow`, a cursor and monitor query, `set_position`, a
`set_size` only when the monitor's scale changed, `show`, `set_focus`, one `emit`; no disk, no lock
beyond atomics, no decoding. Its backend share measured 0.4 to 0.8 ms.

What the Xvfb run showed (25 presses per run, show then hide, 1,000 item history, 8 s after start):

| Run | First press | Second press | Median of the rest | p95 of the rest |
|---|---:|---:|---:|---:|
| no pre-warm, run 1 | 374.9 ms | 38.4 ms | 20.3 ms | 22.1 ms |
| no pre-warm, run 2 | 325.7 ms | 24.9 ms | 20.4 ms | 22.3 ms |
| `CLIPDUMP_PREWARM=1`, run 1 | 21.6 ms | 23.3 ms | 20.4 ms | 21.1 ms |
| `CLIPDUMP_PREWARM=1`, run 2 | 23.4 ms | 22.0 ms | 20.3 ms | 22.0 ms |

So on Linux the first show of a window that was never shown is 15 times slower than later ones
(WebKitGTK creates and renders its first frame only when mapped), although everything above is
done while hidden. Showing the window once, off screen (-32000, -32000), 1.5 s after start for
0.4 s makes the first press like the rest. That is `window::prewarm`, enabled only by
`CLIPDUMP_PREWARM=1`, **off by default**: whether WebView2 behaves like this is unknown, and a
shown window may take the keyboard focus from the user's current app at login, which nothing in this
container can check. Windows step: compare the first and the median `hotkey_to_painted` with and
without `$env:CLIPDUMP_PREWARM=1`; if the first is slow and the knob fixes it without stealing
focus, make it the default.

### Search in Rust over 1,000 items

400 searches per query, perf profile, a checker that says every file exists, 1,000 items as in the
cold start test (`search_timing_1000_items`; limit 50 rows):

| Query | p50 ms | p95 ms | max ms |
|---|---:|---:|---:|
| empty | 0.012 | 0.017 | 0.148 |
| `m` | 0.153 | 0.195 | 0.664 |
| `me` | 0.239 | 0.290 | 0.506 |
| `mee` | 0.483 | 0.682 | 0.963 |
| `meeting` | 0.234 | 0.364 | 0.482 |
| `budget` | 0.163 | 0.247 | 0.320 |
| `Budget` (smart case) | 0.179 | 0.278 | 0.468 |
| `snap` (OCR token) | 0.344 | 0.623 | 2.857 |
| `restartocr` (OCR word) | 0.258 | 0.433 | 0.533 |
| `invoice total` | 0.295 | 0.407 | 0.532 |
| `https` | 0.197 | 0.273 | 0.844 |
| `fn ` | 0.308 | 0.372 | 2.884 |
| `img:` | 0.010 | 0.010 | 0.041 |
| `file:` | 0.020 | 0.022 | 0.057 |
| no match (`zzzzqq`) | 0.255 | 0.292 | 0.478 |
| empty, real `stat` of file rows, cache empty | 0.297 | 0.509 | 0.589 |

The Phase 8 criterion bench (`cargo bench`, `bench` profile) is the Windows procedure; it could not be
re-run here with the `perf` profile (it inherits `panic = abort`, which the bench harness cannot use)
and the `bench` profile would need another 1 GB of disk. The older Phase 8 table in
`manual-checks.md` stands as the Linux reference.

### 4K image ingest and the optimizations it led to

Stage times for one 3840x2160 picture on an idle machine, perf profile, medians of 7 pictures (paired
comparisons come from the same run):

| Step | Before | After |
|---|---:|---:|
| Decode `CF_DIBV5` (BMP wrapper and the `image` crate, one extra 33 MB copy) | 100 to 127 ms | **13 ms** (direct BGRA to RGBA) |
| Decode 24 bit `CF_DIB` | 90 to 104 ms | **13 ms** |
| Decode PNG (the stored original; unchanged) | 41 to 52 ms | |
| Hash (xxh3 of the pixels) | 6 ms | 6 ms |
| Thumbnail (`imageops::thumbnail`, then box filter) | 78 ms | **16 ms** |
| RGBA to RGB repack before the PNG encode | 44 ms | **18 ms** |
| PNG filter and deflate (`CompressionType::Fast`, Sub) | about 135 ms | about 135 ms (unchanged) |
| `image::build`, PNG on the clipboard | 145 ms | **73 ms** |
| `image::build`, `CF_DIBV5` | 409 ms | **219 ms** |
| `image::build`, `CF_DIB` 24 bit | 399 ms | **209 ms** |
| Build `CF_DIBV5` bytes for a copy (`fill_dibv5`) | 59 ms | **12 ms** |
| Ctrl+Enter, CPU and disk parts for a 4K screenshot | median 130 ms, max 163 | **median 93, max 112** |

The commits (each carries the before and after numbers in its message):
`3756cc4` (RGB repack), `5d1198c` (direct DIB decode, box thumbnail, PNG encode on a second thread
beside the hash and thumbnail for pictures over 2 MB), `b10af2e` (`CF_DIBV5` build). The new DIB
decode takes only 32 bit BGRA with the standard masks and 24 bit BGR; everything else still goes
through the `image` crate, and a test compares both routes on every DIB variant, both row orders
and odd widths. What was tried and left: `FilterType::NoFilter` (not measured, expected to hurt
fdeflate's ratio and speed), a parallel deflate (needs hand made zlib stream joining), threaded
`nucleo` (search is 0.7 ms p95, not needed), a smaller result limit (not needed).

### Cold start, restart and copy

- Cold start and the app's `--perf` startup lines: see the table above. `Store::open` decrypts in
  parallel on a short lived rayon pool; nothing was changed.
- The restart test is part of `cargo test`. The kill variant copies the data folder while the store
  is running (so the WAL is un-checkpointed, exactly what a power cut leaves) and opens the copy.
  Persist-level kill tests from Phase 13 cover torn WAL tails.
- The copy figures exclude `OpenClipboard` and `SetClipboardData`; the Windows `--perf` events
  `copy_text`, `copy_image`, `copy_files` and `copy_payload` measure the whole command.

## Findings and risks (for the Windows run to confirm or dismiss)

1. **First show of a never shown window** is 15 times slower than later shows on WebKitGTK under
   Xvfb. Unknown for WebView2. Experiment knob: `CLIPDUMP_PREWARM=1`.
2. **Search `stat` cost.** `search` checks whether the files of file items exist for up to 50 rows,
   128 paths each, cached for 2 seconds, on the search path (not on the keystroke path once cached).
   Free on Linux for nonexistent paths (0.3 ms for the empty query), but a Windows `stat` through
   Defender or over a network share can be milliseconds each. If `search_core` p95 is over 1 ms with
   many file items, check this first (move the check off the search thread, or limit it to the rows
   actually visible).
3. **OCR fills the blob cache.** The OCR worker reads each screenshot through `Store::get_blob`, which
   puts the PNG in the 64 MB LRU, so after a few screenshots the cache sits at its budget and stays
   there, which counts against the idle RAM budget. It also makes a Ctrl+Enter on a fresh screenshot a
   cache hit, which is useful. Choose after the Windows idle RAM number: either keep, or read without
   caching in the OCR path, or lower the default budget.
4. **Search while ingest saturates the CPUs** has p95 2 to 3 ms and rare spikes (22 ms once). Ingest
   threads are below normal priority on Windows (not in this container); if keystroke feel suffers
   during a burst of screenshots on Windows, that is where to look.
5. **Raw captures wait in a 64 slot channel** (`INGEST_QUEUE`) between the watcher and the ingest
   workers. A script that copies 4K pictures faster than the ingest keeps up with (about 11 per
   second here) could queue up to 64 raw pictures (about 2 GB). A person cannot; the watcher drops instead of blocking when it is full.
   Not changed; a byte budget on that channel would be the fix if it ever matters.
6. **Allocator retention.** On glibc about 150 MB of freed memory stays resident after a heavy
   ingest; the Windows heap returns large blocks at once, so this may not exist there.
7. **Image ingest under full load** reaches p95 255 ms and a maximum of 351 ms on four saturated
   vCPUs; idle it is 73 to 219 ms. On Windows the DIB path is the slow one; if `ingest_image` p95
   is over 300 ms there, the only remaining big cost is the single threaded PNG deflate (about
   135 ms), which could be moved after the item is visible.

## How to produce the Windows numbers

On the Windows machine (MSVC Rust toolchain, Node 22, pnpm), from the repo root. One command each:

| Row | Command or step |
|---|---|
| Everything that `--perf` measures (hotkey, search, ingest, OCR, copy, startup) | `pnpm tauri build`, then `src-tauri\target\release\clipdump.exe --perf`; use it following `docs/manual-checks.md`, Phase 14; quit from the tray; read `%LOCALAPPDATA%\clipdump\perf-summary.md` (or `clipdump.exe --perf-summary "%LOCALAPPDATA%\clipdump\perf.log" > summary.md`) |
| First show against later shows | the same session, first and median `hotkey_to_painted`; then again with `$env:CLIPDUMP_PREWARM=1; .\clipdump.exe --perf` |
| Keystroke to rendered rows | DevTools Performance recording while typing, `manual-checks.md` Phase 10 |
| Search in Rust | `cd src-tauri; cargo bench --bench search_bench` |
| Stress, three scenarios (retention, RAM, disk cap, responsiveness, restart of the store) | `node scripts/stress.mjs` (about 10 minutes, about 3 GB of free disk; add `--dir D:\scratch` to choose the disk) |
| Cold start | restart the PC, then `clipdump.exe --perf` once; `startup_load`. History built with `$env:CLIPDUMP_RESTART_KEEP_DIR="C:\clip-cold"; node scripts/stress.mjs cold` |
| Restart persistence | the reboot procedure in `manual-checks.md`, Phase 14, "Restart test" |
| Idle RAM and CPU | Task Manager and the PowerShell line in `manual-checks.md`, Phase 14 |
| Installer size | Phase 15 |

Then fill the Windows column of the table above (with the date, CPU and Windows version), tick the
boxes in `docs/manual-checks.md`, and only when every row is under its target tick the Section 7
checkbox in `PLAN.md`.

## What was not measured, and why

OCR speed, clipboard write time, WebView2 anything, DPAPI, Defender, installer size, idle RAM and CPU of
the real stack, the real first show, a real reboot, 30 day expiry on a real history. All need Windows.
The criterion search bench was not re-run (see above). Nothing was run with the shipping fat LTO
profile.
