import { Info, TriangleAlert, X } from "lucide-react";
import { cn } from "@/lib/utils";

export interface NoticeState {
  kind: "error" | "info";
  message: string;
}

interface NoticeProps extends NoticeState {
  onDismiss: () => void;
  /** A message that matters and is long (the history was moved aside): wrap it over up to three lines. */
  wrap?: boolean;
}

/**
 * A non-blocking message in the footer, for example when copying fails.
 * It never takes focus and goes away on its own (the caller owns the timer).
 */
export function Notice({ kind, message, onDismiss, wrap = false }: NoticeProps) {
  const Icon = kind === "error" ? TriangleAlert : Info;
  return (
    <div
      role={kind === "error" ? "alert" : "status"}
      className={cn(
        "flex w-full min-w-0 gap-2 text-xs",
        wrap ? "items-start py-0.5 leading-snug" : "items-center",
        kind === "error" ? "text-destructive" : "text-muted-foreground",
      )}
    >
      <Icon className={cn("size-3.5 shrink-0", wrap && "mt-px")} aria-hidden />
      <span className={cn("min-w-0 flex-1", wrap ? "line-clamp-3" : "truncate")} title={message}>
        {message}
      </span>
      <button
        type="button"
        aria-label="Dismiss"
        tabIndex={-1}
        // Do not take focus from the search input.
        onMouseDown={(e) => e.preventDefault()}
        onClick={onDismiss}
        // A 24 px hit area (WCAG 2.5.8) around the 12 px icon, without making the footer taller.
        className="-my-1 -mr-1 flex size-6 shrink-0 items-center justify-center rounded-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        <X className="size-3" aria-hidden />
      </button>
    </div>
  );
}
