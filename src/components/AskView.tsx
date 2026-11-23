import { useCallback, useState, type KeyboardEvent, type RefObject } from "react";
import { Copy, FileText, ListChecks, Loader2, Sparkles } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { AskApi } from "@/hooks/useAsk";
import { cleanQuestion, searchedLine, sourceLabel, splitAnswer, splitAtQuote } from "@/lib/ask";
import { copyItem, errorMessage, type AskHit } from "@/lib/ipc";
import { relativeTime, fullTime } from "@/lib/time";
import { cn } from "@/lib/utils";

interface AskViewProps {
  api: AskApi;
  /** The App's one input ref: the bar focuses it when it is shown. */
  inputRef: RefObject<HTMLInputElement | null>;
  onNotice: (kind: "error" | "info", message: string) => void;
  /** Open the notes or the task list (the hit's own view). */
  onOpen: (view: "notes" | "tasks") => void;
  /** Hide the bar. */
  onClose: () => void;
}

const MAX_QUESTION = 500;
const EXAMPLES = [
  "What was that thing my manager told me 2 weeks ago?",
  "What is the wifi password I copied last month?",
  "What did I note about the budget meeting?",
];

function hitId(n: number): string {
  return `ask-hit-${n}`;
}

/**
 * "Ask": one question answered from the notes, the tasks and the clipboard history. The answer
 * cites its sources as `[n]` chips; every cited source shows the exact quote that backs the claim
 * (the backend dropped any claim whose quote it could not find), and the excerpts that were sent
 * but not cited are listed below, so the person sees what left the machine.
 */
export function AskView({
  api,
  inputRef,
  onNotice,
  onOpen,
  onClose,
}: AskViewProps) {
  const [draft, setDraft] = useState("");
  const { status, answer, error, asked } = api;

  const submit = useCallback(() => {
    const q = cleanQuestion(draft);
    if (q !== null && status !== "loading") api.ask(q);
  }, [api, draft, status]);

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter" && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      // Layers, innermost first: the typed text, the answer, then the bar.
      if (draft !== "") setDraft("");
      else if (status !== "idle") api.reset();
      else onClose();
    }
  };

  const jumpTo = (n: number) => {
    document.getElementById(hitId(n))?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  };

  const copyClip = (id: number) => {
    copyItem(id, false).then(
      () => onNotice("info", "Copied"),
      (e: unknown) => onNotice("error", errorMessage(e)),
    );
  };

  const cited = answer?.hits.filter((h) => h.cited) ?? [];
  const others = answer?.hits.filter((h) => !h.cited) ?? [];
  const now = Date.now();

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div>
        <div className="flex h-12 items-center gap-2.5 border-b px-4 transition-shadow has-[input:focus-visible]:border-ring has-[input:focus-visible]:shadow-[inset_0_-1px_0_0_var(--ring)]">
          <Sparkles className="size-4 shrink-0 text-muted-foreground" aria-hidden />
          <input
            ref={inputRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Ask your notes, tasks and clipboard anything"
            aria-label="Question"
            maxLength={MAX_QUESTION}
            autoFocus
            autoComplete="off"
            spellCheck={false}
            className="h-11 min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted-foreground"
          />
          <div className="flex shrink-0 items-center gap-0.5">
          </div>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {status === "idle" && (
          <div className="flex min-h-40 flex-col items-center justify-center gap-2 text-center">
            <Sparkles className="mb-1 size-8 text-muted-foreground/60" strokeWidth={1.5} aria-hidden />
            <p className="text-sm font-medium">Ask about anything you wrote or copied</p>
            <ul className="flex flex-col gap-1 text-xs text-muted-foreground">
              {EXAMPLES.map((ex) => (
                <li key={ex}>
                  <button
                    type="button"
                    tabIndex={-1}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => {
                      setDraft(ex);
                      api.ask(ex);
                    }}
                    className="rounded px-1.5 py-0.5 hover:bg-foreground/[0.06] hover:text-foreground"
                  >
                    “{ex}”
                  </button>
                </li>
              ))}
            </ul>
            <p className="mt-2 max-w-96 text-xs text-muted-foreground">
              Only your question and a few matching excerpts are sent, to the LLM server set up in Settings.
              Every claim in an answer is backed by a quote you can check.
            </p>
          </div>
        )}

        {status === "loading" && (
          <div role="status" className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
            <Loader2 className="size-4 animate-spin" aria-hidden />
            Looking through your notes, tasks and clipboard…
          </div>
        )}

        {status === "error" && (
          <p role="alert" className="py-6 text-sm text-destructive">
            {error}
          </p>
        )}

        {status === "done" && answer && (
          <div className="flex flex-col gap-4">
            <section aria-label="Answer" className="flex flex-col gap-1.5">
              <p className="text-xs text-muted-foreground">{asked}</p>
              <p
                role="status"
                aria-live="polite"
                className={cn("text-[15px] leading-relaxed", !answer.found && "text-muted-foreground")}
              >
                {splitAnswer(answer.answer, answer.hits).map((part, i) =>
                  part.kind === "text" ? (
                    <span key={i}>{part.text}</span>
                  ) : (
                    <button
                      key={i}
                      type="button"
                      tabIndex={-1}
                      title={`Show source ${part.n}`}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => jumpTo(part.n)}
                      className="mx-0.5 inline-flex h-4 min-w-4 items-center justify-center rounded bg-primary/15 px-1 align-baseline text-[10px] font-medium tabular-nums text-primary hover:bg-primary/25"
                    >
                      {part.n}
                    </button>
                  ),
                )}
              </p>
              <p className="text-[11px] text-muted-foreground">{searchedLine(answer.searched)}</p>
            </section>

            {cited.length > 0 && (
              <section aria-label="Sources" className="flex flex-col gap-2">
                {cited.map((h) => (
                  <HitCard key={h.n} hit={h} now={now} onCopyClip={copyClip} onOpen={onOpen} />
                ))}
              </section>
            )}

            {others.length > 0 && (
              <details className="text-xs">
                <summary className="cursor-pointer select-none text-muted-foreground hover:text-foreground">
                  {answer.found ? "Also sent to the model" : "Looked at"} ({others.length})
                </summary>
                <div className="mt-2 flex flex-col gap-2">
                  {others.map((h) => (
                    <HitCard key={h.n} hit={h} now={now} onCopyClip={copyClip} onOpen={onOpen} />
                  ))}
                </div>
              </details>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

const SOURCE_ICON = { note: FileText, task: ListChecks, clip: Copy } as const;

function HitCard({
  hit,
  now,
  onCopyClip,
  onOpen,
}: {
  hit: AskHit;
  now: number;
  onCopyClip: (id: number) => void;
  onOpen: (view: "notes" | "tasks") => void;
}) {
  const Icon = SOURCE_ICON[hit.source];
  const [before, quote, after] = splitAtQuote(hit.excerpt, hit.quote);
  return (
    <article
      id={hitId(hit.n)}
      className={cn(
        "rounded-md border px-3 py-2 text-xs",
        hit.cited ? "border-primary/30 bg-primary/[0.04]" : "bg-foreground/[0.02]",
      )}
    >
      <header className="mb-1 flex items-center gap-1.5 text-muted-foreground">
        <span className="inline-flex h-4 min-w-4 items-center justify-center rounded bg-foreground/[0.08] px-1 text-[10px] font-medium tabular-nums">
          {hit.n}
        </span>
        <Icon className="size-3.5 shrink-0" aria-hidden />
        <span>{sourceLabel(hit.source)}</span>
        <span aria-hidden>·</span>
        <time title={fullTime(hit.createdAt)} dateTime={new Date(hit.createdAt).toISOString()}>
          {relativeTime(hit.createdAt, now)} ago
        </time>
        {hit.title !== "" && (
          <span className="min-w-0 flex-1 truncate text-foreground/80" title={hit.title}>
            · {hit.title}
          </span>
        )}
        <span className="ml-auto shrink-0">
          {hit.source === "clip" ? (
            <Button
              type="button"
              variant="ghost"
              size="xs"
              tabIndex={-1}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => onCopyClip(hit.id)}
              title="Copy this clipboard item again"
            >
              Copy
            </Button>
          ) : (
            <Button
              type="button"
              variant="ghost"
              size="xs"
              tabIndex={-1}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => onOpen(hit.source === "note" ? "notes" : "tasks")}
              title={hit.source === "note" ? "Open the notes" : "Open the task list"}
            >
              Open
            </Button>
          )}
        </span>
      </header>
      <p className="whitespace-pre-wrap break-words leading-snug text-foreground/90">
        {before}
        {quote !== "" && (
          <mark className="rounded bg-primary/20 px-0.5 text-foreground">{quote}</mark>
        )}
        {after}
      </p>
    </article>
  );
}
