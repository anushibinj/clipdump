// Keyboard shortcut table (PLAN.md section 1). Pure: a key event goes in, an
// action comes out, so the whole mapping is unit tested without a DOM.

import type { TypeFilter } from "@/lib/ipc";

export interface KeyInfo {
  key: string;
  /** Physical key (`Digit1`, `KeyP`, ...). Layout independent, so Alt+digit works on every layout. */
  code: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  /** An input method editor is composing text (CJK, dead keys): never steal those keys. */
  isComposing?: boolean;
  keyCode?: number;
}

export type View = "search" | "settings" | "tasks";

export type Action =
  | { type: "copy"; paste: boolean }
  /** Plain Enter: copies, and auto-pastes when that setting is on (the caller decides). */
  | { type: "enter" }
  | { type: "copyNth"; n: number }
  | { type: "togglePin" }
  | { type: "delete" }
  | { type: "clear" }
  | { type: "filter"; filter: TypeFilter }
  | { type: "togglePreview" }
  /** Ctrl+Shift+S: open the preview and split the selected text into copyable items. */
  | { type: "split" }
  | { type: "toggleSettings" }
  /** Ctrl+?: open or close the shortcuts help. */
  | { type: "toggleShortcuts" }
  /** Ctrl+T: between the clipboard search and the task list. */
  | { type: "toggleTasks" }
  | { type: "move"; delta: number }
  | { type: "escape" };

/** The order of `Alt+1..4`. */
export const FILTER_ORDER: readonly TypeFilter[] = ["all", "text", "images", "files"];

/** How far PageUp and PageDown move the selection. */
export const PAGE_STEP = 8;

function digit(e: KeyInfo): number | null {
  const m = /^(?:Digit|Numpad)([0-9])$/.exec(e.code);
  if (m) return Number(m[1]);
  return /^[0-9]$/.test(e.key) ? Number(e.key) : null;
}

/** The action for a key press, or `null` when the key is left to the browser (typing, caret keys). */
export function resolveShortcut(e: KeyInfo, view: View): Action | null {
  if (e.isComposing || e.keyCode === 229) return null;
  const { ctrlKey: ctrl, shiftKey: shift, altKey: alt, metaKey: meta } = e;
  if (meta) return null;

  // The task view peels its own layers with Esc (an edit, the typed text, then the bar).
  if (e.key === "Escape" && !ctrl && !alt && !shift && view !== "tasks") return { type: "escape" };
  if (ctrl && !alt && !shift && (e.key === "," || e.code === "Comma")) {
    return { type: "toggleSettings" };
  }
  // Ctrl+? (Ctrl+Shift+/ on a US layout). Matched by character or by the physical slash key.
  if (ctrl && !alt && (e.key === "?" || (shift && e.code === "Slash"))) {
    return { type: "toggleShortcuts" };
  }
  if (ctrl && !alt && !shift && (e.code === "KeyT" || e.key.toLowerCase() === "t")) {
    return { type: "toggleTasks" };
  }
  // The settings and task views have their own fields and keys: only Esc, Ctrl+, Ctrl+? and Ctrl+T act here.
  if (view !== "search") return null;

  const d = digit(e);
  if (alt && !ctrl && !shift && d !== null && d >= 1 && d <= FILTER_ORDER.length) {
    return { type: "filter", filter: FILTER_ORDER[d - 1] };
  }
  if (ctrl && !alt && !shift && d !== null && d >= 1 && d <= 9) {
    return { type: "copyNth", n: d };
  }
  if (ctrl && shift && !alt && (e.code === "KeyS" || e.key.toLowerCase() === "s")) {
    return { type: "split" };
  }
  if (ctrl && !alt && !shift && (e.code === "KeyP" || e.key.toLowerCase() === "p")) {
    return { type: "togglePin" };
  }
  if (e.key === "Delete" && !alt) {
    if (ctrl && shift) return { type: "clear" };
    if (ctrl || shift) return { type: "delete" };
    return null; // plain Delete edits the query
  }
  if (e.key === "Enter" && !alt && !shift) {
    return ctrl ? { type: "copy", paste: false } : { type: "enter" };
  }
  if (e.key === "Tab" && !ctrl && !alt && !shift) return { type: "togglePreview" };
  if (!ctrl && !alt && !shift) {
    if (e.key === "ArrowDown") return { type: "move", delta: 1 };
    if (e.key === "ArrowUp") return { type: "move", delta: -1 };
    if (e.key === "PageDown") return { type: "move", delta: PAGE_STEP };
    if (e.key === "PageUp") return { type: "move", delta: -PAGE_STEP };
  }
  return null;
}

/** Next index for an arrow move: wraps at both ends; page moves stop at the ends. */
export function moveIndex(index: number, delta: number, length: number): number {
  if (length <= 0) return -1;
  if (Math.abs(delta) === 1) return (index + delta + length) % length;
  return Math.min(length - 1, Math.max(0, index + delta));
}
