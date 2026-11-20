import { describe, expect, it } from "vitest";
import { DIGEST_FRESH_MS, ENERGY_CHOICES, MINUTE_CHOICES, digestIsFresh } from "./taskAi";

describe("digestIsFresh", () => {
  it("reuses an answer for half an hour and not longer", () => {
    const at = 1_000_000;
    expect(digestIsFresh(at, at)).toBe(true);
    expect(digestIsFresh(at, at + DIGEST_FRESH_MS - 1)).toBe(true);
    expect(digestIsFresh(at, at + DIGEST_FRESH_MS)).toBe(false);
  });
  it("does not trust an answer from the future (the clock was set back)", () => {
    expect(digestIsFresh(2_000_000, 1_000_000)).toBe(false);
  });
});

describe("the pick panel's choices", () => {
  it("offer a time limit, with no limit last, and three energy levels", () => {
    expect(MINUTE_CHOICES.map((c) => c.minutes)).toEqual([10, 30, 60, null]);
    expect(ENERGY_CHOICES.map((c) => c.energy)).toEqual(["low", "normal", "high"]);
  });
});
