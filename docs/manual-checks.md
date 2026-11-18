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
