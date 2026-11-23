// Typed bridge to the Rust backend (src-tauri/src/commands.rs, scheme.rs,
// events.rs). Every type here mirrors a serde struct there (camelCase fields,
// lowercase enum words). The Phase 10 UI imports only from this file, so a
// command or event never appears as a bare string anywhere else.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type ClipKind = "text" | "image" | "files";
/** The UI type filter (`Alt+1..4`). A typed `img:`, `file:`, `txt:` prefix in the query wins. */
export type TypeFilter = "all" | "text" | "images" | "files";
export type MatchSource = "text" | "ocr" | "file";
export type OcrStatus = "pending" | "done" | "failed" | "unavailable";

/** One search result row (`search`). Never carries full content. */
export interface ResultDto {
  id: number;
  kind: ClipKind;
  /** One line, 300 chars at most. */
  preview: string;
  /** Window of the matched text when the match is not visible in `preview`. */
  snippet: string | null;
  /** Sorted character (code point, not UTF-16) positions to highlight, in `snippet` if set, else `preview`. */
  matchIndices: number[];
  matchSource: MatchSource;
  /** Unix ms. */
  createdAt: number;
  useCount: number;
  pinned: boolean;
  width: number;
  height: number;
  byteSize: number;
  fileCount: number;
  missingFiles: number;
  /** Lines of a text item (1 for a single line), 0 for other kinds. */
  lineCount: number;
  hasThumb: boolean;
}

export interface FilePreview {
  path: string;
  /** Last path component. */
  name: string;
  size: number;
  isDir: boolean;
  exists: boolean;
}

/** The Tab pane content (`getPreview`), already capped in size by the backend. */
export type PreviewDto =
  | {
      kind: "text";
      id: number;
      /** At most 200 KB. */
      text: string;
      truncated: boolean;
      /** Characters of the whole text, not of `text`. */
      totalChars: number;
      byteSize: number;
    }
  | {
      kind: "image";
      id: number;
      width: number;
      height: number;
      byteSize: number;
      /** Recognized text, empty until OCR has finished. */
      ocrText: string;
      ocrTruncated: boolean;
      ocrStatus: OcrStatus;
    }
  | {
      kind: "files";
      id: number;
      /** The first 500 files. */
      files: FilePreview[];
      totalFiles: number;
      truncated: boolean;
      totalSize: number;
    };

/** Settings as `getSettings` returns and `setSettings` takes them (all fields required). */
export interface SettingsDto {
  maxTextBytes: number;
  maxImageBytes: number;
  ignoredApps: string[];
  /** 0 means no limit. */
  maxItems: number;
  /** 0 means no limit. */
  maxAgeDays: number;
  /** 0 means no limit. */
  diskCapBytes: number;
  /** 0 turns the RAM cache off. */
  ramCacheBytes: number;
  ocrEnabled: boolean;
  /** Enter copies and pastes into the window the bar came from; Ctrl+Enter and Ctrl+1..9 only copy. */
  autoPasteOnEnter: boolean;
  /** Keep the bar visible; it does not hide on blur. */
  pinnedBar: boolean;
  /** Capture paused. Not saved: the app always starts capturing. */
  paused: boolean;
  /** The global hotkey as chord text, for example "Ctrl+Shift+Space" (modifiers Ctrl, Alt, Shift, Super). */
  hotkey: string;
  /** Start the app when the user logs in. */
  startAtLogin: boolean;
  /** Base URL of an OpenAI compatible server for Split, e.g. "http://localhost:11434/v1". Empty: not set up. */
  llmBaseUrl: string;
  /** Model name sent to that server. */
  llmModel: string;
  /** Embedding model for semantic search in Ask (sends every note, task and clip to the server once). Empty: off. */
  llmEmbeddingModel: string;
  /** Whether an API key is stored. The key itself never comes back from the backend. */
  llmApiKeySet: boolean;
  /** Send a new key to store it, "" to remove the stored one, null to keep it. Always null when read. */
  llmApiKey: string | null;
}

/** One copyable piece of a split text: what it is, and the exact text to copy. */
export interface SplitPart {
  label: string;
  value: string;
}

export interface StatsDto {
  items: number;
  pinned: number;
  /** Estimated payload bytes on disk. */
  diskBytes: number;
  cacheBytes: number;
  cacheBudgetBytes: number;
  ocrPending: number;
  ocrUnavailable: number;
  ocrBacklog: number;
  ocrEngine: "unknown" | "available" | "unavailable";
  /** How to add a Windows OCR language. Show it when `ocrEngine` is "unavailable" or `ocrUnavailable > 0`. */
  ocrHint: string;
  /** How the data key is protected, e.g. "dpapi". */
  keyProtector: string;
  /** Set when the configured hotkey could not be registered (another app owns it), else null. */
  hotkeyError: string | null;
  /**
   * Set for the whole run when the history was damaged (or its key unreadable) at startup and
   * was moved aside: what happened, the folder and the new file names. Show it until dismissed.
   */
  recovery: string | null;
  /** The app runs with `--perf`: send the timings of `perfAck`. False in normal use. */
  perf: boolean;
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/**
 * Whether the backend runs with `--perf` (`getStats().perf`). Off in normal use, and then the
 * only cost of the perf support is this flag being read: no timer, no extra IPC.
 */
let perfOn = false;
export function setPerf(on: boolean): void {
  perfOn = on;
}
export const perfEnabled = (): boolean => perfOn;

/** Report a timing or a paint to the `--perf` log (`perf_ack`). Only call it when `perfEnabled()`. */
export function perfAck(kind: "barPainted" | "searchRtt", ms?: number): void {
  invoke<void>("perf_ack", { kind, ms }).catch(() => {});
}

/** Run `fn` after the next frame has been produced (two animation frames), as the bar is painted. */
export function afterPaint(fn: () => void): void {
  requestAnimationFrame(() => requestAnimationFrame(fn));
}

/** Search the history. `limit` defaults to 50 on the backend (200 at most). */
export function search(
  query: string,
  filter: TypeFilter = "all",
  limit?: number,
): Promise<ResultDto[]> {
  if (!perfOn) return invoke<ResultDto[]>("search", { query, filter, limit });
  // `--perf` only: the round trip as the webview sees it (the backend logs its own share).
  const t0 = performance.now();
  return invoke<ResultDto[]>("search", { query, filter, limit }).then((r) => {
    perfAck("searchRtt", performance.now() - t0);
    return r;
  });
}

export function getPreview(id: number): Promise<PreviewDto> {
  return invoke<PreviewDto>("get_preview", { id });
}

/**
 * Copy an item back to the clipboard and hide the bar; with `paste`, also paste
 * into the window that was focused before the bar opened. Rejects with a plain
 * message (use `errorMessage`). A message that starts with "copied, but" means the
 * item is on the clipboard and only the auto-paste failed.
 */
export function copyItem(id: number, paste = false): Promise<void> {
  return invoke<void>("copy_item", { id, paste });
}

/** Resolves to whether the item existed (deleting a gone item is not an error). */
export function deleteItem(id: number): Promise<boolean> {
  return invoke<boolean>("delete_item", { id });
}

/** Resolves to the new pinned state. */
export function togglePin(id: number): Promise<boolean> {
  return invoke<boolean>("toggle_pin", { id });
}

/** Delete every unpinned item. Resolves to how many were removed. */
export function clearHistory(): Promise<number> {
  return invoke<number>("clear_history");
}

/** Where the split of one text item stands (`requestSplit`, `getSplit`). */
export type SplitStatus =
  /** Never asked for. */
  | { status: "none" }
  /** Waiting in the queue (two LLM calls run at a time). */
  | { status: "queued" }
  | { status: "running" }
  | { status: "done"; parts: SplitPart[] }
  | { status: "error"; message: string };

/**
 * Ask for a text item to be split into copyable pieces by the configured LLM server, and get where
 * it stands. The work happens in a backend queue; `onSplitUpdated` fires when it moves. Asking
 * again for a finished item returns the stored result; `force` runs it again (re-split). Rejects
 * with a plain message when Split is not set up or the item is not text.
 */
export function requestSplit(id: number, force = false): Promise<SplitStatus> {
  return invoke<SplitStatus>("request_split", { id, force });
}

/** Where the split of an item stands, without starting one. */
export function getSplit(id: number): Promise<SplitStatus> {
  return invoke<SplitStatus>("get_split", { id });
}

/** Put one piece on the clipboard. The bar stays open and nothing is added to the history. */
export function copyText(text: string): Promise<void> {
  return invoke<void>("copy_text", { text });
}

export function getSettings(): Promise<SettingsDto> {
  return invoke<SettingsDto>("get_settings");
}

/**
 * Applies at once and saves to settings.json in the background. Resolves to the settings as stored
 * (a canonical hotkey, a tidied ignore list). Rejects with a plain message (use `errorMessage`)
 * when nothing was applied: for example a hotkey another app owns, or a login item the OS refuses.
 */
export function setSettings(settings: SettingsDto): Promise<SettingsDto> {
  return invoke<SettingsDto>("set_settings", { settings });
}

// ---------------------------------------------------------------------------
// Tasks (src-tauri/src/tasks.rs, commands/tasks.rs)
// ---------------------------------------------------------------------------

export type TaskStatus = "todo" | "doing" | "done";
/** How long a task takes, as estimated by the LLM: about 15 minutes or less, up to an hour, longer. */
export type TaskSize = "quick" | "short" | "long";
/** What kind of work a task is: small and mechanical, needs a block of focus, or blocked on someone else. */
export type TaskKind = "quick" | "deep" | "waiting";

export interface Task {
  id: number;
  title: string;
  /** Where the work was left off. Empty when there is nothing to say. */
  note: string;
  status: TaskStatus;
  important: boolean;
  /** Unix ms. */
  createdAt: number;
  /** Unix ms of the last change of any kind. */
  touchedAt: number;
  /** Unix ms, set while the task is done. */
  doneAt: number | null;
  /** The LLM's estimate, null until it has made one (or when no LLM server is set up). */
  size: TaskSize | null;
  kind: TaskKind | null;
  /** A concrete first step suggested by the LLM. Empty when there is none. */
  nextStep: string;
  /** History ids of the clipboard items linked to this task. One may have left the history since. */
  clipIds: number[];
  /** Unix ms. Until then the task is not nagged about as gone quiet. */
  deferredUntil: number | null;
}

/** The fields `updateTask` can change; a field left out stays as it is. */
export interface TaskPatch {
  title?: string;
  note?: string;
  status?: TaskStatus;
  important?: boolean;
  /** null clears the estimate. */
  size?: TaskSize | null;
  kind?: TaskKind | null;
  /** "" clears the suggestion. */
  nextStep?: string;
  /** Replaces the linked items (20 at most). */
  clipIds?: number[];
  /** null clears the deferral. */
  deferredUntil?: number | null;
}

/** Every task, in the order they were added. */
export function listTasks(): Promise<Task[]> {
  return invoke<Task[]>("list_tasks");
}

/** Add a task; `start` puts it in flight at once. Rejects with a plain message (use `errorMessage`). */
export function addTask(title: string, important = false, start = false): Promise<Task> {
  return invoke<Task>("add_task", { title, important, start });
}

/** Resolves to the task as stored. */
export function updateTask(id: number, patch: TaskPatch): Promise<Task> {
  return invoke<Task>("update_task", { id, patch });
}

/** Resolves to whether the task existed. */
export function deleteTask(id: number): Promise<boolean> {
  return invoke<boolean>("delete_task", { id });
}

// ---------------------------------------------------------------------------
// The task assistant (src-tauri/src/taskai.rs, commands/task_ai.rs). Every call below is one round
// trip to the LLM server set up in Settings and rejects with a plain message (use `errorMessage`)
// when there is none or it fails. They only suggest: nothing changes a task except `estimateTask`
// and `suggestNextStep`, which keep their answer on it.
// ---------------------------------------------------------------------------

/** How much energy there is, which steers "what next" toward lighter or heavier tasks. */
export type Energy = "low" | "normal" | "high";

export interface TaskPick {
  id: number;
  /** One short sentence: why this one, now. */
  reason: string;
}

export interface NextAction {
  /** The first concrete step. Also kept on the task as `nextStep`. */
  step: string;
  /** The steps after it, for a big task. Empty for a small one. They become tasks only when accepted. */
  subtasks: string[];
}

export interface TaskDigest {
  summary: string;
  /** Ids of open tasks, best first. */
  order: number[];
}

export interface TaskLink {
  taskId: number;
  clipId: number;
  reason: string;
}

export interface ExtractedTask {
  title: string;
  important: boolean;
  /** The id of an open task that already says the same, else null. */
  duplicateOf: number | null;
}

export interface ExtractResult {
  /** The start of the text the tasks were taken from. */
  source: string;
  tasks: ExtractedTask[];
}

export interface StaleVerdict {
  id: number;
  verdict: "do" | "defer" | "drop";
  reason: string;
}

/** A clipboard item linked to a task, enough to list it and copy it back. */
export interface ClipRef {
  id: number;
  kind: ClipKind;
  preview: string;
  createdAt: number;
}

/** Estimate (or estimate again) a task now. Resolves to the task with its size and kind. */
export function estimateTask(id: number): Promise<Task> {
  return invoke<Task>("estimate_task", { id });
}

/** Queue estimates for the open tasks that have none. Resolves to how many; `onTasksUpdated` fires as each finishes. */
export function estimateBacklog(): Promise<number> {
  return invoke<number>("estimate_backlog");
}

/** "What should I do now?" Null when nothing is open. `minutes` null means not said. */
export function pickNextTask(minutes: number | null, energy: Energy): Promise<TaskPick | null> {
  return invoke<TaskPick | null>("pick_next_task", { minutes, energy });
}

/** A concrete first step for a task (kept on it) and, for a big one, subtasks. */
export function suggestNextStep(id: number): Promise<NextAction> {
  return invoke<NextAction>("suggest_next_step", { id });
}

/** A draft of where the task was left off, from what was copied since. Not saved: the caller decides. */
export function draftResumeNote(id: number): Promise<string> {
  return invoke<string>("draft_resume_note", { id });
}

/** The morning briefing or the week's recap. */
export function taskDigest(kind: "morning" | "week"): Promise<TaskDigest> {
  return invoke<TaskDigest>("task_digest", { kind });
}

/** Which recent clipboard items belong to which open task. */
export function suggestTaskLinks(): Promise<TaskLink[]> {
  return invoke<TaskLink[]>("suggest_task_links");
}

/** Action items in `text`; with no text, in the newest copied text. */
export function extractTasks(text: string | null): Promise<ExtractResult> {
  return invoke<ExtractResult>("extract_tasks", { text });
}

/** What to do about each of the quiet tasks: do it, defer it or drop it. */
export function reviewStaleTasks(ids: number[]): Promise<StaleVerdict[]> {
  return invoke<StaleVerdict[]>("review_stale_tasks", { ids });
}

/** The clipboard items still in the history among `ids`, in that order. No LLM involved. */
export function getClips(ids: number[]): Promise<ClipRef[]> {
  return invoke<ClipRef[]>("get_clips", { ids });
}

// ---------------------------------------------------------------------------
// Ask (src-tauri/src/ask.rs, commands/ask.rs)
// ---------------------------------------------------------------------------

export type AskSource = "note" | "task" | "clip";

/** One excerpt that was sent to the model. */
export interface AskHit {
  /** The number the answer cites this excerpt by (`[2]`). Not its position in the list. */
  n: number;
  source: AskSource;
  /** The note, task or history item id. */
  id: number;
  /** Unix ms. */
  createdAt: number;
  title: string;
  /** Exactly the text the model saw. */
  excerpt: string;
  /** The verbatim quote that backs the answer; only on a cited hit. */
  quote: string | null;
  cited: boolean;
}

/** What was searched, so a wrong guess about the time or the words is visible. */
export interface AskSearched {
  /** `YYYY-MM-DD` both, or null when the question named no time. */
  from: string | null;
  to: string | null;
  terms: string[];
  sources: AskSource[];
  /** Items looked through. */
  scanned: number;
  /** Whether items were also ranked by meaning (an embedding model is set up). */
  semantic: boolean;
  /** Searches made: 1, plus one for each time the model searched again with other words. */
  rounds: number;
}

export interface AskAnswer {
  /** False when nothing verified answers the question; `answer` then says so. */
  found: boolean;
  answer: string;
  /** Every excerpt that was sent, the cited ones first. */
  hits: AskHit[];
  searched: AskSearched;
}

/**
 * Answer a question from the notes, the tasks and the clipboard history with the LLM server set up
 * in Settings. Two round trips; nothing but the question and a few excerpts leaves the machine.
 */
export function askEverything(question: string): Promise<AskAnswer> {
  return invoke<AskAnswer>("ask_everything", { question });
}

// ---------------------------------------------------------------------------
// Notes (src-tauri/src/notes.rs, commands/notes.rs)
// ---------------------------------------------------------------------------

export interface Note {
  id: number;
  /** Optional: empty when the note has none. */
  title: string;
  body: string;
  pinned: boolean;
  /** Lowercase, no spaces, no duplicates. */
  tags: string[];
  /** Unix ms. */
  createdAt: number;
  /** Unix ms of the last change of any kind. */
  updatedAt: number;
}

/** The fields `updateNote` can change; a field left out stays as it is. */
export interface NotePatch {
  title?: string;
  body?: string;
  pinned?: boolean;
  tags?: string[];
}

/** Every note, in the order they were added. */
export function listNotes(): Promise<Note[]> {
  return invoke<Note[]>("list_notes");
}

/** Add a note; every field may be empty. Rejects with a plain message (use `errorMessage`). */
export function addNote(title = "", body = "", tags: string[] = []): Promise<Note> {
  return invoke<Note>("add_note", { title, body, tags });
}

/** Resolves to the note as stored. */
export function updateNote(id: number, patch: NotePatch): Promise<Note> {
  return invoke<Note>("update_note", { id, patch });
}

/** Resolves to whether the note existed. */
export function deleteNote(id: number): Promise<boolean> {
  return invoke<boolean>("delete_note", { id });
}

export function getStats(): Promise<StatsDto> {
  return invoke<StatsDto>("get_stats");
}

/** Open the history folder in the file manager (the path is fixed in the backend). */
export function openDataFolder(): Promise<void> {
  return invoke<void>("open_data_folder");
}

export function hideBar(): Promise<void> {
  return invoke<void>("hide_bar");
}

/**
 * Hand the mouse to the OS window drag (call it on mousedown over a drag handle). The backend
 * remembers where the bar is dropped, per monitor, and opens it there next time.
 */
export function startDrag(): Promise<void> {
  return invoke<void>("start_drag");
}

/** Backend errors are plain strings; anything else is stringified. */
export function errorMessage(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

// ---------------------------------------------------------------------------
// Events (names mirror src-tauri/src/events.rs)
// ---------------------------------------------------------------------------

export const EVENTS = {
  barShown: "bar:shown",
  barHidden: "bar:hidden",
  /** Debounced to one per 50 ms. Re-run the search. */
  historyChanged: "history:changed",
  settingsOpen: "settings:open",
  /** Settings changed behind the UI's back (the tray toggled Pause capture): call `getSettings` again. */
  settingsChanged: "settings:changed",
  /** The history was moved aside at startup: call `getStats`, its `recovery` field has the sentence. */
  historyRecovered: "history:recovered",
  /** A split job started or finished: call `getSplit` again for the item on screen. */
  splitUpdated: "split:updated",
  /** A task changed behind the UI's back (the background estimator finished one): call `listTasks` again. */
  tasksUpdated: "tasks:updated",
} as const;

type Handler = () => void;

function on(event: string, handler: Handler): Promise<UnlistenFn> {
  return listen(event, () => handler());
}

/** The bar became visible: focus and select the input. */
export const onBarShown = (h: Handler) => on(EVENTS.barShown, h);
export const onBarHidden = (h: Handler) => on(EVENTS.barHidden, h);
export const onHistoryChanged = (h: Handler) => on(EVENTS.historyChanged, h);
/** The tray asked for the settings view. */
export const onSettingsOpen = (h: Handler) => on(EVENTS.settingsOpen, h);
export const onSettingsChanged = (h: Handler) => on(EVENTS.settingsChanged, h);
export const onHistoryRecovered = (h: Handler) => on(EVENTS.historyRecovered, h);
export const onSplitUpdated = (h: Handler) => on(EVENTS.splitUpdated, h);
export const onTasksUpdated = (h: Handler) => on(EVENTS.tasksUpdated, h);

// ---------------------------------------------------------------------------
// Thumbnails and images: the `clip` URI scheme
// ---------------------------------------------------------------------------

/**
 * The origin of the `clip` scheme. Tauri serves a custom scheme as
 * `http://clip.localhost` on Windows (WebView2) and Android, and as
 * `clip://localhost` on macOS and Linux.
 */
export function clipOrigin(windowsLike: boolean = isWindowsLike()): string {
  // `vite --mode mock` (browser, fake backend): the dev server draws the pictures.
  // The constant folds away in production builds.
  if (import.meta.env.MODE === "mock") return "/mock-clip";
  return windowsLike ? "http://clip.localhost" : "clip://localhost";
}

/** True where Tauri uses the `http://<scheme>.localhost` form (Windows, Android). */
export function isWindowsLike(): boolean {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  return ua.includes("Windows") || ua.includes("Android");
}

function clipUrl(
  resource: "thumb" | "blob",
  id: number,
  version?: number,
  windowsLike?: boolean,
): string {
  // The content behind an id never changes, so the backend sends year long
  // immutable cache headers. Ids restart from max(id) + 1 after a restart, so a
  // deleted newest item can hand its id to another one; `createdAt` in the query
  // makes that a different URL. The backend ignores the query.
  const v = version === undefined ? "" : `?v=${version}`;
  return `${clipOrigin(windowsLike)}/${resource}/${id}${v}`;
}

/** WebP thumbnail (96 px tall at most), from RAM. Pass the row's `createdAt` as `version`. */
export function thumbUrl(id: number, version?: number, windowsLike?: boolean): string {
  return clipUrl("thumb", id, version, windowsLike);
}

/** Full size PNG of an image item. Pass the row's `createdAt` as `version`. */
export function blobUrl(id: number, version?: number, windowsLike?: boolean): string {
  return clipUrl("blob", id, version, windowsLike);
}
