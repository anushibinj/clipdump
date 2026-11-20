// What the task assistant's panel shows, as plain data. The panel is one card above the list that
// holds the answer to the last thing asked: what to do now, a next step, a digest, links, or tasks
// found in a text. The hook (`hooks/useTaskAi.ts`) moves it through loading, error and done.

import type { Energy, ExtractResult, NextAction, TaskDigest, TaskPick } from "@/lib/ipc";

/** One request: waiting for the server, failed with a plain sentence, or answered. */
export type Async<T> =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "done"; value: T };

/** A suggested link with what the clipboard item says, so it can be judged without opening it. */
export interface LinkRow {
  taskId: number;
  clipId: number;
  reason: string;
  /** One line of the clipboard item, or "" when it has left the history. */
  preview: string;
}

export type AiPanel =
  | { kind: "pick"; minutes: number | null; energy: Energy; result: Async<TaskPick | null> }
  | { kind: "steps"; taskId: number; result: Async<NextAction> }
  | { kind: "digest"; period: "morning" | "week"; result: Async<TaskDigest> }
  | { kind: "links"; result: Async<LinkRow[]> }
  | {
      kind: "extract";
      /** What the person typed or pasted. Empty means "use what was just copied". */
      text: string;
      result: Async<ExtractResult> | null;
      /** Which of the found tasks are ticked. */
      selected: boolean[];
    };

/** The "time I have" choices of the pick panel. `null` is "no limit". */
export const MINUTE_CHOICES: readonly { label: string; minutes: number | null }[] = [
  { label: "10 min", minutes: 10 },
  { label: "30 min", minutes: 30 },
  { label: "1 h", minutes: 60 },
  { label: "Any", minutes: null },
];

export const ENERGY_CHOICES: readonly { label: string; energy: Energy }[] = [
  { label: "Low", energy: "low" },
  { label: "Normal", energy: "normal" },
  { label: "High", energy: "high" },
];

/** A cached answer is reused for this long, so reopening the digest is instant. */
export const DIGEST_FRESH_MS = 30 * 60 * 1000;

/** Whether a cached digest taken at `at` can still be shown at `now`. */
export function digestIsFresh(at: number, now: number): boolean {
  return now - at >= 0 && now - at < DIGEST_FRESH_MS;
}
