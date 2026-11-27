import { useCallback, useEffect, useRef, useState } from "react";
import {
  EMPTY_DOC,
  addItem,
  addMember,
  markAlerted,
  overdueCount,
  parseDoc,
  removeItem,
  removeMember,
  setDone,
  unalerted,
  updateItem,
  type Delegation,
  type Doc,
} from "@/lib/delegations";
import { alert as sendAlert } from "@/lib/focusAlert";
import { errorMessage, getDelegations, setDelegations } from "@/lib/ipc";

/** How often deadlines are checked. A minute of slack on an alert is fine for a follow-up. */
const TICK_MS = 30_000;

export interface DelegationsApi {
  doc: Doc;
  /** The first load has finished (so "nothing delegated" is not shown before then). */
  ready: boolean;
  /** Why the delegations could not be loaded, else null. Nothing can be changed then. */
  error: string | null;
  /** `Date.now()` as of the last tick, for the due labels. */
  now: number;
  /** Delegations at or past their deadline: the number on the tab's red circle. */
  overdue: number;
  addMember: (name: string) => void;
  removeMember: (id: number) => void;
  add: (title: string, assigneeId: number, dueAt: number) => void;
  update: (id: number, patch: Partial<Pick<Delegation, "title" | "assigneeId" | "dueAt" | "notes">>) => void;
  /** Mark done (it leaves the open list, notes kept) or open again. */
  setDone: (id: number, done: boolean) => void;
  /** Delete for good, notes included. */
  remove: (id: number) => void;
}

/**
 * The team and the delegations. Lives in the App (not the view) so deadlines still alert while the
 * view is closed or the bar is hidden. This window is the only writer: every change is applied
 * here at once and saved as a whole, one save at a time. The mutators throw a plain `Error` for a
 * rule broken (see `lib/delegations.ts`) and change nothing.
 */
export function useDelegations(onError: (message: string) => void): DelegationsApi {
  const [doc, setDoc] = useState<Doc>(EMPTY_DOC);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const ref = useRef({ doc: EMPTY_DOC, loaded: false });
  const saving = useRef<Promise<unknown>>(Promise.resolve());
  const fail = useRef(onError);
  fail.current = onError;

  const commit = useCallback((next: Doc) => {
    if (!ref.current.loaded) throw new Error("the delegations are not available");
    ref.current.doc = next;
    setDoc(next);
    setNow(Date.now());
    const text = JSON.stringify(next);
    saving.current = saving.current.then(() => setDelegations(text)).catch((e: unknown) => fail.current(errorMessage(e)));
  }, []);

  // A deadline that has passed and not been announced yet: one notification, however many.
  const alertDue = useCallback(
    (at: number) => {
      const due = unalerted(ref.current.doc.items, at);
      if (due.length === 0 || !ref.current.loaded) return;
      const who = (d: Delegation) => ref.current.doc.team.find((m) => m.id === d.assigneeId)?.name ?? "";
      sendAlert(
        due.length === 1 ? "Follow up now" : `${due.length} delegations are overdue`,
        due.length === 1 ? `${due[0].title} (${who(due[0])})` : due.map((d) => d.title).join(", "),
      );
      commit(markAlerted(ref.current.doc, due.map((d) => d.id)));
    },
    [commit],
  );

  useEffect(() => {
    getDelegations().then(
      (text) => {
        try {
          const loaded = parseDoc(text);
          ref.current = { doc: loaded, loaded: true };
          setDoc(loaded);
          alertDue(Date.now());
        } catch (e) {
          setError(errorMessage(e));
        }
        setReady(true);
      },
      (e: unknown) => {
        setError(errorMessage(e));
        setReady(true);
      },
    );
    const timer = window.setInterval(() => {
      const at = Date.now();
      setNow(at);
      alertDue(at);
    }, TICK_MS);
    return () => window.clearInterval(timer);
  }, [alertDue]);

  const act = useCallback((change: (d: Doc) => Doc) => commit(change(ref.current.doc)), [commit]);

  return {
    doc,
    ready,
    error,
    now,
    overdue: overdueCount(doc.items, now),
    addMember: useCallback((name) => act((d) => addMember(d, name)), [act]),
    removeMember: useCallback((id) => act((d) => removeMember(d, id)), [act]),
    add: useCallback((title, assigneeId, dueAt) => act((d) => addItem(d, { title, assigneeId, dueAt }, Date.now())), [act]),
    update: useCallback((id, patch) => act((d) => updateItem(d, id, patch)), [act]),
    setDone: useCallback((id, done) => act((d) => setDone(d, id, done ? Date.now() : null)), [act]),
    remove: useCallback((id) => act((d) => removeItem(d, id)), [act]),
  };
}
