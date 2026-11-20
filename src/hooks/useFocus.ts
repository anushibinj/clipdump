import { useCallback, useEffect, useRef, useState } from "react";
import {
  answerCheckin,
  breakLock,
  dismissCheckin,
  endBreak,
  finishEarly,
  formatClock,
  park,
  pauseSession,
  releaseLock,
  remainingMs,
  resumeSession,
  startBreak,
  startSession,
  tick,
  unpark,
  type FocusData,
} from "@/lib/focus";
import { alert as sendAlert, requestAlertPermission } from "@/lib/focusAlert";
import { readFocus, writeFocus } from "@/lib/focusStore";
import type { Task } from "@/lib/ipc";

export interface FocusApi {
  data: FocusData;
  /** The clock as of the last second tick, so a countdown repaints without its own timer. */
  now: number;
  /** The task the list is locked to, null when nothing is locked. */
  lockedId: number | null;
  start: (task: { id: number; title: string }, minutes: number) => void;
  pause: () => void;
  resume: () => void;
  /** Break the lock on purpose: the streak starts over. The 30 second wait is the caller's. */
  breakLock: () => void;
  answer: (answer: "done" | "more" | "blocked") => void;
  dismissCheckin: () => void;
  startBreak: () => void;
  endBreak: () => void;
  park: (text: string) => void;
  unpark: (id: number) => void;
}

/**
 * The focus lock. One owner for the whole app (it lives in `App`, not in the task view), so the
 * countdown keeps running and the end is announced while the user is on the search view or the
 * bar is hidden. The state is persisted on every change (`lib/focusStore.ts`); the rules are in
 * `lib/focus.ts`. `onNotice` shows a short footer message.
 */
export function useFocus(
  tasks: readonly Task[],
  tasksReady: boolean,
  onNotice: (kind: "error" | "info", message: string) => void,
): FocusApi {
  // A session that ran out while the app was closed ends here, at load.
  const [initial] = useState(() => tick(readFocus(), Date.now()).data);
  const ref = useRef<FocusData>(initial);
  const [data, setData] = useState<FocusData>(initial);
  const [now, setNow] = useState(() => Date.now());
  const noticeRef = useRef(onNotice);
  noticeRef.current = onNotice;

  const commit = useCallback((change: (d: FocusData) => FocusData) => {
    const next = change(ref.current);
    if (next === ref.current) return;
    ref.current = next;
    setData(next);
    writeFocus(next);
  }, []);

  const step = useCallback(() => {
    const t = Date.now();
    const { data: next, events } = tick(ref.current, t);
    if (next !== ref.current) {
      ref.current = next;
      setData(next);
      writeFocus(next);
    }
    setNow(t);
    const title = next.checkin?.title ?? next.session?.title ?? "";
    for (const e of events) {
      if (e === "warn") {
        noticeRef.current("info", "5 minutes left in your focus session.");
        sendAlert("Focus: 5 minutes left", title);
      } else if (e === "ended") {
        noticeRef.current("info", "Focus session over. Well done.");
        sendAlert("Focus session over", title);
      } else {
        noticeRef.current("info", "Break over.");
        sendAlert("Break over", "Ready for the next one?");
      }
    }
  }, []);

  const active = data.session !== null || data.breakEndsAt !== null;
  useEffect(() => {
    if (!active) return;
    step();
    const timer = setInterval(step, 1000);
    // Timers are slowed in a hidden window: catch up the moment it is shown again.
    const onVisible = () => {
      if (document.visibilityState === "visible") step();
    };
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", step);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", step);
    };
  }, [active, step]);

  // The countdown in the window title, so it shows in the taskbar and the tab switcher.
  const baseTitle = useRef<string | null>(null);
  const session = data.session;
  useEffect(() => {
    if (baseTitle.current === null) baseTitle.current = document.title;
    document.title = session
      ? `⏱ ${formatClock(remainingMs(session, now))} · ${session.title}`
      : baseTitle.current;
  }, [session, now]);

  // The locked task finished (or vanished) by another way than the lock's own buttons: the lock
  // follows it. Not before the first load of the list, which would look like "task gone".
  useEffect(() => {
    const s = data.session;
    if (!s || !tasksReady) return;
    const task = tasks.find((t) => t.id === s.taskId);
    if (!task) {
      commit(releaseLock);
      noticeRef.current("info", "The locked task is gone, so the lock is off.");
    } else if (task.status === "done") {
      commit((d) => finishEarly(d, Date.now()));
      noticeRef.current("info", "Done before the time was up. That counts.");
    }
  }, [data.session, tasks, tasksReady, commit]);

  const start = useCallback(
    (task: { id: number; title: string }, minutes: number) => {
      requestAlertPermission();
      commit((d) => startSession(d, task, minutes, Date.now()));
      setNow(Date.now());
    },
    [commit],
  );

  return {
    data,
    now,
    lockedId: data.session?.taskId ?? null,
    start,
    pause: useCallback(() => commit((d) => pauseSession(d, Date.now())), [commit]),
    resume: useCallback(() => commit((d) => resumeSession(d, Date.now())), [commit]),
    breakLock: useCallback(() => {
      commit((d) => breakLock(d, Date.now()));
      noticeRef.current("info", "Lock broken. Your streak starts over.");
    }, [commit]),
    answer: useCallback((a: "done" | "more" | "blocked") => commit((d) => answerCheckin(d, a)), [commit]),
    dismissCheckin: useCallback(() => commit(dismissCheckin), [commit]),
    startBreak: useCallback(() => {
      commit((d) => startBreak(d, Date.now()));
      setNow(Date.now());
    }, [commit]),
    endBreak: useCallback(() => commit(endBreak), [commit]),
    park: useCallback((text: string) => commit((d) => park(d, text, Date.now())), [commit]),
    unpark: useCallback((id: number) => commit((d) => unpark(d, id)), [commit]),
  };
}
