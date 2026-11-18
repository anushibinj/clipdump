# Changelog

All notable changes to Clipdump are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). The version is the one in `package.json`;
`src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` must say the same
(`pnpm check:version` checks all of it).

## [Unreleased]

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

[Unreleased]: https://github.com/anushibinj/clipdump/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/anushibinj/clipdump/releases/tag/v0.1.0
