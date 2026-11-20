import { Loader2, Moon, Sparkles, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { StaleVerdict, Task } from "@/lib/ipc";
import { relativeTime } from "@/lib/time";

interface TaskNudgeProps {
  /** The task that has been quiet the longest. */
  task: Task;
  /** How many tasks have gone quiet in all (this one included). */
  count: number;
  now: number;
  /** What the assistant said about this task, when it was asked. */
  verdict: StaleVerdict | undefined;
  asking: boolean;
  /** Start it (a waiting task) or touch it (one in flight): either way it is being worked on again. */
  onDo: (task: Task) => void;
  /** Set it aside for a week. */
  onDefer: (task: Task) => void;
  onDrop: (task: Task) => void;
  /** "Not now": ask again tomorrow. */
  onSnooze: (task: Task) => void;
  onAsk: () => void;
}

const VERDICT_TEXT = { do: "Do it", defer: "Set it aside", drop: "Drop it" } as const;

/**
 * "Still relevant?": one task that has gone quiet, with the three things to do about it. It works
 * without the assistant; asking only adds a recommendation, and nothing happens until you press one.
 */
export function TaskNudge({ task, count, now, verdict, asking, onDo, onDefer, onDrop, onSnooze, onAsk }: TaskNudgeProps) {
  const doing = task.status === "doing";
  return (
    <section
      aria-label="Gone quiet"
      className="border-b bg-amber-500/[0.07] px-4 py-2 text-sm"
      onMouseDown={(e) => e.preventDefault()}
    >
      <div className="flex items-start gap-2.5">
        <Moon className="mt-0.5 size-3.5 shrink-0 text-amber-700 dark:text-amber-400" aria-hidden />
        <div className="min-w-0 flex-1">
          <p className="truncate leading-5" title={task.title}>
            <span className="text-amber-700 dark:text-amber-400 tabular-nums">
              Quiet for {relativeTime(task.touchedAt, now)}.
            </span>{" "}
            Still relevant? <span className="font-medium">{task.title}</span>
          </p>
          {verdict && (
            <p className="truncate text-xs text-muted-foreground" title={verdict.reason}>
              <Sparkles className="mr-1 inline size-3" aria-hidden />
              Suggests: {VERDICT_TEXT[verdict.verdict]}
              {verdict.reason !== "" && `. ${verdict.reason}`}
            </p>
          )}
          <div className="mt-1.5 flex flex-wrap items-center gap-1">
            <Button
              type="button"
              size="xs"
              variant={verdict?.verdict === "do" ? "default" : "outline"}
              tabIndex={-1}
              onClick={() => onDo(task)}
            >
              {doing ? "I am on it" : "Start it"}
            </Button>
            <Button
              type="button"
              size="xs"
              variant={verdict?.verdict === "defer" ? "default" : "outline"}
              tabIndex={-1}
              onClick={() => onDefer(task)}
              title="Do not ask about it for a week (Ctrl+B does this for the selected task)"
            >
              Set aside a week
            </Button>
            <Button
              type="button"
              size="xs"
              variant={verdict?.verdict === "drop" ? "destructive" : "outline"}
              tabIndex={-1}
              onClick={() => onDrop(task)}
            >
              Drop
            </Button>
            {!verdict && (
              <Button type="button" size="xs" variant="ghost" tabIndex={-1} disabled={asking} onClick={onAsk}>
                {asking ? <Loader2 className="animate-spin" aria-hidden /> : <Sparkles aria-hidden />}
                Ask the assistant
              </Button>
            )}
            {count > 1 && <span className="ml-1 text-xs text-muted-foreground">+{count - 1} more</span>}
          </div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          tabIndex={-1}
          aria-label="Not now"
          title="Not now: ask again tomorrow"
          onClick={() => onSnooze(task)}
          className="text-muted-foreground"
        >
          <X aria-hidden />
        </Button>
      </div>
    </section>
  );
}
