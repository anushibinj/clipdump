// Focus mode for the Tasks view, as pure functions (no DOM, no storage, no timers), so every rule
// is unit tested. A "focus session" locks the list to one task for a while: the other tasks cannot
// be changed until the time is up, the task is done, or the lock is broken on purpose (which is
// slow on purpose, and costs the streak).
//
// All times are unix ms and passed in, never read from the clock here.

/** The durations offered as one click. */
export const PRESET_MINUTES = [15, 25, 30, 60] as const;
export const DEFAULT_MINUTES = 30;
export const MIN_MINUTES = 1;
export const MAX_MINUTES = 240;
/** Breaking the lock takes this long, so an impulse has time to pass. */
export const BREAK_FRICTION_MS = 30_000;
/** One pause per session, this long at most. The countdown stands still meanwhile. */
export const PAUSE_MAX_MS = 5 * 60_000;
/** A heads-up this long before the end (only for sessions longer than this). */
export const WARN_MS = 5 * 60_000;
/** The break offered after a session. */
export const BREAK_MS = 5 * 60_000;
/** Parked thoughts: how many are kept and how long each may be. */
export const MAX_PARKED = 100;
export const MAX_PARKED_CHARS = 300;
/** Sessions kept in the log (the oldest are dropped). */
export const MAX_LOG = 300;

const MINUTE = 60_000;

export interface FocusSession {
  taskId: number;
  /** The title when it started, for the log (the task may be renamed or deleted later). */
  title: string;
  startedAt: number;
  durationMs: number;
  /** When the time is up. Moves later by the time spent paused. */
  endsAt: number;
  /** Set while paused. */
  pausedAt: number | null;
  pauseUsed: boolean;
  /** The five-minutes-left heads-up was already given. */
  warned: boolean;
}

/** What happens after a session ends: ask how it went, then offer a break or another session. */
export interface CheckIn {
  taskId: number;
  title: string;
  /** "ask": done, need more time, blocked? "next": a break, another session, or nothing. */
  stage: "ask" | "next";
}

export type Outcome =
  /** The time ran out. */
  | "completed"
  /** The task was marked done before the time ran out. */
  | "done"
  /** The lock was broken. */
  | "broken";

export interface LogEntry {
  taskId: number;
  title: string;
  startedAt: number;
  endedAt: number;
  /** Time actually focused: the pauses are not in it. */
  focusedMs: number;
  outcome: Outcome;
}

export interface Streak {
  /** Sessions in a row that were not broken. */
  current: number;
  best: number;
}

/** A thought written down during a session, to look at afterwards instead of acting on now. */
export interface ParkedItem {
  id: number;
  text: string;
  at: number;
}

export interface FocusData {
  session: FocusSession | null;
  checkin: CheckIn | null;
  /** When the current break ends, null when there is none. */
  breakEndsAt: number | null;
  /** The duration used last, offered first next time. */
  minutes: number;
  streak: Streak;
  log: LogEntry[];
  parked: ParkedItem[];
  nextParkedId: number;
}

export type FocusEvent = "warn" | "ended" | "breakOver";

export function emptyFocus(): FocusData {
  return {
    session: null,
    checkin: null,
    breakEndsAt: null,
    minutes: DEFAULT_MINUTES,
    streak: { current: 0, best: 0 },
    log: [],
    parked: [],
    nextParkedId: 1,
  };
}

export function clampMinutes(m: number): number {
  if (!Number.isFinite(m)) return DEFAULT_MINUTES;
  return Math.min(MAX_MINUTES, Math.max(MIN_MINUTES, Math.round(m)));
}

// ---- reading a session -----------------------------------------------------------------------

/** Time left. Stands still while paused. */
export function remainingMs(s: FocusSession, now: number): number {
  return Math.max(0, s.endsAt - (s.pausedAt ?? now));
}

/** Time focused so far (the pauses are not counted). */
export function focusedSoFar(s: FocusSession, now: number): number {
  return Math.min(s.durationMs, Math.max(0, s.durationMs - remainingMs(s, now)));
}

/** How long a pause may still last, 0 when not paused. */
export function pauseLeftMs(s: FocusSession, now: number): number {
  return s.pausedAt === null ? 0 : Math.max(0, PAUSE_MAX_MS - (now - s.pausedAt));
}

export function canPause(s: FocusSession): boolean {
  return s.pausedAt === null && !s.pauseUsed;
}

/** Whether `taskId` may be changed right now: with no lock everything may, with one only its task. */
export function isTaskFree(data: FocusData, taskId: number): boolean {
  return data.session === null || data.session.taskId === taskId;
}

// ---- the session -----------------------------------------------------------------------------

/** Lock the list to `taskId` for `minutes`. Does nothing while a session is running. */
export function startSession(
  data: FocusData,
  task: { id: number; title: string },
  minutes: number,
  now: number,
): FocusData {
  if (data.session) return data;
  const m = clampMinutes(minutes);
  const durationMs = m * MINUTE;
  return {
    ...data,
    minutes: m,
    session: {
      taskId: task.id,
      title: task.title,
      startedAt: now,
      durationMs,
      endsAt: now + durationMs,
      pausedAt: null,
      pauseUsed: false,
      warned: false,
    },
    checkin: null,
    breakEndsAt: null,
  };
}

export function pauseSession(data: FocusData, now: number): FocusData {
  const s = data.session;
  if (!s || !canPause(s)) return data;
  return { ...data, session: { ...s, pausedAt: now, pauseUsed: true } };
}

export function resumeSession(data: FocusData, now: number): FocusData {
  const s = data.session;
  if (!s || s.pausedAt === null) return data;
  return { ...data, session: resumed(s, now) };
}

function resumed(s: FocusSession, now: number): FocusSession {
  if (s.pausedAt === null) return s;
  return { ...s, endsAt: s.endsAt + Math.max(0, now - s.pausedAt), pausedAt: null };
}

function addToLog(log: LogEntry[], entry: LogEntry): LogEntry[] {
  const next = [...log, entry];
  return next.length > MAX_LOG ? next.slice(next.length - MAX_LOG) : next;
}

function bumped(streak: Streak): Streak {
  const current = streak.current + 1;
  return { current, best: Math.max(streak.best, current) };
}

function finish(data: FocusData, s: FocusSession, outcome: Outcome, now: number): FocusData {
  const entry: LogEntry = {
    taskId: s.taskId,
    title: s.title,
    startedAt: s.startedAt,
    endedAt: now,
    focusedMs: outcome === "completed" ? s.durationMs : focusedSoFar(s, now),
    outcome,
  };
  return {
    ...data,
    session: null,
    log: addToLog(data.log, entry),
    streak: outcome === "broken" ? { ...data.streak, current: 0 } : bumped(data.streak),
  };
}

/** The lock was broken on purpose: the streak starts over. */
export function breakLock(data: FocusData, now: number): FocusData {
  const s = data.session;
  if (!s) return data;
  return { ...finish(data, s, "broken", now), checkin: null };
}

/** The locked task was finished before the time was up: that counts, and a break is offered. */
export function finishEarly(data: FocusData, now: number): FocusData {
  const s = data.session;
  if (!s) return data;
  return { ...finish(data, s, "done", now), checkin: { taskId: s.taskId, title: s.title, stage: "next" } };
}

/** The locked task is gone (deleted elsewhere): drop the lock without counting anything. */
export function releaseLock(data: FocusData): FocusData {
  return data.session ? { ...data, session: null } : data;
}

/** Advance the clock: end a session that ran out, give the heads-up, resume a pause that ran out. */
export function tick(data: FocusData, now: number): { data: FocusData; events: FocusEvent[] } {
  const events: FocusEvent[] = [];
  let next = data;

  let s = next.session;
  if (s) {
    if (s.pausedAt !== null && now - s.pausedAt >= PAUSE_MAX_MS) {
      s = resumed(s, s.pausedAt + PAUSE_MAX_MS);
      next = { ...next, session: s };
    }
    if (s.pausedAt === null) {
      if (now >= s.endsAt) {
        next = {
          ...finish(next, s, "completed", s.endsAt),
          checkin: { taskId: s.taskId, title: s.title, stage: "ask" },
        };
        events.push("ended");
      } else if (!s.warned && s.durationMs > WARN_MS && s.endsAt - now <= WARN_MS) {
        next = { ...next, session: { ...s, warned: true } };
        events.push("warn");
      }
    }
  }

  if (next.breakEndsAt !== null && now >= next.breakEndsAt) {
    next = { ...next, breakEndsAt: null };
    events.push("breakOver");
  }
  return { data: next, events };
}

// ---- after a session -------------------------------------------------------------------------

/** "done" and "blocked" go on to the break offer; "more" leaves the check-in for the picker. */
export function answerCheckin(data: FocusData, answer: "done" | "more" | "blocked"): FocusData {
  const c = data.checkin;
  if (!c || answer === "more") return data;
  return { ...data, checkin: { ...c, stage: "next" } };
}

export function dismissCheckin(data: FocusData): FocusData {
  return data.checkin ? { ...data, checkin: null } : data;
}

export function startBreak(data: FocusData, now: number): FocusData {
  return { ...data, checkin: null, breakEndsAt: now + BREAK_MS };
}

export function endBreak(data: FocusData): FocusData {
  return data.breakEndsAt === null ? data : { ...data, breakEndsAt: null };
}

// ---- the parking lot -------------------------------------------------------------------------

/** Write a thought down. Blank text and a full lot are ignored. */
export function park(data: FocusData, text: string, now: number): FocusData {
  const t = text.trim().slice(0, MAX_PARKED_CHARS);
  if (t === "" || data.parked.length >= MAX_PARKED) return data;
  return {
    ...data,
    parked: [...data.parked, { id: data.nextParkedId, text: t, at: now }],
    nextParkedId: data.nextParkedId + 1,
  };
}

export function unpark(data: FocusData, id: number): FocusData {
  return data.parked.some((p) => p.id === id) ? { ...data, parked: data.parked.filter((p) => p.id !== id) } : data;
}

// ---- stats -----------------------------------------------------------------------------------

/** Local midnight of the day `ms` falls on. */
export function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** Focused time of sessions that ended today. */
export function focusedToday(log: readonly LogEntry[], now: number): number {
  const from = startOfDay(now);
  return log.reduce((sum, e) => (e.endedAt >= from ? sum + e.focusedMs : sum), 0);
}

export function focusedForTask(log: readonly LogEntry[], taskId: number): number {
  return log.reduce((sum, e) => (e.taskId === taskId ? sum + e.focusedMs : sum), 0);
}

/** The last `days` days, today first, each with the focused time of that day (0 when none). */
export function focusedByDay(
  log: readonly LogEntry[],
  now: number,
  days: number,
): { dayStart: number; ms: number }[] {
  const out: { dayStart: number; ms: number }[] = [];
  const today = new Date(startOfDay(now));
  for (let i = 0; i < days; i++) {
    const d = new Date(today);
    d.setDate(today.getDate() - i);
    const from = d.getTime();
    const to = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1).getTime();
    out.push({
      dayStart: from,
      ms: log.reduce((sum, e) => (e.endedAt >= from && e.endedAt < to ? sum + e.focusedMs : sum), 0),
    });
  }
  return out;
}

// ---- text ------------------------------------------------------------------------------------

/** A countdown: "24:10", or "1:05:10" from an hour up. Rounds up, so the last second reads 0:01. */
export function formatClock(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** A length of time for stats: "0m", "25m", "1h 05m". */
export function formatDuration(ms: number): string {
  const minutes = Math.round(ms / MINUTE);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}
