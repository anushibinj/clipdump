import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react";
import {
  Flame,
  Link2,
  ListChecks,
  ListPlus,
  ListTodo,
  Newspaper,
  Sparkles,
  WandSparkles,
  Zap,
  type LucideIcon,
} from "lucide-react";
import { FocusBanner } from "@/components/FocusBanner";
import { FocusPicker } from "@/components/FocusPicker";
import { TaskAiPanel, type TaskAiActions } from "@/components/TaskAiPanel";
import { TaskNotes } from "@/components/TaskNotes";
import { TaskNudge } from "@/components/TaskNudge";
import { TaskRow, taskRowId } from "@/components/TaskRow";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import type { FocusApi } from "@/hooks/useFocus";
import { useTaskAi } from "@/hooks/useTaskAi";
import type { TasksApi } from "@/hooks/useTasks";
import { focusedByDay, focusedToday, formatDuration, type ParkedItem } from "@/lib/focus";
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

/** The task the duration picker is open for. */
interface Picking {
  id: number;
  title: string;
}

interface TasksViewProps {
  api: TasksApi;
  /** The focus lock (owned by the App so its countdown outlives this view). */
  focus: FocusApi;
  /** The App's one input ref: the bar focuses it when it is shown. */
  inputRef: RefObject<HTMLInputElement | null>;
  onNotice: (kind: "error" | "info", message: string) => void;
  /** Hide the bar. */
  onClose: () => void;
  /** Changes each time the bar is hidden: the hidden tasks fold away again (the view itself stays open). */
  collapseSignal: number;
}

const LIST_ID = "task-list";

const PLACEHOLDER = {
  add: "Add a task. End it with ! to mark it important",
  title: "Task title",
} as const;

/** A one-line helper beside each section title, saying what the section is about. */
const SECTION_HINT: Record<string, string> = {
  now: "What you are working on right now",
  quick: "Short tasks you can finish in a few minutes",
  next: "Queued and ready to pick up next",
  later: "Set aside for now",
  done: "Finished tasks",
};

/**
 * The task list. One input does everything: type and press Enter to add, press Enter on an empty
 * input to move the selected task along, Ctrl+E to write where you left off. The keys are in
 * `lib/taskKeys.ts`, what shows (and what stays hidden) in `lib/tasks.ts`.
 */
export function TasksView({
  api,
  focus,
  inputRef,
  onNotice,
  onClose,
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
  const parkRef = useRef<HTMLInputElement | null>(null);
  const [picking, setPicking] = useState<Picking | null>(null);
  const lockedId = focus.lockedId;
  const setNotes = useCallback((open: boolean) => {
    setNotesOpen(open);
    writeTaskNotesOpen(open);
  }, []);

  const view = useMemo(
    () => buildTaskView(api.tasks, { showHidden, now: api.now, quickOnly }),
    [api.tasks, api.now, showHidden, quickOnly],
  );
  const quiet = useMemo(() => quietTasks(api.tasks, api.now), [api.tasks, api.now]);

  const focusedByTask = useMemo(() => {
    const m = new Map<number, number>();
    for (const e of focus.data.log) m.set(e.taskId, (m.get(e.taskId) ?? 0) + e.focusedMs);
    return m;
  }, [focus.data.log]);

  // Keep the selection on the same task while the list changes; when it leaves (marked done and
  // hidden, deleted), the task now at its place is selected, so a run of Enters works down a list.
  const previousRows = useRef<readonly Task[]>([]);
  // While locked, the selection cannot leave the locked task.
  const lockedVisible = lockedId !== null && view.rows.some((t) => t.id === lockedId);
  const selected = lockedVisible ? lockedId : reselect(view.rows, previousRows.current, selectedId);
  useEffect(() => {
    previousRows.current = view.rows;
    if (selected !== selectedId) setSelectedId(selected);
  }, [view.rows, selected, selectedId]);

  // Set when hover moved the selection: the pointer is already on the row, so do not scroll.
  const hoverMoved = useRef(false);
  useEffect(() => {
    if (hoverMoved.current) {
      hoverMoved.current = false;
      return;
    }
    if (selected !== null)
      document
        .getElementById(taskRowId(selected))
        ?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  // Read by the stable callbacks below.
  const latest = useRef({ api, view, selected, text, mode, showHidden, lockedId, picking, quickOnly, ai, quiet });
  latest.current = { api, view, selected, text, mode, showHidden, lockedId, picking, quickOnly, ai, quiet };
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

  /**
   * While a task is locked in, the others cannot be changed: say so and return true. Pass
   * `selfToo` for a change that would also undo the lock's own task (stepping it back, deleting it).
   */
  const lockedOut = useCallback(
    (task: Task, selfToo = false) => {
      const lid = latest.current.lockedId;
      if (lid === null) return false;
      if (task.id === lid && !selfToo) return false;
      const locked = latest.current.api.tasks.find((t) => t.id === lid);
      onNotice(
        "info",
        task.id === lid
          ? "That would undo the lock. Finish the task, or break the lock first."
          : `Locked in on “${locked?.title ?? "your task"}”. Finish it, or break the lock to switch.`,
      );
      return true;
    },
    [onNotice],
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
    (task: Task) => {
      if (lockedOut(task)) return;
      change(task.id, { deferredUntil: Date.now() + DEFER_MS }, () =>
        onNotice("info", "Set aside for a week. It will not nag you until then."),
      );
    },
    [change, lockedOut, onNotice],
  );

  const addTyped = useCallback(
    (start: boolean) => {
      const parsed = parseQuickAdd(latest.current.text);
      if (!parsed) return;
      const { api: a, showHidden: all } = latest.current;
      // Adding is always allowed, but nothing new starts while the list is locked.
      const locked = latest.current.lockedId !== null;
      a.add(parsed.title, parsed.important, start && !locked).then((task) => {
        setText("");
        if (locked) {
          onNotice("info", "Added. It waits until the lock is over.");
          return;
        }
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
          if (task && !lockedOut(task)) {
            const status = nextStatus(task.status);
            change(task.id, { status }, () => {
              if (status === "done")
                onNotice("info", "Marked done. Shift+Enter puts it back.");
            });
          }
          break;
        case "toggleDone":
          if (task && !lockedOut(task))
            change(task.id, {
              status: task.status === "done" ? "todo" : "done",
            });
          break;
        case "stepBack": {
          const status = task ? previousStatus(task.status) : null;
          if (task && status && !lockedOut(task, true)) change(task.id, { status });
          break;
        }
        case "toggleImportant":
          if (task && !lockedOut(task)) change(task.id, { important: !task.important });
          break;
        case "editNote":
          // Ctrl+E: open the notes sidebar and put the caret in it.
          if (task && !lockedOut(task)) {
            setNotes(true);
            setTimeout(() => notesRef.current?.focus(), 0);
          }
          break;
        case "editTitle":
          if (task && !lockedOut(task)) {
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
          if (task && !lockedOut(task, true)) {
            a.remove(task.id).then(() => onNotice("info", "Deleted."), fail);
          }
          break;
        case "focus":
          if (latest.current.lockedId !== null) onNotice("info", "Already locked in. Finish the task or break the lock first.");
          else if (task && task.status !== "done") setPicking({ id: task.id, title: task.title });
          break;
        case "park":
          if (latest.current.lockedId === null) onNotice("info", "Lock in on a task (Ctrl+L), then park distractions here.");
          else parkRef.current?.focus();
          break;
        case "move": {
          if (v.rows.length === 0 || latest.current.lockedId !== null) break;
          const at = v.rows.findIndex((t) => t.id === sel);
          setSelectedId(v.rows[moveIndex(at, action.delta, v.rows.length)].id);
          break;
        }
        case "escape":
          if (latest.current.picking) setPicking(null);
          else if (m.kind !== "add") {
            setMode({ kind: "add" });
            setText("");
          } else if (latest.current.text !== "") setText("");
          else if (latest.current.ai.panel) latest.current.ai.close();
          else onClose();
          break;
      }
    },
    [addTyped, change, deferWeek, fail, lockedOut, onClose, onNotice, setNotes],
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

  const onSelect = useCallback((id: number) => {
    if (latest.current.lockedId !== null) return;
    if (id !== latest.current.selected) hoverMoved.current = true;
    setSelectedId(id);
  }, []);
  const onAdvance = useCallback(
    (id: number) => {
      const task = latest.current.api.tasks.find((t) => t.id === id);
      if (task && !lockedOut(task)) change(id, { status: nextStatus(task.status) });
    },
    [change, lockedOut],
  );
  const onToggleImportant = useCallback(
    (id: number) => {
      const task = latest.current.api.tasks.find((t) => t.id === id);
      if (task && !lockedOut(task)) change(id, { important: !task.important });
    },
    [change, lockedOut],
  );

  const onOpenNotes = useCallback(
    (id: number) => {
      const task = latest.current.api.tasks.find((t) => t.id === id);
      if (task && lockedOut(task)) return;
      setSelectedId(id);
      setNotes(true);
      setTimeout(() => notesRef.current?.focus(), 0);
    },
    [lockedOut, setNotes],
  );

  // ---- focus lock -------------------------------------------------------------------------

  const onFocusTask = useCallback((id: number) => {
    const task = latest.current.api.tasks.find((t) => t.id === id);
    if (task && latest.current.lockedId === null && task.status !== "done")
      setPicking({ id: task.id, title: task.title });
  }, []);

  /** Lock in: the task goes in flight (if it was not), then the list locks to it. */
  const startFocus = useCallback(
    (minutes: number) => {
      const p = latest.current.picking;
      if (!p) return;
      const task = latest.current.api.tasks.find((t) => t.id === p.id);
      if (!task) {
        setPicking(null);
        return;
      }
      const begin = () => {
        focus.start({ id: task.id, title: task.title }, minutes);
        setPicking(null);
        setSelectedId(task.id);
        inputRef.current?.focus();
      };
      if (task.status === "doing") begin();
      else latest.current.api.update(task.id, { status: "doing" }).then(begin, fail);
    },
    [fail, focus, inputRef],
  );
  const markDone = useCallback((id: number) => change(id, { status: "done" }), [change]);
  const moreTime = useCallback((id: number, title: string) => setPicking({ id, title }), []);
  const blocked = useCallback(
    (id: number) => {
      setSelectedId(id);
      setNotes(true);
      setTimeout(() => notesRef.current?.focus(), 0);
    },
    [setNotes],
  );
  const canContinue = useCallback(
    (id: number) => {
      const task = latest.current.api.tasks.find((t) => t.id === id);
      return task !== undefined && task.status !== "done";
    },
    [],
  );
  const parkedToTask = useCallback(
    (item: ParkedItem) => {
      latest.current.api.add(item.text, false, false).then(() => {
        focus.unpark(item.id);
        onNotice("info", "Added to your tasks.");
      }, fail);
    },
    [fail, focus, onNotice],
  );
  const stats = focus.data;
  const todayMs = focusedToday(stats.log, focus.now);
  const weekTip = focusedByDay(stats.log, focus.now, 7)
    .map((d) => `${new Date(d.dayStart).toLocaleDateString(undefined, { weekday: "short" })} ${formatDuration(d.ms)}`)
    .join("\n");

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
      start: (id) => {
        const task = latest.current.api.tasks.find((t) => t.id === id);
        if (task && lockedOut(task)) return;
        change(id, { status: "doing" }, () => reveal(id));
      },
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
    [addAll, askLinks, askNextStep, change, closePanel, digest, fail, lockedOut, onNotice, pickNext, returnFocus, reveal, setPanel],
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
      <div>
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
            {!quickOnly && todayMs > 0 && ` · ${formatDuration(todayMs)} focused today`}
          </span>
          {stats.streak.best > 0 && (
            <span
              className="flex shrink-0 items-center gap-1 tabular-nums"
              title={`Focus streak: sessions in a row without breaking the lock. Best: ${stats.streak.best}\n${weekTip}`}
            >
              <Flame className="size-3.5" aria-hidden />
              {stats.streak.current}
            </span>
          )}
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
            <ToolButton icon={Link2} label="Link recent clipboard items to tasks" keys="Ctrl J" onClick={() => run({ type: "links" })} />
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

        <div
          onMouseDown={(e) => {
            // Not a window drag, and clicks on the buttons keep the caret in the task input.
            e.stopPropagation();
            if ((e.target as HTMLElement).tagName !== "INPUT") e.preventDefault();
          }}
        >
          {picking && lockedId === null && (
            <FocusPicker
              key={picking.id}
              taskTitle={picking.title}
              initialMinutes={stats.minutes}
              onStart={startFocus}
              onCancel={() => {
                setPicking(null);
                returnFocus();
              }}
            />
          )}
          <FocusBanner
            focus={focus}
            parkRef={parkRef}
            onReturnFocus={returnFocus}
            onMarkDone={markDone}
            onMoreTime={moreTime}
            onBlocked={blocked}
            canContinue={canContinue}
            onAddTask={parkedToTask}
          />
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
          onDo={(t) => {
            if (!lockedOut(t)) change(t.id, t.status === "doing" ? { note: t.note } : { status: "doing" });
          }}
          onDefer={deferWeek}
          onDrop={(t) => {
            if (!lockedOut(t, true)) api.remove(t.id).then(() => onNotice("info", "Dropped."), fail);
          }}
          onSnooze={(t) => {
            if (!lockedOut(t)) change(t.id, { deferredUntil: Date.now() + SNOOZE_MS });
          }}
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
                  {SECTION_HINT[s.key] && (
                    <span className="ml-2 font-normal tracking-normal normal-case opacity-70">
                      {SECTION_HINT[s.key]}
                    </span>
                  )}
                </h3>
                {s.tasks.map((t) => (
                  <TaskRow
                    key={t.id}
                    task={t}
                    selected={t.id === selected}
                    now={api.now}
                    onSelect={onSelect}
                    onAdvance={onAdvance}
                    onToggleImportant={onToggleImportant}
                    onOpenNotes={onOpenNotes}
                    onFocus={onFocusTask}
                    locked={lockedId !== null && t.id !== lockedId}
                    focused={t.id === lockedId}
                    focusedMs={focusedByTask.get(t.id) ?? 0}
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
            onSplitCreated={api.refresh}
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
