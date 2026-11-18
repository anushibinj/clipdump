import { Info, TriangleAlert, X } from "lucide-react";
import { cn } from "@/lib/utils";

export interface NoticeState {
  kind: "error" | "info";
  message: string;
}

interface NoticeProps extends NoticeState {
  onDismiss: () => void;
}

/**
 * A non-blocking message in the footer, for example when copying fails.
 * It never takes focus and goes away on its own (the caller owns the timer).
 */
export function Notice({ kind, message, onDismiss }: NoticeProps) {
  const Icon = kind === "error" ? TriangleAlert : Info;
  return (
    <div
      role={kind === "error" ? "alert" : "status"}
      className={cn(
        "flex w-full min-w-0 items-center gap-2 text-xs",
        kind === "error" ? "text-destructive" : "text-muted-foreground",
      )}
    >
      <Icon className="size-3.5 shrink-0" aria-hidden />
      <span className="min-w-0 flex-1 truncate" title={message}>
        {message}
      </span>
      <button
        type="button"
        aria-label="Dismiss"
        tabIndex={-1}
        // Do not take focus from the search input.
        onMouseDown={(e) => e.preventDefault()}
        onClick={onDismiss}
        className="shrink-0 rounded-sm p-0.5 text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        <X className="size-3" aria-hidden />
      </button>
    </div>
  );
}
