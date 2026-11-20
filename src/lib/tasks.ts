// What the Tasks view shows, as pure functions (no DOM, no IPC), so the rules are unit tested.
//
// The view is built around one question, "what am I working on right now?", and keeps
// everything else out of the way until it is asked for:
//
//   Now      every task in flight, most recently touched first. Never hidden, never capped.
//   Up next  waiting tasks marked important, oldest first, at most NEXT_VISIBLE of them.
//   Later    waiting tasks not marked important. Hidden until asked for.
//   Done     finished in the last day. Hidden until asked for; a count stays in view.
//
// "Hidden" is a count, not a deletion: one key shows everything and the same key hides it again.

import type { ExtractedTask, Task, TaskKind, TaskSize, TaskStatus } from "@/lib/ipc";

/** How many "Up next" tasks show before the rest fold into the hidden count. */
export const NEXT_VISIBLE = 3;
/** A finished task counts as done "today" for this long. */
export const DONE_WINDOW_MS = 24 * 60 * 60 * 1000;
/** Longest title the backend accepts (`tasks::MAX_TITLE_CHARS`). */
export const MAX_TITLE_CHARS = 300;
/** Longest note the backend accepts (`tasks::MAX_NOTE_CHARS`). */
export const MAX_NOTE_CHARS = 1000;

/** A task that has not been touched for this long while in flight "has gone quiet". */
export const QUIET_DOING_MS = 3 * 24 * 60 * 60 * 1000;
/** ... and while waiting. */
export const QUIET_WAITING_MS = 14 * 24 * 60 * 60 * 1000;
/** "Defer" sets a quiet task aside for this long. */
export const DEFER_MS = 7 * 24 * 60 * 60 * 1000;
/** Dismissing the nudge ("not now") sets it aside for this long. */
export const SNOOZE_MS = 24 * 60 * 60 * 1000;

export type SectionKey = "now" | "next" | "later" | "done" | "quick";

export interface TaskSection {
  key: SectionKey;
  title: string;
  tasks: Task[];
}

export interface TaskView {
  /** The sections that have something to show, in display order. */
  sections: TaskSection[];
  /** Every selectable task, in display order (the sections flattened). */
  rows: Task[];
  /** Waiting tasks kept out of sight. 0 when everything is shown. */
  hiddenCount: number;
  /** Tasks in flight. */
  inFlight: number;
  /** Tasks finished within `DONE_WINDOW_MS`, shown or not. */
  doneRecently: number;
  /** Open tasks estimated as quick (about 15 minutes or less) that are not set aside. */
  quickCount: number;
  /** Open tasks the LLM has not estimated (yet). */
  unsized: number;
}

export interface ViewOptions {
  /** Show the hidden tasks (later, the rest of up next, done). */
  showHidden: boolean;
  /** Unix ms. */
  now: number;
  /**
   * "Quick wins": only the tasks estimated as quick, all of them, whether or not they would be
   * folded away. For when there are ten minutes to spare.
   */
  quickOnly?: boolean;
}

/** Whether a waiting task is set aside right now (deferred, and the day has not come). */
export function isDeferred(t: Task, now: number): boolean {
  return t.deferredUntil !== null && t.deferredUntil > now;
}

/** An open task that is a quick win: estimated quick, in flight or waiting and not set aside. */
function isQuickWin(t: Task, now: number): boolean {
  return t.size === "quick" && (t.status === "doing" || (t.status === "todo" && !isDeferred(t, now)));
}

export function buildTaskView(tasks: readonly Task[], { showHidden, now, quickOnly = false }: ViewOptions): TaskView {
  const quickCount = tasks.filter((t) => isQuickWin(t, now)).length;
  const unsized = tasks.filter((t) => t.status !== "done" && t.size === null).length;
  const doneRecently = tasks.filter(
    (t) => t.status === "done" && t.doneAt !== null && now - t.doneAt <= DONE_WINDOW_MS,
  ).length;
  const inFlightCount = tasks.filter((t) => t.status === "doing").length;
  if (quickOnly) {
    const doingQuick = tasks
      .filter((t) => t.status === "doing" && t.size === "quick")
      .sort((a, b) => b.touchedAt - a.touchedAt);
    // Important first, then the one that has waited longest: the order they would be picked in.
    const waitingQuick = tasks
      .filter((t) => t.status === "todo" && isQuickWin(t, now))
      .sort((a, b) => Number(b.important) - Number(a.important) || a.createdAt - b.createdAt);
    const quick: TaskSection[] = [
      { key: "now", title: "Now", tasks: doingQuick },
      { key: "quick", title: "Quick wins", tasks: waitingQuick },
    ];
    const sections = quick.filter((s) => s.tasks.length > 0);
    return {
      sections,
      rows: sections.flatMap((s) => s.tasks),
      hiddenCount: 0,
      inFlight: inFlightCount,
      doneRecently,
      quickCount,
      unsized,
    };
  }
  const doing = tasks.filter((t) => t.status === "doing").sort((a, b) => b.touchedAt - a.touchedAt);
  const waiting = tasks.filter((t) => t.status === "todo");
  const next = waiting.filter((t) => t.important).sort((a, b) => a.createdAt - b.createdAt);
  const later = waiting.filter((t) => !t.important).sort((a, b) => b.createdAt - a.createdAt);
  const done = tasks
    .filter((t) => t.status === "done" && t.doneAt !== null && now - t.doneAt <= DONE_WINDOW_MS)
    .sort((a, b) => (b.doneAt ?? 0) - (a.doneAt ?? 0));

  const shownNext = showHidden ? next : next.slice(0, NEXT_VISIBLE);
  const hiddenCount = showHidden ? 0 : next.length - shownNext.length + later.length;

  const all: TaskSection[] = [
    { key: "now", title: "Now", tasks: doing },
    { key: "next", title: "Up next", tasks: shownNext },
    { key: "later", title: "Later", tasks: showHidden ? later : [] },
    { key: "done", title: "Done", tasks: showHidden ? done : [] },
  ];
  const sections = all.filter((s) => s.tasks.length > 0);
  return {
    sections,
    rows: sections.flatMap((s) => s.tasks),
    hiddenCount,
    inFlight: doing.length,
    doneRecently,
    quickCount,
    unsized,
  };
}

/** Whether an open task has gone quiet: untouched for days and not set aside. */
export function isQuiet(t: Task, now: number): boolean {
  if (t.status === "done" || isDeferred(t, now)) return false;
  return now - t.touchedAt >= (t.status === "doing" ? QUIET_DOING_MS : QUIET_WAITING_MS);
}

/** The tasks that have gone quiet, the longest quiet first. */
export function quietTasks(tasks: readonly Task[], now: number): Task[] {
  return tasks.filter((t) => isQuiet(t, now)).sort((a, b) => a.touchedAt - b.touchedAt);
}

/** Short labels for an estimate, with a longer sentence for a tooltip. */
export const SIZE_LABEL: Record<TaskSize, string> = { quick: "15 min", short: "1 h", long: "Long" };
export const SIZE_TITLE: Record<TaskSize, string> = {
  quick: "Quick: about 15 minutes or less",
  short: "Short: up to about an hour",
  long: "Long: more than an hour",
};
export const KIND_LABEL: Record<TaskKind, string> = { quick: "Quick win", deep: "Deep work", waiting: "Waiting" };

/** Which tasks found in a text start ticked: the new ones, not the ones already on the list. */
export function defaultSelection(found: readonly ExtractedTask[]): boolean[] {
  return found.map((t) => t.duplicateOf === null);
}

/**
 * What the input holds when Enter adds a task. A lone `!` at the end marks it important
 * ("call the bank !"); a `!` stuck to a word stays part of the title ("ship it!").
 * Returns null for nothing to add.
 */
export function parseQuickAdd(text: string): { title: string; important: boolean } | null {
  let title = text.trim();
  let important = false;
  const m = /(?:^|\s)!$/.exec(title);
  if (m) {
    important = true;
    title = title.slice(0, m.index).trim();
  }
  return title === "" ? null : { title, important };
}

/** Enter on an empty input moves the selected task one step along: waiting, in flight, done, and around. */
export function nextStatus(status: TaskStatus): TaskStatus {
  return status === "todo" ? "doing" : status === "doing" ? "done" : "todo";
}

/** Shift+Enter: one step back, or null when there is none (a waiting task has nowhere to go back to). */
export function previousStatus(status: TaskStatus): TaskStatus | null {
  return status === "done" ? "doing" : status === "doing" ? "todo" : null;
}

/** The task that should be selected after the list changed: the same one, else its neighbor. */
export function reselect(
  rows: readonly Task[],
  previousRows: readonly Task[],
  selectedId: number | null,
): number | null {
  if (rows.length === 0) return null;
  if (selectedId !== null && rows.some((t) => t.id === selectedId)) return selectedId;
  const at = selectedId === null ? 0 : previousRows.findIndex((t) => t.id === selectedId);
  return rows[Math.min(Math.max(at, 0), rows.length - 1)].id;
}
