# Manual checks (Windows)

Steps that cannot run in the Linux development container. Run them on Windows
with `pnpm tauri dev` from a terminal so the log lines are visible. Tick each box
only after seeing the expected result.

## Phase 4: Clipboard capture

Until Phase 5 lands, a captured item is only logged by a placeholder consumer.
Each capture prints one line to the terminal, for example
`clipdump: captured text, 12 bytes, from notepad.exe`. The log never contains
clipboard content. Skips print a short reason, or nothing when paused.

### Text
- [ ] Copy text in Notepad: one line `captured text, N bytes, from notepad.exe`.
- [ ] Copy text in Chrome (address bar and page text): one line each, source `chrome.exe`.
- [ ] Copy text in VS Code: one line, source `Code.exe`.
- [ ] Copy once: exactly one line per copy (no duplicate lines for a single copy).
- [ ] Copy a very large text (over 5 MB, for example a big log file): a line
      saying the text is over the limit, and no capture line.

### Images
- [ ] Win+Shift+S screenshot: one `captured image ...` line (format `Png` or `Dibv5`/`Dib`).
- [ ] Copy a region in Paint: one image line, source `mspaint.exe`.
- [ ] Chrome "Copy image" on a web image: one image line, source `chrome.exe`.
- [ ] Copy cells in Excel: a `captured text ...` line (text wins over the bitmap
      Excel also offers), not an image line.
- [ ] The log line shows the byte size of the raw clipboard data (a 4K screenshot
      is several MB); nothing is decoded or written to disk at this stage.

### Files
- [ ] Select one file in Explorer and Ctrl+C: `captured files, 1 paths`.
- [ ] Select several files and a folder: `captured files, N paths` with the right N.
- [ ] Copy files with non-ASCII names: still one line with the right count.

### Privacy and skip rules
- [ ] Copy a password from a password manager that sets the exclusion formats
      (Bitwarden, 1Password, KeePass, KeePassXC): no capture line, and a line
      saying the copy was marked private.
- [ ] Windows clipboard history (Win+V) does not record that password either
      (confirms the same flag is set; skip if your manager does not set it).
- [ ] Tray, Pause capture: copy text, nothing is logged. Resume: copies are
      logged again.
- [ ] Add the executable name of a test app to the ignore list (no settings UI
      yet, so temporarily put it in `Settings::default().ignored_apps` in
      `src-tauri/src/settings.rs`): copies from that app print "ignored app" and
      are not captured, copies from other apps still are.

### Behavior and cost
- [ ] Idle: Task Manager shows about 0% CPU for the app while nothing is copied
      (the watcher sleeps in `GetMessageW`, there is no polling).
- [ ] Hammer test: copy ten different short texts in a row as fast as you can;
      each produces a capture line and the app stays responsive.
- [ ] Hold the clipboard busy from another tool while copying: the update is
      either captured after a short retry or logged as "clipboard busy"; never a
      hang.
- [ ] Quit from the tray: the process exits (the watcher thread shuts down) and
      does not stay in Task Manager.

### Not testable yet (arrive with later phases)
- Self-write suppression (500 ms text echo, 150 ms image echo, bump to top):
  checked in the Phase 11 section below.
- Source app name stored with the item (Phase 5).

## Phase 5: Ingest pipeline

Phase 5 replaces the logging placeholder with the real ingest pool. A temporary
sink (`ingest/sink.rs`, replaced by the store in Phase 6) writes every item to
the encrypted history database in `%LOCALAPPDATA%\clipdump\`. There is no UI yet,
so the checks use the terminal log (`pnpm tauri dev`) and the files on disk.
Each ingested item prints one line, for example
`clipdump: ingested image 3840x2160, 2113456 bytes png, from mspaint.exe in 140 ms`.
The log never contains clipboard content.

### End to end capture into the database
- [ ] Start the app, copy text in Notepad: one `ingested text, N bytes, from notepad.exe in N ms` line.
- [ ] `%LOCALAPPDATA%\clipdump\history.db` and `history.db-wal` exist after the first copy;
      open `history.db` in a hex or text viewer and confirm no readable copied text.
- [ ] Copy the same text again, and the same text from another app: no new item
      (the row count does not grow; the item is moved to the top). Check by
      counting rows with a SQLite viewer (`SELECT count(*) FROM items`), the
      columns are encrypted but the count and `use_count` are readable.
- [ ] Copy text with multiple lines from Notepad (CRLF) and the same text from
      VS Code (LF): one item, not two.
- [ ] Copy a Win+Shift+S screenshot: one `ingested image WxH` line, a new file
      under `blobs\xx\` and `thumbs\xx\` (both unreadable as images, they are
      encrypted), and `ocr_status = 0` (pending) in the row.
- [ ] Copy the same picture from Paint (DIB) and from Chrome "Copy image" (PNG):
      they dedup to one item (same pixels).
- [ ] Copy a screenshot that has transparency (a PNG with alpha opened in a
      browser, "Copy image"): ingested without error.
- [ ] Copy files and a folder in Explorer: one `ingested files, N paths` line.
      Copy one image file: its thumbnail file appears under `thumbs\`.
- [ ] Copy files, then copy the same files in a different selection order:
      still one item.
- [ ] Restart the app, copy something that was copied before the restart: it
      does not create a second row (dedup survives restart).
- [ ] The source app (`notepad.exe`, `chrome.exe`) shows in the log line and in
      the `source_app` column.

### Timings (Section 4: clipboard event to item visible < 300 ms excluding OCR)
- [ ] 4K screenshot (Win+Shift+S on a 4K monitor, full screen): the `in N ms`
      value of the log line is under 300 ms in a release build
      (`pnpm tauri build` then run the exe from a terminal). Record the value.
      The Linux figure from the development container is not a substitute.
- [ ] Same for a 1080p screenshot and for a 5 MB text.
- [ ] While a 4K screenshot is being processed, typing in another window and the
      hotkey stay responsive (workers run at below normal priority).
- [ ] In Process Explorer or Task Manager details, the `ingest-0` and `ingest-1`
      threads show priority "Below Normal" (Process Explorer, thread tab).

### Edge cases
- [ ] A text over 5 MB and an image over 50 MB are still skipped by the watcher.
- [ ] Copy an image from an app that offers only `CF_DIB` (older tools, for
      example MS Paint from Windows 7 era) and from an app with a 32 bit
      `CF_DIBV5` with an alpha channel: the picture is right way up and not
      transparent or inverted (open the item in the Phase 10 UI when available;
      until then, compare `width`/`height` in the row with the source picture).
- [ ] Copy 20 different short texts in a row very quickly: 20 rows, in copy order
      (`created_at` increasing).

### Not testable yet (arrive with later phases)
- `history:changed` reaching the UI (Phase 9/10 listens for it); until then it is
  emitted but nobody receives it.
- Queued OCR (Phase 7): image rows stay `ocr_status = 0`.
- Retention, in-memory store, search (Phases 6 and 8).

## Phase 6: In-memory store, retention, cache

Phase 6 replaces the temporary Phase 5 sink with the real store: items live in
RAM (metadata, search text, thumbnails), every change is queued to the encrypted
database, retention runs after every insert and every 15 minutes, and big
payloads go through a 64 MB byte budgeted cache. There is still no search or UI
(Phases 8 to 10), so the checks use the terminal log from `pnpm tauri dev` and the
files in `%LOCALAPPDATA%\clipdump\`. On start the log shows
`clipdump: loaded N items in N ms`. The Phase 5 `ingested ...` lines are
unchanged.

### Restart persistence with real data
- [ ] Copy about 20 mixed items (several texts, a multi line text, two
      screenshots, a file list, a folder). Quit from the tray, start the app
      again: the start line says `loaded 20 items` (the same count as before the
      restart, minus nothing).
- [ ] Copy one of the same texts again after the restart: the item count in the
      next restart does not grow (dedup survives the restart; check with a
      SQLite viewer: `SELECT count(*) FROM items`, and `use_count` of that row
      went up by one).
- [ ] Copy the same screenshot from Paint (DIB) and from Chrome "Copy image"
      (PNG): one row only, `use_count` 1.
- [ ] Copy 5 new items after a restart: their `id` values continue after the
      highest id from before the restart (ids are not reused).
- [ ] Kill the process hard (Task Manager, End task) right after a copy and
      start again: at worst the last copy (within about 50 ms) is missing, the
      rest is intact and the app starts normally.
- [ ] Copy a text over 64 KB (for example a 200 KB log), restart: the item is
      still there (its full text is read from the database on demand, Phase 9
      shows it in the preview).

### Cold start timing (target: under 300 ms for 1,000 items)
- [ ] Build a 1,000 item history (a script that copies 1,000 distinct short texts
      works, or copy a few hundred and let Phase 14's stress script fill the
      rest), quit, then start a release build (`pnpm tauri build`, run the exe
      from a terminal). Record the `loaded 1000 items in N ms` line. It must be
      under 300 ms; run it three times, the first run is cold (file cache) and
      the later ones warm. The Linux numbers in the Phase 6 report are not a
      substitute: Windows has NTFS, Defender and slower file opens, and the
      thumbnail files are read one by one at startup.
- [ ] Same with a history that includes 300 screenshots (so 300 thumbnail files
      are read at start). If this is over budget, the thumbnail reads are the
      first suspect (note the number; the fix is packing thumbnails into SQLite).
- [ ] Windows Defender real time protection on and off: note whether the cold
      start differs much.

### Retention (1,000 items, 30 days, disk cap)
- [ ] With the default settings copy more than 1,000 distinct short texts (a
      small script that sets the clipboard in a loop works). The row count in
      `history.db` stays at 1,000 and the oldest ones are gone.
- [ ] Pin three items (no pin UI before Phase 10: set `pinned = 1` on three rows
      with a SQLite tool while the app is closed), start the app, copy 1,100
      more items: the three pinned rows are still there and the unpinned count is
      1,000.
- [ ] Age: while the app is closed, edit `created_at` of a few unpinned rows to a
      date 31 days ago (`created_at` is unix milliseconds) and set one pinned
      row the same way. Start the app: the unpinned old rows are deleted at
      start, the pinned one stays, and their files under `blobs\` and `thumbs\`
      are gone.
- [ ] Leave the app running across a clock change: set the system date 31 days
      ahead, wait up to 15 minutes (or restart), the old unpinned items are
      evicted. Set the date back afterwards.
- [ ] Disk cap: temporarily put `disk_cap_bytes` to 50 MB in
      `settings.json` (or the "Disk cap" field in Settings), copy ten 4K
      screenshots (several MB each): the oldest screenshot rows and their
      `blobs\` and `thumbs\` files disappear until the folder is about 50 MB or
      less. SQLite and WAL overhead is not counted, so allow a few MB extra.
- [ ] Deleting files: after any eviction, confirm the matching `blobs\xx\*.bin`
      and `thumbs\xx\*.bin` files were removed (no growth of the folders).

### RAM behavior with many large screenshots
- [ ] Copy 30 different 4K screenshots (each is a lossless PNG of several MB).
      In Task Manager the process memory must not grow by 30 times the PNG size:
      blobs are written to disk and are not kept in RAM after ingest. Expect
      growth of roughly 10 KB per item for the thumbnail plus the transient
      decode buffers while a screenshot is being processed.
- [ ] Phase 9 and 11 will exercise the blob cache (copying an image back). When
      they land, copy back 100 different screenshots in a row: the working set
      stays within about 64 MB above the idle level (the cache budget) instead of
      growing with the number of screenshots.
- [ ] Idle CPU stays near 0% with the app idle for a minute (the retention
      thread sleeps 15 minutes at a time and the writer blocks on its channel).
- [ ] Quit from the tray: the process exits and does not stay in Task Manager
      (the retention thread stops when the app state is dropped).

### Not testable yet
- Search, OCR results and the `missing` badge for moved or deleted files (Phases
  7 to 9).
- Cache hits while copying an item back (Phase 11 uses `Store::get_blob` and
  `get_full_text`; the cache and the byte budget are covered by unit tests).

## Phase 7: OCR

Phase 7 adds the OCR worker. Every new image item is queued, one background
thread (lowest priority) loads its PNG, runs `Windows.Media.Ocr` and stores the
text: in RAM as the item's search text, encrypted in `items.ocr_enc` with
`ocr_status = 1`. There is no search UI before Phases 8 to 10, so these checks
use the terminal log from `pnpm tauri dev` and the database. The log shows, per
image, `clipdump: ocr: item <id>: <n> chars in <ms> ms` (the text itself is
never logged). At start it can also show `clipdump: ocr: queued <n> images from
the history`, or `clipdump: ocr: no OCR language installed, ...`. The OCR text is
encrypted in the database, so a SQLite viewer shows only `ocr_status` and the
length of `ocr_enc`, not the words.

Statuses in `items.ocr_status`: 0 pending, 1 done, 2 failed, 3 unavailable.

### The plan's check: a screenshot is found by its words
- [ ] Open `PLAN.md` in an editor with a large font, take a screenshot of a
      paragraph with Win+Shift+S. The log shows an `ingested image ...` line
      and then an `ocr: item N: <n> chars in <ms> ms` line, with `n` roughly the
      number of characters you can see in the shot (not 0). In the database that
      row has `ocr_status = 1` and a `length(ocr_enc)` above 40.
- [ ] Search for a word from the screenshot and see it in the results: this part
      cannot be done before the search engine, IPC and UI land (Phases 8 to 10).
      Until then the log line and the database row above are the check. When
      Phase 10 lands, repeat with a word that appears only in the picture: the
      screenshot must appear in the results with the "OCR" badge.
- [ ] Take a screenshot of a blank wall or a plain color: the log shows
      `0 chars` and the row still ends with `ocr_status = 1` (an empty result is
      a finished OCR, not a failure).
- [ ] Take a very small crop (one word, under 40 pixels tall): it is recognized
      (the worker enlarges small pictures 2x first). Check the log shows more
      than 0 chars.
- [ ] Copy a screenshot with a transparent background (a PNG with alpha, for
      example from a design tool or "Copy image" on a transparent logo with dark
      text): the text is still recognized (transparency is flattened onto
      white before OCR).

### Speed (budget: under 1 s for a 1080p screenshot)
- [ ] Take three full 1080p screenshots of text heavy pages (an article, code).
      Note the `<ms>` of each `ocr: item` line. Each must be under 1000 ms on a
      release build (`pnpm tauri build`, run the exe from a terminal). A debug
      build is slower and does not count. Record the numbers in
      `docs/perf.md` later (Phase 14).
- [ ] Do the same with a 4K screenshot: it is not part of the budget, but note
      the time. A picture over `MaxImageDimension` (10,000 px on a side, for
      example a long stitched page capture) is shrunk, not rejected.
- [ ] While OCR runs (copy ten large screenshots in a row), type in Notepad and
      press the hotkey: both stay responsive, because the OCR thread runs at idle
      priority. In Task Manager the process CPU shows a short burst, then ~0%.
- [ ] The Phase 5 timing line for the same screenshot (`ingested image ... in N
      ms`) is unchanged by OCR: OCR is not part of that number.

### Restart and re-queue
- [ ] Copy 10 screenshots quickly, then quit from the tray at once (before the
      log shows all ten `ocr: item` lines). Start the app again: the log shows
      `queued <n> images from the history` and the remaining ones finish,
      newest first. No row stays at `ocr_status = 0` afterwards.
- [ ] After a normal restart the OCR text of old screenshots is still there (the
      row keeps `ocr_status = 1`, and no `ocr: item` line is printed for it).
- [ ] Delete a screenshot right after copying it (no UI before Phase 10: remove
      the row while the app is closed). No crash or error line at the next
      start, and the file is not recreated.

### No OCR language installed
- [ ] Remove every OCR capable language, or use a Windows install without one
      (Settings > Time & language > Language & region; English (United States)
      normally has OCR, a language added "without optional features" may not).
      Start the app: the log shows `ocr: no OCR language installed`. Copy a
      screenshot: it is stored normally, the row gets `ocr_status = 3`, and no
      `ocr: item` line is printed.
- [ ] Add a language with the OCR optional feature
      (Settings > Time & language > Language & region > Add a language), restart
      Clipdump: the log shows `queued <n> images from the history` and the
      screenshots that were `3` become `1`.
- [ ] The hint text for the settings screen is the constant
      `ocr::OCR_UNAVAILABLE_HINT`; the settings UI that shows it arrives in
      Phase 10. The count of affected items is `StoreStats::ocr_unavailable`
      (`get_stats`, Phase 9).

### The OCR setting
- [ ] Settings, Capture, "Recognize text in images (OCR)" off, then copy a
      screenshot: no `ocr: item` line, the row stays `ocr_status = 0`. Switch it
      back on: the pending screenshots are processed at once (live, through
      `OcrWorker::apply_settings`). It survives a restart.

### Not testable yet
- Searching OCR text, the "OCR" badge and the snippet (Phases 8 to 10).
- The language status and hint in settings (Phase 10).

## Phase 8: Search engine

Phase 8 adds the search code (`src-tauri/src/search.rs` and `search/`), the
`Store::search` entry point and a Criterion benchmark. There is no IPC command or
UI before Phases 9 and 10, so nothing is visible in `pnpm tauri dev` yet. The
Linux figures from the development container are relative only; the numbers that
matter are the ones below on Windows.

### The 1 ms budget (Section 4: search itself in Rust, 1,000 items)
- [ ] From `src-tauri` in a normal terminal run `cargo bench --bench search_bench`.
      It uses the `bench` profile from `Cargo.toml` (optimization level 3, no fat
      LTO, so it builds in about two minutes instead of recompiling all of Tauri
      with LTO). Read the `search_1000/...` lines. The line that counts is
      `search_1000/full/<query>` (ranking plus building the result rows); the
      `rank` lines are the scan alone. Every query must show a median under
      1 ms. Record the table for `docs/perf.md` (Phase 14). The Linux reference
      from the container, medians in microseconds for 1,000 items: empty 5,
      1 char 106, common word 371, rare word 198, two words 423, fuzzy code 360,
      typo 177, smart case 102, url 162, unicode 120, `img:` 68, no match 83.
- [ ] Look at `search_10000/...` too. It is not part of the budget (retention caps
      the history at 1,000 unpinned items) but shows how it scales: expect roughly
      10 times the 1,000 item numbers.
- [ ] Repeat once with Windows Defender real time protection on (the benchmark
      reads no files, so there should be no difference; this just rules it out).
- [ ] If any `search_1000/full` median is over 1 ms on Windows, the knobs are in
      `search.rs`: `FUZZY_MAX_BYTES` (lower it to make fewer texts take the
      scattered letter fuzzy match) and `MAX_OCCURRENCES`. Report the table.

### Missing files check (the existence test of file results)
The result rows carry `missingFiles`, computed with a real `stat` on the file
paths of the returned file items only (cached for 2 seconds). It is wired to the
real file system by Phase 9; until then only unit tests with a fake file system
cover it. When Phase 10 shows the badge:
- [ ] Copy two files in Explorer, find them with `file:`, rename one in Explorer,
      wait 2 seconds, type one more character: the badge appears for the renamed
      one without restarting.
- [ ] Copy a file from a USB drive or a network share, then disconnect it: the
      badge appears, and typing does not freeze. A disconnected network share can
      make a Windows `stat` block for several seconds; if typing stalls here,
      note it. The mitigation would be to move the check to a background thread.
- [ ] Copy a folder tree with several thousand files: search still answers
      instantly (only the first 128 paths of an item are checked, so the missing
      count of a huge list is a lower bound).

### Behaviors to confirm once the UI exists (Phase 10)
- [ ] `Alt+1` to `Alt+4` filters and typed `img:`, `file:`, `txt:` prefixes agree
      (a typed prefix wins over the toggle).
- [ ] A word that appears only inside a screenshot (OCR) finds it with a subtitle
      snippet around the word and the word highlighted in the snippet.
- [ ] Highlights are right for text with accents, CJK and emoji (they count
      characters, not bytes).
- [ ] Text longer than 256 bytes is found by any word it contains, but not by
      scattered letters or by an unaccented spelling of an accented word (for
      example `cafe` finds a short note spelled with an accent (cafe with an acute e), not a 5 KB text that only
      has that word somewhere in it). This is deliberate (see `FUZZY_MAX_BYTES`).

### Not testable here, and when it is
- Keystroke to results rendered under 16 ms (Section 4) needs the UI and IPC. It
  is verified in Phase 14 (`--perf` log lines and the stress script), not before.
- Search is only ever called from the command thread; nothing in Phase 8 runs on
  the clipboard watcher or ingest threads.

## Phase 9: IPC layer

Phase 9 adds the commands (`src-tauri/src/commands.rs`), the `clip` URI scheme
(`scheme.rs`), the per command permissions (`build.rs` and
`capabilities/default.json`) and the typed frontend bridge (`src/lib/ipc.ts`).
There is still no search UI (Phase 10), so these checks use the WebView2 developer
tools of `pnpm tauri dev` (right click in the bar, Inspect, Console).

What was already verified in the Linux container: unit tests for every command
function and the scheme, and an Xvfb run of the debug binary in which a page
calls the commands, loads `clip://localhost/thumb/<id>` and `/blob/<id>` as
images and with `fetch`, and is refused for commands outside the capability.

### The `clip` URL form in WebView2
- [ ] Tauri serves the scheme on Windows as `http://clip.localhost/...` (on
      macOS and Linux it is `clip://localhost/...`). Copy a screenshot, open the
      bar, and in the console run:
      `await __TAURI_INTERNALS__.invoke("search", { query: "", filter: "images" })`
      and take the first `id`.
- [ ] `fetch("http://clip.localhost/thumb/<id>").then(r => [r.status, r.headers.get("content-type"), r.headers.get("cache-control")])`
      gives `200`, `image/webp`, `max-age=31536000, immutable`.
- [ ] Same for `http://clip.localhost/blob/<id>`: `200`, `image/png`.
- [ ] `fetch("http://clip.localhost/thumb/999999")` gives `404` with
      `cache-control: no-store`. A text item id gives `404` for `thumb` and `blob`.
- [ ] Paste this in the console and see the picture appear in the top left corner:
      `document.body.append(Object.assign(new Image(), { src: "http://clip.localhost/blob/<id>", style: "position:fixed;top:0;left:0;max-width:300px;z-index:9" }))`.
      The ipc.ts helpers build the same URLs (`thumbUrl(id, createdAt)` and
      `blobUrl(id, createdAt)`; the `?v=` query is ignored by the backend and
      keeps a reused id from showing an old cached picture).
- [ ] If the images do not load but `fetch` works, check that the page and the
      scheme are not blocked by a CSP (the config sets `csp: null`).
- [ ] The Network tab shows the second load of the same thumbnail served from the
      memory or disk cache (no new request).
- [ ] Opening a large screenshot (4K) with `blob/<id>` the first time takes about
      as long as decrypting and reading the file; the bar stays responsive meanwhile
      (the read runs on a worker thread).

### Capabilities
- [ ] In the console, `__TAURI_INTERNALS__.invoke("search", { query: "" })`
      works, and so does `invoke("plugin:event|listen", ...)` through the app's
      own `listen` calls (the bar still reacts to the hotkey).
- [ ] Commands outside the capability are refused. Each of these rejects with
      "... not allowed": `invoke("plugin:autostart|enable")`,
      `invoke("plugin:window|hide")`,
      `invoke("plugin:global-shortcut|unregister_all")`, `invoke("nope")`.
- [ ] The tray, the hotkey, autostart and a second launch (single instance) still
      work: they are driven from Rust and need no frontend permission.

### Commands on Windows
- [ ] `copy_item` works since Phase 11: `invoke("copy_item", { id, paste: false })`
      resolves, the item is on the clipboard and moves to the top (checked in
      the Phase 11 section).
- [ ] `get_preview` of a text copied from a big file (a few MB) returns at most
      about 200 KB of `text` with `truncated: true` and the real `totalChars`.
- [ ] `get_preview` of a screenshot shows `width`, `height`, `ocrStatus` and
      `ocrText` (after OCR finished). With no OCR language: `ocrStatus` is
      `"unavailable"` and `get_stats` has `ocrEngine: "unavailable"`.
- [ ] `get_preview` of copied files lists names and sizes; rename one in Explorer
      and call it again: that entry has `exists: false`.
- [ ] `toggle_pin`, then `clear_history`: the pinned item survives and
      `clear_history` resolves to the number removed. `delete_item` on the pinned
      one removes it, and its files under `blobs\` and `thumbs\` disappear
      shortly after.
- [ ] `get_stats` shows `keyProtector: "dpapi"`, a plausible `diskBytes` and
      `cacheBytes`, and `ocrHint` text.
- [ ] `set_settings` with `maxItems: 3`, then `search("")`: only three unpinned
      items remain (retention ran at once). `pinnedBar: true` keeps the bar
      visible when it loses focus. Since Phase 12 these are saved to
      `settings.json` (see the Phase 12 section) and survive a restart.

## Phase 10: UI (shadcn look)

Phase 10 adds the whole search UI (`src/App.tsx`, `src/components`, `src/hooks`)
and one small backend addition: result rows now carry `lineCount` (the "n lines"
badge). What was verified in the Linux container: type check, production build,
vitest (highlight splitting with non-ASCII, relative time, bytes, shortcut table,
request-counter staleness), a Playwright keyboard flow against a fake backend
(`node scripts/ui-check.mjs keys`, 57 checks), light and dark screenshots
(`docs/screenshots/`, drawn by Chromium), a relative keystroke timing, and the
real Tauri app under Xvfb (WebKitGTK) with a seeded history: second launch and
the global hotkey show the bar, typing finds an OCR match with the highlighted
snippet, emoji highlights land on the right characters, `clip://` thumbnails
load, Tab shows the preview, `Ctrl+,` opens settings, `Esc` hides the bar and
`Ctrl+Enter` shows the backend error as an inline message. WebKitGTK is not
WebView2, so everything below still needs a Windows run.

### Rendering in WebView2
- [ ] `pnpm tauri dev`, press the hotkey: a rounded card with a soft shadow, no
      black or white corners, no rectangular frame around it (window transparency
      works), on both a light and a dark wallpaper.
- [ ] `backdrop-blur` only blurs what is behind the card inside the webview, so
      over the desktop the card is simply 95% opaque. Check it still reads well
      over a busy wallpaper and a white window behind it.
- [ ] The native shadow (`shadow: true`) and the CSS shadow do not double up into
      a hard dark halo. If they do, set `shadow` to false in `tauri.conf.json`.
- [ ] DPI 100, 125, 150 and 200 percent: text is crisp, the 8 px margin around
      the card is kept, about eight text rows fit, thumbnails are sharp. Move the
      bar between two monitors of different scale and open it on each.
- [ ] Geist loads from the bundle (no network): disconnect the network and
      reload, the font is unchanged.

### Focus and show
- [ ] Hotkey: the input has the caret at once, with no click. Type a query, `Esc`,
      press the hotkey again: the old query is selected and typing replaces it.
      Repeat 20 times quickly; focus must never be lost.
- [ ] Tray "Settings" shows the bar with the settings view; `Esc` goes back to
      search, `Esc` again hides the bar. After the bar hides while settings or
      the preview was open, the next open starts on the plain search view.
- [ ] Click outside the bar: it hides (unless "Pinned bar" is on, then it stays
      and the hotkey only focuses it).
- [ ] Clicking a filter, a hint or an empty spot in the card does not take the
      caret out of the input.

### Thumbnails and images through `clip.localhost`
- [ ] Copy a few screenshots: rows show a 40 px thumbnail, the Tab preview shows
      the full picture over the thumbnail, no broken image icons. DevTools Network:
      the URLs are `http://clip.localhost/thumb/<id>?v=<createdAt>` and
      `.../blob/<id>?v=...` and a second view comes from the cache.
- [ ] A file item that is an image (copy a PNG in Explorer) shows its thumbnail.
- [ ] Delete the newest screenshot, restart, copy another one: the new row must
      not show the old picture (the `?v=` cache buster).

### Search rows
- [ ] A word that is only inside a screenshot finds it: "OCR" badge, a subtitle
      with the word highlighted, the preview pane shows the OCR text.
- [ ] Highlights are right for text with accents, CJK and emoji (copy
      `caf` plus an e acute, a Japanese sentence and a rocket emoji, search a word
      after each).
- [ ] A multi-line copy shows "n lines" (a code block, an email); a single line
      does not.
- [ ] A very long single line with the match far to the right: the row starts
      near the match with a leading ellipsis, the match is visible.
- [ ] Rename a copied file in Explorer, retype one character: the row gets the
      red "missing" badge (this is the Phase 8 missing-files check, now visible).
- [ ] The first nine rows show `Ctrl 1` to `Ctrl 9`; with the preview open the
      hints disappear (narrow list), nothing overflows.
- [ ] Pinned items are on top with an empty query and show the pin icon.

### Keyboard (inside the bar)
- [ ] `Ctrl+Enter` and `Enter` both copy and hide the bar; `Enter` also pastes
      once "Paste on Enter" is on (Phase 11 section). A copy that fails shows
      "Could not copy: ..." in the footer, the bar stays open and keeps working.
- [ ] `Ctrl+1` to `Ctrl+9`, `Ctrl+P`, `Ctrl+Delete`, `Shift+Delete`: act on the
      right row. After a delete the next row is selected.
- [ ] `Ctrl+Shift+Delete`: the footer asks to press again (with the count of
      unpinned items), expires after 3 seconds, `Esc` cancels it, a second press
      inside the window clears and keeps pinned items.
- [ ] `Alt+1` to `Alt+4` switch the filter. Windows must not move focus to a menu
      or beep on Alt+digit. A typed `img:`, `file:`, `txt:` prefix wins.
- [ ] `Tab` toggles the preview and never moves focus out of the input.
- [ ] `Ctrl+,` toggles settings. `Esc` order: confirmation, settings, preview, hide.
- [ ] Global conflicts: the global `Ctrl+Shift+Space` still works while the bar
      is focused (it toggles the bar); no other program that registers `Alt+digit`
      or `Ctrl+,` globally steals them (PowerToys, AutoHotkey, IDE overlays).
- [ ] WebView2 built in accelerator keys: `Ctrl+P` must not open the print
      dialog, `F5` or `Ctrl+R` do not reload the page. If one does, set
      `AreBrowserAcceleratorKeysEnabled` to false (Rust side) and note it.
- [ ] Holding `Ctrl+Delete` deletes one item per press, not a page of them.

### IME
- [ ] With a Japanese or Chinese IME: composing in the search box shows the
      candidate window at the caret, results follow the composed text, `Enter`
      that confirms a candidate does NOT copy the selected row, and the shortcuts
      work again right after the composition ends.

### Light, dark and theme
- [ ] Settings, Theme: System follows Windows (switch Windows light/dark while
      the bar is open and while it is hidden), Light and Dark force a palette
      and the choice is still there after a restart (stored in the webview's
      localStorage on purpose: it is needed before the first paint, so it is not in
      settings.json).
- [ ] Dark mode: selected row, "missing" badge, highlights and Kbd chips have
      enough contrast.

### Settings
- [ ] Every control applies at once. Lower "Keep at most" to 3: the list shrinks
      and the count badge updates. "Pause capture" shows the "Paused" badge and
      stops capture; the tray check mark follows it (Phase 12).
- [ ] Enter a limit below 1 KB for the text or image size: an inline error from
      the backend appears and the field returns to the stored value.
- [ ] Ignored apps: add `notepad.exe`, copy in Notepad: nothing is captured.
- [ ] No control is disabled any more: hotkey, Start at login and "Open data
      folder" work since Phase 12 (see that section).
- [ ] The stats show items, pinned, disk use, RAM cache and the key protector
      (`dpapi`); with no OCR language installed the OCR hint explains how to add one.

### Real keystroke latency (budget: under 16 ms, PLAN section 4)
- [ ] With about 1,000 items (use the Phase 14 stress script once it exists, or
      copy a lot), open DevTools, Performance, record while typing a 10 letter
      word at normal speed. Each keystroke's "Event: input" to the end of the
      resulting frame should stay under 16 ms; note the worst. The task shows
      `search` IPC, a React commit of the rows and one paint.
- [ ] Quick console measurement (paste in DevTools, then type): 
      `new PerformanceObserver(l => l.getEntries().forEach(e => e.name === "keydown" && console.log(e.duration.toFixed(1)))).observe({type: "event", durationThreshold: 16, buffered: true})`
      lists every key press slower than 16 ms.
- [ ] Reference from the Linux container (Chromium, production build, JavaScript
      fake backend, 1,000 fake items; relative only, not the Windows number):
      input event to rows in the DOM p50 about 2.5 to 4.7 ms and p95 about 7 to
      12 ms; the fake search alone is 0.3 ms. With a 4x CPU slowdown p50 was
      about 11 to 17 ms and p95 32 to 50 ms, so a slow PC is the thing to watch.
      If Windows is over budget, the first knobs are the result limit (the
      backend default is 50 rows; `search(query, filter, limit)` in `ipc.ts`
      takes a smaller one) and the rows' `content-visibility`.

## Phase 11: Copy and auto-paste

Run with `pnpm tauri dev` from a terminal. Everything below needs Windows: the
clipboard writer, the focus restore and `SendInput` cannot run in the Linux
container. What the unit tests pin down (exact bytes of `CF_UNICODETEXT`,
`CF_DIBV5`, `CF_HDROP`, the Ctrl+V key plan, the focus wait and the self-write
guard) does not need to be rechecked; what needs checking is how real apps react.

### What is written
- Text: `CF_UNICODETEXT` only, with CRLF line endings. The history keeps text
  with LF only (ingest folds CRLF), so the original endings are not known; CRLF
  is what classic Windows apps expect, and editors that prefer LF accept it.
- Images: the registered `PNG` format (the stored lossless PNG, as it is) and
  `CF_DIBV5` (32 bit, bottom up, BGRA bitfields, straight alpha, sRGB), in one
  clipboard session. Windows adds `CF_DIB` and `CF_BITMAP` on request.
- Files: `CF_HDROP` (wide paths) plus `Preferred DropEffect` = copy. Paths that no
  longer exist are left out; if none is left the copy fails with "none of the
  files exist any more" and the bar stays open.
- `ExcludeClipboardContentFromMonitorProcessing` is not set: Windows clipboard
  history (Win+V) and other clipboard managers still see what Clipdump copies.

### The plan's check: paste into other apps
For each target, copy the item with `Ctrl+Enter`, switch to the target and press
`Ctrl+V`. Use a text with several lines and non ASCII characters, a screenshot
(Win+Shift+S, ideally with a semi transparent region such as a rounded window
corner) and two files copied in Explorer.

| Target | Text | Screenshot | Files |
|---|---|---|---|
| Notepad | [ ] lines are separate, no boxes | n/a | n/a |
| Chrome (a Gmail or Docs compose box) | [ ] | [ ] pastes as an image | n/a |
| VS Code | [ ] | n/a | n/a |
| Paint | n/a | [ ] pastes, colors right, not upside down | n/a |
| Explorer | n/a | n/a | [ ] copies (the originals stay), a name clash asks as usual |
| Word | [ ] | [ ] transparent areas stay transparent or turn white, never black | [ ] |
| Slack or Teams | [ ] | [ ] | [ ] attaches both files |

- [ ] Pasting files into Explorer with `Ctrl+V` copies, never moves.
- [ ] Copying a file list where one file was deleted since: the others paste, the
      deleted one is skipped, no error toast (the "missing" badge shows in the list).
- [ ] An item whose files are all gone: the bar stays open and the footer says
      "Could not copy: none of the files exist any more".
- [ ] Paste the screenshot into Paint and Word and compare it with the original:
      no color shift (red and blue not swapped), not flipped, not shifted.
- [ ] A text with an embedded tab and trailing spaces keeps them.

### Self-write echo: no duplicate, the item moves to the top
The copy changes the clipboard, so the watcher sees an update. It must not create
a new history entry.
- [ ] Copy an old text item with `Ctrl+Enter`, reopen the bar: the item is first,
      there is no second copy of it, and the item count did not change.
- [ ] Same for a screenshot (large ones included) and for a file item.
- [ ] A file item copied while one of its files is missing: no second, shorter
      file item appears.
- [ ] The terminal shows no new `ingested ...` line for these copies. A line
      appearing for an image copy means the 150 ms window was missed (see the
      budget below); note how long the copy took.
- [ ] Copy the same text again in Notepad right after (more than half a second
      later): that is a real copy and moves the item to the top as usual.
- [ ] The use count is one per copy: copy an item three times, then compare its
      ranking against an item copied once (it ranks higher by a small margin, not
      by a large one).

### Image budget: Ctrl+Enter to clipboard set under 150 ms (4K screenshot, cache miss)
A cache miss means the PNG is read from disk, decrypted and decoded. To force it,
restart Clipdump (the cache is empty) and copy a 4K screenshot from the history.
- [ ] Measure from the key press to the moment `Ctrl+V` pastes it (or time the
      `copy_item` call in DevTools: `performance.now()` around
      `__TAURI_INTERNALS__.invoke("copy_item", { id, paste: false })`).
      Budget: under 150 ms. Note the number and the screenshot size.
- [ ] A second copy of the same screenshot (cache hit) should be clearly faster:
      only the PNG decode and the pixel conversion remain.
- If it is over budget, the time is the PNG decode plus building `CF_DIBV5`
  (about 33 MB for 4K). Rendering is eager on purpose; the options are delayed
  rendering (`WM_RENDERFORMAT`) or caching the decoded pixels. Neither is done
  because they cannot be tested without Windows.

### Paste on Enter (setting `autoPasteOnEnter`, default off)
Settings, General, "Paste on Enter". Saved to `settings.json` since Phase 12.
- [ ] Off: `Enter` and `Ctrl+Enter` only copy and hide the bar.
- [ ] On: open Notepad, press the hotkey, search, select an item, press `Enter`:
      the bar hides and the item appears in Notepad without touching `Ctrl+V`.
- [ ] On: clicking a result row does the same as `Enter`.
- [ ] On: `Ctrl+Enter` and `Ctrl+1` to `Ctrl+9` still only copy, they never paste.
- [ ] On: the text appears once, not twice. Hold `Shift` while pressing `Enter`:
      the held modifiers are released before `Ctrl+V`, so it is a plain paste, not
      `Ctrl+Shift+V` ("paste as plain text" in some apps).
- [ ] A failed paste (see the elevated window below) shows "copied, but auto-paste
      failed: ..." in the footer the next time the bar is opened (it is kept
      until then), and the item really is on the clipboard.

### Auto-paste into elevated (administrator) windows
Windows blocks synthetic keystrokes into a window with a higher integrity level
than the sending process (UIPI). Clipdump started normally cannot paste into an
elevated window, for example an admin PowerShell or Task Manager.
- [ ] With "Paste on Enter" on, press `Enter` while an elevated PowerShell was
      focused before the bar opened: the item is copied, nothing is pasted, and
      the footer says "copied, but auto-paste failed: SendInput injected 0 of 4
      key events; the window may be running as administrator ...". Press `Ctrl+V`
      in that window: the item pastes (plain copy always works).
- [ ] Start Clipdump as administrator: the same `Enter` now pastes into the
      elevated window.

### Focus restore edge cases (Paste on Enter on)
- [ ] The previous window was closed while the bar was open (open the bar over
      Notepad, close Notepad from the taskbar, press `Enter`): the item is copied,
      the footer later says "the previous window no longer exists", nothing is
      typed anywhere else.
- [ ] The previous window is minimized after the bar opens (Win+D while the bar is
      open is blocked by focus; use the taskbar to minimize it): it is restored and
      receives the paste, or the footer says it could not be brought to the front.
- [ ] The previous window is on another virtual desktop (open the bar over a
      window, then use the mouse on the taskbar's task view button to move to a
      different desktop and click the bar's item, with the bar still open): the
      paste either brings that window back (switching desktops) and pastes, or
      fails with a message. It must never paste into a window of the desktop you
      are looking at.
- [ ] No previous window (bar opened from the tray with nothing focused, or
      with the desktop focused): `Enter` just copies, no error.
- [ ] Alt key: hold `Alt` while pressing `Enter`. The target window must not open
      its menu bar (File, Edit, ...) and must paste once; if the menu opens, note
      the app.
- [ ] Hold the Windows key while pressing `Enter`: the Start menu must not open.
- [ ] The bar's own window is never the target: open the bar twice in a row
      (hotkey, hotkey) and press `Enter`; the text must not be pasted into the
      search box.
- [ ] The bar hides before the paste lands (no flash of the bar in the target
      window) and the focus returns to the same window within about 100 ms.

### Not covered here
- Rich text and HTML are not written (non goal for v1), so pasting text into Word
  gives plain text in the destination style.
- Delayed rendering and the owner window's `WM_RENDERFORMAT` handling are not
  implemented (eager rendering only).

## Phase 12: settings persistence (Windows only)

Everything below needs Windows. Container checks (unit tests, the Xvfb run of
the real app on Linux, the mock UI) are listed in the Phase 12 hand-off, not here.

### settings.json
- [ ] First launch creates `%APPDATA%\clipdump\settings.json` (type
      `%APPDATA%\clipdump` in Explorer's address bar). It is readable, pretty
      printed JSON with `version`, `hotkey`, `start_at_login`, `pinned_bar`,
      `max_items` and the other settings, and no `paused` and no theme. It is NOT
      encrypted (history stays in `%LOCALAPPDATA%\clipdump`).
- [ ] Change "Keep at most" in Settings, wait one second, open the file: the new
      value is there. Change five settings quickly: the file ends with all five
      (the writes are coalesced, nothing is written on the click itself).
- [ ] Quit from the tray right after changing a setting: the change is in the file.
- [ ] Restart the app: every setting is as left (hotkey, pinned bar, limits,
      ignore list, Paste on Enter, OCR). Capture is NOT paused after a restart,
      by design.
- [ ] Edit the file by hand: delete `max_items`, put `"max_age_days": "soon"`,
      add `"foo": 1`, save, restart. The app starts, the missing field has its
      default, the bad one falls back to 30, the unknown one is ignored.
- [ ] Break the file (delete the last brace), restart. The app starts with the
      defaults, the broken file is renamed to `settings.corrupt-<number>.json`
      next to a fresh `settings.json`.
- [ ] A file saved by Notepad with a byte order mark loads normally.

### Hotkey
- [ ] Settings, General, Hotkey, Change: the row shows "Press the new shortcut";
      hold Ctrl and Alt: the key caps show them live; press K: the row shows
      Ctrl Alt K, the file has `"hotkey": "Ctrl+Alt+K"`. The new chord toggles the
      bar, the old one (Ctrl+Shift+Space) does nothing.
- [ ] Esc while recording cancels and does NOT close the settings view.
- [ ] A key alone (for example Space) or Shift plus a letter shows "Use Ctrl, Alt
      or Win together with another key." under the row and changes nothing.
- [ ] A chord Windows reserves, for example Win+L (or Win+D, Ctrl+Alt+Delete,
      which Windows never gives to apps): the row shows "That shortcut is taken
      by another app or reserved by Windows. Choose a different one." and the old
      hotkey still works. `settings.json` still has the old chord.
- [ ] A chord another app owns (start an app that registers, for example,
      Ctrl+Alt+M, such as AutoHotkey with `^!m::MsgBox hi`): same message.
- [ ] Put a chord another app owns into `settings.json` by hand and start the
      app: it starts, the tray works, Settings shows the message under the
      hotkey row. Close the other app and change any setting: the message goes
      away and the hotkey works.

### Retention on change
- [ ] With more than 20 unpinned items and two pinned ones, set "Keep at most"
      to 10: the list shrinks to 10 unpinned plus the pinned ones at once, the
      count badge follows, the evicted items' files disappear from
      `%LOCALAPPDATA%\clipdump\blobs`. Set it to 0 (no limit): nothing more is
      evicted.
- [ ] "Keep for at most" 1 day with older items: they go at once, pinned stay.

### Start at login
- [ ] A first launch (no `settings.json`, installed build) adds Clipdump to
      Task Manager, Startup apps as Enabled, and to
      `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
- [ ] Switch "Start at login" off: the Run value is gone and Task Manager no
      longer lists it. Switch it on: it is back.
- [ ] Disable it in Task Manager's Startup tab, restart the app: the Settings
      switch now shows off and the app did not re-enable it.
- [ ] A dev build (`pnpm tauri dev`) never touches the Run key unless started
      with `CLIPDUMP_AUTOSTART=1`.

### Pinned bar
- [ ] Switch on "Pinned bar": the bar stays when you click another window and
      stays on top of it. Restart with it on: the bar is shown at startup without
      taking the keyboard focus.
- [ ] The hotkey focuses the bar instead of hiding it (input focused, text
      selected). Esc hides it (the hotkey shows it again).
- [ ] Ctrl+Enter copies and the bar stays visible. After a plain copy the bar
      still has the keyboard focus, so click the target window before Ctrl+V
      (known limitation, documented). With "Paste on Enter" on, Enter copies, the
      focus returns to the window the hotkey was pressed over and the text pastes
      there; the bar stays.
- [ ] Over a fullscreen game or video: the bar is always on top of normal
      windows; note whether it shows over exclusive fullscreen (Windows does not
      allow that for any ordinary window).
- [ ] Switch it off: clicking elsewhere hides the bar again.

### Pause capture and the tray
- [ ] Settings, Pause capture on: the tray menu's "Pause capture" shows a check
      mark; copy something: nothing is captured. Off: the check goes away.
- [ ] Tray, Pause capture: the Settings switch (open it, also while the bar is
      pinned and already showing settings) and the "Paused" badge follow.
- [ ] Pause, restart the app: capture is running again and neither the tray
      check nor the switch is set.

### Open data folder
- [ ] Settings, Data, Open: Explorer opens `%LOCALAPPDATA%\clipdump` with
      `history.db`, `blobs`, `thumbs` and `key.bin`. Click it twice: two windows
      or one reused, no error. Delete the folder first (app closed), start the
      app and click Open: the folder is created and opens.
