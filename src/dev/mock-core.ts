// Browser stand-in for `@tauri-apps/api/core`: `invoke` runs the fake backend.
// Swapped in by the `mock` Vite mode (vite.config.ts). Never part of a
// production build. Exposes `window.__clipMock` for tests and manual poking.

import { generateItems, type MockItem } from "./data";
import { emit } from "./mock-event";

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

interface Row extends MockItem {
  /** What the backend orders by (copying an item moves it to the top). */
  usedAt: number;
  lower: string;
}

const NOW = Date.now();
let rows: Row[] = [];
let nextId = 1;

function toRow(it: MockItem): Row {
  const searchable = it.kind === "files" ? it.files.map((f) => f.path).join("\n") : it.text;
  return { ...it, usedAt: it.createdAt, lower: searchable.slice(0, 10_000).toLowerCase() };
}

function load(count: number) {
  rows = generateItems(count, NOW).map(toRow);
  nextId = count + 1;
}

const DEFAULT_SETTINGS = {
  maxTextBytes: 5 * 1024 * 1024,
  maxImageBytes: 50 * 1024 * 1024,
  ignoredApps: ["keepassxc.exe"],
  maxItems: 1000,
  maxAgeDays: 30,
  diskCapBytes: 2 * 1024 ** 3,
  ramCacheBytes: 64 * 1024 * 1024,
  ocrEnabled: true,
  pinnedBar: false,
  paused: false,
};
let settings = { ...DEFAULT_SETTINGS };

const control = {
  /** Every command the page called, in order. */
  calls: [] as { cmd: string; args: unknown }[],
  /** Simulated IPC round trip per call, in ms (0 = resolve on the next microtask). */
  latencyMs: 0,
  /** `copy_item` rejects with this message when set (the real one rejects on Windows until Phase 11). */
  copyError: null as string | null,
  /** `get_stats` reports an OCR engine without a language. */
  ocrUnavailable: false,
};

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

type Filter = "all" | "text" | "images" | "files";

function parsePrefix(query: string, filter: Filter): { text: string; filter: Filter } {
  const m = /^(img|file|txt):\s*/i.exec(query);
  if (!m) return { text: query, filter };
  const f: Filter = m[1].toLowerCase() === "img" ? "images" : m[1].toLowerCase() === "file" ? "files" : "text";
  return { text: query.slice(m[0].length), filter: f };
}

function kindOf(f: Filter): MockItem["kind"] | null {
  return f === "text" ? "text" : f === "images" ? "image" : f === "files" ? "files" : null;
}

/** Positions (code points) of every `term` occurrence inside `hay`, case insensitive. */
function indicesOf(hay: string, terms: string[]): number[] {
  const chars = Array.from(hay);
  const lower = chars.map((c) => c.toLowerCase());
  const out = new Set<number>();
  for (const term of terms) {
    const t = Array.from(term.toLowerCase());
    if (t.length === 0) continue;
    outer: for (let i = 0; i + t.length <= lower.length; i += 1) {
      for (let j = 0; j < t.length; j += 1) if (lower[i + j] !== t[j]) continue outer;
      for (let j = 0; j < t.length; j += 1) out.add(i + j);
    }
  }
  return Array.from(out).sort((a, b) => a - b);
}

function snippetAround(text: string, term: string): string {
  const flat = text.replace(/\s+/g, " ");
  const at = flat.toLowerCase().indexOf(term.toLowerCase());
  const chars = Array.from(flat);
  // Convert the UTF-16 index to a code point index.
  const pos = at < 0 ? 0 : Array.from(flat.slice(0, at)).length;
  const start = Math.max(0, pos - 24);
  return chars.slice(start, start + 96).join("").trim();
}

function search(query: string, filter: Filter, limit = 50) {
  const parsed = parsePrefix(query, filter);
  const terms = parsed.text.split(/\s+/).filter(Boolean);
  const kind = kindOf(parsed.filter);
  let hits: { row: Row; score: number }[] = [];
  for (const row of rows) {
    if (kind && row.kind !== kind) continue;
    let score = 0;
    if (terms.length > 0) {
      let ok = true;
      for (const term of terms) {
        const needle = term.toLowerCase();
        const at = row.lower.indexOf(needle);
        if (at < 0) {
          ok = false;
          break;
        }
        score += 100 - Math.min(at, 90);
      }
      if (!ok) continue;
    }
    hits.push({ row, score });
  }
  if (terms.length === 0) {
    hits.sort((a, b) => Number(b.row.pinned) - Number(a.row.pinned) || b.row.usedAt - a.row.usedAt);
  } else {
    const newest = NOW;
    hits.forEach((h) => {
      h.score += Math.max(0, 20 - (newest - h.row.usedAt) / 3_600_000) + Math.log(1 + h.row.useCount);
    });
    hits.sort((a, b) => b.score - a.score || b.row.usedAt - a.row.usedAt);
  }
  hits = hits.slice(0, Math.min(limit, 200));
  return hits.map(({ row }) => {
    let snippet: string | null = null;
    let matchIndices: number[] = [];
    if (terms.length > 0) {
      matchIndices = row.kind === "image" ? [] : indicesOf(row.preview, terms);
      if (matchIndices.length === 0) {
        const hay = row.kind === "files" ? row.files.map((f) => f.path).join(" ") : row.text;
        snippet = snippetAround(hay, terms[0]);
        matchIndices = indicesOf(snippet, terms);
      }
    }
    const missing = row.files.filter((f) => !f.exists).length;
    return {
      id: row.id,
      kind: row.kind,
      preview: row.preview,
      snippet,
      matchIndices,
      matchSource: row.kind === "text" ? "text" : row.kind === "image" ? "ocr" : "file",
      createdAt: row.createdAt,
      useCount: row.useCount,
      pinned: row.pinned,
      width: row.width,
      height: row.height,
      byteSize: row.byteSize,
      fileCount: row.files.length,
      missingFiles: missing,
      lineCount: row.kind === "text" ? row.text.replace(/\n+$/, "").split("\n").length : 0,
      hasThumb: row.hasThumb,
    };
  });
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

function changed() {
  setTimeout(() => emit("history:changed"), 0);
}

function find(id: number): Row {
  const row = rows.find((r) => r.id === id);
  if (!row) throw "the item no longer exists";
  return row;
}

function stats() {
  const unavailable = control.ocrUnavailable;
  return {
    items: rows.length,
    pinned: rows.filter((r) => r.pinned).length,
    diskBytes: rows.reduce((a, r) => a + (r.kind === "image" ? r.byteSize : r.kind === "text" ? r.byteSize : 600), 0),
    cacheBytes: 18_400_000,
    cacheBudgetBytes: settings.ramCacheBytes,
    ocrPending: unavailable ? 3 : 0,
    ocrUnavailable: unavailable ? 3 : 0,
    ocrBacklog: 0,
    ocrEngine: unavailable ? "unavailable" : "available",
    ocrHint: unavailable
      ? "No OCR language is installed. Add one in Windows Settings > Time & language > Language & region, then restart Clipdump."
      : "",
    keyProtector: "dpapi",
  };
}

function run(cmd: string, args: Record<string, unknown>): unknown {
  switch (cmd) {
    case "search":
      return search(String(args.query ?? ""), (args.filter as Filter) ?? "all", args.limit as number | undefined);
    case "get_preview": {
      const row = find(Number(args.id));
      if (row.kind === "text") {
        const text = row.text.slice(0, 200_000);
        return { kind: "text", id: row.id, text, truncated: text.length < row.text.length, totalChars: row.text.length, byteSize: row.byteSize };
      }
      if (row.kind === "image") {
        return { kind: "image", id: row.id, width: row.width, height: row.height, byteSize: row.byteSize, ocrText: row.text, ocrTruncated: false, ocrStatus: "done" };
      }
      return { kind: "files", id: row.id, files: row.files, totalFiles: row.files.length, truncated: false, totalSize: row.byteSize };
    }
    case "copy_item": {
      const row = find(Number(args.id));
      if (control.copyError) throw control.copyError;
      row.usedAt = Date.now();
      row.useCount += 1;
      setTimeout(() => emit("bar:hidden"), 0);
      changed();
      return null;
    }
    case "delete_item": {
      const before = rows.length;
      rows = rows.filter((r) => r.id !== Number(args.id));
      changed();
      return rows.length !== before;
    }
    case "toggle_pin": {
      const row = find(Number(args.id));
      row.pinned = !row.pinned;
      changed();
      return row.pinned;
    }
    case "clear_history": {
      const before = rows.length;
      rows = rows.filter((r) => r.pinned);
      changed();
      return before - rows.length;
    }
    case "get_settings":
      return { ...settings };
    case "set_settings": {
      const next = args.settings as typeof settings;
      if (next.maxTextBytes < 1024 || next.maxImageBytes < 1024) {
        throw "the text and image size limits must be at least 1 KB";
      }
      settings = { ...next };
      return { ...settings };
    }
    case "get_stats":
      return stats();
    case "hide_bar":
      setTimeout(() => emit("bar:hidden"), 0);
      return null;
    default:
      throw `${cmd} not allowed`;
  }
}

export function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  control.calls.push({ cmd, args });
  const go = () => {
    try {
      return Promise.resolve(run(cmd, args) as T);
    } catch (e) {
      return Promise.reject(e);
    }
  };
  if (control.latencyMs > 0) {
    return new Promise<T>((resolve, reject) => setTimeout(() => go().then(resolve, reject), control.latencyMs));
  }
  return go();
}

// ---------------------------------------------------------------------------
// Test hooks
// ---------------------------------------------------------------------------

load(200);

declare global {
  interface Window {
    __clipMock: typeof hooks;
  }
}

const hooks = {
  control,
  emit,
  /** Replace the history with `count` fresh fake items. */
  setCount(count: number) {
    load(count);
    changed();
  },
  reset() {
    settings = { ...DEFAULT_SETTINGS };
    control.calls.length = 0;
    control.latencyMs = 0;
    control.copyError = null;
    control.ocrUnavailable = false;
    load(200);
    changed();
  },
  /** Add one text item on top, like a new clipboard capture. */
  capture(text: string) {
    const id = nextId++;
    rows.unshift(
      toRow({
        id,
        kind: "text",
        text,
        preview: text.replace(/\s+/g, " ").trim().slice(0, 300),
        files: [],
        width: 0,
        height: 0,
        byteSize: text.length,
        createdAt: Date.now(),
        pinned: false,
        useCount: 0,
        hasThumb: false,
      }),
    );
    changed();
  },
  rows: () => rows,
  settings: () => settings,
};
window.__clipMock = hooks;
