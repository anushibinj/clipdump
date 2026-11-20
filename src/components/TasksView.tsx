import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
  type RefObject,
} from "react";
import { ListChecks, ListTodo, Pin, PinOff, Search, X } from "lucide-react";
import { TaskNotes } from "@/components/TaskNotes";
import { TaskRow, taskRowId } from "@/components/TaskRow";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import type { TasksApi } from "@/hooks/useTasks";
import { errorMessage, type Task, type TaskPatch } from "@/lib/ipc";
import { readTaskNotesOpen, writeTaskNotesOpen } from "@/lib/pane";
import { moveIndex } from "@/lib/shortcuts";
import { resolveTaskKey, type TaskAction } from "@/lib/taskKeys";
import {
  MAX_TITLE_CHARS,
  buildTaskView,
  nextStatus,
  parseQuickAdd,
  previousStatus,
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
  const [notesOpen, setNotesOpen] = useState(readTaskNotesOpen);
  const notesRef = useRef<HTMLTextAreaElement | null>(null);
  const setNotes = useCallback((open: boolean) => {
    setNotesOpen(open);
    writeTaskNotesOpen(open);
  }, []);

  const view = useMemo(
    () => buildTaskView(api.tasks, { showHidden, now: api.now }),
    [api.tasks, api.now, showHidden],
  );

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
  const latest = useRef({ api, view, selected, text, mode, showHidden });
  latest.current = { api, view, selected, text, mode, showHidden };

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
          else onClose();
          break;
      }
    },
    [addTyped, change, fail, onClose, onNotice, setNotes],
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

        <div className="flex h-10 items-center justify-between gap-3 border-b px-4 text-xs text-muted-foreground">
          <span className="truncate tabular-nums">
            {view.inFlight === 0
              ? "Nothing in flight"
              : `${view.inFlight} in flight`}
            {view.doneRecently > 0 && ` · ${view.doneRecently} done today`}
          </span>
          {(hasExtras || showHidden) && (
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
}: {
  ready: boolean;
  error: string | null;
  total: number;
  hidden: number;
}) {
  if (!ready) return null;
  let title: string;
  let hint: string;
  if (error) {
    title = "Tasks are not available";
    hint = error;
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
