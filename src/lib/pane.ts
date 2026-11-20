// What the right pane shows (the item preview or the AI split) and whether it is open. Webview
// presentation choices like the theme, so they live in localStorage and are read before the first
// paint.

export type RightTab = "preview" | "split";

const KEY = "clipdump.rightTab";

export function readRightTab(): RightTab {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "preview" || v === "split") return v;
  } catch {
    // Storage can be blocked: start on the preview.
  }
  return "preview";
}

export function writeRightTab(tab: RightTab): void {
  try {
    localStorage.setItem(KEY, tab);
  } catch {
    // Not persisted; the choice still applies for this session.
  }
}

const OPEN_KEY = "clipdump.paneOpen";

export function readPaneOpen(): boolean {
  try {
    return localStorage.getItem(OPEN_KEY) === "1";
  } catch {
    return false;
  }
}

export function writePaneOpen(open: boolean): void {
  try {
    localStorage.setItem(OPEN_KEY, open ? "1" : "0");
  } catch {
    // Not persisted; the choice still applies for this session.
  }
}
