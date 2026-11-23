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
  llmEmbeddingModel: "",
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
  size: "quick" | "short" | "long" | null;
  kind: "quick" | "deep" | "waiting" | null;
  nextStep: string;
  clipIds: number[];
  deferredUntil: number | null;
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
    size: null,
    kind: null,
    nextStep: "",
    clipIds: [],
    deferredUntil: null,
  });
  const sized = (task: MockTask, size: MockTask["size"], kind: MockTask["kind"]): MockTask => ({ ...task, size, kind });
  return [
    sized(
      t(1, "Fix the flaky checkout test", "doing", true, 0.2, "Fails only on the second run, check the seed data"),
      "short",
      "deep",
    ),
    sized(t(2, "Review the migration PR", "doing", true, 30, "Left at the rollback script"), "long", "deep"),
    sized(t(3, "Reply to the vendor about the invoice", "todo", true, 3), "quick", "waiting"),
    sized(t(4, "Prepare the Thursday demo", "todo", true, 5), "long", "deep"),
    sized(t(5, "Update the onboarding doc", "todo", false, 8), "short", "deep"),
    sized(t(6, "Look into the slow dashboard query", "todo", false, 12), "long", "deep"),
    sized(t(7, "Rename the staging buckets", "todo", false, 40), "quick", "quick"),
    // Waiting for weeks and no estimate yet: the "gone quiet" nudge and the unestimated count.
    { ...t(8, "Book the dentist", "todo", false, 24 * 20), nextStep: "" },
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
        size: null,
        kind: null,
        nextStep: "",
        clipIds: [],
        deferredUntil: null,
      };
      tasks.push(t);
      // Like the backend's estimator: a new task gets its estimate a moment later, once an LLM is set up.
      if (llmSetUp()) estimateLater(t.id);
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
      // Estimates and suggestions come from the LLM and do not count as touching the task.
      if (patch.size !== undefined) t.size = patch.size;
      if (patch.kind !== undefined) t.kind = patch.kind;
      if (patch.nextStep !== undefined) {
        if (Array.from(patch.nextStep.trim()).length > 300) throw "the suggested step is too long";
        t.nextStep = patch.nextStep.trim();
      }
      if (patch.clipIds !== undefined) {
        const ids = Array.from(new Set(patch.clipIds));
        if (ids.length > 20) throw "too many linked clipboard items";
        t.clipIds = ids;
      }
      if (patch.deferredUntil !== undefined) t.deferredUntil = patch.deferredUntil;
      const touches =
        title !== undefined ||
        note !== undefined ||
        patch.important !== undefined ||
        patch.status !== undefined ||
        patch.clipIds !== undefined ||
        patch.deferredUntil !== undefined;
      if (touches) t.touchedAt = Date.now();
      return { ...t };
    }
    case "delete_task": {
      const before = tasks.length;
      tasks = tasks.filter((t) => t.id !== Number(args.id));
      return tasks.length !== before;
    }
    default:
      return runAssistant(cmd, args);
  }
}

// ---------------------------------------------------------------------------
// The task assistant: canned answers that follow the real backend's rules (what it needs, what it
// returns, what it validates), so the UI can be built and checked without an LLM server.
// ---------------------------------------------------------------------------

const llmSetUp = () => Boolean(settings.llmBaseUrl && settings.llmModel);
const openTasks = () => tasks.filter((t) => t.status !== "done");
const taskById = (id: unknown): MockTask => {
  const t = tasks.find((x) => x.id === Number(id));
  if (!t) throw "task not found";
  return t;
};

/** A plausible estimate from the words of the title. */
function guessEstimate(t: MockTask): Pick<MockTask, "size" | "kind"> {
  const words = t.title.trim().split(/\s+/).length;
  const kind = /\b(wait|waiting|vendor|reply|approval|delivery)\b/i.test(t.title)
    ? "waiting"
    : words <= 3
      ? "quick"
      : "deep";
  return { size: words <= 3 ? "quick" : words <= 6 ? "short" : "long", kind };
}

function estimateLater(id: number) {
  setTimeout(() => {
    const t = tasks.find((x) => x.id === id);
    if (!t || t.size !== null) return;
    Object.assign(t, guessEstimate(t));
    emit("tasks:updated");
  }, control.estimateDelayMs);
}

function requireLlm() {
  if (control.llmError) throw control.llmError;
  if (!llmSetUp()) throw "set up an LLM server in Settings first";
}

function runAssistant(cmd: string, args: Record<string, unknown>): unknown {
  switch (cmd) {
    case "get_clips":
      return (args.ids as number[])
        .map((id) => rows.find((r) => r.id === id))
        .filter((r): r is Row => r !== undefined)
        .map((r) => ({ id: r.id, kind: r.kind, preview: r.preview, createdAt: r.createdAt }));
    case "estimate_task": {
      requireLlm();
      const t = taskById(args.id);
      Object.assign(t, guessEstimate(t));
      return { ...t };
    }
    case "estimate_backlog": {
      requireLlm();
      const missing = openTasks().filter((t) => t.size === null);
      for (const t of missing) estimateLater(t.id);
      return missing.length;
    }
    case "pick_next_task": {
      requireLlm();
      const now = Date.now();
      const minutes = args.minutes === null ? null : Number(args.minutes);
      const energy = String(args.energy);
      const candidates = openTasks()
        .filter((t) => t.status === "doing" || t.deferredUntil === null || t.deferredUntil <= now)
        .sort(
          (a, b) =>
            Number(b.status === "doing") - Number(a.status === "doing") ||
            Number(b.important) - Number(a.important) ||
            a.createdAt - b.createdAt,
        );
      if (candidates.length === 0) return null;
      const light = (minutes !== null && minutes <= 15) || energy === "low";
      const fits = light ? candidates.filter((t) => t.size === "quick") : candidates;
      const pick = (fits.length > 0 ? fits : candidates)[0];
      const why = light && pick.size === "quick" ? "It is a quick one that fits the time and energy you have." : "It is the most pressing open task.";
      return { id: pick.id, reason: why };
    }
    case "suggest_next_step": {
      requireLlm();
      const t = taskById(args.id);
      t.nextStep = `Open "${t.title}" and write down the very first small step.`;
      const big = t.title.trim().split(/\s+/).length > 3;
      return { step: t.nextStep, subtasks: big ? ["Gather what you need", "Do the first part", "Check the result"] : [] };
    }
    case "draft_resume_note": {
      requireLlm();
      const t = taskById(args.id);
      const recent = rows
        .filter((r) => r.kind === "text" && r.usedAt >= t.touchedAt - 30 * 60_000)
        .slice(0, 3);
      if (recent.length === 0) throw "no recent clipboard item seems to belong to this task";
      return `Was looking at ${recent.map((r) => `"${r.preview.slice(0, 40)}"`).join(" and ")}.`;
    }
    case "task_digest": {
      requireLlm();
      const open = openTasks();
      const quiet = open.filter((t) => Date.now() - t.touchedAt > 5 * 24 * HOUR).length;
      const order = open
        .sort((a, b) => Number(b.important) - Number(a.important) || a.createdAt - b.createdAt)
        .slice(0, 3)
        .map((t) => t.id);
      const week = args.kind === "week";
      return {
        summary: week
          ? `${tasks.length - open.length} done this week, ${open.length} still open and ${quiet} gone quiet. Start next week with the first one below.`
          : `${open.filter((t) => t.status === "doing").length} in flight and ${quiet} gone quiet. Finish the important ones first.`,
        order,
      };
    }
    case "suggest_task_links": {
      requireLlm();
      const clip = rows.find((r) => r.kind === "text");
      const task = openTasks()[0];
      return clip && task && !task.clipIds.includes(clip.id)
        ? [{ taskId: task.id, clipId: clip.id, reason: "it looks related" }]
        : [];
    }
    case "extract_tasks": {
      requireLlm();
      const text =
        args.text === null || args.text === undefined
          ? (rows.find((r) => r.kind === "text")?.text ?? "")
          : String(args.text);
      if (text.trim() === "" && args.text === null) throw "there is no copied text to read";
      const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();
      const found = text
        .split(/[.\n;]/)
        .map((x) => x.trim())
        .filter((x) => x.length > 15 && /\b(please|need to|send|call|book|review|reply|prepare|update)\b/i.test(x))
        .slice(0, 20)
        .map((x) => {
          const title = Array.from(x.charAt(0).toUpperCase() + x.slice(1))
            .slice(0, 80)
            .join("");
          const dup = openTasks().find((t) => norm(t.title) === norm(title));
          return { title, important: /\b(urgent|asap|today|tomorrow|friday)\b/i.test(x), duplicateOf: dup?.id ?? null };
        });
      return { source: text.trim().slice(0, 2000), tasks: found };
    }
    case "review_stale_tasks": {
      requireLlm();
      return (args.ids as number[])
        .map((id) => tasks.find((t) => t.id === id))
        .filter((t): t is MockTask => t !== undefined && t.status !== "done")
        .map((t) => {
          const days = (Date.now() - t.touchedAt) / (24 * HOUR);
          const verdict = t.important ? "do" : days > 30 ? "drop" : "defer";
          return { id: t.id, verdict, reason: `untouched for ${Math.floor(days)} days` };
        });
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
  /** The assistant's commands reject with this message when set (the LLM server is down). */
  llmError: null as string | null,
  /** How long a background estimate takes, in ms. */
  estimateDelayMs: 300,
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

// ---------------------------------------------------------------------------
// Ask (a keyword stand-in for the plan, retrieve, answer and verify steps of ask.rs)
// ---------------------------------------------------------------------------

const ASK_STOP = new Set(["what", "was", "that", "thing", "the", "did", "for", "and", "told", "ago", "about", "week", "weeks", "last", "month"]);

function runAsk(question: unknown): unknown {
  const q = String(question ?? "").trim();
  if (q === "") throw "type a question first";
  requireLlm();
  const words = q
    .toLowerCase()
    .split(/[^\p{L}\p{N}-]+/u)
    .filter((w) => w.length >= 3 && !ASK_STOP.has(w));
  type Cand = { source: "note" | "task" | "clip"; id: number; createdAt: number; title: string; text: string };
  const all: Cand[] = [
    ...notes.map((n) => ({ source: "note" as const, id: n.id, createdAt: n.createdAt, title: n.title, text: n.body })),
    ...tasks.map((t) => ({ source: "task" as const, id: t.id, createdAt: t.createdAt, title: t.title, text: t.note })),
    ...rows
      .filter((r) => r.kind !== "files" && r.text.trim() !== "")
      .slice(0, 400)
      .map((r) => ({ source: "clip" as const, id: r.id, createdAt: r.createdAt, title: "", text: r.text.slice(0, 600) })),
  ];
  const scored = all
    .map((c) => ({ c, score: words.filter((w) => (c.title + " " + c.text).toLowerCase().includes(w)).length }))
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score || b.c.createdAt - a.c.createdAt)
    .slice(0, 8);
  const searched = {
    from: null,
    to: null,
    terms: words.slice(0, 12),
    sources: ["note", "task", "clip"],
    scanned: all.length,
  };
  const hits = scored.map(({ c }, i) => ({
    n: i + 1,
    source: c.source,
    id: c.id,
    createdAt: c.createdAt,
    title: c.title,
    excerpt: c.text,
    quote: null as string | null,
    cited: false,
  }));
  const top = hits[0];
  if (!top) {
    return { found: false, answer: "I could not find that in your notes, tasks or clipboard history.", hits, searched };
  }
  // The quote is the first line of the best hit that holds a question word, like a verified quote.
  const line = top.excerpt.split("\n").find((l) => words.some((w) => l.toLowerCase().includes(w))) ?? top.excerpt;
  top.quote = line.trim();
  top.cited = true;
  return {
    found: true,
    answer: `Closest match: ${top.quote} [${top.n}]`,
    hits,
    searched,
  };
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
    case "estimate_task":
    case "estimate_backlog":
    case "pick_next_task":
    case "suggest_next_step":
    case "draft_resume_note":
    case "task_digest":
    case "suggest_task_links":
    case "extract_tasks":
    case "review_stale_tasks":
    case "get_clips":
      if (control.tasksError) throw control.tasksError;
      return runTask(cmd, args);
    case "ask_everything":
      return runAsk(args.question);
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
    control.llmError = null;
    control.estimateDelayMs = 300;
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
