import { useCallback, useRef, useState } from "react";
import {
  errorMessage,
  extractTasks,
  getClips,
  pickNextTask,
  reviewStaleTasks,
  suggestNextStep,
  suggestTaskLinks,
  taskDigest,
  type Energy,
  type StaleVerdict,
  type TaskDigest,
} from "@/lib/ipc";
import { createLatest } from "@/lib/latest";
import { digestIsFresh, type AiPanel, type LinkRow } from "@/lib/taskAi";
import { defaultSelection } from "@/lib/tasks";

export interface TaskAi {
  /** The answer on show, else null. */
  panel: AiPanel | null;
  setPanel: React.Dispatch<React.SetStateAction<AiPanel | null>>;
  close: () => void;
  pickNext: (minutes?: number | null, energy?: Energy) => void;
  nextStep: (taskId: number) => void;
  digest: (period: "morning" | "week", force?: boolean) => void;
  links: () => void;
  openExtract: () => void;
  /** Read `text`, or the newest copied text when it is empty. */
  runExtract: (text: string) => void;
  /** What the assistant says about the quiet tasks it was asked about, by task id. */
  verdicts: ReadonlyMap<number, StaleVerdict>;
  reviewing: boolean;
  /** Ask what to do about these quiet tasks. Resolves to an error sentence, else null. */
  review: (ids: number[]) => Promise<string | null>;
}

/**
 * The assistant's state: one panel at a time, and the verdicts on quiet tasks. Each request takes a
 * ticket (`createLatest`), so an answer that arrives after another question was asked, or after the
 * panel was closed, is dropped instead of painted over the newer one.
 *
 * `onStored` runs when a call kept something on a task (a suggested step), so the list reloads.
 */
export function useTaskAi(onStored: () => void): TaskAi {
  const [panel, setPanel] = useState<AiPanel | null>(null);
  const [verdicts, setVerdicts] = useState<ReadonlyMap<number, StaleVerdict>>(new Map());
  const [reviewing, setReviewing] = useState(false);
  const latest = useRef(createLatest());
  const digests = useRef(new Map<"morning" | "week", { at: number; value: TaskDigest }>());
  const lastPick = useRef<{ minutes: number | null; energy: Energy }>({ minutes: null, energy: "normal" });

  const close = useCallback(() => {
    latest.current.next();
    setPanel(null);
  }, []);

  const pickNext = useCallback((minutes?: number | null, energy?: Energy) => {
    const m = minutes === undefined ? lastPick.current.minutes : minutes;
    const e = energy ?? lastPick.current.energy;
    lastPick.current = { minutes: m, energy: e };
    const ticket = latest.current.next();
    setPanel({ kind: "pick", minutes: m, energy: e, result: { status: "loading" } });
    pickNextTask(m, e).then(
      (value) => {
        if (!latest.current.isCurrent(ticket)) return;
        setPanel({ kind: "pick", minutes: m, energy: e, result: { status: "done", value } });
      },
      (err: unknown) => {
        if (!latest.current.isCurrent(ticket)) return;
        setPanel({ kind: "pick", minutes: m, energy: e, result: { status: "error", message: errorMessage(err) } });
      },
    );
  }, []);

  const nextStep = useCallback(
    (taskId: number) => {
      const ticket = latest.current.next();
      setPanel({ kind: "steps", taskId, result: { status: "loading" } });
      suggestNextStep(taskId).then(
        (value) => {
          if (!latest.current.isCurrent(ticket)) return;
          setPanel({ kind: "steps", taskId, result: { status: "done", value } });
          // The step is kept on the task: load the list again so it shows under the title.
          onStored();
        },
        (err: unknown) => {
          if (!latest.current.isCurrent(ticket)) return;
          setPanel({ kind: "steps", taskId, result: { status: "error", message: errorMessage(err) } });
        },
      );
    },
    [onStored],
  );

  const digest = useCallback((period: "morning" | "week", force = false) => {
    const cached = digests.current.get(period);
    const ticket = latest.current.next();
    if (cached && !force && digestIsFresh(cached.at, Date.now())) {
      setPanel({ kind: "digest", period, result: { status: "done", value: cached.value } });
      return;
    }
    setPanel({ kind: "digest", period, result: { status: "loading" } });
    taskDigest(period).then(
      (value) => {
        if (!latest.current.isCurrent(ticket)) return;
        digests.current.set(period, { at: Date.now(), value });
        setPanel({ kind: "digest", period, result: { status: "done", value } });
      },
      (err: unknown) => {
        if (!latest.current.isCurrent(ticket)) return;
        setPanel({ kind: "digest", period, result: { status: "error", message: errorMessage(err) } });
      },
    );
  }, []);

  const links = useCallback(() => {
    const ticket = latest.current.next();
    setPanel({ kind: "links", result: { status: "loading" } });
    suggestTaskLinks()
      .then(async (found) => {
        const clips = found.length > 0 ? await getClips(found.map((l) => l.clipId)) : [];
        const rows: LinkRow[] = found.map((l) => ({
          ...l,
          preview: clips.find((c) => c.id === l.clipId)?.preview ?? "",
        }));
        if (!latest.current.isCurrent(ticket)) return;
        setPanel({ kind: "links", result: { status: "done", value: rows } });
      })
      .catch((err: unknown) => {
        if (!latest.current.isCurrent(ticket)) return;
        setPanel({ kind: "links", result: { status: "error", message: errorMessage(err) } });
      });
  }, []);

  const openExtract = useCallback(() => {
    latest.current.next();
    setPanel({ kind: "extract", text: "", result: null, selected: [] });
  }, []);

  const runExtract = useCallback((text: string) => {
    const ticket = latest.current.next();
    setPanel({ kind: "extract", text, result: { status: "loading" }, selected: [] });
    extractTasks(text.trim() === "" ? null : text).then(
      (value) => {
        if (!latest.current.isCurrent(ticket)) return;
        setPanel({
          kind: "extract",
          text,
          result: { status: "done", value },
          selected: defaultSelection(value.tasks),
        });
      },
      (err: unknown) => {
        if (!latest.current.isCurrent(ticket)) return;
        setPanel({ kind: "extract", text, result: { status: "error", message: errorMessage(err) }, selected: [] });
      },
    );
  }, []);

  const review = useCallback(async (ids: number[]): Promise<string | null> => {
    setReviewing(true);
    try {
      const found = await reviewStaleTasks(ids);
      setVerdicts((old) => {
        const next = new Map(old);
        for (const v of found) next.set(v.id, v);
        return next;
      });
      return null;
    } catch (err) {
      return errorMessage(err);
    } finally {
      setReviewing(false);
    }
  }, []);

  return { panel, setPanel, close, pickNext, nextStep, digest, links, openExtract, runExtract, verdicts, reviewing, review };
}
