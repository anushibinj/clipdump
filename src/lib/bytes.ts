// Human readable byte sizes. Binary units (1 KB = 1024 B) to match what the
// backend's disk and cache budgets count.

const UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** `1536` gives "1.5 KB", `0` gives "0 B". One decimal below 10, none from 10 up. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${Math.round(value)} B`;
  // Rounding can carry over ("1023.97 KB" must not read "1024 KB").
  const rounded = value < 10 ? Math.round(value * 10) / 10 : Math.round(value);
  if (rounded >= 1024 && unit < UNITS.length - 1) {
    return `1 ${UNITS[unit + 1]}`;
  }
  return `${rounded} ${UNITS[unit]}`;
}

export const MIB = 1024 * 1024;

/** Bytes to megabytes for a settings field, at most two decimals. */
export function bytesToMb(bytes: number): number {
  return Math.round((bytes / MIB) * 100) / 100;
}

export function mbToBytes(mb: number): number {
  return Math.round(mb * MIB);
}
