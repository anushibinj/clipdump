// Where the focus state is kept: localStorage, like the theme, the view and the notes pane. The
// webview keeps it across openings and restarts of the app, so a lock outlives both. Nothing
// goes to the backend.
//
// What comes back from storage is untrusted (an older build, a hand edit, a half written value):
// `sanitizeFocus` keeps what is well formed and falls back to a clean state for the rest, so a
// bad value can never lock the list for good.

import {
  DEFAULT_MINUTES,
  MAX_LOG,
  MAX_PARKED,
  MAX_PARKED_CHARS,
  clampMinutes,
  emptyFocus,
  type CheckIn,
  type FocusData,
  type FocusSession,
  type LogEntry,
  type Outcome,
  type ParkedItem,
} from "@/lib/focus";

const KEY = "clipdump.focus";
const OUTCOMES: readonly Outcome[] = ["completed", "done", "broken"];

type Obj = Record<string, unknown>;

const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isInt = (v: unknown): v is number => isNum(v) && Number.isInteger(v);

function sanitizeSession(v: unknown): FocusSession | null {
  if (!isObj(v)) return null;
  if (!isInt(v.taskId) || typeof v.title !== "string") return null;
  if (!isNum(v.startedAt) || !isNum(v.durationMs) || !isNum(v.endsAt)) return null;
  if (v.durationMs <= 0 || v.endsAt < v.startedAt) return null;
  const pausedAt = v.pausedAt === null || v.pausedAt === undefined ? null : isNum(v.pausedAt) ? v.pausedAt : undefined;
  if (pausedAt === undefined) return null;
  return {
    taskId: v.taskId,
    title: v.title,
    startedAt: v.startedAt,
    durationMs: v.durationMs,
    endsAt: v.endsAt,
    pausedAt,
    pauseUsed: v.pauseUsed === true || pausedAt !== null,
    warned: v.warned === true,
  };
}

function sanitizeCheckIn(v: unknown): CheckIn | null {
  if (!isObj(v) || !isInt(v.taskId) || typeof v.title !== "string") return null;
  if (v.stage !== "ask" && v.stage !== "next") return null;
  return { taskId: v.taskId, title: v.title, stage: v.stage };
}

function sanitizeLog(v: unknown): LogEntry[] {
  if (!Array.isArray(v)) return [];
  const out: LogEntry[] = [];
  for (const e of v) {
    if (!isObj(e)) continue;
    if (!isInt(e.taskId) || typeof e.title !== "string") continue;
    if (!isNum(e.startedAt) || !isNum(e.endedAt) || !isNum(e.focusedMs) || e.focusedMs < 0) continue;
    if (!OUTCOMES.includes(e.outcome as Outcome)) continue;
    out.push({
      taskId: e.taskId,
      title: e.title,
      startedAt: e.startedAt,
      endedAt: e.endedAt,
      focusedMs: e.focusedMs,
      outcome: e.outcome as Outcome,
    });
  }
  return out.slice(-MAX_LOG);
}

function sanitizeParked(v: unknown): ParkedItem[] {
  if (!Array.isArray(v)) return [];
  const out: ParkedItem[] = [];
  const seen = new Set<number>();
  for (const p of v) {
    if (!isObj(p) || !isInt(p.id) || typeof p.text !== "string" || !isNum(p.at)) continue;
    const text = p.text.trim().slice(0, MAX_PARKED_CHARS);
    if (text === "" || seen.has(p.id)) continue;
    seen.add(p.id);
    out.push({ id: p.id, text, at: p.at });
  }
  return out.slice(0, MAX_PARKED);
}

/** A well formed `FocusData` from whatever was stored. */
export function sanitizeFocus(raw: unknown): FocusData {
  const base = emptyFocus();
  if (!isObj(raw)) return base;
  const parked = sanitizeParked(raw.parked);
  const streakRaw = isObj(raw.streak) ? raw.streak : {};
  const current = isInt(streakRaw.current) && streakRaw.current > 0 ? streakRaw.current : 0;
  const best = isInt(streakRaw.best) && streakRaw.best > 0 ? Math.max(streakRaw.best, current) : current;
  const minutes = isNum(raw.minutes) ? clampMinutes(raw.minutes) : DEFAULT_MINUTES;
  const highestParkedId = parked.reduce((m, p) => Math.max(m, p.id), 0);
  return {
    session: sanitizeSession(raw.session),
    checkin: sanitizeCheckIn(raw.checkin),
    breakEndsAt: isNum(raw.breakEndsAt) ? raw.breakEndsAt : null,
    minutes,
    streak: { current, best },
    log: sanitizeLog(raw.log),
    parked,
    nextParkedId: Math.max(isInt(raw.nextParkedId) ? raw.nextParkedId : 1, highestParkedId + 1),
  };
}

export function readFocus(): FocusData {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw !== null) return sanitizeFocus(JSON.parse(raw));
  } catch {
    // Storage blocked or the value is not JSON: start clean.
  }
  return emptyFocus();
}

export function writeFocus(data: FocusData): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(data));
  } catch {
    // Not persisted; the state still applies for this session.
  }
}
