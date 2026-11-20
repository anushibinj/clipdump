import { describe, expect, it } from "vitest";
import type { Note } from "./ipc";
import {
  cleanTag,
  filterNotes,
  hasTagToken,
  isBlank,
  keepSelection,
  noteLabel,
  parseQuery,
  sortNotes,
  tagCounts,
  toggleTagToken,
} from "./notes";

let n = 0;
function note(p: Partial<Note> = {}): Note {
  n += 1;
  return { id: n, title: "", body: "", pinned: false, tags: [], createdAt: n, updatedAt: n, ...p };
}

describe("notes", () => {
  it("labels an untitled note by its first line", () => {
    expect(noteLabel(note({ title: "T", body: "b" }))).toBe("T");
    expect(noteLabel(note({ body: "\n  first line \nsecond" }))).toBe("first line");
    expect(noteLabel(note())).toBe("");
  });

  it("knows a blank note", () => {
    expect(isBlank(note())).toBe(true);
    expect(isBlank(note({ body: "  \n" }))).toBe(true);
    expect(isBlank(note({ tags: ["a"] }))).toBe(false);
    expect(isBlank(note({ title: "x" }))).toBe(false);
  });

  it("cleans tags like the backend", () => {
    expect(cleanTag(" #Big  Idea ")).toBe("big-idea");
    expect(cleanTag("##")).toBe("");
  });

  it("sorts pinned first, then newest change", () => {
    const a = note({ updatedAt: 10 });
    const b = note({ updatedAt: 30 });
    const c = note({ updatedAt: 5, pinned: true });
    const d = note({ updatedAt: 20, pinned: true });
    expect(sortNotes([a, b, c, d]).map((x) => x.id)).toEqual([d.id, c.id, b.id, a.id]);
  });

  it("parses tag words apart from text words", () => {
    expect(parseQuery("  #Work  report #q3 ")).toEqual({ tags: ["work", "q3"], words: ["report"] });
    expect(parseQuery("#")).toEqual({ tags: [], words: ["#"] });
  });

  it("filters by words (title, body, tag) and by tags", () => {
    const a = note({ title: "Wifi", body: "password is hunter2", tags: ["home"] });
    const b = note({ body: "Quarterly report", tags: ["work", "q3"] });
    const c = note({ body: "nothing", tags: [] });
    const all = [a, b, c];
    expect(filterNotes(all, "").length).toBe(3);
    expect(filterNotes(all, "REPORT").map((x) => x.id)).toEqual([b.id]);
    expect(filterNotes(all, "home").map((x) => x.id)).toEqual([a.id]);
    expect(filterNotes(all, "#work").map((x) => x.id)).toEqual([b.id]);
    expect(filterNotes(all, "#wo").map((x) => x.id)).toEqual([b.id]);
    expect(filterNotes(all, "#work #q3 report").map((x) => x.id)).toEqual([b.id]);
    expect(filterNotes(all, "#work #home")).toEqual([]);
  });

  it("counts tags", () => {
    const all = [note({ tags: ["b", "a"] }), note({ tags: ["a"] }), note({ tags: ["c"] })];
    expect(tagCounts(all)).toEqual([
      { tag: "a", count: 2 },
      { tag: "b", count: 1 },
      { tag: "c", count: 1 },
    ]);
  });

  it("toggles a tag word in the query", () => {
    expect(toggleTagToken("", "work")).toBe("#work");
    expect(toggleTagToken("report", "work")).toBe("report #work");
    expect(toggleTagToken("report #work", "work")).toBe("report");
    expect(hasTagToken("a #work", "work")).toBe(true);
    expect(hasTagToken("a #wor", "work")).toBe(false);
  });

  it("keeps the selection when it is still shown, else takes the first", () => {
    const a = note();
    const b = note();
    expect(keepSelection([a, b], b.id)).toBe(b.id);
    expect(keepSelection([a, b], 999)).toBe(a.id);
    expect(keepSelection([], 5)).toBeNull();
    expect(keepSelection([a], null)).toBe(a.id);
  });
});
