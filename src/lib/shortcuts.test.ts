import { describe, expect, it } from "vitest";
import { moveIndex, resolveShortcut, type KeyInfo } from "./shortcuts";

function k(key: string, mods: Partial<KeyInfo> = {}): KeyInfo {
  const code = /^[0-9]$/.test(key)
    ? `Digit${key}`
    : key.length === 1
      ? `Key${key.toUpperCase()}`
      : key === ","
        ? "Comma"
        : key;
  return {
    key,
    code,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    ...mods,
  };
}

const ctrl = { ctrlKey: true };

describe("resolveShortcut in the search view", () => {
  const r = (e: KeyInfo) => resolveShortcut(e, "search");

  it("Ctrl+Enter copies without paste, Enter is the plain enter action", () => {
    expect(r(k("Enter", ctrl))).toEqual({ type: "copy", paste: false });
    expect(r(k("Enter"))).toEqual({ type: "enter" });
    expect(r(k("Enter", { shiftKey: true }))).toBeNull();
  });

  it("Ctrl+Shift+S splits the selected text, Ctrl+S and Shift+S are left alone", () => {
    expect(r(k("S", { ctrlKey: true, shiftKey: true }))).toEqual({ type: "split" });
    expect(r(k("s", { ctrlKey: true, shiftKey: true }))).toEqual({ type: "split" });
    expect(r(k("s", ctrl))).toBeNull();
    expect(r(k("S", { shiftKey: true }))).toBeNull();
    expect(resolveShortcut(k("S", { ctrlKey: true, shiftKey: true }), "settings")).toBeNull();
  });

  it("Ctrl+1..9 copies the Nth row, Ctrl+0 does nothing", () => {
    for (let n = 1; n <= 9; n += 1) {
      expect(r(k(String(n), ctrl))).toEqual({ type: "copyNth", n });
    }
    expect(r(k("0", ctrl))).toBeNull();
  });

  it("Alt+1..4 pick the filters in order, Alt+5 is nothing", () => {
    const alt = { altKey: true };
    expect(r(k("1", alt))).toEqual({ type: "filter", filter: "all" });
    expect(r(k("2", alt))).toEqual({ type: "filter", filter: "text" });
    expect(r(k("3", alt))).toEqual({ type: "filter", filter: "images" });
    expect(r(k("4", alt))).toEqual({ type: "filter", filter: "files" });
    expect(r(k("5", alt))).toBeNull();
  });

  it("uses the physical key, so Alt+digit works when the layout makes another character", () => {
    expect(r({ ...k("¡", { altKey: true }), code: "Digit1" })).toEqual({
      type: "filter",
      filter: "all",
    });
    expect(r({ ...k("2", ctrl), code: "Numpad2" })).toEqual({ type: "copyNth", n: 2 });
  });

  it("Ctrl+P pins, plain P and Ctrl+Shift+P are typing or untouched", () => {
    expect(r(k("p", ctrl))).toEqual({ type: "togglePin" });
    expect(r(k("P", ctrl))).toEqual({ type: "togglePin" });
    expect(r(k("p"))).toBeNull();
    expect(r(k("p", { ctrlKey: true, shiftKey: true }))).toBeNull();
  });

  it("Ctrl+Delete and Shift+Delete delete, Ctrl+Shift+Delete clears, plain Delete edits", () => {
    expect(r(k("Delete", ctrl))).toEqual({ type: "delete" });
    expect(r(k("Delete", { shiftKey: true }))).toEqual({ type: "delete" });
    expect(r(k("Delete", { ctrlKey: true, shiftKey: true }))).toEqual({ type: "clear" });
    expect(r(k("Delete"))).toBeNull();
  });

  it("Tab switches to tasks, Shift+Tab does not", () => {
    expect(r(k("Tab"))).toEqual({ type: "toggleTasks" });
    expect(r(k("Tab", { shiftKey: true }))).toBeNull();
  });

  it("Tab also leaves the task view", () => {
    expect(resolveShortcut(k("Tab"), "tasks")).toEqual({ type: "toggleTasks" });
  });

  it("Alt+P toggles the preview", () => {
    expect(r(k("p", { altKey: true, code: "KeyP" }))).toEqual({ type: "togglePreview" });
    expect(r(k("p", { altKey: true, ctrlKey: true, code: "KeyP" }))).toBeNull();
  });

  it("arrows and pages move, Home and End stay with the input", () => {
    expect(r(k("ArrowDown"))).toEqual({ type: "move", delta: 1 });
    expect(r(k("ArrowUp"))).toEqual({ type: "move", delta: -1 });
    expect(r(k("PageDown"))).toEqual({ type: "move", delta: 8 });
    expect(r(k("PageUp"))).toEqual({ type: "move", delta: -8 });
    expect(r(k("Home"))).toBeNull();
    expect(r(k("End"))).toBeNull();
    expect(r(k("ArrowDown", { shiftKey: true }))).toBeNull();
  });

  it("Esc and Ctrl+, work", () => {
    expect(r(k("Escape"))).toEqual({ type: "escape" });
    expect(r(k(",", ctrl))).toEqual({ type: "toggleSettings" });
  });

  it("Ctrl+? opens the shortcuts help, a plain ? is typing", () => {
    expect(r(k("?", { ctrlKey: true, shiftKey: true, code: "Slash" }))).toEqual({ type: "toggleShortcuts" });
    expect(r(k("/", { ctrlKey: true, shiftKey: true, code: "Slash" }))).toEqual({ type: "toggleShortcuts" });
    expect(r(k("?", { shiftKey: true, code: "Slash" }))).toBeNull();
    expect(r(k("/", ctrl))).toBeNull();
  });

  it("ignores IME composition and the meta key", () => {
    expect(r(k("Enter", { isComposing: true }))).toBeNull();
    expect(r(k("Enter", { keyCode: 229 }))).toBeNull();
    expect(r(k("Enter", { metaKey: true }))).toBeNull();
  });

  it("leaves ordinary typing and word editing alone", () => {
    expect(r(k("a"))).toBeNull();
    expect(r(k("a", ctrl))).toBeNull();
    expect(r(k("Backspace", ctrl))).toBeNull();
    expect(r(k("ArrowLeft", ctrl))).toBeNull();
  });
});

describe("resolveShortcut in the settings view", () => {
  const r = (e: KeyInfo) => resolveShortcut(e, "settings");
  it("only Esc and Ctrl+, act", () => {
    expect(r(k("Escape"))).toEqual({ type: "escape" });
    expect(r(k(",", ctrl))).toEqual({ type: "toggleSettings" });
    expect(r(k("Enter", ctrl))).toBeNull();
    expect(r(k("1", { altKey: true }))).toBeNull();
    expect(r(k("Delete", ctrl))).toBeNull();
    expect(r(k("ArrowDown"))).toBeNull();
    expect(r(k("Tab"))).toBeNull();
  });
});

describe("moveIndex", () => {
  it("wraps with single steps", () => {
    expect(moveIndex(0, -1, 5)).toBe(4);
    expect(moveIndex(4, 1, 5)).toBe(0);
    expect(moveIndex(2, 1, 5)).toBe(3);
  });
  it("clamps page moves and handles an empty list", () => {
    expect(moveIndex(1, -8, 5)).toBe(0);
    expect(moveIndex(1, 8, 5)).toBe(4);
    expect(moveIndex(0, 1, 0)).toBe(-1);
  });
  it("a single row stays put", () => {
    expect(moveIndex(0, 1, 1)).toBe(0);
    expect(moveIndex(0, -1, 1)).toBe(0);
  });
});
