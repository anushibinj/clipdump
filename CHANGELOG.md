# Changelog

All notable changes to Clipdump are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). The version is the one in `package.json`;
`src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` must say the same
(`pnpm check:version` checks all of it).

## [Unreleased]

### Added

- Focus lock for tasks. `Ctrl+L` (or the timer icon on a row) locks the list to one task for 15, 25,
  30 or 60 minutes, or a number you type. While it runs every other task is dimmed and cannot be
  changed, selected, or deleted; you can still add new tasks, which wait. The lock survives
  restarts and ends by itself, when you finish the task, or when you break it: that takes 30 seconds
  of waiting and starts the streak over.
- The countdown is also in the window title; a heads-up comes 5 minutes before the end, and a tone
  (and a system notification, where allowed) when it is over. One pause of up to 5 minutes per session.
- A parking lot for distractions: `Ctrl+P` while locked, type the thought, Enter. Afterwards each one
  can be turned into a task or dropped.
- After a session: "Done, Need more time, Blocked", then a 5 minute break, another session, or skip.
- Focus stats: a streak counter (sessions in a row without breaking the lock), the time focused today
  (with the last 7 days on hover), and the total time focused on each task.

## [0.2.0] - 2026-11-20

### Added

- A task list inside the bar, one key away: `Ctrl+T` switches between the clipboard search and
  the tasks (and back), from any view. Type a task and press `Enter` to add it; end it with ` !`
  to mark it important; `Ctrl+Enter` adds it and starts it right away.
- Everything you are working on is in view at once. The **Now** section lists every task in
  flight, most recently touched first, each with a "Left off" note (`Ctrl+E`) so picking it up
  again after a switch takes no thinking. A task in flight that has not been touched for a day
  shows its age in amber. The search view's filter row names the task touched last (and how many
  more are in flight), so it is in sight every time the bar opens.
- Tasks that are not important stay out of the way. The list shows **Now** and a short **Up next**
  (important tasks, three at most); the rest, and the tasks finished today, are one count away
  ("Show 4 hidden"), toggled with `Ctrl+H`.
- Moving a task along is one key: `Enter` on an empty input goes waiting, in flight, done;
  `Shift+Enter` steps back; `Ctrl+I` toggles important; `F2` edits the title;
  `Ctrl+Delete` deletes. Marking a task done says how to undo it.
- The bar reopens on the view it was left on, across openings and restarts: leave it on the task
  list and it opens on the task list. Only the revealed hidden tasks fold away again on each open;
  a half typed task stays.
- Tasks are stored encrypted with the history's data key in their own file (`tasks.bin`), so
  "Clear history" and a history recovery never touch them. Finished tasks are dropped after 30 days.

## [0.1.2] - 2026-11-20

### Added

- AI split turns key-value pairs into two copyable items. When a text pairs a key with a
  value, in any notation (`key: value`, `key - value`, `key = value`, `key | value`,
  `key => value`, a tab or a table row), the split returns a "<name> key" entry and a
  "<name> value" entry, so each can be pasted into a form on its own. The separator is
  never part of either item.

### Changed

- Installing a newer version over an existing install now pre-selects "Update the existing
  installation", which installs in place and keeps the clipboard history and settings. Before,
  the installer pre-selected "Uninstall before installing" and ran the full uninstaller.
  Uninstalling first is still offered as the second choice.

### Improved

- Finished AI splits are cached in memory for the session. Going back to an item shows its
  pieces at once instead of the skeleton loader. Failed splits are not cached, so they can
  be retried.

## [0.1.1] - 2026-11-20

### Added

- The bar can be moved: drag it by the search row or the filter row. The position is
  remembered per monitor (in logical pixels from the monitor's corner, so a DPI change
  keeps it) and used every time the bar opens on that monitor. It is pulled back on
  screen if the monitor got smaller. Monitors never dragged on keep the old placement.
- Pin and Dismiss icons at the top right of the bar. Pin is the "Pinned bar" setting
  (the bar stays open on blur and after a copy); Dismiss hides the bar like Esc.
- AI split: with an OpenAI compatible server configured in Settings (OpenAI, Ollama,
  LM Studio, ...), the right pane has two tabs, Preview and AI split. Every text you copy
  (20 characters or more) is queued for the model right away, so its pieces (numbers,
  e-mails, tokens, user names, passwords, ...) are usually ready when you open the bar;
  each one is a click to copy. While a text waits or is being split the tab shows a
  loading skeleton. Re-split asks again if the first answer is not right. The calls run
  in a background queue, first in first out, two at a time, so the app stays fast and the
  server is not flooded. Items the model invents or alters are dropped: every value must
  occur in the text exactly. Results are kept in the encrypted history database and go
  with their item when it is deleted, cleared or expires. The tab and whether the right
  pane is open are remembered (Esc now hides the bar even when the pane is open; Tab
  closes the pane). `Ctrl+Shift+S` opens the pane on the AI split tab. The API key is
  stored in `settings.json` (not encrypted) and never returned to the UI. Note that a
  copied text, passwords included, is sent to your server whenever Split is set up
  (apps on the ignore list are never captured, so never sent).

### Changed

- Esc hides the bar even when the right pane is open (the pane is remembered; Alt+P closes it).
- The footer no longer shows the "move" and "settings" shortcut hints; the shortcuts still work.
- `settings.json` gains `window_positions` and the LLM fields, and the history database gains
  a `splits` table; both are upgraded automatically.

## [0.1.0] - Unreleased

First version. Not yet released: the code is written and tested on Linux and
cross-checked against the Windows APIs, but **it has not been run on Windows**. The
installer has not been built, installed or uninstalled on a real machine, and none of
the performance budgets in `docs/perf.md` has a Windows measurement yet. Until the
checklists in `docs/manual-checks.md` are done, treat this as a release candidate.

### Added

- A background app that records what is copied and finds it again from the keyboard:
  global hotkey (default `Ctrl+Shift+Space`), a borderless always-on-top search bar that
  is created once and only hidden afterwards, tray icon with Show, Pause capture,
  Settings and Quit, single instance, start at login.
- Capture of text, images and screenshots (registered `PNG` format first, then
  `CF_DIBV5` and `CF_DIB`) and files and folders (`CF_HDROP`, stored as references), with
  the skip rules for password managers and the "exclude from history" clipboard formats,
  an ignore list of apps, size limits, and suppression of the app's own clipboard writes.
- Ingest on background threads: text normalization and preview, lossless PNG storage,
  thumbnails, hashing of the decoded pixels so the same picture from different formats is
  one item, file lists with sizes.
- OCR of images and screenshots with the built-in Windows.Media.Ocr engine, in the
  background, so a word seen in a screenshot finds the screenshot. Can be switched off.
- Search on every keystroke over text, OCR text and file names with fuzzy matching and
  highlighted matches, `img:`, `file:` and `txt:` prefixes, type filters, pinned items
  first for an empty query, ranking by match, recency and use count.
- Copy back of any kind: text, images as PNG and `CF_DIBV5`, files as `CF_HDROP` with a
  copy drop effect. `Ctrl+Enter` copies, `Ctrl+1` to `Ctrl+9` copy the Nth result, and an
  optional setting makes `Enter` copy and paste into the window that was active before.
- Pin, delete and clear history (pinned items are kept), a preview pane, inline
  confirmation, keyboard hints.
- Retention of at most 1,000 unpinned items and 30 days, plus a 2 GB disk cap, all
  configurable; pinned items are never evicted.
- Persistence across restarts: SQLite in WAL mode, content addressed blob and thumbnail
  files, every piece of content encrypted with XChaCha20-Poly1305 and the key protected
  with Windows DPAPI. A history that cannot be opened is moved aside and a fresh one is
  started, with a notice in the bar and the tray.
- Settings in `%APPDATA%\clipdump\settings.json`: hotkey, limits, OCR, auto-paste,
  pinned bar mode, start at login, ignored apps, and an "Open data folder" button.
- Ordered, time bounded shutdown that flushes the writer on quit, logoff and Windows
  shutdown.
- A `--perf` mode that logs timings, and the measurement tooling and Linux figures in
  `docs/perf.md`.
- Windows packaging: a per-user NSIS installer (no administrator prompt), the WebView2
  bootstrapper downloaded silently when the runtime is missing, an uninstaller checkbox
  "Also delete clipboard history" (unchecked by default), removal of the start at login
  entry on uninstall, and a GitHub Actions workflow that builds the installer on a `v*`
  tag. Builds are not code signed, so Windows SmartScreen will warn about them.

### Known limitations

- Plain text only; no rich text or HTML.
- Files are stored as references; a moved or deleted file shows a "missing" badge.
- Auto-paste into an elevated (administrator) window needs Clipdump to run elevated
  too; copying always works.
- Windows only. The macOS and Linux ports are not started.

[Unreleased]: https://github.com/anushibinj/clipdump/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/anushibinj/clipdump/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/anushibinj/clipdump/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/anushibinj/clipdump/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/anushibinj/clipdump/releases/tag/v0.1.0
