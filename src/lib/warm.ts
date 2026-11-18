// Pre-warming of the hidden window (Phase 14). The webview is created at startup and loads this
// page while the window is hidden, so by the first hotkey press everything that is slow the first
// time has already happened: the page and its script are loaded, React has mounted, the first
// (empty) search has been answered and its rows rendered. This file covers the two things that
// would otherwise still wait for the first paint of a visible window: the font file and the
// thumbnails of the first rows. Nothing here runs per keystroke.

/** Text the bar shows, so the font subsets it needs are the ones that get loaded. */
const SAMPLE = "Search text, screenshots, files... 0123456789 Ctrl+Enter Esc";

/** Load and decode the UI font now instead of at the first paint. Safe where `document.fonts` is missing. */
export function warmFonts(): Promise<unknown> {
  if (typeof document === "undefined" || !document.fonts?.load) return Promise.resolve();
  return Promise.all([
    document.fonts.load('400 14px "Geist Variable"', SAMPLE),
    document.fonts.load('500 14px "Geist Variable"', SAMPLE),
  ]).catch(() => {});
}

/** The thumbnails worth fetching ahead of the first show: the rows that fit the list (about 8). */
export function thumbsToWarm<T extends { id: number; hasThumb: boolean; createdAt: number }>(
  rows: readonly T[],
  url: (id: number, version: number) => string,
  max = 9,
): string[] {
  return rows
    .filter((r) => r.hasThumb)
    .slice(0, max)
    .map((r) => url(r.id, r.createdAt));
}

let thumbsWarmed = false;

/**
 * Fetch and decode the first rows' thumbnails once, from the first search response. They then sit
 * in the webview's image cache (the backend sends immutable cache headers), so the first show
 * paints the same list as the second. Does nothing after the first call.
 */
export function warmThumbs(urls: string[]): void {
  if (thumbsWarmed || typeof Image === "undefined") return;
  thumbsWarmed = true;
  for (const u of urls) {
    const img = new Image();
    img.src = u;
    img.decode?.().catch(() => {});
  }
}
