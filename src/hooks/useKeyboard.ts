import { useCallback, useRef, type KeyboardEvent } from "react";
import { resolveShortcut, type Action, type View } from "@/lib/shortcuts";

/**
 * Turns key presses into actions (lib/shortcuts.ts holds the table) and
 * returns a capture phase handler for the palette root.
 *
 * - Handled keys are `preventDefault`ed (so Ctrl+P does not print, Ctrl+Delete
 *   does not delete a word, Tab does not leave the input).
 * - In the search view every key is also stopped from reaching cmdk, whose own
 *   keyboard handling is replaced by this one (it would swallow Home and End,
 *   which belong to the input's caret).
 * - Key repeat is honored for moving only; holding Ctrl+Delete must not delete a
 *   whole page of items.
 *
 * `run` can change on every render; the returned handler keeps one identity.
 */
export function useKeyboard(view: View, run: (action: Action) => void) {
  const latest = useRef({ view, run });
  latest.current = { view, run };

  return useCallback((e: KeyboardEvent<HTMLElement>) => {
    const { view: v, run: dispatch } = latest.current;
    const action = resolveShortcut(e.nativeEvent, v);
    if (action) {
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat && action.type !== "move") return;
      dispatch(action);
    } else if (v === "search") {
      e.stopPropagation();
    }
  }, []);
}
