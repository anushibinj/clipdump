// Keys inside the Notes view. Pure like `taskKeys.ts`: a key event goes in, an action comes
// out. Plain letters are never taken (the filter box and the editor are for typing).

import type { KeyInfo } from "@/lib/shortcuts";

export type NoteAction =
  /** Ctrl+Enter: a new empty note, with the caret in its body. */
  | { type: "new" }
  /** Ctrl+P: pin or unpin the selected note. */
  | { type: "togglePin" }
  /** Ctrl+Delete or Shift+Delete: delete the selected note (press again to confirm). */
  | { type: "delete" }
  /** Ctrl+E: the caret goes to the selected note's body. */
  | { type: "editBody" }
  /** Ctrl+G: the caret goes to the selected note's tag box. */
  | { type: "editTags" }
  | { type: "move"; delta: number }
  /** Esc: clear the filter, else close the bar. */
  | { type: "escape" };

/** How far PageUp and PageDown move the selection. */
export const NOTE_PAGE_STEP = 6;

/** Keys of the filter box. */
export function resolveNoteKey(e: KeyInfo): NoteAction | null {
  if (e.isComposing || e.keyCode === 229) return null;
  const { ctrlKey: ctrl, shiftKey: shift, altKey: alt, metaKey: meta } = e;
  if (meta) return null;
  const letter = (k: string) => e.code === `Key${k.toUpperCase()}` || e.key.toLowerCase() === k;

  if (e.key === "Escape" && !ctrl && !alt && !shift) return { type: "escape" };
  if (e.key === "Enter" && ctrl && !alt && !shift) return { type: "new" };
  if (ctrl && !alt && !shift) {
    if (letter("p")) return { type: "togglePin" };
    if (letter("e")) return { type: "editBody" };
    if (letter("g")) return { type: "editTags" };
  }
  if (e.key === "Delete" && !alt && (ctrl || shift)) return { type: "delete" };
  if (!ctrl && !alt && !shift) {
    if (e.key === "ArrowDown") return { type: "move", delta: 1 };
    if (e.key === "ArrowUp") return { type: "move", delta: -1 };
    if (e.key === "PageDown") return { type: "move", delta: NOTE_PAGE_STEP };
    if (e.key === "PageUp") return { type: "move", delta: -NOTE_PAGE_STEP };
  }
  return null;
}
