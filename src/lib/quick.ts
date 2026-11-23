// Pure helpers of the tiny clipboard (Ctrl twice).

/** How many matches the popup shows. */
export const QUICK_LIMIT = 3;

/** Row to select after an arrow key: wraps at both ends, 0 when there are no rows. */
export function stepSelection(current: number, delta: 1 | -1, count: number): number {
  if (count <= 0) return 0;
  return (current + delta + count) % count;
}

/** The row a Ctrl+1..3 / Alt+1..3 style number key asks for, or null when out of range. */
export function rowForDigit(key: string, count: number): number | null {
  if (!/^[1-9]$/.test(key)) return null;
  const i = Number(key) - 1;
  return i < count ? i : null;
}
