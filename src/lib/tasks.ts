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

import type { Task, TaskStatus } from "@/lib/ipc";

/** How many "Up next" tasks show before the rest fold into the hidden count. */
export const NEXT_VISIBLE = 3;
/** A finished task counts as done "today" for this long. */
export const DONE_WINDOW_MS = 24 * 60 * 60 * 1000;
/** Longest title the backend accepts (`tasks::MAX_TITLE_CHARS`). */
export const MAX_TITLE_CHARS = 300;
/** Longest note the backend accepts (`tasks::MAX_NOTE_CHARS`). */
export const MAX_NOTE_CHARS = 1000;

export type SectionKey = "now" | "next" | "later" | "done";

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
}

export interface ViewOptions {
  /** Show the hidden tasks (later, the rest of up next, done). */
  showHidden: boolean;
  /** Unix ms. */
  now: number;
}

export function buildTaskView(tasks: readonly Task[], { showHidden, now }: ViewOptions): TaskView {
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
    doneRecently: tasks.filter(
      (t) => t.status === "done" && t.doneAt !== null && now - t.doneAt <= DONE_WINDOW_MS,
    ).length,
  };
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
