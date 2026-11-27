import { useEffect, useRef, useState } from "react";
import { Check, RotateCcw, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { DelegationsApi } from "@/hooks/useDelegations";
import { fromLocalInput, isOverdue, postponed, toLocalInput, type Delegation } from "@/lib/delegations";
import { fullDateTime } from "@/lib/time";
import { cn } from "@/lib/utils";

interface Props {
  api: DelegationsApi;
  item: Delegation;
  /** Run an action and show a rule it broke as a footer message. */
  guard: (action: () => void) => void;
  onClose: () => void;
}

export const fieldClass =
  "h-8 min-w-0 rounded-md border border-input bg-transparent px-2 text-sm outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 dark:bg-input/30";

/**
 * The sidebar of one delegation: notes, who has it, and the follow-up time, which can be moved
 * (they may be on leave or busy at the deadline) but never cleared.
 */
export function DelegationPanel({ api, item, guard, onClose }: Props) {
  // The typed text lives in a ref, not read from the textarea: on unmount the element is gone already.
  const text = useRef(item.notes);
  const saved = useRef(item.notes);
  saved.current = item.notes;
  const flush = () => {
    if (text.current !== saved.current) guard(() => api.update(item.id, { notes: text.current }));
  };
  // Notes typed but not yet blurred are kept when another delegation is picked or the view closes.
  const latest = useRef(flush);
  latest.current = flush;
  useEffect(() => () => latest.current(), []);

  const done = item.doneAt !== null;
  // Deleting done work loses its notes for good: the first press only arms.
  const [armed, setArmed] = useState(false);
  const overdue = isOverdue(item, api.now);
  const postpone = (days: number) => guard(() => api.update(item.id, { dueAt: postponed(item.dueAt, api.now, days) }));

  return (
    <aside aria-label="Delegation details" className="flex w-72 shrink-0 flex-col gap-3 overflow-y-auto border-l p-3">
      <div className="flex items-start gap-2">
        <p className="min-w-0 flex-1 break-words text-sm font-medium">{item.title}</p>
        <Button type="button" variant="ghost" size="icon-xs" aria-label="Close details" title="Close (Esc)" onClick={onClose}>
          <X aria-hidden />
        </Button>
      </div>

      {done ? (
        <p className="text-xs text-muted-foreground">
          Done {fullDateTime(item.doneAt!)}, was delegated to {api.doc.team.find((m) => m.id === item.assigneeId)?.name}
        </p>
      ) : (
        <>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">
        Delegated to
        <select
          value={item.assigneeId}
          onChange={(e) => guard(() => api.update(item.id, { assigneeId: Number(e.target.value) }))}
          className={cn(fieldClass, "text-foreground")}
        >
          {api.doc.team.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name}
            </option>
          ))}
        </select>
      </label>

      <div className="flex flex-col gap-1 text-xs text-muted-foreground">
        <label htmlFor="due-at">Follow up on {overdue && <span className="font-medium text-red-600 dark:text-red-400">(overdue)</span>}</label>
        <input
          id="due-at"
          key={item.dueAt}
          type="datetime-local"
          required
          defaultValue={toLocalInput(item.dueAt)}
          onBlur={(e) => {
            const ms = fromLocalInput(e.target.value);
            // Cleared or half typed: put the old time back instead of losing the deadline.
            if (Number.isNaN(ms)) e.target.value = toLocalInput(item.dueAt);
            else if (ms !== item.dueAt) guard(() => api.update(item.id, { dueAt: ms }));
          }}
          className={cn(fieldClass, "text-foreground")}
        />
        <div className="flex items-center gap-1">
          <span>Postpone</span>
          <Button type="button" variant="outline" size="xs" onClick={() => postpone(1)}>
            1 day
          </Button>
          <Button type="button" variant="outline" size="xs" onClick={() => postpone(7)}>
            1 week
          </Button>
        </div>
      </div>
        </>
      )}

      <label className="flex min-h-0 flex-1 flex-col gap-1 text-xs text-muted-foreground">
        Notes
        <textarea
          defaultValue={item.notes}
          onChange={(e) => (text.current = e.target.value)}
          onBlur={flush}
          placeholder="What was agreed, blockers, what to ask"
          className={cn(fieldClass, "min-h-32 flex-1 resize-none py-1.5 text-foreground")}
        />
      </label>

      {done ? (
        <div className="flex gap-2">
          <Button type="button" variant="outline" size="sm" className="flex-1" onClick={() => guard(() => api.setDone(item.id, false))}>
            <RotateCcw aria-hidden />
            Reopen
          </Button>
          <Button
            type="button"
            variant={armed ? "destructive" : "outline"}
            size="sm"
            className="flex-1"
            onBlur={() => setArmed(false)}
            onClick={() => (armed ? guard(() => api.remove(item.id)) : setArmed(true))}
            title="Delete for good, notes included"
          >
            <Trash2 aria-hidden />
            {armed ? "Press again" : "Delete"}
          </Button>
        </div>
      ) : (
        <Button type="button" size="sm" onClick={() => guard(() => api.setDone(item.id, true))} title="Mark as done: it moves to Done, notes kept">
          <Check aria-hidden />
          Done
        </Button>
      )}
    </aside>
  );
}
