import { afterEach, describe, expect, it, vi } from "vitest";
import { VIEWS, readView, writeView } from "./view";

function stubStorage(initial: Record<string, string> = {}) {
  const data = { ...initial };
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => data[k] ?? null,
    setItem: (k: string, v: string) => void (data[k] = v),
  });
  return data;
}

afterEach(() => vi.unstubAllGlobals());

describe("remembered view", () => {
  it("starts on the search the first time", () => {
    stubStorage();
    expect(readView()).toBe("search");
  });
  it("gives back every view that was left", () => {
    stubStorage();
    for (const v of VIEWS) {
      writeView(v);
      expect(readView()).toBe(v);
    }
  });
  it("ignores a value it does not know", () => {
    stubStorage({ "clipdump.view": "nonsense" });
    expect(readView()).toBe("search");
  });
  it("falls back when storage throws", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    });
    expect(readView()).toBe("search");
    expect(() => writeView("tasks")).not.toThrow();
  });
});
