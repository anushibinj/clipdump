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
  /** Ctrl+N: ask the LLM what to do now. */
  | { type: "pickNext" }
  /** Ctrl+Q: only the quick wins. */
  | { type: "toggleQuick" }
  /** Ctrl+G: a concrete next step (and subtasks) for the selected task. */
  | { type: "nextStep" }
  /** Ctrl+R: draft "where I left off" from what was copied since. */
  | { type: "draftNote" }
  /** Ctrl+D: this morning's briefing; Ctrl+Shift+D: the week's recap. */
  | { type: "digest"; period: "morning" | "week" }
  /** Ctrl+L: which recent clipboard items belong to which task. */
  | { type: "links" }
  /** Ctrl+K: find tasks in a pasted or just copied text. */
  | { type: "extract" }
  /** Ctrl+B: set the selected task aside for a week. */
  | { type: "defer" }
  /** Alt+Enter: take the main suggestion of the assistant's card (start the picked task, add the subtasks, ...). */
  | { type: "panelPrimary" }
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

  if (e.key === "Enter" && alt && !ctrl && !shift) return state.editing ? null : { type: "panelPrimary" };

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
    // The assistant's chords. Not while a title is being edited: the input then holds a title.
    if (!state.editing) {
      if (letter("n")) return { type: "pickNext" };
      if (letter("q")) return { type: "toggleQuick" };
      if (letter("g")) return { type: "nextStep" };
      if (letter("r")) return { type: "draftNote" };
      if (letter("d")) return { type: "digest", period: "morning" };
      if (letter("l")) return { type: "links" };
      if (letter("k")) return { type: "extract" };
      if (letter("b")) return { type: "defer" };
    }
  }
  if (ctrl && shift && !alt && !state.editing && letter("d")) return { type: "digest", period: "week" };
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
