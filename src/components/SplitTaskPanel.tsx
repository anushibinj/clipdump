import { useRef, useState } from "react";
import { Check, Loader2, Sparkles } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  addTask,
  errorMessage,
  splitTaskNotes,
  updateTask,
  type SplitTaskProposal,
} from "@/lib/ipc";
import { createLatest } from "@/lib/latest";
import { MAX_NOTE_CHARS, MAX_TITLE_CHARS } from "@/lib/tasks";

/** A proposed task the person can tick and edit before it is created. */
interface Row extends SplitTaskProposal {
  on: boolean;
}

type State =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "done"; rows: Row[] };

interface SplitTaskPanelProps {
  taskId: number;
  /** The notes have something to split (the button is off otherwise). */
  hasNotes: boolean;
  /** Tasks were created: reload the list. */
  onCreated: () => void;
  onFail: (e: unknown) => void;
}

/**
 * "Split work with AI": asks the assistant to propose tasks from this task's notes, lists them
 * with a checkmark each and editable title and note, and creates only the ticked ones. Nothing
 * is created until "Create" is pressed.
 */
export function SplitTaskPanel({ taskId, hasNotes, onCreated, onFail }: SplitTaskPanelProps) {
  const [state, setState] = useState<State>({ status: "idle" });
  const [creating, setCreating] = useState(false);
  const ticket = useRef(createLatest());

  // A different task: its proposals are not this one's.
  const shownFor = useRef(taskId);
  if (shownFor.current !== taskId) {
    shownFor.current = taskId;
    ticket.current.next();
    if (state.status !== "idle") setState({ status: "idle" });
  }

  const ask = () => {
    const t = ticket.current.next();
    setState({ status: "loading" });
    splitTaskNotes(taskId).then(
      (found) =>
        ticket.current.isCurrent(t) &&
        setState({ status: "done", rows: found.map((p) => ({ ...p, on: true })) }),
      (e: unknown) =>
        ticket.current.isCurrent(t) && setState({ status: "error", message: errorMessage(e) }),
    );
  };

  const edit = (i: number, patch: Partial<Row>) =>
    setState((s) =>
      s.status === "done" ? { ...s, rows: s.rows.map((r, j) => (j === i ? { ...r, ...patch } : r)) } : s,
    );

  const close = () => {
    ticket.current.next();
    setState({ status: "idle" });
  };

  const create = async () => {
    if (state.status !== "done") return;
    const chosen = state.rows.filter((r) => r.on && r.title.trim() !== "");
    if (chosen.length === 0) return;
    setCreating(true);
    // Rows that were created are dropped from the list, so a retry after a failure does not
    // make them twice.
    let remaining = state.rows;
    try {
      for (const row of chosen) {
        const made = await addTask(row.title.trim(), row.important);
        const note = row.note.trim();
        if (note !== "") await updateTask(made.id, { note });
        remaining = remaining.filter((r) => r !== row);
      }
      setState({ status: "idle" });
    } catch (e) {
      setState({ status: "done", rows: remaining });
      onFail(e);
    } finally {
      setCreating(false);
      onCreated();
    }
  };

  const ticked = state.status === "done" ? state.rows.filter((r) => r.on && r.title.trim() !== "").length : 0;

  return (
    <div className="border-t px-3 py-2 text-xs" onMouseDown={(e) => e.stopPropagation()}>
      <Button
        type="button"
        variant="outline"
        size="xs"
        tabIndex={-1}
        disabled={!hasNotes || state.status === "loading" || creating}
        title={hasNotes ? "Propose separate tasks from what these notes say" : "Write some notes first"}
        onClick={ask}
        className="w-full"
      >
        {state.status === "loading" ? <Loader2 className="animate-spin" aria-hidden /> : <Sparkles aria-hidden />}
        Split work with AI
      </Button>
      {state.status === "error" && (
        <p className="mt-1.5 text-destructive" role="status">
          {state.message}
        </p>
      )}
      {state.status === "done" && (
        <div className="mt-2" role="group" aria-label="Suggested tasks">
          {state.rows.length === 0 ? (
            <p className="text-muted-foreground" role="status">
              Nothing to split: the notes read as a single piece of work.
            </p>
          ) : (
            <ul className="flex max-h-72 flex-col gap-2 overflow-y-auto">
              {state.rows.map((r, i) => (
                <li key={i} className="flex gap-1.5">
                  <button
                    type="button"
                    role="checkbox"
                    aria-checked={r.on}
                    aria-label={`Create "${r.title}"`}
                    onClick={() => edit(i, { on: !r.on })}
                    className={`mt-0.5 flex size-4 shrink-0 items-center justify-center rounded border ${
                      r.on ? "border-primary bg-primary text-primary-foreground" : "border-input"
                    }`}
                  >
                    {r.on && <Check className="size-3" aria-hidden />}
                  </button>
                  <div className="flex min-w-0 flex-1 flex-col gap-1">
                    <input
                      value={r.title}
                      maxLength={MAX_TITLE_CHARS}
                      onChange={(e) => edit(i, { title: e.target.value })}
                      aria-label="New task title"
                      spellCheck={false}
                      className="w-full rounded bg-foreground/[0.04] px-1.5 py-0.5 text-xs outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    />
                    <textarea
                      value={r.note}
                      maxLength={MAX_NOTE_CHARS}
                      rows={2}
                      onChange={(e) => edit(i, { note: e.target.value })}
                      aria-label="New task notes"
                      placeholder="Notes (optional)"
                      spellCheck={false}
                      className="w-full resize-none rounded bg-foreground/[0.04] px-1.5 py-0.5 text-xs outline-none placeholder:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
                    />
                  </div>
                </li>
              ))}
            </ul>
          )}
          <div className="mt-2 flex gap-1">
            {state.rows.length > 0 && (
              <Button type="button" size="xs" tabIndex={-1} disabled={ticked === 0 || creating} onClick={create}>
                {creating && <Loader2 className="animate-spin" aria-hidden />}
                Create {ticked} {ticked === 1 ? "task" : "tasks"}
              </Button>
            )}
            <Button type="button" size="xs" variant="ghost" tabIndex={-1} disabled={creating} onClick={close}>
              {state.rows.length > 0 ? "Cancel" : "Dismiss"}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
