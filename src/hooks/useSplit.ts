import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage, getSplit, onSplitUpdated, requestSplit, type SplitStatus } from "@/lib/ipc";

/** `pending`: the request for this item has not been answered yet. */
export type SplitView = SplitStatus | { status: "pending" };

export interface SplitControl {
  state: SplitView;
  /** Run the split again, replacing the result (the re-split button). */
  rerun: () => void;
}

/** How long a selection must rest before it is sent: arrowing past items asks for nothing. */
export const SPLIT_DEBOUNCE_MS = 250;

/**
 * The split of the selected item, started by itself: once `enabled` (the AI split tab is showing a
 * text item) and the selection has rested a moment, the backend is asked. It queues the job (two
 * LLM calls at a time), keeps the result, and tells us through `split:updated` when something
 * moves, so no polling. A response is applied only if it is for the item still wanted.
 */
export function useSplit(itemId: number | null, enabled: boolean): SplitControl {
  const [shown, setShown] = useState<{ id: number; status: SplitStatus } | null>(null);
  const wanted = useRef<number | null>(null);
  wanted.current = enabled ? itemId : null;

  // Finished splits kept in RAM, so hovering back to an item shows its pieces at once instead of
  // flashing the skeleton while the backend is asked again.
  const cache = useRef(new Map<number, SplitStatus>());

  const apply = useCallback((id: number, status: SplitStatus) => {
    if (status.status === "done") cache.current.set(id, status);
    else if (status.status === "error") cache.current.delete(id);
    if (wanted.current === id) setShown({ id, status });
  }, []);

  const request = useCallback(
    (id: number, force: boolean) => {
      requestSplit(id, force).then(
        (s) => apply(id, s),
        (e: unknown) => apply(id, { status: "error", message: errorMessage(e) }),
      );
    },
    [apply],
  );

  useEffect(() => {
    if (!enabled || itemId === null) return;
    const t = window.setTimeout(() => request(itemId, false), SPLIT_DEBOUNCE_MS);
    return () => window.clearTimeout(t);
  }, [enabled, itemId, request]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let gone = false;
    try {
      onSplitUpdated(() => {
        const id = wanted.current;
        if (id !== null) getSplit(id).then((s) => apply(id, s), () => {});
      }).then(
        (fn) => (gone ? fn() : (unlisten = fn)),
        () => {},
      );
    } catch {
      // No event bus (plain browser without the mock): the result shows on the next look.
    }
    return () => {
      gone = true;
      unlisten?.();
    };
  }, [apply]);

  const rerun = useCallback(() => {
    if (itemId === null) return;
    setShown({ id: itemId, status: { status: "queued" } });
    request(itemId, true);
  }, [itemId, request]);

  const cached = itemId !== null ? cache.current.get(itemId) : undefined;
  const state: SplitView =
    shown !== null && shown.id === itemId ? shown.status : (cached ?? { status: "pending" });
  return { state, rerun };
}
