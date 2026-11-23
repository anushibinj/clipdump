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

  it("Alt+1..9 copies the Nth row, Alt+0 does nothing", () => {
    const alt = { altKey: true };
    for (let n = 1; n <= 9; n += 1) {
      expect(r(k(String(n), alt))).toEqual({ type: "copyNth", n });
    }
    expect(r(k("0", alt))).toBeNull();
  });

  it("Alt+Shift+1..4 pick the filters in order, Alt+Shift+5 is nothing", () => {
    const alt = { altKey: true, shiftKey: true };
    expect(r(k("1", alt))).toEqual({ type: "filter", filter: "all" });
    expect(r(k("2", alt))).toEqual({ type: "filter", filter: "text" });
    expect(r(k("3", alt))).toEqual({ type: "filter", filter: "images" });
    expect(r(k("4", alt))).toEqual({ type: "filter", filter: "files" });
    expect(r(k("5", alt))).toBeNull();
  });

  it("uses the physical key, so Alt+digit works when the layout makes another character", () => {
    expect(r({ ...k("¡", { altKey: true, shiftKey: true }), code: "Digit1" })).toEqual({
      type: "filter",
      filter: "all",
    });
    expect(r({ ...k("2", { altKey: true }), code: "Numpad2" })).toEqual({ type: "copyNth", n: 2 });
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
    expect(r(k("/", ctrl))).toEqual({ type: "toggleShortcuts" }); // Ctrl+/ works too: no Shift needed
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
    expect(r(k("1", ctrl))).toEqual({ type: "switchMode", view: "search" });
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

describe("Ctrl+Shift+N and the notes view", () => {
  it("Ctrl+Shift+N toggles the notes from every view", () => {
    for (const v of ["search", "tasks", "notes", "settings"] as const) {
      expect(resolveShortcut(k("n", { ...ctrl, shiftKey: true }), v)).toEqual({ type: "toggleNotes" });
    }
  });
  it("the notes view keeps its own Esc and keys", () => {
    expect(resolveShortcut(k("Escape"), "notes")).toBeNull();
    expect(resolveShortcut(k("Enter", ctrl), "notes")).toBeNull();
    expect(resolveShortcut(k("p", ctrl), "notes")).toBeNull();
    expect(resolveShortcut(k("Tab"), "notes")).toBeNull();
    // Ctrl+N alone is not the notes key (it is "what next" in the task view).
    expect(resolveShortcut(k("n", ctrl), "notes")).toBeNull();
  });
});

describe("Ctrl+/ and Ctrl+?", () => {
  it("Ctrl+/ and Ctrl+? both open the shortcuts help from every view", () => {
    for (const v of ["search", "tasks", "notes", "ask", "settings"] as const) {
      expect(resolveShortcut(k("/", ctrl), v)).toEqual({ type: "toggleShortcuts" });
      expect(resolveShortcut({ ...k("-", ctrl), code: "Slash" }, v)).toEqual({ type: "toggleShortcuts" });
      expect(resolveShortcut(k("?", { ...ctrl, shiftKey: true }), v)).toEqual({ type: "toggleShortcuts" });
    }
  });
  it("a bare slash stays typing", () => {
    expect(resolveShortcut(k("/"), "search")).toBeNull();
    expect(resolveShortcut(k("/"), "ask")).toBeNull();
  });
  it("the ask view keeps its own Esc, Enter and Tab", () => {
    expect(resolveShortcut(k("Escape"), "ask")).toBeNull();
    expect(resolveShortcut(k("Enter"), "ask")).toBeNull();
    expect(resolveShortcut(k("Enter", ctrl), "ask")).toBeNull();
    expect(resolveShortcut(k("Tab"), "ask")).toBeNull();
    expect(resolveShortcut(k("t", ctrl), "ask")).toEqual({ type: "toggleTasks" });
  });
});

describe("Ctrl+digit switches the app mode", () => {
  const alt = { ctrlKey: true };
  it("Ctrl+1 clipboard, Ctrl+2 notes, Ctrl+3 tasks, from every view", () => {
    for (const view of ["search", "notes", "tasks", "ask", "settings"] as const) {
      const r = (e: KeyInfo) => resolveShortcut(e, view);
      expect(r(k("1", alt))).toEqual({ type: "switchMode", view: "search" });
      expect(r(k("2", alt))).toEqual({ type: "switchMode", view: "notes" });
      expect(r(k("3", alt))).toEqual({ type: "switchMode", view: "tasks" });
    }
  });
  it("numbers past the last mode do nothing, and Ctrl+Alt+digit is not a mode switch", () => {
    expect(resolveShortcut(k("4", alt), "search")).toBeNull();
    expect(resolveShortcut(k("1", { altKey: true, ctrlKey: true }), "search")).toBeNull();
  });
  it("uses the physical key", () => {
    expect(resolveShortcut({ ...k("¡", alt), code: "Digit1" }, "tasks")).toEqual({
      type: "switchMode",
      view: "search",
    });
  });
});

describe("Alt+/ and Alt+? open Ask", () => {
  it("from every view, by character or physical key", () => {
    for (const view of ["search", "notes", "tasks", "ask", "settings"] as const) {
      const r = (e: KeyInfo) => resolveShortcut(e, view);
      expect(r({ ...k("/", { altKey: true }), code: "Slash" })).toEqual({ type: "openAsk" });
      expect(r({ ...k("?", { altKey: true, shiftKey: true }), code: "Slash" })).toEqual({ type: "openAsk" });
    }
  });
  it("Ctrl+/ still opens the shortcuts help", () => {
    expect(resolveShortcut({ ...k("/", { ctrlKey: true }), code: "Slash" }, "search")).toEqual({
      type: "toggleShortcuts",
    });
  });
});
