import { memo } from "react";
import { CircleDot, ListTodo } from "lucide-react";
import { Kbd } from "@/components/ui/kbd";
import type { Task } from "@/lib/ipc";

interface TasksButtonProps {
  /** Tasks in flight, most recently touched first. */
  inFlight: readonly Task[];
  onClick: () => void;
}

/**
 * The way into the task list, at the right of the filter row. With tasks in flight it names
 * the one touched last (and how many more), so what you are working on is in view each time
 * the bar opens, without leaving the clipboard search.
 */
export const TasksButton = memo(function TasksButton({ inFlight, onClick }: TasksButtonProps) {
  const first = inFlight[0];
  const more = inFlight.length - 1;
  return (
    <button
      type="button"
      tabIndex={-1}
      title="Tasks (Ctrl+T)"
      // Keep the caret in the search input.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className="flex h-7 max-w-[55%] min-w-0 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground transition-colors hover:bg-foreground/[0.06] hover:text-foreground"
    >
      {first ? (
        <>
          <CircleDot className="size-3.5 shrink-0 text-primary" aria-hidden />
          <span className="sr-only">Working on:</span>
          <span className="truncate text-foreground">{first.title}</span>
          {more > 0 && <span className="shrink-0 tabular-nums">+{more}</span>}
        </>
      ) : (
        <>
          <ListTodo className="size-3.5 shrink-0" aria-hidden />
          Tasks
        </>
      )}
      <Kbd className="h-4 min-w-4 shrink-0 px-1 text-[10px]">Ctrl T</Kbd>
    </button>
  );
});
