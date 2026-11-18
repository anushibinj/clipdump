// Browser stand-in for `@tauri-apps/api/event` (swapped in by the `mock` Vite
// mode, see vite.config.ts). Never part of a production build.

type Handler = (event: { event: string; id: number; payload: unknown }) => void;

const handlers = new Map<string, Set<Handler>>();
let nextId = 1;

export type UnlistenFn = () => void;

export function listen(event: string, handler: Handler): Promise<UnlistenFn> {
  let set = handlers.get(event);
  if (!set) handlers.set(event, (set = new Set()));
  set.add(handler);
  const mine = set;
  return Promise.resolve(() => {
    mine.delete(handler);
  });
}

/** Deliver an event to the page, like the backend's `emit`. */
export function emit(event: string, payload: unknown = null): void {
  const set = handlers.get(event);
  if (!set) return;
  const id = nextId++;
  for (const h of Array.from(set)) h({ event, id, payload });
}
