import { memo } from "react";
import { NotebookPen } from "lucide-react";
import { Kbd } from "@/components/ui/kbd";

interface NotesButtonProps {
  /** Notes kept so far, 0 for none (then no number shows). */
  count: number;
  onClick: () => void;
}

/** The way into the notes, next to the Tasks button at the right of the filter row. */
export const NotesButton = memo(function NotesButton({ count, onClick }: NotesButtonProps) {
  return (
    <button
      type="button"
      tabIndex={-1}
      title="Notes (Ctrl+N)"
      // Keep the caret in the search input.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className="flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground transition-colors hover:bg-foreground/[0.06] hover:text-foreground"
    >
      <NotebookPen className="size-3.5 shrink-0" aria-hidden />
      Notes
      {count > 0 && <span className="tabular-nums">{count}</span>}
      <Kbd className="h-4 min-w-4 shrink-0 px-1 text-[10px]">Ctrl N</Kbd>
    </button>
  );
});
