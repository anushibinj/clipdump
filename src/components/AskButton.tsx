import { memo } from "react";
import { Sparkles } from "lucide-react";

interface AskButtonProps {
  /** Ask is the view showing now: the button then leads back to the clipboard search. */
  active: boolean;
  onClick: () => void;
}

/** The only way into "Ask": bottom left of the footer, which every view shows. */
export const AskButton = memo(function AskButton({ active, onClick }: AskButtonProps) {
  return (
    <button
      type="button"
      tabIndex={-1}
      title={active ? "Back to the clipboard search" : "Ask your notes, tasks and clipboard"}
      aria-label="Ask"
      aria-pressed={active}
      // Keep the caret in the search input.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className={`flex min-h-6 shrink-0 items-center gap-1.5 rounded-sm px-1 text-xs transition-colors hover:text-foreground ${
        active ? "text-foreground" : "text-muted-foreground"
      }`}
    >
      <Sparkles className="size-3.5 shrink-0" aria-hidden />
      Ask
    </button>
  );
});
