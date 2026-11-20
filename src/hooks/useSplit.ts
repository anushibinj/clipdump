import { useCallback, useState } from "react";
import { errorMessage, splitItem, type SplitPart } from "@/lib/ipc";

export type SplitState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "done"; parts: SplitPart[] }
  | { status: "error"; message: string };

const IDLE: SplitState = { status: "idle" };
/** Results kept, newest last. The content behind an id never changes, so a result stays right. */
const KEEP = 30;

export interface SplitControl {
  /** The state of `itemId`: what was asked for it, or idle. */
  state: SplitState;
  /** Ask the LLM to split `itemId` (no-op while it is already loading). */
  run: () => void;
  /** Back to the plain text view of `itemId`, forgetting its result. */
  reset: () => void;
}

/**
 * Split state per item. Switching to another item and back shows the earlier result without a
 * new (slow, possibly paid) request, and a response is stored under the id it was asked for, so
 * moving the selection while it loads never paints the pieces on the wrong item.
 */
export function useSplit(itemId: number | null): SplitControl {
  const [byId, setById] = useState<ReadonlyMap<number, SplitState>>(new Map());

  const put = useCallback((id: number, state: SplitState | null) => {
    setById((m) => {
      const next = new Map(m);
      next.delete(id); // re-insert at the end: the Map order is the age
      if (state) next.set(id, state);
      while (next.size > KEEP) next.delete(next.keys().next().value as number);
      return next;
    });
  }, []);

  const state = itemId === null ? IDLE : (byId.get(itemId) ?? IDLE);

  const run = useCallback(() => {
    if (itemId === null || byId.get(itemId)?.status === "loading") return;
    const id = itemId;
    put(id, { status: "loading" });
    splitItem(id).then(
      (parts) => put(id, { status: "done", parts }),
      (e: unknown) => put(id, { status: "error", message: errorMessage(e) }),
    );
  }, [itemId, byId, put]);

  const reset = useCallback(() => {
    if (itemId !== null) put(itemId, null);
  }, [itemId, put]);

  return { state, run, reset };
}
