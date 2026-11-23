// Pure helpers of the tiny clipboard (Ctrl twice).

/** How many rows are visible at once in the popup. */
export const QUICK_VISIBLE = 3;

/** How many rows the popup loads; the rest of that history scrolls. */
export const QUICK_LIMIT = 50;

/** Row to select after an arrow key: wraps at both ends, 0 when there are no rows. */
export function stepSelection(current: number, delta: 1 | -1, count: number): number {
  if (count <= 0) return 0;
  return (current + delta + count) % count;
}

/** The row an Alt+1..3 style number key asks for, or null when out of range. */
export function rowForDigit(key: string, count: number): number | null {
  if (!/^[1-9]$/.test(key)) return null;
  const i = Number(key) - 1;
  return i < count ? i : null;
}
