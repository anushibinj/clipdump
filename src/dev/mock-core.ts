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
let nextId = 100_001;

function toRow(it: MockItem): Row {
  const searchable = it.kind === "files" ? it.files.map((f) => f.path).join("\n") : it.text;
  return { ...it, usedAt: it.createdAt, lower: searchable.slice(0, 10_000).toLowerCase() };
}

function load(count: number) {
  rows = generateItems(count, NOW).map(toRow);
  nextId = 100_001;
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
  autoPasteOnEnter: false,
  pinnedBar: false,
  paused: false,
  hotkey: "Ctrl+Shift+Space",
  startAtLogin: true,
  llmBaseUrl: "http://localhost:11434/v1",
  llmModel: "llama3.2",
  llmApiKeySet: false,
  llmApiKey: null as string | null,
};
let settings = { ...DEFAULT_SETTINGS };

// ---------------------------------------------------------------------------
// Tasks (the same rules as src-tauri/src/tasks.rs)
// ---------------------------------------------------------------------------

interface MockTask {
  id: number;
  title: string;
  note: string;
  status: "todo" | "doing" | "done";
  important: boolean;
  createdAt: number;
  touchedAt: number;
  doneAt: number | null;
}

const HOUR = 3_600_000;
function seedTasks(): MockTask[] {
  const t = (
    id: number,
    title: string,
    status: MockTask["status"],
    important: boolean,
    agoH: number,
    note = "",
    doneAgoH: number | null = null,
  ): MockTask => ({
    id,
    title,
    note,
    status,
    important,
    createdAt: NOW - (agoH + 20) * HOUR,
    touchedAt: NOW - agoH * HOUR,
    doneAt: doneAgoH === null ? null : NOW - doneAgoH * HOUR,
  });
  return [
    t(1, "Fix the flaky checkout test", "doing", true, 0.2, "Fails only on the second run, check the seed data"),
    t(2, "Review the migration PR", "doing", true, 30, "Left at the rollback script"),
    t(3, "Reply to the vendor about the invoice", "todo", true, 3),
    t(4, "Prepare the Thursday demo", "todo", true, 5),
    t(5, "Update the onboarding doc", "todo", false, 8),
    t(6, "Look into the slow dashboard query", "todo", false, 12),
    t(7, "Rename the staging buckets", "todo", false, 40),
    t(8, "Book the dentist", "todo", false, 60),
    t(9, "Send the weekly summary", "done", false, 2, "", 1),
  ];
}
let tasks: MockTask[] = seedTasks();
let nextTaskId = 100;

const TASK_TITLE_MAX = 300;
const TASK_NOTE_MAX = 1000;

function taskTitle(raw: unknown): string {
  const title = String(raw ?? "").trim();
  if (title === "") throw "the task needs a title";
  if (Array.from(title).length > TASK_TITLE_MAX) throw "the title is too long";
  return title;
}

function setTaskStatus(t: MockTask, status: MockTask["status"]) {
  t.status = status;
  t.doneAt = status === "done" ? Date.now() : null;
}

function runTask(cmd: string, args: Record<string, unknown>): unknown {
  switch (cmd) {
    case "list_tasks":
      return tasks.map((t) => ({ ...t }));
    case "add_task": {
      const title = taskTitle(args.title);
      if (tasks.length >= 500 && !tasks.some((t) => t.status === "done")) {
        throw "too many tasks, delete or finish some first";
      }
      const now = Date.now();
      const t: MockTask = {
        id: nextTaskId++,
        title,
        note: "",
        status: args.start ? "doing" : "todo",
        important: Boolean(args.important),
        createdAt: now,
        touchedAt: now,
        doneAt: null,
      };
      tasks.push(t);
      return { ...t };
    }
    case "update_task": {
      const t = tasks.find((x) => x.id === Number(args.id));
      if (!t) throw "task not found";
      const patch = (args.patch ?? {}) as Partial<MockTask>;
      const title = patch.title === undefined ? undefined : taskTitle(patch.title);
      const note = patch.note === undefined ? undefined : String(patch.note).trim();
      if (note !== undefined && Array.from(note).length > TASK_NOTE_MAX) throw "the note is too long";
      if (title !== undefined) t.title = title;
      if (note !== undefined) t.note = note;
      if (patch.important !== undefined) t.important = Boolean(patch.important);
      if (patch.status !== undefined) setTaskStatus(t, patch.status);
      t.touchedAt = Date.now();
      return { ...t };
    }
    case "delete_task": {
      const before = tasks.length;
      tasks = tasks.filter((t) => t.id !== Number(args.id));
      return tasks.length !== before;
    }
  }
  throw `${cmd} not allowed`;
}

const control = {
  /** Every command the page called, in order. */
  calls: [] as { cmd: string; args: unknown }[],
  /** Simulated IPC round trip per call, in ms (0 = resolve on the next microtask). */
  latencyMs: 0,
  /** Extra delay per exact query string for `search`, to force out of order responses in tests. */
  delayByQuery: {} as Record<string, number>,
  /** `copy_item` rejects with this message when set (the real one does on Linux, where there is no clipboard backend). */
  copyError: null as string | null,
  /** `get_stats` reports an OCR engine without a language. */
  ocrUnavailable: false,
  /** Chords "another app owns": `set_settings` refuses them like the real registrar does. */
  takenHotkeys: ["Super+L", "Ctrl+Alt+Delete"] as string[],
  /** `set_settings` refuses to change the login item. */
  loginItemError: false,
  /** `get_stats` reports the hotkey as not registered at startup. */
  hotkeyStartupError: null as string | null,
  /** A split fails with this message when set. */
  splitError: null as string | null,
  /** Simulated LLM time for `split_item`, in ms. */
  splitDelayMs: 150,
  /** `open_data_folder` rejects with this message when set. */
  openFolderError: null as string | null,
  /** `get_stats` reports that the history was moved aside at startup (the recovery notice). */
  recovery: null as string | null,
  /** `get_stats` reports `--perf` (the page then sends `perf_ack` timings). */
  perf: false,
  /** Every task command rejects with this message when set (the real ones do when the store could not start). */
  tasksError: null as string | null,
  /** Every note command rejects with this message when set. */
  notesError: null as string | null,
};

// ---------------------------------------------------------------------------
// Notes (the same rules as src-tauri/src/notes.rs)
// ---------------------------------------------------------------------------

interface MockNote {
  id: number;
  title: string;
  body: string;
  pinned: boolean;
  tags: string[];
  createdAt: number;
  updatedAt: number;
}

function seedNotes(): MockNote[] {
  const n = (id: number, title: string, body: string, tags: string[], agoH: number, pinned = false): MockNote => ({
    id,
    title,
    body,
    pinned,
    tags,
    createdAt: NOW - (agoH + 10) * HOUR,
    updatedAt: NOW - agoH * HOUR,
  });
  return [
    n(1, "Release checklist", "1. Bump the version\n2. Update the changelog\n3. Tag and push", ["work", "release"], 2, true),
    n(2, "", "Ask about the staging bucket quota before Friday", ["work"], 6),
    n(3, "Wifi at the flat", "Network: flat-5g\nAsk the landlord for the guest password", ["home"], 30),
    n(4, "Book ideas", "The Pragmatic Programmer\nDesigning Data-Intensive Applications", ["reading"], 90, true),
    n(5, "", "Random thought: a calmer way to show hidden tasks", [], 200),
  ];
}
let notes: MockNote[] = seedNotes();
let nextNoteId = 100;

const NOTE_TITLE_MAX = 120;
const NOTE_BODY_MAX = 20_000;

function noteTags(raw: unknown): string[] {
  const out: string[] = [];
  for (const t of (Array.isArray(raw) ? raw : []) as string[]) {
    const tag = String(t).trim().replace(/^#+/, "").split(/\s+/).filter(Boolean).join("-").toLowerCase();
    if (!tag || out.includes(tag)) continue;
    if (Array.from(tag).length > 32) throw "a tag is too long";
    out.push(tag);
  }
  if (out.length > 10) throw "too many tags on this note";
  return out;
}

function noteTitle(raw: unknown): string {
  const title = String(raw ?? "").trim();
  if (Array.from(title).length > NOTE_TITLE_MAX) throw "the title is too long";
  return title;
}

function noteBody(raw: unknown): string {
  const body = String(raw ?? "").trimEnd();
  if (Array.from(body).length > NOTE_BODY_MAX) throw "the note is too long";
  return body;
}

function runNote(cmd: string, args: Record<string, unknown>): unknown {
  switch (cmd) {
    case "list_notes":
      return notes.map((n) => ({ ...n, tags: [...n.tags] }));
    case "add_note": {
      if (notes.length >= 1000) throw "too many notes, delete some first";
      const now = Date.now();
      const n: MockNote = {
        id: nextNoteId++,
        title: noteTitle(args.title),
        body: noteBody(args.body),
        pinned: false,
        tags: noteTags(args.tags),
        createdAt: now,
        updatedAt: now,
      };
      notes.push(n);
      return { ...n, tags: [...n.tags] };
    }
    case "update_note": {
      const n = notes.find((x) => x.id === Number(args.id));
      if (!n) throw "note not found";
      const patch = (args.patch ?? {}) as Partial<MockNote>;
      const title = patch.title === undefined ? undefined : noteTitle(patch.title);
      const body = patch.body === undefined ? undefined : noteBody(patch.body);
      const tags = patch.tags === undefined ? undefined : noteTags(patch.tags);
      if (title !== undefined) n.title = title;
      if (body !== undefined) n.body = body;
      if (tags !== undefined) n.tags = tags;
      if (patch.pinned !== undefined) n.pinned = patch.pinned;
      n.updatedAt = Date.now();
      return { ...n, tags: [...n.tags] };
    }
    case "delete_note": {
      const before = notes.length;
      notes = notes.filter((n) => n.id !== Number(args.id));
      return notes.length !== before;
    }
  }
  throw `${cmd} not allowed`;
}

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
    hotkeyError: control.hotkeyStartupError,
    recovery: control.recovery,
    perf: control.perf,
  };
}

type MockSplit =
  | { status: "queued" | "running" | "none" }
  | { status: "done"; parts: { label: string; value: string }[] }
  | { status: "error"; message: string };
const splits = new Map<string, MockSplit>();
const splitQueue: string[] = [];
let splitsRunning = 0;

function pumpSplits() {
  while (splitsRunning < 2 && splitQueue.length > 0) {
    const key = splitQueue.shift() as string;
    splitsRunning += 1;
    splits.set(key, { status: "running" });
    emit("split:updated");
    void splitText(key).then((parts) => {
      splitsRunning -= 1;
      splits.set(key, control.splitError ? { status: "error", message: control.splitError } : { status: "done", parts });
      emit("split:updated");
      pumpSplits();
    });
  }
}

/** A stand-in for the model: pick out e-mails, phone numbers, URLs and key=value tokens, exactly as written. */
function splitText(text: string): Promise<{ label: string; value: string }[]> {
  const found: { label: string; value: string; at: number }[] = [];
  const rules: [string, RegExp][] = [
    ["Email", /[\w.+-]+@[\w-]+\.[\w.-]+/g],
    ["URL", /https?:\/\/[^\s,;)]+/g],
    ["Phone", /\+?\d[\d ()-]{6,}\d/g],
    ["Token", /\b[A-Za-z0-9_-]{24,}\b/g],
  ];
  for (const [label, re] of rules) {
    for (const m of text.matchAll(re)) found.push({ label, value: m[0], at: m.index ?? 0 });
  }
  found.sort((a, b) => a.at - b.at);
  const seen = new Set<string>();
  const parts = found.filter((p) => !seen.has(p.value) && seen.add(p.value)).map(({ label, value }) => ({ label, value }));
  return new Promise((resolve) => setTimeout(() => resolve(parts), control.splitDelayMs));
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
      if (next.hotkey !== settings.hotkey) {
        if (!/^(Ctrl|Alt|Shift|Super)(\+(Ctrl|Alt|Shift|Super))*\+[A-Za-z0-9]+$/.test(next.hotkey)) {
          throw "Use Ctrl, Alt or Win together with another key.";
        }
        if (control.takenHotkeys.includes(next.hotkey)) {
          throw "That shortcut is taken by another app or reserved by Windows. Choose a different one.";
        }
        control.hotkeyStartupError = null;
      }
      if (next.startAtLogin !== settings.startAtLogin && control.loginItemError) {
        throw "could not change the start at login setting";
      }
      // The key is write-only, like the real backend: null keeps it, "" removes it.
      const keySet = next.llmApiKey === null ? settings.llmApiKeySet : next.llmApiKey !== "";
      settings = { ...next, llmApiKey: null, llmApiKeySet: keySet };
      return { ...settings };
    }
    case "get_stats":
      return stats();
    case "open_data_folder":
      if (control.openFolderError) throw control.openFolderError;
      return null;
    case "perf_ack":
      return null; // recorded in `control.calls` like every command
    case "request_split": {
      const row = find(Number(args.id));
      if (!settings.llmBaseUrl || !settings.llmModel) throw "set up an LLM server in Settings first";
      if (row.kind !== "text") throw "only text items can be split";
      const key = row.text;
      const known = splits.get(key);
      if (known && !args.force) return known;
      if (known && (known.status === "queued" || known.status === "running")) return known;
      // Two at a time, like the backend: the rest wait in the queue.
      splits.set(key, { status: "queued" });
      splitQueue.unshift(key);
      pumpSplits();
      return { status: "queued" };
    }
    case "get_split": {
      const row = find(Number(args.id));
      return splits.get(row.text) ?? { status: "none" };
    }
    case "list_tasks":
    case "add_task":
    case "update_task":
    case "delete_task":
      if (control.tasksError) throw control.tasksError;
      return runTask(cmd, args);
    case "list_notes":
    case "add_note":
    case "update_note":
    case "delete_note":
      if (control.notesError) throw control.notesError;
      return runNote(cmd, args);
    case "copy_text":
      return null;
    case "start_drag":
      return null; // the browser has no window to move
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
  const delay =
    cmd === "search" ? (control.delayByQuery[String(args.query ?? "")] ?? control.latencyMs) : control.latencyMs;
  if (delay > 0) {
    return new Promise<T>((resolve, reject) => setTimeout(() => go().then(resolve, reject), delay));
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
    splits.clear();
    splitQueue.length = 0;
    control.calls.length = 0;
    control.latencyMs = 0;
    control.delayByQuery = {};
    control.copyError = null;
    control.ocrUnavailable = false;
    control.takenHotkeys = ["Super+L", "Ctrl+Alt+Delete"];
    control.loginItemError = false;
    control.hotkeyStartupError = null;
    control.openFolderError = null;
    control.recovery = null;
    control.perf = false;
    control.tasksError = null;
    tasks = seedTasks();
    nextTaskId = 100;
    control.notesError = null;
    notes = seedNotes();
    nextNoteId = 100;
    load(200);
    changed();
  },
  /** Replace the task list (an empty array for a fresh start). */
  setTasks(list: MockTask[]) {
    tasks = list.map((t) => ({ ...t }));
  },
  tasks: () => tasks,
  /** Replace the notes (an empty array for a fresh start). */
  setNotes(list: MockNote[]) {
    notes = list.map((n) => ({ ...n, tags: [...n.tags] }));
  },
  notes: () => notes,
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
  /** The fake search alone (no IPC, no UI), to separate its cost from rendering. */
  searchNow: (query: string, filter: Filter = "all") => search(query, filter),
  rows: () => rows,
  settings: () => settings,
};
window.__clipMock = hooks;
