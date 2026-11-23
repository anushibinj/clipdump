import { useEffect, useRef, useState } from "react";
import { Copy, Loader2, Paperclip, Sparkles, Unlink, X } from "lucide-react";
import { SplitTaskPanel } from "@/components/SplitTaskPanel";
import { Button } from "@/components/ui/button";
import { copyItem, draftResumeNote, errorMessage, getClips, type ClipRef, type Task } from "@/lib/ipc";
import { createLatest } from "@/lib/latest";
import type { Async } from "@/lib/taskAi";
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
  /** Bumped each time a draft is asked for (Ctrl+R in the list): the sidebar then asks the assistant. */
  draftRequest: number;
  /** Remove the link between the task and a clipboard item. */
  onUnlink: (taskId: number, clipId: number) => void;
  /** A copy or a draft failed: say so in the bar's notice. */
  onFail: (e: unknown) => void;
  /** Tasks were created from the notes: reload the list. */
  onSplitCreated: () => void;
  /** Inside the focus lock: fills its slot instead of being a sidebar, and cannot be closed. */
  embedded?: boolean;
}

/**
 * The notes of one task, in a sidebar beside the list. Typing saves itself shortly after you
 * pause, and at once when you pick another task or leave, so nothing needs a Save button.
 */
export function TaskNotes({
  task,
  now,
  onSave,
  onClose,
  onReturnFocus,
  textareaRef,
  draftRequest,
  onUnlink,
  onFail,
  onSplitCreated,
  embedded = false,
}: TaskNotesProps) {
  const [text, setText] = useState(task.note);
  // The assistant's draft of "where I left off": shown for the person to take or leave, never saved by itself.
  const [draft, setDraft] = useState<Async<string> | null>(null);
  const draftTicket = useRef(createLatest());
  const [clips, setClips] = useState<ClipRef[]>([]);
  const clipKey = task.clipIds.join(",");
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

  const askDraft = () => {
    const ticket = draftTicket.current.next();
    setDraft({ status: "loading" });
    draftResumeNote(task.id).then(
      (value) => draftTicket.current.isCurrent(ticket) && setDraft({ status: "done", value }),
      (e: unknown) =>
        draftTicket.current.isCurrent(ticket) && setDraft({ status: "error", message: errorMessage(e) }),
    );
  };
  const askDraftRef = useRef(askDraft);
  askDraftRef.current = askDraft;

  // Another task: its draft is not this one's.
  useEffect(() => {
    draftTicket.current.next();
    setDraft(null);
  }, [task.id]);

  // Ctrl+R in the list asks for a draft for the selected task.
  useEffect(() => {
    if (draftRequest > 0) askDraftRef.current();
  }, [draftRequest]);

  // The linked clipboard items still in the history.
  useEffect(() => {
    if (clipKey === "") {
      setClips([]);
      return;
    }
    let live = true;
    getClips(clipKey.split(",").map(Number)).then(
      (found) => live && setClips(found),
      () => live && setClips([]),
    );
    return () => {
      live = false;
    };
  }, [clipKey]);

  const change = (value: string) => {
    setText(value);
    pending.current = { id: task.id, note: value.trim() };
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(flush, SAVE_DELAY_MS);
  };

  return (
    <aside
      aria-label={`Notes for ${task.title}`}
      className={
        embedded
          ? "flex h-52 min-h-0 min-w-0 flex-1 flex-col overflow-y-auto rounded-md border bg-background/60"
          : "flex h-full min-h-0 w-[16.5rem] shrink-0 flex-col border-l"
      }
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
        {!embedded && (
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
        )}
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
      <div className="flex items-center gap-2 px-3 pb-2 text-[11px] text-muted-foreground">
        <Button
          type="button"
          variant="ghost"
          size="xs"
          tabIndex={-1}
          disabled={draft?.status === "loading"}
          title="Draft where you left off from what you copied since (Ctrl+R)"
          onMouseDown={(e) => e.preventDefault()}
          onClick={askDraft}
        >
          {draft?.status === "loading" ? <Loader2 className="animate-spin" aria-hidden /> : <Sparkles aria-hidden />}
          Draft
        </Button>
        <span className="ml-auto tabular-nums">
          {text.length}/{MAX_NOTE_CHARS}
        </span>
      </div>
      {draft && draft.status !== "loading" && (
        <div className="mx-2 mb-2 rounded-md border bg-foreground/[0.03] p-2 text-xs" role="status">
          {draft.status === "error" ? (
            <p className="text-destructive">{draft.message}</p>
          ) : (
            <>
              <p className="leading-4">{draft.value}</p>
              <div className="mt-1.5 flex gap-1">
                <Button
                  type="button"
                  size="xs"
                  tabIndex={-1}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => {
                    change(draft.value);
                    setDraft(null);
                  }}
                >
                  Use it
                </Button>
              </div>
            </>
          )}
          <Button
            type="button"
            size="xs"
            variant="ghost"
            tabIndex={-1}
            className="mt-1"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => setDraft(null)}
          >
            Dismiss
          </Button>
        </div>
      )}
      <SplitTaskPanel
        taskId={task.id}
        hasNotes={text.trim().length > 0}
        onCreated={onSplitCreated}
        onFail={onFail}
      />
      {clips.length > 0 && (
        <div className="border-t px-3 py-2" aria-label="Linked clipboard items">
          <p className="mb-1 flex items-center gap-1 text-[11px] font-medium text-muted-foreground">
            <Paperclip className="size-3" aria-hidden />
            Linked clipboard items
          </p>
          <ul className="flex max-h-28 flex-col gap-0.5 overflow-y-auto">
            {clips.map((c) => (
              <li key={c.id} className="group/clip flex items-center gap-1 text-xs">
                <span className="min-w-0 flex-1 truncate" title={c.preview}>
                  {c.kind === "text" ? c.preview : `${c.kind === "image" ? "Image" : "Files"}: ${c.preview}`}
                </span>
                <button
                  type="button"
                  tabIndex={-1}
                  aria-label="Copy it back"
                  title="Copy it back to the clipboard"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => copyItem(c.id).catch(onFail)}
                  className="rounded p-0.5 text-muted-foreground hover:text-foreground"
                >
                  <Copy className="size-3" aria-hidden />
                </button>
                <button
                  type="button"
                  tabIndex={-1}
                  aria-label="Unlink"
                  title="Remove the link"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => onUnlink(task.id, c.id)}
                  className="rounded p-0.5 text-muted-foreground hover:text-foreground"
                >
                  <Unlink className="size-3" aria-hidden />
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </aside>
  );
}
