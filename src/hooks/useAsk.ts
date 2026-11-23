import { useCallback, useRef, useState } from "react";
import { MAX_HISTORY, toHistory, type AskExchange } from "@/lib/ask";
import { askEverything, errorMessage, type AskAnswer } from "@/lib/ipc";
import { createLatest } from "@/lib/latest";

export interface AskApi {
  status: "idle" | "loading" | "done" | "error";
  /** The question the answer on show belongs to (trimmed). */
  asked: string;
  answer: AskAnswer | null;
  /** The finished questions before the one on show, oldest first: the conversation so far. */
  earlier: AskExchange[];
  /** Why the last question failed (a plain sentence), else null. */
  error: string | null;
  /** Ask `question`, as a follow-up to the conversation so far; one asked while another runs replaces it. */
  ask: (question: string) => void;
  /** Forget the whole conversation (a running question is dropped too). */
  reset: () => void;
}

/**
 * The Ask view's state: a conversation. Each question is sent with the finished ones before it, so
 * "and who else was there?" works. Each request takes a ticket (`createLatest`), so an answer that
 * arrives after a newer question, or after `reset`, is dropped instead of painted over it. The
 * conversation is kept while the bar is hidden and the view left, so a question is not lost by a
 * glance at the clipboard; Esc in the view clears it.
 */
export function useAsk(): AskApi {
  const [status, setStatus] = useState<AskApi["status"]>("idle");
  const [asked, setAsked] = useState("");
  const [answer, setAnswer] = useState<AskAnswer | null>(null);
  const [earlier, setEarlier] = useState<AskExchange[]>([]);
  const [error, setError] = useState<string | null>(null);
  const latest = useRef(createLatest());
  // The finished questions including the one on show; the next question's history.
  const done = useRef<AskExchange[]>([]);

  const ask = useCallback((question: string) => {
    const q = question.trim();
    if (q === "") return;
    const ticket = latest.current.next();
    const history = toHistory(done.current);
    setEarlier([...done.current]);
    setAsked(q);
    setAnswer(null);
    setStatus("loading");
    setError(null);
    askEverything(q, history).then(
      (a) => {
        if (!latest.current.isCurrent(ticket)) return;
        done.current = [...done.current, { question: q, answer: a }].slice(-MAX_HISTORY);
        setEarlier(done.current.slice(0, -1));
        setAnswer(a);
        setStatus("done");
      },
      (e: unknown) => {
        if (!latest.current.isCurrent(ticket)) return;
        setAnswer(null);
        setError(errorMessage(e));
        setStatus("error");
      },
    );
  }, []);

  const reset = useCallback(() => {
    latest.current.next();
    done.current = [];
    setStatus("idle");
    setAsked("");
    setAnswer(null);
    setEarlier([]);
    setError(null);
  }, []);

  return { status, asked, answer, earlier, error, ask, reset };
}
