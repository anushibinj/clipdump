import { memo } from "react";
import { Sparkles } from "lucide-react";

interface AskButtonProps {
  onClick: () => void;
}

/** The way into "Ask", left of the Notes button. Icon only: the filter row is already full. */
export const AskButton = memo(function AskButton({ onClick }: AskButtonProps) {
  return (
    <button
      type="button"
      tabIndex={-1}
      title="Ask your notes, tasks and clipboard (Ctrl+/)"
      aria-label="Ask (Ctrl+/)"
      // Keep the caret in the search input.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className="flex h-7 shrink-0 items-center rounded-md px-2 text-muted-foreground transition-colors hover:bg-foreground/[0.06] hover:text-foreground"
    >
      <Sparkles className="size-3.5 shrink-0" aria-hidden />
    </button>
  );
});
