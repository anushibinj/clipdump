import { afterEach, describe, expect, it, vi } from "vitest";
import { emptyFocus, park, startSession, tick } from "./focus";
import { readFocus, sanitizeFocus, writeFocus } from "./focusStore";

function stubStorage(initial: Record<string, string> = {}) {
  const data = { ...initial };
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => data[k] ?? null,
    setItem: (k: string, v: string) => void (data[k] = v),
  });
  return data;
}

afterEach(() => vi.unstubAllGlobals());

const MIN = 60_000;
const T0 = 1_800_000_000_000;

describe("focus storage", () => {
  it("starts clean the first time", () => {
    stubStorage();
    expect(readFocus()).toEqual(emptyFocus());
  });

  it("gives back a running lock after a restart", () => {
    stubStorage();
    let d = startSession(emptyFocus(), { id: 4, title: "ship it" }, 30, T0);
    d = park(d, "call mum", T0 + 1);
    writeFocus(d);
    expect(readFocus()).toEqual(d);
  });

  it("gives back a finished session's check-in and log", () => {
    stubStorage();
    const d = tick(startSession(emptyFocus(), { id: 4, title: "ship it" }, 25, T0), T0 + 26 * MIN).data;
    writeFocus(d);
    const back = readFocus();
    expect(back.session).toBeNull();
    expect(back.checkin?.stage).toBe("ask");
    expect(back.log).toHaveLength(1);
    expect(back.streak.current).toBe(1);
  });

  it("falls back when the value is not JSON or storage throws", () => {
    stubStorage({ "clipdump.focus": "{not json" });
    expect(readFocus()).toEqual(emptyFocus());
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    });
    expect(readFocus()).toEqual(emptyFocus());
    expect(() => writeFocus(emptyFocus())).not.toThrow();
  });
});

describe("sanitizeFocus", () => {
  it("drops a malformed session instead of locking the list on it", () => {
    for (const session of [
      { taskId: "7", title: "x", startedAt: 1, durationMs: 5, endsAt: 6, pausedAt: null },
      { taskId: 7, title: "x", startedAt: 1, durationMs: 0, endsAt: 6, pausedAt: null },
      { taskId: 7, title: "x", startedAt: 10, durationMs: 5, endsAt: 6, pausedAt: null },
      { taskId: 7, title: "x", startedAt: 1, durationMs: 5, endsAt: 6, pausedAt: "soon" },
      "nonsense",
      [],
    ]) {
      expect(sanitizeFocus({ session }).session).toBeNull();
    }
  });

  it("treats a paused session as having used its pause", () => {
    const s = sanitizeFocus({
      session: { taskId: 1, title: "x", startedAt: 1, durationMs: 5, endsAt: 6, pausedAt: 3 },
    }).session;
    expect(s?.pauseUsed).toBe(true);
  });

  it("ignores anything that is not an object", () => {
    for (const raw of [null, undefined, 3, "x", []]) expect(sanitizeFocus(raw)).toEqual(emptyFocus());
  });

  it("keeps good log entries and parked thoughts and skips bad ones", () => {
    const d = sanitizeFocus({
      log: [
        { taskId: 1, title: "a", startedAt: 1, endedAt: 2, focusedMs: 5, outcome: "completed" },
        { taskId: 1, title: "a", startedAt: 1, endedAt: 2, focusedMs: -5, outcome: "completed" },
        { taskId: 1, title: "a", startedAt: 1, endedAt: 2, focusedMs: 5, outcome: "who knows" },
        null,
      ],
      parked: [
        { id: 3, text: " hi ", at: 1 },
        { id: 3, text: "dup", at: 1 },
        { id: 4, text: "  ", at: 1 },
        { id: "x", text: "no id", at: 1 },
      ],
      nextParkedId: 1,
    });
    expect(d.log).toHaveLength(1);
    expect(d.parked).toEqual([{ id: 3, text: "hi", at: 1 }]);
    // A new thought never reuses an id still in the list.
    expect(d.nextParkedId).toBe(4);
  });

  it("repairs the streak and the remembered length", () => {
    const d = sanitizeFocus({ streak: { current: 5, best: 2 }, minutes: 100000 });
    expect(d.streak).toEqual({ current: 5, best: 5 });
    expect(d.minutes).toBe(240);
    expect(sanitizeFocus({ streak: { current: -3, best: "x" }, minutes: "x" })).toMatchObject({
      streak: { current: 0, best: 0 },
      minutes: 30,
    });
  });
});
