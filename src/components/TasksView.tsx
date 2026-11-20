import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
  type RefObject,
} from "react";
import {
  Link2,
  ListChecks,
  ListPlus,
  ListTodo,
  Newspaper,
  Pin,
  PinOff,
  Search,
  Sparkles,
  WandSparkles,
  X,
  Zap,
  type LucideIcon,
} from "lucide-react";
import { TaskAiPanel, type TaskAiActions } from "@/components/TaskAiPanel";
import { TaskNotes } from "@/components/TaskNotes";
import { TaskNudge } from "@/components/TaskNudge";
import { TaskRow, taskRowId } from "@/components/TaskRow";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { useTaskAi } from "@/hooks/useTaskAi";
import type { TasksApi } from "@/hooks/useTasks";
import { errorMessage, estimateBacklog, type Task, type TaskPatch } from "@/lib/ipc";
import { readTaskNotesOpen, writeTaskNotesOpen } from "@/lib/pane";
import { moveIndex } from "@/lib/shortcuts";
import { resolveTaskKey, type TaskAction } from "@/lib/taskKeys";
import type { LinkRow } from "@/lib/taskAi";
import {
  DEFER_MS,
  MAX_TITLE_CHARS,
  SNOOZE_MS,
  buildTaskView,
  nextStatus,
  parseQuickAdd,
  previousStatus,
  quietTasks,
  reselect,
} from "@/lib/tasks";

type Mode = { kind: "add" } | { kind: "title"; id: number };

interface TasksViewProps {
  api: TasksApi;
  /** The App's one input ref: the bar focuses it when it is shown. */
  inputRef: RefObject<HTMLInputElement | null>;
  onNotice: (kind: "error" | "info", message: string) => void;
  /** Back to the clipboard search. */
  onBack: () => void;
  /** Hide the bar. */
  onClose: () => void;
  /** Drag the window by the header. */
  onDragHeader: (e: MouseEvent) => void;
  pinned: boolean;
  onTogglePinned: () => void;
  /** Changes each time the bar is hidden: the hidden tasks fold away again (the view itself stays open). */
  collapseSignal: number;
}

const LIST_ID = "task-list";

const PLACEHOLDER = {
  add: "Add a task. End it with ! to mark it important",
  title: "Task title",
} as const;

/**
 * The task list. One input does everything: type and press Enter to add, press Enter on an empty
 * input to move the selected task along, Ctrl+E to write where you left off. The keys are in
 * `lib/taskKeys.ts`, what shows (and what stays hidden) in `lib/tasks.ts`.
 */
export function TasksView({
  api,
  inputRef,
  onNotice,
  onBack,
  onClose,
  onDragHeader,
  pinned,
  onTogglePinned,
  collapseSignal,
}: TasksViewProps) {
  const [text, setText] = useState("");
  const [mode, setMode] = useState<Mode>({ kind: "add" });
  const [showHidden, setShowHidden] = useState(false);
  useEffect(() => setShowHidden(false), [collapseSignal]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  // "Quick wins": only the tasks estimated as quick. Stays as it is when the bar hides, like the view.
  const [quickOnly, setQuickOnly] = useState(false);
  // Bumped when a draft of "where I left off" is asked for; the notes sidebar answers it.
  const [draftTick, setDraftTick] = useState(0);
  const ai = useTaskAi(api.refresh);
  const [notesOpen, setNotesOpen] = useState(readTaskNotesOpen);
  const notesRef = useRef<HTMLTextAreaElement | null>(null);
  const setNotes = useCallback((open: boolean) => {
    setNotesOpen(open);
    writeTaskNotesOpen(open);
  }, []);

  const view = useMemo(
    () => buildTaskView(api.tasks, { showHidden, now: api.now, quickOnly }),
    [api.tasks, api.now, showHidden, quickOnly],
  );
  const quiet = useMemo(() => quietTasks(api.tasks, api.now), [api.tasks, api.now]);

  // Keep the selection on the same task while the list changes; when it leaves (marked done and
  // hidden, deleted), the task now at its place is selected, so a run of Enters works down a list.
  const previousRows = useRef<readonly Task[]>([]);
  const selected = reselect(view.rows, previousRows.current, selectedId);
  useEffect(() => {
    previousRows.current = view.rows;
    if (selected !== selectedId) setSelectedId(selected);
  }, [view.rows, selected, selectedId]);

  useEffect(() => {
    if (selected !== null)
      document
        .getElementById(taskRowId(selected))
        ?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  // Read by the stable callbacks below.
  const latest = useRef({ api, view, selected, text, mode, showHidden, quickOnly, ai, quiet });
  latest.current = { api, view, selected, text, mode, showHidden, quickOnly, ai, quiet };
  // The assistant card's buttons, set below once they exist: `run` takes its main suggestion (Alt+Enter).
  const actionsRef = useRef<TaskAiActions | null>(null);

  const fail = useCallback(
    (e: unknown) => onNotice("error", errorMessage(e)),
    [onNotice],
  );

  const change = useCallback(
    (id: number, patch: TaskPatch, after?: (t: Task) => void) => {
      latest.current.api.update(id, patch).then(after, fail);
    },
    [fail],
  );

  /** Select a task, showing the hidden ones first when it is one of them. */
  const reveal = useCallback((id: number) => {
    if (!latest.current.view.rows.some((t) => t.id === id)) {
      setQuickOnly(false);
      setShowHidden(true);
    }
    setSelectedId(id);
  }, []);

  /** Add tasks one after the other; resolves to how many were added (a failure is reported and stops). */
  const addAll = useCallback(
    async (items: readonly { title: string; important: boolean }[]) => {
      let added = 0;
      try {
        for (const item of items) {
          await latest.current.api.add(item.title, item.important, false);
          added += 1;
        }
      } catch (e) {
        fail(e);
      }
      if (added > 0)
        onNotice(
          "info",
          `Added ${added} ${added === 1 ? "task" : "tasks"}.${latest.current.showHidden ? "" : " Waiting ones are under Ctrl+H."}`,
        );
      return added;
    },
    [fail, onNotice],
  );

  const deferWeek = useCallback(
    (task: Task) =>
      change(task.id, { deferredUntil: Date.now() + DEFER_MS }, () =>
        onNotice("info", "Set aside for a week. It will not nag you until then."),
      ),
    [change, onNotice],
  );

  const addTyped = useCallback(
    (start: boolean) => {
      const parsed = parseQuickAdd(latest.current.text);
      if (!parsed) return;
      const { api: a, showHidden: all } = latest.current;
      a.add(parsed.title, parsed.important, start).then((task) => {
        setText("");
        // A new task the calm view would hide must not just vanish: say where it went.
        const shown = buildTaskView([...a.tasks, task], {
          showHidden: all,
          now: a.now,
        }).rows.some((t) => t.id === task.id);
        if (shown) setSelectedId(task.id);
        else
          onNotice(
            "info",
            "Added. It is in the hidden tasks, press Ctrl+H to show them.",
          );
      }, fail);
    },
    [fail, onNotice],
  );

  const run = useCallback(
    (action: TaskAction) => {
      const { view: v, selected: sel, mode: m, api: a } = latest.current;
      const task =
        sel === null ? null : (a.tasks.find((t) => t.id === sel) ?? null);
      switch (action.type) {
        case "add":
          addTyped(false);
          break;
        case "addStart":
          addTyped(true);
          break;
        case "advance":
          if (task) {
            const status = nextStatus(task.status);
            change(task.id, { status }, () => {
              if (status === "done")
                onNotice("info", "Marked done. Shift+Enter puts it back.");
            });
          }
          break;
        case "toggleDone":
          if (task)
            change(task.id, {
              status: task.status === "done" ? "todo" : "done",
            });
          break;
        case "stepBack": {
          const status = task ? previousStatus(task.status) : null;
          if (task && status) change(task.id, { status });
          break;
        }
        case "toggleImportant":
          if (task) change(task.id, { important: !task.important });
          break;
        case "editNote":
          // Ctrl+E: open the notes sidebar and put the caret in it.
          if (task) {
            setNotes(true);
            setTimeout(() => notesRef.current?.focus(), 0);
          }
          break;
        case "editTitle":
          if (task) {
            setMode({ kind: "title", id: task.id });
            setText(task.title);
          }
          break;
        case "commitEdit": {
          if (m.kind === "add") break;
          const value = latest.current.text.trim();
          if (m.kind === "title" && value === "") {
            onNotice("error", "The task needs a title.");
            break;
          }
          change(m.id, { title: value }, () => {
            setMode({ kind: "add" });
            setText("");
          });
          break;
        }
        case "toggleHidden":
          setShowHidden((s) => !s);
          break;
        case "pickNext":
          latest.current.ai.pickNext();
          break;
        case "toggleQuick": {
          const on = !latest.current.quickOnly;
          setQuickOnly(on);
          // Quick wins need estimates: ask for the ones that are missing (they arrive by themselves).
          if (on && v.unsized > 0)
            estimateBacklog().then(
              (n) => n > 0 && onNotice("info", `Estimating ${n} ${n === 1 ? "task" : "tasks"}.`),
              (e: unknown) => onNotice("info", errorMessage(e)),
            );
          break;
        }
        case "nextStep":
          if (task) latest.current.ai.nextStep(task.id);
          else onNotice("info", "Select a task first.");
          break;
        case "draftNote":
          if (task) {
            setNotes(true);
            setDraftTick((n) => n + 1);
          } else onNotice("info", "Select a task first.");
          break;
        case "digest":
          latest.current.ai.digest(action.period);
          break;
        case "links":
          latest.current.ai.links();
          break;
        case "extract":
          latest.current.ai.openExtract();
          break;
        case "defer":
          if (task) deferWeek(task);
          break;
        case "panelPrimary": {
          const p = latest.current.ai.panel;
          const actions = actionsRef.current;
          if (!p || !actions) break;
          if (p.kind === "pick" && p.result.status === "done" && p.result.value) actions.start(p.result.value.id);
          else if (p.kind === "steps" && p.result.status === "done" && p.result.value.subtasks.length > 0)
            actions.addSubtasks(p.result.value.subtasks);
          else if (p.kind === "links" && p.result.status === "done" && p.result.value.length > 0)
            actions.link(p.result.value);
          else if (p.kind === "extract" && p.result?.status === "done") actions.addFound();
          else if (p.kind === "digest" && p.result.status === "done" && p.result.value.order.length > 0)
            actions.show(p.result.value.order[0]);
          break;
        }
        case "delete":
          if (task) {
            a.remove(task.id).then(() => onNotice("info", "Deleted."), fail);
          }
          break;
        case "move": {
          if (v.rows.length === 0) break;
          const at = v.rows.findIndex((t) => t.id === sel);
          setSelectedId(v.rows[moveIndex(at, action.delta, v.rows.length)].id);
          break;
        }
        case "escape":
          if (m.kind !== "add") {
            setMode({ kind: "add" });
            setText("");
          } else if (latest.current.text !== "") setText("");
          else if (latest.current.ai.panel) latest.current.ai.close();
          else onClose();
          break;
      }
    },
    [addTyped, change, deferWeek, fail, onClose, onNotice, setNotes],
  );

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      const { text: t, mode: m } = latest.current;
      const action = resolveTaskKey(e.nativeEvent, {
        inputEmpty: t.trim() === "",
        editing: m.kind !== "add",
      });
      if (!action) return;
      e.preventDefault();
      e.stopPropagation();
      // Holding a key repeats moving only: not a run of "mark done".
      if (e.repeat && action.type !== "move") return;
      run(action);
    },
    [run],
  );

  const onHover = useCallback((id: number) => setSelectedId(id), []);
  const onAdvance = useCallback(
    (id: number) => {
      const task = latest.current.api.tasks.find((t) => t.id === id);
      if (task) change(id, { status: nextStatus(task.status) });
    },
    [change],
  );
  const onToggleImportant = useCallback(
    (id: number) => {
      const task = latest.current.api.tasks.find((t) => t.id === id);
      if (task) change(id, { important: !task.important });
    },
    [change],
  );

  const onOpenNotes = useCallback(
    (id: number) => {
      setSelectedId(id);
      setNotes(true);
      setTimeout(() => notesRef.current?.focus(), 0);
    },
    [setNotes],
  );
  const saveNote = useCallback(
    (id: number, note: string) => change(id, { note }),
    [change],
  );
  const returnFocus = useCallback(() => inputRef.current?.focus(), [inputRef]);
  const selectedTask =
    selected === null
      ? null
      : (api.tasks.find((t) => t.id === selected) ?? null);

  const { setPanel, close: closePanel, pickNext, nextStep: askNextStep, digest, links: askLinks } = ai;
  const aiActions: TaskAiActions = useMemo(
    () => ({
      close: closePanel,
      closeAndFocus: () => {
        closePanel();
        returnFocus();
      },
      pick: pickNext,
      start: (id) => change(id, { status: "doing" }, () => reveal(id)),
      show: reveal,
      nextStep: askNextStep,
      addSubtasks: (titles) => {
        void addAll(titles.map((title) => ({ title, important: false }))).then((added) =>
          // The ones that went in leave the card; a failure leaves the rest to try again.
          setPanel((p) =>
            p && p.kind === "steps" && p.result.status === "done"
              ? { ...p, result: { status: "done", value: { ...p.result.value, subtasks: p.result.value.subtasks.slice(added) } } }
              : p,
          ),
        );
      },
      clearStep: (taskId) => change(taskId, { nextStep: "" }),
      digest,
      findLinks: askLinks,
      link: (rows: LinkRow[]) => {
        const a = latest.current.api;
        const byTask = new Map<number, number[]>();
        for (const r of rows) byTask.set(r.taskId, [...(byTask.get(r.taskId) ?? []), r.clipId]);
        const writes = Array.from(byTask, ([taskId, clipIds]) => {
          const task = a.tasks.find((t) => t.id === taskId);
          if (!task) return Promise.resolve();
          const merged = Array.from(new Set([...task.clipIds, ...clipIds]));
          return a.update(taskId, { clipIds: merged }).then(() => undefined);
        });
        Promise.all(writes).then(() => {
          onNotice("info", rows.length === 1 ? "Linked." : `Linked ${rows.length} clipboard items.`);
          setPanel((p) =>
            p && p.kind === "links" && p.result.status === "done"
              ? {
                  ...p,
                  result: {
                    status: "done",
                    value: p.result.value.filter((r) => !rows.some((x) => x.taskId === r.taskId && x.clipId === r.clipId)),
                  },
                }
              : p,
          );
        }, fail);
      },
      setExtractText: (value) => setPanel((p) => (p && p.kind === "extract" ? { ...p, text: value } : p)),
      runExtract: ai.runExtract,
      toggleFound: (i) =>
        setPanel((p) =>
          p && p.kind === "extract" ? { ...p, selected: p.selected.map((on, j) => (j === i ? !on : on)) } : p,
        ),
      addFound: () => {
        const p = latest.current.ai.panel;
        if (!p || p.kind !== "extract" || p.result?.status !== "done") return;
        const chosen = p.result.value.tasks.filter((_, i) => p.selected[i]);
        if (chosen.length === 0) return;
        void addAll(chosen).then((added) => {
          if (added === 0) return;
          // The box that had the focus goes away with the card: hand the focus back to the input.
          closePanel();
          returnFocus();
        });
      },
    }),
    // `ai.runExtract` and the others are stable callbacks; `ai` itself is a new object each render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [addAll, askLinks, askNextStep, change, closePanel, digest, fail, onNotice, pickNext, returnFocus, reveal, setPanel],
  );
  actionsRef.current = aiActions;

  const nudgeTask = ai.panel === null && !quickOnly && mode.kind === "add" ? (quiet[0] ?? null) : null;
  const unlink = useCallback(
    (taskId: number, clipId: number) => {
      const task = latest.current.api.tasks.find((t) => t.id === taskId);
      if (task) change(taskId, { clipIds: task.clipIds.filter((c) => c !== clipId) });
    },
    [change],
  );

  const editing = mode.kind !== "add";
  const maxLength = MAX_TITLE_CHARS;
  const hasExtras = view.hiddenCount > 0 || view.doneRecently > 0;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div onMouseDown={onDragHeader}>
        <div className="flex h-12 items-center gap-2.5 border-b px-4 transition-shadow has-[input:focus-visible]:border-ring has-[input:focus-visible]:shadow-[inset_0_-1px_0_0_var(--ring)]">
          <ListTodo
            className="size-4 shrink-0 text-muted-foreground"
            aria-hidden
          />
          {editing && (
            <Badge variant="secondary" className="shrink-0">
              Title
            </Badge>
          )}
          <input
            ref={inputRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={PLACEHOLDER[mode.kind]}
            role="combobox"
            aria-expanded="true"
            aria-controls={LIST_ID}
            aria-autocomplete="none"
            aria-label={editing ? PLACEHOLDER[mode.kind] : "Add a task"}
            aria-activedescendant={
              selected === null ? undefined : taskRowId(selected)
            }
            maxLength={maxLength}
            autoFocus
            autoComplete="off"
            spellCheck={false}
            className="h-11 min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted-foreground"
          />
          <div className="flex shrink-0 items-center gap-0.5">
            <Button
              type="button"
              variant="ghost"
              size="xs"
              tabIndex={-1}
              title="Back to clipboard search (Ctrl+T)"
              onMouseDown={(e) => e.preventDefault()}
              onClick={onBack}
              className="gap-1.5 text-muted-foreground"
            >
              <Search aria-hidden />
              Search
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              tabIndex={-1}
              aria-label={pinned ? "Unpin window" : "Pin window open"}
              aria-pressed={pinned}
              title={
                pinned
                  ? "Unpin: hide when focus is lost"
                  : "Pin: keep the window open"
              }
              onMouseDown={(e) => e.preventDefault()}
              onClick={onTogglePinned}
              className={pinned ? "text-foreground" : "text-muted-foreground"}
            >
              {pinned ? (
                <Pin className="fill-current" aria-hidden />
              ) : (
                <PinOff aria-hidden />
              )}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              tabIndex={-1}
              aria-label="Dismiss"
              title="Dismiss (Esc)"
              onMouseDown={(e) => e.preventDefault()}
              onClick={onClose}
              className="text-muted-foreground"
            >
              <X aria-hidden />
            </Button>
          </div>
        </div>

        <div className="flex h-10 items-center gap-2 border-b px-4 text-xs text-muted-foreground">
          <span className="min-w-0 flex-1 truncate tabular-nums">
            {quickOnly
              ? `Quick wins${view.unsized > 0 ? ` · ${view.unsized} not estimated yet` : ""}`
              : view.inFlight === 0
                ? "Nothing in flight"
                : `${view.inFlight} in flight`}
            {!quickOnly && view.doneRecently > 0 && ` · ${view.doneRecently} done today`}
          </span>
          <div className="flex shrink-0 items-center gap-0.5" role="toolbar" aria-label="Assistant">
            <ToolButton
              icon={Zap}
              label="Quick wins"
              keys="Ctrl Q"
              pressed={quickOnly}
              badge={view.quickCount}
              onClick={() => run({ type: "toggleQuick" })}
            />
            <ToolButton icon={Sparkles} label="What next" keys="Ctrl N" onClick={() => run({ type: "pickNext" })} />
            <ToolButton icon={WandSparkles} label="Next step for the selected task" keys="Ctrl G" onClick={() => run({ type: "nextStep" })} />
            <ToolButton icon={Newspaper} label="Digest" keys="Ctrl D" onClick={() => run({ type: "digest", period: "morning" })} />
            <ToolButton icon={Link2} label="Link recent clipboard items to tasks" keys="Ctrl L" onClick={() => run({ type: "links" })} />
            <ToolButton icon={ListPlus} label="Tasks from a text" keys="Ctrl K" onClick={() => run({ type: "extract" })} />
          </div>
          {(hasExtras || showHidden) && !quickOnly && (
            <button
              type="button"
              tabIndex={-1}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setShowHidden((s) => !s)}
              className="flex shrink-0 items-center gap-1.5 rounded-md px-1.5 py-1 hover:bg-foreground/[0.06] hover:text-foreground"
              aria-pressed={showHidden}
            >
              {showHidden
                ? "Show less"
                : view.hiddenCount > 0
                  ? `Show ${view.hiddenCount} hidden`
                  : "Show done"}
              <Kbd className="h-4 min-w-4 px-1 text-[10px]">Ctrl H</Kbd>
            </button>
          )}
        </div>
      </div>

      {ai.panel && <TaskAiPanel panel={ai.panel} tasks={api.tasks} actions={aiActions} />}
      {nudgeTask && (
        <TaskNudge
          task={nudgeTask}
          count={quiet.length}
          now={api.now}
          verdict={ai.verdicts.get(nudgeTask.id)}
          asking={ai.reviewing}
          onDo={(t) => change(t.id, t.status === "doing" ? { note: t.note } : { status: "doing" })}
          onDefer={deferWeek}
          onDrop={(t) => api.remove(t.id).then(() => onNotice("info", "Dropped."), fail)}
          onSnooze={(t) => change(t.id, { deferredUntil: Date.now() + SNOOZE_MS })}
          onAsk={() => {
            void ai.review(quiet.slice(0, 10).map((t) => t.id)).then((message) => message && onNotice("error", message));
          }}
        />
      )}

      <div className="flex min-h-0 flex-1">
        <div
          id={LIST_ID}
          role="listbox"
          aria-label="Tasks"
          className="min-h-0 min-w-0 flex-1 overflow-y-auto overscroll-contain py-1"
        >
          {view.rows.length === 0 ? (
            <Empty
              ready={api.ready}
              error={api.error}
              total={api.tasks.length}
              hidden={view.hiddenCount}
              quick={quickOnly}
              unsized={view.unsized}
            />
          ) : (
            view.sections.map((s) => (
              <section key={s.key} aria-label={s.title}>
                <h3 className="px-4 pt-2 pb-1 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
                  {s.title}{" "}
                  <span className="tabular-nums">{s.tasks.length}</span>
                </h3>
                {s.tasks.map((t) => (
                  <TaskRow
                    key={t.id}
                    task={t}
                    selected={t.id === selected}
                    now={api.now}
                    onHover={onHover}
                    onAdvance={onAdvance}
                    onToggleImportant={onToggleImportant}
                    onOpenNotes={onOpenNotes}
                  />
                ))}
              </section>
            ))
          )}
        </div>
        {notesOpen && selectedTask && (
          <TaskNotes
            task={selectedTask}
            now={api.now}
            onSave={saveNote}
            onClose={() => {
              setNotes(false);
              returnFocus();
            }}
            onReturnFocus={returnFocus}
            textareaRef={notesRef}
            draftRequest={draftTick}
            onUnlink={unlink}
            onFail={fail}
          />
        )}
      </div>
    </div>
  );
}

function Empty({
  ready,
  error,
  total,
  hidden,
  quick,
  unsized,
}: {
  ready: boolean;
  error: string | null;
  total: number;
  hidden: number;
  quick: boolean;
  unsized: number;
}) {
  if (!ready) return null;
  let title: string;
  let hint: string;
  if (error) {
    title = "Tasks are not available";
    hint = error;
  } else if (quick) {
    title = "No quick wins";
    hint =
      unsized > 0
        ? `${unsized} ${unsized === 1 ? "task has" : "tasks have"} no estimate yet. They appear here as the assistant sizes them. Press Ctrl+Q to see everything.`
        : "Nothing is estimated as about 15 minutes or less. Press Ctrl+Q to see everything.";
  } else if (total === 0) {
    title = "Nothing on the list";
    hint =
      "Type a task and press Enter. Ctrl+Enter adds it and starts it right away.";
  } else if (hidden > 0) {
    title = "Nothing in flight";
    hint = `${hidden} hidden. Press Ctrl+H to see them, or add a task ending with ! to keep it up next.`;
  } else {
    title = "All clear";
    hint = "Type a task and press Enter to add the next one.";
  }
  return (
    <div className="flex h-full min-h-40 flex-col items-center justify-center gap-1.5 px-6 text-center">
      <ListChecks
        className="mb-1 size-8 text-muted-foreground/60"
        strokeWidth={1.5}
        aria-hidden
      />
      <p className="text-sm font-medium">{title}</p>
      <p className="max-w-80 text-xs text-muted-foreground">{hint}</p>
    </div>
  );
}

/** One of the assistant's buttons in the status row: an icon, its name and key in the tooltip. */
function ToolButton({
  icon: Icon,
  label,
  keys,
  onClick,
  pressed,
  badge,
}: {
  icon: LucideIcon;
  label: string;
  keys: string;
  onClick: () => void;
  pressed?: boolean;
  /** A count shown beside the icon (quick wins). */
  badge?: number;
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      size="xs"
      tabIndex={-1}
      aria-label={label}
      aria-pressed={pressed}
      title={`${label} (${keys.replace(" ", "+")})`}
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className={pressed ? "bg-foreground/[0.1] px-1.5 text-foreground" : "px-1.5 text-muted-foreground"}
    >
      <Icon aria-hidden />
      {badge !== undefined && badge > 0 && <span className="tabular-nums">{badge}</span>}
    </Button>
  );
}
