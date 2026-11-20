import { useCallback, useRef, useState } from "react";
import {
  addNote,
  deleteNote,
  errorMessage,
  listNotes,
  updateNote,
  type Note,
  type NotePatch,
} from "@/lib/ipc";
import { createLatest } from "@/lib/latest";
import { minuteNow } from "@/lib/time";

export interface NotesApi {
  notes: Note[];
  /** The first load has finished (so "no notes" is not shown before then). */
  ready: boolean;
  /** Why the notes could not be loaded, else null. */
  error: string | null;
  /** `minuteNow()` as of the last change, for the relative times. */
  now: number;
  /** Load the notes again (the bar was shown). */
  refresh: () => void;
  /** Reject with a plain message (use `errorMessage`) and change nothing. */
  add: (title?: string, body?: string, tags?: string[]) => Promise<Note>;
  update: (id: number, patch: NotePatch) => Promise<Note>;
  remove: (id: number) => Promise<void>;
}

/**
 * The notes. Like `useTasks`: this window is the only writer, so a change the backend
 * confirmed is applied here directly; `refresh` is only a safety net when the bar is shown.
 */
export function useNotes(): NotesApi {
  const [notes, setNotes] = useState<Note[]>([]);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => minuteNow());
  const latest = useRef(createLatest());

  const refresh = useCallback(() => {
    const ticket = latest.current.next();
    listNotes().then(
      (list) => {
        if (!latest.current.isCurrent(ticket)) return;
        setNotes(list);
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

  const apply = useCallback((note: Note) => {
    // A change made now beats a list request still in flight.
    latest.current.next();
    setNotes((list) =>
      list.some((n) => n.id === note.id) ? list.map((n) => (n.id === note.id ? note : n)) : [...list, note],
    );
    setNow(minuteNow());
  }, []);

  const add = useCallback(
    async (title?: string, body?: string, tags?: string[]) => {
      const note = await addNote(title, body, tags);
      apply(note);
      return note;
    },
    [apply],
  );

  const update = useCallback(
    async (id: number, patch: NotePatch) => {
      const note = await updateNote(id, patch);
      apply(note);
      return note;
    },
    [apply],
  );

  const remove = useCallback(async (id: number) => {
    await deleteNote(id);
    latest.current.next();
    setNotes((list) => list.filter((n) => n.id !== id));
  }, []);

  return { notes, ready, error, now, refresh, add, update, remove };
}
