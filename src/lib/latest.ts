// "Latest request wins": every async request takes a ticket, and a response is
// applied only if its ticket is still the newest. No debounce is involved, so a
// fast typist fires one request per keystroke and only the last answer is
// painted.

export interface Latest {
  /** Take a new ticket; every older ticket becomes stale. */
  next(): number;
  /** True while `ticket` is the newest one handed out. */
  isCurrent(ticket: number): boolean;
}

export function createLatest(): Latest {
  let current = 0;
  return {
    next: () => (current += 1),
    isCurrent: (ticket) => ticket === current,
  };
}
