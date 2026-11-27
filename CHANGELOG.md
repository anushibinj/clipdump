# Changelog

All notable changes to Clipdump are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). The version is the one in `package.json`;
`src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` must say the same
(`pnpm check:version` checks all of it).

## [Unreleased]

## [0.11.0] - 2026-11-27

### Added

- Delegations: marking one done now keeps it, with its notes, under a collapsible Done list. From there it can be
  reopened or deleted for good. Done work never counts as overdue and never alerts.
- Delegations: a section in the shortcuts help.

### Changed

- Delegations: removing a team member now also removes their done delegations (their open ones still block it).

## [0.10.0] - 2026-11-27

### Added

- A Delegations tab (`Ctrl+4`) to follow up on work handed to the team. Every delegation needs a follow-up time.
- A fixed Team list, edited in the tab, to pick the assignee from. Delegations are grouped by person; people with
  nothing delegated are not shown.
- A sidebar per delegation for notes, reassigning, and postponing the deadline (1 day, 1 week, or a chosen time).
- A sound and a system notification when a deadline passes, and a red circle on the tab icon with the number of
  overdue delegations.

## [0.9.0] - 2026-11-23

### Added

- While a task is locked in a focus session, its notes now show inside the lock, laid out as the title, then
  notes (70%) beside the timer (30%), then parking.
- The remaining tasks are collapsed while the focus lock is on; a toggle docked at the bottom expands them.

### Changed

- The locked panel stretches to fill the window while the other tasks are collapsed.

### Fixed

- The timer panel in the focus lock is centered.
- The quiet-task nudge is hidden while the focus lock is on.

## [0.8.0] - 2026-11-23

### Added

- A 1 px progress bar right below the title bar while a task is locked in a focus session: full when the
  session starts, empty when it ends, in the theme's destructive (red) color.
- The time left in a focus session (mm:ss) is shown next to the Tasks icon in the title bar.
- A Copy button appears on a result row when you hover it; click it to copy that item.

### Changed

- Clicking a result row now only selects it (it no longer copies). Use the hover Copy button, Enter, or
  Alt+digit to copy.

## [0.7.0] - 2026-11-23

### Added

- A Settings gear button in the footer, right of Shortcuts; click it again to go back to search.

### Changed

- Tiny clipboard: Enter and a click now paste into the window the popup came from (when "Paste on Enter" is on),
  like the main bar. Alt+digit still only copies.
- The preview pane is always full-width below the results, capped at 30% of the height, instead of moving
  beside the list in a wide window.

## [0.6.0] - 2026-11-23

### Added

- "Split work with AI" under a task's notes: the assistant proposes tasks from what the notes say, each with a
  checkmark and an editable title and note; only the ticked ones are created.
- The main window can be resized, and the zoom hotkeys work in it.
- When the window is narrow, the preview pane stacks under the results instead of squeezing them.
- Tiny clipboard: the popup loads the last 50 items in recency order (pinned ones first) and scrolls
  through them, with 3 rows visible at a time.

### Changed

- Hovering a task or a clipboard result no longer moves the selection; click to select (a click on a result still copies it).
- Search: the empty query can list pinned items in recency order (`pinned_first`), used by the tiny clipboard.

## [0.5.1] - 2026-11-23

### Fixed

- Tiny clipboard: the popup's copy, close and pin buttons were blocked by the default capability, so
  they did nothing. They are allowed now.
- Ask: a follow-up question no longer shadows the clipboard history while the answer is being built,
  so conversational follow-ups pair the previous question with the new one correctly when ranking by
  meaning.
- Ask: the embedding query is built without `format!`, which keeps the question text out of
  format-string handling.

## [0.5.0] - 2026-11-23

### Added

- Tiny clipboard: press `Ctrl` twice anywhere (Windows) to open a minimal popup next to the text
  cursor, or next to the mouse pointer in apps that do not expose one (browsers, Electron apps).
  It shows the three best matches while you type; `Enter` or a click copies, `Up`/`Down` move,
  `Alt+1..3` copy a row, `Esc` closes. `Ctrl` twice again also closes it. The pin button keeps it
  open after a copy and when you click elsewhere. A setting, **Tiny clipboard on Ctrl twice**, turns
  the shortcut off.
- Focus lock banner has a **Done** button to finish a session early.
- Each task section title has a one-line helper text beside it.
- Ask is a conversation. A follow-up ("and who else was there?") is sent with the last three questions
  and answers, so the model can work out what it means; the thread stays on screen above the new
  answer and `Esc` clears it.
- Ask can search by meaning. Set an **Embedding model** in Settings (for example `nomic-embed-text`) and
  the items nearest in meaning to the question join the word hits in the ranking, so "what did my boss
  want" finds "Priya asked for the Q4 numbers". It is off until you set the model, because the text of
  every note, task and clipboard item is sent to your server once to be embedded. The vectors are kept
  in memory only (never on disk); the newest 800 items are embedded on the first question and the rest
  on the next ones.
- Ask can dig. When the excerpts are not enough, the model may search again with other words and dates
  (twice at most), or open an item in full; new items are added after the old ones so numbers never
  shift. A first search that finds nothing is retried once, told which words failed. The line under an
  answer says how many searches it took and whether meaning was used.
- Ask answers summaries ("what did I copy this week?"): a `summary` answer may cite whole items, shown
  without a highlighted quote.

### Changed

- Ask sends up to 40 excerpts of 1,000 characters (was 25 of 600), within a 24,000 character prompt.
- An answer's quote is checked ignoring punctuation as well as case and whitespace, so a dropped full
  stop or a curly quote no longer turns a right answer into "not found". A fact still needs a quote
  that occurs in its item; only a summary may cite without one.

## [0.4.0] - 2026-11-23

### Added

- A title bar above every view: it shows the app version, is the only place to drag the window, and
  holds the mode icons (clipboard, notes, tasks), the pin and the close button. The per-view pin,
  close and back buttons are gone.

- The shortcuts dialog has a search box, focused when it opens. `Ctrl+/` and `Ctrl+?` both open it.

- `Ctrl+1`, `Ctrl+2`, `Ctrl+3` switch between the modes: clipboard, notes, tasks (the order of the title bar
  icons). Works from every view. A new mode takes the next number (`MODE_ORDER` in `lib/shortcuts.ts`).

- `Alt+/` or `Alt+?` opens Ask with the caret in its input, from every view.

- The mode keys (`Ctrl+1..3`) and the filter keys (`Alt+Shift+1..4`) are shown as hints in the UI.

### Changed

- The clipboard type filters moved from `Alt+1..4` to `Alt+Shift+1..4`. Copying the Nth result moved from `Ctrl+1..9` to `Alt+1..9`, to free `Ctrl+digit` for the modes.
- The Ask button moved to the bottom-left footer and is no longer in the filter row. The old Ask
  shortcut is dropped in favor of `Alt+/`.

### Fixed

- Hovering with the mouse no longer scrolls the result list or the task list when it moves the selection.
- Removed the transparent padding around the window card.

### Internal

- CI: Rust caches are saved from `master` with fixed keys, and a cache-warm workflow fills them.
- The package version is exposed to the UI as `__APP_VERSION__`.

## [0.3.0] - 2026-11-23

### Added

- Ask. The Ask button at the bottom left of the window opens a view where one question
  is answered from your notes, tasks and clipboard history ("what was that thing my manager told me 2
  weeks ago?"), with the LLM server already set up for AI split. No vector database or embeddings: the
  history is small and in memory, so the work is in the steps around the model. (1) The model turns the
  question and today's date into a date range, search words with synonyms and the sources to look in;
  an unreadable plan falls back to the question's own words. (2) The app ranks the items itself: word
  hits inside the range, items of the range by recency, and word hits outside it at a discount, so a
  wrong date costs rank, not the answer. At most 25 excerpts of 600 characters are kept. (3) Only those
  excerpts, numbered and dated, go to the model, which must say "not found" when they do not answer.
  (4) The answer is shown only when it cites an excerpt and quotes it word for word (case and
  whitespace aside); a claim whose quote is not in the excerpt is dropped, and an answer left with no
  verified source becomes "I could not find that". Cited sources show the highlighted quote; the other
  excerpts that were sent are listed under "Also sent to the model", and the line under the answer says
  which dates, sources and how many items were searched. Nothing is sent until you press Enter, and
  only the question and those excerpts go, only to your own server; clips copied from ignored apps are
  never captured, so they are never searched. Esc clears the text, then the answer, then closes the bar.

- Notes. `Ctrl+Shift+N` (or the Notes button in the search view's filter row) opens a notes view next to the
  clipboard search and the tasks. Keep as many notes as you like; every one has an optional title, free
  tags and a pin (pinned notes stay on top). No folders or categories, only tags. `Ctrl+Enter` makes a new
  note, `Up`/`Down` pick one, `Ctrl+E` writes in it, `Ctrl+G` adds tags, `Ctrl+P` pins, `Ctrl+Delete` twice
  deletes. The filter box matches the title, the text and the tags; a `#tag` word (or a click on a tag
  chip) keeps only notes with that tag. Edits save by themselves; a new note left empty is dropped. The bar
  reopens on the notes if that is where it was left. Notes are stored encrypted in their own file
  (`notes.bin`), so "Clear history" never touches them. Limits: 1,000 notes, titles 120 characters, text
  20,000, 10 tags of 32 characters.

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
- A task assistant in the task list, using the LLM server already set up for AI split. It only
  suggests; nothing changes until you accept it.
- `Ctrl+N` answers "what should I do now?" with one task and a reason, for the time (10 minutes,
  30, an hour or any) and the energy (low, normal, high) you have. `Alt+Enter` starts it.
- Every task gets an effort estimate in the background (about 15 minutes, up to an hour, or longer,
  and whether it is quick, needs focus or is waiting on someone), shown as a small chip on the
  row. `Ctrl+Q` shows only the quick wins, for when there are ten minutes to spare.
- `Ctrl+G` turns a vague task into a concrete first step, shown under its title, and for a big task
  offers subtasks to add with one key.
- `Ctrl+R` drafts "where I left off" from what you copied since you last touched the task. It is a
  draft in the notes sidebar: you take it or dismiss it.
- `Ctrl+D` writes today's digest and `Ctrl+Shift+D` the week's recap, each with the order to start in.
- `Ctrl+J` suggests which recent clipboard items belong to which task. Linked items show in the
  notes sidebar, where you can copy one back or unlink it.
- `Ctrl+K` finds the tasks in a pasted text, or in the text you just copied, and flags the ones that
  are already on the list.
- A task that has gone quiet (in flight for 3 days, waiting for 14) gets a "still relevant?" card:
  start it, set it aside for a week (`Ctrl+B`), drop it, or ask again tomorrow. It works without an
  LLM server; the assistant can add a recommendation, and never recommends dropping an important task.
- `Alt+Enter` takes the main suggestion of whichever card is open.

### Changed

- Estimates and suggestions do not count as touching a task, so a task you left alone still goes quiet.
- `Tab` now cycles between the clipboard search and the tasks; the preview pane moves to `Alt+P`.
- The Settings tab "Split with AI" is now called "AI", with copy that covers the task assistant, notes
  and Ask as well as AI split (they all use the same LLM server).
- Tasks keep a size, kind, next step, linked clips and a set-aside date, stored with the task.
- Task notes show in a sidebar inside the tasks view, with a notes button on each row.

### Internal

- Release installers are built for every platform by one matrix workflow (`release.yml`), replacing the
  Windows-only build.

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
