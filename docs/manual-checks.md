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
  needs the Phase 11 copy path. Unit tests cover the logic, the real Windows
  behavior must be re-checked in Phase 11 by copying an item back and confirming
  that no new history entry appears.
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
      `Settings::default()` (no settings file before Phase 12), copy ten 4K
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
- [ ] There is no settings screen or `settings.json` before Phase 12, so the
      switch is `Settings::default().ocr_enabled` in `src-tauri/src/settings.rs`.
      Set it to `false`, run `pnpm tauri dev`, copy a screenshot: no `ocr: item`
      line, the row stays `ocr_status = 0`. Set it back to `true` and restart:
      the pending screenshots are processed. (Changing it live is wired in
      Phase 12, which calls `OcrWorker::apply_settings`.)

### Not testable yet
- Searching OCR text, the "OCR" badge and the snippet (Phases 8 to 10).
- The language status and hint in settings (Phase 10).
- Live on/off switching from a settings screen (Phase 12).
