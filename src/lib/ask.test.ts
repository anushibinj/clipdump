import { describe, expect, it } from "vitest";
import type { AskSearched } from "./ipc";
import { cleanQuestion, quoteRange, searchedLine, sourceLabel, splitAnswer, splitAtQuote } from "./ask";

const hits = [{ n: 2 }, { n: 5 }];

describe("splitAnswer", () => {
  it("turns known [n] markers into citations and keeps the text around them", () => {
    expect(splitAnswer("Report due Friday [2], draft too [5].", hits)).toEqual([
      { kind: "text", text: "Report due Friday " },
      { kind: "cite", n: 2 },
      { kind: "text", text: ", draft too " },
      { kind: "cite", n: 5 },
      { kind: "text", text: "." },
    ]);
  });

  it("keeps markers that name no hit, and other brackets, as text", () => {
    expect(splitAnswer("a [9] b [note] c", hits)).toEqual([{ kind: "text", text: "a [9] b [note] c" }]);
  });

  it("handles no markers, only a marker, and an empty answer", () => {
    expect(splitAnswer("plain", hits)).toEqual([{ kind: "text", text: "plain" }]);
    expect(splitAnswer("[2]", hits)).toEqual([{ kind: "cite", n: 2 }]);
    expect(splitAnswer("", hits)).toEqual([]);
  });
});

describe("quoteRange", () => {
  it("finds the quote ignoring case and whitespace differences", () => {
    const text = "Priya wants the Q4 report\nby   Friday. Thanks";
    const r = quoteRange(text, "q4 REPORT by friday");
    expect(r && text.slice(r[0], r[1])).toBe("Q4 report\nby   Friday");
  });

  it("ignores punctuation differences and leaves the quote's own edge punctuation out", () => {
    const text = "Priya wants the Q4 report, by Friday!";
    const r = quoteRange(text, "“wants the Q4 report by Friday.”");
    expect(r && text.slice(r[0], r[1])).toBe("wants the Q4 report, by Friday");
  });

  it("treats regex characters in the quote literally", () => {
    const text = "total (approx.) is $5+3 [maybe]";
    const r = quoteRange(text, "(approx.) is $5+3 [maybe]");
    expect(r && text.slice(r[0], r[1])).toBe("approx.) is $5+3 [maybe");
    expect(quoteRange("abc", "a.c")).toBeNull();
  });

  it("is null for a missing or empty quote", () => {
    expect(quoteRange("abc", "xyz")).toBeNull();
    expect(quoteRange("abc", "   ")).toBeNull();
  });
});

describe("splitAtQuote", () => {
  it("cuts around the quote and leaves the text whole when there is none", () => {
    expect(splitAtQuote("one two three", "TWO")).toEqual(["one ", "two", " three"]);
    expect(splitAtQuote("one two", null)).toEqual(["one two", "", ""]);
    expect(splitAtQuote("one two", "nine")).toEqual(["one two", "", ""]);
  });
});

describe("labels", () => {
  it("names the sources", () => {
    expect(sourceLabel("clip")).toBe("Clipboard");
    expect(sourceLabel("note")).toBe("Note");
    expect(sourceLabel("task")).toBe("Task");
  });

  it("says what was searched", () => {
    const s: AskSearched = {
      from: "2026-11-05",
      to: "2026-11-08",
      terms: [],
      sources: ["note", "task", "clip"],
      scanned: 412,
    };
    expect(searchedLine(s)).toBe("Looked in notes, tasks and clipboard · 2026-11-05 to 2026-11-08 · 412 items");
    expect(searchedLine({ ...s, from: null, to: null, sources: ["task"], scanned: 1 })).toBe(
      "Looked in tasks · 1 item",
    );
    expect(searchedLine({ ...s, to: "2026-11-05", sources: ["note", "clip"] })).toBe(
      "Looked in notes and clipboard · 2026-11-05 · 412 items",
    );
  });

  it("cleans a question", () => {
    expect(cleanQuestion("  hi  ")).toBe("hi");
    expect(cleanQuestion("   ")).toBeNull();
  });
});
