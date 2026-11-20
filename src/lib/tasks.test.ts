import { describe, expect, it } from "vitest";
import type { Task } from "./ipc";
import { resolveShortcut, type KeyInfo } from "./shortcuts";
import { resolveTaskKey } from "./taskKeys";
import {
  DEFER_MS,
  DONE_WINDOW_MS,
  NEXT_VISIBLE,
  QUIET_DOING_MS,
  QUIET_WAITING_MS,
  buildTaskView,
  defaultSelection,
  isDeferred,
  isQuiet,
  quietTasks,
  nextStatus,
  parseQuickAdd,
  previousStatus,
  reselect,
} from "./tasks";

const NOW = 1_000_000_000_000;
let n = 0;
function task(p: Partial<Task> = {}): Task {
  n += 1;
  return {
    id: n,
    title: `task ${n}`,
    note: "",
    status: "todo",
    important: false,
    createdAt: NOW - 1000 * (100 - n),
    touchedAt: NOW - 1000 * (100 - n),
    doneAt: null,
    size: null,
    kind: null,
    nextStep: "",
    clipIds: [],
    deferredUntil: null,
    ...p,
  };
}
const ids = (ts: readonly Task[]) => ts.map((t) => t.id);

describe("buildTaskView", () => {
  it("shows every task in flight, most recently touched first, however many there are", () => {
    const a = task({ status: "doing", touchedAt: NOW - 5000 });
    const b = task({ status: "doing", touchedAt: NOW - 1000 });
    const c = task({ status: "doing", touchedAt: NOW - 9000 });
    const v = buildTaskView([a, b, c], { showHidden: false, now: NOW });
    expect(v.sections.map((s) => s.key)).toEqual(["now"]);
    expect(ids(v.rows)).toEqual([b.id, a.id, c.id]);
    expect(v.inFlight).toBe(3);
    expect(v.hiddenCount).toBe(0);
  });

  it("hides tasks that are not important until asked, and counts them", () => {
    const now1 = task({ status: "doing" });
    const imp = task({ important: true });
    const plain1 = task();
    const plain2 = task();
    const calm = buildTaskView([now1, imp, plain1, plain2], { showHidden: false, now: NOW });
    expect(calm.sections.map((s) => s.key)).toEqual(["now", "next"]);
    expect(ids(calm.rows)).toEqual([now1.id, imp.id]);
    expect(calm.hiddenCount).toBe(2);

    const all = buildTaskView([now1, imp, plain1, plain2], { showHidden: true, now: NOW });
    expect(all.sections.map((s) => s.key)).toEqual(["now", "next", "later"]);
    expect(all.hiddenCount).toBe(0);
    expect(all.rows).toHaveLength(4);
  });

  it("caps up next and folds the rest into the hidden count", () => {
    const imp = Array.from({ length: NEXT_VISIBLE + 2 }, () => task({ important: true }));
    const calm = buildTaskView(imp, { showHidden: false, now: NOW });
    expect(calm.rows).toHaveLength(NEXT_VISIBLE);
    expect(calm.hiddenCount).toBe(2);
    // Oldest first: what has waited longest comes first.
    expect(ids(calm.rows)).toEqual(ids(imp.slice(0, NEXT_VISIBLE)));
    expect(buildTaskView(imp, { showHidden: true, now: NOW }).rows).toHaveLength(imp.length);
  });

  it("lists later tasks newest first", () => {
    const old = task({ createdAt: NOW - 5000 });
    const fresh = task({ createdAt: NOW - 1000 });
    const v = buildTaskView([old, fresh], { showHidden: true, now: NOW });
    expect(ids(v.rows)).toEqual([fresh.id, old.id]);
  });

  it("keeps done tasks out of sight but counts the recent ones", () => {
    const recent = task({ status: "done", doneAt: NOW - 1000 });
    const edge = task({ status: "done", doneAt: NOW - DONE_WINDOW_MS });
    const stale = task({ status: "done", doneAt: NOW - DONE_WINDOW_MS - 1 });
    const calm = buildTaskView([recent, edge, stale], { showHidden: false, now: NOW });
    expect(calm.rows).toEqual([]);
    expect(calm.doneRecently).toBe(2);
    expect(calm.hiddenCount).toBe(0);
    const all = buildTaskView([recent, edge, stale], { showHidden: true, now: NOW });
    expect(ids(all.rows)).toEqual([recent.id, edge.id]);
  });

  it("is empty for no tasks", () => {
    const v = buildTaskView([], { showHidden: false, now: NOW });
    expect(v.sections).toEqual([]);
    expect(v.rows).toEqual([]);
    expect(v.hiddenCount).toBe(0);
  });
});

describe("parseQuickAdd", () => {
  it("takes the text as the title", () => {
    expect(parseQuickAdd("  write the report  ")).toEqual({ title: "write the report", important: false });
  });
  it("marks a lone trailing ! as important", () => {
    expect(parseQuickAdd("call the bank !")).toEqual({ title: "call the bank", important: true });
    expect(parseQuickAdd("call the bank  !  ")).toEqual({ title: "call the bank", important: true });
  });
  it("keeps a ! that belongs to a word", () => {
    expect(parseQuickAdd("ship it!")).toEqual({ title: "ship it!", important: false });
    expect(parseQuickAdd("!important thing")).toEqual({ title: "!important thing", important: false });
  });
  it("has nothing to add for blanks or a bare !", () => {
    expect(parseQuickAdd("")).toBeNull();
    expect(parseQuickAdd("   ")).toBeNull();
    expect(parseQuickAdd("!")).toBeNull();
    expect(parseQuickAdd(" ! ")).toBeNull();
  });
});

describe("status steps", () => {
  it("goes waiting, in flight, done, and around", () => {
    expect(nextStatus("todo")).toBe("doing");
    expect(nextStatus("doing")).toBe("done");
    expect(nextStatus("done")).toBe("todo");
  });
  it("steps back, and says when there is no step", () => {
    expect(previousStatus("done")).toBe("doing");
    expect(previousStatus("doing")).toBe("todo");
    expect(previousStatus("todo")).toBeNull();
  });
});

describe("reselect", () => {
  const a = task();
  const b = task();
  const c = task();
  it("keeps the selection while it is listed", () => {
    expect(reselect([a, b, c], [a, b, c], b.id)).toBe(b.id);
  });
  it("moves to the neighbor at the same position when the selection leaves", () => {
    expect(reselect([a, c], [a, b, c], b.id)).toBe(c.id);
    expect(reselect([a, b], [a, b, c], c.id)).toBe(b.id);
  });
  it("starts on the first row and is null for none", () => {
    expect(reselect([a, b], [], null)).toBe(a.id);
    expect(reselect([], [a], a.id)).toBeNull();
  });
});

function key(k: string, mods: Partial<KeyInfo> = {}): KeyInfo {
  return {
    key: k,
    code: k.length === 1 ? `Key${k.toUpperCase()}` : k,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    ...mods,
  };
}
const empty = { inputEmpty: true, editing: false };
const typed = { inputEmpty: false, editing: false };
const editing = { inputEmpty: false, editing: true };

describe("resolveTaskKey", () => {
  it("Enter adds typed text and advances the selection when the input is empty", () => {
    expect(resolveTaskKey(key("Enter"), typed)).toEqual({ type: "add" });
    expect(resolveTaskKey(key("Enter"), empty)).toEqual({ type: "advance" });
  });
  it("Ctrl+Enter adds and starts, or finishes when the input is empty", () => {
    expect(resolveTaskKey(key("Enter", { ctrlKey: true }), typed)).toEqual({ type: "addStart" });
    expect(resolveTaskKey(key("Enter", { ctrlKey: true }), empty)).toEqual({ type: "toggleDone" });
  });
  it("Shift+Enter steps back only on an empty input", () => {
    expect(resolveTaskKey(key("Enter", { shiftKey: true }), empty)).toEqual({ type: "stepBack" });
    expect(resolveTaskKey(key("Enter", { shiftKey: true }), typed)).toBeNull();
  });
  it("Enter commits an edit and nothing else adds while editing", () => {
    expect(resolveTaskKey(key("Enter"), editing)).toEqual({ type: "commitEdit" });
    expect(resolveTaskKey(key("Enter", { ctrlKey: true }), editing)).toBeNull();
  });
  it("maps the chords", () => {
    expect(resolveTaskKey(key("i", { ctrlKey: true }), empty)).toEqual({ type: "toggleImportant" });
    expect(resolveTaskKey(key("e", { ctrlKey: true }), empty)).toEqual({ type: "editNote" });
    expect(resolveTaskKey(key("h", { ctrlKey: true }), empty)).toEqual({ type: "toggleHidden" });
    expect(resolveTaskKey(key("l", { ctrlKey: true }), empty)).toEqual({ type: "focus" });
    expect(resolveTaskKey(key("p", { ctrlKey: true }), empty)).toEqual({ type: "park" });
    expect(resolveTaskKey(key("F2"), empty)).toEqual({ type: "editTitle" });
    expect(resolveTaskKey(key("Delete", { ctrlKey: true }), empty)).toEqual({ type: "delete" });
    expect(resolveTaskKey(key("Delete", { shiftKey: true }), empty)).toEqual({ type: "delete" });
    expect(resolveTaskKey(key("ArrowDown"), empty)).toEqual({ type: "move", delta: 1 });
    expect(resolveTaskKey(key("PageUp"), empty)).toEqual({ type: "move", delta: -5 });
    expect(resolveTaskKey(key("Escape"), empty)).toEqual({ type: "escape" });
  });
  it("maps the assistant's chords, and leaves them out while a title is edited", () => {
    const chord = (k: string, extra: Partial<KeyInfo> = {}) =>
      resolveTaskKey(key(k, { ctrlKey: true, ...extra }), empty);
    expect(chord("n")).toEqual({ type: "pickNext" });
    expect(chord("q")).toEqual({ type: "toggleQuick" });
    expect(chord("g")).toEqual({ type: "nextStep" });
    expect(chord("r")).toEqual({ type: "draftNote" });
    expect(chord("d")).toEqual({ type: "digest", period: "morning" });
    expect(chord("d", { shiftKey: true })).toEqual({ type: "digest", period: "week" });
    expect(chord("j")).toEqual({ type: "links" });
    expect(chord("k")).toEqual({ type: "extract" });
    expect(chord("b")).toEqual({ type: "defer" });
    for (const k of ["n", "q", "g", "r", "d", "j", "k", "b"]) {
      expect(resolveTaskKey(key(k, { ctrlKey: true }), editing)).toBeNull();
    }
    expect(resolveTaskKey(key("d", { ctrlKey: true, shiftKey: true }), editing)).toBeNull();
    // Plain letters are still typing, and Alt turns a chord into something else.
    expect(resolveTaskKey(key("n"), empty)).toBeNull();
    expect(chord("n", { altKey: true })).toBeNull();
  });
  it("Alt+Enter takes the assistant's suggestion, except while a title is edited", () => {
    expect(resolveTaskKey(key("Enter", { altKey: true }), empty)).toEqual({ type: "panelPrimary" });
    expect(resolveTaskKey(key("Enter", { altKey: true }), typed)).toEqual({ type: "panelPrimary" });
    expect(resolveTaskKey(key("Enter", { altKey: true }), editing)).toBeNull();
    expect(resolveTaskKey(key("Enter", { altKey: true, ctrlKey: true }), empty)).toBeNull();
  });
  it("leaves typing, plain Delete and composition alone", () => {
    expect(resolveTaskKey(key("a"), empty)).toBeNull();
    expect(resolveTaskKey(key("Delete"), typed)).toBeNull();
    expect(resolveTaskKey(key("i"), typed)).toBeNull();
    expect(resolveTaskKey(key("Enter", { isComposing: true }), typed)).toBeNull();
    expect(resolveTaskKey(key("Enter", { keyCode: 229 }), typed)).toBeNull();
    expect(resolveTaskKey(key("i", { ctrlKey: true, metaKey: true }), empty)).toBeNull();
  });
});

describe("resolveShortcut with the task view", () => {
  it("Ctrl+T switches from every view", () => {
    for (const view of ["search", "tasks", "settings"] as const) {
      expect(resolveShortcut(key("t", { ctrlKey: true }), view)).toEqual({ type: "toggleTasks" });
    }
  });
  it("leaves Esc and the other keys to the task view", () => {
    expect(resolveShortcut(key("Escape"), "tasks")).toBeNull();
    expect(resolveShortcut(key("Enter"), "tasks")).toBeNull();
    expect(resolveShortcut(key(",", { ctrlKey: true }), "tasks")).toEqual({ type: "toggleSettings" });
  });
  it("still closes with Esc elsewhere", () => {
    expect(resolveShortcut(key("Escape"), "search")).toEqual({ type: "escape" });
    expect(resolveShortcut(key("Escape"), "settings")).toEqual({ type: "escape" });
  });
});

describe("quick wins", () => {
  it("shows every quick task, in flight first, important first, then the one waiting longest", () => {
    const running = task({ status: "doing", size: "quick" });
    const plain = task({ size: "quick", createdAt: NOW - 9000 });
    const old = task({ size: "quick", createdAt: NOW - 90000 });
    const star = task({ size: "quick", important: true, createdAt: NOW - 100 });
    const long = task({ size: "long", important: true });
    const unknown = task({ important: true });
    const v = buildTaskView([long, unknown, plain, star, running, old], {
      showHidden: false,
      now: NOW,
      quickOnly: true,
    });
    expect(v.sections.map((s) => s.key)).toEqual(["now", "quick"]);
    expect(ids(v.rows)).toEqual([running.id, star.id, old.id, plain.id]);
    expect(v.hiddenCount).toBe(0);
  });

  it("does not fold quick tasks away the way the calm view folds the later ones", () => {
    const many = Array.from({ length: NEXT_VISIBLE + 3 }, () => task({ size: "quick" }));
    const calm = buildTaskView(many, { showHidden: false, now: NOW });
    expect(calm.rows.length).toBe(0);
    const quick = buildTaskView(many, { showHidden: false, now: NOW, quickOnly: true });
    expect(quick.rows).toHaveLength(NEXT_VISIBLE + 3);
  });

  it("leaves out done tasks and tasks set aside, and counts quick wins and the unestimated", () => {
    const aside = task({ size: "quick", deferredUntil: NOW + 1000 });
    const back = task({ size: "quick", deferredUntil: NOW - 1000 });
    const done = task({ size: "quick", status: "done", doneAt: NOW - 1000 });
    const none = task();
    const v = buildTaskView([aside, back, done, none], { showHidden: true, now: NOW, quickOnly: true });
    expect(ids(v.rows)).toEqual([back.id]);
    expect(v.quickCount).toBe(1);
    expect(v.unsized).toBe(1);
    // The counts are the same with the filter off.
    const all = buildTaskView([aside, back, done, none], { showHidden: true, now: NOW });
    expect([all.quickCount, all.unsized]).toEqual([1, 1]);
  });
});

describe("tasks that have gone quiet", () => {
  it("is a task in flight untouched for days, or a waiting one untouched for weeks", () => {
    const doingOld = task({ status: "doing", touchedAt: NOW - QUIET_DOING_MS });
    const doingNew = task({ status: "doing", touchedAt: NOW - QUIET_DOING_MS + 1 });
    const waitingOld = task({ touchedAt: NOW - QUIET_WAITING_MS });
    const waitingNew = task({ touchedAt: NOW - QUIET_DOING_MS * 2 });
    const done = task({ status: "done", touchedAt: 0, doneAt: 0 });
    expect([doingOld, doingNew, waitingOld, waitingNew, done].map((t) => isQuiet(t, NOW))).toEqual([
      true,
      false,
      true,
      false,
      false,
    ]);
  });

  it("is not nagged about while set aside, and is again once the day has come", () => {
    const t = task({ touchedAt: 0, deferredUntil: NOW + DEFER_MS });
    expect(isDeferred(t, NOW)).toBe(true);
    expect(isQuiet(t, NOW)).toBe(false);
    expect(isQuiet(t, NOW + DEFER_MS)).toBe(true);
    expect(isDeferred(task(), NOW)).toBe(false);
  });

  it("lists the longest quiet first", () => {
    const a = task({ touchedAt: NOW - QUIET_WAITING_MS - 5000 });
    const b = task({ touchedAt: NOW - QUIET_WAITING_MS - 9000 });
    const fresh = task();
    expect(ids(quietTasks([a, fresh, b], NOW))).toEqual([b.id, a.id]);
  });
});

describe("tasks found in a text", () => {
  it("starts with the new ones ticked and the ones already on the list not", () => {
    expect(
      defaultSelection([
        { title: "a", important: false, duplicateOf: null },
        { title: "b", important: true, duplicateOf: 7 },
      ]),
    ).toEqual([true, false]);
  });
});
