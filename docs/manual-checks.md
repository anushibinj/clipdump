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
