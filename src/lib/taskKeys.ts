// Keys inside the Tasks view. Pure like `shortcuts.ts`: a key event and the state of the input
// go in, an action comes out. The view's own input is the one place text is typed, so plain
// letters are never taken; everything here is Enter, arrows, Esc or a Ctrl chord.

import type { KeyInfo } from "@/lib/shortcuts";

export type TaskAction =
  /** Enter with text: add it, waiting. */
  | { type: "add" }
  /** Ctrl+Enter with text: add it and start it. */
  | { type: "addStart" }
  /** Enter on an empty input: waiting, in flight, done, and around. */
  | { type: "advance" }
  /** Ctrl+Enter on an empty input: finish (or reopen) the selected task. */
  | { type: "toggleDone" }
  /** Shift+Enter on an empty input: one step back. */
  | { type: "stepBack" }
  | { type: "toggleImportant" }
  /** Ctrl+E: write where you left off. */
  | { type: "editNote" }
  /** F2: change the title. */
  | { type: "editTitle" }
  /** Enter while editing. */
  | { type: "commitEdit" }
  | { type: "toggleHidden" }
  /** Ctrl+L: lock the list to the selected task for a while (focus mode). */
  | { type: "focus" }
  /** Ctrl+P: write a distraction down instead of acting on it. */
  | { type: "park" }
  | { type: "delete" }
  | { type: "move"; delta: number }
  /** Esc: leave an edit, else clear the input, else close the bar. */
  | { type: "escape" };

export interface TaskKeyState {
  /** The input holds nothing but blanks. */
  inputEmpty: boolean;
  /** The input is editing a note or a title instead of adding a task. */
  editing: boolean;
}

/** How far PageUp and PageDown move the selection. */
export const TASK_PAGE_STEP = 5;

export function resolveTaskKey(e: KeyInfo, state: TaskKeyState): TaskAction | null {
  if (e.isComposing || e.keyCode === 229) return null;
  const { ctrlKey: ctrl, shiftKey: shift, altKey: alt, metaKey: meta } = e;
  if (meta) return null;
  const letter = (k: string) => e.code === `Key${k.toUpperCase()}` || e.key.toLowerCase() === k;

  if (e.key === "Escape" && !ctrl && !alt && !shift) return { type: "escape" };

  if (e.key === "Enter" && !alt) {
    if (state.editing) return ctrl || shift ? null : { type: "commitEdit" };
    if (ctrl && !shift) return state.inputEmpty ? { type: "toggleDone" } : { type: "addStart" };
    if (shift && !ctrl) return state.inputEmpty ? { type: "stepBack" } : null;
    if (!ctrl && !shift) return state.inputEmpty ? { type: "advance" } : { type: "add" };
    return null;
  }

  if (ctrl && !alt && !shift) {
    if (letter("i")) return { type: "toggleImportant" };
    if (letter("e")) return { type: "editNote" };
    if (letter("h")) return { type: "toggleHidden" };
    if (letter("l")) return { type: "focus" };
    if (letter("p")) return { type: "park" };
  }
  if (e.key === "F2" && !ctrl && !alt && !shift) return { type: "editTitle" };
  if (e.key === "Delete" && !alt && (ctrl || shift)) return { type: "delete" };

  if (!ctrl && !alt && !shift) {
    if (e.key === "ArrowDown") return { type: "move", delta: 1 };
    if (e.key === "ArrowUp") return { type: "move", delta: -1 };
    if (e.key === "PageDown") return { type: "move", delta: TASK_PAGE_STEP };
    if (e.key === "PageUp") return { type: "move", delta: -TASK_PAGE_STEP };
  }
  return null;
}
