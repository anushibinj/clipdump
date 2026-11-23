import { describe, expect, it } from "vitest";
import { rowForDigit, stepSelection } from "./quick";

describe("stepSelection", () => {
  it("wraps in both directions", () => {
    expect(stepSelection(2, 1, 3)).toBe(0);
    expect(stepSelection(0, -1, 3)).toBe(2);
    expect(stepSelection(0, 1, 3)).toBe(1);
  });
  it("stays at 0 with no rows", () => {
    expect(stepSelection(0, 1, 0)).toBe(0);
  });
});

describe("rowForDigit", () => {
  it("maps 1..n to rows 0..n-1", () => {
    expect(rowForDigit("1", 3)).toBe(0);
    expect(rowForDigit("3", 3)).toBe(2);
  });
  it("rejects out of range and non digits", () => {
    expect(rowForDigit("4", 3)).toBeNull();
    expect(rowForDigit("0", 3)).toBeNull();
    expect(rowForDigit("a", 3)).toBeNull();
  });
});
