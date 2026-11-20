import { memo } from "react";
import { Pin } from "lucide-react";
import type { Note } from "@/lib/ipc";
import { noteLabel } from "@/lib/notes";
import { fullDateTime, relativeTime } from "@/lib/time";
import { cn } from "@/lib/utils";

/** DOM id of a row, for scroll-into-view and `aria-activedescendant`. */
export const noteRowId = (id: number) => `note-row-${id}`;

interface NoteRowProps {
  note: Note;
  selected: boolean;
  /** Minute rounded "now" (lib/time.ts `minuteNow`), so the memo holds within a minute. */
  now: number;
  onSelect: (id: number) => void;
  onTogglePin: (id: number) => void;
}

/** The line under the label: the body's first line, or the second one when it is the label. */
function snippet(note: Note): string {
  const lines = note.body.split("\n").map((l) => l.trim()).filter(Boolean);
  const rest = note.title ? lines : lines.slice(1);
  return rest[0] ?? "";
}

/** One note in the list. Hover does not select: the editor beside the list follows the selection. Memoized: moving the selection repaints two rows, a minute tick none. */
export const NoteRow = memo(function NoteRow({ note, selected, now, onSelect, onTogglePin }: NoteRowProps) {
  const label = noteLabel(note);
  const sub = snippet(note);
  return (
    <div
      id={noteRowId(note.id)}
      role="option"
      aria-selected={selected}
      data-selected={selected}
      data-id={note.id}
      data-pinned={note.pinned}
      className={cn(
        "group/note mx-1 flex cursor-default items-start gap-2 rounded-lg px-2.5 py-2 text-sm transition-colors",
        "scroll-my-1 data-[selected=true]:bg-foreground/[0.07] dark:data-[selected=true]:bg-foreground/[0.1]",
      )}
      onClick={() => onSelect(note.id)}
      // Keep keyboard focus where it is (the filter box).
      onMouseDown={(e) => e.preventDefault()}
    >
      <div className="min-w-0 flex-1">
        <div
          className={cn("truncate leading-5", label === "" && "text-muted-foreground italic", note.title && "font-medium")}
          title={label || undefined}
        >
          {label || "Empty note"}
        </div>
        {sub !== "" && <div className="truncate text-xs leading-4 text-muted-foreground">{sub}</div>}
        {note.tags.length > 0 && (
          <div className="mt-1 flex flex-wrap gap-1">
            {note.tags.map((t) => (
              <span key={t} className="rounded-full bg-foreground/[0.06] px-1.5 text-[11px] leading-4 text-muted-foreground">
                #{t}
              </span>
            ))}
          </div>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-1.5 pt-px">
        <span
          className="text-xs text-muted-foreground tabular-nums"
          title={`Changed ${fullDateTime(note.updatedAt)}\nCreated ${fullDateTime(note.createdAt)}`}
        >
          {relativeTime(note.updatedAt, now)}
        </span>
        <button
          type="button"
          tabIndex={-1}
          aria-label={note.pinned ? "Unpin note" : "Pin note"}
          aria-pressed={note.pinned}
          title={note.pinned ? "Pinned (Ctrl+P)" : "Pin (Ctrl+P)"}
          onClick={(e) => {
            e.stopPropagation();
            onTogglePin(note.id);
          }}
          className={cn(
            "rounded p-0.5 transition-opacity",
            note.pinned
              ? "text-foreground/70"
              : "text-muted-foreground opacity-0 group-hover/note:opacity-100 group-data-[selected=true]/note:opacity-100",
          )}
        >
          <Pin className={cn("size-3.5", note.pinned && "fill-current")} aria-hidden />
        </button>
      </div>
    </div>
  );
});
