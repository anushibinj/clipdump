// The hotkey recorder's pure part: a key press in, a chord out. The chord text
// is what the backend parses (src-tauri/src/hotkey.rs): `Ctrl+Alt+Shift+Super+Key`
// with the modifiers in that order. Keys are named from the physical key
// (`KeyboardEvent.code`), so a chord does not change with the keyboard layout.
// The backend validates again; this only gives instant feedback while recording.

export interface ChordKey {
  /** `KeyboardEvent.code`. */
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

export type Capture =
  /** Esc alone: stop recording, change nothing. */
  | { type: "cancel" }
  /** Only modifiers held so far (shown live while the user presses the rest). */
  | { type: "pending"; mods: string[] }
  /** A key with modifiers the OS will not take as a global shortcut. */
  | { type: "invalid"; mods: string[]; reason: string }
  | { type: "chord"; chord: string };

const MODIFIER_CODES = /^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/;

const NAMED: Record<string, string> = {
  Space: "Space",
  Enter: "Enter",
  NumpadEnter: "Enter",
  Tab: "Tab",
  Backspace: "Backspace",
  Delete: "Delete",
  Insert: "Insert",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Comma: "Comma",
  Period: "Period",
  Slash: "Slash",
  Backslash: "Backslash",
  Semicolon: "Semicolon",
  Quote: "Quote",
  Backquote: "Backquote",
  Minus: "Minus",
  Equal: "Equal",
  BracketLeft: "BracketLeft",
  BracketRight: "BracketRight",
};

/** Canonical key name for a `KeyboardEvent.code`, or null when the key cannot be part of a chord. */
export function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  const fn = /^F([1-9]|1[0-9]|2[0-4])$/.exec(code);
  if (fn) return `F${fn[1]}`;
  return NAMED[code] ?? null;
}

/** Held modifiers in canonical order. */
export function modifiersOf(e: Pick<ChordKey, "ctrlKey" | "altKey" | "shiftKey" | "metaKey">): string[] {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  return mods;
}

export const NEEDS_MODIFIER = "Use Ctrl, Alt or Win together with another key.";
export const UNSUPPORTED_KEY = "That key cannot be used in a shortcut.";

/** What a key press means while recording a hotkey. */
export function captureChord(e: ChordKey): Capture {
  const mods = modifiersOf(e);
  if (e.code === "Escape" && mods.length === 0) return { type: "cancel" };
  if (MODIFIER_CODES.test(e.code)) return { type: "pending", mods };
  const key = keyName(e.code);
  if (key === null) return { type: "invalid", mods, reason: UNSUPPORTED_KEY };
  // Shift alone would take a capital letter away from every app.
  if (!(e.ctrlKey || e.altKey || e.metaKey)) return { type: "invalid", mods, reason: NEEDS_MODIFIER };
  return { type: "chord", chord: [...mods, key].join("+") };
}

/** The pieces of a chord as shown on key caps ("Super" is the Windows key). */
export function chordParts(chord: string): string[] {
  return chord
    .split("+")
    .filter((p) => p !== "")
    .map((p) => (p === "Super" ? "Win" : p));
}
