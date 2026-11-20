import { memo, type PointerEvent } from "react";
import { Check, Circle, CircleDot, Star } from "lucide-react";
import type { Task } from "@/lib/ipc";
import { fullTime, relativeTime } from "@/lib/time";
import { cn } from "@/lib/utils";

/** DOM id of a row, for scroll-into-view and `aria-activedescendant`. */
export const taskRowId = (id: number) => `task-row-${id}`;

/** A task in flight untouched for this long gets a quiet amber age, so it is not forgotten. */
export const STALE_MS = 24 * 60 * 60 * 1000;

interface TaskRowProps {
  task: Task;
  selected: boolean;
  /** Minute rounded "now" (lib/time.ts `minuteNow`), so the memo holds within a minute. */
  now: number;
  onHover: (id: number) => void;
  /** Click on the status circle: waiting, in flight, done, and around. */
  onAdvance: (id: number) => void;
  onToggleImportant: (id: number) => void;
}

const STATUS_LABEL = {
  todo: "Start",
  doing: "Mark done",
  done: "Reopen",
} as const;

function StatusIcon({ status }: { status: Task["status"] }) {
  if (status === "done") return <Check className="size-3 text-background" strokeWidth={3} aria-hidden />;
  if (status === "doing") return <CircleDot className="size-[18px] text-primary" aria-hidden />;
  return <Circle className="size-[18px] text-muted-foreground/70" aria-hidden />;
}

/** One task. Memoized: moving the selection repaints two rows, a minute tick none. */
export const TaskRow = memo(function TaskRow({
  task,
  selected,
  now,
  onHover,
  onAdvance,
  onToggleImportant,
}: TaskRowProps) {
  const doing = task.status === "doing";
  const done = task.status === "done";
  const stale = doing && now - task.touchedAt >= STALE_MS;
  const age = done ? (task.doneAt ?? task.touchedAt) : task.touchedAt;
  const showAge = doing || done;
  return (
    <div
      id={taskRowId(task.id)}
      role="option"
      aria-selected={selected}
      data-selected={selected}
      data-id={task.id}
      data-status={task.status}
      className={cn(
        "group/task mx-1 flex cursor-default items-start gap-2.5 rounded-lg px-2.5 py-2 text-sm transition-colors",
        "scroll-my-1 data-[selected=true]:bg-foreground/[0.07] dark:data-[selected=true]:bg-foreground/[0.1]",
      )}
      onPointerMove={(e: PointerEvent) => {
        if (e.movementX !== 0 || e.movementY !== 0) onHover(task.id);
      }}
      // Keep keyboard focus in the input.
      onMouseDown={(e) => e.preventDefault()}
    >
      <button
        type="button"
        tabIndex={-1}
        aria-label={`${STATUS_LABEL[task.status]}: ${task.title}`}
        title={STATUS_LABEL[task.status]}
        onClick={() => onAdvance(task.id)}
        className="mt-px flex size-[18px] shrink-0 items-center justify-center rounded-full"
      >
        {done ? (
          <span className="flex size-[18px] items-center justify-center rounded-full bg-primary">
            <StatusIcon status={task.status} />
          </span>
        ) : (
          <StatusIcon status={task.status} />
        )}
      </button>

      <div className="min-w-0 flex-1">
        <div
          className={cn(
            "truncate leading-5",
            doing && "font-medium",
            done && "text-muted-foreground line-through decoration-muted-foreground/50",
          )}
          title={task.title}
        >
          {task.title}
        </div>
        {task.note !== "" ? (
          <div className="truncate text-xs leading-4 text-muted-foreground" title={task.note}>
            <span className="text-foreground/60">Left off:</span> {task.note}
          </div>
        ) : (
          doing &&
          selected && (
            <div className="truncate text-xs leading-4 text-muted-foreground/70">
              Add where you left off with Ctrl+E
            </div>
          )
        )}
      </div>

      <div className="flex shrink-0 items-center gap-1.5 pt-px">
        {showAge && (
          <span
            className={cn(
              "text-xs tabular-nums",
              stale ? "text-amber-700 dark:text-amber-400" : "text-muted-foreground",
            )}
            title={`${done ? "Done" : "Last touched"} ${fullTime(age)}`}
          >
            {relativeTime(age, now)}
          </span>
        )}
        <button
          type="button"
          tabIndex={-1}
          aria-label={task.important ? "Not important" : "Important"}
          aria-pressed={task.important}
          title={task.important ? "Important (Ctrl+I)" : "Mark important (Ctrl+I)"}
          onClick={() => onToggleImportant(task.id)}
          className={cn(
            "rounded p-0.5 transition-opacity",
            task.important
              ? "text-amber-600 dark:text-amber-400"
              : "text-muted-foreground opacity-0 group-hover/task:opacity-100 group-data-[selected=true]/task:opacity-100",
          )}
        >
          <Star className={cn("size-3.5", task.important && "fill-current")} aria-hidden />
        </button>
      </div>
    </div>
  );
});
