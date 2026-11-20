import { describe, expect, it } from "vitest";
import { resolveNoteKey } from "./noteKeys";
import type { KeyInfo } from "./shortcuts";

function k(key: string, mods: Partial<KeyInfo> = {}): KeyInfo {
  return {
    key,
    code: key.length === 1 ? `Key${key.toUpperCase()}` : key,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    ...mods,
  };
}
const ctrl = { ctrlKey: true };

describe("resolveNoteKey", () => {
  it("maps the chords", () => {
    expect(resolveNoteKey(k("Enter", ctrl))).toEqual({ type: "new" });
    expect(resolveNoteKey(k("p", ctrl))).toEqual({ type: "togglePin" });
    expect(resolveNoteKey(k("g", ctrl))).toEqual({ type: "editTags" });
    expect(resolveNoteKey(k("e", ctrl))).toEqual({ type: "editBody" });
    expect(resolveNoteKey(k("Tab"))).toEqual({ type: "editBody" });
    expect(resolveNoteKey(k("Delete", ctrl))).toEqual({ type: "delete" });
    expect(resolveNoteKey(k("Delete", { shiftKey: true }))).toEqual({ type: "delete" });
    expect(resolveNoteKey(k("Escape"))).toEqual({ type: "escape" });
  });
  it("moves with the arrows and pages", () => {
    expect(resolveNoteKey(k("ArrowDown"))).toEqual({ type: "move", delta: 1 });
    expect(resolveNoteKey(k("PageUp"))).toEqual({ type: "move", delta: -6 });
  });
  it("leaves typing and plain Delete alone", () => {
    expect(resolveNoteKey(k("a"))).toBeNull();
    expect(resolveNoteKey(k("Delete"))).toBeNull();
    expect(resolveNoteKey(k("Enter"))).toBeNull();
    expect(resolveNoteKey({ ...k("p", ctrl), isComposing: true })).toBeNull();
  });
});
