import { memo, type PointerEvent } from "react";
import { Check, Circle, CircleDot, Clock, Hourglass, NotebookPen, Paperclip, Star } from "lucide-react";
import type { Task } from "@/lib/ipc";
import { KIND_LABEL, SIZE_LABEL, SIZE_TITLE, isDeferred } from "@/lib/tasks";
import { fullDateTime, fullTime, relativeTime } from "@/lib/time";
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
  /** Click on the notes icon: open the notes sidebar for this task. */
  onOpenNotes: (id: number) => void;
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
  onOpenNotes,
}: TaskRowProps) {
  const doing = task.status === "doing";
  const done = task.status === "done";
  const stale = doing && now - task.touchedAt >= STALE_MS;
  const aside = !done && isDeferred(task, now);
  const age = done ? (task.doneAt ?? task.touchedAt) : task.touchedAt;
  const showAge = stale || done;
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
        {task.note !== "" && (
          <div className="truncate text-xs leading-4 text-muted-foreground" title={task.note}>
            <span className="text-foreground/60">Left off:</span> {task.note}
          </div>
        )}
        {/* The assistant's suggested first step. With a note too, only the selected row shows both. */}
        {task.nextStep !== "" && !done && (selected || task.note === "") && (
          <div className="truncate text-xs leading-4 text-muted-foreground" title={task.nextStep}>
            <span className="text-foreground/60">Next:</span> {task.nextStep}
          </div>
        )}
        {task.note === "" && task.nextStep === "" && doing && selected && (
          <div className="truncate text-xs leading-4 text-muted-foreground/70">
            Add where you left off with Ctrl+E
          </div>
        )}
      </div>

      <div className="flex shrink-0 items-center gap-1.5 pt-px">
        {aside && (
          <span
            className="text-muted-foreground"
            title={`Set aside until ${fullDateTime(task.deferredUntil ?? 0)}`}
            aria-label="Set aside"
          >
            <Clock className="size-3.5" aria-hidden />
          </span>
        )}
        {task.clipIds.length > 0 && (
          <span
            className="flex items-center gap-0.5 text-xs text-muted-foreground tabular-nums"
            title={`${task.clipIds.length} linked clipboard ${task.clipIds.length === 1 ? "item" : "items"}`}
          >
            <Paperclip className="size-3" aria-hidden />
            {task.clipIds.length}
          </span>
        )}
        {task.kind === "waiting" && !done && (
          <span className="text-muted-foreground" title={KIND_LABEL.waiting} aria-label={KIND_LABEL.waiting}>
            <Hourglass className="size-3.5" aria-hidden />
          </span>
        )}
        {task.size !== null && !done && (
          <span
            className="rounded bg-foreground/[0.07] px-1 text-[11px] leading-4 text-muted-foreground"
            title={task.kind === null ? SIZE_TITLE[task.size] : `${SIZE_TITLE[task.size]}. ${KIND_LABEL[task.kind]}.`}
          >
            {SIZE_LABEL[task.size]}
          </span>
        )}
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
        <span
          className="text-xs text-muted-foreground tabular-nums"
          title={`Created ${fullDateTime(task.createdAt)}`}
          aria-label={`Created ${relativeTime(task.createdAt, now)} ago`}
        >
          {relativeTime(task.createdAt, now)}
        </span>
        <button
          type="button"
          tabIndex={-1}
          aria-label={task.note !== "" ? "Open notes" : "Add notes"}
          title="Notes (Ctrl+E)"
          onClick={() => onOpenNotes(task.id)}
          className={cn(
            "rounded p-0.5 transition-opacity",
            task.note !== ""
              ? "text-foreground/60"
              : "text-muted-foreground opacity-0 group-hover/task:opacity-100 group-data-[selected=true]/task:opacity-100",
          )}
        >
          <NotebookPen className="size-3.5" aria-hidden />
        </button>
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
