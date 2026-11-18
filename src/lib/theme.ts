// Light/dark theme. shadcn's `dark:` variant keys off a `.dark` class on <html>
// (see `@custom-variant dark` in index.css), so something must toggle it from
// `prefers-color-scheme`. The user's choice (System, Light, Dark) is a UI
// preference kept in localStorage on purpose (it must be known before the first
// paint, so it does not wait for the backend); it is not part of settings.json.

export type ThemePref = "system" | "light" | "dark";

const KEY = "clipdump.theme";
const QUERY = "(prefers-color-scheme: dark)";

export function readThemePref(): ThemePref {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    // Storage can be blocked; fall back to following the system.
  }
  return "system";
}

export function writeThemePref(pref: ThemePref): void {
  try {
    localStorage.setItem(KEY, pref);
  } catch {
    // Not persisted; the choice still applies for this session.
  }
}

export function systemPrefersDark(): boolean {
  return typeof matchMedia === "function" && matchMedia(QUERY).matches;
}

/** Whether `pref` resolves to the dark palette right now. */
export function isDark(pref: ThemePref, systemDark: boolean): boolean {
  return pref === "dark" || (pref === "system" && systemDark);
}

export function applyTheme(pref: ThemePref): void {
  const dark = isDark(pref, systemPrefersDark());
  const root = document.documentElement;
  root.classList.toggle("dark", dark);
  root.style.colorScheme = dark ? "dark" : "light";
}

/**
 * Apply the stored preference now and keep following the system while the
 * preference is "system". Call once before the first render (no flash).
 * Returns a function that stops listening.
 */
export function initTheme(getPref: () => ThemePref = readThemePref): () => void {
  applyTheme(getPref());
  if (typeof matchMedia !== "function") return () => {};
  const mq = matchMedia(QUERY);
  const onChange = () => applyTheme(getPref());
  mq.addEventListener("change", onChange);
  return () => mq.removeEventListener("change", onChange);
}
