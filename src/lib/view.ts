// Which view the bar shows (search, tasks or settings), remembered across openings and restarts.
//
// Design rule: the bar always opens on the view it was left on. Nothing resets the view when the bar
// is hidden or shown, and any new view must be added to `VIEWS` so it is restored too. Like the
// theme and the right pane (`pane.ts`) this is a webview presentation choice, kept in localStorage
// and read before the first paint.

import type { View } from "@/lib/shortcuts";

export const VIEWS: readonly View[] = ["search", "tasks", "settings"];

const KEY = "clipdump.view";

/** The view the bar was left on, or the clipboard search the first time (and when storage is blocked). */
export function readView(): View {
  try {
    const v = localStorage.getItem(KEY);
    if (v && (VIEWS as readonly string[]).includes(v)) return v as View;
  } catch {
    // Storage can be blocked: start on the search.
  }
  return "search";
}

export function writeView(view: View): void {
  try {
    localStorage.setItem(KEY, view);
  } catch {
    // Not persisted; the view still applies for this session.
  }
}
