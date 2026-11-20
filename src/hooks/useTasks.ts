import { useCallback, useRef, useState } from "react";
import {
  addTask,
  deleteTask,
  errorMessage,
  listTasks,
  updateTask,
  type Task,
  type TaskPatch,
} from "@/lib/ipc";
import { createLatest } from "@/lib/latest";
import { minuteNow } from "@/lib/time";

export interface TasksApi {
  tasks: Task[];
  /** The first load has finished (so "no tasks" is not shown before then). */
  ready: boolean;
  /** Why the list could not be loaded, else null. */
  error: string | null;
  /** `minuteNow()` as of the last change, for the relative times. */
  now: number;
  /** Load the list again (the bar was shown). */
  refresh: () => void;
  /** Reject with a plain message (use `errorMessage`) and change nothing. */
  add: (title: string, important: boolean, start: boolean) => Promise<Task>;
  update: (id: number, patch: TaskPatch) => Promise<Task>;
  remove: (id: number) => Promise<void>;
}

/**
 * The task list. This window is the only writer, so a change the backend confirmed is applied
 * here directly instead of loading the whole list again; `refresh` is only a safety net when the
 * bar is shown. Rows are not reordered here: `buildTaskView` decides what shows and where.
 */
export function useTasks(): TasksApi {
  const [tasks, setTasks] = useState<Task[]>([]);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => minuteNow());
  const latest = useRef(createLatest());

  const refresh = useCallback(() => {
    const ticket = latest.current.next();
    listTasks().then(
      (list) => {
        if (!latest.current.isCurrent(ticket)) return;
        setTasks(list);
        setError(null);
        setReady(true);
        setNow(minuteNow());
      },
      (e: unknown) => {
        if (!latest.current.isCurrent(ticket)) return;
        setError(errorMessage(e));
        setReady(true);
      },
    );
  }, []);

  const apply = useCallback((task: Task) => {
    // A change made now beats a list request still in flight.
    latest.current.next();
    setTasks((list) =>
      list.some((t) => t.id === task.id) ? list.map((t) => (t.id === task.id ? task : t)) : [...list, task],
    );
    setNow(minuteNow());
  }, []);

  const add = useCallback(
    async (title: string, important: boolean, start: boolean) => {
      const task = await addTask(title, important, start);
      apply(task);
      return task;
    },
    [apply],
  );

  const update = useCallback(
    async (id: number, patch: TaskPatch) => {
      const task = await updateTask(id, patch);
      apply(task);
      return task;
    },
    [apply],
  );

  const remove = useCallback(async (id: number) => {
    await deleteTask(id);
    latest.current.next();
    setTasks((list) => list.filter((t) => t.id !== id));
  }, []);

  return { tasks, ready, error, now, refresh, add, update, remove };
}
