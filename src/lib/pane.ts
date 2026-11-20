// Which tab the right pane shows: the item preview or the AI split. A webview presentation
// choice like the theme, so it lives in localStorage and is read before the first paint.

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
