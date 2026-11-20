import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { Task } from "@/lib/ipc";
import { MAX_NOTE_CHARS } from "@/lib/tasks";
import { fullDateTime, relativeTime } from "@/lib/time";

const SAVE_DELAY_MS = 500;

interface TaskNotesProps {
  task: Task;
  now: number;
  /** Save the note. Resolves when stored; the caller reports failures. */
  onSave: (id: number, note: string) => void;
  onClose: () => void;
  /** Give focus back to the bar's input (Esc inside the notes). */
  onReturnFocus: () => void;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
}

/**
 * The notes of one task, in a sidebar beside the list. Typing saves itself shortly after you
 * pause, and at once when you pick another task or leave, so nothing needs a Save button.
 */
export function TaskNotes({ task, now, onSave, onClose, onReturnFocus, textareaRef }: TaskNotesProps) {
  const [text, setText] = useState(task.note);
  // The latest unsaved text and where it belongs, flushed on task change and unmount.
  const pending = useRef<{ id: number; note: string } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveRef = useRef(onSave);
  saveRef.current = onSave;

  const flush = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const p = pending.current;
    pending.current = null;
    if (p) saveRef.current(p.id, p.note);
  };

  // A different task: show its note (the previous one was flushed by the cleanup below).
  useEffect(() => {
    setText(task.note);
    return flush;
    // Only the task identity matters: its note changing because we just saved must not reset
    // the text under the caret.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task.id]);

  const change = (value: string) => {
    setText(value);
    pending.current = { id: task.id, note: value.trim() };
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(flush, SAVE_DELAY_MS);
  };

  return (
    <aside
      aria-label={`Notes for ${task.title}`}
      className="flex h-full min-h-0 w-[16.5rem] shrink-0 flex-col border-l"
      onMouseDown={(e) => e.stopPropagation()}
    >
      <div className="flex h-10 shrink-0 items-center gap-2 border-b px-3 text-xs">
        <span className="font-medium">Notes</span>
        <span
          className="ml-auto text-muted-foreground tabular-nums"
          title={`Created ${fullDateTime(task.createdAt)}`}
        >
          Created {relativeTime(task.createdAt, now)}
        </span>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          tabIndex={-1}
          aria-label="Close notes"
          title="Close notes (Ctrl+E)"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            flush();
            onClose();
          }}
          className="text-muted-foreground"
        >
          <X aria-hidden />
        </Button>
      </div>
      <div className="truncate px-3 pt-2 text-xs font-medium" title={task.title}>
        {task.title}
      </div>
      <textarea
        ref={textareaRef}
        value={text}
        onChange={(e) => change(e.target.value)}
        onBlur={flush}
        onKeyDown={(e) => {
          if (e.key === "Escape" || (e.ctrlKey && e.key.toLowerCase() === "e")) {
            e.preventDefault();
            e.stopPropagation();
            flush();
            onReturnFocus();
          }
        }}
        maxLength={MAX_NOTE_CHARS}
        placeholder="Notes for this task. Where did you leave off?"
        aria-label="Task notes"
        spellCheck={false}
        className="m-2 min-h-0 flex-1 resize-none rounded-md bg-transparent p-1.5 text-sm outline-none placeholder:text-muted-foreground focus-visible:bg-foreground/[0.04]"
      />
      <div className="px-3 pb-2 text-right text-[11px] text-muted-foreground tabular-nums">
        {text.length}/{MAX_NOTE_CHARS}
      </div>
    </aside>
  );
}
