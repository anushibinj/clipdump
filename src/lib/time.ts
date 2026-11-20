// Compact relative times for the result rows ("now", "5m", "3h", "2d").

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * Age of `then` as seen at `now` (both unix ms). Short on purpose: the row has
 * little room. A time slightly in the future (clock skew) reads as "now".
 */
export function relativeTime(then: number, now: number): string {
  const age = now - then;
  if (age < MINUTE) return "now";
  if (age < HOUR) return `${Math.floor(age / MINUTE)}m`;
  if (age < DAY) return `${Math.floor(age / HOUR)}h`;
  const days = Math.floor(age / DAY);
  if (days < 14) return `${days}d`;
  if (days < 60) return `${Math.floor(days / 7)}w`;
  if (days < 365) return `${Math.floor(days / 30)}mo`;
  return `${Math.floor(days / 365)}y`;
}

/** `now` rounded down to the minute, so memoized rows keep their props within a minute. */
export function minuteNow(ms: number = Date.now()): number {
  return Math.floor(ms / MINUTE) * MINUTE;
}

/** Full local date and time for a tooltip. */
export function fullTime(ms: number): string {
  return new Date(ms).toLocaleString();
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** Local date and time as "20-Nov-2025 04:11 PM", for tooltips. */
export function fullDateTime(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  const h = d.getHours();
  return `${pad(d.getDate())}-${MONTHS[d.getMonth()]}-${d.getFullYear()} ${pad(h % 12 === 0 ? 12 : h % 12)}:${pad(d.getMinutes())} ${h < 12 ? "AM" : "PM"}`;
}
