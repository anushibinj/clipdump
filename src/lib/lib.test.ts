import { describe, expect, it } from "vitest";
import { bytesToMb, formatBytes, mbToBytes } from "./bytes";
import { highlightSegments, windowAroundFirstHit } from "./highlight";
import { createLatest } from "./latest";
import { minuteNow, relativeTime } from "./time";

describe("formatBytes", () => {
  it("formats the common sizes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(-5)).toBe("0 B");
    expect(formatBytes(Number.NaN)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1.2 * 1024 * 1024)).toBe("1.2 MB");
    expect(formatBytes(12 * 1024 * 1024)).toBe("12 MB");
    expect(formatBytes(2 * 1024 ** 3)).toBe("2 GB");
  });
  it("never shows 1024 of a unit", () => {
    expect(formatBytes(1024 * 1024 - 1)).toBe("1 MB");
    expect(formatBytes(1023)).toBe("1023 B");
  });
  it("converts megabytes for settings fields", () => {
    expect(mbToBytes(5)).toBe(5 * 1024 * 1024);
    expect(bytesToMb(5 * 1024 * 1024)).toBe(5);
    expect(bytesToMb(1536 * 1024)).toBe(1.5);
  });
});

describe("relativeTime", () => {
  const now = 1_000_000_000_000;
  const MIN = 60_000;
  it("reads compactly", () => {
    expect(relativeTime(now - 5_000, now)).toBe("now");
    expect(relativeTime(now + 5_000, now)).toBe("now");
    expect(relativeTime(now - 5 * MIN, now)).toBe("5m");
    expect(relativeTime(now - 59 * MIN, now)).toBe("59m");
    expect(relativeTime(now - 60 * MIN, now)).toBe("1h");
    expect(relativeTime(now - 23 * 60 * MIN, now)).toBe("23h");
    expect(relativeTime(now - 24 * 60 * MIN, now)).toBe("1d");
    expect(relativeTime(now - 13 * 24 * 60 * MIN, now)).toBe("13d");
    expect(relativeTime(now - 30 * 24 * 60 * MIN, now)).toBe("4w");
    expect(relativeTime(now - 90 * 24 * 60 * MIN, now)).toBe("3mo");
    expect(relativeTime(now - 800 * 24 * 60 * MIN, now)).toBe("2y");
  });
  it("rounds now down to the minute", () => {
    expect(minuteNow(125_999)).toBe(120_000);
  });
});

describe("highlightSegments", () => {
  const join = (s: { text: string; hit: boolean }[]) =>
    s.map((x) => (x.hit ? `[${x.text}]` : x.text)).join("");

  it("returns one plain run without indices", () => {
    expect(highlightSegments("hello", [])).toEqual([{ text: "hello", hit: false }]);
    expect(highlightSegments("", [0])).toEqual([{ text: "", hit: false }]);
  });
  it("marks runs and keeps the text intact", () => {
    expect(join(highlightSegments("hello world", [0, 1, 6, 7, 8]))).toBe("[he]llo [wor]ld");
    expect(join(highlightSegments("abc", [0, 1, 2]))).toBe("[abc]");
    expect(join(highlightSegments("abc", [2]))).toBe("ab[c]");
  });
  it("counts characters, not UTF-16 units: astral characters are one position", () => {
    // "a", rocket (two UTF-16 units), "b", "c": positions 0..3
    const text = "a\u{1F680}bc";
    expect(text.length).toBe(5);
    expect(join(highlightSegments(text, [2, 3]))).toBe("a\u{1F680}[bc]");
    expect(join(highlightSegments(text, [1]))).toBe("a[\u{1F680}]bc");
  });
  it("handles accents, CJK and combining text by code point", () => {
    expect(join(highlightSegments("café au lait", [3]))).toBe("caf[é] au lait");
    expect(join(highlightSegments("日本語のテキスト", [3, 4]))).toBe(
      "日本語[のテ]キスト",
    );
    // e + combining acute is two code points
    expect(join(highlightSegments("éx", [1]))).toBe("e[́]x");
  });
  it("ignores positions past the end and tolerates duplicates", () => {
    expect(join(highlightSegments("ab", [1, 1, 9]))).toBe("a[b]");
  });
});

describe("windowAroundFirstHit", () => {
  it("leaves early matches alone", () => {
    const w = windowAroundFirstHit("hello world", [0, 1]);
    expect(w).toEqual({ text: "hello world", indices: [0, 1] });
  });
  it("shifts a late match into view with a leading ellipsis", () => {
    const text = "x".repeat(60) + "needle tail";
    const w = windowAroundFirstHit(text, [60, 61, 62]);
    expect(w.text.startsWith("…")).toBe(true);
    const segs = highlightSegments(w.text, w.indices);
    expect(segs.filter((s) => s.hit).map((s) => s.text)).toEqual(["nee"]);
    expect(w.text.indexOf("needle")).toBeLessThanOrEqual(14);
  });
  it("counts astral characters before the window as one position each", () => {
    const text = "\u{1F680}".repeat(50) + "needle";
    const w = windowAroundFirstHit(text, [50, 51]);
    const segs = highlightSegments(w.text, w.indices);
    expect(segs.filter((s) => s.hit).map((s) => s.text)).toEqual(["ne"]);
  });
});

describe("createLatest", () => {
  it("only the newest ticket is current", () => {
    const l = createLatest();
    const a = l.next();
    expect(l.isCurrent(a)).toBe(true);
    const b = l.next();
    expect(l.isCurrent(a)).toBe(false);
    expect(l.isCurrent(b)).toBe(true);
  });
  it("drops out of order responses", async () => {
    const l = createLatest();
    const applied: string[] = [];
    const request = (label: string, ms: number) => {
      const t = l.next();
      return new Promise<void>((resolve) =>
        setTimeout(() => {
          if (l.isCurrent(t)) applied.push(label);
          resolve();
        }, ms),
      );
    };
    await Promise.all([request("slow-old", 30), request("fast-new", 5)]);
    expect(applied).toEqual(["fast-new"]);
  });
});
