import { Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { ParkedItem } from "@/lib/focus";
import { relativeTime } from "@/lib/time";

interface ParkedListProps {
  items: readonly ParkedItem[];
  now: number;
  /** Turn a thought into a task. Resolves when added; the thought is dropped then. */
  onAddTask: (item: ParkedItem) => void;
  onDismiss: (id: number) => void;
}

/** The thoughts written down during a session, to act on (as a task) or drop. */
export function ParkedList({ items, now, onAddTask, onDismiss }: ParkedListProps) {
  if (items.length === 0) return null;
  return (
    <ul aria-label="Parked thoughts" className="flex max-h-32 flex-col gap-0.5 overflow-y-auto">
      {items.map((p) => (
        <li key={p.id} className="group/parked flex items-center gap-2 rounded-md px-1.5 py-1 hover:bg-foreground/[0.05]">
          <span className="min-w-0 flex-1 truncate" title={p.text}>
            {p.text}
          </span>
          <span className="shrink-0 text-muted-foreground tabular-nums">{relativeTime(p.at, now)}</span>
          <Button
            type="button"
            variant="ghost"
            size="xs"
            tabIndex={-1}
            title="Add as a task"
            aria-label={`Add as a task: ${p.text}`}
            onClick={() => onAddTask(p)}
            className="shrink-0 text-muted-foreground"
          >
            <Plus aria-hidden />
            Task
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            tabIndex={-1}
            title="Drop it"
            aria-label={`Drop: ${p.text}`}
            onClick={() => onDismiss(p.id)}
            className="shrink-0 text-muted-foreground"
          >
            <X aria-hidden />
          </Button>
        </li>
      ))}
    </ul>
  );
}
