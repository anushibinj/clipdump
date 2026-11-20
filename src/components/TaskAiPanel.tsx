import { useEffect, useRef, type ReactNode } from "react";
import { Loader2, Sparkles, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import type { Energy, Task } from "@/lib/ipc";
import { ENERGY_CHOICES, MINUTE_CHOICES, type Async, type AiPanel, type LinkRow } from "@/lib/taskAi";
import { cn } from "@/lib/utils";

/** What the panel's buttons do. The view owns the tasks, so it does the work. */
export interface TaskAiActions {
  close: () => void;
  /** Escape or "close" from inside the text box: close and give the focus back to the bar's input. */
  closeAndFocus: () => void;
  pick: (minutes: number | null, energy: Energy) => void;
  /** Put the task in flight and select it. */
  start: (id: number) => void;
  /** Select the task, revealing it when it is hidden. */
  show: (id: number) => void;
  /** Ask for the next step of a task (again). */
  nextStep: (taskId: number) => void;
  addSubtasks: (titles: string[]) => void;
  clearStep: (taskId: number) => void;
  digest: (period: "morning" | "week", force?: boolean) => void;
  /** Ask which recent clipboard items belong to which task (again). */
  findLinks: () => void;
  link: (rows: LinkRow[]) => void;
  setExtractText: (text: string) => void;
  runExtract: (text: string) => void;
  toggleFound: (index: number) => void;
  addFound: () => void;
}

interface TaskAiPanelProps {
  panel: AiPanel;
  tasks: readonly Task[];
  actions: TaskAiActions;
}

const TITLES: Record<AiPanel["kind"], string> = {
  pick: "What to do now",
  steps: "Next step",
  digest: "Digest",
  links: "Clipboard items for your tasks",
  extract: "Tasks from a text",
};

/**
 * The assistant's answer, in one card above the list. The buttons keep keyboard focus in the bar's
 * input (like the rows do); `Alt+Enter` takes the main suggestion, `Esc` closes the card.
 */
export function TaskAiPanel({ panel, tasks, actions }: TaskAiPanelProps) {
  const titleOf = (id: number) => tasks.find((t) => t.id === id)?.title ?? null;
  return (
    <section
      aria-label={TITLES[panel.kind]}
      className="border-b bg-foreground/[0.03] px-4 py-2.5 text-sm"
      onMouseDown={(e) => {
        // Keep focus where it is, except in the text box.
        if (!(e.target instanceof HTMLTextAreaElement)) e.preventDefault();
      }}
    >
      <div className="mb-1.5 flex items-center gap-2">
        <Sparkles className="size-3.5 text-muted-foreground" aria-hidden />
        <h3 className="text-xs font-medium">{TITLES[panel.kind]}</h3>
        <span className="ml-auto flex items-center gap-1.5 text-[11px] text-muted-foreground">
          <Kbd className="h-4 min-w-4 px-1 text-[10px]">Esc</Kbd> closes
        </span>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          tabIndex={-1}
          aria-label="Close"
          onClick={actions.close}
          className="text-muted-foreground"
        >
          <X aria-hidden />
        </Button>
      </div>
      <div className="max-h-60 overflow-y-auto overscroll-contain">
        {panel.kind === "pick" && <Pick panel={panel} titleOf={titleOf} actions={actions} />}
        {panel.kind === "steps" && <Steps panel={panel} titleOf={titleOf} actions={actions} />}
        {panel.kind === "digest" && <Digest panel={panel} titleOf={titleOf} actions={actions} />}
        {panel.kind === "links" && <Links panel={panel} titleOf={titleOf} actions={actions} />}
        {panel.kind === "extract" && <Extract panel={panel} tasks={tasks} actions={actions} />}
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------

function Waiting({ what }: { what: string }) {
  return (
    <p className="flex items-center gap-2 text-xs text-muted-foreground" role="status">
      <Loader2 className="size-3.5 animate-spin" aria-hidden />
      {what}
    </p>
  );
}

function Failed({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <p className="flex flex-wrap items-center gap-2 text-xs text-destructive" role="alert">
      <span className="min-w-0">{message}</span>
      {onRetry && (
        <Button type="button" variant="ghost" size="xs" tabIndex={-1} onClick={onRetry}>
          Try again
        </Button>
      )}
    </p>
  );
}

/** The loading, error and answer states of one request. */
function Result<T>({
  result,
  waiting,
  onRetry,
  children,
}: {
  result: Async<T>;
  waiting: string;
  onRetry?: () => void;
  children: (value: T) => ReactNode;
}) {
  if (result.status === "loading") return <Waiting what={waiting} />;
  if (result.status === "error") return <Failed message={result.message} onRetry={onRetry} />;
  return <>{children(result.value)}</>;
}

function Choice({ label, on, onClick }: { label: string; on: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      tabIndex={-1}
      aria-pressed={on}
      onClick={onClick}
      className={cn(
        "rounded px-1.5 py-0.5 text-xs transition-colors",
        on ? "bg-foreground/[0.12] text-foreground" : "text-muted-foreground hover:bg-foreground/[0.06]",
      )}
    >
      {label}
    </button>
  );
}

type TitleOf = (id: number) => string | null;

function Pick({
  panel,
  titleOf,
  actions,
}: {
  panel: Extract<AiPanel, { kind: "pick" }>;
  titleOf: TitleOf;
  actions: TaskAiActions;
}) {
  const again = () => actions.pick(panel.minutes, panel.energy);
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
        <span className="flex items-center gap-0.5" role="group" aria-label="Time you have">
          <span className="mr-1 text-xs text-muted-foreground">Time</span>
          {MINUTE_CHOICES.map((c) => (
            <Choice
              key={c.label}
              label={c.label}
              on={panel.minutes === c.minutes}
              onClick={() => actions.pick(c.minutes, panel.energy)}
            />
          ))}
        </span>
        <span className="flex items-center gap-0.5" role="group" aria-label="Your energy">
          <span className="mr-1 text-xs text-muted-foreground">Energy</span>
          {ENERGY_CHOICES.map((c) => (
            <Choice
              key={c.label}
              label={c.label}
              on={panel.energy === c.energy}
              onClick={() => actions.pick(panel.minutes, c.energy)}
            />
          ))}
        </span>
      </div>
      <Result result={panel.result} waiting="Choosing a task…" onRetry={again}>
        {(pick) => {
          if (pick === null) return <p className="text-xs text-muted-foreground">Nothing open to pick from.</p>;
          const title = titleOf(pick.id);
          if (title === null) return <p className="text-xs text-muted-foreground">That task is gone. Ask again.</p>;
          return (
            <div className="flex items-start gap-3">
              <div className="min-w-0 flex-1">
                <p className="font-medium leading-5">{title}</p>
                {pick.reason !== "" && <p className="text-xs text-muted-foreground">{pick.reason}</p>}
              </div>
              <div className="flex shrink-0 items-center gap-1">
                <Button type="button" size="xs" tabIndex={-1} onClick={() => actions.start(pick.id)}>
                  Start it
                </Button>
                <Button type="button" size="xs" variant="ghost" tabIndex={-1} onClick={() => actions.show(pick.id)}>
                  Show it
                </Button>
              </div>
            </div>
          );
        }}
      </Result>
    </div>
  );
}

function Steps({
  panel,
  titleOf,
  actions,
}: {
  panel: Extract<AiPanel, { kind: "steps" }>;
  titleOf: TitleOf;
  actions: TaskAiActions;
}) {
  return (
    <Result result={panel.result} waiting="Thinking about the first step…" onRetry={() => actions.nextStep(panel.taskId)}>
      {(a) => (
        <div className="flex flex-col gap-2">
          <p className="text-xs text-muted-foreground">
            For <span className="text-foreground">{titleOf(panel.taskId) ?? "this task"}</span>
          </p>
          <div className="flex items-start gap-3">
            <p className="min-w-0 flex-1 leading-5">{a.step}</p>
            <Button
              type="button"
              size="xs"
              variant="ghost"
              tabIndex={-1}
              onClick={() => actions.clearStep(panel.taskId)}
              title="Remove the suggested step from the task"
            >
              Clear
            </Button>
          </div>
          {a.subtasks.length === 0 ? (
            <p className="text-xs text-muted-foreground">Small enough to do as it is.</p>
          ) : (
            <div className="flex flex-col gap-1">
              <ol className="list-decimal pl-5 text-xs">
                {a.subtasks.map((s) => (
                  <li key={s} className="py-px">
                    {s}
                  </li>
                ))}
              </ol>
              <div>
                <Button type="button" size="xs" tabIndex={-1} onClick={() => actions.addSubtasks(a.subtasks)}>
                  Add {a.subtasks.length} as tasks
                </Button>
              </div>
            </div>
          )}
        </div>
      )}
    </Result>
  );
}

function Digest({
  panel,
  titleOf,
  actions,
}: {
  panel: Extract<AiPanel, { kind: "digest" }>;
  titleOf: TitleOf;
  actions: TaskAiActions;
}) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-0.5" role="group" aria-label="Period">
        <Choice label="Today" on={panel.period === "morning"} onClick={() => actions.digest("morning")} />
        <Choice label="This week" on={panel.period === "week"} onClick={() => actions.digest("week")} />
        <Button
          type="button"
          size="xs"
          variant="ghost"
          tabIndex={-1}
          className="ml-auto"
          onClick={() => actions.digest(panel.period, true)}
        >
          Refresh
        </Button>
      </div>
      <Result result={panel.result} waiting="Writing the digest…" onRetry={() => actions.digest(panel.period, true)}>
        {(d) => (
          <div className="flex flex-col gap-1.5">
            <p className="leading-5">{d.summary}</p>
            {d.order.length > 0 && (
              <ol className="list-decimal pl-5 text-xs">
                {d.order.map((id) => {
                  const title = titleOf(id);
                  return title === null ? null : (
                    <li key={id} className="py-px">
                      <button
                        type="button"
                        tabIndex={-1}
                        onClick={() => actions.show(id)}
                        className="text-left hover:underline"
                      >
                        {title}
                      </button>
                    </li>
                  );
                })}
              </ol>
            )}
          </div>
        )}
      </Result>
    </div>
  );
}

function Links({
  panel,
  titleOf,
  actions,
}: {
  panel: Extract<AiPanel, { kind: "links" }>;
  titleOf: TitleOf;
  actions: TaskAiActions;
}) {
  return (
    <Result result={panel.result} waiting="Matching what you copied to your tasks…" onRetry={actions.findLinks}>
      {(rows) =>
        rows.length === 0 ? (
          <p className="text-xs text-muted-foreground">Nothing you copied lately clearly belongs to a task.</p>
        ) : (
          <div className="flex flex-col gap-1.5">
            <ul className="flex flex-col gap-1">
              {rows.map((r) => (
                <li key={`${r.taskId}-${r.clipId}`} className="flex items-start gap-3">
                  <div className="min-w-0 flex-1 text-xs">
                    <p className="truncate font-medium text-sm" title={titleOf(r.taskId) ?? ""}>
                      {titleOf(r.taskId) ?? "A task that is gone"}
                    </p>
                    <p className="truncate text-muted-foreground" title={r.preview}>
                      {r.preview !== "" ? r.preview : "(no longer in the history)"}
                      {r.reason !== "" && ` · ${r.reason}`}
                    </p>
                  </div>
                  <Button type="button" size="xs" variant="ghost" tabIndex={-1} onClick={() => actions.link([r])}>
                    Link
                  </Button>
                </li>
              ))}
            </ul>
            <div>
              <Button type="button" size="xs" tabIndex={-1} onClick={() => actions.link(rows)}>
                Link all {rows.length}
              </Button>
            </div>
          </div>
        )
      }
    </Result>
  );
}

function Extract({
  panel,
  tasks,
  actions,
}: {
  panel: Extract<AiPanel, { kind: "extract" }>;
  tasks: readonly Task[];
  actions: TaskAiActions;
}) {
  const box = useRef<HTMLTextAreaElement | null>(null);
  useEffect(() => box.current?.focus(), []);
  const ticked = panel.selected.filter(Boolean).length;
  return (
    <div className="flex flex-col gap-2">
      <textarea
        ref={box}
        value={panel.text}
        onChange={(e) => actions.setExtractText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            actions.closeAndFocus();
          } else if (e.key === "Enter" && e.ctrlKey) {
            e.preventDefault();
            e.stopPropagation();
            actions.runExtract(panel.text);
          }
        }}
        rows={3}
        aria-label="Text to read tasks from"
        placeholder="Paste an e-mail, a chat or notes. Leave it empty to use what you just copied."
        spellCheck={false}
        className="w-full resize-none rounded-md border bg-transparent p-2 text-sm outline-none placeholder:text-muted-foreground focus-visible:border-ring"
      />
      <div className="flex items-center gap-2">
        <Button type="button" size="xs" tabIndex={-1} onClick={() => actions.runExtract(panel.text)}>
          Find tasks
        </Button>
        <span className="text-[11px] text-muted-foreground">
          <Kbd className="h-4 min-w-4 px-1 text-[10px]">Ctrl Enter</Kbd>
        </span>
      </div>
      {panel.result !== null && (
        <Result result={panel.result} waiting="Reading the text…" onRetry={() => actions.runExtract(panel.text)}>
          {(r) =>
            r.tasks.length === 0 ? (
              <p className="text-xs text-muted-foreground">No action for you in that text.</p>
            ) : (
              <div className="flex flex-col gap-1.5">
                <ul className="flex flex-col gap-0.5">
                  {r.tasks.map((t, i) => {
                    const existing = t.duplicateOf === null ? null : tasks.find((x) => x.id === t.duplicateOf);
                    return (
                      <li key={`${i}-${t.title}`}>
                        <label className="flex cursor-pointer items-start gap-2 text-sm">
                          <input
                            type="checkbox"
                            tabIndex={-1}
                            checked={panel.selected[i] ?? false}
                            onChange={() => actions.toggleFound(i)}
                            className="mt-1"
                          />
                          <span className="min-w-0 flex-1">
                            {t.title}
                            {t.important && <span className="ml-1.5 text-xs text-amber-700 dark:text-amber-400">important</span>}
                            {existing && (
                              <span className="ml-1.5 text-xs text-muted-foreground" title={existing.title}>
                                already on the list
                              </span>
                            )}
                          </span>
                        </label>
                      </li>
                    );
                  })}
                </ul>
                <div>
                  <Button type="button" size="xs" tabIndex={-1} disabled={ticked === 0} onClick={actions.addFound}>
                    Add {ticked} {ticked === 1 ? "task" : "tasks"}
                  </Button>
                </div>
              </div>
            )
          }
        </Result>
      )}
    </div>
  );
}
