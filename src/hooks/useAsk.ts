import { useCallback, useRef, useState } from "react";
import { askEverything, errorMessage, type AskAnswer } from "@/lib/ipc";
import { createLatest } from "@/lib/latest";

export interface AskApi {
  status: "idle" | "loading" | "done" | "error";
  /** The question the answer on show belongs to (trimmed). */
  asked: string;
  answer: AskAnswer | null;
  /** Why the last question failed (a plain sentence), else null. */
  error: string | null;
  /** Ask `question`; a question asked while another is running replaces it. */
  ask: (question: string) => void;
  /** Forget the answer (a running question is dropped too). */
  reset: () => void;
}

/**
 * The Ask view's state. One question at a time: each request takes a ticket (`createLatest`), so an
 * answer that arrives after a newer question, or after `reset`, is dropped instead of painted over it.
 * The answer is kept while the bar is hidden and the view left, so a question is not lost by a glance
 * at the clipboard; Esc in the view clears it.
 */
export function useAsk(): AskApi {
  const [status, setStatus] = useState<AskApi["status"]>("idle");
  const [asked, setAsked] = useState("");
  const [answer, setAnswer] = useState<AskAnswer | null>(null);
  const [error, setError] = useState<string | null>(null);
  const latest = useRef(createLatest());

  const ask = useCallback((question: string) => {
    const q = question.trim();
    if (q === "") return;
    const ticket = latest.current.next();
    setAsked(q);
    setStatus("loading");
    setError(null);
    askEverything(q).then(
      (a) => {
        if (!latest.current.isCurrent(ticket)) return;
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
    setStatus("idle");
    setAsked("");
    setAnswer(null);
    setError(null);
  }, []);

  return { status, asked, answer, error, ask, reset };
}
