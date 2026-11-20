import { Loader2, RefreshCw } from "lucide-react";
import { SplitParts } from "@/components/SplitParts";
import { Button } from "@/components/ui/button";
import type { SplitControl } from "@/hooks/useSplit";
import type { ClipKind } from "@/lib/ipc";

interface SplitTabProps {
  /** Kind of the selected row, null when nothing is selected. */
  kind: ClipKind | null;
  split: SplitControl;
  /** A copy failed: the footer message of the bar. */
  onError: (message: string) => void;
}

function Note({ children }: { children: string }) {
  return <p className="p-4 text-xs text-muted-foreground">{children}</p>;
}

/**
 * The "AI split" tab: the copyable pieces of the selected text, made by the LLM without being asked
 * (see `useSplit`). The one manual control is Re-split, for when the first answer is not right.
 */
export function SplitTab({ kind, split, onError }: SplitTabProps) {
  if (kind === null) return <Note>Nothing selected.</Note>;
  if (kind !== "text") return <Note>Only text items are split into pieces.</Note>;

  const { state } = split;
  const busy = state.status === "pending" || state.status === "queued" || state.status === "running";
  if (busy) {
    return (
      <p className="flex items-center gap-2 p-4 text-xs text-muted-foreground" role="status">
        <Loader2 className="size-3.5 animate-spin" aria-hidden />
        {state.status === "queued" ? "Waiting for the LLM..." : "Splitting with the LLM..."}
      </p>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b px-3 text-xs">
        <span className="text-muted-foreground tabular-nums">
          {state.status === "done" ? `${state.parts.length} ${state.parts.length === 1 ? "item" : "items"}` : "Failed"}
        </span>
        <Button
          type="button"
          variant="outline"
          size="xs"
          tabIndex={-1}
          className="ml-auto"
          title="Ask the LLM again if this split is not right"
          onMouseDown={(e) => e.preventDefault()}
          onClick={split.rerun}
        >
          <RefreshCw aria-hidden />
          Re-split
        </Button>
      </div>
      {state.status === "error" ? (
        <p role="alert" className="p-4 text-xs text-red-700 dark:text-destructive">
          {state.message}
        </p>
      ) : state.status === "done" ? (
        <SplitParts parts={state.parts} onError={onError} />
      ) : null}
    </div>
  );
}
