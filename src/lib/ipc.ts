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
  /** Keep the bar visible; it does not hide on blur. */
  pinnedBar: boolean;
  /** Capture paused. */
  paused: boolean;
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
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/** Search the history. `limit` defaults to 50 on the backend (200 at most). */
export function search(
  query: string,
  filter: TypeFilter = "all",
  limit?: number,
): Promise<ResultDto[]> {
  return invoke<ResultDto[]>("search", { query, filter, limit });
}

export function getPreview(id: number): Promise<PreviewDto> {
  return invoke<PreviewDto>("get_preview", { id });
}

/**
 * Copy an item back to the clipboard and hide the bar; with `paste`, also paste
 * into the window that was focused before the bar opened. Rejects with a plain
 * message (use `errorMessage`). Until Phase 11 it always rejects.
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

export function getSettings(): Promise<SettingsDto> {
  return invoke<SettingsDto>("get_settings");
}

/** Applies at once (in memory; saved to disk from Phase 12). Resolves to the settings as stored. */
export function setSettings(settings: SettingsDto): Promise<SettingsDto> {
  return invoke<SettingsDto>("set_settings", { settings });
}

export function getStats(): Promise<StatsDto> {
  return invoke<StatsDto>("get_stats");
}

export function hideBar(): Promise<void> {
  return invoke<void>("hide_bar");
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
