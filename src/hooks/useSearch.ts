import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage, search, thumbUrl, type ResultDto, type TypeFilter } from "@/lib/ipc";
import { createLatest } from "@/lib/latest";
import { moveIndex } from "@/lib/shortcuts";
import { minuteNow } from "@/lib/time";
import { thumbsToWarm, warmThumbs } from "@/lib/warm";

export interface SearchApi {
  query: string;
  filter: TypeFilter;
  results: ResultDto[];
  /** `minuteNow()` at the time of the last response, for the relative times. */
  now: number;
  selectedId: number | null;
  /** The first response has arrived (so "no results" is not shown before then). */
  ready: boolean;
  error: string | null;
  setQuery: (value: string) => void;
  setFilter: (value: TypeFilter) => void;
  select: (id: number) => void;
  /** Arrow keys: wraps at the ends for single steps. */
  move: (delta: number) => void;
  /** Search again with the same query (history changed, bar shown). */
  refresh: (opts?: { resetSelection?: boolean }) => void;
}

/**
 * Search state. Rules from the plan:
 *
 * - Search on every input change, **no debounce**: the call goes out inside
 *   the input's change handler, before React has re-rendered.
 * - Drop stale responses with a request counter (`createLatest`).
 * - Re-run on `history:changed` (the caller wires the event to `refresh`).
 * - A new query or filter resets the selection to the first row; a refresh
 *   keeps the selected item if it is still listed, else the row now at the
 *   same position (so after a delete the neighbor is selected).
 *
 * Everything is read through refs inside the callbacks, so they keep one
 * identity for the life of the component and memoized children never see them
 * change.
 */
export function useSearch(): SearchApi {
  const [query, setQueryState] = useState("");
  const [filter, setFilterState] = useState<TypeFilter>("all");
  const [results, setResults] = useState<ResultDto[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [now, setNow] = useState(() => minuteNow());
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const latest = useRef(createLatest());
  const queryRef = useRef(query);
  const filterRef = useRef(filter);
  const resultsRef = useRef(results);
  const selectedRef = useRef(selectedId);
  // Set by a new query or filter; consumed by the response that finally gets applied.
  const resetPending = useRef(true);

  const run = useCallback((q: string, f: TypeFilter) => {
    const ticket = latest.current.next();
    search(q, f).then(
      (next) => {
        if (!latest.current.isCurrent(ticket)) return; // a newer request is on its way
        const prev = resultsRef.current;
        const prevSel = selectedRef.current;
        let sel: number | null;
        if (resetPending.current || prevSel === null) {
          sel = next[0]?.id ?? null;
        } else if (next.some((r) => r.id === prevSel)) {
          sel = prevSel;
        } else {
          const at = Math.max(0, prev.findIndex((r) => r.id === prevSel));
          sel = next[Math.min(at, next.length - 1)]?.id ?? null;
        }
        resetPending.current = false;
        // Once, from the first answer (which arrives while the window is still hidden).
        warmThumbs(thumbsToWarm(next, (id, v) => thumbUrl(id, v)));
        resultsRef.current = next;
        selectedRef.current = sel;
        setResults(next);
        setSelectedId(sel);
        setNow(minuteNow());
        setReady(true);
        setError(null);
      },
      (e: unknown) => {
        if (latest.current.isCurrent(ticket)) {
          setReady(true);
          setError(errorMessage(e));
        }
      },
    );
  }, []);

  const setQuery = useCallback(
    (value: string) => {
      queryRef.current = value;
      resetPending.current = true;
      setQueryState(value);
      run(value, filterRef.current);
    },
    [run],
  );

  const setFilter = useCallback(
    (value: TypeFilter) => {
      filterRef.current = value;
      resetPending.current = true;
      setFilterState(value);
      run(queryRef.current, value);
    },
    [run],
  );

  const refresh = useCallback(
    (opts?: { resetSelection?: boolean }) => {
      if (opts?.resetSelection) resetPending.current = true;
      run(queryRef.current, filterRef.current);
    },
    [run],
  );

  const select = useCallback((id: number) => {
    selectedRef.current = id;
    setSelectedId(id);
  }, []);

  const move = useCallback((delta: number) => {
    const list = resultsRef.current;
    const at = list.findIndex((r) => r.id === selectedRef.current);
    const to = moveIndex(at < 0 ? 0 : at, at < 0 && delta > 0 ? 0 : delta, list.length);
    const id = list[to]?.id;
    if (id !== undefined) {
      selectedRef.current = id;
      setSelectedId(id);
    }
  }, []);

  // First list.
  useEffect(() => {
    run("", "all");
  }, [run]);

  return { query, filter, results, now, selectedId, ready, error, setQuery, setFilter, select, move, refresh };
}
