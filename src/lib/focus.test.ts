import { describe, expect, it } from "vitest";
import {
  BREAK_MS,
  MAX_LOG,
  MAX_PARKED,
  MAX_PARKED_CHARS,
  PAUSE_MAX_MS,
  answerCheckin,
  breakLock,
  canPause,
  clampMinutes,
  dismissCheckin,
  emptyFocus,
  finishEarly,
  focusedByDay,
  focusedForTask,
  focusedSoFar,
  focusedToday,
  formatClock,
  formatDuration,
  isTaskFree,
  park,
  pauseLeftMs,
  pauseSession,
  releaseLock,
  remainingMs,
  resumeSession,
  startBreak,
  startSession,
  tick,
  unpark,
  type FocusData,
} from "./focus";

const MIN = 60_000;
const T0 = new Date(2026, 10, 20, 10, 0, 0).getTime();
const task = { id: 7, title: "write the report" };

function started(minutes = 30, now = T0): FocusData {
  return startSession(emptyFocus(), task, minutes, now);
}

describe("startSession", () => {
  it("locks to the task for the chosen time and remembers the length", () => {
    const d = started(25);
    expect(d.session).toMatchObject({ taskId: 7, durationMs: 25 * MIN, endsAt: T0 + 25 * MIN, pausedAt: null });
    expect(d.minutes).toBe(25);
    expect(remainingMs(d.session!, T0 + 5 * MIN)).toBe(20 * MIN);
  });
  it("keeps the running session when asked again", () => {
    const d = started(25);
    expect(startSession(d, { id: 9, title: "other" }, 60, T0 + MIN)).toBe(d);
  });
  it("clamps the length", () => {
    expect(clampMinutes(0)).toBe(1);
    expect(clampMinutes(9999)).toBe(240);
    expect(clampMinutes(12.6)).toBe(13);
    expect(clampMinutes(NaN)).toBe(30);
  });
  it("clears an open check-in and break", () => {
    let d = { ...started(25), session: null };
    d = { ...d, checkin: { taskId: 1, title: "x", stage: "next" }, breakEndsAt: T0 + MIN };
    const s = startSession(d, task, 25, T0);
    expect(s.checkin).toBeNull();
    expect(s.breakEndsAt).toBeNull();
  });
});

describe("isTaskFree", () => {
  it("frees every task with no lock and only the locked one with a lock", () => {
    expect(isTaskFree(emptyFocus(), 1)).toBe(true);
    const d = started();
    expect(isTaskFree(d, 7)).toBe(true);
    expect(isTaskFree(d, 8)).toBe(false);
  });
});

describe("tick", () => {
  it("does nothing while time is left", () => {
    const d = started(30);
    const r = tick(d, T0 + 10 * MIN);
    expect(r.data).toBe(d);
    expect(r.events).toEqual([]);
  });
  it("ends the session at the time: logs it, counts the streak, opens the check-in", () => {
    const r = tick(started(30), T0 + 30 * MIN + 5000);
    expect(r.events).toEqual(["ended"]);
    expect(r.data.session).toBeNull();
    expect(r.data.checkin).toEqual({ taskId: 7, title: "write the report", stage: "ask" });
    expect(r.data.streak).toEqual({ current: 1, best: 1 });
    expect(r.data.log).toEqual([
      { taskId: 7, title: "write the report", startedAt: T0, endedAt: T0 + 30 * MIN, focusedMs: 30 * MIN, outcome: "completed" },
    ]);
  });
  it("gives the five-minute heads-up once", () => {
    const d = started(30);
    const a = tick(d, T0 + 25 * MIN);
    expect(a.events).toEqual(["warn"]);
    expect(tick(a.data, T0 + 26 * MIN).events).toEqual([]);
  });
  it("gives no heads-up for a session of five minutes or less", () => {
    expect(tick(started(5), T0 + 1000).events).toEqual([]);
  });
  it("still ends a session found long after its time (the app was closed)", () => {
    const r = tick(started(30), T0 + 10 * 60 * MIN);
    expect(r.events).toEqual(["ended"]);
    expect(r.data.log[0].endedAt).toBe(T0 + 30 * MIN);
  });
  it("ends a break when its time is up", () => {
    const d = startBreak(emptyFocus(), T0);
    expect(d.breakEndsAt).toBe(T0 + BREAK_MS);
    expect(tick(d, T0 + BREAK_MS - 1).events).toEqual([]);
    const r = tick(d, T0 + BREAK_MS);
    expect(r.events).toEqual(["breakOver"]);
    expect(r.data.breakEndsAt).toBeNull();
  });
});

describe("pause", () => {
  it("freezes the countdown and moves the end by the time paused", () => {
    let d = pauseSession(started(30), T0 + 10 * MIN);
    expect(remainingMs(d.session!, T0 + 14 * MIN)).toBe(20 * MIN);
    d = resumeSession(d, T0 + 14 * MIN);
    expect(d.session!.endsAt).toBe(T0 + 34 * MIN);
    expect(remainingMs(d.session!, T0 + 14 * MIN)).toBe(20 * MIN);
  });
  it("allows one pause per session", () => {
    let d = pauseSession(started(30), T0 + MIN);
    d = resumeSession(d, T0 + 2 * MIN);
    expect(canPause(d.session!)).toBe(false);
    expect(pauseSession(d, T0 + 3 * MIN)).toBe(d);
  });
  it("does not end while paused, however long it takes", () => {
    const d = pauseSession(started(30), T0 + 29 * MIN);
    const r = tick(d, T0 + 29 * MIN + PAUSE_MAX_MS - 1);
    expect(r.events).toEqual([]);
    expect(r.data.session!.pausedAt).not.toBeNull();
  });
  it("resumes by itself after five minutes and loses no focus time", () => {
    const d = pauseSession(started(30), T0 + 10 * MIN);
    const r = tick(d, T0 + 10 * MIN + PAUSE_MAX_MS + 1000);
    expect(r.data.session!.pausedAt).toBeNull();
    expect(r.data.session!.endsAt).toBe(T0 + 30 * MIN + PAUSE_MAX_MS);
  });
  it("tells how much pause is left", () => {
    const d = pauseSession(started(30), T0);
    expect(pauseLeftMs(d.session!, T0 + 2 * MIN)).toBe(3 * MIN);
    expect(pauseLeftMs(started().session!, T0)).toBe(0);
  });
  it("counts only the time focused when the lock is broken during a pause", () => {
    const d = pauseSession(started(30), T0 + 10 * MIN);
    expect(focusedSoFar(d.session!, T0 + 13 * MIN)).toBe(10 * MIN);
  });
});

describe("breakLock", () => {
  it("ends the session, starts the streak over, and logs the time actually focused", () => {
    let d = tick(started(30), T0 + 31 * MIN).data;
    d = startSession(dismissCheckin(d), task, 30, T0 + 40 * MIN);
    expect(d.streak.current).toBe(1);
    const broken = breakLock(d, T0 + 52 * MIN);
    expect(broken.session).toBeNull();
    expect(broken.checkin).toBeNull();
    expect(broken.streak).toEqual({ current: 0, best: 1 });
    expect(broken.log[1]).toMatchObject({ outcome: "broken", focusedMs: 12 * MIN });
  });
  it("does nothing without a session", () => {
    const d = emptyFocus();
    expect(breakLock(d, T0)).toBe(d);
  });
});

describe("finishEarly", () => {
  it("counts for the streak and goes straight to the break offer", () => {
    const d = finishEarly(started(30), T0 + 12 * MIN);
    expect(d.session).toBeNull();
    expect(d.checkin).toEqual({ taskId: 7, title: "write the report", stage: "next" });
    expect(d.streak).toEqual({ current: 1, best: 1 });
    expect(d.log[0]).toMatchObject({ outcome: "done", focusedMs: 12 * MIN });
  });
});

describe("releaseLock", () => {
  it("drops the lock and counts nothing", () => {
    const d = releaseLock(started());
    expect(d.session).toBeNull();
    expect(d.log).toEqual([]);
    expect(d.streak).toEqual({ current: 0, best: 0 });
  });
});

describe("check-in", () => {
  const asking = tick(started(30), T0 + 31 * MIN).data;
  it("done and blocked go on to the break offer", () => {
    expect(answerCheckin(asking, "done").checkin?.stage).toBe("next");
    expect(answerCheckin(asking, "blocked").checkin?.stage).toBe("next");
  });
  it("needing more time leaves it for the picker", () => {
    expect(answerCheckin(asking, "more")).toBe(asking);
  });
  it("a break closes it", () => {
    const d = startBreak(asking, T0 + 32 * MIN);
    expect(d.checkin).toBeNull();
    expect(d.breakEndsAt).toBe(T0 + 32 * MIN + BREAK_MS);
  });
});

describe("parking lot", () => {
  it("keeps thoughts in order with their own ids", () => {
    let d = park(emptyFocus(), "  email Sam  ", T0);
    d = park(d, "check the invoice", T0 + 1);
    expect(d.parked.map((p) => [p.id, p.text])).toEqual([
      [1, "email Sam"],
      [2, "check the invoice"],
    ]);
    d = unpark(d, 1);
    expect(d.parked.map((p) => p.id)).toEqual([2]);
    expect(park(d, "again", T0).parked.at(-1)?.id).toBe(3);
  });
  it("ignores blanks, cuts long text and stops at the cap", () => {
    expect(park(emptyFocus(), "   ", T0).parked).toEqual([]);
    expect(park(emptyFocus(), "x".repeat(MAX_PARKED_CHARS + 50), T0).parked[0].text).toHaveLength(MAX_PARKED_CHARS);
    let d = emptyFocus();
    for (let i = 0; i < MAX_PARKED + 5; i++) d = park(d, `t${i}`, T0);
    expect(d.parked).toHaveLength(MAX_PARKED);
  });
  it("unparking an unknown id changes nothing", () => {
    const d = park(emptyFocus(), "a", T0);
    expect(unpark(d, 99)).toBe(d);
  });
});

describe("log and stats", () => {
  it("keeps the newest sessions only", () => {
    let d = emptyFocus();
    for (let i = 0; i < MAX_LOG + 3; i++) {
      d = startSession(d, { id: i, title: `t${i}` }, 5, T0);
      d = finishEarly(d, T0 + MIN);
    }
    expect(d.log).toHaveLength(MAX_LOG);
    expect(d.log[0].title).toBe("t3");
  });
  const log = [
    { taskId: 1, title: "a", startedAt: 0, endedAt: T0 - 24 * 60 * MIN + MIN, focusedMs: 20 * MIN, outcome: "completed" as const },
    { taskId: 1, title: "a", startedAt: 0, endedAt: T0 - 60 * MIN, focusedMs: 30 * MIN, outcome: "completed" as const },
    { taskId: 2, title: "b", startedAt: 0, endedAt: T0 + MIN, focusedMs: 10 * MIN, outcome: "broken" as const },
  ];
  it("sums today and per task", () => {
    expect(focusedToday(log, T0)).toBe(10 * MIN + 30 * MIN);
    expect(focusedForTask(log, 1)).toBe(50 * MIN);
    expect(focusedForTask(log, 3)).toBe(0);
  });
  it("lists the last days, today first", () => {
    const days = focusedByDay(log, T0, 3);
    expect(days.map((d) => d.ms)).toEqual([40 * MIN, 20 * MIN, 0]);
  });
});

describe("text", () => {
  it("formats a countdown, rounding up", () => {
    expect(formatClock(30 * MIN)).toBe("30:00");
    expect(formatClock(24 * MIN + 10_000)).toBe("24:10");
    expect(formatClock(1)).toBe("0:01");
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(65 * MIN + 5000)).toBe("1:05:05");
  });
  it("formats a length of time", () => {
    expect(formatDuration(0)).toBe("0m");
    expect(formatDuration(25 * MIN)).toBe("25m");
    expect(formatDuration(65 * MIN)).toBe("1h 05m");
  });
});
