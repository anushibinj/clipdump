# Clipdump: Ultra-fast Clipboard Dumpyard (Windows first)

## 0. Working rules (follow throughout)

- **Commit small and often.** Make a git commit after every small module or logical unit (one file, one function group, one config change). Ten commits per feature is fine. Never batch a whole phase into one commit. Use Conventional Commit one-liners, e.g. `feat(store): add hash dedup index`.
- Use **pnpm** for every Node operation (`pnpm install`, `pnpm dlx`, `pnpm tauri dev`). Never use npm or npx.
- Do not use em dashes in any text, docs, UI copy or comments.
- Speed is the top requirement. When a choice trades speed for convenience, pick speed and note why in a code comment.
- **The hot path (hotkey, typing, copy) must never touch disk, OCR or image decoding.** Those run on background threads.
- Local only. No network calls at runtime, no telemetry, no remote fonts or CDNs.
- History persists across restarts, **encrypted at rest**. RAM holds metadata, search text and thumbnails; large payloads live on disk.
- Keep platform-specific code behind traits/modules so macOS and Linux can be added later without touching core logic.

## 1. Product spec

A background app that records everything copied (text, images, screenshots, files) and lets the user find and re-copy any past item in under a second, keyboard only. History survives PC restarts.

### Core flow
1. App starts at login, lives in the system tray, uses ~0% CPU when idle.
2. User presses a global hotkey (default `Ctrl+Shift+Space`). A small, borderless, always-on-top search bar appears instantly, centered near the top of the active monitor, input already focused.
3. User types. Results update on every keystroke. Search covers text content, **OCR text extracted from images and screenshots**, and file names/paths.
4. Arrow keys move selection. **`Ctrl+Enter` copies the selected item to the clipboard and hides the bar.** User pastes anywhere with `Ctrl+V`. Images paste as images, files paste as files (e.g. into Explorer, Slack, email).
5. Optional (setting, default off): **`Enter` copies AND auto-pastes** into the window that was focused before the bar opened.
6. `Esc` or clicking outside hides the bar. The bar is never destroyed, only hidden, so the next open is instant.

### What gets captured
| Type | Windows source | Stored as |
|---|---|---|
| Text | `CF_UNICODETEXT` | UTF-8 text |
| Images and screenshots (Win+Shift+S, PrtSc, browser "copy image") | registered `PNG` format first, else `CF_DIBV5` / `CF_DIB` | lossless PNG blob + thumbnail + OCR text |
| Files, folders, media (video, audio, docs) copied in Explorer | `CF_HDROP` | list of paths + sizes (references, not copies), thumbnail for image files |

Files are stored as **references**. If a file is later moved or deleted, the item shows a "missing" badge and re-copy is disabled for that path. (Future option: snapshot files under a size limit.)

### History rules
- **Retention: keep at most 1,000 items OR 30 days, whichever limit is hit first.** Both values are configurable. Oldest unpinned items are evicted first.
- **Pinned items are exempt** from both limits and from "clear history". They show at the top when the query is empty.
- Additional safety cap on disk usage (default 2 GB); when exceeded, evict oldest unpinned blobs first.
- Duplicate copies do not create new entries; the existing item moves to the top.
- Password manager copies are never captured.

### Shortcuts (inside the bar)
- `Ctrl+Enter`: copy selected. `Enter`: copy + auto-paste (if enabled).
- `Ctrl+1` .. `Ctrl+9`: copy the Nth visible result instantly.
- `Ctrl+P`: pin/unpin selected item.
- `Ctrl+Delete` (or `Shift+Delete`): delete selected item.
- `Ctrl+Shift+Delete`: clear entire history except pinned items (inline confirm: press again to confirm).
- `Alt+1` .. `Alt+4`: type filter All / Text / Images / Files. Typed prefixes also work: `img:`, `file:`, `txt:`.
- `Tab`: toggle preview pane (full text, large image with OCR text below, or file list).
- `Ctrl+,`: settings.

### Optional "pinned bar" mode (setting)
Keep the bar permanently visible as a slim always-on-top strip (does not hide on blur). Hotkey then just focuses it.

### Non-goals for v1
Rich text/HTML formats (plain text only), sync, cloud anything, copying file contents into storage.

## 2. Tech stack

| Layer | Choice | Reason |
|---|---|---|
| Shell | **Tauri v2** (Rust) | Tiny binary, native backend, web UI so shadcn/ui works as-is. |
| Backend | **Rust** stable | Zero-cost Win32 hooks, fast search, no GC pauses. |
| Frontend | **Vite + React 19 + TypeScript** | Fast, shadcn/ui is React-first. |
| Styling | **Tailwind CSS v4 + shadcn/ui** | Use `Command` (cmdk), `Badge`, `Kbd`, `Switch`, `Input`, `ScrollArea`, `Separator`, `Tooltip`, `ToggleGroup`. |
| Fuzzy search | **`nucleo-matcher`** | Helix editor's matcher, very fast, gives highlight indices. |
| Hashing | **`xxhash-rust`** (xxh3) | Fastest non-crypto hash for dedup. |
| Persistence | **`rusqlite`** (feature `bundled`), WAL mode | Metadata, text, OCR text. Single file, crash safe. |
| Blob storage | Content-addressed files on disk | Images and thumbnails, kept out of SQLite and out of RAM. |
| Encryption | **`chacha20poly1305`** (XChaCha20-Poly1305) + Windows **DPAPI** to protect the key | Clipboard data is sensitive; only the logged-in Windows user can decrypt. |
| Images | **`image`** crate (png, bmp, jpeg, webp features) | Decode DIB, encode PNG, make thumbnails. |
| OCR | **Windows.Media.Ocr** (WinRT, via the `windows` crate) | Built into Windows 10/11, offline, fast, no extra install. |
| RAM cache | **`lru`** crate | Hot blobs cached under a byte budget. |
| Windows APIs | **`windows`** crate | Clipboard listener, DPAPI, OCR, SendInput, focus tracking. |
| Tauri plugins | `global-shortcut`, `single-instance`, `autostart` | Official plugins. |

## 3. Storage architecture

```
RAM (always)                          Disk: %LOCALAPPDATA%\clipdump\
- item metadata (id, kind, times,     - history.db        (SQLite, WAL)
  pinned, sizes, source app)            items table, encrypted text columns
- search text: text content (first    - blobs\ab\<hash>.bin   (encrypted PNG)
  10,000 chars), OCR text, file names - thumbs\ab\<hash>.bin  (encrypted WebP)
- thumbnails (small, ~10 KB each)     - key.bin  (32-byte key, DPAPI protected)
                                      - settings.json (not encrypted)
RAM (bounded LRU, default 64 MB)
- full text of large items
- full image blobs recently used
```

Rules:
- **Every item is written to disk** (needed for restart persistence). RAM is a fast index and cache on top.
- Text items up to 64 KB keep full text in RAM. Larger text keeps only the first 10,000 chars in RAM for search and preview; the full text loads from SQLite on copy/preview.
- Image blobs are never required in RAM. They load from disk into the LRU only when copied or previewed.
- Thumbnails (max 96 px tall, WebP) stay in RAM for all items (1,000 x ~10 KB = ~10 MB).
- When the LRU exceeds its budget (setting, default 64 MB), least recently used blobs are dropped from RAM only.
- Thumbnails and previews reach the webview through a custom URI scheme (`clip://thumb/<id>`, `clip://blob/<id>`), never as base64 over IPC.

### SQLite schema (v1, use a `user_version` pragma for migrations)
```sql
CREATE TABLE items (
  id            INTEGER PRIMARY KEY,
  hash          BLOB NOT NULL UNIQUE,   -- xxh3-128 of normalized content
  kind          INTEGER NOT NULL,       -- 0 text, 1 image, 2 files
  created_at    INTEGER NOT NULL,       -- unix ms
  last_used_at  INTEGER NOT NULL,
  use_count     INTEGER NOT NULL DEFAULT 0,
  pinned        INTEGER NOT NULL DEFAULT 0,
  preview_enc   BLOB NOT NULL,          -- encrypted short preview
  text_enc      BLOB,                   -- encrypted full text (text items)
  ocr_enc       BLOB,                   -- encrypted OCR text (image items)
  ocr_status    INTEGER NOT NULL DEFAULT 0, -- 0 pending, 1 done, 2 failed, 3 unavailable
  files_enc     BLOB,                   -- encrypted JSON [{path,size,is_dir}]
  blob_hash     TEXT,                   -- image blob file name
  width         INTEGER, height INTEGER,
  byte_size     INTEGER NOT NULL,
  source_app    TEXT                    -- exe name of clipboard owner
);
CREATE INDEX idx_items_created ON items(created_at);
CREATE INDEX idx_items_pinned ON items(pinned);
```
Pragmas: `journal_mode=WAL`, `synchronous=NORMAL`, `temp_store=MEMORY`.

## 4. Performance budgets (verify in Phase 14)

| Metric | Target |
|---|---|
| Hotkey press to bar visible with focused input | < 50 ms |
| Keystroke to results rendered (1,000 items incl. OCR text) | < 16 ms (one frame) |
| Search itself in Rust (1,000 items) | < 1 ms |
| Ctrl+Enter to clipboard set (text) | < 20 ms |
| Ctrl+Enter to clipboard set (image, cache miss, 4K screenshot) | < 150 ms |
| Cold start: load 1,000 items from disk and ready to search | < 300 ms |
| Clipboard event to item visible in history (image, excl. OCR) | < 300 ms |
| OCR of a 1080p screenshot (background) | < 1 s |
| Idle RAM (whole process incl. WebView2) | < 120 MB |
| Idle CPU | ~0% (event driven, no clipboard polling) |
| Installer size | < 12 MB |

## 5. Project structure

```
clipdump/
  PLAN.md
  package.json, pnpm-lock.yaml, vite.config.ts, tsconfig.json, components.json, index.html
  src/                          # React frontend
    main.tsx, App.tsx, index.css
    lib/  ipc.ts  utils.ts  time.ts  bytes.ts
    components/
      ui/                       # shadcn generated
      SearchBar.tsx  TypeFilter.tsx  ResultList.tsx  ResultItem.tsx
      PreviewPane.tsx  ImagePreview.tsx  FileList.tsx
      SettingsView.tsx  EmptyState.tsx  KbdHints.tsx  ConfirmInline.tsx
    hooks/  useSearch.ts  useKeyboard.ts
  src-tauri/
    Cargo.toml  tauri.conf.json  capabilities/default.json  icons/
    src/
      main.rs                   # calls lib::run()
      lib.rs                    # builder, plugins, setup, uri scheme
      state.rs                  # AppState
      model.rs                  # ClipItem, ClipKind, Payload, FileRef
      store/
        mod.rs                  # in-memory index, dedup, pins, retention
        retention.rs            # 1,000 items / 30 days / disk cap logic
        cache.rs                # LRU blob cache with byte budget
      persist/
        mod.rs                  # writer thread + channel
        db.rs                   # SQLite schema, migrations, queries
        blobs.rs                # content-addressed blob + thumb files
        crypto.rs               # XChaCha20 encrypt/decrypt, key load
      ingest/
        mod.rs                  # processing pipeline (thread pool)
        image.rs                # DIB to PNG, normalize, thumbnail
        files.rs                # file refs, existence checks
      ocr/
        mod.rs                  # OcrEngine trait + worker queue
      search.rs                 # nucleo wrapper + ranking + filters
      commands.rs               # tauri commands
      window.rs                 # show/hide/position/toggle
      settings.rs
      tray.rs
      platform/
        mod.rs                  # traits
        windows/
          watcher.rs            # AddClipboardFormatListener
          read.rs               # read text / PNG / DIB / HDROP
          write.rs              # set text / image / HDROP
          paste.rs              # SendInput Ctrl+V
          focus.rs              # foreground window tracking
          dpapi.rs              # CryptProtectData / CryptUnprotectData
          ocr.rs                # Windows.Media.Ocr implementation
    benches/ search_bench.rs
```

## 6. Phases

### Phase 0: Prerequisites (user installs, Claude Code verifies)
- Rust stable via rustup, MSVC toolchain. Visual Studio 2022 Build Tools with "Desktop development with C++".
- WebView2 runtime (preinstalled on Windows 10/11).
- Node 22 LTS and pnpm (`corepack enable && corepack prepare pnpm@latest --activate`).
- An OCR language installed in Windows (Settings > Time & language > Language; English usually present).
- Verify: `rustc -V`, `cargo -V`, `node -v`, `pnpm -v`.

### Phase 1: Scaffold
1. `pnpm create tauri-app@latest clipdump` (TypeScript, pnpm, React, Tauri v2). Commit.
2. `git init` if needed, `.gitignore` (node_modules, target, dist). Commit.
3. Tailwind v4 via `@tailwindcss/vite`. Commit.
4. `pnpm dlx shadcn@latest init` (new-york, zinc, CSS variables). Set `@/` alias in `tsconfig.json` and `vite.config.ts`. Commit.
5. `pnpm dlx shadcn@latest add command badge input switch scroll-area separator tooltip kbd toggle-group`. Commit.
6. Local font (`@fontsource-variable/geist` or Inter). Commit.
7. Release profile in `src-tauri/Cargo.toml`. Commit:
   ```toml
   [profile.release]
   opt-level = 3
   lto = "fat"
   codegen-units = 1
   panic = "abort"
   strip = true
   ```
8. Check: `pnpm tauri dev` opens the default window.

### Phase 2: Window shell, hotkey, tray, lifecycle
1. `tauri.conf.json` window `main`: `width 720`, `height 460`, `decorations false`, `transparent true`, `alwaysOnTop true`, `skipTaskbar true`, `visible false`, `resizable false`, `shadow true`. Commit.
2. `tauri-plugin-single-instance`: second launch shows the bar. Commit.
3. `tauri-plugin-global-shortcut`: `Ctrl+Shift+Space` calls `window::toggle()`. Commit.
4. `window.rs`: `show()` records foreground HWND, positions on the cursor's monitor (~20% from top, DPI aware), shows, focuses, emits `bar:shown`. `hide()` emits `bar:hidden`. Hide on blur unless pinned-bar mode. Commit each.
5. Tray: Show, Pause capture, Settings, Quit. Left click toggles. Commit.
6. `tauri-plugin-autostart` enabled by default. Commit.
7. Window close only hides. Commit.
8. Check: hotkey toggles a blank bar instantly and repeatedly.

### Phase 3: Encryption and persistence layer
1. `platform/windows/dpapi.rs`: wrap `CryptProtectData` / `CryptUnprotectData` (current user scope). Commit.
2. `persist/crypto.rs`: on first run generate a random 32-byte key, DPAPI-protect it, save `key.bin`. Load and unprotect on start. `encrypt(&[u8]) -> Vec<u8>` (24-byte random nonce prefix) and `decrypt`. Zeroize key on drop (`zeroize` crate). Commit.
3. Unit tests: round trip, tamper detection. Commit.
4. `persist/db.rs`: open `history.db`, pragmas, schema from Section 3, migrations via `user_version`. Commit.
5. Queries: `insert_item`, `update_used`, `set_pinned`, `delete_item`, `delete_unpinned`, `delete_older_than`, `set_ocr`, `load_all_meta`, `load_text(id)`. Commit each small group.
6. `persist/blobs.rs`: write/read/delete encrypted blob and thumb files at `blobs/<first2>/<hash>.bin`. Write to a temp file then rename (atomic). Commit.
7. `persist/mod.rs`: single **writer thread** receiving `WriteOp` messages over a crossbeam channel; batches ops into one transaction every 50 ms or 32 ops. Reads happen on caller threads with a separate read connection. Commit.
8. On startup: verify every blob referenced exists; drop orphan rows and delete orphan blob files. Commit.
9. Check: tests pass with a temp directory; DB file is unreadable as plain text.

### Phase 4: Clipboard capture (Windows)
1. `platform/mod.rs` traits:
   ```rust
   pub enum Captured { Text(String), Image(ImageData), Files(Vec<PathBuf>) }
   pub trait ClipboardWatcher { fn start(tx: Sender<Captured>) -> anyhow::Result<Self> where Self: Sized; }
   pub trait ClipboardWriter { fn set(payload: &Payload) -> anyhow::Result<()>; }
   pub trait Paster { fn paste_into(target: Option<isize>) -> anyhow::Result<()>; }
   ```
   Commit.
2. `watcher.rs`: dedicated thread, message-only window (`HWND_MESSAGE`), `AddClipboardFormatListener`, `GetMessageW` loop on `WM_CLIPBOARDUPDATE`. Commit.
3. Skip rules before reading anything: capture paused; registered formats `ExcludeClipboardContentFromMonitorProcessing`, `Clipboard Viewer Ignore`, or `CanIncludeInClipboardHistory` = 0 present; owner process in user ignore list; self-write window (see 6). Commit.
4. `read.rs`, priority order: `CF_HDROP` (files) > `PNG` registered format > `CF_DIBV5` > `CF_DIB` > `CF_UNICODETEXT`. If both image and text exist (e.g. copied Excel cells, which also offer a bitmap), prefer **text** when the source app also provides `CF_UNICODETEXT` and the text is non-empty; otherwise take the image. Keep this rule in one function with tests. `OpenClipboard` retry up to 5 times, 5 ms apart. Copy the raw bytes out and close the clipboard immediately; do no decoding on this thread. Commit.
5. Size limits (settings): text max 5 MB, image max 50 MB raw. Over limit: skip and log. Commit.
6. Self-write suppression: when the app sets the clipboard, store the item hash and a timestamp in atomics; ignore updates within 500 ms whose content hash matches (or any update within 150 ms for images, since Windows synthesizes formats). Instead, bump that item to the top. Commit.
7. Send `Captured` to the ingest pipeline. Commit.
8. Check: text from Notepad, Chrome, VS Code; image from Win+Shift+S, Paint, Chrome "Copy image"; files from Explorer. Password manager copies are skipped.

### Phase 5: Ingest pipeline
1. `ingest/mod.rs`: small thread pool (2 threads, below-normal priority via `SetThreadPriority`) consuming `Captured`. Commit.
2. Text: normalize line endings, hash, build preview (first 300 chars, whitespace collapsed). Commit.
3. `ingest/image.rs`: DIB to image by prepending a BMP file header and decoding with `image`; PNG decoded directly. Hash the **RGBA pixels + dimensions** (so the same picture from different formats dedups). Encode lossless PNG for storage. Make a WebP thumbnail max 96 px tall. Commit each step.
4. `ingest/files.rs`: collect path, size, is_dir, a thumbnail if the file is an image under 20 MB. Hash the sorted path list. Preview = file names joined. Commit.
5. Record source app exe name (`GetClipboardOwner` > `GetWindowThreadProcessId` > `QueryFullProcessImageNameW`). Commit.
6. Hand the finished item to the store (Phase 6), which emits `history:changed` (debounced, max once per 50 ms) and queues a persist op. Image items are also queued for OCR. Commit.

### Phase 6: In-memory store, retention, cache
1. `model.rs`:
   ```rust
   pub enum ClipKind { Text, Image, Files }
   pub struct ClipItem {
     pub id: u64, pub hash: u128, pub kind: ClipKind,
     pub created_at: i64, pub last_used_at: i64, pub use_count: u32, pub pinned: bool,
     pub preview: Arc<str>,
     pub search_text: Arc<str>,        // text (first 10k chars) or OCR text or file names
     pub text_in_ram: Option<Arc<str>>,// full text when <= 64 KB
     pub thumb: Option<Arc<[u8]>>,
     pub files: Option<Arc<[FileRef]>>,
     pub width: u32, pub height: u32, pub byte_size: u64,
     pub ocr_status: OcrStatus, pub source_app: Option<Arc<str>>,
   }
   ```
   Commit.
2. `store/mod.rs`: `Vec`/`VecDeque` newest first + `HashMap<u128, u64>` for O(1) dedup + `HashMap<u64, usize>` id lookup. `parking_lot::RwLock`. Search takes a read lock only. Commit.
3. Operations: `insert`, `get`, `remove`, `toggle_pin`, `clear_unpinned`, `mark_used`, `set_ocr_text`. Each also queues the matching persist op. Removing an item deletes its blob and thumb files. Commit each group.
4. `store/retention.rs`: after every insert, evict oldest unpinned while `unpinned_count > max_items` (default 1,000). On startup and every 15 minutes (one timer thread, sleeps between runs), evict unpinned older than `max_age_days` (default 30). Also enforce disk cap (default 2 GB). Pinned items are never evicted. Commit.
5. `store/cache.rs`: `lru::LruCache<u64, Arc<[u8]>>` with a byte budget (default 64 MB). `get_full_text(id)` and `get_blob(id)` check RAM, else load from disk, decrypt, insert. Commit.
6. Startup load: read all rows, decrypt preview/search text/thumbs in parallel (`rayon`), build indexes. Must meet the < 300 ms budget for 1,000 items. Commit.
7. Unit tests: dedup across formats, 1,000-item cap, 30-day expiry, pins survive both limits and clear-all, disk cap, cache eviction, reload after restart gives identical state. Commit.

### Phase 7: OCR
1. `ocr/mod.rs`: `trait OcrEngine { fn recognize(&self, rgba: &[u8], w: u32, h: u32) -> anyhow::Result<String>; fn available(&self) -> bool; }` plus a single worker thread with a queue (lowest thread priority). Commit.
2. `platform/windows/ocr.rs`: `OcrEngine::TryCreateFromUserProfileLanguages()`. Convert RGBA to `SoftwareBitmap` (`Bgra8`). Downscale if a side exceeds `OcrEngine::MaxImageDimension`. Small images (under 40 px tall) upscale 2x for better accuracy. Join lines with newlines. Needs `windows` features: `Media_Ocr`, `Graphics_Imaging`, `Storage_Streams`, `Foundation`. Commit.
3. If no OCR language is installed: mark items `ocr_status = unavailable` and show a hint in settings explaining how to add a Windows language pack. Commit.
4. On result: update the item's `search_text` in the store, persist `ocr_enc`, emit `history:changed`. Commit.
5. On startup, re-queue items with `ocr_status = pending` (e.g. app closed mid-OCR). Commit.
6. Setting to turn OCR off. Commit.
7. Check: take a screenshot of this file with Win+Shift+S, then search a word from it; the screenshot appears in results.

### Phase 8: Search engine
1. `search.rs`: reuse a `nucleo_matcher::Matcher` via `thread_local!`. `Pattern::parse` with smart case and normalization. Commit.
2. Parse filters: `img:`, `file:`, `txt:` prefixes and the UI type filter. Commit.
3. Empty query: pinned first, then most recent. Commit.
4. Non-empty: match against `search_text` (capped at 10,000 chars). Rank = fuzzy score + recency boost + `ln(1 + use_count)`. Exact substring matches above fuzzy-only. Commit.
5. Return top 50 DTOs: `{ id, kind, preview, match_indices, match_source ("text" | "ocr" | "file"), created_at, use_count, pinned, width, height, byte_size, file_count, missing_files, has_thumb }`. For OCR matches, send a short snippet around the match as the subtitle. Never send full content. Commit.
6. Criterion bench with 1,000 and 10,000 items mixing short text, URLs, code, paragraphs and long OCR text. Must meet < 1 ms at 1,000. Commit.
7. Ranking tests. Commit.

### Phase 9: IPC layer
1. `commands.rs` (sync, fast), commit each group:
   - `search(query, filter, limit) -> Vec<ResultDto>`
   - `get_preview(id) -> PreviewDto` (full text, or OCR text + dimensions, or file list with existence)
   - `copy_item(id, paste: bool)`
   - `delete_item(id)`, `toggle_pin(id)`, `clear_history()` (unpinned only)
   - `get_settings()`, `set_settings(settings)`, `get_stats()` (counts, RAM cache size, disk usage)
   - `hide_bar()`
2. Register URI scheme `clip`: `clip://thumb/<id>` serves the thumbnail from RAM, `clip://blob/<id>` serves the full PNG via the cache. Set long cache headers. Commit.
3. Capabilities: allow only used commands and plugins. Commit.
4. `src/lib/ipc.ts`: typed wrappers + listeners for `bar:shown`, `bar:hidden`, `history:changed`. Commit.

### Phase 10: UI (shadcn look)
Design: Raycast/Linear-style palette with shadcn defaults. Rounded-xl card, 1 px `border`, `bg-popover/95` + `backdrop-blur`, soft shadow, zinc palette, follows system light/dark.

1. `App.tsx`: transparent body, one rounded card, 8 px padding for shadow. Commit.
2. `SearchBar.tsx`: `CommandInput` with lucide search icon, placeholder "Search text, screenshots, files...", badges on the right for item count and "Paused". Focus and select-all on `bar:shown`. Commit.
3. `TypeFilter.tsx`: shadcn `ToggleGroup` (All / Text / Images / Files) under the input, bound to `Alt+1..4`. Commit.
4. `ResultList.tsx`: `Command` with **`shouldFilter={false}`**. About 8 rows visible. Commit.
5. `ResultItem.tsx`: Commit.
   - Text: one-line preview with highlighted matches, "n lines" badge for multi-line.
   - Image: 40 px thumbnail (`<img src="clip://thumb/<id>" loading="lazy">`), dimensions and size ("1920x1080, 1.2 MB"), OCR snippet with highlights when matched via OCR, small "OCR" badge.
   - Files: file icon, first file name + "+3 more", "missing" badge in red if a path no longer exists.
   - Right side: relative time, pin icon, `Kbd` `Ctrl+1..9` for first nine.
6. `PreviewPane.tsx` (Tab): text in monospace; `ImagePreview.tsx` shows the full image fitted plus OCR text below; `FileList.tsx` lists paths with sizes. Commit each.
7. `ConfirmInline.tsx`: footer turns into "Press Ctrl+Shift+Delete again to clear N items (pinned kept)" for 3 seconds. Commit.
8. `KbdHints.tsx`: footer hints. Commit.
9. `EmptyState.tsx`. Commit.
10. `useSearch.ts`: search on every input change, no debounce, drop stale responses via a request counter, re-run on `history:changed`, reset selection on new query. Commit.
11. `useKeyboard.ts`: all shortcuts in Section 1. Commit.
12. `SettingsView.tsx`: hotkey recorder; max items (default 1,000); max age days (default 30); disk cap; RAM cache budget; max text/image size; OCR on/off with language status; auto-paste on Enter; pinned-bar mode; start at login; pause capture; ignore apps list; theme; "Open data folder"; stats (items, disk used, RAM cache used). Commit each section.
13. No router, no state library, no animation library. CSS transitions only. Commit.

### Phase 11: Copy and auto-paste
1. `write.rs`, commit each:
   - Text: `CF_UNICODETEXT`.
   - Image: set both `CF_DIBV5` (built from RGBA) and the registered `PNG` format so every app gets its preferred form.
   - Files: build a `DROPFILES` struct for `CF_HDROP`, plus `Preferred DropEffect` = copy. Skip missing paths; if all are missing, show an error toast instead.
2. `copy_item`: record self-write hash/timestamp, set clipboard, `mark_used`, hide window. Commit.
3. Auto-paste: `SetForegroundWindow(prev_hwnd)` (with `AttachThreadInput` fallback), wait until it is foreground (poll every 2 ms, max 100 ms), release any held modifiers, then `SendInput` Ctrl+V. Commit.
4. Check: paste text, a screenshot and files into Notepad, Chrome, VS Code, Paint, Explorer, Word, Slack/Teams. Document that auto-paste into elevated (admin) windows needs Clipdump elevated too; plain copy always works.

### Phase 12: Settings persistence
1. `settings.rs`: serde struct with defaults, `%APPDATA%\clipdump\settings.json`. Commit.
2. Changing the hotkey re-registers it; show an error if taken. Commit.
3. Changing retention values applies immediately (runs retention once). Commit.

### Phase 13: Polish
1. Tray icon variant when paused. Commit.
2. Multi-monitor and DPI correctness. Commit.
3. Very long text: preview truncated in Rust; never send megabytes to the webview. Commit.
4. Graceful shutdown: flush writer thread on quit, on `WM_ENDSESSION` (Windows shutdown/restart) and on logoff so nothing is lost. Commit.
5. Corruption safety: if `history.db` fails to open, rename it to `history.corrupt-<ts>.db` and start fresh with a tray notification. Commit.
6. Accessibility: cmdk aria, visible focus ring. Commit.
7. App icon via `pnpm tauri icon`. Commit.

### Phase 14: Performance verification
1. `--perf` flag logs: hotkey to shown, IPC round trip, search time, ingest time per kind, OCR time, startup load time. Commit.
2. Pre-warm the hidden window at startup. Commit.
3. Stress script: push 10,000 mixed entries (text, 4K screenshots, file lists). App stays responsive, retention holds at 1,000, RAM stays under budget, disk cap enforced. Commit.
4. Restart test: reboot the PC, open the bar, confirm all items, pins and OCR search still work. Commit.
5. Record results in `docs/perf.md` against Section 4. Commit.
6. If keystroke latency is over budget, reduce result limit or switch to the threaded `nucleo` crate. Commit.

### Phase 15: Packaging (Windows)
1. `pnpm tauri build` with NSIS, per-user install (no admin). Commit.
2. WebView2 mode `downloadBootstrapper` (or `embedBootstrapper` for offline). Commit.
3. Uninstaller option "Also delete clipboard history". Commit.
4. `CHANGELOG.md`, version from `package.json`. Commit.
5. GitHub Actions `build-windows.yml`: on tag, `cargo test`, `pnpm tsc --noEmit`, `pnpm tauri build`, upload installer. Commit.
6. Note code signing as a future step (unsigned builds trigger SmartScreen).

### Phase 16: Later (do not build now, keep code ready)
- **macOS**: poll `NSPasteboard.changeCount` every 250 ms (no change notification exists), skip `org.nspasteboard.ConcealedType`, paste via CGEvent (Accessibility permission), OCR via Vision framework (`VNRecognizeTextRequest`), key in Keychain instead of DPAPI.
- **Linux**: X11 via XFixes selection events; Wayland via `ext-data-control` / `wl-clipboard`; paste via `wtype`/`xdotool`; OCR via Tesseract (`leptess`); key in Secret Service (libsecret).
- Rich text/HTML, snapshotting small files, snippets, sync.

## 7. Definition of done (v1, Windows)
- [ ] Starts at login, tray icon present, idle CPU ~0%.
- [ ] Hotkey shows bar in < 50 ms with input focused.
- [ ] Text, images, screenshots and files are all captured once each; password manager copies are not.
- [ ] Searching a word visible in a screenshot finds that screenshot (OCR).
- [ ] Ctrl+Enter copies any kind back correctly (images paste as images, files paste as files); Enter auto-pastes when enabled.
- [ ] Retention: never more than 1,000 unpinned items and nothing unpinned older than 30 days.
- [ ] Pinned items survive retention and "clear history".
- [ ] Delete single item and clear all (except pinned) both work and remove blob files from disk.
- [ ] RAM stays under budget with many large screenshots; blobs live on disk.
- [ ] History, pins and OCR text survive a PC restart; data on disk is encrypted.
- [ ] `cargo test` and `pnpm tsc --noEmit` pass.
- [ ] NSIS installer builds and installs without admin.
- [ ] `docs/perf.md` shows all budgets met.
