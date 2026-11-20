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
}

/** The fields `updateTask` can change; a field left out stays as it is. */
export interface TaskPatch {
  title?: string;
  note?: string;
  status?: TaskStatus;
  important?: boolean;
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
